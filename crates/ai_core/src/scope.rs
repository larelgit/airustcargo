//! The simulation's scope: whether what runs now is the simulation itself.
//!
//! A replay repeats the simulation, not everything a game does: the fixed ticks, the recorded
//! action calls and the world they build run again, tick for tick; frames, a window, the agents'
//! queries and what only watches do not. A module that records what the simulation *reads* (the
//! content database: the definitions a recording depends on, for its replay check) must count
//! only reads the playback makes too, or a recording and its playback, headless or drawn, would
//! disagree about what was read. [`SimScope`] says when a read counts:
//!
//! - **open** while a fixed tick runs (`FixedMain`, around Bevy's `run_fixed_main`), while an
//!   `action` tool call runs (live calls run between ticks, recorded ones inside a tick), and
//!   wherever a module builds the world (the engine's state transitions, the start region, a
//!   region change, a prefab or a level spawned): [`open`];
//! - **suspended** inside that for what only watches the world: invariants, coverage, replay and
//!   snapshot hashing, a snapshot load putting a state back: [`suspend`];
//! - **closed** for everything else: frames (`Update`, `PostUpdate`, the render world), query and
//!   control tools, validation.
//!
//! Each world has its own (a heavy-mode twin and a stand's players are other worlds in the same
//! process), and a world runs one schedule at a time, so the counters are plain atomics that
//! systems running in parallel inside a tick may read.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use bevy::app::FixedMain;
use bevy::prelude::*;

#[derive(Default)]
struct Counters {
    /// Scopes open now (they nest: a recorded call inside a tick, a region change inside a call).
    open: AtomicU32,
    /// Suspensions inside them.
    suspended: AtomicU32,
}

/// Whether what runs now is the simulation's own work (see the [module](self)). Cheap to clone:
/// a handle on the world's counters, which a module keeps where it has no world (the content
/// database checks it on every read).
#[derive(Resource, Clone, Default)]
pub struct SimScope(Arc<Counters>);

impl SimScope {
    /// The scope is open and nothing suspends it: a read now is the simulation's.
    #[inline]
    pub fn is_open(&self) -> bool {
        self.0.open.load(Ordering::Relaxed) > 0 && self.0.suspended.load(Ordering::Relaxed) == 0
    }

    /// Opens the scope until the guard drops.
    pub fn open(&self) -> ScopeOpen {
        self.0.open.fetch_add(1, Ordering::Relaxed);
        ScopeOpen(self.clone())
    }

    /// Suspends the scope until the guard drops: for code inside it that only watches.
    pub fn suspend(&self) -> ScopeSuspended {
        self.0.suspended.fetch_add(1, Ordering::Relaxed);
        ScopeSuspended(self.clone())
    }

    /// Whether `other` is a handle on the same counters.
    pub fn same(&self, other: &SimScope) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }

    fn enter(&self) {
        self.0.open.fetch_add(1, Ordering::Relaxed);
    }

    fn leave(&self) {
        let _ = self.0.open.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
    }
}

/// The scope [`SimScope::open`] opened, until it drops.
#[must_use = "the scope closes when the guard drops"]
pub struct ScopeOpen(SimScope);

impl Drop for ScopeOpen {
    fn drop(&mut self) {
        self.0.leave();
    }
}

/// The suspension [`SimScope::suspend`] made, until it drops.
#[must_use = "the suspension ends when the guard drops"]
pub struct ScopeSuspended(SimScope);

impl Drop for ScopeSuspended {
    fn drop(&mut self) {
        let _ = self.0 .0.suspended.try_update(Ordering::Relaxed, Ordering::Relaxed, |n| n.checked_sub(1));
    }
}

/// Opens `world`'s scope until the guard drops: for a module that builds the simulation's world
/// outside a tick or an action call (the start region while the game loads). `None` for a world
/// without one (no `AiCorePlugin`).
pub fn open(world: &World) -> Option<ScopeOpen> {
    world.get_resource::<SimScope>().map(SimScope::open)
}

/// Suspends `world`'s scope until the guard drops: for code that runs inside a tick or a call
/// and only watches (a rule, a hash). `None` for a world without one.
pub fn suspend(world: &World) -> Option<ScopeSuspended> {
    world.get_resource::<SimScope>().map(SimScope::suspend)
}

/// Whether `world`'s scope is open now.
pub fn is_open(world: &World) -> bool {
    world.get_resource::<SimScope>().is_some_and(SimScope::is_open)
}

/// `FixedMain`, before the fixed schedules run: a tick is the simulation.
fn open_tick(scope: Res<SimScope>) {
    scope.enter();
}

/// `FixedMain`, after the fixed schedules ran.
fn close_tick(scope: Res<SimScope>) {
    scope.leave();
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<SimScope>().add_systems(
        FixedMain,
        (open_tick.before(FixedMain::run_fixed_main), close_tick.after(FixedMain::run_fixed_main)),
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scopes_nest_and_a_suspension_closes_them_until_it_ends() {
        let scope = SimScope::default();
        assert!(!scope.is_open());
        let outer = scope.open();
        let inner = scope.open();
        assert!(scope.is_open());
        drop(inner);
        assert!(scope.is_open(), "the outer scope is still open");
        let quiet = scope.suspend();
        assert!(!scope.is_open());
        drop(quiet);
        assert!(scope.is_open());
        drop(outer);
        assert!(!scope.is_open());
        // An unbalanced close never wraps the counter.
        scope.leave();
        assert!(!scope.is_open());
        let again = scope.clone().open();
        assert!(scope.is_open() && scope.same(&again.0));
    }

    #[derive(Resource, Default)]
    struct Seen(Vec<(&'static str, bool)>);

    #[test]
    fn a_tick_is_open_and_a_frame_is_not() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).init_resource::<Seen>();
        plugin(&mut app);
        app.add_systems(FixedUpdate, |scope: Res<SimScope>, mut seen: ResMut<Seen>| seen.0.push(("tick", scope.is_open())));
        app.add_systems(Update, |scope: Res<SimScope>, mut seen: ResMut<Seen>| seen.0.push(("frame", scope.is_open())));
        crate::sim::run_tick(app.world_mut()).unwrap();
        app.update();
        let seen = &app.world().resource::<Seen>().0;
        assert!(seen.contains(&("tick", true)), "{seen:?}");
        assert!(seen.iter().filter(|(what, _)| *what == "frame").all(|(_, open)| !open), "{seen:?}");
        assert!(!app.world().resource::<SimScope>().is_open(), "closed after the tick");
    }
}
