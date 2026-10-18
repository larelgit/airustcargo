//! The state machine of a run: one transition per tick at most, as a pure function.
//!
//! [`advance`] reads what the world shows ([`Obs`], made by the step), changes the [`Run`] and
//! says what the world must do about it ([`Fx`]: events to trigger, members to spawn, despawn,
//! pull back). It touches no `World`, so every row of the table below is a unit test with a
//! hand-made observation.
//!
//! | State | Each tick, in this order | Next |
//! |---|---|---|
//! | `Idle` | the start condition is false | `Armed` |
//! | `Armed` | the start condition is true, or a member of the placed wave is out, hurt or hunts a standing participant within the radius | the placed wave has members: `Running`; else `Between` for the next wave (`Running` when there is none, so the normal clear path runs) |
//! | `Running` | 1. every counted member of wave `wave` is out | last wave: `Won`; else `Between` |
//! | | 2. all participants out | wipe, `Wiped { None }` |
//! | | 3. the absence clock is due | reset, `Resetting` |
//! | `Between` | 1. all out: a wipe that rewinds nothing | `Wiped { None }` |
//! | | 2. the delay has run out: spawn wave `wave + 1` | `Running` |
//! | | 3. the absence clock is due | `Resetting` |
//! | `Wiped { None }` | someone stands | `Wiped { Some }` |
//! | | else the absence clock is due | `Resetting` |
//! | `Wiped { Some }` | 1. all out again | `Wiped { None }` |
//! | | 2. the resume has run out: the wipe pulled back the placed wave | `Idle` |
//! | | 2. the resume has run out, otherwise: spawn wave `wave + 1` | `Running` |
//! | | 3. the absence clock is due | `Resetting` |
//! | `Resetting` | every survivor is home, or the deadline has run out | `Idle` |
//! | `Won` | nothing | `Won` |
//!
//! **Timers.** Every timer is a count of whole ticks. A timer set in tick `N` is set to
//! `n = ceil(60 s - 60 EPS)` and counted down by one at the end of the same tick, so the check of
//! tick `N + k` sees `n - k` and expires at `k = n` (72 ticks for 1.2 s). The check of the tick
//! that set it is the transition itself: a run has one transition per tick.
//!
//! **Clearing beats wiping.** When the last counted member and the last participant fall on the
//! same tick, the wave is cleared first, in that tick (and the encounter won, if it was the last
//! wave). The wipe is checked from the next tick on, against the new state: in `Between` it
//! rewinds nothing, in `Won` it does not happen.

use ai_core::AiId;
use bevy::prelude::*;

use crate::def::{CueDef, EncounterDef, OnReset};
use crate::events::*;
use crate::run::{Member, Run, RunState};

const EPS: f32 = 1e-4;

/// What the world shows about one member of a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemberObs {
    /// Dead, or gone from the world.
    pub out: bool,
    /// Out, hurt, or has a target: a placed pack that is being fought before it started.
    pub disturbed: bool,
}

/// What the world shows about a run this tick.
#[derive(Clone, Debug, Default)]
pub struct Obs {
    /// Seconds per tick.
    pub dt: f32,
    /// Participants (player bodies) in all, and the ones that are not out.
    pub participants: u32,
    pub standing: u32,
    /// Some standing participant is within the encounter's radius.
    pub near: bool,
    /// The start condition holds now.
    pub start_met: bool,
    /// One per entry of `run.members`, in that order.
    pub members: Vec<MemberObs>,
    /// No member is on its way home.
    pub all_home: bool,
}

impl Obs {
    /// At least one participant exists and every one is out.
    pub fn all_out(&self) -> bool {
        self.participants > 0 && self.standing == 0
    }
}

/// What the world must do about a transition. Applied in order.
#[derive(Clone, Debug, PartialEq)]
pub enum Fx {
    Started(EncounterStarted),
    Warned(WaveWarned),
    Cleared(WaveCleared),
    Completed(EncounterCompleted),
    Wiped(EncounterWiped),
    Resumed(EncounterResumed),
    Reset(EncounterReset),
    /// Spawn wave `wave` (1-based) and trigger `WaveStarted`.
    Spawn { wave: u32 },
    /// Despawn silently: no `Died`, so no loot and no death cue.
    Despawn(Vec<AiId>),
    /// Out members that stay in the world as corpses: take the marker off.
    Retire(Vec<AiId>),
    /// Heal if `pull_back.heal`, and send home.
    PullBack(Vec<AiId>),
    /// A win: the adds lose the marker and leave the run.
    Release(Vec<AiId>),
    /// The deadline of a reset ran out: every survivor still on its way is placed home.
    PlaceHome,
    /// A `once` encounter was won: remember its key.
    Remember,
}



