//! A save from a newer version of the game is not damage: it is refused, never repaired from
//! a backup, never renamed, never rotated away by this build's saves.

use std::collections::BTreeMap;
use std::fs;

use serde_json::json;
use shards::ai_core::replay;
use shards::ai_core::JournalKind;
use shards::save::{SaveFile, SaveRequests, SaveWhat};

use shards::prelude::*;

use crate::common::*;

const PRIMARY: &str = "world/slot1.save";

/// The world saved three times: day 1 (`.bak2`), day 2 (`.bak1`), day 3 (the save).
fn three_days(dir: &TempDir) -> App {
    let mut app = game(dir);
    for day in 1..=3 {
        app.world_mut().resource_mut::<Calendar>().day = day;
        ok(&mut app, "save_game", json!({"what": "world", "label": format!("day {day}")}));
        step(&mut app, 1);
    }
    app
}

/// Every file of the save folder: name → bytes.
fn files_of(dir: &TempDir) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for kind in ["world", "character"] {
        let Ok(entries) = fs::read_dir(dir.file(kind)) else { continue };
        for entry in entries {
            let entry = entry.unwrap();
            out.insert(format!("{kind}/{}", entry.file_name().to_string_lossy()), fs::read(entry.path()).unwrap());
        }
    }
    out
}

fn edited(bytes: &[u8], edit: impl FnOnce(&mut SaveFile)) -> Vec<u8> {
    let mut file = SaveFile::decode(bytes).unwrap();
    edit(&mut file);
    file.encode()
}

/// A file whose envelope says it was written by the next engine format.
fn newer_envelope(bytes: &[u8]) -> Vec<u8> {
    String::from_utf8_lossy(bytes).replacen("shards-save/1", "shards-save/2", 1).into_bytes()
}

/// How the primary becomes a newer one, and what the error says is newer.
fn newer_primaries() -> Vec<(&'static str, &'static str, Box<dyn Fn(&[u8]) -> Vec<u8>>)> {
    vec![
        ("a section above this build's", "section calendar v7 > v1", Box::new(|b| edited(b, |f| f.sections.0.get_mut("calendar").unwrap().version = 7))),
        ("a newer envelope", "save format shards-save/2 > shards-save/1", Box::new(newer_envelope)),
    ]
}

#[test]
fn a_newer_primary_is_refused_and_nothing_on_disk_changes() {
    for (what, detail, make) in newer_primaries() {
        let dir = TempDir::new("newer");
        let mut app = three_days(&dir);
        let bytes = fs::read(dir.file(PRIMARY)).unwrap();
        fs::write(dir.file(PRIMARY), make(&bytes)).unwrap();
        let on_disk = files_of(&dir);
        let events = replay::save(app.world()).events.len();
        app.world_mut().resource_mut::<Calendar>().day = 99;

        let err = fails(&mut app, "load_game", json!({"what": "world", "slot": "slot1"}));
        assert!(
            err.starts_with(&format!("world/slot1 was saved by a newer version of the game ({detail}); nothing was changed")),
            "{what}: {err}"
        );
        assert!(err.contains("use another slot"), "{what}: the error says what to do: {err}");
        assert_eq!(calendar(&app).day, 99, "{what}: nothing was applied");
        assert_eq!(files_of(&dir), on_disk, "{what}: byte-identical: no older backup tried, no .damaged, no rewritten primary");
        assert!(!dir.file("world/slot1.save.damaged").exists(), "{what}");
        assert!(lines(&app, JournalKind::Warn, "restored from backup").is_empty() && lines(&app, JournalKind::Warn, "damaged").is_empty(), "{what}: {:?}", journal(&app));
        assert_eq!(replay::save(app.world()).events.len(), events, "{what}: a load that changed nothing is not in the recording");

        // The other tools say the same, and `save_list` calls it what it is.
        let check = fails(&mut app, "save_check", json!({"what": "world", "slot": "slot1"}));
        assert!(check.starts_with("world/slot1 was saved by a newer version of the game"), "{what}: {check}");
        let list = act(&mut app, "save_list", json!({})).result;
        assert_eq!(list["slots"][0]["files"][0]["state"], "newer", "{what}: {list}");
        assert_eq!(list["slots"][0]["files"][0]["problem"], detail, "{what}");
        assert_eq!(list["slots"][0]["files"][1]["state"], "ok", "{what}: the backups are fine");
        assert!(list["summary"][0].as_str().unwrap().contains("save from a newer version of the game"), "{what}: {list}");
        assert_eq!(files_of(&dir), on_disk, "{what}: looking changes nothing either");
    }
}

