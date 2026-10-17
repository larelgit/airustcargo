//! The levels placed by id ([`spawn_level_by_id`](crate::spawn_level_by_id), the `spawn_level`
//! tool), as snapshot state.
//!
//! A region's level tiles come back with the region a snapshot builds again; a level placed by
//! hand has no such owner. [`PlacedLevels`] names each one, where it stands and the numbers it
//! took, the snapshot saves it, and a load places the saved ones again, the same down to the
//! instance numbers and collider ids, so the `levels` and `static_colliders` checks compare
//! equal in a game that never placed them. Two prepares, around the region's: `levels` takes
//! away what this game placed before the region is built again (a tile of the region may take
//! the number a placed level had here), `placed levels` places the snapshot's after it (a
//! placed level may have the number a tile had here).

use std::collections::BTreeSet;

use ai_core::Snapshot;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_core::LoadStatus;
use shards_motion::{ColliderId, ColliderIds};

use crate::library::{request_level, LevelLibrary};
use crate::spawn::{despawn_level, LevelCounter};
use crate::tools::{def_of, spawn_level_by_id};

/// The levels placed by id and standing now, in the order they were placed. Present while
/// there is one: a snapshot of a game without any is what it was before they were saved.
/// Snapshot state, kept on each machine of a co-op session (like [`LevelCounter`]).
#[derive(Resource, Reflect, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[reflect(Resource, Default)]
pub struct PlacedLevels {
    pub levels: Vec<PlacedLevel>,
}

/// One level placed by id.
#[derive(Reflect, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct PlacedLevel {
    /// Its instance number.
    pub instance: u32,
    /// The level's content id.
    pub id: String,
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    /// [`ColliderIds`] when it was placed (or last reloaded): its colliders take the ids from
    /// there on.
    pub first_collider: u32,
}

impl PlacedLevel {
    fn at(&self) -> Transform {
        Transform::from_translation(Vec3::from_array(self.translation)).with_rotation(Quat::from_array(self.rotation))
    }
}

/// The id the next collider gets ([`ColliderIds`]).
pub(crate) fn next_collider(world: &World) -> u32 {
    world.get_resource::<ColliderIds>().map_or(1, |ids| ids.peek().0)
}

/// Records a level placed by id.
pub(crate) fn record(world: &mut World, instance: u32, id: &str, at: Transform, first_collider: u32) {
    let placed = PlacedLevel {
        instance,
        id: id.to_owned(),
        translation: at.translation.to_array(),
        rotation: at.rotation.to_array(),
        first_collider,
    };
    world.get_resource_or_init::<PlacedLevels>().levels.push(placed);
}

/// A level that went: the resource goes with the last one.
pub(crate) fn forget(world: &mut World, instance: u32) {
    let Some(mut placed) = world.get_resource_mut::<PlacedLevels>() else { return };
    placed.levels.retain(|p| p.instance != instance);
    if placed.levels.is_empty() {
        world.remove_resource::<PlacedLevels>();
    }
}

/// A level placed again from its file (a reload): its colliders took ids from `first_collider`.
pub(crate) fn reloaded(world: &mut World, instance: u32, first_collider: u32) {
    let Some(mut placed) = world.get_resource_mut::<PlacedLevels>() else { return };
    if let Some(level) = placed.levels.iter_mut().find(|p| p.instance == instance) {
        level.first_collider = first_collider;
    }
}

fn saved(snapshot: &Snapshot) -> Result<PlacedLevels, String> {
    match snapshot.resources.get(<PlacedLevels as TypePath>::type_path()) {
        Some(value) => serde_json::from_value(value.clone()).map_err(|e| format!("its placed levels do not read: {e}")),
        None => Ok(PlacedLevels::default()),
    }
}

fn current(world: &World) -> PlacedLevels {
    world.get_resource::<PlacedLevels>().cloned().unwrap_or_default()
}

/// Despawns every level placed by id; returns how many.
fn take_away_all(world: &mut World) -> usize {
    let placed = current(world).levels;
    for level in &placed {
        despawn_level(world, level.instance);
    }
    world.remove_resource::<PlacedLevels>();
    placed.len()
}

/// Snapshot prepare `levels`, before the region's: the levels this game placed by id go when
/// the snapshot's differ, so the region built again does not meet them under its numbers.
pub(crate) fn take_away(world: &mut World, snapshot: &Snapshot) -> Result<Option<String>, String> {
    if saved(snapshot)? == current(world) {
        return Ok(None);
    }
    Ok(match take_away_all(world) {
        0 => None,
        1 => Some("1 level placed by id taken away".to_owned()),
        n => Some(format!("{n} levels placed by id taken away")),
    })
}

/// Snapshot prepare `placed levels`, after the region's: the snapshot's levels placed by id stand
/// again where they stood, with the instance numbers and collider ids they had. A file this game
/// has not loaded is asked for, and the load refused until it is in.
pub(crate) fn put_in(world: &mut World, snapshot: &Snapshot) -> Result<Option<String>, String> {
    let saved = saved(snapshot)?;
    if saved == current(world) {
        return Ok(None);
    }
    let mut loading = BTreeSet::new();
    for level in &saved.levels {
        let def = def_of(world, &level.id).map_err(|e| format!("level {}: {e}", level.instance))?;
        request_level(world, &def.file);
        if let LoadStatus::Pending(_) = world.resource::<LevelLibrary>().status(world, &def.file) {
            loading.insert(format!("`{}`", def.file));
        }
    }
    if !loading.is_empty() {
        let s = if loading.len() == 1 { "" } else { "s" };
        let files: Vec<String> = loading.into_iter().collect();
        return Err(format!(
            "it placed levels from the file{s} {}, which this game is loading only now: load the snapshot again in a moment",
            files.join(", ")
        ));
    }
    take_away_all(world);
    for level in &saved.levels {
        world.get_resource_or_init::<LevelCounter>().restart_at(level.instance);
        if let Some(mut ids) = world.get_resource_mut::<ColliderIds>() {
            ids.restart_at(ColliderId(level.first_collider));
        }
        spawn_level_by_id(world, &level.id, Some(level.at())).map_err(|e| format!("level {} `{}`: {e}", level.instance, level.id))?;
    }
    Ok(match saved.levels.len() {
        0 => None,
        1 => Some("1 level placed by id stands again".to_owned()),
        n => Some(format!("{n} levels placed by id stand again")),
    })
}
