//! # ai_core — the AI integration layer
//!
//! The AI does not look at the screen like a human. The engine already knows
//! everything in the frame, so it describes the world as structured text and offers
//! a typed API for actions; screenshots are only a side channel for visual bugs.
//!
//! There is no LLM inside the engine. The agents are external coding agents
//! (Claude Code, Codex, …) that drive a running game from a shell through the
//! `shards` CLI, over the `ai/*` methods on localhost.
//!
//! ```text
//! intent sources:  player · network client · agent (via shards) · replay
//!                          │
//! gameplay:        FixedUpdate, fixed tick, seed       (SimSet order, SimRng)
//!                          │
//! ai_core:         Observe  — world slice as text      (observe, inspect, nearby)
//!                  Journal  — what happened since       (journal_event, warn!/error!)
//!                  Tools    — typed actions             (add_ai_tool, Scope)
//!                  SimCtl   — pause · step · run-until  (sim)
//!                  Rules    — invariants after a tick   (invariants, add_ai_invariant)
//!                  Replay   — record inputs, play back  (replay)
//!                  Snapshot — save / load the state     (snapshot)
//!                  Perf     — real time of every tick   (perf)
//!                          │
//! transport:       BRP `ai/*` on localhost → `shards` CLI ← coding agents, scripts, CI
//! ```
//!
//! Setup:
//!
//! ```ignore
//! App::new()
//!     .add_plugins(DefaultPlugins.set(LogPlugin { custom_layer: ai_core::capture_log_layer, ..default() }))
//!     .add_plugins(AiCorePlugin::default())
//!     .register_ai_component::<Health>(|h| format!("hp {:.0}/{:.0}", h.current, h.max))
//!     .journal_event::<Damaged>(|e, w| Some(format!("{} -> {} -{}", label(w, e.source), label(w, e.target), e.amount)))
//!     .add_ai_tool("move_to", Scope::Player, move_to)
//!     .add_ai_invariant("health_in_range", health_in_range)
//!     .replay_input::<Intent>()
//!     .replay_ignore::<Sparkle>();
//! ```

pub mod builtin;
pub mod crash;
pub mod describe;
pub mod dev;
pub mod display;
#[cfg(test)]
mod determinism;
pub mod failures;
pub mod heavy;
#[cfg(feature = "render")]
pub mod highlight;
pub mod id;
pub mod invariants;
pub mod journal;
pub mod net_rules;
pub mod observe;
pub mod paths;
pub mod perf;
pub mod process;
#[cfg(feature = "remote")]
pub mod remote;
pub mod replay;
pub mod rng;
pub mod scenario;
pub mod session;
#[cfg(feature = "render")]
pub mod shot;
pub mod sim;
pub mod snapshot;
pub mod spatial;
pub mod state;
pub mod status;
pub mod tools;

use std::sync::Arc;
use std::time::Duration;

use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};
use bevy::prelude::*;
use bevy::reflect::{FromReflect, GetTypeRegistration, TypePath};
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;

pub use builtin::{AiConditions, StopWhen};
pub use describe::{
    AdviceFn, AiAdvisors, AiDescribers, AiKind, AiObserveLines, AiObserveSections, Describer, EntityFacts, ObserveLinesFn, ObserveSectionFn,
};
pub use dev::{DevCommand, DevEntry, DevLine, DevMenu};
pub use failures::{ai_core_error_handler, AiFailures, SystemFailure};
pub use id::{label, AiId, AiImportance, AiIndex, AiLandmark, AiPlayer, AiVisible};
pub use heavy::{HeavyConfig, HeavyFailure, HeavyTwin};
pub use invariants::{AiInvariants, InvariantSystems, Violation};
pub use journal::{capture_log_layer, AiJournal, JournalEntry, JournalKind};
pub use net_rules::{replicated_components, rules_hash, NetRules};
pub use observe::{AiCamera, AiPerception, ObserveOptions};
pub use perf::{AiPerf, TickStats};
pub use process::{activate, AppToken};
pub use replay::{
    AiReplay, Breakdown, CheckDetail, Divergence, EntityChange, EntityDiff, Replay, ReplayHashing, ReplayReport,
};
pub use rng::{DetRng, SimRng, SimSeed};
pub use session::{configure_session, AgentSession, AgentSessions, Caller, PlayerResolver, Scope, SessionChange};
pub use sim::{Halt, SimConfig, SimGate, SimSet, SimTick, StepQueue, StepRoute, TickClock};
pub use snapshot::{world_rebuilt, EntityKey, Snapshot, SnapshotRegistry, WorldRebuilt};
pub use spatial::{AiLineOfSight, AiOccluder};
pub use status::AiStatusFields;
pub use tools::{
    call_tool, AiTools, Call, CallOutcome, ForwardedCall, Forwarder, Id, NetRoute, NoArgs, ToolKind, ToolResult, ToolSpec,
};

