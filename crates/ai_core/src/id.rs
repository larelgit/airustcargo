//! Stable ids for the AI layer.
//!
//! `Entity` indices are recycled: after a skeleton dies, a freshly spawned arrow can
//! get the same index, and an agent that says "attack e42" hits the wrong thing.
//! [`AiId`] only ever grows and is what agents, journals, replays and the CLI use.

use std::collections::VecDeque;
use std::fmt;

use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::sim::SimTick;

/// Monotonic, never reused entity id. Printed as `e42`.
///
/// `AiId(0)` means "not assigned yet": the add-hook replaces it with the next free id.
/// A non-zero id (e.g. restored from a snapshot) is kept as is.
#[derive(
    Component, Reflect, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize,
)]
#[reflect(Component, Default)]
#[component(on_add = on_add_ai_id, on_remove = on_remove_ai_id)]
pub struct AiId(pub u32);

impl fmt::Display for AiId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "e{}", self.0)
    }
}

impl AiId {
    /// Parses `42`, `e42` or `E42`.
    pub fn parse(s: &str) -> Option<AiId> {
        let s = s.trim();
        let digits = s.strip_prefix(['e', 'E']).unwrap_or(s);
        digits.parse().ok().filter(|&n| n > 0).map(AiId)
    }
}

/// Opt-in marker: this entity exists for the AI (observations, queries, diffs).
///
/// Thousands of render/UI/internal entities live in the world; the AI only sees
/// entities that carry this marker, described by the components registered with
/// [`AiAppExt::register_ai_component`](crate::AiAppExt::register_ai_component).
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
#[require(AiId)]
pub struct AiVisible;

/// Extra weight for sorting observations ("by threat"): bosses high, props low.
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct AiImportance(pub f32);

/// Static scenery worth knowing about (pillars, doors, shrines): listed compactly on
/// one LANDMARKS line instead of taking a full line each.
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
#[require(AiVisible)]
pub struct AiLandmark;

/// The entity the local player controls: default viewpoint and default "you"
/// for agent sessions that do not set their own subject.
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct AiPlayer;

/// What is known about a despawned id, for helpful errors ("e57 died at tick 1530").
#[derive(Clone, Debug)]
pub struct Tombstone {
    pub id: AiId,
    pub label: String,
    pub tick: u64,
}

/// `AiId` ⇄ `Entity` lookup, maintained by component hooks (never by scanning).
#[derive(Resource, Debug)]
pub struct AiIndex {
    next: u32,
    live: HashMap<AiId, Entity>,
    graveyard: VecDeque<Tombstone>,
}

const GRAVEYARD_LEN: usize = 256;

impl Default for AiIndex {
    fn default() -> Self {
        Self { next: 1, live: HashMap::default(), graveyard: VecDeque::new() }
    }
}

