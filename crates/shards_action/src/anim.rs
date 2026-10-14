//! Animation, as data and decisions: the `animset` content kind and the choice of the
//! clip a character should show.
//!
//! **Gameplay leads, animation catches up.** The action machine decides what happens and
//! when; an [`AnimSet`] only says which clip of a glTF file shows it. While an action runs,
//! its clip is *seeked* to the action's progress (gameplay time scaled to the clip's
//! length), so a hit window opens at the same moment of the animation on every machine and
//! at every frame rate. Between actions, locomotion clips (`idle`, `walk`, `run`, ...) loop,
//! at a speed that follows the character's velocity. [`select_clip`] is that decision as one
//! pure function; the systems that apply it to a Bevy `AnimationPlayer` live behind the
//! `render` feature, and nothing here needs a window.
//!
//! ## Content
//!
//! ```ron
//! // content/anim/humanoid.animset.ron
//! {
//!     "humanoid": (
//!         // One glTF holds the clips. Characters of other models share them when their
//!         // skeletons have the same node names.
//!         source: "models/humanoid.glb",
//!         // Library: our name -> the animation in the file (default: the same name).
//!         clips: {
//!             "idle": (),
//!             "walk": (),
//!             "run": (speed: 1.0),
//!             "fall": (name: "Armature|Falling"),
//!             "swing_1": (name: "Armature|Slash1", cues: [(at: 0.28, cue: "swoosh")]),
//!             "roll": (blend: 0.04),
//!             "hit": (),
//!             "die": (),
//!         },
//!         // Locomotion state -> clip. Missing ones fall back (sprint -> run -> walk -> idle).
//!         locomotion: {"idle": "idle", "walk": "walk", "run": "run", "air": "fall",
//!                      "stagger": "hit", "dead": "die"},
//!         // Action id -> clip; or give the action `anim: "swing_1"`, or map its tags.
//!         actions: {"light_1": "swing_1", "roll": "roll"},
//!         tags: {},
//!         blend: 0.2,         // seconds between locomotion clips
//!         action_blend: 0.06, // seconds into an action's clip
//!         // What speed (m/s) the walk, run and sprint clips were made for.
//!         speeds: (walk: 2.0, run: 5.0, sprint: 8.0),
//!     ),
//! }
//! ```
//!
//! A character is [`Animated`] with the set's id and has the glTF scene as a child
//! (`WorldAssetRoot(asset_server.load(GltfAssetLabel::Scene(0).from_asset("models/hero.glb")))`).
//!
//! ## Cues
//!
//! A clip can carry timed [`CueDef`]s; when the animation passes one, an [`AnimationCue`]
//! event fires for the entity. Cues are for effects and sound (a footstep, a whoosh, a
//! spark), never for gameplay: hits, i-frames and cancels come from the action's own
//! timeline.

use std::collections::BTreeMap;

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::{ContentDef, Refs};
use shards_motion::{MotionMode, MotionState};

use crate::def::{ActionDef, EPS};

/// Marks a character the animation bridge drives: `animset` is the id of its
/// [`AnimSet`]. The glTF scene is a child entity. Presentation only: replays ignore it, so
/// a game may add it only when it has a window.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct Animated {
    pub animset: String,
}

impl Animated {
    /// A character animated by `animset`.
    pub fn new(animset: impl Into<String>) -> Self {
        Self { animset: animset.into() }
    }
}

/// The animation passed a cue of its clip: for effects and sound only (VFX, SFX). Never
/// drive gameplay from it.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct AnimationCue {
    pub entity: Entity,
    /// The cue's name from content: `footstep`, `swoosh`.
    pub cue: String,
    /// The clip (our name for it) that fired it.
    pub clip: String,
}

/// A moment of a clip for effects or sound.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct CueDef {
    /// Seconds into the clip, on the clip's own timeline.
    pub at: f32,
    pub cue: String,
}

/// One clip of an animset's library.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ClipDef {
    /// The animation's name in the glTF file; default: this clip's own name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Playback speed multiplier while it loops as locomotion (1: as authored).
    pub speed: f32,
    /// Seconds to blend into it; default: the set's `blend` (`action_blend` for actions).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blend: Option<f32>,
    /// Effects and sound on its timeline.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub cues: Vec<CueDef>,
}

impl Default for ClipDef {
    fn default() -> Self {
        Self { name: None, speed: 1.0, blend: None, cues: Vec::new() }
    }
}

/// What speeds the locomotion clips were made for, and where a walk becomes a run.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct GroundSpeeds {
    /// m/s the `walk` clip's feet match.
    pub walk: f32,
    /// m/s of the `run` clip.
    pub run: f32,
    /// m/s of the `sprint` clip.
    pub sprint: f32,
    /// Slower than this (m/s) the character stands: `idle`.
    pub idle_below: f32,
    /// The clip never plays slower than this multiple of its authored speed...
    pub min_scale: f32,
    /// ...nor faster than this one.
    pub max_scale: f32,
}

