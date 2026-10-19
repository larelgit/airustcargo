//! Old files load: the committed samples were written by a game whose sections were all at
//! version 1; the game here reads `inventory` at version 3 through a chain of two migrations.

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::json;
use shards::ai_core::JournalKind;
use shards::save::{SaveFile, SaveKind, SaveSection};

use shards::prelude::*;

use crate::common::*;

const SAMPLES: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/samples");

/// The inventory as the game first wrote it: gold and the names of the items.
#[derive(Serialize, Deserialize)]
struct InventoryV1 {
    gold: u32,
    items: Vec<String>,
}

impl SaveSection for InventoryV1 {
    const NAME: &'static str = "inventory";
    const VERSION: u32 = 1;
    const KIND: SaveKind = SaveKind::Character;

    fn capture(world: &World, body: Option<Entity>) -> Result<Self, String> {
        let hero = world.get::<Hero>(body.ok_or("no body")?).ok_or("no hero")?;
        let items = hero.items.iter().flat_map(|i| std::iter::repeat_n(i.name.clone(), i.count as usize)).collect();
        Ok(InventoryV1 { gold: hero.coins.gold, items })
    }

    fn apply(self, _: &mut World, _: Option<Entity>) -> Result<(), String> {
        Err("version 1 is only written".to_owned())
    }
}

/// Writes the samples. They are old files: committed once and never rewritten, so this refuses
/// to overwrite one (delete a file on purpose to make it again). Run with
/// `cargo test -p shards_save --test save -- --ignored write_v1_samples`.
#[test]
#[ignore = "writes tests/samples/*.save once; they are committed"]
fn write_v1_samples() {
    let dir = TempDir::new("samples");
    let mut app = bare_in(&dir, |app| {
        app.save_resource::<Calendar>("calendar", 1).save_component::<Chest>(SaveKind::World, "chests", 1).add_save_section::<InventoryV1>();
    });
    set_hero(&mut app, Hero { coins: Coins { gold: 12, silver: 0 }, items: vec![Item { name: "sword".into(), count: 1 }, Item { name: "potion".into(), count: 2 }], ..Hero::default() });
    *app.world_mut().resource_mut::<Calendar>() = Calendar { day: 40, season: "autumn".into(), omens: Default::default() };
    set_chests(&mut app, &[Chest { open: true, loot: vec![5] }, Chest::default()]);
    shards::save::save_slot(app.world_mut(), shards::save::SaveWhat::All, "old", "version 1 sample", "sample").unwrap();
    for (from, to) in [("world/old.save", "world_v1.save"), ("character/old.save", "character_v1.save")] {
        let to = PathBuf::from(SAMPLES).join(to);
        assert!(!to.exists(), "{} is committed: old files are never rewritten", to.display());
        fs::create_dir_all(SAMPLES).unwrap();
        fs::copy(dir.file(from), &to).unwrap();
    }
}

/// The samples as slot `old` of a fresh save folder.
fn with_samples(dir: &TempDir) {
    for (kind, sample) in [("world", "world_v1.save"), ("character", "character_v1.save")] {
        fs::create_dir_all(dir.file(kind)).unwrap();
        fs::copy(PathBuf::from(SAMPLES).join(sample), dir.file(&format!("{kind}/old.save"))).unwrap();
    }
}

/// The game of version 3: `inventory` through the chain, `weather` which the samples predate.
fn current(dir: &TempDir) -> App {
    bare_in(dir, |app| {
        app.save_resource::<Calendar>("calendar", 1).save_component::<Chest>(SaveKind::World, "chests", 1);
        register_inventory(app);
    })
}

#[test]
fn the_committed_samples_really_are_version_one() {
    for sample in ["world_v1.save", "character_v1.save"] {
        let file = SaveFile::decode(&fs::read(PathBuf::from(SAMPLES).join(sample)).unwrap()).unwrap();
        assert!(file.sections.0.values().all(|s| s.version == 1), "{sample}: {:?}", file.sections.0.iter().map(|(n, s)| (n, s.version)).collect::<Vec<_>>());
        assert_eq!(file.label, "version 1 sample");
    }
    let character = SaveFile::decode(&fs::read(PathBuf::from(SAMPLES).join("character_v1.save")).unwrap()).unwrap();
    assert_eq!(character.sections.0["inventory"].data, json!({"gold": 12, "items": ["sword", "potion", "potion"]}));
}

#[test]
fn old_files_migrate_and_load() {
    let dir = TempDir::new("old");
    with_samples(&dir);
    let mut app = current(&dir);
    let out = ok(&mut app, "load_game", json!({"what": "all", "slot": "old"}));
    assert!(out.contains("world/old loaded") && out.contains("character/old loaded"), "{out}");

    // inventory: 1 -> 2 (counted items) -> 3 (coins).
    let h = hero(&mut app);
    assert_eq!(h.coins, Coins { gold: 12, silver: 0 });
    assert_eq!(h.items, vec![Item { name: "potion".into(), count: 2 }, Item { name: "sword".into(), count: 1 }]);
    // Untouched sections of the old file.
    assert_eq!(calendar(&app).day, 40);
    assert_eq!(calendar(&app).season, "autumn");
    assert_eq!(chests(&mut app)[0], ("chest.0".to_owned(), Chest { open: true, loot: vec![5] }));
    // `weather` is newer than the file: its `missing` ran.
    assert_eq!(app.world().resource::<Weather>().0, "clear");
    assert!(lines(&app, JournalKind::Warn, "").is_empty(), "no warnings: {:?}", journal(&app));

    // The file is not touched by reading it; saving again writes the current versions.
    let sample = fs::read(dir.file("world/old.save")).unwrap();
    assert_eq!(sample, fs::read(PathBuf::from(SAMPLES).join("world_v1.save")).unwrap());
    ok(&mut app, "save_game", json!({"what": "character", "slot": "new"}));
    let saved = SaveFile::decode(&fs::read(dir.file("character/new.save")).unwrap()).unwrap();
    assert_eq!(saved.sections.0["inventory"].version, 3);
    assert_eq!(saved.sections.0["inventory"].data["coins"], json!({"gold": 12, "silver": 0}));

    // `save_check` against the old file: the state is what the file says, plus the section it lacks.
    let check = act(&mut app, "save_check", json!({"what": "world", "slot": "old"})).result;
    assert_eq!(check["world"]["file"]["differing"], json!([]), "{check}");
    assert_eq!(check["world"]["file"]["only_here"], json!(["rules", "encounters", "weather"]), "{check}");
}

