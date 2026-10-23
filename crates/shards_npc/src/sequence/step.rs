//! The step system: every tick, every run in number order carries its step on.
//!
//! A step **begins** once (`Run::begun`): it looks up its marker, forces its action, says its sound,
//! tells its signal, and journals a `SequenceStep`. Then each tick it is **checked** for done: a
//! `MoveTo` when the body is within reach of the marker, a `Face` when it faces the target, a `Play`
//! when the action run is over, a `Wait` when its ticks have gone, a `WaitFor` when the condition holds.
//! A step that is done lets the next begin on the same tick. The runs write the bodies they hold into
//! their `NpcDrive` (where to go, what to face) every tick, so the NPC's own navigation, avoidance and
//! steering carry it out, exactly as for a brain: the sequence stands in for the behaviour.

use ai_core::journal::{self, JournalKind};
use ai_core::{AiConditions, AiId, AiIndex};
use bevy::prelude::*;
use shards_action::{force_action, ActionSound, ActionState, Dead, Downed};
use shards_content::ContentDb;
use shards_core::flat;

use super::def::{FaceTarget, SequenceDef, Step, FACE_TOLERANCE_DEG};
use super::run::{Facing, Run, SequenceEnded, SequenceHold, SequenceId, SequenceSignal, SequenceStep, Sequences};
use crate::brain::NpcDrive;

/// How a run's tick came out.
enum Outcome {
    /// Carry on next tick.
    Continue,
    /// The last step is done.
    Done,
    /// It cannot go on: the reason.
    Failed(String),
    /// Something that heard one of its events ended it (`stop_sequence`) while it ran: nothing more is done for it.
    Gone,
}

/// A step's state this tick.
enum Check {
    Pending,
    Complete,
    Fail(String),
}

/// `NpcSystems::Decide`, after the behaviour system (which leaves the bodies held here alone).
pub(crate) fn run_sequences(world: &mut World) {
    let Some(ids) = world.get_resource::<Sequences>().map(|s| s.runs.iter().map(|r| r.id).collect::<Vec<_>>()) else { return };
    for id in ids {
        // An earlier run's observer may have stopped this one.
        let Some(mut run) = world.resource::<Sequences>().index(id).map(|at| world.resource::<Sequences>().runs[at].clone()) else { continue };
        match advance(world, &mut run) {
            Outcome::Continue => {
                drive(world, &run);
                store(world, run);
            }
            Outcome::Done => end(world, run, false, None),
            Outcome::Failed(why) => end(world, run, true, Some(why)),
            Outcome::Gone => {}
        }
    }
}

fn store(world: &mut World, run: Run) {
    let mut sequences = world.resource_mut::<Sequences>();
    if let Some(at) = sequences.index(run.id) {
        sequences.runs[at] = run;
    }
}

/// The seconds in a tick.
pub(crate) fn dt_of(world: &World) -> f32 {
    world.get_resource::<Time<Fixed>>().map_or(1.0 / 60.0, |t| t.timestep().as_secs_f32()).max(1e-4)
}

/// Whole ticks in `secs`, the nearest.
pub(crate) fn ticks(world: &World, secs: f32) -> u32 {
    bevy::math::ops::round(secs / dt_of(world)).max(0.0) as u32
}

