//! Dungeons: rooms joined door to door, from an entrance to a goal.
//!
//! The main path is laid first: the entrance, then `path` rooms (the dungeon's hand-made
//! `fixed` rooms among them, at random places), then the goal, each room entering through
//! a door that faces a free door of the room before. The path is found by depth-first
//! search with bounded backtracking, so a dungeon that comes out always has a walk from the
//! entrance to the goal. Branches then hang extra rooms off free doors. Rooms are tried
//! in a weighted random order, turns and doors in a random order, all from [`DetRng`]
//! keyed by seed, version, attempt and stage, like grid regions.

use std::collections::{BTreeMap, BTreeSet};

use ai_core::DetRng;
use shards_content::ContentDb;

use crate::checks::{self, CheckInput, RegionGraph};
use crate::content::{resolve_set, DungeonDef, RoomDef};
use crate::generate::{AttemptFailure, GenError, Generated, Stage, WorldGen};
use crate::grid::{degrees_of, mm, turn, turn_cell, turns_of, Dir};
use crate::manifest::{content_hash, fnv, Layout, ManifestLink, ManifestPoi, PlacedTile, RegionManifest, FORMAT};

/// A room turned: its footprint and doors.
#[derive(Clone, Debug)]
struct Shape {
    size: (u32, u32),
    doors: Vec<((u32, u32), Dir)>,
}

fn shape(room: &RoomDef, turns: u8) -> Shape {
    let mut size = room.size;
    let mut doors: Vec<((u32, u32), Dir)> = room.doors.iter().map(|d| (d.at, d.side)).collect();
    for _ in 0..turns {
        doors = doors.into_iter().map(|(cell, side)| (turn_cell(cell, size), side.turned(1))).collect();
        size = (size.1, size.0);
    }
    Shape { size, doors }
}

/// A room in the dungeon being built.
#[derive(Clone, Debug)]
struct Placed {
    room: usize,
    turns: u8,
    /// North-west cell.
    at: (i32, i32),
    shape: Shape,
    used: Vec<bool>,
}

impl Placed {
    fn door_cell(&self, door: usize) -> (i32, i32) {
        let ((x, y), _) = self.shape.doors[door];
        (self.at.0 + x as i32, self.at.1 + y as i32)
    }

    fn cells(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        let (w, h) = self.shape.size;
        (0..h as i32).flat_map(move |y| (0..w as i32).map(move |x| (self.at.0 + x, self.at.1 + y)))
    }
}

/// A door-to-door connection: (room, door) on each side.
type Joint = ((usize, usize), (usize, usize));

struct Builder<'a> {
    def: &'a DungeonDef,
    rooms: Vec<(&'a str, &'a RoomDef)>,
    /// Indices into `rooms` usable as fillers.
    fillers: Vec<usize>,
    placed: Vec<Placed>,
    occupied: BTreeMap<(i32, i32), usize>,
    joints: Vec<Joint>,
    counts: Vec<u32>,
    backtracks: u32,
    rng: DetRng,
}

/// What to put in the next place on the main path.
#[derive(Clone, Copy, Debug)]
enum Want {
    Room(usize),
    Filler,
}

