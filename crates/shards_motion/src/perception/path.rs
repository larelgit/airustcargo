//! A small local path planner: deterministic grid A* over what [`Terrain`] says a
//! character can stand on and move between. It serves `path_to` and `move_to path=true`;
//! navmesh pathing across a whole region is E6 and replaces it there.
//!
//! The grid has [`CELL`] spacing, aligned to the world, and a node is a cell at one floor
//! level (stacked floors are different nodes). Moves go to the eight neighbours:
//!
//! - **walk**: floors up to `step_height` apart (more on slopes), or a short drop;
//! - **swim**: into and across water deep enough to swim in, at twice the cost;
//! - **mantle**: onto a floor up to `mantle_height` higher, for a cost of a few meters
//!   (a ledge whose face is within the body's reach of the cell next to it is crossed in
//!   one two-cell hop);
//! - **drop**: down to `safe_drop`, or into water from higher up.
//!
//! The search is bounded by a box around start and goal and by a node budget, and
//! everything about it is integer arithmetic on costs and index order on ties, so the same
//! world gives the same path on every machine and in every replay.

use std::cmp::Reverse;
use std::collections::{BinaryHeap, HashMap};

use ai_core::spatial::compass;
use bevy::prelude::*;
use serde_json::{json, Value};

use super::terrain::{Block, Probe, Terrain};
use crate::controller::CharacterParams;
use crate::physics::PhysicsWorld;

/// Grid spacing (m).
pub const CELL: f32 = 0.5;
/// Cost units per meter.
const UNIT: f32 = 1000.0;
/// What a mantle costs on top of its length (m): climbing is slow and a last resort.
const MANTLE_COST: f32 = 3.0;
/// Swimming costs this many times its length.
const SWIM_FACTOR: f32 = 2.0;
/// Dropping costs this much per meter fallen (on top of its length).
const DROP_COST: f32 = 0.4;
/// The highest fall into water the planner accepts (m).
const WATER_DROP: f32 = 20.0;
/// The goal's floor must be within this height of the target's feet (m).
const GOAL_BAND: f32 = 2.5;
/// Nodes the search may expand before it gives up.
const MAX_EXPANSIONS: usize = 40_000;
/// Neighbour offsets, clockwise from north (N = -Z, E = +X).
const DIRS: [(i32, i32); 8] = [(0, -1), (1, -1), (1, 0), (1, 1), (0, 1), (-1, 1), (-1, 0), (-1, -1)];

/// How a leg of a path is travelled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StepKind {
    Walk,
    Swim,
    /// Climb onto a higher floor.
    Mantle,
    /// Step or fall down to a lower floor.
    Drop,
}

impl StepKind {
    fn name(self) -> &'static str {
        match self {
            StepKind::Walk => "walk",
            StepKind::Swim => "swim",
            StepKind::Mantle => "mantle",
            StepKind::Drop => "drop",
        }
    }
}

/// What to plan.
#[derive(Clone, Debug)]
pub struct PathRequest {
    /// Feet of the walker.
    pub start: Vec3,
    /// The surface of the water the walker swims in, if it does.
    pub swimming_at: Option<f32>,
    /// Where to go: `x` and `z`; `y` too when `goal_level` is set.
    pub goal: Vec3,
    /// The floor must be near `goal.y` (the goal is an entity standing somewhere).
    pub goal_level: bool,
    /// Arrive within this many meters of the goal (at least half a cell is needed).
    pub within: f32,
    /// Longest route to accept (m).
    pub max_length: f32,
    /// Drops deeper than this are cliffs (m).
    pub safe_drop: f32,
    /// An entity whose colliders never count (the walker).
    pub exclude: Option<Entity>,
}

/// One straight run of a path: a direction, a way of moving and a length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Leg {
    /// Compass name of the heading (`NE`).
    pub dir: &'static str,
    /// Horizontal length (m).
    pub length: f32,
    pub kind: StepKind,
    /// Height climbed (mantle) or fallen (drop), m.
    pub rise: f32,
    /// Where the leg ends: feet.
    pub end: Vec3,
}

/// A planned route.
#[derive(Clone, Debug, PartialEq)]
pub struct PathPlan {
    pub start: Vec3,
    pub legs: Vec<Leg>,
    /// Horizontal length (m).
    pub length: f32,
    /// Cells the search expanded.
    pub explored: usize,
}

