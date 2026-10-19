//! The animation bridge: glTF clips follow the action machine.
//!
//! **Gameplay leads, animation catches up.** Nothing here decides anything. Each frame, for
//! every [`Animated`] character, [`select_clip`](crate::select_clip) says which clip of its
//! [`AnimSet`] to show; this module applies it to Bevy's `AnimationPlayer`:
//!
//! - **One graph per set.** [`build_graphs`] loads the set's glTF files once (its `source`
//!   and any clip's own `source`), makes an `AnimationGraph` with a node for every clip of the
//!   library, and every character of the set shares it (clip reuse). Characters whose models
//!   have the same skeleton node names share the clips too ([`retarget_on_ready`] gives a model
//!   without animations of its own a player and the animation targets its bones need).
//! - **Binding.** [`bind_players`] finds the `AnimationPlayer` under the character and gives
//!   it the graph and an `AnimationTransitions`.
//! - **Following.** [`drive`] switches clips with a blend (`AnimationTransitions`), seeks an
//!   action's clip to the action's progress each frame (the player's own clock stopped:
//!   speed 0), lets locomotion clips loop at the speed the velocity asks for, and fires
//!   [`AnimationCue`]s as clips pass their cue times. Between two ticks it looks one step
//!   ahead (`Time<Fixed>` overstep) so the pose moves smoothly at any frame rate.
//! - **Root motion in place.** [`hold_root_motion`] puts the set's `root_motion` bone back
//!   where it rests along the ground after the clips are applied: the body already moves by
//!   the action's `displacement`, baked from that bone.
//! - **Layers.** An action the set maps to a layer (`actions: {"drink": (clip: "drink", layer:
//!   "upper")}`) plays on that part of the body only; the rest keeps its locomotion clip (see
//!   "Layers" below).
//!
//! ## Layers
//!
//! Bevy blends every clip that is playing, bone by bone, by their weights; a node's *mask* keeps
//! it off the bones of the mask groups it names. A layered action's clip gets a node of its own
//! (one per layer and clip, in [`build_graph`]) whose mask names the layer's group, and
//! [`bind_players`] fills that group of the graph with the bones **outside** the layer (the
//! layer's bone and everything below it are left out, so the clip moves only them). Locomotion
//! keeps playing on every bone, through `AnimationTransitions`, exactly as without a layer.
//!
//! A bone's pose is the weighted average of the clips that reach it, and locomotion's weights
//! always add up to 1. So on the layer's bones the action's clip with weight `a` has the share
//! `a / (1 + a)`: for a share `s` its weight is `s / (1 - s)`. [`drive`] fades `s` in and out
//! itself (`AnimationTransitions` has one "main" animation, and making the layered clip that one
//! would fade locomotion out of the legs too), over the same blend seconds, and never lets `s`
//! reach 1 ([`LAYER_FULL`]): the last 0.001% of locomotion on the arms is below what a pose can
//! show.
//!
//! The character is `Animated` plus a child with the model:
//!
//! ```ignore
//! commands.spawn((hero_bundle, Animated::new("humanoid"))).with_child(WorldAssetRoot(
//!     asset_server.load(GltfAssetLabel::Scene(0).from_asset("models/hero.glb")),
//! ));
//! ```

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::time::Duration;

use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle, AnimationNodeIndex};
use bevy::animation::transition::AnimationTransitions;
use bevy::animation::{AnimatedBy, AnimationClip, AnimationPlayer, AnimationTargetId, RepeatAnimation};
use bevy::asset::LoadState;
use bevy::ecs::query::QueryData;
use bevy::gltf::{Gltf, GltfNode};
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;
use ai_core::{AiId, PresentationHolds};
use shards_content::ContentDb;
use shards_core::Predicted;
use shards_motion::{MotionState, MoveModifiers};

use crate::anim::{
    crossed_cues, select_clip_in, select_layer_in, AnimSet, Animated, AnimationCue, BodyStatus, ClipChoice, ClipDef, LayerChoice,
    LayerDef, PropSwaps, MAX_LAYERS,
};
use crate::def::{ActionDef, MovesetDef};
use crate::fighter::{Dead, Fighter};
use crate::machine::ActionState;
use crate::predict::PredictedAction;
use crate::render::props::{self, skeleton, MountEnv, MountFrame, MountState, PropOf, RunView};

/// A seeked clip stops this far (seconds) before its end: Bevy's player wraps a time at the
/// duration back to zero, which would show the first pose in place of the last.
const END_MARGIN: f32 = 1e-3;

/// The most of a layer's bones a layered clip gets (see the module docs): the rest, 0.001%, is
/// locomotion's. A share of 1 would need an infinite weight.
const LAYER_FULL: f32 = 1.0 - 1e-5;

/// A set's graph and what is in it.
pub struct Built {
    pub graph: Handle<AnimationGraph>,
    /// Our clip name -> its node and its clip asset.
    nodes: Nodes,
    /// Layer name -> our clip name -> the node that plays the clip on that layer only.
    layered: HashMap<String, Nodes>,
}

/// Our clip name -> its node in the graph and its clip asset.
type Nodes = HashMap<String, (AnimationNodeIndex, Handle<AnimationClip>)>;

impl Built {
    /// The graph node playing clip `clip` of the set.
    pub fn node(&self, clip: &str) -> Option<AnimationNodeIndex> {
        self.nodes.get(clip).map(|(n, _)| *n)
    }

    /// The graph node playing clip `clip` on layer `layer` only: the set maps an action to
    /// that clip on that layer.
    pub fn layer_node(&self, layer: &str, clip: &str) -> Option<AnimationNodeIndex> {
        self.layered.get(layer)?.get(clip).map(|(n, _)| *n)
    }

    /// Names of the clips the graph has, sorted.
    pub fn clips(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.nodes.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }
}

/// What of a set shapes its graph: an edit of anything else (sockets, carried props, mounts,
/// speeds, blends) is read from the set in place each frame and rebuilds nothing.
#[derive(Clone, Debug, PartialEq)]
struct GraphShape {
    source: String,
    clips: BTreeMap<String, ClipDef>,
    layers: BTreeMap<String, LayerDef>,
    /// (layer, clip) of every mapping that plays on a layer.
    layered: BTreeSet<(String, String)>,
    /// A layered clip's family renames need nodes of their own.
    by_moveset: BTreeMap<String, BTreeMap<String, String>>,
}

impl GraphShape {
    fn of(set: &AnimSet) -> Self {
        Self {
            source: set.source.clone(),
            clips: set.clips.clone(),
            layers: set.layers.clone(),
            layered: set
                .actions
                .values()
                .chain(set.tags.values())
                .filter_map(|m| Some((m.layer.clone()?, m.clip.clone())))
                .collect(),
            by_moveset: set.by_moveset.clone(),
        }
    }
}

struct SetGraph {
    /// What the graph was built from: an edit of it rebuilds the graph.
    shape: GraphShape,
    built: Option<Built>,
}

/// What the bridge's own stages cost last frame (`ai/status` -> `anim.ms`).
#[derive(Resource, Default, Debug)]
pub struct AnimStats {
    /// Bodies with `Animated`, and those the bridge has bound to a player.
    pub animated: usize,
    pub bound: usize,
    /// Props drawn, and clips playing on layers.
    pub props: usize,
    pub plays: usize,
    /// Overlays started and feet solved last frame (later slices of the animation lane fill these).
    pub overlays_started: usize,
    pub feet_solved: usize,
    /// Milliseconds the bridge's own stages of all bodies took last frame.
    pub ms: f32,
}

/// The animation graphs of the animsets in use, and the glTF libraries they come from.
#[derive(Resource, Default)]
pub struct AnimGraphs {
    sets: HashMap<String, SetGraph>,
    libraries: HashMap<String, Handle<Gltf>>,
    warned: BTreeSet<String>,
}

impl AnimGraphs {
    /// The graph of set `animset`, once its glTF has loaded.
    pub fn built(&self, animset: &str) -> Option<&Built> {
        self.sets.get(animset).and_then(|s| s.built.as_ref())
    }

    /// Uses `library` as the glTF at `source` instead of loading it through the asset server
    /// (a game that preloads, a test).
    pub fn insert_library(&mut self, source: &str, library: Handle<Gltf>) {
        self.libraries.insert(source.to_owned(), library);
    }

    fn warn_once(&mut self, key: String, text: String) {
        if self.warned.insert(key) {
            warn!("{text}");
        }
    }
}

/// Marks a character whose `AnimationPlayer` has the set's graph, and remembers what the
/// bridge has it playing. Not reflected: a cache, no part of any replay.
#[derive(Component, Debug)]
pub struct AnimationBound {
    /// The entity with the `AnimationPlayer`.
    pub player: Entity,
    graph: AssetId<AnimationGraph>,
    /// The clip (our name) playing as the main one.
    clip: Option<String>,
    /// Seconds on that clip's timeline at the last frame, for cues.
    last_time: Option<f32>,
    /// The set's `root_motion` bone under the player and its rest translation, once found.
    root: Option<(Entity, Vec3)>,
    /// The set's layers whose bones the model has: only on these can an action's clip play.
    ready: BTreeSet<String>,
    /// What plays on each layer that has played something lately.
    overlays: BTreeMap<String, Overlay>,
    /// The character's animation is paused by a [`PresentationHolds`] hold (a hit stop in co-op).
    held: bool,
    /// The simulation clock when that hold began.
    held_since: Option<f64>,
    /// The props it carries and what it remembers of them.
    pub(crate) mounts: MountState,
}

impl AnimationBound {
    /// The clip the bridge has playing now.
    pub fn clip(&self) -> Option<&str> {
        self.clip.as_deref()
    }

    /// The clip (our name) of the action playing on `layer` now, once it has started to fade in
    /// and until the action ends; a clip fading out is not counted.
    pub fn layer_clip(&self, layer: &str) -> Option<&str> {
        self.overlays.get(layer)?.main.as_ref().map(|m| m.clip.as_str())
    }

    /// How much of `layer`'s bones the action's clip has, 0 to 1: the fade in and out.
    pub fn layer_share(&self, layer: &str) -> f32 {
        self.overlays.get(layer).map_or(0.0, Overlay::share)
    }

    /// The entity showing prop `prop` (a key of the set's `carry`), once it is drawn.
    pub fn prop(&self, prop: &str) -> Option<Entity> {
        self.mounts.entity(prop)
    }

