//! Lock-on rules (C6) as data and as plain functions: the content kind `lock_on`
//! (`*.lock_on.ron`, the id `default`), the scored pick, the switch to a neighbour, the aim
//! point on a body, the frustum test and the line of sight. No systems: the render camera
//! (its pick, its flick) and the action module (the `lock_on` tool, the `lock_switch` press,
//! the check of every lock on every tick) call the same functions, so a window and a headless
//! run decide by one rule.
//!
//! **Without a `lock_on` definition none of this is in force**: the camera picks, switches and
//! lets go as it always did (`math::pick_lock_target`, `next_lock_target`, `lock_holds`), and
//! nothing checks a lock somebody else set. A game opts in with one file:
//!
//! ```ron
//! // assets/content/combat/lock.lock_on.ron
//! { "default": (range: 30.0, obstruction_s: Some(2.0), weights: (distance: 1.0, screen: 1.0)) }
//! ```
//!
//! **One lock point per body**: the aim point is `aim_height` of the body's height above its
//! feet. Several points on one body (a boss's head, a limb) are not supported.

use std::f32::consts::{FRAC_PI_2, PI};

use ai_core::{AiId, AiIndex};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::{ContentDb, ContentDef, Refs};
use shards_core::{flat, Intent};

use super::math::Side;
use super::LockOnTarget;
use crate::physics::{Collider, LayerMask, PhysicsWorld, QueryFilter};

/// A body without a collider is taken to be this tall (m) for its aim point and its eye.
pub const DEFAULT_BODY_HEIGHT: f32 = 1.8;

macro_rules! is_default {
    ($name:ident, $ty:ty, $value:expr) => {
        fn $name(v: &$ty) -> bool {
            *v == $value
        }
    };
}

is_default!(one, f32, 1.0);
is_default!(view_is_default, f32, 0.5);
is_default!(danger_is_default, f32, 0.5);
is_default!(current_is_default, f32, 0.3);
is_default!(range_is_default, f32, 30.0);
is_default!(aim_is_default, f32, 0.6);
is_default!(eye_is_default, f32, 0.9);
is_default!(obstruction_is_default, Option<f32>, Some(2.0));
is_default!(dead_band_is_default, f32, 0.05);
is_default!(min_distance_is_default, f32, 0.1);

fn is_true(v: &bool) -> bool {
    *v
}

fn yes() -> bool {
    true
}

/// How a scored pick weighs what it knows of a candidate. The lowest score wins; every term is
/// made comparable first (a fraction of the range, of half the view, of a half turn), so a
/// weight says how much that term counts against the others.
#[derive(Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[reflect(Default)]
#[serde(default, deny_unknown_fields)]
pub struct LockWeights {
    /// Distance from the one locking, per `range`: a candidate at the edge of the range scores
    /// this much more than one at its feet.
    #[serde(skip_serializing_if = "one")]
    pub distance: f32,
    /// Angle between the centre of the view and the candidate's aim point, per half the view's
    /// width (the camera's, or a half turn for a body's own eyes).
    #[serde(skip_serializing_if = "one")]
    pub screen: f32,
    /// Angle on the ground between the way the one locking looks and the way to the candidate,
    /// from the one locking, per half turn: straight ahead 0, behind this much.
    #[serde(skip_serializing_if = "view_is_default")]
    pub view: f32,
    /// Taken off for a candidate that is attacking the one locking now (times how dangerous,
    /// 0..1: one of its hit zones would land on it).
    #[serde(skip_serializing_if = "danger_is_default")]
    pub danger: f32,
    /// Taken off for the current target: a pick made while a lock is held keeps it unless
    /// another candidate is clearly better.
    #[serde(skip_serializing_if = "current_is_default")]
    pub current: f32,
}

impl Default for LockWeights {
    fn default() -> Self {
        Self { distance: 1.0, screen: 1.0, view: 0.5, danger: 0.5, current: 0.3 }
    }
}

