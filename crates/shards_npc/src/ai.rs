//! What the AI layer sees of NPCs: a fragment of their line (`chase e5 · token · leash
//! 12/20m`), journal lines for state changes, tokens and plan memory, and the tools `npc_state`,
//! `npc_schedule`, `nav_path` (queries), `plan_audit` (a dev query), `npc_set_state`, `npc_plan_reset`
//! and `npc_plan_cooldown` (dev).

use ai_core::prelude::*;
use ai_core::spatial::{compass, fmt_pos, relative};
use ai_core::AiIndex;
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use shards_content::db::closest;
use shards_content::{ContentDb, PrefabDef};
use shards_core::flat;
use shards_motion::perception::{Leg, PathPlan, StepKind};
use shards_rules::RulesAppExt;
use shards_motion::{physics::sync_physics, CharacterController, CharacterParams, LayerMask, PhysicsWorld, QueryFilter, TargetArg};

use crate::brain::{force_state, Home, Npc, NpcBrain, NpcDrive, NpcStateChanged, NpcThink};
use crate::chain::{ChainPressed, ChainRefused, NpcChain};
use crate::audit::{field, follow_pairs, plan_hits, prefab_component, unreachable_options, FollowPair};
use crate::def::AttackPlanDef;
use crate::director::{AttackToken, TokenGranted, TokenReturned};
use crate::memory::{forgotten, reset_plan, set_plan_cooldown, CooldownSet, PlanCooldownSet, PlanReset};
use crate::nav::{local_grid_path, local_grid_route, NavAgent, NavPaths, NAVMESH};
use crate::perception::Perceived;
use crate::planner::{repeats_of, NpcOptions, NpcPlan};
use crate::schedule::{schedule_report, ScheduleParams, WhatIf};
use crate::strike::{wait_of, NpcStrike};
use crate::hide::{hide_status, threat_eye};
use crate::brain::NpcCtx;
use crate::ranged::{band_text, NpcFireLine};
use crate::threat::NpcThreats;
use crate::turret::{set_turret_active, turret_line, Turret, TurretSwitched};

pub(crate) fn register(app: &mut App) {
    app.add_rules_guard("npc_engaged", rules_guard)
        .register_ai_describer("npc", describe_npc)
        // F11: `hiding`, `cowering`, `keeping range 8–14m · no line of fire: e9 in the way`, `turret tracking e5 (yaw 12°/60°)`.
        .register_ai_world_describer("npc_ranged", describe_ranged)
        .journal_event::<TurretSwitched>(|e, w| Some(format!("{} turret {}", label(w, e.entity), if e.active { "on" } else { "off" })))
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
        .journal_event::<ChainPressed>(|e, w| Some(format!("{} chains {} after {}", label(w, e.entity), e.input, e.after)))
        .journal_event::<ChainRefused>(|e, w| {
            Some(format!(
                "{} chain {} refused after {}: {} began a strike less than {} ticks ago ({} to go)",
                label(w, e.entity),
                e.input,
                e.after,
                e.by,
                e.spacing,
                e.ticks_left
            ))
        })
        .journal_event::<PlanReset>(|e, w| Some(format!("{} plan reset ({}): {}", label(w, e.entity), e.why, forgotten(e.forgot, e.cleared))))
        .journal_event::<PlanCooldownSet>(|e, w| {
            Some(format!("{} {} cooldown {:.2} s (was {:.2} s)", label(w, e.entity), e.option, e.left, e.was))
        })
        .add_ai_query("npc_state", Scope::Player, npc_state)
        .add_ai_query("npc_schedule", Scope::Player, npc_schedule)
        .add_ai_query("nav_path", Scope::Player, nav_path)
        .add_ai_query("plan_audit", Scope::Dev, plan_audit)
        .add_ai_tool("npc_set_state", Scope::Dev, npc_set_state)
        .add_ai_tool("npc_plan_reset", Scope::Dev, npc_plan_reset)
        .add_ai_tool("npc_plan_cooldown", Scope::Dev, npc_plan_cooldown)
        .add_ai_tool("turret_active", Scope::Dev, turret_active)
        // `step --until named:npc_hidden`: a hiding NPC (F11) stands at its hiding spot.
        .add_ai_condition("npc_hidden", npc_hidden)
        // `step --until named:npc_line_blocked`: a ranged NPC (F11) has an ally or a wall in its line of fire.
        .add_ai_condition("npc_line_blocked", npc_line_blocked)
        // `step --until named:npc_attacking`: until an NPC's attack (by its token) is under way.
        .add_ai_condition("npc_attacking", |w| {
            w.try_query::<&AttackToken>().is_some_and(|mut q| q.iter(w).any(|t| t.held.as_ref().is_some_and(|g| g.started)))
        })
        // `step --until named:npcs_calm`: until no active NPC has a target.
        .add_ai_condition("npcs_calm", |w| {
            w.try_query::<&NpcBrain>().is_none_or(|mut q| q.iter(w).all(|b| !b.active || b.target.is_none()))
        })
        // `step --until named:npc_engaged`: an active NPC with a target is in a fighting state
        // (`chase`, `attack`, `wait_token`): what adaptive music calls combat. A calm NPC that
        // sees someone is not. Reads only what a co-op client is sent too.
        .add_ai_condition("npc_engaged", npc_engaged)
        // `step --until named:npc_near`: a player's body is within an active NPC's sight range
        // and half again: it may be noticed any moment (what adaptive music calls tension).
        .add_ai_condition("npc_near", npc_near)
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
        })
        // `step --until named:npc_avoiding`: an active NPC steers off its route around another
        // body (an NPC, a player) it would run into.
        .add_ai_condition("npc_avoiding", |w| {
            w.try_query::<(&NpcBrain, &NavAgent)>().is_some_and(|mut q| q.iter(w).any(|(b, a)| b.active && a.avoiding.is_some()))
        });
}

