//! # shards_ui — screens, widgets and menus (E9)
//!
//! A game's HUD and menus are content: `content/**/*.screen.ron` says what a screen holds and
//! how it behaves, `*.theme.ron` how screens look. The engine keeps the open screens in a
//! stack, moves focus the way a gamepad does, presses buttons and changes values, and tells
//! agents all of it as text, so an agent goes through a menu by the UI tree alone.
//!
//! - **Screens** ([`ScreenDef`]): a layer ([`Layer`]: `Hud` takes no input, `Menu` takes the
//!   controls, `Modal` sits over menus), `pauses` (single player: `PauseState::Paused` while it
//!   is open, so the simulation stops; in co-op the world goes on), `open_at_start` (a HUD),
//!   `main_menu` (opened on entering `EngineState::MainMenu`, closed with all on it when the game
//!   starts), `closable` (whether `back` closes it), a title, where it sits and its widgets.
//! - **Widgets** ([`WidgetDef`]): panels, lists, grids and tabs hold others; text, hints, pictures,
//!   bars, cooldowns, a compass, ally frames and a before → after comparison show values; buttons,
//!   sliders, toggles, choices and bindings take input. A binding shows an action's inputs (source
//!   [`BINDINGS_SOURCE`], which the `shards` crate gives from the input map); confirming it waits
//!   for the next key, mouse or gamepad button ([`capturing`]), and the input it gets (or an
//!   agent's `ui_set`) goes into the `controls` settings, which refuse an input another action
//!   reads, and the widget says which. A widget shows a source's value (`bind:
//!   "you.health"`, sources from [`UiAppExt::add_ui_source`]; the engine gives `you.*` and
//!   `allies`) or a player setting (`setting: "accessibility.text_scale"`), which changing the
//!   widget changes and saves.
//! - **Languages**: a title, label or text that names a message (`@menu-resume`) shows it in the
//!   language played ([`shards_locale`]); when the player switches language, every open screen says
//!   it again in the new one, and a choice's options have labels (`labels`) of their own.
//! - **Buttons** do `close`, `back`, `open <screen>`, `quit`, `tool <name> {json}` (as the agent
//!   that pressed, with its scope), or a game action ([`UiAppExt::add_ui_action`]).
//! - **Focus and navigation** ([`model::navigate`]): `up`/`down`/`next`/`prev` move the focus on
//!   the screen that takes input (rows and grids also left and right), `left`/`right` change a
//!   slider, choice, toggle or tabs, `confirm` presses, `back` closes (a binding that waits for an
//!   input stops waiting instead).
//! - **Agents** read the tree with `ui_read` and in `observe` (a `UI` section while a menu is
//!   open: its widgets, values and focus), and act with `ui_navigate`, `ui_click`, `ui_set`,
//!   `ui_open` and `ui_close`. The journal says `ui: opened pause`, `ui: pause/resume pressed
//!   (close)`, `ui: settings/text_size 1 → 1.25`. Code hears [`UiPressed`], [`UiChanged`],
//!   [`UiFocusMoved`] and [`UiScreenChanged`].
//! - **Full-screen flashes** ([`flash`](mod@flash), kind `flash`): a colour over the whole picture for a
//!   moment (a reality flash, a glitch's pulses), above the HUD and under menus, on real time, as
//!   `accessibility.flashes` allows (`reduced` softer and slower, `off` none). Code calls
//!   [`flash()`] or triggers [`PlayFlash`]; the dev tool `flash_play` plays one.
//! - **This machine's, not the simulation's**: no widget has an `AiId` (opening a menu shifts no
//!   id), screens are outside snapshots and the replay hash, and a host does not send them. The
//!   tools that change them are actions, so a replay presses the same buttons at the same ticks.
//! - **Drawing** (feature `render`, with a window or offscreen): Bevy UI nodes for every widget, the theme's
//!   colours and sizes, the player's text size, the mouse through picking, and every widget
//!   named for screen readers (AccessKit roles, labels, values; focus follows the UI's). Text
//!   that names no font draws with the engine's ([`DEFAULT_FONT`]: Fira Sans, Latin, Greek and
//!   Cyrillic); a theme names its own (`font`). The keyboard and gamepad reach the UI through
//!   the engine's input map (`menu` context), which the `shards` crate joins to it.
//! - **The loading picture** (`EngineConfig::loading_image`, with a window or offscreen): full screen
//!   on every frame drawn while the game loads (`Loading`) or a region loads (`RegionLoading`). A
//!   region change that finishes inside the call that made it draws no frame in between, so there
//!   it is not seen.

pub mod bind;
pub mod def;
pub mod flash;
pub mod model;
#[cfg(feature = "render")]
mod loading;
#[cfg(feature = "render")]
mod render;
pub mod text;
mod tools;

use ai_core::AiAppExt;
use bevy::prelude::*;
use serde_json::{json, Value};
use shards_content::ContentAppExt;
use shards_core::{EngineAppExt, EngineState, PauseState};

pub use bind::{live_value, read_source, viewer, UiActionFn, UiActions, UiPress, UiSourceFn, UiSources};
pub use def::{Anchor, ButtonAction, CompareRow, Layer, Layout, ScreenDef, ThemeDef, WidgetDef};
pub use flash::{flash, FlashDef, PlayFlash, ScreenFlash};
#[cfg(feature = "render")]
pub use loading::LoadingScreen;
pub use model::{
    capturing, focused, top_screen, Ally, BoundInput, BoundInputs, Mark, Nav, UiChanged, UiFocusMoved, UiPressed, UiScreen, UiScreenChanged,
    UiStack, UiValue, UiWidget, WidgetKind, BINDINGS_SOURCE, DEFAULT_PROMPT,
};
pub use tools::{FlashPlayArgs, UiClickArgs, UiCloseArgs, UiNavigateArgs, UiOpenArgs, UiReadArgs, UiSetArgs};

