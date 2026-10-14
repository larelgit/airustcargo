//! Stable ids for the AI layer.
//!
//! `Entity` indices are recycled: after a skeleton dies, a freshly spawned arrow can
//! get the same index, and an agent that says "attack e42" hits the wrong thing.
//! [`AiId`] only ever grows and is what agents, journals, replays and the CLI use.
//!
//! The engine owns the id. [`AiIndex`] follows it through the component hooks
//! `on_insert` and `on_discard`, so a new value inserted over an old one (a raw BRP
//! `world.insert_components`) renumbers the entity and leaves a tombstone for the
//! old id. In-place writes would bypass the hooks, so the `set` tool and BRP's
//! `world.mutate_components` refuse `AiId`.

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
/// `AiId(0)` means "not assigned yet": the insert hook replaces it with the next free id.
/// A non-zero id (e.g. restored from a snapshot) is kept as is, unless another live
/// entity already has it; then the newcomer gets the next free id instead.
///
/// Managed by the engine: never write it in place (`get_mut`, reflection). Inserting a
/// new `AiId` over an existing one renumbers the entity.
#[derive(
    Component, Reflect, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, Debug, Serialize, Deserialize,
)]
#[reflect(Component, Default)]
#[component(on_insert = on_insert_ai_id, on_discard = on_discard_ai_id, on_remove = on_remove_ai_id)]
pub struct AiId(pub u32);

/// Why tools refuse to write [`AiId`].
pub const AI_ID_IS_MANAGED: &str = "AiId is managed by the engine; despawn and spawn instead";

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

/// What is known about an id that is gone, for helpful errors ("e57 died at tick 1530").
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Tombstone {
    pub id: AiId,
    pub label: String,
    pub tick: u64,
    /// The entity lives on under this id: a new `AiId` was inserted over the old one.
    pub renumbered_to: Option<AiId>,
}

/// `AiId` ⇄ `Entity` lookup, maintained by component hooks (never by scanning).
#[derive(Resource, Debug)]
pub struct AiIndex {
    next: u32,
    live: HashMap<AiId, Entity>,
    graveyard: VecDeque<Tombstone>,
    /// Old ids between `on_discard` and the `on_insert` (new value) or `on_remove`
    /// (no value) that follows it.
    discarded: HashMap<Entity, AiId>,
}

const GRAVEYARD_LEN: usize = 256;