/// How far past its leash an NPC may stand before `npc_beyond_leash` holds (m).
pub const LEASH_SLACK: f32 = 1.0;

/// The engine's states an NPC fights in, by kind: `npc_engaged` holds while one is in them.
pub const FIGHTING_STATES: [&str; 3] = ["chase", "attack", "wait_token"];

/// The first active NPC with a target that is in a state of a [`FIGHTING_STATES`] kind, in `AiId` order: its
/// id, its behaviour, its state and its target. What the condition `npc_engaged` and the rules guard of the
/// same name ask.
pub fn engaged_npc(w: &World) -> Option<(AiId, String, String, Option<AiId>)> {
    let mut npcs = w.try_query::<(&AiId, &Npc, &NpcBrain)>()?;
    let db = w.get_resource::<shards_content::ContentDb>();
    npcs.iter(w)
        .filter(|(_, _, b)| b.active && b.target.is_some())
        .filter(|(_, npc, brain)| {
            let behavior = db.and_then(|db| db.get::<crate::def::BehaviorDef>(&npc.behavior));
            let kind = behavior.map_or(brain.state.as_str(), |b| b.kind_of(&brain.state));
            FIGHTING_STATES.contains(&kind)
        })
        .min_by_key(|(id, _, _)| **id)
        .map(|(id, npc, brain)| (*id, npc.behavior.clone(), brain.state.clone(), brain.target))
}

/// `npc_engaged`: an active NPC with a target is in a state of a [`FIGHTING_STATES`] kind.
fn npc_engaged(w: &World) -> bool {
    engaged_npc(w).is_some()
}

/// The rules guard `npc_engaged`: the rules change only out of combat, and an NPC that fights (a partner's
/// fight too) is combat. The reason names the first such NPC: ``e6 `skeleton` is fighting (`chase`, target e5)``.
fn rules_guard(w: &World) -> Option<String> {
    let (id, behavior, state, target) = engaged_npc(w)?;
    let target = target.map(|t| format!(", target {t}")).unwrap_or_default();
    Some(format!("{id} `{behavior}` is fighting (`{state}`{target})"))
}

/// How far past its sight range an NPC counts as near for `npc_near` (× the range).
pub const NEAR_SIGHT: f32 = 1.5;

/// `npc_near`: some standing player's (not dead, not downed) body is within [`NEAR_SIGHT`] × the sight range of an
/// active NPC (its perception's, the engine's default without one).
fn npc_near(w: &World) -> bool {
    let Some(mut players) = w.try_query_filtered::<&Transform, (With<shards_core::PlayerId>, shards_action::Standing)>() else {
        return false;
    };
    let players: Vec<Vec3> = players.iter(w).map(|t| t.translation).collect();
    let Some(mut npcs) = w.try_query::<(&Npc, &NpcBrain, &Transform)>() else { return false };
    let db = w.get_resource::<shards_content::ContentDb>();
    npcs.iter(w).filter(|(_, b, _)| b.active).any(|(npc, _, at)| {
        let sight = db.and_then(|db| db.get::<crate::def::PerceptionDef>(&npc.perception)).map_or_else(|| crate::def::PerceptionDef::default().sight_range, |d| d.sight_range);
        let reach = sight * NEAR_SIGHT;
        players.iter().any(|p| flat(*p - at.translation).length() <= reach)
    })
}

/// `chase e5 · token · leash 12/20m`, `attack e5 · circle`, `idle (dormant)`.
pub fn describe_npc(e: EntityRef) -> Option<String> {
    e.get::<Npc>()?;
    npc_line(e.get::<NpcBrain>()?, e.get::<AttackToken>(), e.get::<Home>(), e.get::<Transform>())
}