impl PathPlan {
    /// Where each leg ends, in order. Walking a straight line between consecutive ones
    /// stays on cells the planner checked.
    pub fn waypoints(&self) -> Vec<(Vec3, StepKind)> {
        self.legs.iter().map(|l| (l.end, l.kind)).collect()
    }

    /// `path 23.5m, 4 turns: 6.0m NE → 4.5m N (mantle 1.1m) → 9.0m E (swim) → 4.0m N`.
    /// A path of no legs is `already there`.
    pub fn summary(&self) -> String {
        if self.legs.is_empty() {
            return "already there".to_owned();
        }
        let legs: Vec<String> = self
            .legs
            .iter()
            .map(|l| {
                let note = match l.kind {
                    StepKind::Walk => String::new(),
                    StepKind::Swim => " (swim)".to_owned(),
                    StepKind::Mantle => format!(" (mantle {:.1}m)", l.rise),
                    StepKind::Drop => format!(" (drop {:.1}m)", l.rise),
                };
                format!("{:.1}m {}{note}", l.length, l.dir)
            })
            .collect();
        format!("path {:.1}m, {} turns: {}", self.length, self.legs.len(), legs.join(" → "))
    }

    /// The summary and the waypoints, for tools.
    pub fn to_json(&self) -> Value {
        let round = |v: f32| (v * 100.0).round() / 100.0;
        let waypoints: Vec<Value> = self
            .legs
            .iter()
            .map(|l| json!({"x": round(l.end.x), "y": round(l.end.y), "z": round(l.end.z), "mode": l.kind.name()}))
            .collect();
        json!({"path": self.summary(), "distance": round(self.length), "turns": self.legs.len(), "waypoints": waypoints})
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Cell {
    Floor,
    Swim,
}

/// A grid cell at one level: `(i, j, level, swimming)`.
type Key = (i32, i32, i32, bool);

struct Node {
    pos: Vec3,
    cell: Cell,
    /// The floor here is flat (no slope allowance when stepping from it).
    flat: bool,
    cost: u32,
    parent: Option<usize>,
    kind: StepKind,
    rise: f32,
    closed: bool,
}

/// A move the planner may take.
struct Move {
    pos: Vec3,
    cell: Cell,
    flat: bool,
    kind: StepKind,
    cost: u32,
    rise: f32,
}

fn level(y: f32) -> i32 {
    (y * 4.0).round() as i32
}

fn cell_of(v: f32) -> i32 {
    (v / CELL).round() as i32
}

fn centre(i: i32) -> f32 {
    i as f32 * CELL
}

fn flat_distance(a: Vec3, b: Vec3) -> f32 {
    Vec2::new(a.x - b.x, a.z - b.z).length()
}

/// Plans a route for a character of `params`. `Err` says why there is none and what stops
/// it, in words an agent can act on.
pub fn plan_path(physics: &PhysicsWorld, params: &CharacterParams, request: &PathRequest) -> Result<PathPlan, String> {
    Planner::new(physics, params, request).run()
}

struct Planner<'a> {
    terrain: Terrain<'a>,
    request: &'a PathRequest,
    probes: HashMap<(i32, i32, i32), Probe>,
    nodes: Vec<Node>,
    index: HashMap<Key, usize>,
    open: BinaryHeap<Reverse<(u32, u32, usize)>>,
    /// Cells beyond this box are not searched: `(min_i, max_i, min_j, max_j)`.
    bounds: (i32, i32, i32, i32),
    /// How far the box reaches beyond start and goal (m).
    margin: f32,
    /// The search met the edge of its box.
    clipped: bool,
}

impl<'a> Planner<'a> {
    fn new(physics: &'a PhysicsWorld, params: &'a CharacterParams, request: &'a PathRequest) -> Self {
        let margin = (request.max_length * 0.25).clamp(6.0, 20.0);
        let (a, b) = (request.start, request.goal);
        let bounds = (
            cell_of(a.x.min(b.x) - margin),
            cell_of(a.x.max(b.x) + margin),
            cell_of(a.z.min(b.z) - margin),
            cell_of(a.z.max(b.z) + margin),
        );
        Self {
            terrain: Terrain::new(physics, params, request.exclude),
            request,
            probes: HashMap::new(),
            nodes: Vec::new(),
            index: HashMap::new(),
            open: BinaryHeap::new(),
            bounds,
            margin,
            clipped: false,
        }
    }

