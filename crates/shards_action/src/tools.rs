//! What agents can do in a fight. Player tools act only through `Intent` (a press, a
//! held button, an order, the lock-on target) and answer by the same rule the machine
//! uses ([`can_start`]): `ok: light_1`, `ok: queued, dodge starts in 0.12s (N)`, or an
//! error that says why and when. Dev tools change the world directly.

use ai_core::prelude::*;
use ai_core::spatial::{compass, compass_vector, relative};
use ai_core::{AiIndex, DevCommand};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_content::ContentDb;
use shards_core::input_names::{BLOCK, DODGE, HEAVY, INTERACT, LIGHT, PARRY, TECHNIQUE, USE_ITEM};
use shards_core::{flat, Intent, NetRole, Order, Predicted};
use shards_motion::camera::{LockOnDef, Side};

use crate::body::{clear_barrier, clear_damage_taken, describe_damage_taken, extend_damage_taken, grant_barrier, set_damage_taken, Barrier, DamageTaken};
use crate::combat::{react, revive, revive_downed, stagger, Downed, Reaction, StaggerCause, Standing};
use crate::context::{forget, remember, resolve as resolve_context};
use crate::def::{ActionDef, BufferMode, CombatTuning, MovesetDef, Steer, EPS};
use crate::fighter::{hostile, Dead, Fighter, Health, Invulnerable, Poise, Stamina, Team};
use crate::lock::{may_lock_on, pick_for_body, pick_point_of, release_lock, switch_lock_of, switch_rules, take_lock, target_name};
use crate::parts::{body_of, body_takes_lock, lock_points_of, part_of, Part};
use crate::machine::{
    buffer_press, can_start, fits_buffer, press_life, understands, ActionState, FighterView, Healed, InputBuffer, Queued,
};
use crate::predict::{shown_queue, shown_state};
use crate::zone::{body_radius, zones_threatening};

pub(crate) fn register(app: &mut App) {
    app.add_ai_tool("light_attack", Scope::Player, light_attack)
        .add_ai_tool("heavy_attack", Scope::Player, heavy_attack)
        .add_ai_tool("parry", Scope::Player, parry)
        .add_ai_tool("block", Scope::Player, block)
        .add_ai_tool("dodge", Scope::Player, dodge)
        .add_ai_tool("use_technique", Scope::Player, use_technique)
        .add_ai_tool("use_item", Scope::Player, use_item)
        .add_ai_tool("interact", Scope::Player, interact)
        .add_ai_tool("lock_on", Scope::Player, lock_on)
        .add_ai_tool("attack", Scope::Player, attack)
        // The movement tools belong to the motion module, which does not know death.
        .add_ai_tool_guard("move", dead_refusal)
        .add_ai_tool_guard("move_to", dead_refusal)
        .add_ai_tool_guard("stop", dead_refusal)
        .add_ai_tool_guard("jump", dead_refusal)
        .add_ai_tool_guard("sprint", dead_refusal)
        .add_ai_tool_guard("glide", dead_refusal)
        .add_ai_tool_guard("crouch", dead_refusal)
        .add_ai_tool("god", Scope::Dev, god)
        .add_ai_tool("heal", Scope::Dev, heal)
        .add_ai_tool("barrier", Scope::Dev, barrier)
        .add_ai_tool("damage_taken", Scope::Dev, damage_taken)
        .add_ai_tool("stagger", Scope::Dev, stagger_tool)
        .add_ai_tool("react", Scope::Dev, react_tool)
        .add_ai_tool("down", Scope::Dev, down_tool)
        .add_ai_tool("remember", Scope::Dev, remember_tool)
        .add_ai_condition("reacting", reacting)
        .add_dev_entry("combat", "god", "Immortality on / off", DevCommand::Live(god_line))
        .add_dev_entry("combat", "heal", "Heal / revive", DevCommand::Run("heal".into()))
        .add_dev_entry("combat", "down_me", "Go down (until revived)", DevCommand::Live(down_me_line))
        .add_dev_entry("combat", "down", "Down a body (bleeds out)...", DevCommand::Fill("down id= bleed=30".into()));
}

/// The menu's "go down": the caller's own body, down until a partner's revive (or `heal`).
fn down_me_line(world: &World, caller: &Caller) -> Result<String, String> {
    let me = caller.require_subject()?;
    let id = world.get::<AiId>(me).ok_or_else(|| "your body has no AiId yet: try again on the next tick".to_owned())?;
    Ok(format!("down id={id}"))
}

/// The menu's immortality switch: off when the caller's body has it.
fn god_line(world: &World, caller: &Caller) -> Result<String, String> {
    let me = caller.require_subject()?;
    Ok(ai_core::dev::toggle_line("god", world.get::<Invulnerable>(me).is_some()))
}

const DEAD: &str = "you are dead; a dev agent can `heal` you";

/// The refusal every action tool gives a dead body ([`me`]), for the movement tools too; a downed
/// body (B5) is refused them as its actions are (`Blocker::Downed`).
fn dead_refusal(world: &World, caller: &Caller) -> Result<(), String> {
    match caller.subject {
        Some(me) if world.get::<Dead>(me).is_some() => Err(DEAD.into()),
        Some(me) if world.get::<Downed>(me).is_some() => Err(crate::machine::Blocker::Downed.message()),
        _ => Ok(()),
    }
}

