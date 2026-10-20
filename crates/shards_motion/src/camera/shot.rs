//! Scene cameras (K3): a shot is a short camera move a game plays over its third-person camera, a
//! scene of half a minute or a boss's entrance of a few seconds (GDD §6.3, §15.1).
//!
//! ```ron
//! // content/scenes/warden_intro.shot.ron
//! {
//!     "warden_intro": (
//!         keys: [
//!             (at: 0.0, eye: On(role: "boss", offset: (0.0, 2.0, -7.0), local: true), look: On(role: "boss", offset: (0.0, 3.0, 0.0))),
//!             (at: 4.0, eye: On(role: "boss", offset: (2.0, 1.0, -4.0), local: true), look: On(role: "boss", offset: (0.0, 4.0, 0.0)),
//!              fov_deg: 45.0, ease: InOut),
//!             (at: 4.5, eye: At((0.0, 3.0, 12.0)), look: On(role: "hero"), cut: true),
//!             (at: 6.0, eye: At((0.0, 2.0, 9.0)), look: On(role: "hero", offset: (0.0, 1.5, 0.0))),
//!         ],
//!         blend_in_s: 0.5,
//!         blend_out_s: 0.8,
//!     ),
//! }
//! ```
//!
//! - **Keys.** Each key says where the camera stands (`eye`) and what it looks at (`look`) at `at`
//!   seconds from the start, and optionally its field of view (`fov_deg`; without one, the
//!   player's). Between two keys the camera moves from one to the other, `ease` (`Linear` or
//!   `InOut`) shaping the move into the later one; a key that is a `cut` is jumped to at its time
//!   (the camera holds the key before it until then). Before the first key the camera holds the
//!   first; the shot ends at the last.
//! - **Points.** `At((x, y, z))` is a place in the world. `On(role:, offset:, local:)` is a place
//!   on a body the game names when it plays the shot (`boss`, `hero`): its position plus `offset`,
//!   turned with the body when `local` (its forward is `-z`). A body that goes away while the shot
//!   plays holds the last place it was seen.
//! - **Playing.** [`play`] starts a shot on this machine's camera; [`stop`] skips it (refused for
//!   one that is not `skippable`), [`cancel`] ends any. A shot blends in from what the camera
//!   showed over `blend_in_s` and back to the orbit camera over `blend_out_s`, at its end or when
//!   stopped. A second [`play`] ends the first as skipped.
//! - **The camera meanwhile.** The orbit camera stands still: its yaw and pitch are kept, so the
//!   move axes keep their meaning, and a lock-on picks and switches nothing (a lock the player
//!   had is framed again after the shot). Shake and the game's offsets do not touch the shot.
//! - **Time** is `Time<Virtual>`: the pause menu pauses a shot, a slowed game slows it.
//! - **Messages.** [`ShotStarted`] and [`ShotEnded`] (`skipped` when stopped before its last key)
//!   are triggered as observer events, and each writes a `Local` journal line; `observe` shows the
//!   shot playing.
//! - **What stays the game's.** Blocking the player's input during a scene, letterbox bars and
//!   subtitles, pausing or slowing the simulation, which button skips (it calls [`stop`]).
//! - **Presentation only.** The simulation never reads the camera; nothing of a shot is in a
//!   snapshot or a replay. A headless game has no camera, and [`play`] refuses there.

use std::collections::{BTreeMap, BTreeSet};

use ai_core::tools::{register_tool, ToolKind};
use ai_core::{journal, AiAppExt, Call, Id, JournalKind, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use shards_content::{ContentAppExt, ContentDb, ContentDef, Refs};

/// The longest a shot lasts (s): a scene of the GDD is at most 30 to 40 s.
pub const SHOT_MAX_S: f32 = 120.0;

/// The longest blend in or out (s).
pub const BLEND_MAX_S: f32 = 10.0;

/// A place a shot's camera stands at or looks at.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum ShotPoint {
    /// A place in the world.
    At(Vec3),
    /// A place on a body the game names when it plays the shot.
    On {
        /// The role's name (`boss`): [`play`] is given a body for each.
        role: String,
        /// From the body's position (m).
        #[serde(default)]
        offset: Vec3,
        /// The offset turns with the body (its forward is `-z`); otherwise it is in the world's axes.
        #[serde(default)]
        local: bool,
    },
}

