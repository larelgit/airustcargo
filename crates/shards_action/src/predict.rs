//! A client starts its own body's actions at once, ahead of the host ([`NetSet::Predict`]).
//!
//! Each predicted tick, for the body marked [`Predicted`]:
//!
//! 1. What the host answered (its forwarded [`ActionStarted`] and [`ActionRejected`], and its
//!    replicated [`ActionState`]) settles the predictions waiting for it: confirmed, refused
//!    (`net: predicted dodge not confirmed (stamina 10/20)`), or not started in time (the
//!    host's tick passed the press's by the lead and six ticks).
//! 2. An action the host started that this machine did not (a stagger, a death, a press the
//!    host held in its buffer longer) is taken over, caught up to now; one this machine
//!    predicted is put where the host has it.
//! 3. This tick's presses (the ones the capture sent) wait in a buffer as the host's do, and
//!    [`can_start`] over the replicated `Stamina`, `Cooldowns` and `Charges` and the action
//!    shown here decides whether one starts now: a [`PredictedAction`] and a local
//!    [`ActionStarted`] (`predicted: true`).
//! 4. The action shown moves the body through `LocomotionInput` the way the machine does
//!    ([`steer_locomotion`]), turns into its charged action when the hold is long enough (a
//!    prediction of the charged action, answered as any other), and stops when its `while_held`
//!    input is let go.
//!
//! A host run is new to this machine when its instance is new or when it is the instance shown
//! here with another action (the host turned a charge this machine did not). The run shown
//! takes what only the host changes: the extra recovery of a miss, whether a hit landed. A run
//! the host ended with no new one (no event says so) leaves the body to the host's state, and a
//! charge the host never turned (the stamina, a charge or the cooldown was not there) goes back to the host's action once the
//! charge could no longer turn.
//!
//! Hits, effects, damage, stamina spent and statuses are never predicted: they come with the
//! host's state.

use std::collections::VecDeque;

use ai_core::journal::{self, JournalKind};
use ai_core::{AiAppExt, AiIndex, SimTick};
use bevy::ecs::query::QueryData;
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::input_names::SPRINT;
use shards_core::{Intent, NetPrediction, Predicted};
use shards_motion::{CharacterController, CharacterParams, LocomotionInput, MotionState, MoveModifiers, PhysicsWorld};

use crate::combat::Died;
use crate::def::{ActionDef, BufferMode, CombatTuning, MovesetDef, EPS};
use crate::fighter::{sprint_rule, Charges, Cooldowns, Dead, Fighter, Stamina};
use crate::machine::{
    advance, body_height, buffer_press, cap_queue, charge_due, choose_in, free_sprint, in_sight, press_life, start_facing,
    steer_locomotion, step_run, tracked, understands, ActionRejected, ActionStarted, ActionState, FighterView, Queued, RunExtras, Running,
};

/// Ticks past a press's tick (and the lead) after which a prediction the host has not
/// answered counts as refused.
pub const CONFIRM_SLACK: u64 = 6;

/// Predictions waiting for the host's word, at most.
const PENDING_KEPT: usize = 8;

/// The own body's action as a client shows it now, ahead of the host's state: one it
/// predicted (`confirmed` once the host started it too) or one the host started that it
/// took over. Never replicated, never in replays: each client's own.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component)]
pub struct PredictedAction {
    pub action: String,
    /// `None`: the engine forced it on the host (a stagger, a death).
    pub input: Option<String>,
    /// `ActionState.started` once the host has started it.
    pub expect_instance: u32,
    /// The client tick it started on here (a taken-over one: the tick it was taken over).
    pub start_tick: u64,
    pub run: Running,
    /// The host started it too (or started it itself).
    pub confirmed: bool,
    /// The run's [`RunExtras`] as this machine keeps them (its own hold; the host's extra
    /// recovery and landing), when its action needs them.
    pub extras: Option<RunExtras>,
}

/// A prediction the host has not answered yet.
#[derive(Clone, Debug, PartialEq)]
struct Pending {
    action: String,
    input: String,
    instance: u32,
    start_tick: u64,
    /// A charge turned here: the action it turned from (the run keeps its instance).
    branch_from: Option<String>,
}