/// Your body, if it is a fighter you can control.
pub(crate) fn me(world: &World, caller: &Caller) -> Result<Entity, String> {
    let me = caller.require_subject()?;
    if world.get::<Fighter>(me).is_none() {
        return Err("your body cannot fight (it has no Fighter component)".into());
    }
    if world.get::<Intent>(me).is_none() {
        return Err("your body cannot be controlled (it has no Intent)".into());
    }
    if world.get::<Dead>(me).is_some() {
        return Err(DEAD.into());
    }
    Ok(me)
}

/// When a press will start: now (as this action), or in so many seconds.
enum When {
    Now(String),
    In(f32),
}

/// The shared rule's answer for `input`: now, within the press's life in the buffer
/// ([`press_life`]: longer while a `queue` span holds it for a window), or the error. A moveset
/// with a single slot answers as its buffer will: a press weaker than the one waiting is dropped.
/// On a client's own body it asks about the action and the presses shown there ([`shown_state`],
/// [`shown_queue`]).
fn when(world: &World, me: Entity, input: &str) -> Result<When, String> {
    let dt = world.resource::<Time<Fixed>>().timestep().as_secs_f32();
    let ttl = CombatTuning::of(world.resource::<ContentDb>()).input_buffer;
    let shown = shown_state(world, me);
    // The context the next tick resolves (a client predicts with its host's, replicated).
    let fresh = world.get_resource::<NetRole>().is_none_or(|r| r.simulates()).then(|| resolve_context(world, me)).flatten();
    let mut view = FighterView::of(world, me).ok_or("your body is not a complete fighter")?;
    if let Some(state) = &shown {
        view.state = state;
    }
    if let Some(context) = &fresh {
        view.context = Some(context);
    }
    let run = view.state.current.as_ref();
    let def = run.and_then(|r| view.db.get::<ActionDef>(&r.action));
    let life = press_life(def, run, input, ttl);
    if let Some(mode @ BufferMode::SingleSlot { .. }) = view.moveset_def().map(|m| &m.buffer)
        && let Some(waiting) = waiting(world, me, &view).into_iter().next()
        && mode.rank(input) < mode.rank(&waiting.input)
    {
        return Err(format!(
            "{} is waiting and comes first: one press waits at a time, and {input} ranks below it",
            waiting.input
        ));
    }
    match can_start(&view, input, dt) {
        Ok(start) => Ok(When::Now(start.action)),
        Err(blocker) => match blocker.ready_in() {
            Some(t) if fits_buffer(t, life) => Ok(When::In(t)),
            _ => Err(blocker.message()),
        },
    }
}

/// The presses `me` has waiting as its next tick takes them: its buffer (on a client's own body,
/// the predictor's), with the presses still in its `Intent` added the way the buffer adds them.
fn waiting(world: &World, me: Entity, view: &FighterView) -> Vec<Queued> {
    let mut queued = shown_queue(world, me)
        .unwrap_or_else(|| world.get::<InputBuffer>(me).map(|b| b.queued.clone()).unwrap_or_default());
    let moveset = view.moveset_def();
    let mode = moveset.map_or(&BufferMode::PerInput, |m| &m.buffer);
    let running = view.state.current.as_ref().and_then(|r| view.db.get::<ActionDef>(&r.action));
    for p in world.get::<Intent>(me).map_or(&[][..], |i| &i.pressed[..]) {
        if understands(&p.input, moveset, running) {
            buffer_press(&mut queued, mode, Queued { input: p.input.clone(), dir: p.dir, left: 0.0 });
        }
    }
    queued
}

/// Presses `input` (towards `dir`) when the rule says it will start. `detail` follows the
/// answer (a dodge's direction). `hold`: `Some(true)` presses and keeps the input held (a
/// charge, an action that goes on while held), `Some(false)` lets go of it without a press.
pub(crate) fn press(world: &mut World, caller: &Caller, input: &str, dir: Option<Vec3>, detail: &str) -> ToolResult {
    press_held(world, caller, input, dir, detail, None)
}

pub(crate) fn press_held(world: &mut World, caller: &Caller, input: &str, dir: Option<Vec3>, detail: &str, hold: Option<bool>) -> ToolResult {
    let me = me(world, caller)?;
    if hold == Some(false) {
        world.get_mut::<Intent>(me).ok_or("no Intent")?.set_held(input, false);
        return Ok(format!("ok: {input} released"));
    }
    let when = when(world, me, input)?;
    let mut note = predicted_note(world, me);
    // A paired action is not predicted: the host starts it and the client takes it over.
    if let When::Now(action) = &when
        && !note.is_empty()
        && world.resource::<ContentDb>().get::<ActionDef>(action).is_some_and(|d| d.pair.is_some())
    {
        note = " · the host starts it (a paired action is not predicted)";
    }
    let (displaced, tap) = {
        let view = FighterView::of(world, me).ok_or("your body is not a complete fighter")?;
        let shown = shown_state(world, me);
        let view = FighterView { state: shown.as_ref().unwrap_or(view.state), ..view };
        let slot = matches!(view.moveset_def().map(|m| &m.buffer), Some(BufferMode::SingleSlot { .. }));
        let displaced = waiting(world, me, &view)
            .into_iter()
            .next()
            .filter(|q| slot && q.input != input)
            .map(|q| format!(" ({} no longer waits)", q.input))
            .unwrap_or_default();
        // An action that goes on only while its input is held, pressed without holding it.
        let action = match &when {
            When::Now(action) => Some(action.as_str()),
            When::In(_) => view.moveset_def().and_then(|m| m.inputs.get(input)).map(String::as_str),
        };
        let lets_go = action.and_then(|a| view.db.get::<ActionDef>(a)).is_some_and(|d| d.while_held.as_deref() == Some(input));
        let tap = if lets_go && hold != Some(true) && !view.held.iter().any(|h| h == input) {
            format!(" (a tap: it stops as soon as {input} is not held; hold=true keeps it going)")
        } else {
            String::new()
        };
        (displaced, tap)
    };
    let mut intent = world.get_mut::<Intent>(me).ok_or("no Intent")?;
    match dir {
        Some(dir) => intent.press_dir(input, dir),
        None => intent.press(input),
    }
    let holding = if hold == Some(true) {
        intent.set_held(input, true);
        " (holding)"
    } else {
        ""
    };
    Ok(match when {
        When::Now(action) => format!("ok: {action}{holding}{detail}{displaced}{tap}{note}"),
        When::In(t) => format!("ok: queued, {input} starts in {t:.2}s{holding}{detail}{displaced}{tap}{note}"),
    })
}

