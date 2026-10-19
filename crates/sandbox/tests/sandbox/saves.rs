//! Saves (E8) on the sandbox: the engine's own sections. Where the player stands and how
//! healthy they are come back from a file, in a fresh game too; the region of the wilds comes
//! back from the manifest the save keeps, as it was, after an update that changes the generator
//! too; content the manifest names and the game lost is refused, a manifest that is not the one
//! its hash names is damage; a save of the section's version 1 (no manifest) is made again from
//! its seed and refused when it no longer comes out the same.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use shards::action::{ActionState, Charges, Dead, Health, Stamina};
use shards::ai_core::{AiIndex, JournalKind};
use shards::prelude::*;
use shards::save::{SaveConfig, SaveFile, SectionData};
use shards::serde_json::{json, Value};
use shards::world::{ActiveRegion, RegionManifest, RegionPart};

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

/// The sandbox after an update that changes the wilds' generator: its definition at version 2,
/// grass half as likely. A copy of the assets, removed when the test ends.
pub(crate) struct Updated(PathBuf);

impl Updated {
    pub(crate) fn new(tag: &str) -> Self {
        let dir = crate::content::assets_copy(&format!("update_{tag}"));
        let edit = |file: &str, from: &str, to: &str| {
            let path = dir.join("content/region").join(file);
            let text = fs::read_to_string(&path).unwrap();
            assert_eq!(text.matches(from).count(), 1, "{file}: the update edits `{from}`");
            fs::write(&path, text.replacen(from, to, 1)).unwrap();
        };
        edit("wilds.region.ron", "version: 1,", "version: 2,");
        edit("wilds.tile.ron", "weight: 6.0,", "weight: 3.0,");
        Self(dir)
    }

    /// The updated sandbox, saving in `dir`.
    pub(crate) fn game_in(&self, dir: &Path) -> App {
        let mut app = crate::content::app_on(&self.0);
        app.world_mut().resource_mut::<SaveConfig>().dir = dir.to_owned();
        app
    }
}

