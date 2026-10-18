//! # shards_hitstop — hit stop (C2)
//!
//! When a blow lands the picture stands still for a beat (60-100 ms), which is what makes a hit
//! feel heavy. It is a **hold**, never slow motion, and never the simulation's: no tick is lost
//! and none is added, so replays, snapshots and gameplay do not see it.
//!
//! - **What holds is content.** `*.hitstop.ron` ([`HitstopDef`], one per game) is an ordered list of
//!   rules over the combat events: an event (`Damaged`, `Blocked`, `Parried`, `PerfectEvaded`,
//!   `GuardBroken`, `Staggered`), the hit's weight (`weight_at_least`), its reaction tier
//!   (`reaction_at_least`), the cause of a stagger, the action. The first rule that matches an event
//!   gives its hold in seconds; an event no rule matches gives none. A game writes its own ladder
//!   (finisher, then poise break, then counter, then a heavy reaction, then the rest); the engine has
//!   no names from any game.
//! - **Every machine that simulates or shows combat events** turns them into a [`HitStopped`] event
//!   (the rule, the hold, the bodies) and writes a `hitstop:` line to the journal as
//!   [`JournalKind::Local`](ai_core::JournalKind::Local). It changes no gameplay, so it needs no
//!   `simulates` guard; a co-op client gets the same combat events from its host and decides for
//!   itself. Whether anything is held is then decided by the machine:
//!   - **Alone, with a picture** (`NetRole::Single`, a window or `--offscreen`, real time): the
//!     [`ClockHold`](ai_core::ClockHold) keeps the fixed clock still for the real time of the hold
//!     (virtual speed 0, not a pause: no pause menu, no muted sound, the gate does not refuse). The
//!     camera and the input run on real time; everything on the simulation's clock, the pose
//!     included, stands still. Zero ticks run inside it.
//!   - **Co-op** (host and clients): the clock is never held. Only the pictures of the bodies in the
//!     blow stand still ([`PresentationHolds`](ai_core::PresentationHolds), by `AiId`, read by the drawn-pose interpolation and the
//!     animation bridge).
//!   - **Alone, and headless, `--paused`, a `step`, a replay playing back (`--verify`), a driven
//!     clock, tests**: nothing is held. The event and the journal line still appear, and `hitstop`
//!     says why nothing was held. A co-op host or client, headless or in a stand, still holds the
//!     pictures.
//! - **Overlapping holds merge by the later end.** A hold asked for while one runs lasts as long as
//!   the longer of the two has left; a table's `cooldown` keeps a flurry from turning into a stutter.
//! - **Accessibility** (`settings`, group `accessibility`): `hit_stop` is the scale of every hold (0
//!   turns hit stop off), `hit_stop_local_only` holds only for blows the local player takes part in.
//!   The table's `max_seconds` (at most [`MAX_HOLD`]) clamps a hold whatever a rule says.
//! - **Agents**: `hitstop` shows what is held, the table and the last holds; `hitstop_play` makes
//!   one as a blow would (and says what the machine did with it); `ai/status` has
//!   `hitstop: {mode, holds, ...}`.
//! - **Not the simulation's**: [`HitStops`], [`ClockHold`](ai_core::ClockHold) and
//!   [`PresentationHolds`](ai_core::PresentationHolds) are never saved in a snapshot or recorded in
//!   a replay. The `hitstop` kind is not simulation content: editing it never changes a replay's
//!   content hash.

pub mod def;
mod tools;

use std::collections::VecDeque;
use std::time::Duration;

use ai_core::journal::{self, JournalKind};
use ai_core::{AiAppExt, AiId, AiReplay, ClockHold, PresentationHolds};
use bevy::prelude::*;
use serde_json::{json, Value};
use shards_action::{Blocked, Damaged, GuardBroken, Parried, PerfectEvaded, Staggered};
use shards_content::{ContentAppExt, ContentDb};
use ai_core::SimTick;
use shards_core::{EngineAppExt, LocalPlayer, NetRole};
use shards_settings::groups::AccessibilitySettings;

pub use def::{Cause, EventKind, HitInfo, HitstopDef, Rule, Weight, When, MAX_HOLD};
pub use tools::{HitstopArgs, HitstopPlayArgs};

/// Version of the engine crates (they share one).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How many of the last holds [`HitStops`] remembers.
pub const LOG_KEPT: usize = 32;

/// A blow turned into a hold by the table. Triggered on every machine that sees the combat event;
/// changes nothing in the world by itself.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct HitStopped {
    pub event: EventKind,
    /// Index of the rule that gave the hold, and its name.
    pub rule: usize,
    pub name: String,
    /// The hold the table gives (s), before the player's scale; within the table's ceiling.
    pub seconds: f32,
    /// The bodies in the blow (the attacker, the one hit), and their ids, in id order.
    pub bodies: Vec<Entity>,
    pub ids: Vec<AiId>,
}

