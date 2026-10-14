//! Exit hooks: what must happen before the game ends, however it ends.
//!
//! A module registers a hook with [`EngineAppExt::add_exit_hook`](crate::EngineAppExt::add_exit_hook)
//! (a host saves the world and tells its clients the session is over). The hooks run once,
//! in registration order, as soon as the game is ending: an `AppExit` was written (a closed
//! window, `--verify` finishing, the game's own quit), seen by a `Last` system before the
//! runner stops, or a termination signal arrived (`shards::run` runs them when it sees
//! one). Whatever ends the game afterwards does not run them again.

use bevy::app::AppExit;
use bevy::prelude::*;

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