/// [`describe_npc`] from the parts it reads.
pub(crate) fn npc_line(brain: &NpcBrain, token: Option<&AttackToken>, home: Option<&Home>, tf: Option<&Transform>) -> Option<String> {
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
    // A role kept while a grant is held belongs to a plan with its own waiting role (tokenless or not): say the role.
    if token.is_some_and(|t| t.held.is_some()) && brain.role.is_none() {
        line.push_str(" · token");
    } else if let Some(role) = &brain.role {
        line.push_str(&format!(" · {role}"));
    }
    if let (Some(home), Some(tf)) = (home, tf)
        && let Some(at) = home.pos
    {
        line.push_str(&format!(" · leash {:.0}/{:.0}m", flat(tf.translation - at).length(), home.leash));
    }
    Some(line)
}

/// What F11 puts on an NPC's mind, for its describer and `npc_overlay`: `hiding` or `cowering` (a
/// `hide` or `cower` state), `keeping range 8–14m` and `no line of fire: e9 in the way` (a plan that
/// keeps a range, while it fights), `turret tracking e5 (yaw 12°/60°)`. Content is read with `peek`.
pub fn ranged_parts(world: &World, entity: Entity) -> Vec<String> {
    let mut parts = Vec::new();
    let (Some(npc), Some(brain)) = (world.get::<Npc>(entity), world.get::<NpcBrain>(entity)) else { return parts };
    if !brain.active || brain.state.is_empty() {
        return parts;
    }
    if let Some((spot, cowering)) = hide_status(world, entity) {
        parts.push(if cowering { "cowering" } else if spot.is_some() { "hiding" } else { "looking for cover" }.to_owned());
    }
    if let Some(line) = turret_line(world, entity) {
        parts.push(line);
    }
    let fighting = world.get::<NpcDrive>(entity).is_some_and(|d| d.strike || d.waiting);
    if fighting
        && let Some(db) = world.get_resource::<shards_content::ContentDb>()
        && let Some(band) = band_text(db, npc)
    {
        parts.push(band);
        let target = world.get::<AttackToken>(entity).and_then(|t| t.held.as_ref()).map(|g| g.target).or(brain.target);
        if let Some(block) = target.and_then(|t| world.get::<NpcFireLine>(entity).and_then(|l| l.blocking(t))) {
            parts.push(format!("no line of fire: {}", block.text()));
        }
    }
    parts
}

