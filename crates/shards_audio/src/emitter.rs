//! Emitters: a sound placed in the world.
//!
//! A [`SoundEmitter`] on an entity plays a looped sound (`looped: true`) from where the entity stands.
//! The mixer keeps one voice for it while the ears are within the sound's `far` and the emitter is
//! `playing`; it lets the voice go beyond `far`, when the emitter stops, or when the entity is gone,
//! and takes one again when the ears come back (the loop starts over). A level file puts one on a node
//! like any other component (`SoundEmitter = {"sound": "vent_hum"}`), so a sound is placed where an
//! artist put the empty.
//!
//! **Voices.** Emitters ask for their voices under the rules every sound follows: a free voice if
//! there is one, else the weakest voice by priority and then loudness if the emitter is stronger. So a
//! one-shot of a lower priority never takes a loop's voice, and a stronger one does and gives it back
//! when it has played (the emitter asks again at every refresh). An emitter that waits for a voice, or
//! is out of earshot, is not a "drop": `audio` says which.
//!
//! **Local.** Like the rest of the mixer: emitters are not `AiId` state (a level's are level content,
//! the same on every machine without being hashed), `playing` is not in a snapshot or the replay hash
//! (`replay_ignore`), and the journal lines are `local`. A game that stops an emitter as part of its
//! story sets `playing` again from its own state after a load.

use std::collections::{BTreeMap, BTreeSet};

use ai_core::journal::{self, JournalKind};
use bevy::prelude::*;
use shards_content::db::closest;
use shards_content::{ContentDb, PrefabDef, Refs};

use crate::def::SoundDef;
use crate::mixer::{bus_volume, start, voice_limit, Listener, Mixer, Played, SoundAt};
use crate::occlusion::name_of;

/// A looped sound placed at an entity's `Transform` (top-level, like physics colliders). Reflected, so a
/// level node, a prefab or `inspect` names it: `SoundEmitter = {"sound": "vent_hum"}`.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct SoundEmitter {
    /// A `*.sound.ron` id of a sound with `looped: true`.
    pub sound: String,
    /// Whether it sounds: off, the emitter holds no voice. A level node's emitter plays unless it says
    /// `"playing": false`.
    pub playing: bool,
}

impl Default for SoundEmitter {
    fn default() -> Self {
        Self { sound: String::new(), playing: true }
    }
}

impl SoundEmitter {
    /// An emitter that plays `sound`.
    pub fn new(sound: impl Into<String>) -> Self {
        Self { sound: sound.into(), playing: true }
    }

    /// The same, not playing until [`emitter_play`].
    pub fn stopped(mut self) -> Self {
        self.playing = false;
        self
    }
}

/// Starts emitter `emitter` playing (it takes a voice at the next refresh, if its sound is in earshot).
pub fn emitter_play(world: &mut World, emitter: Entity) -> Result<(), String> {
    set_playing(world, emitter, true)
}

/// Stops emitter `emitter`: its voice goes at the next refresh.
pub fn emitter_stop(world: &mut World, emitter: Entity) -> Result<(), String> {
    set_playing(world, emitter, false)
}

fn set_playing(world: &mut World, emitter: Entity, playing: bool) -> Result<(), String> {
    match world.get_mut::<SoundEmitter>(emitter) {
        Some(mut e) => {
            e.set_if_neq(SoundEmitter { playing, sound: e.sound.clone() });
            Ok(())
        }
        None => Err(format!("{} has no SoundEmitter", ai_core::label(world, emitter))),
    }
}

/// One emitter as the mixer reads it.
pub(crate) struct Seen {
    pub entity: Entity,
    /// What the AI calls it: its `Name` (a level node's) and its id, whichever it has, else the entity.
    pub name: String,
    /// The `Name` alone.
    pub node: Option<String>,
    /// `e20`, when it has an id.
    pub id: Option<String>,
    pub sound: String,
    pub playing: bool,
    pub at: Option<Vec3>,
}

/// What an emitter wants now.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum State {
    /// `playing` is off.
    Stopped,
    /// It cannot play, and why (an unknown sound, one that does not loop).
    Broken(String),
    /// Beyond the sound's `far`.
    Far { far: f32 },
    /// Within earshot: it wants a voice. `gain` is what it would be heard at before walls.
    Near { priority: u8, gain: f32 },
}

pub(crate) struct Assessed {
    pub state: State,
    /// From the ears, when both have a place.
    pub distance: Option<f32>,
}

/// Every emitter, in an order that does not depend on `Entity` (a window adds entities and shifts their
/// indices): by name, sound and place; the entity breaks the last tie. Which of two emitters at the same
/// place with the same name gets the last voice is not observable, and this is the machine's own sound,
/// never hashed, so the entity as the last resort costs nothing.
pub(crate) fn collect(world: &World) -> Vec<Seen> {
    let Some(mut query) = world.try_query::<(Entity, &SoundEmitter, Option<&Transform>)>() else { return Vec::new() };
    let mut seen: Vec<Seen> = query
        .iter(world)
        .map(|(entity, emitter, at)| Seen {
            entity,
            name: name_of(world, entity),
            node: world.get::<Name>(entity).map(|n| n.as_str().to_owned()),
            id: world.get::<ai_core::AiId>(entity).map(ToString::to_string),
            sound: emitter.sound.clone(),
            playing: emitter.playing,
            at: at.map(|t| t.translation),
        })
        .collect();
    seen.sort_by_cached_key(|s| (s.node.clone(), s.id.clone(), s.sound.clone(), s.at.map(|p| p.to_array().map(f32::to_bits)), s.entity));
    seen
}

