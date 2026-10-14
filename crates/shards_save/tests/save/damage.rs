//! A damaged file is restored from a backup, with a journal line; when every copy is bad the
//! error says what is wrong with each and nothing is applied.

use std::fs;

use serde_json::json;
use shards::ai_core::JournalKind;
use shards::save::SaveFile;

use shards::prelude::*;

use crate::common::*;

const PRIMARY: &str = "world/slot1.save";

/// The world saved three times: day 1 (now `.bak2`), day 2 (`.bak1`), day 3 (the save).
fn three_days(dir: &TempDir) -> App {
    let mut app = game(dir);
    for day in 1..=3 {
        app.world_mut().resource_mut::<Calendar>().day = day;
        ok(&mut app, "save_game", json!({"what": "world", "label": format!("day {day}")}));
        step(&mut app, 1);
    }
    app
}

/// A file with a right header around any body: damage the checksum cannot see.
fn forged(body: &str) -> Vec<u8> {
    let mut out = format!("shards-save/1 crc32={:08x} len={}\n", crc32fast::hash(body.as_bytes()), body.len()).into_bytes();
    out.extend_from_slice(body.as_bytes());
    out
}

fn edited(bytes: &[u8], edit: impl FnOnce(&mut SaveFile)) -> Vec<u8> {
    let mut file = SaveFile::decode(bytes).unwrap();
    edit(&mut file);
    file.encode()
}

/// What a damage does to a file's bytes.
type Damage = Box<dyn Fn(&[u8]) -> Vec<u8>>;

/// How the primary gets damaged, and the word its journal line must carry.
fn damages() -> Vec<(&'static str, &'static str, Damage)> {
    vec![
        ("a flipped byte", "checksum mismatch", Box::new(|b| {
            let mut b = b.to_vec();
            let at = b.len() - 9;
            b[at] ^= 0x04;
            b
        })),
        ("a truncated file", "length mismatch", Box::new(|b| b[..b.len() / 2].to_vec())),
        ("a wrong length in the header", "length mismatch", Box::new(|b| String::from_utf8_lossy(b).replacen("len=", "len=9", 1).into_bytes())),
        ("JSON that does not parse", "bad JSON at byte", Box::new(|_| forged("{\"kind\": \"world\", "))),
        ("JSON of the wrong shape", "not a save", Box::new(|_| forged("[1, 2, 3]"))),
        ("a file without its header", "no header", Box::new(|b| b[b.iter().position(|c| *c == b'\n').unwrap() + 1..].to_vec())),
        ("a zero-length file", "file is empty", Box::new(|_| Vec::new())),
        ("a section from a newer version", "section calendar was saved by a newer version: 7 > 1", Box::new(|b| {
            edited(b, |f| f.sections.0.get_mut("calendar").unwrap().version = 7)
        })),
        ("a section that does not read", "section calendar does not read", Box::new(|b| {
            edited(b, |f| f.sections.0.get_mut("calendar").unwrap().data = json!("not a calendar"))
        })),
        ("a file of the other kind", "it is a character save, not a world save", Box::new(|b| edited(b, |f| f.kind = shards::save::SaveKind::Character))),
    ]
}

