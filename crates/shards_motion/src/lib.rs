//! # shards_motion — physics layer
//!
//! [`physics`]: colliders with deterministic ids, collision layers, sensor volumes
//! (water, updrafts, triggers) and queries (`raycast`, `shapecast`, `overlaps`,
//! `volumes_at`) on [`PhysicsWorld`], usable from systems and from `&World` (AI tools).
//!
//! Everything runs in `FixedUpdate`:
//!
//! ```text
//! PhysicsSystems::Sync          start of the tick: colliders moved between ticks
//! SimSet::Input … Move          gameplay
//! PhysicsSystems::SyncAfterMove new positions
//! SimSet::Resolve …             hit detection sees post-move positions
//! ```
//!
//! Collision math comes from parry3d, which stays an implementation detail: the API
//! speaks glam (`Vec3`, `Quat`, `Dir3`) and our own [`Shape`].

pub mod physics;

pub use physics::{
    Body, Collider, ColliderId, ColliderIds, ColliderInfo, CollisionLayers, LayerMask, Penetration, PhysicsPlugin,
    PhysicsSystems, PhysicsWorld, QueryFilter, RayHit, Shape, ShapeHit, SpatialQuery, Volume, VolumeEntered,
    VolumeExited, VolumeKind,
};
