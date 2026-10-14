//! Engine states and who plays.
//!
//! ```text
//! Loading ──(loading gate open)──► EngineConfig::after_loading: InGame (default) or MainMenu
//! InGame ─► PauseState: Running ⇄ Paused        RegionLoading ──(gate open)──► InGame
//! ```
//!
//! The simulation never runs while the engine is `Loading` or `RegionLoading` (the
//! [loading gate](crate::loading) holds it). A trip `InGame → RegionLoading → InGame` (a
//! region change) keeps the game: `OnExit(InGame)` and `OnEnter(InGame)` do not run for
//! it, so the players a game spawned on entering `InGame` stay (the region module moves
//! them); `OnEnter`/`OnExit(RegionLoading)` mark the trip. `PauseState::Paused` is the player's pause
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
    /// The game's menu; the engine leaves it to the game.
    MainMenu,
    /// Playing: games spawn their world on entering it.
    InGame,
    /// A region is being replaced (`shards_world`): no ticks run, and the engine returns
    /// to `InGame` once the loading gate is open again. Coming from `InGame`, the trip
    /// keeps the game: `OnExit(InGame)` and `OnEnter(InGame)` do not run for it.
    RegionLoading,
}

/// The player's pause, inside `InGame`.
#[derive(SubStates, Reflect, Clone, Copy, PartialEq, Eq, Hash, Debug, Default, Serialize, Deserialize)]
#[source(EngineState = EngineState::InGame)]
pub enum PauseState {
    /// Playing.
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
    /// One machine, no network.
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

/// Whether a region trip keeps the game, and the schedules hidden while its transition runs.
#[derive(Resource, Default)]
pub(crate) struct RegionTrip {
    /// `RegionLoading` was entered from `InGame`: coming back continues that game.
    from_game: bool,
    /// Taken out of [`Schedules`] for the transition running now.
    hidden: Vec<Schedule>,
}

/// `StateTransition`, before the exit schedules of `EngineState`: a region trip out of or
/// back into `InGame` hides `OnExit(InGame)` / `OnEnter(InGame)` from this transition, so
/// the game's world (its players above all) is neither torn down nor spawned twice.
pub(crate) fn hide_game_schedules(In(transition): In<Option<StateTransitionEvent<EngineState>>>, world: &mut World) {
    let Some(StateTransitionEvent { exited: Some(from), entered: Some(to), .. }) = transition else {
        return;
    };
    let mut trip = world.get_resource_or_init::<RegionTrip>();
    let hidden = match (from, to) {
        (EngineState::InGame, EngineState::RegionLoading) => {
            trip.from_game = true;
            world.resource_mut::<Schedules>().remove(OnExit(EngineState::InGame))
        }
        (EngineState::RegionLoading, EngineState::InGame) if trip.from_game => {
            trip.from_game = false;
            world.resource_mut::<Schedules>().remove(OnEnter(EngineState::InGame))
        }
        (EngineState::RegionLoading, _) => {
            trip.from_game = false;
            None
        }
        _ => None,
    };
    if let Some(schedule) = hidden {
        world.resource_mut::<RegionTrip>().hidden.push(schedule);
    }
}

/// `StateTransition`, after the enter schedules of `EngineState`: puts back what
/// [`hide_game_schedules`] took out.
pub(crate) fn restore_game_schedules(world: &mut World) {
    let hidden = std::mem::take(&mut world.get_resource_or_init::<RegionTrip>().hidden);
    let mut schedules = world.resource_mut::<Schedules>();
    for schedule in hidden {
        schedules.insert(schedule);
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
