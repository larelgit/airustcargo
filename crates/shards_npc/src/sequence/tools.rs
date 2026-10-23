//! What agents see of sequences and do with them: the tools `sequence_play` and `sequence_stop` (dev) and
//! `sequences` (a query), the journal lines, the `SEQUENCE` lines of `observe`, the NPC's fragment, the
//! condition `sequence_playing`, the replay check and the engine rule `npc.sequence_holds`.

use std::collections::BTreeMap;

use ai_core::prelude::*;
use ai_core::{with_query, AiId, AiIndex, Offenders, Snapshot};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::Value;
use shards_content::ContentDb;
use shards_core::EngineAppExt;

use super::def::{SequenceDef, Step};
use super::run::{Run, SequenceEnded, SequenceHold, SequenceId, SequenceStarted, SequenceStep, Sequences};
use super::{play_sequence, stop_sequence};

pub(super) fn register(app: &mut App) {
    app.add_ai_tool("sequence_play", Scope::Dev, sequence_play)
        .add_ai_tool("sequence_stop", Scope::Dev, sequence_stop)
        .add_ai_query("sequences", Scope::Player, sequences)
        // `step --until named:sequence_playing`: a sequence is under way.
        .add_ai_condition("sequence_playing", playing)
        .register_ai_world_describer("sequence", describe_held)
        .register_ai_observe_section(observe_section)
        .add_engine_invariant("npc.sequence_holds", sequence_holds)
        .journal_event::<SequenceStarted>(|e, _| {
            let roles: Vec<String> = e.roles.iter().map(|(r, id)| format!("{r}={id}")).collect();
            Some(format!("sequence {} ({}) starts: {}", e.sequence, e.def, roles.join(" ")))
        })
        .journal_event::<SequenceStep>(|e, _| Some(format!("sequence {} ({}) {}/{}: {}", e.sequence, e.def, e.index + 1, e.of, e.text)))
        .journal_event::<SequenceEnded>(|e, _| {
            // A run that could not go on says why in a warning of its own.
            if e.why.is_some() {
                return None;
            }
            Some(if e.skipped {
                format!("sequence {} ({}) stopped at step {}/{}", e.sequence, e.def, e.step + 1, e.of)
            } else {
                format!("sequence {} ({}) ended", e.sequence, e.def)
            })
        });
}

/// Plays a sequence: binds each of its roles to a body and runs it. Name the roles as `role.NAME=eN`
/// (one for each role of the definition), or as `roles` in a JSON object; a body is its id, or its name when
/// only one body has it (`role.guide=Guide`). The bodies some step acts on must be NPCs: their brains are
/// dormant until it ends. `sequences` lists the definitions and the runs.
#[derive(Deserialize, JsonSchema)]
pub struct PlayArgs {
    /// The `sequence` definition, e.g. "greeting".
    pub id: String,
    /// Role name to body: `{"guard": "e7"}`.
    #[serde(default)]
    pub roles: BTreeMap<String, String>,
    /// `role.NAME=eN` for each role, e.g. `role.guard=e7`.
    #[serde(flatten)]
    pub rest: BTreeMap<String, Value>,
}

/// The body a tool call names: an id (`e7`, `7`), else the name only one body with an id has (any case).
fn body_of(world: &mut World, text: &str) -> Result<Entity, String> {
    let text = text.trim();
    if let Some(id) = AiId::parse(text) {
        return Id(id).entity(world);
    }
    let mut named: Vec<(AiId, Entity)> = Vec::new();
    if let Some(mut q) = world.try_query::<(Entity, &AiId, &Name)>() {
        named = q.iter(world).filter(|(_, _, n)| n.as_str().eq_ignore_ascii_case(text)).map(|(e, id, _)| (*id, e)).collect();
    }
    named.sort();
    match named[..] {
        [(_, entity)] => Ok(entity),
        [] => Err(format!("no body with the id or the name `{text}`")),
        _ => Err(format!("{} bodies are named `{text}`: {}; name one by id", named.len(), named.iter().map(|(id, _)| id.to_string()).collect::<Vec<_>>().join(", "))),
    }
}