#[test]
fn every_kind_of_damage_to_the_primary_is_restored_from_the_first_backup() {
    for (what, word, damage) in damages() {
        let dir = TempDir::new("matrix");
        let mut app = three_days(&dir);
        let primary = dir.file(PRIMARY);
        let before = fs::read(&primary).unwrap();
        let backup = fs::read(dir.file("world/slot1.save.bak1")).unwrap();
        fs::write(&primary, damage(&before)).unwrap();
        app.world_mut().resource_mut::<Calendar>().day = 99;

        let out = ok(&mut app, "load_game", json!({"what": "world", "slot": "slot1"}));
        assert!(out.contains("from backup 1"), "{what}: {out}");
        assert_eq!(calendar(&app).day, 2, "{what}: the newest good copy is day 2");

        let warns = lines(&app, JournalKind::Warn, "save: world/slot1.save is damaged");
        assert_eq!(warns.len(), 1, "{what}: {warns:?}");
        let tick = SaveFile::decode(&backup).unwrap().tick;
        assert!(warns[0].contains(word), "{what}: {}", warns[0]);
        assert!(warns[0].ends_with(&format!("); restored from backup 1 (tick {tick})")), "{what}: {}", warns[0]);

        // The primary is a good file again, the one the backup is; the damaged one is kept.
        assert_eq!(fs::read(&primary).unwrap(), backup, "{what}: the primary is rewritten from the backup");
        assert!(dir.file("world/slot1.save.damaged").exists(), "{what}: the damaged file is kept aside");
        assert!(!dir.file("world/slot1.save.tmp").exists());
        app.world_mut().resource_mut::<Calendar>().day = 50;
        let again = ok(&mut app, "load_game", json!({"what": "world", "slot": "slot1"}));
        assert!(!again.contains("backup"), "{what}: {again}");
        assert_eq!(lines(&app, JournalKind::Warn, "is damaged").len(), 1, "{what}: no second warning");
    }
}

#[test]
fn the_next_backup_is_tried_when_the_first_is_bad_too() {
    let dir = TempDir::new("second");
    let mut app = three_days(&dir);
    for name in ["world/slot1.save", "world/slot1.save.bak1"] {
        let mut bytes = fs::read(dir.file(name)).unwrap();
        bytes[40] ^= 0xff;
        fs::write(dir.file(name), bytes).unwrap();
    }
    ok(&mut app, "load_game", json!({"what": "world"}));
    assert_eq!(calendar(&app).day, 1);
    let line = &lines(&app, JournalKind::Warn, "restored from backup")[0];
    assert!(line.contains("restored from backup 2"), "{line}");
    assert_eq!(fs::read(dir.file(PRIMARY)).unwrap(), fs::read(dir.file("world/slot1.save.bak2")).unwrap());
}

#[test]
fn when_every_copy_is_bad_the_error_names_each_and_nothing_is_applied() {
    let dir = TempDir::new("allbad");
    let mut app = three_days(&dir);
    fs::write(dir.file("world/slot1.save"), b"").unwrap();
    fs::write(dir.file("world/slot1.save.bak1"), b"not a save at all\nreally").unwrap();
    let mut bak2 = fs::read(dir.file("world/slot1.save.bak2")).unwrap();
    bak2.truncate(60);
    fs::write(dir.file("world/slot1.save.bak2"), bak2).unwrap();
    app.world_mut().resource_mut::<Calendar>().day = 99;

    let err = fails(&mut app, "load_game", json!({"what": "world"}));
    for part in [
        "cannot load world/slot1",
        "nothing was applied",
        "slot1.save: file is empty",
        "slot1.save.bak1: no header",
        "slot1.save.bak2: length mismatch",
        "slot1.save.bak3: missing",
    ] {
        assert!(err.contains(part), "{part}: {err}");
    }
    assert_eq!(calendar(&app).day, 99, "nothing was applied");
    assert!(fs::read(dir.file("world/slot1.save")).unwrap().is_empty(), "and nothing was rewritten");
}

#[test]
fn a_slot_that_was_never_saved_says_so() {
    let dir = TempDir::new("none");
    let mut app = game(&dir);
    let err = fails(&mut app, "load_game", json!({"what": "world", "slot": "nothing-here"}));
    assert!(err.starts_with("there is no world save in slot `nothing-here`") && err.contains("save_list"), "{err}");
}

