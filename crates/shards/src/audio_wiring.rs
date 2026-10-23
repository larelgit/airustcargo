//! Where sound meets the camera and the levels: the ears face the way the engine's third-person camera
//! does (the audio module hears with the body the HUD is about and faces with an [`AudioListener`]), and
//! a level file's `SoundEmitter` nodes are checked against the sounds there are.

use std::any::TypeId;

use bevy::prelude::*;
use serde_json::Value;
use shards_audio::{AudioListener, SoundEmitter};
use shards_content::prefab::resolve_component;
use shards_content::{ContentAppExt, Refs};
use shards_level::{LevelAsset, LevelDef, LevelPlugin};
use shards_motion::ThirdPersonCamera;

/// Gives every [`ThirdPersonCamera`] an [`AudioListener`], and (with the level module) checks the
/// emitters in level files. Part of [`EnginePlugins`](crate::EnginePlugins), after the audio module.
pub struct EngineAudioPlugin;

impl Plugin for EngineAudioPlugin {
    fn build(&self, app: &mut App) {
        app.register_required_components::<ThirdPersonCamera, AudioListener>();
        if app.is_plugin_added::<LevelPlugin>() {
            app.add_content_check::<LevelDef>("sound emitters", check_level_emitters);
        }
    }
}

/// A node of a level file that carries a `SoundEmitter` names a sound some file defines, and one that
/// loops. A value that does not read is the level's own error (its check names it).
fn check_level_emitters(def: &LevelDef, _id: &str, refs: &mut Refs) {
    let (Some(root), Some(registry)) = (refs.asset_root(), refs.type_registry()) else { return };
    let Ok(level) = LevelAsset::read(root, &def.file) else { return };
    let mut found = Vec::new();
    for node in &level.nodes {
        for (key, value) in &node.extras {
            if !resolve_component(registry, key).is_ok_and(|reg| reg.type_id() == TypeId::of::<SoundEmitter>()) {
                continue;
            }
            // Blender may hold the object as text.
            let value = match value {
                Value::String(text) => serde_json::from_str(text).unwrap_or(Value::Null),
                other => other.clone(),
            };
            if let Some(sound) = value.get("sound").and_then(Value::as_str) {
                found.push((node.path.clone(), sound.to_owned()));
            }
        }
    }
    for (path, sound) in found {
        shards_audio::check_sound(refs, &sound, &format!("node `{path}`: SoundEmitter.sound"));
    }
}
