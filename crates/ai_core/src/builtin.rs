//! Built-in tools every game gets: perception queries, sim control, reflection edits.

use std::time::Duration;

use bevy::prelude::*;
use bevy::reflect::serde::{TypedReflectDeserializer, TypedReflectSerializer};
use bevy::reflect::{GetPath, PartialReflect, ReflectFromReflect, ReflectPath, TypeRegistration, TypeRegistry};
use schemars::JsonSchema;
use serde::de::DeserializeSeed;
use serde::Deserialize;
use serde_json::Value;

use crate::describe::facts;
use crate::id::{AiId, AI_ID_IS_MANAGED};
use crate::journal::{self, AiJournal, JournalKind};
use crate::observe::{self, entity_line, ObserveOptions};
use crate::session::{configure_session, AgentSessions, Scope, SessionChange};
use crate::sim::{self, SimGate, StepEnd};
use crate::spatial::{self, relative, EYE_HEIGHT};
use crate::tools::{register_tool, AiTools, Call, Id, NoArgs, ToolKind, ToolResult};

pub(crate) fn register(world: &mut World) {
    use Scope::{Dev, Player};
    use ToolKind::{Action, Control, Query};
    register_tool(world, "observe", Player, Query, observe_tool);
    register_tool(world, "inspect", Player, Query, inspect_tool);
    register_tool(world, "nearby", Player, Query, nearby_tool);
    register_tool(world, "line_of_sight", Player, Query, line_of_sight_tool);
    register_tool(world, "journal", Player, Query, journal_tool);
    register_tool(world, "tools", Player, Query, tools_tool);

    register_tool(world, "pause", Dev, Control, pause_tool);
    register_tool(world, "resume", Dev, Control, resume_tool);
    register_tool(world, "speed", Dev, Control, speed_tool);
    register_tool(world, "step", Dev, Control, step_tool);
    register_tool(world, "possess", Dev, Control, possess_tool);
    register_tool(world, "teleport", Dev, Action, teleport_tool);
    register_tool(world, "despawn", Dev, Action, despawn_tool);
    register_tool(world, "get", Dev, Query, get_tool);
    register_tool(world, "set", Dev, Action, set_tool);
}

// --- perception ------------------------------------------------------------------

/// Look at the world: your status, what you see from your eyes (sorted by threat;
/// `occluded` when something is in the way), what is near but outside your field of
/// view (if the game sets one), and journal entries since your last look. Distances and
/// compass directions are relative to you; N = -Z, E = +X. `scr(x,y)`: where it is in
/// the game window, when there is one.
#[derive(Deserialize, JsonSchema, Default)]
pub struct ObserveArgs {
    /// Show journal entries after this sequence number instead of "since my last look".
    pub since_seq: Option<u64>,
}

fn observe_tool(In(call): In<Call<ObserveArgs>>, world: &mut World) -> ToolResult {
    let agent = call.caller.agent.clone();
    let after = call
        .args
        .since_seq
        .unwrap_or_else(|| world.resource::<AgentSessions>().get(&agent).map_or(0, |s| s.seen_seq));
    let text = observe::observe(world, &ObserveOptions { subject: call.caller.subject, journal_after: Some(after) });
    let last = world.resource::<AiJournal>().last_seq();
    world.resource_mut::<AgentSessions>().get_or_create(&agent).seen_seq = last;
    Ok(text)
}

/// Everything known about one entity: its description, position and every gameplay
/// component with current values (reflection).
#[derive(Deserialize, JsonSchema)]
pub struct InspectArgs {
    pub id: Id,
}

fn inspect_tool(In(call): In<Call<InspectArgs>>, world: &mut World) -> ToolResult {
    let e = call.args.id.entity(world)?;
    observe::inspect(world, e, call.caller.subject)
}

/// AI-visible entities within a radius, nearest first (including ones out of view).
#[derive(Deserialize, JsonSchema, Default)]
pub struct NearbyArgs {
    /// Radius in meters. Default 15.
    pub radius: Option<f32>,
    /// Only entities whose line contains this text (case-insensitive): "skeleton", "boss".
    pub filter: Option<String>,
    /// Center of the search. Default: you.
    pub around: Option<Id>,
}

