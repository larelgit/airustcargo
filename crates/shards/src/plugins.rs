//! `EnginePlugins`: the whole engine from one [`EngineConfig`].

use std::time::Duration;

use bevy::app::{PluginGroupBuilder, ScheduleRunnerPlugin, TerminalCtrlCHandlerPlugin};
use bevy::log::LogPlugin;
use bevy::prelude::*;
use bevy::window::ExitCondition;
use shards_action::ActionPlugin;
use shards_content::ContentPlugin;
use shards_core::{CorePlugin, EngineConfig};
use shards_motion::MotionPlugin;
use shards_world::WorldPlugin;

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
        let config = self.config;
        let headless = config.headless || config.testing;
        let assets = AssetPlugin {
            file_path: config.asset_dir.clone(),
            watch_for_changes_override: Some(config.hot_reload && !config.testing && cfg!(feature = "hot_reload")),
            ..default()
        };
        let mut group = DefaultPlugins.build().set(assets);
        group = if config.testing {
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
        group = if headless { without_display(group, config.testing) } else { with_window(group, &config) };
        // The engine's modules, in dependency order. A new module is one more line.
        group
            .add(config.ai_core_plugin())
            .add(CorePlugin { config: config.clone() })
            .add(ContentPlugin)
            .add(MotionPlugin)
            .add(ActionPlugin)
            .add(WorldPlugin)
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

fn without_display(mut group: PluginGroupBuilder, testing: bool) -> PluginGroupBuilder {
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
    let runner = if testing {
        ScheduleRunnerPlugin::run_once()
    } else {
        ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(1.0 / 60.0))
    };
    group.add(runner)
}
