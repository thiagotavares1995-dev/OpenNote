//! The performance in time: which keys are down, the grips each hand makes, and the hand
//! poses between them, including how the two hands keep out of each other's way.

pub mod janela;

use std::collections::BTreeMap;

use on_fingering::biomech::{BiomechModel, Grip};
use on_hand::keyboard::{is_black, MIDI_HIGHEST, MIDI_LOWEST};
use on_hand::skeleton::{dof, HandPose};
use on_hand::{Finger, Hand, HandProfile};
use on_score::{Fingering, NoteId, Score};

pub const APPROACH_SECONDS: f64 = 0.28;

const LIFT_RATE_MM: f32 = 120.0;
const LIFT_MAX_MM: f32 = 28.0;

const KEY_SECONDS: (f64, f64) = (0.110, 0.022);

const KEY_BEFORE_SOUND: f64 = 0.85;

const KEY_AFTER_SOUND: f64 = 0.15;

const KEY_CONTACT_SPEED: f64 = 1.5;

pub fn key_seconds(velocity: u8) -> f64 {
    let loud = ((f64::from(velocity) - 20.0) / 107.0).clamp(0.0, 1.0);
    KEY_SECONDS.0 * (KEY_SECONDS.1 / KEY_SECONDS.0).powf(loud)
}

const KEY_RELEASE_SECONDS: f64 = 0.06;

#[derive(Debug, Clone, PartialEq)]
pub struct TimelineNote {
    pub id: NoteId,
    pub midi: u8,
    pub start: f64,
    pub end: f64,
    pub damped: f64,
    pub hand: Hand,
    pub finger: Option<Finger>,
    pub velocity: u8,
    pub restruck: Option<f64>,
    pub key_moves: f64,
    pub key_rises: f64,
}

impl TimelineNote {
    pub fn sounds_at(&self, time: f64) -> bool {
        self.start <= time && time < self.end
    }

    pub fn is_black(&self) -> bool {
        is_black(self.midi)
    }

    pub fn key_bottoms(&self) -> f64 {
        self.start + KEY_AFTER_SOUND * key_seconds(self.velocity)
    }

