//! The loading gate: the simulation starts only when everything it needs is loaded.
//!
//! Modules register what must be ready with [`LoadingGate::require`] (content
//! registers its files). Every frame, before time is counted, the gate asks each
//! requirement; while one is pending it holds [`SimGate`], so no tick runs, in real
//! time or through `step`, and loading never shifts a tick: tick 1 always runs on the
//! same world. Once all are ready, `Loading` moves on to
//! [`EngineConfig::after_loading`](crate::EngineConfig::after_loading) (and
//! `RegionLoading` back to `InGame`); the simulation is released the frame after, when
//! `OnEnter` has spawned the world.

use ai_core::journal::{self, JournalKind};
use ai_core::SimGate;
use bevy::platform::collections::HashSet;
use bevy::prelude::*;

use crate::{EngineConfig, EngineState};

/// What a requirement reports each frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadStatus {
    /// Loaded; the simulation may run.
    Ready,
    /// Still loading; the progress text shows in `ai/status` and `observe`:
    /// `2 files left`.
    Pending(String),
    /// Cannot load: the simulation stays held and the reason is journaled once.
    Failed(String),
}

/// Asks the world whether one requirement is loaded.
pub type LoadCheck = fn(&World) -> LoadStatus;

/// Requirements that must be [`LoadStatus::Ready`] before the simulation may run.
#[derive(Resource, Default)]
pub struct LoadingGate {
    checks: Vec<(String, LoadCheck)>,
    /// Failures already journaled.
    reported: HashSet<String>,
}

impl LoadingGate {
    /// Holds the simulation while `check` is not ready, under `label` (`content`). A
    /// second requirement with the same label replaces the first.
    pub fn require(&mut self, label: &str, check: LoadCheck) {
        self.checks.retain(|(l, _)| l != label);
        self.checks.push((label.to_owned(), check));
    }

    /// Labels of the registered requirements.
    pub fn labels(&self) -> Vec<&str> {
        self.checks.iter().map(|(l, _)| l.as_str()).collect()
    }
}

/// Every requirement and what it reports right now, in registration order: for code that
/// must know whether a loading state can end at once (a region change finishes inside the
/// call that started it when nothing else is pending).
pub fn statuses(world: &World) -> Vec<(String, LoadStatus)> {
    let Some(gate) = world.get_resource::<LoadingGate>() else {
        return Vec::new();
    };
    gate.checks.iter().map(|(label, check)| (label.clone(), check(world))).collect()
}

/// Hold name for the frame between "everything is loaded" and `OnEnter` having run.
const ENTERING: &str = "engine";

/// `First`, before [`ai_core::sim::apply_gate`]: asks every requirement, holds or
/// releases the simulation, and leaves a loading state when all are ready.
pub(crate) fn update_loading(world: &mut World) {
    let checks = world.resource::<LoadingGate>().checks.clone();
    let statuses: Vec<(String, LoadStatus)> = checks.into_iter().map(|(label, check)| (label, check(world))).collect();
    let mut all_ready = true;
    for (label, status) in statuses {
        match status {
            LoadStatus::Ready => world.resource_mut::<SimGate>().release(&label),
            LoadStatus::Pending(progress) => {
                all_ready = false;
                world.resource_mut::<SimGate>().hold(&label, progress);
            }
            LoadStatus::Failed(why) => {
                all_ready = false;
                world.resource_mut::<SimGate>().hold(&label, format!("failed: {why}"));
                if world.resource_mut::<LoadingGate>().reported.insert(label.clone()) {
                    journal::emit(world, JournalKind::Error, format!("loading {label} failed: {why}"));
                }
            }
        }
    }
    let after_loading = world.get_resource::<EngineConfig>().map_or(EngineState::InGame, |c| c.after_loading);
    let next = match world.resource::<State<EngineState>>().get() {
        EngineState::Loading => Some(after_loading),
        EngineState::RegionLoading => Some(EngineState::InGame),
        _ => None,
    };
    match next {
        Some(next) if all_ready => {
            world.resource_mut::<SimGate>().hold(ENTERING, format!("entering {next:?}"));
            world.resource_mut::<NextState<EngineState>>().set(next);
        }
        // Pending requirements hold it themselves; out of a loading state, nothing does.
        _ => world.resource_mut::<SimGate>().release(ENTERING),
    }
}
