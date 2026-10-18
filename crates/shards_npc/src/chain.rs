//! Combo continuation: an NPC continues an attack the way a player continues a combo.
//!
//! An option of an attack plan may carry a [`ChainDef`]: when its action starts, the NPC rolls
//! the odds from its own `SimRng` stream (salt `CHAIN`) and, on success, keeps an [`NpcChain`].
//! At the start of the action's recovery ([`recovery_start`]: the phase named `recovery`, else
//! the last one) the NPC presses the chain's input into `Intent`, like anyone: the press goes
//! into the input queue, with the queue's buffer life, and takes effect only if the action has
//! a `Combo` cancel window that lets the input through before the press dies. A window that
//! opens later than the buffer life is a chain that never fires, as the content says.
//!
//! The continuation is an action start in its own right: it rolls the odds written on the
//! option that names *its* action (an option with `action: "swing_2"` and `weight: 0.0` is the
//! link of a continuation no moveset input starts; the planner never picks it).
//!
//! The press is a strike for the director's spacing: if the director spaces its strikes and
//! another NPC began one too recently, the continuation is *refused*. Nothing is pressed and
//! the chain ends.

use ai_core::{AiId, SimRng, SimTick};
use bevy::prelude::*;
use shards_action::def::CancelTo;
use shards_action::{can_start, ActionDef, FighterView, MovesetDef};
use shards_content::ContentDb;

use crate::brain::{salt, Npc};
use crate::def::{AttackPlanDef, ChainDef, DirectorDef};
use crate::director::{order, Fighting};
use crate::planner::NpcOptions;
use crate::strike::{chain_note, note_list, pool, spacing_ticks, strike_pressed, tags_for, waiting};

/// An NPC's pending combo continuation: gameplay state, kept in replays and snapshots. Only an
/// NPC whose attack rolled a chain carries it, from the roll to the end of the attack.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct NpcChain {
    /// The moveset input it presses.
    pub input: String,
    /// The action instance it belongs to (`Running::instance`): the chain dies with the attack.
    pub instance: u32,
    /// The tick the press was made (the continuation is on its way, or the press died unused).
    pub pressed: Option<u64>,
    /// The continuation is a strike for the director's spacing: until it starts, the press
    /// counts as a start of the pressed tick for the other NPCs.
    pub strike: bool,
    /// The spacing is asked when the action machine would take the queued input into the combo
    /// window ([`ChainDef::gate_at_start`]), not when it is pressed.
    pub at_start: bool,
    /// The tick the spacing let such a continuation through (it starts that tick).
    pub cleared: Option<u64>,
}

/// An NPC pressed the input of its combo continuation (journaled: `e7 chains light after
/// swing_1`).
#[derive(Event, Clone, Debug, PartialEq)]
pub struct ChainPressed {
    pub entity: Entity,
    /// The action it continues.
    pub after: String,
    pub input: String,
}

/// A combo continuation was refused because another NPC of the director began a strike too
/// recently; the chain ends (journaled: `e7 chain refused after swing_1: e8 began a strike
/// 12 ticks ago`).
#[derive(Event, Clone, Debug, PartialEq)]
pub struct ChainRefused {
    pub entity: Entity,
    pub after: String,
    pub input: String,
    /// The NPC whose strike was in the way, and the ticks of the spacing still to run.
    pub by: AiId,
    pub ticks_left: u64,
    /// The spacing, in ticks.
    pub spacing: u64,
}

/// When the recovery of `def` starts (seconds into the action): its phase named `recovery`,
/// else its last phase.
pub fn recovery_start(def: &ActionDef) -> f32 {
    def.phase_span("recovery")
        .or_else(|| def.phases.last().and_then(|p| def.phase_span(&p.0)))
        .map_or(0.0, |(from, _)| from)
}

/// The chain `npc` rolls when `action` starts: the odds are those of the first option of its
/// plan that has a chain, is open (an option with `when` counts only while the NPC's
/// [`NpcOptions`] lists it) and names the action (by `action`, or by the action of its `input`).
pub(crate) fn definition<'a>(db: &'a ContentDb, npc: &Npc, open: Option<&NpcOptions>, moveset: &str, action: &str) -> Option<&'a ChainDef> {
    let plan = npc.plan.as_deref().and_then(|p| db.get::<AttackPlanDef>(p))?;
    let moveset = db.get::<MovesetDef>(moveset);
    plan.options.iter().find_map(|o| {
        let chain = o.chain.as_ref()?;
        // An option with `when` rolls only while it is open, so one action can carry different odds.
        if !o.when.is_empty() && !open.is_some_and(|options| options.allowed.iter().any(|a| a == o.label())) {
            return None;
        }
        let named = o.action.as_deref().or_else(|| o.input.as_deref().and_then(|i| moveset?.action(i)))?;
        (named == action).then_some(chain)
    })
}

/// The roll when `action` starts: a chain, if the option has one and the die falls its way.
#[allow(clippy::too_many_arguments, reason = "the roll reads the die, the NPC, its open options and the action that began")]
pub(crate) fn roll(db: &ContentDb, rng: &SimRng, id: AiId, npc: &Npc, open: Option<&NpcOptions>, moveset: &str, action: &str, instance: u32) -> Option<NpcChain> {
    let chain = definition(db, npc, open, moveset, action)?;
    rng.stream(id, salt::CHAIN).chance(chain.chance).then(|| NpcChain { input: chain.input.clone(), instance, pressed: None, strike: false, at_start: chain.gate_at_start, cleared: None })
}

