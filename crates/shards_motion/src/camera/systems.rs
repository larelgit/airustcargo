//! The systems that drive a [`ThirdPersonCamera`] with the camera math. They need a
//! window to be seen, so they are part of the `render` feature; headless apps have no
//! camera and nothing here runs.

use ai_core::{AiId, AiIndex};
use bevy::camera::Projection;
use bevy::post_process::motion_blur::MotionBlur;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use shards_core::{Intent, LocalPlayer};
use shards_settings::AccessibilitySettings;

use super::math::{
    aim_assist, arm_length, arm_reach, lock_framing, lock_holds, next_lock_target, orbit_angles, orbit_position, pick_lock_target,
    pitch_limits, resolve_camera_collision, shake_offset_with, wrap_angle, FlickState, LockCandidate, LockView,
};
use super::{CameraOffsets, CameraSettings, CameraShake, CameraSystems, CameraTuning, LockOnTarget, ThirdPersonCamera};
use crate::input::{ControlState, ControlSystems};
use crate::interp::{shown_translation, InterpSystems, Interpolated};
use crate::physics::{LayerMask, PhysicsWorld, QueryFilter};

/// The local player, as opposed to the camera that follows it.
type LocalPlayerFilter = (With<LocalPlayer>, Without<ThirdPersonCamera>);

type Lockable<'w, 's> =Query<'w, 's, (&'static AiId, &'static Transform), (With<LockOnTarget>, Without<ThirdPersonCamera>)>;

pub(super) fn plugin(app: &mut App) {
    app.add_systems(PreUpdate, lock_on.after(ControlSystems::Read))
        .add_systems(PostUpdate, orbit_camera.in_set(CameraSystems::Orbit).after(InterpSystems::Blend).before(TransformSystems::Propagate))
        .add_systems(Update, apply_motion_blur);
}

/// `Update`: the accessibility setting `motion_blur` (0 none, 1 full) on every camera that
/// carries Bevy's `MotionBlur` (a game opts in by adding the component; this only sets how
/// strong, and 0 switches the effect off). Presentation only.
fn apply_motion_blur(settings: Option<Res<AccessibilitySettings>>, mut cameras: Query<&mut MotionBlur>) {
    let Some(settings) = settings else { return };
    let strength = settings.blur();
    for mut blur in &mut cameras {
        // A shutter that is never open is no blur at all (Bevy skips the pass at 0). The samples are
        // the game's choice of quality and stay as they are, so turning the blur off and on again
        // gives back what the game asked for.
        if blur.shutter_angle != strength {
            blur.shutter_angle = strength;
        }
    }
}

/// The chest of a character: where the camera aims and rays go.
fn chest(tf: &Transform) -> Vec3 {
    chest_at(tf.translation)
}

/// The chest of a character standing (or drawn) at `feet`.
fn chest_at(feet: Vec3) -> Vec3 {
    feet + Vec3::Y
}

