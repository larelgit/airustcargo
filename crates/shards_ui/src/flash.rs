//! Full-screen flashes (U2): a colour over the whole picture for a moment, a reality flash or the
//! pulses of a glitch, as the player's `accessibility.flashes` allows.
//!
//! A flash is content (`content/**/*.flash.ron`, kind `flash`):
//!
//! ```ron
//! {
//!     "reality": (color: "#fff4e0", alpha: 0.9, fade_in_s: 0.05, hold_s: 0.15, fade_out_s: 0.8),
//!     "crack": (color: "#b8e6ff", alpha: 0.5, fade_in_s: 0.1, hold_s: 2.2, fade_out_s: 0.4, pulses: 12, pulse_rate: 5.0),
//! }
//! ```
//!
//! - **Starting one.** [`flash`] from code, `commands.trigger(PlayFlash("reality".into()))` from a
//!   system or an observer, the dev tool `flash_play flash=reality`. One flash is on the screen at a
//!   time: a newer one replaces the one playing.
//! - **The flashes setting** (`accessibility.flashes`), read on every frame: `full` plays the flash as
//!   written; `reduced` makes it at most [`REDUCED_ALPHA`] strong, its pulses never faster than
//!   [`REDUCED_PULSE_RATE`] a second and half as deep, and its edges eased and at least
//!   [`REDUCED_EDGE_S`] long; `off` draws nothing. A flash is started and journaled at every level,
//!   so a game that plays a sound with it still can.
//! - **The clock is real time** (`Time<Real>`): a hit stop that holds the simulation's clock on a
//!   boss's last blow does not hold the flash that blow starts. A menu opened over it (the pause
//!   menu) draws above it, and the flash runs on under it.
//! - **Drawn** (with a window or offscreen) as one full-screen node above the HUD and under menus and
//!   modals, which takes no clicks. Headless a flash is started, timed and journaled, and nothing is
//!   drawn.
//! - **This machine's picture**: no `AiId`, not in a snapshot or the replay hash, and `flash_play` is
//!   a control tool that replays never record. What a flash goes with (its sound, a shader) is the
//!   game's.

use std::f32::consts::TAU;
use std::time::Duration;

use ai_core::journal::{self, JournalKind};
use bevy::math::ops;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::db::closest;
use shards_content::{parse_color, ContentDb, ContentDef, Refs};
use shards_settings::{AccessibilitySettings, FlashLevel};

/// The strongest a flash covers the picture with `accessibility.flashes = "reduced"`.
pub const REDUCED_ALPHA: f32 = 0.35;
/// The most pulses a second with `reduced`.
pub const REDUCED_PULSE_RATE: f32 = 2.0;
/// The shortest fade in or out with `reduced` (s): no flash pops on or off.
pub const REDUCED_EDGE_S: f32 = 0.15;
/// The longest flash the content check takes (s): longer is a typo, not a flash.
pub const MAX_LENGTH_S: f32 = 30.0;

/// A flash (kind `flash`): a colour that fades in over the whole picture, holds and fades out,
/// optionally dipping and coming back (`pulses`) from its start.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FlashDef {
    /// `#rrggbb` (or `#rgb`). How strong it is, is `alpha`.
    pub color: String,
    /// How much it covers the picture at its strongest, 0 to 1.
    pub alpha: f32,
    /// Seconds from nothing to `alpha`.
    pub fade_in_s: f32,
    /// Seconds at `alpha`.
    pub hold_s: f32,
    /// Seconds from `alpha` back to nothing.
    pub fade_out_s: f32,
    /// How many times the colour dips and comes back, from the start (a glitch); 0: none.
    pub pulses: u32,
    /// Pulses a second.
    pub pulse_rate: f32,
    /// How deep a dip goes, 0 to 1 of the strength (1: to nothing).
    pub pulse_depth: f32,
}

impl Default for FlashDef {
    fn default() -> Self {
        Self { color: "#ffffff".into(), alpha: 0.8, fade_in_s: 0.05, hold_s: 0.1, fade_out_s: 0.5, pulses: 0, pulse_rate: 6.0, pulse_depth: 1.0 }
    }
}

