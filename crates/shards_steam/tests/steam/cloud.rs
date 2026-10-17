//! Saves in Steam Cloud: each save written is copied up (never a backup or a temporary file); at
//! startup, before the launch loads a slot, the cloud's copies are held against this machine's.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use shards::ai_core::journal::JournalKind;
use shards::net::MemoryHub;
use shards::prelude::*;
use shards::save::{SaveConfig, SaveFile, SaveKind, Which};
use shards_steam::fake::FIRST_USER;
use shards_steam::{Cloud, FakeSteam};

use crate::*;

fn cloud(world: &World) -> &Cloud {
    world.resource::<Cloud>()
}

fn path(dir: &Path, kind: SaveKind, slot: &str, which: Which) -> PathBuf {
    SaveConfig { dir: dir.to_owned(), ..SaveConfig::default() }.store().path(kind, slot, which)
}

fn now() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs() as i64
}

#[test]
fn every_save_written_is_copied_up_and_nothing_else_is() {
    let steam = FakeSteam::new(MemoryHub::new(1));
    let saves = save_folder();
    let mut app = game_in(config(|_| {}), steam.user(FIRST_USER, "player"), saves.clone());
    assert_eq!(cloud(app.world()).enabled, Some(Ok(())));
    // The world: the tests' game has no player body, whose character would be saved too.
    ok(&mut app, "save_game", json!({"what": "world", "slot": "slot1"}));
    ok(&mut app, "step", json!({"ticks": 5}));
    ok(&mut app, "save_game", json!({"what": "world", "slot": "slot1"}));
    ok(&mut app, "save_game", json!({"what": "world", "slot": "slot2"}));
    let files = steam.cloud(FIRST_USER);
    let names: Vec<&str> = files.keys().map(String::as_str).collect();
    assert_eq!(names, ["saves/world/slot1.save", "saves/world/slot2.save"], "the primaries only: no backups, no .tmp");
    for slot in ["slot1", "slot2"] {
        let disk = std::fs::read(path(&saves, SaveKind::World, slot, Which::Save)).unwrap();
        assert_eq!(files[&format!("saves/world/{slot}.save")].0, disk, "{slot} is the newest save");
    }
    assert!(path(&saves, SaveKind::World, "slot1", Which::Backup(1)).exists(), "the older one is a backup on disk only");
    assert_eq!(cloud(app.world()).mirrored, 3);
}

#[test]
fn with_steam_cloud_off_the_saves_stay_on_this_machine() {
    let steam = FakeSteam::new(MemoryHub::new(1));
    let user = steam.user(FIRST_USER, "player");
    steam.set_cloud_enabled(FIRST_USER, false);
    let mut app = game(config(|_| {}), user);
    ok(&mut app, "save_game", json!({"what": "world", "slot": "slot1"}));
    assert!(steam.cloud(FIRST_USER).is_empty());
    assert!(matches!(&cloud(app.world()).enabled, Some(Err(why)) if why.contains("Steam Cloud is off")), "{:?}", cloud(app.world()).enabled);
    let said = ok(&mut app, "steam", json!({}));
    assert!(said.contains("cloud: off (Steam Cloud is off"), "{said}");
}

/// Two different world saves of the tests' game, older and newer (by tick), as bytes.
fn two_world_saves() -> (Vec<u8>, Vec<u8>) {
    let steam = FakeSteam::new(MemoryHub::new(1));
    let saves = save_folder();
    let user = steam.user(FIRST_USER + 9, "maker");
    steam.set_cloud_enabled(FIRST_USER + 9, false);
    let mut app = game_in(config(|_| {}), user, saves.clone());
    ok(&mut app, "save_game", json!({"what": "world", "slot": "old"}));
    ok(&mut app, "step", json!({"ticks": 10}));
    ok(&mut app, "save_game", json!({"what": "world", "slot": "new"}));
    let read = |slot: &str| std::fs::read(path(&saves, SaveKind::World, slot, Which::Save)).unwrap();
    let saved = (read("old"), read("new"));
    assert_ne!(saved.0, saved.1);
    saved
}