fn nearby_tool(In(call): In<Call<NearbyArgs>>, world: &mut World) -> ToolResult {
    let center_entity = match call.args.around {
        Some(id) => id.entity(world)?,
        None => call.caller.require_subject()?,
    };
    let center = facts(world, center_entity).ok_or("center is not visible to the AI")?;
    let radius = call.args.radius.unwrap_or(15.0).clamp(0.5, 500.0);
    let lines = observe::nearby(world, center.pos, radius, call.args.filter.as_deref(), Some(center.id));
    if lines.is_empty() {
        return Ok(format!("nothing within {radius:.0}m of {}", center.id));
    }
    Ok(format!("within {radius:.0}m of {}:\n  {}", center.id, lines.join("\n  ")))
}

/// Is the line of sight between two entities clear (rays at eye height)?
#[derive(Deserialize, JsonSchema)]
pub struct LineOfSightArgs {
    /// Default: you.
    pub from: Option<Id>,
    pub to: Id,
}

fn line_of_sight_tool(In(call): In<Call<LineOfSightArgs>>, world: &mut World) -> ToolResult {
    let from = match call.args.from {
        Some(id) => id.entity(world)?,
        None => call.caller.require_subject()?,
    };
    let to = call.args.to.entity(world)?;
    let (a, b) = (facts(world, from).ok_or("from: not visible")?, facts(world, to).ok_or("to: not visible")?);
    let clear = spatial::line_of_sight(world, a.pos + Vec3::Y * EYE_HEIGHT, b.pos + Vec3::Y * EYE_HEIGHT, &[from, to]);
    let verdict = if clear { "clear" } else { "blocked" };
    Ok(format!("{verdict}: {} → {} ({})", a.id, b.id, relative(a.pos, b.pos)))
}

/// Journal entries: gameplay events, agent actions, sim control, warnings and errors.
#[derive(Deserialize, JsonSchema, Default)]
pub struct JournalArgs {
    /// Only entries after this sequence number.
    pub since_seq: Option<u64>,
    /// At most this many newest entries. Default 30.
    pub last: Option<usize>,
}

fn journal_tool(In(call): In<Call<JournalArgs>>, journal: Res<AiJournal>) -> ToolResult {
    let (entries, missed) = journal.after(call.args.since_seq.unwrap_or(0));
    let entries: Vec<_> = entries.collect();
    let last = call.args.last.unwrap_or(30).max(1);
    let skip = entries.len().saturating_sub(last);
    let mut out = format!("journal (newest seq {})", journal.last_seq());
    if missed > 0 {
        out.push_str(&format!(", {missed} entries dropped"));
    }
    for e in &entries[skip..] {
        out.push_str(&format!("\n  #{} {e}", e.seq));
    }
    Ok(out)
}

/// The tools you may call, with signatures.
fn tools_tool(In(call): In<Call<NoArgs>>, tools: Res<AiTools>) -> ToolResult {
    let specs = tools.specs(call.caller.scope);
    let lines: Vec<String> = specs
        .iter()
        .map(|s| {
            let first = s.description.lines().next().unwrap_or("");
            format!("{}  [{}]  {}", s.signature, s.scope, first)
        })
        .collect();
    Ok(lines.join("\n"))
}

// --- time ------------------------------------------------------------------------

/// Freeze the simulation (rendering, UI and these tools keep working).
fn pause_tool(In(_): In<Call<NoArgs>>, world: &mut World) -> ToolResult {
    sim::pause(world);
    // Paused on purpose: the end of loading must not resume it.
    if let Some(mut gate) = world.get_resource_mut::<SimGate>() {
        gate.forget_pause();
    }
    journal::log(world, JournalKind::Sim, "paused");
    Ok(format!("paused at tick {}", sim::tick(world)))
}

/// Unfreeze the simulation (real-time play at the current speed).
fn resume_tool(In(_): In<Call<NoArgs>>, world: &mut World) -> ToolResult {
    if let Some(loading) = sim::gate_message(world) {
        return Err(format!("{loading}; a game started without --paused runs by itself once loaded"));
    }
    sim::resume(world);
    journal::log(world, JournalKind::Sim, "resumed");
    Ok(format!("running from tick {}", sim::tick(world)))
}

/// Real-time speed multiplier, e.g. 0.25 for slow motion. Does not affect `step`.
#[derive(Deserialize, JsonSchema)]
pub struct SpeedArgs {
    pub x: f32,
}