impl Default for GroundSpeeds {
    fn default() -> Self {
        Self { walk: 2.0, run: 5.0, sprint: 8.0, idle_below: 0.1, min_scale: 0.6, max_scale: 1.6 }
    }
}

/// An animation set (content kind `animset`): which clips of a glTF file show which
/// locomotion state and which action, how they blend, and their cues.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct AnimSet {
    /// The glTF (`.gltf` or `.glb`) asset with the clips.
    pub source: String,
    /// The library: our clip name -> its definition.
    pub clips: BTreeMap<String, ClipDef>,
    /// Locomotion state ([`Locomotion::ALL`]) -> clip.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub locomotion: BTreeMap<String, String>,
    /// [`ActionDef`] id -> clip.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub actions: BTreeMap<String, String>,
    /// Action tag -> clip, for actions the set does not map by id (or `anim`).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub tags: BTreeMap<String, String>,
    /// Seconds to blend between locomotion clips.
    #[serde(default = "default_blend")]
    pub blend: f32,
    /// Seconds to blend into an action's clip: short, the action is already under way.
    #[serde(default = "default_action_blend")]
    pub action_blend: f32,
    #[serde(default)]
    pub speeds: GroundSpeeds,
}

fn default_blend() -> f32 {
    0.2
}

fn default_action_blend() -> f32 {
    0.06
}

/// What a character's body is doing between actions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Locomotion {
    Idle,
    Walk,
    Run,
    Sprint,
    /// Jumping or falling.
    Air,
    Swim,
    Glide,
    /// Climbing onto a ledge.
    Mantle,
    /// Reeling from a hit (the moveset's `stagger` action).
    Stagger,
    Dead,
}

impl Locomotion {
    pub const ALL: [Locomotion; 10] = [
        Locomotion::Idle,
        Locomotion::Walk,
        Locomotion::Run,
        Locomotion::Sprint,
        Locomotion::Air,
        Locomotion::Swim,
        Locomotion::Glide,
        Locomotion::Mantle,
        Locomotion::Stagger,
        Locomotion::Dead,
    ];

    /// The key in `locomotion:`.
    pub fn name(self) -> &'static str {
        match self {
            Locomotion::Idle => "idle",
            Locomotion::Walk => "walk",
            Locomotion::Run => "run",
            Locomotion::Sprint => "sprint",
            Locomotion::Air => "air",
            Locomotion::Swim => "swim",
            Locomotion::Glide => "glide",
            Locomotion::Mantle => "mantle",
            Locomotion::Stagger => "stagger",
            Locomotion::Dead => "dead",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }

    /// What to show when a set has no clip for this state, nearest first.
    pub fn fallbacks(self) -> &'static [Locomotion] {
        use Locomotion::*;
        match self {
            Idle | Stagger | Dead => &[],
            Walk => &[Run, Idle],
            Run => &[Walk, Idle],
            Sprint => &[Run, Walk, Idle],
            Air => &[Idle],
            Swim => &[Walk, Idle],
            Glide => &[Air, Idle],
            Mantle => &[Air, Idle],
        }
    }
}

impl AnimSet {
    /// The clip's definition.
    pub fn clip(&self, clip: &str) -> Option<&ClipDef> {
        self.clips.get(clip)
    }

    /// The animation's name in the glTF file for our clip name.
    pub fn file_name<'a>(&'a self, clip: &'a str) -> &'a str {
        self.clips.get(clip).and_then(|c| c.name.as_deref()).unwrap_or(clip)
    }

    /// Our clip for locomotion `state`, or the nearest fallback the set has, with the state
    /// it stands in for.
    pub fn locomotion_clip(&self, state: Locomotion) -> Option<(Locomotion, &str)> {
        std::iter::once(state).chain(state.fallbacks().iter().copied()).find_map(|s| {
            let clip = self.locomotion.get(s.name())?;
            self.clips.contains_key(clip).then_some((s, clip.as_str()))
        })
    }

    /// The clip for exactly this state, no fallbacks.
    fn exact(&self, state: Locomotion) -> Option<&str> {
        self.locomotion.get(state.name()).and_then(|clip| self.known(clip))
    }

    /// `clip` if the library has it.
    fn known<'a>(&self, clip: &'a str) -> Option<&'a str> {
        self.clips.contains_key(clip).then_some(clip)
    }

    /// Our clip for an action looked up by `key`: an id the set maps, a clip's own name (an
    /// action's `anim`), or a mapped tag.
    pub fn action_clip(&self, key: &str) -> Option<&str> {
        self.actions
            .get(key)
            .and_then(|clip| self.known(clip))
            .or_else(|| self.clips.get_key_value(key).map(|(k, _)| k.as_str()))
            .or_else(|| self.tags.get(key).and_then(|clip| self.known(clip)))
    }

    /// The key to look an action up under: its id if the set maps it, else its `anim` name
    /// if that is a clip of the set, else its first mapped tag; the id when nothing matches.
    pub fn action_key<'a>(&self, id: &'a str, anim: Option<&'a str>, tags: &'a [String]) -> &'a str {
        if self.actions.get(id).is_some_and(|c| self.clips.contains_key(c)) {
            return id;
        }
        if let Some(anim) = anim.filter(|a| self.clips.contains_key(*a)) {
            return anim;
        }
        tags.iter().map(String::as_str).find(|t| self.action_clip(t).is_some()).unwrap_or(id)
    }

    /// Seconds to blend into `clip`.
    pub fn blend_into(&self, clip: &str, action: bool) -> f32 {
        let own = self.clips.get(clip).and_then(|c| c.blend);
        own.unwrap_or(if action { self.action_blend } else { self.blend }).max(0.0)
    }

    /// The speed multiplier `clip` carries.
    fn clip_speed(&self, clip: &str) -> f32 {
        self.clips.get(clip).map_or(1.0, |c| c.speed)
    }
}

