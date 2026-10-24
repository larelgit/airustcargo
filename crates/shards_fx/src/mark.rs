//! Marks (F9): spots laid on surfaces. A bullet hole in a wall, blood on a floor, a scorch on a ceiling:
//! the content kind `mark` (`*.mark.ron`) says what a spot looks like, how big it is, how it fades and how
//! many of its kind there can be; the game lays one with [`spawn_mark`](crate::spawn_mark), or the `fx`
//! table does, from an engine event ([`MarkLink`]: a shot stopping at a wall, a shot going through a body,
//! a blow, a death, a part breaking off).
//!
//! A mark is a decal: Bevy's `ForwardDecal` with a `StandardMaterial`, so it is lit and shadowed as the
//! surface under it is. Its quad is turned so that its +Y is the surface's normal and it projects along
//! it, onto a wall, a ceiling or a floor alike (the telegraph decals project straight down only). It is
//! picture only: no `AiId`, nothing in a replay or a snapshot, nothing on a headless machine.
//!
//! The numbers here are pure; the pools' entities and materials are drawn in [`crate::marks_render`].

use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::db::closest;
use shards_content::{parse_color, ContentDef, Refs};

use crate::def::{asset_problem, check_tags};

/// The most marks of one kind alive at once.
pub const MAX_LIVE: u32 = 4096;
/// The longest a mark lasts (s): a day. Longer is `UntilCleared`.
pub const MAX_LIFETIME: f32 = 86_400.0;
/// The most marks one link lays at once.
pub const MAX_COUNT: u32 = 16;
/// The farthest a link looks for the surface behind or below a body (m).
pub const MAX_REACH: f32 = 100.0;
/// How far a mark's quad stands off the surface along its normal (m).
pub const LIFT: f32 = 0.002;

/// How long a mark lasts.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub enum MarkLife {
    /// This many seconds on the simulation's clock, then it is gone (it fades over its last `fade_out`).
    Secs(f32),
    /// Until it is cleared (`shards_fx::clear`, a region change) or the pool recycles it for a newer one.
    #[default]
    UntilCleared,
}

/// A kind of mark (kind `mark`; its id is what [`MarkLink`], [`spawn_mark`](crate::spawn_mark) and `mark_spawn` name):
/// a spot of a colour and, if it has one, a picture, laid on a surface.
///
/// ```ron
/// {
///     "bullet_hole": (texture: "textures/marks/bullet_hole.png", size: (0.07, 0.11), roughness: 0.95,
///         lifetime: Secs(90.0), fade_out: 20.0, max_live: 96, tags: ["gore"]),
///     "blood": (color: "#6e0f12", size: (0.25, 0.6), fade_in: 0.15, max_live: 128, tags: ["gore"]),
/// }
/// ```
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct MarkDef {
    /// A PNG inside the assets folder: the spot's colour times the picture's, its shape the picture's alpha.
    /// Without one the mark is a soft round spot.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub texture: Option<String>,
    /// Its colour, `#rrggbb` or `#rrggbbaa` (sRGB); the alpha is how opaque it is at most.
    pub color: String,
    /// Its width in metres, a range: each mark takes its own size from it.
    pub size: (f32, f32),
    /// Each mark is turned at random about the surface's normal. Off, the picture's top points up the wall (along the
    /// surface's steepest climb; any way on a floor or a ceiling).
    pub rotate: bool,
    /// Seconds a mark takes to show in full when it is laid (a stain that spreads in): 0 shows it at once.
    #[serde(skip_serializing_if = "is_zero")]
    pub fade_in: f32,
    /// Seconds before its end a `Secs` mark takes to fade out.
    #[serde(skip_serializing_if = "is_zero")]
    pub fade_out: f32,
    pub lifetime: MarkLife,
    /// Marks of this kind alive at once at most: one more takes the place of the oldest.
    pub max_live: u32,
    /// How far off the quad's plane (m) a surface still takes the mark, fading: a small number keeps it to the surface it
    /// was laid on and off a body in front of it, a large one lets it wrap round a corner or a step.
    pub depth: f32,
    /// How rough it is, 0 (wet, it shines) to 1 (dry, it does not).
    pub roughness: f32,
    /// Its own colour in any light: no shading and no shadow.
    #[serde(skip_serializing_if = "is_false")]
    pub unlit: bool,
    /// Words `shards_fx::clear` takes the kind's marks away by (`"gore"`, `"level"`).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_false(v: &bool) -> bool {
    !*v
}

