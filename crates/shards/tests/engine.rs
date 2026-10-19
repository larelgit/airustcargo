//! The engine as a game gets it: one plugin group, headless the same plugins minus
//! window, GPU and audio, and test apps that come back loaded at tick 0.

use shards::ai_core::{AiJournal, JournalKind};
use shards::bevy::app::{PluginGroup, ScheduleRunnerPlugin};
use shards::core::{CorePlugin, EngineModules};
use shards::prelude::*;
use shards::serde_json::json;
use shards::testing::{call, ok};

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
struct Totem;

/// A game of one entity, spawned when the engine enters the game.
struct TinyGame;

impl Plugin for TinyGame {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(EngineState::InGame), |mut commands: Commands| {
            commands.spawn((Totem, AiKind::new("Totem"), Transform::from_xyz(1.0, 0.0, 2.0)));
        });
    }
}

#[test]
fn headless_is_the_same_plugins_minus_display() {
    let windowed = EnginePlugins::new(EngineConfig::default()).build();
    let headless = EnginePlugins::new(EngineConfig { headless: true, ..default() }).build();
    for group in [&windowed, &headless] {
        assert!(group.enabled::<AiCorePlugin>() && group.enabled::<CorePlugin>() && group.enabled::<ContentPlugin>());
        assert!(group.enabled::<shards::motion::MotionPlugin>());
        assert!(group.enabled::<TransformPlugin>() && group.enabled::<AssetPlugin>() && group.enabled::<bevy::state::app::StatesPlugin>());
    }
    assert!(headless.enabled::<ScheduleRunnerPlugin>() && !windowed.contains::<ScheduleRunnerPlugin>());
    #[cfg(feature = "render")]
    {
        assert!(windowed.enabled::<bevy::winit::WinitPlugin>() && !headless.enabled::<bevy::winit::WinitPlugin>());
        assert!(headless.enabled::<bevy::render::RenderPlugin>(), "render plugins stay registered, without a GPU");
    }
    let testing = EnginePlugins::new(shards::testing::config()).build();
    assert!(!testing.enabled::<bevy::log::LogPlugin>(), "a process has one logger");
}

/// Offscreen is headless's window (none) and run loop with the window's GPU: the renderer looks
/// for an adapter, and gizmos (the `eN` marks) are drawn.
#[cfg(feature = "render")]
#[test]
fn offscreen_is_the_window_plugins_without_a_window() {
    let offscreen = EnginePlugins::new(EngineConfig { offscreen: true, ..default() }).build();
    assert!(offscreen.enabled::<ScheduleRunnerPlugin>() && !offscreen.enabled::<bevy::winit::WinitPlugin>());
    assert!(offscreen.enabled::<bevy::render::RenderPlugin>() && offscreen.enabled::<bevy::gizmos_render::GizmoRenderPlugin>());
    let headless = EnginePlugins::new(EngineConfig { headless: true, ..default() }).build();
    assert!(!headless.enabled::<bevy::gizmos_render::GizmoRenderPlugin>(), "no GPU to draw gizmos with");
    assert!(EngineConfig { offscreen: true, ..default() }.draws() && !EngineConfig { headless: true, ..default() }.draws());
}

/// A headless game says so, and `shot` names the ways to get a picture.
#[cfg(feature = "render")]
#[test]
fn a_headless_game_has_no_picture_and_says_how_to_get_one() {
    let mut app = shards::testing::test_app(|_| {}, |_| {});
    let display = &shards::ai_core::status::status(app.world())["display"];
    assert_eq!(display, &json!({"mode": "headless", "size": null, "scene": null, "gpu": null}));
    let refused = call(&mut app, "shot", json!({}));
    let text = refused.result.as_str().unwrap_or_default().to_owned();
    assert!(!refused.ok && text.contains("--offscreen") && text.contains("observe"), "{text}");
}

/// `ai/status` counts the entities and the meshes; headless no frame is drawn and nothing is seen.
#[test]
fn the_status_counts_what_there_is_and_headless_has_no_frames() {
    let app = shards::testing::test_app(|_| {}, |app| {
        app.add_plugins(TinyGame);
    });
    let status = shards::ai_core::status::status(app.world());
    assert_eq!(status["frames"], json!(null), "{status}");
    let counts = &status["counts"];
    assert!(counts["entities"].as_u64().is_some_and(|n| n > 1), "the totem and the engine's own: {counts}");
    assert_eq!((&counts["meshes"], &counts["visible_meshes"], &counts["camera_meshes"]), (&json!(0), &json!(0), &json!(0)), "{counts}");
}

