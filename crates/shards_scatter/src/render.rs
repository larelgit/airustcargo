//! Strewing the scatters (feature `render`): each tile a region builds gets, under its root, one
//! entity per (scatter, look) holding the `Look`, and under that one mesh entity per instance and
//! level of detail: the shared mesh of the level, the shared white material (the look derives its
//! own, one per look), the level's `VisibilityRange`. Nothing here touches the simulation.

use std::collections::{BTreeMap, HashMap};

use bevy::camera::visibility::VisibilityRange;
use bevy::light::NotShadowCaster;
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_look::Look;
use shards_world::{ActiveRegion, RegionTile, TileDef, TileId, RoomDef};

use crate::def::{ScatterDef, ScatterMesh};
use crate::place::{self, Placed, Skipped};
use crate::{ScatterInstance, ScatterRoot, ScatterState, ScatterSystems, ScatterTile};

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(Update, place.run_if(ai_core::display::drawn).run_if(needs_placing).after(crate::watch).in_set(ScatterSystems::Place));
}

/// The meshes and materials of the scatters, made once.
#[derive(Resource)]
struct ScatterAssets {
    /// White, back faces culled.
    solid: Handle<StandardMaterial>,
    /// White, both faces drawn (blades).
    two_sided: Handle<StandardMaterial>,
    /// Shape (as it describes itself) → its mesh.
    meshes: HashMap<String, Handle<Mesh>>,
}

impl FromWorld for ScatterAssets {
    fn from_world(world: &mut World) -> Self {
        let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
        let white = StandardMaterial { base_color: Color::WHITE, perceptual_roughness: 1.0, reflectance: 0.0, ..default() };
        // Blades lit from above whichever side is seen: no culling, normals not flipped.
        let two_sided = StandardMaterial { cull_mode: None, double_sided: false, ..white.clone() };
        Self { solid: materials.add(white), two_sided: materials.add(two_sided), meshes: HashMap::new() }
    }
}

/// Tiles wait, or everything is to be strewn again.
fn needs_placing(state: Res<ScatterState>, waiting: Query<(), (With<RegionTile>, Without<ScatterTile>)>) -> bool {
    state.redo || !waiting.is_empty()
}

/// Strews the tiles that have nothing yet (all of them after a redo), in cell order.
pub(crate) fn place(world: &mut World) {
    if !world.contains_resource::<Assets<StandardMaterial>>() || !world.contains_resource::<Assets<Mesh>>() {
        return;
    }
    if std::mem::take(&mut world.resource_mut::<ScatterState>().redo) {
        clear(world);
    }
    let mut tiles: Vec<(Entity, TileId, RegionTile, Transform)> = world
        .query_filtered::<(Entity, &TileId, &RegionTile, &Transform), Without<ScatterTile>>()
        .iter(world)
        .map(|(e, id, tile, tf)| (e, *id, tile.clone(), *tf))
        .collect();
    if tiles.is_empty() {
        return;
    }
    // Tiles without their region (being built or taken away): later.
    let Some(region) = world.get_resource::<ActiveRegion>() else { return };
    let (seed, region_id, tile_size) = (region.manifest.seed, region.manifest.region.clone(), region.manifest.tile_size);
    tiles.sort_unstable_by_key(|(_, id, ..)| (id.y, id.x));
    world.init_resource::<ScatterAssets>();
    let state = world.resource::<ScatterState>();
    let (on, foliage, defs) = (state.on, state.foliage, state.defs.clone());
    // Colliders the region just spawned are in the physics world after its next sync: the same
    // pure function of the ECS the ticks run, so it changes no tick.
    let _ = world.run_system_cached(shards_motion::physics::sync_physics);

    let mut plans = Vec::new();
    let mut skipped_by: BTreeMap<String, Skipped> = BTreeMap::new();
    if on {
        let db = world.resource::<ContentDb>();
        let physics = world.resource::<shards_motion::PhysicsWorld>();
        for (entity, id, tile, tf) in &tiles {
            let tags: &[String] = db
                .get::<TileDef>(&tile.tile)
                .map(|t| t.tags.as_slice())
                .or_else(|| db.get::<RoomDef>(&tile.tile).map(|r| r.tags.as_slice()))
                .unwrap_or(&[]);
            let extent = Vec2::new(tile.size.x as f32, tile.size.y as f32) * tile_size;
            // A room's north-west cell: where it is in the region.
            let cell = (id.x, id.y);
            for (scatter, def) in &defs {
                if !def.in_region(&region_id) || !def.on_tile(&tile.tile, tags) || def.lods.is_empty() {
                    continue;
                }
                let thrown = place::candidates(def, &mut place::rng_for(seed, &region_id, cell, scatter), extent);
                let kept = place::thinned(thrown.len(), foliage);
                let (placed, skipped) = place::settle(def, tf.translation, &thrown[..kept], physics);
                skipped_by.entry(scatter.clone()).or_default().add(skipped);
                if !placed.is_empty() {
                    plans.push((*entity, *id, *tf, scatter.clone(), def.clone(), placed));
                }
            }
        }
    }
    let mut state = world.resource_mut::<ScatterState>();
    for (scatter, skipped) in skipped_by {
        state.skipped.entry(scatter).or_default().add(skipped);
    }
    for (entity, ..) in &tiles {
        world.entity_mut(*entity).insert(ScatterTile);
    }
    for (tile, id, tf, scatter, def, placed) in plans {
        spawn(world, tile, id, tf, &scatter, &def, &placed);
    }
}

