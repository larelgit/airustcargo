//! Tile visuals: what the picture (a window, or `--offscreen`) shows for each tile, loaded and
//! unloaded by distance.
//!
//! Gameplay geometry of the whole region is spawned when it loads. What only the picture
//! needs — meshes, scenes, their level of detail — comes and goes with the players: every
//! frame (`Update`, not the fixed tick) each tile picks the first LOD whose distance
//! ([`WorldSettings::visual_lods`]) covers the nearest player, or none beyond the last.
//! When that changes, the old visual is despawned and the game's spawner
//! ([`WorldAppExt::set_tile_visual_spawner`](crate::WorldAppExt::set_tile_visual_spawner))
//! makes the new one, in the tile's frame; the engine parents it to the tile, which is
//! placed and turned. Visual entities are never gameplay: no colliders, no AI ids, so
//! streaming them changes no tick. Headless (nothing drawn), nothing streams.

use ai_core::AiPlayer;
use bevy::prelude::*;
use shards_core::{EngineConfig, PlayerId};

use crate::region::{RegionTile, TileId};
use crate::WorldSettings;

/// What the tile visual spawner gets.
pub struct TileVisualSpawn<'a> {
    pub tile: TileId,
    /// `tile` or `room` id.
    pub tile_id: &'a str,
    /// The tile's `visual`.
    pub visual: Option<&'a str>,
    /// 0 for the nearest band of [`WorldSettings::visual_lods`].
    pub lod: usize,
    /// Cells it covers, and the edge of a cell (m).
    pub size: UVec2,
    pub tile_size: f32,
}

/// Makes the visual of a tile in its own frame (center at the origin, north -Z, unturned);
/// `None` for nothing at this LOD.
pub type TileVisualFn = fn(&mut World, &TileVisualSpawn) -> Option<Entity>;

/// The game's tile visual spawner.
#[derive(Resource, Clone, Copy)]
pub struct TileVisualSpawner(pub TileVisualFn);

/// A tile's visual as streaming left it (on the tile's root entity).
#[derive(Component, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TileVisual {
    /// The LOD shown; `None`: unloaded.
    pub lod: Option<usize>,
    pub entity: Option<Entity>,
}

/// Whether visuals stream: a spawner exists and something is drawn, a window or offscreen (or the
/// settings say so).
pub fn streaming(settings: Res<WorldSettings>, config: Option<Res<EngineConfig>>, spawner: Option<Res<TileVisualSpawner>>) -> bool {
    let drawn = config.is_none_or(|c| c.draws());
    spawner.is_some() && settings.stream_visuals.unwrap_or(drawn)
}

/// `Update`: every tile's LOD from the nearest player; changed ones respawn their visual.
pub fn stream_tile_visuals(world: &mut World) {
    // Only a picture: whatever it spawns, what it reads is not the simulation's.
    let _quiet = ai_core::scope::suspend(world);
    let Some(TileVisualSpawner(spawner)) = world.get_resource::<TileVisualSpawner>().copied() else {
        return;
    };
    let lods = world.resource::<WorldSettings>().visual_lods.clone();
    let tile_size = world.get_resource::<crate::region::ActiveRegion>().map_or(1.0, |r| r.manifest.tile_size);
    let players: Vec<Vec3> = world
        .query_filtered::<&Transform, Or<(With<PlayerId>, With<AiPlayer>)>>()
        .iter(world)
        .map(|tf| tf.translation)
        .collect();
    let mut tiles: Vec<(TileId, Entity, Vec3, RegionTile, TileVisual)> = world
        .query::<(Entity, &TileId, &Transform, &RegionTile, Option<&TileVisual>)>()
        .iter(world)
        .map(|(e, id, tf, tile, visual)| (*id, e, tf.translation, tile.clone(), visual.copied().unwrap_or_default()))
        .collect();
    tiles.sort_unstable_by_key(|(id, ..)| (id.y, id.x));
    for (id, root, center, tile, visual) in tiles {
        let distance = players.iter().map(|p| Vec2::new(p.x - center.x, p.z - center.z).length()).fold(f32::INFINITY, f32::min);
        let lod = lods.iter().position(|d| distance <= *d);
        if lod == visual.lod {
            continue;
        }
        if let Some(old) = visual.entity
            && let Ok(old) = world.get_entity_mut(old)
        {
            old.despawn();
        }
        let entity = lod.and_then(|lod| {
            let request = TileVisualSpawn { tile: id, tile_id: &tile.tile, visual: tile.visual.as_deref(), lod, size: tile.size, tile_size };
            spawner(world, &request)
        });
        if let Some(entity) = entity {
            world.entity_mut(entity).insert(ChildOf(root));
        }
        world.entity_mut(root).insert(TileVisual { lod, entity });
    }
}