/// Ticks in `secs` seconds: `ceil(secs / dt - EPS / dt)`, never negative.
pub fn ticks(secs: f32, dt: f32) -> u32 {
    if !secs.is_finite() || secs <= 0.0 || dt <= 0.0 {
        return 0;
    }
    ((secs - EPS) / dt).ceil().max(0.0) as u32
}

fn waves_of(def: &EncounterDef) -> u32 {
    def.waves.len() as u32
}

fn cue_of(def: &EncounterDef, wave: u32) -> Option<CueDef> {
    def.waves.get(wave.checked_sub(1)? as usize)?.warn.clone()
}

/// A wave is due: `WaveWarned` for wave `wave` (1-based).
fn warn(run: &Run, def: &EncounterDef, wave: u32, fx: &mut Vec<Fx>) {
    let delay = def.waves.get(wave.saturating_sub(1) as usize).map_or(0.0, |w| w.delay);
    fx.push(Fx::Warned(WaveWarned {
        key: run.key.clone(),
        encounter: run.encounter.clone(),
        wave,
        waves: waves_of(def),
        delay,
        at: run.at,
        cue: cue_of(def, wave),
    }));
}

/// Whether the absence clock is due: nobody stood near for `reset_after`.
fn absent_due(run: &Run, def: &EncounterDef, dt: f32) -> bool {
    def.reset_after.is_some_and(|secs| run.absent > 0 && run.absent >= ticks(secs, dt))
}

/// Whether member `m` belongs to the placed first wave.
fn is_placed(def: &EncounterDef, m: &Member) -> bool {
    def.placed() && m.counted && m.wave == 1
}

/// The run's members with what the world shows about them.
fn view(run: &Run, obs: &Obs) -> Vec<(Member, MemberObs)> {
    run.members.iter().enumerate().map(|(i, m)| (*m, obs.members.get(i).copied().unwrap_or_default())).collect()
}

/// One visit of a run in one tick. See the module's table.
pub fn advance(run: &mut Run, def: &EncounterDef, obs: &Obs, fx: &mut Vec<Fx>) {
    if run.state.absence_counts() {
        if def.reset_after.is_some() && !obs.near {
            run.absent = run.absent.saturating_add(1);
        } else {
            run.absent = 0;
        }
    }
    match run.state.clone() {
        RunState::Idle => {
            if !obs.start_met {
                run.state = RunState::Armed;
            }
        }
        RunState::Armed => {
            let view = view(run, obs);
            let disturbed = def.placed() && view.iter().any(|(m, o)| is_placed(def, m) && o.disturbed);
            if obs.start_met || disturbed {
                begin(run, def, obs, fx);
            }
        }
        RunState::Running => running(run, def, obs, fx),
        RunState::Between { left } => {
            if obs.all_out() {
                wipe(run, def, obs, fx);
            } else if left == 0 {
                spawn_next(run, def, fx);
            } else if absent_due(run, def, obs.dt) {
                reset(run, def, obs, ResetWhy::Absent, fx);
            }
        }
        RunState::Wiped { resume: None, pulled_back } => {
            if obs.standing > 0 {
                let pending = if pulled_back { 0 } else { run.wave + 1 };
                let delay = def.resume_after;
                fx.push(Fx::Resumed(EncounterResumed {
                    key: run.key.clone(),
                    encounter: run.encounter.clone(),
                    wave: if pending as usize <= def.waves.len() { pending } else { 0 },
                    waves: waves_of(def),
                    delay,
                }));
                if !pulled_back && (run.wave as usize) < def.waves.len() {
                    warn(run, def, run.wave + 1, fx);
                }
                run.state = RunState::Wiped { resume: Some(ticks(delay, obs.dt)), pulled_back };
            } else if absent_due(run, def, obs.dt) {
                reset(run, def, obs, ResetWhy::Absent, fx);
            }
        }
        RunState::Wiped { resume: Some(left), pulled_back } => {
            if obs.all_out() {
                run.state = RunState::Wiped { resume: None, pulled_back };
            } else if left == 0 {
                if pulled_back {
                    run.state = RunState::Idle;
                    run.absent = 0;
                } else {
                    spawn_next(run, def, fx);
                }
            } else if absent_due(run, def, obs.dt) {
                reset(run, def, obs, ResetWhy::Absent, fx);
            }
        }
        RunState::Resetting { left } => {
            if obs.all_home || left == 0 {
                if !obs.all_home {
                    fx.push(Fx::PlaceHome);
                }
                run.state = RunState::Idle;
                run.absent = 0;
            }
        }
        RunState::Won => {}
    }
    tick_timers(run);
}

