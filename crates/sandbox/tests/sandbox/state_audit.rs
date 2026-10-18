//! The state audit: a static complement of heavy mode that needs no twin. After the combat
//! prototype and the region replays have played to their end, every resource in the world is
//! either saved in snapshots (`add_snapshot_resource`), or named in [`OUTSIDE`] as outside the
//! simulation, with why. A resource that is neither may be state a snapshot misses: the audit
//! fails and names its type, and it goes into one list or the other.

use std::any::TypeId;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use shards::ai_core::replay;
use shards::ai_core::SnapshotRegistry;
use shards::prelude::*;

/// Resources outside the simulation, by the start of their type name, and why. A generic type
/// is named up to its `<`.
const OUTSIDE: &[(&str, &str)] = &[
    // Bevy
    ("bevy_a11y::", "accessibility of the window"),
    ("bevy_animation::", "animation graphs of drawn models: the picture"),
    ("bevy_app::main_schedule::", "the order of Bevy's schedules, fixed at build time"),
    ("bevy_asset::", "assets and their server: files the world is built around, which snapshots check by hash"),
    ("bevy_camera::", "cameras and what they see: the picture"),
    ("bevy_clipboard::", "the system clipboard"),
    ("bevy_core_pipeline::", "rendering"),
    ("bevy_diagnostic::", "frame diagnostics"),
    ("bevy_ecs::entity_disabling::", "the ECS's default query filters, fixed at build time"),
    ("bevy_ecs::error::", "the ECS's error handler, fixed at build time"),
    ("bevy_ecs::message::message_registry::", "the registry of message types, fixed at build time"),
    // A game's own message type (`Messages<mygame::…>`) is not here: one written in a tick and read in the next is state.
    ("bevy_ecs::message::messages::Messages<bevy_", "Bevy's own messages (input, windows, assets, app exit), outside the fixed tick"),
    ("bevy_ecs::reflect::", "the type registry, fixed at build time"),
    ("bevy_ecs::schedule::", "the schedules, fixed at build time"),
    ("bevy_ecs::system::system_registry::", "ids of registered systems, fixed at build time"),
    ("bevy_gizmos::", "debug drawing: the picture"),
    ("bevy_gltf::", "the glTF loader's settings"),
    ("bevy_image::", "image formats the GPU supports"),
    ("bevy_input::", "raw device input: the simulation reads Intent, which the replay records"),
    ("bevy_input_focus::", "which UI element has focus"),
    ("bevy_light::", "lights: the picture"),
    ("bevy_pbr::", "rendering"),
    ("bevy_picking::", "pointer picking for UI and the window"),
    ("bevy_render::", "rendering"),
    ("bevy_sprite_render::", "rendering"),
    ("bevy_state::", "engine states: a snapshot is taken and loaded in the game, and the loading states are the gate's"),
    ("bevy_text::", "text layout and fonts: the picture"),
    ("bevy_time::", "Bevy's clocks: the simulation counts SimTick, which every snapshot holds"),
    ("bevy_transform::", "transform propagation settings, fixed at build time"),
    ("bevy_ui::", "UI layout: screens are outside the simulation"),
    ("bevy_ui_render::", "rendering"),
    ("bevy_world_serialization::", "the scene spawner"),
    // ai_core
    ("ai_core::builtin::AiConditions", "the registry of named conditions, filled at build time"),
    ("ai_core::coverage::CoverageParts", "the coverage parts and catalogs, registered at build time (AiCoverage itself exists only with coverage on)"),
    ("ai_core::crash::GameInfo", "the game's name and version for crash reports, set at build time"),
    ("ai_core::crash::Reported", "the panics a crash report was written for already"),
    ("ai_core::crash::Watched", "the crash reporter's count of panic hooks it has seen"),
    ("ai_core::describe::AiAdvisors", "the registry of observe advisors, filled at build time"),
    ("ai_core::describe::AiDescribers", "the registry of component describers, filled at build time"),
    ("ai_core::describe::AiObserveLines", "the registry of extra observe lines, filled at build time"),
    ("ai_core::describe::AiObserveSections", "the registry of observe sections, filled at build time"),
    ("ai_core::dev::DevKeyboard", "whether the window's developer keys are on"),
    ("ai_core::dev::DevMenu", "the developer menu's entries, registered at build time"),
    ("ai_core::invariants::RuleQueries", "a cache of the invariant rules' query states, rebuilt on demand"),
    ("ai_core::failures::AiFailures", "failed systems and whether a tick failed: the AI layer's bookkeeping"),
    ("ai_core::failures::HandlerSlot", "the slot of this app's error handler in the process"),
    ("ai_core::highlight::AiOverlay", "highlight drawing settings for screenshots: the picture"),
    ("ai_core::highlight::Highlights", "highlights queued for the next screenshot: the picture"),
    ("ai_core::id::AiIndex", "the id index, kept by the AiId hooks; snapshot::restore puts its counters back"),
    ("ai_core::invariants::AiInvariants", "rules and their firings, which are not part of the replay hash or the snapshot"),
    ("ai_core::journal::AiJournal", "the journal: what the AI reads about the game, outside it"),
    ("ai_core::net_rules::NetRules", "which components the network replicates, filled at build time"),
    ("ai_core::observe::AiPerception", "how far and how much `observe` tells"),
    ("ai_core::perf::AiPerf", "tick timing, which is not part of the hash or the replay"),
    ("ai_core::process::AppToken", "this app's identity in the process"),
    ("ai_core::replay::AiReplay", "the replay recorder and player"),
    ("ai_core::replay::InputMemory<", "the recorder's memory of the last input values, which it compares against; gameplay never reads it"),
    ("ai_core::replay::PlaybackHooks", "what runs when a playback starts, registered at build time"),
    ("ai_core::replay::ReplayHashing", "what replay checks hash and leave out, registered at build time"),
    ("ai_core::rng::SimSeed", "the world's seed, in every snapshot's header and checked on load"),
    ("ai_core::session::AgentSessions", "the sessions and scopes of the agents that call tools"),
    ("ai_core::shot::AiShots", "screenshot requests: the picture"),
    ("ai_core::shot::PipelinesCompiling", "render pipelines still compiling: the picture"),
    ("ai_core::sim::SimConfig", "the tick rate and the clock's settings, set at start"),
    ("ai_core::sim::SimGate", "what holds the clock while the game loads, which a load sets for itself"),
    ("ai_core::sim::SimTick", "the tick, which every snapshot holds and restore puts back"),
    ("ai_core::sim::TickClock", "the tick shared with the log layer, which restore sets with the tick"),
    ("ai_core::snapshot::SnapshotRegistry", "what snapshots save, rebuild and check, registered at build time"),
    ("ai_core::spatial::AiLineOfSight", "the line-of-sight query, set at build time"),
    ("ai_core::status::AiStatusFields", "the status fields, registered at build time"),
    ("ai_core::tools::AiTools", "the tools, registered at build time"),
    // sandbox
    ("sandbox::gameplay::layout::BuiltLayouts", "the layouts the world was built from: content, which tools and describers read"),
    // shards
    ("shards::coop::SessionCharacters", "a co-op host's last captured character of each player, for rejoining"),
    ("shards::tile_levels::TileLevels", "which level instance is which tile, made again when the region is (snapshot prepare `region`)"),
    ("shards::ui_wiring::MenuControls", "the menus' controls"),
    // shards_action
    ("shards_action::combat::DamageHooks", "the damage hooks, registered at build time"),
    ("shards_action::machine::ActionEffects", "the custom action effects, registered at build time"),
    ("shards_action::machine::PendingEffects", "effects queued and run inside one tick: empty between ticks"),
    ("shards_action::predict::ActionPredictor", "a co-op client's predicted actions; a host or a single player never uses it"),
    ("shards_action::render::", "animation graphs, the threat HUD and the combat overlay: the picture"),
    ("shards_action::status::StatusEffects", "the status effect hooks, registered at build time"),
    // shards_audio
    ("shards_audio::", "the mixer, the music's diagnostics and the audio device: sound only reads the world"),
    // shards_content
    ("shards_content::ContentGuards", "the content guards, registered at build time"),
    ("shards_content::db::ContentDb", "loaded content: the world is built around it, and snapshots check its hash"),
    ("shards_content::db::ContentKinds", "the content kinds, registered at build time"),
    ("shards_content::load::ContentLoading", "the content loader's progress"),
    // shards_core
    ("shards_core::EngineConfig", "the engine's configuration, set at start"),
    ("shards_core::EngineModules", "the engine modules, registered at build time"),
    ("shards_core::exit::ExitHooks", "the exit hooks, registered at build time"),
    ("shards_core::loading::LoadingGate", "what loading waits for, and the failures journaled already"),
    ("shards_core::menu::MainMenuClock", "the main menu's clock, outside the game"),
    ("shards_core::seed::SeedLock", "a co-op client's wait for the host's seed at start"),
    ("shards_core::state::MenuPaused", "whether the pause menu paused the clock"),
    ("shards_core::state::NetPrediction", "a co-op client's view of the host's tick, written by the network"),
    ("shards_core::state::NetRole", "single player, host or client: the session's, chosen at start"),
    ("shards_core::state::PredictedMotion", "the motion module's correction functions, set at build time"),
    ("shards_core::state::RegionTrip", "the scratch state of one region change, empty between them"),
    // shards_dev
    ("shards_dev::", "the developer console and window: developer tools"),
    // shards_fx
    ("shards_fx::", "effects and decals: the picture, on the simulation's clock but never changing the world"),
    // shards_level
    ("shards_level::library::LevelLibrary", "loaded level files: what the world is built around, which a snapshot check guards"),
    ("shards_level::spawn::LevelInstances", "the levels spawned, made again by the snapshot prepares `levels` and `placed levels`"),
    // shards_locale
    ("shards_locale::", "languages and their texts: content and the player's setting"),
    // shards_look
    ("shards_look::", "looks and their materials: the picture"),
    // shards_motion
    ("shards_motion::ai::MotionAi", "the settings of motion queries (fall height, clearance range)"),
    ("shards_motion::camera::", "the camera: the picture"),
    ("shards_motion::input::", "the local controls and key bindings: gameplay reads Intent, which the replay records"),
    ("shards_motion::overlay::PhysicsOverlay", "the physics overlay: the picture"),
    ("shards_motion::perception::RoutePlanner", "the route planner the NPC module installs at build time"),
    ("shards_motion::physics::world::PhysicsWorld", "the colliders' broad phase, synced from entities every tick and made again by the snapshot rebuild `physics`"),
    // shards_net
    ("shards_net::events::EventInbox", "a client's host events, for effects"),
    ("shards_net::events::EventOutbox", "a host's events on the way to its clients"),
    ("shards_net::events::NetEvents", "the forwarded event types, registered at build time"),
    ("shards_net::hooks::NetHooks", "the network hooks, registered at build time"),
    ("shards_net::interp::", "a client's interpolation buffer for drawing"),
    ("shards_net::overlay::NetOverlay", "the network overlay: the picture"),
    ("shards_net::predict::NetCorrection", "a client's prediction corrections"),
    ("shards_net::session::", "the session and its connection: transport, not the world"),
    // shards_npc
    ("shards_npc::brain::NpcActivity", "a hook the game sets at build time"),
    ("shards_npc::brain::NpcHooks", "the NPC conditions and states, registered at build time"),
    ("shards_npc::invariants::TokenAudit", "what the token rule found right after the director this tick: recomputed every tick, read only by the invariant"),
    ("shards_npc::nav::NavPaths", "the path finder, set at build time"),
    ("shards_npc::nav::overlay::", "the navigation overlay: the picture"),
    ("shards_npc::nav::region::NavBakes", "baked navigation tiles by hash: a function of files and content"),
    ("shards_npc::nav::region::NavRegion", "the active region's navmesh, made again by the snapshot rebuild `navmesh`"),
    ("shards_npc::nav::tools::NavOverlay", "the navigation overlay: the picture"),
    ("shards_npc::overlay::NpcOverlay", "the NPC overlay: the picture"),
    // shards_save
    ("shards_save::autosave::SaveRequests", "saves asked for and the autosave debounce: when files are written, not what the world is"),
    ("shards_save::launch::", "whether a save exists to continue, and whether the launch slot was loaded"),
    ("shards_save::ops::", "save settings, routing and the current slots: files on disk"),
    ("shards_save::section::SaveRegistry", "the save sections, registered at build time"),
    // shards_scatter
    ("shards_scatter::ScatterState", "strewn foliage: the picture, without AiId or colliders"),
    // shards_voxel
    ("shards_voxel::worldgen::VoxelSteps", "the voxel generation steps and checks, registered at build time"),
    // shards_settings
    ("shards_settings::", "the player's settings and their registry"),
    // shards_ui
    ("shards_ui::", "screens and menus: outside the simulation (no AiId), driven through tools"),
    // shards_world
    ("shards_world::WorldSettings", "the world's settings, from content"),
    ("shards_world::generate::WorldGen", "the generator's steps and checks, registered at build time"),
    ("shards_world::overlay::", "the tile overlay: the picture"),
    ("shards_world::region::ActiveRegion", "the region in the world, built again from the saved manifest (RegionBuild) by the snapshot prepare `region`"),
    ("shards_world::region::LevelFiles", "the level files the region waits for: what the world is built around"),
    ("shards_world::region::LevelSpawner", "the level spawner the level module installs at build time"),
    ("shards_world::region::RegionLoadedHooks", "the region-loaded hooks, registered at build time"),
    ("shards_world::region::StartRegion", "the start region, chosen at launch"),
];

