//! The visual side of a level (the `render` feature): the same file loaded by Bevy's own
//! glTF loader, as one scene under one root entity. Visuals never carry gameplay
//! components, so a window adds no gameplay entity and no `AiId`.
//!
//! A mover's node in the picture ([`MoverPicture`]) is drawn where the mover stands: the scene's
//! nodes below it hang under it and come along.

use bevy::prelude::*;

/// Marks the root of a level's visual scene. Not reflected, not AI-visible: it is not
/// gameplay.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct LevelVisual {
    /// The level instance the scene shows.
    pub instance: u32,
}

/// Loads scene `scene` of `path` (the one the level's nodes come from) under a new root at
/// `at`. `None` without the `render` feature, for a level that was not read from a file, and
/// in an app that has no glTF scene support (the headless engine has it; a test app built
/// from `MinimalPlugins` does not).
#[cfg(feature = "render")]
pub(crate) fn spawn_visual(world: &mut World, path: &str, scene: usize, at: Transform, instance: u32) -> Option<Entity> {
    use bevy::gltf::GltfAssetLabel;
    use bevy::world_serialization::{WorldAsset, WorldAssetRoot};

    if path.is_empty() || !world.contains_resource::<Assets<WorldAsset>>() {
        return None;
    }
    let scene = world.get_resource::<AssetServer>()?.load(GltfAssetLabel::Scene(scene).from_asset(path.to_owned()));
    Some(world.spawn((LevelVisual { instance }, WorldAssetRoot(scene), at)).id())
}

#[cfg(not(feature = "render"))]
pub(crate) fn spawn_visual(_: &mut World, _: &str, _: usize, _: Transform, _: u32) -> Option<Entity> {
    None
}

/// A node of a level's picture that shows a mover (the node with `mover=<id>`; the picture's nodes
/// below it hang under it and follow): drawn where the mover stands, blended between the last two
/// ticks as a character's picture is (`shards_motion::Interpolated`). Presentation only: no
/// `AiId`, not in a snapshot or a replay. The mover is found by its level instance and node path
/// every time (a snapshot load replaces the entity).
#[cfg(feature = "render")]
#[derive(Component, Clone, Debug)]
pub struct MoverPicture {
    /// The level instance.
    pub instance: u32,
    /// The mover's node path in the file (`Lifts/Lift`).
    pub node: String,
    /// The node's own transform as the file has it.
    rest: Transform,
    /// The mover's poses after the last two ticks.
    history: shards_motion::Interpolated,
}

#[cfg(feature = "render")]
pub(crate) fn plugin(app: &mut App) {
    use bevy::transform::TransformSystems;
    use shards_motion::InterpSystems;

    app.add_systems(Update, follow::find_mover_pictures)
        .add_systems(FixedLast, follow::capture_movers.after(InterpSystems::Capture))
        .add_systems(PostUpdate, follow::draw_movers.after(InterpSystems::Blend).before(TransformSystems::Propagate));
}

#[cfg(feature = "render")]
mod follow {
    use std::collections::{BTreeMap, BTreeSet};

    use ai_core::SimTick;
    use bevy::prelude::*;
    use shards_core::{EngineConfig, NetRole};
    use shards_motion::camera::CameraTuning;
    use shards_motion::{Mover, MoverState, Pose};
    use shards_settings::GraphicsSettings;

    use super::MoverPicture;
    use crate::spawn::{FromLevel, LevelInstances};

