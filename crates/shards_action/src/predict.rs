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
//!    ([`steer_locomotion`]).
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
use shards_motion::{LocomotionInput, MotionState, MoveModifiers};

use crate::combat::Died;
use crate::def::{ActionDef, CombatTuning, MovesetDef, EPS};
use crate::fighter::{Charges, Cooldowns, Dead, Fighter, Stamina};
use crate::machine::{
    advance, choose_in, free_sprint, steer, steer_locomotion, step_run, understands, ActionRejected, ActionStarted, ActionState,
    FighterView, Queued, Running,
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
}

/// A prediction the host has not answered yet.
#[derive(Clone, Debug, PartialEq)]
struct Pending {
    action: String,
    input: String,
    instance: u32,
    start_tick: u64,
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
    mut commands: Commands,
    positions: Query<&Transform>,
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
    let late = host.saturating_sub(net.lead + CONFIRM_SLACK);
    while let Some(i) = predictor.pending.iter().position(|p| p.start_tick < late) {
        let p = predictor.pending.remove(i).expect("found");
        settled.push((p, Err(format!("not started by host tick {host}"))));
    }
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
                    shown = None;
                }
            }
        }
    }

    // 2. Put the action shown where the host has it, or take the host's over.
    let behind = now.saturating_sub(1).saturating_sub(host);
    if let Some(r) = fresh {
        let caught_up = catch_up(r.clone(), &db, b.intent, behind, dt);
        let same = shown.as_ref().is_some_and(|s| s.action == r.action && s.input == r.input && r.instance >= s.run.instance);
        let older = shown.as_ref().is_some_and(|s| r.instance < s.run.instance);
        if same {
            let s = shown.as_mut().expect("same");
            s.confirmed = true;
            s.expect_instance = r.instance + 1;
            match caught_up {
                Some(run) => s.run = run,
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
            });
        }
    }

    // 3. This tick's presses: buffered as the host buffers them; one may start now.
    let moveset = db.get::<MovesetDef>(&b.fighter.moveset);
    let ttl = CombatTuning::of(&db).input_buffer;
    if b.dead {
        predictor.queued.clear();
    } else {
        let running = shown.as_ref().and_then(|s| db.get::<ActionDef>(&s.run.action));
        for press in b.intent.pressed.iter().filter(|p| understands(&p.input, moveset, running)) {
            predictor.queued.retain(|q| q.input != press.input);
            predictor.queued.push(Queued { input: press.input.clone(), dir: press.dir, left: ttl });
        }
    }
    let here = b.transform.translation;
    let forward = *b.transform.forward();
    let target = b.intent.target.and_then(|id| index.get(id)).and_then(|e| positions.get(e).ok()).map(|t| t.translation);
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
        let facing = steer(def, choice.dir, b.intent, here, forward, target);
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
        });
        while predictor.pending.len() > PENDING_KEPT {
            predictor.pending.pop_front();
        }
        shown = Some(PredictedAction {
            action: run.action.clone(),
            input: run.input.clone(),
            expect_instance: instance + 1,
            start_tick: now,
            run,
            confirmed: false,
        });
    }
    if !predictor.queued.is_empty() {
        for q in &mut predictor.queued {
            q.left -= dt;
        }
        predictor.queued.retain(|q| q.left >= -EPS);
    }

    // 4. The action shown takes this tick and moves the body.
    let mut external = Vec3::ZERO;
    if let Some(s) = shown.as_mut() {
        match db.get::<ActionDef>(&s.run.action) {
            Some(def) => {
                (_, external) = step_run(&mut s.run, def, b.intent, here, target, dt);
                s.run.refresh(def);
            }
            None => shown = None,
        }
    }
    let out_of_stamina = !free_sprint(b.modifiers, b.motion)
        && b.stamina.is_some_and(|s| s.current <= EPS || (s.exhausted && b.intent.is_held(SPRINT)));
    steer_locomotion(&mut b.loco, shown.as_ref().map(|s| &s.run), &db, b.dead, out_of_stamina, external);
    // It ends in the tick its time runs out, as on the host.
    if shown.as_ref().is_some_and(|s| s.run.t >= s.run.length - EPS) {
        shown = None;
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

/// The host's run of `ticks` ago, moved on to now; `None` once it would have ended.
fn catch_up(mut run: Running, db: &ContentDb, intent: &Intent, ticks: u64, dt: f32) -> Option<Running> {
    let def = db.get::<ActionDef>(&run.action)?;
    let holding = def.hold.as_ref().is_some_and(|h| intent.is_held(&h.input));
    for _ in 0..ticks {
        if run.t >= run.length - EPS {
            return None;
        }
        advance(&mut run, def, dt, holding);
    }
    run.refresh(def);
    (run.t < run.length - EPS).then_some(run)
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
