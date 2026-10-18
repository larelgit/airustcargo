//! Strike spacing: NPCs of one director do not start strikes less than
//! [`DirectorDef::min_spacing`] seconds apart (two nearly simultaneous blows cannot both be
//! parried), except for the actions the director exempts by tag
//! ([`DirectorDef::spacing_exempt_tags`]: a retreat, a buff, a repositioning).
//!
//! - **Noting.** Every strike an NPC of such a director starts is noted on the NPC as an
//!   [`NpcStrike`], at the tick its action began: planner presses, combo continuations and
//!   whatever a state hook pressed itself. An exempt action is not noted. An NPC of a
//!   director without a `min_spacing` never gets the component.
//! - **Waiting.** A strike is *spaced* when no other NPC of the same director noted one
//!   within the last `ceil(min_spacing / tick)` ticks. The director grants no token for a
//!   strike that is not spaced, the planner does not press one, and a combo continuation that
//!   is not spaced is refused and ends its chain; options the director exempts are not
//!   held back. Presses that have not started yet count as started when they were pressed,
//!   and so do the presses of this tick, in `AiId` order.
//! - **Hooks.** A state hook that presses a strike itself asks [`NpcCtx::strike_ready`].
//!
//! The clock is the simulation's tick count, so the spacing is the same with a window and
//! without one and survives replays and snapshots (the notes are components of the NPCs).
//! Encounters are not told apart by distance yet: all NPCs of a director share one clock.

use ai_core::{AiId, SimRng, SimTick};
use bevy::prelude::*;
use shards_action::{can_start, ActionDef, ActionStarted, ActionState, Fighter, FighterView, InputBuffer};
use shards_content::ContentDb;
use shards_core::NetRole;

use crate::brain::{Npc, NpcCtx};
use crate::chain::NpcChain;
use crate::def::{AttackPlanDef, DirectorDef};
use crate::director::{AttackToken, Grant};
use crate::planner::{resolve, NpcOptions};

/// The last strike an NPC began, for the spacing of its director. Gameplay state: kept in
/// replays and snapshots. Only NPCs of a director with a `min_spacing` carry it.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct NpcStrike {
    /// The tick its action began.
    pub last_start: u64,
    /// The start went through the spacing rule (the planner pressed it, or it is a combo
    /// continuation that was checked): `npc.strike_spacing` holds it to the rule. A hook's
    /// own press is noted but not held to it.
    pub gated: bool,
    /// The director pool the NPC fought by when it began the strike. A note counts only while
    /// this is still the NPC's pool: an NPC moved to another director does not space the new
    /// pool's NPCs by a strike it began in the old one.
    pub pool: String,
}

/// Ticks in `secs` seconds of `dt`-long ticks, rounded up (with a hair of slack for the sum
/// of tick lengths): 0.7 s at 60 Hz is 42 ticks.
pub fn spacing_ticks(secs: f32, dt: f32) -> u64 {
    if secs <= 1e-4 || dt <= 0.0 {
        return 0;
    }
    ((secs - 1e-4) / dt).ceil() as u64
}

/// A note of one NPC's strike: who, the director's pool, the tick.
pub(crate) type Note = (AiId, String, u64);

/// The director an NPC fights by, as a pool key (`default` when it names none).
pub(crate) fn pool(director: &str) -> String {
    if director.is_empty() { DirectorDef::DEFAULT.to_owned() } else { director.to_owned() }
}

/// Whether `me` of `pool` may not start a strike at tick `now`, as the `notes` of others
/// say: who began one, and how many ticks of the `ticks` are left.
pub(crate) fn waiting(notes: &[Note], me: AiId, pool: &str, now: u64, ticks: u64) -> Option<(AiId, u64)> {
    if ticks == 0 {
        return None;
    }
    notes
        .iter()
        .filter(|(who, p, at)| *who != me && p == pool && now.saturating_sub(*at) < ticks)
        .map(|(who, _, at)| (*who, ticks - now.saturating_sub(*at)))
        .max_by_key(|(who, left)| (*left, std::cmp::Reverse(*who)))
}

/// Whether the plan of NPC `npc` never waits for the director's spacing
/// ([`AttackPlanDef::ignores_spacing`]): its starts are still noted for the others.
pub(crate) fn ignores_spacing(db: &ContentDb, npc: &Npc) -> bool {
    npc.plan.as_deref().and_then(|p| db.get::<AttackPlanDef>(p)).is_some_and(|p| p.ignores_spacing)
}

