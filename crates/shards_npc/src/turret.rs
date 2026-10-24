//! Turrets (F11): a body that does not walk, turns its look toward its target at a content rate within
//! yaw and pitch limits, and fires through the shot pipeline (F3) when its look is on the target.
//!
//! A turret is an NPC ([`Npc`]: its perception finds the target, its behaviour can be one idle state)
//! with a [`Turret`] naming its `turret` definition ([`TurretDef`]) and whether it is on. Its body is a
//! character whose `locomotion` cannot move (no walking speed); with `face: Look` the whole body turns
//! with its look. Its **rest** is the way the body faced the first tick it ran (kept in
//! [`Turret::rest`]; a level may set it), and the yaw limits are measured from it. Each tick, after the
//! NPCs' intents are written ([`turrets`], `NpcSystems::Drive`), every turret in `AiId` order:
//!
//! - **aims** at its focus while it sees it (the target's aim point), at where it last knew it while it
//!   remembers it, else at its rest; the aim is held inside its limits (`yaw_deg` either way of the
//!   rest, `pitch_deg`), and its look (`Intent::look`) turns toward it by at most `turn_rate_deg` a
//!   second on each axis;
//! - **fires** (presses `input`) when it is on, sees its target within `range`, its look is within
//!   `on_target_deg` of the aim point and the aim needed no holding, the line of fire along its look
//!   meets the target first ([`line_of_fire`]: no friend, no wall), and it runs no action and has no press
//!   of that input waiting. Its shots fly along its look (a body that looks aims itself: no aim error).
//!
//! Off ([`set_turret_active`], the `turret_active` tool, or a level's `Turret` component with
//! `active: false`), it turns back to its rest and holds fire. A dead or downed turret, or one whose
//! brain is not active (dormant), does nothing.

use ai_core::{label, AiId, AiIndex};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_action::{line_of_fire, ActionState, Dead, Downed, InputBuffer};
use shards_content::{ContentDb, ContentDef, Refs};
use shards_core::{flat, Gaze, Intent};
use shards_motion::{aim_point_of, eye_of};

use crate::brain::{Npc, NpcBrain};
use crate::perception::Perceived;
use crate::sequence::SequenceHold;

/// How a turret turns and fires (content kind `turret`, F11). Missing fields keep their defaults.
///
/// ```ron
/// // content/npc/sentry.turret.ron
/// { "sentry": (turn_rate_deg: 90, yaw_deg: 60, pitch_deg: (-20, 30), input: "light", range: 30) }
/// ```
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct TurretDef {
    /// How fast the look turns, degrees per second, on each axis (yaw and pitch alike); above 0.
    pub turn_rate_deg: f32,
    /// How far the look turns either way of the body's facing (its rest), degrees, 0 to 180 (180: all
    /// round, turning the short way).
    pub yaw_deg: f32,
    /// How far the look pitches, `(lowest, highest)` degrees within -90..90 (negative looks down).
    pub pitch_deg: (f32, f32),
    /// The moveset input it fires with.
    pub input: String,
    /// It fires only while its look is within this many degrees of the target's aim point.
    pub on_target_deg: f32,
    /// It fires at a target within this many meters; farther, it tracks and holds fire.
    pub range: f32,
}

impl Default for TurretDef {
    fn default() -> Self {
        Self { turn_rate_deg: 90.0, yaw_deg: 180.0, pitch_deg: (-30.0, 45.0), input: "light".to_owned(), on_target_deg: 2.0, range: 40.0 }
    }
}

impl ContentDef for TurretDef {
    const KIND: &'static str = "turret";

    fn validate(&self, _: &str, refs: &mut Refs) {
        if !(self.turn_rate_deg > 0.0 && self.turn_rate_deg.is_finite()) {
            refs.error(format!("turn_rate_deg must be a number of degrees per second above 0 (is {})", self.turn_rate_deg));
        }
        if !(0.0..=180.0).contains(&self.yaw_deg) {
            refs.error(format!("yaw_deg must be within 0..180 degrees either way of the rest (is {}); 180 turns all round", self.yaw_deg));
        }
        let (lo, hi) = self.pitch_deg;
        if !(lo >= -90.0 && hi <= 90.0 && lo <= hi) {
            refs.error(format!("pitch_deg ({lo}, {hi}) must be -90 <= lowest <= highest <= 90 degrees"));
        }
        if self.input.is_empty() {
            refs.error("input is empty: name the moveset input it fires with, e.g. \"light\"");
        }
        if !(self.on_target_deg >= 0.0 && self.on_target_deg <= 90.0) {
            refs.error(format!("on_target_deg must be within 0..90 degrees (is {})", self.on_target_deg));
        }
        if !(self.range > 0.0 && self.range.is_finite()) {
            refs.error(format!("range must be a number of meters above 0 (is {})", self.range));
        }
    }
}

