//! The replay check `content` covers the definitions the session's simulation read, not every
//! one: a definition the session never read can be added or changed, and its replays still
//! verify; a definition it read diverges, and the report names it (`docs/manual.md`, "What the
//! content check covers").

use std::collections::BTreeMap;

use shards::ai_core::replay::{self, parts_at, Replay, ReplayHashing};
use shards::ai_core::snapshot;
use shards::ai_core::AiReplay;
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::*;
use crate::content::{app_on, assets_copy};

/// A short fight: the player swings at the boss for a few seconds, everyone else calm.
fn fight(app: &mut App) {
    calm_skeletons(app);
    calm_warden(app);
    ok(app, "teleport", json!({"id": PLAYER, "x": 0, "z": -3}));
    ok(app, "attack", json!({"target": BOSS, "repeat": true}));
    step(app, 200);
}

/// What the recording says its session had read by its end, part by part.
fn read_at_end(recording: &Replay) -> BTreeMap<String, u64> {
    parts_at(&recording.check_parts["content"], recording.end.tick)
}

/// `recording` played on `app` to its end.
fn played_on(app: &mut App, recording: &Replay) -> replay::ReplayReport {
    replay::play(app.world_mut(), recording.clone()).unwrap();
    step(app, recording.end.tick - recording.start.as_ref().map_or(0, |s| s.tick));
    app.update();
    app.world().resource::<AiReplay>().report().cloned().expect("the replay finished")
}

fn edit(dir: &std::path::Path, file: &str, from: &str, to: &str) {
    let path = dir.join("content").join(file);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains(from), "the test edits `{from}` in {file}");
    std::fs::write(&path, text.replacen(from, to, 1)).unwrap();
}

fn whole_hash(app: &App) -> u64 {
    app.world().resource::<ContentDb>().hash()
}

#[test]
fn a_definition_the_session_never_read_leaves_its_replay_verifying() {
    let mut app = sandbox::test_app(42);
    fight(&mut app);
    let recording = replay::save(app.world());
    let read = read_at_end(&recording);
    assert!(read.contains_key("action/light_1") && read.contains_key("prefab/frost_warden"), "{read:?}");
    for unread in ["prefab/skeleton_captain", "action/heavy"] {
        assert!(!read.contains_key(unread), "{unread} is never read in this session: {read:?}");
    }
    assert!(!read.keys().any(|k| k.starts_with("look/") || k.starts_with("sound/")), "a picture's kinds are never parts: {read:?}");

    // A definition added, one never read changed, and a look changed: the content differs, the
    // session's does not.
    let dir = assets_copy("unread");
    std::fs::write(dir.join("content/characters/extra.prefab.ron"), "{ \"crate\": (components: { \"Name\": \"Crate\" }) }").unwrap();
    edit(&dir, "characters/characters.prefab.ron", "\"Health\": (current: 90.0, max: 90.0)", "\"Health\": (current: 95.0, max: 95.0)");
    edit(&dir, "combat/player.action.ron", "damage: 55, poise: 45", "damage: 56, poise: 45");
    edit(&dir, "look/sandbox.look.ron", "\"#408cff\"", "\"#ff8c40\"");
    let mut again = app_on(&dir);
    assert_ne!(whole_hash(&again), whole_hash(&app), "the content is not the same");
    let report = played_on(&mut again, &recording);
    assert!(report.identical(), "{report}");
}

#[test]
fn a_changed_definition_the_session_read_diverges_and_the_report_names_it() {
    let mut app = sandbox::test_app(42);
    fight(&mut app);
    let recording = replay::save(app.world());

    let dir = assets_copy("read");
    edit(&dir, "combat/player.action.ron", "damage: 20, poise: 12", "damage: 21, poise: 12");
    let mut again = app_on(&dir);
    let report = played_on(&mut again, &recording);
    assert_eq!(report.diverged_at, Some(60), "{report}");
    let summary = report.divergence.as_ref().unwrap().summary();
    assert!(summary.ends_with("replay check content differs: action/light_1 differs"), "{summary}");
}

#[test]
fn a_kind_the_simulation_reads_whole_counts_a_definition_added_to_it() {
    let mut app = sandbox::test_app(42);
    step(&mut app, 60);
    let recording = replay::save(app.world());
    // The sandbox builds every layout there is (`spawn_world` iterates them).
    let read = read_at_end(&recording);
    assert!(read.contains_key("layout (all)") && read.contains_key("layout/watch"), "{read:?}");

    let dir = assets_copy("whole");
    std::fs::write(dir.join("content/world/empty.layout.ron"), "{ \"empty\": () }").unwrap();
    let mut again = app_on(&dir);
    let report = played_on(&mut again, &recording);
    let summary = report.divergence.as_ref().map(|d| d.summary()).unwrap_or_default();
    assert!(summary.ends_with("replay check content differs: layout/empty extra"), "{report}");
}