impl ContentDef for FlashDef {
    const KIND: &'static str = "flash";
    // Only what this machine shows: never in the replay check.
    const SIMULATION: bool = false;

    fn validate(&self, _id: &str, refs: &mut Refs) {
        match parse_color(&self.color) {
            Err(e) => refs.error(format!("color: {e}")),
            Ok(_) if self.color.len() == 9 => {
                refs.error(format!("color `{}`: give it as `#rrggbb`; how strong the flash is, is `alpha`", self.color));
            }
            Ok(_) => {}
        }
        if !(0.0..=1.0).contains(&self.alpha) {
            refs.error(format!("alpha: 0 to 1, not {}", self.alpha));
        }
        let times = [("fade_in_s", self.fade_in_s), ("hold_s", self.hold_s), ("fade_out_s", self.fade_out_s)];
        for (field, value) in times {
            if !(value >= 0.0 && value.is_finite()) {
                refs.error(format!("{field}: 0 or more seconds, not {value}"));
            }
        }
        let length = self.fade_in_s + self.hold_s + self.fade_out_s;
        if times.iter().all(|(_, v)| v.is_finite() && *v >= 0.0) && !(length > 0.0 && length <= MAX_LENGTH_S) {
            let length = (length * 100.0).round() / 100.0;
            refs.error(format!("fade_in_s + hold_s + fade_out_s is {length} s: a flash lasts more than 0 and at most {MAX_LENGTH_S} s"));
        }
        if self.pulses > 0 && !(self.pulse_rate > 0.0 && self.pulse_rate.is_finite()) {
            refs.error(format!("pulse_rate: pulses a second, above 0, not {}", self.pulse_rate));
        }
        if !(0.0..=1.0).contains(&self.pulse_depth) {
            refs.error(format!("pulse_depth: 0 to 1, not {}", self.pulse_depth));
        }
    }
}

/// A flash as a flashes level plays it ([`FlashDef::shaped`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlashShape {
    pub alpha: f32,
    pub fade_in_s: f32,
    pub hold_s: f32,
    pub fade_out_s: f32,
    pub pulses: u32,
    pub pulse_rate: f32,
    pub pulse_depth: f32,
    /// Its edges ease in and out (smoothstep) instead of running straight.
    pub eased: bool,
}

impl FlashDef {
    /// The flash as `level` plays it; `None` with `off`, which draws nothing.
    pub fn shaped(&self, level: FlashLevel) -> Option<FlashShape> {
        let written = FlashShape {
            alpha: self.alpha,
            fade_in_s: self.fade_in_s,
            hold_s: self.hold_s,
            fade_out_s: self.fade_out_s,
            pulses: self.pulses,
            pulse_rate: self.pulse_rate,
            pulse_depth: self.pulse_depth,
            eased: false,
        };
        match level {
            FlashLevel::Full => Some(written),
            FlashLevel::Reduced => Some(FlashShape {
                alpha: self.alpha.min(REDUCED_ALPHA),
                fade_in_s: self.fade_in_s.max(REDUCED_EDGE_S),
                fade_out_s: self.fade_out_s.max(REDUCED_EDGE_S),
                pulse_rate: self.pulse_rate.min(REDUCED_PULSE_RATE),
                pulse_depth: self.pulse_depth * 0.5,
                eased: true,
                ..written
            }),
            FlashLevel::Off => None,
        }
    }

    /// How long it lasts as `level` plays it (s); 0 with `off`.
    pub fn length(&self, level: FlashLevel) -> f32 {
        self.shaped(level).map_or(0.0, |s| s.length())
    }
}

impl FlashShape {
    /// Fade in, hold and fade out (s).
    pub fn length(&self) -> f32 {
        self.fade_in_s + self.hold_s + self.fade_out_s
    }