/// A turret (F11): its `turret` definition, whether it is on, and its rest. Gameplay state: kept in
/// replays and snapshots. A level or prefab sets it (`"Turret": (def: "sentry", active: true)`); the
/// game switches it with [`set_turret_active`].
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct Turret {
    /// Its `turret` definition.
    pub def: String,
    /// On: it tracks its target and fires. Off: it turns back to its rest and holds fire.
    pub active: bool,
    /// The yaw (radians, as `Gaze::yaw`) its limits are measured from: `None` until the first tick it
    /// runs, which keeps the way its body faces then.
    pub rest: Option<f32>,
}

impl Turret {
    /// A turret of definition `def`, on or off; its rest is the way its body first faces.
    pub fn new(def: impl Into<String>, active: bool) -> Self {
        Self { def: def.into(), active, rest: None }
    }
}

/// A turret was switched on or off ([`set_turret_active`]).
#[derive(Event, Clone, Debug, PartialEq)]
pub struct TurretSwitched {
    pub entity: Entity,
    pub active: bool,
}

/// Switches turret `entity` on or off; journaled (`e9 turret on`). `Err` for an entity with no
/// [`Turret`]. Gameplay: call it where the simulation runs (the host; [`TurretSwitched`] is no net
/// event, a co-op client gets the component in the host's state).
pub fn set_turret_active(world: &mut World, entity: Entity, on: bool) -> Result<String, String> {
    let mut turret = world.get_mut::<Turret>(entity).ok_or("not a turret (it has no Turret component)")?;
    let was = turret.active;
    if was != on {
        turret.active = on;
        world.trigger(TurretSwitched { entity, active: on });
    }
    let word = if on { "on" } else { "off" };
    Ok(if was == on { format!("ok: {} turret already {word}", label(world, entity)) } else { format!("ok: {} turret {word}", label(world, entity)) })
}

/// The yaw (radians) of a direction on the ground plane, as [`Gaze`] measures it (0 along -Z,
/// positive to the left).
fn yaw_of(dir: Vec3) -> f32 {
    Gaze::toward(Vec3::ZERO, flat(dir)).map_or(0.0, |g| g.yaw)
}