/// The `npc_ranged` describer: [`ranged_parts`], joined.
fn describe_ranged(world: &World, entity: Entity) -> Option<String> {
    let parts = ranged_parts(world, entity);
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// Dev: switches a turret on or off ([`set_turret_active`]), or every turret; journaled (`e9 turret on`).
#[derive(Deserialize, JsonSchema)]
pub struct TurretActiveArgs {
    /// The turret: 42 or "e42". Default: every turret.
    pub id: Option<Id>,
    /// On (it tracks its target and fires) or off (it turns back to its rest and holds fire).
    pub on: bool,
}

fn turret_active(In(call): In<Call<TurretActiveArgs>>, world: &mut World) -> ToolResult {
    if let Some(id) = &call.args.id {
        let entity = id.entity(world)?;
        return set_turret_active(world, entity, call.args.on).map_err(|e| format!("{id}: {e}; `npc_state` says which NPCs are turrets"));
    }
    let mut turrets: Vec<(AiId, Entity)> = match world.try_query::<(Entity, &AiId, &Turret)>() {
        Some(mut q) => q.iter(world).map(|(e, id, _)| (*id, e)).collect(),
        None => Vec::new(),
    };
    turrets.sort();
    if turrets.is_empty() {
        return Err("there is no turret (no entity has a Turret component)".to_owned());
    }
    let lines: Vec<String> = turrets.into_iter().filter_map(|(_, e)| set_turret_active(world, e, call.args.on).ok()).collect();
    Ok(lines.join("\n"))
}

/// `npc_hidden`: an active NPC in a `hide` state stands at the hiding spot it found (F11).
fn npc_hidden(w: &World) -> bool {
    let Some(mut npcs) = w.try_query::<(Entity, &NpcBrain, &Transform)>() else { return false };
    npcs.iter(w).filter(|(_, b, _)| b.active).any(|(e, _, tf)| {
        matches!(hide_status(w, e), Some((Some(spot), false)) if flat(tf.translation - spot).length() <= HIDDEN_WITHIN)
    })
}

/// How near its spot a hiding NPC stands for `npc_hidden` (m).
pub const HIDDEN_WITHIN: f32 = 0.6;

/// `npc_line_blocked`: an active NPC whose plan keeps a range has an ally or a wall in its line of fire to
/// its target (F11).
fn npc_line_blocked(w: &World) -> bool {
    w.try_query::<(&NpcBrain, &NpcFireLine)>().is_some_and(|mut q| q.iter(w).any(|(b, l)| b.active && l.blocked.is_some()))
}

/// Everything about an NPC's mind: its state and timers, its target and what it knows of
/// it, its attack token, its plan (pending attack, cooldowns, history, rest), its route and
/// its leash. Read-only.
#[derive(Deserialize, JsonSchema)]
pub struct NpcStateArgs {
    /// The NPC: 42 or "e42".
    pub id: Id,
}

/// Which options an NPC may choose now, the first rule that refuses each of the others, and each
/// eligible option's chance (the director's grant and the planner judge by the same rules). A dev
/// caller also gets `pick`: the option the next tick's draw would take, when the next tick is a
/// decision tick of an NPC that holds a grant. Read-only.
#[derive(Deserialize, JsonSchema)]
pub struct NpcScheduleArgs {
    /// The NPC: 42 or "e42".
    pub id: Id,
    /// Judge against this target. Default: its grant's target, else its brain's.
    pub target: Option<Id>,
    /// Meters, instead of the measured distance.
    pub distance: Option<f32>,
    /// Degrees 0-180 (0 ahead, 180 behind), instead of the measured angle.
    pub angle: Option<f32>,
    /// Living players, instead of the counted ones.
    pub players: Option<u32>,
    /// The live tags, instead of the collected ones: `["capture"]`, or `[]` for none.
    pub live: Option<Vec<String>>,
}

fn npc_schedule(In(call): In<Call<NpcScheduleArgs>>, p: ScheduleParams, role: Option<Res<shards_core::NetRole>>) -> ToolResult<Value> {
    // The grants, the targets and the think timers it reads are the host's; a client has none of them.
    if role.is_some_and(|r| !r.simulates()) {
        return Err("the host owns NPC plans: ask the host".to_owned());
    }
    let args = &call.args;
    if let Some(angle) = args.angle
        && !(0.0..=180.0).contains(&angle)
    {
        return Err(format!("angle must be 0..180 degrees (is {angle})"));
    }
    if let Some(distance) = args.distance
        && !(distance >= 0.0 && distance.is_finite())
    {
        return Err(format!("distance must be >= 0 and finite (is {distance})"));
    }
    if args.players == Some(0) {
        return Err("players must be at least 1 (is 0)".to_owned());
    }
    if args.live.as_ref().is_some_and(|tags| tags.iter().any(|t| t.trim().is_empty())) {
        return Err("live lists tags, e.g. live=[\"capture\"] (an empty tag names nothing)".to_owned());
    }
    let entity = p.resolve(args.id.0)?;
    if !p.is_npc(entity) {
        return Err(format!("{} is not an NPC (it has no Npc component)", args.id));
    }
    let what_if =
        WhatIf { target: args.target.map(|t| t.0), distance: args.distance, angle: args.angle, players: args.players, live: args.live.clone() };
    let report = schedule_report(&p, entity, &what_if, call.caller.scope.allows(Scope::Dev))?;
    serde_json::to_value(report).map_err(|e| e.to_string())
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
    // A tokenless plan (a boss among its adds): its grant is outside the director's books.
    let tokenless = world
        .get_resource::<shards_content::ContentDb>()
        .and_then(|db| npc.plan.as_deref().and_then(|p| db.peek::<crate::def::AttackPlanDef>(p)))
        .is_some_and(|p| p.tokenless);
    let token = e.get::<AttackToken>().and_then(|t| t.held.as_ref()).map(|g| {
        let mut held = json!({
            "target": g.target.to_string(),
            "attack": g.attack,
            "held_for": round(g.age),
            "pressed": g.pressed.is_some(),
            "started": g.started,
        });
        if tokenless {
            held["counted"] = json!(false);
        }
        held
    });
    let asking = e.get::<AttackToken>().and_then(|t| t.asking_since);
    let plan = e.get::<NpcPlan>().cloned().unwrap_or_default();
    let cooldowns: serde_json::Map<String, Value> =
        plan.cooldowns.iter().map(|c| (c.name.clone(), json!(round(c.left)))).collect();
    let body = body_of(world, entity).unwrap_or_default();
    let nav = e.get::<NavAgent>().map(|a| {
        json!({
            // Where its routes come from: the navmesh of the class that serves its body, or
            // the local grid and why.
            "routes": crate::nav::routes_at(world, &body, here),
            "goal": a.goal.map(fmt_pos),
            "waypoints": a.path.len(),
            "next": a.path.get(a.index as usize).map(|p| fmt_pos(*p)),
            "direct": a.direct,
            "arrived": a.arrived,
            // A goal with no way there: it holds until it plans again.
            "no_route": a.goal.is_some() && a.path.is_empty(),
            // The body avoidance steers it around this tick (it walks off its route for it).
            "avoiding": a.avoiding.map(|id| id.to_string()),
        })
    });
    let home = e.get::<Home>().map(|h| {
        let at = h.pos.unwrap_or(here);
        json!({"at": fmt_pos(at), "leash": h.leash, "distance": round(flat(here - at).length())})
    });
    let drive = e.get::<NpcDrive>().cloned().unwrap_or_default();
    let mut out = json!({
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
    });
    if tokenless {
        out["plan"]["tokenless"] = json!(true);
    }
    extras(world, entity, npc, &perceived, here, &mut out);
    Ok(out)
}

/// The perception of an NPC: its own definition, else the engine's defaults.
fn perception_of(world: &World, npc: &Npc) -> crate::def::PerceptionDef {
    world.get_resource::<shards_content::ContentDb>().and_then(|db| db.get::<crate::def::PerceptionDef>(&npc.perception)).cloned().unwrap_or_default()
}

/// Targets seen beyond the sight range, kept in sight by the perception's `keep_range`.
fn kept<'a>(def: &crate::def::PerceptionDef, perceived: &'a Perceived, here: Vec3) -> impl Iterator<Item = &'a crate::perception::Sensed> + 'a {
    let sight = def.sight_range;
    let keeps = def.keep_range.is_some();
    perceived.known.iter().filter(move |k| keeps && k.seen && flat(k.pos - here).length() > sight)
}

