//! `clearance`: how far a character can go in each of the eight compass directions, and
//! what ends it. The march walks the ground the way the controller does (floors, steps,
//! drops, water, the capsule's room) through [`Terrain`], one probe every [`STEP`] meters,
//! then narrows each stop to a few centimeters.

use ai_core::spatial::compass_vector;
use bevy::prelude::*;

use super::terrain::{Probe, Terrain};
use crate::controller::CharacterParams;
use crate::physics::PhysicsWorld;

/// Distance between probes along a direction (m).
const STEP: f32 = 0.5;
/// Halvings of a step when narrowing a stop down.
const REFINE: u32 = 5;
/// Compass names, clockwise from north.
pub const COMPASS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];

/// What ends the way in one direction.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hazard {
    /// Nothing within range.
    Free,
    /// Solid in the way (or a floor too steep, or a gap too tight): no going on.
    Wall,
    /// The ground ends and the next floor is deeper than a safe drop (`None`: no floor
    /// within the probe range at all).
    Cliff { drop: Option<f32> },
    /// Water deep enough to swim in.
    Water,
    /// A step up too high to walk onto but within a climb: `rise` meters.
    Ledge { rise: f32 },
}

/// One direction of [`clearance`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reach {
    /// `N`, `NE`, ...
    pub dir: &'static str,
    /// How far the character can go before the hazard (m); the range when `Free`.
    pub distance: f32,
    pub hazard: Hazard,
}

impl Reach {
    /// `N free 12.0m`, `NE wall 2.1m`, `E cliff 3.0m (drop 8.5m)`, `SE water 4.2m`,
    /// `S ledge 2.0m (+1.1m)`.
    pub fn describe(&self) -> String {
        let d = self.distance;
        match self.hazard {
            Hazard::Free => format!("{} free {d:.1}m", self.dir),
            Hazard::Wall => format!("{} wall {d:.1}m", self.dir),
            Hazard::Cliff { drop: Some(drop) } => format!("{} cliff {d:.1}m (drop {drop:.1}m)", self.dir),
            Hazard::Cliff { drop: None } => format!("{} cliff {d:.1}m (no floor in reach)", self.dir),
            Hazard::Water => format!("{} water {d:.1}m", self.dir),
            Hazard::Ledge { rise } => format!("{} ledge {d:.1}m (+{rise:.1}m)", self.dir),
        }
    }
}

/// How a probe relates to the walker's state.
enum Sample {
    /// Walkable: the walker is now at height `y`.
    Pass { y: f32, flat: bool, swim: bool },
    Stop(Hazard),
    /// Blocked or tight: the next probes say whether it is a wall, a ledge or the edge of a drop.
    Blocked,
}

/// What a character of `params` at `origin` (feet) finds in the eight directions, up to
/// `range` meters. `swimming_at`: the water surface, when the character swims.
pub fn clearance(
    physics: &PhysicsWorld,
    params: &CharacterParams,
    origin: Vec3,
    swimming_at: Option<f32>,
    range: f32,
    safe_drop: f32,
    exclude: Option<Entity>,
) -> Vec<Reach> {
    let (marcher, start) = Marcher::at(physics, params, origin, swimming_at, range, safe_drop, exclude);
    COMPASS
        .iter()
        .map(|&name| {
            let dir = compass_vector(name).unwrap_or(Vec3::NEG_Z);
            let (distance, hazard) = marcher.march(origin, dir, start);
            Reach { dir: name, distance, hazard }
        })
        .collect()
}

/// [`clearance`] along one heading `dir` (a horizontal vector): how far, and what stops it.
#[allow(clippy::too_many_arguments, reason = "the same inputs as `clearance`, plus the heading")]
pub fn reach_toward(
    physics: &PhysicsWorld,
    params: &CharacterParams,
    origin: Vec3,
    swimming_at: Option<f32>,
    dir: Vec3,
    range: f32,
    safe_drop: f32,
    exclude: Option<Entity>,
) -> (f32, Hazard) {
    let (marcher, start) = Marcher::at(physics, params, origin, swimming_at, range, safe_drop, exclude);
    marcher.march(origin, dir.normalize_or(Vec3::NEG_Z), start)
}

struct Marcher<'a> {
    terrain: Terrain<'a>,
    safe_drop: f32,
    range: f32,
}

/// The walker's state along a march: floor (or swimming) height, whether the floor is flat.
#[derive(Clone, Copy)]
struct State {
    y: f32,
    flat: bool,
    swim: bool,
}

