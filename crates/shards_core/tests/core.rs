//! The loading gate, engine states, orders and presses, driven like a game drives them.

use std::time::Duration;

use ai_core::prelude::*;
use ai_core::{call_tool, AiJournal, CallOutcome, JournalKind, SimGate};
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use serde_json::{json, Value};
use shards_core::prelude::*;
use shards_core::{CorePlugin, EngineModules};

/// Whether the test's own "assets" are loaded.
#[derive(Resource, Default)]
struct Loaded(bool);

fn loaded(world: &World) -> LoadStatus {
    if world.resource::<Loaded>().0 { LoadStatus::Ready } else { LoadStatus::Pending("3 files left".into()) }
}

/// Presses of "jump" consumed so far.
#[derive(Resource, Default)]
struct Jumps(u32);

fn jump(mut q: Query<&mut Intent>, mut jumps: ResMut<Jumps>) {
    for mut intent in &mut q {
        while intent.take_press(input_names::JUMP).is_some() {
            jumps.0 += 1;
        }
    }
}

/// Moves characters along `move_dir` at 6 m/s, like a game's movement would.
fn walk(time: Res<Time>, mut q: Query<(&Intent, &mut Transform)>) {
    for (intent, mut tf) in &mut q {
        tf.translation += intent.move_dir * 6.0 * time.delta_secs();
    }
}

#[derive(Component)]
struct Hero;

fn spawn_hero(mut commands: Commands) {
    commands.spawn((Hero, AiPlayer, AiKind::new("Hero"), Intent::default(), Transform::default()));
}

fn app(config: EngineConfig) -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, StatesPlugin))
        .add_plugins(config.ai_core_plugin())
        .add_plugins(CorePlugin { config })
        .init_resource::<Loaded>()
        .init_resource::<Jumps>()
        .require_loaded("assets", loaded)
        .add_systems(OnEnter(EngineState::InGame), spawn_hero)
        .add_systems(FixedUpdate, (jump.in_set(SimSet::Act), walk.in_set(SimSet::Move)));
    app.finish();
    app.cleanup();
    app
}

fn test_config() -> EngineConfig {
    EngineConfig { headless: true, port: None, paused: true, crash_replays: false, testing: true, ..default() }
}

fn act(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "dev", tool, args)
}

