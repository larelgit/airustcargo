//! The step: the world layer around the pure machine of [`crate::machine`].
//!
//! One exclusive system, in `SimSet::Resolve` after `ActionSystems::Deaths`: every death of the
//! tick is final there, the bodies have moved (`MotionState.volumes` is this tick's), and what
//! it spawns is first driven by the NPC systems of the next tick. It
//!
//! 1. reconciles the anchors with the runs (7.1: a run is made for an anchor that has none, a
//!    run whose anchor is gone is dropped),
//! 2. reads the participants once, in `PlayerId` then `AiId` order,
//! 3. visits the runs in key order: observe, advance, apply what the machine asked for.
//!
//! A world with no anchors and no resource returns at the first line.

use ai_core::journal::{self, JournalKind};
use ai_core::{AiId, AiIndex};
use bevy::prelude::*;
use shards_action::{reset_poise, Dead, Healed, Health};
use shards_content::{spawn_prefab, ContentDb};
use shards_core::Intent;
use shards_motion::{ColliderId, MotionState, Volume, VolumeKind};
use shards_npc::{force_state, Home, NpcBrain};
use shards_world::region::players;
use shards_world::{Dormant, RegionPart, TileId};

use crate::anchor::{region_id, run_key, EncounterAnchor};
use crate::def::{EncounterDef, StartDef};
use crate::events::*;
use crate::machine::{self, Fx, MemberObs, Obs};
use crate::run::{EncounterMember, EncounterRuns, Member, Run, RunState, NOTE_CONTENT, NOTE_LOCK, NOTE_NO_VOLUME};
use crate::spawn::{self, Part};
use crate::EncounterHooks;

/// How near its home a returning member counts as home (the NPC module's `at_home`).
const HOME_RADIUS: f32 = 0.6;

/// Whether a participant counts as inside the start condition.
type Inside = Box<dyn Fn(&World, &Part) -> bool>;

/// An anchor entity as the step sees it.
#[derive(Clone, Debug)]
pub(crate) struct Anchor {
    pub key: String,
    pub encounter: String,
    pub at: Vec3,
    pub yaw: f32,
    pub tile: Option<TileId>,
}

fn flat(v: Vec3) -> Vec3 {
    Vec3::new(v.x, 0.0, v.z)
}

fn mm(v: f32) -> i64 {
    (v * 1000.0).round() as i64
}

/// The anchors of the world, sorted by key, then position (x, z, y in millimetres): the first of
/// two with one key wins.
pub(crate) fn anchors(world: &mut World) -> Vec<Anchor> {
    let region = region_id(world).map(str::to_owned);
    let mut q = world.query::<(&EncounterAnchor, &Transform, Option<&TileId>)>();
    let mut found: Vec<Anchor> = q
        .iter(world)
        .map(|(a, t, tile)| Anchor {
            key: run_key(region.as_deref(), a, t.translation),
            encounter: a.encounter.clone(),
            at: t.translation,
            yaw: t.rotation.to_euler(EulerRot::YXZ).0,
            tile: tile.copied(),
        })
        .collect();
    found.sort_by(|a, b| a.key.cmp(&b.key).then((mm(a.at.x), mm(a.at.z), mm(a.at.y)).cmp(&(mm(b.at.x), mm(b.at.z), mm(b.at.y)))));
    found
}

/// Seconds per tick.
pub(crate) fn dt_of(world: &World) -> f32 {
    world.get_resource::<Time<Fixed>>().map_or(1.0 / 60.0, |t| t.timestep().as_secs_f32())
}