#[test]
fn a_test_app_comes_back_loaded_and_paused_at_tick_zero() {
    let mut app = shards::testing::test_app(|config| config.seed = 7, |app| {
        app.add_plugins(TinyGame);
    });
    assert_eq!(shards::ai_core::sim::tick(app.world()), 0);
    assert!(shards::ai_core::sim::is_paused(app.world()));
    assert_eq!(app.world().resource::<State<EngineState>>().get(), &EngineState::InGame);
    let seen = ok(&mut app, "observe", json!({}));
    assert!(seen.result.as_str().unwrap().contains("e1 Totem"), "{}", seen.result);
    let modules: Vec<String> = app.world().resource::<EngineModules>().iter().map(|m| m.name.clone()).collect();
    let mut expected = vec!["core", "content", "settings", "motion", "rules", "action", "level", "world", "npc", "encounter", "net", "save", "coop", "locale", "ui", "audio", "look", "scatter", "voxel", "fx", "hitstop"];
    // Steam is a module only in a game built with the `steam` feature.
    if cfg!(feature = "steam") {
        expected.push("steam");
    }
    expected.push("dev");
    assert_eq!(modules, expected);
    // This crate has no assets folder: no content, and the journal says where it looked.
    let warned = app.world().resource::<AiJournal>().after(0).0.any(|e| e.kind == JournalKind::Warn && e.text.contains("no content folder"));
    assert!(warned);
    ok(&mut app, "step", json!({"ticks": 3}));
    assert_eq!(shards::ai_core::sim::tick(app.world()), 3);
    assert!(!call(&mut app, "content_get", json!({"kind": "prefab", "id": "nothing"})).ok);
}

/// Every module a game may leave out goes (with what needs it), and the engine still loads and
/// runs without it: the module and its tools are gone from what agents see.
#[test]
fn a_game_runs_without_each_module_it_may_leave_out() {
    use shards::modules::{check_without, OPTIONAL_MODULES};
    for module in OPTIONAL_MODULES.iter().filter(|m| m.name != "steam" || cfg!(feature = "steam")) {
        // The module, and every module that needs one that goes.
        let mut without = vec![module.name.to_owned()];
        while let Some(more) = OPTIONAL_MODULES.iter().find(|m| !without.iter().any(|w| w == m.name) && m.needs.iter().any(|n| without.iter().any(|w| w == n))) {
            without.push(more.name.to_owned());
        }
        let gone = check_without(&without).unwrap_or_else(|e| panic!("{without:?}: {e}"));
        let leaves = without.clone();
        let mut app = shards::testing::test_app(move |config| config.without = leaves, |app| {
            app.add_plugins(TinyGame);
        });
        ok(&mut app, "step", json!({"ticks": 60}));
        assert_eq!(shards::ai_core::sim::tick(app.world()), 60, "without {without:?}");
        let modules: Vec<String> = app.world().resource::<EngineModules>().iter().map(|m| m.name.clone()).collect();
        for name in &gone {
            assert!(!modules.iter().any(|m| m == name), "without {without:?}: {name} still runs: {modules:?}");
        }
        for name in shards::modules::REQUIRED_MODULES {
            assert!(modules.iter().any(|m| m == name), "without {without:?}: {name} does not run: {modules:?}");
        }
    }
    // A module's tools go with it.
    let mut app = shards::testing::test_app(|config| config.without = vec!["audio".into(), "fx".into()], |_| {});
    for tool in ["audio", "sound_play", "effects", "effect_play"] {
        assert!(!call(&mut app, tool, json!({})).ok, "{tool} is still there");
    }
    assert!(ok(&mut app, "observe", json!({})).ok);
}

/// What cannot be left out is said before anything is built.
#[test]
#[should_panic(expected = "EngineConfig::without: `ui` (screens, menus and the HUD) needs `locale`")]
fn leaving_out_a_module_something_needs_is_refused() {
    let _ = EnginePlugins::new(EngineConfig { without: vec!["locale".into()], ..shards::testing::config() }).build();
}