/// The end of every visit: the timers count down by one tick, including a timer set in this
/// very tick.
fn tick_timers(run: &mut Run) {
    match &mut run.state {
        RunState::Between { left } | RunState::Resetting { left } => *left = left.saturating_sub(1),
        RunState::Wiped { resume: Some(left), .. } => *left = left.saturating_sub(1),
        _ => {}
    }
}

/// The start: `Armed` to the first wave. `participants` is what the event reports.
pub fn begin(run: &mut Run, def: &EncounterDef, obs: &Obs, fx: &mut Vec<Fx>) {
    fx.push(Fx::Started(EncounterStarted {
        key: run.key.clone(),
        encounter: run.encounter.clone(),
        participants: obs.standing,
        of: obs.participants,
    }));
    let placed_stands = def.placed() && run.wave == 1 && run.members.iter().any(|m| is_placed(def, m));
    if placed_stands {
        // The placed wave fights: `Running` clears it when its members are out, also when they
        // were all out before the start.
        run.state = RunState::Running;
    } else if let Some(wave) = def.waves.get(run.wave as usize) {
        warn(run, def, run.wave + 1, fx);
        run.state = RunState::Between { left: ticks(wave.delay, obs.dt) };
    } else {
        // Nothing is left to spawn: the normal clear path ends the encounter.
        run.state = RunState::Running;
    }
    run.absent = 0;
}

/// `start_encounter` and the dev tool: the start as the start condition would make it, between
/// ticks. A timer set here is counted as set by the tick that just ran, so it expires the same
/// number of ticks after the call as one set by a tick.
pub fn start_now(run: &mut Run, def: &EncounterDef, obs: &Obs, fx: &mut Vec<Fx>) {
    begin(run, def, obs, fx);
    tick_timers(run);
}

fn running(run: &mut Run, def: &EncounterDef, obs: &Obs, fx: &mut Vec<Fx>) {
    let view = view(run, obs);
    let cleared = view.iter().filter(|(m, _)| m.counted && m.wave == run.wave).all(|(_, o)| o.out);
    if cleared {
        clear(run, def, &view, obs, fx);
    } else if obs.all_out() {
        wipe(run, def, obs, fx);
    } else if absent_due(run, def, obs.dt) {
        reset(run, def, obs, ResetWhy::Absent, fx);
    }
}

/// The wave's counted members are all out: prune, then the next wave or the win.
fn clear(run: &mut Run, def: &EncounterDef, view: &[(Member, MemberObs)], obs: &Obs, fx: &mut Vec<Fx>) {
    let wave = run.wave;
    fx.push(Fx::Cleared(WaveCleared { key: run.key.clone(), encounter: run.encounter.clone(), wave, waves: waves_of(def) }));
    // Out entries are pruned; the corpses that stay in the world lose the marker.
    let out: Vec<AiId> = view.iter().filter(|(_, o)| o.out).map(|(m, _)| m.id).collect();
    run.members.retain(|m| !out.contains(&m.id));
    if !out.is_empty() {
        fx.push(Fx::Retire(out));
    }
    if wave as usize >= def.waves.len() {
        let adds: Vec<AiId> = run.members.drain(..).map(|m| m.id).collect();
        if !adds.is_empty() {
            fx.push(Fx::Release(adds));
        }
        fx.push(Fx::Completed(EncounterCompleted { key: run.key.clone(), encounter: run.encounter.clone() }));
        if def.once {
            fx.push(Fx::Remember);
        }
        run.state = RunState::Won;
        run.absent = 0;
    } else {
        warn(run, def, wave + 1, fx);
        let delay = def.waves.get(wave as usize).map_or(0.0, |w| w.delay);
        run.state = RunState::Between { left: ticks(delay, obs.dt) };
    }
}

/// Spawn wave `wave + 1`, `Running`. The pending wave of `Between`, or of a resume.
fn spawn_next(run: &mut Run, def: &EncounterDef, fx: &mut Vec<Fx>) {
    if (run.wave as usize) < def.waves.len() {
        run.wave += 1;
        fx.push(Fx::Spawn { wave: run.wave });
    }
    run.state = RunState::Running;
    run.absent = 0;
}