/// The participants (player bodies) with whether each is out, where it stands and which way it
/// faces. Facing is simulation state: the lock-on target (`Intent.target`), else the point it
/// aims at (`Intent.aim`), else the body's flat forward. Never the camera.
pub(crate) fn participants(world: &mut World) -> Vec<Part> {
    let hooks = world.get_resource::<EncounterHooks>().cloned().unwrap_or_default();
    let bodies = players(world);
    let mut parts = Vec::with_capacity(bodies.len());
    for entity in bodies {
        let Some(tf) = world.get::<Transform>(entity) else { continue };
        let pos = tf.translation;
        let forward = flat(*tf.forward()).try_normalize().unwrap_or(Vec3::NEG_Z);
        let mut facing = forward;
        if let Some(intent) = world.get::<Intent>(entity) {
            let toward = match (intent.target, intent.aim) {
                (Some(id), _) => world.get_resource::<AiIndex>().and_then(|i| i.get(id)).and_then(|e| world.get::<Transform>(e)).map(|t| t.translation),
                (None, aim) => aim,
            };
            if let Some(point) = toward
                && let Some(dir) = flat(point - pos).try_normalize()
            {
                facing = dir;
            }
        }
        let out = world.get::<Dead>(entity).is_some() || hooks.out_rules.iter().any(|(_, rule)| rule(world, entity));
        parts.push(Part { entity, pos, facing, out });
    }
    parts
}

/// The exclusive system.
pub fn step(world: &mut World) {
    let has_runs = world.contains_resource::<EncounterRuns>();
    let found = anchors(world);
    if !has_runs && found.is_empty() {
        return;
    }
    let dt = dt_of(world);
    let parts = participants(world);
    reconcile(world, &found, &parts);
    let keys: Vec<String> = world.get_resource::<EncounterRuns>().map(|r| r.runs.iter().map(|r| r.key.clone()).collect()).unwrap_or_default();
    for key in keys {
        let tile = found.iter().find(|a| a.key == key).and_then(|a| a.tile);
        visit(world, &key, &parts, tile, dt);
    }
    drop_when_empty(world);
}

fn def_of(world: &World, id: &str) -> Option<EncounterDef> {
    world.get_resource::<ContentDb>()?.get::<EncounterDef>(id).cloned()
}

// --- anchors and runs -----------------------------------------------------------------------

/// A run for every anchor that has none; no run for an anchor that is gone.
fn reconcile(world: &mut World, found: &[Anchor], parts: &[Part]) {
    // The first anchor of a key wins; the others are said once.
    let mut seen: Vec<&Anchor> = Vec::new();
    for anchor in found {
        if seen.iter().any(|a| a.key == anchor.key) {
            let text = format!("duplicate {}", anchor.key);
            if world.get_resource_or_init::<EncounterRuns>().report(&text) {
                journal::emit(
                    world,
                    JournalKind::Error,
                    format!("encounter: two anchors are keyed {}; give one its own key", anchor.key),
                );
            }
            continue;
        }
        seen.push(anchor);
    }
    // Runs whose anchor is gone.
    let gone: Vec<String> = world
        .get_resource::<EncounterRuns>()
        .map(|r| r.runs.iter().filter(|run| !seen.iter().any(|a| a.key == run.key)).map(|run| run.key.clone()).collect())
        .unwrap_or_default();
    for key in gone {
        drop_run(world, &key);
    }
    // Runs for anchors that have none.
    for anchor in seen {
        if world.get_resource::<EncounterRuns>().is_some_and(|r| r.get(&anchor.key).is_some()) {
            continue;
        }
        make_run(world, anchor, parts);
    }
}

fn make_run(world: &mut World, anchor: &Anchor, parts: &[Part]) {
    let Some(def) = def_of(world, &anchor.encounter) else {
        let text = format!("unknown {}", anchor.key);
        if world.get_resource_or_init::<EncounterRuns>().report(&text) {
            let ids: Vec<String> = world.resource::<ContentDb>().peek_ids::<EncounterDef>().map(str::to_owned).collect();
            let hint = crate::def::did_you_mean(&anchor.encounter, ids.iter().map(String::as_str));
            journal::emit(
                world,
                JournalKind::Error,
                format!("encounter {}: no encounter `{}` in content{hint}; the anchor stands for nothing", anchor.key, anchor.encounter),
            );
        }
        return;
    };
    let mut run = Run {
        key: anchor.key.clone(),
        encounter: anchor.encounter.clone(),
        at: anchor.at,
        yaw: anchor.yaw,
        ..default()
    };
    let completed = world.get_resource::<EncounterRuns>().is_some_and(|r| r.completed.contains(&anchor.key));
    if completed {
        // A load or a revisit: won, with no members and no event.
        run.state = RunState::Won;
    } else if def.placed() {
        spawn::spawn_wave(world, &mut run, &def, 1, parts, anchor.tile);
        run.wave = 1;
    }
    let mut runs = world.get_resource_or_init::<EncounterRuns>();
    if let Err(at) = runs.index(&run.key) {
        runs.runs.insert(at, run);
    }
}