/// What to play now. `clip` is our name for it; the bridge maps it to a node of the graph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ClipChoice<'a> {
    /// `None`: the set has nothing for this moment; whatever is playing stays as it is.
    pub clip: Option<&'a str>,
    /// Gameplay's position in the clip, 0..1 of its length (an action's progress, a
    /// mantle's). `None`: the clip plays by itself.
    pub seek_to: Option<f32>,
    /// Playback speed; 0 while the clip is seeked (gameplay moves it).
    pub speed: f32,
    /// Seconds to blend from what played before.
    pub blend: f32,
    /// Loops (locomotion) or plays once and holds its last pose.
    pub looping: bool,
}

impl<'a> ClipChoice<'a> {
    fn none(set: &AnimSet) -> Self {
        Self { clip: None, seek_to: None, speed: 1.0, blend: set.blend.max(0.0), looping: false }
    }

    fn seeked(set: &AnimSet, clip: &'a str, progress: f32, action: bool) -> Self {
        Self {
            clip: Some(clip),
            seek_to: Some(progress),
            speed: 0.0,
            blend: set.blend_into(clip, action),
            looping: false,
        }
    }
}

/// What the body is, besides what [`MotionState`] says.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BodyStatus {
    /// The fighter has the `Dead` marker.
    pub dead: bool,
    /// The running action is the moveset's forced `stagger`.
    pub staggered: bool,
}

/// The locomotion state of a character: by motion mode, then by how fast it moves on the
/// ground (`idle` below `idle_below`, `walk` up to halfway to the run clip's speed, then
/// `run`, or `sprint` while the controller sprints).
pub fn locomotion_state(motion: &MotionState, speeds: &GroundSpeeds) -> Locomotion {
    match motion.mode {
        MotionMode::Water => Locomotion::Swim,
        MotionMode::Glide => Locomotion::Glide,
        MotionMode::Mantle => Locomotion::Mantle,
        MotionMode::Air => Locomotion::Air,
        MotionMode::Ground => {
            let v = Vec2::new(motion.velocity.x, motion.velocity.z).length();
            let walk_to_run = (speeds.walk + speeds.run) * 0.5;
            if v < speeds.idle_below {
                Locomotion::Idle
            } else if motion.sprinting && v > walk_to_run {
                Locomotion::Sprint
            } else if v < walk_to_run {
                Locomotion::Walk
            } else {
                Locomotion::Run
            }
        }
    }
}

/// How fast the clip for `state` plays so the feet match the ground speed.
fn ground_scale(state: Locomotion, motion: &MotionState, speeds: &GroundSpeeds) -> f32 {
    let authored = match state {
        Locomotion::Walk => speeds.walk,
        Locomotion::Run => speeds.run,
        Locomotion::Sprint => speeds.sprint,
        _ => return 1.0,
    };
    let v = Vec2::new(motion.velocity.x, motion.velocity.z).length();
    if authored <= EPS {
        return 1.0;
    }
    (v / authored).clamp(speeds.min_scale, speeds.max_scale.max(speeds.min_scale))
}

/// The clip a character should show, as a pure function of the action machine and the
/// controller. `action` is the running action as `(key, t, length)` in seconds, the key
/// being what [`AnimSet::action_key`] chose. Same as [`select_clip_for`] for a body that is
/// alive and not staggered.
///
/// - An action with a clip in the set is **seeked** to `t / length`: gameplay leads.
/// - Otherwise the body's state decides: ledge climbs are seeked to their progress, every
///   other locomotion clip loops with its speed following the velocity.
pub fn select_clip<'a>(action: Option<(&str, f32, f32)>, motion: &MotionState, set: &'a AnimSet) -> ClipChoice<'a> {
    select_clip_for(action, motion, BodyStatus::default(), set)
}

