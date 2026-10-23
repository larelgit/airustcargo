//! The movers' tick: the host's step ([`step_movers`]) and a co-op client's prediction of its own
//! body on them ([`predict_movers`]).

use std::collections::BTreeMap;

use ai_core::{sim, AiId};
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::{NetPrediction, Predicted};

use super::def::{Blocked, MoverDef};
use super::run::{self, Note};
use super::{movers_in_order, Mover, MoverArrived, MoverBlocked, MoverPart, MoverStarted, MoverState};
use crate::controller::mover::Mover as Capsule;
use crate::controller::{CharacterController, CharacterParams, Crouch, MotionMode, MotionState};
use crate::physics::{Body, Collider, ColliderId, CollisionLayers, PhysicsWorld, Volume};

/// A client runs a mover's state on by at most this many ticks past the host's (two seconds).
const MAX_AHEAD: u64 = 120;

/// Pushes per body and mover: a corner of two of the mover's colliders takes two.
const PUSHES: usize = 4;

/// Every mover's parts by the mover's id: `(part id, part, offset)`, each list in `AiId` order.
type Parts = BTreeMap<AiId, Vec<(AiId, Entity, Transform)>>;

/// A mover as a client foresees it: its id, its pieces, and its poses a tick before and at this
/// tick (`None`: it stands as replicated).
struct Foreseen {
    id: AiId,
    pieces: Vec<Piece>,
    poses: Option<(Transform, Transform)>,
}

/// A character as movers see it.
struct Walker {
    entity: Entity,
    params: CharacterParams,
    layers: CollisionLayers,
    feet: Vec3,
    /// It stands on the ground (not in the air, the water or a climb).
    grounded: bool,
}

/// An entity that moves with a mover: the mover itself (`offset` identity) or one of its parts.
struct Piece {
    entity: Entity,
    offset: Transform,
    /// It has a collider (a part can be a marker that only moves).
    solid: bool,
}

/// One mover's step, worked out before it is written.
struct Plan {
    entity: Entity,
    def: MoverDef,
    before: MoverState,
    after: MoverState,
    notes: Vec<Note>,
    pieces: Vec<Piece>,
}

/// What a step makes happen, triggered once every mover has taken its step.
enum Fire {
    Started(MoverStarted),
    Arrived(MoverArrived),
    Blocked(MoverBlocked),
}

/// The characters, in [`ColliderId`] order.
fn walkers(world: &mut World) -> Vec<Walker> {
    let Some(mut q) = world.try_query::<(
        Entity,
        &ColliderId,
        &CharacterController,
        Option<&CharacterParams>,
        &Collider,
        &Transform,
        Option<&MotionState>,
        Option<&Crouch>,
    )>() else {
        return Vec::new();
    };
    let db = world.get_resource::<ContentDb>();
    let mut out: Vec<(ColliderId, Walker)> = q
        .iter(world)
        .map(|(entity, id, controller, own, collider, tf, state, crouch)| {
            let walker = Walker {
                entity,
                // The size it has now: a crouched body rides, and is pushed, as the shorter capsule it is.
                params: controller.resolve(own, db).at_depth(crouch.map_or(0.0, |c| c.depth)).into_owned(),
                layers: collider.layers,
                feet: tf.translation,
                grounded: state.is_some_and(|s| s.mode == MotionMode::Ground && s.grounded),
            };
            (*id, walker)
        })
        .collect();
    out.sort_unstable_by_key(|(id, _)| *id);
    out.into_iter().map(|(_, w)| w).collect()
}

/// The mover `entity` (id `id`) and its parts in `AiId` order.
fn pieces(world: &World, entity: Entity, parts: &Parts, id: AiId) -> Vec<Piece> {
    let solid = |e: Entity| world.get::<Collider>(e).is_some();
    let mut out = vec![Piece { entity, offset: Transform::IDENTITY, solid: solid(entity) }];
    for &(_, part, offset) in parts.get(&id).into_iter().flatten() {
        out.push(Piece { entity: part, offset, solid: solid(part) });
    }
    out
}

