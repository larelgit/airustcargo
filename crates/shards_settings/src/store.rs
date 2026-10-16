//! Where settings live and when they are written.
//!
//! Bevy's settings framework ([`bevy::settings::SettingsPlugin`]) reads `settings.toml` once,
//! when its plugin is added, into the group resources, and writes only when told to
//! (`SaveSettingsDeferred`, `SaveSettingsSync`). Two consequences shape this module:
//!
//! - it must be added **after** the game registered its groups (it scans the type registry
//!   once), so [`install`] is called by `shards::run` once the game's builder returned, not
//!   by [`ShardsSettingsPlugin`](crate::ShardsSettingsPlugin);
//! - a game without it (a test, `--no-settings`, `--verify`, a stand's client) still has every
//!   group, as defaults, and changes to them go nowhere: [`save_soon`] and [`flush`] then do
//!   nothing.

use std::path::{Path, PathBuf};
use std::time::Duration;

use ai_core::{journal, JournalKind};
use bevy::prelude::*;
use bevy::settings::{SaveSettingsDeferred, SaveSettingsSync, SettingsPlugin};
use bevy::tasks::IoTaskPool;
use shards_core::{EngineConfig, SettingsStore};

use crate::file::{self, backup_path, bad_copy, has_backup, is_blank, read_table, refresh_backup, restore_backup, unique_bad_path, ApplyTo, StoreHealth};
use crate::{recover, registry, ControlSettings};

/// The name of this game's settings folder: `<config dir>/<id>/settings.toml`. A game sets it
/// before the engine's run starts the store (`app.insert_resource(SettingsAppId::new("my-game"))`);
/// without one the id comes from the window title (`shards-<title>`).
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct SettingsAppId(pub String);

impl SettingsAppId {
    /// An id as a folder name: lowercase letters, digits and `-`, whatever `name` is.
    pub fn new(name: &str) -> Self {
        Self(slug(name))
    }

    /// The id of a game titled `title`: `shards-<title>`, without doubling a leading `shards`.
    pub fn from_title(title: &str) -> Self {
        let slug = slug(title);
        if slug == "shards" || slug.starts_with("shards-") { Self(slug) } else { Self(format!("shards-{slug}")) }
    }
}

fn slug(text: &str) -> String {
    let mut out = String::new();
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    let trimmed = out.trim_end_matches('-');
    if trimmed.is_empty() { "game".to_owned() } else { trimmed.to_owned() }
}

/// Present when settings are kept on disk: where, so tools and the journal can say it.
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct SettingsStorage {
    /// The settings file. `None` when the platform has no settings folder.
    pub file: Option<PathBuf>,
    /// The platform's own settings folder (no `--settings-dir`): the one place a game's old
    /// files from the same user folder may be looked for. A folder somebody named is theirs alone.
    pub platform_default: bool,
}

/// What [`install`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Installed {
    /// No store: defaults, nothing read, nothing written.
    No,
    /// Settings are read from and written to this file.
    At(Option<PathBuf>),
}