fn speed_tool(In(call): In<Call<SpeedArgs>>, world: &mut World) -> ToolResult {
    sim::set_speed(world, call.args.x);
    journal::log(world, JournalKind::Sim, format!("speed {}x", call.args.x));
    Ok(format!("speed {}x", sim::speed(world)))
}

/// Stop condition for `step`. Any matching condition stops the run.
#[derive(Deserialize, JsonSchema, Clone, Debug)]
#[serde(rename_all = "snake_case")]
pub enum StopWhen {
    /// A new journal entry contains this text (case-insensitive), e.g. "phase", "died".
    Journal(String),
    /// The entity no longer exists.
    Gone(Id),
    /// The entity's observation line contains this text, e.g. {"id": 42, "text": "Windup"}.
    Shows { id: Id, text: String },
    /// Two entities are within `meters` of each other.
    Near { a: Id, b: Id, meters: f32 },
    /// A numeric component field crosses a threshold:
    /// {"id": 12, "component": "Health", "path": "current", "below": 30}.
    Field { id: Id, component: String, path: String, below: Option<f64>, above: Option<f64> },
    /// A condition the game registered by name.
    Named(String),
}

/// Predicate registered by the game: `app.add_ai_condition("boss_phase_2", |w| ...)`.
#[derive(Resource, Default)]
pub struct AiConditions(pub bevy::platform::collections::HashMap<String, fn(&World) -> bool>);

/// Advance the paused simulation by fixed ticks (60 per second by default) and return
/// what happened. With `until`, runs until a condition fires (at most `ticks`).
#[derive(Deserialize, JsonSchema, Default)]
pub struct StepArgs {
    /// Maximum ticks to run. Default: 1, or 600 (10 s) when `until` is given.
    pub ticks: Option<u64>,
    /// Stop as soon as any of these conditions is met.
    #[serde(default)]
    pub until: Vec<StopWhen>,
}

const MAX_STEP: u64 = 216_000; // one hour of game time

fn step_tool(In(call): In<Call<StepArgs>>, world: &mut World) -> ToolResult {
    let StepArgs { ticks, until } = call.args;
    let max = ticks.unwrap_or(if until.is_empty() { 1 } else { 600 }).min(MAX_STEP);
    for cond in &until {
        validate(world, cond)?;
    }
    let journal_mark = world.resource::<AiJournal>().last_seq();
    let report = sim::step(world, max, Duration::from_secs(20), |w| {
        // The tick's log lines are in the journal before `journal:` conditions look.
        journal::drain_logs(w);
        check(w, &until, journal_mark)
    })?;
    let mut msg = format!("ran {} ticks ({} → {})", report.ticks(), report.from_tick, report.to_tick);
    match report.end {
        StepEnd::Completed if until.is_empty() => {}
        StepEnd::Completed => msg.push_str(", no condition met"),
        StepEnd::Condition(why) => msg.push_str(&format!(", stopped: {why}")),
        StepEnd::WallClock => msg.push_str(", stopped: 20 s wall-clock budget"),
        StepEnd::SystemFailed(failures) => {
            let first = &failures[0];
            msg.push_str(&format!(", stopped: gameplay {}", first.headline()));
            if failures.len() > 1 {
                msg.push_str(&format!(" (+{} more)", failures.len() - 1));
            }
            msg.push_str(&format!(": {}", first.message));
            if let Some(path) = failures.iter().find_map(|f| f.crash_replay.as_ref()) {
                msg.push_str(&format!("; crash replay: {path}"));
            }
        }
    }
    Ok(msg)
}

fn validate(world: &World, cond: &StopWhen) -> Result<(), String> {
    match cond {
        StopWhen::Gone(_) | StopWhen::Journal(_) => Ok(()),
        StopWhen::Shows { id, .. } | StopWhen::Field { id, .. } => id.entity(world).map(|_| ()),
        StopWhen::Near { a, b, .. } => a.entity(world).and(b.entity(world)).map(|_| ()),
        StopWhen::Named(name) => {
            let known = world.get_resource::<AiConditions>().map(|c| c.0.contains_key(name)).unwrap_or(false);
            if known {
                Ok(())
            } else {
                let mut names: Vec<String> =
                    world.get_resource::<AiConditions>().map(|c| c.0.keys().cloned().collect()).unwrap_or_default();
                names.sort();
                Err(format!("unknown condition `{name}`; known: {}", names.join(", ")))
            }
        }
    }
}