/// The notes in `world`: every noted strike, and every press that has not started yet
/// (counted from the tick it was pressed).
pub(crate) fn notes_of(world: &World) -> Vec<Note> {
    let Some(mut q) = world.try_query::<(&AiId, &Npc, Option<&NpcStrike>, Option<&AttackToken>, Option<&NpcChain>, Option<&InputBuffer>)>() else {
        return Vec::new();
    };
    let now = ai_core::sim::tick(world);
    note_list(q.iter(world).map(|(id, npc, strike, token, chain, buffer)| {
        (*id, npc.director.as_str(), strike, token.and_then(|t| t.held.as_ref()), chain_note(chain, buffer, now))
    }))
}

/// What [`note_list`] reads of an NPC: its id, its director, the last strike it noted, the
/// token it holds, the tick of its pending combo continuation ([`chain_note`]).
pub(crate) type NoteRow<'a> = (AiId, &'a str, Option<&'a NpcStrike>, Option<&'a Grant>, Option<u64>);

/// The tick a combo continuation was pressed, while it is still on its way: a press of this tick,
/// or one whose input still waits in the queue. A continuation gated at its start counts only on the tick it is let through. A press that died unused (the combo window did
/// not open within the buffer life) started nothing and is no note.
pub(crate) fn chain_note(chain: Option<&NpcChain>, buffer: Option<&InputBuffer>, now: u64) -> Option<u64> {
    let chain = chain.filter(|c| c.strike)?;
    // Gated at its start, the continuation is a start only on the tick the spacing let it through.
    if chain.at_start {
        return (chain.cleared == Some(now)).then_some(now);
    }
    let at = chain.pressed?;
    (at == now || buffer.is_some_and(|b| b.queued.iter().any(|q| q.input == chain.input))).then_some(at)
}

/// The notes of NPCs given as [`NoteRow`]s: their noted strikes, and the presses that have not
/// started yet (a planner press, a combo continuation that is a strike), from the tick pressed.
pub(crate) fn note_list<'a>(rows: impl Iterator<Item = NoteRow<'a>>) -> Vec<Note> {
    let mut notes: Vec<Note> = Vec::new();
    for (id, director, strike, grant, continuing) in rows {
        let pending = grant.filter(|g| !g.started).and_then(|g| g.pressed);
        let pool = pool(director);
        let noted = strike.filter(|s| s.pool == pool).map(|s| s.last_start);
        for at in noted.into_iter().chain(pending).chain(continuing) {
            notes.push((id, pool.clone(), at));
        }
    }
    notes.sort();
    notes
}

/// How long NPC `entity` must still wait to start a (non-exempt) strike: the NPC that began
/// the last one in the way and the ticks left; `None`: it may strike now.
pub fn wait_of(world: &World, entity: Entity) -> Option<(AiId, u64)> {
    let npc = world.get::<Npc>(entity)?;
    let id = *world.get::<AiId>(entity)?;
    let db = world.get_resource::<ContentDb>()?;
    if ignores_spacing(db, npc) {
        return None;
    }
    let dt = world.get_resource::<Time<Fixed>>()?.timestep().as_secs_f32();
    let ticks = spacing_ticks(DirectorDef::of(db, &npc.director).min_spacing, dt);
    waiting(&notes_of(world), id, &pool(&npc.director), ai_core::sim::tick(world), ticks)
}

impl NpcCtx<'_> {
    /// Whether the NPC may start a strike now: no other NPC of its director began one less
    /// than the director's `min_spacing` ago. A state hook that presses a strike itself
    /// (through [`NpcOutput::press`](crate::NpcOutput::press)) asks first, as the planner
    /// does; an NPC of a director without a `min_spacing` is always ready.
    pub fn strike_ready(&self) -> bool {
        wait_of(self.world, self.entity).is_none()
    }
}

/// The tags a start of `action` by `npc` counts by: the action's own, and those of the plan
/// options that name it (their tags and their names).
pub(crate) fn tags_for(db: &ContentDb, npc: &Npc, moveset: &str, action: &str) -> Vec<String> {
    let mut tags: Vec<String> = db.get::<ActionDef>(action).map(|d| d.tags.clone()).unwrap_or_default();
    if let Some(plan) = npc.plan.as_deref().and_then(|p| db.get::<AttackPlanDef>(p)) {
        for r in resolve(plan, db, moveset, 0.4) {
            if r.action == action {
                tags.extend(r.tags);
            }
        }
    }
    tags
}