fn replays_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays")
}

/// The sandbox after `name` (a reference replay) has played to its end.
fn played(name: &str) -> App {
    let recording = replay::load(&replays_dir().join(name)).unwrap();
    let config = EngineConfig { seed: recording.seed, tick_hz: recording.tick_hz, ..shards::testing::config() };
    let mut app = sandbox::build_app(&config);
    app.finish();
    app.cleanup();
    let report = shards::testing::verify(&mut app, recording).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(report.identical(), "{name}: {report}");
    app
}

/// The type names of `world`'s resources that snapshots do not save.
fn not_saved(world: &World) -> BTreeSet<String> {
    let saved: BTreeSet<TypeId> =
        world.get_resource::<SnapshotRegistry>().map(|r| r.resource_types().map(|(id, _)| id).collect()).unwrap_or_default();
    world
        .iter_resources()
        .filter(|(_, info, _)| !info.type_id().is_some_and(|id| saved.contains(&id)))
        .map(|(_, info, _)| info.name().to_string())
        .collect()
}

/// Of `names`, those [`OUTSIDE`] does not name.
fn unclassified(names: &BTreeSet<String>) -> Vec<&str> {
    names.iter().map(String::as_str).filter(|name| !OUTSIDE.iter().any(|(pattern, _)| name.starts_with(pattern))).collect()
}