/// What this machine did with a [`HitStopped`].
#[derive(Clone, Debug, PartialEq)]
pub enum Outcome {
    /// The real-time clock was held (alone, with a picture).
    Clock,
    /// The pictures of the bodies were held (co-op).
    Picture,
    /// Nothing was held, and why.
    Skipped(String),
}

impl Outcome {
    pub fn describe(&self) -> String {
        match self {
            Outcome::Clock => "clock held".to_owned(),
            Outcome::Picture => "pictures held".to_owned(),
            Outcome::Skipped(why) => format!("not held: {why}"),
        }
    }
}

/// One hold the module decided on.
#[derive(Clone, Debug, PartialEq)]
pub struct Record {
    pub tick: u64,
    pub event: EventKind,
    pub name: String,
    /// What the table gave, and what was held after the player's scale (s).
    pub seconds: f32,
    pub held: f32,
    pub ids: Vec<AiId>,
    pub outcome: Outcome,
}

/// What the module has decided so far; the picture's, never the simulation's.
#[derive(Resource, Default, Debug)]
pub struct HitStops {
    /// Newest last, at most [`LOG_KEPT`].
    pub log: VecDeque<Record>,
    /// Blows the table turned into holds, and what became of them.
    pub events: u64,
    pub clock: u64,
    pub picture: u64,
    pub skipped: u64,
    /// `Time<Real>` the newest hold ends at, for the table's cooldown.
    pub ends: Duration,
}

/// The hit stop module: the `hitstop` content kind, the combat events turned into holds, the
/// tools and the status. Needs the engine core, content, settings and actions first;
/// `shards::EnginePlugins` adds it after them.
pub struct HitstopPlugin;

impl Plugin for HitstopPlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<ai_core::AiCorePlugin>(),
            "HitstopPlugin needs AiCorePlugin added first (shards::EnginePlugins adds both)"
        );
        app.add_content::<HitstopDef>()
            .init_resource::<HitStops>()
            .add_engine_module("hitstop", VERSION)
            .add_engine_invariant("hitstop.no_ticks_inside", no_ticks_inside)
            .add_ai_status("hitstop", status)
            .add_dev_entry("combat", "hitstop", "Hit stop: the table, what is held, the last holds", ai_core::DevCommand::Run("hitstop".into()))
            .add_observer(on_hit_stopped)
            .add_observer(|e: On<Damaged>, c: Commands| {
                let e = e.event();
                let info = HitInfo {
                    event: EventKind::Damaged,
                    weight: Some(Weight::of(e.weight)),
                    reaction: e.reaction,
                    cause: None,
                    action: Some(e.action.clone()),
                    ignored: e.invulnerable || e.ignore_reason.is_some(),
                    local: false,
                };
                blow(c, info, vec![e.attacker, e.target]);
            })
            .add_observer(|e: On<Blocked>, c: Commands| {
                let e = e.event();
                let info = HitInfo { event: EventKind::Blocked, weight: Some(Weight::of(e.weight)), reaction: None, cause: None, action: Some(e.action.clone()), ignored: false, local: false };
                blow(c, info, vec![e.attacker, e.defender]);
            })
            .add_observer(|e: On<Parried>, c: Commands| {
                let e = e.event();
                let info = HitInfo { event: EventKind::Parried, weight: None, reaction: None, cause: None, action: Some(e.action.clone()), ignored: false, local: false };
                blow(c, info, vec![e.attacker, e.defender]);
            })
            .add_observer(|e: On<PerfectEvaded>, c: Commands| {
                let e = e.event();
                let info = HitInfo { event: EventKind::PerfectEvaded, weight: None, reaction: None, cause: None, action: Some(e.action.clone()), ignored: false, local: false };
                blow(c, info, vec![e.attacker, e.defender]);
            })
            .add_observer(|e: On<GuardBroken>, c: Commands| {
                let e = e.event();
                let info = HitInfo { event: EventKind::GuardBroken, weight: Some(Weight::of(e.weight)), reaction: None, cause: None, action: Some(e.action.clone()), ignored: false, local: false };
                blow(c, info, vec![e.attacker, e.defender]);
            })
            .add_observer(|e: On<Staggered>, c: Commands| {
                let e = e.event();
                let info = HitInfo { event: EventKind::Staggered, weight: None, reaction: None, cause: Some(Cause::of(e.cause)), action: None, ignored: false, local: false };
                blow(c, info, vec![e.entity]);
            });
        tools::register(app.world_mut());
    }
}