/// ` · predicted, host confirms` on a client's own body: it starts here at once, and the
/// host has the last word.
fn predicted_note(world: &World, me: Entity) -> &'static str {
    if world.get::<Predicted>(me).is_some() { " · predicted, host confirms" } else { "" }
}

/// Whether to hold the button after the press.
#[derive(Deserialize, JsonSchema, Default)]
pub struct HoldArgs {
    /// true presses and keeps the button held (an attack that charges while held turns into
    /// its charged version); false lets go of it (no press). Default: a plain press.
    pub hold: Option<bool>,
}

/// Light attack in the direction you face (or at your lock-on target). Chains into a
/// combo when pressed again inside the attack's cancel window. Answers `ok`, `ok:
/// queued` (it starts within the input buffer) or why not and when it would work.
fn light_attack(In(call): In<Call<HoldArgs>>, world: &mut World) -> ToolResult {
    press_held(world, &call.caller, LIGHT, None, "", call.args.hold)
}

/// Heavy attack: slower, costs more stamina, hits harder. `hold=true` keeps the button down
/// (a charged heavy turns while it is held), `hold=false` lets go.
fn heavy_attack(In(call): In<Call<HoldArgs>>, world: &mut World) -> ToolResult {
    press_held(world, &call.caller, HEAVY, None, "", call.args.hold)
}

/// Parry: a short perfect guard. A hit landing in it does no damage and staggers the
/// attacker; time it to the end of an enemy windup.
fn parry(In(call): In<Call<NoArgs>>, world: &mut World) -> ToolResult {
    press(world, &call.caller, PARRY, None, "")
}

/// Raise (or lower) your guard: hold block. Blocked hits cost stamina instead of
/// health; when stamina runs out the guard breaks. Raising it just in time (its first
/// moments) is a perfect block.
#[derive(Deserialize, JsonSchema, Default)]
pub struct BlockArgs {
    /// false lowers the guard. Default true.
    pub on: Option<bool>,
}

fn block(In(call): In<Call<BlockArgs>>, world: &mut World) -> ToolResult {
    let on = call.args.on.unwrap_or(true);
    let me = me(world, &call.caller)?;
    if !on {
        world.get_mut::<Intent>(me).ok_or("no Intent")?.set_held(BLOCK, false);
        return Ok("ok: block lowered".into());
    }
    let db = world.resource::<ContentDb>();
    let moveset = world.get::<Fighter>(me).and_then(|f| db.get::<crate::MovesetDef>(&f.moveset));
    let action = moveset.and_then(|m| m.action(BLOCK)).map(str::to_owned);
    let held = action
        .as_deref()
        .and_then(|a| db.get::<ActionDef>(a))
        .and_then(|d| d.hold.as_ref())
        .is_some_and(|h| h.input == BLOCK);
    if !held {
        // A block without a hold loop is a timed press.
        return press(world, &call.caller, BLOCK, None, "");
    }
    if action.as_deref().is_some_and(|a| holding_up(world, me, a)) {
        world.get_mut::<Intent>(me).ok_or("no Intent")?.set_held(BLOCK, true);
        return Ok("ok: already blocking".into());
    }
    let dt = world.resource::<Time<Fixed>>().timestep().as_secs_f32();
    let shown = shown_state(world, me);
    let mut view = FighterView::of(world, me).ok_or("your body is not a complete fighter")?;
    if let Some(state) = &shown {
        view.state = state;
    }
    let note = predicted_note(world, me);
    let answer = match can_start(&view, BLOCK, dt) {
        Ok(start) => format!("ok: {} (holding block){note}", start.action),
        // Held, it rises whenever it can: no buffer to run out.
        Err(blocker) => match blocker.ready_in() {
            Some(t) => format!("ok: holding block; it rises in {t:.2}s{note}"),
            None => return Err(blocker.message()),
        },
    };
    world.get_mut::<Intent>(me).ok_or("no Intent")?.set_held(BLOCK, true);
    Ok(answer)
}

/// Whether `action` is running and still inside its hold span, i.e. a held input keeps
/// it up. A block that is lowering is not: it ends, and only then rises again.
fn holding_up(world: &World, me: Entity, action: &str) -> bool {
    let shown = shown_state(world, me);
    let state = shown.as_ref().or_else(|| world.get::<ActionState>(me));
    let run = state.and_then(|s| s.current.as_ref()).filter(|r| r.action == action);
    let def = world.resource::<ContentDb>().get::<ActionDef>(action);
    let end = def.and_then(|d| d.span(&d.hold.as_ref()?.span)).map(|(_, end)| end);
    run.zip(end).is_some_and(|(run, end)| run.t < end - EPS)
}

