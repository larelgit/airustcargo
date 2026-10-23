//! The view of a body, as agents use it (F1a): the `look` tool, and the `AIM` line of `observe`.
//!
//! `Intent::look` is where a character looks (yaw and pitch); `shards_motion::eye_of` and
//! `look_dir` turn it into a ray. This module adds what needs the combat module's bodies and
//! parts:
//!
//! - **`look`** (a player tool, so it acts through `Intent` and is recorded like any input):
//!   `look yaw=90 pitch=-10` (degrees), `look at=e7`, `look at=e7 part=head`, `look clear=true`.
//! - **`AIM`**, a line under `YOU` while the subject has a look: what the centre of its view
//!   meets, from the simulation's eye and look direction, never from a camera: `AIM e7 Dummy head
//!   14.2m`, `AIM e3 Pillar 3.1m`, `AIM wall 3.1m`, `AIM nothing within 100m`.
//! - [`view_hit`], the ray the line is made from, for whoever needs "what is the character looking
//!   at": a hit through a body's part is reported as that part.

use ai_core::prelude::*;
use ai_core::spatial::{compass, relative};
use ai_core::describe::label_of;
use ai_core::{AiAppExt, AiId};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_core::{Gaze, Intent};
use shards_motion::{aim_point_of, eye_of, look_dir, params_of, LayerMask, PhysicsWorld, QueryFilter};

use crate::fighter::Dead;
use crate::parts::{body_of, part_of, parts_of, Part};
use crate::shot::{walk_ray, Met};

/// How far the `AIM` line looks (m).
pub const AIM_RANGE: f32 = 100.0;

pub(crate) fn plugin(app: &mut App) {
    app.add_ai_tool("look", Scope::Player, look_tool)
        .add_ai_tool_guard("look", dead_refusal)
        .register_ai_observe_lines(aim_lines);
}

fn dead_refusal(world: &World, caller: &Caller) -> Result<(), String> {
    match caller.subject {
        Some(me) if world.get::<Dead>(me).is_some() => Err("you are dead; a dev agent can `heal` you".into()),
        _ => Ok(()),
    }
}

/// What the centre of a character's view meets ([`view_hit`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ViewHit {
    /// What was hit: the body, when the ray met one (through a part: the part's body), else the
    /// world collider.
    pub entity: Entity,
    /// The part of the body the ray went through, if it met one.
    pub part: Option<Entity>,
    /// Metres from the eye along the look.
    pub distance: f32,
    /// Where it met it.
    pub point: Vec3,
    /// The surface normal there, facing the eye.
    pub normal: Vec3,
}

/// What the centre of `viewer`'s view meets within `range` metres: a ray from its eye
/// ([`eye_of`]) along its look ([`look_dir`]) against the world and the bodies (character
/// colliders and hurtboxes), leaving out its own body and parts. A ray that meets a body with parts
/// looks for the part inside its capsule and names it; a body whose definition says its own
/// collider takes no hits (`body_hurtbox: false`) is looked through unless a part is met.
///
/// `None`: nothing in range, or `viewer` is no character with a position (no eye). Computed from the
/// simulation, so a headless run, a replay and a window agree.
pub fn view_hit(world: &World, viewer: Entity, range: f32) -> Option<ViewHit> {
    world.get_resource::<PhysicsWorld>()?;
    let eye = eye_of(world, viewer)?;
    let dir = Dir3::new(look_dir(world, viewer)?).ok()?;
    let mut filter = QueryFilter::new(LayerMask::WORLD | LayerMask::HURTBOX | LayerMask::CHARACTER).excluding(viewer);
    for part in parts_of(world, viewer) {
        filter = filter.excluding(part);
    }
    // The first thing met: a body (through a part, named so), else whatever collider it is.
    let mut seen = None;
    walk_ray(world, eye, dir, range, filter, |met, hit| {
        let (entity, part) = match met {
            Met::Body { body, part } => (body, part),
            Met::Solid | Met::Other => (hit.entity, None),
        };
        seen = Some(ViewHit { entity, part, distance: hit.distance, point: hit.point, normal: hit.normal });
        true
    });
    seen
}

/// What a hit is called in the `AIM` line: `e7 Dummy head` for a body (through a part), `e3 Pillar`
/// for any AI-visible thing, else by its surface: `floor`, `ceiling` or `wall`.
fn hit_name(world: &World, hit: &ViewHit) -> String {
    let named = world.get_entity(hit.entity).ok().and_then(|e| {
        let id = e.get::<AiId>()?;
        e.contains::<AiVisible>().then(|| format!("{id} {}", label_of(e)))
    });
    let mut name = named.unwrap_or_else(|| surface(hit.normal).to_owned());
    if let Some(part) = hit.part.and_then(|p| world.get::<Part>(p)) {
        name.push(' ');
        name.push_str(&part.id);
    }
    name
}

