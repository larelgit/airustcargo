//! The visual side of a level (the `render` feature): the same file loaded by Bevy's own
//! glTF loader, as one scene under one root entity. Visuals never carry gameplay
//! components, so a window adds no gameplay entity and no `AiId`.

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
