//! Sequences (F12): scripted scenes for NPCs, as content.
//!
//! A `sequence` ([`SequenceDef`], `*.sequence.ron`) names **roles** and **steps**: `MoveTo(marker)`,
//! `Face(marker or role)`, `Play(action)`, `Sound(id)`, `Wait(s)`, `WaitFor(condition)`, `Signal(name)`,
//! each on a role's NPC. A game plays one with [`play_sequence`]`(world, "greeting", &[("guard", e7),
//! ("visitor", player)])`, which binds the roles to bodies and returns the run's [`SequenceId`]; an agent
//! plays one with `sequence_play id=greeting role.guard=e7 role.visitor=e1`. The scene itself, its lines
//! and when it plays are the game's.
//!
//! - **It runs in the simulation** ([`run_sequences`], `NpcSystems::Decide`, after the behaviour system):
//!   the runs are visited in number order, a step that is done lets the next begin on the same tick, and
//!   the state ([`Sequences`], the hold [`SequenceHold`] on the bodies) is in snapshots, in replay checks
//!   and in the replay's `content` read, so a game loaded in the middle of a scene goes on with it. Timers
//!   are whole ticks. A run is ordered by its number, its bodies by `AiId`; nothing here draws a random
//!   number.
//! - **An agent's orders are dropped.** `play_sequence` clears the `Intent.orders` of the NPCs it holds: an NPC under orders
//!   is driven by them before anything else, so the run could not walk it.
//! - **A body that acts is held, its brain dormant.** While a run holds an NPC, its behaviour does not
//!   choose (`decide` leaves its brain, plan and decision timer exactly as they were), it asks for no
//!   attack token and presses nothing (it counts as *under orders*, as when an agent drives it), its
//!   turret does not track and its ranged fighter traces no line; its perception goes on (what it
//!   sees and hears is remembered). The run tells it where to go and what to face through its `NpcDrive`,
//!   so the NPC's own navigation, avoidance and steering carry it out. When the run ends the hold goes
//!   and the brain goes on from the state it was in. A held NPC is awake even where the world module
//!   would have put it to sleep: the scene needs it.
//! - **`MoveTo`** walks by the NPC's routes (the navmesh) to a [`LevelMarker`](shards_level::LevelMarker)
//!   of that name, at the step's `speed` (0.5 walks, 1 runs). **`Face`** turns the body to a marker or to
//!   another role's body, which it keeps facing (following a body that moves) until a `MoveTo` or another
//!   `Face`. **`Play`** forces the action (`shards_action::force_action`) and ends when the run is over.
//!   **`Sound`** triggers [`ActionSound`](shards_action::ActionSound), the event an action's `Sound`
//!   effect triggers, so the audio module plays it from the body with its subtitle and speaker as for any
//!   action; the simulation never waits for audio (a `Wait` as long as the line follows). **`WaitFor`**
//!   asks the condition registry (`add_ai_condition`). **`Signal`** triggers [`SequenceSignal`].
//! - **A step that cannot finish ends the run**: a marker the world lacks, a body that died or is gone, a
//!   step past its `timeout`. The run ends *skipped* ([`SequenceEnded::skipped`]) with a warning that says
//!   why, so a game that unlocks the player on `SequenceEnded` never leaves it locked.
//! - **Events**: [`SequenceStarted`], [`SequenceStep`] (a step began), [`SequenceSignal`],
//!   [`SequenceEnded`]. They are triggered where the simulation runs: an observer that changes gameplay
//!   returns at once unless `shards::core::simulates`.
//!
//! The crate this lives in: it needs the NPC's own movement and brain (`NpcDrive`, the navmesh, the
//! director), the action machine (`force_action`), levels (markers) and the condition registry, and
//! nothing above the NPC module needs to know it: `shards_audio` is reached through the event an action's
//! sound already uses.

mod def;
mod run;
mod step;
mod tools;

use ai_core::{AiAppExt, AiId};
use bevy::prelude::*;
use shards_action::{ActionState, Dead, Downed};
use shards_content::{ContentAppExt, ContentDb};
use shards_core::Intent;

