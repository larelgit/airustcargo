//! Saves in Steam Cloud: each save written is copied up (never a backup or a temporary file); at
//! startup, before the launch loads a slot, the cloud's copies are held against this machine's.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;
use shards::ai_core::journal::JournalKind;
use shards::net::MemoryHub;
use shards::prelude::*;
use shards::save::{SaveConfig, SaveKind, Which};
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
