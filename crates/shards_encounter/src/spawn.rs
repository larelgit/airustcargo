//! Spawning a wave: counts, points, the facing filter, the ground, the patch.
//!
//! Groups are spawned in definition order and the members of a group in index order, each with
//! `spawn_prefab`, so the `AiId`s follow the definition's order. Randomness comes from a
//! `DetRng` keyed by the seed, the tick and the run key, wave, group and purpose (the way the
//! NPC brains key theirs), so two runs never share numbers and a replay with or without a
//! window draws the same ones. Facing is read from the participants' bodies and their `Intent`
//! (recorded input), never from a camera.

use ai_core::journal::{self, JournalKind};
use ai_core::{AiId, AiIndex, DetRng, SimSeed};
use bevy::math::ops;
use bevy::prelude::*;
use bevy::reflect::serde::TypedReflectSerializer;
use serde_json::{json, Value};
use shards_content::spawn_prefab;
use shards_level::LevelMarker;
use shards_motion::CharacterParams;
use shards_npc::{navmesh_route, region_nav, Home};
use shards_world::region::ground_below;
use shards_world::{Marker, Poi, RegionPart, TileId};

use crate::def::{AvoidBehind, EncounterDef, Face, HomeAt, MemberGroup, SpawnAt};
use crate::run::{
    EncounterMember, Member, Run, NOTE_ALL_BEHIND, NOTE_NOT_WALKABLE, NOTE_NO_CANDIDATES, NOTE_TOO_FEW,
};

/// A player body as the step sees it this tick.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Part {
    pub entity: Entity,
    pub pos: Vec3,
    /// Flat unit vector the body faces: its lock-on target, else its aim, else its forward.
    pub facing: Vec3,
    pub out: bool,
}

/// What a draw is for: separates the streams of one group.
mod purpose {
    pub const COUNT: u64 = 1;
    pub const POINTS: u64 = 2;
    pub const RING: u64 = 3;
}