impl ShotPoint {
    /// Where it is, with the roles' bodies where they are (a role not given: the offset alone).
    pub fn resolve(&self, roles: &BTreeMap<String, Transform>) -> Vec3 {
        match self {
            ShotPoint::At(at) => *at,
            ShotPoint::On { role, offset, local } => match roles.get(role) {
                Some(body) => body.translation + if *local { body.rotation * *offset } else { *offset },
                None => *offset,
            },
        }
    }

    fn role(&self) -> Option<&str> {
        match self {
            ShotPoint::At(_) => None,
            ShotPoint::On { role, .. } => Some(role),
        }
    }
}

/// How the move into a key eases.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Ease {
    /// At an even pace.
    #[default]
    Linear,
    /// Slow out of the key before and slow into this one (smoothstep).
    InOut,
}

impl Ease {
    /// `s` (0..1 of the way) eased.
    pub fn apply(self, s: f32) -> f32 {
        let s = s.clamp(0.0, 1.0);
        match self {
            Ease::Linear => s,
            Ease::InOut => s * s * (3.0 - 2.0 * s),
        }
    }
}

/// One key of a shot.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ShotKey {
    /// When (s from the shot's start).
    pub at: f32,
    /// Where the camera stands.
    pub eye: ShotPoint,
    /// What it looks at.
    pub look: ShotPoint,
    /// The field of view (degrees, 10 to 120). `None`: the player's.
    #[serde(default)]
    pub fov_deg: Option<f32>,
    /// How the move from the key before eases into this one.
    #[serde(default)]
    pub ease: Ease,
    /// The camera jumps to this key at its time instead of moving to it.
    #[serde(default)]
    pub cut: bool,
}

/// A scene camera shot (content kind `shot`, `*.shot.ron`). See the [module](self).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ShotDef {
    /// In time order, at least one.
    pub keys: Vec<ShotKey>,
    /// From what the camera showed to the shot (s).
    pub blend_in_s: f32,
    /// From the shot back to the orbit camera (s).
    pub blend_out_s: f32,
    /// Whether [`stop`] (the game's skip button) may end it early.
    pub skippable: bool,
}

impl Default for ShotDef {
    fn default() -> Self {
        Self { keys: Vec::new(), blend_in_s: 0.0, blend_out_s: 0.5, skippable: true }
    }
}

impl ContentDef for ShotDef {
    const KIND: &'static str = "shot";
    // A camera move is only a view: the simulation never reads it.
    const SIMULATION: bool = false;

    fn validate(&self, _id: &str, refs: &mut Refs) {
        for problem in self.problems() {
            refs.error(problem);
        }
    }
}

impl ShotDef {
    /// How long its keys last (s): the last key's time.
    pub fn duration(&self) -> f32 {
        self.keys.last().map_or(0.0, |k| k.at)
    }

    /// The roles its keys name, each once.
    pub fn roles(&self) -> BTreeSet<&str> {
        self.keys.iter().flat_map(|k| [k.eye.role(), k.look.role()]).flatten().collect()
    }