#[test]
fn at_startup_the_newer_side_wins_a_tie_keeps_both_and_the_launch_loads_what_the_cloud_brought() {
    let (old, new) = two_world_saves();
    let saves = save_folder();
    let put = |slot: &str, bytes: &[u8]| {
        let file = path(&saves, SaveKind::World, slot, Which::Save);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
    };
    put("pull", &old);
    put("push", &new);
    put("tie", &old);
    put("only-here", &old);
    put("repair", &old);
    put("same", &new);

    let steam = FakeSteam::new(MemoryHub::new(1));
    let user = steam.user(FIRST_USER, "player");
    let t = now();
    let up = |slot: &str, bytes: &[u8], at: i64| steam.put_cloud(FIRST_USER, &format!("saves/world/{slot}.save"), bytes, at);
    up("pull", &new, t + 100);
    up("push", &old, t - 100);
    up("tie", &new, t);
    up("fresh", &new, t);
    up("repair", b"not a save", t + 100);
    up("same", &new, t - 100);
    steam.put_cloud(FIRST_USER, "saves/world/stray.save.tmp", b"half", t);
    steam.put_cloud(FIRST_USER, "elsewhere/world/x.save", &new, t);

    // `--world fresh`: a slot only the cloud has, loaded at launch.
    let app = game_in(config(|c| c.world_slot = Some("fresh".into())), user, saves.clone());
    let disk = |slot: &str| std::fs::read(path(&saves, SaveKind::World, slot, Which::Save)).ok();
    assert_eq!(disk("pull").as_deref(), Some(new.as_slice()), "the cloud's newer copy is the save now");
    assert_eq!(std::fs::read(path(&saves, SaveKind::World, "pull", Which::Backup(1))).unwrap(), old, "this machine's older one is its backup");
    assert_eq!(disk("push").as_deref(), Some(new.as_slice()));
    assert_eq!(disk("tie").as_deref(), Some(old.as_slice()), "this machine's stays");
    assert_eq!(disk("tie-cloud").as_deref(), Some(new.as_slice()), "the cloud's beside it");
    assert_eq!(disk("fresh").as_deref(), Some(new.as_slice()));
    assert_eq!(disk("repair").as_deref(), Some(old.as_slice()), "a damaged cloud copy is never taken");
    assert_eq!(disk("stray"), None);

    let files = steam.cloud(FIRST_USER);
    let up_there = |slot: &str| files[&format!("saves/world/{slot}.save")].0.clone();
    assert_eq!(up_there("push"), new, "this machine's newer one went up");
    assert_eq!(up_there("tie"), old, "after a tie the cloud has this machine's, so the next start finds them the same");
    assert_eq!(up_there("only-here"), old, "a save only here goes up");
    assert_eq!(up_there("repair"), old, "a good save here repairs the cloud's");
    assert_eq!(up_there("same"), new);

    let c = cloud(app.world());
    assert_eq!(c.pulled, ["world/fresh", "world/pull"]);
    assert_eq!(c.kept_both, ["world/tie"]);
    assert_eq!(c.pushed, ["world/push", "world/repair", "world/tie", "world/only-here"]);
    let world = app.world();
    assert!(journal(world, Some(JournalKind::Warn)).iter().any(|l| l.starts_with("steam: world/tie differs between this machine and Steam Cloud")));
    assert!(journaled(world, "steam: world/pull taken from Steam Cloud (newer), this machine's older one kept as world/pull.save.bak1"));
    // The launch came after: it found the slot the cloud brought.
    assert!(!journaled(world, "no world save in that slot yet"), "{:#?}", journal(world, None));
    assert!(!journaled(world, "was not loaded"), "{:#?}", journal(world, None));
    let _ = std::fs::remove_dir_all(&saves);
}

/// `bytes` as a save from a newer version of the game: its format (`shards-save/2`), or one of its
/// sections (a version above this build's).
fn from_a_newer_version(bytes: &[u8], format: bool) -> Vec<u8> {
    if format {
        return String::from_utf8(bytes.to_vec()).unwrap().replacen("shards-save/1", "shards-save/2", 1).into_bytes();
    }
    let mut file = SaveFile::decode(bytes).unwrap();
    let section = file.sections.0.values_mut().next().expect("a world save has sections");
    section.version += 100;
    file.encode()
}