impl Builder<'_> {
    fn turns_of(&self, room: usize) -> Vec<u8> {
        let mut turns: Vec<u8> = self.rooms[room].1.rotations.iter().filter_map(|r| turns_of(*r)).collect();
        turns.sort_unstable();
        turns.dedup();
        turns
    }

    /// Rooms in a weighted random order (drawn without replacement).
    fn room_order(&mut self, want: Want) -> Vec<usize> {
        let mut pool: Vec<usize> = match want {
            Want::Room(r) => vec![r],
            Want::Filler => self
                .fillers
                .iter()
                .copied()
                .filter(|&r| self.rooms[r].1.max_count.is_none_or(|max| self.counts[r] < max))
                .collect(),
        };
        let mut order = Vec::with_capacity(pool.len());
        while !pool.is_empty() {
            let weights: Vec<u64> = pool.iter().map(|&r| (self.rooms[r].1.weight.max(0.0) * 1000.0).round() as u64).collect();
            let total: u64 = weights.iter().sum();
            let pick = if total == 0 {
                0
            } else {
                let mut x = ((self.rng.next_u64() as u128 * total as u128) >> 64) as u64;
                weights.iter().position(|w| {
                    if x < *w {
                        true
                    } else {
                        x -= w;
                        false
                    }
                })
                .unwrap_or(0)
            };
            order.push(pool.remove(pick));
        }
        order
    }

    fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.rng.below(i as u32 + 1) as usize;
            items.swap(i, j);
        }
    }

    /// Where `room` turned `turns` goes so that its door `door` faces door `from_door` of
    /// placed room `from`; `None` when it would overlap or break the size limit.
    fn fit(&self, from: usize, from_door: usize, room: usize, turns: u8, door: usize) -> Option<Placed> {
        let base = &self.placed[from];
        let side = base.shape.doors[from_door].1;
        let s = shape(self.rooms[room].1, turns);
        let ((dx, dy), door_side) = s.doors[door];
        if door_side != side.opposite() {
            return None;
        }
        let (fx, fy) = base.door_cell(from_door);
        let (ox, oy) = side.offset();
        let target = (fx + ox, fy + oy);
        let at = (target.0 - dx as i32, target.1 - dy as i32);
        let used = vec![false; s.doors.len()];
        let candidate = Placed { room, turns, at, shape: s, used };
        if candidate.cells().any(|c| self.occupied.contains_key(&c)) {
            return None;
        }
        if let Some((mw, mh)) = self.def.max_size {
            let cells = self.occupied.keys().copied().chain(candidate.cells());
            let (mut min, mut max) = ((i32::MAX, i32::MAX), (i32::MIN, i32::MIN));
            for (x, y) in cells {
                min = (min.0.min(x), min.1.min(y));
                max = (max.0.max(x), max.1.max(y));
            }
            if (max.0 - min.0 + 1) as u32 > mw || (max.1 - min.1 + 1) as u32 > mh {
                return None;
            }
        }
        Some(candidate)
    }

    fn put(&mut self, mut room: Placed, joint: Option<(usize, usize, usize)>) -> usize {
        let index = self.placed.len();
        for cell in room.cells().collect::<Vec<_>>() {
            self.occupied.insert(cell, index);
        }
        if let Some((from, from_door, door)) = joint {
            room.used[door] = true;
            self.placed[from].used[from_door] = true;
            self.joints.push(((from, from_door), (index, door)));
        }
        self.counts[room.room] += 1;
        self.placed.push(room);
        index
    }

    fn take_back(&mut self) {
        let Some(room) = self.placed.pop() else { return };
        let index = self.placed.len();
        self.occupied.retain(|_, i| *i != index);
        self.counts[room.room] -= 1;
        if let Some(((from, from_door), _)) = self.joints.pop() {
            self.placed[from].used[from_door] = false;
        }
    }

    /// Lays `path[k..]` after the last placed room, depth first. `Err` when the backtrack
    /// budget ran out.
    fn lay(&mut self, path: &[Want], k: usize) -> Result<bool, ()> {
        let Some(&want) = path.get(k) else { return Ok(true) };
        let from = self.placed.len() - 1;
        let mut doors: Vec<usize> = (0..self.placed[from].shape.doors.len()).filter(|&d| !self.placed[from].used[d]).collect();
        self.shuffle(&mut doors);
        let last = k + 1 == path.len();
        for room in self.room_order(want) {
            if !last && self.rooms[room].1.doors.len() < 2 {
                continue;
            }
            let mut tries: Vec<(u8, usize, usize)> = Vec::new();
            for turns in self.turns_of(room) {
                for &from_door in &doors {
                    for door in 0..self.rooms[room].1.doors.len() {
                        tries.push((turns, from_door, door));
                    }
                }
            }
            self.shuffle(&mut tries);
            for (turns, from_door, door) in tries {
                let Some(candidate) = self.fit(from, from_door, room, turns, door) else { continue };
                self.put(candidate, Some((from, from_door, door)));
                if self.lay(path, k + 1)? {
                    return Ok(true);
                }
                self.take_back();
                self.backtracks += 1;
                if self.backtracks > self.def.backtracks {
                    return Err(());
                }
            }
        }
        Ok(false)
    }

    /// Hangs one filler room off a free door somewhere; whether one fit.
    fn branch(&mut self) -> bool {
        let mut free: Vec<(usize, usize)> = Vec::new();
        for (p, placed) in self.placed.iter().enumerate() {
            free.extend(placed.used.iter().enumerate().filter(|(_, used)| !**used).map(|(d, _)| (p, d)));
        }
        self.shuffle(&mut free);
        for (from, from_door) in free {
            for room in self.room_order(Want::Filler) {
                let mut tries: Vec<(u8, usize)> =
                    self.turns_of(room).into_iter().flat_map(|t| (0..self.rooms[room].1.doors.len()).map(move |d| (t, d))).collect();
                self.shuffle(&mut tries);
                for (turns, door) in tries {
                    if let Some(candidate) = self.fit(from, from_door, room, turns, door) {
                        self.put(candidate, Some((from, from_door, door)));
                        return true;
                    }
                }
            }
        }
        false
    }
}

