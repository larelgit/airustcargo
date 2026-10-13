//! # shards_motion — physics layer and character controller
//!
//! - [`physics`]: colliders with deterministic ids, collision layers, sensor volumes
//!   (water, updrafts, triggers) and queries (`raycast`, `shapecast`, `overlaps`,
//!   `volumes_at`) on [`PhysicsWorld`], usable from systems and from `&World` (AI tools).
//! - [`controller`]: the kinematic character controller — walk, run, sprint, slopes,
//!   stairs, jump, mantle, swim, glide — driven by [`LocomotionInput`].
//!
//! Everything runs in `FixedUpdate`:
//!
//! ```text
//! PhysicsSystems::Sync          start of the tick: colliders moved between ticks
//! SimSet::Input … Act           gameplay fills LocomotionInput
//! SimSet::Move
//!   ControllerSystems::Move     move_characters: casts against PhysicsWorld
//! PhysicsSystems::SyncAfterMove characters' new positions
//! SimSet::Resolve …             hit detection sees post-move positions
//! ```
//!
//! Collision math comes from parry3d, which stays an implementation detail: the API
//! speaks glam (`Vec3`, `Quat`, `Dir3`) and our own [`Shape`].
//!
//! ```ignore
//! app.add_plugins(MotionPlugin);
//! commands.spawn((Collider::standing_box(Vec3::new(5.0, 0.5, 5.0)), Transform::default()));
//! commands.spawn(character(CharacterParams::default(), Vec3::new(0.0, 1.0, 0.0)));
//! ```

pub mod controller;
pub mod physics;

use bevy::prelude::*;

pub use controller::{
    character, CharacterController, CharacterParams, ControllerPlugin, ControllerSystems, Jumped, Landed,
    LocomotionInput, Mantle, Mantled, ModeChanged, MotionMode, MotionState,
};
pub use physics::{
    Body, Collider, ColliderId, ColliderIds, ColliderInfo, CollisionLayers, LayerMask, Penetration, PhysicsPlugin,
    PhysicsSystems, PhysicsWorld, QueryFilter, RayHit, Shape, ShapeHit, SpatialQuery, Volume, VolumeEntered,
    VolumeExited, VolumeKind,
};

/// Physics and the character controller.
pub struct MotionPlugin;

impl Plugin for MotionPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<PhysicsPlugin>() {
            app.add_plugins(PhysicsPlugin);
        }
        if !app.is_plugin_added::<ControllerPlugin>() {
            app.add_plugins(ControllerPlugin);
        }
    }
}