    pub fn key_depth(&self, time: f64) -> f64 {
        let going_down = |at: f64| {
            let (from, bottom) = (self.key_moves, self.key_bottoms());
            if at <= from {
                return 0.0;
            }
            if at >= bottom {
                return 1.0;
            }
            let span = bottom - from;
            let s = (at - from) / span;
            let push = KEY_CONTACT_SPEED * span / key_seconds(self.velocity);
            push * (s * s * s - 2.0 * s * s + s) + (3.0 * s * s - 2.0 * s * s * s)
        };
        if time < self.key_rises {
            return going_down(time);
        }
        let u = ((time - self.key_rises) / KEY_RELEASE_SECONDS).min(1.0);
        going_down(self.key_rises) * (1.0 - u * u * (3.0 - 2.0 * u))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct GripEvent {
    pub time: f64,
    pub release: f64,
    pub grip: Grip,
    pub struck: Vec<(Finger, u8)>,
}

#[derive(Debug, Clone)]
pub struct Timeline {
    pub notes: Vec<TimelineNote>,
    pub grips: [Vec<GripEvent>; 2],
    pub duration: f64,
    pub title: Option<String>,
    pub style: MotionStyle,
}

#[derive(Debug, Clone, PartialEq)]
pub struct MotionStyle {
    pub lift: f32,
    pub weight: f32,
    pub phrase_ends: Vec<f64>,
}

impl Default for MotionStyle {
    fn default() -> Self {
        Self { lift: 1.0, weight: 1.0, phrase_ends: Vec::new() }
    }
}

const BREATH_LIFT_MM: (f32, f32) = (10.0, 15.0);

const BREATH_DEG: f32 = 8.0;

const BREATH_NEAR_SECONDS: f64 = 0.08;

const REST_LIFT_MM: f32 = 40.0;

const HAND_TILT_SHARE: f32 = 0.85;

const HAND_TILT_REACH_MM: f32 = 130.0;

const REST_CLEAR_MM: f32 = 140.0;

const REST_MARGIN_SECONDS: f64 = 0.5;

impl Timeline {
    pub fn audio_notes(&self) -> Vec<on_audio::Note> {
        self.audio_notes_for([true, true])
    }

    /// What is heard, counting only the hands that are playing.
    pub fn audio_notes_for(&self, playing: [bool; 2]) -> Vec<on_audio::Note> {
        const RELEASE_GAP: f64 = 0.012;
        self.notes
            .iter()
            .filter(|note| playing[note.hand as usize])
            .map(|note| on_audio::Note {
                midi: note.midi,
                start: note.start,
                end: if note.damped > note.end + 1e-6 {
                    note.damped
                } else {
                    (note.end - RELEASE_GAP).max(note.start + 0.02)
                },
                velocity: note.velocity,
            })
            .collect()
    }

    pub fn build(score: &Score, fingerings: &[Fingering]) -> Self {
        Self::build_for(score, fingerings, &HandProfile::default())
    }

    pub fn build_for(score: &Score, fingerings: &[Fingering], profile: &HandProfile) -> Self {
        let by_note: BTreeMap<NoteId, Finger> = fingerings.iter().map(|f| (f.note, f.finger)).collect();

        let mut notes: Vec<TimelineNote> = score
            .notes
            .iter()
            .filter_map(|n| {
                Some(TimelineNote {
                    id: n.id,
                    midi: n.midi,
                    start: n.onset_seconds,
                    end: score.release_seconds(n.id),
                    damped: score.damped_seconds(n.id),
                    hand: n.hand?,
                    finger: by_note.get(&n.id).copied(),
                    velocity: n.velocity,
                    restruck: None,
                    key_moves: n.onset_seconds,
                    key_rises: score.release_seconds(n.id),
                })
            })
            .collect();
        notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.midi.cmp(&b.midi)));

        let spans = on_fingering::SpanModel::for_hand_size(profile.size()).table();
        let grips = Hand::ALL.map(|hand| {
            let model = BiomechModel::new(profile.clone(), hand, Default::default());
            Self::grips_for(&notes, hand, spans, &model)
        });
        roll_in_time(&mut notes, &grips);
        notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.midi.cmp(&b.midi)));
        let mut next_on_key: BTreeMap<u8, f64> = BTreeMap::new();
        for note in notes.iter_mut().rev() {
            note.restruck = next_on_key.insert(note.midi, note.start);
        }
        let mut last_on_key: BTreeMap<u8, usize> = BTreeMap::new();
        for i in 0..notes.len() {
            let key = key_seconds(notes[i].velocity);
            let earliest = notes[i].start - KEY_BEFORE_SOUND * key;
            let mut after_last = f64::MIN;
            if let Some(&j) = last_on_key.get(&notes[i].midi) {
                let up = (earliest - 0.6 * KEY_RELEASE_SECONDS).min(notes[j].key_rises);
                notes[j].key_rises = up.max(notes[j].key_bottoms());
                after_last = notes[j].key_rises + 0.6 * KEY_RELEASE_SECONDS;
            }
            notes[i].key_moves = earliest.max(after_last).min(notes[i].start - KEY_AFTER_SOUND * key);
            last_on_key.insert(notes[i].midi, i);
        }

        let duration = notes.iter().map(|n| n.end).fold(0.0, f64::max);

        Self { notes, grips, duration, title: score.title.clone(), style: MotionStyle::default() }
    }

    pub fn animators(
        &self,
        profile: &HandProfile,
        weights: on_fingering::biomech::BiomechWeights,
    ) -> Vec<HandAnimator> {
        let mut animators: Vec<HandAnimator> = Hand::ALL
            .iter()
            .map(|hand| {
                let model = BiomechModel::new(profile.clone(), *hand, weights);
                if smooth_motion() {
                    HandAnimator::moving(
                        *hand,
                        model,
                        self.hand_grips(*hand).to_vec(),
                        &self.notes,
                        &self.style,
                    )
                } else {
                    HandAnimator::new(*hand, model, self.hand_grips(*hand).to_vec())
                }
            })
            .collect();

        let keyboard = on_hand::keyboard::Keyboard::new();
        let lowest = self.notes.iter().map(|n| n.midi).min().unwrap_or(60);
        let highest = self.notes.iter().map(|n| n.midi).max().unwrap_or(60);
        let (left_edge, right_edge) = (keyboard.centre_x(lowest), keyboard.centre_x(highest));
        for animator in &mut animators {
            animator.park_at(match animator.hand() {
                Hand::Left => left_edge - IDLE_CLEARANCE_MM,
                Hand::Right => right_edge + IDLE_CLEARANCE_MM,
            });
        }
        animators
    }

    fn grips_for(
        notes: &[TimelineNote],
        hand: Hand,
        spans: &on_fingering::SpanTable,
        model: &BiomechModel,
    ) -> Vec<GripEvent> {
        let mut mine: Vec<(&TimelineNote, Finger)> = notes
            .iter()
            .filter(|note| note.hand == hand)
            .filter_map(|note| Some((note, note.finger?)))
            .collect();
        mine.sort_by(|a, b| a.0.start.total_cmp(&b.0.start));

        let mut events: Vec<GripEvent> = Vec::new();
        let mut held: Vec<(&TimelineNote, Finger)> = Vec::new();
        let mut index = 0;

        while index < mine.len() {
            let time = mine[index].0.start;
            held.retain(|(note, _)| note.end > time + 1e-6);

            while index < mine.len() && mine[index].0.start <= time + TOGETHER_SECONDS {
                let (note, finger) = mine[index];
                held.retain(|(_, other)| *other != finger);
                held.push((note, finger));
                index += 1;
            }

            let holdable = |held: &[(&TimelineNote, Finger)]| {
                held.iter().enumerate().all(|(i, (low, from))| {
                    held[i + 1..].iter().all(|(high, to)| {
                        let apart = i32::from(high.midi) - i32::from(low.midi);
                        spans.get(hand, *from, *to).is_practical(apart)
                    })
                })
            };

            let drop_one = |held: &mut Vec<(&TimelineNote, Finger)>| {
                if held.len() < 2 {
                    return false;
                }
                let Some(at) = held
                    .iter()
                    .enumerate()
                    .filter(|(_, (note, _))| note.start < time - 1e-6)
                    .min_by(|a, b| a.1 .0.start.total_cmp(&b.1 .0.start))
                    .map(|(at, _)| at)
                else {
                    return false;
                };
                held.remove(at);
                true
            };

            held.sort_by_key(|(note, _)| note.midi);

            let makeable = |held: &[(&TimelineNote, Finger)]| {
                holdable(held) && {
                    let shape = Grip::new(held.iter().map(|(note, finger)| (note.midi, *finger)).collect());
                    model.grip_outcome(&shape).reachable
                }
            };

            while held.len() > 1 && !makeable(&held) && drop_one(&mut held) {}

            let mut rolls: Vec<Vec<(&TimelineNote, Finger)>> = Vec::new();
            while held.len() > 1 && !makeable(&held) {
                let lowest = held.remove(0);
                match rolls.last_mut() {
                    Some(group) => {
                        group.push(lowest);
                        if !makeable(group) {
                            let overflow = group.pop().expect("just pushed");
                            rolls.push(vec![overflow]);
                        }
                    }
                    None => rolls.push(vec![lowest]),
                }
            }

            let step = mine.get(index).map_or(ROLL_SECONDS, |(next, _)| {
                ROLL_SECONDS.min((next.start - time) / (rolls.len() + 1) as f64)
            });
            let mut grip_time = time;
            for group in &rolls {
                events.push(grip_event(group, grip_time, time));
                grip_time += step;
            }

            let struck: Vec<(Finger, u8)> = held
                .iter()
                .filter(|(note, _)| together(note.start, time))
                .map(|(note, finger)| (*finger, note.velocity))
                .collect();
            let keys: Vec<(u8, Finger)> = held.iter().map(|(note, finger)| (note.midi, *finger)).collect();
            let release = held.iter().fold(grip_time, |latest, (note, _)| latest.max(note.end));
            let mut grip = Grip::new(keys);
            grip.keys.sort_by_key(|(midi, _)| *midi);
            events.push(GripEvent { time: grip_time, release, grip, struck });
        }
        events
    }

    pub fn hand_grips(&self, hand: Hand) -> &[GripEvent] {
        &self.grips[hand as usize]
    }

    pub fn key_depression(&self, time: f64) -> KeyStates {
        self.key_depression_for(time, [true, true])
    }

    /// The keys down at a moment, counting only the hands that are playing.
    pub fn key_depression_for(&self, time: f64, playing: [bool; 2]) -> KeyStates {
        self.key_depression_among(time, playing, 0..self.notes.len())
    }

    /// As `key_depression_for`, looking only at `candidates`: indices into `notes`, in
    /// increasing order, that include every note whose key moves at `time` (an index built
    /// from `key_spans` lists them).
    pub fn key_depression_among(
        &self,
        time: f64,
        playing: [bool; 2],
        candidates: impl IntoIterator<Item = usize>,
    ) -> KeyStates {
        let mut states = KeyStates::default();
        let notes = candidates.into_iter().map(|index| &self.notes[index]);
        for note in notes.filter(|note| playing[note.hand as usize]) {
            if time < note.key_moves || time > note.key_rises + KEY_RELEASE_SECONDS {
                continue;
            }
            let depth = note.key_depth(time);
            let slot = states.slot(note.midi);
            if depth as f32 >= states.depth[slot] {
                states.depth[slot] = depth as f32;
                if depth > 0.0 {
                    states.hand[slot] = Some(note.hand);
                }
            }
        }
        states
    }

    /// For each note, the stretch of time in which its key is moving or down.
    pub fn key_spans(&self) -> impl Iterator<Item = (f64, f64)> + '_ {
        self.notes.iter().map(|note| (note.key_moves, note.key_rises + KEY_RELEASE_SECONDS))
    }

    pub fn visible_notes(&self, time: f64, lookahead: f64) -> impl Iterator<Item = &TimelineNote> {
        self.notes.iter().filter(move |n| n.end > time - 0.5 && n.start < time + lookahead)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct KeyStates {
    pub depth: Vec<f32>,
    pub hand: Vec<Option<Hand>>,
}

impl Default for KeyStates {
    fn default() -> Self {
        let count = (MIDI_HIGHEST - MIDI_LOWEST + 1) as usize;
        Self { depth: vec![0.0; count], hand: vec![None; count] }
    }
}

impl KeyStates {
    fn slot(&self, midi: u8) -> usize {
        (midi.clamp(MIDI_LOWEST, MIDI_HIGHEST) - MIDI_LOWEST) as usize
    }

    pub fn depth_of(&self, midi: u8) -> f32 {
        self.depth[self.slot(midi)]
    }

    pub fn hand_on(&self, midi: u8) -> Option<Hand> {
        self.hand[self.slot(midi)]
    }

    pub fn pressed(&self) -> impl Iterator<Item = u8> + '_ {
        (MIDI_LOWEST..=MIDI_HIGHEST).filter(move |m| self.depth_of(*m) > 0.05)
    }
}

