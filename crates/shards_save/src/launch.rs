//! Loading the saves a launch names: `--world SLOT` and `--character SLOT`.
//!
//! A single game (`NetLaunch::Off`) loads them once, after the world has spawned and before
//! the first simulated tick: [`SavePlugin`](crate::SavePlugin) runs [`load_launch_slots`] in
//! `RunFixedMainLoop`, ahead of the fixed loop, on the first frame the game is in
//! `EngineState::InGame` with nothing holding the simulation (so a `--paused` game is
//! already loaded when `shards wait` answers, and an `observe` at tick 0 shows the loaded
//! world).
//!
//! The load is a call of the `load_game` tool like any other: it journals as an agent call,
//! reads the files, applies the sections and restarts the recording from the loaded state.
//! A replay of the session therefore starts from a snapshot of that state (a replay cannot
//! redo reading a file), and a replay being played back loads nothing: its own snapshot is
//! the start.
//!
//! A slot with no save is a new game, not an error: the journal says so and the first save
//! creates the slot. A slot that is there but cannot be loaded (every copy damaged, a save
//! from a newer game) is an error in the journal and the game starts fresh; it never
//! overwrites what it could not read (see [`store`](crate::store)).
//!
//! Hosts and clients of a networked game (`--host`, `--join`) load their slots in their own
//! way (the host: the world and its character, a client: its character, sent to the host);
//! the network code calls [`load_launch_slot`] or [`load_launch_slots`] itself.

use ai_core::journal::{self, JournalKind};
use ai_core::{call_tool, AgentSessions, Scope};
use bevy::prelude::*;
use serde_json::json;
use shards_core::{EngineConfig, EngineState, NetLaunch};

use crate::ops::{replay_playing, SaveConfig};
use crate::section::SaveKind;

/// The agent the launch's loads are made as: dev scope, the player's body.
pub const LAUNCH_AGENT: &str = "launch";

/// Whether the launch's slots were handled (loaded or found empty), once per game.
#[derive(Resource, Default, Debug, PartialEq, Eq)]
pub struct LaunchLoaded(pub bool);

/// What loading one launch slot came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchOutcome {
    /// The slot's save is in the world.
    Loaded,
    /// The slot has no save yet: a new game; the first save creates it.
    Fresh,
    /// The slot has a save that could not be loaded; the game starts as it is. The error
    /// is the tool's answer.
    Failed(String),
}

/// One launch slot and what became of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LaunchLoad {
    pub kind: SaveKind,
    pub slot: String,
    pub outcome: LaunchOutcome,
}

/// Loads `kind/slot` as the launch asks (the `load_game` tool, as the [`LAUNCH_AGENT`]),
/// journaling what came of it: a missing slot is a new game, any other failure is an
/// error in the journal and the world stays as it is. For the network code, which has
/// its own idea of which slots a host or a client loads.
pub fn load_launch_slot(world: &mut World, kind: SaveKind, slot: &str) -> LaunchLoad {
    let flag = match kind {
        SaveKind::World => "--world",
        SaveKind::Character => "--character",
    };
    let load = |outcome| LaunchLoad { kind, slot: slot.to_owned(), outcome };
    let Some(config) = world.get_resource::<SaveConfig>() else {
        return load(LaunchOutcome::Failed("the save module is not installed (add SavePlugin)".to_owned()));
    };
    if !config.store().exists(kind, slot) {
        journal::emit(world, JournalKind::Sim, format!("save: {flag} {slot}: no {kind} save in that slot yet, starting a new game (saving creates it)"));
        return load(LaunchOutcome::Fresh);
    }
    let own = world.resource_mut::<AgentSessions>().get_or_create(LAUNCH_AGENT).clone();
    world.resource_mut::<AgentSessions>().configure(LAUNCH_AGENT, Scope::Dev, None);
    let out = call_tool(world, LAUNCH_AGENT, "load_game", json!({"what": kind.as_str(), "slot": slot}));
    world.resource_mut::<AgentSessions>().configure(LAUNCH_AGENT, own.scope, own.subject);
    if out.ok {
        return load(LaunchOutcome::Loaded);
    }
    let why = out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned);
    journal::emit(world, JournalKind::Error, format!("save: {flag} {slot} was not loaded, starting without it: {why}"));
    load(LaunchOutcome::Failed(why))
}

/// Loads the slots `EngineConfig` names (`world_slot`, then `character_slot`), when it
/// names any.
pub fn load_launch_slots(world: &mut World) -> Vec<LaunchLoad> {
    let Some(config) = world.get_resource::<EngineConfig>() else {
        return Vec::new();
    };
    let wanted: Vec<(SaveKind, String)> =
        [(SaveKind::World, config.world_slot.clone()), (SaveKind::Character, config.character_slot.clone())]
            .into_iter()
            .filter_map(|(kind, slot)| Some((kind, slot?)))
            .collect();
    wanted.into_iter().map(|(kind, slot)| load_launch_slot(world, kind, &slot)).collect()
}

/// `RunFixedMainLoop`, before the fixed loop (single games): once the game is in play and
/// nothing holds the simulation, loads the launch's slots. Not while a replay plays.
pub(crate) fn load_at_launch(world: &mut World) {
    if world.resource::<LaunchLoaded>().0 {
        return;
    }
    let in_game = world.get_resource::<State<EngineState>>().is_some_and(|s| *s.get() == EngineState::InGame);
    if !in_game || ai_core::sim::gate_message(world).is_some() {
        return;
    }
    world.resource_mut::<LaunchLoaded>().0 = true;
    if !replay_playing(world) {
        load_launch_slots(world);
    }
}

/// Whether a game launched as `config` loads its slots by itself: a single game does, the
/// network code handles hosts and clients.
pub(crate) fn loads_by_itself(config: &EngineConfig) -> bool {
    config.net == NetLaunch::Off
}