/// Generates dungeon `id` for `seed`.
pub fn generate(db: &ContentDb, worldgen: &WorldGen, id: &str, def: &DungeonDef, seed: u64) -> Result<Generated, GenError> {
    let all: Vec<(&str, &RoomDef)> = db.iter::<RoomDef>().collect();
    let filler_ids = resolve_set(&def.rooms, all.iter().copied(), |r: &RoomDef| &r.tags, "room").map_err(|e| format!("dungeon `{id}`: rooms: {e}"))?;
    let index = |room: &str| all.iter().position(|(r, _)| *r == room).ok_or_else(|| format!("dungeon `{id}`: unknown room `{room}`"));
    let entrance = index(&def.entrance)?;
    let goal = index(&def.goal)?;
    let fixed: Vec<usize> = def.fixed.iter().map(|r| index(r)).collect::<Result<_, _>>()?;
    let fillers: Vec<usize> = filler_ids.iter().map(|r| index(r)).collect::<Result<_, _>>()?;
    let mut parts: Vec<(&str, &str)> = vec![("dungeon", id)];
    let used: BTreeSet<usize> = fillers.iter().chain(&fixed).chain([&entrance, &goal]).copied().collect();
    parts.extend(used.iter().map(|&r| ("room", all[r].0)));
    let pois: BTreeSet<&str> = used
        .iter()
        .flat_map(|&r| all[r].1.pois.iter().map(|p| p.poi.as_str()).chain(all[r].1.slots.iter().flat_map(|s| s.accepts.iter().map(String::as_str))))
        .chain(def.secondary.iter().map(|r| r.poi.as_str()))
        .chain([def.spawn.as_str()])
        .collect();
    parts.extend(pois.into_iter().map(|p| ("poi", p)));
    let hash = content_hash(db, &parts);

    let mut failures = Vec::new();
    for attempt in 0..def.attempts {
        let key = |stage: &str| DetRng::from_keys(&[seed, def.version as u64, attempt as u64, fnv(stage.as_bytes())]);
        let mut b = Builder {
            def,
            rooms: all.clone(),
            fillers: fillers.clone(),
            placed: Vec::new(),
            occupied: BTreeMap::new(),
            joints: Vec::new(),
            counts: vec![0; all.len()],
            backtracks: 0,
            rng: key("dungeon/path"),
        };
        let fail = |stage: Stage, rule: &str, message: String| AttemptFailure {
            attempt: attempt + 1,
            stage,
            step: "engine.dungeon".into(),
            rule: format!("{stage}: {rule}"),
            message: format!("{stage}: {message}"),
            cells: Vec::new(),
        };
        // The main path: entrance, the fixed rooms at random places among the fillers, goal. It
        // is never shorter than the fixed list: a room the author put there is not dropped.
        let shortest = def.path.0.max(fixed.len() as u32);
        let length = shortest + b.rng.below(def.path.1.max(shortest) - shortest + 1);
        let mut middle = vec![Want::Filler; length as usize];
        let mut places: Vec<usize> = (0..middle.len()).collect();
        b.shuffle(&mut places);
        for (slot, room) in places.into_iter().zip(&fixed) {
            middle[slot] = Want::Room(*room);
        }
        let mut path = middle;
        path.push(Want::Room(goal));
        let mut entrance_turns = b.turns_of(entrance);
        b.shuffle(&mut entrance_turns);
        let start = Placed {
            room: entrance,
            turns: entrance_turns.first().copied().unwrap_or(0),
            at: (0, 0),
            shape: shape(all[entrance].1, entrance_turns.first().copied().unwrap_or(0)),
            used: vec![false; all[entrance].1.doors.len()],
        };
        b.put(start, None);
        match b.lay(&path, 0) {
            Ok(true) => {}
            Ok(false) => {
                failures.push(fail(Stage::Fill, "no path", format!("no path: rooms cannot be joined from `{}` to `{}` in {length} steps", def.entrance, def.goal)));
                continue;
            }
            Err(()) => {
                failures.push(fail(Stage::Fill, "solver gave up", format!("solver gave up after {} backtracks laying the main path", def.backtracks)));
                continue;
            }
        }
        b.rng = key("dungeon/branches");
        let branches = def.branches.0 + b.rng.below(def.branches.1 - def.branches.0 + 1);
        let mut grown = 0;
        while grown < branches && b.branch() {
            grown += 1;
        }
        if grown < def.branches.0 {
            failures.push(fail(Stage::Fill, "no branch", format!("no branch: only {grown} of at least {} branches found room", def.branches.0)));
            continue;
        }
        let mut rng = key("dungeon/secondary");
        let (manifest, remarks) = finish(&b, def, id, seed, attempt, &hash, &mut rng);
        let graph = match RegionGraph::build(db, &manifest) {
            Ok(graph) => graph,
            Err(e) => return Err(GenError { message: e, failures }),
        };
        let input = CheckInput { manifest: &manifest, rules: &def.checks, graph: &graph, db };
        let mut problems = checks::engine_rules(&input);
        problems.extend(checks::custom_checks(worldgen, &input));
        if let Some(first) = problems.first() {
            failures.push(AttemptFailure {
                attempt: attempt + 1,
                stage: Stage::Checks,
                step: "engine.checks".into(),
                rule: format!("check {}", first.rule),
                message: first.to_string(),
                cells: first.cells.clone(),
            });
            continue;
        }
        return Ok(Generated { manifest, failures, remarks });
    }
    let last = failures.last().map(|f| format!("; the last: {}", f.message)).unwrap_or_default();
    Err(GenError {
        message: format!("dungeon `{id}` seed {seed}: no attempt of {} passed{last}; give it more `attempts`, `backtracks` or rooms", def.attempts),
        failures,
    })
}