/// `NpcSystems::Plan`, before the planner: every NPC with a chain whose action has reached its
/// recovery presses the continuation, unless the spacing refuses it. A chain whose attack is
/// over, cut short by a forced action or taken over by orders is dropped.
pub(crate) fn chain_press(
    mut commands: Commands,
    time: Res<Time>,
    tick: Res<SimTick>,
    db: Res<ContentDb>,
    mut npcs: Query<Fighting>,
) {
    let dt = time.delta_secs();
    let mut notes = note_list(npcs.iter().map(|f| (*f.id, f.npc.director.as_str(), f.strike, f.token.held.as_ref(), chain_note(f.chain, Some(f.buffer), tick.0))));
    // Presses already made this tick (a hook's, a continuation's) are starts of this tick.
    notes.extend(
        npcs.iter()
            .filter(|f| strike_pressed(&db, f, dt))
            .map(|f| (*f.id, pool(&f.npc.director), tick.0)),
    );
    for (_, entity) in order(&npcs) {
        let Ok(mut f) = npcs.get_mut(entity) else { continue };
        let Some(chain) = f.chain.as_deref().cloned() else { continue };
        let running = f.state.current.as_ref().filter(|r| r.instance == chain.instance && !r.forced);
        let Some(run) = running.filter(|_| !f.dead && f.brain.active && !f.under_orders()) else {
            commands.entity(entity).remove::<NpcChain>();
            continue;
        };
        if chain.pressed.is_some() {
            if chain.at_start && chain.strike {
                gate_buffered(&mut f, &chain, &mut notes, &mut commands, &db, tick.0, dt);
            }
            continue;
        }
        let Some(def) = db.get::<ActionDef>(&run.action) else {
            commands.entity(entity).remove::<NpcChain>();
            continue;
        };
        if run.t + 1e-4 < recovery_start(def) {
            continue;
        }
        let after = run.action.clone();
        // What the press turns into: the combo link of this action, else the moveset's action.
        let next = match def.next_cancel(&chain.input, -1.0) {
            Some((_, CancelTo::Action(next))) => Some(next),
            _ => db.get::<MovesetDef>(&f.fighter.moveset).and_then(|m| m.action(&chain.input)).map(str::to_owned),
        };
        let director = DirectorDef::of(&db, &f.npc.director);
        let strike = next.as_deref().is_none_or(|a| director.is_strike(&tags_for(&db, f.npc, &f.fighter.moveset, a)));
        let spacing = spacing_ticks(director.min_spacing, dt);
        let pool = pool(&f.npc.director);
        // A continuation gated at its start is asked only if the machine takes it this very tick
        // (the combo window is already open); else it is asked when it would start.
        let takes_now = chain.at_start && takes_input(&f, &db, &chain.input, dt);
        let asked = strike && (!chain.at_start || takes_now);
        if asked && let Some((by, ticks_left)) = waiting(&notes, *f.id, &pool, tick.0, spacing) {
            commands.entity(entity).remove::<NpcChain>();
            commands.trigger(ChainRefused { entity, after, input: chain.input, by, ticks_left, spacing });
            continue;
        }
        f.drive.press.push(chain.input.clone());
        if let Some(c) = f.chain.as_mut() {
            c.pressed = Some(tick.0);
            c.strike = strike;
            c.cleared = (asked && chain.at_start).then_some(tick.0);
        }
        if asked {
            notes.push((*f.id, pool, tick.0));
        }
        commands.trigger(ChainPressed { entity, after, input: chain.input });
    }
}

/// Whether the action machine would take `input` this tick.
fn takes_input(f: &crate::director::FightingItem, db: &ContentDb, input: &str, dt: f32) -> bool {
    let view = FighterView {
        db,
        moveset: &f.fighter.moveset,
        state: f.state,
        stamina: f.stamina,
        cooldowns: f.cooldowns,
        charges: f.charges,
        held: &f.drive.held,
        dead: f.dead,
    };
    can_start(&view, input, dt).is_ok()
}

/// A gated continuation ([`ChainDef::gate_at_start`]) waits in the input queue. On the tick the
/// machine would take it, the spacing is asked, as the design asks it for every attack start:
/// refused, the input is withdrawn from the queue and the chain ends; allowed, it counts as a
/// start of this tick for the planners that come after.
fn gate_buffered(
    f: &mut crate::director::FightingItem,
    chain: &NpcChain,
    notes: &mut Vec<crate::strike::Note>,
    commands: &mut Commands,
    db: &ContentDb,
    now: u64,
    dt: f32,
) {
    if !f.buffer.queued.iter().any(|q| q.input == chain.input) || !takes_input(f, db, &chain.input, dt) {
        return;
    }
    let director = DirectorDef::of(db, &f.npc.director);
    let spacing = spacing_ticks(director.min_spacing, dt);
    let pool = pool(&f.npc.director);
    if let Some((by, ticks_left)) = waiting(notes, *f.id, &pool, now, spacing) {
        f.buffer.queued.retain(|q| q.input != chain.input);
        let after = f.state.current.as_ref().map_or_else(String::new, |r| r.action.clone());
        commands.entity(f.entity).remove::<NpcChain>();
        commands.trigger(ChainRefused { entity: f.entity, after, input: chain.input.clone(), by, ticks_left, spacing });
        return;
    }
    if let Some(c) = f.chain.as_mut() {
        c.cleared = Some(now);
    }
    notes.push((*f.id, pool, now));
}