const ROLL_SECONDS: f64 = 0.05;

const HOLD_RAMP_SECONDS: f64 = 0.15;

const TOGETHER_SECONDS: f64 = on_fingering::playability::CHORD_SECONDS;

fn together(start: f64, onset: f64) -> bool {
    start >= onset - 1e-6 && start <= onset + TOGETHER_SECONDS
}

const CONTINUITY: [f32; on_hand::skeleton::DOF] = {
    let mut weights = [0.3f32; on_hand::skeleton::DOF];
    weights[dof::WRIST_X] = 0.0;
    weights[dof::WRIST_Y] = 0.0;
    weights[dof::WRIST_Z] = 0.0;
    weights[dof::WRIST_DEVIATION] = 0.8;
    weights[dof::WRIST_FLEXION] = 0.8;
    weights[dof::WRIST_PRONATION] = 0.8;
    weights
};

const ALIKE_RADIANS: f32 = 0.15;

fn alike(a: &HandPose, b: &HandPose) -> bool {
    let angles = dof::WRIST_DEVIATION..on_hand::skeleton::DOF;
    let n = angles.len() as f32;
    let sum: f32 = angles.map(|i| (a.q[i] - b.q[i]).powi(2)).sum();
    (sum / n).sqrt() < ALIKE_RADIANS
}

fn roll_in_time(notes: &mut [TimelineNote], grips: &[Vec<GripEvent>; 2]) {
    for (side, events) in grips.iter().enumerate() {
        for event in events {
            for (finger, _) in &event.struck {
                let Some((midi, _)) = event.grip.keys.iter().find(|(_, f)| f == finger) else { continue };
                let on_time = notes.iter().any(|n| {
                    n.hand as usize == side && n.midi == *midi && (n.start - event.time).abs() < 1e-6
                });
                if on_time {
                    continue;
                }
                let rolled = notes.iter_mut().filter(|n| {
                    n.hand as usize == side
                        && n.midi == *midi
                        && n.finger == Some(*finger)
                        && n.start < event.time - 1e-6
                        && n.start > event.time - 4.0 * ROLL_SECONDS - 1e-6
                });
                if let Some(note) = rolled.max_by(|a, b| a.start.total_cmp(&b.start)) {
                    note.start = event.time;
                    note.end = note.end.max(event.time + 0.02);
                    note.damped = note.damped.max(note.end);
                }
            }
        }
    }
}

fn grip_event(notes: &[(&TimelineNote, Finger)], time: f64, struck_at: f64) -> GripEvent {
    let struck: Vec<(Finger, u8)> = notes
        .iter()
        .filter(|(note, _)| together(note.start, struck_at))
        .map(|(note, finger)| (*finger, note.velocity))
        .collect();
    let mut grip = Grip::new(notes.iter().map(|(note, finger)| (note.midi, *finger)).collect());
    grip.keys.sort_by_key(|(midi, _)| *midi);
    let release = notes.iter().fold(time, |latest, (note, _)| latest.max(note.end));
    GripEvent { time, release, grip, struck }
}

const PARK_Y_MM: f32 = -80.0;
const PARK_Z_MM: f32 = 60.0;

pub const IDLE_CLEARANCE_MM: f32 = 170.0;

pub const CLEARANCE_SLACK_MM: f32 = 3.0;

pub const HAND_THICKNESS_MM: f32 = 20.0;

const SWING_LEAD_SECONDS: f64 = 0.08;

const DECIDE_EVERY_SECONDS: f64 = 1.0 / 30.0;

const APART_WIDEN_SECONDS: f64 = 0.15;

const APART_SMOOTH_SECONDS: f64 = 0.3;

const SHIFT_SMOOTH_SECONDS: f64 = 0.12;

const FINISH_TAPS: i32 = 4;

const FINISH_SECONDS: f64 = 0.010;

const FINISH_KEEP: f32 = 1e-6;

const ENGAGE_MM: f32 = 30.0;

const CLEARANCE_MAX_MM: f32 = 80.0;

const SWING_MAX_DEG: f32 = 30.0;

const SWING_TOLERANCE_MM: f32 = 6.0;

const SWING_SLIDE_MM: f32 = 20.0;

const STRIKE_LIFT_SECONDS: f64 = 0.16;

const STRIKE_LIFT_DEG: (f32, f32) = (2.5, 9.0);

const WRIST_DROP_MM: (f32, f32) = (1.6, 6.5);

const WRIST_DROP_DEG: (f32, f32) = (3.5, 13.0);

const WRIST_FALL_SECONDS: f64 = 0.045;
const WRIST_SETTLE_SECONDS: f64 = 0.22;