/// [`select_clip`] with the body's status: a dead body shows the `dead` clip (seeked while
/// its death action runs, then held on its last pose; no clip: the pose it has stays), a
/// staggered one the `stagger` clip seeked to the stagger action's progress.
pub fn select_clip_for<'a>(
    action: Option<(&str, f32, f32)>,
    motion: &MotionState,
    status: BodyStatus,
    set: &'a AnimSet,
) -> ClipChoice<'a> {
    let progress = action.map(|(_, t, len)| if len > EPS { (t / len).clamp(0.0, 1.0) } else { 1.0 });
    if let Some((id, ..)) = action
        && let Some(clip) = set.action_clip(id)
    {
        return ClipChoice::seeked(set, clip, progress.unwrap_or(0.0), true);
    }
    if status.dead {
        return match (set.exact(Locomotion::Dead), progress) {
            (Some(clip), Some(p)) => ClipChoice::seeked(set, clip, p, true),
            (Some(clip), None) => ClipChoice {
                clip: Some(clip),
                seek_to: None,
                speed: set.clip_speed(clip),
                blend: set.blend_into(clip, false),
                looping: false,
            },
            (None, _) => ClipChoice::none(set),
        };
    }
    if status.staggered
        && let (Some(clip), Some(p)) = (set.exact(Locomotion::Stagger), progress)
    {
        return ClipChoice::seeked(set, clip, p, true);
    }
    let state = locomotion_state(motion, &set.speeds);
    let Some((resolved, clip)) = set.locomotion_clip(state) else { return ClipChoice::none(set) };
    if resolved == Locomotion::Mantle
        && let Some(mantle) = &motion.mantle
    {
        return ClipChoice::seeked(set, clip, mantle.t.clamp(0.0, 1.0), false);
    }
    ClipChoice {
        clip: Some(clip),
        seek_to: None,
        speed: ground_scale(resolved, motion, &set.speeds) * set.clip_speed(clip),
        blend: set.blend_into(clip, false),
        looping: true,
    }
}

/// The cues the animation passed going from `prev` to `now` (seconds on the clip's
/// timeline), in time order.
///
/// - `prev` `None`: the clip just started; every cue at or before `now` fires.
/// - `now >= prev`: cues in `(prev, now]`.
/// - `now < prev`: it went back. A looping clip (`wrap_at`: its length) wrapped around: the
///   rest of the last pass and the start of the new one. Anything else restarted: cues up
///   to `now` fire again.
pub fn crossed_cues(cues: &[CueDef], prev: Option<f32>, now: f32, wrap_at: Option<f32>) -> Vec<&CueDef> {
    let mut found: Vec<&CueDef> = cues
        .iter()
        .filter(|c| match prev {
            None => c.at <= now + EPS,
            Some(p) if now >= p => c.at > p + EPS && c.at <= now + EPS,
            Some(p) => match wrap_at {
                Some(len) => (c.at > p + EPS && c.at <= len + EPS) || c.at <= now + EPS,
                None => c.at <= now + EPS,
            },
        })
        .collect();
    found.sort_by(|a, b| a.at.total_cmp(&b.at));
    found
}

/// `field -> unknown clip `x` (did you mean `y`?); clips: a, b`.
fn unknown_clip(refs: &mut Refs, names: &[&str], field: &str, clip: &str) {
    let hint = shards_content::db::closest(clip, names.iter().copied())
        .map(|c| format!(" (did you mean `{c}`?)"))
        .unwrap_or_default();
    refs.error(format!("{field} -> unknown clip `{clip}`{hint}; clips: {}", names.join(", ")));
}

impl ContentDef for AnimSet {
    const KIND: &'static str = "animset";

