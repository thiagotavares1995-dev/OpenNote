//! Renders the performance offscreen, frame by frame, and pipes it to ffmpeg with the
//! soundtrack.

use std::collections::BTreeMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use bevy::prelude::*;
use bevy::render::view::screenshot::{Screenshot, ScreenshotCaptured};

use crate::render::{Audio, Performance, Transport, VisualizerPlugin, LEAD_IN};

const TAIL_SECONDS: f64 = 2.0;

/// The most app updates spent warming up before the first frame, whatever is still loading.
const WARMUP_LIMIT: u32 = 2400;

/// Updates in a row with the hands in place and no render pipeline still compiling before the
/// scene counts as ready.
const READY_STREAK: u32 = 8;

/// Updates after the scene is ready, still at the first moment, so that the temporal
/// anti-aliasing history and everything else that settles over frames has settled. The history
/// of a still scene converges slowly; with the scene ready after the usual fifty-odd updates,
/// this gives the first frame the same settled history as the fixed warm-up of 240 did.
const SETTLE_UPDATES: u32 = 192;

/// The first frame is captured on an update whose frame count is a multiple of this, the
/// length of the anti-aliasing jitter sequence, as it was after the fixed warm-up of 240.
const JITTER_PERIOD: u32 = 8;

#[derive(Debug, Clone)]
pub struct VideoOptions {
    pub path: PathBuf,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub speed: f64,
    pub ffmpeg: PathBuf,
    pub crf: u8,
    /// The ffmpeg video encoder: libx264 (CPU), or h264_nvenc/hevc_nvenc (NVIDIA GPU).
    pub codec: String,
    /// An optional ffmpeg -vf filter applied while encoding (e.g. burnt-in subtitles).
    pub video_filter: Option<String>,
    /// An information overlay (fixed image and timed text) drawn by the GPU; see `overlay`.
    pub overlay: Option<PathBuf>,
    pub note_colours: [bevy::color::Color; 2],
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self {
            path: PathBuf::from("opennote.mp4"),
            width: 1920,
            height: 1080,
            fps: 60,
            speed: 1.0,
            ffmpeg: PathBuf::from("ffmpeg"),
            crf: 18,
            codec: "libx264".into(),
            video_filter: None,
            overlay: None,
            note_colours: crate::render::NoteColours::default().0,
        }
    }
}

#[derive(Resource)]
struct Export {
    options: VideoOptions,
    target: Handle<Image>,
    /// The next frame to ask for.
    frame: u32,
    /// The next frame to hand to the encoder; frames captured early wait in `ready`.
    sent: u32,
    total: u32,
    /// Updates spent warming up so far, and those in a row with the scene ready.
    warmup: u32,
    ready_streak: u32,
    /// The update from which the first frame may be captured, once the scene is ready.
    capture_from: Option<u32>,
    in_flight: u32,
    ready: BTreeMap<u32, Vec<u8>>,
    frames: SyncSender<Vec<u8>>,
    trouble: Arc<AtomicBool>,
    start: f64,
}

impl Export {
    fn moment(&self, frame: u32) -> f64 {
        self.start + f64::from(frame) / f64::from(self.options.fps) * self.options.speed
    }
}

