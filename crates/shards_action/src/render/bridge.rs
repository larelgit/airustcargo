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
//!
//! The character is `Animated` plus a child with the model:
//!
//! ```ignore
//! commands.spawn((hero_bundle, Animated::new("humanoid"))).with_child(WorldAssetRoot(
//!     asset_server.load(GltfAssetLabel::Scene(0).from_asset("models/hero.glb")),
//! ));
//! ```

use std::collections::{BTreeSet, HashMap};
use std::time::Duration;

use bevy::animation::graph::{AnimationGraph, AnimationGraphHandle, AnimationNodeIndex};
use bevy::animation::transition::AnimationTransitions;
use bevy::animation::{AnimatedBy, AnimationClip, AnimationPlayer, AnimationTargetId, RepeatAnimation};
use bevy::asset::LoadState;
use bevy::ecs::query::QueryData;
use bevy::gltf::{Gltf, GltfNode};
use bevy::prelude::*;
use bevy::world_serialization::WorldInstanceReady;
use shards_content::ContentDb;
use shards_core::Predicted;
use shards_motion::MotionState;

use crate::anim::{select_clip_for, AnimSet, Animated, AnimationCue, BodyStatus, crossed_cues};
use crate::def::{ActionDef, MovesetDef};
use crate::fighter::{Dead, Fighter};
use crate::machine::ActionState;
use crate::predict::PredictedAction;

/// A seeked clip stops this far (seconds) before its end: Bevy's player wraps a time at the
/// duration back to zero, which would show the first pose in place of the last.
const END_MARGIN: f32 = 1e-3;

/// A set's graph and what is in it.
pub struct Built {
    pub graph: Handle<AnimationGraph>,
    /// Our clip name -> its node and its clip asset.
    nodes: Nodes,
}

/// Our clip name -> its node in the graph and its clip asset.
type Nodes = HashMap<String, (AnimationNodeIndex, Handle<AnimationClip>)>;

impl Built {
    /// The graph node playing clip `clip` of the set.
    pub fn node(&self, clip: &str) -> Option<AnimationNodeIndex> {
        self.nodes.get(clip).map(|(n, _)| *n)
    }

    /// Names of the clips the graph has, sorted.
    pub fn clips(&self) -> Vec<&str> {
        let mut names: Vec<&str> = self.nodes.keys().map(String::as_str).collect();
        names.sort_unstable();
        names
    }
}

struct SetGraph {
    /// The definition the graph was built from: an edit rebuilds it.
    def: AnimSet,
    built: Option<Built>,
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
}

impl AnimationBound {
    /// The clip the bridge has playing now.
    pub fn clip(&self) -> Option<&str> {
        self.clip.as_deref()
    }
}

/// What [`build_graph`] made.
pub struct GraphBuild {
    pub graph: AnimationGraph,
    nodes: Nodes,
    /// `(file, animation)` of the animations the set wants and its files lack.
    pub missing: Vec<(String, String)>,
}

/// The graph of `set`: a node for each clip of the library whose animation `find` supplies
/// (it gets the clip's file and the animation's name in it), in the order of the clip names.
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
    GraphBuild { graph, nodes, missing }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<AnimGraphs>()
        .add_observer(retarget_on_ready)
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
        let changed = state.sets.get(id).is_none_or(|s| s.def != *set);
        if changed {
            state.sets.insert(id.to_owned(), SetGraph { def: set.clone(), built: None });
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
        let GraphBuild { graph, nodes, missing } =
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
            entry.built = Some(Built { graph, nodes });
        }
    }
}

