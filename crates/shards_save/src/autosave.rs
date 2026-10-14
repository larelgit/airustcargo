//! Saves the game decides on: rewards, discoveries, camps, beacons, bosses.
//!
//! Gameplay does not write files in the middle of a tick. It *asks*: a system calls
//! [`SaveRequests::request`], or the game registers [`autosave_on::<E>`](crate::SaveAppExt::autosave_on)
//! and an event does. The request is carried out at `FixedLast` of that tick, after the
//! simulation, so the file holds the state at the end of the tick; capturing reads the
//! world and writing touches only files, so the simulation is the same with or without
//! autosaves (a test hashes it both ways).
//!
//! A request waits while its kind is inside its debounce window: `debounce_secs` of the
//! `character` or `world` definition in `*.save.ron` content (default
//! [`DEFAULT_DEBOUNCE_SECS`]) counted in ticks since that kind was last autosaved. It is
//! deferred, not dropped, so the state that is written is the newest.
//!
//! ```ron
//! // assets/content/saves.save.ron
//! {
//!     "world": (debounce_secs: 10.0),
//!     "character": (debounce_secs: 2.0),
//! }
//! ```

use std::collections::BTreeMap;

use ai_core::journal::{self, JournalKind};
use ai_core::{sim, AiReplay, SimConfig};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::{ContentDb, ContentDef, Refs};

use crate::ops::{self, resolve_body, SaveSlots, SaveWhat};
use crate::section::SaveKind;

/// Seconds between two autosaves of a kind when content does not say.
pub const DEFAULT_DEBOUNCE_SECS: f32 = 2.0;

/// How a kind of save is autosaved: the content kind `save`, with one definition per
/// [`SaveKind`] (`character`, `world`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct SaveDef {
    /// Autosaves of this kind closer than this (seconds of game time) wait for it.
    #[serde(default = "default_debounce")]
    pub debounce_secs: f32,
}

fn default_debounce() -> f32 {
    DEFAULT_DEBOUNCE_SECS
}

impl SaveDef {
    /// What is wrong with definition `id`.
    fn problems(&self, id: &str) -> Vec<String> {
        let mut problems = Vec::new();
        if !SaveKind::ALL.iter().any(|k| k.as_str() == id) {
            problems.push(format!("`{id}` is not a kind of save; the ids are `character` and `world`"));
        }
        if !self.debounce_secs.is_finite() || self.debounce_secs < 0.0 {
            problems.push(format!("debounce_secs {} must be zero or more seconds", self.debounce_secs));
        }
        problems
    }
}

impl ContentDef for SaveDef {
    const KIND: &'static str = "save";

    fn validate(&self, id: &str, refs: &mut Refs) {
        for problem in self.problems(id) {
            refs.error(problem);
        }
    }
}

struct Pending {
    kind: SaveKind,
    body: Option<Entity>,
    reason: String,
}

/// Saves asked for and not yet written. Systems ask with [`request`](Self::request); the
/// plugin writes them at the end of the tick.
#[derive(Resource, Default)]
pub struct SaveRequests {
    pending: Vec<Pending>,
    /// The tick each kind was last autosaved at.
    last: BTreeMap<SaveKind, u64>,
}

impl SaveRequests {
    /// Asks for `what` to be saved into the current slots at the end of this tick (or when
    /// the debounce window allows). Asking again for the same kind and body before it is
    /// written changes nothing: one file is written, with the first reason.
    pub fn request(&mut self, what: SaveWhat, reason: impl Into<String>) {
        let reason = reason.into();
        for (kind, body) in what.parts() {
            if !self.pending.iter().any(|p| p.kind == kind && p.body == body) {
                self.pending.push(Pending { kind, body, reason: reason.clone() });
            }
        }
    }

    /// Whether something waits to be written.
    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }
}

/// `CampRested` → `camp_rested`.
pub(crate) fn snake_case(name: &str) -> String {
    let mut out = String::new();
    for (i, c) in name.chars().enumerate() {
        if c.is_uppercase() {
            if i > 0 {
                out.push('_');
            }
            out.extend(c.to_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// `FixedLast`, after the replay's bookkeeping: writes what was asked for.
pub(crate) fn write_requests(world: &mut World) {
    if !world.resource::<SaveRequests>().is_pending() {
        return;
    }
    let tick = sim::tick(world);
    // A replay is being played back (to verify it, or to watch it): its states are not the
    // player's progress and must not overwrite it.
    if world.get_resource::<AiReplay>().is_some_and(AiReplay::is_playing) {
        let skipped = std::mem::take(&mut world.resource_mut::<SaveRequests>().pending);
        for p in skipped {
            journal::log(world, JournalKind::Sim, format!("save: {} autosave ({}) skipped while a replay plays", p.kind, p.reason));
        }
        return;
    }
    let hz = world.get_resource::<SimConfig>().map_or(60.0, |c| c.tick_hz);
    let pending = std::mem::take(&mut world.resource_mut::<SaveRequests>().pending);
    let mut waiting = Vec::new();
    for p in pending {
        let secs = world.get_resource::<ContentDb>().and_then(|db| db.get::<SaveDef>(p.kind.as_str())).map_or(DEFAULT_DEBOUNCE_SECS, |d| d.debounce_secs);
        let window = (f64::from(secs) * hz).ceil() as u64;
        let last = world.resource::<SaveRequests>().last.get(&p.kind).copied();
        if last.is_some_and(|last| tick < last.saturating_add(window)) {
            waiting.push(p);
            continue;
        }
        world.resource_mut::<SaveRequests>().last.insert(p.kind, tick);
        let slot = world.resource::<SaveSlots>().get(p.kind).to_owned();
        let body = resolve_body(world, p.body);
        // The failure is in the journal (Error) already; a request is not retried.
        let _ = ops::save_part(world, p.kind, body, &slot, "", &p.reason);
    }
    world.resource_mut::<SaveRequests>().pending.extend(waiting);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reasons_are_snake_case() {
        assert_eq!(snake_case("CampRested"), "camp_rested");
        assert_eq!(snake_case("Beacon"), "beacon");
        assert_eq!(snake_case("BossDefeated2"), "boss_defeated2");
    }

    #[test]
    fn asking_twice_writes_once() {
        let mut requests = SaveRequests::default();
        requests.request(SaveWhat::All, "camp");
        requests.request(SaveWhat::World, "beacon");
        requests.request(SaveWhat::Character(None), "reward");
        let asked: Vec<(SaveKind, &str)> = requests.pending.iter().map(|p| (p.kind, p.reason.as_str())).collect();
        assert_eq!(asked, [(SaveKind::World, "camp"), (SaveKind::Character, "camp")]);
        // A named body is another request.
        requests.request(SaveWhat::Character(Some(Entity::from_raw_u32(7).unwrap())), "ally");
        assert_eq!(requests.pending.len(), 3);
    }

    #[test]
    fn definitions_name_kinds_and_seconds() {
        assert!(SaveDef { debounce_secs: 3.0 }.problems("world").is_empty());
        let errors = SaveDef { debounce_secs: -1.0 }.problems("boss");
        assert!(errors.iter().any(|e| e.contains("is not a kind of save")) && errors.iter().any(|e| e.contains("debounce_secs")), "{errors:?}");
        assert!(!SaveDef { debounce_secs: f32::NAN }.problems("world").is_empty());
        assert_eq!(default_debounce(), DEFAULT_DEBOUNCE_SECS);
    }
}