    /// What is wrong with it, one line each; empty when it is fine.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        if self.keys.is_empty() {
            out.push("keys: none; a shot needs at least one".to_owned());
        }
        let mut before: Option<f32> = None;
        for (i, key) in self.keys.iter().enumerate() {
            let name = format!("keys[{i}]");
            if !key.at.is_finite() || key.at < 0.0 {
                out.push(format!("{name}.at {}: seconds from the start, 0 or more", key.at));
            } else {
                if let Some(before) = before
                    && key.at <= before
                {
                    out.push(format!("{name}.at {}: not after the key before it ({before}); keys go in time order", key.at));
                }
                if key.at > SHOT_MAX_S {
                    out.push(format!("{name}.at {}: a shot lasts at most {SHOT_MAX_S} s", key.at));
                }
                before = Some(key.at);
            }
            for (what, point) in [("eye", &key.eye), ("look", &key.look)] {
                match point {
                    ShotPoint::At(at) if !at.is_finite() => out.push(format!("{name}.{what}: finite numbers, not {at}")),
                    ShotPoint::On { role, offset, .. } => {
                        if role.trim().is_empty() {
                            out.push(format!("{name}.{what}: an empty role; name the body the game gives (`boss`)"));
                        }
                        if !offset.is_finite() {
                            out.push(format!("{name}.{what}.offset: finite numbers, not {offset}"));
                        }
                    }
                    ShotPoint::At(_) => {}
                }
            }
            if let (ShotPoint::At(eye), ShotPoint::At(look)) = (&key.eye, &key.look)
                && eye == look
            {
                out.push(format!("{name}: `eye` and `look` are the same place; the camera has no direction"));
            }
            if let Some(fov) = key.fov_deg
                && !(fov.is_finite() && (10.0..=120.0).contains(&fov))
            {
                out.push(format!("{name}.fov_deg {fov}: 10 to 120 degrees"));
            }
        }
        for (name, s) in [("blend_in_s", self.blend_in_s), ("blend_out_s", self.blend_out_s)] {
            if !(s.is_finite() && (0.0..=BLEND_MAX_S).contains(&s)) {
                out.push(format!("{name} {s}: 0 to {BLEND_MAX_S} s"));
            }
        }
        out
    }
}

/// Where a shot's camera is at one moment.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotFrame {
    pub eye: Vec3,
    pub look: Vec3,
    /// Degrees.
    pub fov_deg: f32,
}

/// A camera's place, turn and field of view (radians): what a blend mixes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShotPose {
    pub at: Vec3,
    pub rotation: Quat,
    pub fov: f32,
}

impl ShotFrame {
    /// The camera at `eye` looking at `look` (up is `+y`).
    pub fn pose(&self) -> ShotPose {
        let tf = Transform::from_translation(self.eye).looking_at(self.look, Vec3::Y);
        ShotPose { at: tf.translation, rotation: tf.rotation, fov: self.fov_deg.to_radians() }
    }
}

/// `def` at `t` seconds, its roles' bodies at `roles`, a key without a field of view at
/// `base_fov_deg`. See the [module](self) for holds and cuts.
pub fn sample(def: &ShotDef, t: f32, roles: &BTreeMap<String, Transform>, base_fov_deg: f32) -> ShotFrame {
    let frame = |key: &ShotKey| ShotFrame {
        eye: key.eye.resolve(roles),
        look: key.look.resolve(roles),
        fov_deg: key.fov_deg.unwrap_or(base_fov_deg),
    };
    let Some(first) = def.keys.first() else {
        return ShotFrame { eye: Vec3::ZERO, look: Vec3::NEG_Z, fov_deg: base_fov_deg };
    };
    let Some(i) = def.keys.iter().rposition(|k| k.at <= t) else {
        return frame(first);
    };
    let from = &def.keys[i];
    match def.keys.get(i + 1) {
        None => frame(from),
        Some(to) if to.cut => frame(from),
        Some(to) => {
            let s = to.ease.apply((t - from.at) / (to.at - from.at).max(1e-6));
            let (a, b) = (frame(from), frame(to));
            ShotFrame { eye: a.eye.lerp(b.eye, s), look: a.look.lerp(b.look, s), fov_deg: a.fov_deg + (b.fov_deg - a.fov_deg) * s }
        }
    }
}

/// `a` turned into `b`, `s` (0..1) of the way, eased in and out.
pub fn blend(a: ShotPose, b: ShotPose, s: f32) -> ShotPose {
    let s = Ease::InOut.apply(s);
    ShotPose { at: a.at.lerp(b.at, s), rotation: a.rotation.slerp(b.rotation, s), fov: a.fov + (b.fov - a.fov) * s }
}