#[test]
#[should_panic(expected = "save section `inventory` is at version 3 but has no migration from version 2")]
fn a_gap_in_the_chain_panics_when_the_app_is_finished() {
    let dir = TempDir::new("gap");
    drop(bare_in(&dir, |app| {
        app.add_save_section::<InventoryV3>().add_save_migration("inventory", 1, inventory_1_to_2);
    }));
}

#[test]
#[should_panic(expected = "no one registers")]
fn a_migration_for_a_section_that_is_not_there_panics_too() {
    let dir = TempDir::new("typo");
    drop(bare_in(&dir, |app| {
        app.add_save_migration("inventroy", 1, inventory_1_to_2);
    }));
}

#[test]
fn a_section_from_a_newer_version_is_refused_by_name() {
    let dir = TempDir::new("newer");
    let mut app = current(&dir);
    ok(&mut app, "save_game", json!({"what": "character"}));
    let path = dir.file("character/slot1.save");
    let mut file = SaveFile::decode(&fs::read(&path).unwrap()).unwrap();
    file.sections.0.get_mut("inventory").unwrap().version = 7;
    fs::write(&path, file.encode()).unwrap();
    let err = fails(&mut app, "load_game", json!({"what": "character"}));
    assert!(
        err.starts_with("character/slot1 was saved by a newer version of the game (section inventory v7 > v3); nothing was changed"),
        "{err}"
    );
}

#[test]
fn a_section_this_game_does_not_save_is_skipped_with_a_warning() {
    let dir = TempDir::new("unknown");
    with_samples(&dir);
    let path = dir.file("character/old.save");
    let mut file = SaveFile::decode(&fs::read(&path).unwrap()).unwrap();
    file.sections.0.insert("mystery".into(), shards::save::SectionData { version: 4, data: json!({"a": 1}) });
    fs::write(&path, file.encode()).unwrap();
    let mut app = current(&dir);
    ok(&mut app, "load_game", json!({"what": "character", "slot": "old"}));
    let warns = lines(&app, JournalKind::Warn, "unknown character section mystery");
    assert_eq!(warns.len(), 1, "{:?}", journal(&app));
    assert_eq!(hero(&mut app).coins.gold, 12, "the rest loaded");
    // `save_check` names it.
    let check = act(&mut app, "save_check", json!({"what": "character", "slot": "old"})).result;
    assert_eq!(check["character"]["file"]["only_in_file"], json!(["mystery"]), "{check}");
}

#[test]
fn a_migration_that_fails_makes_the_copy_damaged_and_a_backup_is_used() {
    let dir = TempDir::new("badmigration");
    let mut app = current(&dir);
    for gold in [3, 4] {
        set_hero(&mut app, Hero { coins: Coins { gold, silver: 0 }, ..Hero::default() });
        ok(&mut app, "save_game", json!({"what": "character"}));
    }
    // The primary is a version 1 inventory the first migration cannot read: no `gold`.
    let path = dir.file("character/slot1.save");
    let mut file = SaveFile::decode(&fs::read(&path).unwrap()).unwrap();
    *file.sections.0.get_mut("inventory").unwrap() = shards::save::SectionData { version: 1, data: json!({"items": ["x"]}) };
    fs::write(&path, file.encode()).unwrap();

    set_hero(&mut app, Hero::default());
    ok(&mut app, "load_game", json!({"what": "character"}));
    assert_eq!(hero(&mut app).coins.gold, 3, "the backup before the bad one");
    let warn = &lines(&app, JournalKind::Warn, "restored from backup 1")[0];
    assert!(warn.contains("section inventory: migration from version 1 failed: version 1 has no gold"), "{warn}");
}

#[test]
fn the_registry_lists_what_is_saved() {
    let dir = TempDir::new("registry");
    let mut app = current(&dir);
    let out = act(&mut app, "save_registry", json!({})).result;
    let sections = out["sections"].as_array().unwrap();
    let find = |name: &str| sections.iter().find(|s| s["name"] == name).unwrap_or_else(|| panic!("{name} in {out}"));
    assert_eq!(find("inventory")["migrations_from"], json!([1, 2]));
    assert_eq!((find("inventory")["version"].as_u64(), find("inventory")["kind"].as_str()), (Some(3), Some("character")));
    assert_eq!(find("calendar")["kind"], "world");
    // The engine's own, registered by the umbrella: world first, then the character.
    for (name, kind) in [("region", "world"), ("position", "character"), ("vitals", "character")] {
        assert_eq!((find(name)["kind"].as_str(), find(name)["source"].as_str()), (Some(kind), Some("shards")), "{name}");
    }
    assert_eq!(find("inventory")["source"], "save", "the crate the section type comes from (this test crate)");
}