    /// The props drawn now: prop id and entity, in prop id order.
    pub fn props(&self) -> impl Iterator<Item = (&str, Entity)> {
        self.mounts.drawn()
    }

    /// Where prop `prop` sat at the last frame.
    pub fn place(&self, prop: &str) -> Option<&crate::anim::Place> {
        self.mounts.places.get(prop)
    }

    /// Whether the equip of a weapon switch is open: the props of the moveset the body had before
    /// still count, and the switch's moves apply.
    pub fn equipping(&self) -> bool {
        self.mounts.since.is_some()
    }

    /// Whether a co-op picture hold has this body standing still now.
    pub fn held(&self) -> bool {
        self.held
    }

    /// The weapon-switch rules and the seconds since the switch, while its equip is open.
    pub(crate) fn mounts_equip<'a>(&self, set: &'a AnimSet, clock: f64) -> Option<(&'a crate::anim::MountRules, f32)> {
        let since = self.mounts.since(clock)?;
        let rules = set.equip_rules()?;
        crate::anim::equip_open(rules, since).then_some((rules, since))
    }

    /// The bone socket `socket` rides, once the bridge has found it in the model.
    pub fn socket_bone(&self, socket: &str) -> Option<Entity> {
        self.mounts.bones.get(socket).map(|(_, bone)| *bone)
    }
}

/// What plays on one layer: the clip of the action there (`main`), the clips fading out after
/// it, and the share of the layer's bones they have between them. The rest is locomotion's.
/// Pure bookkeeping; [`drive`] applies it to the player.
#[derive(Debug, Default)]
struct Overlay {
    main: Option<OverlayMain>,
    /// Oldest first.
    fading: Vec<OverlayFade>,
    /// Seconds on the main clip's timeline at the last frame, for cues.
    last_time: Option<f32>,
    /// The share of the layer's bones all the clips had at the last step.
    total: f32,
}

#[derive(Debug)]
struct OverlayMain {
    clip: String,
    node: AnimationNodeIndex,
    /// 0 to 1, how far it has faded in, by time alone.
    ramp: f32,
    /// How much ramp it gains a second.
    rise: f32,
    /// Its share of the layer's bones now: the ramp, or what the clips fading out leave.
    share: f32,
}

#[derive(Debug)]
struct OverlayFade {
    node: AnimationNodeIndex,
    /// 1 to 0, how much of its share it has left.
    left: f32,
    /// How much it loses a second.
    fall: f32,
}

impl Overlay {
    /// The share of the layer's bones all its clips have now.
    fn share(&self) -> f32 {
        self.total
    }

    /// Nothing plays or fades.
    fn idle(&self) -> bool {
        self.main.is_none() && self.fading.is_empty()
    }

    /// The layer shows `want` (our clip name and its node) from now on, or no clip (`None`: the
    /// action ended). What was the main clip fades out over `blend` seconds, or `snap`s out when
    /// the clock is stopped and no fade would advance. Nodes that must stop playing go to
    /// `stopped`.
    fn switch(
        &mut self,
        want: Option<(&str, AnimationNodeIndex)>,
        blend: f32,
        snap: bool,
        stopped: &mut Vec<AnimationNodeIndex>,
    ) {
        if self.main.as_ref().map(|m| m.node) == want.map(|(_, node)| node) {
            return;
        }
        if let Some(old) = self.main.take() {
            if snap {
                stopped.push(old.node);
            } else {
                self.fading.push(OverlayFade { node: old.node, left: old.share, fall: 1.0 / blend });
            }
        }
        if snap {
            stopped.extend(self.fading.drain(..).map(|f| f.node));
        }
        if let Some((clip, node)) = want {
            // Back to a clip that was fading out: it starts again, as `AnimationTransitions` does.
            self.fading.retain(|f| f.node != node);
            let ramp = if snap { 1.0 } else { 0.0 };
            self.main = Some(OverlayMain { clip: clip.to_owned(), node, ramp, rise: 1.0 / blend, share: ramp });
        }
        self.last_time = None;
    }

    /// `dt` seconds on: the weight each node plays with. The clips fading out come first, newest
    /// first, each with what is left of its share of what is not yet taken (as
    /// `AnimationTransitions` does it); the main clip gets its ramp, or what the others leave.
    /// Nodes that have faded out go to `stopped`.
    fn advance(&mut self, dt: f32, stopped: &mut Vec<AnimationNodeIndex>) -> Vec<(AnimationNodeIndex, f32)> {
        let mut shares = Vec::with_capacity(self.fading.len() + 1);
        let mut free = 1.0;
        for fade in self.fading.iter_mut().rev() {
            fade.left = (fade.left - fade.fall * dt).max(0.0);
            let share = fade.left * free;
            free -= share;
            shares.push((fade.node, share));
        }
        self.fading.retain(|f| {
            let alive = f.left > 0.0;
            if !alive {
                stopped.push(f.node);
            }
            alive
        });
        if let Some(main) = &mut self.main {
            main.ramp = (main.ramp + main.rise * dt).min(1.0);
            main.share = main.ramp.min(free);
            shares.push((main.node, main.share));
        }
        // Locomotion's weights add up to 1, so a share `s` of the layer's bones takes the weight
        // `s / (1 - s)`; the shares may not reach 1 (see `LAYER_FULL`).
        let total: f32 = shares.iter().map(|(_, s)| s).sum();
        let held = total.min(LAYER_FULL);
        self.total = held;
        let scale = if total > held { held / total } else { 1.0 };
        shares.into_iter().map(|(node, share)| (node, share * scale / (1.0 - held))).collect()
    }
}

/// What [`build_graph`] made.
pub struct GraphBuild {
    pub graph: AnimationGraph,
    nodes: Nodes,
    layered: HashMap<String, Nodes>,
    /// `(file, animation)` of the animations the set wants and its files lack.
    pub missing: Vec<(String, String)>,
}

/// The graph of `set`: a node for each clip of the library whose animation `find` supplies
/// (it gets the clip's file and the animation's name in it), in the order of the clip names;
/// then, for each clip an action or tag plays on a layer, another node of the same clip that
/// leaves out the bones outside the layer (its mask is the layer's group: the layer's position
/// among the set's layers; [`bind_players`] fills the group).
pub fn build_graph(set: &AnimSet, mut find: impl FnMut(&str, &str) -> Option<Handle<AnimationClip>>) -> GraphBuild {
    let mut graph = AnimationGraph::new();
    let mut nodes = HashMap::new();
    let mut missing = Vec::new();
    for clip in set.clips.keys() {
        let (source, name) = (set.source_of(clip), set.file_name(clip));
        match find(source, name) {
            Some(handle) => {
                let node = graph.add_clip(handle.clone(), 1.0, graph.root);
                nodes.insert(clip.clone(), (node, handle));
            }
            None => missing.push((source.to_owned(), name.to_owned())),
        }
    }
    let mut layered: HashMap<String, Nodes> = HashMap::new();
    for mapping in set.actions.values().chain(set.tags.values()) {
        let Some(layer) = mapping.layer.as_deref() else { continue };
        let Some(index) = set.layer_index(layer).filter(|i| *i < MAX_LAYERS) else { continue };
        // The clip, and the clips the families show in its place.
        let renamed = set.by_moveset.values().filter_map(|table| table.get(&mapping.clip));
        for clip in std::iter::once(&mapping.clip).chain(renamed) {
            let Some((_, handle)) = nodes.get(clip) else { continue };
            let on_layer = layered.entry(layer.to_owned()).or_default();
            if !on_layer.contains_key(clip) {
                let node = graph.add_clip_with_mask(handle.clone(), 1 << index, 1.0, graph.root);
                on_layer.insert(clip.clone(), (node, handle.clone()));
            }
        }
    }
    GraphBuild { graph, nodes, layered, missing }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<AnimGraphs>()
        .init_resource::<AnimStats>()
        .add_observer(retarget_on_ready)
        .add_plugins(props::plugin)
        .add_systems(Update, (build_graphs, bind_players, drive).chain())
        .add_systems(
            PostUpdate,
            hold_root_motion.after(bevy::app::AnimationSystems).before(bevy::transform::TransformSystems::Propagate),
        );
}

/// `Update`: loads the glTF files of every animset a character uses and builds its graph once
/// they are there. An edited animset (hot reload) is built again.
pub fn build_graphs(
    db: Option<Res<ContentDb>>,
    animated: Query<&Animated>,
    asset_server: Res<AssetServer>,
    gltfs: Res<Assets<Gltf>>,
    mut graph_assets: ResMut<Assets<AnimationGraph>>,
    mut state: ResMut<AnimGraphs>,
) {
    let Some(db) = db else { return };
    let used: BTreeSet<&str> = animated.iter().map(|a| a.animset.as_str()).collect();
    for id in used {
        let Some(set) = db.get::<AnimSet>(id) else {
            state.warn_once(
                format!("missing {id}"),
                format!("animset `{id}` is not in content; `content_list kind=animset` lists them"),
            );
            continue;
        };
        let shape = GraphShape::of(set);
        if state.sets.get(id).is_none_or(|s| s.shape != shape) {
            state.sets.insert(id.to_owned(), SetGraph { shape, built: None });
        }
        if state.sets.get(id).is_some_and(|s| s.built.is_some()) {
            continue;
        }
        let mut files: HashMap<&str, &Gltf> = HashMap::new();
        for source in set.sources() {
            let library =
                state.libraries.entry(source.to_owned()).or_insert_with(|| asset_server.load::<Gltf>(source.to_owned())).clone();
            match gltfs.get(&library) {
                Some(gltf) => {
                    files.insert(source, gltf);
                }
                None if matches!(asset_server.load_state(&library), LoadState::Failed(_)) => state.warn_once(
                    format!("load {source}"),
                    format!("animset `{id}`: cannot load `{source}`; check the path under the assets folder"),
                ),
                None => {}
            }
        }
        if files.len() < set.sources().len() {
            continue; // still loading (or failed: warned); the clips come together
        }
        let GraphBuild { graph, nodes, layered, missing } =
            build_graph(set, |source, name| files.get(source).and_then(|g| g.named_animations.get(name).cloned()));
        for source in set.sources() {
            let lacks: Vec<String> = missing.iter().filter(|(s, _)| s == source).map(|(_, m)| format!("`{m}`")).collect();
            if lacks.is_empty() {
                continue;
            }
            let mut have: Vec<&str> = files[source].named_animations.keys().map(|k| &**k).collect();
            have.sort_unstable();
            state.warn_once(
                format!("clips {id} {source} {lacks:?}"),
                format!(
                    "animset `{id}`: `{source}` has no animation {}; it has: {}",
                    lacks.join(", "),
                    if have.is_empty() { "none".to_owned() } else { have.join(", ") }
                ),
            );
        }
        let graph = graph_assets.add(graph);
        if let Some(entry) = state.sets.get_mut(id) {
            entry.built = Some(Built { graph, nodes, layered });
        }
    }
}

