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
    assert_eq!(modules, ["core", "content"]);
    // This crate has no assets folder: no content, and the journal says where it looked.
    let warned = app.world().resource::<AiJournal>().after(0).0.any(|e| e.kind == JournalKind::Warn && e.text.contains("no content folder"));
    assert!(warned);
    ok(&mut app, "step", json!({"ticks": 3}));
    assert_eq!(shards::ai_core::sim::tick(app.world()), 3);
    assert!(!call(&mut app, "content_get", json!({"kind": "prefab", "id": "nothing"})).ok);
}
