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
    EngineConfig { headless: true, port: None, paused: true, crash_replays: false, report_perf: false, testing: true, ..default() }
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
    // An action would run on a world not spawned yet, and its replay on a spawned one.
    let teleport = act(&mut app, "teleport", json!({"id": "e1", "x": 1, "z": 1}));
    assert_eq!(teleport.result, json!("the game is still loading (assets: 3 files left); wait with `shards wait`"));
    assert!(ai_core::replay::save(app.world()).events.is_empty(), "and it is not recorded");

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
    let mut app = app(menu_config());
    app.world_mut().resource_mut::<Loaded>().0 = true;
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(state(&app), EngineState::MainMenu);
    assert!(app.world().get_resource::<State<PauseState>>().is_none(), "PauseState only exists in game");
    assert!(!app.world().resource::<SimGate>().is_held());
}

/// A game with a main menu, in a run that shows it.
fn menu_config() -> EngineConfig {
    EngineConfig { after_loading: EngineState::MainMenu, menu: MenuLaunch::Show, ..test_config() }
}

/// Updates until the loading gate lets go: the game is in its main menu, or in play.
fn load_to_menu(app: &mut App) {
    app.world_mut().resource_mut::<Loaded>().0 = true;
    for _ in 0..10 {
        app.update();
        if state(app) != EngineState::Loading && !app.world().resource::<SimGate>().is_held() {
            return;
        }
    }
    panic!("still loading: {:?}", app.world().resource::<SimGate>().reasons());
}

fn heroes(app: &mut App) -> usize {
    app.world_mut().query_filtered::<Entity, With<Hero>>().iter(app.world()).count()
}

#[test]
fn a_main_menu_shows_only_in_runs_a_person_may_be_at() {
    let windowed = EngineConfig { after_loading: EngineState::MainMenu, ..default() };
    assert_eq!(windowed.start_state(), EngineState::MainMenu, "a window shows it");
    let start = |config: EngineConfig| config.start_state();
    assert_eq!(start(EngineConfig { headless: true, ..windowed.clone() }), EngineState::InGame, "headless goes straight in");
    assert_eq!(start(EngineConfig { headless: true, offscreen: true, ..windowed.clone() }), EngineState::MainMenu, "offscreen draws");
    assert_eq!(start(EngineConfig { testing: true, ..windowed.clone() }), EngineState::InGame, "tests go straight in");
    assert_eq!(start(EngineConfig { world_slot: Some("slot1".into()), ..windowed.clone() }), EngineState::InGame, "a named save is played");
    assert_eq!(start(EngineConfig { character_slot: Some("slot1".into()), ..windowed.clone() }), EngineState::InGame);
    assert_eq!(start(EngineConfig { menu: MenuLaunch::Skip, ..windowed.clone() }), EngineState::InGame, "--no-menu");
    let no_ui = EngineConfig { without: vec!["locale".into(), "ui".into()], ..windowed.clone() };
    assert_eq!(start(no_ui.clone()), EngineState::InGame, "no screens to show it with");
    assert_eq!(start(EngineConfig { menu: MenuLaunch::Show, ..no_ui }), EngineState::MainMenu, "--menu: an agent leaves it with new_game");
    assert_eq!(start(EngineConfig { menu: MenuLaunch::Show, ..test_config() }), EngineState::InGame, "a game without a menu has none to show");
    assert_eq!(start(menu_config()), EngineState::MainMenu, "--menu shows it headless");
}

#[test]
fn a_main_menu_headless_by_default_is_passed_over() {
    let mut app = app(EngineConfig { after_loading: EngineState::MainMenu, ..test_config() });
    load(&mut app);
    assert_eq!(heroes(&mut app), 1, "the game's world, as without a menu");
    let journal: Vec<String> = app.world().resource::<AiJournal>().after(0).0.map(|e| e.text.clone()).collect();
    assert!(journal.iter().any(|t| t == "state Loading -> InGame"), "{journal:?}");
}

