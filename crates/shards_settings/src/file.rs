//! The settings file as a file: reading it safely, keeping a copy of the last good one, moving
//! a broken one aside, and noticing when a write went wrong.
//!
//! Bevy's settings framework writes `settings.toml.new` and renames it over `settings.toml`,
//! and when the write fails (a full disk, a quota) it logs the error and renames all the same:
//! an empty or half-written file takes the place of the good one, and Bevy never tries again.
//! The framework cannot be asked whether it succeeded, so this module looks at the file
//! afterwards ([`check_store`]): after a write the store asked for or ran, a file that changed
//! and no longer parses, or an empty one (Bevy never writes less than one table per group), is a
//! failed write. The last good copy (`settings.toml.bak`, refreshed after every write that
//! checks out) goes back in its place, the journal says so, and the write is tried again a few
//! times, a pause apart. While a write has failed and none has checked out since, the exit hook
//! writes everything once more; it looks at the file twice, and the second look writes nothing,
//! so a disk that stays full leaves the good copy and not an empty file.
//!
//! A change of the file nobody asked for is the player's own edit, and is none of a write's
//! business: a valid one only refreshes the copy, one that does not read is kept aside as
//! `settings.toml.bad`, as at the start.

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

/// How long a requested write may leave the file as it was before it counts as failed, and how
/// long a write that left a bad file waits before it is tried again.
const RETRY_AFTER: Duration = Duration::from_secs(3);

/// How many times a failed write is tried again before the journal says it gives up.
const MAX_RETRIES: u8 = 3;

/// The player's file as a TOML table: a regular file (a link to one is read through, as Bevy
/// reads it), small, UTF-8, valid TOML.
pub(crate) fn read_table(file: &Path) -> Result<toml::Table, String> {
    read_with(file, fs::metadata(file))
}

/// What a write left, read the same way except that a link in its place is not a regular file:
/// it means Bevy renamed its temporary file over the settings, and that file was a link to
/// somewhere a write cannot succeed (`/dev/full`).
fn read_written_table(file: &Path) -> Result<toml::Table, String> {
    read_with(file, fs::symlink_metadata(file))
}

