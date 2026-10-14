//! The third-person camera (E2.5): orbit, wall collision, lock-on, aim assist, shake, FOV.
//!
//! **Presentation only.** The simulation and the AI's `observe` never read the camera:
//! what the AI sees comes from its subject's eyes, and a window with this camera plays the
//! same game as a headless run. The one thing the camera writes into the game is the
//! lock-on target, as `Intent.target` between ticks (it is input, so replays record it).
//!
//! The math is in [`math`] as plain functions (`orbit_position`, `resolve_camera_collision`,
//! `pick_lock_target`, `next_lock_target`, `shake_offset`, `aim_assist`); the systems that
//! drive a camera entity with it need a window and live behind the `render` feature.
//!
//! ```ignore
//! // Follows the entity with `LocalPlayer`:
//! commands.spawn((Camera3d::default(), AiCamera, ThirdPersonCamera::default()));
//! // A hit shakes it (when the player's setting allows):
//! commands.trigger(AddTrauma(0.4));
//! ```

pub mod math;
#[cfg(feature = "render")]
mod systems;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

pub use math::{
    aim_assist, damp, lock_holds, next_lock_target, orbit_angles, orbit_offset, orbit_position, pick_lock_target,
    resolve_camera_collision, shake_offset, wrap_angle, LockCandidate, LockView, ShakeOffset, Side,
};

/// A camera that orbits a character. Put it on the camera entity; `yaw` and `pitch` are
/// where the player looks (`input` turns the move axes by `yaw`).
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component, Default)]
pub struct ThirdPersonCamera {
    /// The character to follow. [`Entity::PLACEHOLDER`] (the default): the entity with
    /// `LocalPlayer`, whichever it is now.
    pub target: Entity,
    /// Turn about Y, radians (0: behind a character facing -Z).
    pub yaw: f32,
    /// Tilt, radians; positive looks down from above.
    pub pitch: f32,
    /// Distance from the focus point (m) when nothing is in the way.
    pub distance: f32,
    /// Height of the focus point above the character's feet (m).
    pub height: f32,
    /// The focus point as smoothed so far.
    pub focus: Vec3,
    /// How quickly the focus catches up with the character (1/s).
    pub follow_rate: f32,
}

impl Default for ThirdPersonCamera {
    fn default() -> Self {
        Self {
            target: Entity::PLACEHOLDER,
            yaw: 0.0,
            pitch: 0.35,
            distance: 5.5,
            height: 1.5,
            focus: Vec3::ZERO,
            follow_rate: 14.0,
        }
    }
}

impl ThirdPersonCamera {
    /// A camera following `target`.
    pub fn following(target: Entity) -> Self {
        Self { target, ..default() }
    }
}

/// How the camera feels; the player's accessibility choices (E8 saves them).
#[derive(Resource, Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[reflect(Resource, Default)]
#[serde(default)]
pub struct CameraSettings {
    /// Vertical field of view, degrees.
    pub fov_deg: f32,
    /// Look speed multiplier.
    pub sensitivity: f32,
    /// Looking up moves the camera up instead of down.
    pub invert_y: bool,
    /// Whether hits shake the camera.
    pub shake: bool,
    /// How hard they do (0..1).
    pub shake_intensity: f32,
    /// How much the look bends toward a target near the crosshair (0 off, 1 full).
    pub aim_assist: f32,
    /// A lock-on picks and holds targets this close (m).
    pub lock_range: f32,
    /// Half the width of the cone in front of the camera a lock-on picks from (degrees).
    pub lock_cone_deg: f32,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            fov_deg: 70.0,
            sensitivity: 1.0,
            invert_y: false,
            shake: true,
            shake_intensity: 1.0,
            aim_assist: 0.35,
            lock_range: 20.0,
            lock_cone_deg: 55.0,
        }
    }
}

/// Marks what a lock-on can pick: enemies and the like (the game or the action module
/// adds it, and removes it when they die, which releases the lock).
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct LockOnTarget;

/// The camera's current shake: `trauma` in 0..1 decays on its own.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq)]
pub struct CameraShake {
    pub trauma: f32,
}

impl CameraShake {
    /// A hit of `amount` (0..1): trauma adds up, to at most 1.
    pub fn add_trauma(&mut self, amount: f32) {
        self.trauma = (self.trauma + amount.max(0.0)).min(1.0);
    }
}

/// `commands.trigger(AddTrauma(0.4))`: shakes the camera if its setting is on.
#[derive(Event, Clone, Copy, Debug, PartialEq)]
pub struct AddTrauma(pub f32);

pub(crate) fn plugin(app: &mut App) {
    app.register_type::<ThirdPersonCamera>()
        .register_type::<CameraSettings>()
        .register_type::<LockOnTarget>()
        .init_resource::<CameraSettings>()
        .init_resource::<CameraShake>()
        .add_observer(|trigger: On<AddTrauma>, settings: Res<CameraSettings>, mut shake: ResMut<CameraShake>| {
            if settings.shake {
                shake.add_trauma(trigger.event().0 * settings.shake_intensity.clamp(0.0, 1.0));
            }
        });
    #[cfg(feature = "render")]
    systems::plugin(app);
}