/// What only the duel rules put on an NPC's mind, for `npc_state`: strike notes and the spacing
/// it waits out, the decision timer, a pending combo continuation, the options its conditions
/// open and close, the keep range. Nothing for an NPC that uses none of them.
fn extras(world: &World, entity: Entity, npc: &Npc, perceived: &Perceived, here: Vec3, out: &mut Value) {
    let e = world.entity(entity);
    let round = |v: f32| (v * 100.0).round() / 100.0;
    if let Some(s) = e.get::<NpcStrike>() {
        out["strike"] = json!({"last_start": s.last_start, "gated": s.gated});
    }
    if let Some((by, left)) = wait_of(world, entity) {
        out["spacing"] = json!({"waits_for": by.to_string(), "ticks_left": left});
    }
    if let Some(t) = e.get::<NpcThink>() {
        let every = world.get_resource::<shards_content::ContentDb>().and_then(|db| db.get::<crate::def::BehaviorDef>(&npc.behavior)).and_then(|b| b.think);
        out["think"] = json!({"left": round(t.left), "every": every.map(round), "decided": t.decided});
    }
    if let Some(c) = e.get::<NpcChain>() {
        out["chain"] = json!({"input": c.input, "pressed": c.pressed.is_some(), "gate_at_start": c.at_start});
    }
    if let Some(o) = e.get::<NpcOptions>() {
        let gated: Vec<String> = world
            .get_resource::<shards_content::ContentDb>()
            .and_then(|db| npc.plan.as_deref().and_then(|p| db.get::<crate::def::AttackPlanDef>(p)))
            .map(|p| p.options.iter().filter(|opt| !opt.when.is_empty()).map(|opt| opt.label().to_owned()).collect())
            .unwrap_or_default();
        let closed: Vec<&String> = gated.iter().filter(|g| !o.allowed.contains(g)).collect();
        out["plan"]["options"] = json!({"open": o.allowed, "closed": closed});
    }
    // The repeat gate's count: the newest attack and how many times in a row it was chosen.
    if let Some(newest) = world.get::<NpcPlan>(entity).and_then(|p| p.history.last().map(|h| (h.name.clone(), repeats_of(&p.history, &h.name))))
        && world
            .get_resource::<shards_content::ContentDb>()
            .and_then(|db| npc.plan.as_deref().and_then(|p| db.peek::<crate::def::AttackPlanDef>(p)))
            .is_some_and(|p| p.options.iter().any(|o| o.max_repeats.is_some()))
    {
        out["plan"]["repeats"] = json!({"name": newest.0, "times": newest.1});
    }
    // F11: what it holds for a threat, where it hides, the band it keeps and its line of fire, its turret.
    if let Some(t) = e.get::<NpcThreats>() {
        out["threats"] = json!(t.known.iter().map(|t| json!({"id": t.id.to_string(), "since": round(t.age)})).collect::<Vec<_>>());
    }
    if let Some((spot, cowering)) = hide_status(world, entity) {
        let brain = e.get::<NpcBrain>();
        let from = brain.and_then(|b| NpcCtx::new(world, entity, b, 0.0)).and_then(|c| threat_eye(&c));
        out["hiding"] = json!({
            "spot": spot.map(fmt_pos),
            "where": spot.map(|s| relative(here, s)),
            "cowering": cowering,
            "from": from.map(|(id, eye)| id.map_or_else(|| fmt_pos(eye), |id| id.to_string())),
        });
    }
    if let Some((min, max)) = world
        .get_resource::<shards_content::ContentDb>()
        .and_then(|db| npc.plan.as_deref().and_then(|p| db.peek::<crate::def::AttackPlanDef>(p)))
        .and_then(|p| p.keep_range)
    {
        let line = e.get::<NpcFireLine>();
        out["range"] = json!({
            "keep": [min, max],
            "line_of_fire": line.map(|l| json!({
                "target": l.target.map(|t| t.to_string()),
                "blocked": l.blocked.map(|b| b.text()),
            })),
        });
    }
    if let Some(t) = e.get::<Turret>() {
        out["turret"] = json!({"def": t.def, "active": t.active, "line": turret_line(world, entity)});
    }
    let def = perception_of(world, npc);
    if def.notice != crate::def::Notice::Hostile {
        out["perception"]["notice"] = json!(format!("{:?}", def.notice));
    }
    if let Some(keep) = def.keep_range {
        out["perception"]["sight_range"] = json!(def.sight_range);
        out["perception"]["keep_range"] = json!(keep);
        let kept: Vec<String> = kept(&def, perceived, here).map(|k| k.id.to_string()).collect();
        if let Some(list) = out["perceived"].as_array_mut() {
            for entry in list {
                if kept.iter().any(|id| entry["id"] == json!(id)) {
                    entry["kept"] = json!(true);
                }
            }
        }
    }
}

