//! Saves (E8) on the sandbox: the engine's own sections. Where the player stands and how
//! healthy they are come back from a file, in a fresh game too; the region of the wilds is made
//! again from its seed; a region that no longer comes out the same is refused.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use shards::action::{ActionState, Charges, Dead, Health, Stamina};
use shards::ai_core::{AiIndex, JournalKind};
use shards::prelude::*;
use shards::save::{SaveConfig, SaveFile};
use shards::serde_json::{json, Value};
use shards::world::ActiveRegion;

use crate::common::*;

/// A save folder of its own, removed when the test ends.
struct Saves(PathBuf);

impl Saves {
    fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("sandbox_saves_{tag}_{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&dir);
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// A sandbox saving here.
    fn game(&self) -> App {
        let mut app = sandbox::test_app(42);
        app.world_mut().resource_mut::<SaveConfig>().dir = self.0.clone();
        app
    }
}

impl Drop for Saves {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn player_entity(app: &App) -> Entity {
    app.world().resource::<AiIndex>().get(AiId::parse(PLAYER).unwrap()).unwrap()
}

fn feet(app: &App) -> Vec3 {
    app.world().get::<Transform>(player_entity(app)).unwrap().translation
}

fn region(app: &App) -> Option<(String, u64, u64)> {
    app.world().get_resource::<ActiveRegion>().map(|r| (r.manifest.region.clone(), r.manifest.seed, r.hash))
}

fn warns(app: &App, needle: &str) -> Vec<String> {
    app.world().resource::<shards::ai_core::AiJournal>().after(0).0.filter(|e| e.kind == JournalKind::Warn && e.text.contains(needle)).map(|e| e.text.clone()).collect()
}

#[test]
fn the_engine_registers_what_it_saves() {
    let saves = Saves::new("registry");
    let mut app = saves.game();
    let out = act(&mut app, "save_registry", json!({})).result;
    let sections: Vec<(&str, &str, u64, &str)> = out["sections"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["name"].as_str().unwrap(), s["kind"].as_str().unwrap(), s["version"].as_u64().unwrap(), s["source"].as_str().unwrap()))
        .collect();
    assert_eq!(sections, [("region", "world", 1, "shards"), ("position", "character", 1, "shards"), ("vitals", "character", 1, "shards")]);
    let status = shards::ai_core::status::status(app.world());
    assert!(status["engine"]["modules"].as_array().unwrap().iter().any(|m| m["name"] == "save"));
}

#[test]
fn the_players_position_and_vitals_come_back_in_this_game_and_in_a_fresh_one() {
    let saves = Saves::new("player");
    let mut app = saves.game();
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 3.5, "z": -2.25}));
    let player = player_entity(&app);
    {
        let world = app.world_mut();
        let mut tf = world.get_mut::<Transform>(player).unwrap();
        tf.rotation = Quat::from_rotation_y(1.25);
        world.get_mut::<Health>(player).unwrap().current = 61.5;
        world.get_mut::<Stamina>(player).unwrap().current = 33.25;
        world.get_mut::<Charges>(player).unwrap().get_mut("flask").unwrap().current = 1;
    }
    let (at, turned) = {
        let tf = app.world().get::<Transform>(player).unwrap();
        (tf.translation, tf.rotation)
    };
    let out = text(&ok(&mut app, "save_game", json!({"what": "character", "slot": "ford", "label": "at the ford"}))).to_owned();
    assert!(out.starts_with("ok: character/ford saved (save_game, "), "{out}");

    // This game, somewhere else and in better health.
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": -9.0, "z": 7.0}));
    {
        let world = app.world_mut();
        world.get_mut::<Transform>(player).unwrap().rotation = Quat::IDENTITY;
        world.get_mut::<Health>(player).unwrap().current = 100.0;
        world.get_mut::<Stamina>(player).unwrap().current = 100.0;
        world.get_mut::<Charges>(player).unwrap().get_mut("flask").unwrap().current = 3;
    }
    let differs = act(&mut app, "save_check", json!({"what": "character", "slot": "ford"})).result;
    assert_eq!(differs["character"]["file"]["same"], false, "{differs}");
    assert_eq!(differs["character"]["file"]["differing"], json!(["position", "vitals"]));
    ok(&mut app, "load_game", json!({"what": "character", "slot": "ford"}));
    let check = |app: &App| {
        let tf = app.world().get::<Transform>(player_entity(app)).unwrap();
        assert_eq!((tf.translation, tf.rotation), (at, turned), "position and rotation, bit for bit");
        assert_eq!(app.world().get::<Health>(player_entity(app)).unwrap().current, 61.5);
        assert_eq!(app.world().get::<Stamina>(player_entity(app)).unwrap().current, 33.25);
        assert_eq!(app.world().get::<Charges>(player_entity(app)).unwrap().get("flask").unwrap().current, 1);
    };
    check(&app);
    let same = act(&mut app, "save_check", json!({"what": "character", "slot": "ford"})).result;
    assert_eq!(same["character"]["file"]["same"], true, "{same}");

    // A fresh game with nothing but the file.
    let mut fresh = saves.game();
    assert_ne!(feet(&fresh), at);
    ok(&mut fresh, "load_game", json!({"what": "character", "slot": "ford"}));
    check(&fresh);
    // The same ticks afterwards: a loaded player stands as a played one did.
    step(&mut fresh, 10);
    assert!((feet(&fresh) - at).length() < 1.0, "{:?}", feet(&fresh));
    assert!(warns(&fresh, "damaged").is_empty());
}

