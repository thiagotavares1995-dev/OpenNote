//! `opennote render`: the performance as a video file, with sound, through ffmpeg.

use std::path::PathBuf;

use anyhow::Result;
use clap::Args;

use crate::scene::{self, SceneArgs};
use crate::ModelArgs;

/// Render a score to an MP4 with sound.
#[derive(Debug, Args)]
pub struct RenderArgs {
    /// The score to read.
    pub input: PathBuf,

    /// Where to write the video.
    #[arg(long, short, default_value = "opennote.mp4")]
    pub out: PathBuf,

    /// Frame width in pixels.
    #[arg(long, default_value_t = 1920, value_parser = even_pixels)]
    pub width: u32,

    /// Frame height in pixels.
    #[arg(long, default_value_t = 1080, value_parser = even_pixels)]
    pub height: u32,

    /// Frames per second.
    #[arg(long, default_value_t = 60, value_parser = clap::value_parser!(u32).range(1..=240))]
    pub fps: u32,

    /// Playback rate, where 1.0 is the score's own tempo. Useful for a slow practice
    /// video of a fast piece.
    #[arg(long, default_value_t = 1.0, value_parser = crate::between(0.05, 4.0))]
    pub speed: f64,

    /// Quality, as x264's constant rate factor: 0 is lossless, 18 is very good, 28 is
    /// small. Lower means a bigger file.
    #[arg(long, default_value_t = 18, value_parser = clap::value_parser!(u8).range(0..=51))]
    pub crf: u8,

    /// The ffmpeg video encoder: libx264 on the CPU, or h264_nvenc / hevc_nvenc on an
    /// NVIDIA GPU (where --crf is used as the constant quality level).
    #[arg(long, default_value = "libx264")]
    pub codec: String,

    /// An ffmpeg -vf filter applied while encoding, such as burnt-in subtitles.
    #[arg(long)]
    pub video_filter: Option<String>,

    /// An information overlay drawn by the GPU: a JSON file naming a fixed image, fonts
    /// and timed lines of text (see on-viz's overlay module).
    #[arg(long)]
    pub overlay: Option<PathBuf>,

    /// The ffmpeg executable, if it is not on PATH.
    #[arg(long, default_value = "ffmpeg")]
    pub ffmpeg: PathBuf,

    #[command(flatten)]
    pub scene: SceneArgs,

    #[command(flatten)]
    pub model: ModelArgs,
}

pub fn run(args: RenderArgs) -> Result<()> {
    let settings = args.scene.settings(&args.model)?;
    let performance = scene::build(&args.input, &settings)?;
    on_viz::export(
        performance,
        on_viz::VideoOptions {
            path: args.out,
            width: args.width,
            height: args.height,
            fps: args.fps,
            speed: args.speed,
            ffmpeg: args.ffmpeg,
            crf: args.crf,
            codec: args.codec,
            video_filter: args.video_filter,
            overlay: args.overlay,
            note_colours: settings.note_colours,
        },
    )
}

/// The encoder stores colour at half resolution, so a frame must be an even number of
/// pixels each way.
fn even_pixels(text: &str) -> std::result::Result<u32, String> {
    let pixels = crate::between(16u32, 8192)(text)?;
    if pixels % 2 == 0 {
        Ok(pixels)
    } else {
        Err(format!("{pixels} is odd; the video encoder needs an even number of pixels"))
    }
}