/// A run whose anchor is gone: its living members are despawned silently.
fn drop_run(world: &mut World, key: &str) {
    let Some(run) = world.get_resource::<EncounterRuns>().and_then(|r| r.get(key)).cloned() else { return };
    let ids: Vec<AiId> = run.members.iter().map(|m| m.id).collect();
    despawn_ids(world, &ids);
    if let Some(lock) = run.lock {
        despawn_ids(world, &[lock]);
    }
    if let Some(mut runs) = world.get_resource_mut::<EncounterRuns>()
        && let Ok(at) = runs.index(key)
    {
        runs.runs.remove(at);
    }
    journal::log(world, JournalKind::Event, format!("encounter {key}: gone with its anchor"));
}

fn take_run(world: &World, key: &str) -> Option<Run> {
    world.get_resource::<EncounterRuns>()?.get(key).cloned()
}

fn put_run(world: &mut World, run: Run) {
    let mut runs = world.get_resource_or_init::<EncounterRuns>();
    match runs.index(&run.key) {
        Ok(at) => runs.runs[at] = run,
        Err(at) => runs.runs.insert(at, run),
    }
}

// --- one visit ------------------------------------------------------------------------------

fn visit(world: &mut World, key: &str, parts: &[Part], tile: Option<TileId>, dt: f32) {
    let Some(mut run) = take_run(world, key) else { return };
    let Some(def) = def_of(world, &run.encounter) else {
        // The definition is gone (an editor session): the run goes with it.
        journal::emit(world, JournalKind::Error, format!("encounter {key}: its definition `{}` is gone; the run is dropped", run.encounter));
        drop_run(world, key);
        return;
    };
    settle_returning(world, &mut run);
    let obs = observe(world, &mut run, &def, parts, dt);
    let mut fx = Vec::new();
    if run.wave as usize > def.waves.len() {
        // The definition shrank under the run (an editor session; replays never reload content).
        if run.noted & NOTE_CONTENT == 0 {
            run.noted |= NOTE_CONTENT;
            journal::emit(
                world,
                JournalKind::Error,
                format!("encounter {key}: its definition now has {} waves, the run was in wave {}; reset", def.waves.len(), run.wave),
            );
        }
        if run.state.fighting() && !matches!(run.state, RunState::Resetting { .. }) {
            machine::reset(&mut run, &def, &obs, ResetWhy::Content, &mut fx);
        }
        run.wave = run.wave.min(def.waves.len() as u32);
    } else {
        machine::advance(&mut run, &def, &obs, &mut fx);
    }
    apply(world, &mut run, &def, fx, parts, tile);
    reconcile_lock(world, &mut run, &def);
    put_run(world, run);
}

/// What the world shows about `run`.
fn observe(world: &mut World, run: &mut Run, def: &EncounterDef, parts: &[Part], dt: f32) -> Obs {
    let standing = parts.iter().filter(|p| !p.out).count() as u32;
    let near = parts.iter().any(|p| !p.out && flat(p.pos - run.at).length() <= def.radius);
    let start_met = match run.state {
        RunState::Idle | RunState::Armed => start_condition(world, run, def, parts),
        _ => false,
    };
    let index = world.get_resource::<AiIndex>();
    let members: Vec<MemberObs> = run
        .members
        .iter()
        .map(|m| {
            let Some(entity) = index.and_then(|i| i.get(m.id)) else { return MemberObs { out: true, disturbed: true, hunting: false } };
            let out = world.get::<Dead>(entity).is_some();
            let hurt = world.get::<Health>(entity).is_some_and(|h| h.current < h.max - 1e-4);
            // Hunting someone standing within the radius: a member that still holds the target of a
            // fight the players have walked away from (a pull-back) is not a pack being fought.
            let hunting = world.get::<NpcBrain>(entity).and_then(|b| b.target).and_then(|t| index.and_then(|i| i.get(t))).is_some_and(|target| {
                parts.iter().any(|p| p.entity == target && !p.out && flat(p.pos - run.at).length() <= def.radius)
            });
            MemberObs { out, disturbed: out || hurt || hunting, hunting }
        })
        .collect();
    let all_home = !run.members.iter().any(|m| {
        index.and_then(|i| i.get(m.id)).and_then(|e| world.get::<EncounterMember>(e)).is_some_and(|em| em.returning)
    });
    Obs { dt, participants: parts.len() as u32, standing, near, start_met, members, all_home }
}

