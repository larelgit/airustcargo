//! Hit zones in the world: where an attack will land, when, and how to get out.
//!
//! [`hit_zones`] lists every hit window still to come or active, placed where the
//! attacker will be when it lands (the rest of its lunge included, walls not), for
//! ground decals, the AI's advice and the off-screen warnings of
//! [`threat_indicators`]. [`WorldShape::exit`] answers "how do I leave it": the
//! shortest move on the ground plane that takes a body of a given radius out of the
//! zone's footprint. Footprints are 2D: a sphere counts as its widest disc, a cone
//! as a flat sector; good enough to step out of an attack, which is what it is for.

use ai_core::AiId;
use bevy::prelude::*;
use serde::Serialize;
use shards_content::ContentDb;
use shards_core::flat;
use shards_motion::{PhysicsWorld, QueryFilter, Shape};

use crate::def::{ActionDef, HitFlag, HitShape, Local, EPS};
use crate::fighter::{hostile, Dead, Fighter, Team};
use crate::machine::ActionState;

/// A hit shape placed in the world: `origin` is its [`HitDef::at`](crate::HitDef)
/// point, `facing` the action's direction on the ground plane.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct WorldShape {
    pub shape: HitShape,
    pub origin: Vec3,
    pub facing: Vec3,
}

impl WorldShape {
    /// `shape` at `at` in the frame of an actor standing at `feet`, facing `facing`.
    pub fn new(shape: HitShape, at: Local, feet: Vec3, facing: Vec3) -> Self {
        let facing = flat(facing).try_normalize().unwrap_or(Vec3::NEG_Z);
        let origin = feet + yaw(facing) * at.to_vec3();
        Self { shape, origin, facing }
    }

    /// Right of the facing, on the ground plane.
    pub fn right(&self) -> Vec3 {
        Vec3::new(-self.facing.z, 0.0, self.facing.x)
    }

    /// The physics shape that bounds it, with its center and rotation. A cone is
    /// queried as the cylinder around it and narrowed with [`contains`](Self::contains).
    pub fn physics(&self) -> (Shape, Vec3, Quat) {
        let rot = yaw(self.facing);
        let (f, up) = (self.facing, Vec3::Y);
        match self.shape {
            HitShape::Sphere { radius } => (Shape::sphere(radius), self.origin, rot),
            HitShape::Capsule { radius, length } => {
                // Physics capsules stand along Y: lay it along the facing.
                let lay = rot * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
                (Shape::Capsule { half_height: length * 0.5, radius }, self.origin + f * length * 0.5, lay)
            }
            HitShape::Cone { range, height, .. } => {
                (Shape::Cylinder { half_height: height * 0.5, radius: range }, self.origin + up * height * 0.5, rot)
            }
            HitShape::Box { width, height, depth } => {
                (Shape::cuboid(Vec3::new(width, height, depth) * 0.5), self.origin, rot)
            }
            HitShape::Circle { radius, height } => {
                (Shape::Cylinder { half_height: height * 0.5, radius }, self.origin + up * height * 0.5, rot)
            }
            HitShape::Line { length, width, height } => (
                Shape::cuboid(Vec3::new(width, height, length) * 0.5),
                self.origin + f * length * 0.5 + up * height * 0.5,
                rot,
            ),
        }
    }

    /// Entities whose hurtboxes it overlaps, in collider id order. `exclude`: the
    /// attacker itself.
    pub fn overlapping(&self, physics: &PhysicsWorld, exclude: Entity) -> Vec<Entity> {
        let (shape, center, rot) = self.physics();
        let filter = QueryFilter::new(shards_motion::LayerMask::HURTBOX).excluding(exclude);
        let found = physics.overlaps(&shape, center, rot, &filter);
        match self.shape {
            HitShape::Cone { .. } => found
                .into_iter()
                .filter(|e| {
                    physics.collider(*e).is_some_and(|c| {
                        self.contains(c.position, c.shape.half_extents().x.max(c.shape.half_extents().z))
                    })
                })
                .collect(),
            _ => found,
        }
    }