impl Default for MarkDef {
    fn default() -> Self {
        Self {
            texture: None,
            color: "#ffffff".into(),
            size: (0.2, 0.3),
            rotate: true,
            fade_in: 0.0,
            fade_out: 0.0,
            lifetime: MarkLife::UntilCleared,
            max_live: 64,
            depth: 0.15,
            roughness: 0.9,
            unlit: false,
            tags: Vec::new(),
        }
    }
}

impl MarkDef {
    /// The colour as sRGB bytes; one that does not read is white (the content check names it).
    pub fn rgba(&self) -> [u8; 4] {
        parse_color(&self.color).unwrap_or([255; 4])
    }

    /// The pool's size: `max_live`, [`MAX_LIVE`] at most.
    pub fn cap(&self) -> usize {
        self.max_live.clamp(1, MAX_LIVE) as usize
    }

    /// How much of its colour a mark shows `age` seconds after it was laid: 0 to 1.
    pub fn alpha(&self, age: f32) -> f32 {
        let shown = if self.fade_in > 0.0 { (age / self.fade_in).clamp(0.0, 1.0) } else { 1.0 };
        let kept = match self.lifetime {
            MarkLife::Secs(secs) if self.fade_out > 0.0 => ((secs - age) / self.fade_out).clamp(0.0, 1.0),
            _ => 1.0,
        };
        shown * kept
    }

    /// Whether a mark `age` seconds old is still there.
    pub fn alive(&self, age: f32) -> bool {
        match self.lifetime {
            MarkLife::Secs(secs) => age < secs,
            MarkLife::UntilCleared => true,
        }
    }

    /// `#6e0f12 · 0.25-0.6 m · until cleared · up to 128` (the pool's live count is the tool's to say).
    pub fn describe(&self) -> String {
        let mut parts = vec![format!("{} {}", self.texture.as_deref().unwrap_or("spot"), self.color)];
        parts.push(if self.size.0 == self.size.1 { format!("{} m", self.size.0) } else { format!("{}-{} m", self.size.0, self.size.1) });
        parts.push(match self.lifetime {
            MarkLife::Secs(secs) => format!("{secs} s"),
            MarkLife::UntilCleared => "until cleared".into(),
        });
        if self.fade_in > 0.0 {
            parts.push(format!("fade in {} s", self.fade_in));
        }
        if self.fade_out > 0.0 {
            parts.push(format!("fade out {} s", self.fade_out));
        }
        if self.unlit {
            parts.push("unlit".into());
        }
        if !self.tags.is_empty() {
            parts.push(format!("tags {}", self.tags.join(", ")));
        }
        parts.join(" · ")
    }
}

impl ContentDef for MarkDef {
    const KIND: &'static str = "mark";
    // Only what a machine shows: never in the replay check.
    const SIMULATION: bool = false;

    fn validate(&self, _: &str, refs: &mut Refs) {
        if !(1..=MAX_LIVE).contains(&self.max_live) {
            refs.error(format!("max_live: 1 to {MAX_LIVE}, not {}", self.max_live));
        }
        let (min, max) = self.size;
        if !(min.is_finite() && max.is_finite() && min > 0.0 && min <= max) {
            refs.error(format!("size: (min, max) in metres with min above 0 and at most max, not ({min}, {max})"));
        }
        for (field, value) in [("fade_in", self.fade_in), ("fade_out", self.fade_out)] {
            if !(value >= 0.0 && value.is_finite()) {
                refs.error(format!("{field}: seconds, 0 or more, not {value}"));
            }
        }
        match self.lifetime {
            MarkLife::Secs(secs) => {
                if !(secs > 0.0 && secs <= MAX_LIFETIME) {
                    refs.error(format!("lifetime: Secs(n) with n above 0 and at most {MAX_LIFETIME}, not {secs}; a mark that stays is `UntilCleared`"));
                } else if self.fade_in + self.fade_out > secs {
                    refs.error(format!(
                        "fade_in {} and fade_out {} do not fit in lifetime Secs({secs}): together at most as long",
                        self.fade_in, self.fade_out
                    ));
                }
            }
            MarkLife::UntilCleared => {
                if self.fade_out > 0.0 {
                    refs.error("fade_out: a mark that lasts until cleared has no end to fade out towards; give `lifetime: Secs(..)` or take it out");
                }
            }
        }
        if !(self.depth > 0.0 && self.depth.is_finite()) {
            refs.error(format!("depth: metres above 0, not {}", self.depth));
        }
        if !(0.0..=1.0).contains(&self.roughness) {
            refs.error(format!("roughness: 0 to 1, not {}", self.roughness));
        }
        if let Err(e) = parse_color(&self.color) {
            refs.error(format!("color: {e}"));
        }
        if let Some(texture) = &self.texture
            && let Some(why) = asset_problem(refs, texture, &[".png"], false)
        {
            refs.error(format!("texture {why}"));
        }
        check_tags(refs, &self.tags);
    }
}

