//! The mixer: which sounds play now, from where, how loud. It decides on the simulation's
//! clock (a `step` runs it, a pause stops it; menu sounds keep real time), the same with a window
//! and without one; playback (feature `playback`) only carries out what it decided.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use ai_core::journal::{self, JournalKind};
use ai_core::AiId;
use bevy::prelude::*;
use shards_content::db::closest;
use shards_content::ContentDb;
use shards_settings::AudioSettings;

use crate::def::{AudioDef, Bus, Falloff, SoundDef, DEFAULT_VOICES};
use crate::length::file_seconds;
use crate::subtitles::{self, Subtitles};

/// Where a sound is heard from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SoundAt {
    /// The same everywhere (menus); a spatial sound played so is heard at full volume.
    Everywhere,
    /// From an entity, following it while it exists.
    Entity(Entity),
    /// From a point; `source` is whose sound it is (an attacker's warning lands elsewhere).
    Point { position: Vec3, source: Option<Entity> },
}

/// Plays sound `sound` (a [`SoundDef`] id) as soon as commands apply:
/// `commands.trigger(PlaySound::new("coin", SoundAt::Entity(hero)))`.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct PlaySound {
    pub sound: String,
    pub at: SoundAt,
}

impl PlaySound {
    pub fn new(sound: impl Into<String>, at: SoundAt) -> Self {
        Self { sound: sound.into(), at }
    }
}

/// The ear: the body this machine plays (the HUD's [`viewer`](shards_ui::viewer)) hears, facing
/// the way an [`AudioListener`] (the camera) faces, else the way the body does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Listener {
    pub position: Vec3,
    pub rotation: Quat,
    pub body: Option<Entity>,
}

/// Marks what the ears face with: the game's camera. Without one, they face with the body.
#[derive(Component, Clone, Copy, Debug, Default)]
pub struct AudioListener;

/// A sound playing.
#[derive(Clone, Debug, PartialEq)]
pub struct Voice {
    /// Numbered from 1 in the order sounds started.
    pub id: u64,
    pub sound: String,
    /// The file playing, of the sound's `files`.
    pub file: String,
    pub bus: Bus,
    pub priority: u8,
    /// What it comes from (it follows that entity while it exists), and that entity's id.
    pub source: Option<Entity>,
    pub source_id: Option<AiId>,
    /// Where it is heard from; `None`: everywhere the same.
    pub position: Option<Vec3>,
    pub falloff: Option<Falloff>,
    /// The sound's own volume.
    pub volume: f32,
    /// When it started, on its clock (the simulation's; real time on the `Ui` bus).
    pub started: f64,
    /// Seconds it plays.
    pub length: f64,
    /// How loud it is heard now: its volume × the bus's × distance (0 out of earshot).
    pub gain: f32,
    pub distance: Option<f32>,
    /// Where it comes from, seen from the listener (−z ahead, +x right): what playback pans by.
    pub direction: Option<Vec3>,
}

/// A sound that was not heard, and why.
#[derive(Clone, Debug, PartialEq)]
pub struct Dropped {
    pub sound: String,
    pub reason: String,
    /// Simulation seconds when it was dropped.
    pub at: f64,
}

/// What [`play`] did.
#[derive(Clone, Debug, PartialEq)]
pub enum Played {
    /// It plays as voice `id`.
    Started { id: u64, stole: Option<String> },
    /// It is not heard: out of earshot, or weaker than every voice when all are taken.
    Dropped(String),
}

/// How many dropped sounds the mixer remembers for the `audio` tool.
const DROPS_KEPT: usize = 16;

/// The mixer's state: the voices playing and the ear.
#[derive(Resource, Default)]
pub struct Mixer {
    voices: Vec<Voice>,
    next_id: u64,
    /// Which of a sound's files plays next.
    turns: BTreeMap<String, usize>,
    pub listener: Option<Listener>,
    drops: VecDeque<Dropped>,
    /// Sounds started and dropped since the game began.
    pub started: u64,
    pub dropped: u64,
    /// Seconds of simulation and of real time the voices are timed by.
    sim_seconds: f64,
    real_seconds: f64,
    /// Sound files' lengths, read once.
    lengths: BTreeMap<String, f64>,
    /// Sounds named but not defined, journaled once each.
    unknown: BTreeSet<String>,
}

