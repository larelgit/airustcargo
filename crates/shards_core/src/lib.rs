//! # shards_core — what every Shards module and game shares
//!
//! - [`EngineConfig`]: window or headless, seed, port, tick rate, assets; one value
//!   builds the whole engine (`shards::EnginePlugins::new(config)`).
//! - [`Intent`]: the only input of a character, written by keyboard, agents, network
//!   and replays, read by gameplay. One-shot [`Press`]es are consumed during a tick;
//!   [`Order`]s are carried out over many ([`follow_orders`] walks).
//! - [`EngineState`] / [`PauseState`]: loading, menu, game, pause.
//! - [`LoadingGate`]: the simulation starts only once everything is loaded, so loading
//!   never shifts a tick.
//! - [`EngineModules`]: which engine modules run, shown in `ai/status` under `engine`.
//!
//! Gameplay runs in `FixedUpdate`, in `SimSet` order: the engine's own systems are in
//! [`CoreSystems`] so a game can order its systems against them.

pub mod intent;
pub mod loading;
pub mod state;

use ai_core::{AiAppExt, AiCorePlugin, SimConfig, SimSet};
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy::state::state::StateTransitionSystems;
use serde::Serialize;
use serde_json::{json, Value};

pub use intent::{flat, follow_orders, input_names, Intent, Order, Press, Target};
pub use loading::{statuses as loading_statuses, LoadCheck, LoadStatus, LoadingGate};
pub use state::{player_body, predicts, simulates, EngineState, LocalPlayer, NetRole, NetSet, PauseState, PlayerId, Predicted};

/// Version of the engine crates (they share one).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The types a game module uses most.
pub mod prelude {
    pub use crate::{
        input_names, CoreSystems, EngineAppExt, EngineConfig, EngineState, Intent, LoadStatus, LocalPlayer, NetRole, NetSet,
        Order, PauseState, PlayerId, Predicted, Press, Target,
    };
}

/// Everything that decides how the engine is built and run. `shards::run` fills it
/// from the command line; tests use `shards::testing::config()`.
#[derive(Resource, Clone, Debug)]
pub struct EngineConfig {
    /// Window title.
    pub title: String,
    /// No window, no GPU, no audio: CI, servers, parallel playtests. Gameplay is the
    /// same plugins, so a game plays the same with a window or without.
    pub headless: bool,
    /// World seed: same seed + same inputs = same game.
    pub seed: u64,
    /// BRP port for `shards` (`remote` feature). `None`: no HTTP server.
    pub port: Option<u16>,
    /// Start with the simulation paused; advance it with `shards step`.
    pub paused: bool,
    /// Simulation ticks per second.
    pub tick_hz: f64,
    /// A gameplay failure saves `replays/crash-tick<N>.json` to reproduce it with.
    pub crash_replays: bool,
    /// Window size in logical pixels.
    pub window: (u32, u32),
    /// Where `EngineState::Loading` goes once everything is loaded.
    pub after_loading: EngineState,
    /// Apply content files as they change on disk. Needs the `hot_reload` cargo feature
    /// (Bevy's file watcher); on by default, off in tests and `--verify`.
    pub hot_reload: bool,
    /// The asset folder: relative to the game's folder (`CARGO_MANIFEST_DIR` under
    /// cargo, else the executable's folder; `BEVY_ASSET_ROOT` overrides both), or
    /// absolute. Content lives in `<asset_dir>/content/`.
    pub asset_dir: String,
    /// A test app: headless, no logger (a process has only one), no Ctrl+C handler and
    /// no run loop; tests drive it with `app.update()` and tools.
    pub testing: bool,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            title: "Shards".into(),
            headless: false,
            seed: 42,
            port: Some(ai_core::DEFAULT_PORT),
            paused: false,
            tick_hz: 60.0,
            crash_replays: true,
            window: (1280, 720),
            after_loading: EngineState::InGame,
            hot_reload: true,
            asset_dir: "assets".into(),
            testing: false,
        }
    }
}

impl EngineConfig {
    /// The AI layer as this config wants it.
    pub fn ai_core_plugin(&self) -> AiCorePlugin {
        AiCorePlugin {
            sim: SimConfig { tick_hz: self.tick_hz, seed: self.seed, start_paused: self.paused },
            remote_port: if self.testing { None } else { self.port },
            crash_replays: self.crash_replays,
            ..default()
        }
    }
}

/// The engine's own gameplay systems, for ordering a game's systems against them.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum CoreSystems {
    /// `SimSet::Input`: [`follow_orders`] steers `Order::MoveTo`.
    FollowOrders,
    /// `SimSet::Cleanup`: unconsumed presses are dropped ([`intent::clear_presses`]).
    ClearPresses,
}

/// An engine module as `ai/status` lists it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct EngineModule {
    /// `core`, `content`, `motion`, ...
    pub name: String,
    /// Crate version.
    pub version: String,
}

/// The engine modules this game runs, in the order they were added.
#[derive(Resource, Default, Clone, Debug)]
pub struct EngineModules(Vec<EngineModule>);

impl EngineModules {
    /// Adds a module; adding one with the same name again updates its version.
    pub fn add(&mut self, name: &str, version: &str) {
        match self.0.iter_mut().find(|m| m.name == name) {
            Some(module) => module.version = version.to_owned(),
            None => self.0.push(EngineModule { name: name.to_owned(), version: version.to_owned() }),
        }
    }

