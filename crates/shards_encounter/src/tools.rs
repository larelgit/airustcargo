//! What agents can do with encounters: look at the runs, start one, reset one; the conditions
//! `step --until named:...` waits on, the describer and the status line.

use ai_core::prelude::*;
use ai_core::tools::register_tool;
use ai_core::{AiIndex, NoArgs, ToolKind};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use shards_action::Dead;
use shards_content::ContentDb;
use shards_motion::MotionState;
use shards_npc::Home;

use crate::def::{EncounterDef, StartDef};
use crate::run::{EncounterMember, EncounterRuns, Run, RunState};
use crate::spawn::Part;

pub(crate) fn register(world: &mut World) {
    register_tool(world, "encounters", Scope::Player, ToolKind::Query, encounters);
    register_tool(world, "encounter", Scope::Player, ToolKind::Query, encounter);
    register_tool(world, "encounter_start", Scope::Dev, ToolKind::Action, encounter_start);
    register_tool(world, "encounter_reset", Scope::Dev, ToolKind::Action, encounter_reset);
    register_tool(world, "encounter_clear", Scope::Dev, ToolKind::Action, encounter_clear);
}

fn secs(world: &World, ticks: u32) -> f32 {
    ticks as f32 * crate::step::dt_of(world)
}

/// `Running`, `Between 2.5 s`, `Wiped (resume in 1.0 s)`.
fn state_text(world: &World, run: &Run) -> String {
    match &run.state {
        RunState::Between { left } => format!("Between {:.1} s", secs(world, *left)),
        RunState::Wiped { resume: Some(left), pulled_back } => {
            format!("Wiped (resume in {:.1} s{})", secs(world, *left), if *pulled_back { ", placed wave pulled back" } else { "" })
        }
        RunState::Wiped { resume: None, .. } => "Wiped (waiting for someone to stand)".to_owned(),
        RunState::Resetting { left } => format!("Resetting {:.1} s", secs(world, *left)),
        other => other.name().to_owned(),
    }
}

fn is_out(world: &World, id: AiId) -> bool {
    match world.get_resource::<AiIndex>().and_then(|i| i.get(id)) {
        None => true,
        Some(e) => world.get::<Dead>(e).is_some(),
    }
}

fn def_of<'w>(world: &'w World, id: &str) -> Option<&'w EncounterDef> {
    world.get_resource::<ContentDb>()?.peek::<EncounterDef>(id)
}

/// One line per run of the world, and the encounters won for good. Without runs it says
/// `no encounters`.
fn encounters(_: In<Call<NoArgs>>, world: &World) -> ToolResult {
    let Some(runs) = world.get_resource::<EncounterRuns>() else { return Ok("no encounters".to_owned()) };
    let mut out = format!("{} encounter run{}", runs.runs.len(), if runs.runs.len() == 1 { "" } else { "s" });
    for run in &runs.runs {
        let waves = def_of(world, &run.encounter).map_or(0, |d| d.waves.len());
        let alive: Vec<String> = run.members.iter().filter(|m| !is_out(world, m.id)).map(|m| m.id.to_string()).collect();
        out.push_str(&format!(
            "\n  {} {} {} wave {}/{} members {} alive ({}) absent {:.1} s",
            run.key,
            run.encounter,
            state_text(world, run),
            run.wave,
            waves,
            alive.len(),
            alive.join(" "),
            secs(world, run.absent)
        ));
    }
    out.push_str(&format!("\ncompleted: [{}]", runs.completed.join(", ")));
    Ok(out)
}

/// One run in full: its state and timers, the start condition now, its members, the
/// definition's waves, budget and tags, and the lock.
#[derive(Deserialize, JsonSchema)]
pub struct EncounterArgs {
    /// Run key, e.g. "yard/ring" (`encounters` lists them).
    pub key: String,
}

