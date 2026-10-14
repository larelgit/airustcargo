//! Content the way a game uses it: files in an asset folder, loaded by the asset
//! server behind the loading gate, read by gameplay, edited while the game runs, and
//! replayed.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ai_core::prelude::*;
use ai_core::replay::{self, ReplayEvent};
use ai_core::{call_tool, AiJournal, AiReplay, CallOutcome, JournalKind, SimGate};
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shards_content::prelude::*;
use shards_content::{check_dir, spawn_prefab, ContentKinds};
use shards_core::{CorePlugin, EngineConfig, EngineState};

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
struct AttackDef {
    damage: f32,
    #[serde(default)]
    windup: f32,
}

impl ContentDef for AttackDef {
    const KIND: &'static str = "attack";

    fn validate(&self, _: &str, refs: &mut Refs) {
        if self.damage < 0.0 {
            refs.error("damage cannot be negative");
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct MovesetDef {
    light: String,
    heavy: Option<String>,
}

impl ContentDef for MovesetDef {
    const KIND: &'static str = "moveset";

    fn validate(&self, _: &str, refs: &mut Refs) {
        refs.require::<AttackDef>(&self.light, "light");
        if let Some(heavy) = &self.heavy {
            refs.require::<AttackDef>(heavy, "heavy");
        }
    }
}

#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component)]
struct Health {
    current: f32,
    max: f32,
}

#[derive(Component, Reflect, Default, Clone, Debug, PartialEq)]
#[reflect(Component, Default)]
struct Brain {
    aggro: f32,
    cooldown: f32,
}

#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
struct Monster;

/// What the sword hits for, as gameplay saw it in the last tick (hashed in replays).
#[derive(Component, Reflect, Default, Clone, Debug)]
#[reflect(Component, Default)]
struct SwordDamage(f32);

fn read_sword(attacks: Content<AttackDef>, mut q: Query<&mut SwordDamage>) {
    for mut seen in &mut q {
        seen.0 = attacks.get("sword").map_or(-1.0, |a| a.damage);
    }
}

fn spawn_dummy(mut commands: Commands) {
    commands.spawn((AiKind::new("Dummy"), SwordDamage::default(), Transform::default()));
}

const ATTACKS: &str = r#"{
    "sword": (damage: 25.0, windup: 0.15),
    "claw": (damage: 8.0),
}"#;

const MOVESETS: &str = r#"{
    "hero": (light: "sword", heavy: "sword"),
}"#;

const PREFABS: &str = r#"{
    "skeleton": (
        components: {
            "Name": "Skeleton",
            "AiKind": ("Skeleton"),
            "Monster": (),
            "Health": (current: 40.0, max: 40.0),
            "Brain": (aggro: 18.0),
        },
    ),
    "skeleton_captain": (
        extends: "skeleton",
        components: { "Health": (current: 90.0, max: 90.0), "Name": "Skeleton Captain" },
    ),
}"#;