/// What the host said about the own body (forwarded events, with their host tick).
#[derive(Clone, Debug, PartialEq)]
enum Answer {
    Started { tick: u64, action: String, input: Option<String> },
    Rejected { tick: u64, input: String, reason: String },
}

/// A client's bookkeeping of the actions it predicts.
#[derive(Resource, Default, Debug)]
pub struct ActionPredictor {
    epoch: u32,
    /// The host's actions (instances) below this are dealt with.
    seen: u32,
    pending: VecDeque<Pending>,
    /// Presses waiting to start, as the host buffers them.
    queued: Vec<Queued>,
    /// Forwarded events since the last predicted tick.
    answers: Vec<Answer>,
    /// The run shown here that last ran out of time, with the extra recovery it had: the host may
    /// still be in a longer recovery of it.
    ended: Option<(u32, f32)>,
}

pub(crate) fn plugin(app: &mut App) {
    app.register_type::<PredictedAction>()
        .net_ignore::<PredictedAction>()
        .replay_ignore::<PredictedAction>()
        .init_resource::<ActionPredictor>()
        .add_observer(heard_started)
        .add_observer(heard_rejected)
        .add_observer(heard_died);
}

/// What [`predict_actions`] reads and writes of the own body.
#[derive(QueryData)]
#[query_data(mutable)]
pub(crate) struct PredictedBody {
    entity: Entity,
    fighter: &'static Fighter,
    intent: &'static Intent,
    state: &'static ActionState,
    stamina: Option<&'static Stamina>,
    cooldowns: Option<&'static Cooldowns>,
    charges: Option<&'static Charges>,
    transform: &'static Transform,
    loco: &'static mut LocomotionInput,
    motion: Option<&'static MotionState>,
    modifiers: Option<&'static MoveModifiers>,
    body: Option<(&'static CharacterController, Option<&'static CharacterParams>)>,
    /// The host's, replicated.
    extras: Option<&'static RunExtras>,
    shown: Option<&'static mut PredictedAction>,
    dead: Has<Dead>,
}