/// `Update`: gives the `AnimationPlayer` under each [`Animated`] character its set's graph
/// and an `AnimationTransitions`; again when the graph was rebuilt.
pub fn bind_players(
    mut commands: Commands,
    graphs: Res<AnimGraphs>,
    animated: Query<(Entity, &Animated, Option<&AnimationBound>)>,
    children: Query<&Children>,
    mut players: Query<&mut AnimationPlayer>,
) {
    for (entity, set, bound) in &animated {
        let Some(built) = graphs.built(&set.animset) else { continue };
        // The graph is the set's current one and its player is still there (a model reload
        // or a scene swap spawns the scene again, the player with it).
        if bound.is_some_and(|b| b.graph == built.graph.id() && players.contains(b.player)) {
            continue;
        }
        let found = std::iter::once(entity).chain(children.iter_descendants(entity)).find(|e| players.contains(*e));
        let Some(player) = found else { continue };
        if let Ok(mut p) = players.get_mut(player) {
            p.stop_all();
        }
        commands
            .entity(player)
            .insert((AnimationGraphHandle(built.graph.clone()), AnimationTransitions::new()));
        commands.entity(entity).insert(AnimationBound { player, graph: built.graph.id(), clip: None, last_time: None, root: None });
    }
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
    motion: Option<&'static MotionState>,
    fighter: Option<&'static Fighter>,
    dead: Has<Dead>,
}

/// `Update`: the clip each character shows, applied to its player. See the module docs.
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
pub fn drive(
    mut commands: Commands,
    time: Res<Time>,
    fixed: Res<Time<Fixed>>,
    db: Option<Res<ContentDb>>,
    graphs: Res<AnimGraphs>,
    clips: Res<Assets<AnimationClip>>,
    mut animated: Query<Driven>,
    mut players: Query<(&mut AnimationPlayer, &mut AnimationTransitions)>,
) {
    let Some(db) = db else { return };
    let overstep = fixed.overstep().as_secs_f32();
    let frozen = time.delta_secs() <= 0.0;
    let standing = MotionState::default();
    for DrivenItem { entity, animated: set_ref, mut bound, state, shown, predicted, motion, fighter, dead } in &mut animated {
        let (Some(set), Some(built)) = (db.get::<AnimSet>(&set_ref.animset), graphs.built(&set_ref.animset)) else {
            continue;
        };
        if bound.graph != built.graph.id() {
            continue; // `bind_players` takes it over to the rebuilt graph next
        }
        let Ok((mut player, mut transitions)) = players.get_mut(bound.player) else { continue };

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
        let choice = select_clip_for(action, motion.unwrap_or(&standing), BodyStatus { dead, staggered: staggers }, set);
        let Some(clip) = choice.clip else { continue };
        let Some((node, handle)) = built.nodes.get(clip) else { continue };
        let duration = clips.get(handle).map_or(0.0, |c| c.duration());

        if bound.clip.as_deref() != Some(clip) {
            if bound.clip.is_none() || frozen {
                // Nothing to blend from, or the clock is stopped (a paused simulation, an agent
                // stepping tick by tick): a fade would never advance and leave the old pose
                // standing, so the new clip snaps in.
                player.stop_all();
                *transitions = AnimationTransitions::new();
                transitions.play(&mut player, *node, Duration::ZERO);
            } else {
                // Not zero (Bevy's fade divides by it), not absurd (a typo in content must not
                // overflow a `Duration`).
                let blend = choice.blend.clamp(0.01, 10.0);
                transitions.play(&mut player, *node, Duration::from_secs_f32(blend));
            }
            bound.clip = Some(clip.to_owned());
            bound.last_time = None;
        }
        let Some(active) = player.animation_mut(*node) else { continue };
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
    mut transforms: Query<&mut Transform>,
) {
    let Some(db) = db else { return };
    for (set_ref, mut bound) in &mut animated {
        let Some(bone) = db.get::<AnimSet>(&set_ref.animset).and_then(|s| s.root_motion.as_deref().map(|b| (s, b))) else {
            continue;
        };
        let (set, bone) = bone;
        if bound.root.is_none_or(|(e, _)| !transforms.contains(e)) {
            let found = std::iter::once(bound.player)
                .chain(children.iter_descendants(bound.player))
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
pub fn retarget_on_ready(
    ready: On<WorldInstanceReady>,
    mut commands: Commands,
    parents: Query<&ChildOf>,
    animated: Query<(), With<Animated>>,
    children: Query<&Children>,
    players: Query<(), With<AnimationPlayer>>,
    names: Query<&Name>,
) {
    let root = ready.entity;
    if !std::iter::once(root).chain(parents.iter_ancestors(root)).any(|e| animated.contains(e)) {
        return;
    }
    if std::iter::once(root).chain(children.iter_descendants(root)).any(|e| players.contains(e)) {
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
    use bevy::animation::AnimationPlugin;
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
}