pub fn export(performance: Performance, options: VideoOptions) -> Result<()> {
    #[cfg(feature = "perfil")]
    {
        crate::perfil::marca("cena_pronta");
        crate::perfil::contar("notas", performance.timeline.notes.len() as u64);
    }
    let ffmpeg = check_ffmpeg(&options.ffmpeg)?;

    let audio_path = options.path.with_extension("opennote.wav");
    let _soundtrack = Scratch(audio_path.clone());
    let last = performance.timeline.duration;
    let seconds = (last - LEAD_IN + TAIL_SECONDS) / options.speed.max(1e-6);
    let total = (seconds * f64::from(options.fps)).ceil() as u32;
    #[cfg(feature = "perfil")]
    let total = crate::perfil::parar_em().map_or(total, |limite| total.min(limite));

    // The soundtrack is rendered while the app starts up and warms up; ffmpeg, which reads it,
    // is started once it is written, and until then the frames wait for ffmpeg as they would
    // for a slow encoder.
    let soundtrack = {
        let path = audio_path.clone();
        let notes = performance.timeline.audio_notes();
        let soundfont = performance.soundfont.clone();
        let speed = options.speed;
        std::thread::Builder::new()
            .name("opennote-soundtrack".into())
            .spawn(move || {
                let written =
                    on_audio::render_to_wav(&path, notes, 48_000, LEAD_IN, speed, TAIL_SECONDS, soundfont.as_deref())
                        .context("rendering the soundtrack");
                #[cfg(feature = "perfil")]
                crate::perfil::marca("wav_pronto");
                written
            })
            .context("starting the soundtrack thread")?
    };

    // Bounded, so that rendering waits for the encoder rather than holding every frame
    // it has not yet taken in memory.
    let (frames, incoming) = mpsc::sync_channel::<Vec<u8>>(FRAMES_IN_FLIGHT);
    let trouble = Arc::new(AtomicBool::new(false));
    let report = Arc::clone(&trouble);
    let encoding = (ffmpeg, options.clone(), audio_path.clone());
    let writer = std::thread::Builder::new()
        .name("opennote-encode".into())
        .spawn(move || -> Result<Child> {
            let started = match soundtrack.join() {
                Ok(written) => written.and_then(|_| start_ffmpeg(&encoding.0, &encoding.1, &encoding.2, total)),
                Err(_) => Err(anyhow::anyhow!("the soundtrack thread panicked")),
            };
            let mut encoder = match started {
                Ok(encoder) => encoder,
                Err(error) => {
                    report.store(true, Ordering::Relaxed);
                    return Err(error);
                }
            };
            let Some(stdin) = encoder.stdin.take() else {
                report.store(true, Ordering::Relaxed);
                bail!("ffmpeg did not give us anywhere to write frames");
            };
            let mut stdin = std::io::BufWriter::with_capacity(1 << 20, stdin);
            #[cfg(feature = "perfil")]
            let mut hashes = crate::perfil::arquivo_hashes().map(|caminho| (caminho, String::new()));
            for frame in incoming {
                #[cfg(feature = "perfil")]
                if let Some((_, texto)) = hashes.as_mut() {
                    texto.push_str(&format!("{:016x}\n", crate::perfil::hash_quadro(&frame)));
                }
                if let Err(error) = stdin.write_all(&frame) {
                    tracing::error!("ffmpeg stopped taking frames: {error}");
                    report.store(true, Ordering::Relaxed);
                    return Ok(encoder);
                }
            }
            if let Err(error) = stdin.flush() {
                tracing::error!("could not finish writing to ffmpeg: {error}");
                report.store(true, Ordering::Relaxed);
            }
            #[cfg(feature = "perfil")]
            if let Some((caminho, texto)) = hashes {
                let _ = std::fs::write(caminho, texto);
            }
            Ok(encoder)
        })
        .context("starting the encoder thread")?;

    println!("Rendering {} frames at {}x{}, {} fps.", total, options.width, options.height, options.fps);

    let overlay = options.overlay.as_deref().map(crate::overlay::Overlay::load).transpose()?;
    let assets = performance.assets_root.clone();
    let width = options.width;
    let height = options.height;
    let mut app = App::new();
    app.add_plugins(headless_plugins(&assets))
        .insert_resource(crate::render::NoteColours(options.note_colours))
        .insert_resource(performance)
        .init_resource::<Audio>()
        .add_plugins(VisualizerPlugin)
        .add_systems(
            PostStartup,
            (
                move |mut commands: Commands, mut images: ResMut<Assets<Image>>| {
                    let target = images.add(offscreen_target(width, height));
                    commands.insert_resource(ExportTarget(target));
                },
                aim_camera_at_target,
                build_export_resource,
            )
                .chain(),
        );

    app.world_mut().insert_resource(PendingExport {
        options: options.clone(),
        total,
        frames,
        trouble,
        start: LEAD_IN,
    });
    app.add_systems(First, drive_export);
    install_pipeline_watch(&mut app);
    if let Some(overlay) = overlay {
        overlay.install(&mut app);
    }
    #[cfg(feature = "perfil")]
    {
        crate::perfil::agenda::instalar(&mut app);
        crate::perfil::marca("app_inicio");
    }
    app.run();

    #[cfg(feature = "perfil")]
    crate::perfil::marca("app_fim");
    drop(app);
    let mut encoder = match writer.join() {
        Ok(encoder) => encoder?,
        Err(_) => bail!("the encoder thread panicked"),
    };
    let status = encoder.wait().context("waiting for ffmpeg")?;
    #[cfg(feature = "perfil")]
    {
        crate::perfil::marca("encoder_fim");
        crate::perfil::gravar();
    }
    if !status.success() {
        bail!("ffmpeg failed: {status}");
    }
    println!("Wrote {}", options.path.display());
    Ok(())
}

