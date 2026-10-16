//! The tools an agent (or a menu) saves and loads with.

use std::fs;

use serde::{Deserialize, Serialize};
use serde_json::json;
use shards::ai_core::{call_tool, AgentSessions, AiId, JournalKind, Scope, WorldRebuilt};
use shards::save::{SaveFile, SaveKind, SaveSection};

use shards::prelude::*;

use crate::common::*;

fn some_state(app: &mut App) {
    set_hero(app, Hero { name: "Ada".into(), level: 7, xp: 900, coins: Coins { gold: 5, silver: 2 }, ..Hero::default() });
    app.world_mut().resource_mut::<Calendar>().day = 40;
    set_chests(app, &[Chest { open: true, loot: vec![1, 2] }, Chest::default()]);
}

fn other_state(app: &mut App) {
    set_hero(app, Hero { name: "Bob".into(), ..Hero::default() });
    app.world_mut().resource_mut::<Calendar>().day = 1;
    set_chests(app, &[Chest::default()]);
}

#[test]
fn save_game_and_load_game_round_trip() {
    let dir = TempDir::new("tools");
    let mut app = game(&dir);
    some_state(&mut app);
    let out = ok(&mut app, "save_game", json!({"what": "all", "slot": "camp", "label": "by the ford"}));
    assert!(out.starts_with("ok: world/camp saved (save_game, ") && out.contains("; character/camp saved (save_game, "), "{out}");
    assert!(dir.file("world/camp.save").exists() && dir.file("character/camp.save").exists());

    other_state(&mut app);
    let out = ok(&mut app, "load_game", json!({"what": "all", "slot": "camp"}));
    assert!(out.starts_with("ok: world/camp loaded (saved at tick 0, ") && out.contains("; character/camp loaded"), "{out}");
    assert_eq!(hero(&mut app).name, "Ada");
    assert_eq!(calendar(&app).day, 40);
    assert_eq!(chests(&mut app)[0].1, Chest { open: true, loot: vec![1, 2] });

    // Only what was asked for.
    other_state(&mut app);
    ok(&mut app, "load_game", json!({"what": "character"}));
    assert_eq!((hero(&mut app).name, calendar(&app).day), ("Ada".to_owned(), 1), "the current slot is camp now; the world stayed");

    // `save_list` shows both, with the label.
    let list = act(&mut app, "save_list", json!({})).result;
    let summary: Vec<&str> = list["summary"].as_array().unwrap().iter().map(|l| l.as_str().unwrap()).collect();
    assert_eq!(summary.len(), 2, "{summary:?}");
    assert!(summary[0].starts_with("world/camp (current): save ok (tick 0, ") && summary[0].contains("\"by the ford\"") && summary[0].contains("bak1 missing"), "{summary:?}");
    assert_eq!(list["current"], json!({"world": "camp", "character": "camp"}));
}

#[test]
fn the_current_slot_follows_what_was_last_used() {
    let dir = TempDir::new("current");
    let mut app = game(&dir);
    ok(&mut app, "save_game", json!({"what": "world", "slot": "first"}));
    ok(&mut app, "save_game", json!({"what": "world"}));
    assert_eq!(fs::read_dir(dir.file("world")).unwrap().count(), 2, "first.save and its backup: the unnamed save went to `first`");
    ok(&mut app, "save_game", json!({"what": "world", "slot": "second"}));
    ok(&mut app, "save_game", json!({"what": "world"}));
    assert!(dir.file("world/second.save.bak1").exists());
}

#[test]
fn slot_names_are_validated_and_never_reach_outside_the_folder() {
    let dir = TempDir::new("slots");
    let mut app = game(&dir);
    for bad in ["../x", "a/b", "Bad Slot", "", "x.y", "é", &"y".repeat(33)] {
        for tool in ["save_game", "load_game", "save_check", "save_damage"] {
            let mut args = json!({"what": "world", "slot": bad});
            if tool == "save_damage" {
                args["how"] = json!("flip");
            }
            let err = fails(&mut app, tool, args);
            assert!(err.contains("is not allowed") && err.contains("slot1"), "{tool} {bad:?}: {err}");
        }
    }
    assert!(!dir.file("world").exists(), "nothing was written");
    assert!(!dir.path().parent().unwrap().join("x.save").exists());
    ok(&mut app, "save_game", json!({"what": "world", "slot": "a-b_9"}));
    ok(&mut app, "save_game", json!({"what": "world", "slot": "x".repeat(32)}));
}