const BREATH_MM: f32 = 1.4;
const BREATH_HZ: f64 = 0.23;

const BREATH_EASE_SECONDS: f64 = 0.35;

const APPROACH_BY_FORCE: f32 = 0.6;

const STRIKE_PEAK: (f32, f32) = (0.5, 0.72);

pub struct HandAnimator {
    hand: Hand,
    skeleton: on_hand::Skeleton,
    events: Vec<GripEvent>,
    poses: Vec<HandPose>,
    resting: HandPose,
    motion: Option<Motion>,
}

struct Motion {
    track: crate::motion::Track,
    strikes: crate::motion::Strikes,
    lag: crate::motion::Lag,
    style: MotionStyle,
}

pub fn smooth_motion() -> bool {
    std::env::var("ON_MOTION").map_or(true, |v| v != "v1")
}

impl HandAnimator {
    pub fn new(hand: Hand, model: BiomechModel, events: Vec<GripEvent>) -> Self {
        let mut poses: Vec<HandPose> = Vec::with_capacity(events.len());
        for event in &events {
            let cold = model.grip_pose(&event.grip);
            let pose = match poses.last() {
                Some(previous) if !alike(previous, &cold) => {
                    let (warm, error) = model.grip_pose_near(&event.grip, previous, CONTINUITY);
                    if error > on_hand::ik::CONTACT_TOLERANCE_MM && model.grip_outcome(&event.grip).reachable
                    {
                        cold
                    } else {
                        warm
                    }
                }
                _ => cold,
            };
            poses.push(pose);
        }

        let resting = poses.first().copied().unwrap_or_else(|| {
            let centre = model.keyboard().centre_x(60);
            model.skeleton().rest_pose(glam::Vec3::new(centre, PARK_Y_MM, PARK_Z_MM))
        });

        let skeleton = model.skeleton().clone();
        Self { hand, skeleton, events, poses, resting, motion: None }
    }

    pub fn moving(
        hand: Hand,
        model: BiomechModel,
        events: Vec<GripEvent>,
        notes: &[TimelineNote],
        style: &MotionStyle,
    ) -> Self {
        let twin = BiomechModel::new(model.skeleton().profile().clone(), hand, *model.weights());
        let mut animator = Self::new(hand, twin, events);
        if animator.events.is_empty() {
            return animator;
        }
        animator.poses = crate::motion::smooth_wrists(&animator.events, &animator.poses, &model);
        crate::motion::relax_idle_fingers(&animator.events, &mut animator.poses, &model);
        if let Some(first) = animator.poses.first() {
            animator.resting = *first;
        }
        let mine: Vec<TimelineNote> = notes.iter().filter(|n| n.hand == hand).cloned().collect();
        let keyboard = model.keyboard().clone();
        let rest = |from: f64, until: f64, pose: &HandPose| {
            let mut rest = *pose;
            rest.q[dof::WRIST_Z] += REST_LIFT_MM;
            let other = notes
                .iter()
                .filter(|n| {
                    n.hand != hand
                        && n.end > from - REST_MARGIN_SECONDS
                        && n.start < until + REST_MARGIN_SECONDS
                })
                .map(|n| keyboard.centre_x(n.midi));
            let (lo, hi) = other.fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
            if lo <= hi {
                let x = &mut rest.q[dof::WRIST_X];
                *x = match hand {
                    Hand::Right => x.max(hi + REST_CLEAR_MM).min(keyboard.width()),
                    Hand::Left => x.min(lo - REST_CLEAR_MM).max(0.0),
                };
            }
            rest
        };
        let track = crate::motion::Track::new(&animator.events, &animator.poses, &rest);
        let mut strikes = crate::motion::Strikes::new(
            &mine,
            &animator.events,
            &track.stays,
            &animator.poses,
            &animator.skeleton,
        );
        strikes.lift = style.lift;
        let (from, until) = track.span();
        let (from, until) = (from - 1.0, until + 2.0);
        let lag = crate::motion::Lag::new(&track, from, until);
        animator.motion = Some(Motion { track, strikes, lag, style: style.clone() });
        animator
    }

    pub fn hand(&self) -> Hand {
        self.hand
    }

    pub fn park_at(&mut self, centre_x: f32) {
        if !self.events.is_empty() {
            return;
        }
        self.resting = self.skeleton.rest_pose(glam::Vec3::new(centre_x, PARK_Y_MM, PARK_Z_MM));
    }

    pub fn skeleton(&self) -> &on_hand::Skeleton {
        &self.skeleton
    }

    fn current_index(&self, time: f64) -> Option<usize> {
        if self.events.is_empty() || time < self.events[0].time {
            return None;
        }
        let index = self.events.partition_point(|e| e.time <= time).saturating_sub(1);
        Some(index)
    }

    pub fn pose_at(&self, time: f64) -> HandPose {
        self.finished(time, |at| self.unpressed(at))
    }

    fn finished(&self, time: f64, base: impl Fn(f64) -> HandPose) -> HandPose {
        let mut pose = base(time);
        let Some(motion) = &self.motion else { return pose };
        let taps: Vec<[f32; on_hand::skeleton::DOF]> = (-FINISH_TAPS..=FINISH_TAPS)
            .map(|i| {
                let at = time + FINISH_SECONDS * f64::from(i) / f64::from(FINISH_TAPS.max(1));
                let raw = base(at);
                let mut done = raw;
                motion.strikes.press(&self.skeleton, &mut done, at);
                std::array::from_fn(|d| done.q[d] - raw.q[d])
            })
            .collect();
        let exact = &taps[FINISH_TAPS as usize];
        for d in dof::THUMB_CMC_FLEX..on_hand::skeleton::DOF {
            let (mut sum, mut total) = (0.0f32, 0.0f32);
            for (k, tap) in taps.iter().enumerate() {
                let u = (k as f32 - FINISH_TAPS as f32) / (FINISH_TAPS + 1) as f32;
                let weight = (1.0 - u * u).powi(3);
                sum += weight * tap[d];
                total += weight;
            }
            let smooth = sum / total;
            pose.q[d] += smooth + FINISH_KEEP * ((exact[d] - smooth) / FINISH_KEEP).tanh();
        }
        self.skeleton.clamp(&mut pose);
        pose
    }

    pub fn unpressed(&self, time: f64) -> HandPose {
        if let Some(motion) = &self.motion {
            return self.flowing(motion, time);
        }
        let Some(index) = self.current_index(time) else {
            return self.resting;
        };
        let mut pose = self.carried(index, time);
        pose.q[dof::WRIST_Z] += self.breath(index, time);
        self.sink_into_note(&mut pose, index, time);
        self.raise_fingers_about_to_strike(&mut pose, index, time);
        self.skeleton.clamp(&mut pose);
        pose
    }