#[test]
fn content_applied_in_a_session_is_hashed_from_its_tick_and_replays_the_same() {
    let mut app = sandbox::test_app(42);
    calm_skeletons(&mut app);
    calm_warden(&mut app);
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 0, "z": -3}));
    ok(&mut app, "attack", json!({"target": BOSS, "repeat": true}));
    step(&mut app, 100);
    let before = ok(&mut app, "content_get", json!({"kind": "action", "id": "light_1"})).result["def"].clone();
    let mut harder = before.clone();
    harder["hits"][0]["damage"] = json!(40.0);
    let captain = ok(&mut app, "content_get", json!({"kind": "prefab", "id": "skeleton_captain"})).result["def"].clone();
    let mut tougher = captain.clone();
    tougher["components"]["Health"] = json!("(current: 99.0, max: 99.0)");
    let changes = json!([
        {"kind": "action", "id": "light_1", "def": harder, "before": before},
        {"kind": "prefab", "id": "skeleton_captain", "def": tougher, "before": captain},
    ]);
    ok(&mut app, "content_apply", json!({"changes": changes}));
    step(&mut app, 150);
    let recording = replay::save(app.world());

    // light_1 has one hash up to the change and another from the first check after it; the
    // captain, never read, is no part at all.
    let log = &recording.check_parts["content"];
    let light = |tick: u64| parts_at(log, tick).get("action/light_1").copied();
    assert!(light(60).is_some() && light(120).is_some() && light(60) != light(120), "{log:?}");
    assert!(!read_at_end(&recording).contains_key("prefab/skeleton_captain"));

    // A fresh game starts from the recorded `before`s and applies the change at its tick.
    let mut again = sandbox::test_app(42);
    let report = played_on(&mut again, &recording);
    assert!(report.identical(), "{report}");
}

#[test]
fn a_snapshot_carries_what_its_session_read_and_a_load_compares_only_that() {
    let mut app = sandbox::test_app(42);
    fight(&mut app);
    let taken = snapshot::capture(app.world()).unwrap();
    let read: Vec<&str> = taken.check_parts["content"].keys().map(String::as_str).collect();
    assert!(read.contains(&"action/light_1"), "{read:?}");
    let names = |app: &App| -> Vec<String> { app.world().resource::<ContentDb>().used_parts().into_iter().map(|(n, _)| n).collect() };

    // A fresh game has read only what built its world; loaded, it has read what the session had,
    // and it goes on as that session does.
    let mut fresh = sandbox::test_app(42);
    assert!(!names(&fresh).contains(&"action/light_1".to_owned()));
    snapshot::restore_exact(fresh.world_mut(), &taken, false).unwrap();
    assert_eq!(names(&fresh), names(&app));
    step(&mut app, 120);
    step(&mut fresh, 120);
    assert_eq!(names(&fresh), names(&app));
    assert_eq!(replay::state_hash(fresh.world()), replay::state_hash(app.world()));

    // Content the session never read can differ; content it read cannot, and the refusal says which.
    let dir = assets_copy("snapshot_unread");
    edit(&dir, "characters/characters.prefab.ron", "\"Health\": (current: 90.0, max: 90.0)", "\"Health\": (current: 95.0, max: 95.0)");
    let mut other = app_on(&dir);
    snapshot::restore_exact(other.world_mut(), &taken, false).unwrap();
    let dir = assets_copy("snapshot_read");
    edit(&dir, "combat/player.action.ron", "damage: 20, poise: 12", "damage: 21, poise: 12");
    let mut changed = app_on(&dir);
    let refused = snapshot::restore_exact(changed.world_mut(), &taken, false).unwrap_err();
    assert!(refused.contains("content differs (action/light_1 differs)"), "{refused}");
}