/// The wilds of seed 42, the player standing in tile (1, 3).
fn in_the_wilds(app: &mut App, seed: u64) {
    ok(app, "regen", json!({"region": "wilds", "seed": seed}));
    ok(app, "goto_tile", json!({"x": 1, "y": 3}));
}

#[test]
fn the_region_of_the_wilds_is_made_again_from_its_seed() {
    let saves = Saves::new("wilds");
    let mut app = saves.game();
    in_the_wilds(&mut app, 42);
    let (wilds, at) = (region(&app).unwrap(), feet(&app));
    assert_eq!((wilds.0.as_str(), wilds.1), ("wilds", 42));
    ok(&mut app, "save_game", json!({"what": "all", "slot": "wild"}));
    let file = SaveFile::decode(&fs::read(saves.path().join("world/wild.save")).unwrap()).unwrap();
    assert_eq!(file.sections.0["region"].data["active"]["id"], "wilds");
    assert_eq!(file.sections.0["region"].data["active"]["manifest_hash"], json!(format!("{:016x}", wilds.2)));

    // A fresh game has no region at all: the load builds it, and the player stands where they stood.
    let mut fresh = saves.game();
    assert_eq!(region(&fresh), None);
    let out = text(&ok(&mut fresh, "load_game", json!({"what": "all", "slot": "wild"}))).to_owned();
    assert!(out.starts_with("ok: world/wild loaded") && out.contains("character/wild loaded"), "{out}");
    assert_eq!(region(&fresh), Some(wilds.clone()), "the same manifest, byte for byte");
    assert_eq!(feet(&fresh), at, "the position is applied after the region is built");
    assert!(!lines(&fresh, "region loaded: wilds seed 42 v1").is_empty(), "{:?}", story(&fresh));
    let check = act(&mut fresh, "save_check", json!({"slot": "wild"})).result;
    assert_eq!((check["world"]["file"]["same"].as_bool(), check["character"]["file"]["same"].as_bool()), (Some(true), Some(true)), "{check}");

    // A game in another region of the same kind: the saved one replaces it.
    let mut other = saves.game();
    in_the_wilds(&mut other, 43);
    assert_ne!(region(&other), Some(wilds.clone()));
    ok(&mut other, "load_game", json!({"what": "world", "slot": "wild"}));
    assert_eq!(region(&other), Some(wilds.clone()));
    assert!(!lines(&other, "region unloaded: wilds seed 43").is_empty());

    // The same file already loaded does not rebuild anything.
    let ids_before = other.world().resource::<AiIndex>().next_id();
    ok(&mut other, "load_game", json!({"what": "world", "slot": "wild"}));
    assert_eq!(other.world().resource::<AiIndex>().next_id(), ids_before);
}

#[test]
fn a_position_saved_in_another_region_does_not_move_the_body() {
    let saves = Saves::new("elsewhere");
    let mut app = saves.game();
    in_the_wilds(&mut app, 42);
    ok(&mut app, "save_game", json!({"what": "character", "slot": "wild"}));

    let mut other = saves.game();
    in_the_wilds(&mut other, 43);
    let stays = feet(&other);
    ok(&mut other, "load_game", json!({"what": "character", "slot": "wild"}));
    assert_eq!(feet(&other), stays);
    let said = warns(&other, "the position was saved in region wilds seed 42 but the world has wilds seed 43");
    assert_eq!(said.len(), 1, "{:?}", story(&other));

    // And one saved outside every region is applied wherever the world is.
    let mut arena = saves.game();
    ok(&mut arena, "teleport", json!({"id": PLAYER, "x": 2.0, "z": 2.0}));
    ok(&mut arena, "save_game", json!({"what": "character", "slot": "arena"}));
    ok(&mut other, "load_game", json!({"what": "character", "slot": "arena"}));
    assert_eq!((feet(&other).x, feet(&other).z), (2.0, 2.0));
}