/// A shot started.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct ShotStarted {
    pub id: String,
}

/// A shot ended: its blend back to the orbit camera is done. `skipped`: it was stopped (or
/// another took its place) before its last key.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct ShotEnded {
    pub id: String,
    pub skipped: bool,
}

/// The shot playing, if any (one at a time).
#[derive(Resource, Default)]
pub struct Shots {
    playing: Option<Playing>,
}

#[cfg_attr(not(feature = "render"), allow(dead_code, reason = "shown by `show_shot`, which needs a window"))]
struct Playing {
    id: String,
    def: ShotDef,
    roles: BTreeMap<String, Entity>,
    /// Where each role's body was last seen: a body that goes holds its last place.
    seen: BTreeMap<String, Transform>,
    /// `Time<Virtual>` when it started (s).
    started: f64,
    /// What each camera showed when it started: the blend in comes from it.
    from: BTreeMap<Entity, ShotPose>,
    /// What each camera showed on the last frame.
    shown: BTreeMap<Entity, ShotPose>,
    ending: Option<Ending>,
}

/// The blend back to the orbit camera.
#[cfg_attr(not(feature = "render"), allow(dead_code, reason = "shown by `show_shot`, which needs a window"))]
struct Ending {
    at: f64,
    skipped: bool,
    from: BTreeMap<Entity, ShotPose>,
}

impl Shots {
    /// The id of the shot playing (its blend out too).
    pub fn playing(&self) -> Option<&str> {
        self.playing.as_ref().map(|p| p.id.as_str())
    }
}

/// Whether a shot plays: the orbit camera stands still and lock-on picks nothing meanwhile.
pub fn shot_playing(shots: Option<Res<Shots>>) -> bool {
    shots.is_some_and(|s| s.playing.is_some())
}

pub(super) fn plugin(app: &mut App) {
    app.add_content::<ShotDef>().init_resource::<Shots>().register_ai_observe_section(observe_section);
    register_tool(app.world_mut(), "play_shot", Scope::Dev, ToolKind::Control, play_shot);
    register_tool(app.world_mut(), "stop_shot", Scope::Dev, ToolKind::Control, stop_shot);
    #[cfg(feature = "render")]
    app.add_systems(
        PostUpdate,
        show_shot.after(super::CameraSystems::Orbit).before(bevy::transform::TransformSystems::Propagate),
    );
}

fn now(world: &World) -> f64 {
    world.get_resource::<Time<Virtual>>().map_or(0.0, |t| t.elapsed_secs_f64())
}

/// What each camera shows now: its place, turn and field of view.
#[cfg(feature = "render")]
fn cameras_now(world: &mut World) -> BTreeMap<Entity, ShotPose> {
    let mut q = world.query_filtered::<(Entity, &Transform, Option<&bevy::camera::Projection>), With<super::ThirdPersonCamera>>();
    q.iter(world)
        .map(|(e, tf, projection)| {
            let fov = match projection {
                Some(bevy::camera::Projection::Perspective(p)) => p.fov,
                _ => std::f32::consts::FRAC_PI_4,
            };
            (e, ShotPose { at: tf.translation, rotation: tf.rotation, fov })
        })
        .collect()
}

#[cfg(not(feature = "render"))]
fn cameras_now(_world: &mut World) -> BTreeMap<Entity, ShotPose> {
    BTreeMap::new()
}