/// Whether the start condition holds now (7.3). A trigger volume that is not there is said once.
fn start_condition(world: &mut World, run: &mut Run, def: &EncounterDef, parts: &[Part]) -> bool {
    let standing: Vec<&Part> = parts.iter().filter(|p| !p.out).collect();
    let (all, inside): (bool, Inside) = match &def.start {
        StartDef::Manual => return false,
        StartDef::Near { within, all_players_in } => {
            let (within, at) = (*within, run.at);
            (*all_players_in, Box::new(move |_, p| flat(p.pos - at).length() <= within))
        }
        StartDef::Trigger { volume, all_players_in } => {
            let Some(vol) = find_volume(world, run, volume, def.radius) else {
                if run.noted & NOTE_NO_VOLUME == 0 {
                    run.noted |= NOTE_NO_VOLUME;
                    journal::emit(
                        world,
                        JournalKind::Warn,
                        format!(
                            "encounter {}: no trigger volume `{volume}` within {:.0} m of its anchor; check the volume's name",
                            run.key, def.radius
                        ),
                    );
                }
                return false;
            };
            (*all_players_in, Box::new(move |w, p| w.get::<MotionState>(p.entity).is_some_and(|m| m.volumes.contains(&vol))))
        }
    };
    if standing.is_empty() {
        return false;
    }
    if all { standing.iter().all(|p| inside(world, p)) } else { standing.iter().any(|p| inside(world, p)) }
}

/// The trigger volume `name`: the nearest within `radius` of the anchor (the anchor's own is at
/// distance 0); ties go by `ColliderId`.
pub(crate) fn find_volume(world: &mut World, run: &Run, name: &str, radius: f32) -> Option<Entity> {
    let mut q = world.query::<(Entity, &Volume, &Transform, Option<&ColliderId>)>();
    q.iter(world)
        .filter(|(_, v, ..)| matches!(&v.kind, VolumeKind::Trigger { name: n } if n == name))
        .filter_map(|(e, _, t, id)| {
            let d = flat(t.translation - run.at).length();
            (d <= radius).then(|| ((mm(d), id.map_or(u32::MAX, |c| c.0)), e))
        })
        .min_by_key(|(order, _)| *order)
        .map(|(_, e)| e)
}

// --- applying what the machine asked --------------------------------------------------------

fn apply(world: &mut World, run: &mut Run, def: &EncounterDef, fx: Vec<Fx>, parts: &[Part], tile: Option<TileId>) {
    for f in fx {
        match f {
            Fx::Started(e) => world.trigger(e),
            Fx::Warned(e) => world.trigger(e),
            Fx::Cleared(e) => world.trigger(e),
            Fx::Completed(e) => world.trigger(e),
            Fx::Wiped(e) => world.trigger(e),
            Fx::Resumed(e) => world.trigger(e),
            Fx::Reset(e) => world.trigger(e),
            Fx::Spawn { wave } => {
                let members = spawn::spawn_wave(world, run, def, wave, parts, tile);
                world.trigger(WaveStarted {
                    key: run.key.clone(),
                    encounter: run.encounter.clone(),
                    wave,
                    waves: def.waves.len() as u32,
                    members,
                });
            }
            Fx::Despawn(ids) => despawn_ids(world, &ids),
            Fx::Retire(ids) | Fx::Release(ids) => unmark(world, &ids),
            Fx::PullBack(ids) => pull_back(world, def, &ids),
            Fx::PlaceHome => {
                let ids: Vec<AiId> = run.members.iter().map(|m| m.id).collect();
                for (_, entity) in spawn::resolve(world, &ids) {
                    if world.get::<EncounterMember>(entity).is_some_and(|m| m.returning) {
                        place_home(world, entity);
                    }
                }
            }
            Fx::Remember => {
                let key = run.key.clone();
                world.get_resource_or_init::<EncounterRuns>().complete(&key);
            }
        }
    }
}