#[test]
fn a_crash_while_writing_leaves_the_previous_save_loadable() {
    let dir = TempDir::new("crash");
    let mut app = three_days(&dir);
    // Died after writing the temporary file, before the rename.
    fs::write(dir.file("world/slot1.save.tmp"), b"shards-save/1 crc32=00000000 len=").unwrap();
    app.world_mut().resource_mut::<Calendar>().day = 99;
    let out = ok(&mut app, "load_game", json!({"what": "world"}));
    assert!(!out.contains("backup"), "{out}");
    assert_eq!(calendar(&app).day, 3);
    assert!(!dir.file("world/slot1.save.tmp").exists(), "the leftover is cleaned up");

    // Died after the older files moved up, before the rename: no primary at all.
    fs::remove_file(dir.file(PRIMARY)).unwrap();
    fs::write(dir.file("world/slot1.save.tmp"), b"half").unwrap();
    app.world_mut().resource_mut::<Calendar>().day = 99;
    ok(&mut app, "load_game", json!({"what": "world"}));
    assert_eq!(calendar(&app).day, 2, "the save before it");
    assert!(lines(&app, JournalKind::Warn, "world/slot1.save is missing; restored from backup 1").len() == 1, "{:?}", journal(&app));
    assert!(dir.file(PRIMARY).exists());
}

#[test]
fn exactly_three_backups_are_kept() {
    let dir = TempDir::new("rotation");
    let mut app = game(&dir);
    for day in 1..=7 {
        app.world_mut().resource_mut::<Calendar>().day = day;
        ok(&mut app, "save_game", json!({"what": "world"}));
    }
    let mut names: Vec<String> = fs::read_dir(dir.file("world")).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
    names.sort();
    assert_eq!(names, ["slot1.save", "slot1.save.bak1", "slot1.save.bak2", "slot1.save.bak3"]);
    let day = |name: &str| SaveFile::decode(&fs::read(dir.file(name)).unwrap()).unwrap().sections.0["calendar"].data["day"].as_u64().unwrap();
    assert_eq!([day(PRIMARY), day("world/slot1.save.bak1"), day("world/slot1.save.bak2"), day("world/slot1.save.bak3")], [7, 6, 5, 4]);
}

#[test]
fn a_damaged_primary_is_not_made_a_backup_by_the_next_save() {
    let dir = TempDir::new("nopromote");
    let mut app = three_days(&dir);
    let mut bytes = fs::read(dir.file(PRIMARY)).unwrap();
    bytes[50] ^= 0x10;
    fs::write(dir.file(PRIMARY), bytes).unwrap();
    app.world_mut().resource_mut::<Calendar>().day = 4;
    ok(&mut app, "save_game", json!({"what": "world"}));
    let list = act(&mut app, "save_list", json!({})).result;
    let files = list["slots"][0]["files"].as_array().unwrap();
    let states: Vec<&str> = files.iter().map(|f| f["state"].as_str().unwrap()).collect();
    assert_eq!(states, ["ok", "ok", "ok", "missing"], "{list}");
    assert!(list["slots"][0]["damaged_copy"].as_u64().is_some());
}

#[test]
fn save_damage_breaks_files_so_the_recovery_can_be_tried() {
    let dir = TempDir::new("tool");
    let mut app = three_days(&dir);
    for how in ["flip", "truncate", "garbage"] {
        let out = ok(&mut app, "save_damage", json!({"what": "world", "how": how}));
        assert!(out.contains("world/slot1.save"), "{out}");
        app.world_mut().resource_mut::<Calendar>().day = 99;
        ok(&mut app, "load_game", json!({"what": "world"}));
        assert_eq!(calendar(&app).day, 2, "{how}");
    }
    assert_eq!(lines(&app, JournalKind::Warn, "restored from backup 1").len(), 3);

    // A backup can be hit too; a file that is not there cannot.
    ok(&mut app, "save_damage", json!({"what": "world", "how": "flip", "copy": "bak1"}));
    let err = fails(&mut app, "save_damage", json!({"what": "character", "how": "flip"}));
    assert!(err.contains("character/slot1.save does not exist"), "{err}");
    let err = fails(&mut app, "save_damage", json!({"what": "world", "how": "flip", "slot": "../etc"}));
    assert!(err.contains("is not allowed"), "{err}");
    assert!(!act(&mut app, "save_damage", json!({"what": "world", "how": "bend"})).ok);
}
