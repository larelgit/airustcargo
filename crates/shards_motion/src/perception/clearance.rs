//! `clearance`: how far a character can go in each of the eight compass directions, and
//! what ends it. The march walks the ground the way the controller does (floors, steps,
//! drops, water, the capsule's room) through [`Terrain`], one probe every [`STEP`] meters,
//! then narrows each stop to a few centimeters.

use ai_core::spatial::compass_vector;
use bevy::prelude::*;

use super::terrain::{Block, Probe, Terrain, FLOOR_NOISE};
use crate::controller::CharacterParams;
use crate::physics::PhysicsWorld;

/// Distance between probes along a direction (m).
const STEP: f32 = 0.5;
/// Halvings of a step when narrowing a stop down.
const REFINE: u32 = 5;
/// Compass names, clockwise from north.
pub const COMPASS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];
/// [`COMPASS`] as steps between grid columns `(x, z)` (N is -Z, E is +X).
pub const COMPASS_STEPS: [(i32, i32); 8] = [(0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1)];

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
    /// The ground goes on, but the [`RoutePlanner`](super::RoutePlanner) `by` (the navmesh)
    /// knows no way on: a tile edge that does not connect, a spot blocked now.
    Closed { by: &'static str },
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
    /// `S ledge 2.0m (+1.1m)`, `W closed 4.0m (the navmesh goes no further)`.
    pub fn describe(&self) -> String {
        let d = self.distance;
        match self.hazard {
            Hazard::Free => format!("{} free {d:.1}m", self.dir),
            Hazard::Wall => format!("{} wall {d:.1}m", self.dir),
            Hazard::Cliff { drop: Some(drop) } => format!("{} cliff {d:.1}m (drop {drop:.1}m)", self.dir),
            Hazard::Cliff { drop: None } => format!("{} cliff {d:.1}m (no floor in reach)", self.dir),
            Hazard::Water => format!("{} water {d:.1}m", self.dir),
            Hazard::Ledge { rise } => format!("{} ledge {d:.1}m (+{rise:.1}m)", self.dir),
            Hazard::Closed { by } => format!("{} closed {d:.1}m (the {by} goes no further)", self.dir),
        }
    }
}

/// How a probe relates to the walker's state.
enum Sample {
    /// Walkable: the walker is now at height `y`.
    Pass { y: f32, up: f32, swim: bool },
    Stop(Hazard),
    /// Blocked or tight: for a tight spot the next probes say whether it is a wall, a ledge or
    /// the edge of a drop.
    Blocked(Block),
}

/// Whether a floor `rise` above a walker (from the height its feet float at, when swimming) is a
/// ledge the controller climbs: by walking at it, or, for a swimmer, by jumping at it.
fn climbable(p: &CharacterParams, rise: f32, swimming: bool) -> bool {
    p.can_climb(rise, swimming) || (swimming && p.can_jump_out(rise))
}

/// What a character of `params` at `origin` (feet) finds in the eight directions, up to
/// `range` meters. `swimming_at`: the water surface, when the character swims. `exclude`:
/// entities whose colliders never count (the character, and whatever it looks around).
pub fn clearance(
    physics: &PhysicsWorld,
    params: &CharacterParams,
    origin: Vec3,
    swimming_at: Option<f32>,
    range: f32,
    safe_drop: f32,
    exclude: &[Entity],
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
    let (marcher, start) = Marcher::at(physics, params, origin, swimming_at, range, safe_drop, exclude.as_slice());
    marcher.march(origin, dir.normalize_or(Vec3::NEG_Z), start)
}

struct Marcher<'a> {
    terrain: Terrain<'a>,
    safe_drop: f32,
    range: f32,
}

/// The walker's state along a march: floor (or swimming) height, the `y` of the floor's
/// normal (1: flat).
#[derive(Clone, Copy)]
struct State {
    y: f32,
    up: f32,
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
        exclude: &[Entity],
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
                let allowance = self.terrain.slope_rise(STEP, from.up, normal.y);
                if dy < -self.safe_drop.max(FLOOR_NOISE) {
                    Sample::Stop(Hazard::Cliff { drop: Some(-dy) })
                } else if dy > p.step_height + allowance + FLOOR_NOISE {
                    // Up to what the controller climbs it is a ledge, above that a wall.
                    Sample::Stop(if climbable(p, dy, from.swim) { Hazard::Ledge { rise: dy } } else { Hazard::Wall })
                } else {
                    Sample::Pass { y, up: normal.y, swim: false }
                }
            }
            Probe::Water { surface } if from.swim => Sample::Pass { y: self.terrain.swim_height(surface), up: 1.0, swim: true },
            Probe::Water { .. } => Sample::Stop(Hazard::Water),
            Probe::Void => Sample::Stop(Hazard::Cliff { drop: None }),
            Probe::Blocked(block) => Sample::Blocked(block),
        }
    }

    /// Marches from `origin` along `dir`; returns how far the walker gets and what stops it.
    fn march(&self, origin: Vec3, dir: Vec3, start: (f32, bool)) -> (f32, Hazard) {
        let mut state = State { y: start.0, up: 1.0, swim: start.1 };
        let mut passed = 0.0;
        let mut s = 0.0;
        while passed < self.range - 1e-4 {
            s = (s + STEP).min(self.range);
            let at = |t: f32| origin + dir * t;
            let stop = match self.sample(at(s), state) {
                Sample::Pass { y, up, swim } => {
                    state = State { y, up, swim };
                    passed = s;
                    continue;
                }
                Sample::Stop(hazard) => hazard,
                Sample::Blocked(block) => self.beyond_blocked(at(s), dir, state, block),
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

    /// A blocked probe is a wall, unless it is only a tight spot (floor, but the body does
    /// not fit) and just past it there is a ledge to climb (the cell beside a ledge face is
    /// within the body's reach of it) or the ground drops away (the cell beside a cliff edge
    /// is within reach of the cliff's face). What lies past a solid taller than a climb says
    /// nothing about it: behind a wall there may be anything.
    fn beyond_blocked(&self, at: Vec3, dir: Vec3, state: State, block: Block) -> Hazard {
        let p = self.terrain.params;
        if block != Block::Tight {
            return Hazard::Wall;
        }
        if self.terrain.blocker_top(at.x, at.z, state.y).is_some_and(|top| !climbable(p, top - state.y, state.swim)) {
            return Hazard::Wall;
        }
        for k in 1..=2 {
            let ahead = at + dir * (STEP * k as f32);
            match self.terrain.probe(ahead.x, ahead.z, state.y) {
                Probe::Floor { y, .. } if y - state.y > p.step_height + FLOOR_NOISE && climbable(p, y - state.y, state.swim) => {
                    return Hazard::Ledge { rise: y - state.y };
                }
                Probe::Floor { y, .. } if y - state.y < -self.safe_drop.max(FLOOR_NOISE) => return Hazard::Cliff { drop: Some(state.y - y) },
                Probe::Void => return Hazard::Cliff { drop: None },
                Probe::Water { .. } if !state.swim => return Hazard::Water,
                Probe::Blocked(_) => {}
                _ => return Hazard::Wall,
            }
        }
        Hazard::Wall
    }
}