    fn flowing(&self, motion: &Motion, time: f64) -> HandPose {
        if self.events.is_empty() {
            return self.resting;
        }
        let mut pose = motion.track.at(time);
        let lag = motion.lag.at(time);
        for (k, offset) in lag.iter().enumerate() {
            let index = dof::THUMB_CMC_FLEX + k;
            let idle = 1.0 - motion.strikes.engaged(crate::motion::digit_of(index), time);
            pose.q[index] += offset * idle;
        }
        let stays = &motion.track.stays;
        let index = stays.partition_point(|s| s.arrive <= time).saturating_sub(1);
        if let (Some(current), Some(next)) = (self.events.get(index), stays.get(index + 1)) {
            let gap = next.arrive - current.release;
            if gap > 1e-3
                && gap < crate::motion::REST_GAP_SECONDS
                && time > current.release
                && time < next.arrive
            {
                let hardest = self.events[index + 1].struck.iter().map(|(_, v)| *v).max().unwrap_or(64);
                let force = 1.0 + APPROACH_BY_FORCE * (f32::from(hardest) / 127.0 - 0.5) * 2.0;
                let room = crate::motion::hop_room(gap);
                let mut height = (gap as f32 * LIFT_RATE_MM * force).min(LIFT_MAX_MM) * room;
                let shape = crate::motion::bump((time - current.release) / gap);
                let ends = &motion.style.phrase_ends;
                let at = ends.partition_point(|e| *e < current.release - BREATH_NEAR_SECONDS);
                if ends.get(at).is_some_and(|e| *e <= current.release + BREATH_NEAR_SECONDS) {
                    let breath =
                        (BREATH_LIFT_MM.0 + BREATH_LIFT_MM.1 * (motion.style.lift - 1.0).max(0.0)) * room;
                    height = height.max(breath * motion.style.lift.min(1.5));
                    pose.q[dof::WRIST_FLEXION] += BREATH_DEG.to_radians() * motion.style.lift * room * shape;
                }
                pose.q[dof::WRIST_Z] += height * shape;
            }
            pose.q[dof::WRIST_Z] += self.breath(index, time);
        }
        let (down, up) = crate::motion::stroke(&self.events, &motion.strikes, time);
        let up = up * motion.style.lift;
        pose.q[dof::WRIST_Z] += up * (1.0 - HAND_TILT_SHARE) - down * motion.style.weight;
        pose.q[dof::WRIST_FLEXION] -= (up * HAND_TILT_SHARE / HAND_TILT_REACH_MM).atan();
        self.skeleton.clamp(&mut pose);
        pose
    }

    fn carried(&self, index: usize, time: f64) -> HandPose {
        let current = &self.events[index];
        let pose = self.poses[index];

        let Some(next) = self.events.get(index + 1) else {
            return pose;
        };
        let next_pose = self.poses[index + 1];

        let start = self.departure(index);
        if time <= start {
            return pose;
        }
        let span = (next.time - start).max(1e-4);
        let t = ((time - start) / span).clamp(0.0, 1.0);

        let resting_before = start > current.time + 1e-4;
        let resting_after =
            self.events.get(index + 2).is_none_or(|_| self.departure(index + 1) > next.time + 1e-4);

        let mut moved = pose.lerp(&next_pose, travel(t, resting_before, resting_after) as f32);
        moved.q[dof::WRIST_Z] += lift_between(current, next, time);
        moved
    }

    fn sink_into_note(&self, pose: &mut HandPose, index: usize, time: f64) {
        let current = &self.events[index];
        let since = time - current.time;
        if !(0.0..WRIST_SETTLE_SECONDS).contains(&since) {
            return;
        }
        let Some(hardest) = current.struck.iter().map(|(_, v)| *v).max() else {
            return;
        };
        let hardness = f32::from(hardest) / 127.0;

        let shape = if since < WRIST_FALL_SECONDS {
            (since / WRIST_FALL_SECONDS) as f32
        } else {
            let back = ((since - WRIST_FALL_SECONDS) / (WRIST_SETTLE_SECONDS - WRIST_FALL_SECONDS)) as f32;
            1.0 - back * back * (3.0 - 2.0 * back)
        };
        let until = self.events.get(index + 1).map_or(f64::INFINITY, |next| next.time - time);
        let handover = ((until / WRIST_FALL_SECONDS) as f32).clamp(0.0, 1.0);
        let shape = shape.clamp(0.0, 1.0) * handover * handover * (3.0 - 2.0 * handover);

        let depth = WRIST_DROP_MM.0 + (WRIST_DROP_MM.1 - WRIST_DROP_MM.0) * hardness;
        let angle = WRIST_DROP_DEG.0 + (WRIST_DROP_DEG.1 - WRIST_DROP_DEG.0) * hardness;
        pose.q[dof::WRIST_Z] -= depth * shape;
        pose.q[dof::WRIST_FLEXION] -= angle.to_radians() * shape;
    }

    fn breath(&self, index: usize, time: f64) -> f32 {
        let current = &self.events[index];
        if time <= current.release {
            return 0.0;
        }
        let until = self.events.get(index + 1).map_or(f64::INFINITY, |next| next.time - time);
        let free = ((time - current.release).min(until) / BREATH_EASE_SECONDS).clamp(0.0, 1.0) as f32;
        let free = free * free * (3.0 - 2.0 * free);
        let offset = match self.hand {
            Hand::Right => 0.0,
            Hand::Left => 2.1,
        };
        let phase = (time * BREATH_HZ * std::f64::consts::TAU) as f32 + offset;
        free * BREATH_MM * phase.sin()
    }

    fn raise_fingers_about_to_strike(&self, pose: &mut HandPose, index: usize, time: f64) {
        let Some(next) = self.events.get(index + 1) else {
            return;
        };
        let current = &self.events[index];
        let window = (next.time - current.time).min(STRIKE_LIFT_SECONDS);
        if window <= 1e-4 || time < next.time - window || time >= next.time {
            return;
        }
        let through = ((time - (next.time - window)) / window) as f32;

        for (finger, velocity) in &next.struck {
            if !current.grip.keys.iter().any(|(_, f)| f == finger) {
                continue;
            }
            let hardness = f32::from(*velocity) / 127.0;
            let height = STRIKE_LIFT_DEG.0 + (STRIKE_LIFT_DEG.1 - STRIKE_LIFT_DEG.0) * hardness;
            let peak = STRIKE_PEAK.0 + (STRIKE_PEAK.1 - STRIKE_PEAK.0) * hardness;
            let shape = if through < peak { through / peak } else { (1.0 - through) / (1.0 - peak) };
            let lift = height.to_radians() * shape.clamp(0.0, 1.0);

            match finger {
                Finger::Thumb => {
                    pose.q[dof::THUMB_MCP_FLEX] -= lift;
                    pose.q[dof::THUMB_CMC_FLEX] -= lift * 0.5;
                }
                other => {
                    let base = dof::finger(other.index() - 1);
                    pose.q[base + dof::MCP_FLEX] -= lift;
                    pose.q[base + dof::PIP_FLEX] += lift * 0.5;
                }
            }
        }
        self.skeleton.clamp(pose);
    }