impl Mixer {
    /// The voices playing, oldest first.
    pub fn voices(&self) -> &[Voice] {
        &self.voices
    }

    /// The sounds dropped lately, oldest first.
    pub fn drops(&self) -> impl Iterator<Item = &Dropped> {
        self.drops.iter()
    }

    pub(crate) fn now(&self, bus: Bus) -> f64 {
        if bus == Bus::Ui { self.real_seconds } else { self.sim_seconds }
    }

    fn drop_sound(&mut self, sound: &str, reason: String) -> Played {
        self.dropped += 1;
        if self.drops.len() == DROPS_KEPT {
            self.drops.pop_front();
        }
        self.drops.push_back(Dropped { sound: sound.to_owned(), reason: reason.clone(), at: self.sim_seconds });
        Played::Dropped(reason)
    }
}

/// Voices the game may play at once (its [`AudioDef`]).
pub fn voice_limit(world: &World) -> u32 {
    audio_def(world).map_or(DEFAULT_VOICES, |d| d.voices)
}

/// The game's mixer definition, if it has one.
pub fn audio_def(world: &World) -> Option<&AudioDef> {
    let db = world.get_resource::<ContentDb>()?;
    db.iter::<AudioDef>().next().map(|(_, def)| def)
}

/// What a voice of `bus` is heard at before distance: the player's volume of the bus.
pub fn bus_volume(world: &World, bus: Bus) -> f32 {
    world.get_resource::<AudioSettings>().map_or(1.0, |s| s.volume(bus.name()))
}