    /// Modules in the order they were added.
    pub fn iter(&self) -> impl Iterator<Item = &EngineModule> {
        self.0.iter()
    }

    /// Whether module `name` runs.
    pub fn contains(&self, name: &str) -> bool {
        self.0.iter().any(|m| m.name == name)
    }
}

/// Registration API for engine modules and games.
pub trait EngineAppExt {
    /// Lists an engine module in `ai/status` (`engine.modules`).
    fn add_engine_module(&mut self, name: &str, version: &str) -> &mut Self;

    /// Holds the simulation until `check` reports [`LoadStatus::Ready`] (see
    /// [`LoadingGate`]).
    fn require_loaded(&mut self, label: &str, check: LoadCheck) -> &mut Self;
}

impl EngineAppExt for App {
    fn add_engine_module(&mut self, name: &str, version: &str) -> &mut Self {
        self.world_mut().get_resource_or_init::<EngineModules>().add(name, version);
        self
    }

    fn require_loaded(&mut self, label: &str, check: LoadCheck) -> &mut Self {
        self.world_mut().get_resource_or_init::<LoadingGate>().require(label, check);
        self
    }
}

/// States, the loading gate, `Intent` (a replay input) and its orders, engine modules
/// and the `engine` field of `ai/status`. Needs `AiCorePlugin` first; `EnginePlugins`
/// adds both.
pub struct CorePlugin {
    /// Inserted as a resource; `after_loading` decides where loading goes.
    pub config: EngineConfig,
}

impl Plugin for CorePlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<AiCorePlugin>(),
            "CorePlugin needs AiCorePlugin added first (shards::EnginePlugins adds both)"
        );
        if !app.is_plugin_added::<StatesPlugin>() {
            app.add_plugins(StatesPlugin);
        }
        // Explicitly, for apps built without `reflect_auto_register`: replay checks and
        // `inspect` read components through the type registry.
        app.register_type::<Intent>()
            .register_type::<PlayerId>()
            .register_type::<LocalPlayer>()
            .register_type::<Predicted>()
            .register_type::<NetRole>();
        // `ai/session {player: N}`, `possess player=N`: the body with `PlayerId(N)`.
        app.world_mut().get_resource_or_init::<ai_core::AgentSessions>().set_player_resolver(state::player_body);
        app.insert_resource(self.config.clone())
            .init_resource::<NetRole>()
            .init_resource::<LoadingGate>()
            .init_resource::<state::MenuPaused>()
            .init_resource::<state::RegionTrip>()
            .init_state::<EngineState>()
            .add_sub_state::<PauseState>()
            .replay_input::<Intent>()
            // "My body" is each machine's own.
            .net_ignore::<LocalPlayer>()
            .net_ignore::<Predicted>()
            .replay_ignore::<Predicted>()
            .add_engine_module("core", VERSION)
            .add_ai_status("engine", engine_status)
            // A client is fed the world by its host: it runs no gameplay, only its prediction.
            .configure_sets(
                FixedUpdate,
                (SimSet::Input, SimSet::Think, SimSet::Act, SimSet::Move, SimSet::Resolve, SimSet::Cleanup)
                    .distributive_run_if(simulates),
            )
            .configure_sets(FixedUpdate, NetSet::Predict.run_if(predicts))
            .configure_sets(
                FixedUpdate,
                (CoreSystems::FollowOrders.in_set(SimSet::Input), CoreSystems::ClearPresses.in_set(SimSet::Cleanup)),
            )
            .add_systems(
                FixedUpdate,
                (
                    follow_orders.in_set(CoreSystems::FollowOrders),
                    intent::clear_presses.in_set(CoreSystems::ClearPresses),
                ),
            )
            .add_systems(First, loading::update_loading.before(ai_core::sim::apply_gate))
            .add_systems(PreUpdate, state::close_pause_when_resumed)
            .add_systems(
                StateTransition,
                (
                    last_transition::<EngineState>
                        .pipe(state::hide_game_schedules)
                        .in_set(StateTransitionSystems::ExitSchedules)
                        .before(ExitSchedules::<EngineState>::default()),
                    state::restore_game_schedules
                        .in_set(StateTransitionSystems::EnterSchedules)
                        .after(EnterSchedules::<EngineState>::default()),
                ),
            )
            .add_systems(OnEnter(PauseState::Paused), state::pause_simulation)
            .add_systems(OnExit(PauseState::Paused), state::resume_simulation)
            .add_systems(Update, (state::journal_transitions::<EngineState>, state::journal_transitions::<PauseState>));
    }

    fn finish(&self, app: &mut App) {
        // After the game's describers: an entity's orders end its line.
        app.register_ai_describer("orders", intent::describe_orders);
    }
}

/// `ai/status` → `engine: {version, modules, state, pause}`.
fn engine_status(world: &World) -> Value {
    let modules: Vec<&EngineModule> = world.get_resource::<EngineModules>().map(|m| m.iter().collect()).unwrap_or_default();
    let state = world.get_resource::<State<EngineState>>().map(|s| format!("{:?}", s.get()));
    let pause = world.get_resource::<State<PauseState>>().map(|s| format!("{:?}", s.get()));
    json!({"version": VERSION, "modules": modules, "state": state, "pause": pause})
}