/// `NetSet::Predict`, after `MotionSystems::PredictIntent`, before the controller: see the
/// module docs.
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
pub(crate) fn predict_actions(
    time: Res<Time>,
    db: Res<ContentDb>,
    index: Res<AiIndex>,
    tick: Res<SimTick>,
    mut net: ResMut<NetPrediction>,
    mut predictor: ResMut<ActionPredictor>,
    physics: Res<PhysicsWorld>,
    mut commands: Commands,
    positions: Query<(&Transform, Option<&CharacterParams>)>,
    mut bodies: Query<PredictedBody, With<Predicted>>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let now = tick.0;
    if predictor.epoch != net.epoch {
        // A world taken from the host again: what was predicted belonged to the old one.
        *predictor = ActionPredictor { epoch: net.epoch, ..default() };
        for body in &bodies {
            if body.shown.is_some() {
                commands.entity(body.entity).remove::<PredictedAction>();
            }
        }
        return;
    }
    let answers = std::mem::take(&mut predictor.answers);
    let Some(mut b) = bodies.iter_mut().next() else { return };
    let entity = b.entity;
    let mut lines: Vec<String> = Vec::new();
    let mut shown: Option<PredictedAction> = b.shown.as_deref().cloned();
    let host = net.host_tick;

    // 1. The host's word on what this machine predicted.
    let mut settled: Vec<(Pending, Result<(), String>)> = Vec::new();
    for answer in &answers {
        let found = predictor.pending.iter().position(|p| match answer {
            Answer::Started { tick, action, input } => {
                *tick >= p.start_tick && *action == p.action && input.as_deref() == Some(p.input.as_str())
            }
            Answer::Rejected { tick, input, .. } => *tick >= p.start_tick && *input == p.input,
        });
        if let Some(i) = found {
            let p = predictor.pending.remove(i).expect("found");
            let verdict = match answer {
                Answer::Started { .. } => Ok(()),
                Answer::Rejected { reason, .. } => Err(reason.clone()),
            };
            settled.push((p, verdict));
        }
    }
    // The host's running action, the first time it is seen: confirms, or is taken over.
    let fresh = b.state.current.as_ref().filter(|r| r.instance >= predictor.seen).cloned();
    if let Some(r) = &fresh {
        predictor.seen = r.instance + 1;
        if let Some(i) = predictor.pending.iter().position(|p| {
            p.action == r.action && r.input.as_deref() == Some(p.input.as_str()) && r.instance >= p.instance
        }) {
            let p = predictor.pending.remove(i).expect("found");
            settled.push((p, Ok(())));
        }
    }
    // A charge turned here waits for the host's word while it could still turn there, however late.
    let late = host.saturating_sub(net.lead + CONFIRM_SLACK);
    let turning = |p: &Pending| {
        let Some(from) = p.branch_from.as_deref().and_then(|a| db.get::<ActionDef>(a)) else { return false };
        let until = from.charge.as_ref().and_then(|c| c.until).unwrap_or_else(|| from.windup_end());
        shown.as_ref().is_some_and(|s| s.run.instance == p.instance && s.run.t <= until + EPS)
    };
    while let Some(i) = predictor.pending.iter().position(|p| p.start_tick < late && !turning(p)) {
        let p = predictor.pending.remove(i).expect("found");
        settled.push((p, Err(format!("not started by host tick {host}"))));
    }
    let behind = now.saturating_sub(1).saturating_sub(host);
    let host_run = b.state.current.as_ref();
    let host_extras = b.extras.copied().filter(|x| host_run.is_some_and(|r| r.instance == x.instance));
    for (p, verdict) in settled {
        let mine = shown.as_ref().is_some_and(|s| s.start_tick == p.start_tick && s.action == p.action && !s.confirmed);
        match verdict {
            Ok(()) => {
                net.confirmed += 1;
                if mine && let Some(s) = shown.as_mut() {
                    s.confirmed = true;
                }
            }
            Err(reason) => {
                net.mispredicted += 1;
                lines.push(format!("net: predicted {} not confirmed ({reason})", p.action));
                if mine {
                    // A charge the host did not turn: back to the host's action, at the host's time.
                    let unturned = p.branch_from.is_some().then(|| host_run.filter(|r| r.instance == p.instance)).flatten();
                    shown = unturned.and_then(|r| take_over(r, host_extras, &db, b.intent, behind, dt, now));
                }
            }
        }
    }

    // 2. Put the action shown where the host has it, or take the host's over.
    if let Some(r) = fresh {
        let caught_up = catch_up(r.clone(), RunExtras::extra_of(host_extras.as_ref()), &db, b.intent, behind, dt);
        let same = shown.as_ref().is_some_and(|s| s.action == r.action && s.input == r.input && r.instance >= s.run.instance);
        let older = shown.as_ref().is_some_and(|s| r.instance < s.run.instance);
        // The host's run of a charge this machine has already turned, before the host's turn: the
        // start is confirmed, and the turn waits for the host's word on it.
        let turned_here = shown.as_ref().is_some_and(|s| s.run.instance == r.instance)
            && predictor.pending.iter().any(|p| p.instance == r.instance && p.branch_from.as_deref() == Some(r.action.as_str()));
        if turned_here {
            let s = shown.as_mut().expect("turned here");
            s.expect_instance = r.instance + 1;
            adopt(&mut s.extras, host_extras);
        } else if same {
            let s = shown.as_mut().expect("same");
            s.confirmed = true;
            s.expect_instance = r.instance + 1;
            match caught_up {
                Some(run) => {
                    s.run = run;
                    adopt(&mut s.extras, host_extras);
                }
                None => shown = None,
            }
        } else if !older {
            if let Some(s) = &shown {
                lines.push(format!("net: predicted {} cut short by the host ({})", s.run.action, r.action));
            }
            shown = caught_up.map(|run| PredictedAction {
                action: r.action.clone(),
                input: r.input.clone(),
                expect_instance: r.instance + 1,
                start_tick: now,
                run,
                confirmed: true,
                extras: host_extras,
            });
        }
    } else if let Some(r) = host_run {
        let shown_here = shown.as_ref().is_some_and(|s| s.run.instance == r.instance);
        if shown_here && let Some(s) = shown.as_mut() {
            if s.run.action == r.action {
                // The same run: it takes what only the host changes (a miss's extra recovery).
                adopt(&mut s.extras, host_extras);
            } else if !predictor.pending.iter().any(|p| p.instance == r.instance && p.branch_from.is_some()) {
                // The host turned a charge this machine did not: its action and time, the hold kept here.
                let kept = s.extras;
                shown = take_over(r, host_extras, &db, b.intent, behind, dt, now).map(|mut taken| {
                    taken.extras = kept.or(taken.extras);
                    adopt(&mut taken.extras, host_extras);
                    taken
                });
            }
        } else if shown.is_none()
            && let Some((instance, extra)) = predictor.ended
            && r.instance == instance
            && host_extras.is_some_and(|x| x.extra > extra + EPS)
        {
            // Its time ran out here, but the host's recovery is longer (a miss): the host's run again.
            shown = take_over(r, host_extras, &db, b.intent, behind, dt, now);
        }
    }
    // The host ended the run shown here with no new one and no word (a cut, a release): the body
    // goes back to the host's state.
    let host_ended = shown.as_ref().is_some_and(|s| {
        s.confirmed && b.state.started > s.run.instance && host_run.is_none_or(|r| r.instance != s.run.instance)
    });
    if host_ended && predictor.pending.is_empty() {
        shown = None;
    }

    // 3. This tick's presses: buffered as the host buffers them; one may start now.
    let moveset = db.get::<MovesetDef>(&b.fighter.moveset);
    let mode = moveset.map_or(&BufferMode::PerInput, |m| &m.buffer);
    let tuning = CombatTuning::of(&db);
    let (ttl, sight) = (tuning.input_buffer, tuning.sight_height);
    if b.dead {
        predictor.queued.clear();
    } else {
        let running = shown.as_ref().and_then(|s| db.get::<ActionDef>(&s.run.action));
        let run = shown.as_ref().map(|s| &s.run);
        for press in b.intent.pressed.iter().filter(|p| understands(&p.input, moveset, running)) {
            let left = press_life(running, run, &press.input, ttl);
            buffer_press(&mut predictor.queued, mode, Queued { input: press.input.clone(), dir: press.dir, left });
        }
    }
    let here = b.transform.translation;
    let forward = *b.transform.forward();
    let locked = b.intent.target.and_then(|id| index.get(id)).and_then(|e| positions.get(e).ok().map(|p| (e, p)));
    let target = locked.map(|(_, (t, _))| t.translation);
    // The machine's sight check for an assist that needs it.
    let mine = body_height(b.body.and_then(|(_, p)| p));
    let sees = || {
        let Some((them, (there, params))) = locked else { return true };
        in_sight(&physics, (entity, here, mine), (them, there.translation, body_height(params)), sight)
    };
    let instance = shown.as_ref().map_or(0, |s| s.run.instance + 1).max(b.state.started).max(predictor.seen);
    let local = ActionState { current: shown.as_ref().map(|s| s.run.clone()), started: instance };
    let view = FighterView {
        db: &db,
        moveset: &b.fighter.moveset,
        state: &local,
        stamina: b.stamina,
        cooldowns: b.cooldowns,
        charges: b.charges,
        held: &b.intent.held,
        dead: b.dead,
    };
    let choice = (!b.dead).then(|| choose_in(&view, &predictor.queued, dt)).flatten();
    if let Some(choice) = choice
        && let Some(def) = db.get::<ActionDef>(&choice.start.action)
    {
        if let Some(i) = choice.queued {
            predictor.queued.remove(i);
        }
        if shown.is_some() {
            cap_queue(&mut predictor.queued, ttl);
        }
        let facing = start_facing(def, choice.dir, b.intent, here, forward, target);
        let run = Running::new(&choice.start.action, def, instance, facing, choice.dir, Some(choice.input.clone()));
        commands.trigger(ActionStarted {
            entity,
            action: run.action.clone(),
            input: run.input.clone(),
            predicted: true,
        });
        net.predicted += 1;
        predictor.pending.push_back(Pending {
            action: run.action.clone(),
            input: choice.input.clone(),
            instance,
            start_tick: now,
            branch_from: None,
        });
        while predictor.pending.len() > PENDING_KEPT {
            predictor.pending.pop_front();
        }
        let extras = def.needs_extras().then(|| RunExtras::start(def, &run, b.intent, here, target));
        shown = Some(PredictedAction {
            action: run.action.clone(),
            input: run.input.clone(),
            expect_instance: instance + 1,
            start_tick: now,
            run,
            confirmed: false,
            extras,
        });
    }
    if !predictor.queued.is_empty() {
        for q in &mut predictor.queued {
            q.left -= dt;
        }
        predictor.queued.retain(|q| q.left >= -EPS);
    }

    // 4. The action shown takes this tick and moves the body, turns its charge, or is let go.
    let mut external = Vec3::ZERO;
    let mut ended = false;
    if let Some(s) = shown.as_mut() {
        match db.get::<ActionDef>(&s.run.action) {
            Some(mut def) => {
                let aimed = tracked(def, b.intent, target, sees);
                (_, external) = step_run(&mut s.run, def, s.extras.as_mut(), b.intent, here, aimed, dt);
                s.run.refresh(def, RunExtras::extra_of(s.extras.as_ref()));
                if let Some(into) = charge_due(&s.run, def, s.extras.as_ref())
                    && let Some(into_def) = db.get::<ActionDef>(into)
                {
                    // Turned without paying: the host pays, or refuses (and then this goes back).
                    predictor.pending.push_back(Pending {
                        action: into.to_owned(),
                        input: s.input.clone().unwrap_or_default(),
                        instance: s.run.instance,
                        start_tick: now,
                        branch_from: Some(s.run.action.clone()),
                    });
                    while predictor.pending.len() > PENDING_KEPT {
                        predictor.pending.pop_front();
                    }
                    net.predicted += 1;
                    s.run.action = into.to_owned();
                    s.run.refresh(into_def, RunExtras::extra_of(s.extras.as_ref()));
                    s.action = into.to_owned();
                    s.start_tick = now;
                    s.confirmed = false;
                    commands.trigger(ActionStarted { entity, action: into.to_owned(), input: s.input.clone(), predicted: true });
                    def = into_def;
                }
                if let Some(x) = s.extras.as_mut() {
                    x.settle_charge(&s.run, def);
                }
                ended = def.while_held.as_ref().is_some_and(|input| !b.intent.is_held(input));
            }
            None => shown = None,
        }
    }
    // The host's `steer_body` settles the lock and then asks; the prediction asks with the lock as it is.
    let rule = sprint_rule(&db, b.motion, b.body);
    let held = b.intent.is_held(SPRINT);
    let out_of_stamina = !free_sprint(b.modifiers, b.motion) && b.stamina.is_some_and(|s| s.sprint_blocked(rule, held));
    steer_locomotion(&mut b.loco, shown.as_ref().map(|s| &s.run), &db, b.dead, out_of_stamina, external);
    // It ends in the tick its time runs out, as on the host.
    if let Some(s) = shown.as_ref()
        && s.run.t >= s.run.length - EPS
    {
        predictor.ended = Some((s.run.instance, RunExtras::extra_of(s.extras.as_ref())));
        ended = true;
    }
    if ended {
        shown = None;
        cap_queue(&mut predictor.queued, ttl);
    }

    match (shown, b.shown.as_mut()) {
        (Some(s), Some(there)) => {
            if **there != s {
                **there = s;
            }
        }
        (Some(s), None) => {
            commands.entity(entity).insert(s);
        }
        (None, Some(_)) => {
            commands.entity(entity).remove::<PredictedAction>();
        }
        (None, None) => {}
    }
    if !lines.is_empty() {
        commands.queue(move |world: &mut World| {
            for line in lines {
                journal::emit(world, JournalKind::Sim, line);
            }
        });
    }
}