    fn validate(&self, _id: &str, refs: &mut Refs) {
        let ext = self.source.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
        if self.source.is_empty() {
            refs.error("source is empty; name the glTF with the clips, e.g. `source: \"models/hero.glb\"`");
        } else if !matches!(ext.as_deref(), Some("gltf" | "glb")) {
            refs.error(format!("source `{}` is not a glTF file (.gltf or .glb)", self.source));
        }
        if self.clips.is_empty() {
            refs.error("clips is empty; list the animations to use, e.g. `clips: {\"idle\": ()}`");
        }
        let names: Vec<&str> = self.clips.keys().map(String::as_str).collect();
        let known = |clip: &str| self.clips.contains_key(clip);
        for (name, clip) in &self.clips {
            if name.is_empty() {
                refs.error("a clip has no name");
            }
            if clip.name.as_deref().is_some_and(str::is_empty) {
                refs.error(format!("clips.{name}.name is empty; leave it out to use `{name}`"));
            }
            if !(clip.speed > 0.0 && clip.speed.is_finite()) {
                refs.error(format!("clips.{name}.speed must be > 0 (is {})", clip.speed));
            }
            if let Some(b) = clip.blend
                && !(b >= 0.0 && b.is_finite())
            {
                refs.error(format!("clips.{name}.blend must be >= 0 (is {b})"));
            }
            for (i, cue) in clip.cues.iter().enumerate() {
                if !(cue.at >= 0.0 && cue.at.is_finite()) {
                    refs.error(format!("clips.{name}.cues[{i}].at must be >= 0 (is {})", cue.at));
                }
                if cue.cue.is_empty() {
                    refs.error(format!("clips.{name}.cues[{i}].cue has no name"));
                }
            }
        }
        let states: Vec<&str> = Locomotion::ALL.iter().map(|s| s.name()).collect();
        for (state, clip) in &self.locomotion {
            if Locomotion::parse(state).is_none() {
                let hint = shards_content::db::closest(state, states.iter().copied())
                    .map(|c| format!(" (did you mean `{c}`?)"))
                    .unwrap_or_default();
                refs.error(format!("locomotion has an unknown state `{state}`{hint}; states: {}", states.join(", ")));
            }
            if !known(clip) {
                unknown_clip(refs, &names, &format!("locomotion.{state}"), clip);
            }
        }
        for (action, clip) in &self.actions {
            refs.require::<ActionDef>(action, "actions");
            if !known(clip) {
                unknown_clip(refs, &names, &format!("actions.{action}"), clip);
            }
        }
        for (tag, clip) in &self.tags {
            if tag.is_empty() {
                refs.error("tags has an empty tag");
            }
            if !known(clip) {
                unknown_clip(refs, &names, &format!("tags.{tag}"), clip);
            }
        }
        for (field, v) in [("blend", self.blend), ("action_blend", self.action_blend)] {
            if !(v >= 0.0 && v.is_finite()) {
                refs.error(format!("{field} must be >= 0 (is {v})"));
            }
        }
        let s = &self.speeds;
        for (field, v) in [("walk", s.walk), ("run", s.run), ("sprint", s.sprint)] {
            if !(v > 0.0 && v.is_finite()) {
                refs.error(format!("speeds.{field} must be > 0 (is {v})"));
            }
        }
        if !(s.idle_below >= 0.0 && s.idle_below.is_finite()) {
            refs.error(format!("speeds.idle_below must be >= 0 (is {})", s.idle_below));
        }
        if !(s.min_scale > 0.0 && s.max_scale >= s.min_scale && s.max_scale.is_finite()) {
            refs.error(format!(
                "speeds.min_scale and max_scale must satisfy 0 < min_scale <= max_scale (are {} and {})",
                s.min_scale, s.max_scale
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::Running;
    use shards_content::{ContentDb, ContentKinds};

    fn set() -> AnimSet {
        ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(
                r#"(
                source: "models/humanoid.glb",
                clips: {
                    "idle": (), "walk": (), "run": (speed: 1.1), "fall": (name: "Armature|Falling"),
                    "swim": (), "climb": (), "hit": (), "die": (),
                    "swing_1": (name: "Armature|Slash1", cues: [(at: 0.4, cue: "swoosh"), (at: 0.1, cue: "draw")]),
                    "roll": (blend: 0.04), "block": (), "generic_attack": (),
                },
                locomotion: {"idle": "idle", "walk": "walk", "run": "run", "air": "fall", "swim": "swim",
                             "mantle": "climb", "stagger": "hit", "dead": "die"},
                actions: {"light_1": "swing_1", "roll": "roll"},
                tags: {"attack": "generic_attack"},
            )"#,
            )
            .unwrap_or_else(|e| panic!("{e}"))
    }

    fn moving(v: f32) -> MotionState {
        MotionState { velocity: Vec3::new(0.0, 0.0, -v), grounded: true, ..default() }
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    #[test]
    fn standing_still_is_idle() {
        let set = set();
        let c = select_clip(None, &moving(0.0), &set);
        assert_eq!(c.clip, Some("idle"));
        assert_eq!(c.seek_to, None);
        assert!(c.looping && close(c.speed, 1.0) && close(c.blend, 0.2), "{c:?}");
        assert_eq!(select_clip(None, &moving(0.05), &set).clip, Some("idle"), "below idle_below");
    }

    #[test]
    fn ground_speed_picks_walk_or_run_and_scales_the_clip() {
        let set = set();
        let walk = select_clip(None, &moving(1.0), &set);
        assert_eq!(walk.clip, Some("walk"));
        assert!(close(walk.speed, 0.6), "1 m/s on a 2 m/s clip is clamped to min_scale: {}", walk.speed);
        let walk = select_clip(None, &moving(2.0), &set);
        assert!(close(walk.speed, 1.0), "{}", walk.speed);
        let jog = select_clip(None, &moving(3.4), &set);
        assert_eq!(jog.clip, Some("walk"), "below the midpoint 3.5 m/s it is still a walk");
        assert!(close(jog.speed, 1.6), "clamped to max_scale: {}", jog.speed);
        let run = select_clip(None, &moving(5.0), &set);
        assert_eq!(run.clip, Some("run"));
        assert!(close(run.speed, 1.1), "the clip's own speed 1.1 times 5/5: {}", run.speed);
        let fast = select_clip(None, &moving(6.0), &set);
        assert!(close(fast.speed, 1.2 * 1.1), "{}", fast.speed);
    }

    #[test]
    fn sprinting_falls_back_to_run_without_a_sprint_clip() {
        let mut set = set();
        let sprinting = MotionState { sprinting: true, ..moving(8.0) };
        let c = select_clip(None, &sprinting, &set);
        assert_eq!(c.clip, Some("run"), "no sprint clip: the run clip, sped up");
        assert!(close(c.speed, 1.6 * 1.1), "authored for sprint speed 8 m/s -> run clip scaled by 8/5, capped: {c:?}");
        set.clips.insert("sprint".into(), ClipDef::default());
        set.locomotion.insert("sprint".into(), "sprint".into());
        let c = select_clip(None, &sprinting, &set);
        assert_eq!(c.clip, Some("sprint"));
        assert!(close(c.speed, 1.0), "{c:?}");
        // The sprint flag at walking pace is still a walk.
        let slow = MotionState { sprinting: true, ..moving(1.5) };
        assert_eq!(select_clip(None, &slow, &set).clip, Some("walk"));
    }

    #[test]
    fn other_modes_have_their_own_clips() {
        let set = set();
        let mode = |mode| MotionState { mode, velocity: Vec3::new(1.0, -3.0, 0.0), ..default() };
        assert_eq!(select_clip(None, &mode(MotionMode::Air), &set).clip, Some("fall"));
        assert_eq!(select_clip(None, &mode(MotionMode::Water), &set).clip, Some("swim"));
        // No glide clip: the air clip stands in.
        assert_eq!(select_clip(None, &mode(MotionMode::Glide), &set).clip, Some("fall"));
    }

    #[test]
    fn a_ledge_climb_is_seeked_to_its_progress() {
        let set = set();
        let mantle = shards_motion::Mantle { from: Vec3::ZERO, corner: Vec3::Y, to: Vec3::ONE, t: 0.4, duration: 0.8 };
        let motion = MotionState { mode: MotionMode::Mantle, mantle: Some(mantle), ..default() };
        let c = select_clip(None, &motion, &set);
        assert_eq!(c.clip, Some("climb"));
        assert_eq!(c.seek_to, Some(0.4));
        assert!(close(c.speed, 0.0) && !c.looping, "{c:?}");
    }

    #[test]
    fn a_mapped_action_is_seeked_to_gameplay_time() {
        let set = set();
        let c = select_clip(Some(("light_1", 0.15, 0.6)), &moving(5.0), &set);
        assert_eq!(c.clip, Some("swing_1"), "the action wins over locomotion");
        assert!(close(c.seek_to.unwrap(), 0.25), "{c:?}");
        assert!(close(c.speed, 0.0) && !c.looping, "gameplay moves it, the player does not");
        assert!(close(c.blend, 0.06), "action blend: {}", c.blend);
        let end = select_clip(Some(("light_1", 0.7, 0.6)), &moving(0.0), &set);
        assert_eq!(end.seek_to, Some(1.0), "past the end it holds the last pose");
        let roll = select_clip(Some(("roll", 0.0, 0.4)), &moving(0.0), &set);
        assert!(close(roll.blend, 0.04), "the clip's own blend: {}", roll.blend);
        assert_eq!(select_clip(Some(("roll", 0.1, 0.0)), &moving(0.0), &set).seek_to, Some(1.0), "zero length");
    }

    #[test]
    fn an_unmapped_action_leaves_locomotion_to_play() {
        let set = set();
        let c = select_clip(Some(("mystery", 0.1, 0.5)), &moving(2.0), &set);
        assert_eq!(c.clip, Some("walk"));
        assert_eq!(c.seek_to, None);
    }

    #[test]
    fn actions_are_found_by_id_clip_name_or_tag() {
        let set = set();
        assert_eq!(set.action_clip("light_1"), Some("swing_1"), "by id");
        assert_eq!(set.action_clip("swing_1"), Some("swing_1"), "by the clip's own name (an action's `anim`)");
        assert_eq!(set.action_clip("attack"), Some("generic_attack"), "by tag");
        assert_eq!(set.action_clip("nothing"), None);
        let tags = ["fire".to_owned(), "attack".to_owned()];
        assert_eq!(set.action_key("heavy", None, &tags), "attack", "the first tag the set maps");
        assert_eq!(set.action_key("heavy", Some("block"), &tags), "block", "the action's anim beats its tags");
        assert_eq!(set.action_key("light_1", Some("block"), &tags), "light_1", "the id beats everything");
        assert_eq!(set.action_key("heavy", Some("not_a_clip"), &[]), "heavy", "nothing matches: the id");
        let c = select_clip(Some((set.action_key("heavy", None, &tags), 0.1, 0.5)), &moving(0.0), &set);
        assert_eq!(c.clip, Some("generic_attack"));
    }

    #[test]
    fn dead_bodies_show_the_dead_clip() {
        let set = set();
        let dead = BodyStatus { dead: true, ..default() };
        // While the death action runs (unmapped), the dead clip is seeked to its progress.
        let dying = select_clip_for(Some(("death", 0.5, 2.0)), &moving(0.0), dead, &set);
        assert_eq!((dying.clip, dying.seek_to), (Some("die"), Some(0.25)));
        // Afterwards it plays once and holds the last pose.
        let after = select_clip_for(None, &moving(0.0), dead, &set);
        assert_eq!(after.clip, Some("die"));
        assert!(after.seek_to.is_none() && !after.looping, "{after:?}");
        // Without a dead clip nothing changes: the last pose stays.
        let mut bare = set.clone();
        bare.locomotion.remove("dead");
        assert_eq!(select_clip_for(None, &moving(3.0), dead, &bare).clip, None);
        // A mapped death action still wins while it runs.
        let mut mapped = set;
        mapped.actions.insert("death".into(), "roll".into());
        assert_eq!(select_clip_for(Some(("death", 0.1, 1.0)), &moving(0.0), dead, &mapped).clip, Some("roll"));
    }

    #[test]
    fn staggering_seeks_the_stagger_clip() {
        let set = set();
        let status = BodyStatus { staggered: true, ..default() };
        let c = select_clip_for(Some(("stagger", 0.3, 0.6)), &moving(0.0), status, &set);
        assert_eq!((c.clip, c.seek_to), (Some("hit"), Some(0.5)));
        // Not staggered: the same unmapped action leaves locomotion alone.
        assert_eq!(select_clip(Some(("stagger", 0.3, 0.6)), &moving(0.0), &set).clip, Some("idle"));
        // A staggered body without a stagger action running has nothing to follow.
        assert_eq!(select_clip_for(None, &moving(0.0), status, &set).clip, Some("idle"));
    }

    #[test]
    fn an_empty_set_chooses_nothing() {
        let mut set = set();
        set.locomotion.clear();
        assert_eq!(select_clip(None, &moving(3.0), &set).clip, None);
        // A mapping to a clip the library lacks is ignored too (validation reports it).
        set.locomotion.insert("idle".into(), "nope".into());
        assert_eq!(select_clip(None, &moving(0.0), &set).clip, None);
    }

    #[test]
    fn locomotion_fallbacks_chain() {
        let mut set = set();
        set.locomotion.retain(|k, _| k == "idle" || k == "walk");
        assert_eq!(set.locomotion_clip(Locomotion::Sprint), Some((Locomotion::Walk, "walk")));
        assert_eq!(set.locomotion_clip(Locomotion::Run), Some((Locomotion::Walk, "walk")));
        assert_eq!(set.locomotion_clip(Locomotion::Swim), Some((Locomotion::Walk, "walk")));
        assert_eq!(set.locomotion_clip(Locomotion::Glide), Some((Locomotion::Idle, "idle")));
        assert_eq!(set.locomotion_clip(Locomotion::Dead), None, "dead never falls back");
        assert_eq!(Locomotion::parse("sprint"), Some(Locomotion::Sprint));
        assert_eq!(Locomotion::parse("jog"), None);
        for s in Locomotion::ALL {
            assert_eq!(Locomotion::parse(s.name()), Some(s));
        }
    }

    #[test]
    fn cues_fire_once_as_time_passes() {
        let cues = set().clips["swing_1"].cues.clone();
        let at: Vec<f32> = crossed_cues(&cues, None, 0.5, None).iter().map(|c| c.at).collect();
        assert_eq!(at, [0.1, 0.4], "a fresh clip fires what is already behind it, in time order");
        assert!(crossed_cues(&cues, Some(0.5), 0.6, None).is_empty());
        let step: Vec<&str> = crossed_cues(&cues, Some(0.05), 0.15, None).iter().map(|c| c.cue.as_str()).collect();
        assert_eq!(step, ["draw"]);
        assert!(crossed_cues(&cues, Some(0.1), 0.1, None).is_empty(), "standing still fires nothing");
        assert_eq!(crossed_cues(&cues, Some(0.3), 0.4, None).len(), 1, "a cue exactly at `now` fires");
        assert!(crossed_cues(&cues, Some(0.4), 0.5, None).is_empty(), "and not again");
    }

    #[test]
    fn cues_survive_loops_and_restarts() {
        let cues = set().clips["swing_1"].cues.clone();
        // A loop of length 1.0 wrapped from 0.9 to 0.2: the tail has nothing, the head has `draw`.
        let wrapped: Vec<&str> = crossed_cues(&cues, Some(0.9), 0.2, Some(1.0)).iter().map(|c| c.cue.as_str()).collect();
        assert_eq!(wrapped, ["draw"]);
        // From 0.3 round to 0.05: the rest of the pass has `swoosh`.
        let wrapped: Vec<&str> = crossed_cues(&cues, Some(0.3), 0.05, Some(1.0)).iter().map(|c| c.cue.as_str()).collect();
        assert_eq!(wrapped, ["swoosh"]);
        // An action clip that restarts (same clip, new swing) fires from its start.
        let restarted: Vec<&str> = crossed_cues(&cues, Some(0.55), 0.12, None).iter().map(|c| c.cue.as_str()).collect();
        assert_eq!(restarted, ["draw"]);
    }

    #[test]
    fn file_names_default_to_our_own() {
        let set = set();
        assert_eq!(set.file_name("fall"), "Armature|Falling");
        assert_eq!(set.file_name("idle"), "idle");
        assert_eq!(set.file_name("unlisted"), "unlisted");
    }

    fn build(animsets: &str) -> Vec<String> {
        let mut kinds = ContentKinds::default();
        kinds.register::<ActionDef>();
        kinds.register::<AnimSet>();
        kinds.register::<crate::def::MovesetDef>();
        kinds.register::<crate::status::StatusDef>();
        let actions = r#"{ "light_1": (net_id: 1, phases: [("active", 0.3)]), "roll": (net_id: 2, phases: [("roll", 0.4)]) }"#;
        let sources = [
            shards_content::db::Source { path: "content/a.action.ron".into(), text: actions.into() },
            shards_content::db::Source { path: "content/h.animset.ron".into(), text: animsets.into() },
        ];
        ContentDb::build(&kinds, &sources).errors().map(ToString::to_string).collect()
    }

    #[test]
    fn the_example_in_the_docs_is_valid_content() {
        // The RON of the module docs, with the actions it names.
        let docs = include_str!("anim.rs");
        let start = docs.find("//!     \"humanoid\": (").expect("the example");
        let end = docs[start..].find("//! }\n").expect("its end") + start;
        let body: String = docs[start..end].lines().map(|l| l.trim_start_matches("//!").to_owned() + "\n").collect();
        let errors = build(&format!("{{\n{body}}}"));
        // The example names `light_1` and `roll`, which the helper's content has too.
        assert!(errors.is_empty(), "{errors:#?}");
    }

    #[test]
    fn a_good_animset_validates() {
        let errors = build(
            r#"{ "h": (source: "models/h.glb", clips: {"idle": (), "slash": (name: "Slash", cues: [(at: 0.2, cue: "x")])},
                locomotion: {"idle": "idle"}, actions: {"light_1": "slash"}, tags: {"fire": "slash"}) }"#,
        );
        assert!(errors.is_empty(), "{errors:#?}");
    }

    #[test]
    fn validation_names_the_field_and_the_fix() {
        let errors = build(
            r#"{
            "bad": (source: "models/h.png", clips: {"idle": (speed: 0), "walk": (blend: -1, cues: [(at: -1, cue: "")])},
                locomotion: {"idel": "idle", "walk": "wlak", "run": "idle"},
                actions: {"light_2": "idle", "roll": "dance"}, tags: {"fire": "nope"},
                blend: -0.1, speeds: (walk: 0, min_scale: 2, max_scale: 1)),
            "empty": (source: "", clips: {}),
        }"#,
        );
        let expected = [
            "content/h.animset.ron: animset `bad`: source `models/h.png` is not a glTF file (.gltf or .glb)",
            "content/h.animset.ron: animset `bad`: clips.idle.speed must be > 0 (is 0)",
            "content/h.animset.ron: animset `bad`: clips.walk.blend must be >= 0 (is -1)",
            "content/h.animset.ron: animset `bad`: clips.walk.cues[0].at must be >= 0 (is -1)",
            "content/h.animset.ron: animset `bad`: clips.walk.cues[0].cue has no name",
            "content/h.animset.ron: animset `bad`: locomotion has an unknown state `idel` (did you mean `idle`?); states: idle, walk, run, sprint, air, swim, glide, mantle, stagger, dead",
            "content/h.animset.ron: animset `bad`: locomotion.walk -> unknown clip `wlak` (did you mean `walk`?); clips: idle, walk",
            "content/h.animset.ron: animset `bad`: actions -> unknown action `light_2` (did you mean `light_1`?)",
            "content/h.animset.ron: animset `bad`: actions.roll -> unknown clip `dance`; clips: idle, walk",
            "content/h.animset.ron: animset `bad`: tags.fire -> unknown clip `nope`; clips: idle, walk",
            "content/h.animset.ron: animset `bad`: blend must be >= 0 (is -0.1)",
            "content/h.animset.ron: animset `bad`: speeds.walk must be > 0 (is 0)",
            "content/h.animset.ron: animset `bad`: speeds.min_scale and max_scale must satisfy 0 < min_scale <= max_scale (are 2 and 1)",
            "content/h.animset.ron: animset `empty`: source is empty; name the glTF with the clips, e.g. `source: \"models/hero.glb\"`",
            "content/h.animset.ron: animset `empty`: clips is empty; list the animations to use, e.g. `clips: {\"idle\": ()}`",
        ];
        assert_eq!(errors, expected);
    }

    #[test]
    fn progress_comes_from_the_running_action() {
        // The bridge feeds `select_clip` from `Running`: t and length in seconds.
        let def: ActionDef = ron::Options::default()
            .from_str(r#"(net_id: 1, phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)])"#)
            .unwrap();
        let mut run = Running::new("light_1", &def, 0, Vec3::NEG_Z, None, Some("light".into()));
        run.t = 0.3;
        let set = set();
        let c = select_clip(Some((&run.action, run.t, run.length)), &moving(0.0), &set);
        assert!(close(c.seek_to.unwrap(), 0.5), "{c:?}");
    }
}
