//! An information overlay drawn by the GPU over an exported video: one fixed image (a
//! header panel, key names) and timed lines of text, as described by a JSON file.
//!
//! ```json
//! { "image": "fixed.png", "fonts": ["C:/Windows/Fonts/arial.ttf"],
//!   "lines": [ { "x": 30, "y": 100, "right": false, "size": 20, "font": 0,
//!                "colour": [1, 1, 1, 1],
//!                "events": [ { "start": 0.0, "end": 1.5, "text": "Tempo: 120" } ] } ] }
//! ```
//!
//! Positions are in output pixels from the top left (`right: true` measures `x` from the
//! right edge instead, for right-aligned text), times are in seconds of video, colours are
//! sRGB with straight alpha, and each line's events are in order and do not overlap.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use bevy::prelude::*;
use serde::Deserialize;

/// Above the scene and its finger labels.
const OVERLAY_Z: i32 = 1_000_000;

#[derive(Debug, Clone, Deserialize)]
pub struct OverlaySpec {
    #[serde(default)]
    pub image: Option<PathBuf>,
    #[serde(default)]
    pub fonts: Vec<PathBuf>,
    #[serde(default)]
    pub lines: Vec<OverlayLine>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OverlayLine {
    pub x: f32,
    pub y: f32,
    #[serde(default)]
    pub right: bool,
    pub size: f32,
    #[serde(default)]
    pub font: usize,
    pub colour: [f32; 4],
    pub events: Vec<OverlayEvent>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct OverlayEvent {
    pub start: f64,
    pub end: f64,
    pub text: String,
    #[serde(default)]
    pub colour: Option<[f32; 4]>,
}

/// The overlay with its files read, ready to hand to the app.
pub struct Overlay {
    spec: OverlaySpec,
    image: Option<image::RgbaImage>,
    fonts: Vec<Vec<u8>>,
}

impl Overlay {
    pub fn load(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
        let mut spec: OverlaySpec =
            serde_json::from_str(&text).with_context(|| format!("parsing {}", path.display()))?;
        let base = path.parent().unwrap_or(Path::new("."));
        let resolve = |p: &PathBuf| if p.is_absolute() { p.clone() } else { base.join(p) };
        spec.image = spec.image.as_ref().map(resolve);
        spec.fonts = spec.fonts.iter().map(resolve).collect();
        let image = match &spec.image {
            Some(file) => Some(
                image::open(file).with_context(|| format!("reading overlay image {}", file.display()))?.to_rgba8(),
            ),
            None => None,
        };
        let fonts = spec
            .fonts
            .iter()
            .map(|file| std::fs::read(file).with_context(|| format!("reading font {}", file.display())))
            .collect::<Result<Vec<_>>>()?;
        for (index, line) in spec.lines.iter().enumerate() {
            if line.font >= fonts.len().max(1) {
                anyhow::bail!("overlay line {index} asks for font {} of {}", line.font, fonts.len());
            }
        }
        Ok(Self { spec, image, fonts })
    }

    pub(crate) fn install(self, app: &mut App) {
        app.insert_resource(OverlayClock(0.0))
            .insert_resource(OverlayData { spec: self.spec, image: self.image, fonts: self.fonts })
            .add_systems(Startup, spawn_overlay)
            .add_systems(Update, show_overlay_text);
    }
}

/// Seconds of video for the frame being rendered; the export sets it each update.
#[derive(Resource)]
pub(crate) struct OverlayClock(pub f64);

#[derive(Resource)]
struct OverlayData {
    spec: OverlaySpec,
    image: Option<image::RgbaImage>,
    fonts: Vec<Vec<u8>>,
}

#[derive(Component)]
struct OverlayText {
    line: usize,
    cursor: usize,
    shown: Option<usize>,
}

fn srgba([r, g, b, a]: [f32; 4]) -> Color {
    Color::srgba(r, g, b, a)
}

fn spawn_overlay(
    mut commands: Commands,
    mut data: ResMut<OverlayData>,
    mut images: ResMut<Assets<Image>>,
    mut fonts: ResMut<Assets<Font>>,
) {
    use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

    if let Some(rgba) = data.image.take() {
        let (width, height) = rgba.dimensions();
        let image = Image::new(
            Extent3d { width, height, depth_or_array_layers: 1 },
            TextureDimension::D2,
            rgba.into_raw(),
            TextureFormat::Rgba8UnormSrgb,
            bevy::asset::RenderAssetUsages::RENDER_WORLD,
        );
        commands.spawn((
            ImageNode::new(images.add(image)),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            },
            GlobalZIndex(OVERLAY_Z),
        ));
    }
    let handles: Vec<Handle<Font>> =
        std::mem::take(&mut data.fonts).into_iter().map(|bytes| fonts.add(Font::from_bytes(bytes))).collect();
    for (index, line) in data.spec.lines.iter().enumerate() {
        let mut font = TextFont { font_size: bevy::text::FontSize::Px(line.size), ..default() };
        if let Some(handle) = handles.get(line.font) {
            font.font = bevy::text::FontSource::Handle(handle.clone());
        }
        let mut node = Node { position_type: PositionType::Absolute, top: Val::Px(line.y), ..default() };
        if line.right {
            node.right = Val::Px(line.x);
        } else {
            node.left = Val::Px(line.x);
        }
        commands.spawn((
            Text::new(""),
            font,
            TextColor(srgba(line.colour)),
            TextLayout::new(Justify::Left, LineBreak::NoWrap),
            node,
            GlobalZIndex(OVERLAY_Z + 1),
            OverlayText { line: index, cursor: 0, shown: None },
        ));
    }
}

fn show_overlay_text(
    clock: Res<OverlayClock>,
    data: Res<OverlayData>,
    texts: Query<(&mut OverlayText, &mut Text, &mut TextColor)>,
) {
    let now = clock.0;
    for (mut state, mut text, mut colour) in texts {
        let line = &data.spec.lines[state.line];
        // The export only moves forward, so each line keeps its place in its events.
        while state.cursor < line.events.len() && line.events[state.cursor].end <= now {
            state.cursor += 1;
        }
        let current = line.events.get(state.cursor).filter(|event| event.start <= now);
        let index = current.map(|_| state.cursor);
        if index == state.shown {
            continue;
        }
        state.shown = index;
        match current {
            Some(event) => {
                text.0.clone_from(&event.text);
                colour.0 = srgba(event.colour.unwrap_or(line.colour));
            }
            None => text.0.clear(),
        }
    }
}
