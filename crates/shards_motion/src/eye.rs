//! The eye and the look direction (F1a): where a character looks from and along.
//!
//! Everything that needs "where is that body looking" asks here: the `look` tool and the `AIM`
//! line of `observe`, and the slices built on them (hitscan shots, use-by-looking, turrets, the
//! audio listener). The answers come from the simulation (`Transform`, `Intent`, the body's
//! `locomotion` numbers), never from a camera, so a headless run, a replay and a window agree.
//!
//! - [`eye_of`]: the point, `eye_height` above the feet (default 0.9 of the body's height).
//! - [`look_dir`]: the unit vector, from `Intent::look` held to the body's pitch limits, else the
//!   way the body faces, level.
//! - [`aim_point_of`]: the point on a body that a look at it meets (the lock-on's aim height).
//!
//! A stance that changes the body's height (the crouch of F2) changes the eye through its
//! `locomotion`; these functions read whatever the body's params say now.

use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::{flat, Intent};

use crate::camera::lock::aim_of;
use crate::camera::LockOnDef;
use crate::controller::{CharacterController, CharacterParams};

/// The numbers `entity` moves by: its own [`CharacterParams`], else the `locomotion` definition its
/// [`CharacterController`] names, else (a controller without content) the code defaults. `None`
/// for an entity that is no character.
pub fn params_of(world: &World, entity: Entity) -> Option<&CharacterParams> {
    let e = world.get_entity(entity).ok()?;
    let controller = e.get::<CharacterController>()?;
    Some(controller.resolve(e.get::<CharacterParams>(), world.get_resource::<ContentDb>()))
}

/// Where the character `entity` looks from: its feet plus its `eye_height` (0.9 of its height
/// unless its `locomotion` says). `None` for an entity with no position or no character controller.
///
/// ```ignore
/// let eye = shards::motion::eye_of(world, me).expect("a character");
/// ```
pub fn eye_of(world: &World, entity: Entity) -> Option<Vec3> {
    let feet = world.get::<Transform>(entity)?.translation;
    Some(feet + Vec3::Y * params_of(world, entity)?.eye_above_feet())
}

/// The unit vector `entity` looks along: its [`Intent::look`] with the pitch held between the
/// body's `pitch_limits_deg`, else the way the body faces (its `Transform`), level. `None` for an
/// entity with no position or no character controller.
///
/// Whether the body's yaw follows the look is its `face` option: a `Look` body faces the look, so
/// the two agree; a `Move` body that was given a look (an agent's `look` tool) still reports it
/// here, and keeps turning by its movement.
pub fn look_dir(world: &World, entity: Entity) -> Option<Vec3> {
    let params = params_of(world, entity)?;
    let tf = world.get::<Transform>(entity)?;
    let looked = world.get::<Intent>(entity).and_then(|i| i.look).filter(|l| l.is_finite());
    Some(match looked {
        Some(look) => params.clamp_look(look).dir(),
        None => flat(*tf.forward()).try_normalize().unwrap_or(Vec3::NEG_Z),
    })
}

/// The point a look at `entity` meets: the body at the lock-on's aim height (the `lock_on`
/// content's `aim_height`, 0.6 of the collider's height by default), or the middle of a
/// [`LockPoint`]. `None` for an entity with no position.
pub fn aim_point_of(world: &World, entity: Entity) -> Option<Vec3> {
    match LockOnDef::active(world.get_resource::<ContentDb>()) {
        Some(def) => aim_of(world, entity, def),
        None => aim_of(world, entity, &LockOnDef::default()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::LockPoint;
    use crate::physics::{Collider, Shape};
    use shards_core::Gaze;

    /// The default character's capsule: 0.4 m wide at the feet, 1.8 m tall.
    fn capsule() -> Collider {
        Collider::new(Shape::capsule(0.4, 1.8)).with_offset(Vec3::Y * 0.9)
    }

    fn world_with(body: impl Bundle) -> (World, Entity) {
        let mut world = World::new();
        let entity = world.spawn(body).id();
        (world, entity)
    }

    #[test]
    fn the_eye_is_above_the_feet_by_the_params() {
        let at = Transform::from_xyz(2.0, 1.0, -3.0);
        let (world, e) = world_with((at, CharacterController::default()));
        assert!(eye_of(&world, e).unwrap().distance(Vec3::new(2.0, 1.0 + 0.9 * 1.8, -3.0)) < 1e-5, "0.9 of the default 1.8 m");
        let own = CharacterParams { height: 2.0, eye_height: Some(1.5), ..default() };
        let (world, e) = world_with((at, CharacterController::default(), own));
        assert!(eye_of(&world, e).unwrap().distance(Vec3::new(2.0, 2.5, -3.0)) < 1e-5, "eye_height: meters above the feet");
        let (world, e) = world_with(at);
        assert_eq!((eye_of(&world, e), look_dir(&world, e)), (None, None), "no character, no eye");
        let mut world = World::new();
        let gone = world.spawn_empty().id();
        world.despawn(gone);
        assert_eq!(eye_of(&world, gone), None);
    }

    #[test]
    fn the_look_direction_is_the_look_held_to_the_limits_else_the_facing_level() {
        let faces_east = Transform::from_xyz(0.0, 0.0, 0.0).with_rotation(Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2));
        let (mut world, e) = world_with((faces_east, CharacterController::default(), Intent::default()));
        assert!(look_dir(&world, e).unwrap().distance(Vec3::X) < 1e-5, "no look: the way the body faces, level");
        world.get_mut::<Intent>(e).unwrap().look = Some(Gaze::from_degrees(90.0, 30.0));
        let dir = look_dir(&world, e).unwrap();
        assert!(dir.distance(Gaze::from_degrees(90.0, 30.0).dir()) < 1e-5 && dir.x < 0.0 && dir.y > 0.0, "{dir}");
        // Straight up is held to the body's limit (89 degrees unless its locomotion says).
        world.get_mut::<Intent>(e).unwrap().look = Some(Gaze::from_degrees(0.0, 90.0));
        let up = look_dir(&world, e).unwrap();
        assert!((up.y - ops::sin(89.0_f32.to_radians())).abs() < 1e-5 && up.z < 0.0, "{up}");
        world.entity_mut(e).insert(CharacterParams { pitch_limits_deg: (-30.0, 45.0), ..default() });
        world.get_mut::<Intent>(e).unwrap().look = Some(Gaze::from_degrees(0.0, -80.0));
        assert!((look_dir(&world, e).unwrap().y + ops::sin(30.0_f32.to_radians())).abs() < 1e-5, "looking down is held to -30");
        // A look that is not a number is no look.
        world.get_mut::<Intent>(e).unwrap().look = Some(Gaze::new(f32::NAN, 0.0));
        assert!(look_dir(&world, e).unwrap().distance(Vec3::X) < 1e-5);
    }

    #[test]
    fn the_aim_point_is_a_fraction_of_the_body() {
        let (world, e) = world_with((Transform::from_xyz(1.0, 0.0, 1.0), capsule()));
        let p = aim_point_of(&world, e).unwrap();
        assert!((p.y - 0.6 * 1.8).abs() < 1e-4 && (p.x, p.z) == (1.0, 1.0), "{p}");
        let (world, e) = world_with((Transform::from_xyz(0.0, 3.0, 0.0), capsule(), LockPoint));
        assert!((aim_point_of(&world, e).unwrap().y - (3.0 + 0.9)).abs() < 1e-4, "a lock point's is its centre");
    }
}