/// Plays shot `id` on this machine's camera, its roles' bodies `roles` (`boss` → the boss).
/// Refused for a shot that is missing or has problems, a role not given, a body that does not
/// exist, and where nothing is drawn (no `ThirdPersonCamera`: a headless game). A shot already
/// playing ends as skipped.
pub fn play(world: &mut World, id: &str, roles: &BTreeMap<String, Entity>) -> Result<(), String> {
    let def = {
        let db = world.get_resource::<ContentDb>().ok_or("no content is loaded")?;
        let Some(def) = db.get::<ShotDef>(id) else {
            let ids: Vec<&str> = db.ids::<ShotDef>().collect();
            let ids = if ids.is_empty() { "none".to_owned() } else { ids.join(", ") };
            return Err(format!("no shot `{id}`; shots: {ids}"));
        };
        def.clone()
    };
    if let Some(problem) = def.problems().first() {
        return Err(format!("shot `{id}` cannot play: {problem}"));
    }
    let wanted = def.roles();
    if let Some(missing) = wanted.iter().find(|r| !roles.contains_key(**r)) {
        let all: Vec<&str> = wanted.iter().copied().collect();
        return Err(format!("shot `{id}` follows role `{missing}`: give a body for each of {} (`role.{missing}=eN`)", all.join(", ")));
    }
    if let Some(extra) = roles.keys().find(|r| !wanted.contains(r.as_str())) {
        return Err(format!("shot `{id}` has no role `{extra}`; its roles: {}", wanted.iter().copied().collect::<Vec<_>>().join(", ")));
    }
    let mut seen = BTreeMap::new();
    for (role, &body) in roles {
        let tf = world.get::<Transform>(body).ok_or_else(|| format!("role `{role}`: that body does not exist (or has no place)"))?;
        seen.insert(role.clone(), *tf);
    }
    let from = cameras_now(world);
    if from.is_empty() {
        return Err("no camera to play a shot on: a shot plays where the game is drawn (a window or --offscreen); a headless game has none"
            .to_owned());
    }
    if world.resource::<Shots>().playing.is_some() {
        end(world, true);
    }
    let started = now(world);
    world.resource_mut::<Shots>().playing = Some(Playing {
        id: id.to_owned(),
        def: def.clone(),
        roles: roles.clone(),
        seen,
        started,
        shown: from.clone(),
        from,
        ending: None,
    });
    let roles: Vec<String> = roles.iter().map(|(r, e)| format!("{r}={}", ai_core::label(world, *e))).collect();
    let roles = if roles.is_empty() { String::new() } else { format!(", {}", roles.join(" ")) };
    journal::emit(world, JournalKind::Local, format!("shot {id} starts: {:.1}s{roles}", def.duration()));
    world.trigger(ShotStarted { id: id.to_owned() });
    Ok(())
}

/// The game's skip: the shot playing blends back to the orbit camera now and ends as skipped.
/// Refused when none plays or it is not `skippable` ([`cancel`] ends any).
pub fn stop(world: &mut World) -> Result<String, String> {
    let shots = world.resource::<Shots>();
    let Some(playing) = &shots.playing else {
        return Err("no shot is playing".to_owned());
    };
    if !playing.def.skippable {
        return Err(format!("shot `{}` is not skippable (`skippable: false`); `cancel` ends it anyway", playing.id));
    }
    Ok(begin_ending(world))
}

/// Ends the shot playing, skippable or not: it blends back to the orbit camera now. `false` when
/// none plays.
pub fn cancel(world: &mut World) -> bool {
    if world.resource::<Shots>().playing.is_none() {
        return false;
    }
    begin_ending(world);
    true
}

fn begin_ending(world: &mut World) -> String {
    let at = now(world);
    let (id, blend) = {
        let mut shots = world.resource_mut::<Shots>();
        let Some(playing) = shots.playing.as_mut() else { return String::new() };
        if playing.ending.is_none() {
            playing.ending = Some(Ending { at, skipped: true, from: playing.shown.clone() });
        }
        (playing.id.clone(), playing.def.blend_out_s)
    };
    if blend <= 0.0 {
        end(world, true);
    }
    format!("shot {id} stopped: back to the orbit camera over {blend:.1}s")
}

