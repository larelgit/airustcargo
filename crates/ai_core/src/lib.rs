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
//!                  Replay   — record inputs, play back  (replay)
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
//!     .add_ai_tool("move_to", Scope::Player, move_to);
//! ```

pub mod builtin;
pub mod describe;
#[cfg(feature = "render")]
pub mod highlight;
pub mod id;
pub mod journal;
pub mod observe;
#[cfg(feature = "remote")]
pub mod remote;
pub mod replay;
pub mod rng;
pub mod session;
#[cfg(feature = "render")]
pub mod shot;
pub mod sim;
pub mod spatial;
pub mod tools;

use std::sync::Arc;

use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::de::DeserializeOwned;
use serde::Serialize;

pub use builtin::{AiConditions, StopWhen};
pub use describe::{AiDescribers, AiKind, Describer, EntityFacts};
pub use id::{label, AiId, AiImportance, AiIndex, AiLandmark, AiPlayer, AiVisible};
pub use journal::{capture_log_layer, AiJournal, JournalEntry, JournalKind};
pub use observe::{AiCamera, AiPerception, ObserveOptions};
pub use replay::{AiReplay, Replay, ReplayReport};
pub use rng::{DetRng, SimRng, SimSeed};
pub use session::{AgentSession, AgentSessions, Caller, Scope};
pub use sim::{SimConfig, SimSet, SimTick};
pub use spatial::{AiLineOfSight, AiOccluder};
pub use tools::{call_tool, AiTools, Call, CallOutcome, Id, NoArgs, ToolKind, ToolResult, ToolSpec};

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
    pub remote_port: Option<u16>,
}

impl Default for AiCorePlugin {
    fn default() -> Self {
        Self { sim: SimConfig::default(), perception: AiPerception::default(), remote_port: Some(DEFAULT_PORT) }
    }
}

impl Plugin for AiCorePlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(self.sim.clone())
            .insert_resource(Time::<Fixed>::from_hz(self.sim.tick_hz))
            .insert_resource(SimSeed(self.sim.seed))
            .insert_resource(self.perception.clone())
            .init_resource::<SimTick>()
            .init_resource::<AiIndex>()
            .init_resource::<AiJournal>()
            .init_resource::<AiDescribers>()
            .init_resource::<AiTools>()
            .init_resource::<AgentSessions>()
            .init_resource::<AiLineOfSight>()
            .init_resource::<AiConditions>()
            .configure_sets(
                FixedUpdate,
                (SimSet::Input, SimSet::Think, SimSet::Act, SimSet::Move, SimSet::Resolve, SimSet::Cleanup).chain(),
            )
            .add_systems(FixedFirst, sim::advance_tick)
            .add_systems(Last, journal::drain_captured_logs)
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
        replay::plugin(app);

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
            app.add_plugins(bevy_remote::http::RemoteHttpPlugin::default().with_port(port));
            remote::check_port(port);
        }
    }

    fn finish(&self, app: &mut App) {
        #[cfg(feature = "remote")]
        if self.remote_port.is_some() {
            remote::register_methods(app.world_mut());
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

    /// Records component `C` in replays: the per-entity input everything writes
    /// (keyboard, network, agents) and gameplay only reads, like the sandbox's `Intent`.
    /// Changes made between ticks are stored; playback sets them back at the same tick.
    fn replay_input<C>(&mut self) -> &mut Self
    where
        C: Component<Mutability = bevy::ecs::component::Mutable> + Clone + PartialEq + Serialize + DeserializeOwned;
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

    fn replay_input<C>(&mut self) -> &mut Self
    where
        C: Component<Mutability = bevy::ecs::component::Mutable> + Clone + PartialEq + Serialize + DeserializeOwned,
    {
        replay::register_input::<C>(self);
        self
    }
}

/// `game::combat::Health` → `Health`.
pub(crate) fn short_type_name<T>() -> &'static str {
    let full = std::any::type_name::<T>();
    let base = full.split('<').next().unwrap_or(full);
    base.rsplit("::").next().unwrap_or(base)
}
