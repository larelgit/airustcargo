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

use crate::invariants::{with_query, Offenders};
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

/// This entity leaves no tombstone when it goes: short-lived, many at a time (a spell's bolts and
/// zones), whose ids would push the ones worth remembering out of the graveyard ("e57 died at tick
/// 1530"). An unknown id of one is reported as never seen. It follows from the component that
/// requires it; it is reflected so a snapshot or the network carries it like any marker.
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct NoTombstone;

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
    /// Why it is gone, when that is not a plain despawn (`out of view`: a client no longer
    /// receives it): `e57 is out of view since tick 1180`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `AiId` ⇄ `Entity` lookup, maintained by component hooks (never by scanning).
///
/// A client of a networked game holds two kinds of ids: the host's, which arrive explicit
/// and are the same on every machine, and its own, for entities only it has (effects, its
/// prediction). [`set_local_base`](Self::set_local_base) keeps them apart: its own come from a
/// second counter that starts at the base, the host's (below it) never move that counter.
#[derive(Resource, Debug)]
pub struct AiIndex {
    next: u32,
    /// Where ids handed out here start, when they must not meet explicit ones (a client).
    local_base: Option<u32>,
    /// The next id handed out from the local range.
    local_next: u32,
    live: HashMap<AiId, Entity>,
    graveyard: VecDeque<Tombstone>,
    /// Old ids between `on_discard` and the `on_insert` (new value) or `on_remove`
    /// (no value) that follows it.
    discarded: HashMap<Entity, AiId>,
}

const GRAVEYARD_LEN: usize = 256;