/// The engine events a game can lay marks from in its [`FxDef`](crate::FxDef), and where each lays them.
pub const MARK_EVENTS: [(&str, &str); 5] = [
    ("shot_impact", "a shot stopped at a world surface: at the point, along the surface's normal"),
    ("shot_hit", "a shot went through a body: from the point, along the shot"),
    ("damaged", "a hit landed: from the one hit, along the blow (from the attacker to them, level)"),
    ("died", "someone died: from them, along the killing blow when it had a direction"),
    ("part_broken", "a body part broke off: from the part, along the blow"),
];

/// Where a [`MarkLink`] finds the surface it lays its marks on.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub enum Place {
    /// At the event's own point, on its surface, along its normal: a shot that stopped at a wall (`shot_impact` only).
    #[default]
    At,
    /// The nearest world surface along the event's direction from its point, within this many metres: the wall behind a
    /// body that was shot, the floor a blow drives it toward.
    Behind(f32),
    /// The nearest world surface straight below its point, within this many metres: the floor under a body.
    Below(f32),
}

/// What an event lays: a mark of kind `mark`, found by `place`; `count` of them, each looked for in a cone of `spread`
/// degrees round the way (blood that sprays) from the event's point plus `offset`.
///
/// ```ron
/// marks: {
///     "shot_impact": [(mark: "bullet_hole")],
///     "shot_hit": [(mark: "blood", place: Behind(6.0), count: 2, spread: 8.0), (mark: "blood", place: Below(3.0))],
///     "died": [(mark: "blood_pool", place: Below(2.0), offset: (0.0, 0.5, 0.0))],
/// }
/// ```
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct MarkLink {
    /// The `mark` id.
    pub mark: String,
    pub place: Place,
    /// Marks per event (1 to [`MAX_COUNT`]).
    #[serde(skip_serializing_if = "is_one")]
    pub count: u32,
    /// Degrees across the cone each mark's search is drawn in round the way (0 to 180, as an effect's `spread`), for `Behind`
    /// and `Below`: blood that sprays.
    #[serde(skip_serializing_if = "is_zero")]
    pub spread: f32,
    /// Metres added to the point the search starts from (world axes): `(0, 1, 0)` from the feet of a body is its chest.
    /// For the events of a body (`damaged`, `died`, `part_broken`) the point is its feet.
    #[serde(skip_serializing_if = "is_origin")]
    pub offset: (f32, f32, f32),
}

fn is_one(v: &u32) -> bool {
    *v == 1
}

fn is_origin(v: &(f32, f32, f32)) -> bool {
    *v == (0.0, 0.0, 0.0)
}

impl Default for MarkLink {
    fn default() -> Self {
        Self::new(String::new(), Place::At)
    }
}

impl MarkLink {
    /// A link that lays one mark with `place`.
    pub fn new(mark: impl Into<String>, place: Place) -> Self {
        Self { mark: mark.into(), place, count: 1, spread: 0.0, offset: (0.0, 0.0, 0.0) }
    }