/// The same for `npc_overlay`'s line: `waits 31t for e7`, `think 0.07s`, `chain light`,
/// `keeps e5 17.2m N`.
pub(crate) fn extra_parts(world: &World, entity: Entity, npc: &Npc, perceived: &Perceived, here: Vec3) -> Vec<String> {
    let e = world.entity(entity);
    let mut parts = Vec::new();
    if let Some((by, left)) = wait_of(world, entity) {
        parts.push(format!("waits {left}t for {by}"));
    }
    if let Some(t) = e.get::<NpcThink>() {
        parts.push(format!("think {:.2}s", t.left));
    }
    if let Some(c) = e.get::<NpcChain>() {
        parts.push(format!("chain {}{}", c.input, if c.pressed.is_some() { " (pressed)" } else { "" }));
    }
    let def = perception_of(world, npc);
    parts.extend(kept(&def, perceived, here).map(|k| format!("keeps {} {}", k.id, relative(here, k.pos))));
    parts.extend(ranged_parts(world, entity));
    parts
}

/// The route an NPC would take, as `path_to` reports one: `path 23.5m, 4 turns: 6.0m NE →
/// 4.5m N (mantle 1.1m) → ...`, with the waypoints. Inside a loaded region it comes from the
/// navmesh (`provider: navmesh`, the body class it was walked as and the tiles it crosses),
/// elsewhere from the local grid (or the game's own provider). From a point or an entity
/// (default: you) to another, for the walker's own body (from a point: the body of the
/// tightest class `radius` fits) on the tightest body class that serves it, or as a body of
/// class `body`. Read-only.
#[derive(Deserialize, JsonSchema)]
pub struct NavPathArgs {
    /// Where the route starts: an entity id or a ground point {x, z}. Default: your body.
    pub from: Option<TargetArg>,
    /// Where it goes: an entity id or a ground point {x, z}.
    pub to: TargetArg,
    /// Body radius in meters. Default: the walker's (the body class's for a point).
    pub radius: Option<f32>,
    /// A body class of the nav settings (`nav_info` lists them): the route on its navmesh,
    /// for its body. Default: the tightest class that serves the walker's body.
    pub body: Option<String>,
}

/// An entity's body numbers: its controller's (or its own).
fn body_of(world: &World, e: Entity) -> Option<CharacterParams> {
    let own = world.get::<CharacterParams>(e);
    world.get::<CharacterController>(e).map(|c| c.resolve(own, world.get_resource()).clone()).or(own.cloned())
}