fn read_with(file: &Path, meta: std::io::Result<fs::Metadata>) -> Result<toml::Table, String> {
    let meta = meta.map_err(|e| format!("cannot read it: {e}"))?;
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

/// A copy of `file` as it is, under a `.bad` name: the one that holds these bytes already, if
/// there is one (a start that finds the same entries unusable again does not add a copy each
/// time), else a new one. `None` when the file cannot be read or copied.
pub(crate) fn bad_copy(file: &Path) -> Option<PathBuf> {
    let bytes = fs::read(file).ok()?;
    let names = std::iter::once(file.with_extension("toml.bad")).chain((1..).map(|n| file.with_extension(format!("toml.bad.{n}"))));
    for name in names {
        if !name.exists() {
            break;
        }
        if fs::read(&name).is_ok_and(|held| held == bytes) {
            return Some(name);
        }
    }
    let name = unique_bad_path(file);
    fs::write(&name, &bytes).ok().map(|()| name)
}

/// Keeps a copy of a file that is good. The copy is written beside the old one and renamed over it
/// only once it is whole, so the old copy stays if the new one cannot be written (a full disk).
pub(crate) fn refresh_backup(file: &Path) {
    let temp = file.with_extension("toml.bak.new");
    if fs::copy(file, &temp).and_then(|_| fs::rename(&temp, backup_path(file))).is_err() {
        let _ = fs::remove_file(&temp);
    }
}

/// Whether the file has nothing but whitespace in it: what a write that went wrong leaves. A file
/// whose lines are all commented out has text, and is the player's way to ask for the defaults.
pub(crate) fn is_blank(file: &Path) -> bool {
    fs::read_to_string(file).is_ok_and(|text| text.trim().is_empty())
}

/// Whether there is a last good copy of `file` and it holds settings.
pub(crate) fn has_backup(file: &Path) -> bool {
    read_written_table(&backup_path(file)).is_ok_and(|t| !t.is_empty())
}

/// Puts the last good copy in place of `file`, if there is one: written beside it and renamed over
/// it (over a symlink, too: the link goes, not what it points at).
pub(crate) fn restore_backup(file: &Path) -> bool {
    if !has_backup(file) {
        return false;
    }
    let backup = backup_path(file);
    let temp = file.with_extension("toml.restore");
    fs::copy(&backup, &temp).and_then(|_| fs::rename(&temp, file)).is_ok()
}

/// What a file looked like: when it was last written and how long it is.
type FileState = Option<(SystemTime, u64)>;

fn file_state(file: &Path) -> FileState {
    fs::symlink_metadata(file).ok().map(|m| (m.modified().unwrap_or(SystemTime::UNIX_EPOCH), m.len()))
}

/// What the store knows about its file: how it last looked, and what has been asked of it.
#[derive(Resource)]
pub(crate) struct StoreHealth {
    file: PathBuf,
    seen: FileState,
    /// When (real time) a write was asked for and the file has not changed since.
    requested: Option<Duration>,
    /// A write was run just now (the exit hook's): the next change of the file is its result.
    wrote: bool,
    /// When (real time) a write that left a bad file was put right: it is tried again
    /// [`RETRY_AFTER`] later, not in the same frame (a disk that was full a moment ago still is).
    retry_at: Option<Duration>,
    attempts: u8,
    /// A write failed or was given up on and none has checked out since: not everything in
    /// memory is on disk, whatever Bevy's marks of what changed say.
    unsaved: bool,
}

impl StoreHealth {
    pub(crate) fn new(file: PathBuf) -> Self {
        let seen = file_state(&file);
        Self { file, seen, requested: None, wrote: false, retry_at: None, attempts: 0, unsaved: false }
    }
}

fn real_time(world: &World) -> Duration {
    world.get_resource::<Time<Real>>().map_or(Duration::ZERO, |t| t.elapsed())
}

/// A write was asked for just now: the file should change within a few seconds.
pub(crate) fn note_request(world: &mut World) {
    let now = real_time(world);
    if let Some(mut health) = world.get_resource_mut::<StoreHealth>() {
        // A request after the store gave up starts the tries over. One that comes while a try is
        // waiting for its turn is that try: the write it asked for is the retry.
        if health.requested.is_none() && health.retry_at.is_none() {
            health.attempts = 0;
        }
        health.retry_at = None;
        health.requested = Some(now);
    }
}

/// Whether a write failed and none has checked out since.
pub(crate) fn unsaved(world: &World) -> bool {
    world.get_resource::<StoreHealth>().is_some_and(|h| h.unsaved)
}

/// The caller is about to write now (the exit hook): the file changing is that write's result.
pub(crate) fn begin_write(world: &mut World) {
    if let Some(mut health) = world.get_resource_mut::<StoreHealth>() {
        health.wrote = true;
    }
}

/// The write begun with [`begin_write`] has been looked at.
pub(crate) fn end_write(world: &mut World) {
    if let Some(mut health) = world.get_resource_mut::<StoreHealth>() {
        health.wrote = false;
    }
}

/// `Last`: after a save, the file is what a save leaves, or the last good copy is back and the
/// write is tried again. Cheap when nothing happened: one `stat`.
pub fn watch_store(world: &mut World) {
    check_store(world, Look::Frame);
}

/// When the file is looked at.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Look {
    /// A frame of the running game: a write asked for gets its time, a retry waits its turn.
    Frame,
    /// The game is ending: a write asked for and not seen counts as failed at once, and a retry
    /// does not wait, there is no later frame.
    Exit,
    /// The second look of the exit, at what a retry of the first one made: no waiting, and a file
    /// that did not change is not judged (the first look did). The last one: a bad file is put right
    /// from the last good copy and nothing is written after it.
    Again,
}