/// Every part by its mover's id, each list in `AiId` order.
fn parts_by_mover(world: &mut World) -> Parts {
    let mut out: Parts = BTreeMap::new();
    let Some(mut q) = world.try_query::<(Entity, &AiId, &MoverPart)>() else { return out };
    for (entity, id, part) in q.iter(world) {
        out.entry(part.mover).or_default().push((*id, entity, part.offset));
    }
    for list in out.values_mut() {
        list.sort_unstable_by_key(|(id, ..)| *id);
    }
    out
}

/// Where a piece stands when its mover stands at `pose`.
fn piece_pose(pose: &Transform, piece: &Piece) -> Transform {
    Transform {
        translation: pose.translation + pose.rotation * piece.offset.translation,
        rotation: (pose.rotation * piece.offset.rotation).normalize(),
        scale: Vec3::ONE,
    }
}

/// Puts the solid pieces at `pose` in the physics world, as `body` (`None`: the body each entity
/// has); the entities stay as they are.
fn place(world: &mut World, pieces: &[Piece], pose: &Transform, body: Option<Body>) {
    let colliders: Vec<(Entity, ColliderId, Collider, Option<Volume>, Transform, Body)> = pieces
        .iter()
        .filter(|p| p.solid)
        .filter_map(|p| {
            let id = *world.get::<ColliderId>(p.entity)?;
            let collider = *world.get::<Collider>(p.entity)?;
            let body = body.or_else(|| world.get::<Body>(p.entity).copied()).unwrap_or_default();
            Some((p.entity, id, collider, world.get::<Volume>(p.entity).cloned(), piece_pose(pose, p), body))
        })
        .collect();
    let mut physics = world.resource_mut::<PhysicsWorld>();
    for (entity, id, collider, volume, tf, body) in &colliders {
        physics.upsert((*entity, id, collider, tf, Some(body), volume.as_ref()));
    }
}

/// Puts the solid pieces back in the physics world as their entities have them.
fn restore(world: &mut World, pieces: &[Piece]) {
    let colliders: Vec<(Entity, ColliderId, Collider, Option<Volume>, Transform, Body)> = pieces
        .iter()
        .filter(|p| p.solid)
        .filter_map(|p| {
            let e = world.get_entity(p.entity).ok()?;
            Some((p.entity, *e.get::<ColliderId>()?, *e.get::<Collider>()?, e.get::<Volume>().cloned(), *e.get::<Transform>()?, e.get::<Body>().copied().unwrap_or_default()))
        })
        .collect();
    let mut physics = world.resource_mut::<PhysicsWorld>();
    for (entity, id, collider, volume, tf, body) in &colliders {
        physics.upsert((*entity, id, collider, tf, Some(body), volume.as_ref()));
    }
}

/// Which mover (by id) each collider of a mover belongs to.
fn owners(world: &World, movers: &[(AiId, Entity)], parts: &Parts) -> BTreeMap<Entity, AiId> {
    let mut out = BTreeMap::new();
    for &(id, entity) in movers {
        out.insert(entity, id);
        for &(_, part, _) in parts.get(&id).into_iter().flatten() {
            if world.get::<Collider>(part).is_some() {
                out.insert(part, id);
            }
        }
    }
    out
}

/// The mover each grounded walker stands on, asked before any mover moves.
fn riding(world: &World, walkers: &[Walker], owners: &BTreeMap<Entity, AiId>) -> Vec<Option<AiId>> {
    let physics = world.resource::<PhysicsWorld>();
    walkers
        .iter()
        .map(|w| {
            if !w.grounded {
                return None;
            }
            let capsule = Capsule::new(physics, &w.params, w.entity, w.layers);
            let ground = capsule.ground(w.feet, w.params.snap_distance).filter(|g| g.walkable)?;
            owners.get(&ground.entity).copied()
        })
        .collect()
}

/// The yaw (about Y) of a rotation: how far it turns the forward direction on the ground plane.
fn yaw_of(rotation: Quat) -> f32 {
    let forward = rotation * Vec3::NEG_Z;
    if Vec2::new(forward.x, forward.z).length_squared() < 1e-8 {
        return 0.0;
    }
    ops::atan2(-forward.x, -forward.z)
}

/// The box the solid pieces fill in the physics world now, if any.
#[expect(clippy::disallowed_methods, reason = "a mover's pieces are colliders (a voxel field is never a mover's piece): their boxes")]
fn bounds(physics: &PhysicsWorld, colliders: &[Entity]) -> Option<(Vec3, Vec3)> {
    colliders.iter().filter_map(|e| physics.collider(*e)).map(|c| (c.aabb_min, c.aabb_max)).reduce(|(a, b), (c, d)| (a.min(c), b.max(d)))
}

