//! Spawning a level: the gameplay nodes of a [`LevelAsset`] become entities.
//!
//! - **Top-level, world-space.** Physics reads `Transform`, not `GlobalTransform`, and
//!   transforms are not propagated inside fixed ticks, so every entity is spawned without a
//!   parent at `at` composed with the node's pose. Scale is baked into collider sizes; the
//!   entity's own scale is 1.
//! - **Deterministic.** Entities are spawned in the order of the file's `nodes` array, so
//!   `AiId`s and `ColliderId`s are the same in every run, with a window or without.
//! - **All or nothing.** The whole file is read into plans first (components built,
//!   prefabs checked); when anything is wrong nothing is spawned and the error lists every
//!   problem with its node.
//! - **Visuals** (the `render` feature) hang under one separate root entity that carries no
//!   gameplay component, so a headless run and a windowed one have the same gameplay
//!   entities.

use ai_core::prelude::*;
use ai_core::replay::ReplayHashing;
use bevy::prelude::*;
use serde_json::Value;
use shards_content::{spawn_prefab, ContentDb};
use shards_motion::{Collider, Shape, Volume};

use crate::asset::{Bounds, Fnv, LevelAsset};
use crate::library::level_asset;
use crate::marker::{LevelMarker, MarkerKind};
use crate::props::{analyze, ColliderSpec, LevelProblem, MarkerSpec, NodePlan, ShapeSpec};

/// Where an entity came from: the level file's node. Every entity a level spawns has it;
/// query it to find (or despawn) a level's entities.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component)]
pub struct FromLevel {
    /// The level's content id when it was spawned by id, else its asset path.
    pub level: String,
    /// Which spawn of the level: one file can be spawned many times (tiles).
    pub instance: u32,
    /// The node's path in the file: `Props/Crate.003`.
    pub node: String,
    /// The node's index in the file.
    pub index: u32,
}

/// One spawn of a level.
#[derive(Clone, Debug)]
pub struct LevelInstance {
    pub id: u32,
    /// The level's content id, or its asset path.
    pub label: String,
    /// The asset path of the file.
    pub path: String,
    /// Where it was placed.
    pub at: Transform,
    /// The visual scene's root entity (`render` feature).
    pub visual: Option<Entity>,
}

/// The levels spawned so far. Instance numbers count up from 1 and are never reused.
#[derive(Resource, Default, Debug)]
pub struct LevelInstances {
    last: u32,
    list: Vec<LevelInstance>,
    /// The replay check `level` is registered (by the first spawn).
    checked: bool,
}

impl LevelInstances {
    pub fn iter(&self) -> impl Iterator<Item = &LevelInstance> {
        self.list.iter()
    }

    pub fn get(&self, id: u32) -> Option<&LevelInstance> {
        self.list.iter().find(|i| i.id == id)
    }

    /// The number the next instance gets.
    pub fn peek_next(&self) -> u32 {
        self.last + 1
    }

    /// Makes `next` the number the next instance gets: a module that places files again as
    /// they were placed before (a region a snapshot puts back) gives them their numbers.
    pub fn restart_at(&mut self, next: u32) {
        self.last = next.saturating_sub(1);
    }

    fn next_id(&mut self) -> u32 {
        self.last += 1;
        self.last
    }
}

/// What [`spawn_level_as`] made.
#[derive(Clone, Debug)]
pub struct SpawnedLevel {
    pub instance: u32,
    /// The gameplay entities, in file node order.
    pub entities: Vec<Entity>,
    /// The visual scene's root, when the `render` feature loads one.
    pub visual: Option<Entity>,
}

/// Spawns `level` placed by `at`. Returns its gameplay entities in file node order; the
/// error lists every problem found in the file and spawns nothing.
pub fn spawn_level(world: &mut World, level: &LevelAsset, at: Transform) -> Result<Vec<Entity>, String> {
    let label = level.path.clone();
    spawn_level_as(world, level, at, &label).map(|spawned| spawned.entities)
}

/// [`spawn_level`] under `label` (the level's content id; `FromLevel::level`), returning
/// the instance number and the visual root too.
pub fn spawn_level_as(world: &mut World, level: &LevelAsset, at: Transform, label: &str) -> Result<SpawnedLevel, String> {
    let plans = prepare(world, level)?;
    let instance = world.get_resource_or_init::<LevelInstances>().next_id();
    place(world, level, &plans, at, label, instance)
}

