//! The events of a run, and their journal lines.
//!
//! Every event is a global trigger with `Reflect` and `Clone` and the run's `key` and
//! `encounter`; the step triggers it right after the transition it reports. A game reacts with an
//! observer (and returns at once unless `shards::core::simulates`); the umbrella sends them to
//! clients as `net_event`s, where observers are presentation only.

use ai_core::journal::EventFormatter;
use ai_core::AiId;
use bevy::prelude::*;

use crate::def::CueDef;

/// A run started: a rising start condition, `start_encounter`, or a placed pack disturbed.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct EncounterStarted {
    pub key: String,
    pub encounter: String,
    /// Participants standing now.
    pub participants: u32,
    /// Participants in all.
    pub of: u32,
}

/// A wave became due (the start, the previous wave cleared, a resume): it spawns `delay` seconds
/// from now. Carries the cue so that a client's presentation needs no content lookup.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct WaveWarned {
    pub key: String,
    pub encounter: String,
    /// 1-based number of the wave that is due.
    pub wave: u32,
    pub waves: u32,
    pub delay: f32,
    /// The anchor.
    pub at: Vec3,
    pub cue: Option<CueDef>,
}

/// A wave spawned.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct WaveStarted {
    pub key: String,
    pub encounter: String,
    pub wave: u32,
    pub waves: u32,
    pub members: Vec<Entity>,
}

/// A wave's counted members are all out.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct WaveCleared {
    pub key: String,
    pub encounter: String,
    pub wave: u32,
    pub waves: u32,
}

/// The last wave cleared; once per run, never by a load.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct EncounterCompleted {
    pub key: String,
    pub encounter: String,
}

/// Every participant is out.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct EncounterWiped {
    pub key: String,
    pub encounter: String,
    /// Waves started now, after the rewind.
    pub wave: u32,
    pub waves: u32,
    /// The wave in progress was despawned and `wave` went back by one.
    pub rewound: bool,
    /// The wipe came between waves: nothing was despawned or rewound.
    pub between: bool,
}

/// Someone stands again after a wipe; the pending wave spawns `delay` seconds from now.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct EncounterResumed {
    pub key: String,
    pub encounter: String,
    /// 1-based number of the pending wave; 0 when the wipe pulled back the placed wave, and the
    /// run goes back to waiting for its start.
    pub wave: u32,
    pub waves: u32,
    pub delay: f32,
}

/// Why a run reset.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResetWhy {
    /// No standing participant within the radius for `reset_after`.
    Absent,
    /// The dev tool or `reset_encounter`.
    Dev,
    /// The definition changed under the run.
    Content,
}

/// The run was reset.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct EncounterReset {
    pub key: String,
    pub encounter: String,
    pub why: ResetWhy,
    /// Seconds nobody was near, for `Absent`.
    pub after: f32,
}

/// `yard/ring (ring_waves)` -> the run's name in a journal line: the key.
fn name(key: &str) -> &str {
    key
}

pub(crate) fn started(e: &EncounterStarted, _: &World) -> Option<String> {
    Some(format!("encounter {} ({}) started: {}/{} players in", name(&e.key), e.encounter, e.participants, e.of))
}

pub(crate) fn warned(e: &WaveWarned, _: &World) -> Option<String> {
    Some(format!("encounter {}: wave {}/{} in {:.1} s", name(&e.key), e.wave, e.waves, e.delay))
}

pub(crate) fn wave_started(e: &WaveStarted, w: &World) -> Option<String> {
    let ids: Vec<String> = e.members.iter().filter_map(|m| w.get::<AiId>(*m)).map(ToString::to_string).collect();
    Some(format!("encounter {}: wave {}/{} spawned {}", name(&e.key), e.wave, e.waves, ids.join(" ")))
}

pub(crate) fn cleared(e: &WaveCleared, _: &World) -> Option<String> {
    Some(format!("encounter {}: wave {}/{} cleared", name(&e.key), e.wave, e.waves))
}

pub(crate) fn completed(e: &EncounterCompleted, _: &World) -> Option<String> {
    Some(format!("encounter {}: won", name(&e.key)))
}

pub(crate) fn wiped(e: &EncounterWiped, _: &World) -> Option<String> {
    Some(if e.rewound {
        format!("encounter {}: wiped in wave {}/{}, back to {}/{}", name(&e.key), e.wave + 1, e.waves, e.wave, e.waves)
    } else if e.between {
        format!("encounter {}: wiped between waves, wave {}/{} still pending", name(&e.key), e.wave + 1, e.waves)
    } else {
        format!("encounter {}: wiped in wave {}/{}, survivors pulled back", name(&e.key), e.wave, e.waves)
    })
}

pub(crate) fn resumed(e: &EncounterResumed, _: &World) -> Option<String> {
    Some(if e.wave > 0 {
        format!("encounter {}: resumed, wave {}/{} in {:.1} s", name(&e.key), e.wave, e.waves, e.delay)
    } else {
        format!("encounter {}: resumed, ready again in {:.1} s", name(&e.key), e.delay)
    })
}

pub(crate) fn reset(e: &EncounterReset, _: &World) -> Option<String> {
    Some(match e.why {
        ResetWhy::Absent => format!("encounter {}: reset (nobody near for {:.1} s)", name(&e.key), e.after),
        ResetWhy::Dev => format!("encounter {}: reset (dev)", name(&e.key)),
        ResetWhy::Content => format!("encounter {}: reset (its definition changed)", name(&e.key)),
    })
}

/// Formatter type alias, for the registration list.
#[allow(dead_code)]
pub(crate) type Formatter<E> = EventFormatter<E>;
