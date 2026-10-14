//! The systems that drive a [`ThirdPersonCamera`] with the camera math. They need a
//! window to be seen, so they are part of the `render` feature; headless apps have no
//! camera and nothing here runs.

use ai_core::{AiId, AiIndex};
use bevy::camera::Projection;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use shards_core::{Intent, LocalPlayer};

use super::math::{
    aim_assist, damp, lock_holds, next_lock_target, orbit_angles, orbit_position, pick_lock_target, resolve_camera_collision,
    shake_offset, wrap_angle, LockCandidate, LockView, Side,
};
use super::{CameraSettings, CameraShake, LockOnTarget, ThirdPersonCamera};
use crate::input::{ControlState, ControlSystems};
use crate::physics::{LayerMask, PhysicsWorld, QueryFilter};

/// Highest and lowest tilt (radians).
const PITCH: (f32, f32) = (-0.45, 1.35);
/// A lock-on lets go at this many times its range.
const RELEASE_FACTOR: f32 = 1.3;
/// A look flick this big in one frame (radians) switches the lock-on target.
const FLICK: f32 = 0.035;
/// Seconds before the lock-on switches again.
const FLICK_COOLDOWN: f32 = 0.35;
/// The camera sphere that is pulled in by walls (m).
const CAMERA_RADIUS: f32 = 0.25;
/// Never closer to the focus than this (m).
const MIN_DISTANCE: f32 = 0.7;
/// How fast trauma drains (per second).
const TRAUMA_DECAY: f32 = 1.4;
/// The camera frames a locked target this fast (1/s).
const LOCK_FRAMING_RATE: f32 = 7.0;

/// The local player, as opposed to the camera that follows it.
type LocalPlayerFilter = (With<LocalPlayer>, Without<ThirdPersonCamera>);

type Lockable<'w, 's> =Query<'w, 's, (&'static AiId, &'static Transform), (With<LockOnTarget>, Without<ThirdPersonCamera>)>;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(PreUpdate, lock_on.after(ControlSystems::Read))
        .add_systems(PostUpdate, orbit_camera.before(TransformSystems::Propagate));
}

/// The chest of a character: where the camera aims and rays go.
fn chest(tf: &Transform) -> Vec3 {
    tf.translation + Vec3::Y
}

/// `PreUpdate`: the `lock_on` button picks a target (nearest the screen center, in range
/// and in sight) or lets go; a look flick switches to the neighbour on that side; a
/// target it picked that is gone, no longer a [`LockOnTarget`] (dead) or too far lets go by
/// itself. The result goes to `Intent.target`, so it is recorded and drives facing. A
/// target somebody else set is left alone.
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
fn lock_on(
    time: Res<Time<Real>>,
    state: Res<ControlState>,
    settings: Res<CameraSettings>,
    physics: Res<PhysicsWorld>,
    index: Res<AiIndex>,
    cameras: Query<&Transform, With<ThirdPersonCamera>>,
    positions: Query<&Transform, Without<ThirdPersonCamera>>,
    targets: Lockable,
    mut players: Query<(Option<&AiId>, &mut Intent), With<LocalPlayer>>,
    mut cooldown: Local<f32>,
    mut picked: Local<Option<AiId>>,
) {
    *cooldown = (*cooldown - time.delta_secs()).max(0.0);
    let (Ok((me, mut intent)), Some(camera)) = (players.single_mut(), cameras.iter().next()) else {
        return;
    };
    let view = LockView {
        eye: camera.translation,
        forward: *camera.forward(),
        range: settings.lock_range,
        half_angle_deg: settings.lock_cone_deg,
    };
    let mut all: Vec<LockCandidate> =
        targets.iter().filter(|(id, _)| Some(*id) != me).map(|(id, tf)| LockCandidate { key: id.0, position: chest(tf) }).collect();
    all.sort_by_key(|c| c.key);
    let position_of = |id: AiId| all.iter().position(|c| c.key == id.0);

    // A lock this camera picked lets go by itself: the target is gone, no longer lockable
    // (dead) or too far from the eye. A lock anyone else set (an agent's tool, a brain) is
    // gameplay's, whatever the camera sees of it: the eye is presentation, so measuring a
    // rule from it would play the same script differently with and without a window.
    if *picked != intent.target {
        *picked = None;
    }
    if let Some(id) = *picked {
        let at = index.get(id).and_then(|e| positions.get(e).ok()).map(chest);
        if position_of(id).is_none() || at.is_none_or(|p| !lock_holds(&view, p, RELEASE_FACTOR)) {
            intent.target = None;
        }
    }

    if state.just_pressed("lock_on") {
        if intent.target.is_some() {
            intent.target = None;
        } else {
            let in_sight: Vec<LockCandidate> = all.iter().copied().filter(|c| clear_view(&physics, view.eye, c.position)).collect();
            if let Some(pick) = pick_lock_target(&view, &in_sight) {
                intent.target = Some(AiId(in_sight[pick].key));
                *picked = intent.target;
            }
        }
        return;
    }
    let Some(current) = intent.target.and_then(position_of) else {
        return;
    };
    if *cooldown > 0.0 {
        return;
    }
    let flick = state.axis("look").x;
    let side = if flick > FLICK {
        Side::Right
    } else if flick < -FLICK {
        Side::Left
    } else {
        return;
    };
    if let Some(next) = next_lock_target(&view, &all, current, side) {
        intent.target = Some(AiId(all[next].key));
        *picked = intent.target;
        *cooldown = FLICK_COOLDOWN;
    }
}