    /// How much it covers the picture `t` seconds after it started: 0 before it and from its end on.
    pub fn alpha_at(&self, t: f32) -> f32 {
        if !(0.0..self.length()).contains(&t) {
            return 0.0;
        }
        let ease = |x: f32| {
            let x = x.clamp(0.0, 1.0);
            if self.eased { x * x * (3.0 - 2.0 * x) } else { x }
        };
        let envelope = if t < self.fade_in_s {
            ease(t / self.fade_in_s)
        } else if t < self.fade_in_s + self.hold_s {
            1.0
        } else {
            ease(1.0 - (t - self.fade_in_s - self.hold_s) / self.fade_out_s)
        };
        // A dip: full at the start of each pulse, `1 - depth` halfway through it.
        let pulse = if self.pulses > 0 && t * self.pulse_rate < self.pulses as f32 {
            1.0 - self.pulse_depth * (0.5 - 0.5 * ops::cos(t * self.pulse_rate * TAU))
        } else {
            1.0
        };
        self.alpha * envelope * pulse
    }
}

/// The flash on this machine's screen: one at a time, the newest.
#[derive(Resource, Clone, Debug, Default, PartialEq)]
pub struct ScreenFlash(pub Option<Flashing>);

/// A flash that is playing.
#[derive(Clone, Debug, PartialEq)]
pub struct Flashing {
    /// The flash's id.
    pub id: String,
    pub def: FlashDef,
    /// `Time<Real>::elapsed` when it started.
    pub started: Duration,
}

impl Flashing {
    /// Seconds since it started, at real time `now`.
    pub fn age(&self, now: Duration) -> f32 {
        now.saturating_sub(self.started).as_secs_f32()
    }

    /// The colour over the picture at real time `now` as `level` plays it (`[r, g, b, a]` in sRGB,
    /// 0 to 1); `None` when nothing covers it (`off`, or ended).
    pub fn color_at(&self, now: Duration, level: FlashLevel) -> Option<[f32; 4]> {
        let shape = self.def.shaped(level)?;
        let age = self.age(now);
        if age >= shape.length() {
            return None;
        }
        let [r, g, b, _] = parse_color(&self.def.color).unwrap_or([255; 4]);
        let unit = |c: u8| f32::from(c) / 255.0;
        Some([unit(r), unit(g), unit(b), shape.alpha_at(age)])
    }
}

/// `commands.trigger(PlayFlash("reality".into()))`: starts flash `reality` (see [`flash`]). An
/// unknown id is a warning in the journal.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct PlayFlash(pub String);

