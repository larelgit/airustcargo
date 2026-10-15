//! Music in layers: a track's stems loop together and the director turns each up or down by the
//! layer the game is in. Layers go from calm to intense; the most intense whose condition holds
//! is wanted. A more intense one comes at once, a calmer one only after it has been wanted for
//! the track's `hold`, so a fight that pauses for breath keeps its music. The director decides on
//! the simulation's clock, after each tick, from conditions that only read the world: the same in
//! a window and headless, on a host and on a co-op client.

use std::collections::BTreeSet;

use ai_core::journal::{self, JournalKind};
use ai_core::AiConditions;
use bevy::prelude::*;
use shards_content::db::closest;
use shards_content::ContentDb;

use crate::def::MusicDef;
use crate::mixer::audio_def;

/// What the music does: the track, its layer, and every stem heard or fading. Saved in snapshots,
/// so a session loaded from one hears what it heard.
#[derive(Resource, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Resource, Default)]
pub struct Music {
    /// The track playing (a [`MusicDef`] id); `None`: silence.
    pub track: Option<String>,
    /// Its layer now; `None` until the director has chosen one.
    pub layer: Option<String>,
    /// A layer set with `music_set`, played whatever the conditions say.
    pub forced: Option<String>,
    /// Seconds in this layer.
    pub since: f32,
    /// Seconds a calmer layer has been wanted (it plays once this reaches the track's `hold`).
    pub calmer_for: f32,
    /// The calmer layer wanted last.
    pub calmer: Option<String>,
    /// The stems of the track, and of one before it while they fade out.
    pub stems: Vec<Stem>,
    /// The game's start track was taken (so silence chosen later stays).
    pub started: bool,
}

/// One stem of a track.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
pub struct Stem {
    pub track: String,
    /// Its place in the track's `stems`.
    pub index: u32,
    pub file: String,
    /// How loud it is now (0 to 1, before the music bus) and where it is going.
    pub gain: f32,
    pub target: f32,
}

/// Conditions named by tracks but not registered, journaled once each.
#[derive(Resource, Default)]
pub(crate) struct UnknownConditions(BTreeSet<String>);

/// How far short of its end a hold (seconds) or a fade (gain) may fall and be over: ticks added up
/// in `f32` come out a little off (120 ticks of 1/60 s are not quite 2 s), and both last a whole
/// number of ticks.
const SLACK: f32 = 1e-4;

/// Seconds a stem fades when its track is gone.
const LOST_FADE: f32 = 1.0;

/// `FixedPostUpdate`: one tick of the director (one step: after a snapshot load the fixed clock's
/// delta is the whole jump).
pub(crate) fn direct(world: &mut World) {
    let dt = ai_core::sim::tick_seconds(world);
    step(world, dt);
}

/// `dt` seconds of the director: the start track once content is there, the layer the game is in,
/// the stems' fades.
pub fn step(world: &mut World, dt: f32) {
    if !world.get_resource::<ContentDb>().is_some_and(ContentDb::is_loaded) {
        return;
    }
    if !world.resource::<Music>().started {
        world.resource_mut::<Music>().started = true;
        if let Some(start) = audio_def(world).and_then(|d| d.music.clone()) {
            // Content's check names a start track that is no track. The journal says the
            // layer it starts in.
            let _ = switch(world, Some(&start), false);
        }
    }
    let track = world.resource::<Music>().track.clone();
    let def = track.as_deref().and_then(|t| world.resource::<ContentDb>().get::<MusicDef>(t)).cloned();
    if let (Some(track), Some(def)) = (track, def) {
        match_stems(world, &track, &def);
        choose_layer(world, &track, &def, dt);
        aim_stems(world, &track, &def);
    }
    fade(world, dt);
}

/// The current track's stems as its definition has them now (an edit keeps the ones it kept).
fn match_stems(world: &mut World, track: &str, def: &MusicDef) {
    let mut music = world.resource_mut::<Music>();
    let mine = |s: &Stem| s.track == track;
    let fits = music.stems.iter().filter(|s| mine(s)).count() == def.stems.len()
        && music.stems.iter().filter(|s| mine(s)).all(|s| def.stems.get(s.index as usize).is_some_and(|d| d.file == s.file));
    if fits {
        return;
    }
    let before: Vec<Stem> = music.stems.iter().filter(|s| mine(s)).cloned().collect();
    music.stems.retain(|s| !mine(s));
    for (index, stem) in def.stems.iter().enumerate() {
        let gain = before.iter().find(|s| s.index as usize == index && s.file == stem.file).map_or(0.0, |s| s.gain);
        music.stems.push(Stem { track: track.to_owned(), index: index as u32, file: stem.file.clone(), gain, target: 0.0 });
    }
}

