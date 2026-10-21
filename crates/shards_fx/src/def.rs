//! The `effect` and `fx` content kinds: what an effect looks like (`*.effect.ron`), and which effect
//! the engine's events and the clips' cues play (one `*.fx.ron` per game).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use shards_content::db::closest;
use shards_content::{parse_color, ContentDef, Refs};

/// The most particles one effect keeps alive at once.
pub const MAX_LIVE: u32 = 4096;
/// The most particles one play starts at once.
pub const MAX_COUNT: u32 = 1024;
/// The longest an effect goes on emitting after it starts (s).
pub const MAX_DURATION: f32 = 10.0;
/// The longest a particle lives (s).
pub const MAX_LIFETIME: f32 = 30.0;

/// The shape each particle is drawn as.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ParticleShape {
    /// A soft round spot that always faces the camera: sparks, glows, dust.
    #[default]
    Quad,
    /// A small cube that tumbles: debris, chips of stone.
    Cube,
    /// A sliver of crystal that tumbles: ice, glass, bone.
    Shard,
    /// A flat round disc, `size` across, that lies level and never turns: a shockwave, a ripple.
    Disc,
}

impl ParticleShape {
    pub fn name(self) -> &'static str {
        match self {
            ParticleShape::Quad => "quad",
            ParticleShape::Cube => "cube",
            ParticleShape::Shard => "shard",
            ParticleShape::Disc => "disc",
        }
    }
}

/// How a play's strength changes an effect's particles ([`play_scaled`](crate::play_scaled): a clip cue's `strength`, a
/// blow's). Left out, their number and their size follow the strength in proportion and their speed does not.
///
/// ```ron
/// // 12 at strength 1, `int(6 + 6 × strength)` at any other, each faster and bigger with it.
/// (count: 12, strength: (count_per: 6.0, speed: true, size: true))
/// ```
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct StrengthResponse {
    /// Particles more for each 1 of strength above 1, fewer below: a play starts `count + count_per × (strength − 1)`,
    /// rounded down (`count: 12, count_per: 6.0` starts 15 at strength 1.5 and 9 at 0.6; `0.0` always `count`). Left
    /// out, `count × strength`, rounded, and at least one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub count_per: Option<f32>,
    /// The speed each particle starts at times the strength.
    pub speed: bool,
    /// The size each particle starts at times the strength.
    pub size: bool,
}

impl Default for StrengthResponse {
    fn default() -> Self {
        Self { count_per: None, speed: false, size: true }
    }
}

impl StrengthResponse {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// The particles a burst of `count` starts at `strength` (above 0).
    pub fn burst(&self, count: u32, strength: f32) -> u32 {
        match self.count_per {
            None => in_proportion(count, strength),
            Some(per) => {
                let n = count as f32 + per * (strength - 1.0);
                if n.is_finite() && n >= 1.0 { n as u32 } else { 0 }
            }
        }
    }

    /// What a particle's starting speed is multiplied by at `strength`.
    pub fn speed_at(&self, strength: f32) -> f32 {
        if self.speed { strength } else { 1.0 }
    }

    /// What a particle's starting size is multiplied by at `strength`.
    pub fn size_at(&self, strength: f32) -> f32 {
        if self.size { strength } else { 1.0 }
    }

    /// `strength: count +6, speed, size` (empty for the default).
    pub fn describe(&self) -> String {
        if self.is_default() {
            return String::new();
        }
        let mut what = vec![match self.count_per {
            Some(per) => format!("count {}{per}", if per < 0.0 { "" } else { "+" }),
            None => "count ×".to_owned(),
        }];
        if self.speed {
            what.push("speed".into());
        }
        if self.size {
            what.push("size".into());
        }
        format!("strength: {}", what.join(", "))
    }
}

/// `count` particles at `strength` above 0, in proportion: scaled and rounded, and at least one while there are any.
pub fn in_proportion(count: u32, strength: f32) -> u32 {
    if count == 0 {
        return 0;
    }
    ((count as f32 * strength).round() as u32).max(1)
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_whole(v: &(f32, f32)) -> bool {
    *v == (1.0, 1.0)
}

/// Which way the particles leave.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Aim {
    /// Up (+Y).
    #[default]
    Up,
    /// Out of the side that was struck, back towards the attacker, for an event that has one (a
    /// hit, a block, a parry: the effect plays on that side of the body); up for the rest. Tilted
    /// up by `lift`.
    Hit,
}