/// The manifest of a laid dungeon: rooms by their north-west cell (row-major), doors,
/// points of interest (rooms' own, then secondary ones in free slots).
fn finish(b: &Builder, def: &DungeonDef, id: &str, seed: u64, attempt: u32, hash: &str, rng: &mut DetRng) -> (RegionManifest, Vec<String>) {
    let min_x = b.occupied.keys().map(|c| c.0).min().unwrap_or(0);
    let min_y = b.occupied.keys().map(|c| c.1).min().unwrap_or(0);
    let max_x = b.occupied.keys().map(|c| c.0).max().unwrap_or(0);
    let max_y = b.occupied.keys().map(|c| c.1).max().unwrap_or(0);
    let shift = |(x, y): (i32, i32)| [x - min_x, y - min_y];
    let mut order: Vec<usize> = (0..b.placed.len()).collect();
    order.sort_by_key(|&p| (b.placed[p].at.1, b.placed[p].at.0));
    let rank: Vec<usize> = {
        let mut rank = vec![0; order.len()];
        for (r, &p) in order.iter().enumerate() {
            rank[p] = r;
        }
        rank
    };
    let size = def.cell_size;
    let center = |p: &Placed| {
        let [x, y] = shift(p.at);
        bevy::math::Vec3::new((x as f32 + p.shape.size.0 as f32 * 0.5) * size, 0.0, (y as f32 + p.shape.size.1 as f32 * 0.5) * size)
    };
    let tiles: Vec<PlacedTile> = order
        .iter()
        .map(|&p| {
            let placed = &b.placed[p];
            PlacedTile {
                at: shift(placed.at),
                size: [placed.shape.size.0, placed.shape.size.1],
                tile: b.rooms[placed.room].0.to_owned(),
                rot: degrees_of(placed.turns),
                decor: None,
            }
        })
        .collect();
    let mut counters: BTreeMap<String, u32> = BTreeMap::new();
    let mut next_id = |poi: &str| {
        let n = counters.entry(poi.to_owned()).or_default();
        let id = format!("{poi}.{n}");
        *n += 1;
        id
    };
    let cell_of = |p: &Placed, pos: bevy::math::Vec3| {
        let [x, y] = shift(p.at);
        [((pos.x / size).floor() as i32).clamp(x, x + p.shape.size.0 as i32 - 1), ((pos.z / size).floor() as i32).clamp(y, y + p.shape.size.1 as i32 - 1)]
    };
    let mut pois = Vec::new();
    for &p in &order {
        let placed = &b.placed[p];
        for own in &b.rooms[placed.room].1.pois {
            let pos = center(placed) + turn(own.at, placed.turns);
            pois.push(ManifestPoi { id: next_id(&own.poi), poi: own.poi.clone(), cell: cell_of(placed, pos), pos: mm(pos), area: 0 });
        }
    }
    let mut remarks = Vec::new();
    for rule in &def.secondary {
        let mut spots: Vec<(usize, usize)> = order
            .iter()
            .flat_map(|&p| {
                b.rooms[b.placed[p].room].1.slots.iter().enumerate().filter(|(_, s)| s.accepts.contains(&rule.poi)).map(move |(k, _)| (p, k))
            })
            .collect();
        let mut placed_n = 0;
        while placed_n < rule.count && !spots.is_empty() {
            let (p, k) = spots.remove(rng.below(spots.len() as u32) as usize);
            let placed = &b.placed[p];
            let pos = center(placed) + turn(b.rooms[placed.room].1.slots[k].at, placed.turns);
            pois.push(ManifestPoi { id: next_id(&rule.poi), poi: rule.poi.clone(), cell: cell_of(placed, pos), pos: mm(pos), area: 0 });
            placed_n += 1;
        }
        if placed_n < rule.count {
            remarks.push(format!("secondary `{}`: {placed_n} of {} found a free slot", rule.poi, rule.count));
        }
    }
    let mut links: Vec<ManifestLink> = b
        .joints
        .iter()
        .map(|&((a, da), (c, dc))| {
            let (ra, rc) = (rank[a], rank[c]);
            let (ca, cc) = (shift(b.placed[a].door_cell(da)), shift(b.placed[c].door_cell(dc)));
            let (first, second, cells) = if ra <= rc { (ra, rc, vec![ca, cc]) } else { (rc, ra, vec![cc, ca]) };
            ManifestLink { kind: "door".into(), a: format!("room.{first}"), b: format!("room.{second}"), cells }
        })
        .collect();
    links.sort_by(|x, y| (&x.a, &x.b, &x.cells).cmp(&(&y.a, &y.b, &y.cells)));
    let spawn = pois.iter().find(|p| p.poi == def.spawn).map(|p| p.id.clone());
    let manifest = RegionManifest {
        format: FORMAT.into(),
        region: id.to_owned(),
        layout: Layout::Rooms,
        seed,
        version: def.version,
        attempt: attempt + 1,
        fallback: None,
        size: [(max_x - min_x + 1) as u32, (max_y - min_y + 1) as u32],
        tile_size: size,
        tiles,
        pois,
        links,
        spawn,
        content_hash: hash.to_owned(),
    };
    (manifest, remarks)
}