/// Looks at the file: a change nobody asked for is the player's edit, the result of a write that
/// was asked for or run is judged (good: the copy is refreshed; bad: the last good one goes back
/// and the write is tried again), and a write asked for that left the file as it was is told after
/// [`RETRY_AFTER`].
pub(crate) fn check_store(world: &mut World, look: Look) {
    let hurry = look != Look::Frame;
    let at_once = look == Look::Exit;
    let Some(health) = world.get_resource::<StoreHealth>() else { return };
    let (file, seen, requested, retry_at, wrote) = (health.file.clone(), health.seen, health.requested, health.retry_at, health.wrote);
    let now = real_time(world);
    let state = file_state(&file);
    let why = if state != seen {
        if requested.is_none() && !wrote {
            // No write was asked for or run, so this is no write's result: the player edited the file.
            outside_change(world, &file, state);
            return;
        }
        match read_written_table(&file) {
            Ok(table) if !table.is_empty() => {
                refresh_backup(&file);
                let mut health = world.resource_mut::<StoreHealth>();
                health.seen = file_state(&file);
                (health.requested, health.retry_at, health.wrote, health.attempts, health.unsaved) = (None, None, false, 0, false);
                return;
            }
            Ok(_) => "the file came out empty".to_owned(),
            Err(why) => format!("the file came out wrong: {why}"),
        }
    } else if retry_at.is_some_and(|at| hurry || now.saturating_sub(at) >= RETRY_AFTER) {
        // The good file is back since the last failure; this puts the change on it. Whether it
        // worked is seen the usual way, and told in the journal if it did not.
        {
            let mut health = world.resource_mut::<StoreHealth>();
            health.retry_at = None;
            health.requested = Some(now);
        }
        SaveSettingsSync::Always.apply_to(world);
        if look == Look::Frame {
            // Looked at now, not a frame later: a bad file does not lie there for a frame.
            check_store(world, Look::Frame);
        }
        return;
    } else if requested.is_some_and(|at| at_once || now.saturating_sub(at) >= RETRY_AFTER) {
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
        if look == Look::Exit && file_state(&file) != state {
            // No later frame: a bad file is put right now, and tried once more, while the exit can.
            check_store(world, Look::Exit);
        }
        return;
    }

    // A failed write. If it left a bad file, the last good one goes back.
    let restored = damaged && restore_backup(&file);
    let attempts = {
        let mut health = world.resource_mut::<StoreHealth>();
        health.seen = file_state(&file);
        health.attempts += 1;
        health.unsaved = true;
        (health.requested, health.wrote) = (None, false);
        health.attempts
    };
    let kept = if restored { format!("; the last good copy ({}) is back", backup_path(&file).display()) } else { String::new() };
    if attempts > MAX_RETRIES {
        world.resource_mut::<StoreHealth>().retry_at = None;
        journal::emit(
            world,
            JournalKind::Error,
            format!(
                "settings: giving up writing {} ({why}); changes stay in memory, and are written at the next change or when the game exits, \
                 if the write works then{kept}",
                file.display()
            ),
        );
        return;
    }
    // The last look of the exit judges what the one before it wrote and writes nothing: a write
    // that failed again must not be the last thing done to the file, the good copy is.
    let last = look == Look::Again;
    let next = if last { "the game is closing, no more tries".to_owned() } else { format!("trying again ({attempts}/{MAX_RETRIES})") };
    journal::emit(world, JournalKind::Error, format!("settings: writing {} failed ({why}){kept}; {next}", file.display()));
    if last {
        return;
    }
    if damaged && !hurry {
        // A disk that was full a moment ago still is: the good file stays as it is for a while.
        world.resource_mut::<StoreHealth>().retry_at = Some(now);
    } else {
        // `Always`: Bevy has moved its "last saved" mark on whether or not the write worked, so
        // `IfChanged` would find nothing to do.
        SaveSettingsSync::Always.apply_to(world);
        world.resource_mut::<StoreHealth>().requested = Some(now);
    }
}

