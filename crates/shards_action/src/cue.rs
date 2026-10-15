//! Sound cues: what the audio module plays.
//!
//! An attack's telegraph names a sound (`TelegraphDef::sound`); when it starts, a
//! [`SoundCue`] is triggered where the attack will land. Nothing here makes a sound: the
//! audio module (E9) observes the event and plays it, a headless run just ignores it.
//! Cues are cosmetic, so they never reach the journal or the replay checks.

use bevy::prelude::*;

use crate::machine::Telegraph;

/// A sound for the audio module to play at `position`.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct SoundCue {
    /// The entity the sound belongs to (the attacker whose telegraph it is).
    pub entity: Entity,
    /// The cue id from content: `boss_roar`, `slam_warning`.
    pub cue: String,
    pub position: Vec3,
}

pub(crate) fn plugin(app: &mut App) {
    app.add_observer(telegraph_sound);
}

/// A [`Telegraph`] with a sound becomes a [`SoundCue`] at the warned zone (else at the
/// attacker).
fn telegraph_sound(telegraph: On<Telegraph>, positions: Query<&Transform>, mut commands: Commands) {
    let telegraph = telegraph.event();
    let Some(cue) = telegraph.sound.clone() else { return };
    let position = telegraph
        .zone
        .map(|zone| zone.center())
        .or_else(|| positions.get(telegraph.attacker).ok().map(|t| t.translation))
        .unwrap_or_default();
    debug!("sound cue `{cue}` for {} ({}) at {position}", telegraph.attacker, telegraph.action);
    commands.trigger(SoundCue { entity: telegraph.attacker, cue, position });
}