    type Movers<'a> = BTreeMap<(u32, &'a str), (Transform, Option<&'a MoverState>)>;

    /// The movers placed by levels, by `(instance, node path)`.
    fn movers_by_node<'a>(movers: impl Iterator<Item = (&'a FromLevel, &'a Transform, Option<&'a MoverState>)>) -> Movers<'a> {
        movers.map(|(from, tf, state)| ((from.instance, from.node.as_str()), (*tf, state))).collect()
    }

    /// The machine blends a mover's ticks itself: everywhere it simulates. A client shows the
    /// host's poses as they come.
    fn blends_here(role: Option<&NetRole>) -> bool {
        role.is_none_or(|r| r.simulates())
    }

    /// `Update`: the picture node of each level mover that has none yet, found under its level's
    /// picture by the node's path, once the scene is in.
    pub(super) fn find_mover_pictures(
        mut commands: Commands,
        instances: Option<Res<LevelInstances>>,
        movers: Query<&FromLevel, With<Mover>>,
        pictures: Query<&MoverPicture>,
        children: Query<&Children>,
        nodes: Query<(&Name, &Transform)>,
        parents: Query<&ChildOf>,
    ) {
        let Some(instances) = instances else { return };
        let shown: BTreeSet<(u32, &str)> = pictures.iter().map(|p| (p.instance, p.node.as_str())).collect();
        for from in &movers {
            if shown.contains(&(from.instance, from.node.as_str())) {
                continue;
            }
            let Some(root) = instances.get(from.instance).and_then(|i| i.visual) else { continue };
            let path: Vec<&str> = from.node.split('/').collect();
            let Some(last) = path.last().copied() else { continue };
            let found = children.iter_descendants(root).find(|&e| {
                if nodes.get(e).map(|(name, _)| name.as_str()) != Ok(last) {
                    return false;
                }
                // The names from the scene's root down to the entity are the node's whole path.
                // The picture's root holds the glTF scene's own root (Bevy names it after the
                // scene, `Scene0` for one without a name), and the file's top nodes hang below
                // it. A path's tail is not enough: `B/A/Mover` ends with `A/Mover` too.
                let mut names = Vec::new();
                let mut at = e;
                loop {
                    let Ok(parent) = parents.get(at) else { return false };
                    if parent.parent() == root {
                        // `at` is the scene's root, not a node of the file.
                        break;
                    }
                    if let Ok((name, _)) = nodes.get(at) {
                        names.push(name.as_str());
                    }
                    at = parent.parent();
                }
                names.reverse();
                names == path
            });
            if let Some(entity) = found
                && let Ok((_, rest)) = nodes.get(entity)
            {
                commands.entity(entity).try_insert(MoverPicture {
                    instance: from.instance,
                    node: from.node.clone(),
                    rest: *rest,
                    history: default(),
                });
            }
        }
    }

    /// `FixedLast`: each picture keeps the pose its mover ended the tick with.
    pub(super) fn capture_movers(
        tick: Option<Res<SimTick>>,
        tuning: Option<Res<CameraTuning>>,
        role: Option<Res<NetRole>>,
        config: Option<Res<EngineConfig>>,
        movers: Query<(&FromLevel, &Transform, Option<&MoverState>), With<Mover>>,
        mut pictures: Query<&mut MoverPicture>,
    ) {
        if pictures.is_empty() || config.is_some_and(|c| !c.draws()) {
            return;
        }
        let snap = tuning.map_or(CameraTuning::default().body_snap_distance, |t| t.body_snap_distance);
        let tick = tick.map(|t| t.0);
        let movers = movers_by_node(movers.iter());
        for mut picture in &mut pictures {
            let node = picture.node.clone();
            match movers.get(&(picture.instance, node.as_str())) {
                Some((tf, _)) if blends_here(role.as_deref()) => picture.history.capture(Pose::of(tf), tick, snap),
                _ => picture.history.reset(),
            }
        }
    }

    /// `PostUpdate`, before transforms propagate: each picture node is put where its mover draws
    /// it: its rest pose moved by the mover's motion since it was put (the mover's pose as drawn,
    /// relative to its origin), in the world, whatever the nodes above it are.
    pub(super) fn draw_movers(
        fixed: Res<Time<Fixed>>,
        virtual_time: Res<Time<Virtual>>,
        settings: Option<Res<GraphicsSettings>>,
        role: Option<Res<NetRole>>,
        movers: Query<(&FromLevel, &Transform, Option<&MoverState>), With<Mover>>,
        mut pictures: Query<(&mut MoverPicture, &mut Transform, Option<&ChildOf>), Without<Mover>>,
        globals: Query<&GlobalTransform>,
    ) {
        if pictures.is_empty() {
            return;
        }
        let on = settings.is_none_or(|s| s.interpolation) && blends_here(role.as_deref());
        // A paused clock (and a `step`) shows the last tick.
        let fraction = if virtual_time.is_paused() { 1.0 } else { fixed.overstep_fraction() };
        let movers = movers_by_node(movers.iter());
        for (picture, mut tf, parent) in &mut pictures {
            let Some((now, state)) = movers.get(&(picture.instance, picture.node.as_str())) else { continue };
            let live = Pose::of(now);
            // Placed between ticks (a snapshot, a hand): drawn where it is.
            let shown = match picture.history.current() {
                Some(cur) if on && cur == live => picture.history.at(fraction).unwrap_or(live),
                _ => live,
            };
            let origin = state.map_or(*now, |s| Transform::from_translation(s.origin).with_rotation(s.origin_rotation));
            let moved = Mat4::from_rotation_translation(shown.rotation, shown.translation)
                * Mat4::from_rotation_translation(origin.rotation, origin.translation).inverse();
            let above = parent.and_then(|p| globals.get(p.parent()).ok()).map_or(Mat4::IDENTITY, GlobalTransform::to_matrix);
            let local = Transform::from_matrix(above.inverse() * moved * above * picture.rest.to_matrix());
            if *tf != local {
                *tf = local;
            }
        }
    }
}