/// The file changed and no write of the store's is the reason: the player edited it while the game
/// ran. Whatever it is now is the file as it stands; a valid one only refreshes the last good copy
/// (nothing is written over it: a run that changed nothing writes nothing), one that is empty or
/// gone is left (an editor that truncates before it writes would otherwise be undone half way),
/// and one that does not read is kept aside as at the start, with the last good copy in its place.
fn outside_change(world: &mut World, file: &Path, state: FileState) {
    world.resource_mut::<StoreHealth>().seen = state;
    if state.is_none() {
        return;
    }
    let why = match read_table(file) {
        Ok(table) => {
            if !table.is_empty() {
                refresh_backup(file);
            }
            return;
        }
        Err(why) => why,
    };
    let bad = unique_bad_path(file);
    let said = match fs::rename(file, &bad) {
        Err(e) => {
            format!("settings: {} was edited and does not read ({why}), and could not be moved aside ({e}); the next save replaces it", file.display())
        }
        Ok(()) => {
            let restored = restore_backup(file);
            world.resource_mut::<StoreHealth>().seen = file_state(file);
            let from = if restored { format!("the last good copy ({}) is back", backup_path(file).display()) } else { "the settings in memory stay".to_owned() };
            format!("settings: {} was edited while the game ran and does not read ({why}); kept as {}; {from}", file.display(), bad.display())
        }
    };
    journal::emit(world, JournalKind::Warn, said);
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

    #[cfg(unix)]
    #[test]
    fn a_link_to_a_file_is_read_through_but_what_a_write_left_must_be_a_file() {
        let dir = temp("link");
        let real = dir.join("dotfiles.toml");
        fs::write(&real, "[audio]\nmaster = 0.5\n").unwrap();
        let link = dir.join("settings.toml");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(read_table(&link).unwrap().len(), 1, "the player's file: read through the link");
        assert!(read_written_table(&link).unwrap_err().contains("not a regular file"), "a write's result: a link is Bevy's temporary file renamed over it");
        // Links to what is not a regular file are still refused, either way.
        let device = dir.join("device.toml");
        std::os::unix::fs::symlink("/dev/null", &device).unwrap();
        assert!(read_table(&device).unwrap_err().contains("not a regular file"));
        let folder = dir.join("folder.toml");
        std::os::unix::fs::symlink(&dir, &folder).unwrap();
        assert!(read_table(&folder).unwrap_err().contains("not a regular file"));
        let dangling = dir.join("dangling.toml");
        std::os::unix::fs::symlink(dir.join("nowhere"), &dangling).unwrap();
        assert!(read_table(&dangling).unwrap_err().contains("cannot read"));
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
    fn a_copy_is_made_once_for_the_same_bytes() {
        let dir = temp("copy");
        let file = dir.join("settings.toml");
        assert_eq!(bad_copy(&file), None, "no file, no copy");
        fs::write(&file, "one").unwrap();
        assert_eq!(bad_copy(&file), Some(dir.join("settings.toml.bad")));
        assert_eq!(bad_copy(&file), Some(dir.join("settings.toml.bad")), "the same text is the same copy");
        assert!(!dir.join("settings.toml.bad.1").exists());
        fs::write(&file, "two").unwrap();
        assert_eq!(bad_copy(&file), Some(dir.join("settings.toml.bad.1")), "other text is kept too");
        assert_eq!(bad_copy(&file), Some(dir.join("settings.toml.bad.1")));
        fs::write(&file, "one").unwrap();
        assert_eq!(bad_copy(&file), Some(dir.join("settings.toml.bad")), "an older text finds its copy");
        assert_eq!(fs::read_to_string(dir.join("settings.toml.bad.1")).unwrap(), "two");
        let _ = fs::remove_dir_all(&dir);
    }

    /// Bevy's plugin on `dir` with one group, and the store's health on its file: the store of a
    /// game without the rest of the engine, so a test can call the looks one by one.
    #[cfg(target_os = "linux")]
    fn store_in(dir: &Path) -> App {
        let mut app = App::new();
        app.init_resource::<ai_core::AiJournal>().register_type::<crate::AudioSettings>().init_resource::<crate::AudioSettings>();
        app.add_plugins(bevy::settings::SettingsPlugin::new(dir.to_str().unwrap()));
        app.insert_resource(StoreHealth::new(dir.join("settings.toml")));
        app
    }

    /// The next write of Bevy's fails and its rename puts the half-written file in place.
    #[cfg(target_os = "linux")]
    fn fill_the_disk(dir: &Path) {
        let _ = std::os::unix::fs::symlink("/dev/full", dir.join("settings.toml.new"));
    }

    /// The exit looks at the file twice, and the second look is the last: on a disk that stays full
    /// it puts the good copy back and does not write over it, whatever it finds.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_last_look_of_the_exit_leaves_the_good_copy_on_a_disk_that_stays_full() {
        let dir = temp("last_look");
        let (file, backup) = (dir.join("settings.toml"), backup_path(&dir.join("settings.toml")));
        let mut app = store_in(&dir);
        let world = app.world_mut();
        let write = |world: &mut World, master: f64| {
            world.resource_mut::<crate::AudioSettings>().master = master;
            begin_write(world);
            SaveSettingsSync::Always.apply_to(world);
        };
        write(world, 0.4);
        check_store(world, Look::Exit);
        assert!(fs::read_to_string(&backup).unwrap().contains("master = 0.4"), "a write that checked out is the last good copy");

        // The write of the exit failed and so did its retry: the disk is still full.
        fill_the_disk(&dir);
        write(world, 0.3);
        fill_the_disk(&dir);
        check_store(world, Look::Again);
        assert!(fs::symlink_metadata(&file).unwrap().file_type().is_file(), "no link, no partial file");
        assert_eq!(fs::read_to_string(&file).unwrap(), fs::read_to_string(&backup).unwrap(), "the good copy is what is left");
        assert!(fs::read_to_string(&file).unwrap().contains("master = 0.4"));
        assert!(world.resource::<StoreHealth>().unsaved, "the change is still in memory only");
        let journal = world.resource::<ai_core::AiJournal>();
        let said: Vec<_> = journal.after(0).0.filter(|e| e.kind == JournalKind::Error).map(|e| e.text.clone()).collect();
        assert!(said.len() == 1 && said[0].contains("no more tries") && !said[0].contains("trying again"), "{said:?}");
    }

    /// A copy that cannot be written whole does not take the place of the one that is: the old
    /// `.bak` stays good and the half-written file is not left beside it.
    #[cfg(target_os = "linux")]
    #[test]
    fn a_copy_that_cannot_be_written_leaves_the_old_one() {
        let dir = temp("refresh");
        let file = dir.join("settings.toml");
        let (backup, temp_copy) = (backup_path(&file), dir.join("settings.toml.bak.new"));
        fs::write(&file, "[audio]\nmaster = 0.4\n").unwrap();
        refresh_backup(&file);
        assert!(fs::read_to_string(&backup).unwrap().contains("master = 0.4"));

        // The disk is full: the copy of the new file fails in the middle of its write.
        fs::write(&file, "[audio]\nmaster = 0.3\n").unwrap();
        std::os::unix::fs::symlink("/dev/full", &temp_copy).unwrap();
        refresh_backup(&file);
        assert!(fs::read_to_string(&backup).unwrap().contains("master = 0.4"), "the old copy is what is left");
        assert!(has_backup(&file), "and it still holds settings");
        assert!(fs::symlink_metadata(&temp_copy).is_err(), "the temporary file is gone");

        refresh_backup(&file);
        assert!(fs::read_to_string(&backup).unwrap().contains("master = 0.3"), "a copy that is written takes its place");
        assert!(fs::symlink_metadata(&temp_copy).is_err());
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