    fn departure(&self, index: usize) -> f64 {
        let current = &self.events[index];
        let Some(next) = self.events.get(index + 1) else {
            return current.time;
        };
        let latest = next.time - APPROACH_SECONDS;
        let start = current.release.min(next.time).max(current.time);
        start.min(latest.max(current.time))
    }

    pub fn joints(&self, pose: &HandPose) -> Vec<glam::Vec3> {
        let posture = self.skeleton.forward(pose);
        let mut out = Vec::with_capacity(21);
        out.push(posture.wrist);
        for digit in &posture.chain {
            out.extend_from_slice(digit);
        }
        out
    }

    pub fn grips_around(&self, time: f64) -> Option<(&Grip, Option<&Grip>, f32)> {
        if let Some(motion) = &self.motion {
            return match motion.track.travelling(time) {
                Some((a, b, blend)) if a != b => {
                    Some((&self.events[a].grip, Some(&self.events[b].grip), blend))
                }
                Some((a, _, _)) => Some((&self.events[a].grip, None, 0.0)),
                None => {
                    let last = if time <= motion.track.stays.first().map_or(0.0, |s| s.arrive) {
                        0
                    } else {
                        self.events.len() - 1
                    };
                    self.events.get(last).map(|e| (&e.grip, None, 0.0))
                }
            };
        }
        let Some(index) = self.current_index(time) else {
            return self.events.first().map(|first| (&first.grip, None, 0.0));
        };
        let current = &self.events[index].grip;
        let Some(next) = self.events.get(index + 1) else {
            return Some((current, None, 0.0));
        };
        let start = self.departure(index);
        if time <= start {
            return Some((current, Some(&next.grip), 0.0));
        }
        let span = (next.time - start).max(1e-4);
        let t = ((time - start) / span).clamp(0.0, 1.0);
        let resting_before = start > self.events[index].time + 1e-4;
        let resting_after =
            self.events.get(index + 2).is_none_or(|_| self.departure(index + 1) > next.time + 1e-4);
        Some((current, Some(&next.grip), travel(t, resting_before, resting_after) as f32))
    }

    pub fn holding(&self, time: f64, ramp: f64) -> f32 {
        if let Some(motion) = &self.motion {
            let stays = &motion.track.stays;
            let ramp = ramp.max(HOLD_RAMP_SECONDS);
            let first = stays.partition_point(|s| s.arrive <= time - ramp).saturating_sub(1);
            let last = stays.partition_point(|s| s.arrive < time + ramp);
            let mut held = 0.0f32;
            for k in first..last {
                if self.events[k].grip.keys.is_empty() {
                    continue;
                }
                let until = stays[k]
                    .leave
                    .max(self.events[k].release.min(stays.get(k + 1).map_or(f64::MAX, |s| s.arrive)));
                let inside = (time - stays[k].arrive).min(until - time);
                held = held.max(crate::motion::minimum_jerk((inside + ramp) / ramp) as f32);
            }
            return held;
        }
        let index = self.current_index(time).unwrap_or(0);
        let mut held = 0.0f32;
        for event in &self.events[index.saturating_sub(1)..(index + 2).min(self.events.len())] {
            if event.grip.keys.is_empty() {
                continue;
            }
            let inside = (time - event.time).min(event.release - time);
            let u = ((inside + ramp) / ramp).clamp(0.0, 1.0) as f32;
            held = held.max(u * u * (3.0 - 2.0 * u));
        }
        held
    }

    fn lands_next(&self, time: f64) -> f64 {
        let after = self.events.partition_point(|e| e.time <= time);
        self.events[after..].iter().find(|e| !e.grip.keys.is_empty()).map_or(f64::INFINITY, |e| e.time)
    }

    pub fn grip_at(&self, time: f64) -> Option<&Grip> {
        let index = self.current_index(time)?;
        let event = &self.events[index];
        (time < event.release).then_some(&event.grip)
    }
}

fn travel(t: f64, resting_before: bool, resting_after: bool) -> f64 {
    let t = t.clamp(0.0, 1.0);
    let m0 = if resting_before { 0.0 } else { 1.0 };
    let m1 = if resting_after { 0.0 } else { 1.0 };
    let (t2, t3) = (t * t, t * t * t);
    (-2.0 * t3 + 3.0 * t2) + m0 * (t3 - 2.0 * t2 + t) + m1 * (t3 - t2)
}

fn lift_between(current: &GripEvent, next: &GripEvent, time: f64) -> f32 {
    let gap = next.time - current.release;
    if gap <= 1e-3 || time <= current.release || time >= next.time {
        return 0.0;
    }
    let hardest = next.struck.iter().map(|(_, v)| *v).max().unwrap_or(64);
    let force = 1.0 + APPROACH_BY_FORCE * (f32::from(hardest) / 127.0 - 0.5) * 2.0;
    let height = (gap as f32 * LIFT_RATE_MM * force).min(LIFT_MAX_MM);
    let through = ((time - current.release) / gap).clamp(0.0, 1.0) as f32;
    height * (through * std::f32::consts::PI).sin()
}

type Shift = [[f32; SHIFTED]; 2];

const SHIFTED: usize = dof::WRIST_PRONATION + 1;

pub struct Decisions {
    first: i64,
    shifts: Vec<Shift>,
}

impl Decisions {
    pub fn new(animators: &[HandAnimator], until: f64) -> Self {
        let lead = ((SHIFT_SMOOTH_SECONDS + FINISH_SECONDS) / DECIDE_EVERY_SECONDS).ceil() as i64 + 1;
        Self::between(animators, -lead, (until.max(0.0) / DECIDE_EVERY_SECONDS).ceil() as i64)
    }