    fn run(mut self) -> Result<PathPlan, String> {
        let req = self.request;
        if ![req.start, req.goal].iter().all(|v| v.x.is_finite() && v.z.is_finite() && v.y.is_finite()) {
            return Err("the start and the goal must be finite coordinates".into());
        }
        let away = flat_distance(req.start, req.goal);
        if away > req.max_length {
            return Err(format!(
                "({:.1}, {:.1}) is {away:.0}m away, farther than max {:.0}m; raise `max`, or go in stages",
                req.goal.x, req.goal.z, req.max_length
            ));
        }
        let start = self.start_node()?;
        let within = req.within.max(CELL * 0.72);
        self.nodes.push(start);
        self.open.push(Reverse((self.heuristic(0, within), 0, 0)));
        let max_cost = (req.max_length * UNIT * (SWIM_FACTOR + 1.0)) as u32 + (MANTLE_COST * UNIT) as u32 * 8;

        let (mut expanded, mut closest) = (0usize, 0usize);
        while let Some(Reverse((_, cost, at))) = self.open.pop() {
            if self.nodes[at].closed || self.nodes[at].cost != cost {
                continue;
            }
            self.nodes[at].closed = true;
            let node = &self.nodes[at];
            let reached = flat_distance(node.pos, req.goal) <= within && (!req.goal_level || (node.pos.y - req.goal.y).abs() <= GOAL_BAND);
            if reached {
                return self.finish(at, expanded);
            }
            if flat_distance(node.pos, req.goal) < flat_distance(self.nodes[closest].pos, req.goal) - 1e-4 {
                closest = at;
            }
            expanded += 1;
            if expanded > MAX_EXPANSIONS {
                return Err(format!(
                    "no route found to ({:.1}, {:.1}) within the search budget ({MAX_EXPANSIONS} cells); \
                     move closer in steps or pick a nearer target",
                    req.goal.x, req.goal.z
                ));
            }
            for next in self.neighbours(at) {
                if self.nodes[at].cost + next.cost > max_cost {
                    continue;
                }
                self.relax(at, next, within);
            }
        }
        Err(self.explain(closest, within, expanded))
    }

    /// The walker's own node: where it stands, not snapped to the grid.
    fn start_node(&self) -> Result<Node, String> {
        let req = self.request;
        let (pos, cell) = match req.swimming_at {
            Some(surface) => (Vec3::new(req.start.x, self.terrain.swim_height(surface), req.start.z), Cell::Swim),
            None => {
                let floor = self.terrain.top_of(req.start.x, req.start.z, req.start.y, 0.5);
                match floor.filter(|y| req.start.y - y < 0.6) {
                    Some(y) => (Vec3::new(req.start.x, y, req.start.z), Cell::Floor),
                    None => {
                        return Err(match floor {
                            Some(y) => format!("you are {:.1}m above the ground: path_to starts from where you stand, so land first", req.start.y - y),
                            None => "there is no ground under you: path_to starts from where you stand, so land first".to_owned(),
                        });
                    }
                }
            }
        };
        Ok(Node { pos, cell, flat: true, cost: 0, parent: None, kind: StepKind::Walk, rise: 0.0, closed: false })
    }

    fn heuristic(&self, node: usize, within: f32) -> u32 {
        let d = flat_distance(self.nodes[node].pos, self.request.goal);
        ((d - within).max(0.0) * UNIT) as u32
    }

    fn relax(&mut self, from: usize, next: Move, within: f32) {
        let (i, j) = (cell_of(next.pos.x), cell_of(next.pos.z));
        let key: Key = (i, j, level(next.pos.y), next.cell == Cell::Swim);
        let cost = self.nodes[from].cost + next.cost;
        let at = match self.index.get(&key) {
            Some(&at) => {
                if self.nodes[at].closed || self.nodes[at].cost <= cost {
                    return;
                }
                at
            }
            None => {
                self.nodes.push(Node {
                    pos: next.pos,
                    cell: next.cell,
                    flat: next.flat,
                    cost,
                    parent: None,
                    kind: next.kind,
                    rise: next.rise,
                    closed: false,
                });
                self.index.insert(key, self.nodes.len() - 1);
                self.nodes.len() - 1
            }
        };
        let node = &mut self.nodes[at];
        node.cost = cost;
        node.parent = Some(from);
        node.kind = next.kind;
        node.rise = next.rise;
        let f = cost + self.heuristic(at, within);
        self.open.push(Reverse((f, cost, at)));
    }