/// `Update`: gives the `AnimationPlayer` under each [`Animated`] character its set's graph
/// and an `AnimationTransitions`; again when the graph was rebuilt. For the set's layers it also
/// fills the graph's mask groups with the character's bones (see the module docs).
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
pub fn bind_players(
    mut commands: Commands,
    db: Option<Res<ContentDb>>,
    mut graphs: ResMut<AnimGraphs>,
    mut graph_assets: ResMut<Assets<AnimationGraph>>,
    animated: Query<(Entity, &Animated, Option<&AnimationBound>)>,
    children: Query<&Children>,
    mut players: Query<&mut AnimationPlayer>,
    names: Query<&Name>,
    targets: Query<&AnimationTargetId>,
    props: Query<(), With<PropOf>>,
) {
    for (entity, set, bound) in &animated {
        let Some(built) = graphs.built(&set.animset) else { continue };
        // The graph is the set's current one and its player is still there (a model reload
        // or a scene swap spawns the scene again, the player with it).
        if bound.is_some_and(|b| b.graph == built.graph.id() && players.contains(b.player)) {
            continue;
        }
        let found = skeleton(entity, &children, &props).into_iter().find(|e| players.contains(*e));
        let Some(player) = found else { continue };
        let graph = built.graph.clone();
        if let Ok(mut p) = players.get_mut(player) {
            p.stop_all();
        }
        let mut ready = BTreeSet::new();
        let layers = db.as_ref().and_then(|db| db.get::<AnimSet>(&set.animset)).map(|s| &s.layers);
        for (index, (layer, def)) in layers.into_iter().flatten().enumerate().take(MAX_LAYERS) {
            if mask_layer(index, &def.root, player, &graph, &mut graph_assets, &children, &names, &targets, &props) {
                ready.insert(layer.clone());
            } else {
                graphs.warn_once(
                    format!("layer {} {layer}", set.animset),
                    format!(
                        "animset `{}`: the model has no bone `{}`, where layer `{layer}` starts; its actions \
                         keep the locomotion clip",
                        set.animset, def.root
                    ),
                );
            }
        }
        commands.entity(player).insert((AnimationGraphHandle(graph.clone()), AnimationTransitions::new()));
        commands.entity(entity).insert(AnimationBound {
            player,
            graph: graph.id(),
            clip: None,
            last_time: None,
            root: None,
            ready,
            overlays: BTreeMap::new(),
            held: false,
            held_since: None,
            // The props and the equip go over to the new binding: a rebind neither forgets an
            // equip nor leaves a second set of props under the bones.
            mounts: bound.map(|b| b.mounts.taken_over()).unwrap_or_default(),
        });
    }
}

/// Puts the bones under `player` that are not the bone named `root` or below it into mask group
/// `index` of the graph, so that a node masked out of that group moves only the layer. False if
/// there is no such bone. Touches the graph only if a bone is not in the group yet.
#[allow(clippy::too_many_arguments, reason = "the queries a walk of the skeleton needs")]
fn mask_layer(
    index: usize,
    root: &str,
    player: Entity,
    graph: &Handle<AnimationGraph>,
    graph_assets: &mut Assets<AnimationGraph>,
    children: &Query<&Children>,
    names: &Query<&Name>,
    targets: &Query<&AnimationTargetId>,
    props: &Query<(), With<PropOf>>,
) -> bool {
    let bones: Vec<Entity> = skeleton(player, children, props);
    let Some(top) = bones.iter().copied().find(|e| names.get(*e).is_ok_and(|n| n.as_str() == root)) else {
        return false;
    };
    let inside: HashSet<Entity> = skeleton(top, children, props).into_iter().collect();
    let outside: Vec<AnimationTargetId> =
        bones.iter().filter(|e| !inside.contains(*e)).filter_map(|e| targets.get(*e).ok().copied()).collect();
    let bit = 1u64 << index;
    let missing = graph_assets
        .get(graph)
        .is_some_and(|g| outside.iter().any(|t| g.mask_groups.get(t).is_none_or(|mask| mask & bit == 0)));
    if missing && let Some(mut g) = graph_assets.get_mut(graph) {
        for target in outside {
            g.add_target_to_mask_group(target, index as u32);
        }
    }
    true
}

/// What [`drive`] reads of a character.
#[derive(QueryData)]
#[query_data(mutable)]
pub struct Driven {
    entity: Entity,
    animated: &'static Animated,
    bound: &'static mut AnimationBound,
    state: Option<&'static ActionState>,
    /// A client's own body shows the action it predicts, not the host's older state.
    shown: Option<&'static PredictedAction>,
    predicted: Has<Predicted>,
    /// For [`PresentationHolds`].
    id: Option<&'static AiId>,
    motion: Option<&'static MotionState>,
    modifiers: Option<&'static MoveModifiers>,
    fighter: Option<&'static Fighter>,
    dead: Has<Dead>,
    swaps: Option<&'static PropSwaps>,
}

/// `Update`: the clip each character shows, applied to its player. See the module docs.
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
pub fn drive(
    mut commands: Commands,
    time: Res<Time>,
    real: Res<Time<Real>>,
    holds: Option<Res<PresentationHolds>>,
    fixed: Res<Time<Fixed>>,
    db: Option<Res<ContentDb>>,
    graphs: Res<AnimGraphs>,
    clips: Res<Assets<AnimationClip>>,
    mut animated: Query<Driven>,
    mut players: Query<(&mut AnimationPlayer, &mut AnimationTransitions)>,
    mut mounting: MountEnv,
    all: Query<(), With<Animated>>,
    stats: Option<ResMut<AnimStats>>,
) {
    let Some(db) = db else { return };
    let overstep = fixed.overstep().as_secs_f32();
    // The simulation's clock, between ticks too: a pause freezes the props' clocks, a `step` moves them.
    let clock = fixed.elapsed_secs_f64() + f64::from(overstep);
    let mut spent = std::time::Duration::ZERO;
    let (mut bound_bodies, mut props_drawn, mut plays) = (0usize, 0usize, 0usize);
    let now = real.elapsed();
    let frozen = time.delta_secs() <= 0.0;
    let standing = MotionState::default();
    for DrivenItem { entity, animated: set_ref, mut bound, state, shown, predicted, id, motion, modifiers, fighter, dead, swaps } in
        &mut animated
    {
        let (Some(set), Some(built)) = (db.get::<AnimSet>(&set_ref.animset), graphs.built(&set_ref.animset)) else {
            continue;
        };
        if bound.graph != built.graph.id() {
            continue; // `bind_players` takes it over to the rebuilt graph next
        }
        let Ok((mut player, mut transitions)) = players.get_mut(bound.player) else { continue };

        // A hit stop in co-op: this body's picture stands still (the clock runs on). Its player
        // is paused as a whole, and nothing is sought or switched until the hold is over.
        let held = holds.as_ref().zip(id).is_some_and(|(holds, id)| holds.is_held(*id, now));
        if held != bound.held {
            bound.held = held;
            if held {
                player.pause_all();
                bound.held_since = Some(clock);
            } else {
                player.resume_all();
                // The clocks of the props resume where they stopped.
                if let Some(since) = bound.held_since.take() {
                    bound.mounts.shift(clock - since);
                }
            }
        }
        if held {
            continue;
        }

        // The running action as (key, seconds in, length). One that has a hold loop (a raised
        // block) is not looked ahead: it would run past the loop's end.
        let run = if predicted { shown.map(|p| &p.run) } else { state.and_then(|s| s.current.as_ref()) };
        let def = run.and_then(|r| db.get::<ActionDef>(&r.action));
        let action = run.map(|r| {
            let key = set.action_key(&r.action, r.anim.as_deref(), def.map_or(&[], |d| d.tags.as_slice()));
            let ahead = if def.is_some_and(|d| d.hold.is_some()) { 0.0 } else { overstep };
            (key, (r.t + ahead).min(r.length), r.length)
        });
        let staggers = fighter
            .and_then(|f| db.get::<MovesetDef>(&f.moveset))
            .and_then(|m| m.stagger.as_deref())
            .zip(run)
            .is_some_and(|(stagger, r)| r.forced && r.action == stagger);
        let status = BodyStatus { dead, staggered: staggers, pace: modifiers.map_or(1.0, MoveModifiers::pace) };
        let moveset = fighter.map(|f| f.moveset.as_str());
        let choice = select_clip_in(action, motion.unwrap_or(&standing), status, set, moveset);
        drive_body(&mut commands, entity, set, built, &clips, &mut bound, &mut player, &mut transitions, &choice, frozen);
        if !built.layered.is_empty() || !bound.overlays.is_empty() {
            let frame = LayerFrame {
                wanted: select_layer_in(action, status, set, moveset),
                back_blend: choice.blend,
                dt: time.delta_secs(),
                frozen,
            };
            drive_layers(&mut commands, entity, set, built, &clips, &mut bound, &mut player, &frame);
        }

        // The props: where they sit now.
        bound_bodies += 1;
        let player_entity = bound.player;
        let frame = MountFrame {
            body: entity,
            player: player_entity,
            animset: &set_ref.animset,
            set,
            db: &db,
            moveset,
            swaps,
            run: run.zip(action).map(|(run, (key, t, _))| RunView { run, def, key, t }),
            clock,
        };
        let began = std::time::Instant::now();
        props::step(&mut mounting, &frame, &mut bound.mounts);
        spent += began.elapsed();
        props_drawn += bound.mounts.drawn().count();
        plays += bound.overlays.values().filter(|o| o.main.is_some()).count();
    }
    if let Some(mut stats) = stats {
        stats.animated = all.iter().count();
        stats.bound = bound_bodies;
        stats.props = props_drawn;
        stats.plays = plays;
        stats.ms = if bound_bodies > 0 { spent.as_secs_f32() * 1000.0 } else { 0.0 };
    }
}