/// Nothing solid between two points.
fn clear_view(physics: &PhysicsWorld, from: Vec3, to: Vec3) -> bool {
    let delta = to - from;
    let Ok(dir) = Dir3::new(delta) else { return true };
    let distance = delta.length();
    physics.raycast(from, dir, distance, &QueryFilter::new(LayerMask::WORLD)).is_none_or(|hit| hit.distance >= distance - 0.05)
}

/// `PostUpdate`, before transforms propagate: look, follow, frame the lock-on, keep out of
/// walls, shake, field of view.
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
fn orbit_camera(
    time: Res<Time<Real>>,
    settings: Res<CameraSettings>,
    state: Res<ControlState>,
    physics: Res<PhysicsWorld>,
    index: Res<AiIndex>,
    mut shake: ResMut<CameraShake>,
    players: Query<(Entity, &Intent), LocalPlayerFilter>,
    followed: Query<&Transform, Without<ThirdPersonCamera>>,
    lockable: Lockable,
    mut cameras: Query<(&mut ThirdPersonCamera, &mut Transform, Option<&mut Projection>)>,
) {
    let dt = time.delta_secs();
    if !settings.shake {
        shake.trauma = 0.0;
    }
    let wobble = shake_offset(shake.trauma, time.elapsed_secs());
    shake.trauma = (shake.trauma - TRAUMA_DECAY * dt).max(0.0);
    let player = players.single().ok();

    for (mut camera, mut tf, projection) in &mut cameras {
        let subject = if camera.target == Entity::PLACEHOLDER { player.map(|(e, _)| e) } else { Some(camera.target) };
        let Some(subject_tf) = subject.and_then(|e| followed.get(e).ok()) else {
            continue;
        };
        let locked = player
            .and_then(|(_, intent)| intent.target)
            .and_then(|id| index.get(id))
            .and_then(|e| followed.get(e).ok())
            .map(chest);

        let head = subject_tf.translation + Vec3::Y * camera.height;
        let rate = 1.0 - (-camera.follow_rate * dt).exp();
        camera.focus = if camera.focus == Vec3::ZERO || camera.focus.distance(head) > 12.0 { head } else { camera.focus.lerp(head, rate) };

        match locked {
            // Framing a target: behind the player, looking at it. The look axis flicks targets.
            Some(target) => {
                let (yaw, pitch) = orbit_angles(camera.focus - target);
                camera.yaw = wrap_angle(camera.yaw + wrap_angle(yaw - camera.yaw) * (1.0 - (-LOCK_FRAMING_RATE * dt).exp()));
                camera.pitch = damp(camera.pitch, pitch.clamp(0.15, 0.7), LOCK_FRAMING_RATE, dt);
            }
            // Free look: the player's turn, bent toward a target near the crosshair.
            None => {
                let up = if settings.invert_y { -1.0 } else { 1.0 };
                let look = state.axis("look") * settings.sensitivity;
                let mut turn = Vec2::new(-look.x, -look.y * up);
                if let Some(error) = crosshair_error(&camera, &lockable).filter(|_| settings.aim_assist > 0.0) {
                    turn = aim_assist(turn, error, settings.aim_assist);
                }
                camera.yaw = wrap_angle(camera.yaw + turn.x);
                camera.pitch = (camera.pitch + turn.y).clamp(PITCH.0, PITCH.1);
            }
        }

        let desired = orbit_position(camera.focus, camera.yaw, camera.pitch, camera.distance);
        let exclude: Vec<Entity> = subject.into_iter().collect();
        let at = resolve_camera_collision(&physics, camera.focus, desired, CAMERA_RADIUS, MIN_DISTANCE, &exclude);
        let looking = Transform::from_translation(at).looking_at(camera.focus, Vec3::Y);
        let shaken = Quat::from_euler(EulerRot::YXZ, wobble.rotation.y, wobble.rotation.x, wobble.rotation.z);
        let next = Transform {
            translation: looking.translation + looking.rotation * wobble.translation,
            rotation: looking.rotation * shaken,
            scale: tf.scale,
        };
        if *tf != next {
            *tf = next;
        }

        if let Some(mut projection) = projection
            && let Projection::Perspective(perspective) = &mut *projection
        {
            let fov = settings.fov_deg.clamp(30.0, 120.0).to_radians();
            if (perspective.fov - fov).abs() > 1e-5 {
                perspective.fov = fov;
            }
        }
    }
}

/// How much (yaw, pitch) the camera must change to put the lockable target nearest the
/// crosshair on it; `aim_assist` ignores the ones too far from it to bend toward.
fn crosshair_error(camera: &ThirdPersonCamera, lockable: &Lockable) -> Option<Vec2> {
    let eye = orbit_position(camera.focus, camera.yaw, camera.pitch, camera.distance);
    lockable
        .iter()
        .map(|(_, tf)| {
            let (yaw, pitch) = orbit_angles(eye - chest(tf));
            Vec2::new(wrap_angle(yaw - camera.yaw), pitch - camera.pitch)
        })
        .min_by(|a, b| a.length_squared().total_cmp(&b.length_squared()))
}
