//! The engine rules `encounter.members_tracked`, `encounter.state_sane` and
//! `encounter.lock_matches`. An invariant that fires is a bug to fix, not a rule to weaken.

use ai_core::{with_query, AiId, AiIndex, Offenders};
use bevy::prelude::*;
use shards_content::ContentDb;

use crate::def::EncounterDef;
use crate::machine::ticks;
use crate::run::{EncounterMember, EncounterRuns, RunState};

/// The engine rule `encounter.members_tracked`: every entity with `EncounterMember` is listed by
/// the run of its `key`, with the same `wave` and `counted`; every listed member that still
/// resolves carries `EncounterMember` naming that run.
///
/// Why: a member the run does not list never ends its wave and is not reset or despawned with
/// it; a listed member without the marker is missed by `NetAlways` and by `encounter_of`. A
/// dead member stays listed (and marked) until its wave clears, and the clear takes both away,
/// so a corpse that stays in the world never carries a marker its run no longer lists.
pub fn members_tracked(world: &World) -> Option<String> {
    let runs = world.get_resource::<EncounterRuns>();
    let mut bad = Offenders::default();
    with_query::<(Option<&AiId>, &EncounterMember), (), _>(world, |query| {
        for (id, member) in query.iter(world) {
            let id = id.copied().unwrap_or_default();
            let Some(run) = runs.and_then(|r| r.get(&member.key)) else {
                bad.note(id, || format!("carries EncounterMember of run `{}`, which does not exist", member.key));
                continue;
            };
            match run.members.iter().find(|m| m.id == id) {
                None => bad.note(id, || format!("carries EncounterMember of run `{}`, which does not list it", member.key)),
                Some(listed) if listed.wave != member.wave || listed.counted != member.counted => bad.note(id, || {
                    format!(
                        "EncounterMember says wave {} counted {}, run `{}` lists wave {} counted {}",
                        member.wave, member.counted, member.key, listed.wave, listed.counted
                    )
                }),
                Some(_) => {}
            }
        }
    })?;
    if let (Some(runs), Some(index)) = (runs, world.get_resource::<AiIndex>()) {
        for run in &runs.runs {
            for listed in &run.members {
                let Some(entity) = index.get(listed.id) else { continue };
                if !world.get::<EncounterMember>(entity).is_some_and(|m| m.key == run.key) {
                    bad.note(listed.id, || format!("is listed by run `{}` but does not carry its EncounterMember", run.key));
                }
            }
        }
    }
    bad.report()
}

/// The engine rule `encounter.state_sane`: `wave <= waves.len()`; a timer is within what the
/// definition can set; runs are sorted with unique keys; a `Won` run of a `once` encounter has
/// its key in `completed`; a `Won` run lists no living counted member.
///
/// Why: the state machine's table holds only if these do; a broken one rewinds, spawns or waits
/// wrong. Not checked: that `Running` lists a member of its wave (a placed pack killed before
/// the start enters `Running` with none, and clears on its first visit).
pub fn state_sane(world: &World) -> Option<String> {
    let runs = world.get_resource::<EncounterRuns>()?;
    let db = world.get_resource::<ContentDb>();
    let index = world.get_resource::<AiIndex>();
    let dt = crate::step::dt_of(world);
    for pair in runs.runs.windows(2) {
        if pair[0].key >= pair[1].key {
            return Some(format!("runs `{}` and `{}` are not sorted by unique keys", pair[0].key, pair[1].key));
        }
    }
    for run in &runs.runs {
        let Some(def) = db.and_then(|db| db.peek::<EncounterDef>(&run.encounter)) else { continue };
        if run.wave as usize > def.waves.len() {
            return Some(format!("run `{}` is in wave {} of {}", run.key, run.wave, def.waves.len()));
        }
        let slowest = def.waves.iter().map(|w| ticks(w.delay, dt)).max().unwrap_or(0);
        let over = |what: &str, left: u32, most: u32| (left > most).then(|| format!("run `{}` {what} has {left} ticks left, at most {most} can be set", run.key));
        let bad = match &run.state {
            RunState::Between { left } => over("Between", *left, slowest),
            RunState::Wiped { resume: Some(left), .. } => over("Wiped resume", *left, ticks(def.resume_after, dt)),
            RunState::Resetting { left } => over("Resetting", *left, ticks(def.pull_back.deadline, dt)),
            RunState::Won => {
                if def.once && !runs.completed.contains(&run.key) {
                    Some(format!("run `{}` is Won and `once`, but its key is not in completed", run.key))
                } else {
                    let alive = run.members.iter().find(|m| {
                        m.counted && index.and_then(|i| i.get(m.id)).is_some_and(|e| world.get::<shards_action::Dead>(e).is_none())
                    });
                    alive.map(|m| format!("run `{}` is Won but lists {} alive and counted", run.key, m.id))
                }
            }
            _ => None,
        };
        if bad.is_some() {
            return bad;
        }
    }
    None
}

/// The engine rule `encounter.lock_matches`: for an encounter with a `lock`, `run.lock` is `Some`
/// and resolves exactly in `Running` and `Between`, and is `None` otherwise.
///
/// Why: a lock left standing after a wipe shuts the respawned players out of the arena, and a
/// lock that vanished mid-wave lets the fight out.
pub fn lock_matches(world: &World) -> Option<String> {
    let runs = world.get_resource::<EncounterRuns>()?;
    let db = world.get_resource::<ContentDb>()?;
    let index = world.get_resource::<AiIndex>()?;
    for run in &runs.runs {
        let Some(def) = db.peek::<EncounterDef>(&run.encounter) else { continue };
        let Some(prefab) = &def.lock else { continue };
        // A lock prefab that did not spawn is said in the journal once; it is content's to fix.
        if run.noted & crate::run::NOTE_LOCK != 0 {
            continue;
        }
        let want = matches!(run.state, RunState::Running | RunState::Between { .. });
        let stands = run.lock.is_some_and(|id| index.get(id).is_some());
        if want != stands {
            return Some(format!(
                "run `{}` is {}: its lock `{prefab}` {} stand",
                run.key,
                run.state.name(),
                if want { "should" } else { "should not" }
            ));
        }
    }
    None
}