    /// What lies at the cell `(i, j)` for a walker at height `from_y`.
    fn probe(&mut self, i: i32, j: i32, from_y: f32) -> Probe {
        let terrain = &self.terrain;
        *self.probes.entry((i, j, level(from_y))).or_insert_with(|| terrain.probe(centre(i), centre(j), from_y))
    }

    /// Every move out of node `at`, in a fixed order.
    fn neighbours(&mut self, at: usize) -> Vec<Move> {
        let from = (self.nodes[at].pos, self.nodes[at].cell, self.nodes[at].flat);
        let (ci, cj) = (cell_of(from.0.x), cell_of(from.0.z));
        let mut out = Vec::with_capacity(8);
        // From an off-grid start, the cell it stands in is a move too.
        if at == 0
            && flat_distance(from.0, Vec3::new(centre(ci), from.0.y, centre(cj))) > 0.05
            && let Some(m) = self.step(from, ci, cj, None)
        {
            out.push(m);
        }
        for (k, &(di, dj)) in DIRS.iter().enumerate() {
            let (i, j) = (ci + di, cj + dj);
            if !(self.bounds.0..=self.bounds.1).contains(&i) || !(self.bounds.2..=self.bounds.3).contains(&j) {
                self.clipped = true;
                continue;
            }
            let diagonal = k % 2 == 1;
            if diagonal {
                // No cutting corners: both cells beside the diagonal must be open to us.
                let open = |this: &mut Self, i: i32, j: i32| this.step(from, i, j, None).is_some();
                if !open(self, ci + di, cj) || !open(self, ci, cj + dj) {
                    continue;
                }
            }
            if let Some(m) = self.step(from, i, j, None) {
                out.push(m);
            } else if !diagonal && matches!(self.probe(i, j, from.0.y), Probe::Blocked(Block::Tight)) {
                // The cell next to a ledge face is within the body's reach of it: hop over.
                if let Some(m) = self.step(from, i + di, j + dj, Some(2.0 * CELL)) {
                    // Over a wall's foot: up the ledge, down off it, or off an edge into water.
                    let hop = matches!(m.kind, StepKind::Mantle | StepKind::Drop) || (m.kind == StepKind::Swim && from.1 == Cell::Floor);
                    if hop {
                        out.push(m);
                    }
                }
            }
        }
        out
    }

    /// The move from `from` to cell `(i, j)`, if one exists. `distance` overrides the
    /// distance between the centres (a two-cell hop).
    fn step(&mut self, from: (Vec3, Cell, bool), i: i32, j: i32, distance: Option<f32>) -> Option<Move> {
        let p = self.terrain.params;
        let (x, z) = (centre(i), centre(j));
        let dist = distance.unwrap_or_else(|| flat_distance(from.0, Vec3::new(x, from.0.y, z)));
        let probe = self.probe(i, j, from.0.y);
        match probe {
            Probe::Water { surface } => {
                let y = self.terrain.swim_height(surface);
                if from.1 == Cell::Floor && from.0.y - y > WATER_DROP {
                    return None;
                }
                Some(Move { pos: Vec3::new(x, y, z), cell: Cell::Swim, flat: true, kind: StepKind::Swim, cost: (dist * SWIM_FACTOR * UNIT) as u32, rise: 0.0 })
            }
            Probe::Floor { y, normal } => {
                let dy = y - from.0.y;
                let flat = normal.y > 0.999;
                let slope = p.max_slope_deg.to_radians().tan();
                let allowance = if flat && from.2 { 0.0 } else { dist * slope };
                let walkable_up = p.step_height + allowance;
                let pos = Vec3::new(x, y, z);
                let base = (dist * UNIT) as u32;
                if dy > p.mantle_height + 0.05 {
                    None
                } else if dy > walkable_up {
                    // Room to go up the face first.
                    let head = self.terrain.room_for(from.0.x, from.0.z, from.0.y + dy);
                    head.then(|| Move { pos, cell: Cell::Floor, flat, kind: StepKind::Mantle, cost: base + ((MANTLE_COST + dy * 0.5) * UNIT) as u32, rise: dy })
                } else if dy < -self.request.safe_drop {
                    None
                } else if dy < -(p.step_height + allowance) {
                    Some(Move { pos, cell: Cell::Floor, flat, kind: StepKind::Drop, cost: base + (-dy * DROP_COST * UNIT) as u32, rise: -dy })
                } else {
                    Some(Move { pos, cell: Cell::Floor, flat, kind: StepKind::Walk, cost: base, rise: 0.0 })
                }
            }
            Probe::Void | Probe::Blocked(_) => None,
        }
    }