#[test]
fn scope_decides_who_may_save_the_world() {
    let dir = TempDir::new("scope");
    let mut app = game(&dir);
    app.world_mut().resource_mut::<AgentSessions>().configure("tester", Scope::Player, None);
    let as_tester = |app: &mut App, tool: &str, args: serde_json::Value| call_tool(app.world_mut(), "tester", tool, args);

    let denied = as_tester(&mut app, "save_game", json!({"what": "world"}));
    assert!(!denied.ok);
    let text = denied.result.as_str().unwrap();
    assert!(text.contains("saving the world needs dev scope") && text.contains("what=character"), "{text}");
    assert!(!as_tester(&mut app, "save_game", json!({})).ok, "`all` includes the world");
    assert!(!dir.file("world/slot1.save").exists());

    let mine = as_tester(&mut app, "save_game", json!({"what": "character"}));
    assert!(mine.ok, "{}", mine.result);
    assert!(dir.file("character/slot1.save").exists());
    assert!(as_tester(&mut app, "save_list", json!({})).ok && as_tester(&mut app, "save_registry", json!({})).ok);
    // Loading and checking are dev tools, and so is breaking files.
    for tool in ["load_game", "save_check", "save_damage"] {
        let out = as_tester(&mut app, tool, json!({"what": "character", "how": "flip"}));
        assert!(!out.ok && out.result.as_str().unwrap().contains("needs dev scope"), "{tool}: {}", out.result);
    }
}

#[test]
fn a_character_save_is_about_the_body_the_agent_is_bound_to() {
    let dir = TempDir::new("bound");
    let mut app = game(&dir);
    set_hero(&mut app, Hero { name: "Ada".into(), ..Hero::default() });
    let second = app.world_mut().spawn((AiKind::new("Hero"), PlayerId(1), Transform::default(), Hero { name: "Bob".into(), ..Hero::default() })).id();
    let id: AiId = *app.world().get::<AiId>(second).unwrap();
    app.world_mut().resource_mut::<AgentSessions>().configure("bob", Scope::Player, Some(id));

    let saved = call_tool(app.world_mut(), "bob", "save_game", json!({"what": "character", "slot": "bobs"}));
    assert!(saved.ok, "{}", saved.result);
    let file = SaveFile::decode(&fs::read(dir.file("character/bobs.save")).unwrap()).unwrap();
    assert_eq!(file.sections.0["hero"].data["name"], "Bob");
    // The dev agent has no subject of its own: the player's body, player 0.
    ok(&mut app, "save_game", json!({"what": "character", "slot": "adas"}));
    let file = SaveFile::decode(&fs::read(dir.file("character/adas.save")).unwrap()).unwrap();
    assert_eq!(file.sections.0["hero"].data["name"], "Ada");

    // And a load goes into the same body.
    app.world_mut().get_mut::<Hero>(second).unwrap().name = "changed".into();
    app.world_mut().resource_mut::<AgentSessions>().configure("bob", Scope::Dev, Some(id));
    let loaded = call_tool(app.world_mut(), "bob", "load_game", json!({"what": "character", "slot": "bobs"}));
    assert!(loaded.ok, "{}", loaded.result);
    assert_eq!(app.world().get::<Hero>(second).unwrap().name, "Bob");
}

