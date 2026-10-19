//! The window side of combat (`render` feature): what the action machine decides, shown.
//!
//! - [`bridge`]: **animation**. glTF clips follow the machine: gameplay leads, the animation
//!   catches up. One `AnimationGraph` per [`AnimSet`](crate::AnimSet), shared by every
//!   [`Animated`](crate::Animated) character of the set.
//! - [`overlay`]: **ground telegraphs** (always on) and the **combat overlay** (the
//!   `combat_overlay` tool, F2), drawn with gizmos from the lists of [`crate::draw`].
//! - [`hud`]: **off-screen warnings**, Bevy UI markers on the screen edge.
//!
//! The plugin adds only what the app can show: the bridge needs Bevy's `AnimationPlugin`, the
//! overlays the `GizmoPlugin` (both in `DefaultPlugins`). The overlays and the markers draw only
//! while a `Window` exists, so a headless engine run, which has the plugins but no window,
//! does nothing here. All of it is presentation: nothing in the simulation reads it, replays
//! ignore it.
//!
//! The pure logic (which clip, what geometry, where on the edge) is in [`crate::anim`] and
//! [`crate::draw`], tested without a window.

pub mod bridge;
pub mod hud;
pub mod overlay;
pub mod props;

use bevy::prelude::*;
use shards_core::LocalPlayer;
use shards_settings::AccessibilitySettings;

pub use bridge::{AnimGraphs, AnimStats, AnimationBound};
pub use hud::ThreatHud;
pub use props::{PropOf, Sockets};
pub use overlay::{CombatOverlay, CombatOverlayGizmos, TelegraphGizmos, TelegraphSettings};

pub(crate) fn plugin(app: &mut App) {
    overlay::plugin(app);
    hud::plugin(app);
    app.add_systems(Update, apply_flashes);
    if app.is_plugin_added::<bevy::animation::AnimationPlugin>() {
        bridge::plugin(app);
    }
}

/// `Update`: the player's flash setting (accessibility) into the ground telegraphs and the
/// off-screen markers, which flicker as an attack nears. Presentation only.
fn apply_flashes(
    settings: Option<Res<AccessibilitySettings>>,
    mut telegraphs: ResMut<TelegraphSettings>,
    mut hud: ResMut<ThreatHud>,
) {
    let Some(settings) = settings.filter(|s| s.is_changed()) else { return };
    let level = settings.flash_level();
    if telegraphs.style.flashes != level {
        telegraphs.style.flashes = level;
    }
    if hud.flashes != level {
        hud.flashes = level;
    }
}

/// The camera the player looks through: the first active 3D camera, the AI's own
/// (`AiCamera`) before any other.
pub(crate) fn active_camera(world: &mut World) -> Option<Entity> {
    let mut q = world.query_filtered::<(Entity, &Camera, Has<ai_core::AiCamera>), With<Camera3d>>();
    let mut cameras: Vec<(Entity, bool)> = q.iter(world).filter(|(_, c, _)| c.is_active).map(|(e, _, ai)| (e, ai)).collect();
    cameras.sort_by_key(|(e, ai)| (!*ai, *e));
    cameras.first().map(|(e, _)| *e)
}

/// The rotation that makes text face the active camera.
pub(crate) fn billboard(world: &mut World) -> Quat {
    active_camera(world)
        .and_then(|e| world.get::<GlobalTransform>(e))
        .map_or(Quat::IDENTITY, |gt| gt.rotation())
}

/// The character this machine's player controls.
pub(crate) fn local_player(world: &mut World) -> Option<Entity> {
    world.query_filtered::<Entity, With<LocalPlayer>>().iter(world).next()
}

#[cfg(test)]
mod tests {
    use shards_settings::FlashLevel;

    use super::*;

    #[test]
    fn the_flash_setting_reaches_the_warnings() {
        let mut app = App::new();
        app.init_resource::<TelegraphSettings>()
            .init_resource::<ThreatHud>()
            .init_resource::<AccessibilitySettings>()
            .add_systems(Update, apply_flashes);
        app.update();
        assert_eq!(app.world().resource::<ThreatHud>().flashes, FlashLevel::Full);
        app.world_mut().resource_mut::<AccessibilitySettings>().flashes = "reduced".into();
        app.update();
        assert_eq!(app.world().resource::<ThreatHud>().flashes, FlashLevel::Reduced);
        assert_eq!(app.world().resource::<TelegraphSettings>().style.flashes, FlashLevel::Reduced);
        // A hand-edited word nobody knows is the default, not a crash.
        app.world_mut().resource_mut::<AccessibilitySettings>().flashes = "strobe".into();
        app.update();
        assert_eq!(app.world().resource::<ThreatHud>().flashes, FlashLevel::Full);
    }
}