/// `angle` wrapped into -π..π.
fn wrap(angle: f32) -> f32 {
    (angle + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI
}

/// `from` moved toward `to` by at most `step`.
fn approach(from: f32, to: f32, step: f32) -> f32 {
    from + (to - from).clamp(-step, step)
}

/// Where a turret's look goes this tick: the aim (a look, yaw relative to the rest) held inside the
/// limits, and the look turned toward it from `current` by one tick of the turn rate. Also whether the
/// aim needed holding (the target is outside the limits).
pub(crate) fn turn(def: &TurretDef, rest: f32, current: Gaze, aim: Gaze, dt: f32) -> (Gaze, bool) {
    let limit = def.yaw_deg.to_radians();
    let (lo, hi) = (def.pitch_deg.0.to_radians(), def.pitch_deg.1.to_radians());
    let want = wrap(aim.yaw - rest);
    let held_yaw = want.clamp(-limit, limit);
    let held_pitch = aim.pitch.clamp(lo, hi);
    let held = (held_yaw - want).abs() > 1e-4 || (held_pitch - aim.pitch).abs() > 1e-4;
    let step = def.turn_rate_deg.to_radians() * dt;
    let now = wrap(current.yaw - rest);
    let yaw = if def.yaw_deg >= 180.0 {
        // All round: the short way.
        now + wrap(held_yaw - now).clamp(-step, step)
    } else {
        approach(now.clamp(-limit, limit), held_yaw, step)
    };
    let pitch = approach(current.pitch.clamp(lo, hi), held_pitch, step);
    (Gaze::new(wrap(rest + yaw), pitch), held)
}

/// What a turret is doing, for tools: `turret tracking e5 (yaw 12°/60°)`, `turret tracking e5 (yaw
/// -60°/60°, at its limit)`, `turret idle`, `turret off`.
pub fn turret_line(world: &World, entity: Entity) -> Option<String> {
    let turret = world.get::<Turret>(entity)?;
    if !turret.active {
        return Some("turret off".to_owned());
    }
    let def = world.get_resource::<ContentDb>().and_then(|db| db.peek::<TurretDef>(&turret.def)).cloned().unwrap_or_default();
    let tf = world.get::<Transform>(entity)?;
    let rest = turret.rest.unwrap_or_else(|| yaw_of(*tf.forward()));
    let look = world.get::<Intent>(entity).and_then(|i| i.look).unwrap_or(Gaze::new(rest, 0.0));
    let yaw = wrap(look.yaw - rest).to_degrees();
    let tracking = world.get::<Perceived>(entity).and_then(|p| p.focus);
    // Held at a yaw limit (the target is outside them): said, so an agent knows why it holds fire.
    let held = def.yaw_deg < 180.0 && yaw.abs() >= def.yaw_deg - 0.5;
    Some(match tracking {
        Some(id) if held => format!("turret tracking {id} (yaw {:.0}°/{:.0}°, at its limit)", yaw, def.yaw_deg),
        Some(id) => format!("turret tracking {id} (yaw {:.0}°/{:.0}°)", yaw, def.yaw_deg),
        None => "turret idle".to_owned(),
    })
}

/// `NpcSystems::Drive`, after the intents: every turret, in `AiId` order, turns its look and fires
/// (see the module docs). The definitions are read for turrets only.
pub(crate) fn turrets(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    let mut list: Vec<(AiId, Entity)> = match world.try_query::<(Entity, &AiId, &Turret)>() {
        Some(mut q) => q.iter(world).map(|(e, id, _)| (*id, e)).collect(),
        None => return,
    };
    if list.is_empty() {
        return;
    }
    list.sort();
    let mut plans: Vec<(Entity, Option<f32>, Gaze, Option<String>)> = Vec::new();
    {
        let Some(db) = world.get_resource::<ContentDb>() else { return };
        let index = world.resource::<AiIndex>();
        for (_, entity) in list {
            let Some(turret) = world.get::<Turret>(entity) else { continue };
            let standing = world.get::<Dead>(entity).is_none() && world.get::<Downed>(entity).is_none();
            let awake = world.get::<NpcBrain>(entity).is_none_or(|b| b.active) && world.get::<Npc>(entity).is_some() && world.get::<SequenceHold>(entity).is_none();
            if !standing || !awake {
                continue;
            }
            let Some(def) = db.get::<TurretDef>(&turret.def) else { continue };
            let (Some(tf), Some(eye)) = (world.get::<Transform>(entity), eye_of(world, entity)) else { continue };
            let first = turret.rest.is_none().then(|| yaw_of(*tf.forward()));
            let rest = turret.rest.or(first).unwrap_or(0.0);
            let current = world.get::<Intent>(entity).and_then(|i| i.look).filter(|l| l.is_finite()).unwrap_or(Gaze::new(rest, 0.0));
            // What it aims at: its focus as it perceives it (seen: the body's aim point).
            let perceived = world.get::<Perceived>(entity);
            let focus = perceived.and_then(|p| p.focus.and_then(|id| p.get(id)));
            let body = focus.and_then(|k| index.get(k.id));
            let seen = turret.active && focus.is_some_and(|k| k.seen);
            let point = match (turret.active, focus) {
                (true, Some(k)) if k.seen => body.and_then(|b| aim_point_of(world, b)).or(Some(k.pos + Vec3::Y)),
                (true, Some(k)) => Some(k.pos + Vec3::Y),
                _ => None,
            };
            let aim = point.and_then(|p| Gaze::toward(eye, p)).unwrap_or(Gaze::new(rest, 0.0));
            let (look, held) = turn(def, rest, current, aim, dt);
            let press = (|| {
                let (point, body) = (point?, body?);
                if !seen || held || (point - eye).length() > def.range {
                    return None;
                }
                let off = look.dir().angle_between((point - eye).try_normalize()?).to_degrees();
                if off > def.on_target_deg + 1e-3 {
                    return None;
                }
                if !line_of_fire(world, entity, eye, look.dir(), def.range, body).clear() {
                    return None;
                }
                let busy = world.get::<ActionState>(entity).is_some_and(|s| s.current.is_some());
                let queued = world.get::<InputBuffer>(entity).is_some_and(|b| b.queued.iter().any(|q| q.input == def.input));
                (!busy && !queued).then(|| def.input.clone())
            })();
            plans.push((entity, first, look, press));
        }
    }
    for (entity, first, look, press) in plans {
        if let Some(rest) = first
            && let Some(mut turret) = world.get_mut::<Turret>(entity)
        {
            turret.rest = Some(rest);
        }
        let Some(mut intent) = world.get_mut::<Intent>(entity) else { continue };
        if intent.look != Some(look) {
            intent.look = Some(look);
        }
        if let Some(input) = press {
            intent.press(input);
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;

    fn def(yaw: f32) -> TurretDef {
        TurretDef { turn_rate_deg: 90.0, yaw_deg: yaw, pitch_deg: (-20.0, 30.0), ..default() }
    }

    #[test]
    fn the_look_turns_at_its_rate_and_stays_inside_its_limits() {
        let d = def(60.0);
        let dt = 1.0 / 60.0;
        // A target 40 degrees to the left: 1.5 degrees a tick toward it, never past it.
        let mut look = Gaze::new(0.0, 0.0);
        let aim = Gaze::from_degrees(40.0, 0.0);
        let (next, held) = turn(&d, 0.0, look, aim, dt);
        assert!(!held);
        assert!((next.yaw.to_degrees() - 1.5).abs() < 1e-3, "{}", next.yaw.to_degrees());
        for _ in 0..60 {
            look = turn(&d, 0.0, look, aim, dt).0;
        }
        assert!((look.yaw.to_degrees() - 40.0).abs() < 1e-3, "on target after 40/90 s: {}", look.yaw.to_degrees());
        // A target 120 degrees to the right: held at the 60 degree limit, and said so.
        let behind = Gaze::from_degrees(-120.0, 50.0);
        let mut look = Gaze::new(0.0, 0.0);
        for _ in 0..240 {
            look = turn(&d, 0.0, look, behind, dt).0;
        }
        assert!((look.yaw.to_degrees() + 60.0).abs() < 1e-3 && (look.pitch.to_degrees() - 30.0).abs() < 1e-3, "{look:?}");
        assert!(turn(&d, 0.0, look, behind, dt).1, "the aim is outside the limits");
        // Limits are measured from the rest: a turret resting at yaw 90 reaches 150 and no farther.
        let mut look = Gaze::from_degrees(90.0, 0.0);
        for _ in 0..240 {
            look = turn(&d, 90f32.to_radians(), look, Gaze::from_degrees(170.0, 0.0), dt).0;
        }
        assert!((look.yaw.to_degrees() - 150.0).abs() < 1e-3, "{}", look.yaw.to_degrees());
    }

    #[test]
    fn an_all_round_turret_turns_the_short_way() {
        let d = def(180.0);
        let look = Gaze::from_degrees(170.0, 0.0);
        let (next, held) = turn(&d, 0.0, look, Gaze::from_degrees(-170.0, 0.0), 1.0 / 60.0);
        assert!(!held);
        assert!((next.yaw.to_degrees() - 171.5).abs() < 1e-3, "across the back, not round the front: {}", next.yaw.to_degrees());
    }

    #[test]
    fn bad_numbers_are_named_by_field() {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<TurretDef>();
        let text = r#"{ "t": (turn_rate_deg: 0, yaw_deg: 200, pitch_deg: (10, -10), input: "", on_target_deg: -1, range: 0), "ok": () }"#;
        let sources = [shards_content::db::Source { path: "content/t.turret.ron".into(), text: text.into() }];
        let db = ContentDb::build(&kinds, &sources);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        let errors = errors.join("\n");
        for needle in ["turn_rate_deg", "yaw_deg", "pitch_deg", "input is empty", "on_target_deg", "range must be"] {
            assert!(errors.contains(needle), "{needle} in {errors}");
        }
        assert!(!errors.contains("`ok`"), "the defaults are clean: {errors}");
    }
}