fn sequence_play(In(call): In<Call<PlayArgs>>, world: &mut World) -> ToolResult {
    let PlayArgs { id, mut roles, rest } = call.args;
    for (key, value) in rest {
        let Some(role) = key.strip_prefix("role.") else {
            return Err(format!("unknown argument `{key}`: sequence_play takes `id=` and `role.NAME=eN` for each role of the sequence"));
        };
        let body = match value {
            Value::String(text) => text,
            Value::Number(n) => n.to_string(),
            other => return Err(format!("`{key}` is a body like e7 (or its name), not {other}")),
        };
        roles.insert(role.to_owned(), body);
    }
    let mut bound: Vec<(String, Entity)> = Vec::new();
    for (role, body) in &roles {
        bound.push((role.clone(), body_of(world, body).map_err(|e| format!("role `{role}`: {e}"))?));
    }
    let named: Vec<(&str, Entity)> = bound.iter().map(|(r, e)| (r.as_str(), *e)).collect();
    let run = play_sequence(world, &id, &named)?;
    let held: Vec<String> = bound
        .iter()
        .map(|(r, e)| format!("{r}={}", world.get::<AiId>(*e).map_or_else(String::new, ToString::to_string)))
        .collect();
    let steps = world.resource::<ContentDb>().peek::<SequenceDef>(&id).map_or(0, |d| d.steps.len());
    Ok(format!("ok: sequence {run} ({id}) plays, {steps} steps: {}", held.join(" ")))
}

/// Stops a sequence: its bodies are free at once and go on from where their brains were. Name it by its
/// number (`run=3`, from `sequences`), by its definition (`id=greeting`: every run of it) or all of
/// them (`all=true`).
#[derive(Deserialize, JsonSchema)]
pub struct StopArgs {
    /// The run's number.
    pub run: Option<u32>,
    /// Every run of this definition.
    pub id: Option<String>,
    /// Every run.
    #[serde(default)]
    pub all: bool,
}

fn sequence_stop(In(call): In<Call<StopArgs>>, world: &mut World) -> ToolResult {
    let StopArgs { run, id, all } = call.args;
    let playing: Vec<(SequenceId, String)> =
        world.get_resource::<Sequences>().map(|s| s.runs.iter().map(|r| (SequenceId(r.id), r.def.clone())).collect()).unwrap_or_default();
    if playing.is_empty() {
        return Err("no sequence is playing".into());
    }
    let chosen: Vec<SequenceId> = match (run, id.as_deref(), all) {
        (None, None, true) => playing.iter().map(|(n, _)| *n).collect(),
        (Some(n), None, false) => {
            if !playing.iter().any(|(r, _)| r.0 == n) {
                let list: Vec<String> = playing.iter().map(|(r, d)| format!("{r} ({d})")).collect();
                return Err(format!("no sequence {n} is playing; playing: {}", list.join(", ")));
            }
            vec![SequenceId(n)]
        }
        (None, Some(def), false) => {
            let hits: Vec<SequenceId> = playing.iter().filter(|(_, d)| d == def).map(|(n, _)| *n).collect();
            if hits.is_empty() {
                let list: Vec<String> = playing.iter().map(|(r, d)| format!("{r} ({d})")).collect();
                return Err(format!("no run of `{def}` is playing; playing: {}", list.join(", ")));
            }
            hits
        }
        _ => return Err("name one thing to stop: `run=N`, `id=DEFINITION` or `all=true`".into()),
    };
    for n in &chosen {
        stop_sequence(world, *n);
    }
    let names: Vec<String> = chosen.iter().map(ToString::to_string).collect();
    Ok(format!("ok: stopped sequence {}", names.join(", ")))
}

/// The sequences content has and the runs playing, one line each; or one run in full (`run=3`).
#[derive(Deserialize, JsonSchema)]
pub struct SequencesArgs {
    /// A run's number, for its steps and bodies.
    pub run: Option<u32>,
}

fn sequences(In(call): In<Call<SequencesArgs>>, world: &World) -> ToolResult {
    let db = world.get_resource::<ContentDb>();
    if let Some(n) = call.args.run {
        let run = world
            .get_resource::<Sequences>()
            .and_then(|s| s.get(SequenceId(n)))
            .ok_or_else(|| format!("no sequence {n} is playing (`sequences` lists the runs)"))?;
        let def = db.and_then(|db| db.peek::<SequenceDef>(&run.def));
        let mut out = format!("sequence {n} ({}) step {}/{}, tick {}", run.def, run.step + 1, def.map_or(0, |d| d.steps.len()), run.started);
        for (role, id) in &run.roles {
            out.push_str(&format!("\n  {role} = {id}{}", if run.actors.contains(id) { " (held)" } else { "" }));
        }
        if let Some(def) = def {
            for (i, step) in def.steps.iter().enumerate() {
                let mark = if i == run.step as usize { ">" } else { " " };
                let who = step.actor().and_then(|r| run.body(r)).map(|id| id.to_string()).unwrap_or_default();
                out.push_str(&format!("\n {mark} {}. {}", i + 1, step.text(&who)));
            }
        }
        return Ok(out);
    }
    let mut out = String::new();
    let defs: Vec<(&str, &SequenceDef)> = db.map(|db| db.peek_iter::<SequenceDef>().collect()).unwrap_or_default();
    out.push_str(&format!("{} sequence{}", defs.len(), if defs.len() == 1 { "" } else { "s" }));
    for (id, def) in &defs {
        out.push_str(&format!("\n  {id}: roles {}, {} steps", def.roles.join(" "), def.steps.len()));
    }
    let runs: Vec<&Run> = world.get_resource::<Sequences>().map(|s| s.runs.iter().collect()).unwrap_or_default();
    out.push_str(&format!("\n{} playing", runs.len()));
    for run in runs {
        out.push_str(&format!("\n  {}", run_line(world, run)));
    }
    Ok(out)
}