    /// Whether a body of `radius` at `point` touches the footprint.
    pub fn contains(&self, point: Vec3, radius: f32) -> bool {
        self.exit(point, radius).is_some()
    }

    /// The shortest move on the ground plane that takes a body of `radius` at `point`
    /// out of the footprint; `None` when it is already out.
    pub fn exit(&self, point: Vec3, radius: f32) -> Option<Vec3> {
        let p = flat(point - self.origin);
        let (f, r) = (self.facing, self.right());
        match self.shape {
            HitShape::Sphere { radius: big } | HitShape::Circle { radius: big, .. } => {
                stadium_exit(p, Vec3::ZERO, Vec3::ZERO, big + radius, f)
            }
            HitShape::Capsule { radius: big, length } => stadium_exit(p, Vec3::ZERO, f * length, big + radius, r),
            HitShape::Box { width, depth, .. } => rect_exit(p, f, r, width * 0.5 + radius, depth * 0.5 + radius),
            HitShape::Line { length, width, .. } => {
                rect_exit(p - f * length * 0.5, f, r, width * 0.5 + radius, length * 0.5 + radius)
            }
            HitShape::Cone { range, angle, .. } => sector_exit(p, f, range, angle.to_radians() * 0.5, radius),
        }
    }

    /// Its center on the ground: where an indicator points.
    pub fn center(&self) -> Vec3 {
        let f = self.facing;
        match self.shape {
            HitShape::Sphere { .. } | HitShape::Circle { .. } | HitShape::Box { .. } => self.origin,
            HitShape::Capsule { length, .. } | HitShape::Line { length, .. } => self.origin + f * length * 0.5,
            HitShape::Cone { range, .. } => self.origin + f * range * 0.5,
        }
    }
}

/// The rotation that turns -Z (Bevy's forward) to `facing` about Y.
pub(crate) fn yaw(facing: Vec3) -> Quat {
    Quat::from_rotation_y((-facing.x).atan2(-facing.z))
}