/// The host's run of `ticks` ago, moved on to now; `None` once it would have ended. `extra`: the
/// host's extra recovery of it ([`RunExtras::extra`]).
fn catch_up(mut run: Running, extra: f32, db: &ContentDb, intent: &Intent, ticks: u64, dt: f32) -> Option<Running> {
    let def = db.get::<ActionDef>(&run.action)?;
    let holding = def.hold.as_ref().is_some_and(|h| intent.is_held(&h.input));
    for _ in 0..ticks {
        if run.t >= run.length - EPS {
            return None;
        }
        advance(&mut run, def, dt, holding);
    }
    run.refresh(def, extra);
    (run.t < run.length - EPS).then_some(run)
}

/// The host's run `r` (with its replicated extras), caught up to now and shown as this machine's.
fn take_over(
    r: &Running,
    extras: Option<RunExtras>,
    db: &ContentDb,
    intent: &Intent,
    ticks: u64,
    dt: f32,
    now: u64,
) -> Option<PredictedAction> {
    let run = catch_up(r.clone(), RunExtras::extra_of(extras.as_ref()), db, intent, ticks, dt)?;
    Some(PredictedAction {
        action: r.action.clone(),
        input: r.input.clone(),
        expect_instance: r.instance + 1,
        start_tick: now,
        run,
        confirmed: true,
        extras,
    })
}