/// Despawns the members `ids` silently (no `Died`, so no loot and no death cue). Each hook the
/// game registered cleans what the member owned first (projectiles, zones, summons).
pub(crate) fn despawn_ids(world: &mut World, ids: &[AiId]) {
    let hooks = world.get_resource::<EncounterHooks>().cloned().unwrap_or_default();
    for (_, entity) in spawn::resolve(world, ids) {
        for (_, hook) in &hooks.despawn {
            hook(world, entity);
        }
        world.despawn(entity);
    }
}

/// Takes the marker off members that stay in the world (corpses, released adds).
fn unmark(world: &mut World, ids: &[AiId]) {
    for (_, entity) in spawn::resolve(world, ids) {
        world.entity_mut(entity).remove::<EncounterMember>();
    }
}

/// Survivors of a placed wave go back : healed, and sent to their home by the
/// behaviour state the encounter names, or placed there at once.
fn pull_back(world: &mut World, def: &EncounterDef, ids: &[AiId]) {
    for (_, entity) in spawn::resolve(world, ids) {
        if def.pull_back.heal {
            let mut amount = None;
            if let Some(mut health) = world.get_mut::<Health>(entity) {
                let gained = health.max - health.current;
                health.current = health.max;
                amount = Some((gained, health.current));
            }
            reset_poise(world, entity);
            if let Some((amount, hp)) = amount {
                world.trigger(Healed { entity, amount, hp });
            }
        }
        if let Some(mut member) = world.get_mut::<EncounterMember>(entity) {
            member.returning = true;
        }
        let dormant = world.get::<Dormant>(entity).is_some();
        match &def.pull_back.state {
            // A dormant survivor's brain does not run: it never walks, so it is placed.
            Some(state) if !dormant => {
                if let Err(why) = force_state(world, entity, state) {
                    journal::emit(world, JournalKind::Warn, format!("encounter: cannot send a survivor to `{state}`: {why}; placed it home"));
                    place_home(world, entity);
                }
            }
            _ => place_home(world, entity),
        }
    }
}

/// Places `entity` at its `Home`, standing still, and clears its returning flag.
pub(crate) fn place_home(world: &mut World, entity: Entity) {
    if let Some(home) = world.get::<Home>(entity).and_then(|h| h.pos)
        && let Some(mut tf) = world.get_mut::<Transform>(entity)
    {
        tf.translation = home;
        if let Some(mut motion) = world.get_mut::<MotionState>(entity) {
            motion.velocity = Vec3::ZERO;
        }
    }
    if let Some(mut member) = world.get_mut::<EncounterMember>(entity) {
        member.returning = false;
    }
}

/// A member that reached its home (or has none to go to) stops returning.
fn settle_returning(world: &mut World, run: &mut Run) {
    let ids: Vec<AiId> = run.members.iter().map(|m| m.id).collect();
    for (_, entity) in spawn::resolve(world, &ids) {
        if !world.get::<EncounterMember>(entity).is_some_and(|m| m.returning) {
            continue;
        }
        let here = world.get::<Transform>(entity).map(|t| t.translation);
        let home = world.get::<Home>(entity).and_then(|h| h.pos);
        let arrived = match (here, home) {
            (Some(here), Some(home)) => flat(here - home).length() <= HOME_RADIUS,
            _ => true,
        } || world.get::<Dead>(entity).is_some();
        if arrived && let Some(mut member) = world.get_mut::<EncounterMember>(entity) {
            member.returning = false;
        }
    }
}

// --- the lock -------------------------------------------------------------------------------

