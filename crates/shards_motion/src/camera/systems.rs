//! The systems that drive a [`ThirdPersonCamera`] with the camera math. They need a
//! window to be seen, so they are part of the `render` feature; headless apps have no
//! camera and nothing here runs.

use ai_core::{journal, label, AiId, AiIndex, JournalKind};
use bevy::camera::Projection;
use bevy::post_process::motion_blur::MotionBlur;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use shards_content::ContentDb;
use shards_core::input_names::{LOCK_ON, LOCK_SWITCH};
use shards_core::{Intent, LocalPlayer, Press};
use shards_settings::AccessibilitySettings;

use super::lock::{angle, half_width, in_frustum, lock_candidates, pick_lock, sight_clear, switch_target, LockOnDef, Lockable, PickView};
use super::math::{
    aim_assist, arm_length, arm_reach, auto_centre, damp, fit_room, fit_tilt, lock_framing, lock_holds, next_lock_target, orbit_angles, orbit_position,
    pick_lock_target, pitch_limits, resolve_camera_collision, shake_offset_with, wrap_angle, FlickState, LockCandidate, LockView,
};
use super::shot::{shot_playing, Shots};
use super::{CameraOffsets, CameraSettings, CameraShake, CameraSystems, CameraTuning, LockOnTarget, LockPoint, ThirdPersonCamera};
use crate::input::{ControlState, ControlSystems};
use crate::interp::{shown_translation, InterpSystems, Interpolated};
use crate::physics::{LayerMask, PhysicsWorld, QueryFilter};

/// The local player, as opposed to the camera that follows it.
type LocalPlayerFilter = (With<LocalPlayer>, Without<ThirdPersonCamera>);

type LockOnTargets<'w, 's> = Query<'w, 's, (&'static AiId, &'static Transform, Has<LockPoint>), (With<LockOnTarget>, Without<ThirdPersonCamera>)>;

/// What the lock-on rules read of the camera, and the local player they lock for.
type RuleCamera = (&'static Transform, Option<&'static Projection>);
type RulePlayer = (With<LocalPlayer>, With<Intent>, Without<ThirdPersonCamera>);