    /// Walks the parents back into legs.
    fn finish(self, goal_node: usize, expanded: usize) -> Result<PathPlan, String> {
        let req = self.request;
        let mut chain = vec![goal_node];
        while let Some(parent) = self.nodes[*chain.last().unwrap_or(&goal_node)].parent {
            chain.push(parent);
        }
        chain.reverse();
        let mut steps: Vec<(Vec3, StepKind, f32)> = chain.iter().skip(1).map(|&n| (self.nodes[n].pos, self.nodes[n].kind, self.nodes[n].rise)).collect();
        // An exact point goal: end on it rather than on the nearest cell centre.
        if let Some((last, _, _)) = steps.last().copied()
            && req.within <= CELL
            && (0.05..=CELL).contains(&flat_distance(last, req.goal))
        {
            steps.push((Vec3::new(req.goal.x, last.y, req.goal.z), StepKind::Walk, 0.0));
        }
        let steps = self.straighten(self.nodes[chain[0]].pos, steps);
        let mut legs: Vec<Leg> = Vec::new();
        let mut at = req.start;
        let mut length = 0.0;
        for (pos, kind, rise) in steps {
            let delta = Vec3::new(pos.x - at.x, 0.0, pos.z - at.z);
            let len = delta.length();
            if len < 1e-4 {
                continue;
            }
            length += len;
            at = pos;
            // The few centimeters from where the walker stands to the first cell centre
            // are no leg of their own.
            if legs.is_empty() && len < 0.3 && kind == StepKind::Walk && rise == 0.0 {
                continue;
            }
            let dir = compass(delta);
            match legs.last_mut() {
                Some(leg) if leg.dir == dir && leg.kind == kind && matches!(kind, StepKind::Walk | StepKind::Swim) => {
                    leg.length += len;
                    leg.end = pos;
                }
                _ => legs.push(Leg { dir, length: len, kind, rise, end: pos }),
            }
        }
        if length > req.max_length {
            return Err(format!(
                "the shortest route to ({:.1}, {:.1}) is {length:.1}m, longer than max {:.0}m; raise `max` or go in stages",
                req.goal.x, req.goal.z, req.max_length
            ));
        }
        Ok(PathPlan { start: req.start, legs, length, explored: expanded })
    }

    /// Straightens runs of walking. Grid moves zigzag where a straight line would do (a
    /// slope between two compass directions is a staircase of both); a waypoint goes when a
    /// straight line from the one before to the one after is as walkable as the grid path.
    /// Climbs, drops and swims stay as the search found them.
    fn straighten(&self, from: Vec3, steps: Vec<(Vec3, StepKind, f32)>) -> Vec<(Vec3, StepKind, f32)> {
        let mut out = Vec::with_capacity(steps.len());
        let (mut anchor, mut run): (Vec3, Vec<Vec3>) = (from, Vec::new());
        for (pos, kind, rise) in steps {
            if kind == StepKind::Walk {
                run.push(pos);
                continue;
            }
            out.extend(self.shorten(anchor, &run).into_iter().map(|p| (p, StepKind::Walk, 0.0)));
            run.clear();
            out.push((pos, kind, rise));
            anchor = pos;
        }
        out.extend(self.shorten(anchor, &run).into_iter().map(|p| (p, StepKind::Walk, 0.0)));
        out
    }