/// The lock stands in `Running` and `Between`, and not otherwise (7.10).
fn reconcile_lock(world: &mut World, run: &mut Run, def: &EncounterDef) {
    let want = def.lock.is_some() && matches!(run.state, RunState::Running | RunState::Between { .. });
    let have = run.lock.and_then(|id| world.get_resource::<AiIndex>().and_then(|i| i.get(id)));
    if want && have.is_none() {
        let Some(prefab) = &def.lock else { return };
        let at = Transform::from_translation(run.at).with_rotation(Quat::from_rotation_y(run.yaw));
        let id = spawn_prefab(world, prefab, at, None).ok().and_then(|e| {
            let id = world.get::<AiId>(e).copied().filter(|id| id.0 != 0);
            if id.is_some() {
                world.entity_mut(e).insert(RegionPart);
            } else {
                world.despawn(e);
            }
            id
        });
        match id {
            Some(id) => run.lock = Some(id),
            None if run.noted & NOTE_LOCK == 0 => {
                run.noted |= NOTE_LOCK;
                journal::emit(world, JournalKind::Error, format!("encounter {}: the lock prefab `{prefab}` did not spawn or has no AiId (give it AiKind)", run.key));
            }
            None => {}
        }
    } else if !want && let Some(id) = run.lock.take() {
        despawn_ids(world, &[id]);
    }
}

// --- the public operations ------------------------------------------------------------------

/// Builds the observation of run `key` now, outside the tick loop (tools).
/// A run with what the world shows about it, for the public operations.
type Context = (Run, EncounterDef, Vec<Part>, Obs, Option<TileId>);

fn context(world: &mut World, key: &str) -> Result<Context, String> {
    let run = take_run(world, key).ok_or_else(|| crate::no_run(world, key))?;
    let def = def_of(world, &run.encounter).ok_or_else(|| format!("encounter `{}` is not in content", run.encounter))?;
    let parts = participants(world);
    let dt = dt_of(world);
    let mut run = run;
    let obs = observe(world, &mut run, &def, &parts, dt);
    let tile = anchors(world).iter().find(|a| a.key == key).and_then(|a| a.tile);
    Ok((run, def, parts, obs, tile))
}

/// `start_encounter`: starts a run in `Idle` or `Armed` as its start condition would. With
/// `wave`, skips to that wave (1-based, nothing despawned).
pub fn start(world: &mut World, key: &str, wave: Option<u32>) -> Result<String, String> {
    let (mut run, def, parts, obs, tile) = context(world, key)?;
    if !matches!(run.state, RunState::Idle | RunState::Armed) {
        return Err(format!("run {key} is {}: only an Idle or Armed run can be started (`encounter_reset key={key}` first)", run.state.name()));
    }
    if let Some(wave) = wave {
        if wave == 0 || wave as usize > def.waves.len() {
            return Err(format!("wave {wave} is outside 1..={} for encounter `{}`", def.waves.len(), run.encounter));
        }
        // Wave 1 of a placed encounter already stands: starting it is the plain start; wave 2 and later skip it.
        if !(def.placed() && wave == 1) {
            run.wave = wave - 1;
        }
    }
    let mut fx = Vec::new();
    machine::start_now(&mut run, &def, &obs, wave.is_some_and(|w| w >= 2), &mut fx);
    let state = run.state.name();
    apply(world, &mut run, &def, fx, &parts, tile);
    reconcile_lock(world, &mut run, &def);
    put_run(world, run);
    Ok(format!("encounter {key} started: {state}"))
}

/// `reset_encounter`: resets a run as if nobody had been near (`why: Dev`), from any state but
/// `Idle` and `Armed`. With `forget`, also drops the key from `completed` and a `Won` run, so a
/// scenario can leave the world as it found it.
pub fn reset(world: &mut World, key: &str, forget: bool) -> Result<String, String> {
    let (mut run, def, parts, obs, tile) = context(world, key)?;
    if forget {
        let ids: Vec<AiId> = run.members.iter().map(|m| m.id).collect();
        let lock = run.lock;
        despawn_ids(world, &ids);
        if let Some(lock) = lock {
            despawn_ids(world, &[lock]);
        }
        if let Some(mut runs) = world.get_resource_mut::<EncounterRuns>() {
            if let Ok(at) = runs.index(key) {
                runs.runs.remove(at);
            }
            runs.completed.retain(|k| k != key);
        }
        journal::log(world, JournalKind::Event, format!("encounter {key}: forgotten (dev)"));
        return Ok(format!("run {key} forgotten: it is made again from its anchor on the next tick"));
    }
    if matches!(run.state, RunState::Idle | RunState::Armed) {
        return Ok(format!("run {key} is {}: nothing to reset", run.state.name()));
    }
    if run.state == RunState::Won {
        return Err(format!("run {key} is Won: `encounter_reset key={key} forget=true` makes it fresh again"));
    }
    let mut fx = Vec::new();
    machine::reset(&mut run, &def, &obs, ResetWhy::Dev, &mut fx);
    apply(world, &mut run, &def, fx, &parts, tile);
    reconcile_lock(world, &mut run, &def);
    put_run(world, run);
    Ok(format!("run {key} reset"))
}