/// What only the host changes in the extras of a run shown here: the extra recovery of a miss,
/// whether a hit landed and the miss check was made (and the host's numbering of the run). The
/// hold and the closing cap's count stay this machine's, which reads the input and moves the body first.
fn adopt(mine: &mut Option<RunExtras>, host: Option<RunExtras>) {
    let Some(h) = host else { return };
    match mine {
        Some(x) => {
            x.instance = h.instance;
            x.extra = x.extra.max(h.extra);
            x.landed |= h.landed;
            x.checked |= h.checked;
        }
        None => *mine = Some(h),
    }
}

/// The action the own body shows now, as an `ActionState` for [`can_start`](crate::can_start):
/// on a client's predicted body, its [`PredictedAction`] (free without one); `None`
/// anywhere else (the replicated or simulated state is the truth there).
pub fn shown_state(world: &World, entity: Entity) -> Option<ActionState> {
    let e = world.get_entity(entity).ok()?;
    if !e.contains::<Predicted>() {
        return None;
    }
    let started = e.get::<ActionState>().map_or(0, |s| s.started);
    let current = e.get::<PredictedAction>().map(|p| p.run.clone());
    let started = current.as_ref().map_or(started, |r| started.max(r.instance + 1));
    Some(ActionState { current, started })
}