fn encounter(In(call): In<Call<EncounterArgs>>, world: &mut World) -> ToolResult {
    let key = call.args.key;
    let run = world.get_resource::<EncounterRuns>().and_then(|r| r.get(&key)).cloned().ok_or_else(|| crate::no_run(world, &key))?;
    let def = def_of(world, &run.encounter).cloned();
    let parts = crate::step::participants(world);
    let mut out = format!("encounter run {} ({}): {}, wave {}", run.key, run.encounter, state_text(world, &run), run.wave);
    out.push_str(&format!("\n  at {:.1}, {:.1}, {:.1}; absent {:.1} s", run.at.x, run.at.y, run.at.z, secs(world, run.absent)));
    if let Some(def) = &def {
        let text = start_summary(world, &run, def, &parts);
        out.push_str(&format!("\n  start: {text}"));
    }
    out.push_str(&format!("\n  members: {}", run.members.len()));
    for m in &run.members {
        let state = if is_out(world, m.id) { "out" } else { "alive" };
        let entity = world.get_resource::<AiIndex>().and_then(|i| i.get(m.id));
        let home = entity
            .and_then(|e| Some((world.get::<Transform>(e)?.translation, world.get::<Home>(e)?.pos?)))
            .map(|(here, home)| format!(", {:.1} m from home", Vec3::new(here.x - home.x, 0.0, here.z - home.z).length()))
            .unwrap_or_default();
        let returning = entity.and_then(|e| world.get::<EncounterMember>(e)).is_some_and(|em| em.returning);
        out.push_str(&format!(
            "\n    {} wave {} {} {}{}{}",
            m.id,
            m.wave,
            if m.counted { "counted" } else { "add" },
            state,
            home,
            if returning { ", returning" } else { "" }
        ));
    }
    if let Some(def) = &def {
        out.push_str(&format!(
            "\n  definition: {} wave{}, budget {}, tags [{}]{}",
            def.waves.len(),
            if def.waves.len() == 1 { "" } else { "s" },
            def.budget.map_or("none".to_owned(), |b| b.to_string()),
            def.tags.join(", "),
            if def.once { ", once" } else { "" }
        ));
        for (i, wave) in def.waves.iter().enumerate() {
            let groups: Vec<String> = wave
                .members
                .iter()
                .map(|g| match g.up_to {
                    Some(up) => format!("{} x{}-{}", g.prefab, g.count, up),
                    None => format!("{} x{}", g.prefab, g.count),
                })
                .collect();
            out.push_str(&format!(
                "\n    wave {}: delay {:.1} s{}: {}",
                i + 1,
                wave.delay,
                if wave.placed { " (placed)" } else { "" },
                groups.join(", ")
            ));
        }
        if let Some(lock) = &def.lock {
            let stands = run.lock.is_some_and(|id| !is_out(world, id));
            out.push_str(&format!("\n  lock `{lock}`: {}", if stands { "standing" } else { "not standing" }));
        }
    }
    Ok(out)
}

/// What the start condition says now.
fn start_summary(world: &mut World, run: &Run, def: &EncounterDef, parts: &[Part]) -> String {
    let standing: Vec<&Part> = parts.iter().filter(|p| !p.out).collect();
    let flat = |v: Vec3| Vec3::new(v.x, 0.0, v.z);
    match &def.start {
        StartDef::Manual => "manual: only `encounter_start` or start_encounter starts it".to_owned(),
        StartDef::Near { within, all_players_in } => {
            let n = standing.iter().filter(|p| flat(p.pos - run.at).length() <= *within).count();
            format!("{}{n}/{} standing participants within {within:.1} m", if *all_players_in { "all_players_in: " } else { "" }, standing.len())
        }
        StartDef::Trigger { volume, all_players_in } => match crate::step::find_volume(world, run, volume, def.radius) {
            None => format!("no trigger volume `{volume}` within {:.0} m of the anchor", def.radius),
            Some(vol) => {
                let n = standing.iter().filter(|p| world.get::<MotionState>(p.entity).is_some_and(|m| m.volumes.contains(&vol))).count();
                format!("{}{n}/{} standing participants inside {volume}", if *all_players_in { "all_players_in: " } else { "" }, standing.len())
            }
        },
    }
}

/// Starts a run that is `Idle` or `Armed`, as its start condition would.
#[derive(Deserialize, JsonSchema)]
pub struct StartArgs {
    /// Run key, e.g. "yard/ring".
    pub key: String,
    /// Skip to this wave (1-based) for a test; nothing is despawned.
    pub wave: Option<u32>,
}

fn encounter_start(In(call): In<Call<StartArgs>>, world: &mut World) -> ToolResult {
    crate::step::start(world, &call.args.key, call.args.wave)
}