/// Plays `sound` from `at`, now: a voice of its own, the oldest of its own when it has its
/// `max_instances`, the weakest voice when all are taken and it is stronger, or nothing (out
/// of earshot, or weaker than every voice). An unknown sound is an error naming the closest.
pub fn play(world: &mut World, sound: &str, at: SoundAt) -> Result<Played, String> {
    let def = world.get_resource::<ContentDb>().and_then(|db| db.get::<SoundDef>(sound)).cloned();
    let Some(def) = def else {
        let db = world.get_resource::<ContentDb>();
        let known: Vec<&str> = db.map(|db| db.iter::<SoundDef>().map(|(id, _)| id).collect()).unwrap_or_default();
        let hint = closest(sound, known.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        return Err(format!("no sound `{sound}`{hint}: content `*.sound.ron` defines sounds; {} known", known.len()));
    };
    if def.files.is_empty() {
        return Err(format!("sound `{sound}` names no file"));
    }
    refresh_listener(world);
    let limit = voice_limit(world) as usize;
    let (source, position) = match at {
        SoundAt::Everywhere => (None, None),
        SoundAt::Entity(e) => (Some(e), world.get::<Transform>(e).map(|t| t.translation)),
        SoundAt::Point { position, source } => (source, Some(position)),
    };
    let source_id = source.and_then(|e| world.get::<AiId>(e).copied());
    let position = position.filter(|_| def.spatial.is_some());
    let mut voice = Voice {
        id: 0,
        sound: sound.to_owned(),
        file: String::new(),
        bus: def.bus,
        priority: def.priority,
        source: if matches!(at, SoundAt::Entity(_)) { source } else { None },
        source_id,
        position,
        falloff: def.spatial,
        volume: def.volume,
        started: 0.0,
        length: 0.0,
        gain: 0.0,
        distance: None,
        direction: None,
    };
    let bus = bus_volume(world, def.bus);
    let listener = world.resource::<Mixer>().listener;
    hear(&mut voice, listener, bus);
    let root = shards_content::load::asset_root(world);
    let mut mixer = world.resource_mut::<Mixer>();
    if let (Some(distance), Some(falloff)) = (voice.distance, voice.falloff)
        && distance >= falloff.far
    {
        return Ok(mixer.drop_sound(sound, format!("out of earshot: {distance:.1} m, heard to {:.0} m", falloff.far)));
    }
    let (mut stole, mut taken) = (None, None);
    if let Some(max) = def.max_instances {
        let own: Vec<usize> = mixer.voices.iter().enumerate().filter(|(_, v)| v.sound == sound).map(|(i, _)| i).collect();
        if own.len() >= max as usize {
            let oldest = mixer.voices.remove(own[0]);
            taken = Some(oldest.id);
            stole = Some(format!("its own #{} (max {max})", oldest.id));
        }
    }
    if stole.is_none() && mixer.voices.len() >= limit {
        // The weakest: lowest priority, then quietest, then oldest.
        let weakest = mixer
            .voices
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.priority.cmp(&b.priority).then(a.gain.total_cmp(&b.gain)).then(a.id.cmp(&b.id)))
            .map(|(i, v)| (i, v.priority, v.gain, v.sound.clone(), v.id));
        match weakest {
            Some((i, priority, gain, name, id)) if (voice.priority, voice.gain) > (priority, gain) => {
                mixer.voices.remove(i);
                taken = Some(id);
                stole = Some(format!("{name} #{id} (voice limit {limit})"));
            }
            _ => {
                return Ok(mixer.drop_sound(sound, format!("all {limit} voices play sounds as strong or stronger")));
            }
        }
    }
    let turn = mixer.turns.entry(sound.to_owned()).or_default();
    voice.file = def.files[*turn % def.files.len()].clone();
    *turn = (*turn + 1) % def.files.len();
    let file = voice.file.clone();
    let length = match mixer.lengths.get(&file) {
        Some(length) => *length,
        // Content's check reported a file that does not play; it is over at once.
        None => *mixer.lengths.entry(file.clone()).or_insert_with(|| file_seconds(&root.join(&file)).unwrap_or(0.0)),
    };
    mixer.next_id += 1;
    mixer.started += 1;
    voice.id = mixer.next_id;
    voice.started = mixer.now(voice.bus);
    voice.length = length;
    let id = voice.id;
    mixer.voices.push(voice);
    // What the taken voice said is no longer heard; what this one says shows.
    if let Some(taken) = taken {
        subtitles::cut(world, taken);
    }
    subtitles::start(world, sound, &def, id, length);
    Ok(Played::Started { id, stole })
}

/// [`play`] for what the engine hears (a cue, an event): an unknown sound is journaled once and
/// otherwise ignored.
pub(crate) fn play_quietly(world: &mut World, sound: &str, at: SoundAt, why: &str) {
    if let Err(e) = play(world, sound, at)
        && world.resource_mut::<Mixer>().unknown.insert(sound.to_owned())
    {
        journal::emit(world, JournalKind::Warn, format!("audio: {why}: {e}"));
    }
}

/// Where the ear is now.
pub fn listener(world: &World) -> Option<Listener> {
    let body = shards_ui::viewer(world);
    let mut ears = world.try_query_filtered::<&Transform, With<AudioListener>>();
    let ears = ears.as_mut().and_then(|q| q.iter(world).next().copied());
    let body_at = body.and_then(|b| world.get::<Transform>(b).copied());
    let position = body_at.or(ears)?.translation;
    let rotation = ears.or(body_at).map_or(Quat::IDENTITY, |t| t.rotation);
    Some(Listener { position, rotation, body })
}

fn refresh_listener(world: &mut World) {
    let ear = listener(world);
    world.resource_mut::<Mixer>().listener = ear;
}

/// Distance, direction and gain of `voice` heard from `listener` through a bus at `bus`.
fn hear(voice: &mut Voice, listener: Option<Listener>, bus: f32) {
    let mut gain = voice.volume * bus;
    voice.distance = None;
    voice.direction = None;
    if let (Some(at), Some(ear)) = (voice.position, listener) {
        let offset = at - ear.position;
        let distance = offset.length();
        voice.distance = Some(distance);
        voice.direction = Some(if distance > 1e-3 { ear.rotation.inverse() * (offset / distance) } else { Vec3::NEG_Z });
        if let Some(falloff) = voice.falloff {
            gain *= falloff.gain(distance);
        }
    }
    voice.gain = gain;
}