/// Whether the presses already in an NPC's `NpcDrive` this tick (a state hook's, a combo
/// continuation's) start a strike now of a director that spaces its strikes: a press the action
/// machine would take this tick (`can_start`), whose action is not exempt. The planner counts
/// them as starts of this tick, so it yields to them instead of starting a second strike on the
/// same tick. (The reverse cannot be: a hook decides before the planner, so a hook never sees
/// the planner's press of the tick.)
pub(crate) fn strike_pressed(db: &ContentDb, f: &crate::director::FightingReadOnlyItem, dt: f32) -> bool {
    if f.drive.press.is_empty() || f.intent.is_some_and(|i| !i.orders.is_empty()) {
        return false;
    }
    let director = DirectorDef::of(db, &f.npc.director);
    if director.min_spacing <= 0.0 {
        return false;
    }
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
    f.drive.press.iter().any(|input| match can_start(&view, input, dt) {
        Ok(start) => director.is_strike(&tags_for(db, f.npc, &f.fighter.moveset, &start.action)),
        Err(_) => false,
    })
}

/// An action began on an NPC: a strike of one whose director spaces its strikes is noted, and
/// the attack rolls its combo continuation ([`crate::chain`]). A forced action (a stagger)
/// is neither: it ends a chain.
#[allow(clippy::too_many_arguments, clippy::type_complexity, reason = "an observer: one parameter and one query per thing it reads")]
pub(crate) fn action_started(
    started: On<ActionStarted>,
    role: Option<Res<NetRole>>,
    tick: Res<SimTick>,
    rng: SimRng,
    db: Res<ContentDb>,
    npcs: Query<(&AiId, &Npc, &Fighter, &ActionState, Option<&AttackToken>, Option<&NpcChain>, Option<&NpcOptions>)>,
    mut commands: Commands,
) {
    let e = started.event();
    // A client is shown the host's world: the host's notes and chains arrive with it.
    if role.is_some_and(|r| !r.simulates()) || e.predicted {
        return;
    }
    let Ok((id, npc, fighter, state, token, chain, open)) = npcs.get(e.entity) else { return };
    if e.input.is_none() {
        if chain.is_some() {
            commands.entity(e.entity).remove::<NpcChain>();
        }
        return;
    }
    let director = DirectorDef::of(&db, &npc.director);
    if director.min_spacing > 0.0 && director.is_strike(&tags_for(&db, npc, &fighter.moveset, &e.action)) {
        let planned = token.and_then(|t| t.held.as_ref()).is_some_and(|g| g.pressed.is_some() && !g.started);
        let continued = chain.is_some_and(|c| c.pressed.is_some());
        commands.entity(e.entity).insert(NpcStrike { last_start: tick.0, gated: planned || continued, pool: pool(&npc.director) });
    }
    // The attack that began rolls its own continuation; the one it replaces is spent.
    let rolled = state.current.as_ref().and_then(|r| crate::chain::roll(&db, &rng, *id, npc, open, &fighter.moveset, &e.action, r.instance));
    match rolled {
        Some(next) => {
            commands.entity(e.entity).insert(next);
        }
        None if chain.is_some() => {
            commands.entity(e.entity).remove::<NpcChain>();
        }
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_spacing_is_a_whole_number_of_ticks() {
        let dt = 1.0 / 60.0;
        assert_eq!(spacing_ticks(0.7, dt), 42);
        assert_eq!(spacing_ticks(0.0, dt), 0);
        assert_eq!(spacing_ticks(0.1, dt), 6);
        assert_eq!(spacing_ticks(0.71, dt), 43);
    }

    #[test]
    fn only_others_of_the_same_director_within_the_spacing_make_it_wait() {
        let notes: Vec<Note> = vec![(AiId(5), "arena".into(), 100), (AiId(6), "other".into(), 120), (AiId(7), "arena".into(), 90)];
        // e5 began a strike at 100: e8 waits until tick 142 (42 ticks).
        assert_eq!(waiting(&notes, AiId(8), "arena", 101, 42), Some((AiId(5), 41)));
        assert_eq!(waiting(&notes, AiId(8), "arena", 141, 42), Some((AiId(5), 1)));
        assert_eq!(waiting(&notes, AiId(8), "arena", 142, 42), None);
        // Its own strike never holds it back; another director's does not either.
        assert_eq!(waiting(&notes, AiId(5), "arena", 101, 42), Some((AiId(7), 31)), "e7's, not its own");
        assert_eq!(waiting(&notes, AiId(8), "elsewhere", 101, 42), None);
        // No spacing, no waiting.
        assert_eq!(waiting(&notes, AiId(8), "arena", 101, 0), None);
    }
}
