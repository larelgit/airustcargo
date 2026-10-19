//! Props: the things a body carries (a sword in the hand, a flask on the hip), drawn under the
//! bones of its model. See "Sockets and props" in [`crate::anim`] for the content.
//!
//! The engine owns the prop entities: they are spawned under the socket's bone, without `AiId`,
//! so replays, snapshots and co-op never see them, and spawned again when the model is (a hot
//! reload of the glTF takes the bones, and the props below them, with it). Where a prop sits is a
//! state read every frame from the action's time and the seconds since a weapon switch
//! ([`mount_places`]), never an edge, so it is right after a join, a snapshot load or a hold.
//!
//! Props hang under bones, below the model child that carries the render interpolation, so they
//! ride its blended pose and are never adopted for interpolation a second time. A prop's glTF is
//! not a rig: [`skeleton`] leaves the subtrees below [`PropOf`] out of every by-name walk of the
//! skeleton, and [`retarget_on_ready`](super::bridge::retarget_on_ready) gives them no player.

use std::collections::{BTreeMap, BTreeSet};

use bevy::ecs::system::SystemParam;
use bevy::gltf::GltfAssetLabel;
use bevy::prelude::*;
use bevy::world_serialization::WorldAssetRoot;
use shards_content::ContentDb;
use shards_look::Look;

use super::bridge::AnimationBound;
use crate::anim::{
    cut_holds, equip_open, mount_places, AnimSet, Animated, MountView, Place, PropDef, PropSwaps,
};
use crate::combat::{Blocked, Damaged, Evaded, GuardBroken, Parried, PerfectEvaded, Reacted, Staggered};
use crate::context::{PairEnded, PairStarted};
use crate::def::ActionDef;
use crate::machine::Running;

/// Marks a prop the engine spawned under a bone of an animated body. Not reflected and without
/// `AiId`: a picture the bridge owns, gone with its bone, never in a snapshot or a replay.
#[derive(Component, Debug, Clone, PartialEq, Eq)]
pub struct PropOf {
    /// The animated body that carries it.
    pub body: Entity,
    /// The `prop` definition it shows (after the body's [`PropSwaps`]).
    pub prop: String,
}

/// `root` and everything below it, in the order of a depth-first walk, leaving out the subtrees
/// below a [`PropOf`] entity: a prop's model may have a node named `root` or `hand.R` too, and it
/// must never stand in for a bone of the skeleton.
pub(crate) fn skeleton(root: Entity, children: &Query<&Children>, props: &Query<(), With<PropOf>>) -> Vec<Entity> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(entity) = stack.pop() {
        out.push(entity);
        if let Ok(kids) = children.get(entity) {
            stack.extend(kids.iter().rev().filter(|k| !props.contains(*k)));
        }
    }
    out
}

/// What the props of one body need to remember between frames. Cloned onto the new
/// [`AnimationBound`] when a body is bound again (a graph rebuilt, a model spawned again), so a
/// rebind neither forgets an equip nor leaves a second set of props.
#[derive(Clone, Debug, Default)]
pub(crate) struct MountState {
    /// Whether a moveset has been seen: the first one a body shows is not a change.
    seen_moveset: bool,
    moveset: Option<String>,
    /// What the body carries (prop, socket), and what it carried before its moveset changed.
    pub carried: Vec<(String, String)>,
    pub previous: Vec<(String, String)>,
    /// The simulation clock when the moveset last changed; `None` once the equip has ended.
    pub since: Option<f64>,
    /// prop id -> the entity showing it.
    props: BTreeMap<String, Spawned>,
    /// socket -> (the bone's name when found, the bone).
    pub bones: BTreeMap<String, (String, Entity)>,
    /// Sockets whose bone the model lacks (warned about once).
    missing: BTreeSet<String>,
    /// prop -> where it sat at the last frame.
    pub places: BTreeMap<String, Place>,
    run: Option<RunSeen>,
    /// A cut run's props held where they were: (prop, where, seconds of the new run), and the new
    /// run's instance.
    cut: Vec<(String, Place, f32)>,
    cut_run: Option<u32>,
    /// Events named in `ended_by` that arrived since the last frame.
    pub events: Vec<String>,
}

