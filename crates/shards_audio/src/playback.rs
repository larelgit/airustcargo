//! Playback (feature `playback`): Bevy's audio plays what the mixer decided. A voice is a sink
//! of its file at the gain the mixer gives it; a spatial one sits a meter from the ears in the
//! direction it comes from, so playback pans it while the mixer's gain says how far it is and what
//! stands in its way (a muffled voice is a quieter one: Bevy's sinks have no per-voice filter). A
//! looped voice, an emitter's, plays on until the mixer lets it go. A track's stems start together
//! once all their files are loaded, and loop. While the game is paused the voices of the simulation
//! hold; music and menu sounds go on. With no audio device the sinks never start, and the mixer does
//! not mind.

use std::collections::{BTreeMap, BTreeSet};

use bevy::audio::{AudioPlayer, AudioSink, AudioSinkPlayback, AudioSource, PlaybackSettings, SpatialAudioSink, SpatialListener, SpatialScale, Volume};
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_settings::AudioSettings;

use crate::def::{Bus, MusicDef, SoundDef};
use crate::mixer::Mixer;
use crate::music::Music;
use crate::AudioSystems;

/// The gap between the ears (m).
const EAR_GAP: f32 = 0.5;
/// How far from the ears a voice's emitter is put (m): close enough that Bevy's own fading by
/// distance stays whole, so the mixer's gain is what is heard.
const EMITTER: f32 = 1.0;
/// How fast a sink's volume follows the mixer's (share of the way per second): no clicks.
const SLEW: f32 = 20.0;

/// The sinks playing and the files loaded for them.
#[derive(Resource, Default)]
struct Sinks {
    handles: BTreeMap<String, Handle<AudioSource>>,
    voices: BTreeMap<u64, Entity>,
    /// Voices whose sink played out before the mixer let them go.
    finished: BTreeSet<u64>,
    stems: BTreeMap<(String, u32), Entity>,
    ears: Option<Entity>,
}

#[derive(Component)]
struct VoiceSink {
    bus: Bus,
}

#[derive(Component)]
struct StemSink;

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<Sinks>().add_systems(
        Update,
        (load, ears, voices, stems, hold_while_paused).chain().in_set(AudioSystems::Play).run_if(resource_exists::<Assets<AudioSource>>),
    );
}

/// Every file of every sound and track, loaded once content names it.
fn load(db: Res<ContentDb>, server: Res<AssetServer>, mut sinks: ResMut<Sinks>) {
    if !db.is_changed() && !sinks.handles.is_empty() {
        return;
    }
    let sounds = db.iter::<SoundDef>().flat_map(|(_, d)| d.files.clone());
    let stems = db.iter::<MusicDef>().flat_map(|(_, d)| d.stems.iter().map(|s| s.file.clone()).collect::<Vec<_>>());
    let files: BTreeSet<String> = sounds.chain(stems).collect();
    sinks.handles.retain(|file, _| files.contains(file));
    for file in files {
        sinks.handles.entry(file.clone()).or_insert_with(|| server.load(file));
    }
}

/// The ears are where the mixer's listener is, facing its way.
fn ears(mut commands: Commands, mixer: Res<Mixer>, mut sinks: ResMut<Sinks>, mut places: Query<&mut Transform, With<SpatialListener>>) {
    let at = mixer.listener.map_or(Transform::IDENTITY, |l| Transform::from_translation(l.position).with_rotation(l.rotation));
    if let Some(mut place) = sinks.ears.and_then(|e| places.get_mut(e).ok()) {
        place.set_if_neq(at);
    } else {
        sinks.ears = Some(commands.spawn((Name::new("audio ears"), SpatialListener::new(EAR_GAP), at)).id());
    }
}

fn slew(now: f32, target: f32, dt: f32) -> f32 {
    now + (target - now) * (SLEW * dt).min(1.0)
}