/// What `s` wants, with the ears at `ear` (none: every sound is heard whole).
pub(crate) fn assess(world: &World, s: &Seen, ear: Option<Listener>) -> Assessed {
    let distance = ear.zip(s.at).map(|(ear, at)| (at - ear.position).length());
    let state = state_of(world, s, distance);
    Assessed { state, distance }
}

fn state_of(world: &World, s: &Seen, distance: Option<f32>) -> State {
    if !s.playing {
        return State::Stopped;
    }
    let db = world.get_resource::<ContentDb>();
    let Some(def) = db.and_then(|db| db.get::<SoundDef>(&s.sound)) else {
        let known: Vec<&str> = db.map(|db| db.iter::<SoundDef>().map(|(id, _)| id).collect()).unwrap_or_default();
        let hint = closest(&s.sound, known.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        let what = if s.sound.is_empty() { "it names no sound".to_owned() } else { format!("no sound `{}`{hint}", s.sound) };
        return State::Broken(what);
    };
    if !def.looped {
        return State::Broken(format!("`{}` does not loop: an emitter plays a sound with `looped: true`", s.sound));
    }
    let falloff = def.spatial;
    if falloff.is_some() && s.at.is_none() {
        return State::Broken("it has no Transform to be heard from".to_owned());
    }
    let mut gain = def.volume * bus_volume(world, def.bus);
    if let (Some(distance), Some(falloff)) = (distance, falloff) {
        if distance >= falloff.far {
            return State::Far { far: falloff.far };
        }
        gain *= falloff.gain(distance);
    }
    State::Near { priority: def.priority, gain }
}

/// `FixedPostUpdate` and `Update`, from [`refresh`](crate::refresh): emitters in earshot take voices,
/// the others let theirs go. Calling it twice with nothing changed starts nothing.
pub(crate) fn sync(world: &mut World) {
    let seen = collect(world);
    if seen.is_empty() && !world.resource::<Mixer>().voices.iter().any(|v| v.emitter.is_some()) {
        return;
    }
    let ear = world.resource::<Mixer>().listener;
    let assessed: Vec<Assessed> = seen.iter().map(|s| assess(world, s, ear)).collect();
    let index: BTreeMap<Entity, usize> = seen.iter().enumerate().map(|(i, s)| (s.entity, i)).collect();

    // Voices whose emitter is gone, stopped, changed its sound or is out of earshot are let go.
    let mut said = Vec::new();
    world.resource_mut::<Mixer>().voices.retain(|v| {
        let Some(owner) = v.emitter else { return true };
        let Some(&i) = index.get(&owner) else {
            said.push(format!("emitter {} ({}) is gone", owner, v.sound));
            return false;
        };
        let (s, a) = (&seen[i], &assessed[i]);
        match &a.state {
            State::Near { .. } if s.sound == v.sound => true,
            State::Near { .. } | State::Broken(_) => {
                said.push(format!("emitter {} ({}) lets go of {}", s.name, s.sound, v.sound));
                false
            }
            State::Stopped => {
                said.push(format!("emitter {} ({}) stopped", s.name, s.sound));
                false
            }
            State::Far { far } => {
                let at = a.distance.map(|d| format!("{d:.1} m, ")).unwrap_or_default();
                said.push(format!("emitter {} ({}) out of earshot ({at}heard to {far:.0} m)", s.name, s.sound));
                false
            }
        }
    });

    // Mistakes are said once, as an unknown sound is.
    let mut warned = Vec::new();
    for (s, a) in seen.iter().zip(&assessed) {
        if let State::Broken(why) = &a.state
            && world.resource_mut::<Mixer>().unknown.insert(format!("emitter {}: {why}", s.name))
        {
            warned.push(format!("audio: emitter {} ({}) cannot play: {why}", s.name, s.sound));
        }
    }

    // Emitters in earshot with no voice ask for one, the strongest first (priority, then loudness, then
    // their order above).
    let held: BTreeSet<Entity> = world.resource::<Mixer>().voices.iter().filter_map(|v| v.emitter).collect();
    let mut wanting: Vec<(usize, u8, f32)> = (0..seen.len())
        .filter(|i| !held.contains(&seen[*i].entity))
        .filter_map(|i| match assessed[i].state {
            State::Near { priority, gain } => Some((i, priority, gain)),
            _ => None,
        })
        .collect();
    wanting.sort_by(|a, b| b.1.cmp(&a.1).then(b.2.total_cmp(&a.2)).then(a.0.cmp(&b.0)));
    for (i, ..) in wanting {
        let s = &seen[i];
        let Some(def) = world.resource::<ContentDb>().get::<SoundDef>(&s.sound).cloned() else { continue };
        match start(world, &s.sound, &def, SoundAt::Entity(s.entity), 1.0, Some(s.entity)) {
            Ok(Played::Started { id, .. }) => said.push(format!("emitter {} ({}) in earshot: voice #{id}", s.name, s.sound)),
            // Waiting for a voice: `audio` says so, and it asks again at the next refresh.
            Ok(Played::Dropped(_)) | Err(_) => {}
        }
    }
    for line in warned {
        journal::emit(world, JournalKind::Warn, line);
    }
    for line in said {
        journal::emit(world, JournalKind::Local, line);
    }
}

/// One emitter's line for `audio`: `Hum.North vent_hum 4.1m heard #3 gain 0.30 (occluded: Glass ×0.60)`.
pub(crate) fn line(world: &World, s: &Seen, a: &Assessed) -> String {
    let at = a.distance.map(|d| format!(" {d:.1}m")).unwrap_or_default();
    let what = match &a.state {
        State::Stopped => "stopped".to_owned(),
        State::Broken(why) => format!("cannot play: {why}"),
        State::Far { far } => format!("out of earshot, heard to {far:.0} m"),
        State::Near { .. } => match world.resource::<Mixer>().voices().iter().find(|v| v.emitter == Some(s.entity)) {
            Some(v) => {
                let walls = if v.occlusion.by.is_empty() { String::new() } else { format!(" (occluded: {})", v.occlusion.describe()) };
                format!("heard #{} gain {:.2}{walls}", v.id, v.gain)
            }
            None => format!("waiting for a voice: all {} play sounds as strong or stronger", voice_limit(world)),
        },
    };
    format!("  {} {}{at} {what}", s.name, s.sound)
}

/// The emitter `query` names, for the tools: its `Name` (a level node's), its id (`e20`), or its sound
/// when only one emitter plays it.
pub(crate) fn find(world: &World, query: &str) -> Result<Entity, String> {
    let seen = collect(world);
    let query = query.trim();
    let by_name: Vec<&Seen> =
        seen.iter().filter(|s| s.name == query || s.node.as_deref() == Some(query) || s.id.as_deref() == Some(query)).collect();
    let found = match by_name.as_slice() {
        [one] => Some(*one),
        [] => {
            let by_sound: Vec<&Seen> = seen.iter().filter(|s| s.sound == query).collect();
            match by_sound.as_slice() {
                [one] => Some(*one),
                _ => None,
            }
        }
        many => {
            let ids: Vec<String> = many.iter().map(|s| s.id.clone().unwrap_or_else(|| format!("{}", s.entity))).collect();
            return Err(format!("{} emitters are called `{query}` ({}); name one by its id", many.len(), ids.join(", ")));
        }
    };
    match found {
        Some(s) => Ok(s.entity),
        None if seen.is_empty() => Err("no emitters here: put a `SoundEmitter` on an entity, or `SoundEmitter = {\"sound\": \"<id>\"}` on a level node".to_owned()),
        None => {
            let names: Vec<String> = seen.iter().map(|s| format!("{} ({})", s.name, s.sound)).collect();
            Err(format!("no emitter `{query}`; emitters: {}", names.join(", ")))
        }
    }
}

/// `line` for one emitter, as now.
pub(crate) fn describe(world: &World, entity: Entity) -> String {
    let ear = world.resource::<Mixer>().listener;
    collect(world).iter().find(|s| s.entity == entity).map(|s| line(world, s, &assess(world, s, ear))).unwrap_or_default()
}

/// A `SoundEmitter` in a prefab or a level node names `sound`: it must be a sound some file defines, and
/// it must loop (`field` says where the emitter is, for the message).
pub fn check_sound(refs: &mut Refs, sound: &str, field: &str) {
    refs.require::<SoundDef>(sound, field);
    if refs.iter::<SoundDef>().any(|(id, def)| id == sound && !def.looped) {
        refs.error(format!("{field}: `{sound}` does not loop: an emitter plays a sound with `looped: true`"));
    }
}

/// Prefabs whose `SoundEmitter` names a sound that is missing or does not loop.
pub(crate) fn check_prefab(prefab: &PrefabDef, _id: &str, refs: &mut Refs) {
    #[derive(serde::Deserialize)]
    struct Fields {
        sound: String,
    }
    for (name, value) in &prefab.components {
        let ours = match refs.type_registry() {
            Some(registry) => shards_content::prefab::resolve_component(registry, name)
                .is_ok_and(|reg| reg.type_id() == std::any::TypeId::of::<SoundEmitter>()),
            None => name == "SoundEmitter",
        };
        // A value that does not read, or names no sound, is the prefab's own error.
        if ours && let Ok(Fields { sound }) = ron::from_str::<Fields>(value.get_ron()) && !sound.is_empty() {
            check_sound(refs, &sound, "SoundEmitter.sound");
        }
    }
}