#[derive(Clone, Debug)]
struct Spawned {
    entity: Entity,
    /// The definition shown (the prop id after the body's swaps) and what it was when spawned.
    shown_as: String,
    def: PropDef,
    parent: Option<Entity>,
    local: Option<Transform>,
    visible: Option<bool>,
}

/// The run the props saw at the last frame.
#[derive(Clone, Debug)]
struct RunSeen {
    instance: u32,
    /// The `mounts` key whose rules apply to the run.
    rules: Option<String>,
    last_t: f32,
    length: f32,
}

impl MountState {
    /// The equip ends: the new moveset's props are in their carry sockets, the old ones are gone.
    pub(crate) fn end_equip(&mut self) {
        self.since = None;
        self.previous.clear();
    }

    /// The picture was held for `secs` of the simulation's clock: the clocks resume where they
    /// stopped.
    pub(crate) fn shift(&mut self, secs: f64) {
        if let Some(since) = &mut self.since {
            *since += secs;
        }
    }

    /// What a new binding of the same body starts from: everything but the bones found in the old
    /// model and the sockets it lacked. The new model's bones are found again (an old bone that is
    /// still there for a frame must not be taken), and the props move under them.
    pub(crate) fn taken_over(&self) -> Self {
        Self { bones: BTreeMap::new(), missing: BTreeSet::new(), ..self.clone() }
    }

    /// The entity showing `prop`, if it is drawn.
    pub fn entity(&self, prop: &str) -> Option<Entity> {
        self.props.get(prop).map(|s| s.entity)
    }

    /// The props drawn now (prop id -> entity), in prop id order.
    pub fn drawn(&self) -> impl Iterator<Item = (&str, Entity)> {
        self.props.iter().map(|(p, s)| (p.as_str(), s.entity))
    }

    /// The seconds since the last weapon switch while its equip is open.
    pub fn since(&self, clock: f64) -> Option<f32> {
        self.since.map(|s| (clock - s) as f32)
    }
}

/// What the props read of the running action.
pub(crate) struct RunView<'a> {
    pub run: &'a Running,
    pub def: Option<&'a ActionDef>,
    /// The key the set looks the action's clip up by ([`AnimSet::action_key`]).
    pub key: &'a str,
    /// Seconds into the action, looked ahead by the frame's overstep.
    pub t: f32,
}

/// One body's frame, for [`step`].
pub(crate) struct MountFrame<'a> {
    pub body: Entity,
    pub player: Entity,
    pub animset: &'a str,
    pub set: &'a AnimSet,
    pub db: &'a ContentDb,
    pub moveset: Option<&'a str>,
    pub swaps: Option<&'a PropSwaps>,
    pub run: Option<RunView<'a>>,
    /// The simulation clock (s), between ticks too.
    pub clock: f64,
}

/// What [`step`] reaches into the world with.
#[derive(SystemParam)]
#[doc(hidden)]
pub struct MountEnv<'w, 's> {
    commands: Commands<'w, 's>,
    asset_server: Res<'w, AssetServer>,
    meshes: Option<ResMut<'w, Assets<Mesh>>>,
    materials: Option<ResMut<'w, Assets<StandardMaterial>>>,
    children: Query<'w, 's, &'static Children>,
    names: Query<'w, 's, &'static Name>,
    parts: Query<'w, 's, (&'static mut Transform, &'static mut Visibility, &'static ChildOf), With<PropOf>>,
    marks: Query<'w, 's, (), With<PropOf>>,
    default_material: Local<'s, Option<Handle<StandardMaterial>>>,
    warned: Local<'s, BTreeSet<String>>,
}

impl MountEnv<'_, '_> {
    fn warn_once(&mut self, key: String, text: String) {
        if self.warned.insert(key) {
            warn!("{text}");
        }
    }