pub use def::{FaceTarget, SequenceDef, Step, DEFAULT_FACE_TIMEOUT, DEFAULT_LONG_TIMEOUT, FACE_TOLERANCE_DEG};
pub use run::{Facing, Run, SequenceEnded, SequenceHold, SequenceId, SequenceSignal, SequenceStarted, SequenceStep, Sequences};
pub use step::{condition_holds, marker_names, marker_point};

pub(crate) use step::run_sequences;
pub(crate) use tools::state_of;

use crate::brain::{Npc, NpcBrain, NpcDrive};

/// Plays sequence `id` with `roles` bound to bodies (`("guard", e7)`): every role the definition
/// declares must be bound once. The roles some step acts on must be NPCs that no other run holds and that
/// stand (not dead, not down), with an action state if a step plays an action; the markers the steps name
/// must be in the world and the conditions they wait for registered. `Err` says what is wrong and how to
/// fix it; nothing has changed then.
///
/// The first step begins on the next tick the NPC systems run. Call it where the simulation runs (a tool, a
/// system in a tick): the run is state.
pub fn play_sequence(world: &mut World, id: &str, roles: &[(&str, Entity)]) -> Result<SequenceId, String> {
    let def = {
        let db = world.get_resource::<ContentDb>().ok_or("no content is loaded")?;
        match db.peek::<SequenceDef>(id) {
            Some(def) => def.clone(),
            None => {
                let known: Vec<&str> = db.peek_iter::<SequenceDef>().map(|(id, _)| id).collect();
                let hint = shards_content::db::closest(id, known.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                return Err(format!("no sequence `{id}`{hint}; sequences: {}", if known.is_empty() { "none".to_owned() } else { known.join(", ") }));
            }
        }
    };
    let mut bound: Vec<(String, AiId)> = Vec::new();
    for (role, entity) in roles {
        if !def.roles.iter().any(|r| r == role) {
            return Err(format!("sequence `{id}` has no role `{role}`; its roles: {}", def.roles.join(", ")));
        }
        if bound.iter().any(|(r, _)| r == role) {
            return Err(format!("role `{role}` is bound twice"));
        }
        let ai = world.get::<AiId>(*entity).copied().ok_or_else(|| format!("the body bound to `{role}` has no `AiId` (what the AI sees has one)"))?;
        if let Some((other, _)) = bound.iter().find(|(_, b)| *b == ai) {
            return Err(format!("{ai} is bound to both `{other}` and `{role}`"));
        }
        bound.push(((*role).to_owned(), ai));
    }
    for role in &def.roles {
        if !bound.iter().any(|(r, _)| r == role) {
            return Err(format!("role `{role}` is not bound: `sequence_play id={id} {}`, or bind it in code", def.roles.iter().map(|r| format!("role.{r}=eN")).collect::<Vec<_>>().join(" ")));
        }
    }
    bound.sort_by(|a, b| a.0.cmp(&b.0));
    // Who acts, and what the world must have for the steps.
    let mut actors: Vec<AiId> = Vec::new();
    for (index, step) in def.steps.iter().enumerate() {
        if let Some(role) = step.actor()
            && let Some((_, ai)) = bound.iter().find(|(r, _)| r == role)
            && !actors.contains(ai)
        {
            actors.push(*ai);
        }
        let at = format!("sequence `{id}` step {}/{} ({})", index + 1, def.steps.len(), step.kind());
        match step {
            Step::MoveTo { marker, .. } | Step::Face { at: FaceTarget::Marker(marker), .. } if marker_point(world, marker).is_none() => {
                let known = marker_names(world);
                let hint = shards_content::db::closest(marker, known.iter().map(String::as_str)).filter(|c| c != marker).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                return Err(format!("{at}: no marker `{marker}` in the world{hint}; markers: {}", if known.is_empty() { "none".to_owned() } else { known.join(", ") }));
            }
            Step::WaitFor { condition, .. } => {
                condition_holds(world, condition).map_err(|e| format!("{at}: {e}"))?;
            }
            _ => {}
        }
    }
    actors.sort();
    for ai in &actors {
        let role = bound.iter().find(|(_, b)| b == ai).map(|(r, _)| r.as_str()).unwrap_or_default();
        let entity = world.get_resource::<ai_core::AiIndex>().and_then(|i| i.get(*ai)).ok_or_else(|| format!("{ai} is gone"))?;
        if world.get::<Npc>(entity).is_none() || world.get::<NpcBrain>(entity).is_none() || world.get::<NpcDrive>(entity).is_none() {
            return Err(format!("{ai} (`{role}`) is no NPC: only NPCs (bodies with a brain) act in a sequence; a role that is only looked at (`Face(at: Role(..))`) may be any body"));
        }
        if world.get::<Dead>(entity).is_some() || world.get::<Downed>(entity).is_some() {
            return Err(format!("{ai} (`{role}`) is {}", if world.get::<Dead>(entity).is_some() { "dead" } else { "down" }));
        }
        if let Some(hold) = world.get::<SequenceHold>(entity) {
            return Err(format!("{ai} (`{role}`) is already in sequence {} ({}); `sequence_stop run={}` ends it", hold.run, hold.def, hold.run));
        }
        let plays = def.steps.iter().any(|s| matches!(s, Step::Play { role: r, .. } if r == role));
        if plays && world.get::<ActionState>(entity).is_none() {
            return Err(format!("{ai} (`{role}`) has no actions to play (it is no fighter), and a step plays one"));
        }
    }
    let started = ai_core::sim::tick(world);
    let number = {
        let mut sequences = world.get_resource_or_insert_with(Sequences::default);
        sequences.next += 1;
        sequences.next
    };
    let run = Run { id: number, def: id.to_owned(), roles: bound.clone(), actors: actors.clone(), started, ..default() };
    world.resource_mut::<Sequences>().runs.push(run);
    for ai in &actors {
        let role = bound.iter().find(|(_, b)| b == ai).map(|(r, _)| r.clone()).unwrap_or_default();
        if let Some(entity) = world.get_resource::<ai_core::AiIndex>().and_then(|i| i.get(*ai)) {
            world.entity_mut(entity).insert(SequenceHold { run: number, def: id.to_owned(), role });
            // An agent's order (`move_to`, `attack`) would take the NPC over from the run (`drive_intents` follows orders first):
            // the scene is the NPC's now, so what it was told to do is dropped.
            if let Some(mut intent) = world.get_mut::<Intent>(entity)
                && !intent.orders.is_empty()
            {
                intent.orders.clear();
                intent.move_dir = Vec3::ZERO;
            }
        }
    }
    world.trigger(SequenceStarted { sequence: SequenceId(number), def: id.to_owned(), roles: bound });
    Ok(SequenceId(number))
}

/// Stops a run: its bodies are free at once (their brains go on from where they were) and
/// [`SequenceEnded`] says `skipped: true`. `false` if no such run plays.
pub fn stop_sequence(world: &mut World, id: SequenceId) -> bool {
    let Some(run) = world.get_resource::<Sequences>().and_then(|s| s.get(id)).cloned() else { return false };
    step::end(world, run, true, None);
    true
}

pub(crate) fn plugin(app: &mut App) {
    app.register_type::<Sequences>()
        .register_type::<Run>()
        .register_type::<Facing>()
        .register_type::<SequenceId>()
        .register_type::<SequenceHold>()
        .add_content::<SequenceDef>()
        // The runs are simulation state outside the entities they hold; a snapshot mid-scene resumes it exactly.
        // A snapshot without runs removes this world's (the bodies' holds are replaced by the load too).
        .add_snapshot_resource::<Sequences>()
        .add_snapshot_prepare_after("sequences", "region", &[], tools::drop_absent)
        .add_optional_replay_check("sequences", tools::runs_hash);
    tools::register(app);
}