/// The layer the conditions want: the most intense that holds, else the calm first.
fn wanted(world: &mut World, track: &str, def: &MusicDef) -> usize {
    let forced = world.resource::<Music>().forced.clone();
    if let Some(i) = forced.and_then(|f| def.layer_index(&f)) {
        return i;
    }
    for (i, layer) in def.layers.iter().enumerate().rev() {
        let Some(when) = &layer.when else { continue };
        match holds(world, when) {
            Ok(true) => return i,
            Ok(false) => {}
            Err(e) => {
                if world.resource_mut::<UnknownConditions>().0.insert(when.clone()) {
                    journal::emit(world, JournalKind::Warn, format!("music: {track} layer {} {e}", layer.name));
                }
            }
        }
    }
    0
}

/// Whether condition `when` (`name`, `!name`) holds, or why it cannot be told.
pub fn holds(world: &World, when: &str) -> Result<bool, String> {
    let (negated, name) = when.strip_prefix('!').map_or((false, when), |n| (true, n));
    let name = name.trim();
    let conditions = world.get_resource::<AiConditions>();
    match conditions.and_then(|c| c.0.get(name)) {
        Some(condition) => Ok(condition(world) != negated),
        None => {
            let mut known: Vec<&str> = conditions.map(|c| c.0.keys().map(String::as_str).collect()).unwrap_or_default();
            known.sort_unstable();
            let hint = closest(name, known.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
            Err(format!("waits on condition `{name}`, which this game has not registered{hint}; conditions: {}", known.join(", ")))
        }
    }
}

fn choose_layer(world: &mut World, track: &str, def: &MusicDef, dt: f32) {
    let want = wanted(world, track, def);
    let forced = world.resource::<Music>().forced.is_some();
    let mut music = world.resource_mut::<Music>();
    music.since += dt;
    let current = music.layer.as_deref().and_then(|l| def.layer_index(l));
    let next = match current {
        None => Some(want),
        Some(now) if want > now || (forced && want != now) => Some(want),
        Some(now) if want < now => {
            // Each calmer layer counts its own time: one wanted for a tick never plays.
            if music.calmer.as_deref() != Some(def.layers[want].name.as_str()) {
                music.calmer = Some(def.layers[want].name.clone());
                music.calmer_for = 0.0;
            }
            music.calmer_for += dt;
            (music.calmer_for + SLACK >= def.hold).then_some(want)
        }
        Some(_) => {
            music.calmer_for = 0.0;
            music.calmer = None;
            None
        }
    };
    let Some(next) = next.filter(|n| Some(*n) != current) else { return };
    let to = def.layers[next].name.clone();
    let line = match current {
        Some(from) => format!("music: {track} {} → {to}", def.layers[from].name),
        None => format!("music: {track} plays {to}"),
    };
    music.layer = Some(to);
    music.since = 0.0;
    music.calmer_for = 0.0;
    music.calmer = None;
    journal::emit(world, JournalKind::Local, line);
}

/// Every stem of the current track aims at its volume in the layer, or at silence.
fn aim_stems(world: &mut World, track: &str, def: &MusicDef) {
    let mut music = world.resource_mut::<Music>();
    let layer = music.layer.clone();
    for stem in music.stems.iter_mut().filter(|s| s.track == track) {
        let d = &def.stems[stem.index as usize];
        let on = layer.as_ref().is_some_and(|l| d.layers.contains(l));
        stem.target = if on { d.volume * def.volume } else { 0.0 };
    }
}

/// Stems move toward their targets at their track's fades; a stem of another track, faded out,
/// is gone.
fn fade(world: &mut World, dt: f32) {
    let db = world.resource::<ContentDb>();
    let fades: Vec<(f32, f32)> = world
        .resource::<Music>()
        .stems
        .iter()
        .map(|s| db.get::<MusicDef>(&s.track).map_or((LOST_FADE, LOST_FADE), |d| (d.fade_in, d.fade_out)))
        .collect();
    let mut music = world.resource_mut::<Music>();
    let track = music.track.clone();
    for (stem, (fade_in, fade_out)) in music.stems.iter_mut().zip(fades) {
        if Some(&stem.track) != track.as_ref() {
            stem.target = 0.0;
        }
        stem.gain = if stem.gain < stem.target {
            if fade_in <= 0.0 { stem.target } else { (stem.gain + dt / fade_in).min(stem.target) }
        } else if fade_out <= 0.0 {
            stem.target
        } else {
            (stem.gain - dt / fade_out).max(stem.target)
        };
        if (stem.gain - stem.target).abs() < SLACK {
            stem.gain = stem.target;
        }
    }
    music.stems.retain(|s| Some(&s.track) == track.as_ref() || s.gain > 0.0);
}

/// Plays `track` (a [`MusicDef`] id) from the layer the game is in, the one before fading out;
/// `None` lets the music fade to silence. Says what changed.
pub fn set_track(world: &mut World, track: Option<&str>) -> Result<String, String> {
    switch(world, track, true)
}

fn switch(world: &mut World, track: Option<&str>, journal: bool) -> Result<String, String> {
    let current = world.resource::<Music>().track.clone();
    if current.as_deref() == track {
        return Ok(match track {
            Some(track) => format!("{track} plays already"),
            None => "no music plays already".to_owned(),
        });
    }
    if let Some(track) = track
        && world.resource::<ContentDb>().get::<MusicDef>(track).is_none()
    {
        let db = world.resource::<ContentDb>();
        let known: Vec<&str> = db.iter::<MusicDef>().map(|(id, _)| id).collect();
        let hint = closest(track, known.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        return Err(format!("no track `{track}`{hint}; tracks: {}", if known.is_empty() { "none".to_owned() } else { known.join(", ") }));
    }
    let mut music = world.resource_mut::<Music>();
    // A track chosen before the first tick stays: the game's start track does not replace it.
    music.started = true;
    music.track = track.map(str::to_owned);
    music.layer = None;
    music.forced = None;
    music.since = 0.0;
    music.calmer_for = 0.0;
    music.calmer = None;
    let line = match (current, track) {
        (Some(from), Some(to)) => format!("music: {from} → {to}"),
        (None, Some(to)) => format!("music: {to} starts"),
        (Some(from), None) => format!("music: {from} fades out"),
        (None, None) => unreachable!("handled above"),
    };
    if journal {
        journal::emit(world, JournalKind::Local, line.clone());
    }
    Ok(line.trim_start_matches("music: ").to_owned())
}

/// Plays layer `layer` of the current track whatever the conditions say; `None` hands the choice
/// back to them.
pub fn force_layer(world: &mut World, layer: Option<&str>) -> Result<String, String> {
    let Some(track) = world.resource::<Music>().track.clone() else {
        return Err("no music plays: start a track first (`music_set track=<id>`)".to_owned());
    };
    let def = world.resource::<ContentDb>().get::<MusicDef>(&track).ok_or_else(|| format!("track `{track}` is gone from content"))?;
    if let Some(layer) = layer {
        check_layer(&track, def, layer)?;
    }
    world.resource_mut::<Music>().forced = layer.map(str::to_owned);
    Ok(match layer {
        Some(layer) => format!("{track} keeps to {layer} until `music_set layer=auto`"),
        None => format!("{track} follows the game again"),
    })
}

/// Whether `layer` is a layer of `track` (defined by `def`); the error names the ones it has.
pub fn check_layer(track: &str, def: &MusicDef, layer: &str) -> Result<(), String> {
    if def.layer_index(layer).is_some() {
        return Ok(());
    }
    let names: Vec<&str> = def.layer_names().collect();
    let hint = closest(layer, names.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
    Err(format!("{track} has no layer `{layer}`{hint}; layers: {}", names.join(", ")))
}

/// `arena: combat for 6.2s; explore wanted 1.0/5.0s` (`no music` when none plays).
pub fn summary(world: &World) -> String {
    let Some(music) = world.get_resource::<Music>() else { return "no music".to_owned() };
    let Some(track) = &music.track else { return "no music".to_owned() };
    let mut out = format!("{track}: {} for {:.1}s", music.layer.as_deref().unwrap_or("starting"), music.since);
    if music.forced.is_some() {
        out.push_str(" (music_set)");
    }
    if let Some(calmer) = &music.calmer {
        let hold = world.get_resource::<ContentDb>().and_then(|db| db.get::<MusicDef>(track)).map_or(0.0, |d| d.hold);
        out.push_str(&format!("; {calmer} wanted {:.1}/{hold:.1}s", music.calmer_for));
    }
    out
}