/// `3 greeting step 2/7: e7 turns to the visitor`.
fn run_line(world: &World, run: &Run) -> String {
    let def = world.get_resource::<ContentDb>().and_then(|db| db.peek::<SequenceDef>(&run.def));
    let step = def.and_then(|d| d.steps.get(run.step as usize));
    let who = step.and_then(Step::actor).and_then(|r| run.body(r)).map(|id| id.to_string()).unwrap_or_default();
    format!(
        "{} {} step {}/{}{}",
        run.id,
        run.def,
        run.step + 1,
        def.map_or(0, |d| d.steps.len()),
        step.map(|s| format!(": {}", s.text(&who))).unwrap_or_default()
    )
}

fn playing(world: &World) -> bool {
    world.get_resource::<Sequences>().is_some_and(|s| !s.runs.is_empty())
}

/// `SEQUENCE 3 greeting step 2/7: e7 turns to the visitor` under `observe`, one line for each run.
fn observe_section(world: &World) -> Vec<String> {
    world.get_resource::<Sequences>().map(|s| s.runs.iter().map(|r| format!("SEQUENCE {}", run_line(world, r))).collect()).unwrap_or_default()
}

/// `in sequence 3 greeting (guard), step 2/7` on a held NPC's line.
fn describe_held(world: &World, entity: Entity) -> Option<String> {
    let hold = world.get::<SequenceHold>(entity)?;
    let run = world.get_resource::<Sequences>()?.get(SequenceId(hold.run))?;
    let total = world.get_resource::<ContentDb>().and_then(|db| db.peek::<SequenceDef>(&hold.def)).map_or(0, |d| d.steps.len());
    Some(format!("in sequence {} {} ({}), step {}/{total}", hold.run, hold.def, hold.role, run.step + 1))
}

/// For `npc_state`: the run and step that hold an NPC.
pub(crate) fn state_of(world: &World, entity: Entity) -> Option<Value> {
    let hold = world.get::<SequenceHold>(entity)?;
    let run = world.get_resource::<Sequences>()?.get(SequenceId(hold.run))?;
    let def = world.get_resource::<ContentDb>().and_then(|db| db.peek::<SequenceDef>(&hold.def));
    let step = def.and_then(|d| d.steps.get(run.step as usize));
    let who = step.and_then(Step::actor).and_then(|r| run.body(r)).map(|id| id.to_string()).unwrap_or_default();
    Some(serde_json::json!({
        "run": hold.run,
        "def": hold.def,
        "role": hold.role,
        "step": run.step + 1,
        "of": def.map_or(0, |d| d.steps.len()),
        "doing": step.map(|s| s.text(&who)),
        "age_ticks": run.age,
    }))
}