#[test]
fn save_check_hashes_the_state_and_names_what_differs_from_a_file() {
    let dir = TempDir::new("check");
    let mut app = game(&dir);
    some_state(&mut app);
    let first = act(&mut app, "save_check", json!({})).result;
    let (world_hash, character_hash) = (first["world"]["hash"].as_str().unwrap().to_owned(), first["character"]["hash"].as_str().unwrap().to_owned());
    assert_eq!(world_hash.len(), 16);
    assert_eq!(act(&mut app, "save_check", json!({})).result, first, "same state, same hash");

    ok(&mut app, "save_game", json!({"slot": "a"}));
    let same = act(&mut app, "save_check", json!({"slot": "a"})).result;
    assert_eq!(same["world"]["file"]["same"], true, "{same}");
    assert_eq!(same["character"]["file"]["same"], true, "{same}");
    assert!(same["summary"][0].as_str().unwrap().contains("same as world/a (tick 0)"), "{same}");

    // A change: the section that holds it is named, the other kind stays the same.
    let leveled = Hero { level: 8, ..hero(&mut app) };
    set_hero(&mut app, leveled);
    let changed = act(&mut app, "save_check", json!({"slot": "a"})).result;
    assert_eq!(changed["character"]["file"]["differing"], json!(["hero"]), "{changed}");
    assert_eq!(changed["character"]["file"]["same"], false);
    assert_eq!(changed["world"]["file"]["same"], true);
    assert_ne!(changed["character"]["hash"], json!(character_hash));
    assert_eq!(changed["world"]["hash"], json!(world_hash));
    let line = changed["summary"][1].as_str().unwrap();
    assert!(line.contains("differs in hero"), "{line}");

    app.world_mut().resource_mut::<Calendar>().day += 1;
    let both = act(&mut app, "save_check", json!({"what": "world", "slot": "a"})).result;
    assert_eq!(both["world"]["file"]["differing"], json!(["calendar"]));

    // After a load the state is the file's again.
    ok(&mut app, "load_game", json!({"slot": "a"}));
    let after = act(&mut app, "save_check", json!({"slot": "a"})).result;
    assert_eq!((after["world"]["file"]["same"].as_bool(), after["character"]["file"]["same"].as_bool()), (Some(true), Some(true)), "{after}");
    assert_eq!(after["world"]["hash"], json!(world_hash));

    // A slot that is not there.
    let err = fails(&mut app, "save_check", json!({"slot": "nope"}));
    assert!(err.contains("there is no world save in slot `nope`"), "{err}");
}

#[test]
fn save_check_reads_a_damaged_primary_through_its_backup_and_leaves_it_alone() {
    let dir = TempDir::new("checkdamaged");
    let mut app = game(&dir);
    for day in [1, 2] {
        app.world_mut().resource_mut::<Calendar>().day = day;
        ok(&mut app, "save_game", json!({"what": "world"}));
    }
    ok(&mut app, "save_damage", json!({"what": "world", "how": "truncate"}));
    let before = fs::read(dir.file("world/slot1.save")).unwrap();
    let check = act(&mut app, "save_check", json!({"what": "world", "slot": "slot1"})).result;
    assert_eq!(check["world"]["file"]["from_backup"], 1, "{check}");
    assert_eq!(check["world"]["file"]["differing"], json!(["calendar"]), "day 2 now, day 1 in the backup");
    assert!(check["summary"][0].as_str().unwrap().contains("[read from backup 1"), "{check}");
    assert_eq!(fs::read(dir.file("world/slot1.save")).unwrap(), before, "a look repairs nothing");
}

#[test]
fn what_the_tools_say_is_in_their_schema() {
    let dir = TempDir::new("schema");
    let mut app = game(&dir);
    let tools = act(&mut app, "tools", json!({})).result;
    let text = tools.as_str().unwrap();
    for name in ["save_game(", "load_game(", "save_check(", "save_list(", "save_registry(", "save_damage("] {
        assert!(text.contains(name), "{name} in\n{text}");
    }
    let mut specs: Vec<(String, String)> = app
        .world()
        .resource::<shards::ai_core::AiTools>()
        .specs(Scope::Dev)
        .into_iter()
        .filter(|s| s.name.starts_with("save_") || s.name == "load_game")
        .map(|s| (s.name, format!("{:?}/{}", s.kind, s.scope)))
        .collect();
    specs.sort();
    assert_eq!(
        specs.iter().map(|(n, k)| format!("{n}={k}")).collect::<Vec<_>>(),
        ["load_game=Action/dev", "save_check=Query/dev", "save_damage=Control/dev", "save_game=Control/player", "save_list=Query/player", "save_registry=Query/player"]
    );
}

#[test]
fn the_status_names_the_save_folder() {
    let dir = TempDir::new("status");
    let app = game(&dir);
    let field = shards::ai_core::status::status(app.world());
    assert_eq!(field["save"]["dir"], json!(dir.path().display().to_string()), "{field}");
    assert!(field["save"]["sections"].as_u64().unwrap() >= 5);
    assert!(field["engine"]["modules"].as_array().unwrap().iter().any(|m| m["name"] == "save"), "{field}");
}

