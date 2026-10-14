//! `EnginePlugins`: the whole engine from one [`EngineConfig`].

use std::path::Path;
use std::time::Duration;

use bevy::app::{PluginGroupBuilder, ScheduleRunnerPlugin, TerminalCtrlCHandlerPlugin};
use bevy::log::LogPlugin;
use bevy::prelude::*;
use bevy::window::ExitCondition;
use shards_action::ActionPlugin;
use shards_content::ContentPlugin;
use shards_core::{CorePlugin, EngineConfig};
use shards_level::LevelPlugin;
use shards_motion::MotionPlugin;
use shards_npc::NpcPlugin;
use shards_save::SavePlugin;
use shards_settings::ShardsSettingsPlugin;
use shards_world::WorldPlugin;

use crate::save_sections::EngineSavePlugin;
use crate::TileLevelsPlugin;

/// Bevy's `DefaultPlugins` (whatever the cargo features compile in), the log capture
/// that feeds `warn!`/`error!` into the journal, the AI layer, the engine core, content
/// and motion (physics, characters, input, camera), all configured from one [`EngineConfig`].
///
/// One plugin set for both modes: headless is the same plugins minus window, GPU and
/// audio (`WindowPlugin` without a window, no winit, `RenderPlugin` without a backend so
/// render plugins stay registered, a 60 Hz run loop instead of the event loop).
/// Transforms, assets and states work the same, so gameplay is the same.
/// [`EngineConfig::testing`] also drops the logger (one per process) and the Ctrl+C
/// handler, and runs no loop: tests drive the app with `update()`.
/// [`EngineConfig::embedded`] does the same for a game beside another in one process, but
/// keeps its BRP port, and its log lines go to its own journal.
///
/// A plugin group: a game can still `.set(...)` or `.disable::<...>()` single plugins.
pub struct EnginePlugins {
    config: EngineConfig,
}

impl EnginePlugins {
    /// The engine as `config` describes it.
    pub fn new(config: EngineConfig) -> Self {
        Self { config }
    }
}

impl PluginGroup for EnginePlugins {
    fn build(self) -> PluginGroupBuilder {
        let mut config = self.config;
        // Resolved once, before anything reads it: the asset plugin, the `EngineConfig`
        // resource that `asset_root` (content loading, `--check-content`) reads, all see it.
        if let Ok(cwd) = std::env::current_dir() {
            config.asset_dir = resolve_asset_path(&config.asset_dir, std::env::var("BEVY_ASSET_ROOT").ok().as_deref(), &cwd);
        }
        // Driven from outside (a test, a stand's driver), beside other apps in the process.
        let driven = config.testing || config.embedded;
        let headless = config.headless || driven;
        let assets = AssetPlugin {
            file_path: config.asset_dir.clone(),
            watch_for_changes_override: Some(config.hot_reload && !driven && cfg!(feature = "hot_reload")),
            ..default()
        };
        let mut group = DefaultPlugins.build().set(assets);
        group = if driven {
            group.disable::<LogPlugin>().disable::<TerminalCtrlCHandlerPlugin>()
        } else {
            let mut filter = bevy::log::DEFAULT_FILTER.to_owned();
            if headless && cfg!(feature = "render") {
                // Render plugins without a render app: Bevy logs one error about the missing
                // sub-app it extracts into. Expected headless, and noise in every journal.
                filter.push_str(",bevy_extract::extract_resource=off");
            }
            group.set(LogPlugin { filter, custom_layer: ai_core::capture_log_layer, ..default() })
        };
        group = if headless { without_display(group, driven) } else { with_window(group, &config) };
        if config.embedded {
            group = group.add(EmbeddedLogCapture);
        }
        // The engine's modules, in dependency order. A new module is one more line.
        group
            .add(config.ai_core_plugin())
            .add(CorePlugin { config: config.clone() })
            .add(ContentPlugin)
            // Player settings: the groups the modules below apply (controls, camera, warnings).
            .add(ShardsSettingsPlugin)
            .add(MotionPlugin)
            .add(ActionPlugin)
            .add(LevelPlugin)
            .add(WorldPlugin)
            // Level files as the geometry of tiles: joins the two above.
            .add(TileLevelsPlugin)
            .add(NpcPlugin)
            // Saves: the registry and tools, then what the engine itself saves.
            .add(SavePlugin)
            .add(EngineSavePlugin)
    }
}

