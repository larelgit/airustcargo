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
use shards_core::input_names::{BLOCK, DODGE, HEAVY, LIGHT, PARRY, TECHNIQUE, USE_ITEM};
use shards_core::{flat, Intent, Order, Predicted};
use shards_motion::camera::{LockOnDef, Side};

use crate::combat::{react, revive, stagger, Reaction, StaggerCause};
use crate::def::{ActionDef, CombatTuning, MovesetDef, Steer, EPS};
use crate::fighter::{hostile, Dead, Fighter, Health, Invulnerable, Poise, Stamina, Team};
use crate::lock::{pick_for_body, switch_lock_of, switch_rules, take_lock};
use crate::machine::{can_start, fits_buffer, ActionState, FighterView, Healed};
use crate::predict::shown_state;
use crate::zone::{body_radius, zones_threatening};

pub(crate) fn register(app: &mut App) {
    app.add_ai_tool("light_attack", Scope::Player, light_attack)
        .add_ai_tool("heavy_attack", Scope::Player, heavy_attack)
        .add_ai_tool("parry", Scope::Player, parry)
        .add_ai_tool("block", Scope::Player, block)
        .add_ai_tool("dodge", Scope::Player, dodge)
        .add_ai_tool("use_technique", Scope::Player, use_technique)
        .add_ai_tool("use_item", Scope::Player, use_item)
        .add_ai_tool("lock_on", Scope::Player, lock_on)
        .add_ai_tool("attack", Scope::Player, attack)
        .add_ai_tool("god", Scope::Dev, god)
        .add_ai_tool("heal", Scope::Dev, heal)
        .add_ai_tool("stagger", Scope::Dev, stagger_tool)
        .add_ai_tool("react", Scope::Dev, react_tool)
        .add_ai_condition("reacting", reacting)
        .add_dev_entry("combat", "god", "Immortality on / off", DevCommand::Live(god_line))
        .add_dev_entry("combat", "heal", "Heal / revive", DevCommand::Run("heal".into()));
}

/// The menu's immortality switch: off when the caller's body has it.
fn god_line(world: &World, caller: &Caller) -> Result<String, String> {
    let me = caller.require_subject()?;
    Ok(ai_core::dev::toggle_line("god", world.get::<Invulnerable>(me).is_some()))
}

/// Your body, if it is a fighter you can control.
fn me(world: &World, caller: &Caller) -> Result<Entity, String> {
    let me = caller.require_subject()?;
    if world.get::<Fighter>(me).is_none() {
        return Err("your body cannot fight (it has no Fighter component)".into());
    }
    if world.get::<Intent>(me).is_none() {
        return Err("your body cannot be controlled (it has no Intent)".into());
    }
    if world.get::<Dead>(me).is_some() {
        return Err("you are dead; a dev agent can `heal` you".into());
    }
    Ok(me)
}

/// When a press will start: now (as this action), or in so many seconds.
enum When {
    Now(String),
    In(f32),
}

/// The shared rule's answer for `input`: now, within the input buffer, or the error. On a
/// client's own body it asks about the action shown there ([`shown_state`]).
fn when(world: &World, me: Entity, input: &str) -> Result<When, String> {
    let dt = world.resource::<Time<Fixed>>().timestep().as_secs_f32();
    let ttl = CombatTuning::of(world.resource::<ContentDb>()).input_buffer;
    let shown = shown_state(world, me);
    let mut view = FighterView::of(world, me).ok_or("your body is not a complete fighter")?;
    if let Some(state) = &shown {
        view.state = state;
    }
    match can_start(&view, input, dt) {
        Ok(start) => Ok(When::Now(start.action)),
        Err(blocker) => match blocker.ready_in() {
            Some(t) if fits_buffer(t, ttl) => Ok(When::In(t)),
            _ => Err(blocker.message()),
        },
    }
}

/// Presses `input` (towards `dir`) when the rule says it will start. `detail` follows the
/// answer (a dodge's direction).
fn press(world: &mut World, caller: &Caller, input: &str, dir: Option<Vec3>, detail: &str) -> ToolResult {
    let me = me(world, caller)?;
    let when = when(world, me, input)?;
    let note = predicted_note(world, me);
    let mut intent = world.get_mut::<Intent>(me).ok_or("no Intent")?;
    match dir {
        Some(dir) => intent.press_dir(input, dir),
        None => intent.press(input),
    }
    Ok(match when {
        When::Now(action) => format!("ok: {action}{detail}{note}"),
        When::In(t) => format!("ok: queued, {input} starts in {t:.2}s{detail}{note}"),
    })
}