/// Dodge roll: a burst of movement with invulnerable frames, for stamina. Default
/// direction: out of the hit zone you stand in (see ALERT lines), else where you move
/// or face. Answers `ok`, `ok: queued` or why not and when.
#[derive(Deserialize, JsonSchema, Default)]
pub struct DodgeArgs {
    /// Compass direction: N, NE, E, SE, S, SW, W, NW.
    pub dir: Option<String>,
    /// Or: straight away from this entity.
    pub away_from: Option<Id>,
}

fn dodge(In(call): In<Call<DodgeArgs>>, world: &mut World) -> ToolResult {
    let me = me(world, &call.caller)?;
    let here = world.get::<Transform>(me).map(|t| t.translation).unwrap_or_default();
    let dir = match (&call.args.dir, call.args.away_from) {
        (Some(d), _) => Some(
            compass_vector(d).ok_or_else(|| format!("unknown direction `{d}`; use N, NE, E, SE, S, SW, W or NW"))?,
        ),
        (None, Some(id)) => {
            let there = world.get::<Transform>(id.entity(world)?).map(|t| t.translation).ok_or("it has no position")?;
            Some(flat(here - there).try_normalize().ok_or("you stand on it; give `dir`")?)
        }
        (None, None) => {
            zones_threatening(world, me, here, body_radius(world, me)).first().and_then(|(_, out)| out.try_normalize())
        }
    };
    // The roll goes where the press says, else along the movement input (as `steer` does).
    let moving = world.get::<Intent>(me).and_then(|i| flat(i.move_dir).try_normalize());
    // With nothing named, a backstep goes back and a plain roll goes where the body faces.
    let db = world.resource::<ContentDb>();
    let backstep = world
        .get::<Fighter>(me)
        .and_then(|f| db.get::<MovesetDef>(&f.moveset)?.inputs.get(DODGE))
        .and_then(|action| db.get::<ActionDef>(action))
        .is_some_and(|d| d.steer == Steer::MoveOrBack);
    let towards = dir.or(moving).map_or(if backstep { "back" } else { "forward" }, compass);
    press(world, &call.caller, DODGE, dir, &format!(" ({towards})"))
}

/// Use a technique from a moveset slot (`technique.<slot>`).
#[derive(Deserialize, JsonSchema)]
pub struct TechniqueArgs {
    pub slot: Slot,
    /// true presses and keeps the button held; false lets go of it (no press). Default: a press.
    pub hold: Option<bool>,
}

/// A technique slot: its number (1) or its name.
#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Slot {
    Number(u32),
    Name(String),
}

fn use_technique(In(call): In<Call<TechniqueArgs>>, world: &mut World) -> ToolResult {
    let slot = match call.args.slot {
        Slot::Number(n) => n.to_string(),
        Slot::Name(s) => s,
    };
    let input = format!("{TECHNIQUE}.{slot}");
    let me = me(world, &call.caller)?;
    require_input(world, me, &input, TECHNIQUE, "technique")?;
    press_held(world, &call.caller, &input, None, "", call.args.hold)
}

/// Use an item, e.g. a healing flask (`use_item.<item>` in your moveset). Items with
/// charges fail when none are left.
#[derive(Deserialize, JsonSchema)]
pub struct ItemArgs {
    /// Item name, e.g. "flask".
    pub item: String,
}

fn use_item(In(call): In<Call<ItemArgs>>, world: &mut World) -> ToolResult {
    let input = format!("{USE_ITEM}.{}", call.args.item);
    let me = me(world, &call.caller)?;
    dead_refusal(world, &call.caller)?;
    require_input(world, me, &input, USE_ITEM, "item")?;
    press(world, &call.caller, &input, None, "")
}

/// An error listing what the moveset has under `prefix` when it lacks `input`.
fn require_input(world: &World, me: Entity, input: &str, prefix: &str, what: &str) -> Result<(), String> {
    let db = world.resource::<ContentDb>();
    let Some(moveset) = world.get::<Fighter>(me).and_then(|f| db.get::<crate::MovesetDef>(&f.moveset)) else {
        return Ok(()); // `press` explains a missing moveset.
    };
    if moveset.inputs.contains_key(input) {
        return Ok(());
    }
    let have: Vec<&str> =
        moveset.inputs.keys().filter_map(|k| k.strip_prefix(prefix).and_then(|r| r.strip_prefix('.'))).collect();
    Err(match have.is_empty() {
        true => format!("you have no {what}s"),
        false => format!("no {what} `{}`; you have: {}", &input[prefix.len() + 1..], have.join(", ")),
    })
}

/// Lock on to a target (your attacks and facing follow it), or to the best one you can see,
/// or let go with `off`; `side` moves the lock you hold to the next target on that side.
/// A big body may have lock points (its parts): `point` names one; a body taken only through
/// its points sends a lock on it to the best of them.
#[derive(Deserialize, JsonSchema, Default)]
pub struct LockOnArgs {
    pub target: Option<Id>,
    /// A lock point (a part, by name) of `target`, or without `target` of the body you are
    /// locked on to (else of the best one you can see): `lock_on point=hand`.
    pub point: Option<String>,
    /// Release the lock-on.
    #[serde(default)]
    pub off: bool,
    /// Move the lock you hold to the next target on this side, seen from you: `left` or
    /// `right` (the nearest angle past a small dead band; only what you can see).
    pub side: Option<Side>,
}