/// The asset folder as the engine hands it to Bevy. Bevy joins it onto `BEVY_ASSET_ROOT`
/// (else the manifest dir, else the executable's folder) and starts a file watcher on the
/// result, which fails for a relative path (`PathNotFound`). So a relative `asset_dir`
/// under a relative root becomes absolute, against `cwd`; `Path::join` with an absolute
/// path keeps it as it is. Anything else is returned unchanged.
fn resolve_asset_path(asset_dir: &str, asset_root: Option<&str>, cwd: &Path) -> String {
    let dir = Path::new(asset_dir);
    match asset_root.map(Path::new) {
        Some(root) if dir.is_relative() && root.is_relative() => cwd.join(root).join(dir).display().to_string(),
        _ => asset_dir.to_owned(),
    }
}

/// An embedded game's log lines: the process's capture layer (installed by the first game's
/// `LogPlugin`) writes them into this game's journal while it is the active one.
struct EmbeddedLogCapture;

impl Plugin for EmbeddedLogCapture {
    fn build(&self, app: &mut App) {
        ai_core::journal::register_capture(app);
    }
}

fn with_window(group: PluginGroupBuilder, config: &EngineConfig) -> PluginGroupBuilder {
    let (width, height) = config.window;
    let window = Window { title: config.title.clone(), resolution: (width, height).into(), ..default() };
    group.set(WindowPlugin { primary_window: Some(window), ..default() })
}

/// What a GPU would have told the image loaders: no compressed formats (Bevy asks for it
/// when `RenderPlugin` runs without a backend).
#[cfg(feature = "render")]
struct NoGpu;

#[cfg(feature = "render")]
impl Plugin for NoGpu {
    fn build(&self, app: &mut App) {
        use bevy::image::{CompressedImageFormatSupport, CompressedImageFormats};
        app.insert_resource(CompressedImageFormatSupport(CompressedImageFormats::NONE));
    }
}

fn without_display(mut group: PluginGroupBuilder, driven: bool) -> PluginGroupBuilder {
    group = group.set(WindowPlugin {
        primary_window: None,
        exit_condition: ExitCondition::DontExit,
        close_when_requested: false,
        ..default()
    });
    #[cfg(feature = "render")]
    {
        use bevy::render::settings::WgpuSettings;
        use bevy::render::RenderPlugin;
        group = group
            .disable::<bevy::winit::WinitPlugin>()
            // Drawing gizmos needs a GPU; gizmos themselves (`GizmoPlugin`) stay.
            .disable::<bevy::gizmos_render::GizmoRenderPlugin>()
            .set(RenderPlugin { render_creation: WgpuSettings { backends: None, ..default() }.into(), ..default() })
            .add_before::<RenderPlugin>(NoGpu);
    }
    #[cfg(feature = "audio")]
    {
        group = group.disable::<bevy::audio::AudioPlugin>();
    }
    #[cfg(feature = "gamepad")]
    {
        group = group.disable::<bevy::gilrs::GilrsPlugin>();
    }
    let runner = if driven {
        ScheduleRunnerPlugin::run_once()
    } else {
        ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(1.0 / 60.0))
    };
    group.add(runner)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_relative_root_makes_the_asset_folder_absolute() {
        let cwd = std::env::current_dir().unwrap();
        let resolved = resolve_asset_path("assets", Some("crates/sandbox"), &cwd);
        assert_eq!(resolved, cwd.join("crates/sandbox/assets").display().to_string());
        // What Bevy does next: it joins its base onto the path, and the absolute one wins.
        assert_eq!(Path::new("crates/sandbox").join(&resolved), Path::new(&resolved));
    }

    #[test]
    fn an_absolute_root_or_no_root_keeps_the_asset_folder() {
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(resolve_asset_path("assets", cwd.to_str(), &cwd), "assets");
        assert_eq!(resolve_asset_path("assets", None, &cwd), "assets");
    }

    #[test]
    fn an_absolute_asset_folder_is_kept_whatever_the_root() {
        let cwd = std::env::current_dir().unwrap();
        let absolute = cwd.join("elsewhere").display().to_string();
        assert_eq!(resolve_asset_path(&absolute, Some("crates/sandbox"), &cwd), absolute);
        assert_eq!(resolve_asset_path(&absolute, None, &cwd), absolute);
    }
}