/// An asset folder of its own for each test.
fn assets(name: &str, files: &[(&str, &str)]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("shards_content_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    for (path, text) in files {
        write(&dir, path, text);
    }
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write(dir: &Path, path: &str, text: &str) {
    let file = dir.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

fn kinds() -> ContentKinds {
    let mut kinds = ContentKinds::default();
    kinds.register::<AttackDef>();
    kinds.register::<MovesetDef>();
    kinds.register::<PrefabDef>();
    kinds
}

/// A headless app on `dir`, loaded and paused at tick 0.
fn app(dir: &Path) -> App {
    app_with(dir, false)
}

fn app_with(dir: &Path, watch: bool) -> App {
    let mut app = unloaded(dir, watch, |_| {});
    load(&mut app);
    app
}

/// The test app on `dir` with `setup` added, built but not loaded yet.
fn unloaded(dir: &Path, watch: bool, setup: impl FnOnce(&mut App)) -> App {
    let asset_dir = dir.to_string_lossy().into_owned();
    let config = EngineConfig {
        headless: true,
        port: None,
        paused: true,
        crash_replays: false,
        testing: true,
        asset_dir: asset_dir.clone(),
        ..default()
    };
    let mut app = App::new();
    let assets = AssetPlugin { file_path: asset_dir, watch_for_changes_override: Some(watch), ..default() };
    app.add_plugins((MinimalPlugins, assets, StatesPlugin))
        .add_plugins(config.ai_core_plugin())
        .add_plugins(CorePlugin { config })
        .add_plugins(ContentPlugin)
        .add_content::<AttackDef>()
        .add_content::<MovesetDef>()
        // A game gets these from `reflect_auto_register`; this test app builds Bevy without it.
        .register_type::<Name>()
        .register_type::<AiKind>()
        .register_type::<Health>()
        .register_type::<Brain>()
        .register_type::<Monster>()
        .register_type::<SwordDamage>()
        .add_systems(OnEnter(EngineState::InGame), spawn_dummy)
        .add_systems(FixedUpdate, read_sword.in_set(SimSet::Act));
    setup(&mut app);
    app.finish();
    app.cleanup();
    app
}

/// Updates `app` until it is in the game and the simulation is free.
fn load(app: &mut App) {
    let started = Instant::now();
    while *app.world().resource::<State<EngineState>>().get() != EngineState::InGame
        || app.world().resource::<SimGate>().is_held()
    {
        assert!(started.elapsed() < Duration::from_secs(20), "still loading: {:?}", app.world().resource::<SimGate>().reasons());
        app.update();
        std::thread::sleep(Duration::from_millis(1));
    }
}

/// Updates `app` (the file watcher works meanwhile) until `done`, or fails naming `what`.
#[cfg(feature = "hot_reload")]
fn wait_for(app: &mut App, what: &str, done: impl Fn(&App) -> bool) {
    let started = Instant::now();
    while !done(app) {
        assert!(started.elapsed() < Duration::from_secs(20), "{what} never happened: {:?}", app.world().resource::<AiJournal>().after(0).0.map(ToString::to_string).collect::<Vec<_>>());
        app.update();
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// `OnEnter(InGame)`, after the dummy: a game that spawns its world from content.
fn spawn_skeleton(world: &mut World) {
    spawn_prefab(world, "skeleton", Transform::default(), None).unwrap();
}

fn health(app: &App, id: u32) -> Health {
    let entity = app.world().resource::<ai_core::AiIndex>().get(AiId(id)).unwrap();
    app.world().get::<Health>(entity).unwrap().clone()
}

fn act(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "dev", tool, args)
}

fn ok(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    let out = act(app, tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    out
}

fn text(out: &CallOutcome) -> String {
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

fn errors(app: &App) -> Vec<String> {
    app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect()
}

fn journal(app: &App, kind: JournalKind) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.filter(|e| e.kind == kind).map(|e| e.text.clone()).collect()
}

#[test]
fn definitions_load_by_id_with_their_files() {
    let dir = assets("load", &[("content/combat/attacks.attack.ron", ATTACKS), ("content/hero.moveset.ron", MOVESETS)]);
    let mut app = app(&dir);
    let db = app.world().resource::<ContentDb>();
    assert_eq!(db.get::<AttackDef>("sword"), Some(&AttackDef { damage: 25.0, windup: 0.15 }));
    assert_eq!(db.ids::<AttackDef>().collect::<Vec<_>>(), ["claw", "sword"]);
    assert_eq!(db.file_of::<AttackDef>("claw"), Some("content/combat/attacks.attack.ron"));
    assert_eq!(db.get_json("attack", "sword"), Some(&json!({"damage": 25.0, "windup": 0.15})), "floats keep their short form");
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    assert!(journal(&app, JournalKind::Event).contains(&"content loaded: 3 definitions in 2 files".to_owned()));

    let listed = text(&ok(&mut app, "content_list", json!({})));
    assert_eq!(listed, "content: 3 definitions in 2 files\n  attack  2\n  moveset  1\n  prefab  0");
    let attacks = text(&ok(&mut app, "content_list", json!({"kind": "attack"})));
    assert_eq!(attacks, "attack: 2 definitions\n  claw  combat/attacks.attack.ron\n  sword  combat/attacks.attack.ron");
    let sword = ok(&mut app, "content_get", json!({"kind": "attack", "id": "sword"})).result;
    assert_eq!(sword, json!({"kind": "attack", "id": "sword", "file": "content/combat/attacks.attack.ron", "def": {"damage": 25.0, "windup": 0.15}}));
    let typo = act(&mut app, "content_get", json!({"kind": "attack", "id": "swrod"}));
    assert_eq!(text(&typo), "no attack `swrod` (did you mean `sword`?); `content_list kind=attack` lists them");
    let kind = act(&mut app, "content_list", json!({"kind": "atack"}));
    assert_eq!(text(&kind), "no content kind `atack` (did you mean `attack`?); kinds: attack, moveset, prefab");
    assert_eq!(text(&ok(&mut app, "content_check", json!({}))), "ok: 3 definitions in 2 files");
    assert_eq!(ai_core::status::status(app.world())["engine"]["modules"][1]["name"], "content");
}

#[test]
fn duplicates_name_both_files_and_the_first_one_wins() {
    let other = r#"{ "sword": (damage: 99.0) }"#;
    let dir = assets("duplicates", &[("content/a.attack.ron", ATTACKS), ("content/b.attack.ron", other)]);
    let app = app(&dir);
    assert_eq!(errors(&app), ["content/b.attack.ron: attack `sword`: already defined in content/a.attack.ron"]);
    assert_eq!(app.world().resource::<ContentDb>().get::<AttackDef>("sword").unwrap().damage, 25.0);
    let journaled = journal(&app, JournalKind::Error);
    assert!(journaled.iter().any(|l| l.contains("b.attack.ron: attack `sword`: already defined")), "{journaled:?}");
}

#[test]
fn a_missing_reference_names_the_file_the_id_and_the_field() {
    let moveset = r#"{ "hero": (light: "sword_lite_1") }"#;
    let attacks = r#"{ "sword_light_1": (damage: 10.0), "sword_light_2": (damage: 12.0) }"#;
    let dir = assets("refs", &[("content/combat/sword.moveset.ron", moveset), ("content/combat/sword.attack.ron", attacks)]);
    let mut app = app(&dir);
    let expected = "content/combat/sword.moveset.ron: moveset `hero`: light -> unknown attack `sword_lite_1` (did you mean `sword_light_1`?)";
    assert_eq!(errors(&app), [expected]);
    assert!(app.world().resource::<ContentDb>().get::<MovesetDef>("hero").is_some(), "what parsed is still used");
    assert_eq!(text(&ok(&mut app, "content_check", json!({}))), format!("1 errors:\n  {expected}"));
}

#[test]
fn schema_errors_say_where_and_in_which_definition() {
    let broken = "{\n    \"sword\": (damage: 25.0),\n    \"claw\": (damge: 8.0),\n}";
    let misnamed = "{}";
    let dir = assets("schema", &[("content/attacks.attack.ron", broken), ("content/old.atack.ron", misnamed)]);
    let app = app(&dir);
    let errors = errors(&app);
    assert_eq!(errors.len(), 2, "{errors:?}");
    assert!(errors[0].starts_with("content/attacks.attack.ron:3:"), "{errors:?}");
    assert!(errors[0].contains("attack `claw`: ") && errors[0].contains("damge"), "{errors:?}");
    assert_eq!(errors[1], "content/old.atack.ron: no content kind `atack` (did you mean `attack`?); registered: attack, moveset, prefab");
    assert!(app.world().resource::<ContentDb>().get::<AttackDef>("sword").is_none(), "a file that does not parse adds nothing");
}

#[test]
fn check_dir_works_without_an_app() {
    let dir = assets("check", &[("content/combat/attacks.attack.ron", ATTACKS), ("content/hero.moveset.ron", MOVESETS)]);
    let summary = check_dir(&dir, &kinds()).unwrap();
    assert_eq!(summary.to_string(), "ok: 3 definitions in 2 files");
    write(&dir, "content/hero.moveset.ron", r#"{ "hero": (light: "axe") }"#);
    let errors = check_dir(&dir, &kinds()).unwrap_err();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0].to_string(), "content/hero.moveset.ron: moveset `hero`: light -> unknown attack `axe`");
    // Looking in the wrong place must not pass.
    let elsewhere = assets("check_elsewhere", &[]);
    let errors = check_dir(&elsewhere, &kinds()).unwrap_err();
    let expected = format!("no content folder at {} (games run from cargo", elsewhere.join("content").display());
    assert!(errors[0].to_string().starts_with(&expected), "{}", errors[0]);
    std::fs::create_dir_all(elsewhere.join("content")).unwrap();
    assert_eq!(check_dir(&elsewhere, &kinds()).unwrap().to_string(), "ok: 0 definitions in 0 files");
    let mut required = kinds();
    required.require::<AttackDef>("kick", "the test's moves");
    write(&dir, "content/hero.moveset.ron", MOVESETS);
    let errors = check_dir(&dir, &required).unwrap_err();
    assert_eq!(errors[0].to_string(), "attack `kick`: missing; the test's moves needs it (define it in a content/**/*.attack.ron file)");
}

#[test]
fn prefabs_spawn_with_extends_and_overrides() {
    let dir = assets("prefabs", &[("content/monsters.prefab.ron", PREFABS)]);
    let mut app = app(&dir);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    let out = ok(&mut app, "spawn_prefab", json!({"id": "skeleton_captain", "x": 3, "z": -4, "overrides": {"Health": {"current": 10}}}));
    assert_eq!(text(&out), "spawned e2 skeleton_captain at (3.0, -4.0)");
    let world = app.world();
    let captain = world.resource::<ai_core::AiIndex>().get(AiId(2)).unwrap();
    assert_eq!(world.get::<Health>(captain), Some(&Health { current: 10.0, max: 90.0 }), "the child's Health, patched");
    assert_eq!(world.get::<Name>(captain).map(Name::as_str), Some("Skeleton Captain"));
    assert_eq!(world.get::<Brain>(captain), Some(&Brain { aggro: 18.0, cooldown: 0.0 }), "the parent's Brain, defaults filled in");
    assert!(world.get::<Monster>(captain).is_some());
    assert_eq!(world.get::<Transform>(captain).unwrap().translation, Vec3::new(3.0, 0.0, -4.0));
    let inspect = text(&ok(&mut app, "inspect", json!({"id": "e2"})));
    assert!(inspect.starts_with("e2 Skeleton \"Skeleton Captain\""), "{inspect}");

    let bad = act(&mut app, "spawn_prefab", json!({"id": "skeleton", "x": 0, "z": 0, "overrides": {"Helth": {"current": 1}}}));
    assert!(text(&bad).starts_with("overrides: unknown component `Helth` (did you mean `Health`?)"), "{}", text(&bad));
    let unknown = act(&mut app, "spawn_prefab", json!({"id": "skeletn", "x": 0, "z": 0}));
    assert_eq!(text(&unknown), "unknown prefab `skeletn` (did you mean `skeleton`?); known: skeleton, skeleton_captain");
    assert_eq!(app.world().resource::<ai_core::AiIndex>().len(), 2, "nothing half-spawned");
}

#[test]
fn prefab_problems_are_found_when_content_loads() {
    let broken = r#"{
        "a": (extends: "b", components: { "Helth": (current: 1.0, max: 1.0) }),
        "b": (extends: "a", components: { "Health": (current: 1.0) }),
        "c": (extends: "nobody"),
        "d": (components: { "Brain": (agro: 1.0) }),
    }"#;
    let typo = r#"{ "e": (extend: "a") }"#;
    let dir = assets("prefab_errors", &[("content/broken.prefab.ron", broken), ("content/typo.prefab.ron", typo)]);
    let app = app(&dir);
    let errors = errors(&app);
    let has = |needle: &str| errors.iter().any(|e| e.contains(needle));
    assert!(has("prefab `a`: extends goes round in a circle: a -> b -> a"), "{errors:?}");
    assert!(has("prefab `a`: unknown component `Helth` (did you mean `Health`?)"), "{errors:?}");
    assert!(has("prefab `b`: Health is missing fields; give all of them: current, max"), "{errors:?}");
    assert!(has("prefab `c`: extends -> unknown prefab `nobody`"), "{errors:?}");
    // A position inside the component's text is none in the file: none is given.
    let brain = errors.iter().find(|e| e.contains("prefab `d`: Brain: ")).unwrap_or_else(|| panic!("{errors:?}"));
    assert!(brain.starts_with("content/broken.prefab.ron: prefab `d`: Brain: ") && brain.contains("agro"), "{brain}");
    assert!(!brain.contains(" 1:"), "{brain}");
    // A misspelled key is an error, not a prefab without a parent.
    let misspelled = errors.iter().find(|e| e.starts_with("content/typo.prefab.ron:1:")).unwrap_or_else(|| panic!("{errors:?}"));
    assert!(misspelled.contains("prefab `e`: ") && misspelled.contains("extend"), "{misspelled}");
}

#[test]
fn reformatting_a_prefab_changes_no_definition() {
    let dir = assets("prefab_format", &[("content/monsters.prefab.ron", PREFABS)]);
    let mut app = app(&dir);
    let hash = app.world().resource::<ContentDb>().hash();
    let health = "\"Health\": (current: 40.0, max: 40.0),";
    assert!(PREFABS.contains(health));
    let reformatted = PREFABS.replace(health, "\"Health\":   (\n  current: 40.0, // full\n  max: 40.0 ) /* hp */,");
    write(&dir, "content/monsters.prefab.ron", &reformatted);
    assert_eq!(text(&ok(&mut app, "content_reload", json!({}))), "content: no definitions changed in monsters.prefab.ron");
    assert_eq!(app.world().resource::<ContentDb>().hash(), hash, "replays recorded before still match");
    let skeleton = ok(&mut app, "content_get", json!({"kind": "prefab", "id": "skeleton"})).result;
    assert_eq!(skeleton["def"]["components"]["Health"], "(current:40.0,max:40.0)");
}

#[test]
fn a_reload_is_recorded_and_its_replay_plays_the_same_game() {
    let dir = assets("replay", &[("content/combat/attacks.attack.ron", ATTACKS)]);
    let mut app = app(&dir);
    ok(&mut app, "step", json!({"ticks": 70}));
    let dummy = app.world().resource::<ai_core::AiIndex>().get(AiId(1)).unwrap();
    assert_eq!(app.world().get::<SwordDamage>(dummy).unwrap().0, 25.0);

    write(&dir, "content/combat/attacks.attack.ron", &ATTACKS.replace("25.0", "40.0"));
    let reload = ok(&mut app, "content_reload", json!({"file": "combat/attacks.attack.ron"}));
    assert_eq!(text(&reload), "content reloaded: combat/attacks.attack.ron (1 definition changed)");
    assert!(journal(&app, JournalKind::Event).contains(&text(&reload)), "journaled");
    ok(&mut app, "step", json!({"ticks": 1}));
    assert_eq!(app.world().get::<SwordDamage>(dummy).unwrap().0, 40.0, "the next tick sees the new number");
    ok(&mut app, "step", json!({"ticks": 69}));

    let recording = replay::save(app.world());
    let calls: Vec<&str> = recording
        .events
        .iter()
        .filter_map(|e| match e {
            ReplayEvent::Call { tool, .. } => Some(tool.as_str()),
            ReplayEvent::Input { .. } => None,
        })
        .collect();
    assert_eq!(calls, ["content_apply"], "the reload is recorded as the content it applied");

    // The file keeps the new number; the replay still starts from the old one.
    let mut again = app_with(&dir, false);
    assert_eq!(again.world().resource::<ContentDb>().get::<AttackDef>("sword").unwrap().damage, 40.0);
    replay::play(again.world_mut(), recording.clone()).unwrap();
    ok(&mut again, "step", json!({"ticks": 1}));
    let dummy = again.world().resource::<ai_core::AiIndex>().get(AiId(1)).unwrap();
    assert_eq!(again.world().get::<SwordDamage>(dummy).unwrap().0, 25.0, "playback starts from the recorded version");
    ok(&mut again, "step", json!({"ticks": recording.end.tick - 1}));
    again.update();
    let report = again.world().resource::<AiReplay>().report().cloned().unwrap();
    assert!(report.identical(), "{report}");
    assert_eq!(again.world().get::<SwordDamage>(dummy).unwrap().0, 40.0);
}

#[test]
fn a_replay_started_before_loading_spawns_from_its_recorded_content() {
    let prefab = r#"{ "skeleton": (components: { "AiKind": ("Skeleton"), "Health": (current: 40.0, max: 40.0) }) }"#;
    let dir = assets("replay_spawn", &[("content/monsters.prefab.ron", prefab)]);
    let game = |app: &mut App| {
        app.add_systems(OnEnter(EngineState::InGame), spawn_skeleton.after(spawn_dummy));
    };
    let mut app = unloaded(&dir, false, game);
    load(&mut app);
    assert_eq!(health(&app, 2).max, 40.0);
    ok(&mut app, "step", json!({"ticks": 10}));
    write(&dir, "content/monsters.prefab.ron", &prefab.replace("40.0", "60.0"));
    ok(&mut app, "content_reload", json!({}));
    ok(&mut app, "step", json!({"ticks": 60}));
    let recording = replay::save(app.world());

    // The file says 60 now; the replay, started before loading, spawns what it recorded.
    let mut again = unloaded(&dir, false, game);
    replay::play(again.world_mut(), recording.clone()).unwrap();
    load(&mut again);
    assert_eq!(health(&again, 2).max, 40.0, "spawned from the recorded content");
    ok(&mut again, "step", json!({"ticks": recording.end.tick}));
    again.update();
    let report = again.world().resource::<AiReplay>().report().cloned().unwrap();
    assert!(report.identical(), "{report}");
}

#[test]
fn content_apply_checks_before_and_a_refused_call_stays_out_of_the_replay() {
    let dir = assets("before", &[("content/combat/attacks.attack.ron", ATTACKS)]);
    let mut app = app(&dir);
    let sword = |before: Option<Value>| {
        let mut change = json!({"kind": "attack", "id": "sword", "def": {"damage": 30.0}});
        if let Some(before) = before {
            change["before"] = before;
        }
        json!({"files": ["content/combat/attacks.attack.ron"], "changes": [change]})
    };
    let now = r#"{"damage":25.0,"windup":0.15}"#;
    let missing = act(&mut app, "content_apply", sword(None));
    assert_eq!(text(&missing), format!("attack `sword` exists: give its current definition as `before`: {now}"));
    let stale = act(&mut app, "content_apply", sword(Some(json!({"damage": 1.0}))));
    assert_eq!(text(&stale), format!("attack `sword` is not what `before` says; it is now {now}"));
    let axe = json!({"changes": [{"kind": "attack", "id": "axe", "def": {"damage": 5.0}, "before": {"damage": 5.0}}]});
    assert_eq!(text(&act(&mut app, "content_apply", axe)), "attack `axe` does not exist: leave `before` out for a new definition");
    assert!(replay::save(app.world()).events.is_empty(), "refused calls are not recorded");

    ok(&mut app, "step", json!({"ticks": 30}));
    ok(&mut app, "content_apply", sword(Some(json!({"damage": 25, "windup": 0.15})))); // 25 is 25.0
    assert_eq!(app.world().resource::<ContentDb>().get::<AttackDef>("sword").unwrap().damage, 30.0);
    ok(&mut app, "step", json!({"ticks": 40}));
    let recording = replay::save(app.world());
    assert_eq!(recording.events.len(), 1);
    let mut again = unloaded(&dir, false, |_| {});
    replay::play(again.world_mut(), recording.clone()).unwrap();
    load(&mut again);
    ok(&mut again, "step", json!({"ticks": recording.end.tick}));
    again.update();
    let report = again.world().resource::<AiReplay>().report().cloned().unwrap();
    assert!(report.identical(), "{report}");

    // A reload compares definitions, not texts: the file never changed, the sword did.
    let reload = ok(&mut app, "content_reload", json!({}));
    assert_eq!(text(&reload), "content reloaded: combat/attacks.attack.ron (1 definition changed)");
    assert_eq!(app.world().resource::<ContentDb>().get::<AttackDef>("sword").unwrap().damage, 25.0);
}

#[test]
fn a_required_id_is_an_error_and_cannot_be_deleted() {
    let dir = assets("required", &[("content/combat/attacks.attack.ron", ATTACKS)]);
    let mut app = unloaded(&dir, false, |app| {
        app.require_content::<AttackDef>(&["sword", "claw"], "AttackKind");
    });
    load(&mut app);
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
    write(&dir, "content/combat/attacks.attack.ron", r#"{ "sword": (damage: 25.0) }"#);
    let refused = act(&mut app, "content_reload", json!({}));
    assert_eq!(
        text(&refused),
        "attack `claw`: missing; AttackKind needs it (define it in a content/**/*.attack.ron file); \
         kept the previous version of combat/attacks.attack.ron"
    );
    assert!(app.world().resource::<ContentDb>().get::<AttackDef>("claw").is_some());
}

#[test]
fn a_broken_edit_is_refused_and_the_previous_version_kept() {
    let dir = assets("refused", &[("content/combat/attacks.attack.ron", ATTACKS), ("content/hero.moveset.ron", MOVESETS)]);
    let mut app = app(&dir);
    // Renaming the sword breaks the moveset that uses it.
    write(&dir, "content/combat/attacks.attack.ron", &ATTACKS.replace("\"sword\"", "\"saber\""));
    let refused = act(&mut app, "content_reload", json!({}));
    assert!(!refused.ok);
    assert_eq!(
        text(&refused),
        "content/hero.moveset.ron: moveset `hero`: light -> unknown attack `sword`; \
         content/hero.moveset.ron: moveset `hero`: heavy -> unknown attack `sword`; \
         kept the previous version of combat/attacks.attack.ron"
    );
    let journaled = journal(&app, JournalKind::Error);
    assert!(
        journaled.contains(&"content: content/hero.moveset.ron: moveset `hero`: light -> unknown attack `sword`; kept the previous version".to_owned()),
        "{journaled:?}"
    );
    assert!(app.world().resource::<ContentDb>().get::<AttackDef>("sword").is_some());
    assert!(replay::save(app.world()).events.is_empty(), "nothing applied, nothing recorded");

    // A file that does not parse is refused the same way.
    write(&dir, "content/combat/attacks.attack.ron", "{ \"sword\": (damage: }");
    let refused = act(&mut app, "content_reload", json!({"kind": "attack"}));
    assert!(text(&refused).starts_with("content/combat/attacks.attack.ron:1:"), "{}", text(&refused));
    assert_eq!(text(&ok(&mut app, "content_reload", json!({"file": "hero.moveset.ron"}))), "content: nothing changed on disk");
}

#[test]
fn removing_a_file_removes_its_definitions() {
    let dir = assets("removed", &[("content/combat/attacks.attack.ron", ATTACKS), ("content/extra.attack.ron", r#"{ "kick": (damage: 3.0) }"#)]);
    let mut app = app(&dir);
    std::fs::remove_file(dir.join("content/extra.attack.ron")).unwrap();
    let out = ok(&mut app, "content_reload", json!({}));
    assert_eq!(text(&out), "content reloaded: extra.attack.ron (1 definition changed)");
    assert!(app.world().resource::<ContentDb>().get::<AttackDef>("kick").is_none());
}

#[cfg(feature = "hot_reload")]
#[test]
fn an_edited_file_is_reloaded_while_the_game_runs() {
    let dir = assets("watch", &[("content/combat/attacks.attack.ron", ATTACKS)]);
    let mut app = app_with(&dir, true);
    // The watcher needs a moment to start before it sees changes.
    std::thread::sleep(Duration::from_millis(500));
    write(&dir, "content/combat/attacks.attack.ron", &ATTACKS.replace("25.0", "31.0"));
    let started = Instant::now();
    while app.world().resource::<ContentDb>().get::<AttackDef>("sword").unwrap().damage != 31.0 {
        assert!(started.elapsed() < Duration::from_secs(20), "the edit never arrived: {:?}", journal(&app, JournalKind::Event));
        app.update();
        std::thread::sleep(Duration::from_millis(10));
    }
    let events = journal(&app, JournalKind::Event);
    assert!(events.contains(&"content reloaded: combat/attacks.attack.ron (1 definition changed)".to_owned()), "{events:?}");
}

#[cfg(feature = "hot_reload")]
#[test]
fn an_edit_across_files_applies_once_its_last_file_is_saved() {
    let dir = assets("watch_rename", &[("content/combat/attacks.attack.ron", ATTACKS), ("content/hero.moveset.ron", MOVESETS)]);
    let mut app = app_with(&dir, true);
    std::thread::sleep(Duration::from_millis(500));
    // The sword becomes a saber: refused while the moveset still says sword...
    write(&dir, "content/combat/attacks.attack.ron", &ATTACKS.replace("\"sword\"", "\"saber\""));
    wait_for(&mut app, "the refusal", |app| journal(app, JournalKind::Error).iter().any(|l| l.contains("unknown attack `sword`")));
    // ...and applied with it once the moveset follows.
    write(&dir, "content/hero.moveset.ron", &MOVESETS.replace("sword", "saber"));
    wait_for(&mut app, "the saber", |app| app.world().resource::<ContentDb>().get::<AttackDef>("saber").is_some());
    let events = journal(&app, JournalKind::Event);
    let applied = "content reloaded: combat/attacks.attack.ron, hero.moveset.ron (3 definitions changed)";
    assert!(events.contains(&applied.to_owned()), "{events:?}");
}

#[cfg(feature = "hot_reload")]
#[test]
fn files_created_or_deleted_while_the_game_runs_are_applied() {
    let dir = assets("watch_files", &[("content/combat/attacks.attack.ron", ATTACKS)]);
    let mut app = app_with(&dir, true);
    std::thread::sleep(Duration::from_millis(500));
    write(&dir, "content/extra.attack.ron", r#"{ "kick": (damage: 3.0) }"#);
    wait_for(&mut app, "the new file", |app| app.world().resource::<ContentDb>().get::<AttackDef>("kick").is_some());
    std::fs::remove_file(dir.join("content/extra.attack.ron")).unwrap();
    wait_for(&mut app, "the deletion", |app| app.world().resource::<ContentDb>().get::<AttackDef>("kick").is_none());
    write(&dir, "content/old.atack.ron", "{}");
    wait_for(&mut app, "the misnamed file", |app| journal(app, JournalKind::Error).iter().any(|l| l.contains("no content kind `atack`")));
}
