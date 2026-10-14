//! The settings file as a file: reading it safely, keeping a copy of the last good one, moving
//! a broken one aside, and noticing when a write went wrong.
//!
//! Bevy's settings framework writes `settings.toml.new` and renames it over `settings.toml`,
//! and when the write fails (a full disk, a quota) it logs the error and renames all the same:
//! an empty or half-written file takes the place of the good one, and Bevy never tries again.
//! The framework cannot be asked whether it succeeded, so this module looks at the file
//! afterwards ([`check_store`]): a file that changed and no longer parses, or an empty one
//! (Bevy never writes less than one table per group), is a failed write. The last good copy
//! (`settings.toml.bak`, refreshed after every write that checks out) goes back in its place,
//! the journal says so, and the write is tried again a few times.

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use ai_core::{journal, JournalKind};
use bevy::prelude::*;
use bevy::settings::SaveSettingsSync;

use crate::store::SettingsStorage;

/// A settings file bigger than this is not one.
const MAX_BYTES: u64 = 4 << 20;

/// How long a requested write may leave the file as it was before it counts as failed.
const RETRY_AFTER: Duration = Duration::from_secs(3);

/// How many times a failed write is tried again before the journal says it gives up.
const MAX_RETRIES: u8 = 3;

/// The file as a TOML table: a regular file, small, UTF-8, valid TOML.
pub(crate) fn read_table(file: &Path) -> Result<toml::Table, String> {
    let meta = fs::symlink_metadata(file).map_err(|e| format!("cannot read it: {e}"))?;
    if !meta.file_type().is_file() {
        return Err("it is not a regular file".to_owned());
    }
    if meta.len() > MAX_BYTES {
        return Err(format!("it is {} bytes, too big for settings", meta.len()));
    }
    let mut text = String::new();
    fs::File::open(file).and_then(|f| f.take(MAX_BYTES).read_to_string(&mut text)).map_err(|e| format!("cannot read it: {e}"))?;
    toml::from_str::<toml::Table>(&text).map_err(|e| format!("it is not valid TOML: {}", e.message()))
}

/// `settings.toml.bak`: the last copy that checked out.
pub(crate) fn backup_path(file: &Path) -> PathBuf {
    file.with_extension("toml.bak")
}

/// A name for a broken file that nothing uses yet: `settings.toml.bad`, `.bad.1`, `.bad.2`, ...
pub(crate) fn unique_bad_path(file: &Path) -> PathBuf {
    let first = file.with_extension("toml.bad");
    if !first.exists() {
        return first;
    }
    (1..).map(|n| file.with_extension(format!("toml.bad.{n}"))).find(|p| !p.exists()).unwrap_or(first)
}

/// Keeps a copy of a file that is good.
pub(crate) fn refresh_backup(file: &Path) {
    let _ = fs::copy(file, backup_path(file));
}

/// Puts the last good copy in place of `file`, if there is one: written beside it and renamed over
/// it (over a symlink, too: the link goes, not what it points at).
pub(crate) fn restore_backup(file: &Path) -> bool {
    let backup = backup_path(file);
    if !read_table(&backup).is_ok_and(|t| !t.is_empty()) {
        return false;
    }
    let temp = file.with_extension("toml.restore");
    fs::copy(&backup, &temp).and_then(|_| fs::rename(&temp, file)).is_ok()
}

/// What a file looked like: when it was last written and how long it is.
type FileState = Option<(SystemTime, u64)>;

fn file_state(file: &Path) -> FileState {
    fs::symlink_metadata(file).ok().map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()))
}

/// What the store knows about its file: how it last looked, and whether a write was asked for.
#[derive(Resource)]
pub(crate) struct StoreHealth {
    file: PathBuf,
    seen: FileState,
    /// When (real time) a write was asked for and the file has not changed since.
    requested: Option<Duration>,
    attempts: u8,
}

impl StoreHealth {
    pub(crate) fn new(file: PathBuf) -> Self {
        let seen = file_state(&file);
        Self { file, seen, requested: None, attempts: 0 }
    }
}

fn real_time(world: &World) -> Duration {
    world.get_resource::<Time<Real>>().map_or(Duration::ZERO, |t| t.elapsed())
}

/// A write was asked for just now: the file should change within a few seconds.
pub(crate) fn note_request(world: &mut World) {
    let now = real_time(world);
    if let Some(mut health) = world.get_resource_mut::<StoreHealth>() {
        // A request after the store gave up starts the tries over.
        if health.requested.is_none() {
            health.attempts = 0;
        }
        health.requested = Some(now);
    }
}

/// `Last`: after a save, the file is what a save leaves, or the last good copy is back and the
/// write is tried again. Cheap when nothing happened: one `stat`.
pub fn watch_store(world: &mut World) {
    check_store(world, false);
}