/// The engine rule `hitstop.no_ticks_inside`: a hold of the clock consumes zero ticks. No tick has run
/// since the clock was stopped, and none ran inside a hold that has ended. A violation names a system
/// that ran the fixed schedule on its own while the clock was held (a hold only stops the real-time
/// clock; `sim::step` ends it first). Hit stop is the picture's: it must never be the simulation's.
pub fn no_ticks_inside(world: &World) -> Option<String> {
    let hold = world.get_resource::<ClockHold>()?;
    let tick = world.get_resource::<SimTick>().map_or(0, |t| t.0);
    let inside = hold.ticks_inside + hold.ticks_since_held(tick).unwrap_or(0);
    (inside > 0).then(|| format!("{inside} simulation tick(s) ran while the clock was held for a hit stop; a hold consumes zero ticks"))
}

/// The game's hit stop table (the one `*.hitstop.ron`), if it has one.
pub fn table(world: &World) -> Option<&HitstopDef> {
    world.get_resource::<ContentDb>().and_then(|db| db.iter::<HitstopDef>().next().map(|(_, def)| def))
}

/// A combat event, to be looked up in the table once commands apply.
fn blow(mut commands: Commands, info: HitInfo, bodies: Vec<Entity>) {
    commands.queue(move |world: &mut World| {
        trigger(world, &info, &bodies);
    });
}

/// Looks `info` up in the table; if a rule gives a hold, triggers [`HitStopped`] and returns it.
pub fn trigger(world: &mut World, info: &HitInfo, bodies: &[Entity]) -> Option<HitStopped> {
    let mut info = info.clone();
    info.local = info.local || bodies.iter().any(|b| world.get::<LocalPlayer>(*b).is_some());
    let def = table(world)?;
    let (index, rule) = def.select(&info)?;
    let seconds = def.hold(rule);
    if seconds <= 0.0 {
        return None;
    }
    let name = rule.label(index);
    stop(world, info.event, index, name, seconds, bodies)
}

/// Triggers [`HitStopped`] for a hold of `seconds` named `name` on `bodies`.
pub fn stop(world: &mut World, event: EventKind, rule: usize, name: String, seconds: f32, bodies: &[Entity]) -> Option<HitStopped> {
    let mut pairs: Vec<(AiId, Entity)> = bodies.iter().filter_map(|b| world.get::<AiId>(*b).map(|id| (*id, *b))).collect();
    pairs.sort();
    pairs.dedup();
    let stopped = HitStopped {
        event,
        rule,
        name,
        seconds,
        bodies: pairs.iter().map(|(_, b)| *b).collect(),
        ids: pairs.iter().map(|(id, _)| *id).collect(),
    };
    world.trigger(stopped.clone());
    Some(stopped)
}

fn on_hit_stopped(event: On<HitStopped>, mut commands: Commands) {
    let stopped = event.event().clone();
    commands.queue(move |world: &mut World| apply(world, &stopped));
}

/// Whether this game draws a picture: a window, or `--offscreen`.
fn draws(world: &World) -> bool {
    world.contains_resource::<ai_core::display::Offscreen>()
        || world.try_query_filtered::<(), With<bevy::window::Window>>().is_some_and(|mut q| q.iter(world).next().is_some())
}

/// `Time<Real>` as this frame began: where a hold asked for now starts. The ticks of this frame
/// have counted the real time up to it, and the frame that follows is the first the clock is held
/// for, so a hold lasts from here to the first frame that starts after its end: its length, plus
/// less than a frame.
fn frame_start(world: &World) -> Duration {
    world.get_resource::<Time<Real>>().map_or(Duration::ZERO, Time::elapsed)
}