/// `FixedPostUpdate`: one tick of simulation time passes for the voices (the step, not the fixed
/// clock's delta, which is the whole jump after a snapshot load).
pub(crate) fn tick_clock(mut mixer: ResMut<Mixer>, time: Res<Time<Fixed>>) {
    mixer.sim_seconds += time.timestep().as_secs_f64();
}

/// `Update`: real time passes for the menu voices.
pub(crate) fn real_clock(mut mixer: ResMut<Mixer>, time: Res<Time<Real>>) {
    mixer.real_seconds = time.elapsed_secs_f64();
}

/// Voices that played out are over; the others follow their sources and are heard again from
/// where the ear is now, through the buses' volumes now.
pub fn refresh(world: &mut World) {
    refresh_listener(world);
    let follow: Vec<(u64, Vec3)> = world
        .resource::<Mixer>()
        .voices
        .iter()
        .filter_map(|v| Some((v.id, world.get::<Transform>(v.source?)?.translation)))
        .collect();
    let buses: BTreeMap<Bus, f32> = Bus::ALL.iter().map(|b| (*b, bus_volume(world, *b))).collect();
    let mut mixer = world.resource_mut::<Mixer>();
    let (sim, real) = (mixer.sim_seconds, mixer.real_seconds);
    mixer.voices.retain(|v| (if v.bus == Bus::Ui { real } else { sim }) - v.started < v.length);
    let listener = mixer.listener;
    for voice in &mut mixer.voices {
        if let Some((_, at)) = follow.iter().find(|(id, _)| *id == voice.id)
            && voice.position.is_some()
        {
            voice.position = Some(*at);
        }
        hear(voice, listener, buses[&voice.bus]);
    }
    if let Some(mut lines) = world.get_resource_mut::<Subtitles>() {
        subtitles::expire(&mut lines, sim, real);
    }
}

/// Stops every voice (a new region, a loaded save: what played belongs to the world that went).
pub fn stop_all(world: &mut World) {
    world.resource_mut::<Mixer>().voices.clear();
    subtitles::clear(world);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn voice(position: Option<Vec3>) -> Voice {
        Voice {
            id: 1,
            sound: "slam".into(),
            file: "slam.wav".into(),
            bus: Bus::Sfx,
            priority: 100,
            source: None,
            source_id: None,
            position,
            falloff: Some(Falloff { near: 1.0, far: 11.0 }),
            volume: 0.8,
            started: 0.0,
            length: 1.0,
            gain: 0.0,
            distance: None,
            direction: None,
        }
    }

    #[test]
    fn a_voice_is_heard_by_distance_and_from_the_ears_side() {
        // The ear faces −x: a sound at −x is straight ahead, one at +z to its left.
        let ear = Listener { position: Vec3::ZERO, rotation: Quat::from_rotation_y(std::f32::consts::FRAC_PI_2), body: None };
        let mut ahead = voice(Some(Vec3::new(-6.0, 0.0, 0.0)));
        hear(&mut ahead, Some(ear), 0.5);
        assert_eq!(ahead.distance, Some(6.0));
        assert!((ahead.gain - 0.8 * 0.5 * 0.25).abs() < 1e-6, "{}", ahead.gain);
        assert!(ahead.direction.unwrap().abs_diff_eq(Vec3::NEG_Z, 1e-5), "{:?}", ahead.direction);
        let mut left = voice(Some(Vec3::new(0.0, 0.0, 3.0)));
        hear(&mut left, Some(ear), 1.0);
        assert!(left.direction.unwrap().abs_diff_eq(Vec3::NEG_X, 1e-5), "{:?}", left.direction);
    }

    #[test]
    fn a_voice_without_a_place_is_heard_whole() {
        let mut menu = voice(None);
        hear(&mut menu, None, 0.5);
        assert_eq!((menu.gain, menu.distance, menu.direction), (0.4, None, None));
    }
}
