//! Engine states and who plays.
//!
//! ```text
//! Loading ──(loading gate open)──► EngineConfig::after_loading: InGame (default) or MainMenu
//! InGame ─► PauseState: Running ⇄ Paused        RegionLoading ──(gate open)──► InGame
//! ```
//!
//! The simulation never runs while the engine is `Loading` or `RegionLoading` (the
//! [loading gate](crate::loading) holds it). `PauseState::Paused` is the player's pause
//! (a menu): in single player it pauses the simulation, and an agent's `resume` closes
//! it again. Pausing the simulation itself (`shards pause`, `--paused`) is a dev tool and
//! leaves `PauseState` alone, so stepping a game never pops a menu up.

use ai_core::journal::{AiJournal, JournalKind};
use ai_core::{sim, SimTick};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Where the whole application is. Games spawn their world in
/// `OnEnter(EngineState::InGame)`: content exists only after `Loading`.
#[derive(States, Reflect, Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
pub enum EngineState {
    /// Waiting for the loading gate (content, ...). No ticks run.
    #[default]
    Loading,
    MainMenu,
    InGame,
    /// Reserved for region streaming: no ticks run, and the engine returns to `InGame`
    /// once the loading gate is open again (`OnEnter(InGame)` runs again then).
    RegionLoading,
}

/// The player's pause, inside `InGame`.
#[derive(SubStates, Reflect, Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[source(EngineState = EngineState::InGame)]
pub enum PauseState {
    #[default]
    Running,
    /// In single player the simulation is paused too.
    Paused,
}

/// This machine's part in a session. The network model is host-authoritative with
/// tick-stamped inputs; networking itself comes later.
#[derive(Resource, Reflect, Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
#[reflect(Resource, Default)]
pub enum NetRole {
    #[default]
    Single,
    /// Runs the simulation for everyone.
    Host,
    /// Sends inputs, shows the host's world.
    Client,
}

/// A player-controlled character: player 0, 1, ... in a session.
#[derive(Component, Reflect, Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[reflect(Component, Default)]
pub struct PlayerId(pub u8);

/// The character this machine's keyboard and camera drive.
#[derive(Component, Reflect, Clone, Copy, Debug, Default)]
#[reflect(Component, Default)]
pub struct LocalPlayer;

/// `PauseState::Paused` paused the simulation, so leaving it resumes.
#[derive(Resource, Default)]
pub(crate) struct MenuPaused(bool);

pub(crate) fn pause_simulation(world: &mut World) {
    if *world.resource::<NetRole>() != NetRole::Single || sim::is_paused(world) {
        return;
    }
    sim::pause(world);
    world.resource_mut::<MenuPaused>().0 = true;
}

pub(crate) fn resume_simulation(world: &mut World) {
    let paused_it = std::mem::take(&mut world.resource_mut::<MenuPaused>().0);
    if paused_it && sim::is_paused(world) && sim::gate_message(world).is_none() {
        sim::resume(world);
    }
}

/// `PreUpdate`: the simulation runs again (an agent's `resume`) while the pause menu is
/// up in single player: the menu closes.
pub(crate) fn close_pause_when_resumed(
    state: Option<Res<State<PauseState>>>,
    role: Res<NetRole>,
    time: Res<Time<Virtual>>,
    mut menu: ResMut<MenuPaused>,
    mut next: ResMut<NextState<PauseState>>,
) {
    if state.is_some_and(|s| *s.get() == PauseState::Paused) && *role == NetRole::Single && !time.is_paused() {
        menu.0 = false;
        next.set(PauseState::Running);
    }
}

/// `state Loading -> InGame` in the journal.
pub(crate) fn journal_transitions<S: States>(
    mut transitions: MessageReader<StateTransitionEvent<S>>,
    tick: Res<SimTick>,
    mut journal: ResMut<AiJournal>,
) {
    for t in transitions.read() {
        if let (Some(from), Some(to)) = (&t.exited, &t.entered)
            && from != to
        {
            journal.push(tick.0, JournalKind::Sim, format!("state {from:?} -> {to:?}"));
        }
    }
}