#[cfg(feature = "render")]
pub use render::DEFAULT_FONT;

/// Version of the engine crates (they share one).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The agent the local player's controls call the `ui_*` tools as (keys, gamepad, mouse), so a
/// replay records what a human pressed in menus like any agent's call.
pub const INPUT_AGENT: &str = "input";

/// Mouse buttons a binding widget took on their press. The release that follows would click the
/// widget under the pointer; that click ends here instead. The engine's controls fill it (and empty
/// it of buttons let go a frame ago); drawing takes a button out when its click comes.
#[derive(Resource, Default, Debug, Clone, PartialEq)]
pub struct CapturedClicks(pub Vec<MouseButton>);

/// The types a game module uses most.
pub mod prelude {
    pub use crate::{UiAppExt, UiChanged, UiFocusMoved, UiPress, UiPressed, UiScreenChanged, UiStack, UiValue};
}

/// The UI systems other modules order against.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum UiSystems {
    /// `Update`: edited screens built again, bound values stored for drawing.
    Model,
    /// `PostUpdate`, before Bevy's UI layout: widgets drawn as Bevy UI nodes (with a window or offscreen).
    Draw,
}

/// Sources and game actions, added while building the app.
pub trait UiAppExt {
    /// Source `name` for widgets' `bind` (`you.health`; `name:arg` passes `arg`).
    fn add_ui_source(&mut self, name: &str, source: UiSourceFn) -> &mut Self;

    /// Game action `name` for buttons (`action: "respawn"`).
    fn add_ui_action(&mut self, name: &str, action: UiActionFn) -> &mut Self;
}

impl UiAppExt for App {
    fn add_ui_source(&mut self, name: &str, source: UiSourceFn) -> &mut Self {
        self.world_mut().get_resource_or_init::<UiSources>().insert(name, source);
        self
    }

    fn add_ui_action(&mut self, name: &str, action: UiActionFn) -> &mut Self {
        self.world_mut().get_resource_or_init::<UiActions>().insert(name, action);
        self
    }
}

/// The UI module: screens and the theme as content, the stack, the tools, the `UI` lines of
/// `observe`, and drawing with a window. Needs the engine core and content first;
/// `shards::EnginePlugins` adds it.
pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<ai_core::AiCorePlugin>(),
            "UiPlugin needs AiCorePlugin added first (shards::EnginePlugins adds both)"
        );
        app.add_content::<ScreenDef>()
            .add_content::<ThemeDef>()
            .add_content::<FlashDef>()
            .init_resource::<ScreenFlash>()
            .init_resource::<UiStack>()
            .init_resource::<UiSources>()
            .init_resource::<UiActions>()
            .init_resource::<CapturedClicks>()
            .init_resource::<model::UiPaused>()
            .init_resource::<model::UiTextVersion>()
            .add_engine_module("ui", VERSION)
            .add_ai_status("ui", status)
            .register_ai_observe_section(text::observe_section)
            .add_systems(OnEnter(EngineState::InGame), model::open_start_screens)
            .add_systems(OnEnter(EngineState::MainMenu), model::open_main_menu)
            .add_systems(OnExit(EngineState::MainMenu), model::close_main_menu)
            .add_systems(OnExit(PauseState::Paused), model::close_pausing_screens)
            .configure_sets(Update, UiSystems::Model.after(shards_locale::LocaleSystems::Build))
            .add_systems(Update, (model::rebuild_edited_screens, model::retell_screens, bind::refresh_values).chain().in_set(UiSystems::Model))
            .add_systems(Update, flash::end_flashes.in_set(UiSystems::Model))
            .add_observer(flash::on_play)
            // Another language: open screens say it again before the call that switched returns.
            .add_observer(|_: On<shards_locale::TextChanged>, mut commands: Commands| commands.queue(model::retell_screens));
        tools::register(app.world_mut());
        #[cfg(feature = "render")]
        {
            render::plugin(app);
            loading::plugin(app);
            app.add_systems(PostUpdate, flash::draw_flash.in_set(UiSystems::Draw).run_if(ai_core::display::drawn));
        }
    }

    fn finish(&self, app: &mut App) {
        #[cfg(feature = "render")]
        render::install_default_font(app);
        #[cfg(not(feature = "render"))]
        let _ = app;
    }
}

/// `ai/status` → `ui: {screens, input, focus, flash}`.
fn status(world: &World) -> Value {
    let screens: Vec<&str> = world.get_resource::<UiStack>().map(|s| s.screens().map(|(n, _)| n).collect()).unwrap_or_default();
    let input = top_screen(world).and_then(|e| world.get::<UiScreen>(e)).map(|s| s.name.clone());
    let focus = focused(world).and_then(|e| world.get::<UiWidget>(e)).map(|w| w.path.clone());
    let flash = world.get_resource::<ScreenFlash>().and_then(|f| f.0.as_ref().map(|f| f.id.clone()));
    json!({"screens": screens, "input": input, "focus": focus, "flash": flash})
}
