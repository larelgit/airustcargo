//! A character's model from a glTF file loaded whole ([`GltfModel`]).
//!
//! `WorldAssetRoot(asset_server.load("models/hero.glb#Scene0"))` reads the file again even while the
//! whole file is loading or loaded (bevy#10549). When the animation bridge loads the same file whole
//! for its clips and that read ends after the model is out, the scene's asset is replaced and the
//! model is spawned a second time: a rebind, with its props spawned again as new entities. A
//! [`GltfModel`] loads the file whole, the way the bridge does, so both share one handle and one read,
//! and spawns its scene when the file is in.

use bevy::asset::AssetPath;
use bevy::gltf::Gltf;
use bevy::prelude::*;
use bevy::world_serialization::{InstanceId, WorldAssetRoot, WorldInstance, WorldInstanceSpawner};

/// A model from glTF file `file`, loaded whole: when the file is in, its scene `scene` is spawned on
/// this entity (a [`WorldAssetRoot`] is inserted beside this component). It requires what a
/// `WorldAssetRoot` does (`Transform`, `Visibility`). Put it where a
/// `WorldAssetRoot` would go:
///
/// ```ignore
/// commands.spawn((body, Animated::new("humanoid")))
///     .with_child((GltfModel::load(&asset_server, "models/humanoid.glb"), Transform::default()));
/// ```
///
/// A file that does not load, or has no such scene, spawns nothing (the latter is warned once, and a
/// model changed to such a scene takes its old one away).
/// Changing the component to another file or scene replaces the scene once the new file is in.
#[derive(Component, Clone, Debug)]
#[require(Transform, Visibility)]
pub struct GltfModel {
    pub file: Handle<Gltf>,
    /// The index in the file's `scenes`; 0 by default.
    pub scene: usize,
}

impl GltfModel {
    /// Scene 0 of a file already being loaded whole.
    pub fn new(file: Handle<Gltf>) -> Self {
        Self { file, scene: 0 }
    }

    /// Scene 0 of file `path`, loaded whole.
    pub fn load<'a>(asset_server: &AssetServer, path: impl Into<AssetPath<'a>>) -> Self {
        Self::new(asset_server.load(path.into().into_owned()))
    }

    /// Another scene of the file.
    pub fn scene(mut self, scene: usize) -> Self {
        self.scene = scene;
        self
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(Update, spawn_models);
}

/// `Update`: the models whose file is in get their scene, and a model changed to another file or
/// scene gets that one (Bevy takes the old scene's entities away when the root changes; until the
/// new file is in, the old scene stays). A model changed to a scene its file does not have shows
/// nothing: its old scene goes.
fn spawn_models(
    mut commands: Commands,
    models: Query<(Entity, &GltfModel, Option<&WorldAssetRoot>, Option<&WorldInstance>)>,
    gltfs: Option<Res<Assets<Gltf>>>,
    mut warned: Local<Vec<(AssetId<Gltf>, usize)>>,
) {
    let Some(gltfs) = gltfs else { return };
    for (entity, model, shown, instance) in &models {
        let Some(gltf) = gltfs.get(&model.file) else { continue };
        match gltf.scenes.get(model.scene) {
            Some(scene) if shown.is_none_or(|root| root.0 != *scene) => {
                commands.entity(entity).insert(WorldAssetRoot(scene.clone()));
            }
            Some(_) => {}
            None => {
                if !warned.contains(&(model.file.id(), model.scene)) {
                    warned.push((model.file.id(), model.scene));
                    warn!("{:?} has {} scenes: no scene {} to spawn", model.file.path(), gltf.scenes.len(), model.scene);
                }
                if shown.is_some() {
                    clear_scene(&mut commands, entity, instance.map(|i| **i));
                }
            }
        }
    }
}

/// Takes the scene off `entity`: its instance's entities first, at once (removing the root only
/// forgets the instance, and a despawn queued before that removal would find nothing), then the root.
fn clear_scene(commands: &mut Commands, entity: Entity, instance: Option<InstanceId>) {
    commands.queue(move |world: &mut World| {
        if let Some(instance) = instance
            && world.contains_resource::<WorldInstanceSpawner>()
        {
            world.resource_scope(|world, mut spawner: Mut<WorldInstanceSpawner>| spawner.despawn_instance_sync(world, &instance));
        }
        if let Ok(mut model) = world.get_entity_mut(entity) {
            model.remove::<(WorldAssetRoot, WorldInstance)>();
        }
    });
}