/// Takes every instance away and marks every tile to be strewn again.
fn clear(world: &mut World) {
    let roots: Vec<Entity> = world.query_filtered::<Entity, With<ScatterRoot>>().iter(world).collect();
    for root in roots {
        if let Ok(root) = world.get_entity_mut(root) {
            root.despawn();
        }
    }
    let tiles: Vec<Entity> = world.query_filtered::<Entity, With<ScatterTile>>().iter(world).collect();
    for tile in tiles {
        world.entity_mut(tile).remove::<ScatterTile>();
    }
    world.resource_mut::<ScatterState>().skipped.clear();
}

/// The mesh of `shape`, made or loaded once.
fn mesh_of(world: &mut World, shape: &ScatterMesh) -> Handle<Mesh> {
    let key = shape.describe();
    if let Some(mesh) = world.resource::<ScatterAssets>().meshes.get(&key) {
        return mesh.clone();
    }
    let handle = match (crate::mesh::build(shape), shape) {
        (Some(mesh), _) => world.resource_mut::<Assets<Mesh>>().add(mesh),
        (None, ScatterMesh::Gltf(path)) => match world.get_resource::<AssetServer>() {
            Some(server) => server.load(path.clone()),
            None => Handle::default(),
        },
        (None, _) => Handle::default(),
    };
    world.resource_mut::<ScatterAssets>().meshes.insert(key, handle.clone());
    handle
}

/// Level `lod`'s distances: it comes in where the one before goes, over `fade` meters, and goes
/// at its `until`, over `fade` meters before it.
fn range_of(def: &ScatterDef, lod: usize) -> VisibilityRange {
    let (start, end) = def.lod_span(lod);
    let fade = def.fade.max(0.0);
    let start_margin = if lod == 0 { 0.0..0.0 } else { (start - fade)..start };
    VisibilityRange { start_margin, end_margin: (end - fade)..end, use_aabb: false }
}

/// One scatter's `placed` instances on tile `tile` (its root at `tf`): a root per look under the
/// tile, under it a mesh entity per instance and level of detail.
fn spawn(world: &mut World, tile: Entity, id: TileId, tf: Transform, scatter: &str, def: &ScatterDef, placed: &[Placed]) {
    let meshes: Vec<Handle<Mesh>> = def.lods.iter().map(|lod| mesh_of(world, &lod.mesh)).collect();
    let assets = world.resource::<ScatterAssets>();
    let materials: Vec<Handle<StandardMaterial>> =
        def.lods.iter().map(|lod| if lod.mesh.two_sided() { assets.two_sided.clone() } else { assets.solid.clone() }).collect();
    let ranges: Vec<VisibilityRange> = (0..def.lods.len()).map(|lod| range_of(def, lod)).collect();
    // The tile's root is not drawn itself; what hangs under it inherits its visibility.
    if !world.entity(tile).contains::<Visibility>() {
        world.entity_mut(tile).insert(Visibility::default());
    }
    let into_tile = tf.rotation.inverse();
    let mut by_look: BTreeMap<usize, Vec<&Placed>> = BTreeMap::new();
    for p in placed {
        by_look.entry(p.look).or_default().push(p);
    }
    for (look, instances) in by_look {
        let Some(look) = def.looks.get(look) else { continue };
        let root = ScatterRoot { scatter: scatter.to_owned(), look: look.clone(), tile: id, instances: instances.len(), lods: def.lods.len() };
        let name = Name::new(format!("scatter {scatter} {look}"));
        let root = world.spawn((root, Look::new(look.clone()), name, Transform::IDENTITY, Visibility::default(), ChildOf(tile))).id();
        let mut batch = Vec::with_capacity(instances.len() * meshes.len());
        for p in instances {
            let local = Transform {
                translation: into_tile * (p.at - tf.translation),
                rotation: into_tile * p.rotation,
                scale: Vec3::splat(p.scale),
            };
            for (lod, mesh) in meshes.iter().enumerate() {
                let instance = ScatterInstance { lod: lod as u8 };
                batch.push((Mesh3d(mesh.clone()), MeshMaterial3d(materials[lod].clone()), local, ranges[lod].clone(), instance, ChildOf(root)));
            }
        }
        if def.shadows {
            world.spawn_batch(batch);
        } else {
            world.spawn_batch(batch.into_iter().map(|bundle| (bundle, NotShadowCaster)));
        }
    }
}