/// ` · predicted, host confirms` on a client's own body: it starts here at once, and the
/// host has the last word.
fn predicted_note(world: &World, me: Entity) -> &'static str {
    if world.get::<Predicted>(me).is_some() { " · predicted, host confirms" } else { "" }
}

/// Light attack in the direction you face (or at your lock-on target). Chains into a
/// combo when pressed again inside the attack's cancel window. Answers `ok`, `ok:
/// queued` (it starts within the input buffer) or why not and when it would work.
fn light_attack(In(call): In<Call<NoArgs>>, world: &mut World) -> ToolResult {
    press(world, &call.caller, LIGHT, None, "")
}

/// Heavy attack: slower, costs more stamina, hits harder.
fn heavy_attack(In(call): In<Call<NoArgs>>, world: &mut World) -> ToolResult {
    press(world, &call.caller, HEAVY, None, "")
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
    press(world, &call.caller, &input, None, "")
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
#[derive(Deserialize, JsonSchema, Default)]
pub struct LockOnArgs {
    pub target: Option<Id>,
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
        world.get_mut::<Intent>(me).ok_or("no Intent")?.target = None;
        return Ok("ok: lock-on released".into());
    }
    // Without a `lock_on` definition: the nearest living enemy within the combat tuning's
    // range, and any target asked for by id (as before the lock-on rules).
    let rules = LockOnDef::active(world.get_resource::<ContentDb>()).cloned();
    if let Some(side) = call.args.side {
        if call.args.target.is_some() {
            return Err("pass `target` or `side`, not both: `side` moves the lock you hold".into());
        }
        let switch = switch_rules(world);
        return match switch_lock_of(world, me, side, &switch) {
            Ok((from, to)) => Ok(format!("ok: locked on {to} (switched {} from {from}){}", side.name(), where_is(world, here, to))),
            Err(None) => Err(format!("you hold no lock to move {}; lock on first (`lock_on`, or `lock_on target=eN`)", side.name())),
            Err(Some(from)) => Err(format!(
                "nobody you can see to the {} of {from} within {:.0}m; the lock stays on {from}",
                side.name(),
                switch.range
            )),
        };
    }
    let target = match (call.args.target, &rules) {
        (Some(id), rules) => {
            let e = id.entity(world)?;
            if e == me {
                return Err("you cannot lock on to yourself".into());
            }
            if let Some(def) = rules {
                // What the check would take away at once is refused here.
                if world.get::<Dead>(e).is_some() {
                    return Err(format!("{} is dead; lock on to a living target", id.0));
                }
                let distance = world.get::<Transform>(e).map_or(0.0, |t| t.translation.distance(here));
                if distance > def.range {
                    return Err(format!("{} is {distance:.1}m away, beyond the lock-on range of {:.0}m; come closer", id.0, def.range));
                }
            }
            id.0
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
    Ok(format!("ok: locked on {target}{}", where_is(world, here, target)))
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
        world.try_query_filtered::<(Entity, &AiId, &Transform, Option<&Team>), (With<Fighter>, Without<Dead>)>()?;
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
    let mut health = world.get_mut::<Health>(e).ok_or_else(|| format!("{who} has no Health"))?;
    let gained = health.max - health.current;
    health.current = health.max;
    let hp = health.current;
    if let Some(mut s) = world.get_mut::<Stamina>(e) {
        s.current = s.max;
    }
    if let Some(mut p) = world.get_mut::<Poise>(e) {
        p.refill();
    }
    let revived = revive(world, e);
    world.trigger(Healed { entity: e, amount: gained, hp });
    Ok(format!("healed {who} to {hp:.0}{}", if revived { ", revived" } else { "" }))
}

/// Stagger a fighter now (its moveset's `stagger` action), as a poise break would.
#[derive(Deserialize, JsonSchema)]
pub struct StaggerArgs {
    pub id: Id,
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
    if !stagger(world, e, StaggerCause::Dev) {
        return Err(format!("{} cannot stagger: its moveset `{moveset}` has no `stagger` action", call.args.id));
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

/// `step --until named:reacting`: some fighter runs one of its moveset's reaction actions.
fn reacting(world: &World) -> bool {
    let db = world.resource::<ContentDb>();
    let Some(mut fighters) = world.try_query::<(&Fighter, &ActionState)>() else { return false };
    fighters.iter(world).any(|(fighter, state)| {
        let (Some(run), Some(m)) = (&state.current, db.get::<crate::MovesetDef>(&fighter.moveset)) else { return false };
        m.reactions.iter().any(|r| r.action == run.action)
    })
}