/// FNV-1a over the state of every run, for the replay check `sequences`. `None` without the resource: a game
/// that never plays a sequence has the replay hashes it had before the module.
pub(super) fn runs_hash(world: &World) -> Option<u64> {
    let sequences = world.get_resource::<Sequences>()?;
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |bytes: &[u8]| {
        for b in bytes {
            h ^= u64::from(*b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    eat(&sequences.next.to_le_bytes());
    for run in &sequences.runs {
        eat(&run.id.to_le_bytes());
        eat(run.def.as_bytes());
        eat(&[0]);
        for (role, id) in &run.roles {
            eat(role.as_bytes());
            eat(&[0]);
            eat(&id.0.to_le_bytes());
        }
        for id in &run.actors {
            eat(&id.0.to_le_bytes());
        }
        eat(&run.step.to_le_bytes());
        eat(&[u8::from(run.begun)]);
        eat(&run.age.to_le_bytes());
        eat(&run.left.to_le_bytes());
        eat(&run.instance.unwrap_or(u32::MAX).to_le_bytes());
        if let Some(p) = run.point {
            for f in [p.x, p.y, p.z] {
                eat(&f.to_bits().to_le_bytes());
            }
        }
        for (role, facing) in &run.faces {
            eat(role.as_bytes());
            match facing {
                super::run::Facing::Point(p) => {
                    eat(&[1]);
                    for f in [p.x, p.y, p.z] {
                        eat(&f.to_bits().to_le_bytes());
                    }
                }
                super::run::Facing::Body(id) => {
                    eat(&[2]);
                    eat(&id.0.to_le_bytes());
                }
            }
        }
        eat(&run.started.to_le_bytes());
        eat(&[0xff]);
    }
    Some(h)
}

/// Snapshot prepare `sequences`: a snapshot that holds no sequences removes the runs this world has (the holds
/// on their bodies are replaced by the load, so a run left would have none).
pub(super) fn drop_absent(world: &mut World, snapshot: &Snapshot) -> Result<Option<String>, String> {
    if snapshot.resources.contains_key(<Sequences as TypePath>::type_path()) || !world.contains_resource::<Sequences>() {
        return Ok(None);
    }
    // A removal does not replicate: a co-op host keeps an empty resource.
    if world.get_resource::<shards_core::NetRole>().is_some_and(|r| *r == shards_core::NetRole::Host) {
        world.insert_resource(Sequences::default());
    } else {
        world.remove_resource::<Sequences>();
    }
    Ok(Some("sequence runs removed (the snapshot holds none)".to_owned()))
}

/// The engine rule `npc.sequence_holds`: every run is one a body can be held by, and the holds say so. Each
/// body that carries a `SequenceHold` names a run that plays, the role it plays there and the definition
/// the run plays; each body of a run that is in the world and acts in it carries the hold; every number is
/// below `next`, the runs ascend, and no step is past the end of the definition. Bodies that are gone are
/// not judged: the run notices them on its next tick and ends.
pub fn sequence_holds(world: &World) -> Option<String> {
    let mut bad = Offenders::default();
    // A problem of a run as a whole, which no body carries: the lowest run's is told.
    let mut whole: Option<String> = None;
    let sequences = world.get_resource::<Sequences>();
    let db = world.get_resource::<ContentDb>();
    if let Some(s) = sequences {
        for pair in s.runs.windows(2) {
            if pair[0].id >= pair[1].id && whole.is_none() {
                whole = Some(format!("Sequences.runs are not ascending: {} then {}", pair[0].id, pair[1].id));
            }
        }
        for run in &s.runs {
            if (run.id == 0 || run.id > s.next) && whole.is_none() {
                whole = Some(format!("sequence {} is outside 1..={}", run.id, s.next));
            }
            let steps = db.and_then(|db| db.peek::<SequenceDef>(&run.def)).map(|d| d.steps.len());
            if let Some(n) = steps
                && run.step as usize > n
                && whole.is_none()
            {
                whole = Some(format!("sequence {} ({}) is at step {}, past the {n} steps of its definition", run.id, run.def, run.step));
            }
            for id in &run.actors {
                let Some(entity) = world.get_resource::<AiIndex>().and_then(|i| i.get(*id)) else { continue };
                if !world.get::<SequenceHold>(entity).is_some_and(|h| h.run == run.id) {
                    bad.note(*id, || format!("acts in sequence {} ({}) and has no SequenceHold for it", run.id, run.def));
                }
            }
        }
    }
    with_query::<(&AiId, &SequenceHold), (), _>(world, |query| {
        for (id, hold) in query.iter(world) {
            let run = sequences.and_then(|s| s.get(SequenceId(hold.run)));
            match run {
                None => bad.note(*id, || format!("SequenceHold names sequence {}, which is not playing", hold.run)),
                Some(run) if run.def != hold.def => {
                    bad.note(*id, || format!("SequenceHold says `{}`, sequence {} plays `{}`", hold.def, run.id, run.def));
                }
                Some(run) if run.body(&hold.role) != Some(*id) => {
                    bad.note(*id, || format!("SequenceHold names role `{}` of sequence {}, which is another body's", hold.role, run.id));
                }
                Some(run) if !run.actors.contains(id) => {
                    bad.note(*id, || format!("SequenceHold in sequence {}, where it does not act", run.id));
                }
                Some(_) => {}
            }
        }
    })?;
    whole.or_else(|| bad.report())
}