/// Takes the shot away and says it ended.
fn end(world: &mut World, skipped: bool) {
    let Some(playing) = world.resource_mut::<Shots>().playing.take() else { return };
    let skipped = playing.ending.as_ref().map_or(skipped, |e| e.skipped);
    let how = if skipped { "skipped" } else { "done" };
    journal::emit(world, JournalKind::Local, format!("shot {} ends ({how})", playing.id));
    world.trigger(ShotEnded { id: playing.id, skipped });
}

/// What a playing shot shows on camera `e` at `t` s in, where the shot itself is at `shot`: for
/// its first `blend_in_s`, the blend in from what the camera showed when the shot started.
#[cfg(feature = "render")]
fn blended_in(playing: &Playing, e: Entity, t: f32, shot: ShotPose) -> ShotPose {
    let blend_in = playing.def.blend_in_s;
    match playing.from.get(&e) {
        Some(from) if blend_in > 0.0 && t < blend_in => blend(*from, shot, t / blend_in),
        _ => shot,
    }
}

/// `PostUpdate`, after the orbit camera: the shot's pose on every camera, blended in from what it
/// showed and back out to where the orbit camera is.
#[cfg(feature = "render")]
fn show_shot(world: &mut World) {
    use bevy::camera::Projection;
    if world.resource::<Shots>().playing.is_none() {
        return;
    }
    let t_now = now(world);
    let base_fov = world.get_resource::<super::CameraSettings>().map_or(70.0, |s| s.fov_deg);
    let orbit = cameras_now(world);
    // Where the roles' bodies are now; one that went holds where it was last seen.
    let bodies: Vec<(String, Entity)> = {
        let playing = world.resource::<Shots>().playing.as_ref().unwrap();
        playing.roles.iter().map(|(r, e)| (r.clone(), *e)).collect()
    };
    let places: Vec<(String, Transform)> = bodies
        .into_iter()
        .filter_map(|(role, e)| world.get::<Transform>(e).map(|tf| (role, *tf)))
        .collect();
    let mut finished = false;
    let mut poses = BTreeMap::new();
    {
        let mut shots = world.resource_mut::<Shots>();
        let playing = shots.playing.as_mut().unwrap();
        playing.seen.extend(places);
        let t = (t_now - playing.started) as f32;
        let duration = playing.def.duration();
        if playing.ending.is_none() && t >= duration {
            // The way back starts from what the shot shows at its end: a blend in longer than
            // the shot is cut short where it got to, not jumped to the last key.
            let end_frame = sample(&playing.def, duration, &playing.seen, base_fov).pose();
            let from = orbit.keys().map(|e| (*e, blended_in(playing, *e, duration, end_frame))).collect();
            playing.ending = Some(Ending { at: playing.started + f64::from(duration), skipped: false, from });
        }
        match &playing.ending {
            Some(ending) => {
                let blend_out = playing.def.blend_out_s;
                let s = if blend_out > 0.0 { ((t_now - ending.at) as f32 / blend_out).clamp(0.0, 1.0) } else { 1.0 };
                if s >= 1.0 {
                    finished = true;
                } else {
                    for (e, back) in &orbit {
                        if let Some(from) = ending.from.get(e) {
                            poses.insert(*e, blend(*from, *back, s));
                        }
                    }
                }
            }
            None => {
                let shot = sample(&playing.def, t, &playing.seen, base_fov).pose();
                for e in orbit.keys() {
                    poses.insert(*e, blended_in(playing, *e, t, shot));
                }
            }
        }
        if !finished {
            playing.shown.extend(poses.iter().map(|(e, p)| (*e, *p)));
        }
    }
    if finished {
        end(world, false);
        return;
    }
    for (e, pose) in poses {
        let Ok(mut camera) = world.get_entity_mut(e) else { continue };
        if let Some(mut tf) = camera.get_mut::<Transform>() {
            tf.translation = pose.at;
            tf.rotation = pose.rotation;
        }
        if let Some(mut projection) = camera.get_mut::<Projection>()
            && let Projection::Perspective(p) = projection.as_mut()
        {
            p.fov = pose.fov;
        }
    }
}