#[test]
fn a_newer_backup_behind_a_damaged_primary_is_not_skipped_for_an_older_one() {
    let dir = TempDir::new("newer_backup");
    let mut app = three_days(&dir);
    // bak1 (day 2) is from a newer build; the primary is damaged: bak2 (day 1) would quietly
    // lose the progress the newer build saved, so the load stops.
    let bak1 = fs::read(dir.file("world/slot1.save.bak1")).unwrap();
    fs::write(dir.file("world/slot1.save.bak1"), edited(&bak1, |f| f.sections.0.get_mut("calendar").unwrap().version = 7)).unwrap();
    ok(&mut app, "save_damage", json!({"what": "world", "how": "flip"}));
    let on_disk = files_of(&dir);
    app.world_mut().resource_mut::<Calendar>().day = 99;

    let err = fails(&mut app, "load_game", json!({"what": "world"}));
    assert!(err.starts_with("world/slot1 (backup 1) was saved by a newer version of the game (section calendar v7 > v1); nothing was changed"), "{err}");
    assert!(err.contains("the copies before it are unusable: slot1.save: checksum mismatch"), "{err}");
    assert_eq!((calendar(&app).day, files_of(&dir)), (99, on_disk), "nothing applied, nothing moved");
}

#[test]
fn saving_never_overwrites_or_rotates_away_a_newer_save() {
    for (what, detail, make) in newer_primaries() {
        let dir = TempDir::new("newer_write");
        let mut app = three_days(&dir);
        let bytes = fs::read(dir.file(PRIMARY)).unwrap();
        fs::write(dir.file(PRIMARY), make(&bytes)).unwrap();
        let on_disk = files_of(&dir);
        app.world_mut().resource_mut::<Calendar>().day = 4;

        let err = fails(&mut app, "save_game", json!({"what": "world"}));
        assert!(
            err.contains(&format!("world/slot1 was saved by a newer version of the game ({detail}), so this build does not overwrite it and nothing was written")),
            "{what}: {err}"
        );
        assert!(err.contains("Save into another slot"), "{what}: {err}");
        assert_eq!(files_of(&dir), on_disk, "{what}: the newer file is kept as it is, no rotation, no .damaged, no .tmp");
        assert_eq!(lines(&app, JournalKind::Error, "save: world/slot1 not saved: world/slot1 was saved by a newer version").len(), 1, "{what}: {:?}", journal(&app));

        // An autosave for the slot is refused the same way, on the tick it asks, and is not retried.
        app.world_mut().resource_mut::<SaveRequests>().request(SaveWhat::World, "camp");
        step(&mut app, 2);
        assert_eq!(files_of(&dir), on_disk, "{what}: the autosave wrote nothing");
        assert_eq!(lines(&app, JournalKind::Error, "not saved").len(), 2, "{what}: {:?}", journal(&app));
        assert!(!app.world().resource::<SaveRequests>().is_pending(), "{what}");

        // Another slot is free to use, and the character (its own folder) saves as ever.
        ok(&mut app, "save_game", json!({"what": "world", "slot": "slot2"}));
        ok(&mut app, "save_game", json!({"what": "character", "slot": "slot1"}));
        let after = files_of(&dir);
        for (name, bytes) in &on_disk {
            assert_eq!(after.get(name), Some(bytes), "{what}: {name} is as it was");
        }
        assert!(after.contains_key("world/slot2.save") && after.contains_key("character/slot1.save"), "{what}: {:?}", after.keys().collect::<Vec<_>>());
    }
}

#[test]
fn a_newer_backup_is_not_pushed_out_by_a_save_over_a_damaged_primary() {
    let dir = TempDir::new("newer_shift");
    let mut app = three_days(&dir);
    let bak1 = fs::read(dir.file("world/slot1.save.bak1")).unwrap();
    fs::write(dir.file("world/slot1.save.bak1"), newer_envelope(&bak1)).unwrap();
    ok(&mut app, "save_damage", json!({"what": "world", "how": "garbage"}));
    let on_disk = files_of(&dir);
    let err = fails(&mut app, "save_game", json!({"what": "world"}));
    assert!(err.contains("world/slot1 (backup 1) was saved by a newer version of the game (save format shards-save/2 > shards-save/1)"), "{err}");
    assert_eq!(files_of(&dir), on_disk);
}

#[test]
fn damage_that_is_only_damage_is_still_repaired_and_overwritten() {
    // The guard looks for newer saves, not at every kind of trouble.
    let dir = TempDir::new("still_damage");
    let mut app = three_days(&dir);
    ok(&mut app, "save_damage", json!({"what": "world", "how": "truncate"}));
    ok(&mut app, "save_game", json!({"what": "world", "label": "day 4"}));
    assert!(dir.file("world/slot1.save.damaged").exists(), "the damaged primary is set aside, as before");
    app.world_mut().resource_mut::<Calendar>().day = 99;
    ok(&mut app, "load_game", json!({"what": "world"}));
    assert_eq!(calendar(&app).day, 3);
}
