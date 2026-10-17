//! Several apps in one process.
//!
//! Two pieces of ai_core are process-wide by nature: the tracing subscriber that feeds
//! `warn!`/`error!` lines into the journal (a process has one), and Bevy's fallback error
//! handler that queues system failures (a plain `fn`, without the world). An app alone in its
//! process never notices. Several apps in one process do: a host and its clients in an
//! in-process stand, two games in one test. Their log lines and failures must not mix.
//!
//! So every app gets an [`AppToken`] (allocated when [`AiCorePlugin`](crate::AiCorePlugin) is
//! built), and the app working right now says so with [`activate`]. The log layer writes into
//! the active app's buffer, and a failure is queued under the active app's token, which only
//! that app's [`failures::collect`](crate::failures::collect) takes.
//!
//! [`activate`] runs at the start of every frame (`First`), and at the entry of everything a
//! driver may call on an app between frames: [`call_tool`](crate::call_tool),
//! [`sim::step`](crate::sim::step), [`sim::run_tick`](crate::sim::run_tick) and
//! [`failures::collect`](crate::failures::collect).
//!
//! **The contract: the apps of one process update one after the other, never at the same
//! time.** A stand driver activates a member, runs its frame or ticks, collects its failures,
//! then goes on to the next one. Within that, systems may run on any thread: the active token
//! is one atomic. Lines logged by other threads on their own (asset loading on the IO pool)
//! land in whichever app is active at the time; that is harmless.
//!
//! The thread that drives an app (calls `update`, `step`, `call_tool`) also remembers the
//! app it activated last, and lines it logs go there even if another thread activated another
//! app meanwhile: tests running in parallel threads keep their journals apart. Failures need
//! no activation at all: every app's error handler carries its own token (see
//! [`failures::handler_for`](crate::failures::handler_for)).

use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

use bevy::prelude::*;

/// The next token to hand out; 0 means "no app".
static NEXT: AtomicU64 = AtomicU64::new(1);

/// The token of the app that is working right now (0: none yet).
static ACTIVE: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The app this thread activated last (0: none): it wins over [`ACTIVE`] here.
    static LOCAL: Cell<u64> = const { Cell::new(0) };
}

/// This app's identity within the process: what [`activate`] makes current, and what log lines
/// and failures are tagged with.
#[derive(Resource, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AppToken(pub u64);

/// The app's token, allocated on first use (the log layer may ask before
/// [`AiCorePlugin`](crate::AiCorePlugin) is built).
pub fn ensure_token(world: &mut World) -> u64 {
    if let Some(token) = world.get_resource::<AppToken>() {
        return token.0;
    }
    let token = NEXT.fetch_add(1, Ordering::Relaxed);
    world.insert_resource(AppToken(token));
    token
}

/// Makes this world's app the active one: what is logged or fails from now on is its own.
/// Nothing for a world without a token (no `AiCorePlugin`).
pub fn activate(world: &World) {
    if let Some(token) = world.get_resource::<AppToken>() {
        ACTIVE.store(token.0, Ordering::Relaxed);
        LOCAL.with(|local| local.set(token.0));
    }
}

/// The token of the active app (0: none activated yet): the one this thread activated last,
/// or else the one activated last in the process.
pub fn active() -> u64 {
    match LOCAL.with(Cell::get) {
        0 => ACTIVE.load(Ordering::Relaxed),
        local => local,
    }
}

/// `First`, before the frame does anything: this app is the one working now. Exclusive, so
/// it runs on the thread that drives the app.
pub fn activate_app(world: &mut World) {
    activate(world);
}

thread_local! {
    /// Set while this thread builds an app beside the active one (heavy mode's twin): it has
    /// no token yet, and its lines must not land in the active app's journal.
    static MUTED: Cell<u32> = const { Cell::new(0) };
}

/// Lines this thread logs go into no app's journal while the returned guard lives: for
/// building an app beside the active one, before it has a token of its own ([`activate`] it
/// after). Other threads (other tests of the process) are not affected.
pub fn mute() -> Muted {
    MUTED.with(|m| m.set(m.get() + 1));
    Muted(())
}

/// See [`mute`].
pub struct Muted(());

impl Drop for Muted {
    fn drop(&mut self) {
        MUTED.with(|m| m.set(m.get().saturating_sub(1)));
    }
}