    /// The points of `run` (walked on, in order, after `anchor`) that a straight-line
    /// version of it still needs: from each kept point, the farthest one a clear line reaches.
    fn shorten(&self, anchor: Vec3, run: &[Vec3]) -> Vec<Vec3> {
        // Long runs are not worth the lines: keep them as found.
        if run.len() > 600 {
            return run.to_vec();
        }
        let (mut kept, mut from, mut next) = (Vec::new(), anchor, 0);
        while next < run.len() {
            let mut far = run.len() - 1;
            while far > next && !self.line_clear(from, run[far]) {
                far -= 1;
            }
            kept.push(run[far]);
            from = run[far];
            next = far + 1;
        }
        kept
    }

    /// Whether walking straight from `a` to `b` stays on floor the body fits on, at heights
    /// that follow the line (so no wall, no drop, no steep ground in between).
    fn line_clear(&self, a: Vec3, b: Vec3) -> bool {
        let len = flat_distance(a, b);
        let samples = (len / 0.25).ceil() as usize;
        let tolerance = self.terrain.params.step_height + 0.1;
        (1..samples).all(|k| {
            let at = a.lerp(b, k as f32 / samples as f32);
            matches!(self.terrain.probe(at.x, at.z, at.y), Probe::Floor { y, .. } if (y - at.y).abs() <= tolerance)
        })
    }

    /// Why there is no route: where the search got closest and what stood in the way.
    fn explain(&mut self, closest: usize, within: f32, expanded: usize) -> String {
        let req = self.request;
        let p = self.terrain.params;
        let node = &self.nodes[closest];
        let (here, gap) = (node.pos, flat_distance(node.pos, req.goal));
        let to_goal = Vec3::new(req.goal.x - here.x, 0.0, req.goal.z - here.z).normalize_or_zero();
        let (ci, cj) = (cell_of(here.x), cell_of(here.z));
        let (di, dj) = *DIRS
            .iter()
            .max_by(|a, b| {
                let dot = |d: &(i32, i32)| Vec3::new(d.0 as f32, 0.0, d.1 as f32).normalize().dot(to_goal);
                dot(a).total_cmp(&dot(b))
            })
            .unwrap_or(&(0, 0));
        let (ni, nj) = (ci + di, cj + dj);
        let what = match self.probe(ni, nj, here.y) {
            Probe::Blocked(Block::Wall) | Probe::Blocked(Block::Tight) => {
                // The solid may be a cell or two further than the one the body cannot stand in.
                let high = (1..=3)
                    .filter_map(|k| self.terrain.top_of(centre(ci + di * k), centre(cj + dj * k), here.y, 40.0))
                    .map(|top| top - here.y)
                    .find(|h| *h > 0.3);
                match high {
                    Some(h) => format!("a wall {h:.1}m high (a character climbs at most {:.1}m)", p.mantle_height),
                    None => format!("a gap too narrow for the {:.1}m wide body", 2.0 * p.radius),
                }
            }
            Probe::Blocked(Block::Steep(angle)) => format!("a slope of {angle:.0}° (the limit is {:.0}°)", p.max_slope_deg),
            Probe::Void => "a drop with no floor in reach".to_owned(),
            Probe::Floor { y, .. } if y - here.y < -req.safe_drop => {
                format!("a cliff {:.1}m deep (the limit for a drop is {:.1}m)", here.y - y, req.safe_drop)
            }
            Probe::Floor { y, .. } if y - here.y > p.mantle_height => format!("a ledge {:.1}m high (a character climbs at most {:.1}m)", y - here.y, p.mantle_height),
            Probe::Floor { .. } | Probe::Water { .. } => "the edge of the search area".to_owned(),
        };
        let place = if gap <= within + CELL {
            "the goal itself is in or against something solid".to_owned()
        } else {
            format!("the closest the search got is {gap:.1}m short, at ({:.1}, {:.1}), where {what} is in the way", here.x, here.z)
        };
        let tail = if self.clipped {
            format!(
                "the search stopped at the edge of its area ({:.0}m beyond you and the goal), so a longer way around may \
                 exist: raise `max` to search wider, or go in stages",
                self.margin
            )
        } else {
            "the ground you can reach is cut off from it (walls, cliffs, or water you cannot leave): open a way, or aim for a \
             point you can reach"
                .to_owned()
        };
        format!("no walkable route to ({:.1}, {:.1}): {place}. {expanded} cells explored; {tail}", req.goal.x, req.goal.z)
    }
}