/// `PreUpdate`: the `lock_on` button picks a target (nearest the screen center, in range
/// and in sight) or lets go; a look flick switches to the neighbour on that side; a
/// target it picked that is gone, no longer a [`LockOnTarget`] (dead) or too far lets go by
/// itself. The result goes to `Intent.target`, so it is recorded and drives facing. A
/// target somebody else set is left alone.
///
/// The flick numbers are the tuning's ([`CameraTuning::flick`]). The cooldown runs on the
/// simulation clock (`Time<Virtual>`): a paused game does not run it down, a slowed one runs it
/// down slowly. The mouse's sum drains on the frame clock (`Time<Real>`), which is input, not game.
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
fn lock_on(
    time: Res<Time<Real>>,
    sim_time: Res<Time<Virtual>>,
    state: Res<ControlState>,
    settings: Res<CameraSettings>,
    tuning: Res<CameraTuning>,
    physics: Res<PhysicsWorld>,
    index: Res<AiIndex>,
    cameras: Query<&Transform, With<ThirdPersonCamera>>,
    positions: Query<&Transform, Without<ThirdPersonCamera>>,
    targets: Lockable,
    mut players: Query<(Option<&AiId>, &mut Intent), With<LocalPlayer>>,
    mut flick: Local<FlickState>,
    mut last_lock: Local<Option<AiId>>,
    mut picked: Local<Option<AiId>>,
) {
    flick.tick(sim_time.delta_secs());
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
        if position_of(id).is_none() || at.is_none_or(|p| !lock_holds(&view, p, tuning.release_factor)) {
            intent.target = None;
        }
    }

    // A stick already pushed over when a lock is taken cannot flick the fresh target.
    if *last_lock != intent.target {
        if last_lock.is_none() {
            flick.lock_taken(&tuning.flick, state.look_stick().x);
        }
        *last_lock = intent.target;
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
    // Both devices see every frame, so the mouse's sum drops when the lock goes and the stick
    // re-arms; the mouse wins a frame in which both flick.
    let current = intent.target.and_then(position_of);
    let (px, deflection) = (state.look_pointer_px().x, state.look_stick().x);
    let by_mouse = flick.mouse(&tuning.flick, px, time.delta_secs(), current.is_some());
    // The stick is asked only when the mouse did not switch: asking it re-arms or latches it.
    let by_stick = if by_mouse.is_some() { None } else { flick.stick(&tuning.flick, deflection, current.is_some()) };
    let (Some(current), Some(side)) = (current, by_mouse.or(by_stick)) else {
        return;
    };
    let next = next_lock_target(&view, &all, current, side);
    if let Some(next) = next {
        intent.target = Some(AiId(all[next].key));
        *picked = intent.target;
    }
    flick.flicked(&tuning.flick, next.is_some());
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
    tuning: Res<CameraTuning>,
    offsets: Res<CameraOffsets>,
    state: Res<ControlState>,
    physics: Res<PhysicsWorld>,
    index: Res<AiIndex>,
    mut shake: ResMut<CameraShake>,
    players: Query<(Entity, &Intent), LocalPlayerFilter>,
    followed: Query<&Transform, Without<ThirdPersonCamera>>,
    lockable: Lockable,
    children: Query<&Children>,
    visuals: Query<&Interpolated>,
    mut cameras: Query<(&mut ThirdPersonCamera, &mut Transform, Option<&mut Projection>)>,
) {
    let dt = time.delta_secs();
    if !settings.shake {
        shake.trauma = 0.0;
    }
    let wobble = shake_offset_with(shake.trauma, time.elapsed_secs(), &tuning.trauma);
    shake.trauma = (shake.trauma - tuning.trauma.decay * dt).max(0.0);
    // What the game adds on top of the base values this frame; the camera keeps none of it.
    let limits = pitch_limits(tuning.pitch, &offsets);
    let player = players.single().ok();

    for (mut camera, mut tf, projection) in &mut cameras {
        let subject = if camera.target == Entity::PLACEHOLDER { player.map(|(e, _)| e) } else { Some(camera.target) };
        let Some(subject_tf) = subject.and_then(|e| followed.get(e).ok()) else {
            continue;
        };
        let locked = player
            .and_then(|(_, intent)| intent.target)
            .and_then(|id| index.get(id))
            .and_then(|e| followed.get(e).ok().map(|tf| (e, tf)))
            .map(|(e, tf)| chest_at(shown_translation(e, tf, &children, &visuals)));

        let subject_at = shown_translation(subject.unwrap_or(Entity::PLACEHOLDER), subject_tf, &children, &visuals);
        let head = subject_at + Vec3::Y * camera.height;
        let rate = 1.0 - ops::exp(-tuning.follow_rate.unwrap_or(camera.follow_rate) * dt);
        if camera.focus == Vec3::ZERO || camera.focus.distance(head) > tuning.snap_distance {
            // A jump (a teleport, a respawn): nothing of the old place to catch up from.
            camera.focus = head;
            camera.arm_reach = 0.0;
        } else {
            camera.focus = camera.focus.lerp(head, rate);
        }

        match locked {
            // Framing a target: behind the player, looking at it. The look axis flicks targets.
            Some(target) => {
                (camera.yaw, camera.pitch) = lock_framing(camera.yaw, camera.pitch, camera.focus, target, &tuning.framing, dt);
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
                camera.pitch += turn.y;
            }
        }
        // The stored pitch is held to the limits, so a cap that lifts does not find it far past it.
        camera.pitch = camera.pitch.clamp(limits.0, limits.1);

        // The dip lowers the camera and what it looks at together: the picture drops without tilting.
        let focus = camera.focus - Vec3::Y * finite_or_zero(offsets.focus_dip_m);
        let previous_length = camera.arm_length;
        camera.arm_length = arm_length(camera.arm_length, camera.distance, &tuning.arm, dt);
        let desired = orbit_position(focus, camera.yaw, camera.pitch, camera.arm_length);
        let exclude: Vec<Entity> = subject.into_iter().collect();
        let mut at = resolve_camera_collision(&physics, focus, desired, tuning.arm.radius, tuning.arm.min_distance, &exclude);
        // A wall pulls the arm in at once; when it lets go the arm grows back at the tuning's rate.
        let free = (at - focus).length();
        let reach = arm_reach(camera.arm_reach, previous_length, camera.arm_length, free, &tuning.arm, dt);
        camera.arm_reach = reach;
        if reach < free {
            at = focus + (desired - focus).normalize_or_zero() * reach;
        }
        let looking = Transform::from_translation(at).looking_at(focus, Vec3::Y);
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
            let fov = (settings.fov_deg + finite_or_zero(offsets.fov_add_deg)).clamp(30.0, 120.0).to_radians();
            if (perspective.fov - fov).abs() > 1e-5 {
                perspective.fov = fov;
            }
        }
    }
}

/// A number a game system wrote into [`CameraOffsets`] that is not finite counts as 0: one bad
/// write must not poison the projection or the transform.
fn finite_or_zero(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
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
