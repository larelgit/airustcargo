//! The systems of the first-person camera (the `render` feature): see the module above for what they
//! do and the maths they call.

use std::collections::BTreeSet;

use ai_core::{journal, label, AiReplay, JournalKind};
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{Camera, Camera3d, Projection};
use bevy::light::{DirectionalLight, PointLight, SpotLight};
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::input_names::LOCK_ON;
use shards_core::{flat, Intent, LocalPlayer};

use super::{
    look_through, turn_look, view_rotation, EyeFrame, FirstPersonCamera, OWN_BODY_LAYER, VIEW_MODEL_LAYER,
};
use crate::camera::math::shake_offset_with;
use crate::camera::shot::Shots;
use crate::camera::systems::orbit_camera;
use crate::camera::{CameraOffsets, CameraSettings, CameraShake, CameraSystems, CameraTuning};
use crate::controller::{CharacterController, CharacterParams, Landed, MotionState};
use crate::input::{ControlState, ControlSystems};
use crate::interp::{shown_translation, InterpSystems, Interpolated};

/// The local player as the look system turns it: its look, and the numbers its pitch is held by.
type LocalBody<'w, 's> = Query<
    'w,
    's,
    (Entity, &'static mut Intent, &'static Transform, Option<&'static CharacterController>, Option<&'static CharacterParams>),
    With<LocalPlayer>,
>;