pub mod prelude {
    pub use crate::{
        label, AiAppExt, AiCamera, AiCorePlugin, AiId, AiImportance, AiKind, AiLandmark, AiOccluder, AiPlayer, AiVisible, Call,
        Caller, DetRng, Id, NoArgs, Scope, SimConfig, SimRng, SimSet, SimTick, ToolResult,
    };
}

/// Default BRP port (same as Bevy's).
pub const DEFAULT_PORT: u16 = 15702;

pub struct AiCorePlugin {
    pub sim: SimConfig,
    pub perception: AiPerception,
    /// Port for the `ai/*` BRP methods (`remote` feature). `None`: no HTTP server.
    /// Every BRP method needs the token the game writes to `.shards/<port>.token`.
    pub remote_port: Option<u16>,
    /// Install [`ai_core_error_handler`]: a gameplay panic is logged, stops `step` at its
    /// tick and pauses the game, instead of panicking again and taking `FixedMain` and
    /// `FixedUpdate` with it. `false` keeps Bevy's own handler.
    pub catch_system_panics: bool,
    /// After a gameplay failure, save the session as `replays/crash-tick<N>.json` to
    /// reproduce it with, and a crash report next to it in `crashes/` ([`crash`]); after a
    /// violated invariant, as `replays/invariant-<name>-tick<N>.json`. A panic nobody handled
    /// (another thread) gets a crash report too. `false` switches off what the game writes by
    /// itself; the `crash_report` tool, a bug report an agent asks for, writes its file whatever
    /// this says (with its `path` in `crashes/`, over the same file each time).
    pub crash_replays: bool,
    /// Ticks between replay checks ([`replay::CHECK_EVERY`] by default), stored in every
    /// replay saved. Record with 1 to have a divergence named at its exact tick.
    pub check_every: u64,
    /// What each replay check keeps next to its hash, to name what diverged.
    pub check_detail: CheckDetail,
    /// A tick whose `FixedMain` run takes longer is over budget. `None`: the tick's own
    /// duration (16.7 ms at 60 Hz).
    pub tick_budget: Option<Duration>,
    /// Report tick timing: a `ticks:` line in every `step` reply and `perf` journal
    /// entries for ticks over budget. Timing is wall-clock, so tests that compare whole
    /// journals turn it off; `ai/status` has it either way.
    pub report_perf: bool,
    /// The game a crash report names (title, version).
    pub game: crash::GameInfo,
    /// Heavy mode ([`heavy`]): every few ticks, a fresh game loaded from a snapshot of this one
    /// must continue like it. `None` (the default): off, and nothing of it exists.
    pub heavy: Option<heavy::HeavyConfig>,
    /// A failed heavy check saves its snapshots and the session's replay (off in tests).
    pub heavy_files: bool,
}

impl Default for AiCorePlugin {
    fn default() -> Self {
        Self {
            sim: SimConfig::default(),
            perception: AiPerception::default(),
            remote_port: Some(DEFAULT_PORT),
            catch_system_panics: true,
            crash_replays: true,
            check_every: replay::CHECK_EVERY,
            check_detail: CheckDetail::default(),
            tick_budget: None,
            report_perf: true,
            game: crash::GameInfo::default(),
            heavy: None,
            heavy_files: true,
        }
    }
}