pub(super) fn plugin(app: &mut App) {
    // Without a `lock_on` definition the camera picks and lets go as it always did; with one it
    // picks by the definition's rules and leaves the checking of a lock to the simulation.
    // While a scene camera shot plays (K3) a lock-on picks and switches nothing.
    app.add_systems(
        PreUpdate,
        (lock_on.run_if(not(lock_rules)), lock_by_rules.run_if(lock_rules)).run_if(not(shot_playing)).after(ControlSystems::Read),
    )
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

/// Where a lock-on target is aimed at: a character's chest, a [`LockPoint`]'s own position (its centre).
fn aim_point(tf: &Transform, point: bool) -> Vec3 {
    if point { tf.translation } else { chest(tf) }
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
    targets: LockOnTargets,
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
        targets.iter().filter(|(id, ..)| Some(*id) != me).map(|(id, tf, point)| LockCandidate { key: id.0, position: aim_point(tf, point) }).collect();
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

/// Run condition: the game has a `lock_on` definition, so the lock-on rules are in force.
fn lock_rules(db: Option<Res<ContentDb>>) -> bool {
    LockOnDef::active(db.as_deref()).is_some()
}

/// `PreUpdate`, with a `lock_on` definition: the `lock_on` button takes the target the scored
/// pick names among what the camera sees (inside its frustum at the player's base field of view,
/// or the cone, and a clear ray from the eye), or lets go of the lock (the player's cancel: a
/// local line `e5 lock released: e6`); a look flick or a turn of the mouse wheel (one switch per
/// turn) is a `lock_switch` press to that side, which the simulation carries out by its own rule
/// (sent only when that rule finds somebody there, so a flick into nothing does not start the
/// cooldown unless the tuning says so). The camera lets go of a lock only on that cancel: the
/// action module checks every lock on every tick, with a window or without. The queries are kept
/// between frames.
fn lock_by_rules(
    world: &mut World,
    cameras: &mut QueryState<RuleCamera, With<ThirdPersonCamera>>,
    players: &mut QueryState<Entity, RulePlayer>,
    mut flick: Local<FlickState>,
    mut last_lock: Local<Option<AiId>>,
) {
    flick.tick(world.resource::<Time<Virtual>>().delta_secs());
    let Some(def) = LockOnDef::active(world.get_resource::<ContentDb>()).cloned() else { return };
    let frame_dt = world.resource::<Time<Real>>().delta_secs();
    let flick_tuning = world.resource::<CameraTuning>().flick.clone();
    let settings = world.resource::<CameraSettings>().clone();
    let (lock_pressed, px, deflection, wheel) = {
        let state = world.resource::<ControlState>();
        (state.just_pressed(LOCK_ON), state.look_pointer_px().x, state.look_stick().x, state.axis(LOCK_SWITCH))
    };
    let Some((camera, aspect)) = cameras.iter(world).next().map(|(tf, projection)| (*tf, aspect_of(projection))) else {
        return;
    };
    let Ok(me) = players.single(world) else { return };
    let target = world.get::<Intent>(me).and_then(|i| i.target);

    // A stick already pushed over when a lock is taken cannot flick the fresh target.
    if *last_lock != target {
        if last_lock.is_none() {
            flick.lock_taken(&flick_tuning, deflection);
        }
        *last_lock = target;
    }

    if lock_pressed {
        if let Some(held) = target {
            if let Some(mut intent) = world.get_mut::<Intent>(me) {
                intent.target = None;
            }
            // The player's cancel, the one way a lock goes that the check does not see.
            let line = format!("{} lock released: {held}", label(world, me));
            journal::emit(world, JournalKind::Local, line);
        } else if let Some(pick) = pick_in_view(world, me, &def, &camera, &settings, aspect) {
            if let Some(mut intent) = world.get_mut::<Intent>(me) {
                intent.target = Some(pick);
            }
            // The camera is input on this machine: its pick is a local line (a replay or a heavy
            // twin sets the same target from the recorded input, without it).
            let line = format!("{} locks {pick}", label(world, me));
            journal::emit(world, JournalKind::Local, line);
        }
        return;
    }
    // Both devices see every frame, so the mouse's sum drops when the lock goes and the stick
    // re-arms; the mouse wins a frame in which both flick, then the stick, then the wheel.
    let locked = target.is_some();
    let by_mouse = flick.mouse(&flick_tuning, px, frame_dt, locked);
    let by_stick = if by_mouse.is_some() { None } else { flick.stick(&flick_tuning, deflection, locked) };
    // The wheel: down (toward the player) is to the right, a tilt to the right is too; one
    // switch per turn of the wheel (`FlickState::wheel`). Asked every frame so it sees it stop.
    let notch = if wheel.y != 0.0 { -wheel.y } else { wheel.x };
    let by_wheel = flick.wheel(notch, locked, def.wheel_threshold);
    let Some(side) = by_mouse.or(by_stick).or(by_wheel) else { return };
    let found = switch_target(world, me, side, &def).is_some();
    if found && let Some(mut intent) = world.get_mut::<Intent>(me) {
        intent.pressed.push(Press { input: LOCK_SWITCH.to_owned(), dir: Some(Vec3::X * side.x()) });
    }
    flick.flicked(&flick_tuning, found);
}

/// Width over height of a perspective camera (16:9 when it has none).
fn aspect_of(projection: Option<&Projection>) -> f32 {
    match projection {
        Some(Projection::Perspective(p)) if p.aspect_ratio.is_finite() && p.aspect_ratio > 0.0 => p.aspect_ratio,
        _ => 16.0 / 9.0,
    }
}

/// The scored pick among what the camera sees: inside its frustum at the base field of view
/// (`def.frustum`) or the cone of `CameraSettings::lock_cone_deg`, with a clear ray from the eye
/// to the aim point; distance and look direction measured from the player.
fn pick_in_view(world: &World, me: Entity, def: &LockOnDef, camera: &Transform, settings: &CameraSettings, aspect: f32) -> Option<AiId> {
    let origin = world.get::<Transform>(me)?.translation;
    let physics = world.get_resource::<PhysicsWorld>();
    // The player's own field of view, never what a sprint adds on top of it (`CameraOffsets`).
    let fov_y = settings.fov_deg.clamp(30.0, 120.0).to_radians();
    let (eye, forward) = (camera.translation, *camera.forward());
    let cone = settings.lock_cone_deg.to_radians();
    let seen = |aim: Vec3| match def.frustum {
        true => in_frustum(eye, camera.rotation, fov_y, aspect, aim),
        false => (aim - eye).length_squared() > 1e-6 && angle(forward, aim - eye) <= cone,
    };
    let candidates: Vec<Lockable> = lock_candidates(world, me, def)
        .into_iter()
        .filter(|(e, c)| seen(c.aim) && physics.is_none_or(|p| sight_clear(p, eye, c.aim, &[me, *e])))
        .map(|(_, c)| c)
        .collect();
    let half_fov = if def.frustum { half_width(fov_y, aspect) } else { cone };
    let view = PickView { origin, eye, forward, look: Vec3::new(forward.x, 0.0, forward.z), half_fov };
    pick_lock(&view, def, &candidates).map(|i| AiId(candidates[i].key))
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
    shots: Option<Res<Shots>>,
    physics: Res<PhysicsWorld>,
    index: Res<AiIndex>,
    mut shake: ResMut<CameraShake>,
    players: Query<(Entity, &Intent), LocalPlayerFilter>,
    followed: Query<&Transform, Without<ThirdPersonCamera>>,
    lockable: LockOnTargets,
    points: Query<(), With<LockPoint>>,
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
    // A scene camera shot plays (K3): the orbit stands still, its yaw and pitch kept for the move axes.
    let in_shot = shots.as_ref().is_some_and(|s| s.playing().is_some());

    for (mut camera, mut tf, projection) in &mut cameras {
        let subject = if camera.target == Entity::PLACEHOLDER { player.map(|(e, _)| e) } else { Some(camera.target) };
        let Some(subject_tf) = subject.and_then(|e| followed.get(e).ok()) else {
            continue;
        };
        let locked = player
            .and_then(|(_, intent)| intent.target)
            .and_then(|id| index.get(id))
            .and_then(|e| followed.get(e).ok().map(|tf| (e, tf)))
            // A body is framed at its chest, a lock point on a body (a part) at its own centre.
            .map(|(e, tf)| {
                let at = shown_translation(e, tf, &children, &visuals);
                if points.contains(e) { at } else { chest_at(at) }
            });

        let subject_at = shown_translation(subject.unwrap_or(Entity::PLACEHOLDER), subject_tf, &children, &visuals);
        let head = subject_at + Vec3::Y * camera.height;
        let rate = 1.0 - ops::exp(-tuning.follow_rate.unwrap_or(camera.follow_rate) * dt);
        if camera.focus == Vec3::ZERO || camera.focus.distance(head) > tuning.snap_distance {
            // A jump (a teleport, a respawn): nothing of the old place to catch up from.
            camera.focus = head;
            camera.arm_reach = 0.0;
            camera.look_idle_s = 0.0;
        } else {
            camera.focus = camera.focus.lerp(head, rate);
        }
        // How fast the followed body walks on the ground, where it is drawn (m/s): from the body
        // itself, since the focus goes on catching up after it stops. A body that moved farther
        // than `body_snap_distance` since the last frame was placed (a teleport too short to
        // jump the focus): it did not walk, and the wait starts over.
        let seen = camera.subject_seen.replace(subject_at);
        let placed = seen.is_some_and(|was| was.distance(subject_at) > tuning.body_snap_distance);
        if placed {
            camera.look_idle_s = 0.0;
        }
        let ground = seen.map_or(0.0, |was| Vec2::new(subject_at.x - was.x, subject_at.z - was.z).length());
        let walk = if dt > 0.0 && !placed { ground / dt } else { 0.0 };

        match locked {
            _ if in_shot => camera.look_idle_s = 0.0,
            // Framing a target: behind the player, looking at it. The look axis flicks targets.
            Some(target) => {
                (camera.yaw, camera.pitch) = lock_framing(camera.yaw, camera.pitch, camera.focus, target, &tuning.framing, dt);
                camera.look_idle_s = 0.0;
            }
            // Free look: the player's turn, bent toward a target near the crosshair.
            None => {
                let up = if settings.invert_y { -1.0 } else { 1.0 };
                // Aiming turns by its own speed (K1): the game says when the player aims.
                let speed = if offsets.aiming { settings.aim_sensitivity } else { settings.sensitivity };
                let raw = state.axis("look");
                let look = raw * speed;
                let mut turn = Vec2::new(-look.x, -look.y * up);
                if let Some(error) = crosshair_error(&camera, &lockable).filter(|_| settings.aim_assist > 0.0) {
                    turn = aim_assist(turn, error, settings.aim_assist);
                }
                camera.yaw = wrap_angle(camera.yaw + turn.x);
                camera.pitch += turn.y;
                // Left alone, the view turns back behind a walking body (K2), when the player wants it.
                camera.look_idle_s = if raw == Vec2::ZERO { camera.look_idle_s + dt } else { 0.0 };
                if settings.auto_centre {
                    let facing = *subject_tf.forward();
                    (camera.yaw, camera.pitch) =
                        auto_centre(camera.yaw, camera.pitch, facing, walk, camera.look_idle_s, &tuning.auto_centre, dt);
                }
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
        // The player's own field of view plus what the game adds this frame (a sprint).
        let fov = (settings.fov_deg + finite_or_zero(offsets.fov_add_deg)).clamp(30.0, 120.0).to_radians();
        // Next to a big body the view turns up from the focus to keep a high lock point in
        // frame with the player (R4); without a fit it looks at the focus, as it always did.
        camera.tilt = match &tuning.framing.fit {
            Some(fit) => {
                let want = locked.map_or(0.0, |target| fit_tilt(at, focus, subject_at, target, fov * 0.5, fit));
                // Eased toward what the target wants, but held at once to what the feet and
                // `tilt_max` allow this frame (a wall that shortens the arm, a lower maximum).
                damp(camera.tilt, want, fit.rate, dt).min(fit_room(at, focus, subject_at, fov * 0.5, fit))
            }
            None => 0.0,
        };
        let mut looking = Transform::from_translation(at).looking_at(focus, Vec3::Y);
        if camera.tilt != 0.0 {
            looking.rotation *= Quat::from_rotation_x(camera.tilt);
        }
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
            && (perspective.fov - fov).abs() > 1e-5
        {
            perspective.fov = fov;
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
fn crosshair_error(camera: &ThirdPersonCamera, lockable: &LockOnTargets) -> Option<Vec2> {
    let eye = orbit_position(camera.focus, camera.yaw, camera.pitch, camera.distance);
    lockable
        .iter()
        .map(|(_, tf, point)| {
            let (yaw, pitch) = orbit_angles(eye - aim_point(tf, point));
            Vec2::new(wrap_angle(yaw - camera.yaw), pitch - camera.pitch)
        })
        .min_by(|a, b| a.length_squared().total_cmp(&b.length_squared()))
}
