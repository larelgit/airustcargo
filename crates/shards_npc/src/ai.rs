//! What the AI layer sees of NPCs: a fragment of their line (`chase e5 · token · leash
//! 12/20m`), journal lines for state changes and tokens, and the tools `npc_state`,
//! `nav_path` (queries) and `npc_set_state` (dev).

use ai_core::prelude::*;
use ai_core::spatial::{compass, fmt_pos, relative};
use ai_core::AiIndex;
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use shards_core::flat;
use shards_motion::perception::{Leg, PathPlan, StepKind};
use shards_motion::{physics::sync_physics, CharacterController, CharacterParams, LayerMask, PhysicsWorld, QueryFilter, TargetArg};

use crate::brain::{force_state, Home, Npc, NpcBrain, NpcDrive, NpcStateChanged};
use crate::director::{AttackToken, TokenGranted, TokenReturned};
use crate::nav::{local_grid_path, navmesh_route, NavAgent, NavPaths, NAVMESH};
use crate::perception::Perceived;
use crate::planner::NpcPlan;

pub(crate) fn register(app: &mut App) {
    app.register_ai_describer("npc", describe_npc)
        .journal_event::<NpcStateChanged>(|e, w| {
            let target = e.target.map(|t| format!(" {t}")).unwrap_or_default();
            let forced = if e.forced { " (npc_set_state)" } else { "" };
            Some(format!("{} {} → {}{target}{forced}", label(w, e.entity), e.from, e.to))
        })
        .journal_event::<TokenGranted>(|e, w| {
            Some(format!("{} takes an attack token on {} ({}/{})", label(w, e.entity), e.target, e.held, e.limit))
        })
        .journal_event::<TokenReturned>(|e, w| {
            let who = label(w, e.entity);
            Some(match &e.attack {
                Some(attack) => format!("{who} returns its attack token after {attack} ({}/{})", e.held, e.limit),
                None => format!("{who} returns its attack token unused: {} ({}/{})", e.reason, e.held, e.limit),
            })
        })
        .add_ai_query("npc_state", Scope::Player, npc_state)
        .add_ai_query("nav_path", Scope::Player, nav_path)
        .add_ai_tool("npc_set_state", Scope::Dev, npc_set_state)
        // `step --until named:npc_attacking`: until an NPC's attack (by its token) is under way.
        .add_ai_condition("npc_attacking", |w| {
            w.try_query::<&AttackToken>().is_some_and(|mut q| q.iter(w).any(|t| t.held.as_ref().is_some_and(|g| g.started)))
        })
        // `step --until named:npcs_calm`: until no active NPC has a target.
        .add_ai_condition("npcs_calm", |w| {
            w.try_query::<&NpcBrain>().is_none_or(|mut q| q.iter(w).all(|b| !b.active || b.target.is_none()))
        })
        // `step --until named:npc_beyond_leash`: an active NPC stands farther from its home than
        // its leash, give or take a meter (its goals stay half a meter inside; the rest is a
        // body pushed by others). A step that should keep NPCs in their zones must not stop on it.
        .add_ai_condition("npc_beyond_leash", |w| {
            w.try_query::<(&NpcBrain, &Home, &Transform)>().is_some_and(|mut q| {
                q.iter(w).any(|(b, h, tf)| b.active && h.pos.is_some_and(|at| flat(tf.translation - at).length() > h.leash + LEASH_SLACK))
            })
        })
        // `step --until named:npc_detour`: an active NPC walks a planned route of several legs:
        // its goal is not in plain sight (a wall, pillars), so it goes around.
        .add_ai_condition("npc_detour", |w| {
            w.try_query::<(&NpcBrain, &NavAgent)>().is_some_and(|mut q| {
                q.iter(w).any(|(b, a)| b.active && a.goal.is_some() && !a.direct && !a.arrived && a.path.len() >= 2)
            })
        });
}

/// How far past its leash an NPC may stand before `npc_beyond_leash` holds (m).
pub const LEASH_SLACK: f32 = 1.0;

/// `chase e5 · token · leash 12/20m`, `attack e5 · circle`, `idle (dormant)`.
pub fn describe_npc(e: EntityRef) -> Option<String> {
    e.get::<Npc>()?;
    let brain = e.get::<NpcBrain>()?;
    if brain.state.is_empty() {
        return None;
    }
    let mut line = brain.state.clone();
    if !brain.active {
        line.push_str(" (dormant)");
        return Some(line);
    }
    if let Some(target) = brain.target {
        line.push_str(&format!(" {target}"));
    }
    if e.get::<AttackToken>().is_some_and(|t| t.held.is_some()) {
        line.push_str(" · token");
    } else if let Some(role) = &brain.role {
        line.push_str(&format!(" · {role}"));
    }
    if let (Some(home), Some(tf)) = (e.get::<Home>(), e.get::<Transform>())
        && let Some(at) = home.pos
    {
        line.push_str(&format!(" · leash {:.0}/{:.0}m", flat(tf.translation - at).length(), home.leash));
    }
    Some(line)
}