fn advance(world: &mut World, run: &mut Run) -> Outcome {
    if let Some(why) = bodies_lost(world, run) {
        return Outcome::Failed(why);
    }
    let Some(def) = world.get_resource::<ContentDb>().and_then(|db| db.get::<SequenceDef>(&run.def)).cloned() else {
        return Outcome::Failed(format!("its definition `{}` is not in content any more", run.def));
    };
    // Each step is visited once: the loop ends at the first that is not done, or at the last.
    loop {
        let Some(step) = def.steps.get(run.step as usize) else { return Outcome::Done };
        if !run.begun {
            if let Err(why) = begin(world, run, &def, step) {
                return Outcome::Failed(why);
            }
            // `begin` triggers events, and whatever hears them may stop this run (a signal is something to react to):
            // it is over then and its bodies are free, so nothing more is forced, said or written for it. (An observer's commands
            // wait for a flush: it is done here, so a `stop_sequence` they hold lands before the next step.)
            world.flush();
            if world.get_resource::<Sequences>().and_then(|s| s.index(run.id)).is_none() {
                return Outcome::Gone;
            }
            run.begun = true;
            run.age = 0;
        }
        match check(world, run, step) {
            Check::Complete => {
                run.step += 1;
                run.begun = false;
                run.age = 0;
                run.left = 0;
                run.instance = None;
                run.point = None;
            }
            Check::Fail(why) => return Outcome::Failed(why),
            Check::Pending => {
                if let Some(limit) = step.limit()
                    && run.age >= ticks(world, limit)
                {
                    return Outcome::Failed(format!(
                        "step {}/{} ({}) did not finish in {limit:.1} s",
                        run.step + 1,
                        def.steps.len(),
                        step.text(&who(run, step))
                    ));
                }
                run.age += 1;
                return Outcome::Continue;
            }
        }
    }
}

/// A body some step acts on (or looks at) that is gone, dead or down: the run cannot go on.
fn bodies_lost(world: &World, run: &Run) -> Option<String> {
    let index = world.get_resource::<AiIndex>()?;
    for (role, id) in &run.roles {
        let Some(entity) = index.get(*id) else { return Some(format!("{id} ({role}) is gone")) };
        if run.actors.contains(id) && (world.get::<Dead>(entity).is_some() || world.get::<Downed>(entity).is_some()) {
            return Some(format!("{id} ({role}) is {}", if world.get::<Dead>(entity).is_some() { "dead" } else { "down" }));
        }
    }
    None
}

/// `e7` for the step's actor (empty when it has none).
fn who(run: &Run, step: &Step) -> String {
    step.actor().and_then(|role| run.body(role)).map_or_else(String::new, |id| id.to_string())
}

fn entity_of(world: &World, id: AiId) -> Option<Entity> {
    world.get_resource::<AiIndex>()?.get(id)
}

/// The place a marker named `name` is at. Several of that name (a level spawned twice): the first of the
/// lowest instance, then the file's node order; markers made by hand last, by position.
pub fn marker_point(world: &World, name: &str) -> Option<Vec3> {
    use shards_level::{FromLevel, LevelMarker};
    // `FromLevel` is looked up apart: a world without the level module has not registered it, and a query naming it would find nothing.
    let mut found: Vec<((u32, u32), Vec3)> = world
        .try_query::<(Entity, &LevelMarker, &Transform)>()?
        .iter(world)
        .filter(|(_, marker, _)| marker.name == name)
        .map(|(e, _, tf)| (world.get::<FromLevel>(e).map_or((u32::MAX, 0), |f| (f.instance, f.index)), tf.translation))
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.x.total_cmp(&b.1.x)).then(a.1.y.total_cmp(&b.1.y)).then(a.1.z.total_cmp(&b.1.z)));
    found.first().map(|f| f.1)
}

/// The names of the markers in the world, ascending, each once.
pub fn marker_names(world: &World) -> Vec<String> {
    use shards_level::LevelMarker;
    let Some(mut q) = world.try_query::<&LevelMarker>() else { return Vec::new() };
    let mut names: Vec<String> = q.iter(world).map(|m| m.name.clone()).collect();
    names.sort();
    names.dedup();
    names
}