/// Starts the settings store for `app` as `config.settings` says. Call it once, after the
/// game's builder returned (every group is registered by then) and before the app runs.
///
/// - `SettingsStore::Off`: nothing.
/// - `SettingsStore::Default`: the platform's settings folder for [`SettingsAppId`]. A
///   headless game gets no store (an agent's playtest must neither read nor change the
///   player's own settings; `--settings-dir` opts in).
/// - `SettingsStore::Dir`: `<dir>/settings.toml`, the folder made absolute against the
///   working directory.
///
/// What is made of the file before it is read:
/// - a file that is not valid TOML (a typo in a hand edit) is moved to `settings.toml.bad` (a
///   free name: an earlier `.bad` is never overwritten) and the game starts from the last good
///   copy (`settings.toml.bak`), or the defaults, instead of the next save replacing the text;
/// - a file that is a link to a regular file is read through it, as Bevy reads it;
/// - an empty file where a last good copy exists is a write that went wrong: the copy comes back
///   (a file with text but no settings, every line commented out, is the defaults and stays as it is);
/// - a missing file is made empty (an empty file reads as the defaults, and nothing is written
///   while nothing changes), so Bevy does not log "not found" on a first run; a file the player
///   deleted takes its old copy with it;
/// - the controls table is read again entry by entry ([`recover`](crate::recover)): one bad
///   entry no longer costs every rebinding.
///
/// Every number a hand edit made `nan` or `inf` gets its default. What happened is in the
/// journal.
///
/// `--settings-dir` needs Bevy's settings folder to exist on this platform (Bevy joins its
/// folder with the directory, and without one stores nothing): without it the call fails and
/// says how to fix that.
pub fn install(app: &mut App, config: &EngineConfig) -> Result<Installed, String> {
    // The values a game gave its groups while building are what a reset goes back to: take them
    // before the file is laid over them, whatever the store turns out to be.
    registry::capture_defaults(app.world_mut());
    // What the game built the engine with (the title, a changed `headless`) over what the command line said.
    let config = app.world().get_resource::<EngineConfig>().unwrap_or(config).clone();
    let name = match &config.settings {
        SettingsStore::Off => return Ok(Installed::No),
        SettingsStore::Default if config.headless || config.offscreen || config.testing || config.embedded => return Ok(Installed::No),
        SettingsStore::Default => match app.world().get_resource::<SettingsAppId>() {
            Some(id) => id.0.clone(),
            None => SettingsAppId::from_title(&config.title).0,
        },
        // An absolute app name replaces the base folder when Bevy joins them: the folder is this one.
        SettingsStore::Dir(dir) => {
            dir_store_usable(bevy::platform::dirs::preferences_dir().as_deref(), dir)?;
            let absolute = std::path::absolute(dir).map_err(|e| format!("--settings-dir {dir}: {e}"))?;
            absolute.to_str().ok_or_else(|| format!("--settings-dir {dir}: not valid UTF-8"))?.to_owned()
        }
    };
    if app.world().contains_resource::<SettingsStorage>() {
        return Err("the settings store was started already".to_owned());
    }
    let platform_default = matches!(config.settings, SettingsStore::Default);
    let file = match &config.settings {
        SettingsStore::Dir(_) => Some(PathBuf::from(&name)),
        _ => bevy::platform::dirs::preferences_dir().map(|base| base.join(&name)),
    }
    .map(|folder| folder.join("settings.toml"));
    let mut notes = file.as_deref().map(prepare_file).unwrap_or_default();
    // Saving asks for the IO pool; apps without Bevy's task plugin have none yet.
    IoTaskPool::get_or_init(Default::default);
    app.add_plugins(SettingsPlugin::new(&name)).insert_resource(SettingsStorage { file: file.clone(), platform_default });

    let world = app.world_mut();
    notes.extend(registry::sanitize(world));
    if let Some(file) = &file {
        notes.extend(recover_controls(world, file));
        world.insert_resource(StoreHealth::new(file.clone()));
    }
    for note in notes {
        journal::emit(world, JournalKind::Warn, note);
    }
    Ok(Installed::At(file))
}

/// Fails when `--settings-dir DIR` could not keep anything: Bevy's settings store joins the
/// platform's settings folder with the name it is given (an absolute name replaces it, which is
/// how `DIR` is used) but without a settings folder it stores nothing at all.
fn dir_store_usable(preferences_dir: Option<&Path>, dir: &str) -> Result<(), String> {
    match preferences_dir {
        Some(_) => Ok(()),
        None => Err(format!(
            "--settings-dir {dir}: this platform has no settings folder (HOME and XDG_CONFIG_HOME are not set?), and Bevy's \
             settings store then keeps nothing anywhere; set HOME or XDG_CONFIG_HOME, or run with --no-settings"
        )),
    }
}

/// See [`install`]: what is made of the file before Bevy reads it.
fn prepare_file(file: &Path) -> Vec<String> {
    if std::fs::symlink_metadata(file).is_err() {
        // A settings file that is gone is the player's reset: the copy of what they threw away goes
        // with it, or the next empty file would read as a write that failed and bring it back.
        let _ = std::fs::remove_file(backup_path(file));
        if let Some(folder) = file.parent()
            && std::fs::create_dir_all(folder).is_ok()
        {
            let _ = std::fs::OpenOptions::new().create_new(true).write(true).open(file);
        }
        return Vec::new();
    }
    match read_table(file) {
        // Bevy never writes less than one table per group, so a file with nothing in it where there
        // was a good copy is a write that went wrong (a full disk, a process killed half way), not
        // defaults. A file with text but no settings (every line commented out) is the player asking
        // for the defaults, and is left alone, as `outside_change` leaves it while the game runs.
        Ok(table) if table.is_empty() && is_blank(file) => {
            if restore_backup(file) {
                vec![format!("settings: {} is empty; the last good copy ({}) is back", file.display(), backup_path(file).display())]
            } else if has_backup(file) {
                vec![format!(
                    "settings: {} is empty and the last good copy ({}) could not be put back; the copy is kept, and the game starts from the defaults",
                    file.display(),
                    backup_path(file).display()
                )]
            } else {
                Vec::new()
            }
        }
        Ok(table) => {
            if !table.is_empty() {
                refresh_backup(file);
            }
            Vec::new()
        }
        Err(why) => {
            let bad = unique_bad_path(file);
            if let Err(e) = std::fs::rename(file, &bad) {
                return vec![format!("settings: {} did not read ({why}) and could not be moved aside ({e}); the next save replaces it", file.display())];
            }
            let restored = restore_backup(file);
            if !restored {
                let _ = std::fs::OpenOptions::new().create_new(true).write(true).open(file);
            }
            let from = if restored { format!("the last good copy ({}) is back", backup_path(file).display()) } else { "starting from the defaults".to_owned() };
            vec![format!("settings: {} did not read ({why}); kept as {}; {from}", file.display(), bad.display())]
        }
    }
}