fn lock_on(In(call): In<Call<LockOnArgs>>, world: &mut World) -> ToolResult {
    let me = me(world, &call.caller)?;
    let here = world.get::<Transform>(me).map(|t| t.translation).unwrap_or_default();
    if call.args.off {
        world.get::<Intent>(me).ok_or("no Intent")?;
        release_lock(world, me);
        return Ok("ok: lock-on released".into());
    }
    // A downed body fights nobody: it takes no lock (releasing one stays allowed).
    dead_refusal(world, &call.caller)?;
    // Without a `lock_on` definition: the nearest living enemy within the combat tuning's
    // range, and any target asked for by id (as before the lock-on rules).
    let rules = LockOnDef::active(world.get_resource::<ContentDb>()).cloned();
    if let Some(side) = call.args.side {
        if call.args.target.is_some() || call.args.point.is_some() {
            return Err("pass `target`/`point` or `side`, not both: `side` moves the lock you hold".into());
        }
        let switch = switch_rules(world);
        return match switch_lock_of(world, me, side, &switch) {
            Ok((from, to)) => {
                let (to_name, from_name) = (target_name(world, to), target_name(world, from));
                Ok(format!("ok: locked on {to_name} (switched {} from {from_name}){}", side.name(), where_is(world, here, to)))
            }
            Err(None) => Err(format!("you hold no lock to move {}; lock on first (`lock_on`, or `lock_on target=eN`)", side.name())),
            Err(Some(from)) => {
                let from = target_name(world, from);
                Err(format!("nobody you can see to the {} of {from} within {:.0}m; the lock stays on {from}", side.name(), switch.range))
            }
        };
    }
    let target = match (call.args.target, &rules) {
        _ if call.args.point.is_some() => lock_point(world, me, here, call.args.target, call.args.point.as_deref().unwrap_or_default(), rules.as_ref())?,
        (Some(id), rules) => {
            let e = id.entity(world)?;
            if e == me {
                return Err("you cannot lock on to yourself".into());
            }
            match rules {
                // A body taken only through its lock points (R3): the best of them you can see.
                Some(def) if world.get::<Part>(e).is_none() && !body_takes_lock(world, e) => match pick_point_of(world, me, e, def) {
                    Some(point) => point,
                    None => {
                        if world.get::<Dead>(e).is_some() || world.get::<Downed>(e).is_some() {
                            may_lock_on(world, me, e, id.0, def)?;
                        }
                        let points = lock_points_of(world, e);
                        return Err(match points.is_empty() {
                            true => format!("{id} is taken only through its lock points, and none is left (broken)"),
                            false => format!(
                                "none of {id}'s lock points ({}) is in sight within {:.0}m; come closer, or pass `point`",
                                points.join(", "),
                                def.range
                            ),
                        });
                    }
                },
                // What the check would take away at once, and what the pick would never name.
                Some(def) => {
                    may_lock_on(world, me, e, id.0, def)?;
                    id.0
                }
                None => id.0,
            }
        }
        (None, Some(def)) => pick_for_body(world, me, def)
            .ok_or_else(|| format!("nothing you can see within {:.0}m to lock on to; pass `target`", def.range))?,
        (None, None) => {
            let range = CombatTuning::of(world.resource::<ContentDb>()).lock_on_range;
            nearest_enemy(world, me, here, range)
                .ok_or_else(|| format!("no enemy within {range:.0}m to lock on to; pass `target`"))?
        }
    };
    take_lock(world, me, target);
    Ok(format!("ok: locked on {}{}", target_name(world, target), where_is(world, here, target)))
}

/// The lock point `point` of a body (`lock_on point=`): of `target` (the body of the part it
/// names), else of the body you are locked on to, else of the one you would lock on to now.
fn lock_point(world: &World, me: Entity, here: Vec3, target: Option<Id>, point: &str, rules: Option<&LockOnDef>) -> Result<AiId, String> {
    let held = world.get::<Intent>(me).and_then(|i| i.target);
    let body = match target.or(held.map(Id)) {
        Some(id) => body_of(world, id.entity(world)?),
        None => {
            let pick = match rules {
                Some(def) => pick_for_body(world, me, def),
                None => nearest_enemy(world, me, here, CombatTuning::of(world.resource::<ContentDb>()).lock_on_range),
            };
            let pick = pick.ok_or("nothing to lock on to in sight; pass `target` with `point`")?;
            body_of(world, world.resource::<AiIndex>().get(pick).ok_or("the pick is gone")?)
        }
    };
    if body == me {
        return Err("you cannot lock on to yourself".into());
    }
    let part = part_of(world, body, point)?;
    let id = *world.get::<AiId>(part).ok_or("the part has no id")?;
    if let Some(def) = rules {
        may_lock_on(world, me, part, id, def)?;
    }
    Ok(id)
}

/// ` (6.2m N)`: where `target` is from `here`, or nothing when it has no position.
fn where_is(world: &World, here: Vec3, target: AiId) -> String {
    let there = world.resource::<AiIndex>().get(target).and_then(|e| world.get::<Transform>(e)).map(|t| t.translation);
    there.map_or(String::new(), |p| format!(" ({})", relative(here, p)))
}

/// The nearest living hostile fighter within `range`, lowest id on ties.
fn nearest_enemy(world: &World, me: Entity, here: Vec3, range: f32) -> Option<AiId> {
    let team = world.get::<Team>(me);
    let mut q =
        world.try_query_filtered::<(Entity, &AiId, &Transform, Option<&Team>), (With<Fighter>, Standing)>()?;
    q.iter(world)
        .filter(|(e, _, _, t)| *e != me && hostile(team, *t))
        .map(|(_, id, tf, _)| (flat(tf.translation - here).length(), *id))
        .filter(|(d, _)| *d <= range)
        .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
        .map(|(_, id)| id)
}