/// Whether a walker's capsule at `feet` can touch the box at all.
fn near(walker: &Walker, feet: Vec3, (lo, hi): (Vec3, Vec3)) -> bool {
    let reach = walker.params.radius + walker.params.skin() * 2.0;
    let height = walker.params.height.max(2.0 * walker.params.radius);
    let (blo, bhi) = (feet - Vec3::new(reach, reach, reach), feet + Vec3::new(reach, height + reach, reach));
    blo.cmple(hi).all() && lo.cmple(bhi).all()
}

/// The walker at `feet` pushed out of `colliders` the shortest way, sliding against the rest of
/// the world. The second value says whether it came clear.
fn push_out(capsule: &Capsule, free: &Capsule, mut feet: Vec3, colliders: &[Entity]) -> (Vec3, bool) {
    for _ in 0..PUSHES {
        let Some(p) = capsule.inside(feet, colliders) else { return (feet, true) };
        let moved = free.slide(feet, p.normal * p.depth, false).feet;
        if moved == feet {
            break;
        }
        feet = moved;
    }
    (feet, capsule.inside(feet, colliders).is_none())
}

/// Where each walker that a mover going from `from` to `to` carries or pushes ends up, and the
/// yaw it turns by: `(walker, feet, yaw)`. `Err` names the first walker (in `ColliderId` order) that
/// cannot be pushed out of the mover's way. The mover's colliders stand at `to` in the physics
/// world.
#[expect(clippy::too_many_arguments, reason = "the step's parts, passed once")]
fn carry_and_push(
    physics: &PhysicsWorld,
    walkers: &[Walker],
    feet: &[Vec3],
    rides: &[bool],
    colliders: &[Entity],
    from: &Transform,
    to: &Transform,
    clear_only: bool,
) -> Result<Vec<(usize, Vec3, f32)>, Entity> {
    let turn = (to.rotation * from.rotation.inverse()).normalize();
    let yaw = yaw_of(turn);
    let area = bounds(physics, colliders);
    let mut out = Vec::new();
    for (i, w) in walkers.iter().enumerate() {
        let start = feet[i];
        let capsule = Capsule::new(physics, &w.params, w.entity, w.layers);
        let free = capsule.excluding(colliders);
        let mut at = start;
        let mut turned = 0.0;
        if rides[i] {
            let target = to.translation + turn * (start - from.translation);
            at = free.slide(start, target - start, false).feet;
            turned = yaw;
        }
        if area.is_some_and(|area| near(w, at, area)) {
            let (pushed, clear) = push_out(&capsule, &free, at, colliders);
            if !clear && !clear_only {
                return Err(w.entity);
            }
            at = pushed;
        }
        if at != start || turned != 0.0 {
            out.push((i, at, turned));
        }
    }
    Ok(out)
}

/// Writes a walker's new feet (and yaw): the controller is told it left it there.
fn move_walker(world: &mut World, entity: Entity, feet: Vec3, yaw: f32) {
    let Ok(mut e) = world.get_entity_mut(entity) else { return };
    if let Some(mut tf) = e.get_mut::<Transform>() {
        tf.translation = feet;
        if yaw != 0.0 {
            tf.rotation = (Quat::from_rotation_y(yaw) * tf.rotation).normalize();
        }
    }
    if let Some(mut state) = e.get_mut::<MotionState>()
        && state.placed.is_some()
    {
        state.placed = Some(feet);
    }
}

/// Writes where the pieces stand and what body they are, when that changed.
fn write_pieces(world: &mut World, pieces: &[Piece], pose: &Transform, body: Body) {
    for piece in pieces {
        let Ok(mut e) = world.get_entity_mut(piece.entity) else { continue };
        let want = piece_pose(pose, piece);
        if let Some(mut tf) = e.get_mut::<Transform>()
            && (tf.translation != want.translation || tf.rotation != want.rotation)
        {
            tf.translation = want.translation;
            tf.rotation = want.rotation;
        }
        if piece.solid
            && let Some(mut b) = e.get_mut::<Body>()
            && *b != body
        {
            *b = body;
        }
    }
}