impl Default for AiIndex {
    fn default() -> Self {
        Self { next: 1, live: HashMap::default(), graveyard: VecDeque::new(), discarded: HashMap::default() }
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

    /// The tombstones, oldest first (for snapshots).
    pub fn graveyard(&self) -> impl Iterator<Item = &Tombstone> {
        self.graveyard.iter()
    }

    /// Live ids with their entities, in ascending id order.
    pub fn live(&self) -> Vec<(AiId, Entity)> {
        let mut live: Vec<(AiId, Entity)> = self.live.iter().map(|(id, e)| (*id, *e)).collect();
        live.sort_unstable_by_key(|(id, _)| *id);
        live
    }

    /// Puts the counter and the tombstones back as a snapshot had them. The counter never
    /// drops below the highest live id, so a stale snapshot cannot hand out an id twice.
    pub(crate) fn restore_counters(&mut self, next: u32, graveyard: impl IntoIterator<Item = Tombstone>) {
        let highest = self.live.keys().map(|id| id.0).max().unwrap_or(0);
        self.next = next.max(highest + 1).max(1);
        self.graveyard = graveyard.into_iter().collect();
        while self.graveyard.len() > GRAVEYARD_LEN {
            self.graveyard.pop_front();
        }
    }

    /// Like [`get`](Self::get), but the error explains what happened and what exists,
    /// so an agent can correct itself instead of retrying blindly.
    pub fn resolve(&self, id: AiId) -> Result<Entity, String> {
        if let Some(e) = self.get(id) {
            return Ok(e);
        }
        let mut msg = match self.tombstone(id) {
            Some(Tombstone { label, tick, renumbered_to: Some(new), .. }) => {
                format!("{id} no longer exists ({label} renumbered to {new} at tick {tick})")
            }
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

impl AiIndex {
    fn bury(&mut self, tombstone: Tombstone) {
        if self.graveyard.len() == GRAVEYARD_LEN {
            self.graveyard.pop_front();
        }
        self.graveyard.push_back(tombstone);
    }
}

fn tombstone_label(world: &DeferredWorld, entity: Entity) -> String {
    world.get::<Name>(entity).map(|n| n.as_str().to_owned()).unwrap_or_else(|| "entity".to_owned())
}

/// A value was written (spawn, insert, or insert over an old id): assign and index it.
fn on_insert_ai_id(mut world: DeferredWorld, ctx: HookContext) {
    let current = world.get::<AiId>(ctx.entity).copied().unwrap_or_default();
    let label = tombstone_label(&world, ctx.entity);
    let tick = world.get_resource::<SimTick>().map_or(0, |t| t.0);
    let id = {
        let Some(mut index) = world.get_resource_mut::<AiIndex>() else {
            return;
        };
        let taken_by = index.live.get(&current).copied().filter(|e| current.0 != 0 && *e != ctx.entity);
        let id = if current.0 == 0 || taken_by.is_some() {
            let id = AiId(index.next);
            index.next += 1;
            id
        } else {
            index.next = index.next.max(current.0 + 1);
            current
        };
        if let Some(owner) = taken_by {
            warn!("{current} is already used by {owner}; {} gets {id} instead", ctx.entity);
        }
        index.live.insert(id, ctx.entity);
        if let Some(old) = index.discarded.remove(&ctx.entity)
            && old != id
        {
            index.bury(Tombstone { id: old, label, tick, renumbered_to: Some(id) });
        }
        id
    };
    if id != current
        && let Some(mut slot) = world.get_mut::<AiId>(ctx.entity)
    {
        *slot = id;
    }
}

/// The value is about to go: replaced by a new `AiId`, removed, or despawned.
fn on_discard_ai_id(mut world: DeferredWorld, ctx: HookContext) {
    let Some(id) = world.get::<AiId>(ctx.entity).copied() else {
        return;
    };
    let Some(mut index) = world.get_resource_mut::<AiIndex>() else {
        return;
    };
    if index.live.get(&id) == Some(&ctx.entity) {
        index.live.remove(&id);
    }
    index.discarded.insert(ctx.entity, id);
}

/// The entity is despawned or loses its `AiId` for good.
fn on_remove_ai_id(mut world: DeferredWorld, ctx: HookContext) {
    let Some(id) = world.get::<AiId>(ctx.entity).copied() else {
        return;
    };
    let label = tombstone_label(&world, ctx.entity);
    let tick = world.get_resource::<SimTick>().map_or(0, |t| t.0);
    let Some(mut index) = world.get_resource_mut::<AiIndex>() else {
        return;
    };
    index.discarded.remove(&ctx.entity);
    if index.live.get(&id) == Some(&ctx.entity) {
        index.live.remove(&id);
    }
    if id.0 != 0 {
        index.bury(Tombstone { id, label, tick, renumbered_to: None });
    }
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
    fn inserting_a_new_id_renumbers_the_entity() {
        let mut w = world();
        let a = w.spawn((AiVisible, Name::new("Skeleton"))).id();
        let b = w.spawn(AiVisible).id();
        w.entity_mut(a).insert(AiId(99));
        let index = w.resource::<AiIndex>();
        assert_eq!(index.get(AiId(99)), Some(a));
        assert_eq!(index.get(AiId(1)), None);
        assert_eq!(index.get(AiId(2)), Some(b));
        assert_eq!(index.len(), 2, "no stale entry for the old id");
        let err = index.resolve(AiId(1)).unwrap_err();
        assert!(err.contains("e1 no longer exists (Skeleton renumbered to e99 at tick 0)"), "{err}");
        let c = w.spawn(AiVisible).id();
        assert_eq!(w.get::<AiId>(c), Some(&AiId(100)), "the counter moves past the inserted id");

        // The same id again changes nothing; despawning still leaves a plain tombstone.
        w.entity_mut(a).insert(AiId(99));
        assert_eq!(w.resource::<AiIndex>().get(AiId(99)), Some(a));
        w.despawn(a);
        assert!(w.resource::<AiIndex>().resolve(AiId(99)).unwrap_err().contains("Skeleton despawned at tick 0"));
    }

    #[test]
    fn an_id_in_use_is_not_taken_over() {
        let mut w = world();
        let a = w.spawn(AiVisible).id();
        let b = w.spawn(AiVisible).id();
        w.entity_mut(b).insert(AiId(1));
        assert_eq!(w.get::<AiId>(a), Some(&AiId(1)));
        assert_eq!(w.get::<AiId>(b), Some(&AiId(3)), "the newcomer gets a fresh id");
        let index = w.resource::<AiIndex>();
        assert_eq!((index.get(AiId(1)), index.get(AiId(3))), (Some(a), Some(b)));
        assert!(index.resolve(AiId(2)).unwrap_err().contains("renumbered to e3"));
    }

    #[test]
    fn parse_accepts_both_forms() {
        assert_eq!(AiId::parse("e42"), Some(AiId(42)));
        assert_eq!(AiId::parse("42"), Some(AiId(42)));
        assert_eq!(AiId::parse("e0"), None);
        assert_eq!(AiId::parse("x"), None);
    }
}