#[test]
fn a_load_keeps_the_recording_verifiable() {
    let dir = TempDir::new("verify");
    let mut app = game(&dir);
    step(&mut app, 5);
    ok(&mut app, "save_game", json!({"what": "all"}));
    app.world_mut().resource_mut::<Calendar>().day = 77;
    step(&mut app, 5);
    ok(&mut app, "load_game", json!({"what": "all"}));
    step(&mut app, 5);
    let recording = shards::ai_core::replay::save(app.world());
    assert!(recording.start.is_some(), "the recording starts from the loaded state");

    let mut fresh = game(&TempDir::new("verify2"));
    let report = shards::testing::verify(&mut fresh, recording).unwrap();
    assert!(report.identical(), "{report}");
}

/// What `SaveWritten` said, in order.
#[derive(Resource, Default)]
struct Written(Vec<(shards::save::SaveKind, String, Vec<u8>)>);

#[test]
fn every_save_written_is_announced_with_its_bytes_and_a_refused_one_is_not() {
    let dir = TempDir::new("written");
    let mut app = game_in(&dir, |app| {
        app.init_resource::<Written>().add_observer(|event: On<shards::save::SaveWritten>, mut written: ResMut<Written>| {
            let e = event.event();
            written.0.push((e.kind, e.slot.clone(), e.bytes.clone()));
        });
    });
    some_state(&mut app);
    ok(&mut app, "save_game", json!({"what": "all", "slot": "camp"}));
    ok(&mut app, "save_game", json!({"what": "world", "slot": "camp"}));
    let store = app.world().resource::<shards::save::SaveConfig>().store();
    let written = &app.world().resource::<Written>().0;
    let said: Vec<(shards::save::SaveKind, &str)> = written.iter().map(|(kind, slot, _)| (*kind, slot.as_str())).collect();
    use shards::save::SaveKind::{Character, World};
    assert_eq!(said, [(World, "camp"), (Character, "camp"), (World, "camp")]);
    // The bytes are the slot's new primary file, the older one is its backup.
    let primary = fs::read(store.path(World, "camp", shards::save::Which::Save)).unwrap();
    assert_eq!(written[2].2, primary);
    assert_eq!(fs::read(store.path(World, "camp", shards::save::Which::Backup(1))).unwrap(), written[0].2);
    // A save that is refused writes nothing and says nothing.
    fails(&mut app, "save_game", json!({"what": "world", "slot": "Not A Slot"}));
    assert_eq!(app.world().resource::<Written>().0.len(), 3);
}

/// The kind whose [`Brittle`] section refuses to load while the world has this.
#[derive(Resource)]
struct Refuse(SaveKind);

fn refused(world: &World, kind: SaveKind) -> Result<(), String> {
    match world.get_resource::<Refuse>() {
        Some(refuse) if refuse.0 == kind => Err("refused on purpose".to_owned()),
        _ => Ok(()),
    }
}

/// A world section that loads unless the world has `Refuse(World)`.
#[derive(Serialize, Deserialize)]
struct Brittle {
    kept: bool,
}

impl SaveSection for Brittle {
    const NAME: &'static str = "brittle";
    const VERSION: u32 = 1;
    const KIND: SaveKind = SaveKind::World;
    // It refuses before it touches anything.
    const FAILS_CLEANLY: bool = true;

    fn capture(_: &World, _: Option<Entity>) -> Result<Self, String> {
        Ok(Brittle { kept: true })
    }

    fn apply(self, world: &mut World, _: Option<Entity>) -> Result<(), String> {
        refused(world, Self::KIND)
    }
}

/// A character section that loads unless the world has `Refuse(Character)`.
#[derive(Serialize, Deserialize)]
struct BrittleHero {
    kept: bool,
}

impl SaveSection for BrittleHero {
    const NAME: &'static str = "brittle_hero";
    const VERSION: u32 = 1;
    const KIND: SaveKind = SaveKind::Character;
    // It refuses before it touches anything.
    const FAILS_CLEANLY: bool = true;