/// Whether a crush that has held for `held` seconds deals its damage again in the next `dt`.
fn crush_due(held: f32, dt: f32, every: f32) -> bool {
    held <= 0.0 || ops::floor(held / every) < ops::floor((held + dt) / every)
}

/// The tick's time step, if it moves anything.
fn tick_dt(world: &World) -> Option<f32> {
    let dt = world.get_resource::<Time>()?.delta_secs();
    (dt > 0.0).then_some(dt)
}

/// `SimSet::Move`, before the controller: every mover takes its step, in `AiId` order (see the
/// [module](super)). Nothing happens in a world without movers.
pub(crate) fn step_movers(world: &mut World) {
    let movers = movers_in_order(world);
    if movers.is_empty() || !world.contains_resource::<PhysicsWorld>() {
        return;
    }
    let Some(dt) = tick_dt(world) else { return };
    // A mover's first tick: it is where it was put.
    for &(_, entity) in &movers {
        if world.get::<MoverState>(entity).is_none()
            && let Some(tf) = world.get::<Transform>(entity).copied()
        {
            world.entity_mut(entity).insert(MoverState::at(&tf));
        }
    }
    let parts = parts_by_mover(world);
    let owners = owners(world, &movers, &parts);
    let walkers = walkers(world);
    let riding = riding(world, &walkers, &owners);
    let mut feet: Vec<Vec3> = walkers.iter().map(|w| w.feet).collect();
    let mut fire: Vec<Fire> = Vec::new();
    for &(id, entity) in &movers {
        let Some(plan) = plan(world, entity, id, &parts, dt) else { continue };
        let (from, to) = (run::pose(&plan.def, &plan.before), run::pose(&plan.def, &plan.after));
        let body = if plan.after.resting() { Body::Static } else { Body::Kinematic };
        if from.translation == to.translation && from.rotation == to.rotation {
            commit(world, &plan, &to, body, &mut fire);
            continue;
        }
        let colliders: Vec<Entity> = plan.pieces.iter().filter(|p| p.solid).map(|p| p.entity).collect();
        place(world, &plan.pieces, &to, Some(Body::Kinematic));
        let rides: Vec<bool> = riding.iter().map(|r| *r == Some(id)).collect();
        let moved = carry_and_push(world.resource::<PhysicsWorld>(), &walkers, &feet, &rides, &colliders, &from, &to, false);
        match moved {
            Ok(moved) => {
                for (i, at, yaw) in moved {
                    feet[i] = at;
                    move_walker(world, walkers[i].entity, at, yaw);
                }
                place(world, &plan.pieces, &to, Some(body));
                commit(world, &plan, &to, body, &mut fire);
            }
            Err(blocker) => {
                restore(world, &plan.pieces);
                held(world, &plan, blocker, dt, &mut fire);
            }
        }
    }
    for event in fire {
        match event {
            Fire::Started(e) => world.trigger(e),
            Fire::Arrived(e) => world.trigger(e),
            Fire::Blocked(e) => world.trigger(e),
        }
    }
}

/// The mover's step as its state would take it, before anything is moved.
fn plan(world: &World, entity: Entity, id: AiId, parts: &Parts, dt: f32) -> Option<Plan> {
    let def_id = &world.get::<Mover>(entity)?.def;
    let def = world.get_resource::<ContentDb>()?.get::<MoverDef>(def_id)?.clone();
    let before = world.get::<MoverState>(entity)?.clone();
    let mut after = before.clone();
    let mut notes = Vec::new();
    run::advance(&def, &mut after, dt, &mut notes);
    after.blocked = 0.0;
    Some(Plan { entity, def, before, after, notes, pieces: pieces(world, entity, parts, id) })
}

/// The step taken: pose, bodies and state written, its events noted.
fn commit(world: &mut World, plan: &Plan, pose: &Transform, body: Body, fire: &mut Vec<Fire>) {
    write_pieces(world, &plan.pieces, pose, body);
    if plan.after != plan.before {
        world.entity_mut(plan.entity).insert(plan.after.clone());
    }
    for note in &plan.notes {
        fire.push(match *note {
            Note::Started { from, to } => Fire::Started(MoverStarted { mover: plan.entity, from, to }),
            Note::Arrived { key } => Fire::Arrived(MoverArrived { mover: plan.entity, key }),
        });
    }
}