/// `SHOT warden_intro 2.4/6.0s skippable, roles boss=e5`, or its blend back.
fn observe_section(world: &World) -> Vec<String> {
    let Some(playing) = world.get_resource::<Shots>().and_then(|s| s.playing.as_ref()) else {
        return Vec::new();
    };
    let t = (now(world) - playing.started) as f32;
    let roles: Vec<String> = playing.roles.iter().map(|(r, e)| format!("{r}={}", ai_core::label(world, *e))).collect();
    let roles = if roles.is_empty() { String::new() } else { format!(", roles {}", roles.join(" ")) };
    let line = match &playing.ending {
        Some(ending) => {
            let left = (playing.def.blend_out_s - (now(world) - ending.at) as f32).max(0.0);
            let how = if ending.skipped { "stopped" } else { "done" };
            format!("SHOT {} {how}, back to the orbit camera in {left:.1}s", playing.id)
        }
        None => {
            let skip = if playing.def.skippable { "skippable" } else { "not skippable" };
            format!("SHOT {} {:.1}/{:.1}s {skip}{roles}", playing.id, t.min(playing.def.duration()), playing.def.duration())
        }
    };
    vec![line]
}

/// Play a scene camera shot (`*.shot.ron`) on this machine's camera, each of its roles on a body:
/// `id=warden_intro role.boss=e5 role.hero=e1`. Only where the game is drawn.
#[derive(Deserialize, JsonSchema)]
pub struct PlayShotArgs {
    /// The shot's id.
    pub id: String,
    /// `role.NAME`: the body (`eN`) the shot's role NAME follows.
    #[serde(flatten)]
    pub roles: BTreeMap<String, serde_json::Value>,
}

fn play_shot(In(call): In<Call<PlayShotArgs>>, world: &mut World) -> ToolResult {
    let PlayShotArgs { id, roles: given } = call.args;
    let mut roles = BTreeMap::new();
    for (key, value) in given {
        let Some(role) = key.strip_prefix("role.") else {
            return Err(format!("unknown argument `{key}`: give `id` and a body per role, `role.NAME=eN`"));
        };
        let body: Id = serde_json::from_value(value.clone()).map_err(|_| format!("role.{role}: `{value}` is not an entity id (`e5`)"))?;
        roles.insert(role.to_owned(), body.entity(world)?);
    }
    play(world, &id, &roles)?;
    let def = world.resource::<ContentDb>().get::<ShotDef>(&id).cloned().unwrap_or_default();
    Ok(format!(
        "shot {id} playing: {:.1}s, blend in {:.1}s, out {:.1}s{}",
        def.duration(),
        def.blend_in_s,
        def.blend_out_s,
        if def.skippable { "" } else { ", not skippable" }
    ))
}

/// Stop the shot playing (the game's skip); `force=true` stops one that is not skippable.
#[derive(Deserialize, JsonSchema, Default)]
pub struct StopShotArgs {
    /// Default false.
    pub force: Option<bool>,
}