/// The lock-on's numbers. Content: `<assets>/content/**/*.lock_on.ron`, one map of ids to
/// definitions; the engine reads the id `default` (hot-reloadable). Its presence switches the
/// rules on (see the module docs); missing fields take the defaults, unknown ones are an error.
#[derive(Reflect, Serialize, Deserialize, Clone, Debug, PartialEq)]
#[reflect(Default)]
#[serde(default, deny_unknown_fields)]
pub struct LockOnDef {
    /// How far a lock reaches (m, in 3D between the two bodies' positions): the pick and the
    /// switch look this far, and a lock on a target farther than this is lost.
    #[serde(skip_serializing_if = "range_is_default")]
    pub range: f32,
    /// What the scored pick weighs.
    #[serde(skip_serializing_if = "LockWeights::is_default")]
    pub weights: LockWeights,
    /// The aim point: this fraction of the target's height above its feet (0 the feet, 1 the
    /// top of its collider). The rays, the screen and the frustum aim there.
    #[serde(skip_serializing_if = "aim_is_default")]
    pub aim_height: f32,
    /// Where the one locking looks from for its rays: this fraction of its own height.
    #[serde(skip_serializing_if = "eye_is_default")]
    pub eye_height: f32,
    /// A lock whose target stays hidden behind the world (a ray from the eye to the aim point)
    /// this long without a break (s; counted in ticks, so 2.0 is 120 ticks at 60 Hz) is lost.
    /// `None`: obstruction never breaks a lock.
    #[serde(skip_serializing_if = "obstruction_is_default")]
    pub obstruction_s: Option<f32>,
    /// A switch takes a candidate only further than this (rad) from the current target on the
    /// side asked for, measured from the one locking.
    #[serde(skip_serializing_if = "dead_band_is_default")]
    pub switch_dead_band: f32,
    /// Candidates nearer than this to the one locking on the ground (m) have no direction from
    /// it, so a switch never goes to them.
    #[serde(skip_serializing_if = "min_distance_is_default")]
    pub switch_min_distance: f32,
    /// The camera picks inside its view frustum at the player's base field of view
    /// (`CameraSettings::fov_deg`; a sprint's `CameraOffsets::fov_add_deg` does not widen what
    /// can be locked). `false`: inside the cone `CameraSettings::lock_cone_deg`.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub frustum: bool,
}

impl Default for LockOnDef {
    fn default() -> Self {
        Self {
            range: 30.0,
            weights: LockWeights::default(),
            aim_height: 0.6,
            eye_height: 0.9,
            obstruction_s: Some(2.0),
            switch_dead_band: 0.05,
            switch_min_distance: 0.1,
            frustum: true,
        }
    }
}

impl LockWeights {
    fn is_default(&self) -> bool {
        *self == Self::default()
    }
}

impl LockOnDef {
    /// The id the engine reads.
    pub const ID: &'static str = "default";

    /// The definition in force, if the game has one: the rules are on only then.
    pub fn active(db: Option<&ContentDb>) -> Option<&LockOnDef> {
        db?.get::<LockOnDef>(Self::ID)
    }

    /// Ticks of `tick_s` seconds that make `obstruction_s` (at least 1); `None` without it.
    pub fn obstruction_ticks(&self, tick_s: f32) -> Option<u32> {
        let s = self.obstruction_s?;
        Some(((s / tick_s.max(1e-6)).round() as u32).max(1))
    }

    /// What is wrong with these numbers, one line each; empty when they are fine.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let finite = |name: &str, v: f32, out: &mut Vec<String>| {
            let ok = v.is_finite();
            if !ok {
                out.push(format!("{name} is {v}, it must be a finite number"));
            }
            ok
        };
        if finite("range", self.range, &mut out) && self.range <= 0.0 {
            out.push(format!("range is {}, it must be above 0 (m)", self.range));
        }
        let w = &self.weights;
        for (name, v) in [("distance", w.distance), ("screen", w.screen), ("view", w.view), ("danger", w.danger), ("current", w.current)] {
            if finite(&format!("weights.{name}"), v, &mut out) && v < 0.0 {
                out.push(format!("weights.{name} is {v}, a weight cannot be negative (0 leaves the term out)"));
            }
        }
        for (name, v) in [("aim_height", self.aim_height), ("eye_height", self.eye_height)] {
            if finite(name, v, &mut out) && !(0.0..=1.0).contains(&v) {
                out.push(format!("{name} is {v}, it is a fraction of the body's height: 0..1"));
            }
        }
        if let Some(s) = self.obstruction_s
            && finite("obstruction_s", s, &mut out)
            && s <= 0.0
        {
            out.push(format!("obstruction_s is {s}, it must be above 0 (leave it None for no obstruction rule)"));
        }
        if finite("switch_dead_band", self.switch_dead_band, &mut out) && !(0.0..FRAC_PI_2).contains(&self.switch_dead_band) {
            out.push(format!("switch_dead_band is {} rad, it must be in 0..π/2", self.switch_dead_band));
        }
        if finite("switch_min_distance", self.switch_min_distance, &mut out) && self.switch_min_distance < 0.0 {
            out.push(format!("switch_min_distance is {}, it cannot be negative", self.switch_min_distance));
        }
        out
    }
}