/// Despawns every entity of `instance` (and its visual root). Returns how many gameplay
/// entities went.
pub fn despawn_level(world: &mut World, instance: u32) -> usize {
    let mut found = world.query::<(Entity, &FromLevel)>();
    let doomed: Vec<Entity> = found.iter(world).filter(|(_, from)| from.instance == instance).map(|(e, _)| e).collect();
    for entity in &doomed {
        world.despawn(*entity);
    }
    if let Some(mut instances) = world.get_resource_mut::<LevelInstances>()
        && let Some(position) = instances.list.iter().position(|i| i.id == instance)
    {
        let gone = instances.list.remove(position);
        if let Some(visual) = gone.visual {
            world.despawn(visual);
        }
    }
    doomed.len()
}

/// Spawns `instance` again from the file as it is now, where it was: the entities are
/// replaced (gameplay changes to them are lost), the instance keeps its number. A file
/// with problems leaves the instance as it was. Returns the number of entities.
pub fn reload_level(world: &mut World, instance: u32) -> Result<usize, String> {
    let Some(old) = world.resource::<LevelInstances>().get(instance).cloned() else {
        return Err(format!("no level instance {instance}"));
    };
    let level = level_asset(world, &old.path)?.clone();
    let plans = prepare(world, &level)?;
    despawn_level(world, instance);
    place(world, &level, &plans, old.at, &old.label, instance).map(|spawned| spawned.entities.len())
}

/// `level `arena.gltf` has 2 problems:` and a line each.
pub(crate) fn describe_problems(level: &LevelAsset, problems: &[LevelProblem]) -> String {
    let what = if level.path.is_empty() { "the level".to_owned() } else { format!("level `{}`", level.path) };
    let count = if problems.len() == 1 { "1 problem".to_owned() } else { format!("{} problems", problems.len()) };
    let lines: Vec<String> = problems.iter().map(|p| format!("  {p}")).collect();
    format!("{what} has {count}:\n{}", lines.join("\n"))
}

/// Reads the whole file into plans, or says what is wrong with it.
fn prepare(world: &World, level: &LevelAsset) -> Result<Vec<NodePlan>, String> {
    let app_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = app_registry.read();
    let (plans, problems) = analyze(level, &registry, world.get_resource::<ContentDb>());
    if problems.is_empty() { Ok(plans) } else { Err(describe_problems(level, &problems)) }
}

fn place(world: &mut World, level: &LevelAsset, plans: &[NodePlan], at: Transform, label: &str, instance: u32) -> Result<SpawnedLevel, String> {
    let at_matrix = at.to_matrix();
    let worlds: Vec<Mat4> = level.nodes.iter().map(|node| at_matrix * node.world).collect();
    let app_registry = world.resource::<AppTypeRegistry>().clone();
    let mut entities = Vec::new();
    for plan in plans.iter().filter(|p| p.spawns()) {
        match spawn_node(world, &app_registry, level, &worlds, plan, label, instance) {
            Ok(entity) => entities.push(entity),
            Err(why) => {
                for entity in entities {
                    world.despawn(entity);
                }
                return Err(why);
            }
        }
    }
    let visual = crate::render::spawn_visual(world, &level.path, at, instance);
    let mut instances = world.get_resource_or_init::<LevelInstances>();
    instances.list.push(LevelInstance { id: instance, label: label.to_owned(), path: level.path.clone(), at, visual });
    // Every replay check adds its name to the state hash, so a check registered at start would
    // change the hash of games that never spawn a level (and invalidate their recordings): it
    // joins with the first level instead, at the same tick in a recording and its playback.
    if !std::mem::replace(&mut instances.checked, true) {
        world.get_resource_or_init::<ReplayHashing>().add_check("level", level_hash);
    }
    Ok(SpawnedLevel { instance, entities, visual })
}

fn spawn_node(
    world: &mut World,
    app_registry: &AppTypeRegistry,
    level: &LevelAsset,
    worlds: &[Mat4],
    plan: &NodePlan,
    label: &str,
    instance: u32,
) -> Result<Entity, String> {
    let node = &level.nodes[plan.node];
    let (scale, rotation, translation) = worlds[plan.node].to_scale_rotation_translation();
    if !(translation.is_finite() && rotation.is_finite() && scale.is_finite()) {
        return Err(format!("node `{}`: its transform is not a valid pose", node.path));
    }
    // Scale is in the collider's size; the entity itself is unscaled.
    let transform = Transform { translation, rotation, scale: Vec3::ONE };

    let entity = match &plan.prefab {
        Some(prefab) => spawn_prefab(world, &prefab.id, transform, Some(&Value::Object(prefab.overrides.clone())))
            .map_err(|why| format!("node `{}`: {why}", node.path))?,
        None => world.spawn_empty().id(),
    };
    let link = plan.marker.as_ref().and_then(|m| m.link.as_ref()).and_then(|target| {
        let found = level.nodes.iter().position(|n| n.name == *target)?;
        Some(worlds[found].w_axis.truncate())
    });
    let mut e = world.entity_mut(entity);
    let display = plan.marker.as_ref().map_or_else(|| node.name.clone(), |m| m.name.clone());
    if !e.contains::<Name>() {
        e.insert(Name::new(display));
    }
    e.insert(FromLevel { level: label.to_owned(), instance, node: node.path.clone(), index: node.index as u32 });
    if let Some(marker) = &plan.marker {
        insert_marker(&mut e, marker, link);
    }
    if let Some(spec) = &plan.collider {
        e.insert(collider_from(spec, node.bounds, scale));
    }
    if let Some(kind) = &plan.volume {
        e.insert(Volume { kind: kind.clone() });
    }
    // The file's own components come last: what an artist wrote beats what the engine
    // derived from the same node.
    {
        let registry = app_registry.read();
        for (type_id, value) in &plan.components {
            if let Some(component) = registry.get(*type_id).and_then(|reg| reg.data::<ReflectComponent>()) {
                component.insert(&mut e, value.as_partial_reflect(), &registry);
            }
        }
    }
    e.insert(transform);
    Ok(entity)
}