#[test]
fn every_resource_is_saved_or_said_to_be_outside_the_simulation() {
    let mut seen = BTreeSet::new();
    for name in ["combat_prototype.json", "region.json"] {
        let app = played(name);
        let names = not_saved(app.world());
        let unknown = unclassified(&names);
        assert!(
            unknown.is_empty(),
            "{name}: resources neither saved in snapshots (add_snapshot_resource) nor in OUTSIDE \
             (crates/sandbox/tests/sandbox/state_audit.rs) with a reason:\n{}",
            unknown.join("\n")
        );
        seen.extend(names);
    }
    // The list names what is there: an entry nothing matches any more goes.
    let stale: Vec<&str> =
        OUTSIDE.iter().map(|(pattern, _)| *pattern).filter(|pattern| !seen.iter().any(|name| name.starts_with(pattern))).collect();
    assert!(stale.is_empty(), "OUTSIDE names resources the sandbox no longer has: {stale:?}");
}

/// A resource the snapshot does not hold, and nobody said why.
#[derive(Resource, Default)]
struct Unlisted(#[allow(dead_code)] u32);

#[test]
fn a_new_resource_nobody_classified_fails_the_audit() {
    // Built as the audit builds it (`test_app` adds its own save folder for the tests).
    let mut app = sandbox::build_app(&shards::testing::config());
    app.finish();
    app.cleanup();
    shards::testing::wait_until_loaded(&mut app, std::time::Duration::from_secs(60)).unwrap();
    let before = not_saved(app.world());
    assert!(unclassified(&before).is_empty(), "{:?}", unclassified(&before));
    app.init_resource::<Unlisted>();
    let after = not_saved(app.world());
    assert_eq!(unclassified(&after), ["sandbox::state_audit::Unlisted"]);
}