impl ContentDef for LockOnDef {
    const KIND: &'static str = "lock_on";

    fn validate(&self, id: &str, refs: &mut Refs) {
        if id != Self::ID {
            refs.error(format!("the engine reads only the lock-on rules `{}`; rename this one (under another id they are never in force)", Self::ID));
        }
        for problem in self.problems() {
            refs.error(problem);
        }
    }
}

/// What the lock-on asks of the modules above motion. The action module fills it in (who is
/// dead, who is hostile, whose attack is about to land); without it a body may lock on to what
/// carries [`LockOnTarget`], and nothing is dangerous.
#[derive(Resource, Clone, Copy)]
pub struct LockHooks {
    /// Whether `viewer` may lock on to `candidate` now (the candidate exists).
    pub lockable: fn(&World, Entity, Entity) -> bool,
    /// What endangers `viewer` now, and how much (0..1), for the pick's `danger` term.
    pub danger: fn(&World, Entity) -> Vec<(Entity, f32)>,
}

impl Default for LockHooks {
    fn default() -> Self {
        Self { lockable: |world, viewer, candidate| viewer != candidate && world.get::<LockOnTarget>(candidate).is_some(), danger: |_, _| Vec::new() }
    }
}

/// A body the pick or the switch may take, as they see it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Lockable {
    /// The key ties are broken by (the `AiId` bits): the lower wins.
    pub key: u32,
    /// Its position (the feet): ranges are measured between positions.
    pub feet: Vec3,
    /// Its aim point ([`LockOnDef::aim_height`] of its body).
    pub aim: Vec3,
    /// How much it endangers the one locking now (0..1).
    pub danger: f32,
    /// It is the current target.
    pub current: bool,
}

/// Where a scored pick looks from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PickView {
    /// The one locking (its feet): the distance and the look direction are measured from here.
    pub origin: Vec3,
    /// The eye the screen is seen from (the camera's, or the body's own) ...
    pub eye: Vec3,
    /// ... and the way it looks.
    pub forward: Vec3,
    /// The way the one locking looks on the ground (the camera's yaw, or the body's facing).
    pub look: Vec3,
    /// Half the width of the view (rad): the unit of the `screen` term. A body's own eyes see a
    /// half turn either way.
    pub half_fov: f32,
}

impl PickView {
    /// A body that looks with its own eyes: from `eye` along `facing`, no screen edge.
    pub fn body(origin: Vec3, eye: Vec3, facing: Vec3) -> Self {
        Self { origin, eye, forward: facing, look: facing, half_fov: PI }
    }
}

/// The angle between two directions, 0..π (0 when either has no length): `ops::atan2` of the
/// cross and the dot, as the engine's maths goes, not glam's `angle_between` (an approximated
/// `acos`, which also loses digits near 0 and π).
pub fn angle(a: Vec3, b: Vec3) -> f32 {
    if a.length_squared() < 1e-12 || b.length_squared() < 1e-12 { 0.0 } else { ops::atan2(a.cross(b).length(), a.dot(b)) }
}

/// The score of `c` (lower is better): distance per range, screen angle per half view, look
/// angle per half turn, less danger and the current-target bonus, each times its weight.
pub fn lock_score(view: &PickView, def: &LockOnDef, c: &Lockable) -> f32 {
    let w = &def.weights;
    let distance = c.feet.distance(view.origin) / def.range.max(1e-3);
    let screen = angle(view.forward, c.aim - view.eye) / view.half_fov.max(1e-3);
    let look = angle(flat(view.look), flat(c.feet - view.origin)) / PI;
    let current = if c.current { 1.0 } else { 0.0 };
    w.distance * distance + w.screen * screen + w.view * look - w.danger * c.danger.clamp(0.0, 1.0) - w.current * current
}

/// The scored pick: of the candidates within `def.range` of the one locking, the lowest
/// [`lock_score`], the lower key on a tie. The caller has already kept only what it can see
/// (in the frustum, a clear ray). Returns its index.
pub fn pick_lock(view: &PickView, def: &LockOnDef, candidates: &[Lockable]) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| c.feet.distance(view.origin) <= def.range)
        .map(|(i, c)| (i, lock_score(view, def, c), c.key))
        .min_by(|a, b| a.1.total_cmp(&b.1).then(a.2.cmp(&b.2)))
        .map(|(i, ..)| i)
}