    /// The content check of one link of `event`'s list (its place in the list is `index`).
    pub(crate) fn validate(&self, event: &str, index: usize, refs: &mut Refs) {
        let at = format!("marks.{event}[{index}]");
        refs.require::<MarkDef>(&self.mark, &format!("{at}.mark"));
        if !(1..=MAX_COUNT).contains(&self.count) {
            refs.error(format!("{at}.count: 1 to {MAX_COUNT}, not {}", self.count));
        }
        if !(0.0..=180.0).contains(&self.spread) {
            refs.error(format!("{at}.spread: 0 to 180 degrees across, not {}", self.spread));
        }
        let (x, y, z) = self.offset;
        if ![x, y, z].iter().all(|v| v.is_finite()) {
            refs.error(format!("{at}.offset: three numbers, not ({x}, {y}, {z})"));
        }
        match self.place {
            Place::At => {
                if event != "shot_impact" {
                    refs.error(format!(
                        "{at}.place: `{event}` has no surface of its own to lay a mark on: give `Behind(metres)` or `Below(metres)`"
                    ));
                }
                if self.spread != 0.0 || self.offset != (0.0, 0.0, 0.0) {
                    refs.error(format!("{at}: `spread` and `offset` search for a surface; `At` lays the mark where the event happened, so take them out"));
                }
                if self.count != 1 {
                    refs.error(format!("{at}.count: `At` lays one mark on the one surface; give `Behind` or `Below` for several"));
                }
            }
            Place::Behind(reach) | Place::Below(reach) => {
                if !(reach > 0.0 && reach <= MAX_REACH) {
                    refs.error(format!("{at}.place: a reach above 0 and at most {MAX_REACH} metres, not {reach}"));
                }
            }
        }
    }
}

/// Checks the event names of a `marks` table and every link in it.
pub(crate) fn validate_links(marks: &BTreeMap<String, Vec<MarkLink>>, refs: &mut Refs) {
    for (event, links) in marks {
        if !MARK_EVENTS.iter().any(|(name, _)| name == event) {
            let names = MARK_EVENTS.iter().map(|(name, _)| *name);
            let hint = closest(event, names.clone()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
            refs.error(format!("marks: no event `{event}` lays marks{hint}; events: {}", names.collect::<Vec<_>>().join(", ")));
            continue;
        }
        for (i, link) in links.iter().enumerate() {
            link.validate(event, i, refs);
        }
    }
}

/// A mark laid: where, how it is turned and how big, and when. Picture only.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    /// The clock (s) when it was laid.
    pub born: f64,
    pub point: Vec3,
    /// The surface's unit normal: the mark's quad faces along it and projects against it.
    pub normal: Vec3,
    /// Its width (m).
    pub size: f32,
    /// Radians about the normal.
    pub spin: f32,
    /// The order it was laid in, in its kind: the pool sets it, and the oldest is the one with the least.
    pub seq: u64,
}

impl Mark {
    /// Seconds since it was laid, at clock `now` (0 before).
    pub fn age(&self, now: f64) -> f32 {
        ((now - self.born) as f32).max(0.0)
    }

    /// Its quad's rotation: +Y along the normal, the picture's top up the wall (any way on a floor or a ceiling), turned
    /// about the normal by `spin`.
    pub fn rotation(&self) -> Quat {
        surface_frame(self.normal) * Quat::from_rotation_y(self.spin)
    }

    /// Where its quad lies: a hair off the surface, `size` across.
    pub fn transform(&self) -> Transform {
        Transform { translation: self.point + self.normal * LIFT, rotation: self.rotation(), scale: Vec3::splat(self.size) }
    }
}

/// The frame a quad lies in on a surface of unit normal `n`: its +Y along `n`, and its top (the decal mesh's -Z) the way
/// up the surface climbs, else (a floor, a ceiling) toward -Z of the world.
pub fn surface_frame(n: Vec3) -> Quat {
    let top = if n.y.abs() > 0.95 { Vec3::NEG_Z } else { (Vec3::Y - n * n.y).normalize_or(Vec3::NEG_Z) };
    let z = -top;
    Quat::from_mat3(&Mat3::from_cols(n.cross(z), n, z)).normalize()
}