fn check(world: &World, until: &[StopWhen], journal_mark: u64) -> Option<String> {
    for cond in until {
        let hit = match cond {
            StopWhen::Journal(text) => {
                let needle = text.to_lowercase();
                world
                    .resource::<AiJournal>()
                    .after(journal_mark)
                    .0
                    .find(|e| e.kind != JournalKind::Agent && e.text.to_lowercase().contains(&needle))
                    .map(|e| format!("journal: {e}"))
            }
            StopWhen::Gone(id) => id.entity(world).is_err().then(|| format!("{id} is gone")),
            StopWhen::Shows { id, text } => {
                let needle = text.to_lowercase();
                id.entity(world).ok().and_then(|e| facts(world, e)).and_then(|f| {
                    let line = entity_line(&f, None, None);
                    line.to_lowercase().contains(&needle).then(|| line.clone())
                })
            }
            StopWhen::Near { a, b, meters } => {
                let pa = a.entity(world).ok().and_then(|e| facts(world, e)).map(|f| f.pos);
                let pb = b.entity(world).ok().and_then(|e| facts(world, e)).map(|f| f.pos);
                match (pa, pb) {
                    (Some(pa), Some(pb)) if pa.distance(pb) <= *meters => {
                        Some(format!("{a} within {:.1}m of {b}", pa.distance(pb)))
                    }
                    _ => None,
                }
            }
            StopWhen::Field { id, component, path, below, above } => id.entity(world).ok().and_then(|e| {
                let v = read_number(world, e, component, path).ok()?;
                let low = below.is_some_and(|b| v < b);
                let high = above.is_some_and(|a| v > a);
                (low || high).then(|| format!("{id} {component}.{path} = {v}"))
            }),
            StopWhen::Named(name) => world
                .get_resource::<AiConditions>()
                .and_then(|c| c.0.get(name).copied())
                .and_then(|f| f(world).then(|| format!("condition {name}"))),
        };
        if hit.is_some() {
            return hit;
        }
    }
    None
}

// --- dev: possession, transforms, reflection ------------------------------------

/// Act as another entity: it becomes your "YOU" and the target of your player tools.
/// Without `id`, return to the default (the player character).
#[derive(Deserialize, JsonSchema, Default)]
pub struct PossessArgs {
    pub id: Option<Id>,
    /// Play as this player's body instead (a networked game's players: 0 is the host's),
    /// found again on every call, so a respawned body is still yours.
    pub player: Option<u8>,
}

fn possess_tool(In(call): In<Call<PossessArgs>>, world: &mut World) -> ToolResult {
    if let Some(id) = call.args.id {
        id.entity(world)?;
    }
    let subject = call.args.id.map(|i| i.0);
    let change = SessionChange {
        subject,
        player: call.args.player,
        clear_subject: subject.is_none() && call.args.player.is_none(),
        ..Default::default()
    };
    let session = configure_session(world, &call.caller.agent, &change)?;
    Ok(match (session.subject, session.player) {
        (Some(id), _) => format!("you are now {id}"),
        (_, Some(n)) => format!("you are now player {n}'s body, whichever entity that is"),
        _ => "you are the player character again".to_owned(),
    })
}

/// Move an entity instantly to (x, z) on the ground plane (y optional).
#[derive(Deserialize, JsonSchema)]
pub struct TeleportArgs {
    pub id: Id,
    pub x: f32,
    pub z: f32,
    pub y: Option<f32>,
}

fn teleport_tool(In(call): In<Call<TeleportArgs>>, world: &mut World) -> ToolResult {
    let e = call.args.id.entity(world)?;
    let mut tf = world.get_mut::<Transform>(e).ok_or("entity has no Transform")?;
    tf.translation.x = call.args.x;
    tf.translation.z = call.args.z;
    if let Some(y) = call.args.y {
        tf.translation.y = y;
    }
    Ok(format!("{} at {}", call.args.id, crate::spatial::fmt_pos(tf.translation)))
}

/// Remove an entity from the world.
#[derive(Deserialize, JsonSchema)]
pub struct DespawnArgs {
    pub id: Id,
}

fn despawn_tool(In(call): In<Call<DespawnArgs>>, world: &mut World) -> ToolResult {
    let e = call.args.id.entity(world)?;
    world.despawn(e);
    Ok(format!("{} despawned", call.args.id))
}