/// `floor`, `ceiling` or `wall`, by which way a surface faces.
pub(crate) fn surface(normal: Vec3) -> &'static str {
    match normal {
        n if n.y > 0.7 => "floor",
        n if n.y < -0.7 => "ceiling",
        _ => "wall",
    }
}

/// The `AIM` line for `viewer`: only while it has a look (`Intent::look`).
fn aim_lines(world: &World, viewer: Entity) -> Vec<String> {
    if world.get::<Intent>(viewer).is_none_or(|i| i.look.is_none()) || eye_of(world, viewer).is_none() {
        return Vec::new();
    }
    vec![match view_hit(world, viewer, AIM_RANGE) {
        Some(hit) => format!("AIM {} {:.1}m", hit_name(world, &hit), hit.distance),
        None => format!("AIM nothing within {AIM_RANGE:.0}m"),
    }]
}

/// Turn your view: where you look, which a first-person body also faces and aims its attacks by
/// (`observe` shows an `AIM` line while you have a look: what the centre of your view meets).
/// `look yaw=90 pitch=-10` turns to those angles in degrees (give one of them to keep the other);
/// `look at=e7` turns toward that entity's aim point and `look at=e7 part=head` toward the centre
/// of one of its body parts (`observe` lists the parts of a body); `look clear=true` stops looking
/// and the body turns by its movement again. A look is held until you change it.
#[derive(Deserialize, JsonSchema, Default)]
pub struct LookArgs {
    /// Heading in degrees. 0 looks north (-Z, the compass `N`), positive turns left: 90 is west,
    /// -90 east, 180 south. Left out: the heading you have.
    pub yaw: Option<f32>,
    /// Up and down in degrees. 0 is level, positive looks up. Held between the pitch limits of
    /// your body (-89 and 89 unless its `locomotion` says). Left out: the pitch you have.
    pub pitch: Option<f32>,
    /// Look at this entity (an id like `e7`) instead of giving angles.
    pub at: Option<Id>,
    /// With `at`: look at this part of its body (a name like `head`) instead of the body's aim point.
    pub part: Option<String>,
    /// `true`: stop looking (no other argument).
    pub clear: Option<bool>,
}

const USAGE: &str = "give yaw= and/or pitch= (degrees), or at=eN [part=NAME], or clear=true";

fn look_tool(In(call): In<Call<LookArgs>>, world: &mut World) -> ToolResult {
    let me = call.caller.require_subject()?;
    let LookArgs { yaw, pitch, at, part, clear } = call.args;
    if world.get::<Intent>(me).is_none() {
        return Err("your body cannot be controlled (it has no Intent)".into());
    }
    let Some(params) = params_of(world, me) else {
        return Err("your body has no eye to look from: it needs a character controller (CharacterController)".into());
    };
    let (lowest, highest) = params.pitch_limits_deg;
    for (name, value) in [("yaw", yaw), ("pitch", pitch)] {
        if value.is_some_and(|v| !v.is_finite()) {
            return Err(format!("{name} must be a finite number of degrees"));
        }
    }
    if clear == Some(true) {
        if yaw.is_some() || pitch.is_some() || at.is_some() || part.is_some() {
            return Err("clear=true stops looking and takes no other argument".into());
        }
        let mut intent = world.get_mut::<Intent>(me).expect("checked above");
        let note = if intent.look.take().is_some() { "ok: not looking; your body turns by its movement" } else { "ok: you were not looking" };
        return Ok(note.into());
    }
    if at.is_some() && (yaw.is_some() || pitch.is_some()) {
        return Err("give angles (yaw=, pitch=) or a target (at=), not both".into());
    }
    if part.is_some() && at.is_none() {
        return Err("part= names a part of the body at=eN looks at; give at= too".into());
    }
    let eye = eye_of(world, me).ok_or("your body has no position")?;
    let (gaze, what) = match at {
        Some(id) => {
            let target = id.entity(world)?;
            if body_of(world, target) == me {
                return Err("that is you; look at something else".into());
            }
            let (point, name) = point_of(world, target, part.as_deref())?;
            let gaze = Gaze::toward(eye, point).ok_or_else(|| format!("{name} is where your eye is; there is no direction to it"))?;
            // Straight above or below there is no heading: keep the one you have.
            let keeps = Vec2::new(point.x - eye.x, point.z - eye.z).length() <= 1e-6;
            let gaze = if keeps { Gaze { yaw: current(world, me).yaw, ..gaze } } else { gaze };
            (gaze, format!("at {name}, {}", relative(eye, point)))
        }
        None if yaw.is_none() && pitch.is_none() => return Err(format!("nothing to do: {USAGE}")),
        None => {
            let now = current(world, me);
            (Gaze::from_degrees(yaw.unwrap_or(now.yaw.to_degrees()), pitch.unwrap_or(now.pitch.to_degrees())), String::new())
        }
    };
    let held = params.clamp_look(gaze);
    // Heading in (-180, 180], so what `observe` shows stays short.
    let held = Gaze { yaw: wrap(held.yaw), pitch: held.pitch };
    let clamped = (held.pitch != gaze.pitch).then(|| format!(" (pitch held to your limits, {lowest}..{highest})"));
    let mut intent = world.get_mut::<Intent>(me).expect("checked above");
    if intent.look != Some(held) {
        intent.look = Some(held);
    }
    // `at e7 head, 3.2m NW` for a target, the compass point of the heading for angles.
    let what = if what.is_empty() { compass(held.dir()).to_owned() } else { what };
    Ok(format!(
        "ok: looking {what} (yaw {:.1}°, pitch {:.1}°){}",
        held.yaw.to_degrees(),
        held.pitch.to_degrees(),
        clamped.unwrap_or_default()
    ))
}