#[test]
fn a_region_that_no_longer_comes_out_the_same_is_refused_and_nothing_is_built() {
    let saves = Saves::new("changed");
    let mut app = saves.game();
    in_the_wilds(&mut app, 42);
    ok(&mut app, "save_game", json!({"what": "world", "slot": "wild"}));
    // The save of a generator that made another manifest: same region and seed, another hash.
    let path = saves.path().join("world/wild.save");
    let mut file = SaveFile::decode(&fs::read(&path).unwrap()).unwrap();
    file.sections.0.get_mut("region").unwrap().data["active"]["manifest_hash"] = Value::String("0000000000000001".into());
    fs::write(&path, file.encode()).unwrap();

    let mut fresh = saves.game();
    let err = fails(&mut fresh, "load_game", json!({"what": "world", "slot": "wild"}));
    assert!(err.contains("section region") && err.contains("wilds seed 42 v1 comes out differently now") && err.contains("0000000000000001"), "{err}");
    assert_eq!(region(&fresh), None, "nothing was built");
    // An apply failure is not a damaged file: it does not look for a backup, and does not touch the file.
    assert_eq!(fs::read(&path).unwrap(), file.encode());
    assert!(warns(&fresh, "restored from backup").is_empty());
}

fn dead(app: &App) -> bool {
    app.world().get::<Dead>(player_entity(app)).is_some()
}

fn health(app: &App) -> f32 {
    app.world().get::<Health>(player_entity(app)).unwrap().current
}

/// The player's health runs out; the next tick's `check_deaths` kills.
fn die(app: &mut App) {
    ok(app, "set", json!({"id": PLAYER, "component": "Health", "path": "current", "value": 0}));
    step(app, 2);
    assert!(dead(app));
}

#[test]
fn a_body_that_died_since_the_save_lives_again_when_it_is_loaded() {
    let saves = Saves::new("revive");
    let mut app = saves.game();
    step(&mut app, 5);
    ok(&mut app, "save_game", json!({"what": "character", "slot": "alive"}));
    let saved = health(&app);
    assert!(saved > 0.0);

    die(&mut app);
    assert_eq!(health(&app), 0.0);
    let refused = fails(&mut app, "light_attack", json!({}));
    assert!(refused.contains("dead"), "a dead body cannot attack: {refused}");

    let out = text(&ok(&mut app, "load_game", json!({"what": "character", "slot": "alive"}))).to_owned();
    assert!(out.starts_with("ok: character/alive loaded"), "{out}");
    assert!(!dead(&app), "the saved health is above zero: the body is revived, as the dev `heal` does");
    assert_eq!(health(&app), saved);
    assert!(app.world().get::<ActionState>(player_entity(&app)).unwrap().current.is_none(), "the death action is cut");
    assert!(!lines(&app, &format!("{PLAYER} healed +")).is_empty(), "{:?}", events(&app));

    // It fights again.
    let attack = text(&ok(&mut app, "light_attack", json!({}))).to_owned();
    assert!(attack.starts_with("ok"), "{attack}");
    step(&mut app, 60);
    assert!(!dead(&app));

    // The recording that follows verifies: it starts from the state right after the load.
    let recording = shards::ai_core::replay::save(app.world());
    let start = recording.start.as_ref().expect("a load restarts the recording").tick;
    assert!(recording.events.iter().all(|e| e.at() >= start));
    let mut fresh = sandbox::test_app(42);
    let report = shards::testing::verify(&mut fresh, recording).unwrap();
    assert!(report.identical(), "{report}");
}

#[test]
fn a_save_made_at_zero_health_leaves_the_body_dead() {
    let saves = Saves::new("stays_dead");
    let mut app = saves.game();
    die(&mut app);
    ok(&mut app, "save_game", json!({"what": "character", "slot": "grave"}));

    // A living body loading it has no health left and dies on the next tick, as any body does.
    ok(&mut app, "heal", json!({"id": PLAYER}));
    assert!(!dead(&app));
    ok(&mut app, "load_game", json!({"what": "character", "slot": "grave"}));
    assert!(!dead(&app), "nothing kills during the load itself");
    step(&mut app, 1);
    assert!(dead(&app));
    assert!(!lines(&app, &format!("{PLAYER} died")).is_empty());

    // A dead body loading it stays dead.
    ok(&mut app, "load_game", json!({"what": "character", "slot": "grave"}));
    assert!(dead(&app));
    assert_eq!(health(&app), 0.0);
}