/// The export draws only into an offscreen image, so it runs without a window: a window that
/// never has focus is held to 60 updates a second, and its present pacing would cap the
/// export at one frame per update tick.
fn headless_plugins(assets: &Path) -> impl PluginGroup {
    let mut plugins = DefaultPlugins
        .build()
        .disable::<bevy::log::LogPlugin>()
        .disable::<bevy::winit::WinitPlugin>()
        .set(WindowPlugin {
            primary_window: None,
            exit_condition: bevy::window::ExitCondition::DontExit,
            ..default()
        })
        .add(bevy::app::ScheduleRunnerPlugin::run_loop(std::time::Duration::ZERO));
    if let Some(path) = assets.to_str() {
        plugins = plugins.set(AssetPlugin { file_path: path.to_string(), ..default() });
    }
    plugins
}

#[derive(Resource, Deref)]
struct ExportTarget(Handle<Image>);

const FRAMES_IN_FLIGHT: usize = 8;

const SCREENSHOTS_IN_FLIGHT: u32 = 8;

/// A temporary file, removed however the export ends.
struct Scratch(std::path::PathBuf);

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

#[derive(Resource)]
struct PendingExport {
    options: VideoOptions,
    total: u32,
    frames: SyncSender<Vec<u8>>,
    trouble: Arc<AtomicBool>,
    start: f64,
}

fn build_export_resource(mut commands: Commands) {
    commands.queue(|world: &mut World| {
        let Some(target) = world.remove_resource::<ExportTarget>() else {
            return;
        };
        let Some(pending) = world.remove_resource::<PendingExport>() else {
            return;
        };
        world.insert_resource(Export {
            options: pending.options,
            target: target.0,
            frame: 0,
            sent: 0,
            total: pending.total,
            warmup: 0,
            ready_streak: 0,
            capture_from: None,
            in_flight: 0,
            ready: BTreeMap::new(),
            frames: pending.frames,
            trouble: pending.trouble,
            start: pending.start,
        });
    });
}