/// Every participant is out (WP08). From `Running` or `Between`.
fn wipe(run: &mut Run, def: &EncounterDef, obs: &Obs, fx: &mut Vec<Fx>) {
    let view = view(run, obs);
    let running = run.state == RunState::Running;
    let between = !running;
    // 1. The adds go.
    let adds: Vec<AiId> = view.iter().filter(|(m, _)| !m.counted).map(|(m, _)| m.id).collect();
    if !adds.is_empty() {
        fx.push(Fx::Despawn(adds.clone()));
        run.members.retain(|m| m.counted);
    }
    let mut rewound = false;
    let mut pulled_back = false;
    if running {
        let in_wave: Vec<&(Member, MemberObs)> = view.iter().filter(|(m, _)| m.counted && m.wave == run.wave).collect();
        if def.placed() && run.wave == 1 {
            // 4. A placed wave is not despawned: its survivors are pulled back.
            let alive: Vec<AiId> = in_wave.iter().filter(|(_, o)| !o.out).map(|(m, _)| m.id).collect();
            if !alive.is_empty() {
                pulled_back = true;
                fx.push(Fx::PullBack(alive));
            }
        } else {
            // 2-3. The living members of a spawned wave are despawned, silently, and the wave
            // goes back by one. Its dead entries go with it, so `Running` does not list corpses
            // of a wave that will spawn again.
            let all: Vec<AiId> = in_wave.iter().map(|(m, _)| m.id).collect();
            let dead: Vec<AiId> = in_wave.iter().filter(|(_, o)| o.out).map(|(m, _)| m.id).collect();
            let alive: Vec<AiId> = in_wave.iter().filter(|(_, o)| !o.out).map(|(m, _)| m.id).collect();
            if !alive.is_empty() {
                rewound = true;
                fx.push(Fx::Despawn(alive));
            }
            if !dead.is_empty() {
                fx.push(Fx::Retire(dead));
            }
            run.members.retain(|m| !all.contains(&m.id));
            if rewound {
                run.wave = run.wave.saturating_sub(1);
            }
        }
    }
    fx.push(Fx::Wiped(EncounterWiped {
        key: run.key.clone(),
        encounter: run.encounter.clone(),
        wave: run.wave,
        waves: waves_of(def),
        rewound,
        between,
    }));
    run.state = RunState::Wiped { resume: None, pulled_back };
}

/// The run resets: nobody near, or a dev, or the content changed. From a fighting state.
pub fn reset(run: &mut Run, def: &EncounterDef, obs: &Obs, why: ResetWhy, fx: &mut Vec<Fx>) {
    let view = view(run, obs);
    let was_running = run.state == RunState::Running;
    // 1. The adds and the living members of spawned waves are despawned, silently.
    let mut gone: Vec<AiId> = Vec::new();
    let mut corpses: Vec<AiId> = Vec::new();
    for (m, o) in &view {
        if is_placed(def, m) {
            continue;
        }
        if o.out && m.counted {
            corpses.push(m.id);
        } else {
            gone.push(m.id);
        }
    }
    if !gone.is_empty() {
        fx.push(Fx::Despawn(gone));
    }
    if !corpses.is_empty() {
        fx.push(Fx::Retire(corpses));
    }
    run.members.retain(|m| is_placed(def, m));
    // 2. The survivors of the placed wave are pulled back.
    let alive: Vec<AiId> = view.iter().filter(|(m, o)| is_placed(def, m) && !o.out).map(|(m, _)| m.id).collect();
    if !alive.is_empty() {
        fx.push(Fx::PullBack(alive));
    }
    // 4. How far the run goes back.
    let after = (run.absent as f32 * obs.dt).max(0.0);
    run.wave = match def.on_reset {
        OnReset::Restart => u32::from(def.placed()),
        OnReset::KeepCleared => {
            if was_running && !(def.placed() && run.wave == 1) {
                run.wave.saturating_sub(1)
            } else {
                run.wave
            }
        }
    };
    fx.push(Fx::Reset(EncounterReset { key: run.key.clone(), encounter: run.encounter.clone(), why, after }));
    run.state = RunState::Resetting { left: ticks(def.pull_back.deadline, obs.dt) };
    run.absent = 0;
}

#[cfg(test)]
mod tests;