    /// The bone of `socket` under `player`: the cached one while it is still there, else the first
    /// node of that name below the skeleton (not below a prop).
    fn bone(&mut self, state: &mut MountState, frame: &MountFrame, socket: &str) -> Option<Entity> {
        let def = frame.set.sockets.get(socket)?;
        if let Some((name, bone)) = state.bones.get(socket)
            && name == &def.bone
            && self.names.get(*bone).is_ok_and(|n| n.as_str() == name)
        {
            return Some(*bone);
        }
        state.bones.remove(socket);
        if state.missing.contains(socket) {
            return None;
        }
        let found = skeleton(frame.player, &self.children, &self.marks)
            .into_iter()
            .find(|e| self.names.get(*e).is_ok_and(|n| n.as_str() == def.bone));
        match found {
            Some(bone) => {
                state.bones.insert(socket.to_owned(), (def.bone.clone(), bone));
                Some(bone)
            }
            None => {
                state.missing.insert(socket.to_owned());
                self.warn_once(
                    format!("socket {} {socket}", frame.animset),
                    format!(
                        "animset `{}`: the model has no bone `{}`, where socket `{socket}` sits; its props stay hidden",
                        frame.animset, def.bone
                    ),
                );
                None
            }
        }
    }

    /// Spawns the prop `shown_as` under `bone`.
    fn spawn(&mut self, frame: &MountFrame, prop: &str, shown_as: &str, def: &PropDef, bone: Entity, local: Transform) -> Entity {
        let mut entity = self.commands.spawn((
            PropOf { body: frame.body, prop: shown_as.to_owned() },
            Name::new(format!("prop {prop}")),
            local,
            Visibility::Inherited,
            ChildOf(bone),
        ));
        if let Some(look) = &def.look {
            entity.insert(Look::new(look.clone()));
        }
        if let Some(model) = &def.model {
            let scene = self.asset_server.load(GltfAssetLabel::Scene(0).from_asset(model.clone()));
            entity.with_child(WorldAssetRoot(scene));
        } else if let (Some(size), Some(meshes), Some(materials)) = (def.size, self.meshes.as_mut(), self.materials.as_mut()) {
            let material = self.default_material.get_or_insert_with(|| materials.add(StandardMaterial::default())).clone();
            entity.insert((Mesh3d(meshes.add(Cuboid::new(size.x, size.y, size.z))), MeshMaterial3d(material)));
        }
        entity.id()
    }
}

/// `Update`, once per body and frame ([`drive`](super::bridge::drive) calls it): the props of the
/// body, where they sit now. In the order of the design: the moveset change and the equip, the run
/// (a cut, the end of an equip), the events, then [`mount_places`] and the entities.
pub(crate) fn step(env: &mut MountEnv, frame: &MountFrame, state: &mut MountState) {
    let set = frame.set;

    // The moveset: a change opens the equip (the first one a body shows is not a change).
    let wanted: Vec<(String, String)> =
        set.carried(frame.moveset).iter().map(|c| (c.prop.clone(), c.socket.clone())).collect();
    if state.seen_moveset && state.moveset.as_deref() != frame.moveset {
        state.previous = std::mem::take(&mut state.carried);
        state.since = Some(frame.clock);
    }
    // The first moveset a body shows is its baseline, however late it appears.
    state.seen_moveset |= frame.moveset.is_some();
    if state.moveset.as_deref() != frame.moveset {
        state.moveset = frame.moveset.map(str::to_owned);
    }
    if state.carried != wanted {
        state.carried = wanted;
    }

    // The run: a new one cuts the old one when it is a whole-body action, and may end the equip.
    let instance = frame.run.as_ref().map(|r| r.run.instance);
    if state.run.as_ref().map(|r| r.instance) != instance {
        if let (Some(old), Some(new)) = (state.run.take(), &frame.run) {
            let unfinished = old.last_t < old.length - 0.02;
            if unfinished
                && !set.is_layered(new.key)
                && let Some(rules) = old.rules.as_deref().and_then(|k| set.mounts.get(k))
            {
                state.cut = cut_holds(rules, &state.carried, &state.places);
                state.cut_run = Some(new.run.instance);
            }
        }
        if let Some(new) = &frame.run {
            if set.ends_equip(new.key) {
                state.end_equip();
            }
            state.run = Some(RunSeen {
                instance: new.run.instance,
                rules: set
                    .mount_key(&new.run.action, new.run.anim.as_deref(), new.def.map_or(&[], |d| d.tags.as_slice()))
                    .map(str::to_owned),
                last_t: new.t,
                length: new.run.length,
            });
        }
    } else if let (Some(seen), Some(now)) = (state.run.as_mut(), &frame.run) {
        seen.last_t = now.t;
        seen.length = now.run.length;
    }
    if state.cut_run != instance {
        state.cut.clear();
        state.cut_run = None;
    }

    // Events that end the equip.
    let events = std::mem::take(&mut state.events);
    if set.equip_rules().is_some_and(|rules| events.iter().any(|e| rules.ended_by.contains(e))) {
        state.end_equip();
    }

    // The equip, while it has something to show.
    let equip = match (set.equip_rules(), state.since(frame.clock)) {
        (Some(rules), Some(since)) if equip_open(rules, since) => Some((rules, since)),
        (_, Some(_)) => {
            state.end_equip();
            None
        }
        _ => None,
    };

    // Where each prop sits.
    let run = frame.run.as_ref().and_then(|r| {
        let rules = state.run.as_ref()?.rules.as_deref().and_then(|k| set.mounts.get(k))?;
        Some((rules, r.def?, r.t))
    });
    let held: Vec<(String, Place)> = match &frame.run {
        Some(r) => state.cut.iter().filter(|(_, _, secs)| r.run.t < *secs).map(|(p, place, _)| (p.clone(), place.clone())).collect(),
        None => Vec::new(),
    };
    let view = MountView { carried: &state.carried, previous: &state.previous, run, equip, cut: &held };
    let placed = mount_places(&view);
    state.places = placed.iter().cloned().collect();

    // Every socket's bone, for the game's own attachments (`Sockets`) too.
    for socket in set.sockets.keys() {
        env.bone(state, frame, socket);
    }

    // The entities.
    let listed: BTreeSet<&str> = placed.iter().map(|(p, _)| p.as_str()).collect();
    let gone: Vec<String> = state.props.keys().filter(|p| !listed.contains(p.as_str())).cloned().collect();
    for prop in gone {
        if let Some(old) = state.props.remove(&prop) {
            env.commands.entity(old.entity).try_despawn();
        }
    }
    for (prop, place) in placed {
        let shown_as = frame.swaps.and_then(|s| s.0.get(&prop)).unwrap_or(&prop).clone();
        let Some(def) = frame.db.get::<PropDef>(&shown_as) else {
            env.warn_once(
                format!("prop {} {shown_as}", frame.animset),
                format!("animset `{}` carries prop `{shown_as}`, which is not in content; `content_list kind=prop` lists them", frame.animset),
            );
            continue;
        };
        // A prop whose entity is gone (the model was spawned again), or whose definition changed
        // (an edit, a variant), is spawned again.
        let stale = state.props.get(&prop).is_some_and(|s| {
            s.shown_as != shown_as || s.def != *def || !env.parts.contains(s.entity)
        });
        if stale && let Some(old) = state.props.remove(&prop) {
            env.commands.entity(old.entity).try_despawn();
        }
        let target = match &place {
            Place::Socket(socket) => env.bone(state, frame, socket).zip(set.sockets.get(socket)),
            Place::Hidden => None,
        };
        let Some(entry) = state.props.get_mut(&prop) else {
            // Hidden and not yet drawn: nothing to spawn.
            let Some((bone, socket)) = target else { continue };
            let local = def.in_socket(socket);
            let entity = env.spawn(frame, &prop, &shown_as, def, bone, local);
            state.props.insert(
                prop,
                Spawned { entity, shown_as, def: def.clone(), parent: Some(bone), local: Some(local), visible: Some(true) },
            );
            continue;
        };
        match target {
            Some((bone, socket)) => {
                let local = def.in_socket(socket);
                if entry.parent != Some(bone) {
                    env.commands.entity(entry.entity).insert(ChildOf(bone));
                    entry.parent = Some(bone);
                }
                if entry.local != Some(local)
                    && let Ok((mut transform, ..)) = env.parts.get_mut(entry.entity)
                {
                    *transform = local;
                    entry.local = Some(local);
                }
                if entry.visible != Some(true)
                    && let Ok((_, mut visibility, _)) = env.parts.get_mut(entry.entity)
                {
                    *visibility = Visibility::Inherited;
                    entry.visible = Some(true);
                }
            }
            None => {
                if entry.visible != Some(false)
                    && let Ok((_, mut visibility, _)) = env.parts.get_mut(entry.entity)
                {
                    *visibility = Visibility::Hidden;
                    entry.visible = Some(false);
                }
            }
        }
    }
}

/// Reads the sockets of animated bodies: where the engine's props sit, and where a game may hang
/// its own things (a crystal on the chest). Presentation only; read-only.
#[derive(SystemParam)]
pub struct Sockets<'w, 's> {
    bodies: Query<'w, 's, (&'static Animated, &'static AnimationBound)>,
    db: Option<Res<'w, ContentDb>>,
    globals: Query<'w, 's, &'static GlobalTransform>,
}

impl Sockets<'_, '_> {
    /// The bone socket `socket` of `body` rides, and the socket's place on it: parent a thing to
    /// the bone with this local transform, and parent it again when the entity changes (a model
    /// spawned again). `None` while the body is not bound or its model lacks the bone.
    pub fn bone(&self, body: Entity, socket: &str) -> Option<(Entity, Transform)> {
        let (animated, bound) = self.bodies.get(body).ok()?;
        let def = self.db.as_ref()?.get::<AnimSet>(&animated.animset)?.sockets.get(socket)?;
        let (name, bone) = bound.mounts.bones.get(socket)?;
        (name == &def.bone).then(|| (*bone, def.transform()))
    }