/// An effect (kind `effect`; its id is what the game's [`FxDef`], a clip's cue or
/// [`PlayEffect`](crate::PlayEffect) names): particles that start where it plays, fly, fall, shrink
/// and fade. Ranges `(min, max)` give each particle its own value between the two.
///
/// ```ron
/// {
///     "hit_spark": (
///         count: 18,
///         lifetime: (0.18, 0.35),
///         speed: (3.0, 7.0),
///         spread: 70.0,
///         aim: Hit,
///         gravity: -9.0,
///         drag: 2.0,
///         size: (0.05, 0.1),
///         color: "#fff2b3",
///         color_end: Some("#ff6a1a00"),
///         glow: true,
///         offset: (0.0, 1.1, 0.0),
///         max_live: 128,
///     ),
///     "heal_glow": (count: 6, rate: 30.0, duration: 0.6, speed: (0.4, 0.9), gravity: 1.5, color: "#7dff9e"),
/// }
/// ```
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct EffectDef {
    /// Particles started at once when it plays.
    pub count: u32,
    /// Particles a second after that, for `duration` seconds (a glow that lingers on a body it
    /// follows); 0 for a burst alone.
    pub rate: f32,
    /// Seconds it goes on emitting at `rate`, from where it plays (an entity it follows).
    pub duration: f32,
    /// Seconds each particle lives.
    pub lifetime: (f32, f32),
    /// Metres a second each particle starts at.
    pub speed: (f32, f32),
    /// Degrees of the cone the particles leave in, around `aim`: 0 a line, 180 a half sphere,
    /// 360 every way.
    pub spread: f32,
    pub aim: Aim,
    /// Degrees `Hit` is tilted up from the ground (sparks fly up and away).
    pub lift: f32,
    /// Metres a second squared along Y: -9.8 falls like a stone, a little up floats.
    pub gravity: f32,
    /// The share of its speed a particle loses each second, as air takes it (0 none).
    pub drag: f32,
    /// Size in metres at birth.
    pub size: (f32, f32),
    /// Size at death, as a share of the size at birth: 0 shrinks to nothing, 1 keeps it.
    pub size_end: f32,
    /// How the size goes from birth to `size_end`: at the share `t` of its life it has gone `1 − (1 − t)^size_ease` of
    /// the way. 1 a line; above 1 most of it early, then settling (a shockwave that opens fast and slows).
    #[serde(skip_serializing_if = "is_one")]
    pub size_ease: f32,
    /// How much a particle swells early in its life, over that: its size times `1 + swell × sin(π t)`. 0 none; 1 about
    /// 1.3 times at a quarter of its life for one that shrinks to nothing (dust that billows before it thins).
    #[serde(skip_serializing_if = "is_zero")]
    pub swell: f32,
    /// Colour at birth, `#rrggbb` or `#rrggbbaa` (sRGB).
    pub color: String,
    /// Colour at death; left out, `color` fading to transparent.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color_end: Option<String>,
    pub shape: ParticleShape,
    /// Light of its own, added to what is behind it (sparks, magic); without it the colour covers
    /// what is behind as its alpha says (dust, smoke), and cubes, shards and discs are shaded.
    pub glow: bool,
    /// Its own colour, unshaded, but covering what is behind as its alpha says (not added, as `glow` adds it): bright
    /// chips that keep their colour on a pale ground, a shockwave. Quads are unshaded anyway; with `glow` it changes nothing.
    #[serde(skip_serializing_if = "is_false")]
    pub unlit: bool,
    /// Where it starts, from the entity's feet or the point (metres, world axes): `(0, 1, 0)` is
    /// a metre up, the chest of a body.
    pub offset: (f32, f32, f32),
    /// Metres around that point the particles start within (a ball).
    pub radius: f32,
    /// Degrees a second a cube or a shard tumbles at most.
    pub spin: f32,
    /// The share of a play's push ([`PlayOptions::push`](crate::PlayOptions::push): the wind of a body's run, the kick of
    /// a roll) each particle takes, a range: `(0.6, 1.0)` scatters it a little. Without a push, nothing.
    #[serde(skip_serializing_if = "is_whole")]
    pub push: (f32, f32),
    /// How a play's strength changes the particles' number, speed and size ([`StrengthResponse`]).
    #[serde(skip_serializing_if = "StrengthResponse::is_default")]
    pub strength: StrengthResponse,
    /// Particles of this effect alive at once at most: its pool, made once. One more takes the
    /// place of the oldest.
    pub max_live: u32,
}

