//! Playback position, speed, looping and the practice section, kept in step with the audio,
//! and the keyboard shortcuts that drive them.

use super::*;

#[derive(Resource, Debug, Clone)]
pub struct Transport {
    pub position: f64,
    pub playing: bool,
    pub speed: f64,
    pub looping: bool,
    /// A stretch to practise, from A to B in seconds: playing past B goes back to A.
    pub section: Option<(f64, f64)>,
    pub(crate) pending_seek: Option<f64>,
}

impl Default for Transport {
    fn default() -> Self {
        Self {
            position: LEAD_IN,
            playing: true,
            speed: 1.0,
            looping: true,
            section: None,
            pending_seek: None,
        }
    }
}

impl Transport {
    pub fn seek_to(&mut self, seconds: f64) {
        self.position = seconds;
        self.pending_seek = Some(seconds);
    }

    /// Marks where the practice section starts. A start after the current end clears the end.
    pub fn mark_start(&mut self, seconds: f64) {
        self.section = match self.section {
            Some((_, end)) if end > seconds => Some((seconds, end)),
            _ => Some((seconds, f64::INFINITY)),
        };
    }

    /// Marks where the practice section ends; without a start, it starts at the beginning.
    pub fn mark_end(&mut self, seconds: f64) {
        let start = self.section.map_or(LEAD_IN, |(start, _)| start);
        self.section = (seconds > start).then_some((start, seconds));
    }
}

/// The next moment a note starts after `position`, or the last before it, for stepping
/// through a piece note by note.
pub(crate) fn next_onset(timeline: &crate::Timeline, position: f64, forward: bool) -> Option<f64> {
    const SAME: f64 = 1e-3;
    let onsets = timeline.notes.iter().map(|note| note.start);
    if forward {
        onsets.filter(|start| *start > position + SAME).min_by(f64::total_cmp)
    } else {
        onsets.filter(|start| *start < position - SAME).max_by(f64::total_cmp)
    }
}

/// Where playback goes once it has passed the end of the section, or of the piece, and
/// whether it keeps playing. A section always repeats; the whole piece only when looping.
pub(crate) fn past_the_end(
    position: f64,
    section: Option<(f64, f64)>,
    end: f64,
    looping: bool,
) -> Option<(f64, bool)> {
    match section {
        Some((start, finish)) if position > finish.min(end) => Some((start, true)),
        Some(_) => None,
        None if position > end => Some(if looping { (LEAD_IN, true) } else { (end, false) }),
        None => None,
    }
}

pub(crate) fn advance_transport(
    time: Res<Time>,
    mut transport: ResMut<Transport>,
    performance: Res<Performance>,
    audio: Res<Audio>,
) {
    medir!("advance_transport");
    match audio.player() {
        Some(player) => {
            let heard = player.position();
            match transport.pending_seek {
                Some(target) if (heard - target).abs() > 0.25 => {}
                _ => {
                    transport.pending_seek = None;
                    transport.position = follow(
                        transport.position,
                        heard,
                        time.delta_secs_f64(),
                        transport.speed,
                        transport.playing,
                    );
                }
            }
        }
        None if transport.playing => {
            transport.position += time.delta_secs_f64() * transport.speed;
        }
        None => return,
    }

    let end = performance.timeline.duration + 1.5;
    if let Some((target, playing)) =
        past_the_end(transport.position, transport.section, end, transport.looping)
    {
        transport.playing = playing;
        transport.seek_to(target);
        if let Some(player) = audio.player() {
            player.seek(target);
            player.set_playing(playing);
        }
    }
}

pub(crate) fn follow(position: f64, heard: f64, dt: f64, speed: f64, playing: bool) -> f64 {
    let error = heard - position;
    if !playing || error.abs() > CLOCK_SNAP_SECONDS {
        return heard;
    }
    position + (dt * speed + error * (dt / CLOCK_SETTLE_SECONDS).min(1.0)).max(0.0)
}

pub(crate) fn handle_input(
    keys: Res<ButtonInput<KeyCode>>,
    mut transport: ResMut<Transport>,
    audio: Res<Audio>,
    performance: Res<Performance>,
    menus: Option<Res<bevy_egui::input::EguiWantsInput>>,
) {
    medir!("handle_input");
    if menus.is_some_and(|menus| menus.wants_any_keyboard_input()) {
        return;
    }
    let before = (transport.playing, transport.speed);
    let mut jump = None;

    if keys.just_pressed(KeyCode::Space) {
        transport.playing = !transport.playing;
    }
    if keys.just_pressed(KeyCode::ArrowLeft) {
        jump = Some(transport.position - 2.0);
    }
    if keys.just_pressed(KeyCode::ArrowRight) {
        jump = Some(transport.position + 2.0);
    }
    if keys.just_pressed(KeyCode::Home) {
        jump = Some(LEAD_IN);
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        transport.speed = (transport.speed * 1.25).min(4.0);
    }
    if keys.just_pressed(KeyCode::ArrowDown) {
        transport.speed = (transport.speed / 1.25).max(0.1);
    }
    if keys.just_pressed(KeyCode::KeyL) {
        transport.looping = !transport.looping;
    }
    if keys.just_pressed(KeyCode::BracketLeft) {
        let here = transport.position;
        transport.mark_start(here);
    }
    if keys.just_pressed(KeyCode::BracketRight) {
        let here = transport.position;
        transport.mark_end(here);
    }
    if keys.just_pressed(KeyCode::Backslash) {
        transport.section = None;
    }
    for (key, forward) in [(KeyCode::Comma, false), (KeyCode::Period, true)] {
        if keys.just_pressed(key) {
            transport.playing = false;
            if let Some(onset) = next_onset(&performance.timeline, transport.position, forward) {
                jump = Some(onset);
            }
        }
    }

    if let Some(target) = jump {
        transport.seek_to(target);
    }
    if let Some(player) = audio.player() {
        if let Some(target) = jump {
            player.seek(target);
        }
        if transport.playing != before.0 {
            player.set_playing(transport.playing);
        }
        if transport.speed != before.1 {
            player.set_speed(transport.speed);
        }
    }
}