fn offscreen_target(width: u32, height: u32) -> Image {
    use bevy::image::Image;
    use bevy::render::render_resource::{TextureDimension, TextureFormat, TextureUsages};

    let mut image = Image::new_fill(
        bevy::render::render_resource::Extent3d { width, height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &[0, 0, 0, 255],
        TextureFormat::Rgba8UnormSrgb,
        bevy::asset::RenderAssetUsages::default(),
    );
    image.texture_descriptor.usage = TextureUsages::COPY_SRC
        | TextureUsages::COPY_DST
        | TextureUsages::TEXTURE_BINDING
        | TextureUsages::RENDER_ATTACHMENT;
    image
}

fn aim_camera_at_target(
    mut commands: Commands,
    target: Res<ExportTarget>,
    cameras: Query<(Entity, &mut bevy::camera::RenderTarget), With<Camera>>,
) {
    for (entity, mut render_target) in cameras {
        *render_target = bevy::camera::RenderTarget::Image((*target).clone().into());
        commands.entity(entity).insert(bevy::ui::IsDefaultUiCamera);
    }
}

/// Whether the render world, after its last update, had no pipeline waiting to be compiled.
#[derive(Resource, Clone, Default)]
struct PipelinesSettled(Arc<AtomicBool>);

fn install_pipeline_watch(app: &mut App) {
    use bevy::render::render_resource::PipelineCache;
    use bevy::render::{Render, RenderApp, RenderSystems};

    let settled = PipelinesSettled::default();
    app.insert_resource(settled.clone());
    if let Some(render) = app.get_sub_app_mut(RenderApp) {
        render.insert_resource(settled).add_systems(
            Render,
            (|cache: Res<PipelineCache>, settled: Res<PipelinesSettled>| {
                settled.0.store(cache.waiting_pipelines().next().is_none(), Ordering::Relaxed);
            })
            .in_set(RenderSystems::Cleanup),
        );
    }
}

/// Holds the first moment until the scene is drawn in full: the hands loaded and skinned and no
/// pipeline still compiling, then a fixed number of updates more, ending in step with the
/// anti-aliasing jitter. Returns whether to keep warming up.
fn warming_up(export: &mut Export, hands: bool, pipelines: bool, frame_count: u32) -> bool {
    if let Some(from) = export.capture_from {
        if export.warmup < from {
            export.warmup += 1;
            return true;
        }
        return false;
    }
    export.warmup += 1;
    export.ready_streak = if hands && pipelines { export.ready_streak + 1 } else { 0 };
    let ready = export.ready_streak >= READY_STREAK;
    if ready || export.warmup >= WARMUP_LIMIT {
        if !ready {
            warn!("the scene was still loading after {WARMUP_LIMIT} updates; exporting anyway");
        }
        // `warmup` counts the updates so far, `frame_count` the ones before this, so the first
        // frame lands on an update whose frame count is a multiple of the jitter period.
        let after = export.warmup + SETTLE_UPDATES;
        let count_then = frame_count + SETTLE_UPDATES + 1;
        let align = (JITTER_PERIOD - count_then % JITTER_PERIOD) % JITTER_PERIOD;
        export.capture_from = Some(after + align);
        #[cfg(feature = "perfil")]
        crate::perfil::contar("aquecimento_ate_pronto", u64::from(export.warmup));
    }
    true
}

fn drive_export(
    mut commands: Commands,
    export: Option<ResMut<Export>>,
    mut transport: ResMut<Transport>,
    clock: Option<ResMut<crate::overlay::OverlayClock>>,
    hands: crate::hands::HandsSettled,
    pipelines: Res<PipelinesSettled>,
    frame_count: Res<bevy::diagnostic::FrameCount>,
    mut exit: MessageWriter<AppExit>,
) {
    medir!("drive_export");
    let Some(mut export) = export else {
        return;
    };
    if export.trouble.load(Ordering::Relaxed) {
        exit.write(AppExit::error());
        return;
    }
    #[cfg(feature = "perfil")]
    {
        crate::perfil::marca("aquecimento_inicio");
        crate::perfil::contar("updates", 1);
    }
    if export.frame == 0
        && warming_up(&mut export, hands.ready(), pipelines.0.load(Ordering::Relaxed), frame_count.0)
    {
        transport.position = export.start;
        transport.playing = false;
        return;
    }
    if export.sent >= export.total {
        #[cfg(feature = "perfil")]
        crate::perfil::marca("quadros_fim");
        exit.write(AppExit::Success);
        return;
    }
    // Several frames are rendered and read back at once; each screenshot keeps its own index
    // and the encoder still receives them in order.
    if export.in_flight >= SCREENSHOTS_IN_FLIGHT || export.frame >= export.total {
        #[cfg(feature = "perfil")]
        crate::perfil::contar(
            if export.frame >= export.total { "updates_esperando_fim" } else { "updates_capturas_cheias" },
            1,
        );
        return;
    }
    #[cfg(feature = "perfil")]
    {
        crate::perfil::marca("quadros_inicio");
        crate::perfil::contar("quadros_pedidos", 1);
        crate::perfil::pico("capturas_em_voo", u64::from(export.in_flight + 1));
    }

    transport.position = export.moment(export.frame);
    if let Some(mut clock) = clock {
        clock.0 = f64::from(export.frame) / f64::from(export.options.fps);
    }
    transport.playing = false;
    export.in_flight += 1;

    let index = export.frame;
    export.frame += 1;
    let total = export.total;
    let handle = export.target.clone();
    commands.spawn(Screenshot::image(handle)).observe(
        move |captured: On<ScreenshotCaptured>, mut export: ResMut<Export>| {
            // The target is 8-bit sRGB RGBA with no row padding, which ffmpeg reads as `rgba`;
            // handing the bytes over as they are keeps per-pixel work off the main thread.
            let expected = (export.options.width * export.options.height * 4) as usize;
            let pixels = match captured.image.data.as_ref() {
                Some(data) if data.len() == expected => data.clone(),
                _ => {
                    error!("could not read frame {index} back: unexpected image size");
                    export.trouble.store(true, Ordering::Relaxed);
                    Vec::new()
                }
            };
            export.in_flight -= 1;
            export.ready.insert(index, pixels);
            loop {
                let next = export.sent;
                let Some(frame) = export.ready.remove(&next) else { break };
                if export.frames.send(frame).is_err() {
                    error!("the encoder went away partway through the export");
                    export.trouble.store(true, Ordering::Relaxed);
                }
                export.sent += 1;
                if next % 60 == 0 {
                    println!("  frame {next} of {total}");
                }
            }
        },
    );
}

fn check_ffmpeg(path: &Path) -> Result<PathBuf> {
    let found = Command::new(path).arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status();
    match found {
        Ok(status) if status.success() => Ok(path.to_path_buf()),
        _ => bail!(
            "could not run ffmpeg at {}.\n\
             The video export hands its frames to ffmpeg, which is not bundled with \
             this program and is not downloaded by it.\n\
             Install it — `winget install ffmpeg` on Windows, `brew install ffmpeg` on \
             macOS, or your package manager on Linux — or point at a copy you already \
             have with --ffmpeg <path>.",
            path.display()
        ),
    }
}

fn start_ffmpeg(ffmpeg: &Path, options: &VideoOptions, audio: &Path, frames: u32) -> Result<Child> {
    let mut command = Command::new(ffmpeg);
    command
        .arg("-y")
        .args(["-loglevel", "error"])
        .args(["-f", "rawvideo", "-pixel_format", "rgba"])
        .args(["-video_size", &format!("{}x{}", options.width, options.height)])
        .args(["-framerate", &options.fps.to_string()])
        .args(["-i", "-"])
        .arg("-i")
        .arg(audio)
        .args(["-c:v", &options.codec]);
    // NVENC takes the RGBA frames as they are and converts them to 4:2:0 on the GPU; a CPU
    // filter or encoder needs ffmpeg to convert first.
    if !options.codec.ends_with("_nvenc") || options.video_filter.is_some() {
        command.args(["-pix_fmt", "yuv420p"]);
    }
    if options.codec.ends_with("_nvenc") {
        command.args(["-preset", "p5", "-tune", "hq", "-rc", "vbr", "-cq", &options.crf.to_string(), "-b:v", "0"]);
    } else {
        command.args(["-preset", "medium", "-crf", &options.crf.to_string()]);
    }
    if let Some(filter) = &options.video_filter {
        command.args(["-vf", filter]);
    }
    command
        .args(["-c:a", "aac", "-b:a", "192k"])
        // The exact length, rather than -shortest: -shortest makes ffmpeg hold seconds of raw
        // frames in its sync queue (about 2 GB at 1080p), which rules out renders side by side.
        .args(["-t", &format!("{:.6}", f64::from(frames) / f64::from(options.fps))])
        .arg(&options.path)
        .stdin(Stdio::piped())
        .stdout(Stdio::null());
    command.spawn().context("starting ffmpeg")
}
