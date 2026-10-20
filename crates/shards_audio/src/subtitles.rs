//! Subtitles (U4): the line a sound has (a voice's words), shown while it plays.
//!
//! - **Content.** A [`SoundDef`] with `subtitle: "@warden-gate"` (and `speaker: "@warden"`) has a
//!   line. When the mixer starts it (heard: not dropped, not out of earshot), the line shows for the
//!   sound's length plus [`LINGER_S`], at least [`MIN_SHOWN_S`], on the voice's clock (the
//!   simulation's; real time on the `Ui` bus), so a pause holds it.
//! - **Shown by the game's UI.** A widget bound to the source `subtitles` (`Hint(bind:
//!   "subtitles")`) shows the newest lines, oldest first, one a line: `Warden: The gate holds.`;
//!   `subtitles:3` shows three ([`SHOWN`] without a number). Messages are resolved when the line is
//!   read, so a language changed in the menu applies to the lines showing. With
//!   `accessibility.subtitles` off the source is empty.
//! - **The same line twice** (two guards bark it) shows once, for the longer of the two.
//! - **A voice taken** by a stronger sound takes its line with it, unless another voice says it too,
//!   and stopping every voice takes them all: a line is what is heard.
//! - **Agents** read each line as it starts in the journal (`[local] subtitle: Warden: The gate
//!   holds.`) whatever the setting, and the lines showing in `audio`.
//! - **This machine's**, like the mixer: never in a snapshot or the replay hash.

use ai_core::journal::{self, JournalKind};
use bevy::prelude::*;
use shards_settings::AccessibilitySettings;
use shards_ui::UiValue;

use crate::def::{Bus, SoundDef};
use crate::mixer::Mixer;

/// Seconds a line stays after its sound ends, to finish reading it.
pub const LINGER_S: f64 = 0.75;
/// Seconds a line shows at least: a short bark is still read.
pub const MIN_SHOWN_S: f64 = 2.0;
/// Lines the source shows without a number (`subtitles`).
pub const SHOWN: usize = 2;
/// Lines kept at most: the oldest goes for a new one.
pub const KEPT: usize = 8;

/// A line showing.
#[derive(Clone, Debug, PartialEq)]
pub struct SubtitleLine {
    /// The sound it belongs to.
    pub sound: String,
    /// The voices that say it (two guards barking it are one line).
    pub voices: Vec<u64>,
    /// As content writes them: text, or a message `@id`.
    pub speaker: Option<String>,
    pub text: String,
    /// Whose clock it goes by.
    pub bus: Bus,
    /// When it goes, on that clock.
    pub until: f64,
}

/// The lines showing, oldest first.
#[derive(Resource, Default, Debug)]
pub struct Subtitles {
    lines: Vec<SubtitleLine>,
}

impl Subtitles {
    /// The lines showing, oldest first.
    pub fn lines(&self) -> &[SubtitleLine] {
        &self.lines
    }
}

/// Whether the player wants subtitles (`accessibility.subtitles`; on without the settings module).
pub fn enabled(world: &World) -> bool {
    world.get_resource::<AccessibilitySettings>().is_none_or(|a| a.subtitles)
}

/// `line` in the language played: `Warden: The gate holds.`, or the text alone.
pub fn line_text(world: &World, line: &SubtitleLine) -> String {
    let text = shards_locale::resolve(world, &line.text);
    match &line.speaker {
        Some(speaker) => format!("{}: {text}", shards_locale::resolve(world, speaker)),
        None => text,
    }
}

/// The newest `max` lines showing, oldest first, in the language played; none with the setting off.
pub fn shown(world: &World, max: usize) -> Vec<String> {
    let Some(subtitles) = world.get_resource::<Subtitles>().filter(|_| enabled(world)) else {
        return Vec::new();
    };
    let lines = subtitles.lines();
    lines[lines.len().saturating_sub(max)..].iter().map(|l| line_text(world, l)).collect()
}

/// The source `subtitles[:max]`: the lines showing, one a line (empty when none).
pub(crate) fn source(world: &World, arg: &str) -> Option<UiValue> {
    let max = arg.trim().parse::<usize>().ok().filter(|n| *n > 0).unwrap_or(SHOWN).min(KEPT);
    Some(UiValue::Text(shown(world, max).join("\n")))
}

/// The mixer started voice `voice` of `sound` (`def`), `length` seconds long: its line, if it has
/// one, shows from now.
pub(crate) fn start(world: &mut World, sound: &str, def: &SoundDef, voice: u64, length: f64) {
    let Some(text) = def.subtitle.clone() else { return };
    let until = world.resource::<Mixer>().now(def.bus) + (length + LINGER_S).max(MIN_SHOWN_S);
    let line = SubtitleLine { sound: sound.to_owned(), voices: vec![voice], speaker: def.speaker.clone(), text, bus: def.bus, until };
    let said = line_text(world, &line);
    let mut subtitles = world.get_resource_or_init::<Subtitles>();
    match subtitles.lines.iter_mut().find(|l| l.speaker == line.speaker && l.text == line.text) {
        Some(same) => {
            same.until = same.until.max(line.until);
            same.voices.push(voice);
        }
        None => {
            if subtitles.lines.len() == KEPT {
                subtitles.lines.remove(0);
            }
            subtitles.lines.push(line);
        }
    }
    journal::emit(world, JournalKind::Local, format!("subtitle: {said}"));
}

/// Voice `voice` was taken by another sound: its line goes, unless another voice says it too.
pub(crate) fn cut(world: &mut World, voice: u64) {
    if let Some(mut subtitles) = world.get_resource_mut::<Subtitles>() {
        for line in &mut subtitles.lines {
            line.voices.retain(|v| *v != voice);
        }
        subtitles.lines.retain(|l| !l.voices.is_empty());
    }
}

/// Lines whose time is up go (each by its bus's clock).
pub(crate) fn expire(subtitles: &mut Subtitles, sim: f64, real: f64) {
    subtitles.lines.retain(|l| (if l.bus == Bus::Ui { real } else { sim }) < l.until);
}

/// Every line goes (every voice stopped).
pub(crate) fn clear(world: &mut World) {
    if let Some(mut subtitles) = world.get_resource_mut::<Subtitles>() {
        subtitles.lines.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(text: &str, bus: Bus, until: f64) -> SubtitleLine {
        SubtitleLine { sound: "s".into(), voices: vec![1], speaker: None, text: text.into(), bus, until }
    }

    #[test]
    fn each_line_goes_by_its_own_clock() {
        let mut subtitles = Subtitles { lines: vec![line("sim", Bus::Voice, 2.0), line("menu", Bus::Ui, 2.0)] };
        expire(&mut subtitles, 1.9, 5.0);
        assert_eq!(subtitles.lines().iter().map(|l| l.text.as_str()).collect::<Vec<_>>(), ["sim"]);
        expire(&mut subtitles, 2.0, 0.0);
        assert!(subtitles.lines().is_empty(), "a line goes at its `until`");
    }

    #[test]
    fn a_line_without_a_speaker_is_its_text() {
        let world = World::new();
        assert_eq!(line_text(&world, &line("Hold the gate.", Bus::Voice, 1.0)), "Hold the gate.");
        let spoken = SubtitleLine { speaker: Some("Warden".into()), ..line("Hold the gate.", Bus::Voice, 1.0) };
        assert_eq!(line_text(&world, &spoken), "Warden: Hold the gate.");
    }
}