/// Attack a target: walk into reach, face it (a lock-on to another enemy moves to it)
/// and use `input` (default "light") when it can start; with `repeat`, keep at it
/// (combos included) until it dies. Replaces your orders. Refused when waiting cannot
/// start the attack (a raised guard); dropped, with the reason in the journal, if that
/// happens later. Check progress with `observe` or `step`.
#[derive(Deserialize, JsonSchema)]
pub struct AttackArgs {
    pub target: Id,
    /// The moveset input to attack with, e.g. "light" or "heavy". Default "light".
    pub input: Option<String>,
    /// Keep attacking until it dies.
    #[serde(default)]
    pub repeat: bool,
}

fn attack(In(call): In<Call<AttackArgs>>, world: &mut World) -> ToolResult {
    let me = me(world, &call.caller)?;
    let target = call.args.target.entity(world)?;
    if target == me {
        return Err("you cannot attack yourself".into());
    }
    if world.get::<Dead>(target).is_some() {
        return Err(format!("{} is already dead", call.args.target));
    }
    if world.get::<Downed>(target).is_some() {
        return Err(format!("{} is down: hits pass over a downed body; attack a standing one", call.args.target));
    }
    // An AI-visible entity need not be in the world (a director, a quest): the order would be
    // dropped by `follow_attack_orders` on the next tick, which walks to and faces a position.
    if world.get::<Transform>(target).is_none() {
        return Err(format!(
            "{} has no position, so there is nothing to walk to and hit; attack a body that stands in the world",
            call.args.target
        ));
    }
    let input = call.args.input.unwrap_or_else(|| LIGHT.to_owned());
    // An order that waiting cannot start (no such input, a raised guard, no charges)
    // would sit there for ever: refuse it now, with the reason. On a client's own body, by
    // the action shown there.
    let dt = world.resource::<Time<Fixed>>().timestep().as_secs_f32();
    let shown = shown_state(world, me);
    let mut view = FighterView::of(world, me).ok_or("your body is not a complete fighter")?;
    if let Some(state) = &shown {
        view.state = state;
    }
    if let Err(blocker) = can_start(&view, &input, dt)
        && blocker.ready_in().is_none()
    {
        return Err(blocker.message());
    }
    let mut intent = world.get_mut::<Intent>(me).ok_or("no Intent")?;
    intent.orders.clear();
    intent.orders.push_back(Order::Attack {
        target: call.args.target.0,
        input: input.clone(),
        repeat: call.args.repeat,
    });
    let until = if call.args.repeat { " until it dies" } else { "" };
    Ok(format!("ok: attacking {} with {input}{until}", call.args.target))
}

/// Who a dev tool is about: `id`, or your own body.
fn dev_target(world: &World, caller: &Caller, id: Option<Id>) -> Result<Entity, String> {
    match id {
        Some(id) => id.entity(world),
        None => caller.require_subject(),
    }
}

/// God mode: hits do nothing to the entity (default: you).
#[derive(Deserialize, JsonSchema, Default)]
pub struct GodArgs {
    pub id: Option<Id>,
    /// false ends it. Default true.
    pub on: Option<bool>,
}

fn god(In(call): In<Call<GodArgs>>, world: &mut World) -> ToolResult {
    let e = dev_target(world, &call.caller, call.args.id)?;
    let who = label(world, e);
    if call.args.on.unwrap_or(true) {
        world.entity_mut(e).insert(Invulnerable);
        Ok(format!("{who} is invulnerable"))
    } else {
        world.entity_mut(e).remove::<Invulnerable>();
        Ok(format!("{who} can be hurt again"))
    }
}

/// Full health, stamina and poise; revives the dead. Default: you.
#[derive(Deserialize, JsonSchema, Default)]
pub struct HealArgs {
    pub id: Option<Id>,
}

fn heal(In(call): In<Call<HealArgs>>, world: &mut World) -> ToolResult {
    let e = dev_target(world, &call.caller, call.args.id)?;
    let who = label(world, e);
    let before = world.get::<Health>(e).ok_or_else(|| format!("{who} has no Health"))?.current;
    // A downed body stands up first (its pose is cut), then is healed in full like any other.
    let stood = revive_downed(world, e, 1.0, 0.0, None);
    let mut health = world.get_mut::<Health>(e).ok_or_else(|| format!("{who} has no Health"))?;
    let gained = health.max - before;
    health.current = health.max;
    let hp = health.current;
    if let Some(mut s) = world.get_mut::<Stamina>(e) {
        s.current = s.max;
    }
    if let Some(mut p) = world.get_mut::<Poise>(e) {
        p.refill();
    }
    let revived = revive(world, e) || stood;
    world.trigger(Healed { entity: e, amount: gained, hp });
    Ok(format!("healed {who} to {hp:.0}{}", if revived { ", revived" } else { "" }))
}

/// Bring a fighter down now, as a lethal hook's `Down` does: health 0, hits ignored, no actions, its
/// moveset's `downed` pose; it bleeds out (dies) after `bleed` seconds unless it is revived first
/// (a partner's revive, or `heal`). Without `bleed` it stays down until revived.
#[derive(Deserialize, JsonSchema)]
pub struct DownArgs {
    pub id: Id,
    /// Seconds before it bleeds out. Default: none (down until revived).
    pub bleed: Option<f32>,
}

