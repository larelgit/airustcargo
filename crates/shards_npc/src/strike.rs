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
use shards_action::{ActionDef, ActionStarted, ActionState, Fighter};
use shards_content::ContentDb;
use shards_core::NetRole;

use crate::brain::{Npc, NpcCtx};
use crate::chain::NpcChain;
use crate::def::{AttackPlanDef, DirectorDef};
use crate::director::{AttackToken, Grant};
use crate::planner::resolve;

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

/// The notes in `world`: every noted strike, and every press that has not started yet
/// (counted from the tick it was pressed).
pub(crate) fn notes_of(world: &World) -> Vec<Note> {
    let Some(mut q) = world.try_query::<(&AiId, &Npc, Option<&NpcStrike>, Option<&AttackToken>)>() else { return Vec::new() };
    note_list(q.iter(world).map(|(id, npc, strike, token)| (*id, npc.director.as_str(), strike, token.and_then(|t| t.held.as_ref()))))
}

/// The notes of NPCs given as `(id, director, last strike, token held)`.
pub(crate) fn note_list<'a>(rows: impl Iterator<Item = (AiId, &'a str, Option<&'a NpcStrike>, Option<&'a Grant>)>) -> Vec<Note> {
    let mut notes: Vec<Note> = Vec::new();
    for (id, director, strike, grant) in rows {
        let pending = grant.filter(|g| !g.started).and_then(|g| g.pressed);
        for at in strike.map(|s| s.last_start).into_iter().chain(pending) {
            notes.push((id, pool(director), at));
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
    npcs: Query<(&AiId, &Npc, &Fighter, &ActionState, Option<&AttackToken>, Option<&NpcChain>)>,
    mut commands: Commands,
) {
    let e = started.event();
    // A client is shown the host's world: the host's notes and chains arrive with it.
    if role.is_some_and(|r| !r.simulates()) || e.predicted {
        return;
    }
    let Ok((id, npc, fighter, state, token, chain)) = npcs.get(e.entity) else { return };
    if e.input.is_none() {
        if chain.is_some() {
            commands.entity(e.entity).remove::<NpcChain>();
        }
        return;
    }
    let director = DirectorDef::of(&db, &npc.director);
    if director.min_spacing > 0.0 && director.is_strike(&tags_for(&db, npc, &fighter.moveset, &e.action)) {
        let planned = token.and_then(|t| t.held.as_ref()).is_some_and(|g| g.pressed.is_some() && !g.started);
        let continued = chain.is_some_and(|c| c.pressed);
        commands.entity(e.entity).insert(NpcStrike { last_start: tick.0, gated: planned || continued });
    }
    // The attack that began rolls its own continuation; the one it replaces is spent.
    let rolled = state.current.as_ref().and_then(|r| crate::chain::roll(&db, &rng, *id, npc, &fighter.moveset, &e.action, r.instance));
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