/// Everything about an NPC's mind: its state and timers, its target and what it knows of
/// it, its attack token, its plan (pending attack, cooldowns, history, rest), its route and
/// its leash. Read-only.
#[derive(Deserialize, JsonSchema)]
pub struct NpcStateArgs {
    /// The NPC: 42 or "e42".
    pub id: Id,
}

fn npc_state(In(call): In<Call<NpcStateArgs>>, world: &World) -> ToolResult<Value> {
    let entity = call.args.id.entity(world)?;
    let e = world.entity(entity);
    let Some(npc) = e.get::<Npc>() else {
        return Err(format!("{} is not an NPC (it has no Npc component)", call.args.id));
    };
    let brain = e.get::<NpcBrain>().cloned().unwrap_or_default();
    let here = e.get::<Transform>().map_or(Vec3::ZERO, |t| t.translation);
    let round = |v: f32| (v * 100.0).round() / 100.0;
    let perceived = e.get::<Perceived>().cloned().unwrap_or_default();
    let known: Vec<Value> = perceived
        .known
        .iter()
        .map(|k| {
            json!({
                "id": k.id.to_string(),
                "at": fmt_pos(k.pos),
                "where": relative(here, k.pos),
                "seen": k.seen,
                "heard": k.heard,
                "since": round(k.age),
            })
        })
        .collect();
    let token = e.get::<AttackToken>().and_then(|t| t.held.as_ref()).map(|g| {
        json!({
            "target": g.target.to_string(),
            "attack": g.attack,
            "held_for": round(g.age),
            "pressed": g.pressed.is_some(),
            "started": g.started,
        })
    });
    let asking = e.get::<AttackToken>().and_then(|t| t.asking_since);
    let plan = e.get::<NpcPlan>().cloned().unwrap_or_default();
    let cooldowns: serde_json::Map<String, Value> =
        plan.cooldowns.iter().map(|c| (c.name.clone(), json!(round(c.left)))).collect();
    let nav = e.get::<NavAgent>().map(|a| {
        json!({
            "goal": a.goal.map(fmt_pos),
            "waypoints": a.path.len(),
            "next": a.path.get(a.index as usize).map(|p| fmt_pos(*p)),
            "direct": a.direct,
            "arrived": a.arrived,
        })
    });
    let home = e.get::<Home>().map(|h| {
        let at = h.pos.unwrap_or(here);
        json!({"at": fmt_pos(at), "leash": h.leash, "distance": round(flat(here - at).length())})
    });
    let drive = e.get::<NpcDrive>().cloned().unwrap_or_default();
    Ok(json!({
        "id": call.args.id.to_string(),
        "line": describe_npc(e),
        "behavior": npc.behavior,
        "state": brain.state,
        "in_state": round(brain.since),
        "timer": round(brain.timer),
        "active": brain.active,
        "target": brain.target.map(|t| t.to_string()),
        "token": token,
        "asking_since_tick": asking,
        "role": brain.role,
        "wants": {
            "token": drive.request_token,
            "strike": drive.strike,
            "goal": drive.goal.map(fmt_pos),
            "face": drive.face.map(fmt_pos),
        },
        "plan": {
            "id": npc.plan,
            "rest": round(plan.rest),
            "cooldowns": cooldowns,
            "history": plan.history.iter().map(|p| p.name.clone()).collect::<Vec<_>>(),
            "attacks": plan.attacks,
        },
        "perceived": known,
        "heard": perceived.noise.map(|n| json!({"at": fmt_pos(n.pos), "since": round(n.age)})),
        "nav": nav,
        "home": home,
    }))
}

/// The route an NPC would take, as `path_to` reports one: `path 23.5m, 4 turns: 6.0m NE →
/// 4.5m N (mantle 1.1m) → ...`, with the waypoints. Inside a loaded region it comes from the
/// navmesh (`provider: navmesh`, and the tiles it crosses), elsewhere from the local grid (or
/// the game's own provider). From a point or an entity (default: you) to another, for a body
/// of `radius` (default: the walker's). Read-only.
#[derive(Deserialize, JsonSchema)]
pub struct NavPathArgs {
    /// Where the route starts: an entity id or a ground point {x, z}. Default: your body.
    pub from: Option<TargetArg>,
    /// Where it goes: an entity id or a ground point {x, z}.
    pub to: TargetArg,
    /// Body radius in meters. Default: the walker's (0.4 for a point).
    pub radius: Option<f32>,
}