/// The signed angle on the ground, seen from `origin`, from the direction to `from` to the
/// direction to `to`: positive to the left (counter-clockwise from above, +Y up, -Z ahead).
/// `None` when either is right above or below `origin`.
pub fn signed_bearing(origin: Vec3, from: Vec3, to: Vec3) -> Option<f32> {
    let (a, b) = (flat(from - origin), flat(to - origin));
    if a.length_squared() < 1e-12 || b.length_squared() < 1e-12 {
        return None;
    }
    Some(ops::atan2(a.cross(b).y, a.dot(b)))
}

/// The switch from the current target (at `current`, its position) to the `side`: of the
/// candidates within `def.range` and at least `switch_min_distance` from the one locking on
/// the ground, those past the dead band on that side (measured from `origin`, the one locking),
/// the one at the smallest angle, the lower key on a tie. The current target itself never
/// qualifies (its angle is 0). The caller has already kept only what it can see.
pub fn switch_lock(origin: Vec3, current: Vec3, candidates: &[Lockable], side: Side, def: &LockOnDef) -> Option<usize> {
    candidates
        .iter()
        .enumerate()
        .filter(|(_, c)| !c.current && c.feet.distance(origin) <= def.range)
        .filter(|(_, c)| flat(c.feet - origin).length() >= def.switch_min_distance)
        .filter_map(|(i, c)| Some((i, signed_bearing(origin, current, c.feet)?, c.key)))
        .filter(|(_, angle, _)| match side {
            Side::Left => *angle > def.switch_dead_band,
            Side::Right => *angle < -def.switch_dead_band,
        })
        .min_by(|a, b| a.1.abs().total_cmp(&b.1.abs()).then(a.2.cmp(&b.2)))
        .map(|(i, ..)| i)
}

/// The point `fraction` of a body's height above its bottom: its collider's lowest point to
/// its highest (the collider turned with the body), or [`DEFAULT_BODY_HEIGHT`] above `tf` for
/// a body without one.
pub fn body_point(tf: &Transform, collider: Option<&Collider>, fraction: f32) -> Vec3 {
    match collider {
        Some(c) => {
            let center = tf.translation + tf.rotation * c.offset;
            let half = c.shape.half_extents().y;
            center + Vec3::Y * (-half + 2.0 * half * fraction)
        }
        None => tf.translation + Vec3::Y * DEFAULT_BODY_HEIGHT * fraction,
    }
}

/// Whether `point` is inside the view frustum of a camera at `eye` turned by `rotation` (looking
/// along its -Z), with vertical field of view `fov_y` (rad) and width over height `aspect`.
pub fn in_frustum(eye: Vec3, rotation: Quat, fov_y: f32, aspect: f32, point: Vec3) -> bool {
    let local = rotation.inverse() * (point - eye);
    let depth = -local.z;
    if depth <= 1e-3 {
        return false;
    }
    let tan_y = ops::tan(fov_y * 0.5);
    let tan_x = tan_y * aspect.max(1e-3);
    local.x.abs() <= depth * tan_x && local.y.abs() <= depth * tan_y
}

/// Half the width of a view with vertical field of view `fov_y` and `aspect` (rad).
pub fn half_width(fov_y: f32, aspect: f32) -> f32 {
    ops::atan(ops::tan(fov_y * 0.5) * aspect.max(1e-3))
}

/// Nothing of the world layer between two points; the `ignore` entities (the two bodies) never
/// block, and a hit at the very end is the target's own surface.
pub fn sight_clear(physics: &PhysicsWorld, from: Vec3, to: Vec3, ignore: &[Entity]) -> bool {
    let delta = to - from;
    let distance = delta.length();
    let Ok(dir) = Dir3::new(delta) else { return true };
    let filter = ignore.iter().fold(QueryFilter::new(LayerMask::WORLD), |f, e| f.excluding(*e));
    physics.raycast(from, dir, distance, &filter).is_none_or(|hit| hit.distance >= distance - 0.01)
}

/// A body's eye for its lock rays ([`LockOnDef::eye_height`]).
pub fn eye_of(world: &World, body: Entity, def: &LockOnDef) -> Option<Vec3> {
    let tf = world.get::<Transform>(body)?;
    Some(body_point(tf, world.get::<Collider>(body), def.eye_height))
}