/// The look `me` has now, or the way its body faces, level.
fn current(world: &World, me: Entity) -> Gaze {
    world
        .get::<Intent>(me)
        .and_then(|i| i.look)
        .filter(|l| l.is_finite())
        .or_else(|| look_dir(world, me).and_then(|d| Gaze::toward(Vec3::ZERO, d)))
        .unwrap_or_default()
}

/// A yaw in radians brought into (-π, π].
pub(crate) fn wrap(yaw: f32) -> f32 {
    if yaw > -std::f32::consts::PI && yaw <= std::f32::consts::PI {
        return yaw;
    }
    let turned = (yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    if turned <= -std::f32::consts::PI { std::f32::consts::PI } else { turned }
}

/// The point to look at on `target`, with a name for the answer: the centre of its part `part`, the
/// centre of `target` itself when it is a part, else its aim point.
fn point_of(world: &World, target: Entity, part: Option<&str>) -> Result<(Vec3, String), String> {
    let who = |entity: Entity| world.get::<AiId>(entity).map_or_else(|| "it".to_owned(), ToString::to_string);
    // A part's collider is centred on its transform (the engine places it so).
    let centre = |entity: Entity| -> Result<Vec3, String> {
        world.get::<Transform>(entity).map(|tf| tf.translation).ok_or_else(|| format!("{} has no position", who(entity)))
    };
    let body = body_of(world, target);
    if let Some(name) = part {
        let found = part_of(world, body, name)?;
        return Ok((centre(found)?, format!("{} {name}", who(body))));
    }
    if let Some(p) = world.get::<Part>(target) {
        return Ok((centre(target)?, format!("{} {}", who(body), p.id)));
    }
    let point = aim_point_of(world, target).ok_or_else(|| format!("{} has no position", who(target)))?;
    Ok((point, who(target)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_surface_is_named_by_which_way_it_faces() {
        assert_eq!(surface(Vec3::Y), "floor");
        assert_eq!(surface(Vec3::NEG_Y), "ceiling");
        assert_eq!(surface(Vec3::X), "wall");
        assert_eq!(surface(Vec3::new(0.0, 0.5, 0.86).normalize()), "wall", "a steep slope is a wall");
    }

    #[test]
    fn a_heading_is_kept_within_a_half_turn() {
        let near = |a: f32, b: f32| (a - b).abs() < 1e-5;
        assert!(near(wrap(0.0), 0.0) && near(wrap(3.0), 3.0) && near(wrap(-3.0), -3.0));
        assert!(near(wrap(4.0), 4.0 - std::f32::consts::TAU) && near(wrap(-4.0), -4.0 + std::f32::consts::TAU));
        assert!(near(wrap(std::f32::consts::PI), std::f32::consts::PI), "a half turn stays a half turn");
        assert!(near(wrap(3.0 * std::f32::consts::PI), std::f32::consts::PI));
        assert!(near(wrap(std::f32::consts::TAU), 0.0));
    }
}
