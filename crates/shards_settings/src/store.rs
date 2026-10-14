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

use std::path::PathBuf;
use std::time::Duration;

use bevy::prelude::*;
use bevy::settings::{SaveSettingsDeferred, SaveSettingsSync, SettingsPlugin};
use bevy::tasks::IoTaskPool;
use shards_core::{EngineConfig, SettingsStore};

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
/// An unreadable file is a warning in the journal and the defaults; the next save replaces it.
pub fn install(app: &mut App, config: &EngineConfig) -> Result<Installed, String> {
    // What the game built the engine with (the title, a changed `headless`) over what the command line said.
    let config = app.world().get_resource::<EngineConfig>().unwrap_or(config).clone();
    let name = match &config.settings {
        SettingsStore::Off => return Ok(Installed::No),
        SettingsStore::Default if config.headless || config.testing || config.embedded => return Ok(Installed::No),
        SettingsStore::Default => match app.world().get_resource::<SettingsAppId>() {
            Some(id) => id.0.clone(),
            None => SettingsAppId::from_title(&config.title).0,
        },
        // An absolute app name replaces the base folder when Bevy joins them: the folder is this one.
        SettingsStore::Dir(dir) => {
            let absolute = std::path::absolute(dir).map_err(|e| format!("--settings-dir {dir}: {e}"))?;
            absolute.to_str().ok_or_else(|| format!("--settings-dir {dir}: not valid UTF-8"))?.to_owned()
        }
    };
    if app.world().contains_resource::<SettingsStorage>() {
        return Err("the settings store was started already".to_owned());
    }
    let file = match &config.settings {
        SettingsStore::Dir(_) => Some(PathBuf::from(&name)),
        _ => bevy::platform::dirs::preferences_dir().map(|base| base.join(&name)),
    }
    .map(|folder| folder.join("settings.toml"));
    // A missing file is a warning in Bevy's log (and so in the journal); a first run is not
    // news. An empty file reads as defaults, and nothing is written while nothing changes.
    if let Some(file) = &file
        && !file.exists()
        && let Some(folder) = file.parent()
        && std::fs::create_dir_all(folder).is_ok()
    {
        let _ = std::fs::OpenOptions::new().create_new(true).write(true).open(file);
    }
    // Saving asks for the IO pool; apps without Bevy's task plugin have none yet.
    IoTaskPool::get_or_init(Default::default);
    app.add_plugins(SettingsPlugin::new(&name)).insert_resource(SettingsStorage { file: file.clone() });
    Ok(Installed::At(file))
}

/// Writes the settings in a second, after the last change (Bevy debounces: a slider dragged
/// for a while is one write). Without a store it does nothing.
pub fn save_soon(commands: &mut Commands) {
    commands.queue(save_soon_in);
}

/// [`save_soon`] for code that has the world.
pub fn save_soon_in(world: &mut World) {
    if world.contains_resource::<SettingsStorage>() {
        SaveSettingsDeferred(Duration::from_secs(1)).apply_to(world);
    }
}

/// Writes the settings that changed since the last write, now. Without a store it does
/// nothing. The exit hook calls it; tests call it to see the file.
pub fn flush(world: &mut World) {
    if world.contains_resource::<SettingsStorage>() {
        SaveSettingsSync::IfChanged.apply_to(world);
    }
}

/// `Command::apply` without importing the trait at every call.
trait ApplyTo {
    fn apply_to(self, world: &mut World);
}

impl<C: bevy::ecs::system::Command> ApplyTo for C {
    fn apply_to(self, world: &mut World) {
        let _ = self.apply(world);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_folder_names() {
        assert_eq!(SettingsAppId::from_title("My game").0, "shards-my-game");
        assert_eq!(SettingsAppId::from_title("Shards").0, "shards");
        assert_eq!(SettingsAppId::from_title("Shards sandbox · seed 42 · BRP :9099").0, "shards-sandbox-seed-42-brp-9099");
        assert_eq!(SettingsAppId::from_title("···").0, "shards-game");
        assert_eq!(SettingsAppId::new("My Game!").0, "my-game");
    }
}