/// The clip that shows on the whole body: locomotion, or an action that has no layer. Switches
/// clips with a blend, seeks the action's clip, fires the cues.
#[allow(clippy::too_many_arguments, reason = "what one character's step reads and writes")]
fn drive_body(
    commands: &mut Commands,
    entity: Entity,
    set: &AnimSet,
    built: &Built,
    clips: &Assets<AnimationClip>,
    bound: &mut AnimationBound,
    player: &mut AnimationPlayer,
    transitions: &mut AnimationTransitions,
    choice: &ClipChoice,
    frozen: bool,
) {
    let Some(clip) = choice.clip else { return };
    let Some((node, handle)) = built.nodes.get(clip) else { return };
    let duration = clips.get(handle).map_or(0.0, |c| c.duration());

    if bound.clip.as_deref() != Some(clip) {
        if bound.clip.is_none() || frozen {
            // Nothing to blend from, or the clock is stopped (a paused simulation, an agent
            // stepping tick by tick): a fade would never advance and leave the old pose
            // standing, so the new clip snaps in.
            player.stop_all();
            *transitions = AnimationTransitions::new();
            transitions.play(player, *node, Duration::ZERO);
        } else {
            // Not zero (Bevy's fade divides by it), not absurd (a typo in content must not
            // overflow a `Duration`).
            let blend = choice.blend.clamp(0.01, 10.0);
            transitions.play(player, *node, Duration::from_secs_f32(blend));
        }
        bound.clip = Some(clip.to_owned());
        bound.last_time = None;
    }
    let Some(active) = player.animation_mut(*node) else { return };
    active.set_repeat(if choice.looping { RepeatAnimation::Forever } else { RepeatAnimation::Never });
    let now = match choice.seek_to {
        Some(progress) => {
            // Gameplay moves it: the player's own clock stands still. Not quite on the
            // end: the player wraps a time at the clip's duration back to its start.
            let at = (progress * duration).min((duration - END_MARGIN).max(0.0));
            active.set_speed(0.0).set_seek_time(at);
            at
        }
        None => {
            active.set_speed(choice.speed);
            active.seek_time()
        }
    };

    let cues = set.clip(clip).map_or(&[][..], |c| c.cues.as_slice());
    if !cues.is_empty() {
        let wrap = (choice.looping && duration > 0.0).then_some(duration);
        for cue in crossed_cues(cues, bound.last_time, now, wrap) {
            commands.trigger(AnimationCue { entity, cue: cue.cue.clone(), clip: clip.to_owned() });
        }
    }
    bound.last_time = Some(now);
}

/// What one frame asks of the layers of a character.
struct LayerFrame<'a> {
    /// The clip an action plays on a layer now, if any.
    wanted: Option<LayerChoice<'a>>,
    /// Seconds to blend back to locomotion when the action ends (the locomotion clip's blend).
    back_blend: f32,
    /// Seconds since the last frame.
    dt: f32,
    /// The clock is stopped: fades would never advance, so clips snap in and out.
    frozen: bool,
}

/// The clips on the layers: each layer's action clip fades in over its own blend and out over
/// the locomotion clip's, seeked to the action's progress, while [`drive_body`] keeps
/// locomotion going on every bone. See the module docs for the weights.
#[allow(clippy::too_many_arguments, reason = "what one character's step reads and writes")]
fn drive_layers(
    commands: &mut Commands,
    entity: Entity,
    set: &AnimSet,
    built: &Built,
    clips: &Assets<AnimationClip>,
    bound: &mut AnimationBound,
    player: &mut AnimationPlayer,
    frame: &LayerFrame,
) {
    // Only on a layer the model has the bones for.
    let wanted = frame.wanted.filter(|l| bound.ready.contains(l.layer));
    if let Some(layer) = wanted {
        bound.overlays.entry(layer.layer.to_owned()).or_default();
    }
    for (name, overlay) in &mut bound.overlays {
        let want = wanted.filter(|l| l.layer == name).and_then(|l| {
            let clip = l.choice.clip?;
            let (node, handle) = built.layered.get(name.as_str())?.get(clip)?;
            Some((l.choice, clip, *node, handle))
        });
        let blend = want.map_or(frame.back_blend, |(choice, ..)| choice.blend).clamp(0.01, 10.0);
        let mut stopped = Vec::new();
        overlay.switch(want.map(|(_, clip, node, _)| (clip, node)), blend, frame.frozen, &mut stopped);
        let weights = overlay.advance(frame.dt, &mut stopped);
        for node in stopped {
            player.stop(node);
        }
        if let Some((choice, clip, node, handle)) = want {
            // Gameplay moves it, as it moves an action's clip on the whole body.
            let duration = clips.get(handle).map_or(0.0, |c| c.duration());
            let at = (choice.seek_to.unwrap_or(0.0) * duration).min((duration - END_MARGIN).max(0.0));
            player.play(node).set_repeat(RepeatAnimation::Never).set_speed(0.0).set_seek_time(at);
            let cues = set.clip(clip).map_or(&[][..], |c| c.cues.as_slice());
            if !cues.is_empty() {
                for cue in crossed_cues(cues, overlay.last_time, at, None) {
                    commands.trigger(AnimationCue { entity, cue: cue.cue.clone(), clip: clip.to_owned() });
                }
            }
            overlay.last_time = Some(at);
        }
        for (node, weight) in weights {
            if let Some(active) = player.animation_mut(node) {
                active.set_weight(weight);
            }
        }
    }
    bound.overlays.retain(|_, overlay| !overlay.idle());
}

/// `PostUpdate`, after the clips are applied and before transforms propagate: the set's
/// `root_motion` bone back at its rest translation along the ground (its height stays, a roll
/// still bobs). The body moves by the action's `displacement`, baked from this very bone, so
/// the model would otherwise run ahead of its body. The rest comes from the set's glTF.
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
pub fn hold_root_motion(
    db: Option<Res<ContentDb>>,
    graphs: Res<AnimGraphs>,
    gltfs: Res<Assets<Gltf>>,
    gltf_nodes: Option<Res<Assets<GltfNode>>>,
    mut animated: Query<(&Animated, &mut AnimationBound)>,
    children: Query<&Children>,
    names: Query<&Name>,
    props: Query<(), With<PropOf>>,
    mut transforms: Query<&mut Transform>,
) {
    let Some(db) = db else { return };
    for (set_ref, mut bound) in &mut animated {
        let Some(bone) = db.get::<AnimSet>(&set_ref.animset).and_then(|s| s.root_motion.as_deref().map(|b| (s, b))) else {
            continue;
        };
        let (set, bone) = bone;
        if bound.root.is_none_or(|(e, _)| !transforms.contains(e)) {
            let found = skeleton(bound.player, &children, &props)
                .into_iter()
                .find(|e| names.get(*e).is_ok_and(|n| n.as_str() == bone));
            let rest = graphs
                .libraries
                .get(&set.source)
                .and_then(|lib| gltfs.get(lib))
                .and_then(|gltf| gltf.named_nodes.get(bone))
                .and_then(|node| gltf_nodes.as_ref()?.get(node))
                .map_or(Vec3::ZERO, |node| node.transform.translation);
            bound.root = found.map(|e| (e, rest));
        }
        let Some((entity, rest)) = bound.root else { continue };
        if let Ok(mut transform) = transforms.get_mut(entity) {
            transform.translation.x = rest.x;
            transform.translation.z = rest.z;
        }
    }
}