/// Resets a run as if nobody had been near (`why: dev`): spawned waves and adds go, survivors of a
/// placed wave are healed and sent home. `key=all` resets every run in a fighting state.
#[derive(Deserialize, JsonSchema)]
pub struct ResetArgs {
    /// Run key, or "all".
    pub key: String,
    /// Also drop the run and its key from the won encounters, so a scenario can leave the world
    /// as it found it; the run is made again from its anchor on the next tick.
    #[serde(default)]
    pub forget: bool,
}

fn encounter_reset(In(call): In<Call<ResetArgs>>, world: &mut World) -> ToolResult {
    let ResetArgs { key, forget } = call.args;
    if key != "all" {
        return crate::step::reset(world, &key, forget);
    }
    let keys: Vec<String> = world.get_resource::<EncounterRuns>().map(|r| r.runs.iter().map(|r| r.key.clone()).collect()).unwrap_or_default();
    let mut done = Vec::new();
    for key in &keys {
        if crate::step::reset(world, key, forget).is_ok_and(|text| !text.contains("nothing to reset")) {
            done.push(key.clone());
        }
    }
    if forget && let Some(mut runs) = world.get_resource_mut::<EncounterRuns>() {
        runs.completed.clear();
    }
    crate::step::drop_when_empty(world);
    Ok(if done.is_empty() { "no run to reset".to_owned() } else { format!("reset {}: {}", done.len(), done.join(", ")) })
}

// --- conditions, describer, status -----------------------------------------------------------

fn any_state(world: &World, test: impl Fn(&RunState) -> bool) -> bool {
    world.get_resource::<EncounterRuns>().is_some_and(|r| r.runs.iter().any(|run| test(&run.state)))
}

pub(crate) fn active(world: &World) -> bool {
    any_state(world, RunState::fighting)
}

pub(crate) fn between(world: &World) -> bool {
    any_state(world, |s| matches!(s, RunState::Between { .. }))
}

pub(crate) fn wiped(world: &World) -> bool {
    any_state(world, |s| matches!(s, RunState::Wiped { .. }))
}

pub(crate) fn won(world: &World) -> bool {
    any_state(world, |s| matches!(s, RunState::Won))
}

pub(crate) fn describe_member(entity: EntityRef) -> Option<String> {
    let m = entity.get::<EncounterMember>()?;
    Some(format!("encounter {} wave {}{}", m.key, m.wave, if m.returning { " returning" } else { "" }))
}

/// `ai/status`: a line per run that is fighting.
pub(crate) fn status(world: &World) -> Value {
    let Some(runs) = world.get_resource::<EncounterRuns>() else { return Value::Null };
    let lines: Vec<String> = runs
        .runs
        .iter()
        .filter(|r| r.state.fighting())
        .map(|r| format!("encounter {} wave {}/{} {}", r.key, r.wave, def_of(world, &r.encounter).map_or(0, |d| d.waves.len()), r.state.name()))
        .collect();
    if lines.is_empty() { Value::Null } else { json!(lines) }
}

/// Kills the counted members of the wave in progress, the way their health running out would
/// (no killer): the run sees them out on the tick's step. For tests and scenarios, which cannot
/// name the members before they spawn.
#[derive(Deserialize, JsonSchema)]
pub struct ClearArgs {
    /// Run key, e.g. "yard/ring".
    pub key: String,
}

fn encounter_clear(In(call): In<Call<ClearArgs>>, world: &mut World) -> ToolResult {
    let key = call.args.key;
    let run = world.get_resource::<EncounterRuns>().and_then(|r| r.get(&key)).cloned().ok_or_else(|| crate::no_run(world, &key))?;
    let mut killed = Vec::new();
    for m in run.members.iter().filter(|m| m.counted && m.wave == run.wave) {
        let Some(entity) = world.get_resource::<AiIndex>().and_then(|i| i.get(m.id)) else { continue };
        if world.get::<Dead>(entity).is_some() {
            continue;
        }
        // A fighter dies the engine's way (its health ran out, no killer); anything else is marked.
        let fighter = world.get::<shards_action::Fighter>(entity).is_some();
        match world.get_mut::<shards_action::Health>(entity) {
            Some(mut health) if fighter => health.current = 0.0,
            _ => {
                world.entity_mut(entity).insert(Dead);
            }
        }
        killed.push(m.id.to_string());
    }
    if killed.is_empty() {
        return Err(format!("run {key} has no living counted member in wave {} to clear", run.wave));
    }
    Ok(format!("cleared wave {} of {key}: {}", run.wave, killed.join(" ")))
}