/// A body's aim point ([`LockOnDef::aim_height`]).
pub fn aim_of(world: &World, body: Entity, def: &LockOnDef) -> Option<Vec3> {
    let tf = world.get::<Transform>(body)?;
    Some(body_point(tf, world.get::<Collider>(body), def.aim_height))
}

/// Every body `viewer` may lock on to now ([`LockHooks::lockable`]), in `AiId` order, within
/// `def.range` of it, with its aim point, its danger to `viewer` and whether it is the current
/// target. What it can see is the caller's to decide (a ray from the camera or from the body).
pub fn lock_candidates(world: &World, viewer: Entity, def: &LockOnDef) -> Vec<(Entity, Lockable)> {
    let hooks = world.get_resource::<LockHooks>().copied().unwrap_or_default();
    let Some(origin) = world.get::<Transform>(viewer).map(|t| t.translation) else { return Vec::new() };
    let current = world.get::<Intent>(viewer).and_then(|i| i.target);
    let danger = (hooks.danger)(world, viewer);
    let Some(mut q) = world.try_query::<(Entity, &AiId, &Transform, Option<&Collider>)>() else { return Vec::new() };
    let mut out: Vec<(Entity, Lockable)> = q
        .iter(world)
        .filter(|(e, _, tf, _)| *e != viewer && tf.translation.distance(origin) <= def.range)
        .filter(|(e, ..)| (hooks.lockable)(world, viewer, *e))
        .map(|(e, id, tf, collider)| {
            let threat = danger.iter().filter(|(d, _)| *d == e).map(|(_, v)| *v).fold(0.0, f32::max);
            let c = Lockable { key: id.0, feet: tf.translation, aim: body_point(tf, collider, def.aim_height), danger: threat, current: current == Some(*id) };
            (e, c)
        })
        .collect();
    out.sort_by_key(|(_, c)| c.key);
    out
}