/// A model of an [`Animated`] character that brings no animations of its own (the clips are in
/// the set's library file) cannot be animated by Bevy as it is: there is no `AnimationPlayer`
/// and its bones have no animation targets. This gives them both, the way the glTF loader
/// would have: the player on the skeleton's root (the top-level node of the glTF scene with
/// the most nodes below it), and every named node below it the target id of its path of names
/// from that root. Clips of another file then find their bones if the **names match**.
#[allow(clippy::too_many_arguments, reason = "an observer: one parameter per thing it reads")]
pub fn retarget_on_ready(
    ready: On<WorldInstanceReady>,
    mut commands: Commands,
    parents: Query<&ChildOf>,
    animated: Query<(), With<Animated>>,
    props: Query<(), With<PropOf>>,
    children: Query<&Children>,
    players: Query<(), With<AnimationPlayer>>,
    names: Query<&Name>,
) {
    let root = ready.entity;
    // A prop's model is not a rig: it gets no player and no animation targets.
    if std::iter::once(root).chain(parents.iter_ancestors(root)).any(|e| props.contains(e)) {
        return;
    }
    if !std::iter::once(root).chain(parents.iter_ancestors(root)).any(|e| animated.contains(e)) {
        return;
    }
    if skeleton(root, &children, &props).into_iter().any(|e| players.contains(e)) {
        return;
    }
    // A glTF scene spawns as `root > SceneN > top-level nodes`: the nodes are what the
    // animations of a glTF file treat as their roots.
    let Some(skeleton) = children
        .get(root)
        .into_iter()
        .flat_map(|scenes| scenes.iter())
        .flat_map(|scene| children.get(scene).into_iter().flat_map(|nodes| nodes.iter()))
        .filter(|e| names.contains(*e))
        .max_by_key(|e| children.iter_descendants(*e).count())
    else {
        return;
    };
    commands.entity(skeleton).insert(AnimationPlayer::default());
    let mut stack: Vec<(Entity, Vec<Name>)> = vec![(skeleton, Vec::new())];
    while let Some((entity, mut path)) = stack.pop() {
        let Ok(name) = names.get(entity) else { continue };
        path.push(name.clone());
        commands.entity(entity).insert((AnimationTargetId::from_names(path.iter()), AnimatedBy(skeleton)));
        if let Ok(kids) = children.get(entity) {
            stack.extend(kids.iter().map(|k| (k, path.clone())));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::animation::animation_curves::{AnimatableCurve, AnimatableKeyframeCurve};
    use bevy::animation::{animated_field, AnimationPlugin};
    use bevy::time::TimeUpdateStrategy;
    use bevy::transform::TransformPlugin;
    use shards_content::db::Source;
    use shards_content::ContentKinds;
    use shards_motion::MotionMode;

    use crate::anim::ClipDef;
    use crate::machine::Running;

    const ACTIONS: &str = r#"{
        "light_1": (net_id: 1, phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)], anim: "swing_1"),
        "roll": (net_id: 2, phases: [("roll", 0.4)]),
        "stagger": (net_id: 3, phases: [("stagger", 0.8)]),
        "drink": (net_id: 4, phases: [("drink", 1.0)]),
        "guard": (net_id: 5, phases: [("guard", 1.0)]),
    }"#;
    const MOVESETS: &str = r#"{ "hero": (inputs: {"light": "light_1", "dodge": "roll"}, stagger: "stagger") }"#;
    const ANIMSETS: &str = r#"{ "humanoid": (
        source: "models/humanoid.glb",
        clips: {
            "idle": (cues: [(at: 0.5, cue: "breath")]), "walk": (), "run": (),
            "swing_1": (name: "Armature|Slash", cues: [(at: 0.5, cue: "swoosh")]),
            "hit": (),
        },
        locomotion: {"idle": "idle", "walk": "walk", "run": "run", "stagger": "hit"},
        actions: {"roll": "idle"},
    ) }"#;

    fn db() -> ContentDb {
        db_from(ANIMSETS)
    }

    fn db_from(animsets: &str) -> ContentDb {
        let mut kinds = ContentKinds::default();
        kinds.register::<ActionDef>();
        kinds.register::<MovesetDef>();
        kinds.register::<AnimSet>();
        kinds.register::<crate::status::StatusDef>();
        let sources = [
            Source { path: "content/a.action.ron".into(), text: ACTIONS.into() },
            Source { path: "content/m.moveset.ron".into(), text: MOVESETS.into() },
            Source { path: "content/h.animset.ron".into(), text: animsets.into() },
        ];
        let db = ContentDb::build(&kinds, &sources);
        assert_eq!(db.errors().count(), 0, "{:?}", db.errors().map(ToString::to_string).collect::<Vec<_>>());
        db
    }

    fn clip(seconds: f32) -> AnimationClip {
        let mut clip = AnimationClip::default();
        clip.set_duration(seconds);
        clip
    }

    /// An app with the animation machinery but no window: clips as assets, the library glTF
    /// inserted by hand, a character with a fake player below it.
    struct Rig {
        app: App,
        character: Entity,
        player: Entity,
        nodes: HashMap<&'static str, AnimationNodeIndex>,
    }

    fn rig() -> Rig {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), AnimationPlugin, TransformPlugin))
            // Frames take no time, so the test moves the clock itself where it matters.
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO))
            .init_asset::<Gltf>()
            .insert_resource(db())
            .init_resource::<AnimGraphs>()
            .add_systems(Update, (build_graphs, bind_players, drive).chain());
        // The library: five clips, the swing one a second long.
        let (mut named, mut handles) = (HashMap::new(), Vec::new());
        {
            let mut assets = app.world_mut().resource_mut::<Assets<AnimationClip>>();
            for (name, secs) in [("idle", 2.0), ("walk", 1.0), ("run", 0.8), ("Armature|Slash", 1.0), ("hit", 0.5)] {
                let handle = assets.add(clip(secs));
                named.insert(Box::<str>::from(name), handle.clone());
                handles.push(handle);
            }
        }
        let gltf = Gltf {
            scenes: Vec::new(),
            named_scenes: default(),
            meshes: Vec::new(),
            named_meshes: default(),
            materials: Vec::new(),
            named_materials: default(),
            nodes: Vec::new(),
            named_nodes: default(),
            skins: Vec::new(),
            named_skins: default(),
            default_scene: None,
            animations: handles,
            named_animations: named.into_iter().collect(),
            source: None,
        };
        let library = app.world_mut().resource_mut::<Assets<Gltf>>().add(gltf);
        app.world_mut().resource_mut::<AnimGraphs>().insert_library("models/humanoid.glb", library);
        let player = app.world_mut().spawn((AnimationPlayer::default(), Name::new("Armature"))).id();
        let character = app
            .world_mut()
            .spawn((
                Animated::new("humanoid"),
                Fighter::new("hero"),
                ActionState::default(),
                MotionState { mode: MotionMode::Ground, ..default() },
            ))
            .add_child(player)
            .id();
        app.update();
        let nodes = ["idle", "walk", "run", "swing_1", "hit"]
            .into_iter()
            .map(|c| (c, app.world().resource::<AnimGraphs>().built("humanoid").unwrap().node(c).unwrap()))
            .collect();
        Rig { app, character, player, nodes }
    }

    impl Rig {
        fn playing(&self) -> Vec<(&'static str, f32, f32, f32)> {
            let player = self.app.world().get::<AnimationPlayer>(self.player).unwrap();
            let mut out: Vec<_> = self
                .nodes
                .iter()
                .filter_map(|(name, node)| {
                    player.animation(*node).map(|a| (*name, a.seek_time(), a.speed(), a.weight()))
                })
                .collect();
            out.sort_by(|a, b| a.0.cmp(b.0));
            out
        }

        fn main_clip(&self) -> Option<String> {
            self.app.world().get::<AnimationBound>(self.character).and_then(|b| b.clip().map(str::to_owned))
        }

        fn run_action(&mut self, id: &str, t: f32, anim: Option<&str>) {
            let def = db().get::<ActionDef>(id).cloned().unwrap();
            let mut run = Running::new(id, &def, 0, Vec3::NEG_Z, None, Some("light".into()));
            run.t = t;
            run.anim = anim.map(str::to_owned);
            self.app.world_mut().get_mut::<ActionState>(self.character).unwrap().current = Some(run);
        }

        fn velocity(&mut self, v: f32) {
            self.app.world_mut().get_mut::<MotionState>(self.character).unwrap().velocity = Vec3::new(0.0, 0.0, -v);
        }
    }

    #[test]
    fn one_graph_per_set_with_a_node_per_clip() {
        let rig = rig();
        let built = rig.app.world().resource::<AnimGraphs>().built("humanoid").unwrap();
        assert_eq!(built.clips(), ["hit", "idle", "run", "swing_1", "walk"]);
        let handle = rig.app.world().get::<AnimationGraphHandle>(rig.player).unwrap();
        assert_eq!(handle.id(), built.graph.id(), "the player uses the set's graph");
        assert!(rig.app.world().get::<AnimationTransitions>(rig.player).is_some());
        let bound = rig.app.world().get::<AnimationBound>(rig.character).unwrap();
        assert_eq!(bound.player, rig.player);
    }

    #[test]
    fn characters_of_a_set_share_its_graph() {
        let mut rig = rig();
        let other_player = rig.app.world_mut().spawn(AnimationPlayer::default()).id();
        rig.app.world_mut().spawn((Animated::new("humanoid"), ActionState::default())).add_child(other_player);
        rig.app.update();
        let a = rig.app.world().get::<AnimationGraphHandle>(rig.player).unwrap().id();
        let b = rig.app.world().get::<AnimationGraphHandle>(other_player).unwrap().id();
        assert_eq!(a, b);
        assert_eq!(rig.app.world().resource::<Assets<AnimationGraph>>().len(), 1);
    }

    #[test]
    fn a_standing_character_idles_and_one_that_moves_blends_to_walk() {
        let mut rig = rig();
        rig.app.update();
        assert_eq!(rig.main_clip().as_deref(), Some("idle"));
        let idle = rig.playing();
        assert_eq!(idle.len(), 1, "{idle:?}");
        assert!((idle[0].3 - 1.0).abs() < 1e-5, "full weight, no fade-in: {idle:?}");

        // The clock runs: idle fades out while walk fades in.
        rig.app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(10)));
        rig.app.update();
        rig.velocity(2.0);
        rig.app.update();
        assert_eq!(rig.main_clip().as_deref(), Some("walk"));
        let blending = rig.playing();
        let names: Vec<&str> = blending.iter().map(|p| p.0).collect();
        assert_eq!(names, ["idle", "walk"], "idle fades out while walk fades in: {blending:?}");
        let walk = blending.iter().find(|p| p.0 == "walk").unwrap();
        assert!((walk.2 - 1.0).abs() < 1e-4, "2 m/s on a 2 m/s clip: speed {}", walk.2);

        // Faster: a run, at a pace that follows the velocity.
        rig.velocity(6.0);
        rig.app.update();
        assert_eq!(rig.main_clip().as_deref(), Some("run"));
        let run = rig.playing().into_iter().find(|p| p.0 == "run").unwrap();
        assert!((run.2 - 1.2).abs() < 1e-4, "{run:?}");
    }

    #[test]
    fn with_the_clock_stopped_a_new_clip_snaps_in() {
        // A paused simulation (an agent stepping tick by tick) stops Bevy's clock, and a fade
        // that cannot advance would leave the old pose standing.
        let mut rig = rig();
        rig.app.update();
        rig.velocity(2.0);
        rig.app.update();
        assert_eq!(rig.main_clip().as_deref(), Some("walk"));
        let playing = rig.playing();
        assert_eq!(playing.len(), 1, "no idle left fading: {playing:?}");
        assert!((playing[0].3 - 1.0).abs() < 1e-5, "{playing:?}");
        rig.run_action("light_1", 0.3, Some("swing_1"));
        rig.app.update();
        let playing = rig.playing();
        assert_eq!((playing.len(), playing[0].0), (1, "swing_1"), "{playing:?}");
        assert!((playing[0].3 - 1.0).abs() < 1e-5, "{playing:?}");
    }

    #[test]
    fn an_action_clip_is_seeked_to_gameplay_time() {
        let mut rig = rig();
        rig.app.update();
        // light_1 maps to `swing_1` through the action's `anim`; 0.3 of 0.6 s in, a 1 s clip.
        rig.run_action("light_1", 0.3, Some("swing_1"));
        rig.app.update();
        assert_eq!(rig.main_clip().as_deref(), Some("swing_1"));
        let swing = rig.playing().into_iter().find(|p| p.0 == "swing_1").unwrap();
        assert!((swing.1 - 0.5).abs() < 1e-3, "seek {swing:?}");
        assert_eq!(swing.2, 0.0, "the player's clock stands still, gameplay moves the clip");

        // Gameplay moves on: so does the clip, whatever the frame time.
        rig.run_action("light_1", 0.45, Some("swing_1"));
        rig.app.update();
        let swing = rig.playing().into_iter().find(|p| p.0 == "swing_1").unwrap();
        assert!((swing.1 - 0.75).abs() < 1e-3, "{swing:?}");

        // The action ends: back to locomotion.
        rig.app.world_mut().get_mut::<ActionState>(rig.character).unwrap().current = None;
        rig.app.update();
        assert_eq!(rig.main_clip().as_deref(), Some("idle"));
    }

    #[test]
    fn clips_blend_over_the_blend_time_of_the_set() {
        let mut rig = rig();
        rig.app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(100)));
        rig.app.update();
        rig.velocity(2.0);
        rig.app.update();
        // 0.1 s into a 0.2 s blend: half and half.
        let weights: Vec<(&str, f32)> = rig.playing().iter().map(|p| (p.0, p.3)).collect();
        assert_eq!(weights.len(), 2, "{weights:?}");
        for (_, w) in &weights {
            assert!((w - 0.5).abs() < 0.05, "{weights:?}");
        }
        rig.app.update();
        rig.app.update();
        let weights: Vec<(&str, f32)> = rig.playing().iter().map(|p| (p.0, p.3)).collect();
        assert_eq!(weights.len(), 1, "idle faded out and stopped: {weights:?}");
        assert_eq!(weights[0].0, "walk");
        assert!((weights[0].1 - 1.0).abs() < 1e-3);
        // An action's clip comes in quicker (0.06 s): one 0.1 s frame and it has all the weight.
        rig.run_action("light_1", 0.0, Some("swing_1"));
        rig.app.update();
        let swing = rig.playing().into_iter().find(|p| p.0 == "swing_1").unwrap();
        assert!((swing.3 - 1.0).abs() < 1e-3, "{swing:?}");
    }

    #[test]
    fn a_body_in_a_picture_hold_stands_still_and_moves_on_when_it_ends() {
        let mut rig = rig();
        rig.app.init_resource::<PresentationHolds>().insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(100)));
        rig.app.world_mut().entity_mut(rig.character).insert(AiId(7));
        rig.velocity(1.5);
        for _ in 0..3 {
            rig.app.update();
        }
        let walk = |rig: &Rig| rig.playing().into_iter().find(|p| p.0 == "walk").unwrap();
        assert_eq!(rig.main_clip().as_deref(), Some("walk"));
        let moving = walk(&rig).1;
        rig.app.update();
        assert!(walk(&rig).1 > moving, "it walks");

        // Held for 0.25 s of real time: frames of 0.1 s.
        let until = rig.app.world().resource::<Time<Real>>().elapsed() + Duration::from_millis(250);
        rig.app.world_mut().resource_mut::<PresentationHolds>().hold(AiId(7), until - Duration::from_millis(250), until);
        rig.app.update();
        let held_at = walk(&rig).1;
        assert!(rig.app.world().get::<AnimationPlayer>(rig.player).unwrap().all_paused());
        // Even the clip it should switch to waits (the body stops: idle is not shown while held).
        rig.velocity(0.0);
        rig.app.update();
        assert_eq!((walk(&rig).1, rig.main_clip().as_deref()), (held_at, Some("walk")), "nothing moves or switches");
        // The hold is over after 0.25 s: the player runs again and the choice catches up.
        for _ in 0..3 {
            rig.app.update();
        }
        assert!(!rig.app.world().get::<AnimationPlayer>(rig.player).unwrap().all_paused());
        assert_eq!(rig.main_clip().as_deref(), Some("idle"));
        // Another body's hold does not touch this one.
        let until = rig.app.world().resource::<Time<Real>>().elapsed() + Duration::from_secs(5);
        rig.app.world_mut().resource_mut::<PresentationHolds>().hold(AiId(9), until - Duration::from_secs(5), until);
        rig.app.update();
        assert!(!rig.app.world().get::<AnimationPlayer>(rig.player).unwrap().all_paused());
    }

    #[test]
    fn between_ticks_the_clip_looks_one_step_ahead() {
        let mut rig = rig();
        rig.run_action("light_1", 0.3, Some("swing_1"));
        // 10 ms into the next tick: the pose is where gameplay is about to be.
        rig.app.world_mut().resource_mut::<Time<Fixed>>().accumulate_overstep(Duration::from_millis(10));
        rig.app.update();
        let swing = rig.playing().into_iter().find(|p| p.0 == "swing_1").unwrap();
        assert!((swing.1 - (0.3 + 0.01) / 0.6).abs() < 1e-3, "{swing:?}");
        // Never past the end of the action; and a hair before the clip's end, where Bevy's
        // player would wrap the time round to the first pose.
        rig.run_action("light_1", 0.595, Some("swing_1"));
        rig.app.update();
        let swing = rig.playing().into_iter().find(|p| p.0 == "swing_1").unwrap();
        assert!((swing.1 - (1.0 - END_MARGIN)).abs() < 1e-4, "{swing:?}");
        rig.app.update();
        let swing = rig.playing().into_iter().find(|p| p.0 == "swing_1").unwrap();
        assert!(swing.1 > 0.99, "it stays on the last pose: {swing:?}");
    }

    #[test]
    fn the_same_action_again_restarts_the_clip_from_gameplay_time() {
        let mut rig = rig();
        rig.run_action("light_1", 0.55, Some("swing_1"));
        rig.app.update();
        rig.run_action("light_1", 0.0, Some("swing_1"));
        rig.app.update();
        let swing = rig.playing().into_iter().find(|p| p.0 == "swing_1").unwrap();
        assert!(swing.1 < 0.01, "a new swing starts at the beginning: {swing:?}");
    }

    #[test]
    fn a_forced_stagger_shows_the_stagger_clip() {
        let mut rig = rig();
        let def = db().get::<ActionDef>("stagger").cloned().unwrap();
        let run = Running::new("stagger", &def, 0, Vec3::NEG_Z, None, None);
        rig.app.world_mut().get_mut::<ActionState>(rig.character).unwrap().current = Some(run);
        rig.app.update();
        assert_eq!(rig.main_clip().as_deref(), Some("hit"));
        // The same unmapped action pressed by the player is no stagger: it idles.
        let mut pressed = Running::new("stagger", &def, 0, Vec3::NEG_Z, None, Some("light".into()));
        pressed.t = 0.1;
        rig.app.world_mut().get_mut::<ActionState>(rig.character).unwrap().current = Some(pressed);
        rig.app.update();
        assert_eq!(rig.main_clip().as_deref(), Some("idle"));
    }

    #[test]
    fn cues_fire_as_the_clip_passes_them() {
        #[derive(Resource, Default)]
        struct Heard(Vec<AnimationCue>);
        let mut rig = rig();
        rig.app.init_resource::<Heard>();
        rig.app.add_observer(|cue: On<AnimationCue>, mut heard: ResMut<Heard>| heard.0.push(cue.event().clone()));
        rig.run_action("light_1", 0.1, Some("swing_1")); // 1/6 in: the cue is at 0.5 s of the 1 s clip
        rig.app.update();
        assert!(rig.app.world().resource::<Heard>().0.is_empty());
        rig.run_action("light_1", 0.35, Some("swing_1"));
        rig.app.update();
        let heard = &rig.app.world().resource::<Heard>().0;
        assert_eq!(heard.len(), 1, "{heard:?}");
        assert_eq!((heard[0].entity, heard[0].cue.as_str(), heard[0].clip.as_str()), (rig.character, "swoosh", "swing_1"));
        rig.run_action("light_1", 0.5, Some("swing_1"));
        rig.app.update();
        assert_eq!(rig.app.world().resource::<Heard>().0.len(), 1, "once");
    }

    #[test]
    fn a_looping_clip_fires_its_cues_on_every_pass() {
        #[derive(Resource, Default)]
        struct Heard(Vec<AnimationCue>);
        let mut rig = rig();
        rig.app.init_resource::<Heard>();
        rig.app.add_observer(|cue: On<AnimationCue>, mut heard: ResMut<Heard>| heard.0.push(cue.event().clone()));
        // The idle clip is 2 s long with a cue at 0.5 s; frames of 0.25 s.
        rig.app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(250)));
        let breaths = |rig: &Rig| rig.app.world().resource::<Heard>().0.iter().filter(|c| c.cue == "breath").count();
        for _ in 0..2 {
            rig.app.update();
        }
        assert_eq!(breaths(&rig), 0, "0.25 s in");
        for _ in 0..2 {
            rig.app.update();
        }
        assert_eq!(breaths(&rig), 1, "passed 0.5 s");
        for _ in 0..4 {
            rig.app.update();
        }
        assert_eq!(breaths(&rig), 1, "the clip wrapped round only just");
        for _ in 0..4 {
            rig.app.update();
        }
        assert_eq!(breaths(&rig), 2, "and again on the next pass");
    }

    #[test]
    fn an_edited_animset_rebuilds_the_graph_and_rebinds() {
        let mut rig = rig();
        let before = rig.app.world().resource::<AnimGraphs>().built("humanoid").unwrap().graph.id();
        // Hot reload: the walk clip is gone from the set.
        let edited = ANIMSETS.replace(r#""walk": (), "#, "").replace(r#""walk": "walk", "#, "");
        assert_ne!(edited, ANIMSETS);
        rig.app.insert_resource(db_from(&edited));
        rig.app.update();
        rig.app.update();
        let built = rig.app.world().resource::<AnimGraphs>().built("humanoid").unwrap();
        assert_ne!(built.graph.id(), before, "a new graph");
        assert_eq!(built.clips(), ["hit", "idle", "run", "swing_1"]);
        let now = rig.app.world().get::<AnimationGraphHandle>(rig.player).unwrap().id();
        assert_eq!(now, built.graph.id(), "the player follows");
        rig.velocity(2.0);
        rig.app.update();
        assert_eq!(rig.main_clip().as_deref(), Some("run"), "no walk clip any more: the run clip stands in");
    }

    #[test]
    fn a_respawned_player_is_bound_again() {
        let mut rig = rig();
        // A scene swap: the old player goes with its scene instance, a new one comes.
        rig.app.world_mut().despawn(rig.player);
        let fresh = rig.app.world_mut().spawn(AnimationPlayer::default()).id();
        rig.app.world_mut().entity_mut(rig.character).add_child(fresh);
        rig.app.update();
        rig.app.update();
        let world = rig.app.world();
        let bound = world.get::<AnimationBound>(rig.character).unwrap();
        assert_eq!(bound.player, fresh, "the binding follows the new player");
        let built = world.resource::<AnimGraphs>().built("humanoid").unwrap();
        assert_eq!(world.get::<AnimationGraphHandle>(fresh).map(|h| h.id()), Some(built.graph.id()));
        assert!(world.get::<AnimationTransitions>(fresh).is_some());
        assert_eq!(rig.main_clip().as_deref(), Some("idle"));
        let playing = world.get::<AnimationPlayer>(fresh).unwrap();
        assert!(playing.animation(rig.nodes["idle"]).is_some(), "the main clip plays on it");
    }

    #[test]
    fn a_missing_animation_in_the_file_is_left_out() {
        let mut set = db().get::<AnimSet>("humanoid").cloned().unwrap();
        set.clips.insert("dance".into(), default());
        let built = build_graph(&set, |_, name| (name != "dance").then(Handle::<AnimationClip>::default));
        assert_eq!(built.missing, [("models/humanoid.glb".to_owned(), "dance".to_owned())]);
        assert_eq!(built.nodes.len(), 5);
        assert_eq!(built.graph.nodes().count(), 5 + 1, "the clips and the root");
    }

    #[test]
    fn clip_names_in_the_file_come_from_the_library() {
        let mut set = db().get::<AnimSet>("humanoid").cloned().unwrap();
        set.clips.insert("bow".into(), ClipDef { source: Some("models/gestures.glb".into()), ..default() });
        let mut asked = Vec::new();
        build_graph(&set, |source, name| {
            asked.push(format!("{source}: {name}"));
            None
        });
        let asked: Vec<&str> = asked.iter().map(String::as_str).collect();
        assert_eq!(
            asked,
            [
                "models/gestures.glb: bow",
                "models/humanoid.glb: hit",
                "models/humanoid.glb: idle",
                "models/humanoid.glb: run",
                "models/humanoid.glb: Armature|Slash",
                "models/humanoid.glb: walk",
            ]
        );
        assert_eq!(set.sources(), ["models/humanoid.glb", "models/gestures.glb"]);
    }

    #[test]
    fn the_root_bone_stays_in_place_along_the_ground() {
        let mut rig = rig();
        let edited = ANIMSETS.replace("actions: {\"roll\": \"idle\"},", "actions: {\"roll\": \"idle\"}, root_motion: \"root\",");
        assert_ne!(edited, ANIMSETS);
        rig.app.insert_resource(db_from(&edited));
        rig.app.add_systems(PostUpdate, hold_root_motion.after(bevy::app::AnimationSystems));
        let bone = rig.app.world_mut().spawn((Name::new("root"), Transform::from_xyz(0.5, 0.2, 3.0))).id();
        rig.app.world_mut().entity_mut(rig.player).add_child(bone);
        rig.app.update();
        rig.app.update();
        let t = rig.app.world().get::<Transform>(bone).unwrap().translation;
        assert_eq!(t, Vec3::new(0.0, 0.2, 0.0), "back to its rest (none in the library: the origin), its height kept");
        // A set without `root_motion` leaves the bone alone.
        rig.app.insert_resource(db());
        rig.app.world_mut().get_mut::<Transform>(bone).unwrap().translation = Vec3::new(0.5, 0.2, 3.0);
        rig.app.update();
        assert_eq!(rig.app.world().get::<Transform>(bone).unwrap().translation, Vec3::new(0.5, 0.2, 3.0));
    }

    // --- layers ---------------------------------------------------------------------------

    const LAYERED: &str = r#"{ "humanoid": (
        source: "models/humanoid.glb",
        clips: {
            "idle": (), "walk": (), "run": (), "swing_1": (),
            "drink": (blend: 0.4, cues: [(at: 0.5, cue: "gulp")]), "guard": (blend: 0.4),
        },
        locomotion: {"idle": "idle", "walk": "walk", "run": "run"},
        layers: {"upper": (root: "spine")},
        actions: {"drink": (clip: "drink", layer: "upper"), "guard": (clip: "guard", layer: "upper")},
    ) }"#;

    /// Each bone of the rig: its name and its parent. The player is `Armature`, above them all.
    const SKELETON: [(&str, &str); 5] =
        [("hips", "Armature"), ("thigh", "hips"), ("spine", "hips"), ("upper_arm", "spine"), ("head", "spine")];

    /// The `x` every clip sets on every bone: a bone's pose says which clip moves it.
    const CLIP_X: [(&str, f32); 6] =
        [("idle", 0.0), ("walk", 1.0), ("run", 2.0), ("swing_1", 3.0), ("drink", 5.0), ("guard", 9.0)];

    /// A character with a skeleton whose clips each move all bones to an `x` of their own.
    struct Skeleton {
        rig: Rig,
        bones: HashMap<&'static str, Entity>,
    }

    fn skeleton(animsets: &str) -> Skeleton {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, AssetPlugin::default(), AnimationPlugin, TransformPlugin))
            .insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO))
            .init_asset::<Gltf>()
            .insert_resource(db_from(animsets))
            .init_resource::<AnimGraphs>()
            .add_systems(Update, (build_graphs, bind_players, drive).chain());
        let target = |name: &str| AnimationTargetId::from_name(&Name::new(name.to_owned()));
        let bone_names = || std::iter::once("Armature").chain(SKELETON.iter().map(|(bone, _)| *bone));
        let (mut named, mut handles) = (HashMap::new(), Vec::new());
        {
            let mut assets = app.world_mut().resource_mut::<Assets<AnimationClip>>();
            for (name, x) in CLIP_X {
                let mut clip = AnimationClip::default();
                for bone in bone_names() {
                    let still = Vec3::new(x, 0.0, 0.0);
                    let curve = AnimatableKeyframeCurve::new([(0.0, still), (1.0, still)]).unwrap();
                    clip.add_curve_to_target(target(bone), AnimatableCurve::new(animated_field!(Transform::translation), curve));
                }
                let handle = assets.add(clip);
                named.insert(Box::<str>::from(name), handle.clone());
                handles.push(handle);
            }
        }
        let gltf = Gltf {
            scenes: Vec::new(),
            named_scenes: default(),
            meshes: Vec::new(),
            named_meshes: default(),
            materials: Vec::new(),
            named_materials: default(),
            nodes: Vec::new(),
            named_nodes: default(),
            skins: Vec::new(),
            named_skins: default(),
            default_scene: None,
            animations: handles,
            named_animations: named.into_iter().collect(),
            source: None,
        };
        let library = app.world_mut().resource_mut::<Assets<Gltf>>().add(gltf);
        app.world_mut().resource_mut::<AnimGraphs>().insert_library("models/humanoid.glb", library);
        let player = app.world_mut().spawn((AnimationPlayer::default(), Name::new("Armature"), Transform::default())).id();
        app.world_mut().entity_mut(player).insert(target("Armature"));
        let mut bones = HashMap::new();
        bones.insert("Armature", player);
        for (bone, parent) in SKELETON {
            let id = app
                .world_mut()
                .spawn((Name::new(bone), Transform::default(), target(bone), AnimatedBy(player)))
                .insert(ChildOf(bones[parent]))
                .id();
            bones.insert(bone, id);
        }
        app.world_mut().entity_mut(player).insert(AnimatedBy(player));
        let character = app
            .world_mut()
            .spawn((
                Animated::new("humanoid"),
                Fighter::new("hero"),
                ActionState::default(),
                MotionState { mode: MotionMode::Ground, ..default() },
            ))
            .add_child(player)
            .id();
        app.update();
        let nodes = HashMap::new();
        Skeleton { rig: Rig { app, character, player, nodes }, bones }
    }

    impl Skeleton {
        /// Where clip `x` puts a bone, as the animation system left it.
        fn x(&self, bone: &str) -> f32 {
            self.rig.app.world().get::<Transform>(self.bones[bone]).unwrap().translation.x
        }

        fn weight(&self, layer: &str, clip: &str) -> Option<f32> {
            let node = self.rig.app.world().resource::<AnimGraphs>().built("humanoid")?.layer_node(layer, clip)?;
            let player = self.rig.app.world().get::<AnimationPlayer>(self.rig.player)?;
            player.animation(node).map(|a| a.weight())
        }

        fn clock(&mut self, millis: u64) {
            self.rig.app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_millis(millis)));
        }

        fn bound(&self) -> &AnimationBound {
            self.rig.app.world().get::<AnimationBound>(self.rig.character).unwrap()
        }

        fn end_action(&mut self) {
            self.rig.app.world_mut().get_mut::<ActionState>(self.rig.character).unwrap().current = None;
        }
    }

    fn near(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-3
    }

    const LEGS: [&str; 3] = ["Armature", "hips", "thigh"];
    const UPPER: [&str; 3] = ["spine", "upper_arm", "head"];

    #[test]
    fn a_layered_clip_has_a_node_of_its_own_masked_to_its_layer() {
        let rig = skeleton(LAYERED);
        let built = rig.rig.app.world().resource::<AnimGraphs>().built("humanoid").unwrap();
        assert_eq!(built.clips(), ["drink", "guard", "idle", "run", "swing_1", "walk"]);
        let (whole, layered) = (built.node("drink").unwrap(), built.layer_node("upper", "drink").unwrap());
        assert_ne!(whole, layered, "the same clip, another node");
        assert!(built.layer_node("upper", "walk").is_none() && built.layer_node("lower", "drink").is_none());
        let graphs = rig.rig.app.world().resource::<Assets<AnimationGraph>>();
        let graph = graphs.get(&built.graph).unwrap();
        assert_eq!(graph.nodes().count(), 6 + 2 + 1, "the clips, the two layered ones and the root");
        assert_eq!(graph.get(layered).unwrap().mask, 1, "the node leaves out group 0, the bones outside the layer");
        assert_eq!(graph.get(whole).unwrap().mask, 0, "locomotion's node leaves nothing out");
        // Group 0 holds the bones outside the layer: the layer's root and what is below it are in none.
        let group = |bone: &str| {
            let id = *rig.rig.app.world().get::<AnimationTargetId>(rig.bones[bone]).unwrap();
            graph.mask_groups.get(&id).copied().unwrap_or(0)
        };
        for bone in LEGS {
            assert_eq!(group(bone), 1, "{bone} is outside the layer");
        }
        for bone in UPPER {
            assert_eq!(group(bone), 0, "{bone} is in the layer");
        }
        assert!(rig.bound().ready.contains("upper"));
    }

    #[test]
    fn the_arms_follow_the_layered_clip_and_the_legs_keep_walking() {
        let mut rig = skeleton(LAYERED);
        rig.rig.velocity(2.0);
        rig.rig.app.update();
        assert_eq!(rig.rig.main_clip().as_deref(), Some("walk"));
        for bone in LEGS.into_iter().chain(UPPER) {
            assert!(near(rig.x(bone), 1.0), "{bone} walks: {}", rig.x(bone));
        }

        // The clock is stopped: the clip snaps in. `drink` is clip x = 5.
        rig.rig.run_action("drink", 0.5, None);
        rig.rig.app.update();
        assert_eq!(rig.rig.main_clip().as_deref(), Some("walk"), "locomotion goes on as the main clip");
        assert_eq!(rig.bound().layer_clip("upper"), Some("drink"));
        for bone in LEGS {
            assert!(near(rig.x(bone), 1.0), "{bone} still walks: {}", rig.x(bone));
        }
        for bone in UPPER {
            assert!(near(rig.x(bone), 5.0), "{bone} drinks: {}", rig.x(bone));
        }

        // The legs change from a walk to a run (the base snaps and stops every animation: the
        // layer comes back at once); the upper body keeps drinking.
        rig.rig.velocity(6.0);
        rig.rig.run_action("drink", 0.6, None);
        rig.rig.app.update();
        assert_eq!(rig.rig.main_clip().as_deref(), Some("run"));
        for bone in LEGS {
            assert!(near(rig.x(bone), 2.0), "{bone} runs: {}", rig.x(bone));
        }
        for bone in UPPER {
            assert!(near(rig.x(bone), 5.0), "{bone} still drinks: {}", rig.x(bone));
        }

        // The action ends: the upper body runs too.
        rig.end_action();
        rig.rig.app.update();
        assert_eq!(rig.bound().layer_clip("upper"), None);
        for bone in LEGS.into_iter().chain(UPPER) {
            assert!(near(rig.x(bone), 2.0), "{bone} runs: {}", rig.x(bone));
        }
        assert_eq!(rig.weight("upper", "drink"), None, "stopped, not left playing at no weight");
        assert!(rig.bound().overlays.is_empty());
    }

    #[test]
    fn the_layer_fades_in_and_out_while_the_legs_keep_walking() {
        let mut rig = skeleton(LAYERED);
        rig.rig.velocity(2.0);
        rig.rig.app.update();
        rig.clock(100);
        // `drink` blends in over 0.4 s: a quarter of the layer's bones a frame.
        rig.rig.run_action("drink", 0.1, None);
        for (frame, arms) in [(1, 2.0), (2, 3.0), (3, 4.0), (4, 5.0)] {
            rig.rig.app.update();
            for bone in UPPER {
                assert!(near(rig.x(bone), arms), "frame {frame}: {bone} is {} of the way from walk (1) to drink (5)", rig.x(bone));
            }
            for bone in LEGS {
                assert!(near(rig.x(bone), 1.0), "frame {frame}: {bone} walks: {}", rig.x(bone));
            }
            assert!(near(rig.bound().layer_share("upper"), 0.25 * frame as f32));
        }
        // Locomotion's own weight is untouched.
        let walk = rig.rig.app.world().resource::<AnimGraphs>().built("humanoid").unwrap().node("walk").unwrap();
        let player = rig.rig.app.world().get::<AnimationPlayer>(rig.rig.player).unwrap();
        assert!(near(player.animation(walk).unwrap().weight(), 1.0));

        // The action ends: back to the walk over the walk clip's blend, 0.2 s: half in a frame.
        rig.end_action();
        rig.rig.app.update();
        assert_eq!(rig.bound().layer_clip("upper"), None, "the clip is no longer the layer's main one");
        for bone in UPPER {
            assert!(near(rig.x(bone), 3.0), "{bone} is half way back: {}", rig.x(bone));
        }
        for bone in LEGS {
            assert!(near(rig.x(bone), 1.0), "{bone} walks: {}", rig.x(bone));
        }
        assert!(rig.weight("upper", "drink").is_some(), "still fading");
        rig.rig.app.update();
        for bone in UPPER {
            assert!(near(rig.x(bone), 1.0), "{bone} walks again: {}", rig.x(bone));
        }
        assert_eq!(rig.weight("upper", "drink"), None, "faded out and stopped");
        assert!(rig.bound().overlays.is_empty(), "nothing left to keep");
    }

    #[test]
    fn one_layered_clip_crossfades_into_another() {
        let mut rig = skeleton(LAYERED);
        rig.rig.velocity(2.0);
        rig.rig.run_action("drink", 0.5, None);
        rig.rig.app.update();
        assert!(near(rig.x("upper_arm"), 5.0));
        // `guard` (x = 9), also on `upper`, takes over over 0.4 s: the layer never lets the walk through.
        rig.clock(100);
        rig.rig.run_action("guard", 0.1, None);
        rig.rig.app.update();
        assert_eq!(rig.bound().layer_clip("upper"), Some("guard"));
        assert!(near(rig.x("upper_arm"), 0.75 * 5.0 + 0.25 * 9.0), "{}", rig.x("upper_arm"));
        rig.rig.app.update();
        assert!(near(rig.x("upper_arm"), 0.5 * 5.0 + 0.5 * 9.0), "{}", rig.x("upper_arm"));
        for _ in 0..3 {
            rig.rig.app.update();
        }
        assert!(near(rig.x("upper_arm"), 9.0), "{}", rig.x("upper_arm"));
        assert_eq!(rig.weight("upper", "drink"), None, "the drink faded out and stopped");
        assert!(near(rig.x("thigh"), 1.0));
    }

    #[test]
    fn a_layered_clip_fires_its_cues_once() {
        #[derive(Resource, Default)]
        struct Heard(Vec<AnimationCue>);
        let mut rig = skeleton(LAYERED);
        rig.rig.app.init_resource::<Heard>();
        rig.rig.app.add_observer(|cue: On<AnimationCue>, mut heard: ResMut<Heard>| heard.0.push(cue.event().clone()));
        rig.rig.run_action("drink", 0.2, None); // a fifth in: the cue is at 0.5 s of the 1 s clip
        rig.rig.app.update();
        assert!(rig.rig.app.world().resource::<Heard>().0.is_empty());
        rig.rig.run_action("drink", 0.7, None);
        rig.rig.app.update();
        rig.rig.run_action("drink", 0.9, None);
        rig.rig.app.update();
        let heard = &rig.rig.app.world().resource::<Heard>().0;
        assert_eq!(heard.len(), 1, "{heard:?}");
        assert_eq!((heard[0].entity, heard[0].cue.as_str(), heard[0].clip.as_str()), (rig.rig.character, "gulp", "drink"));
    }

    #[test]
    fn a_model_without_the_layers_bone_keeps_its_locomotion_clip() {
        let mut rig = skeleton(&LAYERED.replace("root: \"spine\"", "root: \"tail\""));
        assert!(rig.bound().ready.is_empty());
        rig.rig.velocity(2.0);
        rig.rig.run_action("drink", 0.5, None);
        rig.rig.app.update();
        assert_eq!(rig.bound().layer_clip("upper"), None);
        for bone in LEGS.into_iter().chain(UPPER) {
            assert!(near(rig.x(bone), 1.0), "{bone} walks: {}", rig.x(bone));
        }
    }

    #[test]
    fn a_set_without_layers_builds_the_graph_it_always_did() {
        let rig = rig();
        let graph = rig.app.world().resource::<Assets<AnimationGraph>>().get(&rig.app.world().resource::<AnimGraphs>().built("humanoid").unwrap().graph).unwrap();
        assert_eq!(graph.nodes().count(), 5 + 1);
        assert!(graph.mask_groups.is_empty());
    }

    #[test]
    fn the_overlay_fades_by_the_blend_and_never_takes_the_whole_layer() {
        let (a, b) = (AnimationNodeIndex::new(1), AnimationNodeIndex::new(2));
        let mut overlay = Overlay::default();
        let mut stopped = Vec::new();
        let weight_of = |w: &[(AnimationNodeIndex, f32)], node| w.iter().find(|(n, _)| *n == node).map(|(_, w)| *w);
        // In over 0.5 s: a share s has the weight s / (1 - s).
        overlay.switch(Some(("a", a)), 0.5, false, &mut stopped);
        let w = overlay.advance(0.25, &mut stopped);
        assert!(near(weight_of(&w, a).unwrap(), 1.0), "half the layer's bones: weight 1 against locomotion's 1");
        assert!(near(overlay.share(), 0.5));
        let w = overlay.advance(0.25, &mut stopped);
        let full = weight_of(&w, a).unwrap();
        assert!(full > 1e4 && full.is_finite(), "all of it but a hair: {full}");
        assert!(overlay.share() < 1.0 && overlay.share() > 0.9999, "{}", overlay.share());
        // The action ends: out over 0.25 s, the share falling from what it was.
        overlay.switch(None, 0.25, false, &mut stopped);
        let w = overlay.advance(0.125, &mut stopped);
        assert!(near(weight_of(&w, a).unwrap(), 1.0), "half way out");
        assert!(stopped.is_empty());
        let w = overlay.advance(0.125, &mut stopped);
        assert_eq!(weight_of(&w, a), Some(0.0));
        assert_eq!(stopped, [a]);
        assert!(overlay.idle());
        // A clip that takes over from another: the shares add up to what the layer had, and the walk never shows through.
        stopped.clear();
        overlay.switch(Some(("a", a)), 0.5, true, &mut stopped);
        assert!(near(overlay.share(), 0.0), "not advanced yet");
        overlay.advance(0.0, &mut stopped);
        assert!(overlay.share() > 0.9999, "snapped: {}", overlay.share());
        overlay.switch(Some(("b", b)), 0.5, false, &mut stopped);
        let w = overlay.advance(0.25, &mut stopped);
        let (wa, wb) = (weight_of(&w, a).unwrap(), weight_of(&w, b).unwrap());
        assert!(near(wa, wb), "half and half after half the blend: {wa} {wb}");
        assert!(overlay.share() > 0.9999, "the walk does not show through");
        // Back to a clip that was fading starts it again.
        overlay.switch(Some(("a", a)), 0.5, false, &mut stopped);
        assert_eq!(overlay.fading.iter().map(|f| f.node).collect::<Vec<_>>(), [b]);
        assert_eq!(overlay.main.as_ref().map(|m| m.ramp), Some(0.0));
        // The clock stopped: everything snaps, and what played stops.
        stopped.clear();
        overlay.switch(None, 0.5, true, &mut stopped);
        assert_eq!(stopped.len(), 2);
        assert!(overlay.idle());
    }
}