/// A kind's marks: a fixed number of slots that grows up to the kind's `max_live`, each empty or holding one.
/// One more mark than the cap takes the slot of the oldest.
#[derive(Clone, Debug, PartialEq)]
pub struct MarkPool {
    /// The definition the pool was made for.
    pub def: MarkDef,
    pub slots: Vec<Option<Mark>>,
    /// Marks laid in it, and those that took the slot of a live one.
    pub started: u64,
    pub recycled: u64,
}

impl MarkPool {
    pub fn new(def: &MarkDef) -> Self {
        Self { def: def.clone(), slots: Vec::new(), started: 0, recycled: 0 }
    }

    pub fn cap(&self) -> usize {
        self.def.cap()
    }

    pub fn live(&self) -> usize {
        self.slots.iter().filter(|s| s.is_some()).count()
    }

    /// Puts `mark` in a free slot, else in a new one while the pool is under its cap, else in the oldest one's;
    /// the slot it took.
    pub fn add(&mut self, mut mark: Mark) -> usize {
        mark.seq = self.started;
        let slot = match self.slots.iter().position(Option::is_none) {
            Some(free) => free,
            None if self.slots.len() < self.cap() => {
                self.slots.push(None);
                self.slots.len() - 1
            }
            None => {
                // The oldest: the first one laid (marks laid in one tick are told apart by their order).
                let oldest = self.slots.iter().enumerate().filter_map(|(i, m)| m.map(|m| (i, m.seq))).min_by_key(|(_, seq)| *seq).map_or(0, |(i, _)| i);
                self.recycled += 1;
                oldest
            }
        };
        self.slots[slot] = Some(mark);
        self.started += 1;
        slot
    }

    /// Empties the slots of the marks that have run out at clock `now` (and of any laid after it: the clock went
    /// back, a snapshot was loaded).
    pub fn expire(&mut self, now: f64) {
        let def = &self.def;
        for slot in &mut self.slots {
            if slot.is_some_and(|m| !def.alive(m.age(now)) || m.born > now + 1e-6) {
                *slot = None;
            }
        }
    }

    /// Takes all of its marks away; how many there were.
    pub fn clear(&mut self) -> usize {
        let live = self.live();
        self.slots.iter_mut().for_each(|s| *s = None);
        live
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read(text: &str) -> MarkDef {
        ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME).from_str(text).unwrap()
    }

    fn mark(born: f64) -> Mark {
        Mark { born, point: Vec3::ZERO, normal: Vec3::Y, size: 0.2, spin: 0.0, seq: 0 }
    }

