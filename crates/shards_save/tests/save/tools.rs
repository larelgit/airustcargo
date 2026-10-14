//! The tools an agent (or a menu) saves and loads with.

use std::fs;

use serde_json::json;
use shards::ai_core::{call_tool, AgentSessions, AiId, Scope};
use shards::save::SaveFile;

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
#[ignore = "SEAM(C1): needs ai_core::replay::restart_from_current, which C1 adds"]
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