/// FNV-1a, 64 bit: the same on every machine and build (a `DefaultHasher` is randomized).
pub(crate) fn fnv(text: &str) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in text.as_bytes() {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

/// The stream for one purpose of one group: `from_keys(seed, tick, run key, wave, group,
/// purpose)`.
fn rng(world: &World, key: &str, wave: u32, group: usize, purpose: u64, extra: u64) -> DetRng {
    let seed = world.get_resource::<SimSeed>().map_or(0, |s| s.0);
    let tick = ai_core::sim::tick(world);
    DetRng::from_keys(&[seed, tick, fnv(key), u64::from(wave), group as u64, purpose, extra])
}

fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

/// Quarter-turn-free yaw: the rotation about Y of `yaw` radians (the anchor's).
fn turn(yaw: f32) -> Quat {
    Quat::from_rotation_y(yaw)
}

/// Spawns wave `wave` (1-based) of `run` and joins the members to `run.members` in `AiId`
/// order. Returns the entities in spawn order.
pub(crate) fn spawn_wave(
    world: &mut World,
    run: &mut Run,
    def: &EncounterDef,
    wave: u32,
    parts: &[Part],
    tile: Option<TileId>,
) -> Vec<Entity> {
    let mut spawned = Vec::new();
    let Some(wave_def) = def.waves.get(wave.saturating_sub(1) as usize) else { return spawned };
    for (g, group) in wave_def.members.iter().enumerate() {
        let count = match group.up_to {
            Some(up_to) if up_to > group.count => {
                group.count + rng(world, &run.key, wave, g, purpose::COUNT, 0).below(up_to - group.count + 1)
            }
            _ => group.count,
        };
        let points = points_for(world, run, def, wave, g, group, count as usize, parts);
        for (i, point) in points.into_iter().enumerate() {
            let Some(entity) = spawn_member(world, run, group, point, i, wave, tile) else { continue };
            spawned.push(entity);
        }
    }
    run.members.sort_by_key(|m| m.id);
    spawned
}

/// Where the `count` members of group `g` stand: `(point, ground height applied)`.
#[allow(clippy::too_many_arguments)]
fn points_for(
    world: &mut World,
    run: &mut Run,
    def: &EncounterDef,
    wave: u32,
    g: usize,
    group: &MemberGroup,
    count: usize,
    parts: &[Part],
) -> Vec<Vec3> {
    let anchor = run.at;
    let notes_before = run.noted;
    let points = match &group.at {
        SpawnAt::Offsets(offsets) => {
            let rot = turn(run.yaw);
            (0..count).map(|i| anchor + rot * offsets.get(i).copied().unwrap_or(Vec3::ZERO)).map(|p| grounded(world, p)).collect()
        }
        SpawnAt::Marker(name) => {
            let candidates = marker_points(world, name, anchor, def.radius);
            choose(world, run, wave, g, group, count, candidates, parts, true)
        }
        SpawnAt::Poi(poi) => {
            let candidates = poi_points(world, poi, anchor, def.radius);
            choose(world, run, wave, g, group, count, candidates, parts, true)
        }
        SpawnAt::Ring { min, max } => ring_points(world, run, wave, g, group, count, *min, *max, parts),
    };
    // A note is said once per run, when the bit is first set.
    let new = run.noted & !notes_before;
    if new != 0 {
        say_notes(world, run, def, g, group, new);
    }
    points
}

fn grounded(world: &mut World, point: Vec3) -> Vec3 {
    match ground_below(world, point, &[]) {
        Some(y) => Vec3::new(point.x, y, point.z),
        None => point,
    }
}

fn mm(v: f32) -> i64 {
    (v * 1000.0).round() as i64
}

/// `Marker` and `LevelMarker` entities named `name` within `radius` (flat) of `anchor`, sorted
/// by name, then position (x, z, y in millimetres).
fn marker_points(world: &mut World, name: &str, anchor: Vec3, radius: f32) -> Vec<Vec3> {
    let mut found: Vec<Vec3> = Vec::new();
    let mut markers = world.query::<(&Marker, &Transform)>();
    found.extend(markers.iter(world).filter(|(m, _)| m.name == name).map(|(_, t)| t.translation));
    let mut level = world.query::<(&LevelMarker, &Transform)>();
    found.extend(level.iter(world).filter(|(m, _)| m.name == name).map(|(_, t)| t.translation));
    found.retain(|p| flat(*p - anchor).length() <= radius);
    found.sort_by_key(|p| (mm(p.x), mm(p.z), mm(p.y)));
    found.dedup();
    found
}

/// `Poi` entities of type `poi` within `radius` of `anchor`, sorted by `Poi.id`.
fn poi_points(world: &mut World, poi: &str, anchor: Vec3, radius: f32) -> Vec<Vec3> {
    let mut q = world.query::<(&Poi, &Transform)>();
    let mut found: Vec<(String, Vec3)> =
        q.iter(world).filter(|(p, t)| p.poi == poi && flat(t.translation - anchor).length() <= radius).map(|(p, t)| (p.id.clone(), t.translation)).collect();
    found.sort_by(|a, b| a.0.cmp(&b.0).then((mm(a.1.x), mm(a.1.z), mm(a.1.y)).cmp(&(mm(b.1.x), mm(b.1.z), mm(b.1.y)))));
    found.into_iter().map(|(_, p)| p).collect()
}

/// Whether the navmesh can walk from the anchor to `point`. Without a navmesh for the region (a
/// hand-made level, a voxel region) every point counts as walkable: nothing says otherwise.
/// Judged for a body of the default `CharacterParams`.
fn walkable(world: &World, anchor: Vec3, point: Vec3) -> bool {
    if region_nav(world).is_none() {
        return true;
    }
    matches!(navmesh_route(world, anchor, point, &CharacterParams::default(), false), Some(Ok(_)))
}

/// Whether `point` is inside the rear cone of a standing participant that is near it.
fn behind(point: Vec3, avoid: &AvoidBehind, parts: &[Part]) -> bool {
    // Angle between the facing and the direction to the point greater than 180 - angle / 2
    // degrees: its cosine is below -cos(angle / 2).
    let limit = -ops::cos((avoid.angle * 0.5).to_radians());
    parts.iter().filter(|p| !p.out).any(|p| {
        let to = flat(point - p.pos);
        let distance = to.length();
        if distance >= avoid.within || distance <= f32::EPSILON {
            return false;
        }
        (to / distance).dot(p.facing) < limit
    })
}

/// How far `point` is from the nearest standing participant.
fn nearest_participant(point: Vec3, parts: &[Part]) -> f32 {
    parts.iter().filter(|p| !p.out).map(|p| flat(point - p.pos).length()).fold(f32::INFINITY, f32::min)
}

/// Chooses `count` points among the sorted `candidates` of a marker or a POI: grounded, walkable
/// when the navmesh says, not behind a participant, drawn without repeats by a shuffle.
#[allow(clippy::too_many_arguments)]
fn choose(
    world: &mut World,
    run: &mut Run,
    wave: u32,
    g: usize,
    group: &MemberGroup,
    count: usize,
    candidates: Vec<Vec3>,
    parts: &[Part],
    check_walk: bool,
) -> Vec<Vec3> {
    if candidates.is_empty() {
        run.noted |= NOTE_NO_CANDIDATES;
        return vec![run.at; count];
    }
    let mut pool: Vec<Vec3> = candidates.into_iter().map(|p| grounded(world, p)).collect();
    if check_walk {
        let anchor = run.at;
        let walk: Vec<Vec3> = pool.iter().copied().filter(|p| walkable(world, anchor, *p)).collect();
        if walk.is_empty() {
            // A marker the author placed is used all the same; the journal says it is off the mesh.
            run.noted |= NOTE_NOT_WALKABLE;
        } else {
            pool = walk;
        }
    }
    if let Some(avoid) = &group.avoid_behind {
        let kept: Vec<Vec3> = pool.iter().copied().filter(|p| !behind(*p, avoid, parts)).collect();
        if kept.is_empty() {
            // Every point is behind someone: the farthest from the nearest participant.
            run.noted |= NOTE_ALL_BEHIND;
            let farthest = pool
                .iter()
                .copied()
                .fold((f32::NEG_INFINITY, pool[0]), |best, p| {
                    let d = nearest_participant(p, parts);
                    if d > best.0 { (d, p) } else { best }
                })
                .1;
            pool = vec![farthest];
        } else {
            pool = kept;
        }
    }
    // A shuffle (Fisher-Yates) from the group's point stream.
    let mut stream = rng(world, &run.key, wave, g, purpose::POINTS, 0);
    for i in (1..pool.len()).rev() {
        let j = stream.below(i as u32 + 1) as usize;
        pool.swap(i, j);
    }
    if count > pool.len() {
        run.noted |= NOTE_TOO_FEW;
    }
    (0..count).map(|i| pool[i % pool.len()]).collect()
}

/// Ring points: for each member, 8 candidates (angle and distance from the stream); the first
/// that is grounded, walkable and not behind a participant is used.
#[allow(clippy::too_many_arguments)]
fn ring_points(
    world: &mut World,
    run: &mut Run,
    wave: u32,
    g: usize,
    group: &MemberGroup,
    count: usize,
    min: f32,
    max: f32,
    parts: &[Part],
) -> Vec<Vec3> {
    let anchor = run.at;
    let mut stream = rng(world, &run.key, wave, g, purpose::RING, 0);
    let mut out = Vec::with_capacity(count);
    for _ in 0..count {
        let mut walk: Vec<Vec3> = Vec::new();
        let mut all: Vec<Vec3> = Vec::new();
        for _ in 0..8 {
            let angle = stream.f32() * std::f32::consts::TAU;
            let distance = stream.range(min, max.max(min));
            let (sin, cos) = ops::sin_cos(angle);
            let point = grounded(world, anchor + Vec3::new(cos * distance, 0.0, sin * distance));
            all.push(point);
            if walkable(world, anchor, point) {
                walk.push(point);
            }
        }
        if walk.is_empty() {
            // None is on the navmesh (an anchor off the mesh makes every route fail): the grounded
            // candidates are used all the same, as `choose` does for markers, and the journal says it.
            run.noted |= NOTE_NOT_WALKABLE;
            walk = all;
        }
        let open: Vec<Vec3> = match &group.avoid_behind {
            Some(avoid) => walk.iter().copied().filter(|p| !behind(*p, avoid, parts)).collect(),
            None => walk.clone(),
        };
        out.push(match open.first() {
            Some(p) => *p,
            None => {
                run.noted |= NOTE_ALL_BEHIND;
                walk.iter().copied().fold((f32::NEG_INFINITY, walk[0]), |best, p| {
                    let d = nearest_participant(p, parts);
                    if d > best.0 { (d, p) } else { best }
                }).1
            }
        });
    }
    out
}

/// The one-time journal lines for the notes `new` just set.
fn say_notes(world: &mut World, run: &Run, def: &EncounterDef, g: usize, group: &MemberGroup, new: u8) {
    let key = &run.key;
    let what = match &group.at {
        SpawnAt::Marker(name) => format!("marker `{name}`"),
        SpawnAt::Poi(poi) => format!("point of interest `{poi}`"),
        _ => "ring point".to_owned(),
    };
    let mut say = |kind: JournalKind, text: String| {
        journal::emit(world, kind, format!("encounter {key}: {text}"));
    };
    if new & NOTE_NO_CANDIDATES != 0 {
        say(JournalKind::Warn, format!("no {what} within {:.0} m of its anchor (group {g}); spawned at the anchor", def.radius));
    }
    if new & NOTE_ALL_BEHIND != 0 {
        say(JournalKind::Warn, "every spawn point is behind a player; used the farthest".to_owned());
    }
    if new & NOTE_TOO_FEW != 0 {
        say(JournalKind::Warn, format!("group {g} wants more members than it has {what} points; some share a point"));
    }
    if new & NOTE_NOT_WALKABLE != 0 {
        say(JournalKind::Warn, format!("no {what} of group {g} is on the navmesh; check where it stands"));
    }
}

/// The patch `spawn_prefab` takes: the group's overrides, and the `Home` the group asks for.
fn patch_for(world: &World, group: &MemberGroup, run: &Run, point: Vec3) -> Value {
    let mut patch: serde_json::Map<String, Value> = group.overrides.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    if group.home == HomeAt::Anchor || group.leash.is_some() {
        let home = if group.home == HomeAt::Anchor { run.at } else { point };
        let registry = world.resource::<AppTypeRegistry>().read();
        let value = Home { pos: Some(home), leash: group.leash.unwrap_or_default() };
        // Written the way reflection reads it back: an `Option<Vec3>` is not a hand-made form.
        if let Ok(Value::Object(mut json)) = serde_json::to_value(TypedReflectSerializer::new(&value, &registry)) {
            if group.leash.is_none() {
                // The prefab's own leash stays.
                json.remove("leash");
            }
            patch.insert("Home".to_owned(), Value::Object(json));
        }
    }
    json!(patch)
}

/// Spawns one member at `point`, marks it and lists it. `None` (and a journal error) when the
/// prefab does not spawn or has no `AiId`.
fn spawn_member(world: &mut World, run: &mut Run, group: &MemberGroup, point: Vec3, _i: usize, wave: u32, tile: Option<TileId>) -> Option<Entity> {
    let toward = match group.face {
        Face::Center => {
            let to = flat(run.at - point);
            if to.length() > 1e-3 { to.normalize() } else { turn(run.yaw) * Vec3::NEG_Z }
        }
        Face::Anchor => turn(run.yaw) * Vec3::NEG_Z,
        Face::Yaw(degrees) => turn(run.yaw + degrees.to_radians()) * Vec3::NEG_Z,
    };
    let transform = Transform::from_translation(point).looking_to(toward, Vec3::Y);
    let patch = patch_for(world, group, run, point);
    let entity = match spawn_prefab(world, &group.prefab, transform, Some(&patch)) {
        Ok(entity) => entity,
        Err(why) => {
            journal::emit(world, JournalKind::Error, format!("encounter {}: cannot spawn `{}`: {why}", run.key, group.prefab));
            return None;
        }
    };
    let Some(id) = world.get::<AiId>(entity).copied().filter(|id| id.0 != 0) else {
        journal::emit(
            world,
            JournalKind::Error,
            format!("encounter {}: prefab `{}` has no AiId (give it AiKind), so the run cannot count it", run.key, group.prefab),
        );
        world.despawn(entity);
        return None;
    };
    let mut e = world.entity_mut(entity);
    e.insert((EncounterMember { key: run.key.clone(), wave, counted: true, returning: false }, RegionPart));
    if let Some(tile) = tile {
        e.insert(tile);
    }
    run.members.push(Member { id, wave, counted: true });
    Some(entity)
}

/// The members' `AiId`s resolved in the world, in the order given; ids that are gone are left
/// out.
pub(crate) fn resolve(world: &World, ids: &[AiId]) -> Vec<(AiId, Entity)> {
    let Some(index) = world.get_resource::<AiIndex>() else { return Vec::new() };
    ids.iter().filter_map(|id| index.get(*id).map(|e| (*id, e))).collect()
}