/// The marker, and how the AI sees it: landmarks as the compact LANDMARKS line, spawn
/// points, camps, interactables and triggers as entities of their kind; sockets and
/// navlinks are plumbing and stay out of `observe`.
fn insert_marker(e: &mut EntityWorldMut, spec: &MarkerSpec, to: Option<Vec3>) {
    e.insert(LevelMarker { kind: spec.kind.clone(), name: spec.name.clone(), to });
    match &spec.kind {
        MarkerKind::Landmark => {
            if !e.contains::<AiKind>() {
                e.insert(AiKind::new(spec.name.clone()));
            }
            e.insert(AiLandmark);
        }
        MarkerKind::SpawnPoint | MarkerKind::Camp | MarkerKind::Interactable | MarkerKind::Trigger => {
            if !e.contains::<AiKind>() {
                e.insert(AiKind::new(spec.kind.label()));
            }
        }
        MarkerKind::Socket | MarkerKind::NavLink | MarkerKind::Custom(_) => {}
    }
}

/// The collider for a node of world scale `scale`: dimensions the file gave or the mesh
/// bounds, times the scale; centered on `offset` or the middle of the bounds.
pub(crate) fn collider_from(spec: &ColliderSpec, bounds: Option<Bounds>, scale: Vec3) -> Collider {
    let size = scale.abs();
    // Radii follow the ground plane, heights the up axis, a sphere the biggest of them.
    let around = size.x.max(size.z);
    let half = bounds.map(|b| b.half());
    let radius_of = |radius: Option<f32>| radius.or(half.map(|h| h.x.max(h.z))).unwrap_or(0.5) * around;
    let height_of = |height: Option<f32>| height.or(half.map(|h| h.y * 2.0)).unwrap_or(1.0) * size.y;
    let shape = match spec.shape {
        ShapeSpec::Box { size: extents } => Shape::Cuboid { half: extents.map(|s| s * 0.5).or(half).unwrap_or(Vec3::splat(0.5)) * size },
        ShapeSpec::Sphere { radius } => Shape::Sphere { radius: radius.or(half.map(Vec3::max_element)).unwrap_or(0.5) * size.max_element() },
        ShapeSpec::Capsule { radius, height } => Shape::capsule(radius_of(radius), height_of(height)),
        ShapeSpec::Cylinder { radius, height } => Shape::Cylinder { half_height: height_of(height) * 0.5, radius: radius_of(radius) },
        ShapeSpec::Cone { radius, height } => Shape::Cone { half_height: height_of(height) * 0.5, radius: radius_of(radius) },
    };
    let center = spec.offset.or(bounds.map(|b| b.center())).unwrap_or(Vec3::ZERO);
    Collider { shape, offset: center * scale, layers: spec.layers }
}

/// Replay check: which levels are placed where, and which version of each file (so a
/// recording made against another version of a file diverges at its first check).
pub(crate) fn level_hash(world: &World) -> u64 {
    let mut h = Fnv::new();
    let Some(instances) = world.get_resource::<LevelInstances>() else { return h.0 };
    let mut list: Vec<&LevelInstance> = instances.iter().collect();
    list.sort_by_key(|i| i.id);
    for instance in list {
        h.u64(instance.id as u64);
        h.bytes(instance.label.as_bytes());
        h.bytes(instance.path.as_bytes());
        for v in instance.at.translation.to_array().into_iter().chain(instance.at.rotation.to_array()) {
            h.u64(v.to_bits() as u64);
        }
        if let Ok(asset) = level_asset(world, &instance.path) {
            h.u64(asset.digest);
        }
    }
    h.0
}