/// Whether condition `name` (`name`, `!name`) holds, or why it cannot be told.
pub fn condition_holds(world: &World, condition: &str) -> Result<bool, String> {
    let (negated, name) = match condition.strip_prefix('!') {
        Some(name) => (true, name.trim()),
        None => (false, condition.trim()),
    };
    match world.get_resource::<AiConditions>().and_then(|c| c.0.get(name)) {
        Some(condition) => Ok(condition(world) != negated),
        None => {
            let mut known: Vec<&str> = world.get_resource::<AiConditions>().map(|c| c.0.keys().map(String::as_str).collect()).unwrap_or_default();
            known.sort_unstable();
            let hint = shards_content::db::closest(name, known.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
            Err(format!("waits on condition `{name}`, which this game has not registered{hint}; conditions: {}", known.join(", ")))
        }
    }
}

/// What a step does when it begins.
fn begin(world: &mut World, run: &mut Run, def: &SequenceDef, step: &Step) -> Result<(), String> {
    let actor = step.actor().and_then(|role| run.body(role));
    let sequence = SequenceId(run.id);
    let text = step.text(&actor.map(|a| a.to_string()).unwrap_or_default());
    match step {
        Step::MoveTo { role, marker, .. } => {
            let Some(point) = marker_point(world, marker) else {
                return Err(format!("no marker `{marker}` in the world for {}", step.text("it")));
            };
            run.point = Some(point);
            // A walk frees the facing: the body faces where it goes.
            run.set_face(role, None);
        }
        Step::Face { role, at, .. } => {
            let facing = match at {
                FaceTarget::Marker(m) => marker_point(world, m).map(Facing::Point).ok_or_else(|| format!("no marker `{m}` in the world to face"))?,
                FaceTarget::Role(r) => Facing::Body(run.body(r).ok_or_else(|| format!("role `{r}` is not bound"))?),
            };
            run.set_face(role, Some(facing));
        }
        Step::Play { action, .. } => {
            let entity = actor.and_then(|a| entity_of(world, a)).ok_or("the body that plays is gone")?;
            let started = world.get::<ActionState>(entity).map(|s| s.started).ok_or_else(|| format!("{} has no actions to play (it is no fighter)", actor.map_or(String::new(), |a| a.to_string())))?;
            if !force_action(world, entity, action) {
                return Err(format!("could not make {} play `{action}` (is it an action in content?)", actor.map_or(String::new(), |a| a.to_string())));
            }
            run.instance = Some(started);
        }
        Step::Sound { sound, .. } => {
            let entity = actor.and_then(|a| entity_of(world, a)).ok_or("the body that speaks is gone")?;
            world.trigger(ActionSound { entity, action: format!("sequence {}", run.def), sound: sound.clone() });
        }
        Step::Wait { secs } => run.left = ticks(world, *secs),
        Step::WaitFor { condition, .. } => {
            condition_holds(world, condition)?;
        }
        Step::Signal { name } => {
            world.trigger(SequenceSignal { sequence, def: run.def.clone(), name: name.clone() });
        }
    }
    world.trigger(SequenceStep {
        sequence,
        def: run.def.clone(),
        index: run.step,
        of: u32::try_from(def.steps.len()).unwrap_or(u32::MAX),
        kind: step.kind(),
        actor,
        text,
    });
    Ok(())
}

/// Whether the step under way is done.
fn check(world: &World, run: &Run, step: &Step) -> Check {
    let position = |role: &str| run.body(role).and_then(|id| entity_of(world, id)).and_then(|e| world.get::<Transform>(e)).copied();
    match step {
        Step::MoveTo { role, within, .. } => {
            let (Some(tf), Some(goal)) = (position(role), run.point) else { return Check::Fail(format!("{role} has no position")) };
            if flat(goal - tf.translation).length() <= *within + ARRIVED_SLACK { Check::Complete } else { Check::Pending }
        }
        Step::Face { role, .. } => {
            let Some(tf) = position(role) else { return Check::Fail(format!("{role} has no position")) };
            match run.face_of(role).and_then(|f| facing_point(world, f)) {
                Some(point) => match turn_needed_deg(&tf, point) {
                    Some(deg) if deg > FACE_TOLERANCE_DEG => Check::Pending,
                    _ => Check::Complete,
                },
                None => Check::Complete,
            }
        }
        Step::Play { role, .. } => {
            let Some(state) = run.body(role).and_then(|id| entity_of(world, id)).and_then(|e| world.get::<ActionState>(e)) else {
                return Check::Fail(format!("{role} has no actions to play"));
            };
            let over = state.current.as_ref().is_none_or(|r| Some(r.instance) != run.instance);
            if over { Check::Complete } else { Check::Pending }
        }
        Step::Wait { .. } => if run.age >= run.left { Check::Complete } else { Check::Pending },
        Step::WaitFor { condition, .. } => match condition_holds(world, condition) {
            Ok(true) => Check::Complete,
            Ok(false) => Check::Pending,
            Err(why) => Check::Fail(why),
        },
        Step::Sound { .. } | Step::Signal { .. } => Check::Complete,
    }
}

/// A body is at its marker when within `within` plus this (m): the walk's last tick may land a hair outside.
const ARRIVED_SLACK: f32 = 0.05;

/// Where a kept facing points now: the place, or the body's position.
fn facing_point(world: &World, facing: Facing) -> Option<Vec3> {
    match facing {
        Facing::Point(p) => Some(p),
        Facing::Body(id) => entity_of(world, id).and_then(|e| world.get::<Transform>(e)).map(|t| t.translation),
    }
}

/// Degrees the body has to turn on the ground to face `point`; `None` when it stands on it.
fn turn_needed_deg(tf: &Transform, point: Vec3) -> Option<f32> {
    let want = flat(point - tf.translation);
    let facing = flat(*tf.forward());
    if want.length_squared() < 1e-6 || facing.length_squared() < 1e-6 {
        return None;
    }
    let angle = bevy::math::ops::atan2(facing.cross(want).y.abs(), facing.dot(want));
    Some(angle.to_degrees())
}

/// Writes what the run wants of each body it holds into its `NpcDrive`: the goal of a `MoveTo` under
/// way, the point it keeps facing, nothing else (no token, no strike, no press).
fn drive(world: &mut World, run: &Run) {
    let step = world
        .get_resource::<ContentDb>()
        .and_then(|db| db.get::<SequenceDef>(&run.def))
        .and_then(|def| def.steps.get(run.step as usize))
        .cloned();
    for id in &run.actors {
        let Some(entity) = entity_of(world, *id) else { continue };
        let Some(role) = run.role_of(*id).map(str::to_owned) else { continue };
        let walking = match &step {
            Some(Step::MoveTo { role: r, within, speed, .. }) if *r == role => run.point.map(|p| (p, *within, *speed)),
            _ => None,
        };
        let face = run.face_of(&role).and_then(|f| facing_point(world, f));
        let Some(mut drive) = world.get_mut::<NpcDrive>(entity) else { continue };
        // `held` is what the body holds in its `Intent` (kept: the drive system lets go of it by itself).
        let mut next = NpcDrive { held: drive.held.clone(), ..NpcDrive::default() };
        if let Some((goal, within, speed)) = walking {
            next.goal = Some(goal);
            next.within = within;
            next.speed = speed;
        }
        next.face = face;
        if *drive != next {
            *drive = next;
        }
    }
}

/// Ends `run`: removes it, frees its bodies and tells the game. `why` is `Some` when it could not go on.
pub(crate) fn end(world: &mut World, run: Run, skipped: bool, why: Option<String>) {
    let removed = {
        let mut sequences = world.resource_mut::<Sequences>();
        sequences.index(run.id).map(|at| sequences.runs.remove(at))
    };
    // Already gone: an observer stopped it while it was being ended.
    if removed.is_none() {
        return;
    }
    for id in &run.actors {
        let Some(entity) = entity_of(world, *id) else { continue };
        if world.get::<SequenceHold>(entity).is_some_and(|h| h.run == run.id) {
            world.entity_mut(entity).remove::<SequenceHold>();
            // The brain takes over again next tick: nothing of the run's is left in what it was told.
            if let Some(mut drive) = world.get_mut::<NpcDrive>(entity) {
                let held = std::mem::take(&mut drive.held);
                *drive = NpcDrive { held, ..NpcDrive::default() };
            }
        }
    }
    let total = world.get_resource::<ContentDb>().and_then(|db| db.peek::<SequenceDef>(&run.def)).map_or(0, |d| u32::try_from(d.steps.len()).unwrap_or(u32::MAX));
    if let (true, Some(why)) = (skipped, &why) {
        journal::emit(world, JournalKind::Warn, format!("sequence {} ({}) abandoned at step {}/{total}: {why}", run.id, run.def, run.step + 1));
    }
    world.trigger(SequenceEnded { sequence: SequenceId(run.id), def: run.def, step: run.step, of: total, skipped, why });
}