    #[test]
    fn a_mark_reads_short_says_what_it_is_and_writes_what_it_was_given() {
        let hole = read(r##"(texture: "textures/hole.png", size: (0.07, 0.11), lifetime: Secs(90.0), fade_out: 20.0, max_live: 96, tags: ["gore"])"##);
        assert_eq!((hole.max_live, hole.rotate, hole.depth, hole.color.as_str()), (96, true, 0.15, "#ffffff"));
        assert_eq!(hole.describe(), "textures/hole.png #ffffff · 0.07-0.11 m · 90 s · fade out 20 s · tags gore");
        let stain = read(r##"(color: "#6e0f12", fade_in: 0.2)"##);
        assert_eq!(stain.describe(), "spot #6e0f12 · 0.2-0.3 m · until cleared · fade in 0.2 s");
        assert_eq!(stain.rgba(), [0x6e, 0x0f, 0x12, 255]);
        // What is left at its default is not written, so a definition reads back the same.
        assert_eq!(ron::to_string(&MarkDef::default()).unwrap(), ron::to_string(&read("()")).unwrap());
        assert!(!ron::to_string(&stain).unwrap().contains("unlit") && !ron::to_string(&stain).unwrap().contains("tags"));
        assert_eq!(read(&ron::to_string(&hole).unwrap()), hole);
        assert!(ron::from_str::<MarkDef>("(colour: \"#fff\")").is_err(), "unknown fields are refused");
    }

    #[test]
    fn a_mark_shows_in_and_fades_out_over_its_life() {
        let def = read("(lifetime: Secs(10.0), fade_in: 1.0, fade_out: 4.0)");
        let alpha = |age: f32| def.alpha(age);
        assert_eq!([alpha(0.0), alpha(0.5), alpha(1.0), alpha(5.0)], [0.0, 0.5, 1.0, 1.0]);
        assert_eq!([alpha(8.0), alpha(10.0), alpha(12.0)], [0.5, 0.0, 0.0]);
        assert!(def.alive(9.99) && !def.alive(10.0));
        // Until cleared it never ends and has nothing to fade towards.
        let stays = read("(fade_in: 0.0)");
        assert_eq!((stays.alpha(1e6), stays.alive(1e9)), (1.0, true));
    }

    #[test]
    fn a_full_pool_recycles_its_oldest_and_never_holds_more_than_its_cap() {
        let mut pool = MarkPool::new(&read("(max_live: 3, lifetime: Secs(5.0))"));
        assert_eq!(pool.slots.len(), 0, "slots are made as marks come");
        assert_eq!([pool.add(mark(2.0)), pool.add(mark(1.0)), pool.add(mark(3.0))], [0, 1, 2]);
        assert_eq!(pool.add(mark(4.0)), 0, "the first one laid goes first, whatever its time");
        assert_eq!(pool.add(mark(5.0)), 1, "then the second");
        assert_eq!((pool.live(), pool.started, pool.recycled, pool.slots.len()), (3, 5, 2, 3));
        // Lives of 5 s: at 8.5 the mark laid at 3.0 is over.
        pool.expire(8.5);
        assert_eq!(pool.slots.iter().map(|s| s.map(|m| m.born)).collect::<Vec<_>>(), [Some(4.0), Some(5.0), None]);
        // The clock went back (a snapshot): what was laid after it goes.
        pool.expire(4.5);
        assert_eq!(pool.live(), 1);
        assert_eq!(pool.clear(), 1);
        assert_eq!(pool.live(), 0);
        // Marks laid in one tick (a shotgun's) are told apart by their order, not their time.
        let mut pool = MarkPool::new(&read("(max_live: 2)"));
        let first: Vec<usize> = (0..4).map(|_| pool.add(mark(7.0))).collect();
        assert_eq!(first, [0, 1, 0, 1], "each takes the slot of the one laid longest ago");
        // Ten times the cap in a row: the cap holds.
        let mut pool = MarkPool::new(&read("(max_live: 16)"));
        for i in 0..160 {
            pool.add(mark(f64::from(i)));
        }
        assert_eq!((pool.live(), pool.slots.len(), pool.recycled), (16, 16, 144));
        assert_eq!(pool.slots.iter().flatten().map(|m| m.born as u32).min(), Some(144), "the newest 16 are the ones left");
    }

    #[test]
    fn a_mark_lies_a_hair_off_its_surface_facing_along_the_normal() {
        for normal in [Vec3::Y, Vec3::NEG_Y, Vec3::X, Vec3::new(0.3, 0.5, -0.8).normalize()] {
            let m = Mark { born: 0.0, point: Vec3::new(1.0, 2.0, 3.0), normal, size: 0.4, spin: 1.1, seq: 0 };
            let t = m.transform();
            assert!((t.rotation * Vec3::Y).abs_diff_eq(normal, 1e-5), "its up is the normal: {normal}");
            assert!(t.translation.abs_diff_eq(m.point + normal * LIFT, 1e-6));
            assert_eq!(t.scale, Vec3::splat(0.4));
        }
    }

    #[test]
    fn the_picture_of_a_mark_on_a_wall_stands_upright() {
        // The decal mesh's top is its local -Z: on a wall it points up the wall, whichever way the wall faces.
        for normal in [Vec3::X, Vec3::NEG_X, Vec3::Z, Vec3::new(0.6, 0.2, -0.77).normalize()] {
            let top = surface_frame(normal) * Vec3::NEG_Z;
            assert!(top.y > 0.5 && top.dot(normal).abs() < 1e-5, "{normal}: top {top}");
            assert!((surface_frame(normal) * Vec3::Y).abs_diff_eq(normal, 1e-5));
        }
        // A floor and a ceiling have no up: a fixed way, and still a frame.
        for normal in [Vec3::Y, Vec3::NEG_Y] {
            let frame = surface_frame(normal);
            assert!((frame * Vec3::Y).abs_diff_eq(normal, 1e-5) && (frame * Vec3::NEG_Z).abs_diff_eq(Vec3::NEG_Z, 1e-5));
        }
    }
}