/// Out of the segment `a..b` inflated by `reach`. At its core, leave along `side`.
fn stadium_exit(p: Vec3, a: Vec3, b: Vec3, reach: f32, side: Vec3) -> Option<Vec3> {
    let ab = b - a;
    let t = if ab.length_squared() > EPS { ((p - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    let away = p - (a + ab * t);
    let d = away.length();
    if d >= reach {
        return None;
    }
    let dir = if d > 1e-4 { away / d } else { side };
    Some(dir * (reach - d))
}

/// Out of a rectangle centered at the origin: `hx` across `r`, `hz` along `f`.
fn rect_exit(p: Vec3, f: Vec3, r: Vec3, hx: f32, hz: f32) -> Option<Vec3> {
    let (x, z) = (p.dot(r), p.dot(f));
    if x.abs() >= hx || z.abs() >= hz {
        return None;
    }
    [(hx - x, r), (hx + x, -r), (hz - z, f), (hz + z, -f)]
        .into_iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(d, dir)| dir * d)
}

/// Out of a sector (apex at the origin, axis `f`, `half` angle) inflated by `radius`:
/// back out of its range, or sideways past its nearer edge, whichever is shorter.
/// Past ±90° the sides are approximated by their lines.
fn sector_exit(p: Vec3, f: Vec3, range: f32, half: f32, radius: f32) -> Option<Vec3> {
    let d = p.length();
    let side = Vec3::new(-f.z, 0.0, f.x);
    let phi = p.dot(side).atan2(p.dot(f));
    let edge_dir = |edge: f32| f * edge.cos() + side * edge.sin();
    // How far the point is from the (uninflated) sector.
    let outside = if phi.abs() <= half || d < 1e-4 {
        (d - range).max(0.0)
    } else {
        let dir = edge_dir(if phi > 0.0 { half } else { -half });
        (p - dir * p.dot(dir).clamp(0.0, range)).length()
    };
    if outside > 0.0 && outside >= radius {
        return None;
    }
    let mut options: Vec<(f32, Vec3)> = vec![(range + radius - d, if d > 1e-4 { p / d } else { f })];
    if half < std::f32::consts::PI - EPS {
        for edge in [half, -half] {
            // Outward normal of the edge line: crossing it takes `radius - p·normal`.
            let normal = edge_dir(edge + edge.signum() * std::f32::consts::FRAC_PI_2);
            options.push((radius - p.dot(normal), normal));
        }
    }
    options.into_iter().filter(|(dist, _)| *dist > 0.0).min_by(|a, b| a.0.total_cmp(&b.0)).map(|(dist, dir)| dir * dist)
}

/// One attack still to land (or landing now).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HitZone {
    #[serde(skip)]
    pub attacker: Entity,
    pub attacker_id: Option<AiId>,
    #[serde(skip)]
    pub team: Option<Team>,
    pub action: String,
    /// Index of the hit window in the action.
    pub hit: usize,
    pub shape: WorldShape,
    /// Seconds until it lands; 0 while it is active.
    pub lands_in: f32,
    /// Seconds it stays active once it lands.
    pub active_for: f32,
    /// The telegraph's symbol; `None` for attacks without a telegraph.
    pub symbol: Option<String>,
    pub flags: Vec<HitFlag>,
}

/// What [`hit_zones`] reads of an attacker.
type Attacker<'a> = (Entity, Option<&'a AiId>, &'a ActionState, &'a Transform, Option<&'a Team>);

/// Every hit window not over yet, of every fighter in an action, in attacker `AiId`
/// order then time order. Pure: reads the world, changes nothing.
pub fn hit_zones(world: &World) -> Vec<HitZone> {
    let Some(db) = world.get_resource::<ContentDb>() else { return Vec::new() };
    let Some(mut q) = world.try_query_filtered::<Attacker, (With<Fighter>, Without<Dead>)>() else { return Vec::new() };
    let mut attackers: Vec<_> = q.iter(world).filter(|(_, _, s, ..)| s.current.is_some()).collect();
    attackers.sort_by_key(|(e, id, ..)| (id.map_or(u32::MAX, |i| i.0), *e));
    let mut zones = Vec::new();
    for (entity, id, state, tf, team) in attackers {
        let Some(run) = &state.current else { continue };
        let Some(def) = db.get::<ActionDef>(&run.action) else { continue };
        zones.extend(action_zones(def, run.t, tf.translation, run.facing).into_iter().map(|z| HitZone {
            attacker: entity,
            attacker_id: id.copied(),
            team: team.cloned(),
            action: run.action.clone(),
            ..z
        }));
    }
    zones
}

/// The zones of `def` at time `t` for an actor at `feet` facing `facing` (the
/// attacker fields left empty). A telegraph with its own zone shows that zone once,
/// spanning all remaining hit windows.
pub fn action_zones(def: &ActionDef, t: f32, feet: Vec3, facing: Vec3) -> Vec<HitZone> {
    let symbol = def.telegraph.as_ref().map(|tg| tg.symbol.clone());
    let at = |from: f32| {
        feet + flat(facing).normalize_or_zero() * (def.displacement_at(from.max(t)) - def.displacement_at(t))
    };
    let zone = |hit: usize, shape: HitShape, local: Local, from: f32, to: f32, flags: Vec<HitFlag>| HitZone {
        attacker: Entity::PLACEHOLDER,
        attacker_id: None,
        team: None,
        action: String::new(),
        hit,
        shape: WorldShape::new(shape, local, at(from), facing),
        lands_in: (from - t).max(0.0),
        active_for: to - from.max(t),
        symbol: symbol.clone(),
        flags,
    };
    let windows: Vec<(usize, (f32, f32))> = def
        .hits
        .iter()
        .enumerate()
        .filter_map(|(i, h)| Some((i, def.span(&h.span)?)))
        .filter(|(_, w)| w.1 > t + EPS)
        .collect();
    if let Some(tg) = &def.telegraph
        && let Some(shape) = tg.zone
    {
        let Some(from) = windows.iter().map(|(_, w)| w.0).reduce(f32::min) else { return Vec::new() };
        let to = windows.iter().map(|(_, w)| w.1).fold(from, f32::max);
        let flags = windows.iter().flat_map(|(i, _)| def.hits[*i].flags.clone()).collect();
        return vec![zone(windows[0].0, shape, tg.zone_at, from, to, flags)];
    }
    windows
        .into_iter()
        .map(|(i, (from, to))| zone(i, def.hits[i].shape, def.hits[i].at, from, to, def.hits[i].flags.clone()))
        .collect()
}

/// Hostile zones a body of `radius` at `point` stands in, soonest first, with the way
/// out of each.
pub fn zones_threatening(world: &World, me: Entity, point: Vec3, radius: f32) -> Vec<(HitZone, Vec3)> {
    let team = world.get::<Team>(me);
    let mut found: Vec<(HitZone, Vec3)> = hit_zones(world)
        .into_iter()
        .filter(|z| z.attacker != me && hostile(z.team.as_ref(), team))
        .filter_map(|z| {
            let out = z.shape.exit(point, radius)?;
            Some((z, out))
        })
        .collect();
    found.sort_by(|a, b| a.0.lands_in.total_cmp(&b.0.lands_in).then(a.0.attacker_id.cmp(&b.0.attacker_id)));
    found
}

/// The body radius of an entity's hurtbox (its collider's), 0.4 without one.
pub fn body_radius(world: &World, entity: Entity) -> f32 {
    world.get::<shards_motion::Collider>(entity).map_or(0.4, |c| {
        let half = c.shape.half_extents();
        half.x.max(half.z)
    })
}

/// An off-screen warning: which way to look and how soon it lands.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Indicator {
    #[serde(skip)]
    pub source: Entity,
    pub source_id: Option<AiId>,
    pub action: String,
    /// Degrees from `view_forward` to the zone on the ground plane: 0 ahead, +90 right,
    /// -90 left, ±180 behind.
    pub angle_deg: f32,
    pub lands_in: f32,
    pub symbol: String,
    /// Meters from the viewer to the zone's center.
    pub distance: f32,
    /// The viewer stands in it now.
    pub inside: bool,
}