impl AiIndex {
    pub fn get(&self, id: AiId) -> Option<Entity> {
        self.live.get(&id).copied()
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    /// Live ids in ascending order.
    pub fn ids(&self) -> Vec<AiId> {
        let mut ids: Vec<AiId> = self.live.keys().copied().collect();
        ids.sort_unstable();
        ids
    }

    pub fn tombstone(&self, id: AiId) -> Option<&Tombstone> {
        self.graveyard.iter().rev().find(|t| t.id == id)
    }

    /// Next id that will be assigned (for snapshots).
    pub fn next_id(&self) -> u32 {
        self.next
    }

    /// Like [`get`](Self::get), but the error explains what happened and what exists,
    /// so an agent can correct itself instead of retrying blindly.
    pub fn resolve(&self, id: AiId) -> Result<Entity, String> {
        if let Some(e) = self.get(id) {
            return Ok(e);
        }
        let mut msg = match self.tombstone(id) {
            Some(t) => format!("{id} no longer exists ({} despawned at tick {})", t.label, t.tick),
            None if id.0 >= self.next => format!("{id} does not exist (ids so far go up to e{})", self.next - 1),
            None => format!("{id} does not exist"),
        };
        let ids = self.ids();
        let shown: Vec<String> = ids.iter().take(24).map(ToString::to_string).collect();
        msg.push_str(&format!("; live: {}", shown.join(", ")));
        if ids.len() > shown.len() {
            msg.push_str(&format!(" … (+{})", ids.len() - shown.len()));
        }
        Err(msg)
    }
}

fn on_add_ai_id(mut world: DeferredWorld, ctx: HookContext) {
    let current = world.get::<AiId>(ctx.entity).copied().unwrap_or_default();
    let id = {
        let Some(mut index) = world.get_resource_mut::<AiIndex>() else {
            return;
        };
        let id = if current.0 == 0 {
            let id = AiId(index.next);
            index.next += 1;
            id
        } else {
            index.next = index.next.max(current.0 + 1);
            current
        };
        if let Some(previous) = index.live.insert(id, ctx.entity)
            && previous != ctx.entity
        {
            warn!("{id} was already used by {previous}; now points to {}", ctx.entity);
        }
        id
    };
    if id != current
        && let Some(mut slot) = world.get_mut::<AiId>(ctx.entity)
    {
        *slot = id;
    }
}

fn on_remove_ai_id(mut world: DeferredWorld, ctx: HookContext) {
    let Some(id) = world.get::<AiId>(ctx.entity).copied() else {
        return;
    };
    let label = world
        .get::<Name>(ctx.entity)
        .map(|n| n.as_str().to_owned())
        .unwrap_or_else(|| "entity".to_owned());
    let tick = world.get_resource::<SimTick>().map_or(0, |t| t.0);
    let Some(mut index) = world.get_resource_mut::<AiIndex>() else {
        return;
    };
    if index.live.get(&id) == Some(&ctx.entity) {
        index.live.remove(&id);
    }
    if index.graveyard.len() == GRAVEYARD_LEN {
        index.graveyard.pop_front();
    }
    index.graveyard.push_back(Tombstone { id, label, tick });
}

/// `e42` for tracked entities, the raw entity otherwise. For journal formatters.
pub fn label(world: &World, entity: Entity) -> String {
    match world.get::<AiId>(entity) {
        Some(id) => id.to_string(),
        None => format!("{entity}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn world() -> World {
        let mut w = World::new();
        w.init_resource::<AiIndex>();
        w
    }

    #[test]
    fn ids_grow_and_are_never_reused() {
        let mut w = world();
        let a = w.spawn(AiVisible).id();
        let b = w.spawn(AiVisible).id();
        assert_eq!(w.get::<AiId>(a), Some(&AiId(1)));
        assert_eq!(w.get::<AiId>(b), Some(&AiId(2)));
        w.despawn(a);
        let c = w.spawn(AiVisible).id();
        assert_eq!(w.get::<AiId>(c), Some(&AiId(3)), "id 1 must not be reused");
        let index = w.resource::<AiIndex>();
        assert_eq!(index.get(AiId(1)), None);
        assert_eq!(index.get(AiId(3)), Some(c));
        assert!(index.resolve(AiId(1)).unwrap_err().contains("no longer exists"));
    }

    #[test]
    fn explicit_ids_are_kept_and_bump_the_counter() {
        let mut w = world();
        let restored = w.spawn((AiVisible, AiId(40))).id();
        let fresh = w.spawn(AiVisible).id();
        assert_eq!(w.get::<AiId>(restored), Some(&AiId(40)));
        assert_eq!(w.get::<AiId>(fresh), Some(&AiId(41)));
    }

    #[test]
    fn parse_accepts_both_forms() {
        assert_eq!(AiId::parse("e42"), Some(AiId(42)));
        assert_eq!(AiId::parse("42"), Some(AiId(42)));
        assert_eq!(AiId::parse("e0"), None);
        assert_eq!(AiId::parse("x"), None);
    }
}