    fn between(animators: &[HandAnimator], first: i64, last: i64) -> Self {
        let step = DECIDE_EVERY_SECONDS;
        let margin = ((APART_WIDEN_SECONDS + APART_SMOOTH_SECONDS) / step).ceil() as i64 + 1;
        let lowest = first - margin;
        let raw: Vec<Apart> = (lowest..=last + margin)
            .map(|k| if k < 0 { Apart::default() } else { apart_at(animators, k as f64 * step) })
            .collect();
        let shifts = (first..=last)
            .map(|k| {
                let at = k as f64 * step;
                let apart = apart_around(at, |j| raw[(j - lowest) as usize]);
                let base = [animators[0].unpressed(at), animators[1].unpressed(at)];
                let moved = posed_apart(animators, at, base, apart);
                [0, 1].map(|side| std::array::from_fn(|d| moved[side].q[d] - base[side].q[d]))
            })
            .collect();
        Self { first, shifts }
    }

    fn shift(&self, time: f64) -> Shift {
        let step = DECIDE_EVERY_SECONDS;
        let from = ((time - SHIFT_SMOOTH_SECONDS) / step).ceil() as i64;
        let to = ((time + SHIFT_SMOOTH_SECONDS) / step).floor() as i64;
        let mut sum = [[0.0f32; SHIFTED]; 2];
        let mut total = 0.0f32;
        for k in from..=to {
            let u = ((time - k as f64 * step) / SHIFT_SMOOTH_SECONDS) as f32;
            let weight = (1.0 - u * u).max(0.0).powi(3);
            if weight <= 0.0 {
                continue;
            }
            total += weight;
            let Some(shift) = usize::try_from(k - self.first).ok().and_then(|i| self.shifts.get(i)) else {
                continue;
            };
            for side in 0..2 {
                for d in 0..SHIFTED {
                    sum[side][d] += weight * shift[side][d];
                }
            }
        }
        if total > 0.0 {
            for side in &mut sum {
                for value in side.iter_mut() {
                    *value /= total;
                }
            }
        }
        sum
    }

    fn pose(&self, animators: &[HandAnimator], time: f64) -> [HandPose; 2] {
        [0, 1].map(|side| {
            let animator = &animators[side];
            animator.finished(time, |at| {
                let shift = self.shift(at);
                let mut pose = animator.unpressed(at);
                for d in 0..SHIFTED {
                    pose.q[d] += shift[side][d];
                }
                animator.skeleton.clamp(&mut pose);
                pose
            })
        })
    }
}

pub fn pose_both(animators: &[HandAnimator], time: f64) -> [HandPose; 2] {
    let step = DECIDE_EVERY_SECONDS;
    let reach = SHIFT_SMOOTH_SECONDS + FINISH_SECONDS;
    let from = ((time - reach) / step).floor() as i64;
    let to = ((time + reach) / step).ceil() as i64;
    Decisions::between(animators, from, to).pose(animators, time)
}

pub fn pose_both_decided(animators: &[HandAnimator], decisions: &Decisions, time: f64) -> [HandPose; 2] {
    decisions.pose(animators, time)
}

fn posed_apart(animators: &[HandAnimator], time: f64, base: [HandPose; 2], apart: Apart) -> [HandPose; 2] {
    let mut poses = base;
    for (side, animator) in animators.iter().enumerate().take(2) {
        let (swing, lift) = (apart.swing[side], apart.lift[side]);
        if swing == 0.0 && lift == 0.0 {
            continue;
        }
        let hold = animator.holding(time, SWING_LEAD_SECONDS);
        let over = (lift - apart.lift[1 - side]).max(0.0);
        let adjust = |pose: &mut HandPose, grip: &Grip| {
            swing_by(animator, pose, grip, swing);
            raise_by(animator, pose, grip, over * hold);
        };
        if let Some((current, next, blend)) = animator.grips_around(time) {
            let mut a = poses[side];
            adjust(&mut a, current);
            poses[side] = match next {
                Some(next) if blend > 0.0 => {
                    let mut b = poses[side];
                    adjust(&mut b, next);
                    a.lerp(&b, blend)
                }
                _ => a,
            };
        }
        poses[side].q[dof::WRIST_Z] += lift * (1.0 - hold);
    }
    poses
}

#[derive(Clone, Copy, Default)]
struct Apart {
    lift: [f32; 2],
    swing: [f32; 2],
}

fn apart_around(time: f64, decide: impl Fn(i64) -> Apart) -> Apart {
    let step = DECIDE_EVERY_SECONDS;
    let widen = (APART_WIDEN_SECONDS / step).round() as i64;
    let first = ((time - APART_SMOOTH_SECONDS) / step).ceil() as i64;
    let last = ((time + APART_SMOOTH_SECONDS) / step).floor() as i64;
    let lowest = first - widen;
    let raw: Vec<[f32; 6]> = (lowest..=last + widen)
        .map(|k| {
            if k < 0 {
                return [0.0; 6];
            }
            let a = decide(k);
            [
                a.lift[0],
                a.lift[1],
                a.swing[0].max(0.0),
                (-a.swing[0]).max(0.0),
                a.swing[1].max(0.0),
                (-a.swing[1]).max(0.0),
            ]
        })
        .collect();
    let mut sum = [0.0f32; 6];
    let mut total = 0.0f32;
    for k in first..=last {
        let u = ((time - k as f64 * step) / APART_SMOOTH_SECONDS) as f32;
        let weight = (1.0 - u * u).max(0.0).powi(3);
        if weight <= 0.0 {
            continue;
        }
        let at = (k - lowest) as usize;
        let span = &raw[at - widen as usize..=at + widen as usize];
        for (c, slot) in sum.iter_mut().enumerate() {
            *slot += weight * span.iter().map(|r| r[c]).fold(0.0f32, f32::max);
        }
        total += weight;
    }
    if total <= 0.0 {
        return Apart::default();
    }
    let v = sum.map(|x| x / total);
    Apart { lift: [v[0], v[1]], swing: [v[2] - v[3], v[4] - v[5]] }
}

