//! What agents see of movers and do with them: the describer, the journal lines, the dev tool
//! `mover_go`, the condition `mover_arrived` and the engine rule `motion.mover_sane`.

use ai_core::prelude::*;
use ai_core::spatial::compass;
use ai_core::{label, with_query, Offenders};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_content::db::closest;
use shards_content::ContentDb;

use super::def::{MoverDef, MoverLoop};
use super::{mover_go, mover_state, Mover, MoverArrived, MoverBlocked, MoverStarted, MoverState};

pub(crate) fn plugin(app: &mut App) {
    app.register_ai_world_describer("mover", describe)
        .add_ai_tool("mover_go", Scope::Dev, mover_go_tool)
        .add_ai_condition("mover_arrived", any_arrived)
        .journal_event::<MoverStarted>(|e, w| routine(w, e.mover).then(|| format!("{} leaves key {} for key {}", name(w, e.mover), e.from, e.to)))
        .journal_event::<MoverArrived>(|e, w| routine(w, e.mover).then(|| format!("{} at key {}", name(w, e.mover), e.key)))
        .journal_event::<MoverBlocked>(|e, w| {
            Some(if e.damage > 0.0 {
                format!("{} crushes {} ({:.0})", name(w, e.mover), label(w, e.body), e.damage)
            } else {
                format!("{} held by {}", name(w, e.mover), label(w, e.body))
            })
        });
}

/// `e14 Lift`: the id and the name (the level node's, the prefab's), else the definition.
fn name(world: &World, mover: Entity) -> String {
    let id = label(world, mover);
    match (world.get::<Name>(mover), world.get::<Mover>(mover)) {
        (Some(name), _) => format!("{id} {name}"),
        (None, Some(m)) => format!("{id} {}", m.def),
        (None, None) => id,
    }
}

/// Whether a mover's starts and stops are worth a journal line: a mover that goes where it is
/// sent. A looping one (a turning platform, a door that opens and shuts by itself) would fill the
/// journal with its routine; its blocks are still told.
fn routine(world: &World, mover: Entity) -> bool {
    let looping = world
        .get::<Mover>(mover)
        .and_then(|m| world.get_resource::<ContentDb>()?.get::<MoverDef>(&m.def))
        .map(|def| def.looping);
    looping.is_none_or(|l| l == MoverLoop::Off)
}

/// `up`, `down`, or the compass direction on the ground plane.
fn direction(v: Vec3) -> &'static str {
    let flat = Vec2::new(v.x, v.z).length();
    if v.y.abs() >= flat {
        if v.y > 0.0 { "up" } else { "down" }
    } else {
        compass(Vec3::new(v.x, 0.0, v.z))
    }
}

/// `at key 1`, `key 0→1, moving up 1.2m/s`, `key 0→1, turning 45°/s, to key 2`, `…, held 0.5s`.
fn describe(world: &World, entity: Entity) -> Option<String> {
    let info = mover_state(world, entity)?;
    if info.keys == 0 {
        return Some(format!("moves by `{}`, which content does not have", info.def));
    }
    let mut out = match info.next {
        None => format!("at key {}", info.key),
        Some(next) => format!("key {}→{next}", info.key),
    };
    if info.next.is_some() {
        let speed = info.velocity.length();
        if speed > 1e-3 {
            out.push_str(&format!(", moving {} {speed:.1}m/s", direction(info.velocity)));
        }
        if info.turn_rate > 1e-3 {
            out.push_str(&format!(", turning {:.0}°/s", info.turn_rate));
        }
    } else if info.wait > 0.0 && (info.goal.is_some() || !routine(world, entity)) {
        out.push_str(&format!(", leaves in {:.1}s", info.wait));
    }
    if let Some(goal) = info.goal
        && Some(goal) != info.next
    {
        out.push_str(&format!(", to key {goal}"));
    }
    if info.blocked > 0.0 {
        out.push_str(&format!(", held {:.1}s", info.blocked));
    }
    Some(out)
}

/// Send a mover (a door, a lift, a platform: `Mover` in `observe`) to one of its keys (0 is
/// where it started). It goes at its own pace through the keys between and rests there; one on its
/// way turns round when the key is behind it. A body in its way it cannot push holds it (its
/// `blocked` rule). Name it by `id`, or by `name` when only one mover has that name. Watch it
/// with `step --until named:mover_arrived`.
#[derive(Deserialize, JsonSchema)]
pub struct MoverGoArgs {
    /// The mover, e.g. "e14".
    pub id: Option<Id>,
    /// Or its name, e.g. "lift" (any case), when only one mover has it.
    pub name: Option<String>,
    /// The key to go to.
    pub key: u32,
}