impl Default for EffectDef {
    fn default() -> Self {
        Self {
            count: 16,
            rate: 0.0,
            duration: 0.0,
            lifetime: (0.4, 0.8),
            speed: (1.0, 3.0),
            spread: 60.0,
            aim: Aim::Up,
            lift: 25.0,
            gravity: 0.0,
            drag: 0.0,
            size: (0.08, 0.12),
            size_end: 0.0,
            size_ease: 1.0,
            swell: 0.0,
            color: "#ffffff".into(),
            color_end: None,
            shape: ParticleShape::Quad,
            glow: false,
            unlit: false,
            offset: (0.0, 0.0, 0.0),
            radius: 0.0,
            spin: 360.0,
            push: (1.0, 1.0),
            strength: StrengthResponse::default(),
            max_live: 64,
        }
    }
}

impl EffectDef {
    /// The colours at birth and at death as sRGB bytes; one that does not read is white (the
    /// content check names it).
    pub fn colors(&self) -> ([u8; 4], [u8; 4]) {
        let start = parse_color(&self.color).unwrap_or([255; 4]);
        let end = match &self.color_end {
            Some(end) => parse_color(end).unwrap_or([255; 4]),
            None => [start[0], start[1], start[2], 0],
        };
        (start, end)
    }

    /// The most particles one play starts: the burst and what `rate` adds over `duration`
    /// (`u32::MAX` at most, for a rate too big to count: the check names it).
    pub fn per_play(&self) -> u32 {
        self.count.saturating_add(self.later())
    }

    /// The particles `rate` adds over `duration` after the burst (`u32::MAX` at most); 0 for a
    /// burst alone.
    pub fn later(&self) -> u32 {
        if self.rate > 0.0 && self.duration > 0.0 { (self.rate * self.duration).ceil() as u32 } else { 0 }
    }

    /// Its pool's size: `max_live`, [`MAX_LIVE`] at most. A definition the check refuses is still
    /// played at the first load, so what plays keeps to the limits the check names.
    pub fn pool_size(&self) -> usize {
        self.max_live.min(MAX_LIVE) as usize
    }

    /// `18 quad glow #fff2b3 → #ff6a1a00 · 0.18-0.35 s · aim hit` (the pool is the tool's to say).
    pub fn describe(&self) -> String {
        let mut parts = Vec::new();
        let mut what = format!("{} {}", self.count, self.shape.name());
        if self.rate > 0.0 && self.duration > 0.0 {
            what.push_str(&format!(" + {}/s for {}s", self.rate, self.duration));
        }
        if self.glow {
            what.push_str(" glow");
        } else if self.unlit {
            what.push_str(" unlit");
        }
        let end = self.color_end.clone().unwrap_or_else(|| "transparent".into());
        parts.push(format!("{what} {} → {end}", self.color));
        parts.push(if self.lifetime.0 == self.lifetime.1 {
            format!("{} s", self.lifetime.0)
        } else {
            format!("{}-{} s", self.lifetime.0, self.lifetime.1)
        });
        if self.aim == Aim::Hit {
            parts.push("aim hit".into());
        }
        if self.gravity != 0.0 {
            parts.push(format!("gravity {}", self.gravity));
        }
        if self.size_ease != 1.0 {
            parts.push(format!("size ease {}", self.size_ease));
        }
        if self.swell != 0.0 {
            parts.push(format!("swell {}", self.swell));
        }
        if self.push != (1.0, 1.0) {
            parts.push(format!("push {}-{}", self.push.0, self.push.1));
        }
        if !self.strength.is_default() {
            parts.push(self.strength.describe());
        }
        parts.join(" · ")
    }
}