#[test]
fn a_definition_added_for_an_id_the_session_looked_for_and_did_not_find_is_extra() {
    // The engine looks for the NPC tuning `default` every tick; the sandbox defines none, so the
    // session read nothing of it, and no part says it was looked for.
    let mut app = sandbox::test_app(42);
    fight(&mut app);
    let recording = replay::save(app.world());
    assert!(!read_at_end(&recording).keys().any(|k| k.starts_with("npc_tuning")), "{:?}", read_at_end(&recording));

    // Defined (with the numbers the engine used without it), the playback reads it: the same
    // world, and one part more than the recording has.
    let dir = assets_copy("probed");
    std::fs::write(dir.join("content/npc/probed.npc_tuning.ron"), "{ \"default\": () }").unwrap();
    let mut again = app_on(&dir);
    let report = played_on(&mut again, &recording);
    let summary = report.divergence.as_ref().map(|d| d.summary()).unwrap_or_default();
    assert!(summary.ends_with("replay check content differs: npc_tuning/default extra"), "{report}");
    assert!(report.divergence.as_ref().unwrap().entities.is_empty(), "{report}");
}

#[test]
fn a_press_counts_its_screen_and_showing_one_does_not() {
    let mut app = sandbox::test_app(42);
    let screens = |app: &App| -> Vec<String> {
        let read = app.world().resource::<ContentDb>().used_parts();
        read.into_iter().map(|(name, _)| name).filter(|name| name.starts_with("screen")).collect()
    };
    ok(&mut app, "ui_open", json!({"screen": "pause"}));
    step(&mut app, 2);
    assert!(screens(&app).is_empty(), "shown, not pressed: {:?}", screens(&app));
    // The press finds its screen from the widget: `pause/settings` opens `settings` (shown, not
    // pressed), and a value set there (its tabs turned) presses `settings`.
    ok(&mut app, "ui_click", json!({"id": "pause/settings"}));
    assert_eq!(screens(&app), ["screen/pause"]);
    ok(&mut app, "ui_set", json!({"id": "settings/tabs", "value": "audio"}));
    assert_eq!(screens(&app), ["screen/pause", "screen/settings"]);
}

#[test]
fn autosave_numbers_never_change_a_playback() {
    // A playback writes no saves, and asks the autosave definitions nothing (`save` is a kind the
    // simulation never reads): other numbers there leave the reference replay of saves verifying.
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/saves.json");
    let recording = replay::load(&file).unwrap();
    let dir = assets_copy("autosave");
    let text = "{ \"world\": (debounce_secs: 0.0), \"character\": (debounce_secs: 600.0) }";
    std::fs::write(dir.join("content/autosave.save.ron"), text).unwrap();
    let mut again = app_on(&dir);
    assert!(again.world().resource::<ContentDb>().errors().next().is_none());
    let report = played_on(&mut again, &recording);
    assert!(report.identical(), "{report}");
}

#[test]
fn a_refused_load_in_a_session_that_hashes_every_definition_leaves_it_so() {
    let mut app = sandbox::test_app(42);
    fight(&mut app);
    let taken = snapshot::capture(app.world()).unwrap();

    // A game on other numbers for `light_1`, which loads a snapshot taken whole (a file from
    // before the parts): the session hashes every definition from then on, and keeps no reads.
    let dir = assets_copy("whole_refused");
    edit(&dir, "combat/player.action.ron", "damage: 20, poise: 12", "damage: 21, poise: 12");
    let mut game = app_on(&dir);
    let mut legacy = snapshot::capture(game.world()).unwrap();
    legacy.format = "shards-snapshot/1".to_owned();
    legacy.check_parts.clear();
    legacy.checks.insert("content".into(), format!("{:016x}", whole_hash(&game)));
    // Its state hash was taken by parts, so this load reports a difference; from it on, the
    // session hashes every definition, and a snapshot it takes is one taken whole, which loads
    // exactly.
    let report = snapshot::restore(game.world_mut(), &legacy, false).unwrap();
    assert!(!report.exact());
    let whole = |app: &App| app.world().resource::<ReplayHashing>().hashes_whole("content");
    assert!(whole(&game));
    let mut legacy = snapshot::capture(game.world()).unwrap();
    assert!(legacy.check_parts.is_empty());
    legacy.format = "shards-snapshot/1".to_owned();
    snapshot::restore_exact(game.world_mut(), &legacy, false).unwrap();
    assert!(whole(&game));
    assert!(game.world().resource::<ContentDb>().used_parts().is_empty());
    step(&mut game, 30);
    let hash = replay::state_hash(game.world());

    // The session's own snapshot is refused (it read `light_1`, which differs here): the game
    // is as it was, still hashing every definition, its reads none.
    let refused = snapshot::restore_exact(game.world_mut(), &taken, false).unwrap_err();
    assert!(refused.contains("content differs (action/light_1 differs)"), "{refused}");
    assert!(whole(&game));
    assert!(game.world().resource::<ContentDb>().used_parts().is_empty());
    assert_eq!(replay::state_hash(game.world()), hash);
}