/// The voices' emitters: where each sits, and its sink once playback made it (spatial or not).
type VoiceSinks<'w, 's> =
    Query<'w, 's, (&'static mut Transform, Option<&'static mut AudioSink>, Option<&'static mut SpatialAudioSink>), With<VoiceSink>>;

/// A sink for every voice the mixer plays, at its gain and from its side; none for the ones it
/// let go.
fn voices(
    mut commands: Commands,
    mixer: Res<Mixer>,
    time: Res<Time<Real>>,
    mut sinks: ResMut<Sinks>,
    mut players: VoiceSinks,
) {
    let alive: BTreeSet<u64> = mixer.voices().iter().map(|v| v.id).collect();
    let gone: Vec<u64> = sinks.voices.keys().filter(|id| !alive.contains(id)).copied().collect();
    for id in gone {
        if let Some(e) = sinks.voices.remove(&id) {
            commands.entity(e).try_despawn();
        }
    }
    sinks.finished.retain(|id| alive.contains(id));
    let ear = mixer.listener.map_or(Vec3::ZERO, |l| l.position);
    let dt = time.delta_secs();
    for voice in mixer.voices() {
        let place = voice.position.map_or(ear, |p| ear + (p - ear).normalize_or(Vec3::NEG_Z) * EMITTER);
        match sinks.voices.get(&voice.id).copied() {
            Some(e) => match players.get_mut(e) {
                Ok((mut transform, sink, spatial)) => {
                    transform.translation = place;
                    if let Some(mut sink) = sink {
                        let now = sink.volume().to_linear();
                        sink.set_volume(Volume::Linear(slew(now, voice.gain, dt)));
                    }
                    if let Some(mut sink) = spatial {
                        let now = sink.volume().to_linear();
                        sink.set_volume(Volume::Linear(slew(now, voice.gain, dt)));
                    }
                }
                // Bevy despawned it: the file played out first.
                Err(_) => {
                    sinks.voices.remove(&voice.id);
                    sinks.finished.insert(voice.id);
                }
            },
            None if !sinks.finished.contains(&voice.id) => {
                let Some(handle) = sinks.handles.get(&voice.file).cloned() else { continue };
                // A looped voice (an emitter's) goes on until the mixer lets it go; the others end with their file.
                let base = if voice.looped { PlaybackSettings::LOOP } else { PlaybackSettings::DESPAWN };
                let settings = base
                    .with_volume(Volume::Linear(voice.gain))
                    .with_spatial(voice.position.is_some())
                    .with_spatial_scale(SpatialScale::new(1.0));
                let name = Name::new(format!("voice #{} {}", voice.id, voice.sound));
                let e = commands.spawn((name, AudioPlayer(handle), settings, Transform::from_translation(place), VoiceSink { bus: voice.bus })).id();
                sinks.voices.insert(voice.id, e);
            }
            None => {}
        }
    }
}

/// A looping sink for every stem of the music, at its gain through the music bus; a track's stems
/// start together.
fn stems(
    mut commands: Commands,
    music: Res<Music>,
    settings: Option<Res<AudioSettings>>,
    server: Res<AssetServer>,
    time: Res<Time<Real>>,
    mut sinks: ResMut<Sinks>,
    mut players: Query<&mut AudioSink, With<StemSink>>,
) {
    let keys: BTreeSet<(String, u32)> = music.stems.iter().map(|s| (s.track.clone(), s.index)).collect();
    let gone: Vec<(String, u32)> = sinks.stems.keys().filter(|k| !keys.contains(*k)).cloned().collect();
    for key in gone {
        if let Some(e) = sinks.stems.remove(&key) {
            commands.entity(e).try_despawn();
        }
    }
    let bus = settings.map_or(1.0, |s| s.volume(Bus::Music.name()));
    let dt = time.delta_secs();
    let tracks: BTreeSet<&str> = music.stems.iter().map(|s| s.track.as_str()).collect();
    for track in tracks {
        let mine: Vec<_> = music.stems.iter().filter(|s| s.track == track).collect();
        let started = mine.iter().any(|s| sinks.stems.contains_key(&(s.track.clone(), s.index)));
        if !started {
            let handles: Option<Vec<Handle<AudioSource>>> = mine.iter().map(|s| sinks.handles.get(&s.file).cloned()).collect();
            let Some(handles) = handles.filter(|h| h.iter().all(|h| server.is_loaded(h))) else { continue };
            for (stem, handle) in mine.iter().zip(handles) {
                let settings = PlaybackSettings::LOOP.with_volume(Volume::Linear(stem.gain * bus));
                let name = Name::new(format!("music {} stem {}", stem.track, stem.index));
                let e = commands.spawn((name, AudioPlayer(handle), settings, StemSink)).id();
                sinks.stems.insert((stem.track.clone(), stem.index), e);
            }
            continue;
        }
        for stem in mine {
            if let Some(mut sink) = sinks.stems.get(&(stem.track.clone(), stem.index)).and_then(|e| players.get_mut(*e).ok()) {
                let now = sink.volume().to_linear();
                sink.set_volume(Volume::Linear(slew(now, stem.gain * bus, dt)));
            }
        }
    }
}

/// While the game is paused, the simulation's voices hold; music and menu sounds go on.
fn hold_while_paused(time: Res<Time<Virtual>>, sinks: Query<(&VoiceSink, Option<&AudioSink>, Option<&SpatialAudioSink>)>) {
    let paused = time.is_paused();
    for (voice, sink, spatial) in &sinks {
        if voice.bus == Bus::Ui {
            continue;
        }
        if let Some(sink) = sink {
            if paused && !sink.is_paused() {
                sink.pause();
            } else if !paused && sink.is_paused() {
                sink.play();
            }
        }
        if let Some(sink) = spatial {
            if paused && !sink.is_paused() {
                sink.pause();
            } else if !paused && sink.is_paused() {
                sink.play();
            }
        }
    }
}