fn nav_path(In(call): In<Call<NavPathArgs>>, world: &mut World) -> ToolResult<Value> {
    if crate::nav::region_is_volume(world) && !crate::nav::simulates(world) {
        return Err(format!("no navmesh for this region ({}): routes are the host's; `path_to` plans over the local grid", crate::nav::VOXEL_NOT_SIMULATED));
    }
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
    let region = crate::nav::region_nav(world);
    // The body class asked for: its navmesh, and its body unless a radius is given.
    let class = match &call.args.body {
        None => None,
        Some(name) => {
            let Some(nav) = region else {
                return Err(format!(
                    "no navmesh to walk as the body `{name}`: no region is loaded (body classes are navmeshes of a region); \
                     load one with `regen`, or leave out `body`"
                ));
            };
            let names: Vec<&str> = nav.classes().map(|c| c.body.as_str()).collect();
            let Some(class) = nav.class(name) else {
                let hint = closest(name, names.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                return Err(format!("no body class `{name}` in the nav settings{hint}; classes: {}", names.join(", ")));
            };
            Some(class)
        }
    };
    // A walker has its own body, as its routes do (an NPC's `navmesh_path`, anyone else's
    // `move_to path=true`); a point the body of the class its radius fits; the class asked
    // for its own body.
    let mut params = match walker {
        Some(w) if class.is_none() => body_of(world, w).unwrap_or_default(),
        _ => {
            let radius = call.args.radius.or_else(|| walker.and_then(|w| body_of(world, w)).map(|p| p.radius));
            let base = class.or_else(|| radius.and_then(|r| region?.for_radius(r))).or(region);
            let body = base.map_or_else(CharacterParams::default, |nav| nav.agent.clone());
            CharacterParams { radius: radius.unwrap_or(body.radius), ..body }
        }
    };
    if let Some(radius) = call.args.radius {
        params.radius = radius;
    }
    let radius = params.radius;
    if !(radius > 0.0 && radius.is_finite()) {
        return Err(format!("radius must be > 0 (is {radius})"));
    }
    if let Some(class) = class
        && !class.fits(radius, params.height)
    {
        return Err(format!(
            "a body of {radius:.2} m radius does not fit the body class `{}` ({:.2} m); give a smaller `radius`, or another \
             `body`",
            class.body, class.agent_radius
        ));
    }
    let round = |v: f32| (v * 100.0).round() / 100.0;
    let paths = world.get_resource::<NavPaths>().copied().unwrap_or_default();
    let mut provider = paths.name;
    let mut find = paths.find;
    // Off the navmesh, a walker that is no NPC leaves its own capsule out of the local grid,
    // as `move_to path=true` does.
    let mut own_body = None;
    if provider == NAVMESH || class.is_some() {
        // The class asked for, else the tightest that serves the body.
        let on = class.or_else(|| region.and_then(|nav| nav.serving(&params)));
        let routed = on.and_then(|nav| crate::nav::route_on(world, nav, from, to, &params, target.is_some()));
        match routed {
            Some(Ok(path)) => {
                let mut out = path.to_plan().to_json();
                out["provider"] = json!(NAVMESH);
                out["body"] = json!(on.map(|nav| nav.body.clone()));
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
                own_body = walker.filter(|w| world.get::<crate::Npc>(*w).is_none());
            }
        }
    }
    let path = match own_body {
        Some(w) => local_grid_route(world, from, to, &params, Some(w)),
        None => find(world, from, to, &params),
    };
    let Some(path) = path else {
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

/// Forgets what an NPC's plan remembers: its history and its cooldowns (`reset_plan`). The attack
/// count, the rest, its grant and any chain stay.
#[derive(Deserialize, JsonSchema)]
pub struct NpcPlanResetArgs {
    /// The NPC: 42 or "e42".
    pub id: Id,
}

fn npc_plan_reset(In(call): In<Call<NpcPlanResetArgs>>, world: &mut World) -> ToolResult {
    let entity = call.args.id.entity(world)?;
    reset_plan(world, entity, "npc_plan_reset")
}

/// Sets the plan cooldown of one option of an NPC's plan (`set_plan_cooldown`).
#[derive(Deserialize, JsonSchema)]
pub struct NpcPlanCooldownArgs {
    /// The NPC: 42 or "e42".
    pub id: Id,
    /// The option, by the name `npc_schedule` lists.
    pub option: String,
    /// Seconds, >= 0; `0` with `exactly` takes it off cooldown.
    pub seconds: f32,
    /// `exactly` (the default) or `at_least` (keeps the larger).
    pub mode: Option<String>,
}

fn npc_plan_cooldown(In(call): In<Call<NpcPlanCooldownArgs>>, world: &mut World) -> ToolResult {
    let args = &call.args;
    let how = match args.mode.as_deref() {
        None | Some("exactly") => CooldownSet::Exactly,
        Some("at_least") => CooldownSet::AtLeast,
        Some(other) => return Err(format!("mode must be `exactly` or `at_least` (is `{other}`)")),
    };
    let entity = args.id.entity(world)?;
    set_plan_cooldown(world, entity, &args.option, args.seconds, how)
}
/// The content-only audit of an attack plan with a moveset (N8e): every pair of options the plan lets
/// follow, with the shortest gap between the first one's danger and the second one's first hit
/// (shortest first); with `min_gap`, the pairs closer than that under `short`; every hit an NPC of the
/// plan can deal, with how often it can land in one attack; the options the moveset cannot press. The
/// verdict (what is too short, what kills) is the game's. Reads content only.
#[derive(Deserialize, JsonSchema)]
pub struct PlanAuditArgs {
    /// The `attack_plan` id.
    pub plan: String,
    /// The `moveset` id of an NPC that fights by it. Without it the answer names the movesets of the prefabs that do.
    pub moveset: Option<String>,
    /// Seconds: list the pairs with a shorter gap under `short`.
    pub min_gap: Option<f32>,
    /// The factor on the plan's shortest rest (a difficulty's pause factor, below 1 on a hard one); default 1.
    pub rest_scale: Option<f32>,
}

fn plan_audit(In(call): In<Call<PlanAuditArgs>>, db: Res<ContentDb>) -> ToolResult<Value> {
    let args = call.args;
    if let Some(gap) = args.min_gap
        && !(gap >= 0.0 && gap.is_finite())
    {
        return Err(format!("min_gap must be >= 0 and finite (is {gap})"));
    }
    let rest_scale = args.rest_scale.unwrap_or(1.0);
    if !(rest_scale > 0.0 && rest_scale.is_finite()) {
        return Err(format!("rest_scale must be > 0 and finite (is {rest_scale})"));
    }
    let Some(moveset) = args.moveset else {
        let used = movesets_of_plan(&db, &args.plan)?;
        let used = if used.is_empty() { "no prefab's NPC names this plan".to_owned() } else { format!("the plan's NPCs use: {}", used.join(", ")) };
        return Err(format!("plan_audit needs moveset=<id>; {used}"));
    };
    let pairs = follow_pairs(&db, &args.plan, &moveset, rest_scale)?;
    let hits = plan_hits(&db, &args.plan, &moveset)?;
    let unreachable = unreachable_options(&db, &args.plan, &moveset)?;
    // In f64, so 0.7 reads 0.7 and not 0.699999988.
    let round = |v: f32| (f64::from(v) * 100.0).round() / 100.0;
    let pair = |p: &FollowPair| {
        let mut v = json!({"a": p.a, "b": p.b, "gap": round(p.gap), "a_end": round(p.a_end), "b_first": round(p.b_first)});
        if p.chained {
            v["chained"] = json!(true);
        }
        v
    };
    let mut answer = json!({
        "plan": args.plan,
        "moveset": moveset,
        "rest_scale": rest_scale,
        "pairs": pairs.iter().map(pair).collect::<Vec<_>>(),
        "hits": hits,
        "unreachable": unreachable,
    });
    if let Some(min_gap) = args.min_gap {
        let short: Vec<Value> = pairs
            .iter()
            .filter(|p| p.gap < min_gap)
            .map(|p| {
                let chain = if p.chained { " (through its chain)" } else { "" };
                let mut v = pair(p);
                v["why"] = json!(format!("{}'s danger lasts until {:.2} s{chain}; {} hits from {:.2} s", p.a, p.a_end, p.b, p.b_first));
                v
            })
            .collect();
        answer["short"] = json!(short);
    }
    Ok(answer)
}

/// The movesets of the prefabs whose `Npc` fights by plan `plan` (what they extend included), sorted.
/// An unknown plan is an error with a "did you mean".
fn movesets_of_plan(db: &ContentDb, plan: &str) -> Result<Vec<String>, String> {
    if db.peek::<AttackPlanDef>(plan).is_none() {
        let hint = closest(plan, db.peek_ids::<AttackPlanDef>()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        return Err(format!("no attack_plan `{plan}`{hint}"));
    }
    let mut used = std::collections::BTreeSet::new();
    for (id, _) in db.peek_iter::<PrefabDef>() {
        // A child's component replaces its parent's whole: the nearest `Npc` and `Fighter` count. The
        // plan is written `Some("warden")`, or `"warden"` (content reads RON with implicit `Some`).
        let npc_plan = prefab_component(db, id, "Npc").and_then(|npc| match field(&npc, "plan")? {
            ron::Value::String(p) => Some(p),
            ron::Value::Option(Some(p)) => match *p {
                ron::Value::String(p) => Some(p),
                _ => None,
            },
            _ => None,
        });
        let moveset = prefab_component(db, id, "Fighter").and_then(|f| match field(&f, "moveset") {
            Some(ron::Value::String(m)) => Some(m),
            _ => None,
        });
        if npc_plan.as_deref() == Some(plan)
            && let Some(m) = moveset
        {
            used.insert(m);
        }
    }
    Ok(used.into_iter().collect())
}