/// The step refused because `blocker` cannot be pushed out of the way: the mover stays where it
/// was, and its `blocked` rule says what then.
fn held(world: &mut World, plan: &Plan, blocker: Entity, dt: f32, fire: &mut Vec<Fire>) {
    let mut state = plan.before.clone();
    state.arrived = false;
    let held = state.blocked;
    let (tell, damage) = match plan.def.blocked {
        Blocked::Stop => (held <= 0.0, 0.0),
        Blocked::Reverse => {
            run::reverse(&plan.def, &mut state);
            (held <= 0.0, 0.0)
        }
        Blocked::Crush { damage, every } => {
            let due = crush_due(held, dt, every);
            (due, if due { damage } else { 0.0 })
        }
    };
    state.blocked = held + dt;
    if state != plan.before {
        world.entity_mut(plan.entity).insert(state);
    }
    if tell {
        fire.push(Fire::Blocked(MoverBlocked { mover: plan.entity, body: blocker, damage }));
    }
}

/// `NetSet::Predict`, before the predicted move, on a co-op client: the movers as they will stand
/// at this tick (their replicated state run on from the host's tick), in this machine's physics
/// world only, carrying and pushing its own body as the host's step does (see the [module](super)).
/// A body the client cannot push out stays where the push got it: the host decides blocks.
pub(crate) fn predict_movers(world: &mut World) {
    let movers = movers_in_order(world);
    if movers.is_empty() || !world.contains_resource::<PhysicsWorld>() {
        return;
    }
    let parts = parts_by_mover(world);
    let host = world.get_resource::<NetPrediction>().map(|p| p.host_tick);
    let ahead = host.map_or(0, |host| sim::tick(world).saturating_sub(host));
    let dt = tick_dt(world);
    let plans: Vec<Foreseen> = movers
        .iter()
        .map(|&(id, entity)| {
            let pieces = pieces(world, entity, &parts, id);
            let poses = match dt {
                Some(dt) if (1..=MAX_AHEAD).contains(&ahead) => foresee(world, entity, dt, ahead),
                _ => None,
            };
            Foreseen { id, pieces, poses }
        })
        .collect();
    // Where the movers stood a tick ago is where the body stood on them.
    for Foreseen { pieces, poses, .. } in &plans {
        match poses {
            Some((before, _)) => place(world, pieces, before, None),
            None => restore(world, pieces),
        }
    }
    let mut own: Vec<Walker> = walkers(world).into_iter().filter(|w| world.get::<Predicted>(w.entity).is_some()).collect();
    own.truncate(1);
    if own.is_empty() {
        for Foreseen { pieces, poses, .. } in &plans {
            if let Some((_, after)) = poses {
                place(world, pieces, after, None);
            }
        }
        return;
    }
    let owners = owners(world, &movers, &parts);
    let riding = riding(world, &own, &owners);
    let mut feet = vec![own[0].feet];
    for Foreseen { id, pieces, poses } in &plans {
        let Some((before, after)) = poses else { continue };
        place(world, pieces, after, None);
        if before.translation == after.translation && before.rotation == after.rotation {
            continue;
        }
        let colliders: Vec<Entity> = pieces.iter().filter(|p| p.solid).map(|p| p.entity).collect();
        let rides = [riding[0] == Some(*id)];
        if let Ok(moved) = carry_and_push(world.resource::<PhysicsWorld>(), &own, &feet, &rides, &colliders, before, after, true) {
            for (i, at, yaw) in moved {
                feet[i] = at;
                move_walker(world, own[i].entity, at, yaw);
            }
        }
    }
}

/// A mover's pose a tick before and at `ahead` ticks past its replicated state.
fn foresee(world: &World, entity: Entity, dt: f32, ahead: u64) -> Option<(Transform, Transform)> {
    let def = world.get_resource::<ContentDb>()?.get::<MoverDef>(&world.get::<Mover>(entity)?.def)?;
    let mut state = world.get::<MoverState>(entity)?.clone();
    let mut notes = Vec::new();
    for _ in 1..ahead {
        run::advance(def, &mut state, dt, &mut notes);
    }
    let before = run::pose(def, &state);
    run::advance(def, &mut state, dt, &mut notes);
    Some((before, run::pose(def, &state)))
}