#[test]
fn a_cloud_copy_from_a_newer_version_is_neither_taken_nor_overwritten() {
    let (old, new) = two_world_saves();
    let saves = save_folder();
    let put = |slot: &str, bytes: &[u8]| {
        let file = path(&saves, SaveKind::World, slot, Which::Save);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, bytes).unwrap();
    };
    put("format", &old);
    put("section", &old);
    // This machine's is the newer version's: the cloud's (newer by time) is not written over it.
    put("mine-newer", &from_a_newer_version(&old, false));

    let steam = FakeSteam::new(MemoryHub::new(1));
    let user = steam.user(FIRST_USER, "player");
    let t = now();
    let up = |slot: &str, bytes: &[u8]| steam.put_cloud(FIRST_USER, &format!("saves/world/{slot}.save"), bytes, t + 100);
    let format = from_a_newer_version(&new, true);
    let section = from_a_newer_version(&new, false);
    up("format", &format);
    up("section", &section);
    up("cloud-only", &format);
    up("mine-newer", &new);

    let mut app = game_in(config(|_| {}), user, saves.clone());
    let disk = |slot: &str| std::fs::read(path(&saves, SaveKind::World, slot, Which::Save)).ok();
    assert_eq!(disk("format").as_deref(), Some(old.as_slice()), "not taken");
    assert_eq!(disk("section").as_deref(), Some(old.as_slice()), "not taken");
    assert_eq!(disk("cloud-only"), None);
    assert_eq!(disk("mine-newer").as_deref(), Some(from_a_newer_version(&old, false).as_slice()), "not written over");
    let c = cloud(app.world());
    assert_eq!(c.newer, ["world/cloud-only", "world/format", "world/section"]);
    assert!(c.pulled.is_empty() && c.pushed.is_empty(), "{c:?}");
    let world = app.world();
    let warned = journal(world, Some(JournalKind::Warn));
    let line = "steam: the cloud copy of world/format is from a newer version of the game (save format shards-save/2 > shards-save/1): this build \
                neither takes it nor overwrites it, so the save on this machine stays and is not copied to Steam Cloud; update the game to have both";
    assert!(warned.iter().any(|l| l == line), "{warned:#?}");
    assert!(warned.iter().any(|l| l.starts_with("steam: the cloud copy of world/section is from a newer version of the game (section ")), "{warned:#?}");
    assert!(warned.iter().any(|l| l.starts_with("steam: the cloud copy of world/mine-newer was not taken: world/mine-newer was saved by a newer version")), "{warned:#?}");

    // Saving into those slots here does not reach the cloud: its copies stay the newer version's.
    for slot in ["format", "section"] {
        ok(&mut app, "save_game", json!({"what": "world", "slot": slot}));
    }
    ok(&mut app, "save_game", json!({"what": "world", "slot": "other"}));
    let files = steam.cloud(FIRST_USER);
    assert_eq!(files["saves/world/format.save"].0, format);
    assert_eq!(files["saves/world/section.save"].0, section);
    assert_eq!(files["saves/world/mine-newer.save"].0, new);
    assert!(files.contains_key("saves/world/other.save"));
    assert_eq!(cloud(app.world()).mirrored, 1);
    let said = ok(&mut app, "steam", json!({}));
    assert!(said.contains("at startup from a newer version of the game, left alone: world/cloud-only, world/format, world/section"), "{said}");
    let _ = std::fs::remove_dir_all(&saves);
}

#[test]
fn while_a_replay_plays_the_cloud_is_not_held_against_the_saves() {
    let (old, new) = two_world_saves();
    let (mut recorded, _) = single();
    ok(&mut recorded, "step", json!({"ticks": 10}));
    let recording = shards::ai_core::replay::save(recorded.world());
    drop(recorded);

    let saves = save_folder();
    let file = path(&saves, SaveKind::World, "slot1", Which::Save);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(&file, &old).unwrap();
    std::fs::write(path(&saves, SaveKind::World, "only-here", Which::Save), &old).unwrap();
    let steam = FakeSteam::new(MemoryHub::new(1));
    let user = steam.user(FIRST_USER, "player");
    steam.put_cloud(FIRST_USER, "saves/world/slot1.save", &new, now() + 100);
    steam.put_cloud(FIRST_USER, "saves/world/fresh.save", &new, now() + 100);

    let mut app = build(config(|_| {}), user, saves.clone());
    shards::ai_core::replay::play(app.world_mut(), recording).unwrap();
    shards::testing::wait_until_loaded(&mut app, shards::testing::LOAD_TIMEOUT).unwrap();
    assert_eq!(std::fs::read(&file).unwrap(), old, "the newer cloud copy is not taken");
    assert!(!path(&saves, SaveKind::World, "fresh", Which::Save).exists());
    assert!(!steam.cloud(FIRST_USER).contains_key("saves/world/only-here.save"), "nothing goes up");
    assert_eq!(cloud(app.world()).enabled, Some(Err(shards_steam::cloud::NOT_DURING_PLAYBACK.to_owned())));
    let said = ok(&mut app, "steam", json!({}));
    assert!(said.contains(&format!("cloud: off ({})", shards_steam::cloud::NOT_DURING_PLAYBACK)), "{said}");
    let _ = std::fs::remove_dir_all(&saves);
}

#[test]
fn a_full_cloud_says_how_full_it_is_and_the_save_on_disk_is_fine() {
    let steam = FakeSteam::new(MemoryHub::new(1));
    let user = steam.user(FIRST_USER, "player");
    steam.set_cloud_quota(FIRST_USER, Some(100));
    let saves = save_folder();
    let mut app = game_in(config(|_| {}), user, saves.clone());
    let said = ok(&mut app, "steam", json!({}));
    assert!(said.ends_with("cloud: on · 0 saves copied as they were written · 100 B free of 100 B"), "{said}");

    ok(&mut app, "save_game", json!({"what": "world", "slot": "slot1"}));
    let size = std::fs::read(path(&saves, SaveKind::World, "slot1", Which::Save)).unwrap().len() as u64;
    assert!(size > 100, "the tests' world save is larger than the quota: {size} B");
    assert!(steam.cloud(FIRST_USER).is_empty());
    let why = format!("Steam Cloud has 100 B free of 100 B; this save is {}", shards::save::ops::size_text(size));
    let line = format!("steam: the cloud copy of world/slot1 was not written: {why}; the save on this machine is fine");
    assert!(journal(app.world(), Some(JournalKind::Warn)).contains(&line), "{:#?}", journal(app.world(), None));
    let said = ok(&mut app, "steam", json!({}));
    assert!(said.ends_with(&format!("the last copy failed: saves/world/slot1.save: {why}")), "{said}");
}