impl Plugin for AiCorePlugin {
    fn build(&self, app: &mut App) {
        process::ensure_token(app.world_mut());
        app.insert_resource(self.sim.clone())
            .insert_resource(Time::<Fixed>::from_hz(self.sim.tick_hz))
            .insert_resource(SimSeed(self.sim.seed))
            .insert_resource(self.perception.clone())
            .init_resource::<SimTick>()
            .init_resource::<sim::TickClock>()
            .init_resource::<AiIndex>()
            .init_resource::<AiJournal>()
            .init_resource::<AiDescribers>()
            .init_resource::<DevMenu>()
            .init_resource::<AiAdvisors>()
            .init_resource::<AiTools>()
            .init_resource::<AgentSessions>()
            .init_resource::<AiLineOfSight>()
            .init_resource::<AiConditions>()
            .init_resource::<SimGate>()
            .init_resource::<AiStatusFields>()
            .init_resource::<NetRules>()
            // Which body is "mine" is each machine's own business.
            .net_ignore::<AiPlayer>()
            .configure_sets(
                FixedUpdate,
                (SimSet::Input, SimSet::Think, SimSet::Act, SimSet::Move, SimSet::Resolve, SimSet::Cleanup).chain(),
            )
            .add_systems(FixedFirst, sim::advance_tick)
            // Before this frame's time is counted: a held gate means no tick is due.
            .add_systems(First, sim::apply_gate.before(bevy::time::TimeSystems))
            // Several apps in one process: this one is working now (its log lines and
            // failures are its own).
            .add_systems(First, process::activate_app.before(sim::apply_gate))
            // Log lines into the journal after every tick (a long `step` runs many in
            // one frame), and once per frame for lines logged between ticks.
            .add_systems(FixedLast, journal::drain_logs.after(replay::ReplaySystems::Remember))
            .add_systems(Last, journal::drain_logs)
            // Determinism needs an explicit order: warn (into the journal) about gameplay
            // systems that touch the same data with no ordering between them.
            .edit_schedule(FixedUpdate, |schedule| {
                let settings = ScheduleBuildSettings { ambiguity_detection: LogLevel::Warn, ..schedule.get_build_settings() };
                schedule.set_build_settings(settings);
            });

        if self.sim.start_paused {
            app.add_systems(PreStartup, |mut time: ResMut<Time<Virtual>>| time.pause());
        }

        builtin::register(app.world_mut());
        dev::register(app);
        replay::plugin(app, self.check_every, self.check_detail);
        snapshot::plugin(app);
        perf::plugin(app, self.tick_budget, self.report_perf);
        failures::plugin(app, self.catch_system_panics, self.crash_replays);
        crash::plugin(app, self.game.clone());
        invariants::plugin(app, self.crash_replays);
        heavy::plugin(app, self.heavy, self.heavy_files);

        #[cfg(feature = "render")]
        {
            highlight::plugin(app);
            shot::plugin(app);
        }

        #[cfg(feature = "remote")]
        if let Some(port) = self.remote_port {
            if !app.is_plugin_added::<bevy_remote::RemotePlugin>() {
                app.add_plugins(bevy_remote::RemotePlugin::default());
            }
            let token = remote::AiToken::random();
            // Games should refuse to start on a busy port (see `remote::ensure_port_free`);
            // this is the last word for those that do not. The token file of the game
            // that holds the port stays untouched.
            match remote::ensure_port_free(port) {
                Ok(()) => match remote::TokenFile::write(&remote::token_path(port), &token) {
                    Ok(file) => {
                        app.insert_resource(file);
                    }
                    Err(e) => error!("ai_core: {e}; `shards` cannot authenticate to this game"),
                },
                Err(e) => error!("ai_core: {e}; `shards` will not reach this game"),
            }
            app.add_plugins(bevy_remote::http::RemoteHttpPlugin::default().with_port(port))
                .insert_resource(remote::AiInstance::new(Some(port)))
                .insert_resource(token)
                .add_systems(
                    bevy_remote::RemoteLast,
                    remote::protect_new_methods.before(bevy_remote::RemoteSystems::ProcessRequests),
                );
        }
    }

    fn finish(&self, app: &mut App) {
        #[cfg(feature = "remote")]
        if self.remote_port.is_some() {
            remote::register_methods(app.world_mut());
            remote::protect_methods(app.world_mut());
        }
        let _ = app;
    }
}