/// `finishing`: the game is ending, so a write asked for and not seen counts as failed at once.
pub(crate) fn check_store(world: &mut World, finishing: bool) {
    let Some(health) = world.get_resource::<StoreHealth>() else { return };
    let (file, seen, requested) = (health.file.clone(), health.seen, health.requested);
    let now = real_time(world);
    let state = file_state(&file);
    let why = if state != seen {
        match read_table(&file) {
            Ok(table) if !table.is_empty() => {
                refresh_backup(&file);
                let mut health = world.resource_mut::<StoreHealth>();
                (health.seen, health.requested, health.attempts) = (file_state(&file), None, 0);
                return;
            }
            Ok(_) => "the file came out empty".to_owned(),
            Err(why) => format!("the file came out wrong: {why}"),
        }
    } else if requested.is_some_and(|at| finishing || now.saturating_sub(at) >= RETRY_AFTER) {
        "the file was not written".to_owned()
    } else {
        return;
    };

    // The file never changed, though a write was asked for. Bevy decides by change ticks whether
    // there is anything to write, and a change made right after a write can be missed: so the
    // first time, write anyway and say nothing. Only a file that still does not change is a failure.
    let damaged = state != seen;
    if !damaged && world.resource::<StoreHealth>().attempts == 0 {
        {
            let mut health = world.resource_mut::<StoreHealth>();
            health.attempts = 1;
            health.requested = Some(now);
        }
        SaveSettingsSync::Always.apply_to(world);
        return;
    }

    // A failed write. If it left a bad file, the last good one goes back.
    let restored = damaged && restore_backup(&file);
    let attempts = {
        let mut health = world.resource_mut::<StoreHealth>();
        health.seen = file_state(&file);
        health.attempts += 1;
        health.attempts
    };
    let kept = if restored { format!("; the last good copy ({}) is back", backup_path(&file).display()) } else { String::new() };
    if attempts > MAX_RETRIES {
        world.resource_mut::<StoreHealth>().requested = None;
        journal::emit(
            world,
            JournalKind::Error,
            format!("settings: giving up writing {} ({why}); changes stay in memory until a write works{kept}", file.display()),
        );
        return;
    }
    journal::emit(
        world,
        JournalKind::Error,
        format!("settings: writing {} failed ({why}){kept}; trying again ({attempts}/{MAX_RETRIES})", file.display()),
    );
    // `Always`: Bevy has moved its "last saved" mark on whether or not the write worked, so
    // `IfChanged` would find nothing to do.
    SaveSettingsSync::Always.apply_to(world);
    world.resource_mut::<StoreHealth>().requested = Some(now);
}

/// `Command::apply` without importing the trait at every call.
pub(crate) trait ApplyTo {
    fn apply_to(self, world: &mut World);
}

impl<C: bevy::ecs::system::Command> ApplyTo for C {
    fn apply_to(self, world: &mut World) {
        let _ = self.apply(world);
    }
}

/// Entries of table `[group.field]` as the settings file holds them now (what a write left, not
/// what is in memory): `None` when there is no file or no such table.
pub fn stored_entries(world: &World, group: &str, field: &str) -> Option<Vec<String>> {
    let file = world.get_resource::<SettingsStorage>()?.file.clone()?;
    let table = read_table(&file).ok()?;
    Some(table.get(group)?.as_table()?.get(field)?.as_table()?.keys().cloned().collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shards_settings_file_{tag}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_file_reads_as_a_table_or_says_why_not() {
        let dir = temp("read");
        let file = dir.join("settings.toml");
        fs::write(&file, "[audio]\nmaster = 0.5\n").unwrap();
        assert_eq!(read_table(&file).unwrap().len(), 1);
        fs::write(&file, "").unwrap();
        assert!(read_table(&file).unwrap().is_empty());
        fs::write(&file, "[audio\nmaster").unwrap();
        assert!(read_table(&file).unwrap_err().contains("not valid TOML"));
        fs::write(&file, [0xff, 0xfe, 0x00]).unwrap();
        assert!(read_table(&file).is_err(), "not UTF-8");
        assert!(read_table(&dir.join("missing.toml")).unwrap_err().contains("cannot read"));
        assert!(read_table(&dir).unwrap_err().contains("not a regular file"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn broken_files_get_names_that_are_free() {
        let dir = temp("bad");
        let file = dir.join("settings.toml");
        assert_eq!(unique_bad_path(&file), dir.join("settings.toml.bad"));
        fs::write(dir.join("settings.toml.bad"), "x").unwrap();
        assert_eq!(unique_bad_path(&file), dir.join("settings.toml.bad.1"));
        fs::write(dir.join("settings.toml.bad.1"), "x").unwrap();
        assert_eq!(unique_bad_path(&file), dir.join("settings.toml.bad.2"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_last_good_copy_comes_back_only_if_it_is_good() {
        let dir = temp("restore");
        let file = dir.join("settings.toml");
        assert!(!restore_backup(&file), "no copy");
        fs::write(&file, "[audio]\nmaster = 0.5\n").unwrap();
        refresh_backup(&file);
        fs::write(&file, "garbage [").unwrap();
        assert!(restore_backup(&file));
        assert!(fs::read_to_string(&file).unwrap().contains("master = 0.5"));
        fs::write(backup_path(&file), "also garbage [").unwrap();
        assert!(!restore_backup(&file), "a bad copy is not restored");
        let _ = fs::remove_dir_all(&dir);
    }
}
