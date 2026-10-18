//! The hit stop module's tools: `hitstop` (what is held, the table, the last holds) and
//! `hitstop_play` (a hold as a blow would make, through the same table). They show this machine's
//! picture and clock, so both run where they are called, a co-op client too.

use std::time::Duration;

use ai_core::tools::{register_tool, ToolKind};
use ai_core::{Call, ClockHold, Id, PresentationHolds, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_content::db::closest;
use shards_settings::groups::AccessibilitySettings;

use crate::def::{Cause, EventKind, HitInfo, Weight};
use crate::{mode, table, trigger, HitStops};

pub(crate) fn register(world: &mut World) {
    register_tool(world, "hitstop", Scope::Dev, ToolKind::Query, hitstop);
    register_tool(world, "hitstop_play", Scope::Dev, ToolKind::Control, hitstop_play);
}

/// The hit stop: how this machine holds, the player's settings for it, the game's table (the rules
/// in the order they are tried), what is held now, the totals (the clock holds with the real time
/// they took and the ticks that ran inside them, which is 0 when it works) and the last holds.
#[derive(Deserialize, JsonSchema, Default)]
pub struct HitstopArgs {
    /// How many of the last holds to list. Default 8.
    pub last: Option<usize>,
}

fn secs(d: Duration) -> String {
    format!("{:.3}", d.as_secs_f64())
}

fn hitstop(In(call): In<Call<HitstopArgs>>, world: &World) -> ToolResult {
    let settings = world.get_resource::<AccessibilitySettings>().cloned().unwrap_or_default();
    let mut out = format!("hit stop: {}\n", mode(world));
    out.push_str(&format!(
        "settings: scale {} (accessibility.hit_stop; 0 is off) · only the local player's blows: {}\n",
        settings.hit_stop_scale(),
        if settings.hit_stop_local_only { "yes" } else { "no" }
    ));
    match table(world) {
        Some(def) => {
            out.push_str(&format!("table: {} rules · ceiling {:.2} s · cooldown {:.2} s\n", def.rules.len(), def.ceiling(), def.cooldown));
            for (i, rule) in def.rules.iter().enumerate() {
                out.push_str(&format!("  {:>2} {:<14} {} → {:.2} s\n", i + 1, rule.label(i), rule.when.describe(), def.hold(rule)));
            }
        }
        None => out.push_str("no table (`*.hitstop.ron`): no blow holds anything\n"),
    }
    let now = world.get_resource::<Time<Real>>().map_or(Duration::ZERO, Time::elapsed);
    let clock = world.get_resource::<ClockHold>();
    match clock.filter(|c| c.is_held()) {
        Some(c) => {
            let left = c.until().map_or(Duration::ZERO, |u| u.saturating_sub(now));
            out.push_str(&format!("now: the clock is held, {} s left; speed reads {}\n", secs(left), ai_core::sim::speed(world)));
        }
        None => out.push_str("now: the clock runs\n"),
    }
    if let Some(holds) = world.get_resource::<PresentationHolds>() {
        let held: Vec<String> = holds.held(now).map(|(id, until)| format!("{id} ({} s left)", secs(until.saturating_sub(now)))).collect();
        if !held.is_empty() {
            out.push_str(&format!("      pictures held: {}\n", held.join(", ")));
        }
    }
    let stops = world.resource::<HitStops>();
    out.push_str(&format!(
        "total: {} blows · {} clock holds · {} picture holds · {} not held\n",
        stops.events, stops.clock, stops.picture, stops.skipped
    ));
    if let Some(clock) = clock {
        match clock.last {
            Some(span) => out.push_str(&format!(
                "clock: {} s held in all over {} holds · ticks that ran inside a hold: {} · the last: {} s real, {} ticks inside\n",
                secs(clock.total),
                clock.count,
                clock.ticks_inside,
                secs(span.real()),
                span.ticks_inside
            )),
            None => out.push_str("clock: never held yet\n"),
        }
    }
    let last = call.args.last.unwrap_or(8);
    let skip = stops.log.len().saturating_sub(last);
    for r in stops.log.iter().skip(skip) {
        let ids = r.ids.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ");
        let on = if ids.is_empty() { String::new() } else { format!(" on {ids}") };
        out.push_str(&format!(
            "  tick {:<5} {} {:.2} s ({}){on}: {} ({:.2} s)\n",
            r.tick,
            r.name,
            r.seconds,
            r.event.name(),
            r.outcome.describe(),
            r.held
        ));
    }
    Ok(out.trim_end().to_owned())
}

/// Makes a hold as a blow would: looks `event` (and `weight`, `reaction`, `cause`, `action`) up in the
/// table, and gives the hold to this machine as it would for a real blow (the clock when alone with a
/// picture, the pictures of `attacker` and `target` in co-op, nothing headless). `seconds` skips the table and holds that
/// long. Answers the rule and what was done.
#[derive(Deserialize, JsonSchema)]
pub struct HitstopPlayArgs {
    /// The combat event: damaged, blocked, parried, perfect_evaded, guard_broken or staggered.
    pub event: Option<String>,
    /// The hit's weight: Light, Heavy, Counter, Break or Finisher.
    pub weight: Option<String>,
    /// The reaction tier the hit asks (`damaged`).
    pub reaction: Option<u8>,
    /// Why someone staggered: PoiseBroken, Parried, GuardBroken or Dev (`staggered`).
    pub cause: Option<String>,
    /// The action that hit.
    pub action: Option<String>,
    /// The body that struck (for the co-op picture hold): `attacker=e5`.
    pub attacker: Option<Id>,
    /// The body that was struck: `target=e6`.
    pub target: Option<Id>,
    /// Hold this long (s) instead of asking the table; within the engine's ceiling.
    pub seconds: Option<f32>,
}

fn parse_event(text: &str) -> Result<EventKind, String> {
    let flat = |s: &str| s.replace('_', "").to_ascii_lowercase();
    EventKind::ALL.into_iter().find(|e| flat(e.name()) == flat(text)).ok_or_else(|| {
        let names = EventKind::ALL.map(EventKind::name);
        let hint = closest(text, names.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        format!("no event `{text}`{hint}; events: {}", names.join(", "))
    })
}

fn parse_weight(text: &str) -> Result<Weight, String> {
    let all = [Weight::Light, Weight::Heavy, Weight::Counter, Weight::Break, Weight::Finisher];
    all.into_iter()
        .find(|w| format!("{w:?}").eq_ignore_ascii_case(text))
        .ok_or_else(|| format!("no weight `{text}`; weights: Light, Heavy, Counter, Break, Finisher"))
}

fn parse_cause(text: &str) -> Result<Cause, String> {
    let all = [Cause::PoiseBroken, Cause::Parried, Cause::GuardBroken, Cause::Dev];
    all.into_iter()
        .find(|c| format!("{c:?}").eq_ignore_ascii_case(text))
        .ok_or_else(|| format!("no cause `{text}`; causes: PoiseBroken, Parried, GuardBroken, Dev"))
}

fn hitstop_play(In(call): In<Call<HitstopPlayArgs>>, world: &mut World) -> ToolResult {
    let HitstopPlayArgs { event, weight, reaction, cause, action, attacker, target, seconds } = call.args;
    let event = parse_event(event.as_deref().unwrap_or("damaged"))?;
    let weight = weight.as_deref().map(parse_weight).transpose()?;
    let cause = cause.as_deref().map(parse_cause).transpose()?;
    let bodies = [attacker, target].iter().flatten().map(|id| id.entity(world)).collect::<Result<Vec<Entity>, _>>()?;
    let stopped = match seconds {
        Some(s) if !s.is_finite() || s <= 0.0 => return Err(format!("seconds {s}: a duration above 0, at most {} s", crate::MAX_HOLD)),
        Some(s) => {
            // A manual hold keeps the table's own ceiling too, and is named so in the journal.
            let ceiling = table(world).map_or(crate::MAX_HOLD, |t| t.ceiling());
            crate::stop(world, event, 0, "manual".to_owned(), s.min(ceiling), &bodies)
        }
        None => {
            let info = HitInfo { event, weight, reaction, cause, action, ignored: false, local: false };
            trigger(world, &info, &bodies)
        }
    };
    world.flush();
    let Some(stopped) = stopped else {
        let why = match table(world) {
            None => "the game has no hit stop table (`*.hitstop.ron`)".to_owned(),
            Some(_) => format!("no rule of the table gives a hold for {}", event.name()),
        };
        return Ok(format!("no hold: {why}"));
    };
    let stops = world.resource::<HitStops>();
    let record = stops.log.back().expect("the blow was recorded");
    Ok(format!("{} {:.2} s ({}): {} ({:.2} s)", stopped.name, stopped.seconds, stopped.event.name(), record.outcome.describe(), record.held))
}