/// Puts `entity` under run `key` (`adopt`). A counted add holds the wave in progress open. Host
/// only: it changes simulation state, so a machine that does not simulate refuses (a client sees
/// the adoption by replication).
pub fn adopt(world: &mut World, key: &str, entity: Entity, counted: bool) -> Result<(), String> {
    if world.get_resource::<shards_core::NetRole>().is_some_and(|r| !r.simulates()) {
        return Err("adopt changes the simulation: call it where the simulation runs (the host)".to_owned());
    }
    let mut run = take_run(world, key).ok_or_else(|| crate::no_run(world, key))?;
    let id = world.get::<AiId>(entity).copied().filter(|id| id.0 != 0).ok_or("the entity has no AiId (give it AiKind), so a run cannot track it")?;
    if run.members.iter().any(|m| m.id == id) {
        return Err(format!("{id} is already a member of {key}"));
    }
    let wave = if counted { run.wave.max(1) } else { 0 };
    world.entity_mut(entity).insert(EncounterMember { key: key.to_owned(), wave, counted, returning: false });
    run.members.push(Member { id, wave, counted });
    run.members.sort_by_key(|m| m.id);
    put_run(world, run);
    Ok(())
}

/// A world save is loaded (`encounters` section): every run's living members, adds and lock are
/// despawned silently, the runs are cleared and `completed` is what the save holds. The next
/// tick makes fresh runs from the anchors: the completed keys come back `Won` with no members
/// and no events, the rest `Idle` (placed waves spawned anew). The phase of a fight is not saved.
/// Returns the journal line.
pub fn restore_completed(world: &mut World, mut completed: Vec<String>) -> Option<String> {
    let had = world.contains_resource::<EncounterRuns>();
    let runs = world.remove_resource::<EncounterRuns>().unwrap_or_default();
    for run in &runs.runs {
        let mut ids: Vec<AiId> = run.members.iter().map(|m| m.id).collect();
        ids.extend(run.lock);
        despawn_ids(world, &ids);
    }
    completed.sort();
    completed.dedup();
    let note = (!completed.is_empty()).then(|| format!("encounters: {} won restored ({})", completed.len(), completed.join(", ")));
    // A host that had the resource keeps it (see `drop_when_empty`).
    if !completed.is_empty() || (had && world.get_resource::<shards_core::NetRole>().is_some_and(|r| *r == shards_core::NetRole::Host)) {
        world.insert_resource(EncounterRuns { completed, ..default() });
    }
    note
}

/// The keys of the encounters won for good (the `encounters` save section).
pub fn completed(world: &World) -> Vec<String> {
    world.get_resource::<EncounterRuns>().map(|r| r.completed.clone()).unwrap_or_default()
}

/// Removes the resource when it holds nothing, so that a game with no run has the snapshots and
/// replay hashes it had before the module. Not on a host: the net layer replicates a resource
/// that appears or changes, not one that goes, so a client would keep the runs it was last sent
/// (and desync on `EncounterRuns`). A host keeps the empty resource instead.
pub(crate) fn drop_when_empty(world: &mut World) {
    let host = world.get_resource::<shards_core::NetRole>().is_some_and(|r| *r == shards_core::NetRole::Host);
    if !host && world.get_resource::<EncounterRuns>().is_some_and(EncounterRuns::is_empty) {
        world.remove_resource::<EncounterRuns>();
    }
}