/// Read a component (or one field of it) as JSON via reflection.
#[derive(Deserialize, JsonSchema)]
pub struct GetArgs {
    pub id: Id,
    /// Component type, short name ("Health") or full path.
    pub component: String,
    /// Field path inside the component, e.g. "current" or "translation.x".
    pub path: Option<String>,
}

fn get_tool(In(call): In<Call<GetArgs>>, world: &mut World) -> ToolResult<Value> {
    let e = call.args.id.entity(world)?;
    let app_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = app_registry.read();
    let (reg, value) = reflect_component(world, &registry, e, &call.args.component)?;
    let value = match &call.args.path {
        Some(path) => path.as_str().reflect_element(value).map_err(|err| format!("{}: {err}", reg_name(reg)))?,
        None => value,
    };
    serde_json::to_value(TypedReflectSerializer::new(value, &registry)).map_err(|e| e.to_string())
}

/// Overwrite a component (or one field of it) from JSON via reflection. Use `get`
/// first to see the shape. Example: {"id": 12, "component": "Health", "path": "current", "value": 100}.
#[derive(Deserialize, JsonSchema)]
pub struct SetArgs {
    pub id: Id,
    pub component: String,
    pub path: Option<String>,
    pub value: Value,
}

fn set_tool(In(call): In<Call<SetArgs>>, world: &mut World) -> ToolResult {
    let SetArgs { id, component, path, value } = call.args;
    let e = id.entity(world)?;
    let app_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = app_registry.read();
    let reg = find_registration(world, &registry, e, &component)?;
    if reg.type_id() == std::any::TypeId::of::<AiId>() {
        // An in-place write would bypass the hooks that keep `AiIndex` in sync.
        return Err(AI_ID_IS_MANAGED.into());
    }
    let reflect_component = reg.data::<ReflectComponent>().ok_or("not a reflectable component")?;
    let name = reg_name(reg);
    let mut entity = world.entity_mut(e);
    let mut target = reflect_component
        .reflect_mut(&mut entity)
        .ok_or_else(|| format!("{id} has no {name} (or it is immutable)"))?;
    let field: &mut dyn PartialReflect = match &path {
        Some(p) => target.reflect_path_mut(p.as_str()).map_err(|err| format!("{name}: {err}"))?,
        None => target.as_partial_reflect_mut(),
    };
    let field_reg = field
        .get_represented_type_info()
        .and_then(|info| registry.get(info.type_id()))
        .ok_or("field type is not registered for reflection")?;
    let parsed = TypedReflectDeserializer::new(field_reg, &registry)
        .deserialize(value.clone())
        .map_err(|err| format!("value does not fit {}: {err}", field_reg.type_info().type_path_table().short_path()))?;
    // The value replaces what is there: applied, a shorter list (`[]`) would only overwrite
    // the first elements and leave the rest.
    let whole = field_reg.data::<ReflectFromReflect>().and_then(|from| from.from_reflect(parsed.as_ref()));
    let replaced = match (whole, field.try_as_reflect_mut()) {
        (Some(value), Some(field)) => field.set(value).is_ok(),
        _ => false,
    };
    if !replaced {
        field.try_apply(parsed.as_ref()).map_err(|err| err.to_string())?;
    }
    let at = path.map(|p| format!("{name}.{p}")).unwrap_or(name);
    Ok(format!("{id} {at} = {value}"))
}

fn reg_name(reg: &TypeRegistration) -> String {
    reg.type_info().type_path_table().short_path().to_owned()
}

fn find_registration<'r>(
    world: &World,
    registry: &'r TypeRegistry,
    entity: Entity,
    component: &str,
) -> Result<&'r TypeRegistration, String> {
    let found = registry
        .get_with_short_type_path(component)
        .or_else(|| registry.get_with_type_path(component))
        .filter(|r| r.data::<ReflectComponent>().is_some());
    if let Some(reg) = found {
        return Ok(reg);
    }
    if registry.is_ambiguous(component) {
        // Several types share the short name (a game's `Health` and an engine's): the
        // one this entity has is meant.
        let mine: Vec<&TypeRegistration> = world
            .inspect_entity(entity)
            .into_iter()
            .flatten()
            .filter_map(|(_, info)| registry.get(info.type_id()?))
            .filter(|r| r.data::<ReflectComponent>().is_some() && reg_name(r) == component)
            .collect();
        if let [only] = mine.as_slice() {
            return Ok(only);
        }
        let paths: Vec<&str> = registry
            .iter()
            .filter(|r| r.type_info().type_path_table().short_path() == component)
            .map(|r| r.type_info().type_path())
            .collect();
        return Err(format!("`{component}` names several components; use the full path: {}", paths.join(", ")));
    }
    let names = reflectable_components(world, registry, entity);
    Err(format!("unknown component `{component}`; this entity has: {}", names.join(", ")))
}