/// The presses waiting for the own body on a client: the predictor's buffer, which the host's
/// follows. `None` anywhere else (the body's `InputBuffer` is the truth there).
pub fn shown_queue(world: &World, entity: Entity) -> Option<Vec<Queued>> {
    world.get::<Predicted>(entity)?;
    Some(world.get_resource::<ActionPredictor>().map(|p| p.queued.clone()).unwrap_or_default())
}

fn heard_started(
    event: On<ActionStarted>,
    bodies: Query<(), With<Predicted>>,
    net: Option<Res<NetPrediction>>,
    mut predictor: ResMut<ActionPredictor>,
) {
    let e = event.event();
    if e.predicted || !bodies.contains(e.entity) {
        return;
    }
    let tick = net.map_or(0, |n| n.event_tick);
    predictor.answers.push(Answer::Started { tick, action: e.action.clone(), input: e.input.clone() });
}

fn heard_rejected(
    event: On<ActionRejected>,
    bodies: Query<(), With<Predicted>>,
    net: Option<Res<NetPrediction>>,
    mut predictor: ResMut<ActionPredictor>,
) {
    let e = event.event();
    if !bodies.contains(e.entity) {
        return;
    }
    let tick = net.map_or(0, |n| n.event_tick);
    predictor.answers.push(Answer::Rejected { tick, input: e.input.clone(), reason: e.reason.clone() });
}

/// The own body died on the host: it takes the host's state at once.
fn heard_died(event: On<Died>, bodies: Query<(), With<Predicted>>, net: Option<ResMut<NetPrediction>>) {
    if bodies.contains(event.event().entity)
        && let Some(mut net) = net
    {
        net.snap = Some("died".into());
    }
}