    /// The socket's place in the world, as of the last propagated frame.
    pub fn world(&self, body: Entity, socket: &str) -> Option<GlobalTransform> {
        let (bone, local) = self.bone(body, socket)?;
        Some(self.globals.get(bone).ok()?.mul_transform(local))
    }
}

/// Notes that engine event `event` named `body`: if the body's weapon-switch rules list it in
/// `ended_by`, the next frame ends the equip.
fn note(bodies: &mut Query<&mut AnimationBound>, body: Entity, event: &str) {
    if let Ok(mut bound) = bodies.get_mut(body)
        && bound.mounts.since.is_some()
    {
        bound.mounts.events.push(event.to_owned());
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(|e: On<Blocked>, mut b: Query<&mut AnimationBound>| note(&mut b, e.defender, "blocked"))
        .add_observer(|e: On<Parried>, mut b: Query<&mut AnimationBound>| note(&mut b, e.defender, "parried"))
        .add_observer(|e: On<Damaged>, mut b: Query<&mut AnimationBound>| note(&mut b, e.target, "damaged"))
        .add_observer(|e: On<Reacted>, mut b: Query<&mut AnimationBound>| note(&mut b, e.entity, "reacted"))
        .add_observer(|e: On<GuardBroken>, mut b: Query<&mut AnimationBound>| note(&mut b, e.defender, "guard_broken"))
        .add_observer(|e: On<Staggered>, mut b: Query<&mut AnimationBound>| note(&mut b, e.entity, "staggered"))
        .add_observer(|e: On<Evaded>, mut b: Query<&mut AnimationBound>| note(&mut b, e.defender, "evaded"))
        .add_observer(|e: On<PerfectEvaded>, mut b: Query<&mut AnimationBound>| note(&mut b, e.defender, "perfect_evaded"))
        .add_observer(|e: On<PairStarted>, mut b: Query<&mut AnimationBound>| {
            note(&mut b, e.attacker, "pair_started");
            note(&mut b, e.victim, "pair_started");
        })
        .add_observer(|e: On<PairEnded>, mut b: Query<&mut AnimationBound>| {
            note(&mut b, e.attacker, "pair_ended");
            if let Some(victim) = e.victim {
                note(&mut b, victim, "pair_ended");
            }
        })
        .add_observer(|e: On<shards_motion::Landed>, mut b: Query<&mut AnimationBound>| note(&mut b, e.entity, "landed"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_binding_finds_its_bones_again_and_keeps_the_equip() {
        let mut state = MountState { since: Some(1.5), ..default() };
        state.bones.insert("hand".to_owned(), ("hand_r".to_owned(), Entity::PLACEHOLDER));
        state.missing.insert("back".to_owned());
        let next = state.taken_over();
        assert!(next.bones.is_empty() && next.missing.is_empty());
        assert_eq!(next.since, Some(1.5));
    }
}