/// The switch of `viewer`'s lock to the `side`, by the rules of `def`: the candidates it may lock
/// on to and sees from its own eye ([`sight_clear`]), measured from its position. `None` when it
/// holds no lock, its target is gone, or nobody is on that side.
pub fn switch_target(world: &World, viewer: Entity, side: Side, def: &LockOnDef) -> Option<AiId> {
    let current = world.get::<Intent>(viewer)?.target?;
    let target = world.get_resource::<AiIndex>()?.get(current)?;
    let at = world.get::<Transform>(target)?.translation;
    let origin = world.get::<Transform>(viewer)?.translation;
    let eye = eye_of(world, viewer, def)?;
    let physics = world.get_resource::<PhysicsWorld>();
    let seen: Vec<(Entity, Lockable)> = lock_candidates(world, viewer, def)
        .into_iter()
        .filter(|(e, c)| physics.is_none_or(|p| sight_clear(p, eye, c.aim, &[viewer, *e])))
        .collect();
    let plain: Vec<Lockable> = seen.iter().map(|(_, c)| Lockable { current: c.key == current.0, ..*c }).collect();
    switch_lock(origin, at, &plain, side, def).map(|i| AiId(plain[i].key))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::Shape;

    fn at(key: u32, x: f32, z: f32) -> Lockable {
        let feet = Vec3::new(x, 0.0, z);
        Lockable { key, feet, aim: feet + Vec3::Y, danger: 0.0, current: false }
    }

    fn view() -> PickView {
        PickView { origin: Vec3::ZERO, eye: Vec3::new(0.0, 1.5, 4.0), forward: Vec3::NEG_Z, look: Vec3::NEG_Z, half_fov: 0.9 }
    }

    #[test]
    fn the_defaults_validate_and_serialize_to_nothing() {
        let def = LockOnDef::default();
        assert!(def.problems().is_empty(), "{:?}", def.problems());
        assert_eq!(ron::to_string(&def).unwrap(), "()");
        assert_eq!(ron::from_str::<LockOnDef>("()").unwrap(), def);
        let set: LockOnDef = ron::from_str("(range: 25.0, weights: (danger: 2.0), frustum: false)").unwrap();
        assert_eq!((set.range, set.weights.danger, set.weights.distance, set.frustum), (25.0, 2.0, 1.0, false));
        assert_eq!(ron::from_str::<LockOnDef>(&ron::to_string(&set).unwrap()).unwrap(), set);
        assert!(ron::from_str::<LockOnDef>("(rnage: 3.0)").is_err(), "a typo is an error");
        assert_eq!(def.obstruction_ticks(1.0 / 60.0), Some(120), "2 s at 60 Hz");
        assert_eq!(LockOnDef { obstruction_s: None, ..default() }.obstruction_ticks(1.0 / 60.0), None);
    }

    #[test]
    fn validation_names_what_is_out_of_range() {
        let bad = |edit: fn(&mut LockOnDef)| {
            let mut d = LockOnDef::default();
            edit(&mut d);
            d.problems()
        };
        assert!(bad(|d| d.range = 0.0)[0].contains("range"));
        assert!(bad(|d| d.weights.view = -1.0)[0].contains("weights.view"));
        assert!(bad(|d| d.aim_height = 1.5)[0].contains("aim_height"));
        assert!(bad(|d| d.obstruction_s = Some(0.0))[0].contains("obstruction_s"));
        assert!(bad(|d| d.switch_dead_band = 2.0)[0].contains("switch_dead_band"));
        assert!(bad(|d| d.switch_min_distance = -0.1)[0].contains("switch_min_distance"));
        assert!(bad(|d| d.weights.screen = f32::NAN)[0].contains("finite"));
    }

    #[test]
    fn the_scored_pick_weighs_distance_screen_view_danger_and_the_current_target() {
        let def = LockOnDef::default();
        // Straight ahead and near beats far or off to the side.
        let ahead = [at(1, 0.0, -20.0), at(2, 0.0, -6.0), at(3, 8.0, -6.0)];
        assert_eq!(pick_lock(&view(), &def, &ahead), Some(1));
        // Out of range: not a candidate at all.
        assert_eq!(pick_lock(&view(), &def, &[at(1, 0.0, -31.0)]), None);
        assert_eq!(pick_lock(&view(), &def, &[]), None);
        // Danger: the one attacking wins over a slightly better placed one.
        let mut threat = [at(1, 0.0, -6.0), at(2, 2.0, -7.0)];
        threat[1].danger = 1.0;
        assert_eq!(pick_lock(&view(), &def, &threat), Some(1));
        let calm = LockOnDef { weights: LockWeights { danger: 0.0, ..default() }, ..default() };
        assert_eq!(pick_lock(&view(), &calm, &threat), Some(0), "with no danger weight the better placed one wins");
        // The current target keeps the lock against a slightly better placed one.
        let mut held = [at(1, 0.0, -6.0), at(2, 1.0, -6.5)];
        held[1].current = true;
        assert_eq!(pick_lock(&view(), &def, &held), Some(1));
        // A tie goes to the lower key, whatever the order.
        let twins = [at(9, 3.0, -10.0), at(5, -3.0, -10.0)];
        assert_eq!(pick_lock(&view(), &def, &twins), Some(1));
        // Behind costs: the view term (and the screen) put a candidate behind last.
        assert_eq!(pick_lock(&view(), &def, &[at(1, 0.0, 5.0), at(2, 0.0, -9.0)]), Some(1));
    }

    #[test]
    fn the_switch_goes_to_the_nearest_angle_past_the_dead_band_from_the_one_locking() {
        let def = LockOnDef::default();
        let current = Vec3::new(0.0, 0.0, -10.0);
        let mut cands = [at(1, -6.0, -10.0), at(2, -2.0, -10.0), at(3, 0.0, -10.0), at(4, 3.0, -10.0), at(5, 9.0, -10.0)];
        cands[2].current = true;
        // Right is clockwise from above: the nearest on the right is 4, on the left 2.
        assert_eq!(switch_lock(Vec3::ZERO, current, &cands, Side::Right, &def), Some(3));
        assert_eq!(switch_lock(Vec3::ZERO, current, &cands, Side::Left, &def), Some(1));
        // From the far right there is nobody further right.
        let far_right = cands[4].feet;
        let mut from_right = cands;
        from_right[2].current = false;
        from_right[4].current = true;
        assert_eq!(switch_lock(Vec3::ZERO, far_right, &from_right, Side::Right, &def), None);
        // The dead band: a candidate 0.03 rad to the side is not on a side.
        let near = Vec3::new(10.0 * ops::tan(0.03), 0.0, -10.0);
        let barely = [Lockable { feet: near, aim: near, ..at(7, 0.0, 0.0) }];
        assert_eq!(switch_lock(Vec3::ZERO, current, &barely, Side::Right, &def), None);
        assert_eq!(switch_lock(Vec3::ZERO, current, &barely, Side::Right, &LockOnDef { switch_dead_band: 0.01, ..default() }), Some(0));
        // Measured from the one locking, not from a camera behind it: from the body, the one
        // just ahead and to the right is a quarter turn away and the far one is nearer in angle;
        // from an eye 6 m behind it would be the other way round.
        let mut pair = [at(3, 0.0, -10.0), at(1, 1.0, -1.0), at(2, 6.0, -20.0)];
        pair[0].current = true;
        assert_eq!(switch_lock(Vec3::ZERO, current, &pair, Side::Right, &def), Some(2));
        assert_eq!(switch_lock(Vec3::new(0.0, 0.0, 6.0), current, &pair, Side::Right, &def), Some(1));
        // A tie in angle goes to the lower key.
        let mirror = [at(8, 3.0, -10.0), at(6, 3.0, -10.0)];
        assert_eq!(switch_lock(Vec3::ZERO, current, &mirror, Side::Right, &def), Some(1));
        // Too near to have a direction, and beyond the range: never.
        assert_eq!(switch_lock(Vec3::ZERO, current, &[at(1, 0.05, 0.0)], Side::Right, &def), None);
        assert_eq!(switch_lock(Vec3::ZERO, current, &[at(1, 40.0, -10.0)], Side::Right, &def), None);
    }

    #[test]
    fn the_aim_point_is_a_fraction_of_the_body() {
        let tf = Transform::from_xyz(1.0, 2.0, 3.0);
        let capsule = Collider::new(Shape::capsule(0.4, 1.8)).with_offset(Vec3::Y * 0.9);
        let at = body_point(&tf, Some(&capsule), 0.6);
        assert!((at - Vec3::new(1.0, 2.0 + 1.8 * 0.6, 3.0)).length() < 1e-5, "{at}");
        assert_eq!(body_point(&tf, None, 0.5), Vec3::new(1.0, 2.0 + DEFAULT_BODY_HEIGHT * 0.5, 3.0));
    }

    #[test]
    fn the_frustum_is_the_base_field_of_view_and_the_aspect() {
        let (eye, rotation) = (Vec3::ZERO, Quat::IDENTITY);
        let fov = 70f32.to_radians();
        assert!(in_frustum(eye, rotation, fov, 16.0 / 9.0, Vec3::new(0.0, 0.0, -5.0)));
        assert!(!in_frustum(eye, rotation, fov, 16.0 / 9.0, Vec3::new(0.0, 0.0, 5.0)), "behind");
        // 50 degrees to the side: inside a 16:9 frame (half width about 51 degrees), outside a square one.
        let side = Vec3::new(ops::tan(50f32.to_radians()) * 5.0, 0.0, -5.0);
        assert!(in_frustum(eye, rotation, fov, 16.0 / 9.0, side));
        assert!(!in_frustum(eye, rotation, fov, 1.0, side));
        // A sprint's wider view would take it in; the base one does not.
        let edge = Vec3::new(0.0, ops::tan(37f32.to_radians()) * 5.0, -5.0);
        assert!(!in_frustum(eye, rotation, fov, 1.0, edge) && in_frustum(eye, rotation, 75f32.to_radians(), 1.0, edge));
        assert!((half_width(fov, 1.0) - fov * 0.5).abs() < 1e-5);
    }

    #[test]
    fn bearings_are_positive_to_the_left() {
        let b = signed_bearing(Vec3::ZERO, Vec3::NEG_Z, Vec3::NEG_X).unwrap();
        assert!((b - FRAC_PI_2).abs() < 1e-5, "{b}");
        assert!(signed_bearing(Vec3::ZERO, Vec3::NEG_Z, Vec3::X).unwrap() < 0.0);
        assert_eq!(signed_bearing(Vec3::ZERO, Vec3::Y, Vec3::X), None);
        // The plain angle: 0..π, 0 for no direction.
        assert!((angle(Vec3::NEG_Z, Vec3::X) - FRAC_PI_2).abs() < 1e-6);
        assert!((angle(Vec3::NEG_Z, Vec3::Z) - PI).abs() < 1e-6 && angle(Vec3::X, Vec3::X * 3.0) == 0.0);
        assert_eq!(angle(Vec3::ZERO, Vec3::X), 0.0);
    }
}