/// Registration API for games.
pub trait AiAppExt {
    /// Exposes component `C` to the AI as a short fragment of the entity's line,
    /// e.g. `|h: &Health| format!("hp {:.0}/{:.0}", h.current, h.max)`.
    /// Return an empty string to show nothing for this value.
    fn register_ai_component<C: Component>(&mut self, describe: fn(&C) -> String) -> &mut Self;

    /// A fragment computed from the whole entity (several components, conditions).
    fn register_ai_describer(&mut self, key: &'static str, describe: fn(EntityRef) -> Option<String>) -> &mut Self;

    /// Advice for the subject, computed from the whole world: each line shows in
    /// `observe` as `ALERT <line>` right under `YOU` (`in hit zone of e6 slam (lands in
    /// 0.32s): move 2.1m S to leave it`). Return nothing when there is nothing to say.
    fn register_ai_advice(&mut self, advise: describe::AdviceFn) -> &mut Self;

    /// Lines about where the subject is, printed under `YOU` as they are (`TILE (3, 4)
    /// road_bend rot 90`): for modules whose state is about places, not entities.
    fn register_ai_observe_lines(&mut self, lines: describe::ObserveLinesFn) -> &mut Self;

    /// Lines about the game that are no subject's, printed in `observe` after `LANDMARKS` as they
    /// are (`UI  pause (on top, pauses the game)`), whether or not the agent has a subject.
    fn register_ai_observe_section(&mut self, section: describe::ObserveSectionFn) -> &mut Self;

    /// An action tool: journaled, returns the list of changes it caused.
    fn add_ai_tool<A, O, M, S>(&mut self, name: &str, scope: Scope, system: S) -> &mut Self
    where
        A: DeserializeOwned + JsonSchema + Send + Sync + 'static,
        O: Serialize + 'static,
        S: IntoSystem<In<Call<A>>, ToolResult<O>, M> + Clone + Send + Sync + 'static;

    /// A read-only tool: no journal entry, no diff.
    fn add_ai_query<A, O, M, S>(&mut self, name: &str, scope: Scope, system: S) -> &mut Self
    where
        A: DeserializeOwned + JsonSchema + Send + Sync + 'static,
        O: Serialize + 'static,
        S: IntoSystem<In<Call<A>>, ToolResult<O>, M> + Clone + Send + Sync + 'static;

    /// Writes a journal line whenever event `E` is triggered. `None` skips it.
    fn journal_event<E: Event>(&mut self, format: journal::EventFormatter<E>) -> &mut Self;

    /// A named stop condition for `step {until: [{"named": ...}]}`.
    fn add_ai_condition(&mut self, name: &str, condition: fn(&World) -> bool) -> &mut Self;

    /// A rule the game must keep, checked after every tick: `rule` returns
    /// `Some(explanation)` while it is broken (`e5 outside the arena: x = 31.2`), and the
    /// game halts the way it does after a gameplay failure (see [`invariants`]). Names use
    /// letters, digits, `_` and `-` (panics otherwise); the same name replaces a rule.
    fn add_ai_invariant(&mut self, name: &str, rule: invariants::Invariant) -> &mut Self;

    /// Records component `C` in replays: the per-entity input everything writes
    /// (keyboard, network, agents) and gameplay only reads, like the sandbox's `Intent`.
    /// Changes made between ticks are stored; playback sets them back at the same tick.
    fn replay_input<C>(&mut self) -> &mut Self
    where
        C: Component<Mutability = bevy::ecs::component::Mutable> + Clone + PartialEq + Serialize + DeserializeOwned;

    /// Leaves component `C` out of replay checks: cosmetics and caches, whatever may
    /// differ between a recording and its playback without the game being different.
    fn replay_ignore<C: Component>(&mut self) -> &mut Self;

    /// Adds `check(world)` to every replay check, for game data outside AI-visible
    /// entities (projectiles without ids, resources). A second check with the same
    /// name replaces the first.
    fn add_replay_check(&mut self, name: &str, check: fn(&World) -> u64) -> &mut Self;

    /// [`add_replay_check`](Self::add_replay_check) for state that may be absent: while
    /// `check` returns `None` it adds nothing to the hash, not even its name, so a game hashes
    /// the same whether or not that state ever existed (the level module's `level`: a game
    /// without placed levels hashes as one without the module).
    fn add_optional_replay_check(&mut self, name: &str, check: fn(&World) -> Option<u64>) -> &mut Self;

    /// Runs `hook` when a playback is armed ([`replay::play`]), before its first frame or tick:
    /// a module asks for what the recording will need and the game has not loaded (the files its
    /// content edits name, [`AiReplay::playing`]) and holds [`SimGate`] under the name of its
    /// loading requirement while they load. The start snapshot goes in, and the first tick runs,
    /// once the gate lets go, also in a game that had loaded before the playback was armed.
    fn add_playback_hook(&mut self, hook: fn(&mut World)) -> &mut Self;

    /// Adds field `key` to `ai/status` (`shards status`), computed on every call, e.g.
    /// the engine's version and modules. A second field with the same key replaces
    /// the first.
    fn add_ai_status(&mut self, key: &str, field: status::StatusField) -> &mut Self;

    /// An entry of the developer menu (`shards_dev`): `label` in section `group`, running a
    /// tool call written as `shards act` takes it ([`DevCommand`]). A module adds the entries of
    /// its own tools; the same `id` again replaces the entry.
    fn add_dev_entry(&mut self, group: &str, id: &str, label: &str, command: DevCommand) -> &mut Self;

    /// Saves resource `R` in snapshots and puts it back on load: for state that lives
    /// outside entities with an `AiId` and decides what a tick does (a counter, a timer).
    /// `R` reflects (`#[derive(Reflect)]`, `#[reflect(Resource)]`).
    fn add_snapshot_resource<R>(&mut self) -> &mut Self
    where
        R: Resource + Reflect + FromReflect + GetTypeRegistration + TypePath;

    /// What the world is built around and a snapshot does not hold, as a hash: loading
    /// refuses where it differs from the snapshot's, with `hint` (what to do about it).
    /// The content database and the static colliders are checked this way. A second check
    /// with the same name replaces the first.
    fn add_snapshot_check(&mut self, name: &'static str, hint: &'static str, check: fn(&World) -> u64) -> &mut Self;

    /// Runs after a snapshot was loaded, to make a cache again that is derived from
    /// components and resources (a spatial index). A second one with the same name
    /// replaces the first.
    fn add_snapshot_rebuild(&mut self, name: &'static str, rebuild: fn(&mut World)) -> &mut Self;

    /// Runs before a snapshot is checked against this game, to build again the part of the
    /// world the module makes from one of its saved resources when it is not what the
    /// snapshot holds (a generated region, from the manifest it saved): the registered checks
    /// named in `covers` (`static_colliders`, `levels`) run after it. `prepare` returns a note
    /// when it changed the world, `None` when it did not need to. A load refused after the
    /// prepares ran, or by one that returned an error half way, puts the world back as it was
    /// (it runs the prepares again with the world taken before them, so `prepare` must also
    /// build what that world had, or take away what it did not have). A second one with the
    /// same name replaces the first.
    fn add_snapshot_prepare(&mut self, name: &'static str, covers: &'static [&'static str], prepare: snapshot::PrepareFn) -> &mut Self;

    /// [`add_snapshot_prepare`](Self::add_snapshot_prepare) that runs right after prepare `after`,
    /// wherever the two were registered: what a module builds into the world another module's
    /// prepare builds first (the levels placed by hand stand in the region the snapshot puts
    /// back). Without a prepare `after`, it runs where it was registered.
    fn add_snapshot_prepare_after(
        &mut self,
        name: &'static str,
        after: &'static str,
        covers: &'static [&'static str],
        prepare: snapshot::PrepareFn,
    ) -> &mut Self;

    /// How references to entities without an `AiId` are written in snapshots (see
    /// [`EntityKey`]).
    fn add_snapshot_entity_key(&mut self, key: EntityKey) -> &mut Self;

    /// Keeps component `C` on each machine: a host does not send it to clients (see
    /// [`net_rules`]). For what says "this machine" (`AiPlayer`, the local body's marker) and
    /// bookkeeping each machine does for itself.
    fn net_ignore<C: Component>(&mut self) -> &mut Self;

    /// Keeps snapshot resource `R` on each machine: a host does not send it (id counters).
    fn net_ignore_resource<R: Resource>(&mut self) -> &mut Self;

    /// A client predicts component `C` for its own body: it travels with the input
    /// acknowledgement and is corrected from there, not overwritten with the rest.
    fn net_predicted<C: Component>(&mut self) -> &mut Self;

    /// Where tool `name` (registered already) runs on a client of a networked game; the
    /// default is [`NetRoute::default_for`] its kind and scope.
    fn net_route(&mut self, name: &str, route: NetRoute) -> &mut Self;
}

impl AiAppExt for App {
    fn register_ai_component<C: Component>(&mut self, describe: fn(&C) -> String) -> &mut Self {
        let describer =
            Describer { key: short_type_name::<C>(), describe: Arc::new(move |e: EntityRef| e.get::<C>().map(describe)) };
        self.world_mut().get_resource_or_init::<AiDescribers>().0.push(describer);
        self
    }

    fn register_ai_describer(&mut self, key: &'static str, describe: fn(EntityRef) -> Option<String>) -> &mut Self {
        self.world_mut().get_resource_or_init::<AiDescribers>().0.push(Describer { key, describe: Arc::new(describe) });
        self
    }

    fn register_ai_advice(&mut self, advise: describe::AdviceFn) -> &mut Self {
        self.world_mut().get_resource_or_init::<AiAdvisors>().0.push(advise);
        self
    }

    fn register_ai_observe_lines(&mut self, lines: describe::ObserveLinesFn) -> &mut Self {
        self.world_mut().get_resource_or_init::<AiObserveLines>().0.push(lines);
        self
    }

    fn register_ai_observe_section(&mut self, section: describe::ObserveSectionFn) -> &mut Self {
        self.world_mut().get_resource_or_init::<AiObserveSections>().0.push(section);
        self
    }

    fn add_ai_tool<A, O, M, S>(&mut self, name: &str, scope: Scope, system: S) -> &mut Self
    where
        A: DeserializeOwned + JsonSchema + Send + Sync + 'static,
        O: Serialize + 'static,
        S: IntoSystem<In<Call<A>>, ToolResult<O>, M> + Clone + Send + Sync + 'static,
    {
        tools::register_tool(self.world_mut(), name, scope, ToolKind::Action, system);
        self
    }

    fn add_ai_query<A, O, M, S>(&mut self, name: &str, scope: Scope, system: S) -> &mut Self
    where
        A: DeserializeOwned + JsonSchema + Send + Sync + 'static,
        O: Serialize + 'static,
        S: IntoSystem<In<Call<A>>, ToolResult<O>, M> + Clone + Send + Sync + 'static,
    {
        tools::register_tool(self.world_mut(), name, scope, ToolKind::Query, system);
        self
    }

    fn journal_event<E: Event>(&mut self, format: journal::EventFormatter<E>) -> &mut Self {
        self.add_observer(journal::journal_observer::<E>(format));
        self
    }

    fn add_ai_condition(&mut self, name: &str, condition: fn(&World) -> bool) -> &mut Self {
        self.world_mut().get_resource_or_init::<AiConditions>().0.insert(name.to_owned(), condition);
        self
    }

    fn add_ai_invariant(&mut self, name: &str, rule: invariants::Invariant) -> &mut Self {
        if let Err(e) = self.world_mut().get_resource_or_init::<AiInvariants>().add_game_rule(name, rule) {
            panic!("add_ai_invariant: {e}");
        }
        self
    }

    fn replay_input<C>(&mut self) -> &mut Self
    where
        C: Component<Mutability = bevy::ecs::component::Mutable> + Clone + PartialEq + Serialize + DeserializeOwned,
    {
        replay::register_input::<C>(self);
        self
    }

    fn replay_ignore<C: Component>(&mut self) -> &mut Self {
        self.world_mut().get_resource_or_init::<replay::ReplayHashing>().ignore(std::any::TypeId::of::<C>());
        self
    }

    fn add_replay_check(&mut self, name: &str, check: fn(&World) -> u64) -> &mut Self {
        self.world_mut().get_resource_or_init::<replay::ReplayHashing>().add_check(name, check);
        self
    }

    fn add_optional_replay_check(&mut self, name: &str, check: fn(&World) -> Option<u64>) -> &mut Self {
        self.world_mut().get_resource_or_init::<replay::ReplayHashing>().add_optional_check(name, check);
        self
    }

    fn add_playback_hook(&mut self, hook: fn(&mut World)) -> &mut Self {
        self.world_mut().get_resource_or_init::<replay::PlaybackHooks>().0.push(hook);
        self
    }

    fn add_ai_status(&mut self, key: &str, field: status::StatusField) -> &mut Self {
        self.world_mut().get_resource_or_init::<AiStatusFields>().add(key, field);
        self
    }

    fn add_dev_entry(&mut self, group: &str, id: &str, label: &str, command: DevCommand) -> &mut Self {
        let entry = DevEntry { group: group.into(), id: id.into(), label: label.into(), command };
        self.world_mut().get_resource_or_init::<DevMenu>().add(entry);
        self
    }

    fn add_snapshot_resource<R>(&mut self) -> &mut Self
    where
        R: Resource + Reflect + FromReflect + GetTypeRegistration + TypePath,
    {
        snapshot::register_resource::<R>(self);
        self
    }

    fn add_snapshot_check(&mut self, name: &'static str, hint: &'static str, check: fn(&World) -> u64) -> &mut Self {
        self.world_mut().get_resource_or_init::<SnapshotRegistry>().add_check(name, hint, check);
        self
    }

    fn add_snapshot_rebuild(&mut self, name: &'static str, rebuild: fn(&mut World)) -> &mut Self {
        self.world_mut().get_resource_or_init::<SnapshotRegistry>().add_rebuild(name, rebuild);
        self
    }

    fn add_snapshot_prepare(&mut self, name: &'static str, covers: &'static [&'static str], prepare: snapshot::PrepareFn) -> &mut Self {
        self.world_mut().get_resource_or_init::<SnapshotRegistry>().add_prepare(name, None, covers, prepare);
        self
    }

    fn add_snapshot_prepare_after(
        &mut self,
        name: &'static str,
        after: &'static str,
        covers: &'static [&'static str],
        prepare: snapshot::PrepareFn,
    ) -> &mut Self {
        self.world_mut().get_resource_or_init::<SnapshotRegistry>().add_prepare(name, Some(after), covers, prepare);
        self
    }

    fn add_snapshot_entity_key(&mut self, key: EntityKey) -> &mut Self {
        self.world_mut().get_resource_or_init::<SnapshotRegistry>().add_key(key);
        self
    }

    fn net_ignore<C: Component>(&mut self) -> &mut Self {
        self.world_mut().get_resource_or_init::<NetRules>().ignore(std::any::TypeId::of::<C>(), std::any::type_name::<C>());
        self
    }

    fn net_ignore_resource<R: Resource>(&mut self) -> &mut Self {
        let (id, name) = (std::any::TypeId::of::<R>(), std::any::type_name::<R>());
        self.world_mut().get_resource_or_init::<NetRules>().ignore_resource(id, name);
        self
    }

    fn net_predicted<C: Component>(&mut self) -> &mut Self {
        self.world_mut().get_resource_or_init::<NetRules>().predict(std::any::TypeId::of::<C>(), std::any::type_name::<C>());
        self
    }

    fn net_route(&mut self, name: &str, route: NetRoute) -> &mut Self {
        if let Err(why) = self.world_mut().get_resource_or_init::<AiTools>().set_route(name, route) {
            panic!("net_route: {why}");
        }
        self
    }
}

/// `ready: tick 0 · pid 4242 · seed 1 · port 15711`: what a game prints at start, and
/// what `shards wait` prints once the game answers.
pub fn ready_line(tick: u64, pid: u32, seed: u64, port: Option<u16>) -> String {
    let port = port.map_or_else(|| "no BRP".to_owned(), |p| format!("port {p}"));
    format!("ready: tick {tick} · pid {pid} · seed {seed} · {port}")
}

/// `game::combat::Health` → `Health`.
pub(crate) fn short_type_name<T>() -> &'static str {
    let full = std::any::type_name::<T>();
    let base = full.split('<').next().unwrap_or(full);
    base.rsplit("::").next().unwrap_or(base)
}