#[test]
fn time_stands_in_the_main_menu_until_new_game_starts_the_game() {
    let mut app = app(EngineConfig { paused: false, ..menu_config() });
    load_to_menu(&mut app);
    assert_eq!(state(&app), EngineState::MainMenu);
    for _ in 0..3 {
        app.update();
    }
    assert!(ai_core::sim::is_paused(app.world()), "no tick runs in the menu");
    assert_eq!(ai_core::sim::tick(app.world()), 0);
    assert_eq!(heroes(&mut app), 0, "the game's world waits for the game");

    let out = ok(&mut app, "new_game", json!({}));
    assert_eq!(state(&app), EngineState::InGame, "{}", out.result);
    assert_eq!(heroes(&mut app), 1, "spawned inside the call");
    assert!(!ai_core::sim::is_paused(app.world()), "time runs in the game");
    assert!(app.world().get_resource::<State<PauseState>>().is_some());

    let again = act(&mut app, "new_game", json!({}));
    assert!(!again.ok, "{}", again.result);
    assert!(again.result.as_str().unwrap().contains("on already"), "{}", again.result);
}

#[test]
fn a_paused_game_stays_paused_past_its_main_menu() {
    let mut app = app(menu_config());
    load_to_menu(&mut app);
    ok(&mut app, "new_game", json!({}));
    assert_eq!(state(&app), EngineState::InGame);
    assert!(ai_core::sim::is_paused(app.world()), "--paused: time runs only by step");
    ok(&mut app, "step", json!({"ticks": 2}));
    assert_eq!(ai_core::sim::tick(app.world()), 2);
}

#[test]
fn new_game_refuses_outside_the_main_menu_and_records_nothing() {
    let mut app = app(test_config());
    load(&mut app);
    let out = act(&mut app, "new_game", json!({}));
    assert!(!out.ok);
    assert!(out.result.as_str().unwrap().contains("on already"), "{}", out.result);
    assert_eq!(heroes(&mut app), 1);
}