fn nav_path(In(call): In<Call<NavPathArgs>>, world: &mut World) -> ToolResult<Value> {
    // Tools run between ticks: colliders a call just moved must be where the route sees them.
    let _ = world.run_system_cached(sync_physics);
    crate::nav::refresh(world);
    let index = world.resource::<AiIndex>();
    let place = |arg: &TargetArg| -> Result<(Vec3, Option<Entity>), String> {
        match *arg {
            TargetArg::Entity(id) => {
                let entity = index.resolve(id.0)?;
                let at = world.get::<Transform>(entity).ok_or(format!("{id} has no position"))?.translation;
                Ok((at, Some(entity)))
            }
            TargetArg::Point { x, z } if x.is_finite() && z.is_finite() => Ok((Vec3::new(x, 0.0, z), None)),
            TargetArg::Point { .. } => Err("x and z must be finite numbers".into()),
        }
    };
    let (from, walker) = match &call.args.from {
        // A point: standing on top of whatever is there (a plateau, a bridge).
        Some(arg @ TargetArg::Point { .. }) => {
            let (mut at, _) = place(arg)?;
            let filter = QueryFilter::new(LayerMask::WORLD);
            if let Some(hit) = world.resource::<PhysicsWorld>().raycast(at + Vec3::Y * 500.0, Dir3::NEG_Y, 1000.0, &filter) {
                at.y = hit.point.y;
            }
            (at, None)
        }
        Some(arg) => place(arg)?,
        None => {
            let me = call.caller.require_subject().map_err(|e| format!("{e}; give `from`"))?;
            (world.get::<Transform>(me).ok_or("your body has no position")?.translation, Some(me))
        }
    };
    let (mut to, target) = place(&call.args.to)?;
    if target.is_none() {
        to.y = from.y;
    }
    let mut params = walker
        .and_then(|w| {
            let own = world.get::<CharacterParams>(w);
            world.get::<CharacterController>(w).map(|c| c.resolve(own, world.get_resource()).clone()).or(own.cloned())
        })
        .unwrap_or_default();
    if let Some(radius) = call.args.radius {
        params.radius = radius;
    }
    let radius = params.radius;
    if !(radius > 0.0 && radius.is_finite()) {
        return Err(format!("radius must be > 0 (is {radius})"));
    }
    let round = |v: f32| (v * 100.0).round() / 100.0;
    let paths = world.get_resource::<NavPaths>().copied().unwrap_or_default();
    let mut provider = paths.name;
    let mut find = paths.find;
    if provider == NAVMESH {
        match navmesh_route(world, from, to, &params, target.is_some()) {
            Some(Ok(path)) => {
                let mut out = path.to_plan().to_json();
                out["provider"] = json!(NAVMESH);
                out["from"] = json!(fmt_pos(from));
                out["to"] = json!(fmt_pos(to));
                out["tiles"] = json!(path.tiles.iter().map(|[x, y]| format!("({x}, {y})")).collect::<Vec<_>>());
                out["distance"] = json!(round(path.length));
                return Ok(out);
            }
            Some(Err(why)) => {
                return Err(format!("no route from {} to {} for a body of {radius:.2}m (navmesh): {why}", fmt_pos(from), fmt_pos(to)));
            }
            None => {
                provider = "local grid";
                find = local_grid_path;
            }
        }
    }
    let Some(path) = find(world, from, to, radius) else {
        return Err(format!(
            "no route from {} to {} for a body of {radius:.2}m ({provider} paths); something blocks it all round, \
             or the goal is not on walkable ground",
            fmt_pos(from),
            fmt_pos(to)
        ));
    };
    let mut out = walk_plan(from, &path).to_json();
    out["provider"] = json!(provider);
    out["from"] = json!(fmt_pos(from));
    out["to"] = json!(fmt_pos(to));
    Ok(out)
}

/// Waypoints walked in straight lines, as the motion planner reports a route.
fn walk_plan(from: Vec3, points: &[Vec3]) -> PathPlan {
    let mut legs: Vec<Leg> = Vec::new();
    let (mut at, mut length) = (from, 0.0);
    for p in points {
        let delta = flat(*p - at);
        let len = delta.length();
        if len < 1e-4 {
            continue;
        }
        length += len;
        legs.push(Leg { dir: compass(delta), length: len, kind: StepKind::Walk, rise: 0.0, end: *p });
        at = *p;
    }
    PathPlan { start: from, legs, length, explored: 0 }
}

/// Dev: put an NPC in a state of its behaviour at once (its timers start over; the next
/// tick its transitions apply as usual). Journaled with `(npc_set_state)`.
#[derive(Deserialize, JsonSchema)]
pub struct NpcSetStateArgs {
    /// The NPC: 42 or "e42".
    pub id: Id,
    /// A state of its behaviour (`npc_state` shows the current one).
    pub state: String,
}

fn npc_set_state(In(call): In<Call<NpcSetStateArgs>>, world: &mut World) -> ToolResult {
    let entity = call.args.id.entity(world)?;
    let from = force_state(world, entity, &call.args.state).map_err(|e| format!("{}: {e}", call.args.id))?;
    let from = if from.is_empty() { "(not started)".to_owned() } else { from };
    Ok(format!("ok: {} {from} → {}", call.args.id, call.args.state))
}
