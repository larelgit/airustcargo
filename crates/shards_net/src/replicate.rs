//! State replication: what a host sends each client, and how a client puts it into its world.
//!
//! **Host.** Every `send_every` ticks it works out each client's view (the players' bodies,
//! [`NetAlways`] entities and everything within `interest_radius` of the client's body, with
//! hysteresis, plus the entities their values refer to), encodes each viewed entity once
//! ([`EncodeCache`]: a component whose change tick is older than its cached value is not
//! encoded again; the value's hash decides what is sent), and builds a [`StateDelta`] against
//! the newest view the client acknowledged ([`SentView`], 64 kept per client). The own body's
//! predicted components travel in `own` instead.
//!
//! **Client.** [`ClientRing`] keeps the last 32 applied views. A delta is laid over its
//! baseline into the full view at its tick; the difference from the view applied last goes
//! into the world through the shared state codec (`ai_core::state::apply_changes`).

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::sync::Arc;

use ai_core::state::{apply_changes, ComponentValues, StateChanges, StateEncoder, StateRule};
use ai_core::{AiId, AiIndex};
use bevy::ecs::change_detection::Tick;
use bevy::prelude::*;
use serde_json::Value;
use shards_core::PlayerId;

use crate::msg::{EntityDelta, OwnState, StateDelta};

/// Ids a client hands out itself start here; the host's are below.
pub const LOCAL_BASE: u32 = 1 << 30;
/// Views a host keeps per client for baselines.
pub const SENT_KEPT: usize = 64;
/// Views a client keeps.
pub const CLIENT_KEPT: usize = 32;
/// Without a newer acknowledgement for this many ticks, a client gets whole views.
pub const ACK_PATIENCE: u64 = 30;
/// Entities a reference closure may add to a view.
pub const CLOSURE_CAP: usize = 64;

/// In every client's view wherever it is (a quest giver, a world boss's health bar).
#[derive(Component, Reflect, Clone, Copy, Debug, Default)]
#[reflect(Component, Default)]
pub struct NetAlways;

/// FNV-1a, 64 bit: the same on every machine and build.
#[derive(Clone, Copy)]
pub struct Fnv(pub u64);

impl Default for Fnv {
    fn default() -> Self {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
}

impl Fnv {
    pub fn eat(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 ^= *b as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }

    pub fn eat_u64(&mut self, n: u64) {
        self.eat(&n.to_le_bytes());
    }
}

/// The hash of one encoded value: FNV over its JSON bytes.
pub fn value_hash(value: &Value) -> u64 {
    let mut h = Fnv::default();
    h.eat(&serde_json::to_vec(value).unwrap_or_default());
    h.0
}

/// The desync hash of a view: ids in order, each component `(path, value hash)` in path
/// order, then the resources by name.
pub fn view_hash<'a>(
    entities: impl Iterator<Item = (u32, Vec<(&'a str, u64)>)>,
    resources: impl Iterator<Item = (&'a str, u64)>,
) -> u64 {
    let mut h = Fnv::default();
    for (id, components) in entities {
        h.eat_u64(id as u64);
        for (path, hash) in components {
            h.eat(path.as_bytes());
            h.eat_u64(hash);
        }
    }
    h.eat(b"resources");
    for (name, hash) in resources {
        h.eat(name.as_bytes());
        h.eat_u64(hash);
    }
    h.0
}

// --- host: encoding -----------------------------------------------------------------------

struct Cached {
    entity: Entity,
    value: Value,
    hash: u64,
    at: Tick,
}

/// Encoded values kept between send ticks, by `(id, type path)`.
#[derive(Default)]
pub struct EncodeCache {
    values: HashMap<(u32, String), Cached>,
    /// Components encoded and reused, for `net_status`.
    pub encoded: u64,
    pub reused: u64,
}

impl EncodeCache {
    pub fn clear(&mut self) {
        self.values.clear();
    }
}

/// One entity as encoded on a send tick.
#[derive(Clone, Debug, Default)]
pub struct EncodedEntity {
    pub values: ComponentValues,
    pub hashes: BTreeMap<String, u64>,
    pub position: Option<Vec3>,
}

/// Everything encoded on one send tick, shared by every client's delta.
#[derive(Default)]
pub struct Encoded {
    pub entities: BTreeMap<u32, Arc<EncodedEntity>>,
    pub resources: BTreeMap<String, (Value, u64)>,
}

impl Encoded {
    /// Encodes the replicated resources.
    pub fn resources_of(world: &World) -> BTreeMap<String, (Value, u64)> {
        let (values, _) = StateEncoder::new(world, StateRule::Replicated).resources();
        values.into_iter().map(|(k, v)| (k, (value_hash(&v), v))).map(|(k, (h, v))| (k, (v, h))).collect()
    }

    /// Encodes entity `id` unless it is encoded already.
    pub fn ensure(&mut self, world: &World, encoder: &StateEncoder, cache: &mut EncodeCache, id: u32) {
        if self.entities.contains_key(&id) {
            return;
        }
        let Some(entity) = world.resource::<AiIndex>().get(AiId(id)) else { return };
        let now = encoder.change_tick();
        let mut reused: Vec<&'static str> = Vec::new();
        let encoded = {
            let values = &cache.values;
            encoder.entity_reusing(entity, &mut |path, changed| {
                let cached = values.get(&(id, path.to_owned()))?;
                if cached.entity != entity || changed.is_newer_than(cached.at, now) {
                    return None;
                }
                reused.push(path);
                Some(cached.value.clone())
            })
        };
        let Some((values, _)) = encoded else { return };
        let mut hashes = BTreeMap::new();
        for (path, value) in &values {
            let key = (id, path.clone());
            if reused.contains(&path.as_str()) {
                cache.reused += 1;
                hashes.insert(path.clone(), cache.values[&key].hash);
            } else {
                cache.encoded += 1;
                let hash = value_hash(value);
                hashes.insert(path.clone(), hash);
                cache.values.insert(key, Cached { entity, value: value.clone(), hash, at: now });
            }
        }
        let position = world.get::<Transform>(entity).map(|t| t.translation);
        self.entities.insert(id, Arc::new(EncodedEntity { values, hashes, position }));
    }
}

/// The ids a client at `center` sees: players, [`NetAlways`] and entities without a
/// position everywhere; the rest within `radius`, or within `radius + hysteresis` when they
/// were in view already. Host ids only.
pub fn base_view(world: &mut World, center: Option<Vec3>, before: &BTreeSet<u32>, radius: f32, hysteresis: f32) -> BTreeSet<u32> {
    let mut q = world.query::<(&AiId, Option<&Transform>, Has<PlayerId>, Has<NetAlways>)>();
    let mut view = BTreeSet::new();
    for (id, transform, player, always) in q.iter(world) {
        if id.0 >= LOCAL_BASE {
            continue;
        }
        let inside = match (transform, center) {
            _ if player || always => true,
            (None, _) | (_, None) => true,
            (Some(t), Some(c)) => {
                let d = t.translation.distance(c);
                d <= radius || (before.contains(&id.0) && d <= radius + hysteresis)
            }
        };
        if inside {
            view.insert(id.0);
        }
    }
    view
}

/// Every `eN` an encoded value mentions.
pub fn references(value: &Value, out: &mut BTreeSet<u32>) {
    match value {
        Value::String(s) => {
            if let Some(n) = s.strip_prefix('e').and_then(|d| d.parse::<u32>().ok()) {
                out.insert(n);
            }
        }
        Value::Array(items) => items.iter().for_each(|v| references(v, out)),
        Value::Object(map) => map.values().for_each(|v| references(v, out)),
        _ => {}
    }
}

/// `view` plus what its entities refer to (transitively), encoding as it goes. Returns
/// whether the cap stopped it.
pub fn close_view(world: &World, encoder: &StateEncoder, cache: &mut EncodeCache, encoded: &mut Encoded, view: &mut BTreeSet<u32>) -> bool {
    let mut work: Vec<u32> = view.iter().copied().collect();
    let mut added = 0;
    while let Some(id) = work.pop() {
        encoded.ensure(world, encoder, cache, id);
        let Some(entity) = encoded.entities.get(&id).cloned() else { continue };
        let mut refs = BTreeSet::new();
        for value in entity.values.values() {
            references(value, &mut refs);
        }
        for r in refs {
            if r >= LOCAL_BASE || view.contains(&r) || world.resource::<AiIndex>().get(AiId(r)).is_none() {
                continue;
            }
            if added >= CLOSURE_CAP {
                return true;
            }
            added += 1;
            view.insert(r);
            work.push(r);
        }
    }
    false
}

// --- host: deltas -------------------------------------------------------------------------

/// What a client was sent at one tick: component hashes by entity, resource hashes.
#[derive(Clone, Debug, Default)]
pub struct SentView {
    pub entities: BTreeMap<u32, BTreeMap<String, u64>>,
    pub resources: BTreeMap<String, u64>,
}

/// Inputs to one client's delta.
pub struct DeltaInput<'a> {
    pub tick: u64,
    pub view: &'a BTreeSet<u32>,
    pub encoded: &'a Encoded,
    pub baseline: Option<(u64, &'a SentView)>,
    /// The client's body and the components it predicts.
    pub own: Option<(u32, &'a BTreeSet<String>)>,
    pub applied_input_tick: u64,
    pub with_hash: bool,
    pub cap_bytes: usize,
    pub center: Option<Vec3>,
}

/// A delta, what it records as sent, and what the cap held back.
pub struct Built {
    pub delta: StateDelta,
    pub sent: SentView,
    pub held_back: Vec<u32>,
    pub bytes: usize,
}

/// Builds one client's delta.
pub fn build_delta(input: DeltaInput) -> Built {
    let DeltaInput { tick, view, encoded, baseline, own, applied_input_tick, with_hash, cap_bytes, center } = input;
    let base = baseline.map(|(_, b)| b);
    let mut sent = SentView::default();
    let mut entities = Vec::new();
    let mut own_state = None;
    for id in view {
        let Some(entity) = encoded.entities.get(id) else { continue };
        let predicted = own.filter(|(body, _)| body == id).map(|(_, p)| p);
        let mut hashes = BTreeMap::new();
        let mut set = BTreeMap::new();
        let before = base.and_then(|b| b.entities.get(id));
        for (path, value) in &entity.values {
            if let Some(predicted) = predicted
                && predicted.contains(path)
            {
                own_state.get_or_insert_with(|| OwnState { body: *id, applied_input_tick, components: BTreeMap::new() }).components.insert(path.clone(), value.clone());
                continue;
            }
            let hash = entity.hashes[path];
            hashes.insert(path.clone(), hash);
            if before.and_then(|b| b.get(path)) != Some(&hash) {
                set.insert(path.clone(), value.clone());
            }
        }
        let removed: Vec<String> = before.map(|b| b.keys().filter(|p| !hashes.contains_key(*p)).cloned().collect()).unwrap_or_default();
        if !set.is_empty() || !removed.is_empty() || before.is_none() {
            entities.push(EntityDelta { id: *id, set, removed });
        }
        sent.entities.insert(*id, hashes);
    }
    if own.is_some() && own_state.is_none() {
        own_state = own.map(|(body, _)| OwnState { body, applied_input_tick, components: BTreeMap::new() });
    }
    let gone: Vec<u32> = base.map(|b| b.entities.keys().filter(|id| !sent.entities.contains_key(id)).copied().collect()).unwrap_or_default();
    let mut resources = BTreeMap::new();
    for (name, (value, hash)) in &encoded.resources {
        sent.resources.insert(name.clone(), *hash);
        if base.and_then(|b| b.resources.get(name)) != Some(hash) {
            resources.insert(name.clone(), value.clone());
        }
    }
    let mut delta = StateDelta { tick, baseline: baseline.map(|(t, _)| t), entities, gone, resources, own: own_state, ..default() };
    let mut bytes = crate::msg::encode(&delta).len();
    let mut held_back = Vec::new();
    if bytes > cap_bytes {
        // The farthest wait for the next delta: as sent, they stay what the baseline had.
        let distance = |id: u32| match (encoded.entities.get(&id).and_then(|e| e.position), center) {
            (Some(p), Some(c)) => p.distance(c),
            _ => 0.0,
        };
        let mut order: Vec<(usize, f32)> = delta.entities.iter().enumerate().map(|(i, e)| (i, distance(e.id))).collect();
        order.sort_by(|a, b| b.1.total_cmp(&a.1).then(b.0.cmp(&a.0)));
        let sizes: Vec<usize> = delta.entities.iter().map(|e| crate::msg::encode(e).len() + 1).collect();
        let mut drop = BTreeSet::new();
        for (i, _) in order {
            if bytes <= cap_bytes {
                break;
            }
            let id = delta.entities[i].id;
            if own.is_some_and(|(body, _)| body == id) {
                continue;
            }
            drop.insert(i);
            bytes = bytes.saturating_sub(sizes[i]);
            held_back.push(id);
            match base.and_then(|b| b.entities.get(&id)) {
                Some(kept) => {
                    sent.entities.insert(id, kept.clone());
                }
                None => {
                    sent.entities.remove(&id);
                }
            }
        }
        let mut i = 0;
        delta.entities.retain(|_| {
            let keep = !drop.contains(&i);
            i += 1;
            keep
        });
        held_back.sort_unstable();
        bytes = crate::msg::encode(&delta).len();
    }
    if with_hash && held_back.is_empty() {
        let entities = sent.entities.iter().map(|(id, h)| (*id, h.iter().map(|(p, v)| (p.as_str(), *v)).collect()));
        delta.hash = Some(view_hash(entities, sent.resources.iter().map(|(n, h)| (n.as_str(), *h))));
    }
    Built { delta, sent, held_back, bytes }
}

// --- client -------------------------------------------------------------------------------

/// A full view as the client holds it.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewState {
    pub entities: BTreeMap<u32, ComponentValues>,
    pub resources: BTreeMap<String, Value>,
}

/// Why a delta was not applied.
#[derive(Clone, Debug, PartialEq)]
pub enum Skipped {
    /// Not newer than what is applied.
    Stale,
    /// Its baseline is not kept here.
    NoBaseline(u64),
}

/// The client's applied views.
#[derive(Default)]
pub struct ClientRing {
    states: VecDeque<(u64, Arc<ViewState>)>,
    pub last_applied: Option<u64>,
    pub stale: u64,
    pub missing_baseline: u64,
}

impl ClientRing {
    pub fn clear(&mut self) {
        self.states.clear();
        self.last_applied = None;
    }

    pub fn get(&self, tick: u64) -> Option<&Arc<ViewState>> {
        self.states.iter().find(|(t, _)| *t == tick).map(|(_, s)| s)
    }

    /// The view at `delta.tick` and the changes from the view applied last; the ring takes it
    /// as applied.
    pub fn accept(&mut self, delta: &StateDelta) -> Result<(Arc<ViewState>, Option<Arc<ViewState>>), Skipped> {
        if self.last_applied.is_some_and(|t| delta.tick <= t) {
            self.stale += 1;
            return Err(Skipped::Stale);
        }
        let mut state = match delta.baseline {
            Some(b) => match self.get(b) {
                Some(base) => (**base).clone(),
                None => {
                    self.missing_baseline += 1;
                    return Err(Skipped::NoBaseline(b));
                }
            },
            None => ViewState::default(),
        };
        for e in &delta.entities {
            let values = state.entities.entry(e.id).or_default();
            for (path, value) in &e.set {
                values.insert(path.clone(), value.clone());
            }
            for path in &e.removed {
                values.remove(path);
            }
        }
        for id in &delta.gone {
            state.entities.remove(id);
        }
        for (name, value) in &delta.resources {
            state.resources.insert(name.clone(), value.clone());
        }
        let previous = self.last_applied.and_then(|t| self.get(t).cloned());
        let state = Arc::new(state);
        self.states.push_back((delta.tick, state.clone()));
        while self.states.len() > CLIENT_KEPT {
            self.states.pop_front();
        }
        self.last_applied = Some(delta.tick);
        Ok((state, previous))
    }
}

/// What turns the view applied last (`previous`; `None`: the world as it is, after a join or
/// a whole view) into `state`.
pub fn changes_between(world: &mut World, previous: Option<&ViewState>, state: &ViewState, keep: &BTreeSet<(u32, String)>) -> StateChanges {
    let mut changes = StateChanges::default();
    match previous {
        Some(previous) => {
            for (id, values) in &state.entities {
                let before = previous.entities.get(id);
                let set: ComponentValues = values.iter().filter(|(p, v)| before.and_then(|b| b.get(*p)) != Some(*v)).map(|(p, v)| (p.clone(), v.clone())).collect();
                if !set.is_empty() {
                    changes.set.push((*id, set));
                }
                if let Some(before) = before {
                    let removed: Vec<String> = before.keys().filter(|p| !values.contains_key(*p) && !keep.contains(&(*id, (*p).clone()))).cloned().collect();
                    if !removed.is_empty() {
                        changes.remove.push((*id, removed));
                    }
                }
            }
            for id in previous.entities.keys().filter(|id| !state.entities.contains_key(id)) {
                changes.despawn.push((*id, "out of view".to_owned()));
            }
            for (name, value) in &state.resources {
                if previous.resources.get(name) != Some(value) {
                    changes.resources.insert(name.clone(), value.clone());
                }
            }
        }
        None => {
            // A whole view: every value is written, what the world has beyond it goes.
            let index: Vec<(AiId, Entity)> = world.resource::<AiIndex>().live();
            for (id, entity) in index {
                if id.0 >= LOCAL_BASE {
                    continue;
                }
                match state.entities.get(&id.0) {
                    None => changes.despawn.push((id.0, "out of view".to_owned())),
                    Some(values) => {
                        let removed: Vec<String> = ai_core::net_rules::replicated_components(world, entity)
                            .into_iter()
                            .filter(|p| !values.contains_key(*p) && !keep.contains(&(id.0, (*p).to_owned())))
                            .map(str::to_owned)
                            .collect();
                        if !removed.is_empty() {
                            changes.remove.push((id.0, removed));
                        }
                    }
                }
            }
            for (id, values) in &state.entities {
                changes.set.push((*id, values.clone()));
            }
            changes.resources = state.resources.clone();
        }
    }
    changes
}

/// Applies `changes`; returns the codec's warnings.
pub fn apply(world: &mut World, changes: &StateChanges) -> Vec<String> {
    apply_changes(world, changes)
}

/// The client's own hash of what it holds of the host's world: every host-id entity, its
/// replicated components (minus the own body's predicted ones), the replicated resources.
pub fn world_hash(world: &mut World, own: Option<(u32, &BTreeSet<String>)>) -> (u64, ViewState) {
    let mut q = world.query::<(Entity, &AiId)>();
    let mut ids: Vec<(u32, Entity)> = q.iter(world).filter(|(_, id)| id.0 < LOCAL_BASE).map(|(e, id)| (id.0, e)).collect();
    ids.sort_unstable();
    let encoder = StateEncoder::new(world, StateRule::Replicated);
    let mut held = ViewState::default();
    for (id, entity) in ids {
        let Some((mut values, _)) = encoder.entity(entity) else { continue };
        if let Some((body, predicted)) = own
            && body == id
        {
            values.retain(|p, _| !predicted.contains(p));
        }
        held.entities.insert(id, values);
    }
    held.resources = encoder.resources().0;
    let hash = hash_of(&held);
    (hash, held)
}

/// [`view_hash`] of a held view.
pub fn hash_of(state: &ViewState) -> u64 {
    let entities = state.entities.iter().map(|(id, v)| (*id, v.iter().map(|(p, v)| (p.as_str(), value_hash(v))).collect()));
    view_hash(entities, state.resources.iter().map(|(n, v)| (n.as_str(), value_hash(v))))
}

/// The first place two views differ: `e12 Health: here {…}, host {…}`.
pub fn first_difference(here: &ViewState, host: &ViewState) -> String {
    let ids: BTreeSet<&u32> = here.entities.keys().chain(host.entities.keys()).collect();
    for id in ids {
        match (here.entities.get(id), host.entities.get(id)) {
            (Some(_), None) => return format!("e{id}: here, not on the host"),
            (None, Some(_)) => return format!("e{id}: on the host, not here"),
            (Some(a), Some(b)) => {
                let paths: BTreeSet<&String> = a.keys().chain(b.keys()).collect();
                for path in paths {
                    let (x, y) = (a.get(path), b.get(path));
                    if x != y {
                        let short = path.rsplit("::").next().unwrap_or(path);
                        let show = |v: Option<&Value>| v.map_or_else(|| "none".to_owned(), clip);
                        return format!("e{id} {short}: here {}, host {}", show(x), show(y));
                    }
                }
            }
            (None, None) => {}
        }
    }
    let names: BTreeSet<&String> = here.resources.keys().chain(host.resources.keys()).collect();
    for name in names {
        if here.resources.get(name) != host.resources.get(name) {
            return format!("resource {}", name.rsplit("::").next().unwrap_or(name));
        }
    }
    "nothing differs".to_owned()
}

fn clip(value: &Value) -> String {
    let text = value.to_string();
    if text.len() <= 120 {
        return text;
    }
    let cut = (0..=120).rev().find(|&i| text.is_char_boundary(i)).unwrap_or(0);
    format!("{}…", &text[..cut])
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn encoded(entities: &[(u32, Value)]) -> Encoded {
        let mut out = Encoded::default();
        for (id, v) in entities {
            let values: ComponentValues = v.as_object().unwrap().iter().map(|(k, v)| (k.clone(), v.clone())).collect();
            let hashes = values.iter().map(|(k, v)| (k.clone(), value_hash(v))).collect();
            out.entities.insert(*id, Arc::new(EncodedEntity { values, hashes, position: Some(Vec3::new(*id as f32, 0.0, 0.0)) }));
        }
        out
    }

    fn build(tick: u64, view: &BTreeSet<u32>, enc: &Encoded, base: Option<(u64, &SentView)>, cap: usize) -> Built {
        build_delta(DeltaInput {
            tick,
            view,
            encoded: enc,
            baseline: base,
            own: None,
            applied_input_tick: 0,
            with_hash: true,
            cap_bytes: cap,
            center: Some(Vec3::ZERO),
        })
    }

    #[test]
    fn deltas_over_baselines_rebuild_the_host_view_on_the_client() {
        let view: BTreeSet<u32> = [1, 2].into();
        let t2 = encoded(&[(1, json!({"H": 10, "P": [0, 0]})), (2, json!({"H": 5}))]);
        let full = build(2, &view, &t2, None, 1 << 20);
        let mut ring = ClientRing::default();
        let (s2, _) = ring.accept(&full.delta).unwrap();
        assert_eq!(hash_of(&s2), full.delta.hash.unwrap(), "the client's hash of the view is the host's");

        // e1 moves, e2 loses a component, e3 comes into view.
        let view: BTreeSet<u32> = [1, 2, 3].into();
        let t4 = encoded(&[(1, json!({"H": 10, "P": [1, 0]})), (2, json!({})), (3, json!({"H": 1}))]);
        let d4 = build(4, &view, &t4, Some((2, &full.sent)), 1 << 20);
        let e1 = d4.delta.entities.iter().find(|e| e.id == 1).unwrap();
        assert_eq!(e1.set.keys().collect::<Vec<_>>(), ["P"], "only what changed is sent");
        assert_eq!(d4.delta.entities.iter().find(|e| e.id == 2).unwrap().removed, ["H"]);
        let (s4, prev) = ring.accept(&d4.delta).unwrap();
        assert_eq!(hash_of(&s4), d4.delta.hash.unwrap());
        assert_eq!(prev.as_deref(), Some(&*s2));

        // e3 leaves the view: gone.
        let view: BTreeSet<u32> = [1, 2].into();
        let d6 = build(6, &view, &t4, Some((4, &d4.sent)), 1 << 20);
        assert_eq!(d6.delta.gone, [3]);
        let (s6, _) = ring.accept(&d6.delta).unwrap();
        assert!(!s6.entities.contains_key(&3));
    }

    #[test]
    fn old_out_of_order_and_baseless_deltas_are_dropped() {
        let view: BTreeSet<u32> = [1].into();
        let enc = encoded(&[(1, json!({"H": 1}))]);
        let mut ring = ClientRing::default();
        ring.accept(&build(10, &view, &enc, None, 1 << 20).delta).unwrap();
        assert_eq!(ring.accept(&build(8, &view, &enc, None, 1 << 20).delta), Err(Skipped::Stale));
        let sent = SentView::default();
        assert_eq!(ring.accept(&build(12, &view, &enc, Some((4, &sent)), 1 << 20).delta), Err(Skipped::NoBaseline(4)));
        for t in 0..40 {
            ring.accept(&build(20 + t, &view, &enc, None, 1 << 20).delta).unwrap();
        }
        assert!(ring.get(20).is_none(), "the ring keeps {CLIENT_KEPT}");
        assert!(ring.get(59).is_some());
    }

    #[test]
    fn the_cap_holds_back_the_farthest_entities() {
        let view: BTreeSet<u32> = (1..=20).collect();
        let big = "x".repeat(400);
        let enc = encoded(&(1..=20).map(|i| (i, json!({"Blob": big}))).collect::<Vec<_>>());
        let built = build(2, &view, &enc, None, 4000);
        assert!(built.bytes <= 4000, "{} bytes", built.bytes);
        assert_eq!(built.held_back.last(), Some(&20), "the farthest goes first");
        assert!(built.held_back.windows(2).all(|w| w[0] < w[1]));
        assert!(built.delta.hash.is_none(), "a capped view is not complete: no check");
        assert!(!built.sent.entities.contains_key(&20), "held back, never sent: not in the client's view");
    }

    #[test]
    fn references_name_every_id_in_a_value() {
        let mut refs = BTreeSet::new();
        references(&json!({"target": "e7", "list": ["e9", "c3", "east"], "n": 5}), &mut refs);
        assert_eq!(refs, [7, 9].into());
    }
}