/// The controls table, entry by entry, after Bevy read it (see [`recover`](crate::recover)):
/// puts back what is good and keeps a copy of the file when something had to be dropped.
fn recover_controls(world: &mut World, file: &Path) -> Vec<String> {
    let Ok(table) = read_table(file) else { return Vec::new() };
    let Some(controls) = table.get("controls").and_then(toml::Value::as_table) else { return Vec::new() };
    let Some(loaded) = world.get_resource::<ControlSettings>() else { return Vec::new() };
    let mut found = recover::recover(controls, loaded);
    if found.notes.is_empty() && found.bindings.is_empty() && found.modes.is_empty() {
        return Vec::new();
    }
    // One copy per text: while the entry stays in the file (nothing is written until something
    // changes), every start finds it unusable again and must not add another copy.
    if found.dropped
        && let Some(bad) = bad_copy(file)
    {
        found.notes.push(format!("settings: the entries that could not be used are in {} only; the file as it was is kept as {}", file.display(), bad.display()));
    }
    if !found.bindings.is_empty() || !found.modes.is_empty() {
        let mut controls = world.resource_mut::<ControlSettings>();
        controls.bindings.extend(found.bindings);
        controls.hold_or_toggle.extend(found.modes);
    }
    found.notes
}

/// Writes the settings in a second, after the last change (Bevy debounces: a slider dragged
/// for a while is one write). Without a store it does nothing. A change a system makes to a
/// settings resource is written this way without anybody calling this: the engine sees it.
pub fn save_soon(commands: &mut Commands) {
    commands.queue(save_soon_in);
}

/// [`save_soon`] for code that has the world.
pub fn save_soon_in(world: &mut World) {
    if world.contains_resource::<SettingsStorage>() {
        SaveSettingsDeferred(Duration::from_secs(1)).apply_to(world);
        file::note_request(world);
    }
}

/// Writes the settings that changed since the last write, now, and checks what was written
/// (see [`file`](crate::file)): a write that failed is put right from the last good copy.
/// Without a store it does nothing. The exit hook calls it; tests call it to see the file.
pub fn flush(world: &mut World) {
    if world.contains_resource::<SettingsStorage>() {
        // What the player did to the file since the last frame is theirs, not this write's result.
        file::check_store(world, file::Look::Frame);
        // After a write that failed, Bevy has moved its "last saved" mark past the change, so
        // `IfChanged` would find nothing to write: the change is written whatever it says.
        let sync = if file::unsaved(world) { SaveSettingsSync::Always } else { SaveSettingsSync::IfChanged };
        file::begin_write(world);
        sync.apply_to(world);
        file::check_store(world, file::Look::Exit);
        file::check_store(world, file::Look::Again);
        file::end_write(world);
    }
}

/// [`flush`], and says whether the settings file has settings in it afterwards (it is a valid
/// TOML file that holds a table per group). Callers that retire something because it was
/// moved into the settings (the old controls file) wait for this.
pub fn flush_checked(world: &mut World) -> Result<(), String> {
    let Some(storage) = world.get_resource::<SettingsStorage>() else { return Err("this run keeps no settings".to_owned()) };
    let Some(file) = storage.file.clone() else { return Err("the platform has no settings folder".to_owned()) };
    flush(world);
    match read_table(&file) {
        Ok(table) if !table.is_empty() => Ok(()),
        Ok(_) => Err(format!("{} came out empty", file.display())),
        Err(why) => Err(format!("{}: {why}", file.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_settings_dir_needs_a_settings_folder_to_be_kept_in() {
        assert!(dir_store_usable(Some(Path::new("/home/u/.config")), "tmp").is_ok());
        let err = dir_store_usable(None, "tmp/cfg").unwrap_err();
        assert!(err.contains("--settings-dir tmp/cfg") && err.contains("HOME") && err.contains("--no-settings"), "{err}");
    }

    #[test]
    fn ids_are_folder_names() {
        assert_eq!(SettingsAppId::from_title("My game").0, "shards-my-game");
        assert_eq!(SettingsAppId::from_title("Shards").0, "shards");
        assert_eq!(SettingsAppId::from_title("Shards sandbox · seed 42 · BRP :9099").0, "shards-sandbox-seed-42-brp-9099");
        assert_eq!(SettingsAppId::from_title("···").0, "shards-game");
        assert_eq!(SettingsAppId::new("My Game!").0, "my-game");
    }
}