/// Starts flash `id` now, in place of any playing. Answers what it plays as the player's setting
/// allows; an unknown id is an error naming the known ones.
pub fn flash(world: &mut World, id: &str) -> Result<String, String> {
    let db = world.get_resource::<ContentDb>().ok_or("this game has no content")?;
    let Some(def) = db.peek::<FlashDef>(id).cloned() else {
        let known: Vec<&str> = db.peek_ids::<FlashDef>().collect();
        let hint = closest(id, known.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        return Err(format!("no flash `{id}`{hint}: content `*.flash.ron` defines flashes; {} known", known.len()));
    };
    let level = level(world);
    let line = describe(id, &def, level);
    let started = world.get_resource::<Time<Real>>().map_or(Duration::ZERO, Time::elapsed);
    world.get_resource_or_init::<ScreenFlash>().0 = Some(Flashing { id: id.to_owned(), def, started });
    journal::emit(world, JournalKind::Local, format!("flash: {line}"));
    Ok(line)
}

/// The player's flashes level; `full` without settings.
pub fn level(world: &World) -> FlashLevel {
    world.get_resource::<AccessibilitySettings>().map_or(FlashLevel::Full, AccessibilitySettings::flash_level)
}

/// `reality: #fff4e0 at 0.9 for 1 s (in 0.05, hold 0.15, out 0.8)`, then how the setting plays it.
fn describe(id: &str, def: &FlashDef, level: FlashLevel) -> String {
    let n = |v: f32| format!("{}", (v * 100.0).round() / 100.0);
    let mut line = format!(
        "{id}: {} at {} for {} s (in {}, hold {}, out {})",
        def.color,
        n(def.alpha),
        n(def.length(FlashLevel::Full)),
        n(def.fade_in_s),
        n(def.hold_s),
        n(def.fade_out_s)
    );
    if def.pulses > 0 {
        line.push_str(&format!(", {} pulses at {}/s", def.pulses, n(def.pulse_rate)));
    }
    match def.shaped(level) {
        None => line.push_str(" · flashes off: not drawn"),
        Some(shape) if level == FlashLevel::Reduced => {
            line.push_str(&format!(" · flashes reduced: at {} for {} s", n(shape.alpha), n(shape.length())));
            if def.pulses > 0 {
                line.push_str(&format!(", pulses at {}/s", n(shape.pulse_rate)));
            }
        }
        Some(_) => {}
    }
    line
}

pub(crate) fn on_play(trigger: On<PlayFlash>, mut commands: Commands) {
    let id = trigger.event().0.clone();
    commands.queue(move |world: &mut World| {
        if let Err(e) = flash(world, &id) {
            journal::emit(world, JournalKind::Warn, format!("flash: {e}"));
        }
    });
}

/// `Update`: a flash that has ended (as the setting plays it) is gone.
pub(crate) fn end_flashes(time: Res<Time<Real>>, settings: Option<Res<AccessibilitySettings>>, mut flash: ResMut<ScreenFlash>) {
    let level = settings.map_or(FlashLevel::Full, |s| s.flash_level());
    let ended = flash.0.as_ref().is_some_and(|f| f.age(time.elapsed()) >= f.def.length(level));
    if ended {
        flash.0 = None;
    }
}

/// The node a flash is drawn with.
#[cfg(feature = "render")]
#[derive(Component)]
pub struct FlashOverlay;

/// Over the HUD (1), under menus (10 and up) and modals (100 and up).
#[cfg(feature = "render")]
const FLASH_Z: i32 = 5;

/// `PostUpdate`, before Bevy's layout: the flash's node in this frame's colour; none while no
/// flash covers the picture.
#[cfg(feature = "render")]
pub(crate) fn draw_flash(
    mut commands: Commands,
    time: Res<Time<Real>>,
    settings: Option<Res<AccessibilitySettings>>,
    flash: Res<ScreenFlash>,
    mut nodes: Query<(Entity, &mut BackgroundColor), With<FlashOverlay>>,
) {
    let level = settings.map_or(FlashLevel::Full, |s| s.flash_level());
    let color = flash.0.as_ref().and_then(|f| f.color_at(time.elapsed(), level)).map(|[r, g, b, a]| Color::srgba(r, g, b, a));
    match (color, nodes.single_mut()) {
        (None, Ok((node, _))) => commands.entity(node).despawn(),
        (None, Err(_)) => {}
        (Some(color), Ok((_, mut background))) => {
            if background.0 != color {
                background.0 = color;
            }
        }
        (Some(color), Err(_)) => {
            let screen = Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() };
            commands.spawn((Name::new("screen flash"), FlashOverlay, screen, BackgroundColor(color), GlobalZIndex(FLASH_Z), bevy::picking::Pickable::IGNORE));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crack() -> FlashDef {
        FlashDef { color: "#b8e6ff".into(), alpha: 0.6, fade_in_s: 0.1, hold_s: 2.0, fade_out_s: 0.4, pulses: 10, pulse_rate: 5.0, pulse_depth: 1.0 }
    }

    #[test]
    fn a_flash_fades_in_holds_and_fades_out() {
        let def = FlashDef { alpha: 0.8, fade_in_s: 0.1, hold_s: 0.2, fade_out_s: 0.4, ..default() };
        let s = def.shaped(FlashLevel::Full).unwrap();
        assert!((s.length() - 0.7).abs() < 1e-6);
        assert_eq!(s.alpha_at(-0.01), 0.0);
        assert_eq!(s.alpha_at(0.0), 0.0);
        assert!((s.alpha_at(0.05) - 0.4).abs() < 1e-5);
        assert_eq!(s.alpha_at(0.2), 0.8);
        assert!((s.alpha_at(0.5) - 0.4).abs() < 1e-5);
        assert_eq!(s.alpha_at(0.71), 0.0);
        // No fade in: at full strength from the first instant.
        let pop = FlashDef { fade_in_s: 0.0, ..def };
        assert_eq!(pop.shaped(FlashLevel::Full).unwrap().alpha_at(0.0), 0.8);
    }

    #[test]
    fn pulses_dip_to_the_depth_halfway_and_stop_after_their_count() {
        let s = crack().shaped(FlashLevel::Full).unwrap();
        // Pulse 3 runs 0.4..0.6 s: full at its start, nothing halfway.
        assert!((s.alpha_at(0.4) - 0.6).abs() < 1e-4);
        assert!(s.alpha_at(0.5) < 1e-4);
        // Ten pulses at 5 a second end at 2 s: the hold goes on steady.
        assert!((s.alpha_at(2.05) - 0.6).abs() < 1e-5);
    }

    #[test]
    fn reduced_caps_the_strength_the_pulse_rate_and_depth_and_eases_the_edges() {
        let s = crack().shaped(FlashLevel::Reduced).unwrap();
        assert_eq!((s.alpha, s.pulse_rate, s.pulse_depth, s.fade_in_s, s.eased), (REDUCED_ALPHA, REDUCED_PULSE_RATE, 0.5, REDUCED_EDGE_S, true));
        assert!((0..400).all(|i| s.alpha_at(i as f32 * 0.01) <= REDUCED_ALPHA));
        // Halfway through a pulse it keeps half its strength.
        assert!((s.alpha_at(1.25) - REDUCED_ALPHA * 0.5).abs() < 1e-4);
        // A flash that pops on fades in over 0.15 s, eased: a quarter in, a little over 15 %.
        let pop = FlashDef { fade_in_s: 0.0, alpha: 0.2, ..default() }.shaped(FlashLevel::Reduced).unwrap();
        assert!((pop.alpha_at(0.0375) - 0.2 * 0.15625).abs() < 1e-5);
        assert_eq!(crack().shaped(FlashLevel::Off), None);
        assert_eq!(crack().length(FlashLevel::Off), 0.0);
    }

    #[test]
    fn the_colour_is_the_flashs_with_the_strength_of_the_moment() {
        let playing = Flashing { id: "crack".into(), def: crack(), started: Duration::from_secs(10) };
        let at = |s: f64| Duration::from_secs_f64(10.0 + s);
        let [r, g, b, a] = playing.color_at(at(1.0), FlashLevel::Full).unwrap();
        assert_eq!((r, g, b), (184.0 / 255.0, 230.0 / 255.0, 1.0));
        assert!((a - 0.6).abs() < 1e-4);
        assert_eq!(playing.color_at(at(2.5), FlashLevel::Full), None, "ended");
        assert_eq!(playing.color_at(at(1.0), FlashLevel::Off), None);
        assert_eq!(playing.color_at(Duration::from_secs(9), FlashLevel::Full).map(|c| c[3]), Some(0.0), "not yet");
    }

    #[test]
    fn the_line_says_what_plays_at_the_players_level() {
        assert_eq!(describe("crack", &crack(), FlashLevel::Full), "crack: #b8e6ff at 0.6 for 2.5 s (in 0.1, hold 2, out 0.4), 10 pulses at 5/s");
        assert_eq!(
            describe("crack", &crack(), FlashLevel::Reduced),
            "crack: #b8e6ff at 0.6 for 2.5 s (in 0.1, hold 2, out 0.4), 10 pulses at 5/s · flashes reduced: at 0.35 for 2.55 s, pulses at 2/s"
        );
        assert!(describe("crack", &crack(), FlashLevel::Off).ends_with(" · flashes off: not drawn"));
    }
}
