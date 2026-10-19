//! The viewer: a keyboard with falling notes and two animated 3D hands, live with sound or
//! exported as a video. The timeline and hand motion work without the `render` feature;
//! the Bevy app needs it.

#[cfg(feature = "render")]
pub mod export;
#[cfg(feature = "render")]
pub mod glove;
#[cfg(feature = "render")]
mod hands;
pub mod layout;
pub mod motion;
#[cfg(feature = "render")]
pub mod overlay;
#[cfg(feature = "render")]
pub mod render;
pub mod rig;
#[cfg(feature = "render")]
pub mod session;
#[cfg(feature = "render")]
pub mod skin;
pub mod timeline;
#[cfg(feature = "render")]
pub mod ui;

#[cfg(feature = "render")]
pub use export::{export, VideoOptions};
pub use layout::Layout;
#[cfg(feature = "render")]
pub use render::{capture, run, Audio, Capture, NoteColours, Performance, Transport, LEAD_IN};

#[cfg(feature = "render")]
pub use bevy::color::Color as NoteColour;
pub use rig::{BoneRole, RigBone};
#[cfg(feature = "render")]
pub use session::SessionSettings;
pub use timeline::{HandAnimator, KeyStates, Timeline, TimelineNote};
#[cfg(feature = "render")]
pub use ui::Session;

/// The viewer's keyboard shortcuts, as its help window lists them.
pub const SHORTCUTS: &[(&str, &str)] = &[
    ("Space", "play / pause"),
    ("← →", "back / forward two seconds"),
    ("Home", "back to the start"),
    ("↑ ↓", "faster / slower"),
    ("L", "loop the piece on / off"),
    (", .", "pause and step back / on a note"),
    ("[ ]", "practise from here / up to here"),
    ("\\", "play the whole piece again"),
    ("F1  ?", "this list"),
];