    fn capture(_: &World, _: Option<Entity>) -> Result<Self, String> {
        Ok(BrittleHero { kept: true })
    }

    fn apply(self, world: &mut World, _: Option<Entity>) -> Result<(), String> {
        refused(world, Self::KIND)
    }
}

/// The game with both brittle sections, and the weather when `later` (a newer game: files
/// of the older one lack it, so its `missing` runs on load).
fn brittle_game(dir: &TempDir, later: Option<SaveKind>) -> App {
    game_in(dir, |app| {
        if let Some(refuse) = later {
            app.insert_resource(Weather("rain".into())).add_save_section::<WeatherSave>().insert_resource(Refuse(refuse));
        }
        app.add_save_section::<Brittle>().add_save_section::<BrittleHero>();
    })
}

#[test]
fn a_load_that_fails_part_way_says_what_it_applied_and_records_the_load() {
    let dir = TempDir::new("partway");
    let mut app = brittle_game(&dir, None);
    some_state(&mut app);
    ok(&mut app, "save_game", json!({"what": "all", "slot": "camp"}));

    // The world's last section refuses: what ran before it stays, the weather's `missing` too.
    let mut world_fails = brittle_game(&dir, Some(SaveKind::World));
    other_state(&mut world_fails);
    let err = fails(&mut world_fails, "load_game", json!({"what": "all", "slot": "camp"}));
    assert_eq!(
        err,
        "loading world/camp failed at section brittle: refused on purpose; applied before it: region, chests, calendar, weather (missing from the file). \
         The world is partly loaded: fix the cause and load again, or go back with snapshot_load"
    );
    assert_eq!(calendar(&world_fails).day, 40);
    assert_eq!(world_fails.world().resource::<Weather>().0, "clear", "its `missing` ran");
    assert_eq!(hero(&mut world_fails).name, "Bob", "the character was not reached");
    assert_eq!(lines(&world_fails, JournalKind::Error, "save: loading world/camp failed: section brittle: refused on purpose").len(), 1);
    assert!(shards::ai_core::replay::save(world_fails.world()).start.is_some(), "the recording restarts from the partly loaded world");

    // The character's last section refuses after the world was loaded.
    let mut hero_fails = brittle_game(&dir, Some(SaveKind::Character));
    other_state(&mut hero_fails);
    let err = fails(&mut hero_fails, "load_game", json!({"what": "all", "slot": "camp"}));
    assert_eq!(
        err,
        "loading character/camp failed at section brittle_hero: refused on purpose; applied before it: position, vitals, hero; already loaded: world/camp. \
         The world is partly loaded: fix the cause and load again, or go back with snapshot_load"
    );
    assert_eq!((hero(&mut hero_fails).name, calendar(&hero_fails).day), ("Ada".to_owned(), 40));
}

/// What a section that fails half way leaves in the world.
#[derive(Resource)]
struct Scribbled;

/// Changes the world, then refuses: what a section that does not check first does.
fn stop_half_way(world: &mut World) -> Result<(), String> {
    world.insert_resource(Scribbled);
    Err("stopped half way".to_owned())
}

/// A world section named `region`: it takes the engine's place in the order, so nothing is
/// applied before it. It changes the world and then refuses, and does not say it fails cleanly.
#[derive(Serialize, Deserialize)]
struct Scribbler {
    kept: bool,
}

impl SaveSection for Scribbler {
    const NAME: &'static str = "region";
    const VERSION: u32 = 1;
    const KIND: SaveKind = SaveKind::World;

    fn capture(_: &World, _: Option<Entity>) -> Result<Self, String> {
        Ok(Scribbler { kept: true })
    }

    fn apply(self, world: &mut World, _: Option<Entity>) -> Result<(), String> {
        stop_half_way(world)
    }
}

/// The same place in the order, and it refuses before it changes anything, and says so.
#[derive(Serialize, Deserialize)]
struct Tidy {
    kept: bool,
}

impl SaveSection for Tidy {
    const NAME: &'static str = "region";
    const VERSION: u32 = 1;
    const KIND: SaveKind = SaveKind::World;
    const FAILS_CLEANLY: bool = true;

    fn capture(_: &World, _: Option<Entity>) -> Result<Self, String> {
        Ok(Tidy { kept: true })
    }