/// Whether lines this thread logs go nowhere right now (see [`mute`]).
pub(crate) fn muted() -> bool {
    MUTED.with(Cell::get) > 0
}

thread_local! {
    static QUIET: Cell<u32> = const { Cell::new(0) };
}

/// The engine's own journal lines ([`emit`](crate::journal::emit)) this thread writes stay off
/// the console while the returned guard lives; they still reach the journal. For an app that
/// plays beside the one at the terminal, like heavy mode's twin.
pub fn quiet() -> Quiet {
    QUIET.with(|q| q.set(q.get() + 1));
    Quiet(())
}

/// See [`quiet`].
pub struct Quiet(());

impl Drop for Quiet {
    fn drop(&mut self) {
        QUIET.with(|q| q.set(q.get().saturating_sub(1)));
    }
}

/// Whether [`quiet`] holds on this thread.
pub(crate) fn is_quiet() -> bool {
    QUIET.with(Cell::get) > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_app_gets_its_own_token_and_activation_switches_between_them() {
        let (mut a, mut b) = (World::new(), World::new());
        let (ta, tb) = (ensure_token(&mut a), ensure_token(&mut b));
        assert_ne!(ta, tb);
        assert_eq!(ensure_token(&mut a), ta, "allocated once");
        activate(&b);
        assert_eq!(active(), tb);
        activate(&a);
        assert_eq!(active(), ta);
        // A world without a token changes nothing.
        activate(&World::new());
        assert_eq!(active(), ta);
    }

    #[derive(Resource, Default)]
    struct Explode(bool);

    fn maybe_explode(explode: Res<Explode>) {
        assert!(!explode.0, "boom");
    }

    fn game() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin)
            .add_plugins(crate::AiCorePlugin {
                remote_port: None,
                crash_replays: false,
                sim: crate::SimConfig { start_paused: true, ..Default::default() },
                ..Default::default()
            })
            .init_resource::<Explode>()
            .add_systems(FixedUpdate, maybe_explode.in_set(crate::SimSet::Resolve));
        crate::journal::register_capture(&mut app);
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    fn texts(app: &App) -> Vec<String> {
        let (entries, _) = app.world().resource::<crate::AiJournal>().after(0);
        entries.map(|e| e.text.clone()).collect()
    }

    #[test]
    fn two_apps_in_one_process_keep_their_failures_and_log_lines_apart() {
        use bevy::log::tracing_subscriber::layer::SubscriberExt;
        use std::time::Duration;

        let (mut a, mut b) = (game(), game());
        b.world_mut().resource_mut::<Explode>().0 = true;
        let report = crate::sim::step(b.world_mut(), 3, Duration::from_secs(5), |_| None).unwrap();
        let crate::sim::StepEnd::SystemFailed(failed) = report.end else { panic!("{:?}", report.end) };
        assert!(failed[0].name.contains("maybe_explode"), "{failed:?}");
        assert!(texts(&b).iter().any(|t| t.contains("maybe_explode") && t.contains("boom")), "{:?}", texts(&b));

        // A runs on: B's failure is not its own.
        assert!(crate::failures::collect(a.world_mut()).is_empty());
        let report = crate::sim::step(a.world_mut(), 3, Duration::from_secs(5), |_| None).unwrap();
        assert_eq!(report.end, crate::sim::StepEnd::Completed);
        assert!(!texts(&a).iter().any(|t| t.contains("boom")), "{:?}", texts(&a));

        // The one capture layer of the process writes into the active app's journal.
        let mut layer_app = App::new();
        let layer = crate::journal::capture_log_layer(&mut layer_app).unwrap();
        let subscriber = bevy::log::tracing_subscriber::registry().with(layer);
        bevy::log::tracing::subscriber::with_default(subscriber, || {
            activate(b.world());
            warn!("only for B");
            activate(a.world());
            warn!("only for A");
        });
        crate::journal::drain_logs(a.world_mut());
        crate::journal::drain_logs(b.world_mut());
        let (ta, tb) = (texts(&a), texts(&b));
        assert!(tb.iter().any(|t| t.ends_with("only for B")) && !tb.iter().any(|t| t.ends_with("only for A")), "{tb:?}");
        assert!(ta.iter().any(|t| t.ends_with("only for A")) && !ta.iter().any(|t| t.ends_with("only for B")), "{ta:?}");
    }
}