fn reflect_component<'w, 'r>(
    world: &'w World,
    registry: &'r TypeRegistry,
    entity: Entity,
    component: &str,
) -> Result<(&'r TypeRegistration, &'w dyn PartialReflect), String> {
    let reg = find_registration(world, registry, entity, component)?;
    let rc = reg.data::<ReflectComponent>().ok_or("not a component")?;
    let value = rc.reflect(world.entity(entity)).ok_or_else(|| format!("entity has no {}", reg_name(reg)))?;
    Ok((reg, value.as_partial_reflect()))
}

fn reflectable_components(world: &World, registry: &TypeRegistry, entity: Entity) -> Vec<String> {
    let Ok(infos) = world.inspect_entity(entity) else {
        return Vec::new();
    };
    let mut names: Vec<String> = infos
        .filter_map(|(_, info)| info.type_id())
        .filter_map(|t| registry.get(t))
        .filter(|r| r.data::<ReflectComponent>().is_some())
        // Our own markers are noise here; a game's `AimAssist` is not.
        .filter(|r| !r.type_info().type_path().starts_with("ai_core::"))
        .map(reg_name)
        .collect();
    names.sort();
    names
}

/// Reads a numeric field via reflection (f32/f64/integers) as f64.
pub fn read_number(world: &World, entity: Entity, component: &str, path: &str) -> Result<f64, String> {
    let app_registry = world.resource::<AppTypeRegistry>().clone();
    let registry = app_registry.read();
    let (_, value) = reflect_component(world, &registry, entity, component)?;
    let field = path.reflect_element(value).map_err(|e| e.to_string())?;
    macro_rules! try_num {
        ($($t:ty),*) => {
            $(if let Some(v) = field.try_downcast_ref::<$t>() { return Ok(*v as f64); })*
        };
    }
    try_num!(f32, f64, i8, i16, i32, i64, u8, u16, u32, u64, usize, isize);
    Err(format!("{component}.{path} is not a number"))
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;
    use serde_json::json;

    use crate::prelude::*;

    #[derive(Component, Reflect, Default, Clone, PartialEq, Debug)]
    #[reflect(Component, Default)]
    struct Bag {
        items: Vec<String>,
        weight: f32,
    }

    fn game() -> App {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin)
            .add_plugins(crate::AiCorePlugin {
                remote_port: None,
                crash_replays: false,
                sim: SimConfig { seed: 1, start_paused: true, ..Default::default() },
                ..Default::default()
            })
            .register_type::<Bag>();
        app.world_mut().spawn((AiKind::new("Mule"), Bag { items: vec!["rope".into(), "lamp".into()], weight: 3.0 }));
        app.finish();
        app.cleanup();
        app.update();
        app
    }

    fn bag(app: &mut App) -> Bag {
        let world = app.world_mut();
        world.query::<&Bag>().single(world).unwrap().clone()
    }

    #[test]
    fn set_replaces_lists_instead_of_writing_over_their_first_elements() {
        let mut app = game();
        let set = |app: &mut App, path: Option<&str>, value: serde_json::Value| {
            let out = crate::call_tool(app.world_mut(), "dev", "set", json!({"id": "e1", "component": "Bag", "path": path, "value": value}));
            assert!(out.ok, "{}", out.result);
        };
        set(&mut app, Some("items"), json!([]));
        assert_eq!(bag(&mut app).items, Vec::<String>::new(), "an empty list empties it");
        set(&mut app, Some("items"), json!(["map"]));
        assert_eq!(bag(&mut app).items, ["map"]);
        set(&mut app, None, json!({"items": [], "weight": 1.5}));
        assert_eq!(bag(&mut app), Bag { items: Vec::new(), weight: 1.5 }, "the whole component, lists included");
        set(&mut app, Some("weight"), json!(2));
        assert_eq!(bag(&mut app).weight, 2.0);
    }
}