/// Journals the hold and does what this machine does with it.
fn apply(world: &mut World, stop: &HitStopped) {
    let on = if stop.ids.is_empty() { String::new() } else { format!(" on {}", stop.ids.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")) };
    journal::log(world, JournalKind::Local, format!("hitstop: {} {:.2} s ({}){on}", stop.name, stop.seconds, stop.event.name()));
    let (held, outcome) = decide(world, stop);
    let tick = ai_core::sim::tick(world);
    let mut stops = world.resource_mut::<HitStops>();
    stops.events += 1;
    match &outcome {
        Outcome::Clock => stops.clock += 1,
        Outcome::Picture => stops.picture += 1,
        Outcome::Skipped(_) => stops.skipped += 1,
    }
    if stops.log.len() == LOG_KEPT {
        stops.log.pop_front();
    }
    stops.log.push_back(Record { tick, event: stop.event, name: stop.name.clone(), seconds: stop.seconds, held, ids: stop.ids.clone(), outcome });
}

/// Why this machine holds nothing right now, if it does not; `None`: it can hold its clock.
fn cannot_hold_clock(world: &World) -> Option<&'static str> {
    if !draws(world) {
        return Some("nothing is drawn (headless)");
    }
    if ai_core::sim::is_driven(world) {
        return Some("a stand drives this clock");
    }
    if world.get_resource::<AiReplay>().is_some_and(AiReplay::is_playing) {
        return Some("a replay is playing");
    }
    if ai_core::sim::gate_message(world).is_some() {
        return Some("the game is loading");
    }
    if ai_core::sim::is_paused(world) {
        return Some("the clock is paused or stepped");
    }
    None
}

/// What this machine does with `stop`: the hold in seconds after the player's scale, and the outcome.
fn decide(world: &mut World, stop: &HitStopped) -> (f32, Outcome) {
    let settings = world.get_resource::<AccessibilitySettings>().cloned().unwrap_or_default();
    let scale = settings.hit_stop_scale();
    let skip = |why: &str| (0.0, Outcome::Skipped(why.to_owned()));
    if scale <= 0.0 {
        return skip("hit stop is off in the settings");
    }
    if settings.hit_stop_local_only && !stop.bodies.iter().any(|b| world.get::<LocalPlayer>(*b).is_some()) {
        return skip("the local player is not in this blow (hit_stop_local_only)");
    }
    let seconds = (stop.seconds * scale).clamp(0.0, MAX_HOLD);
    if seconds <= 0.0 {
        return skip("a hold of no time");
    }
    let now = frame_start(world);
    let cooldown = table(world).map_or(0.0, |t| t.cooldown.clamp(0.0, 5.0));
    let ends = world.resource::<HitStops>().ends;
    if now >= ends && now < ends + Duration::from_secs_f32(cooldown) {
        return skip("cooldown");
    }
    let until = now + Duration::from_secs_f32(seconds);
    let role = world.get_resource::<NetRole>().copied().unwrap_or_default();
    let outcome = if role == NetRole::Single {
        if let Some(why) = cannot_hold_clock(world) {
            return skip(why);
        }
        world.resource_mut::<ClockHold>().request(until);
        Outcome::Clock
    } else {
        // Co-op: no machine stops the others' clock; only the bodies' pictures stand still. Recorded
        // on every machine (a headless host or stand shows it in `hitstop`), read where it is drawn.
        if stop.ids.is_empty() {
            return skip("no body with an id to hold");
        }
        let mut holds = world.resource_mut::<PresentationHolds>();
        for id in &stop.ids {
            holds.hold(*id, now, until);
        }
        Outcome::Picture
    };
    let mut stops = world.resource_mut::<HitStops>();
    stops.ends = stops.ends.max(until);
    (seconds, outcome)
}

/// How this machine holds, in a few words.
pub fn mode(world: &World) -> &'static str {
    match world.get_resource::<NetRole>().copied().unwrap_or_default() {
        NetRole::Single => match cannot_hold_clock(world) {
            None => "solo: holds the real-time clock (no tick is lost)",
            Some(why) if why.starts_with("nothing is drawn") => "headless: never holds",
            Some(_) => "solo, but this clock is not real time now: not held",
        },
        NetRole::Host => "co-op host: holds the pictures of the bodies only, never the clock",
        NetRole::Client => "co-op client: holds the pictures of the bodies only, never the clock",
    }
}

/// `ai/status` → `hitstop: {mode, rules, events, clock_holds, picture_holds, skipped, clock_held_ms, ticks_inside}`.
fn status(world: &World) -> Value {
    let stops = world.get_resource::<HitStops>();
    let clock = world.get_resource::<ClockHold>();
    json!({
        "mode": mode(world),
        "rules": table(world).map_or(0, |t| t.rules.len()),
        "events": stops.map_or(0, |s| s.events),
        "clock_holds": stops.map_or(0, |s| s.clock),
        "picture_holds": stops.map_or(0, |s| s.picture),
        "skipped": stops.map_or(0, |s| s.skipped),
        "clock_held_ms": clock.map_or(0, |c| c.total.as_millis() as u64),
        "ticks_inside": clock.map_or(0, |c| c.ticks_inside),
        "clock_held_now": clock.is_some_and(ClockHold::is_held),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_rule_is_silent_until_a_tick_ran_inside_a_hold() {
        let mut world = World::new();
        assert_eq!(no_ticks_inside(&world), None, "no hold resource: nothing to judge");
        world.init_resource::<ClockHold>();
        world.init_resource::<SimTick>();
        assert_eq!(no_ticks_inside(&world), None);
        world.resource_mut::<ClockHold>().ticks_inside = 3;
        assert_eq!(
            no_ticks_inside(&world).as_deref(),
            Some("3 simulation tick(s) ran while the clock was held for a hit stop; a hold consumes zero ticks")
        );
    }
}
