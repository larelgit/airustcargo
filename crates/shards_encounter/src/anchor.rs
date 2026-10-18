//! Anchors: where an encounter stands, and what a prefab costs it.
//!
//! An encounter stands wherever an entity with [`EncounterAnchor`] stands: a point of interest's
//! prefab, a prefab node of a level file, a tile's prefab piece, or a game's own
//! [`place_encounter`](crate::place_encounter) call. The run of the encounter is keyed by the
//! region and the anchor's key, so a won `once` encounter is remembered across region changes.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::{ContentDb, PrefabDef, Refs};
use shards_npc::BehaviorDef;
use shards_world::ActiveRegion;

use crate::def::{did_you_mean, EncounterDef};

/// Where an encounter stands: the run of encounter `encounter` is keyed, placed and bound
/// (volumes, markers, points of interest within its radius) at this entity's transform.
#[derive(Component, Reflect, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct EncounterAnchor {
    /// `encounter` content id.
    pub encounter: String,
    /// The run's key within its region; none: `<encounter>@<x>,<z>` in decimeters. Required for
    /// a `once` encounter: a won encounter is remembered by it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
}

/// What a prefab costs an encounter's budget and which role tags it has. Read by content
/// validation; at runtime only `inspect` shows it.
#[derive(Component, Reflect, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct EncounterRole {
    pub cost: u32,
    #[serde(default)]
    pub tags: Vec<String>,
}

/// A prefab's [`EncounterRole`], read from its text.
pub(crate) type PrefabRole = EncounterRole;

/// The run key of an anchor: `<region>/<anchor key>`. `<region>` is the active region's id, or
/// `world` outside any region; the anchor key is [`EncounterAnchor::key`], or
/// `<encounter>@<x>,<z>` with the position in whole decimeters (stable for one manifest, because
/// generation is deterministic).
pub fn run_key(region: Option<&str>, anchor: &EncounterAnchor, at: Vec3) -> String {
    let region = region.unwrap_or("world");
    match &anchor.key {
        Some(key) => format!("{region}/{key}"),
        None => format!("{region}/{}@{},{}", anchor.encounter, (at.x * 10.0).round() as i32, (at.z * 10.0).round() as i32),
    }
}

/// The id of the region in the world, if any.
pub(crate) fn region_id(world: &World) -> Option<&str> {
    world.get_resource::<ActiveRegion>().map(|r| r.manifest.region.as_str())
}

/// The RON text of component `short` (`Npc`, or a full path ending in `::Npc`) of prefab
/// `prefab`, walking `extends`: the nearest definition wins, as the prefab itself spawns.
pub(crate) fn prefab_component(db: &ContentDb, prefab: &str, short: &str) -> Option<String> {
    let mut id = prefab.to_owned();
    // A circular `extends` is reported by the prefab's own validation; stop after a few steps.
    for _ in 0..32 {
        let def = db.peek::<PrefabDef>(&id)?;
        if let Some((_, value)) = def.components.iter().find(|(name, _)| name.as_str() == short || name.ends_with(&format!("::{short}"))) {
            return Some(value.get_ron().to_owned());
        }
        id = def.extends.clone()?;
    }
    None
}

/// The role a prefab has, from its `EncounterRole` component.
pub(crate) fn prefab_role(db: &ContentDb, prefab: &str) -> Option<EncounterRole> {
    ron::from_str(&prefab_component(db, prefab, "EncounterRole")?).ok()
}

/// `Some(hint)` when behaviour `behavior` exists and lacks `state`; the hint is a "did you mean"
/// and the list of its states.
pub(crate) fn behavior_lacks_state(db: &ContentDb, behavior: &str, state: &str) -> Option<String> {
    let def = db.peek::<BehaviorDef>(behavior)?;
    if def.states.contains_key(state) {
        return None;
    }
    let names: Vec<&str> = def.states.keys().map(String::as_str).collect();
    Some(format!("{}; states: {}", did_you_mean(state, names.iter().copied()), names.join(", ")))
}

/// The `PrefabDef` check: an `EncounterAnchor` names an existing encounter, and a `once`
/// encounter's anchor has a key.
pub(crate) fn check_anchor(def: &PrefabDef, _id: &str, refs: &mut Refs) {
    let Some((_, text)) = def.components.iter().find(|(name, _)| name.as_str() == "EncounterAnchor" || name.ends_with("::EncounterAnchor"))
    else {
        return;
    };
    let Ok(anchor) = ron::from_str::<EncounterAnchor>(text.get_ron()) else { return };
    if !refs.require::<EncounterDef>(&anchor.encounter, "EncounterAnchor.encounter") {
        return;
    }
    if refs.get::<EncounterDef>(&anchor.encounter).is_some_and(|d| d.once) && anchor.key.is_none() {
        refs.error(format!(
            "EncounterAnchor: the encounter `{}` is `once`, so its anchor needs a key (a won encounter is remembered by it)",
            anchor.encounter
        ));
    }
}