/// Warnings for `viewer`: the soonest hostile zone of each attacking fighter, with
/// the direction relative to `view_forward` (the camera's, for a HUD), soonest first.
/// Attacks without a telegraph get the symbol `!`.
pub fn threat_indicators(world: &World, viewer: Entity, view_forward: Vec3) -> Vec<Indicator> {
    let Some(here) = world.get::<Transform>(viewer).map(|t| t.translation) else { return Vec::new() };
    let team = world.get::<Team>(viewer);
    let radius = body_radius(world, viewer);
    let fwd = flat(view_forward).try_normalize().unwrap_or(Vec3::NEG_Z);
    let right = Vec3::new(-fwd.z, 0.0, fwd.x);
    let mut out: Vec<Indicator> = Vec::new();
    for z in hit_zones(world).into_iter().filter(|z| z.attacker != viewer && hostile(z.team.as_ref(), team)) {
        if let Some(seen) = out.iter_mut().find(|i| i.source == z.attacker) {
            if z.lands_in < seen.lands_in {
                seen.lands_in = z.lands_in;
            }
            continue;
        }
        let to = flat(z.shape.center() - here);
        let angle_deg = to.dot(right).atan2(to.dot(fwd)).to_degrees();
        out.push(Indicator {
            source: z.attacker,
            source_id: z.attacker_id,
            action: z.action.clone(),
            angle_deg: if to.length() < 1e-4 { 0.0 } else { angle_deg },
            lands_in: z.lands_in,
            symbol: z.symbol.clone().unwrap_or_else(|| "!".into()),
            distance: to.length(),
            inside: z.shape.contains(here, radius),
        });
    }
    out.sort_by(|a, b| a.lands_in.total_cmp(&b.lands_in).then(a.source_id.cmp(&b.source_id)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const N: Vec3 = Vec3::NEG_Z;

    fn at(shape: HitShape) -> WorldShape {
        WorldShape::new(shape, Local::default(), Vec3::ZERO, N)
    }

    fn close(a: Vec3, b: Vec3) -> bool {
        (a - b).length() < 1e-3
    }

    #[test]
    fn circle_exit_is_radial() {
        let z = at(HitShape::Circle { radius: 4.5, height: 2.0 });
        let out = z.exit(Vec3::new(0.0, 0.0, 2.0), 0.4).unwrap();
        assert!(close(out, Vec3::new(0.0, 0.0, 2.9)), "{out}");
        assert_eq!(z.exit(Vec3::new(0.0, 0.0, 5.0), 0.4), None);
        assert!(z.contains(Vec3::new(4.8, 0.0, 0.0), 0.4), "the body's radius counts");
    }

    #[test]
    fn box_exit_takes_the_nearest_side() {
        let z = WorldShape::new(
            HitShape::Box { width: 2.0, height: 2.0, depth: 4.0 },
            Local { forward: 2.0, ..default() },
            Vec3::ZERO,
            N,
        );
        // The box spans x -1..1, z -4..0; a point at x 0.5 is nearest the east side.
        let out = z.exit(Vec3::new(0.5, 0.0, -2.0), 0.0).unwrap();
        assert!(close(out, Vec3::new(0.5, 0.0, 0.0)), "{out}");
        assert_eq!(z.exit(Vec3::new(3.0, 0.0, -2.0), 0.5), None);
    }

    #[test]
    fn line_exit_steps_sideways() {
        let z = at(HitShape::Line { length: 8.0, width: 1.0, height: 0.5 });
        let out = z.exit(Vec3::new(-0.2, 0.0, -5.0), 0.3).unwrap();
        assert!(close(out, Vec3::new(-0.6, 0.0, 0.0)), "{out}");
    }

    #[test]
    fn cone_exit_goes_back_or_sideways() {
        let z = at(HitShape::Cone { range: 3.0, angle: 90.0, height: 2.0 });
        // Near the far edge: back out of range is shortest.
        let out = z.exit(Vec3::new(0.0, 0.0, -2.8), 0.0).unwrap();
        assert!(close(out, Vec3::new(0.0, 0.0, -0.2)), "{out}");
        // Near the east edge: sideways is shortest.
        let p = Vec3::new(1.0, 0.0, -1.2);
        let out = z.exit(p, 0.0).unwrap();
        assert!(out.length() < 0.2 && out.x > 0.0, "{out}");
        assert!(!z.contains(p + out * 1.01, 0.0));
        assert_eq!(z.exit(Vec3::new(0.0, 0.0, 1.0), 0.0), None, "behind the apex");
        assert_eq!(z.exit(Vec3::new(2.5, 0.0, -0.5), 0.0), None, "outside the angle");
    }

    #[test]
    fn capsule_exit_is_perpendicular() {
        let z = at(HitShape::Capsule { radius: 0.5, length: 3.0 });
        let out = z.exit(Vec3::new(0.2, 0.0, -1.5), 0.4).unwrap();
        assert!(close(out, Vec3::new(0.7, 0.0, 0.0)), "{out}");
    }

    #[test]
    fn world_shapes_turn_with_the_facing() {
        let east = WorldShape::new(
            HitShape::Sphere { radius: 1.0 },
            Local { forward: 2.0, up: 1.0, right: 0.5 },
            Vec3::new(10.0, 0.0, 0.0),
            Vec3::X,
        );
        assert!(close(east.origin, Vec3::new(12.0, 1.0, 0.5)), "{}", east.origin);
        let (_, center, rot) =
            WorldShape::new(HitShape::Capsule { radius: 0.3, length: 2.0 }, Local::default(), Vec3::ZERO, Vec3::X)
                .physics();
        assert!(close(center, Vec3::new(1.0, 0.0, 0.0)) && close(rot * Vec3::Y, Vec3::X), "{center} {}", rot * Vec3::Y);
    }
}