impl<'a> Marcher<'a> {
    /// A marcher for a walker at `origin`, and the state it starts in: the floor under the
    /// feet (or the swimmer's height); in the air, the feet's own height.
    fn at(
        physics: &'a PhysicsWorld,
        params: &'a CharacterParams,
        origin: Vec3,
        swimming_at: Option<f32>,
        range: f32,
        safe_drop: f32,
        exclude: Option<Entity>,
    ) -> (Self, (f32, bool)) {
        let marcher = Marcher { terrain: Terrain::new(physics, params, exclude), safe_drop, range };
        let start = match swimming_at {
            Some(surface) => (marcher.terrain.swim_height(surface), true),
            None => match marcher.terrain.top_of(origin.x, origin.z, origin.y, 0.5) {
                Some(floor) if origin.y - floor < 0.6 => (floor, false),
                _ => (origin.y, false),
            },
        };
        (marcher, start)
    }

    fn sample(&self, at: Vec3, from: State) -> Sample {
        let p = self.terrain.params;
        match self.terrain.probe(at.x, at.z, from.y) {
            Probe::Floor { y, normal } => {
                let dy = y - from.y;
                let flat = normal.y > 0.999;
                let allowance = if flat && from.flat { 0.0 } else { STEP * p.max_slope_deg.to_radians().tan() };
                if dy < -self.safe_drop {
                    Sample::Stop(Hazard::Cliff { drop: Some(-dy) })
                } else if dy > p.step_height + allowance {
                    // Up to what the controller climbs it is a ledge, above that a wall.
                    Sample::Stop(if p.can_climb(dy, from.swim) { Hazard::Ledge { rise: dy } } else { Hazard::Wall })
                } else {
                    Sample::Pass { y, flat, swim: false }
                }
            }
            Probe::Water { surface } if from.swim => Sample::Pass { y: self.terrain.swim_height(surface), flat: true, swim: true },
            Probe::Water { .. } => Sample::Stop(Hazard::Water),
            Probe::Void => Sample::Stop(Hazard::Cliff { drop: None }),
            Probe::Blocked(_) => Sample::Blocked,
        }
    }

    /// Marches from `origin` along `dir`; returns how far the walker gets and what stops it.
    fn march(&self, origin: Vec3, dir: Vec3, start: (f32, bool)) -> (f32, Hazard) {
        let mut state = State { y: start.0, flat: true, swim: start.1 };
        let mut passed = 0.0;
        let mut s = 0.0;
        while passed < self.range - 1e-4 {
            s = (s + STEP).min(self.range);
            let at = |t: f32| origin + dir * t;
            let stop = match self.sample(at(s), state) {
                Sample::Pass { y, flat, swim } => {
                    state = State { y, flat, swim };
                    passed = s;
                    continue;
                }
                Sample::Stop(hazard) => hazard,
                Sample::Blocked => self.beyond_blocked(at(s), dir, state),
            };
            // The stop lies between the last pass and `s`: narrow it down.
            let (mut lo, mut hi) = (passed, s);
            for _ in 0..REFINE {
                let mid = (lo + hi) * 0.5;
                match self.sample(at(mid), state) {
                    Sample::Pass { .. } => lo = mid,
                    _ => hi = mid,
                }
            }
            return (lo, stop);
        }
        (self.range, Hazard::Free)
    }

    /// A blocked probe is a wall, unless just past it there is a ledge to climb (the cell
    /// beside a ledge face is within the body's reach of it) or the ground drops away (the
    /// cell beside a cliff edge is within reach of the cliff's face).
    fn beyond_blocked(&self, at: Vec3, dir: Vec3, state: State) -> Hazard {
        let p = self.terrain.params;
        for k in 1..=2 {
            let ahead = at + dir * (STEP * k as f32);
            match self.terrain.probe(ahead.x, ahead.z, state.y) {
                Probe::Floor { y, .. } if y - state.y > p.step_height && p.can_climb(y - state.y, state.swim) => {
                    return Hazard::Ledge { rise: y - state.y };
                }
                Probe::Floor { y, .. } if y - state.y < -self.safe_drop => return Hazard::Cliff { drop: Some(state.y - y) },
                Probe::Void => return Hazard::Cliff { drop: None },
                Probe::Water { .. } if !state.swim => return Hazard::Water,
                Probe::Blocked(_) => {}
                _ => return Hazard::Wall,
            }
        }
        Hazard::Wall
    }
}