/// The mover an agent names: by id, or by a name only one mover has.
fn find(world: &mut World, id: Option<Id>, name: Option<&str>) -> Result<Entity, String> {
    if let Some(id) = id {
        let entity = id.entity(world)?;
        if world.get::<Mover>(entity).is_none() {
            return Err(format!("{id} is no mover; `observe` lists movers as `Mover`"));
        }
        return Ok(entity);
    }
    let Some(wanted) = name.map(str::trim).filter(|n| !n.is_empty()) else {
        return Err("name the mover: `id` (e.g. e14) or `name`".into());
    };
    let movers = super::movers_in_order(world);
    let named: Vec<(AiId, Entity, String)> = movers
        .iter()
        .map(|&(id, e)| {
            let name = world.get::<Name>(e).map(|n| n.as_str().to_owned()).or_else(|| world.get::<Mover>(e).map(|m| m.def.clone()));
            (id, e, name.unwrap_or_default())
        })
        .collect();
    let hits: Vec<&(AiId, Entity, String)> = named.iter().filter(|(_, _, n)| n.eq_ignore_ascii_case(wanted)).collect();
    match hits.as_slice() {
        [(_, entity, _)] => Ok(*entity),
        [] => {
            if named.is_empty() {
                return Err("there are no movers".into());
            }
            let lower: Vec<String> = named.iter().map(|(_, _, n)| n.to_lowercase()).collect();
            let hint = closest(&wanted.to_lowercase(), lower.iter().map(String::as_str))
                .and_then(|c| named.iter().zip(&lower).find(|(_, l)| l.as_str() == c))
                .map(|((_, _, n), _)| format!(" (did you mean `{n}`?)"))
                .unwrap_or_default();
            let all: Vec<String> = named.iter().map(|(id, _, n)| format!("{id} {n}")).collect();
            Err(format!("no mover named `{wanted}`{hint}; movers: {}", all.join(", ")))
        }
        many => {
            let ids: Vec<String> = many.iter().map(|(id, ..)| id.to_string()).collect();
            Err(format!("{} movers are named `{wanted}`: {}; name one by `id`", many.len(), ids.join(", ")))
        }
    }
}

fn mover_go_tool(In(call): In<Call<MoverGoArgs>>, world: &mut World) -> ToolResult {
    let MoverGoArgs { id, name: wanted, key } = call.args;
    let entity = find(world, id, wanted.as_deref())?;
    let info = mover_go(world, entity, key)?;
    let who = name(world, entity);
    Ok(match info.goal {
        Some(goal) => format!("ok: {who} goes to key {goal} ({:.1}s)", info.eta),
        None if info.next.is_some() => format!("ok: {who} goes on to key {}", info.next.unwrap_or(key)),
        None => format!("ok: {who} is at key {key} already"),
    })
}

/// The condition `mover_arrived`: a mover came to rest at a key in the last tick.
fn any_arrived(world: &World) -> bool {
    world.try_query::<&MoverState>().is_some_and(|mut q| q.iter(world).any(|s| s.arrived))
}

/// The engine rule `motion.mover_sane`: every mover's state is one its definition allows. `t` is
/// in `0..1` (and 0 at rest), the two keys are neighbours and both are keys of the definition (as
/// is a goal), the wait and the time held are finite and not below 0, and the origin is finite. The
/// movers' step keeps it; a violation names a writer that set a state by hand (or a hot reload
/// that took keys away under a mover: put them back).
pub fn mover_sane(world: &World) -> Option<String> {
    let mut bad = Offenders::default();
    let db = world.get_resource::<ContentDb>();
    with_query::<(&AiId, &Mover, &MoverState), (), _>(world, |query| {
        for (id, mover, state) in query.iter(world) {
            let keys = db.and_then(|db| db.peek::<MoverDef>(&mover.def)).map(MoverDef::len);
            if let Some(what) = problem(state, keys) {
                bad.note(*id, || what);
            }
        }
    })?;
    bad.report()
}

fn problem(s: &MoverState, keys: Option<u32>) -> Option<String> {
    if !(s.t.is_finite() && (0.0..1.0).contains(&s.t)) {
        return Some(format!("MoverState.t = {} outside 0..1", s.t));
    }
    if s.from == s.to && s.t != 0.0 {
        return Some(format!("MoverState rests at key {} with t = {} (0 at rest)", s.from, s.t));
    }
    if s.from.abs_diff(s.to) > 1 {
        return Some(format!("MoverState goes from key {} to key {}, which are not neighbours", s.from, s.to));
    }
    if !(s.wait.is_finite() && s.wait >= 0.0) {
        return Some(format!("MoverState.wait = {}", s.wait));
    }
    if !(s.blocked.is_finite() && s.blocked >= 0.0) {
        return Some(format!("MoverState.blocked = {}", s.blocked));
    }
    if !(s.origin.is_finite() && s.origin_rotation.is_finite()) {
        return Some(format!("MoverState.origin = {} (rotation {}) is not finite", s.origin, s.origin_rotation));
    }
    let n = keys?;
    let past = [Some(s.from), Some(s.to), s.goal].into_iter().flatten().find(|k| *k >= n)?;
    Some(format!("MoverState names key {past}, past its definition's last ({})", n.saturating_sub(1)))
}
