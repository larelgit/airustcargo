//! Exit hooks: what must happen before the game ends, however it ends.
//!
//! A module registers a hook with [`EngineAppExt::add_exit_hook`](crate::EngineAppExt::add_exit_hook)
//! (a host saves the world and tells its clients the session is over). The hooks run once,
//! in registration order, as soon as the game is ending: an `AppExit` was written (a closed
//! window, `--verify` finishing, the game's own quit), seen by a `Last` system before the
//! runner stops, or a termination signal arrived (`shards::run` runs them when it sees
//! one). Whatever ends the game afterwards does not run them again.
//!
//! The `quit` tool ends the game that way from outside, on every platform: what `kill` (SIGTERM)
//! does on unix, where Windows has no signal to send another console's process.

use std::time::{Duration, Instant};

use ai_core::tools::{register_tool, ToolKind};
use ai_core::{Call, Scope, ToolResult};
use bevy::app::AppExit;
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;

/// How long after `quit` answers the game ends: the answer gets out first.
const QUIT_AFTER: Duration = Duration::from_millis(200);

/// A `quit` that was asked for: the game ends at this moment.
#[derive(Resource)]
pub(crate) struct Quitting(Instant);

/// End the game the way closing its window or Ctrl+C does: the exit hooks run (a host ends its
/// session, settings are written, the BRP token and `--port-file` go) and the process exits with
/// code 0 a moment after this answer. A client of an in-process stand (`--stand`) cannot: the
/// stand's process ends with its host. `shards quit` waits until the game answers no more.
#[derive(Deserialize, JsonSchema, Default)]
pub struct QuitArgs {}

fn quit_tool(In(_): In<Call<QuitArgs>>, world: &mut World) -> ToolResult {
    if world.get_resource::<crate::EngineConfig>().is_some_and(|c| c.embedded) {
        return Err("this game runs inside its stand's process (`--stand`), which ends with its host: `shards quit` the host, or `shards stand stop`".into());
    }
    world.insert_resource(Quitting(Instant::now() + QUIT_AFTER));
    Ok(format!("quitting: the game ends in {:.1}s, after its exit hooks", QUIT_AFTER.as_secs_f32()))
}

pub(crate) fn register(world: &mut World) {
    register_tool(world, "quit", Scope::Dev, ToolKind::Control, quit_tool);
}

/// `First`: a `quit` whose time has come writes the `AppExit` (the hooks run in `Last`).
pub(crate) fn quit_when_due(world: &mut World) {
    if world.get_resource::<Quitting>().is_some_and(|q| Instant::now() >= q.0) {
        world.remove_resource::<Quitting>();
        world.write_message(AppExit::Success);
    }
}

/// A hook: it gets the whole world, the last time anything does.
pub type ExitHook = fn(&mut World);

/// The registered exit hooks, and whether they ran.
#[derive(Resource, Default)]
pub struct ExitHooks {
    hooks: Vec<(String, ExitHook)>,
    ran: bool,
}

impl ExitHooks {
    /// Adds a hook after the others.
    pub fn add(&mut self, name: &str, hook: ExitHook) {
        self.hooks.push((name.to_owned(), hook));
    }

    /// Hook names, in the order they run.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.hooks.iter().map(|(name, _)| name.as_str())
    }

    /// Whether the hooks ran (they run once).
    pub fn ran(&self) -> bool {
        self.ran
    }
}

/// Runs the exit hooks, once: a second call (the game ends twice over: a signal, then the
/// `AppExit` it wrote) does nothing.
pub fn run_exit_hooks(world: &mut World) {
    let hooks = {
        let mut registered = world.get_resource_or_init::<ExitHooks>();
        if registered.ran {
            return;
        }
        registered.ran = true;
        registered.hooks.clone()
    };
    for (name, hook) in hooks {
        debug!("exit hook {name}");
        hook(world);
    }
}

/// Run condition of [`run_exit_hooks`] in `Last`: an `AppExit` was written, so the runner
/// stops after this frame.
pub(crate) fn exiting(exits: Option<Res<Messages<AppExit>>>, hooks: Option<Res<ExitHooks>>) -> bool {
    exits.is_some_and(|m| !m.is_empty()) && hooks.is_some_and(|h| !h.ran)
}