fn apart_at(animators: &[HandAnimator], time: f64) -> Apart {
    let mut apart = Apart::default();
    let poses =
        [animators[Hand::Left as usize].unpressed(time), animators[Hand::Right as usize].unpressed(time)];
    let joints =
        [animators[Hand::Left as usize].joints(&poses[0]), animators[Hand::Right as usize].joints(&poses[1])];
    let near =
        ((ENGAGE_MM - nearest(&joints[0], &joints[1])) / (ENGAGE_MM - HAND_THICKNESS_MM)).clamp(0.0, 1.0);
    if near <= 0.0 {
        return apart;
    }

    let free = [
        animators[Hand::Left as usize].grip_at(time).is_none(),
        animators[Hand::Right as usize].grip_at(time).is_none(),
    ];
    let lands =
        [animators[Hand::Left as usize].lands_next(time), animators[Hand::Right as usize].lands_next(time)];
    let up = match (free[0], free[1]) {
        (true, true) => Some(usize::from(lands[1] > lands[0])),
        (true, false) => Some(0),
        (false, true) => Some(1),
        (false, false) => None,
    };
    if let Some(up) = up {
        apart.lift[up] = lift_to_clear(&joints[up], &joints[1 - up]).min(CLEARANCE_MAX_MM);
    }

    let hold = [
        animators[Hand::Left as usize].holding(time, SWING_LEAD_SECONDS),
        animators[Hand::Right as usize].holding(time, SWING_LEAD_SECONDS),
    ];
    let strength = near * hold[0] * hold[1];
    if strength <= 0.0 {
        return apart;
    }
    let centre = |side: usize| {
        let (lo, hi) = joints[side].iter().fold((f32::MAX, f32::MIN), |(a, b), j| (a.min(j.x), b.max(j.x)));
        (lo + hi) / 2.0
    };
    for side in 0..2 {
        let animator = &animators[side];
        let Some((current, next, blend)) = animator.grips_around(time) else {
            continue;
        };
        let grip = match next {
            Some(next) if blend >= 0.5 => next,
            _ => current,
        };
        apart.swing[side] = swing_angle(animator, &poses[side], grip, centre(1 - side), strength);
    }
    apart
}

pub fn nearest(a: &[glam::Vec3], b: &[glam::Vec3]) -> f32 {
    a.iter().flat_map(|p| b.iter().map(move |q| p.distance(*q))).fold(f32::MAX, f32::min)
}

pub fn overlapping(a: &[glam::Vec3], b: &[glam::Vec3]) -> bool {
    a.iter().any(|p| b.iter().any(|q| p.distance_squared(*q) < HAND_THICKNESS_MM * HAND_THICKNESS_MM))
}

pub fn overlap_depth(a: &[glam::Vec3], b: &[glam::Vec3]) -> f32 {
    (HAND_THICKNESS_MM - nearest(a, b)).max(0.0)
}

fn lift_to_clear(up: &[glam::Vec3], down: &[glam::Vec3]) -> f32 {
    let want_apart = HAND_THICKNESS_MM + CLEARANCE_SLACK_MM;
    let mut needed = 0.0f32;
    for p in up {
        for q in down {
            let flat = (p.x - q.x).hypot(p.y - q.y);
            if flat >= want_apart {
                continue;
            }
            let want = (want_apart * want_apart - flat * flat).sqrt();
            needed = needed.max(want - (p.z - q.z));
        }
    }
    needed
}

fn raise_by(animator: &HandAnimator, pose: &mut HandPose, grip: &Grip, height: f32) {
    if height <= 0.0 {
        return;
    }
    if grip.keys.is_empty() {
        pose.q[dof::WRIST_Z] += height;
        return;
    }
    let raised = {
        let mut trial = *pose;
        let middle = |p: &HandPose| -> glam::Vec3 {
            let posture = animator.skeleton.forward(p);
            grip.keys.iter().map(|(_, finger)| posture.chain[finger.index()][3]).sum::<glam::Vec3>()
                / grip.keys.len() as f32
        };
        let held = middle(&trial);
        trial.q[dof::WRIST_Z] += height;
        const PROBE: f32 = 0.01;
        const RAISE_LEAST_SLOPE: f32 = 40.0;
        const RAISE_STEP: f32 = 0.12;
        for _ in 0..3 {
            let now = middle(&trial);
            let high = now.z - held.z;
            let mut probe = trial;
            probe.q[dof::WRIST_FLEXION] += PROBE;
            let slope = (middle(&probe).z - now.z) / PROBE;
            let usable = slope.signum() * slope.abs().max(RAISE_LEAST_SLOPE);
            let flexion = &mut trial.q[dof::WRIST_FLEXION];
            let step = (-high / usable).clamp(-RAISE_STEP, RAISE_STEP);
            *flexion = on_hand::skeleton::LIMITS[dof::WRIST_FLEXION].clamp(*flexion + step);
        }
        let now = middle(&trial);
        trial.q[dof::WRIST_X] += held.x - now.x;
        trial.q[dof::WRIST_Y] += held.y - now.y;
        trial
    };
    *pose = raised;
}

fn swing_angle(animator: &HandAnimator, pose: &HandPose, grip: &Grip, away_from: f32, urgency: f32) -> f32 {
    if grip.keys.is_empty() {
        return 0.0;
    }
    let mut best = (0.0f32, 0.0f32);
    for sign in [1.0f32, -1.0] {
        let mut trial = *pose;
        let (angle, strayed) =
            swing_by(animator, &mut trial, grip, sign * SWING_MAX_DEG.to_radians() * urgency);
        if strayed > 1.1 {
            continue;
        }
        let gained =
            (trial.wrist_position().x - away_from).abs() - (pose.wrist_position().x - away_from).abs();
        if gained > best.0 {
            best = (gained, angle);
        }
    }
    best.1
}

fn swing_by(animator: &HandAnimator, pose: &mut HandPose, grip: &Grip, angle: f32) -> (f32, f32) {
    if grip.keys.is_empty() || angle == 0.0 {
        return (0.0, 0.0);
    }
    let tips = |p: &HandPose| -> Vec<glam::Vec3> {
        let posture = animator.skeleton.forward(p);
        grip.keys.iter().map(|(_, finger)| posture.chain[finger.index()][3]).collect()
    };
    let middle = |t: &[glam::Vec3]| t.iter().copied().sum::<glam::Vec3>() / t.len() as f32;
    let before = tips(pose);
    let held = middle(&before);

    let swung = |angle: f32| {
        let mut trial = *pose;
        trial.q[dof::WRIST_DEVIATION] += angle;
        animator.skeleton.clamp(&mut trial);
        let moved = middle(&tips(&trial));
        trial.q[dof::WRIST_X] += held.x - moved.x;
        trial.q[dof::WRIST_Y] += held.y - moved.y;
        let strayed = tips(&trial)
            .iter()
            .zip(&before)
            .map(|(now, was)| {
                ((now.x - was.x).abs() / SWING_TOLERANCE_MM).max((now.y - was.y).abs() / SWING_SLIDE_MM)
            })
            .fold(0.0f32, f32::max);
        (trial, strayed)
    };

    let mut angle = angle;
    let (mut trial, mut strayed) = swung(angle);
    for _ in 0..2 {
        if strayed <= 1.0 {
            break;
        }
        angle /= strayed;
        (trial, strayed) = swung(angle);
    }
    let turned = trial.q[dof::WRIST_DEVIATION] - pose.q[dof::WRIST_DEVIATION];
    *pose = trial;
    (turned, strayed)
}

#[cfg(test)]
mod tests;