fn stop_shot(In(call): In<Call<StopShotArgs>>, world: &mut World) -> ToolResult {
    if call.args.force.unwrap_or(false) {
        let id = world.resource::<Shots>().playing().map(str::to_owned);
        return match (id, cancel(world)) {
            (Some(id), true) => Ok(format!("shot {id} cancelled")),
            _ => Err("no shot is playing".to_owned()),
        };
    }
    stop(world)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(at: f32, eye: Vec3, cut: bool) -> ShotKey {
        ShotKey { at, eye: ShotPoint::At(eye), look: ShotPoint::At(Vec3::ZERO), fov_deg: None, ease: Ease::Linear, cut }
    }

    #[test]
    fn a_shot_moves_between_its_keys_eases_holds_and_cuts() {
        let def = ShotDef {
            keys: vec![
                key(1.0, Vec3::new(0.0, 0.0, 10.0), false),
                key(3.0, Vec3::new(10.0, 0.0, 10.0), false),
                ShotKey { ease: Ease::InOut, fov_deg: Some(40.0), ..key(5.0, Vec3::new(10.0, 0.0, 0.0), false) },
                key(6.0, Vec3::new(-10.0, 0.0, 0.0), true),
            ],
            ..default()
        };
        let none = BTreeMap::new();
        let at = |t: f32| sample(&def, t, &none, 70.0);
        assert_eq!(at(0.0).eye, Vec3::new(0.0, 0.0, 10.0), "before the first key: held");
        assert_eq!(at(2.0).eye, Vec3::new(5.0, 0.0, 10.0), "half way, linear");
        let eased = at(3.5);
        assert!((eased.eye.z - (10.0 - 10.0 * 0.15625)).abs() < 1e-4, "a quarter of the way, smoothstep: {}", eased.eye);
        assert!((eased.fov_deg - (70.0 - 30.0 * 0.15625)).abs() < 1e-4, "the field of view eases from the player's: {}", eased.fov_deg);
        assert_eq!(at(5.9).eye, Vec3::new(10.0, 0.0, 0.0), "a cut: the key before is held until its time");
        assert_eq!(at(6.0).eye, Vec3::new(-10.0, 0.0, 0.0), "and jumped to at it");
        assert_eq!(at(9.0).fov_deg, 70.0);
        assert_eq!(def.duration(), 6.0);
    }

    #[test]
    fn a_role_moves_with_its_body_and_its_offset_turns_with_it() {
        let point = ShotPoint::On { role: "boss".into(), offset: Vec3::new(0.0, 2.0, -3.0), local: true };
        let mut roles = BTreeMap::new();
        roles.insert("boss".to_owned(), Transform::from_xyz(5.0, 0.0, 0.0));
        assert_eq!(point.resolve(&roles), Vec3::new(5.0, 2.0, -3.0));
        // Turned to face +x (its forward, -z, now points +x): 3 m ahead is +x.
        roles.insert("boss".to_owned(), Transform::from_xyz(5.0, 0.0, 0.0).looking_to(Vec3::X, Vec3::Y));
        assert!((point.resolve(&roles) - Vec3::new(8.0, 2.0, 0.0)).length() < 1e-5, "{}", point.resolve(&roles));
        let world = ShotPoint::On { role: "boss".into(), offset: Vec3::new(0.0, 2.0, -3.0), local: false };
        assert_eq!(world.resolve(&roles), Vec3::new(5.0, 2.0, -3.0), "in the world's axes");
        let def = ShotDef { keys: vec![ShotKey { look: point, ..key(0.0, Vec3::ZERO, false) }], ..default() };
        assert_eq!(def.roles().into_iter().collect::<Vec<_>>(), ["boss"]);
    }

    #[test]
    fn the_check_names_what_cannot_play() {
        let def = ShotDef {
            keys: vec![
                key(2.0, Vec3::Z, false),
                key(1.0, Vec3::Z, false),
                ShotKey { fov_deg: Some(5.0), ..key(130.0, Vec3::ZERO, false) },
                ShotKey { eye: ShotPoint::On { role: " ".into(), offset: Vec3::NAN, local: false }, ..key(131.0, Vec3::Z, false) },
            ],
            blend_out_s: -1.0,
            ..default()
        };
        let problems = def.problems();
        for needle in [
            "keys[1].at 1: not after the key before it (2)",
            "keys[2].at 130: a shot lasts at most 120 s",
            "keys[2]: `eye` and `look` are the same place",
            "keys[2].fov_deg 5: 10 to 120 degrees",
            "keys[3].eye: an empty role",
            "keys[3].eye.offset: finite numbers",
            "blend_out_s -1: 0 to 10 s",
        ] {
            assert!(problems.iter().any(|p| p.contains(needle)), "no `{needle}` in {problems:#?}");
        }
        assert_eq!(problems.len(), 8, "{problems:#?}");
        assert_eq!(ShotDef::default().problems(), ["keys: none; a shot needs at least one"]);
    }
}