fn down_tool(In(call): In<Call<DownArgs>>, world: &mut World) -> ToolResult {
    let e = call.args.id.entity(world)?;
    let id = call.args.id;
    if world.get::<Fighter>(e).is_none() || world.get::<Health>(e).is_none() {
        return Err(format!("{id} is not a fighter with Health"));
    }
    if world.get::<Dead>(e).is_some() {
        return Err(format!("{id} is dead; `heal` it first"));
    }
    if world.get::<Downed>(e).is_some() {
        return Err(format!("{id} is down already"));
    }
    if let Some(b) = call.args.bleed
        && !(b >= 0.0 && b.is_finite())
    {
        return Err(format!("bleed {b} must be a number of seconds >= 0; leave it out to stay down until revived"));
    }
    if !crate::combat::go_down(world, e, call.args.bleed, None) {
        return Err(format!("{id} cannot go down"));
    }
    Ok(match call.args.bleed {
        Some(b) => format!("{id} is down (bleeds out in {b:.1}s)"),
        None => format!("{id} is down"),
    })
}

/// Interact: press (or hold) the interact button. It starts an action only where the moveset binds
/// one to `interact`, usually a context binding that holds only next to the right body (a
/// revive: a downed ally within reach); that action goes on only while the button is
/// held: `hold=true` keeps it held, `hold=false` lets go. Answers `ok`, `ok: queued` or why not.
fn interact(In(call): In<Call<HoldArgs>>, world: &mut World) -> ToolResult {
    press_held(world, &call.caller, INTERACT, None, "", call.args.hold)
}

/// Grant `id` (default: you) a barrier that takes damage before health: the larger of its pool and
/// `amount`, for `seconds`. `amount=0` removes it.
#[derive(Deserialize, JsonSchema, Default)]
pub struct BarrierArgs {
    pub id: Option<Id>,
    /// Damage it absorbs (health points, not a share). 0 removes the barrier.
    pub amount: f32,
    /// Seconds it lasts. Required unless `amount` is 0.
    pub seconds: Option<f32>,
}

fn barrier(In(call): In<Call<BarrierArgs>>, world: &mut World) -> ToolResult {
    let e = dev_target(world, &call.caller, call.args.id)?;
    let who = label(world, e);
    let amount = call.args.amount;
    if !(amount >= 0.0 && amount.is_finite()) {
        return Err(format!("amount {amount} must be a number >= 0 (0 removes the barrier)"));
    }
    if amount == 0.0 {
        clear_barrier(world, e);
        return Ok(format!("{who} has no barrier"));
    }
    let seconds = call.args.seconds.ok_or("seconds is required with an amount above 0 (barrier id=e5 amount=20 seconds=8)")?;
    if !(seconds > 0.0 && seconds.is_finite()) {
        return Err(format!("seconds {seconds} must be a number above 0"));
    }
    if world.get::<Dead>(e).is_some() {
        return Err(format!("{who} is dead; a barrier is not granted to the dead"));
    }
    if world.get::<Health>(e).is_none() {
        return Err(format!("{who} has no Health; a barrier only guards health (pick a fighter)"));
    }
    grant_barrier(world, e, amount, seconds);
    let b = world.get::<Barrier>(e).copied().unwrap_or_default();
    Ok(format!("{who} has a barrier of {:.0} for {:.1}s", b.amount, b.left))
}

/// Set a named multiplier on the damage `id` (default: you) takes. The key is replaced; the product of
/// all keys multiplies every hit.
#[derive(Deserialize, JsonSchema, Default)]
pub struct DamageTakenArgs {
    pub id: Option<Id>,
    /// The multiplier's name (`core`).
    pub key: String,
    /// The multiplier (1.35: a third more damage). Required unless `clear` is true.
    pub mult: Option<f32>,
    /// Seconds it lasts; none: until cleared.
    pub seconds: Option<f32>,
    /// true removes the key instead.
    pub clear: Option<bool>,
    /// true keeps the later end of a window the key already holds, and a permanent set never cuts a
    /// timed one short (`extend_damage_taken`). Default false: the key is replaced.
    pub keep_later: Option<bool>,
}

fn damage_taken(In(call): In<Call<DamageTakenArgs>>, world: &mut World) -> ToolResult {
    let e = dev_target(world, &call.caller, call.args.id)?;
    let who = label(world, e);
    let key = call.args.key.as_str();
    if key.is_empty() {
        return Err("key must be a name, e.g. key=core".into());
    }
    if call.args.clear.unwrap_or(false) {
        clear_damage_taken(world, e, key);
        return Ok(format!("{who} takes no `{key}` multiplier"));
    }
    let mult = call.args.mult.ok_or("mult is required unless clear=true (damage_taken key=core mult=1.35 seconds=3)")?;
    if !(mult >= 0.0 && mult.is_finite()) {
        return Err(format!("mult {mult} must be a number >= 0"));
    }
    if let Some(s) = call.args.seconds
        && !(s > 0.0 && s.is_finite())
    {
        return Err(format!("seconds {s} must be a number above 0 (clear=true removes the key)"));
    }
    let changed = if call.args.keep_later.unwrap_or(false) {
        extend_damage_taken(world, e, key, mult, call.args.seconds)
    } else {
        set_damage_taken(world, e, key, mult, call.args.seconds);
        true
    };
    let now = world.get::<DamageTaken>(e).map(describe_damage_taken).unwrap_or_default();
    Ok(format!("{who} {}: {now}", if changed { "now" } else { "unchanged, the timed window runs on" }))
}

/// Stagger a fighter now (its moveset's `stagger` action). `poise=true` breaks its poise too, as
/// a poise break does (the poise is 0 and broken for the stagger's length): a broken fighter is
/// one a finisher can take.
#[derive(Deserialize, JsonSchema)]
pub struct StaggerArgs {
    pub id: Id,
    /// Break the poise as well. Default false (the stagger action only).
    pub poise: Option<bool>,
}

