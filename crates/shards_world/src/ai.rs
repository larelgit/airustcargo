//! What the AI sees of the world module: where the subject is (`TILE`), the nearest points of
//! interest with direction and distance (`POI`), dormant characters, `ai/status` → `world`,
//! and the stop condition `near_poi`.
//!
//! ```text
//! YOU  e9 Player  ground  at (40.0, 24.0) facing N
//! TILE (2, 1) road_bend rot 90 · meadow seed 42 v1
//! POI  e3 spawn.0 0.0m here · e5 camp.0 34.2m NE · e6 camp.1 52.0m S
//! ```

use std::collections::BTreeMap;

use ai_core::spatial::relative;
use ai_core::{AiAppExt, AiId, AiIndex};
use bevy::prelude::*;
use serde_json::{json, Value};

use crate::region::{ActiveRegion, Poi};
use crate::zones::Dormant;

/// How many points of interest the `POI` line lists.
const NEAREST: usize = 3;

/// Within this many meters of a point of interest, `near_poi` holds.
pub const NEAR_POI: f32 = 3.0;

pub(crate) fn plugin(app: &mut App) {
    app.register_ai_component::<Dormant>(|_| "dormant".into())
        .register_ai_observe_lines(observe_lines)
        .add_ai_status("world", status)
        .add_ai_condition("near_poi", near_poi);
}

/// The `AiId` of every point of interest entity, by POI id.
fn poi_entities(world: &World) -> BTreeMap<String, AiId> {
    let Some(index) = world.get_resource::<AiIndex>() else { return BTreeMap::new() };
    index
        .ids()
        .into_iter()
        .filter_map(|id| Some((world.get::<Poi>(index.get(id)?)?.id.clone(), id)))
        .collect()
}

/// `TILE` and `POI` lines for `subject`.
fn observe_lines(world: &World, subject: Entity) -> Vec<String> {
    let (Some(region), Some(pos)) = (world.get_resource::<ActiveRegion>(), world.get::<Transform>(subject).map(|t| t.translation)) else {
        return Vec::new();
    };
    let m = &region.manifest;
    let mut lines = vec![match region.tile_at(pos) {
        Some(([x, y], tile)) => format!("TILE ({x}, {y}) {} rot {} · {}", tile.tile, tile.rot, m.title()),
        None => format!("TILE outside {} ({}×{} tiles of {}m from (0, 0))", m.title(), m.size[0], m.size[1], m.tile_size),
    }];
    let mut pois: Vec<(f32, &str, Vec3)> = m
        .pois
        .iter()
        .map(|p| (Vec2::new(p.pos[0] - pos.x, p.pos[2] - pos.z).length(), p.id.as_str(), p.position()))
        .collect();
    pois.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(b.1)));
    if !pois.is_empty() {
        let ids = poi_entities(world);
        let items: Vec<String> = pois
            .iter()
            .take(NEAREST)
            .map(|(_, id, at)| match ids.get(*id) {
                Some(e) => format!("{e} {id} {}", relative(pos, *at)),
                None => format!("{id} {}", relative(pos, *at)),
            })
            .collect();
        lines.push(format!("POI  {}", items.join(" · ")));
    }
    lines
}

/// `ai/status` → `world`: the region in the world, or null.
fn status(world: &World) -> Value {
    let Some(region) = world.get_resource::<ActiveRegion>() else { return Value::Null };
    let m = &region.manifest;
    json!({
        "region": m.region,
        "seed": m.seed,
        "version": m.version,
        "attempt": m.attempt,
        "fallback": m.fallback,
        "size": m.size,
        "tiles": m.tiles.len(),
        "pois": m.pois.len(),
        "entities": region.entities,
        "manifest_hash": format!("{:016x}", region.hash),
        "content_hash": m.content_hash,
    })
}

/// The player stands within [`NEAR_POI`] meters of a point of interest.
fn near_poi(world: &World) -> bool {
    let (Some(region), Some(player)) = (world.get_resource::<ActiveRegion>(), ai_core::observe::player_entity(world)) else {
        return false;
    };
    let Some(at) = world.get::<Transform>(player).map(|t| t.translation) else { return false };
    region.manifest.pois.iter().any(|p| Vec2::new(p.pos[0] - at.x, p.pos[2] - at.z).length() <= NEAR_POI)
}