/// A body a first-person camera looks through, as the camera reads it.
type Subject<'w, 's> = Query<
    'w,
    's,
    (&'static Transform, Option<&'static Intent>, Option<&'static CharacterController>, Option<&'static CharacterParams>, Option<&'static MotionState>),
    Without<FirstPersonCamera>,
>;

/// The lights that carry no `RenderLayers`.
type PlainLights<'w, 's> =
    Query<'w, 's, Entity, (Or<(With<DirectionalLight>, With<PointLight>, With<SpotLight>)>, Without<RenderLayers>)>;

/// The 3D cameras that carry no `RenderLayers` and are not first-person ones.
type PlainCameras<'w, 's> = Query<'w, 's, Entity, (With<Camera3d>, Without<RenderLayers>, Without<FirstPersonCamera>)>;

pub(in crate::camera) fn plugin(app: &mut App) {
    app.add_observer(note_landing)
        .add_systems(PreUpdate, mirror_active.before(ControlSystems::Read))
        // After the input map (it turned the walk by the look of the last frame: the one frame the
        // third-person camera's yaw is behind by too) and before the tick, so a turn made in this frame
        // is the look the next tick reads.
        .add_systems(PreUpdate, first_person_look.after(ControlSystems::Read).run_if(local_first_person_view))
        .add_systems(PreUpdate, first_person_unlock.after(ControlSystems::Read).run_if(local_first_person_view))
        .add_systems(
            PostUpdate,
            first_person_camera
                .in_set(CameraSystems::Orbit)
                .after(InterpSystems::Blend)
                .before(bevy::transform::TransformSystems::Propagate)
                .before(orbit_camera),
        )
        .add_systems(
            PostUpdate,
            (hide_own_body, share_lights, other_cameras_see_the_body)
                .before(bevy::camera::visibility::VisibilitySystems::CheckVisibility),
        );
}

/// Run condition: an active first-person camera looks through the local player. A first-person camera of another
/// body (a spectator's view), or one that is switched off, leaves the local player to the orbit camera.
pub(in crate::camera) fn local_first_person_view(cameras: Query<&FirstPersonCamera>, players: Query<Entity, With<LocalPlayer>>) -> bool {
    let Ok(me) = players.single() else { return false };
    cameras.iter().any(|first| first.looks_through(me, true))
}

/// `PreUpdate`, before the input map: whether a camera is on, as the first-person camera knows it (the input map,
/// the orbit camera's lock-on and the ears read `FirstPersonCamera::active`, which exists without a renderer).
fn mirror_active(mut cameras: Query<(&Camera, &mut FirstPersonCamera)>) {
    for (camera, mut first) in &mut cameras {
        if first.active != camera.is_active {
            first.active = camera.is_active;
        }
    }
}

/// A landing of the body a camera looks through: the dip starts on the next frame.
fn note_landing(landed: On<Landed>, mut cameras: Query<&mut FirstPersonCamera>, players: Query<(), With<LocalPlayer>>) {
    let event = landed.event();
    for mut camera in &mut cameras {
        if camera.looks_through(event.entity, players.contains(event.entity)) {
            camera.eye.land(event.impact_speed);
        }
    }
}

/// `PreUpdate`, after the input map: this frame's `look` axis turns the local player's look.
///
/// It writes `Intent::look` only when the axis moved, and starts from whatever the intent holds
/// (else the body's heading), so a look written by the `look` tool or a recorded input is the one
/// the mouse turns on from, and no look is written on a frame nothing moved. A replay that plays
/// back (`AiReplay`) is the recording's, whatever the mouse does.
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
fn first_person_look(
    state: Res<ControlState>,
    settings: Res<CameraSettings>,
    offsets: Res<CameraOffsets>,
    replay: Option<Res<AiReplay>>,
    shots: Option<Res<Shots>>,
    db: Option<Res<ContentDb>>,
    cameras: Query<&FirstPersonCamera>,
    mut players: LocalBody,
) {
    let axis = state.axis("look");
    // A scene camera shot (K3) holds the view: the look input turns nothing until it ends, as the orbit camera's
    // does not, and a turn made meanwhile must not show when the shot is over. A replay that plays is the recording's.
    if axis == Vec2::ZERO || replay.is_some_and(|r| r.is_playing()) || shots.is_some_and(|s| s.playing().is_some()) {
        return;
    }
    let Ok((me, mut intent, tf, controller, own)) = players.single_mut() else { return };
    // Only a camera that looks through the local player turns its look.
    if !cameras.iter().any(|c| c.looks_through(me, true)) {
        return;
    }
    let params = controller.map(|c| c.resolve(own, db.as_deref()));
    let limits = params.map_or_else(|| CharacterParams::default().pitch_limits(), CharacterParams::pitch_limits);
    // Aiming turns by its own speed (K1): the game says when the player aims.
    let speed = if offsets.aiming { settings.aim_sensitivity } else { settings.sensitivity };
    let base = look_through(Some(&intent), params, Some(tf));
    let next = turn_look(base, axis, speed, settings.invert_y, limits);
    if next.is_finite() && intent.look != Some(next) {
        intent.look = Some(next);
    }
}

/// `PreUpdate`, after the input map: the `lock_on` button lets go of a lock the local player holds, as it does
/// under the orbit camera (a local line `e5 lock released: e6`). A first-person camera picks and switches
/// nothing, the lock-on machinery of the orbit camera being off, but a lock that is there (one the orbit camera
/// took before the mode changed, one a tool or a brain set) must not be stuck to the player: the simulation's
/// own checks still let go of one that is no longer valid, and the `lock_on` tool still releases.
fn first_person_unlock(world: &mut World, players: &mut QueryState<Entity, (With<LocalPlayer>, With<Intent>)>) {
    // A replay that plays is the recording's: a live press is no input of it, and the lock would only come back.
    // A scene shot (K3) holds the view and the orbit camera's lock-on waits for it too.
    if world.get_resource::<AiReplay>().is_some_and(AiReplay::is_playing)
        || world.get_resource::<Shots>().is_some_and(|shots| shots.playing().is_some())
        || !world.get_resource::<ControlState>().is_some_and(|state| state.just_pressed(LOCK_ON))
    {
        return;
    }
    let Ok(me) = players.single(world) else { return };
    let Some(held) = world.get::<Intent>(me).and_then(|intent| intent.target) else { return };
    if let Some(mut intent) = world.get_mut::<Intent>(me) {
        intent.target = None;
    }
    let line = format!("{} lock released: {held}", label(world, me));
    journal::emit(world, JournalKind::Local, line);
}

/// A number a game system wrote into [`CameraOffsets`] that is not finite counts as 0.
fn finite_or_zero(x: f32) -> f32 {
    if x.is_finite() { x } else { 0.0 }
}

/// `PostUpdate`, before transforms propagate and before the orbit camera (which drains the shake
/// after this has read it): the camera at the eye of the body it looks through, turned by its look.
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
fn first_person_camera(
    time: Res<Time<Real>>,
    settings: Res<CameraSettings>,
    tuning: Res<CameraTuning>,
    offsets: Res<CameraOffsets>,
    shake: Res<CameraShake>,
    shots: Option<Res<Shots>>,
    db: Option<Res<ContentDb>>,
    players: Query<Entity, With<LocalPlayer>>,
    bodies: Subject,
    children: Query<&Children>,
    visuals: Query<&Interpolated>,
    mut cameras: Query<(&mut FirstPersonCamera, &mut Transform, Option<&mut Projection>)>,
) {
    // A scene camera shot plays (K3): it has the camera.
    if shots.as_ref().is_some_and(|s| s.playing().is_some()) {
        return;
    }
    let dt = time.delta_secs();
    let player = players.single().ok();
    let trauma = if settings.shake { shake.trauma } else { 0.0 };
    let wobble = shake_offset_with(trauma, time.elapsed_secs(), &tuning.trauma);
    for (mut camera, mut tf, projection) in &mut cameras {
        let subject = if camera.target == Entity::PLACEHOLDER { player } else { Some(camera.target) };
        let Some((subject, (body_tf, intent, controller, own, motion))) = subject.and_then(|e| bodies.get(e).ok().map(|b| (e, b))) else {
            continue;
        };
        let params = controller.map(|c| c.resolve(own, db.as_deref()));
        let eye_above = params.map_or_else(|| CharacterParams::default().eye_above_feet(), CharacterParams::eye_above_feet);
        // Where the body is drawn this frame (between ticks too), not where the tick left it.
        let feet = shown_translation(subject, body_tf, &children, &visuals);
        let seen = camera.subject_seen.replace(feet);
        let placed = seen.is_some_and(|was| was.distance(feet) > tuning.body_snap_distance);

        let look = look_through(intent, params, Some(body_tf));
        let turn = view_rotation(look, 0.0);
        let right = flat(turn * Vec3::X).try_normalize().unwrap_or(Vec3::X);
        let (grounded, velocity) = motion.map_or((true, Vec3::ZERO), |m| (m.grounded, m.velocity));
        let shift = camera.eye.step(
            &tuning.first_person,
            EyeFrame {
                eye_y: feet.y + eye_above,
                placed,
                grounded,
                speed: Vec2::new(velocity.x, velocity.z).length(),
                side_speed: velocity.dot(right),
                dt,
            },
        );
        let at = Vec3::new(feet.x, feet.y + eye_above + shift.lift - finite_or_zero(offsets.focus_dip_m), feet.z) + right * shift.sway;
        let looking = view_rotation(look, shift.roll);
        let shaken = Quat::from_euler(EulerRot::YXZ, wobble.rotation.y, wobble.rotation.x, wobble.rotation.z);
        let next = Transform { translation: at + looking * wobble.translation, rotation: looking * shaken, scale: tf.scale };
        if *tf != next && next.translation.is_finite() && next.rotation.is_finite() {
            *tf = next;
        }
        // The player's own field of view plus what the game adds this frame (a sprint).
        let fov = (settings.fov_deg + finite_or_zero(offsets.fov_add_deg)).clamp(30.0, 120.0).to_radians();
        if let Some(mut projection) = projection
            && let Projection::Perspective(perspective) = &mut *projection
            && (perspective.fov - fov).abs() > 1e-5
        {
            perspective.fov = fov;
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Layers: who draws what
// ---------------------------------------------------------------------------------------------

/// A mesh of the body a first-person camera looks through, on [`OWN_BODY_LAYER`]; what it was on
/// before (`None`: it carried no `RenderLayers`, the default layer) goes back when the camera stops.
#[derive(Component, Clone, Debug)]
pub struct OwnBody(pub Option<RenderLayers>);

/// A light the engine gave the first-person layers because the game gave it none.
#[derive(Component, Clone, Copy, Debug)]
struct SharedLight;

/// A 3D camera the engine let see the own body, because the game gave it no layers.
#[derive(Component, Clone, Copy, Debug)]
struct SeesOwnBody;

/// The layers the first-person lights carry: the world's, the own body's (so it still casts its
/// shadow) and the view model's (so it is lit).
fn light_layers() -> RenderLayers {
    RenderLayers::from_layers(&[0, OWN_BODY_LAYER, VIEW_MODEL_LAYER])
}

/// The bodies the first-person cameras look through.
fn subjects(cameras: &Query<&FirstPersonCamera>, players: &Query<Entity, With<LocalPlayer>>) -> BTreeSet<Entity> {
    let player = players.single().ok();
    cameras.iter().filter(|c| c.active).filter_map(|c| if c.target == Entity::PLACEHOLDER { player } else { Some(c.target) }).collect()
}

/// `PostUpdate`, before visibility is worked out: every mesh under the body a first-person camera
/// looks through moves to [`OWN_BODY_LAYER`], the ones that appear later too (a model that finishes
/// loading, a prop the bridge mounts); a mesh of a body no camera looks through any more, or of a
/// camera that is gone, goes back to the layers it had. The camera draws layer 0 only, so it never
/// sees them, while a camera that has the layer (or a shadow map of a light that has it) does.
fn hide_own_body(
    mut commands: Commands,
    cameras: Query<&FirstPersonCamera>,
    players: Query<Entity, With<LocalPlayer>>,
    children: Query<&Children>,
    meshes: Query<Option<&RenderLayers>, (With<Mesh3d>, Without<OwnBody>)>,
    hidden: Query<(Entity, &OwnBody)>,
) {
    if cameras.is_empty() && hidden.is_empty() {
        return;
    }
    let mut want: BTreeSet<Entity> = BTreeSet::new();
    let mut stack: Vec<Entity> = subjects(&cameras, &players).into_iter().collect();
    while let Some(entity) = stack.pop() {
        if want.insert(entity)
            && let Ok(kids) = children.get(entity)
        {
            stack.extend(kids.iter());
        }
    }
    let own = RenderLayers::layer(OWN_BODY_LAYER);
    for &entity in &want {
        if let Ok(before) = meshes.get(entity) {
            commands.entity(entity).insert((OwnBody(before.cloned()), own.clone()));
        }
    }
    for (entity, body) in &hidden {
        if !want.contains(&entity) {
            let mut restored = commands.entity(entity);
            match &body.0 {
                Some(before) => {
                    restored.insert(before.clone());
                }
                None => {
                    restored.remove::<RenderLayers>();
                }
            }
            restored.remove::<OwnBody>();
        }
    }
}

/// `PostUpdate`: while a first-person camera exists, a light that carries no `RenderLayers` gets
/// the world's, the own body's and the view model's, so it lights the arms and the body's shadow falls
/// (a shadow caster counts only for a light that shares its layer). A light the game gave layers is
/// left as it is: it then needs `[0, OWN_BODY_LAYER, VIEW_MODEL_LAYER]` to do the same. When the
/// camera goes the lights get their default back.
fn share_lights(
    mut commands: Commands,
    cameras: Query<(), With<FirstPersonCamera>>,
    plain: PlainLights,
    shared: Query<(Entity, &RenderLayers), With<SharedLight>>,
) {
    if cameras.is_empty() {
        let ours = light_layers();
        for (entity, layers) in &shared {
            // One the game changed since is the game's.
            if *layers == ours {
                commands.entity(entity).remove::<RenderLayers>();
            }
            commands.entity(entity).remove::<SharedLight>();
        }
        return;
    }
    for entity in &plain {
        commands.entity(entity).insert((light_layers(), SharedLight));
    }
}

/// `PostUpdate`: while a first-person camera exists, a 3D camera that carries no `RenderLayers` (not
/// the first-person camera, which draws the world alone, and not the view-model camera, which has its
/// own) also draws [`OWN_BODY_LAYER`]: another view of the world, a mirror, a minimap, shows the body. What
/// such a camera was given is taken back when the first-person cameras are gone, and when the camera becomes a
/// first-person one itself (`mode: FirstPerson` on an orbit camera): that one draws the world alone.
fn other_cameras_see_the_body(
    mut commands: Commands,
    first: Query<(), With<FirstPersonCamera>>,
    plain: PlainCameras,
    seeing: Query<(Entity, &RenderLayers, Has<FirstPersonCamera>), With<SeesOwnBody>>,
) {
    let ours = RenderLayers::from_layers(&[0, OWN_BODY_LAYER]);
    for (entity, layers, is_first_person) in &seeing {
        if first.is_empty() || is_first_person {
            if *layers == ours {
                commands.entity(entity).remove::<RenderLayers>();
            }
            commands.entity(entity).remove::<SeesOwnBody>();
        }
    }
    if first.is_empty() {
        return;
    }
    for entity in &plain {
        commands.entity(entity).insert((RenderLayers::from_layers(&[0, OWN_BODY_LAYER]), SeesOwnBody));
    }
}