/// A `(min, max)` range: both finite, `min` 0 or more (`positive`: above 0), `min` at most `max`.
fn range(refs: &mut Refs, field: &str, (min, max): (f32, f32), positive: bool) {
    let low_ok = if positive { min > 0.0 } else { min >= 0.0 };
    if !(min.is_finite() && max.is_finite() && low_ok && min <= max) {
        let low = if positive { "above 0" } else { "0 or more" };
        refs.error(format!("{field}: (min, max) with min {low} and at most max, not ({min}, {max})"));
    }
}

impl ContentDef for EffectDef {
    const KIND: &'static str = "effect";
    // Only what a machine shows or plays, or what lies outside a session: never in the replay check.
    const SIMULATION: bool = false;

    fn validate(&self, _: &str, refs: &mut Refs) {
        if !(1..=MAX_LIVE).contains(&self.max_live) {
            refs.error(format!("max_live: 1 to {MAX_LIVE}, not {}", self.max_live));
        }
        if self.count > MAX_COUNT {
            refs.error(format!("count: at most {MAX_COUNT} at once, not {}", self.count));
        }
        if !(self.rate >= 0.0 && self.rate.is_finite()) {
            refs.error(format!("rate: particles a second, 0 or more, not {}", self.rate));
        }
        if !(0.0..=MAX_DURATION).contains(&self.duration) {
            refs.error(format!("duration: 0 to {MAX_DURATION} seconds, not {}", self.duration));
        }
        if self.count == 0 && (self.rate <= 0.0 || self.duration <= 0.0) {
            refs.error("it starts no particle: give `count`, or `rate` and `duration`");
        }
        if self.per_play() > self.max_live && self.max_live > 0 {
            refs.error(format!(
                "one play starts {} particles and max_live keeps {}: raise max_live to {} at least, or a play takes back its own first particles",
                self.per_play(),
                self.max_live,
                self.per_play()
            ));
        }
        range(refs, "lifetime", self.lifetime, true);
        if self.lifetime.1 > MAX_LIFETIME {
            refs.error(format!("lifetime: at most {MAX_LIFETIME} seconds, not {}", self.lifetime.1));
        }
        range(refs, "speed", self.speed, false);
        range(refs, "size", self.size, true);
        if !(0.0..=360.0).contains(&self.spread) {
            refs.error(format!("spread: 0 to 360 degrees, not {}", self.spread));
        }
        if !(-90.0..=90.0).contains(&self.lift) {
            refs.error(format!("lift: -90 to 90 degrees, not {}", self.lift));
        }
        for (field, value) in [("gravity", self.gravity), ("spin", self.spin)] {
            if !value.is_finite() {
                refs.error(format!("{field}: a number, not {value}"));
            }
        }
        for (field, value) in [("drag", self.drag), ("size_end", self.size_end), ("radius", self.radius), ("swell", self.swell)] {
            if !(value >= 0.0 && value.is_finite()) {
                refs.error(format!("{field}: 0 or more, not {value}"));
            }
        }
        if !(self.size_ease > 0.0 && self.size_ease.is_finite()) {
            refs.error(format!("size_ease: above 0 (1 a line), not {}", self.size_ease));
        }
        range(refs, "push", self.push, false);
        if let Some(per) = self.strength.count_per
            && !per.is_finite()
        {
            refs.error(format!("strength.count_per: a number of particles, not {per}"));
        }
        let (x, y, z) = self.offset;
        if ![x, y, z].iter().all(|v| v.is_finite()) {
            refs.error(format!("offset: three numbers, not ({x}, {y}, {z})"));
        }
        if let Err(e) = parse_color(&self.color) {
            refs.error(format!("color: {e}"));
        }
        if let Some(end) = &self.color_end
            && let Err(e) = parse_color(end)
        {
            refs.error(format!("color_end: {e}"));
        }
    }
}

/// The engine's events a game can give an effect in its [`FxDef`], and where each plays.
pub const EVENTS: [(&str, &str); 9] = [
    ("damaged", "a hit landed: on the side of the one hit facing the attacker"),
    ("blocked", "a hit met a guard: on the side of the defender facing the attacker"),
    ("parried", "a hit was parried: on the side of the defender facing the attacker"),
    ("evaded", "a hit missed a dodge: on the side of the defender facing the attacker"),
    ("guard_broken", "a guard broke: on the side of the defender facing the attacker"),
    ("staggered", "someone staggered: at them"),
    ("died", "someone died: at them"),
    ("healed", "someone was healed: at them"),
    ("telegraph", "an attack warns: at the attacker"),
];

