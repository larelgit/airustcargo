//! Where sound meets the camera: the ears face the way the engine's third-person camera does
//! (the audio module hears with the body the HUD is about and faces with an [`AudioListener`]).

use bevy::prelude::*;
use shards_audio::AudioListener;
use shards_motion::ThirdPersonCamera;

/// Gives every [`ThirdPersonCamera`] an [`AudioListener`]. Part of
/// [`EnginePlugins`](crate::EnginePlugins), after the audio module.
pub struct EngineAudioPlugin;

impl Plugin for EngineAudioPlugin {
    fn build(&self, app: &mut App) {
        app.register_required_components::<ThirdPersonCamera, AudioListener>();
    }
}