impl Default for AiIndex {
    fn default() -> Self {
        Self {
            next: 1,
            local_base: None,
            local_next: 0,
            live: HashMap::default(),
            graveyard: VecDeque::new(),
            discarded: HashMap::default(),
        }
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

    /// From now on, ids handed out here (`AiId(0)`, or an id that is taken) come from a
    /// counter starting at `base`; explicit ids below it are indexed as given and move only the
    /// shared counter ([`next_id`](Self::next_id)). For a client: the host's ids are explicit
    /// and below the base, so they never meet the client's own.
    pub fn set_local_base(&mut self, base: u32) {
        let base = base.max(1);
        let highest_local = self.live.keys().map(|id| id.0).filter(|&n| n >= base).max();
        self.local_base = Some(base);
        // No id follows `u32::MAX`: such a highest id leaves the counter where it was.
        self.local_next = highest_local.and_then(|n| n.checked_add(1)).unwrap_or(base).max(self.local_next).max(base);
    }

    /// The base [`set_local_base`](Self::set_local_base) set, if any.
    pub fn local_base(&self) -> Option<u32> {
        self.local_base
    }

    /// Says why `id` is gone on its newest tombstone (see [`Tombstone::note`]).
    pub fn annotate(&mut self, id: AiId, note: impl Into<String>) {
        if let Some(tombstone) = self.graveyard.iter_mut().rev().find(|t| t.id == id) {
            tombstone.note = Some(note.into());
        }
    }

    /// The next id for an entity that brings none.
    fn allocate(&mut self) -> AiId {
        match self.local_base {
            Some(_) => {
                let id = AiId(self.local_next);
                self.local_next += 1;
                id
            }
            None => {
                let id = AiId(self.next);
                self.next += 1;
                id
            }
        }
    }

    /// An explicit id was kept: the counter it belongs to moves past it. `u32::MAX` has no id
    /// after it, so it leaves the counter alone (it must not wrap to 0 or saturate onto itself).
    fn keep_explicit(&mut self, id: AiId) {
        let after = id.0.checked_add(1);
        match self.local_base {
            Some(base) if id.0 >= base => {
                warn!("{id} is in this game's local id range (from e{base}); ids from the host are below it");
                if let Some(after) = after {
                    self.local_next = self.local_next.max(after);
                }
            }
            _ => {
                if let Some(after) = after {
                    self.next = self.next.max(after);
                }
            }
        }
    }

    /// Puts the counter and the tombstones back as a snapshot had them. The counter never
    /// drops below the highest live id (below the local base, if one is set), so a stale
    /// snapshot cannot hand out an id twice.
    pub(crate) fn restore_counters(&mut self, next: u32, graveyard: impl IntoIterator<Item = Tombstone>) {
        let base = self.local_base.unwrap_or(u32::MAX);
        let highest = self.live.keys().map(|id| id.0).filter(|&n| n < base).max().unwrap_or(0);
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
            Some(Tombstone { note: Some(note), tick, .. }) => format!("{id} is {note} since tick {tick}"),
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
            index.allocate()
        } else {
            index.keep_explicit(current);
            current
        };
        if let Some(owner) = taken_by {
            warn!("{current} is already used by {owner}; {} gets {id} instead", ctx.entity);
        }
        index.live.insert(id, ctx.entity);
        if let Some(old) = index.discarded.remove(&ctx.entity)
            && old != id
        {
            index.bury(Tombstone { id: old, label, tick, renumbered_to: Some(id), note: None });
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
    let forget = world.get::<NoTombstone>(ctx.entity).is_some();
    let Some(mut index) = world.get_resource_mut::<AiIndex>() else {
        return;
    };
    index.discarded.remove(&ctx.entity);
    if index.live.get(&id) == Some(&ctx.entity) {
        index.live.remove(&id);
    }
    if id.0 != 0 && !forget {
        index.bury(Tombstone { id, label, tick, renumbered_to: None, note: None });
    }
}

impl AiIndex {
    /// The rule `ai.index_consistent`: the index and the `AiId` components agree both
    /// ways. Every entity with a non-zero `AiId` is found in the index under that id, every
    /// live entry of the index points at an existing entity that carries that id, and the
    /// counts match. Two entities with one id cannot both pass the first check, so
    /// "no id is used twice" follows. Tombstones and the transient `discarded` map are
    /// history, not state, and are not looked at. Reports the lowest broken id first.
    pub(crate) fn check(&self, world: &World) -> Option<String> {
        let mut broken = Offenders::default();
        let mut with_id = 0usize;
        with_query::<(Entity, &AiId), (), _>(world, |query| {
            for (entity, id) in query.iter(world) {
                if id.0 == 0 {
                    continue;
                }
                with_id += 1;
                if self.live.get(id) != Some(&entity) {
                    broken.note(*id, || match self.live.get(id) {
                        Some(other) => format!("AiId is on {entity} but the index maps it to {other}"),
                        None => format!("AiId is on {entity} but the index does not have it"),
                    });
                }
            }
        })?;
        for (id, entity) in &self.live {
            if world.get::<AiId>(*entity) != Some(id) {
                broken.note(*id, || match world.get_entity(*entity) {
                    Ok(_) => format!("is in the index for {entity}, which carries {:?}", world.get::<AiId>(*entity)),
                    Err(_) => format!("is in the index for {entity}, which no longer exists"),
                });
            }
        }
        if broken.count() == 0 && with_id != self.live.len() {
            return Some(format!("the index holds {} ids, the world has {with_id} entities with an AiId", self.live.len()));
        }
        broken.report()
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
    fn the_largest_id_is_kept_and_leaves_the_counter_alone() {
        let mut w = world();
        let last = w.spawn((AiVisible, AiId(u32::MAX))).id();
        assert_eq!(w.get::<AiId>(last), Some(&AiId(u32::MAX)));
        assert_eq!(w.resource::<AiIndex>().next_id(), 1, "no id comes after it, so the counter stays");
        let next = w.spawn(AiVisible).id();
        assert_eq!(w.get::<AiId>(next), Some(&AiId(1)), "the next spawn still gets the next normal id");

        // The same in the local range of a client, and when the base is set after it.
        let mut w = world();
        w.resource_mut::<AiIndex>().set_local_base(1000);
        let last = w.spawn((AiVisible, AiId(u32::MAX))).id();
        assert_eq!(w.get::<AiId>(last), Some(&AiId(u32::MAX)));
        let local = w.spawn(AiVisible).id();
        assert_eq!(w.get::<AiId>(local), Some(&AiId(1000)));

        let mut w = world();
        w.spawn((AiVisible, AiId(u32::MAX)));
        w.resource_mut::<AiIndex>().set_local_base(1000);
        let local = w.spawn(AiVisible).id();
        assert_eq!(w.get::<AiId>(local), Some(&AiId(1000)));
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
    fn local_ids_and_explicit_ids_below_the_base_never_meet() {
        let mut w = world();
        w.spawn((AiVisible, AiId(5)));
        w.resource_mut::<AiIndex>().set_local_base(1000);
        let local = w.spawn(AiVisible).id();
        assert_eq!(w.get::<AiId>(local), Some(&AiId(1000)), "local ids start at the base");
        // The host's ids arrive explicit, in any order, and never move the local counter.
        let host = w.spawn((AiVisible, AiId(700))).id();
        w.spawn((AiVisible, AiId(6)));
        assert_eq!(w.get::<AiId>(host), Some(&AiId(700)));
        let local2 = w.spawn(AiVisible).id();
        assert_eq!(w.get::<AiId>(local2), Some(&AiId(1001)));
        // An explicit id that is taken: the newcomer gets a local one.
        let clash = w.spawn((AiVisible, AiId(700))).id();
        assert_eq!(w.get::<AiId>(clash), Some(&AiId(1002)));
        let index = w.resource::<AiIndex>();
        assert_eq!(index.next_id(), 701, "the shared counter follows the host's ids");
        assert_eq!(index.local_base(), Some(1000));
        w.resource_mut::<AiIndex>().restore_counters(3, []);
        assert_eq!(w.resource::<AiIndex>().next_id(), 701, "local ids do not push the shared counter");
    }

    #[test]
    fn a_tombstone_note_says_why_the_id_is_gone() {
        let mut w = world();
        let a = w.spawn(AiVisible).id();
        w.despawn(a);
        w.resource_mut::<AiIndex>().annotate(AiId(1), "out of view");
        let err = w.resource::<AiIndex>().resolve(AiId(1)).unwrap_err();
        assert!(err.starts_with("e1 is out of view since tick 0"), "{err}");
    }

    #[test]
    fn parse_accepts_both_forms() {
        assert_eq!(AiId::parse("e42"), Some(AiId(42)));
        assert_eq!(AiId::parse("42"), Some(AiId(42)));
        assert_eq!(AiId::parse("e0"), None);
        assert_eq!(AiId::parse("x"), None);
    }
}

#[cfg(test)]
mod index_rule_tests {
    use super::*;

    fn world() -> World {
        let mut w = World::new();
        w.init_resource::<AiIndex>();
        w
    }

    fn check(w: &World) -> Option<String> {
        w.resource::<AiIndex>().check(w)
    }

    #[test]
    fn a_consistent_index_passes_through_spawn_renumber_and_despawn() {
        let mut w = world();
        let a = w.spawn(AiVisible).id();
        let b = w.spawn((AiVisible, AiId(40))).id();
        w.spawn(AiVisible);
        assert_eq!(check(&w), None);
        w.entity_mut(a).insert(AiId(99));
        w.despawn(b);
        assert_eq!(check(&w), None);
    }

    #[test]
    fn an_id_written_in_place_is_found_both_ways() {
        let mut w = world();
        let a = w.spawn(AiVisible).id();
        w.spawn(AiVisible);
        // The hooks do not run for an in-place write: e1 stays in the index, the entity says e7.
        w.get_mut::<AiId>(a).unwrap().0 = 7;
        let text = check(&w).unwrap();
        assert!(text.starts_with("e1 is in the index for"), "{text}");
        assert!(text.ends_with("(+1 more)"), "{text}");
    }

    #[test]
    fn two_entities_with_one_id_are_found() {
        let mut w = world();
        w.spawn(AiVisible);
        let b = w.spawn(AiVisible).id();
        w.get_mut::<AiId>(b).unwrap().0 = 1;
        let text = check(&w).unwrap();
        assert!(text.starts_with("e1 AiId is on"), "{text}");
    }

    #[test]
    fn an_entity_missing_from_the_index_is_found() {
        let mut w = world();
        w.insert_resource(AiIndex::default());
        let a = w.spawn(AiVisible).id();
        w.resource_mut::<AiIndex>().live.clear();
        let text = check(&w).unwrap();
        assert!(text.contains(&format!("AiId is on {a} but the index does not have it")), "{text}");
    }
}