/// The game's effects (kind `fx`; one per game, any id): the effect each engine event plays, and
/// the effect each cue of a clip plays (a cue with no entry plays the effect of its own name, if
/// there is one).
///
/// ```ron
/// {
///     "arena": (
///         effects: {"damaged": "hit_spark", "blocked": "block_spark", "died": "death_dust"},
///         cues: {"footstep": "dust_puff"},
///     ),
/// }
/// ```
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct FxDef {
    /// An engine event ([`EVENTS`]: `damaged`, `died`, …) → the effect it plays.
    pub effects: BTreeMap<String, String>,
    /// A clip's cue (`AnimationCue`: `footstep`, `swoosh`) → the effect it plays: from the body's feet, or
    /// from the cue's bone when it names one (`CueDef::bone`), at the cue's `strength` (fewer, smaller particles).
    pub cues: BTreeMap<String, String>,
}

impl ContentDef for FxDef {
    const KIND: &'static str = "fx";
    // Only what a machine shows or plays, or what lies outside a session: never in the replay check.
    const SIMULATION: bool = false;

    fn validate(&self, id: &str, refs: &mut Refs) {
        let others: Vec<&str> = refs.iter::<FxDef>().map(|(other, _)| other).filter(|other| *other != id).collect();
        if !others.is_empty() {
            refs.error(format!("a game has one table of effects, and this one has more: {}; keep one", others.join(", ")));
        }
        for (event, effect) in &self.effects {
            if EVENTS.iter().any(|(name, _)| name == event) {
                refs.require::<EffectDef>(effect, &format!("effects.{event}"));
            } else {
                let names = EVENTS.iter().map(|(name, _)| *name);
                let hint = closest(event, names.clone()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                refs.error(format!("effects: no engine event `{event}`{hint}; events: {}", names.collect::<Vec<_>>().join(", ")));
            }
        }
        for (cue, effect) in &self.cues {
            if cue.trim().is_empty() {
                refs.error("cues: an empty cue name; give the cue's name as the clip names it (`footstep`)");
            }
            refs.require::<EffectDef>(effect, &format!("cues.{cue}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> EffectDef {
        ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME).from_str(text).unwrap()
    }

    #[test]
    fn an_effect_reads_short_and_says_what_it_is() {
        let spark = read(r##"(count: 18, lifetime: (0.18, 0.35), aim: Hit, color: "#fff2b3", color_end: "#ff6a1a00", glow: true, max_live: 128)"##);
        assert_eq!((spark.count, spark.aim, spark.shape, spark.speed), (18, Aim::Hit, ParticleShape::Quad, (1.0, 3.0)));
        assert_eq!(spark.describe(), "18 quad glow #fff2b3 → #ff6a1a00 · 0.18-0.35 s · aim hit");
        assert_eq!(spark.colors(), ([255, 242, 179, 255], [255, 106, 26, 0]));
        let glow = read(r##"(count: 4, rate: 30.0, duration: 0.5, shape: Shard, color: "#7dff9e")"##);
        assert_eq!(glow.describe(), "4 shard + 30/s for 0.5s #7dff9e → transparent · 0.4-0.8 s");
        assert_eq!(glow.colors().1, [0x7d, 0xff, 0x9e, 0], "without color_end it fades out");
        assert_eq!(glow.per_play(), 19);
        assert!(ron::from_str::<EffectDef>("(colour: \"#fff\")").is_err(), "unknown fields are refused");
    }

    #[test]
    fn a_typo_in_the_numbers_counts_without_overflow_and_plays_within_the_limits() {
        // A rate too big to count saturates instead of overflowing (a panic in a debug build, 15
        // particles that pass the check in a release one).
        let endless = read("(count: 16, rate: 5e9, duration: 1.0)");
        assert_eq!((endless.later(), endless.per_play()), (u32::MAX, u32::MAX));
        assert_eq!(EffectDef { rate: f32::INFINITY, duration: 1.0, ..EffectDef::default() }.per_play(), u32::MAX);
        assert_eq!(read("(rate: -3.0, duration: 1.0)").later(), 0);
        assert_eq!(read("(max_live: 4000000000)").pool_size(), MAX_LIVE as usize);
        assert_eq!(read("(max_live: 24)").pool_size(), 24);
    }

    #[test]
    fn a_strength_response_counts_scales_and_says_what_it_does() {
        // Left out: the number and the size in proportion, the speed not (as a scaled play always was).
        let plain = StrengthResponse::default();
        assert_eq!([plain.burst(12, 1.0), plain.burst(12, 0.5), plain.burst(12, 0.01), plain.burst(0, 2.0)], [12, 6, 1, 0]);
        assert_eq!((plain.speed_at(2.0), plain.size_at(2.0), plain.describe()), (1.0, 2.0, String::new()));
        // `int(6 + 6 × strength)`, written as 12 at strength 1 and 6 more for each 1 above it.
        let fan = read("(count: 12, strength: (count_per: 6.0, speed: true))").strength;
        assert_eq!([fan.burst(12, 1.0), fan.burst(12, 1.5), fan.burst(12, 0.6), fan.burst(12, 1.8)], [12, 15, 9, 16]);
        assert_eq!((fan.speed_at(1.5), fan.size_at(1.5)), (1.5, 1.5), "size stays on when left out");
        assert_eq!(fan.describe(), "strength: count +6, speed, size");
        // Always the count, faster with the strength, never bigger.
        let sparks = StrengthResponse { count_per: Some(0.0), speed: true, size: false };
        assert_eq!([sparks.burst(3, 0.6), sparks.burst(3, 1.8)], [3, 3]);
        assert_eq!((sparks.speed_at(1.8), sparks.size_at(1.8)), (1.8, 1.0));
        assert_eq!(sparks.describe(), "strength: count +0, speed");
        // Fewer below 1, down to none, never a count below zero or one that is no number.
        let steep = StrengthResponse { count_per: Some(10.0), ..StrengthResponse::default() };
        assert_eq!([steep.burst(4, 0.75), steep.burst(4, 0.6), steep.burst(4, 0.2), steep.burst(4, f32::NAN)], [1, 0, 0, 0]);
        assert_eq!(StrengthResponse { count_per: Some(-2.0), speed: false, size: false }.describe(), "strength: count -2");
        assert_eq!(StrengthResponse { speed: true, ..StrengthResponse::default() }.describe(), "strength: count ×, speed, size");
    }

    #[test]
    fn an_effect_without_the_new_fields_reads_and_writes_as_it_always_did() {
        let old = read(r##"(count: 6, shape: Cube, size_end: 0.5, color: "#ffffff")"##);
        assert_eq!((old.size_ease, old.swell, old.unlit, old.push, old.strength), (1.0, 0.0, false, (1.0, 1.0), StrengthResponse::default()));
        let written = ron::to_string(&old).unwrap();
        for field in ["size_ease", "swell", "unlit", "push", "strength"] {
            assert!(!written.contains(field), "{field} written: {written}");
        }
        assert_eq!(old.describe(), "6 cube #ffffff → transparent · 0.4-0.8 s");
        // A shockwave: one level disc that opens fast and slows, its own colour, as big at any strength in number.
        let ring = read(
            r##"(count: 1, shape: Disc, speed: (0.0, 0.0), size: (0.7, 0.7), size_end: 4.0, size_ease: 2.5, color: "#ffffffb3",
                unlit: true, strength: (count_per: 0.0))"##,
        );
        assert_eq!(ring.describe(), "1 disc unlit #ffffffb3 → transparent · 0.4-0.8 s · size ease 2.5 · strength: count +0, size");
        assert_eq!(read(&ron::to_string(&ring).unwrap()), ring, "what it writes reads back the same");
        let dust = read("(count: 8, shape: Cube, glow: true, unlit: true, swell: 1.0, push: (0.6, 1.0))");
        assert_eq!(dust.describe(), "8 cube glow #ffffff → transparent · 0.4-0.8 s · swell 1 · push 0.6-1", "glow says it all");
    }
}
