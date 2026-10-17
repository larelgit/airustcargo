//! The main menu: `EngineState::MainMenu`, between loading and the game.
//!
//! A game with a main menu says so with [`EngineConfig::after_loading`](crate::EngineConfig::after_loading)
//! `MainMenu`; whether a run shows it is [`EngineConfig::menu`](crate::EngineConfig::menu)'s to say
//! ([`start_state`](crate::EngineConfig::start_state)). By default only a run a person may be there
//! for does (a window, `--offscreen`): headless runs, tests, replays and checks go straight in, as a
//! game without a menu does, and build the same world with the same ids.
//!
//! - **Time stands in the menu.** Entering it pauses the simulation in real time (there is no game
//!   yet), leaving it lets it run again, unless it was paused before (`--paused`). A dev's `step`
//!   still runs ticks there.
//! - **The game starts** when something sets `NextState(InGame)`: the `new_game` tool (a menu's
//!   button `tool new_game`) does it inside the call, so the world is spawned when it answers, by the
//!   game's own `OnEnter(InGame)`, as without a menu.
//! - **The recording.** A replay plays straight in: it builds the world at tick 0 and makes the
//!   calls; the menu's presses at tick 0 find no menu there and change nothing. If the menu moved
//!   the world in a way a straight start does not (ticks, a spawn, a region made from the menu),
//!   the recording starts again from the world the game starts on (a snapshot), so a replay of the
//!   session still verifies.

use ai_core::journal::{self, JournalKind};
use ai_core::tools::{register_tool, ToolKind};
use ai_core::{sim, Call, Scope, SimGate, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::EngineState;

/// What entering the main menu found, for leaving it.
#[derive(Resource, Default)]
pub(crate) struct MainMenuClock {
    /// The menu paused the simulation, so leaving it lets it run again.
    paused_it: bool,
    /// The tick and the state hash the menu was entered with.
    entered: Option<(u64, u64)>,
    /// The menu moved the world: the recording starts again once the game stands.
    restart: bool,
}

/// `OnEnter(MainMenu)`: time stands. Coming from loading, the loading gate paused it; the menu
/// takes that pause over, or the gate's release would let ticks run in the menu.
pub(crate) fn enter_menu(world: &mut World) {
    let gate_paused = world.get_resource_mut::<SimGate>().is_some_and(|mut gate| gate.forget_pause());
    let running = !sim::is_paused(world);
    if running {
        sim::pause(world);
    }
    let entered = (sim::tick(world), ai_core::replay::state_hash(world));
    let mut clock = world.resource_mut::<MainMenuClock>();
    clock.paused_it = running || gate_paused;
    clock.entered = Some(entered);
    clock.restart = false;
}

/// `OnExit(MainMenu)`: time runs again if the menu stopped it; whether the menu moved the world.
pub(crate) fn leave_menu(world: &mut World) {
    let (paused_it, entered) = {
        let clock = world.resource::<MainMenuClock>();
        (clock.paused_it, clock.entered)
    };
    let moved = entered.is_some_and(|(tick, hash)| tick != sim::tick(world) || hash != ai_core::replay::state_hash(world));
    let mut clock = world.resource_mut::<MainMenuClock>();
    clock.paused_it = false;
    clock.entered = None;
    clock.restart = moved;
    // A gate that holds now (a region made from the menu still loading) pauses it again at once.
    if paused_it && sim::is_paused(world) {
        sim::resume(world);
    }
}

/// `RunFixedMainLoop`, before the fixed loop (and at the end of [`start_game`]): once the game
/// stands after a menu that moved the world, the recording starts from it.
pub(crate) fn restart_when_due(world: &mut World) {
    if !world.get_resource::<MainMenuClock>().is_some_and(|c| c.restart) {
        return;
    }
    let in_game = world.get_resource::<State<EngineState>>().is_some_and(|s| *s.get() == EngineState::InGame);
    if !in_game || sim::gate_message(world).is_some() {
        return;
    }
    world.resource_mut::<MainMenuClock>().restart = false;
    match ai_core::replay::restart_from_current(world, "the game started from the main menu") {
        Ok(_) => {
            let tick = sim::tick(world);
            journal::log(world, JournalKind::Sim, format!("the recording starts at tick {tick}: the main menu moved the world"));
        }
        Err(why) => {
            journal::emit(world, JournalKind::Warn, format!("the recording did not start again after the main menu ({why}): a replay saved now will not verify"));
        }
    }
}

/// Why [`start_game`] would refuse now: the game is not in its main menu.
pub fn can_start_game(world: &World) -> Result<(), String> {
    match world.get_resource::<State<EngineState>>().map(|s| *s.get()) {
        Some(EngineState::MainMenu) => Ok(()),
        Some(EngineState::InGame) => Err("the game is on already: there is no main menu to leave".to_owned()),
        Some(state) => Err(format!("the game is not in its main menu but in {state:?}: wait with `shards wait`")),
        None => Err("this app has no engine states (CorePlugin)".to_owned()),
    }
}

/// Whether [`start_game`] spawns the game inside the call (not from inside a state transition,
/// where it can only ask for the next frame).
pub fn starts_at_once(world: &World) -> bool {
    world.resource::<Schedules>().contains(StateTransition)
}

/// Leaves the main menu for the game (`MainMenu → InGame`): inside this call when it can, so the
/// game's world is spawned when it returns; called from inside a state transition, at the next
/// frame. Refuses, changing nothing, outside the main menu.
pub fn start_game(world: &mut World) -> Result<String, String> {
    can_start_game(world)?;
    world.resource_mut::<NextState<EngineState>>().set(EngineState::InGame);
    // Inside a transition its schedule is out of the world: the state changes at the next frame.
    if !starts_at_once(world) {
        return Ok("the game starts at the next frame".to_owned());
    }
    let _ = world.try_run_schedule(StateTransition);
    restart_when_due(world);
    Ok(format!("the game starts at tick {}", sim::tick(world)))
}

/// Leave the main menu and play: the game starts the way a run without a menu starts it, its
/// world spawned when this answers. Only in the main menu (a game with one, in a run that shows it:
/// a window, `--offscreen`, `--menu`). `continue_game` plays on from the newest save instead.
#[derive(Deserialize, JsonSchema, Default)]
pub struct NewGameArgs {}

fn new_game(In(_): In<Call<NewGameArgs>>, world: &mut World) -> ToolResult {
    start_game(world).inspect_err(|_| ai_core::replay::forget_call(world))
}

pub(crate) fn register(world: &mut World) {
    register_tool(world, "new_game", Scope::Player, ToolKind::Action, new_game);
}