#[cfg(test)]
mod clock_tests {
    use super::follow;

    fn play(buffer: f64, frames: usize) -> Vec<f64> {
        let (frame, mut position, mut out) = (1.0 / 60.0, 0.0, Vec::new());
        for n in 1..=frames {
            let now = n as f64 * frame;
            let heard = (now / buffer).floor() * buffer;
            position = follow(position, heard, frame, 1.0, true);
            out.push(position);
        }
        out
    }

    #[test]
    fn the_clock_never_runs_backwards_between_audio_buffers() {
        let track = play(0.0107, 600);
        assert!(track.windows(2).all(|w| w[1] >= w[0]));
    }

    #[test]
    fn the_clock_moves_evenly_and_stays_with_the_audio() {
        let track = play(0.0107, 600);
        let steps: Vec<f64> = track.windows(2).skip(60).map(|w| w[1] - w[0]).collect();
        let (lo, hi) = steps.iter().fold((f64::MAX, f64::MIN), |(a, b), s| (a.min(*s), b.max(*s)));
        assert!(hi - lo < 0.004, "frame steps vary from {lo} to {hi}");
        assert!((track[599] - 10.0).abs() < 0.012, "{}", track[599]);
    }

    #[test]
    fn a_section_repeats_and_the_piece_loops_only_when_asked() {
        use super::{past_the_end, LEAD_IN};
        assert_eq!(past_the_end(5.0, Some((2.0, 4.0)), 60.0, false), Some((2.0, true)));
        assert_eq!(past_the_end(3.0, Some((2.0, 4.0)), 60.0, false), None);
        assert_eq!(past_the_end(61.0, Some((2.0, f64::INFINITY)), 60.0, false), Some((2.0, true)));
        assert_eq!(past_the_end(61.0, None, 60.0, true), Some((LEAD_IN, true)));
        assert_eq!(past_the_end(61.0, None, 60.0, false), Some((60.0, false)));
        assert_eq!(past_the_end(30.0, None, 60.0, true), None);
    }

    #[test]
    fn stepping_goes_from_note_to_note() {
        use on_score::{Note, NoteId, Score, TICKS_PER_QUARTER};
        let mut score = Score::default();
        for (i, midi) in [60u8, 62, 64].into_iter().enumerate() {
            let onset = i as i64 * TICKS_PER_QUARTER as i64;
            let id = NoteId(i as u32);
            score.notes.push(Note {
                id,
                midi,
                onset,
                hand: Some(on_hand::Hand::Right),
                ..Default::default()
            });
        }
        score.finalise();
        let timeline = crate::Timeline::build_for(&score, &[], &on_hand::HandProfile::default());
        let starts: Vec<f64> = timeline.notes.iter().map(|n| n.start).collect();
        assert_eq!(super::next_onset(&timeline, -1.0, true), Some(starts[0]));
        assert_eq!(super::next_onset(&timeline, starts[0], true), Some(starts[1]));
        assert_eq!(super::next_onset(&timeline, starts[1], false), Some(starts[0]));
        assert_eq!(super::next_onset(&timeline, starts[2], true), None);
    }

    #[test]
    fn marking_a_section() {
        use super::{Transport, LEAD_IN};
        let mut transport = Transport::default();
        transport.mark_end(8.0);
        assert_eq!(transport.section, Some((LEAD_IN, 8.0)), "B alone runs from the start");
        transport.mark_start(3.0);
        assert_eq!(transport.section, Some((3.0, 8.0)));
        transport.mark_start(9.0);
        assert_eq!(transport.section, Some((9.0, f64::INFINITY)), "A after B lets go of B");
        transport.mark_end(5.0);
        assert_eq!(transport.section, None, "B before A is no section");
    }

    #[test]
    fn a_jump_in_the_audio_is_followed_at_once() {
        assert_eq!(follow(1.0, 5.0, 1.0 / 60.0, 1.0, true), 5.0);
        assert_eq!(follow(1.0, 1.2, 1.0 / 60.0, 1.0, false), 1.2);
    }
}

/// Which hands the viewer plays. A hand that is not playing is left to the pianist: its
/// notes still fall, but its keys stay up, its sound is muted and its hand is hidden.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct Playing(pub [bool; 2]);

impl Default for Playing {
    fn default() -> Self {
        Self([true, true])
    }
}

/// The keys down this frame, worked out once for every system that draws them.
#[derive(Resource, Default)]
pub(crate) struct Keys(pub(crate) crate::timeline::KeyStates);

pub(crate) fn settle_keys(
    transport: Res<Transport>,
    performance: Res<Performance>,
    playing: Res<Playing>,
    mut keys: ResMut<Keys>,
) {
    medir!("settle_keys");
    keys.0 = performance.timeline.key_depression_for(transport.position, playing.0);
}

/// Hears only the hands that are playing, again whenever that changes.
pub(crate) fn mute_hands(playing: Res<Playing>, performance: Res<Performance>, audio: Res<Audio>) {
    medir!("mute_hands");
    if playing.is_changed() && !playing.is_added() {
        audio.load(performance.timeline.audio_notes_for(playing.0));
    }
}
