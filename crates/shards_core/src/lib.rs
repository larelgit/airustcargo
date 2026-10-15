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

pub mod exit;
pub mod intent;
pub mod loading;
pub mod seed;
pub mod state;

use ai_core::{AiAppExt, AiCorePlugin, SimConfig, SimSet};
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use bevy::state::state::StateTransitionSystems;
use serde::Serialize;
use serde_json::{json, Value};

pub use exit::{run_exit_hooks, ExitHook, ExitHooks};
pub use intent::{flat, follow_orders, input_names, Intent, Order, Press, Target};
pub use loading::{statuses as loading_statuses, LoadCheck, LoadStatus, LoadingGate};
pub use seed::{adopt_seed, hold_seed, seed_ready, SeedLock};
pub use state::{
    player_body, predicts, simulates, EngineState, LocalPlayer, NetPrediction, NetRole, NetSet, PauseState, PlayerId, Predicted,
    PredictedMotion,
};

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
    /// A gameplay failure saves `replays/crash-tick<N>.json` to reproduce it with, a
    /// violated invariant `replays/invariant-<name>-tick<N>.json`.
    pub crash_replays: bool,
    /// Ticks between replay checks (`--check-every`), stored in every replay saved.
    pub check_every: u64,
    /// What each replay check keeps to name what diverged (`--check-detail`).
    pub check_detail: ai_core::CheckDetail,
    /// A tick that takes longer is over budget (`--tick-budget-ms`); `None`: the tick's
    /// own duration.
    pub tick_budget: Option<std::time::Duration>,
    /// Tick timing in `step` replies and `perf` journal lines; off in tests, whose
    /// journals must not depend on the machine.
    pub report_perf: bool,
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
    /// A game beside another in one process (a stand's clients): like [`testing`](Self::testing)
    /// (headless, no logger, no Ctrl+C handler, no run loop: a driver updates it), but with
    /// its own BRP port, and its log lines go to its own journal.
    pub embedded: bool,
    /// Taking part in a networked game (`--host`, `--join`, `--stand`).
    pub net: NetLaunch,
    /// The name other players see (`--name`).
    pub player_name: String,
    /// Network conditions this game's links simulate (`--net-sim latency=150,loss=0.02`).
    pub net_sim: Option<String>,
    /// Where saves are written, relative to the game's folder (`--save-dir`).
    pub save_dir: String,
    /// Where player settings are kept (`--settings-dir`, `--no-settings`).
    pub settings: SettingsStore,
    /// The character to play (`--character SLOT`): a save slot.
    pub character_slot: Option<String>,
    /// The world to load (`--world SLOT`): a save slot.
    pub world_slot: Option<String>,
    /// The game exits when its session ends, instead of going back to the menu
    /// (`--exit-with-host`): with 0 when the host ended it, 1 when the game was refused,
    /// kicked, lost its host or could not take its world, 2 when the session cannot be
    /// opened (a host's port taken, an address that does not resolve).
    pub exit_with_host: bool,
}

/// The UDP port of a hosted game unless `--host PORT` says otherwise.
pub const DEFAULT_NET_PORT: u16 = 7777;

/// How a game takes part in a networked session.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum NetLaunch {
    /// Single player: no network.
    #[default]
    Off,
    /// Hosts a session on this UDP port (`--host [PORT]`).
    Host { port: u16 },
    /// Joins the host at `addr` (`--join 127.0.0.1:7777`).
    Join { addr: String },
    /// A host and `clients` clients from one command (`--stand N`): in this process, or
    /// child processes over UDP loopback (`udp`, `--stand-udp`).
    Stand { clients: u8, udp: bool },
}

/// Where player settings (graphics, audio, controls) are kept.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum SettingsStore {
    /// Defaults, no files (`--no-settings`, tests, a stand's clients).
    Off,
    /// The platform's settings folder for this game.
    #[default]
    Default,
    /// This folder (`--settings-dir DIR`).
    Dir(String),
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
            check_every: ai_core::replay::CHECK_EVERY,
            check_detail: ai_core::CheckDetail::default(),
            tick_budget: None,
            report_perf: true,
            window: (1280, 720),
            after_loading: EngineState::InGame,
            hot_reload: true,
            asset_dir: "assets".into(),
            testing: false,
            embedded: false,
            net: NetLaunch::Off,
            player_name: "player".into(),
            net_sim: None,
            save_dir: "saves".into(),
            settings: SettingsStore::Default,
            character_slot: None,
            world_slot: None,
            exit_with_host: false,
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
            check_every: self.check_every,
            check_detail: self.check_detail,
            tick_budget: self.tick_budget,
            report_perf: self.report_perf,
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

    /// Runs `hook` when the game ends: once, after the hooks added before it, when an
    /// `AppExit` is written or a termination signal arrives (see [`exit`]).
    fn add_exit_hook(&mut self, name: &str, hook: ExitHook) -> &mut Self;
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

    fn add_exit_hook(&mut self, name: &str, hook: ExitHook) -> &mut Self {
        self.world_mut().get_resource_or_init::<ExitHooks>().add(name, hook);
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
            .init_resource::<ExitHooks>()
            .init_resource::<SeedLock>()
            .init_state::<EngineState>()
            .add_sub_state::<PauseState>()
            .replay_input::<Intent>()
            // "My body" is each machine's own.
            .net_ignore::<LocalPlayer>()
            .net_ignore::<Predicted>()
            .replay_ignore::<Predicted>()
            // A joining client learns the seed from its host.
            .require_loaded("seed", seed::seed_status)
            .add_engine_module("core", VERSION)
            .add_ai_status("engine", engine_status)
            // A client is fed the world by its host: it runs no gameplay, only its prediction.
            .configure_sets(
                FixedUpdate,
                (SimSet::Input, SimSet::Think, SimSet::Act, SimSet::Move, SimSet::Resolve, SimSet::Cleanup)
                    .distributive_run_if(simulates),
            )
            .init_resource::<NetPrediction>()
            .configure_sets(
                FixedUpdate,
                (NetSet::PredictInput, NetSet::Predict, NetSet::PredictEnd)
                    .chain()
                    .before(SimSet::Input)
                    .distributive_run_if(predicts),
            )
            // Invariants check the simulation where it runs: a client's world is the host's.
            .configure_sets(FixedLast, ai_core::InvariantSystems.run_if(simulates))
            .configure_sets(
                FixedUpdate,
                (CoreSystems::FollowOrders.in_set(SimSet::Input), CoreSystems::ClearPresses.in_set(SimSet::Cleanup)),
            )
            .add_systems(
                FixedUpdate,
                (
                    follow_orders::<()>.in_set(CoreSystems::FollowOrders),
                    intent::clear_presses.in_set(CoreSystems::ClearPresses),
                ),
            )
            .add_systems(First, loading::update_loading.before(ai_core::sim::apply_gate))
            .add_systems(PreUpdate, state::close_pause_when_resumed)
            .add_systems(Last, run_exit_hooks.run_if(exit::exiting))
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