#[test]
fn ticks_run_in_the_main_menu_restart_the_recording_when_the_game_starts() {
    let mut moved = app(menu_config());
    load_to_menu(&mut moved);
    ok(&mut moved, "step", json!({"ticks": 3}));
    assert_eq!(state(&moved), EngineState::MainMenu);
    ok(&mut moved, "new_game", json!({}));
    let journal: Vec<String> = moved.world().resource::<AiJournal>().after(0).0.map(|e| e.text.clone()).collect();
    assert!(journal.iter().any(|t| t.starts_with("the recording starts at tick 3")), "{journal:?}");

    // A menu left as it was entered keeps the recording from tick 0.
    let mut still = app(menu_config());
    load_to_menu(&mut still);
    ok(&mut still, "new_game", json!({}));
    let journal: Vec<String> = still.world().resource::<AiJournal>().after(0).0.map(|e| e.text.clone()).collect();
    assert!(!journal.iter().any(|t| t.starts_with("the recording starts")), "{journal:?}");
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

/// Times `OnEnter(RegionLoading)` ran: the loading screen of a region change.
#[derive(Resource, Default)]
struct RegionScreens(u32);

#[test]
fn a_region_trip_keeps_the_game_and_holds_ticks_until_loaded() {
    let mut app = app(test_config());
    app.init_resource::<RegionScreens>()
        .add_systems(OnEnter(EngineState::RegionLoading), |mut n: ResMut<RegionScreens>| n.0 += 1);
    load(&mut app);
    ok(&mut app, "step", json!({"ticks": 3}));
    let heroes = |app: &mut App| app.world_mut().query_filtered::<Entity, With<Hero>>().iter(app.world()).count();
    assert_eq!(heroes(&mut app), 1);

    // A region change: something must load again before the game goes on.
    app.world_mut().resource_mut::<Loaded>().0 = false;
    app.world_mut().resource_mut::<NextState<EngineState>>().set(EngineState::RegionLoading);
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(state(&app), EngineState::RegionLoading);
    assert_eq!(app.world().resource::<RegionScreens>().0, 1, "the loading screen came up");
    let refused = act(&mut app, "step", json!({"ticks": 1}));
    assert!(!refused.ok, "no tick while the region loads: {}", refused.result);
    assert!(app.world().get_resource::<State<PauseState>>().is_none());

    load(&mut app);
    assert_eq!(heroes(&mut app), 1, "OnEnter(InGame) did not run again: the game kept its hero");
    assert_eq!(app.world().resource::<State<PauseState>>().get(), &PauseState::Running);
    ok(&mut app, "step", json!({"ticks": 2}));
    assert_eq!(ai_core::sim::tick(app.world()), 5, "loading shifted no tick");
    let journal: Vec<String> = app.world().resource::<AiJournal>().after(0).0.map(|e| e.text.clone()).collect();
    assert!(journal.iter().any(|t| t == "state InGame -> RegionLoading"), "{journal:?}");
    assert!(journal.iter().any(|t| t == "state RegionLoading -> InGame"), "{journal:?}");

    // A real exit from the game still runs its schedules: back in, the game starts anew.
    app.world_mut().resource_mut::<NextState<EngineState>>().set(EngineState::MainMenu);
    app.update();
    app.world_mut().resource_mut::<NextState<EngineState>>().set(EngineState::InGame);
    app.update();
    assert_eq!(heroes(&mut app), 2, "entering InGame from the menu spawns the world");
}

/// What ran, tick by tick: one entry per `SimSet` (and the prediction set) that ran a system.
#[derive(Resource, Default)]
struct Ran(Vec<&'static str>);

#[test]
fn a_client_runs_no_sim_set_system_only_its_prediction() {
    let mut app = app(test_config());
    app.init_resource::<Ran>();
    let sets = [
        ("input", SimSet::Input),
        ("think", SimSet::Think),
        ("act", SimSet::Act),
        ("move", SimSet::Move),
        ("resolve", SimSet::Resolve),
        ("cleanup", SimSet::Cleanup),
    ];
    for (name, set) in sets {
        app.add_systems(FixedUpdate, (move |mut ran: ResMut<Ran>| ran.0.push(name)).in_set(set));
    }
    app.add_systems(
        FixedUpdate,
        (|mut ran: ResMut<Ran>| ran.0.push("predict")).in_set(NetSet::Predict),
    );
    load(&mut app);
    let hero = app.world_mut().query_filtered::<Entity, With<Hero>>().single(app.world()).unwrap();
    let order = Order::MoveTo { target: Target::Point(Vec3::new(3.0, 0.0, -4.0)), within: 0.5 };
    app.world_mut().get_mut::<Intent>(hero).unwrap().orders.push_back(order);

    ok(&mut app, "step", json!({"ticks": 1}));
    let single = std::mem::take(&mut app.world_mut().resource_mut::<Ran>().0);
    assert_eq!(single, ["input", "think", "act", "move", "resolve", "cleanup"], "single player simulates, predicts nothing");
    let moved = app.world().get::<Transform>(hero).unwrap().translation;
    assert_ne!(moved, Vec3::ZERO, "follow_orders steered and the game walked");

    app.insert_resource(NetRole::Client);
    ok(&mut app, "step", json!({"ticks": 3}));
    assert_eq!(app.world().resource::<Ran>().0, ["predict"; 3], "a client runs no SimSet system");
    assert_eq!(ai_core::sim::tick(app.world()), 4, "its ticks still count");
    assert_eq!(app.world().get::<Transform>(hero).unwrap().translation, moved, "no gameplay moved the hero");
    assert_eq!(app.world().get::<Intent>(hero).unwrap().orders.len(), 1, "nor did follow_orders run");

    // Each machine marks its own predicted body: it is not replicated.
    app.world_mut().entity_mut(hero).insert(Predicted);
    let replicated = ai_core::replicated_components(app.world(), hero);
    assert!(replicated.iter().any(|c| c.ends_with("::Intent")), "{replicated:?}");
    assert!(!replicated.iter().any(|c| c.ends_with("::Predicted") || c.ends_with("::LocalPlayer")), "{replicated:?}");
}

#[test]
fn an_agent_bound_to_a_player_finds_its_body_after_a_respawn() {
    let mut app = app(test_config());
    load(&mut app);
    let looks = |app: &mut App| {
        let seen = call_tool(app.world_mut(), "p1", "observe", json!({}));
        seen.result.as_str().unwrap().lines().find(|l| l.starts_with("YOU")).unwrap_or_default().to_owned()
    };
    let refused = call_tool(app.world_mut(), "p1", "possess", json!({"player": 1}));
    assert_eq!(refused.result, "there is no player 1 with a body now; players: none");

    let first = app.world_mut().spawn((PlayerId(1), AiKind::new("Guest"), Intent::default(), Transform::default())).id();
    let bound = call_tool(app.world_mut(), "p1", "possess", json!({"player": 1}));
    assert!(bound.ok, "{}", bound.result);
    let id = |app: &App, e: Entity| app.world().get::<AiId>(e).unwrap().to_string();
    let first_id = id(&app, first);
    assert!(looks(&mut app).starts_with(&format!("YOU  {first_id} Guest")), "{}", looks(&mut app));

    app.world_mut().despawn(first);
    let respawned = app.world_mut().spawn((PlayerId(1), AiKind::new("Guest"), Intent::default(), Transform::default())).id();
    let respawned_id = id(&app, respawned);
    assert_ne!(first_id, respawned_id);
    assert!(looks(&mut app).starts_with(&format!("YOU  {respawned_id} Guest")), "{}", looks(&mut app));
    let dev = call_tool(app.world_mut(), "dev", "observe", json!({}));
    assert!(dev.result.as_str().unwrap().contains("YOU  e1 Hero"), "other agents keep the AiPlayer body");
}

/// What the exit hooks did, in order.
#[derive(Resource, Default)]
struct Exited(Vec<&'static str>);

#[test]
fn exit_hooks_run_once_in_order_when_the_game_ends() {
    let mut app = app(test_config());
    app.add_exit_hook("save", |w| w.get_resource_or_init::<Exited>().0.push("save"))
        .add_exit_hook("say goodbye", |w| w.get_resource_or_init::<Exited>().0.push("bye"));
    load(&mut app);
    ok(&mut app, "step", json!({"ticks": 2}));
    assert!(app.world().get_resource::<Exited>().is_none(), "nothing ends while the game runs");
    let names: Vec<&str> = app.world().resource::<shards_core::ExitHooks>().names().collect();
    assert_eq!(names, ["save", "say goodbye"]);

    app.world_mut().write_message(AppExit::Success);
    app.update();
    assert_eq!(app.world().resource::<Exited>().0, ["save", "bye"]);
    assert!(app.should_exit().is_some());
    app.update();
    app.world_mut().write_message(AppExit::from_code(3));
    app.update();
    shards_core::run_exit_hooks(app.world_mut());
    assert_eq!(app.world().resource::<Exited>().0, ["save", "bye"], "once, however often the game ends");
}

/// `quit` ends the game from outside the way an `AppExit` does: the answer first, then the hooks.
#[test]
fn quit_answers_then_ends_the_game_through_its_exit_hooks() {
    let mut app = app(test_config());
    app.add_exit_hook("save", |w| w.get_resource_or_init::<Exited>().0.push("save"));
    load(&mut app);
    let said = ok(&mut app, "quit", json!({}));
    assert_eq!(said.result, json!("quitting: the game ends in 0.2s, after its exit hooks"));
    app.update();
    assert!(app.should_exit().is_none() && app.world().get_resource::<Exited>().is_none(), "not yet: the answer gets out first");
    std::thread::sleep(Duration::from_millis(250));
    app.update();
    assert_eq!(app.should_exit(), Some(AppExit::Success));
    assert_eq!(app.world().resource::<Exited>().0, ["save"]);
}