impl Drop for Updated {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn read_save(path: &Path) -> SaveFile {
    SaveFile::decode(&fs::read(path).unwrap()).unwrap()
}

/// Rewrites the file's region section as the section's version 1 wrote it: the region's names
/// and hash, no manifest.
fn as_version_1(path: &Path) {
    let mut file = read_save(path);
    let mut active = file.sections.0["region"].data["active"].clone();
    active.as_object_mut().unwrap().remove("manifest");
    file.sections.0.insert("region".into(), SectionData { version: 1, data: json!({ "active": active }) });
    fs::write(path, file.encode()).unwrap();
}

fn parts(app: &mut App) -> usize {
    app.world_mut().query_filtered::<(), With<RegionPart>>().iter(app.world()).count()
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
    assert_eq!(sections, [("region", "world", 2, "shards"), ("encounters", "world", 1, "shards"), ("position", "character", 1, "shards"), ("vitals", "character", 1, "shards")]);
    assert_eq!(out["sections"][0]["migrations_from"], json!([1]), "a region of version 1 (no manifest) still reads");
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
fn the_region_of_the_wilds_comes_back_from_the_manifest_the_save_keeps() {
    let saves = Saves::new("wilds");
    let mut app = saves.game();
    in_the_wilds(&mut app, 42);
    let (wilds, at) = (region(&app).unwrap(), feet(&app));
    assert_eq!((wilds.0.as_str(), wilds.1), ("wilds", 42));
    ok(&mut app, "save_game", json!({"what": "all", "slot": "wild"}));
    let path = saves.path().join("world/wild.save");
    let file = read_save(&path);
    let section = &file.sections.0["region"];
    assert_eq!(section.version, 2);
    assert_eq!(section.data["active"]["id"], "wilds");
    assert_eq!(section.data["active"]["manifest_hash"], json!(format!("{:016x}", wilds.2)));
    let manifest: RegionManifest = shards::serde_json::from_value(section.data["active"]["manifest"].clone()).unwrap();
    assert_eq!(&manifest, &app.world().resource::<ActiveRegion>().manifest, "the manifest itself, as it was built");
    // What it weighs: the section in the file's canonical JSON, and the whole world file.
    let bytes = shards::save::canon::to_string(&section.data).len();
    let file_bytes = fs::metadata(&path).unwrap().len();
    // 3151 B in a file of 3392 B when this was written: the manifest is most of a world file.
    assert!(bytes < 8 * 1024 && file_bytes < 8 * 1024, "an 8×8 region: {bytes} B in a file of {file_bytes} B");

    // A fresh game has no region at all: the load builds it, and the player stands where they stood.
    let mut fresh = saves.game();
    assert_eq!(region(&fresh), None);
    let out = text(&ok(&mut fresh, "load_game", json!({"what": "all", "slot": "wild"}))).to_owned();
    assert!(out.starts_with("ok: world/wild loaded") && out.contains("character/wild loaded"), "{out}");
    assert_eq!(region(&fresh), Some(wilds.clone()), "the same manifest, byte for byte");
    assert_eq!(feet(&fresh), at, "the position is applied after the region is built");
    assert!(!lines(&fresh, "region loaded: wilds seed 42 v1").is_empty(), "{:?}", story(&fresh));
    assert_eq!(lines(&fresh, "region wilds seed 42 v1: built from the save's manifest").len(), 1, "{:?}", story(&fresh));
    assert!(lines(&fresh, "region generated").is_empty(), "nothing is generated: {:?}", story(&fresh));
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
fn a_saved_world_stays_as_it_was_after_an_update_changes_the_generator() {
    let saves = Saves::new("update");
    let mut app = saves.game();
    in_the_wilds(&mut app, 42);
    let (wilds, at) = (region(&app).unwrap(), feet(&app));
    let tiles = app.world().resource::<ActiveRegion>().manifest.tiles.clone();
    ok(&mut app, "save_game", json!({"what": "all", "slot": "wild"}));

    // The update: the same seed makes another world now.
    let updated = Updated::new("update");
    let mut later = updated.game_in(saves.path());
    ok(&mut later, "regen", json!({"region": "wilds", "seed": 42}));
    assert!(!lines(&later, "region generated: wilds seed 42 v2").is_empty(), "{:?}", story(&later));
    assert_ne!(region(&later).unwrap().2, wilds.2, "the update changes what seed 42 makes");
    assert_ne!(later.world().resource::<ActiveRegion>().manifest.tiles, tiles);

    // The save brings back its own world, and the player where they stood in it.
    let out = text(&ok(&mut later, "load_game", json!({"what": "all", "slot": "wild"}))).to_owned();
    assert!(out.starts_with("ok: world/wild loaded"), "{out}");
    assert_eq!(region(&later), Some(wilds.clone()), "the same manifest hash");
    assert_eq!(later.world().resource::<ActiveRegion>().manifest.tiles, tiles, "the same tiles, turned the same way");
    assert_eq!(feet(&later), at);
    let said = lines(&later, "region wilds seed 42 v1: built from the save's manifest (the generator is v2 now)");
    assert_eq!(said.len(), 1, "{:?}", story(&later));
    assert!(lines(&later, "region generated: wilds seed 42 v1").is_empty(), "the generator did not run for it");
    let check = act(&mut later, "save_check", json!({"slot": "wild"})).result;
    assert_eq!((check["world"]["file"]["same"].as_bool(), check["character"]["file"]["same"].as_bool()), (Some(true), Some(true)), "{check}");

    // Saved again, it is still the world of version 1, kept for good.
    ok(&mut later, "save_game", json!({"what": "world", "slot": "again"}));
    let again = read_save(&saves.path().join("world/again.save"));
    assert_eq!(again.sections.0["region"].data["active"]["version"], 1);
    assert_eq!(again.sections.0["region"].data["active"]["manifest_hash"], json!(format!("{:016x}", wilds.2)));
}

#[test]
fn a_save_of_version_1_is_made_again_from_its_seed_and_saved_with_its_manifest_next_time() {
    let saves = Saves::new("v1");
    let mut app = saves.game();
    in_the_wilds(&mut app, 42);
    let wilds = region(&app).unwrap();
    ok(&mut app, "save_game", json!({"what": "world", "slot": "old"}));
    let path = saves.path().join("world/old.save");
    as_version_1(&path);

    // The content it was made with: the generator makes the same world again.
    let mut fresh = saves.game();
    ok(&mut fresh, "load_game", json!({"what": "world", "slot": "old"}));
    assert_eq!(region(&fresh), Some(wilds.clone()));
    assert!(!lines(&fresh, "region generated: wilds seed 42 v1").is_empty(), "{:?}", story(&fresh));
    assert!(lines(&fresh, "built from the save's manifest").is_empty());
    let check = act(&mut fresh, "save_check", json!({"what": "world", "slot": "old"})).result;
    assert_eq!(check["world"]["file"]["differing"], json!(["region"]), "the file has no manifest yet: {check}");
    ok(&mut fresh, "save_game", json!({"what": "world", "slot": "old"}));
    let file = read_save(&path);
    assert_eq!(file.sections.0["region"].version, 2, "the next save is a version 2 one");
    assert_eq!(file.sections.0["region"].data["active"]["manifest"]["region"], "wilds");

    // After an update, the old save describes a world the generator no longer makes: refused, as before.
    as_version_1(&path);
    let updated = Updated::new("v1");
    let mut later = updated.game_in(saves.path());
    let err = fails(&mut later, "load_game", json!({"what": "world", "slot": "old"}));
    assert!(err.contains("section region: wilds seed 42 v1 comes out differently now") && err.contains(&format!("{:016x} when saved", wilds.2)), "{err}");
    assert!(err.contains("generator v2") && err.contains("write a migration for `region` or keep the old content"), "{err}");
    assert_eq!(region(&later), None, "nothing was built");
}

#[test]
fn a_region_that_no_longer_comes_out_the_same_is_refused_and_nothing_is_built() {
    let saves = Saves::new("changed");
    let mut app = saves.game();
    in_the_wilds(&mut app, 42);
    ok(&mut app, "save_game", json!({"what": "world", "slot": "wild"}));
    // A save of version 1 by a generator that made another manifest: same region and seed, another hash.
    let path = saves.path().join("world/wild.save");
    as_version_1(&path);
    let mut file = read_save(&path);
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

#[test]
fn a_saved_manifest_naming_a_tile_the_game_no_longer_has_is_refused_and_nothing_changes() {
    let saves = Saves::new("gone");
    let mut app = saves.game();
    in_the_wilds(&mut app, 42);
    ok(&mut app, "save_game", json!({"what": "world", "slot": "gone"}));
    // The save of an older game, whose wilds had a tile `old_ruin`: whole (its hash is its manifest's).
    let path = saves.path().join("world/gone.save");
    let mut file = read_save(&path);
    let active = &mut file.sections.0.get_mut("region").unwrap().data["active"];
    active["manifest"]["tiles"][0]["tile"] = json!("old_ruin");
    let manifest: RegionManifest = shards::serde_json::from_value(active["manifest"].clone()).unwrap();
    active["manifest_hash"] = json!(format!("{:016x}", manifest.hash()));
    fs::write(&path, file.encode()).unwrap();

    let mut other = saves.game();
    in_the_wilds(&mut other, 43);
    let (before, built) = (region(&other), parts(&mut other));
    let err = fails(&mut other, "load_game", json!({"what": "world", "slot": "gone"}));
    assert!(err.contains("section region: the save's world cannot be built: the manifest of wilds seed 42 v1 names tiles content does not have: old_ruin"), "{err}");
    assert!(err.contains("keep that content in the game, or write a migration for `region`"), "{err}");
    assert!(err.ends_with("; nothing was applied: fix the cause and load again") && !err.contains("partly loaded"), "{err}");
    let recording = shards::ai_core::replay::save(other.world());
    let events = shards::serde_json::to_string(&recording.events).unwrap();
    assert!(recording.start.is_none() && !events.contains("load_game"), "a load that applied nothing is not recorded: {events}");
    assert_eq!(region(&other), before, "the region there stays");
    assert_eq!(parts(&mut other), built, "with every entity of it");
    assert!(lines(&other, "region unloaded").is_empty() && lines(&other, "built from the save's manifest").is_empty(), "{:?}", story(&other));
    assert!(warns(&other, "restored from backup").is_empty(), "missing content is not damage");
}

#[test]
fn a_saved_region_content_no_longer_defines_is_built_from_its_manifest_and_the_journal_says_so() {
    let saves = Saves::new("renamed");
    let mut app = saves.game();
    in_the_wilds(&mut app, 42);
    let tiles = app.world().resource::<ActiveRegion>().manifest.tiles.clone();
    ok(&mut app, "save_game", json!({"what": "world", "slot": "renamed"}));
    // The save of an older game whose region was called `old_wilds`: content has no such region
    // now, but every tile of it is still there. Whole (its hash is its manifest's).
    let path = saves.path().join("world/renamed.save");
    let mut file = read_save(&path);
    let active = &mut file.sections.0.get_mut("region").unwrap().data["active"];
    active["id"] = json!("old_wilds");
    active["manifest"]["region"] = json!("old_wilds");
    let manifest: RegionManifest = shards::serde_json::from_value(active["manifest"].clone()).unwrap();
    active["manifest_hash"] = json!(format!("{:016x}", manifest.hash()));
    fs::write(&path, file.encode()).unwrap();

    let mut fresh = saves.game();
    ok(&mut fresh, "load_game", json!({"what": "world", "slot": "renamed"}));
    assert_eq!(region(&fresh), Some(("old_wilds".to_owned(), 42, manifest.hash())));
    assert_eq!(fresh.world().resource::<ActiveRegion>().manifest.tiles, tiles, "the saved tiles, turned the same way");
    let said = lines(&fresh, "region old_wilds seed 42 v1: built from the save's manifest (`old_wilds` is no longer in the content)");
    assert_eq!(said.len(), 1, "{:?}", story(&fresh));
    assert!(lines(&fresh, "region generated").is_empty(), "nothing is generated: {:?}", story(&fresh));
    // The journal says where the region came from once it stands.
    let events = events(&fresh);
    let loaded = events.iter().position(|l| l.contains("region loaded: old_wilds seed 42 v1"));
    let built = events.iter().position(|l| l.contains("built from the save's manifest"));
    assert!(loaded.is_some() && loaded < built, "{events:?}");
}

#[test]
fn a_manifest_that_does_not_match_its_hash_is_damage_and_a_backup_stands_in() {
    let saves = Saves::new("tampered");
    let mut app = saves.game();
    in_the_wilds(&mut app, 42);
    let wilds = region(&app).unwrap();
    // One save of `once`, two of `twice` (the first is its backup).
    for slot in ["once", "twice", "twice"] {
        ok(&mut app, "save_game", json!({"what": "world", "slot": slot}));
    }
    let turn = |slot: &str| {
        let path = saves.path().join(format!("world/{slot}.save"));
        let mut file = read_save(&path);
        let rot = &mut file.sections.0.get_mut("region").unwrap().data["active"]["manifest"]["tiles"][0]["rot"];
        *rot = json!((rot.as_u64().unwrap() + 90) % 360);
        fs::write(&path, file.encode()).unwrap();
    };
    turn("once");
    turn("twice");
    let damage = "section region does not read: the manifest of wilds seed 42 v1 is damaged: it hashes to ";

    let mut fresh = saves.game();
    let err = fails(&mut fresh, "load_game", json!({"what": "world", "slot": "once"}));
    assert!(err.contains("cannot load world/once, every copy is unusable, nothing was applied") && err.contains(damage), "{err}");
    assert!(err.contains(&format!("the save says {:016x}", wilds.2)), "{err}");
    assert_eq!(region(&fresh), None, "nothing was built");

    let out = text(&ok(&mut fresh, "load_game", json!({"what": "world", "slot": "twice"}))).to_owned();
    assert!(out.contains("from backup 1"), "{out}");
    assert_eq!(region(&fresh), Some(wilds));
    let said = warns(&fresh, "save: world/twice.save is damaged (");
    assert!(said.len() == 1 && said[0].contains(damage), "{said:?}");
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

/// The downed state (B5) is not saved: a body saved while down has no health, so it is saved as
/// dead, and loading that save never lays a body down.
#[test]
fn loaded_zero_health_body_stays_dead() {
    use shards::action::Downed;
    let downed = |app: &App| app.world().get::<Downed>(player_entity(app)).is_some();
    let saves = Saves::new("downed");
    let mut app = saves.game();
    ok(&mut app, "set", json!({"id": PLAYER, "component": "Fighter", "path": "moveset", "value": "partner"}));
    step(&mut app, 2);
    ok(&mut app, "save_game", json!({"what": "character", "slot": "standing"}));
    let saved = health(&app);
    ok(&mut app, "down", json!({"id": PLAYER, "bleed": 10}));
    step(&mut app, 2);
    assert!(downed(&app) && !dead(&app));
    ok(&mut app, "save_game", json!({"what": "character", "slot": "down"}));

    // Loaded into a standing body: no health, so it dies on the next tick (no hook is asked to lay it
    // down: its health ran out, no blow), and it is dead, not down.
    ok(&mut app, "heal", json!({"id": PLAYER}));
    assert!(!downed(&app) && !dead(&app));
    ok(&mut app, "load_game", json!({"what": "character", "slot": "down"}));
    step(&mut app, 1);
    assert!(dead(&app) && !downed(&app), "saved down is saved dead");

    // Loaded into a downed body: down no more, and dead on the next tick.
    ok(&mut app, "heal", json!({"id": PLAYER}));
    ok(&mut app, "down", json!({"id": PLAYER}));
    ok(&mut app, "load_game", json!({"what": "character", "slot": "down"}));
    assert!(!downed(&app), "the load ends the downed state");
    step(&mut app, 1);
    assert!(dead(&app) && !downed(&app));

    // A downed body loading a save with health stands up with it.
    ok(&mut app, "heal", json!({"id": PLAYER}));
    ok(&mut app, "down", json!({"id": PLAYER}));
    ok(&mut app, "load_game", json!({"what": "character", "slot": "standing"}));
    assert!(!downed(&app) && !dead(&app));
    assert_eq!(health(&app), saved);
    assert_ne!(action(&app, PLAYER), "spar_down", "the downed pose is cut");
    step(&mut app, 30);
    assert!(!dead(&app));
}