    fn apply(self, _: &mut World, _: Option<Entity>) -> Result<(), String> {
        Err("refused up front".to_owned())
    }
}

/// Like [`Scribbler`], but a section of its own, applied after the others.
#[derive(Serialize, Deserialize)]
struct LateScribbler {
    kept: bool,
}

impl SaveSection for LateScribbler {
    const NAME: &'static str = "scribbled_late";
    const VERSION: u32 = 1;
    const KIND: SaveKind = SaveKind::World;

    fn capture(_: &World, _: Option<Entity>) -> Result<Self, String> {
        Ok(LateScribbler { kept: true })
    }

    fn apply(self, world: &mut World, _: Option<Entity>) -> Result<(), String> {
        stop_half_way(world)
    }
}

/// The reasons the world was said to be built again (`WorldRebuilt`): a co-op host's clients take it whole then.
#[derive(Resource, Default)]
struct Rebuilt(Vec<String>);

fn watch_rebuilds(app: &mut App) {
    app.init_resource::<Rebuilt>().add_observer(|e: On<WorldRebuilt>, mut seen: ResMut<Rebuilt>| seen.0.push(e.reason.clone()));
}

#[test]
fn a_section_that_fails_may_have_changed_the_world_unless_it_says_it_fails_cleanly() {
    // It did change the world, and does not say it fails cleanly: the first to run, and the load
    // does not claim that nothing was applied.
    let dir = TempDir::new("scribbled");
    let mut app = game_in(&dir, |app| {
        app.add_save_section::<Scribbler>();
        watch_rebuilds(app);
    });
    ok(&mut app, "save_game", json!({"what": "world", "slot": "camp"}));
    assert!(shards::ai_core::replay::save(app.world()).start.is_none(), "the recording starts at tick 0");
    let err = fails(&mut app, "load_game", json!({"what": "world", "slot": "camp"}));
    assert_eq!(
        err,
        "loading world/camp failed at section region: stopped half way; no section before it was applied, but the failed section may have changed the world. \
         The world is partly loaded: fix the cause and load again, or go back with snapshot_load"
    );
    assert!(app.world().contains_resource::<Scribbled>(), "the world did change");
    assert!(shards::ai_core::replay::save(app.world()).start.is_some(), "the recording restarts from the world as it is, so a replay saved now verifies");
    assert_eq!(app.world().resource::<Rebuilt>().0, ["save camp loaded"], "whoever mirrors the world takes it whole again");
    assert_eq!(lines(&app, JournalKind::Error, "save: loading world/camp failed: section region: stopped half way").len(), 1);

    // After sections that ran, it says both.
    let dir = TempDir::new("scribbled_late");
    let mut app = game_in(&dir, |app| {
        app.add_save_section::<LateScribbler>();
        watch_rebuilds(app);
    });
    ok(&mut app, "save_game", json!({"what": "world", "slot": "camp"}));
    let err = fails(&mut app, "load_game", json!({"what": "world", "slot": "camp"}));
    assert_eq!(
        err,
        "loading world/camp failed at section scribbled_late: stopped half way; applied before it: region, chests, calendar; \
         the failed section may have changed the world too. The world is partly loaded: fix the cause and load again, or go back with snapshot_load"
    );
    assert!(app.world().contains_resource::<Scribbled>());

    // The same place in the order, a section that checks first and says so: nothing was applied,
    // the call leaves the recording and nobody is told the world was rebuilt.
    let dir = TempDir::new("tidy");
    let mut app = game_in(&dir, |app| {
        app.add_save_section::<Tidy>();
        watch_rebuilds(app);
    });
    ok(&mut app, "save_game", json!({"what": "world", "slot": "camp"}));
    let events = shards::ai_core::replay::save(app.world()).events.len();
    let err = fails(&mut app, "load_game", json!({"what": "world", "slot": "camp"}));
    assert_eq!(err, "loading world/camp failed at section region: refused up front; nothing was applied: fix the cause and load again");
    assert!(!app.world().contains_resource::<Scribbled>());
    let recording = shards::ai_core::replay::save(app.world());
    assert!(recording.start.is_none() && recording.events.len() == events, "the load that applied nothing is not in the recording");
    assert!(app.world().resource::<Rebuilt>().0.is_empty());
}