fn stagger_tool(In(call): In<Call<StaggerArgs>>, world: &mut World) -> ToolResult {
    let e = call.args.id.entity(world)?;
    let moveset = world
        .get::<Fighter>(e)
        .map(|f| f.moveset.clone())
        .ok_or_else(|| format!("{} is not a fighter", call.args.id))?;
    if world.get::<Dead>(e).is_some() {
        return Err(format!("{} is dead", call.args.id));
    }
    if world.get::<Downed>(e).is_some() {
        return Err(format!("{} is down; `heal` stands it up first", call.args.id));
    }
    let length = world
        .resource::<ContentDb>()
        .get::<MovesetDef>(&moveset)
        .and_then(|m| m.stagger.as_ref())
        .and_then(|a| world.resource::<ContentDb>().get::<ActionDef>(a))
        .map(ActionDef::length);
    let breaks = call.args.poise == Some(true);
    if breaks && world.get::<Poise>(e).is_none() {
        return Err(format!("{} has no Poise to break", call.args.id));
    }
    if !stagger(world, e, StaggerCause::Dev) {
        return Err(format!("{} cannot stagger: its moveset `{moveset}` has no `stagger` action", call.args.id));
    }
    if breaks
        && let (Some(len), Some(mut poise)) = (length, world.get_mut::<Poise>(e))
    {
        (poise.current, poise.broken, poise.since_hit) = (0.0, len.max(EPS), 0.0);
        return Ok(format!("{} staggered, poise broken {len:.1}s", call.args.id));
    }
    Ok(format!("{} staggered", call.args.id))
}

/// Force a reaction on a fighter, as a hit of that tier would: ignoring its armour and its
/// immunity (it starts none), pushed backwards from where it faces.
#[derive(Deserialize, JsonSchema)]
pub struct ReactArgs {
    pub id: Id,
    /// 1 light, 2 heavy, 3 knockdown.
    pub tier: u8,
}

fn react_tool(In(call): In<Call<ReactArgs>>, world: &mut World) -> ToolResult {
    let e = call.args.id.entity(world)?;
    let tier = call.args.tier;
    if !(1..=3).contains(&tier) {
        return Err(format!("tier {tier} is not a reaction; use 1 (light), 2 (heavy) or 3 (knockdown)"));
    }
    let moveset = world
        .get::<Fighter>(e)
        .map(|f| f.moveset.clone())
        .ok_or_else(|| format!("{} is not a fighter", call.args.id))?;
    if world.get::<Dead>(e).is_some() {
        return Err(format!("{} is dead", call.args.id));
    }
    if world.get::<Downed>(e).is_some() {
        return Err(format!("{} is down; `heal` stands it up first", call.args.id));
    }
    let back = world.get::<Transform>(e).map_or(Vec3::Z, |t| -flat(*t.forward()));
    if !react(world, e, Reaction::forced(tier, None, back)) {
        return Err(format!(
            "{} cannot react: it is in a stagger or a death, or its moveset `{moveset}` has no reaction or `hit` action",
            call.args.id
        ));
    }
    let word = ["light", "heavy", "knockdown"][usize::from(tier) - 1];
    Ok(format!("ok: {} reacts {word}", call.args.id))
}

/// Make a fighter remember something now, as its moveset's `remember` would (`parried`,
/// `dodged`): a context binding that needs it then holds. `seconds=0` forgets it.
#[derive(Deserialize, JsonSchema)]
pub struct RememberArgs {
    pub id: Id,
    pub key: String,
    /// How long (s). Default: the moveset's own time for the key.
    pub seconds: Option<f32>,
}

/// The longest a developer can make a fighter remember (s): a memory is a few seconds, and one that
/// outlasts any session is a typo.
const MAX_REMEMBER_SECONDS: f32 = 600.0;

fn remember_tool(In(call): In<Call<RememberArgs>>, world: &mut World) -> ToolResult {
    let e = call.args.id.entity(world)?;
    let RememberArgs { id, key, seconds } = call.args;
    let moveset = world.get::<Fighter>(e).map(|f| f.moveset.clone()).ok_or_else(|| format!("{id} is not a fighter"))?;
    let def = world.resource::<ContentDb>().get::<MovesetDef>(&moveset).and_then(|m| m.remember.iter().find(|r| r.key == key)).cloned();
    let seconds = match (seconds, &def) {
        (Some(s), _) if s.is_finite() && (0.0..=MAX_REMEMBER_SECONDS).contains(&s) => s,
        (Some(s), _) => {
            return Err(format!("seconds={s} is not a time; give a number from 0 to {MAX_REMEMBER_SECONDS} (0 forgets)"));
        }
        (None, Some(d)) => d.seconds,
        (None, None) => return Err(format!("the moveset `{moveset}` of {id} remembers no `{key}`, so it has no time for it; give seconds=")),
    };
    if seconds <= EPS {
        return Ok(if forget(world, e, &key) { format!("{id} forgot {key}") } else { format!("{id} did not remember {key}") });
    }
    remember(world, e, &key, seconds);
    let note = if def.is_none() { " (its moveset does not remember it: no binding reads it)" } else { "" };
    Ok(format!("{id} remembers {key} {seconds:.2}s{note}"))
}

/// `step --until named:reacting`: some fighter runs one of its moveset's reaction actions.
fn reacting(world: &World) -> bool {
    let db = world.resource::<ContentDb>();
    let Some(mut fighters) = world.try_query::<(&Fighter, &ActionState)>() else { return false };
    fighters.iter(world).any(|(fighter, state)| {
        let (Some(run), Some(m)) = (&state.current, db.get::<crate::MovesetDef>(&fighter.moveset)) else { return false };
        m.reactions.iter().any(|r| r.action == run.action)
    })
}