fn ok(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    let out = act(app, tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    out
}

fn state(app: &App) -> EngineState {
    *app.world().resource::<State<EngineState>>().get()
}

/// Updates until the game is in `InGame` and the simulation is free.
fn load(app: &mut App) {
    app.world_mut().resource_mut::<Loaded>().0 = true;
    for _ in 0..10 {
        app.update();
        if state(app) == EngineState::InGame && !app.world().resource::<SimGate>().is_held() {
            return;
        }
    }
    panic!("still loading: {:?}", app.world().resource::<SimGate>().reasons());
}

#[test]
fn nothing_ticks_until_loaded_and_loading_never_shifts_a_tick() {
    let mut app = app(EngineConfig { paused: false, ..test_config() });
    app.insert_resource(bevy::time::TimeUpdateStrategy::ManualDuration(Duration::from_millis(50)));
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(state(&app), EngineState::Loading);
    assert_eq!(ai_core::sim::tick(app.world()), 0, "no tick while loading, in real time");
    let status = ai_core::status::status(app.world());
    assert_eq!(status["loading"], json!(["assets: 3 files left"]));
    assert_eq!(status["engine"]["state"], "Loading");
    let refused = act(&mut app, "step", json!({"ticks": 10}));
    assert_eq!(refused.result, json!("the game is still loading (assets: 3 files left); wait with `shards wait`"));
    let resume = act(&mut app, "resume", json!({}));
    assert!(!resume.ok && resume.result.as_str().unwrap().starts_with("the game is still loading"), "{}", resume.result);

    app.world_mut().resource_mut::<Loaded>().0 = true;
    app.update();
    assert_eq!(state(&app), EngineState::InGame, "loaded: the game enters InGame");
    assert_eq!(ai_core::sim::tick(app.world()), 0, "the world is spawned before the first tick");
    let hero = app.world_mut().query_filtered::<Entity, With<Hero>>().single(app.world()).is_ok();
    assert!(hero, "OnEnter(InGame) spawned the world");
    app.update();
    assert!(ai_core::sim::tick(app.world()) > 0, "then real time runs");
    assert_eq!(ai_core::status::status(app.world())["loading"], json!([]));
    let journal = app.world().resource::<AiJournal>();
    assert!(journal.after(0).0.any(|e| e.kind == JournalKind::Sim && e.text == "state Loading -> InGame"));
}

#[test]
fn a_paused_game_stays_paused_after_loading() {
    let mut app = app(test_config());
    load(&mut app);
    for _ in 0..3 {
        app.update();
    }
    assert!(ai_core::sim::is_paused(app.world()));
    assert_eq!(ai_core::sim::tick(app.world()), 0);
    ok(&mut app, "step", json!({"ticks": 5}));
    assert_eq!(ai_core::sim::tick(app.world()), 5);
}

#[test]
fn after_loading_can_be_the_main_menu() {
    let mut app = app(EngineConfig { after_loading: EngineState::MainMenu, ..test_config() });
    app.world_mut().resource_mut::<Loaded>().0 = true;
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(state(&app), EngineState::MainMenu);
    assert!(app.world().get_resource::<State<PauseState>>().is_none(), "PauseState only exists in game");
    assert!(!app.world().resource::<SimGate>().is_held());
}

#[test]
fn presses_are_consumed_in_the_tick_or_dropped_at_its_end() {
    let mut app = app(test_config());
    load(&mut app);
    let hero = app.world_mut().query_filtered::<Entity, With<Hero>>().single(app.world()).unwrap();
    {
        let mut intent = app.world_mut().get_mut::<Intent>(hero).unwrap();
        intent.press(input_names::JUMP);
        intent.press("wave"); // nobody consumes it
    }
    ok(&mut app, "step", json!({"ticks": 1}));
    assert_eq!(app.world().resource::<Jumps>().0, 1);
    assert!(app.world().get::<Intent>(hero).unwrap().pressed.is_empty(), "the unconsumed press is gone");
    ok(&mut app, "step", json!({"ticks": 3}));
    assert_eq!(app.world().resource::<Jumps>().0, 1, "a press counts once");
}

#[test]
fn move_to_walks_in_a_straight_line_and_ends_on_arrival() {
    let mut app = app(test_config());
    load(&mut app);
    let hero = app.world_mut().query_filtered::<Entity, With<Hero>>().single(app.world()).unwrap();
    let order = Order::MoveTo { target: Target::Point(Vec3::new(3.0, 0.0, -4.0)), within: 0.5 };
    app.world_mut().get_mut::<Intent>(hero).unwrap().orders.push_back(order);
    let seen = ok(&mut app, "observe", json!({}));
    assert!(seen.result.as_str().unwrap().contains("order move_to (3.0, -4.0)"), "{}", seen.result);
    ok(&mut app, "step", json!({"ticks": 60}));
    let intent = app.world().get::<Intent>(hero).unwrap();
    assert!(intent.orders.is_empty() && intent.move_dir == Vec3::ZERO, "{intent:?}");
    let at = app.world().get::<Transform>(hero).unwrap().translation;
    assert!(at.distance(Vec3::new(3.0, 0.0, -4.0)) <= 0.5 + 0.1, "{at}");
}

#[test]
fn the_pause_menu_pauses_single_player_and_a_resume_closes_it() {
    let mut app = app(EngineConfig { paused: false, ..test_config() });
    load(&mut app);
    assert!(!ai_core::sim::is_paused(app.world()));
    app.world_mut().resource_mut::<NextState<PauseState>>().set(PauseState::Paused);
    app.update();
    assert!(ai_core::sim::is_paused(app.world()), "the menu pauses the simulation");
    ok(&mut app, "resume", json!({}));
    app.update();
    assert_eq!(app.world().resource::<State<PauseState>>().get(), &PauseState::Running);
    assert!(!ai_core::sim::is_paused(app.world()));

    // Leaving the menu resumes what the menu paused.
    app.world_mut().resource_mut::<NextState<PauseState>>().set(PauseState::Paused);
    app.update();
    app.world_mut().resource_mut::<NextState<PauseState>>().set(PauseState::Running);
    app.update();
    assert!(!ai_core::sim::is_paused(app.world()));
    let journal: Vec<String> = app.world().resource::<AiJournal>().after(0).0.map(|e| e.text.clone()).collect();
    assert!(journal.iter().any(|t| t == "state Running -> Paused"), "{journal:?}");
}

#[test]
fn status_names_the_engine_and_its_modules() {
    let mut app = app(test_config());
    load(&mut app);
    app.add_engine_module("content", "9.9.9");
    let engine = &ai_core::status::status(app.world())["engine"];
    assert_eq!(engine["version"], shards_core::VERSION);
    assert_eq!(engine["state"], "InGame");
    assert_eq!(engine["pause"], "Running");
    let names: Vec<&str> = engine["modules"].as_array().unwrap().iter().map(|m| m["name"].as_str().unwrap()).collect();
    assert_eq!(names, ["core", "content"]);
    assert!(app.world().resource::<EngineModules>().contains("core"));
}
