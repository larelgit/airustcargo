//! Monster brains. They write `Intent`, exactly like the player's keyboard does: they
//! move, aim and press their moveset's inputs by name; the engine's action machine does
//! the rest. Randomness comes from `SimRng` only.

use shards::action::{ActionDef, MovesetDef};
use shards::core::{flat, Intent};
use shards::motion::Collider;
use shards::prelude::*;

use super::{Boss, Player};

const SALT_ATTACK: u64 = 1;
const SALT_COOLDOWN: u64 = 2;
const SALT_REST: u64 = 3;

/// An attack is chosen when the target stands within this share of its reach: then it is
/// inside the zone when the warning shows.
const COVER: f32 = 0.8;

pub(super) fn plugin(app: &mut App) {
    app.register_type::<SkeletonBrain>().register_type::<WardenBrain>();
}

/// Chase the nearest player within `aggro` meters and claw at it.
#[derive(Component, Reflect, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct SkeletonBrain {
    pub aggro: f32,
    /// Seconds until the next claw.
    pub cooldown: f32,
}

impl Default for SkeletonBrain {
    fn default() -> Self {
        Self { aggro: 18.0, cooldown: 0.0 }
    }
}

/// Guard a post: fight players near it, choose attacks by distance, rest between them.
#[derive(Component, Reflect, Clone, Debug, Default)]
#[reflect(Component, Default)]
pub struct WardenBrain {
    /// Where it stands guard: where it stood first.
    pub home: Option<Vec3>,
    /// It fights players within this many meters of home (its arena), else goes back.
    pub leash: f32,
    /// Seconds before it may attack again; counts down while it is free to act.
    pub rest: f32,
    /// The rest after an attack, per boss phase: `(min, max)` seconds.
    pub rest_after: Vec<(f32, f32)>,
    /// Its last attack covered a zone ("zone" tag): the next one does not.
    pub last_zone: bool,
}

/// Living players, for targets.
type Players<'w, 's> = Query<'w, 's, (&'static Transform, Option<&'static Collider>), (With<Player>, Without<Dead>)>;

/// The nearest living player within `range` of `from` (measured from `origin`), with its
/// body radius.
fn nearest_player(players: &Players, origin: Vec3, range: f32, from: Vec3) -> Option<(Vec3, f32)> {
    players
        .iter()
        .map(|(tf, c)| (tf.translation, c.map_or(0.4, |c| c.shape.half_extents().x)))
        .filter(|(p, _)| flat(*p - origin).length() <= range)
        .min_by(|a, b| flat(a.0 - from).length().total_cmp(&flat(b.0 - from).length()))
}

/// Writes only what changes, so `Intent` is not marked changed for nothing.
fn steer(intent: &mut Mut<Intent>, move_dir: Vec3, aim: Option<Vec3>) {
    if intent.move_dir != move_dir {
        intent.move_dir = move_dir;
    }
    if intent.aim != aim {
        intent.aim = aim;
    }
}

/// An attack of a moveset: its input, its reach (m), whether it covers a zone.
struct Attack {
    input: String,
    reach: f32,
    zone: bool,
}

/// The inputs of `moveset` whose actions hit, in input order.
fn attacks(db: &ContentDb, moveset: &str) -> Vec<Attack> {
    let Some(moveset) = db.get::<MovesetDef>(moveset) else { return Vec::new() };
    moveset
        .inputs
        .iter()
        .filter_map(|(input, action)| {
            let def = db.get::<ActionDef>(action).filter(|d| !d.hits.is_empty())?;
            Some(Attack { input: input.clone(), reach: def.reach(), zone: def.tags.iter().any(|t| t == "zone") })
        })
        .collect()
}

pub fn skeleton_brains(
    time: Res<Time>,
    rng: SimRng,
    db: Res<ContentDb>,
    players: Players,
    mut q: Query<
        (&AiId, &Transform, &Fighter, &ActionState, Has<Dead>, &mut SkeletonBrain, &mut Intent),
        Without<Player>,
    >,
) {
    let dt = time.delta_secs();
    for (id, tf, fighter, state, dead, mut brain, mut intent) in &mut q {
        if brain.cooldown > 0.0 {
            brain.cooldown = (brain.cooldown - dt).max(0.0);
        }
        let here = tf.translation;
        let target = if dead { None } else { nearest_player(&players, here, brain.aggro, here) };
        let Some((goal, radius)) = target else {
            steer(&mut intent, Vec3::ZERO, None);
            continue;
        };
        let to = flat(goal - here);
        if state.current.is_some() {
            steer(&mut intent, Vec3::ZERO, Some(goal));
            continue;
        }
        let reach = attacks(&db, &fighter.moveset).iter().find(|a| a.input == "light").map_or(1.5, |a| a.reach);
        if to.length() > reach * 0.9 + radius {
            steer(&mut intent, to.normalize_or_zero(), Some(goal));
        } else {
            steer(&mut intent, Vec3::ZERO, Some(goal));
            if brain.cooldown <= 0.0 {
                intent.press("light");
                brain.cooldown = 1.4 + rng.stream(*id, SALT_COOLDOWN).range(0.0, 0.5);
            }
        }
    }
}

/// The warden: within its leash it closes in and attacks with whatever of its moveset
/// reaches the target, chosen with `SimRng`, never two zone attacks in a row; between
/// attacks it rests (shorter when enraged) and keeps close. Out of its leash it walks home.
pub fn warden_brains(
    time: Res<Time>,
    rng: SimRng,
    db: Res<ContentDb>,
    players: Players,
    mut q: Query<
        (&AiId, &Transform, &Fighter, &ActionState, Option<&Boss>, Has<Dead>, &mut WardenBrain, &mut Intent),
        Without<Player>,
    >,
) {
    let dt = time.delta_secs();
    for (id, tf, fighter, state, boss, dead, mut brain, mut intent) in &mut q {
        let here = tf.translation;
        let home = *brain.home.get_or_insert(here);
        if dead {
            steer(&mut intent, Vec3::ZERO, None);
            continue;
        }
        let Some((goal, radius)) = nearest_player(&players, home, brain.leash, here) else {
            // Nobody to fight: back to the post.
            let back = flat(home - here);
            steer(&mut intent, if back.length() > 0.5 { back.normalize() } else { Vec3::ZERO }, None);
            continue;
        };
        let to = flat(goal - here);
        let dist = to.length();
        if state.current.is_some() {
            // Busy: keep looking at the target (an attack turns toward it while it tracks).
            steer(&mut intent, Vec3::ZERO, Some(goal));
            continue;
        }
        if brain.rest > 0.0 {
            brain.rest = (brain.rest - dt).max(0.0);
        }
        let all = attacks(&db, &fighter.moveset);
        let close = all.iter().map(|a| a.reach).fold(f32::INFINITY, f32::min) * COVER + radius;
        if brain.rest > 0.0 {
            // Resting: stay within reach of its quickest attack.
            let walk = if dist > close { to.normalize_or_zero() } else { Vec3::ZERO };
            steer(&mut intent, walk, Some(goal));
            continue;
        }
        let last_zone = brain.last_zone;
        let fits: Vec<&Attack> =
            all.iter().filter(|a| dist <= a.reach * COVER + radius && !(last_zone && a.zone)).collect();
        let Some(attack) = rng.stream(*id, SALT_ATTACK).pick(&fits).copied() else {
            steer(&mut intent, to.normalize_or_zero(), Some(goal));
            continue;
        };
        steer(&mut intent, Vec3::ZERO, Some(goal));
        intent.press(attack.input.clone());
        brain.last_zone = attack.zone;
        let phase = boss.map_or(1, |b| b.phase.max(1)) as usize;
        let (lo, hi) = brain.rest_after.get(phase - 1).or(brain.rest_after.last()).copied().unwrap_or((1.0, 1.0));
        brain.rest = rng.stream(*id, SALT_REST).range(lo, hi.max(lo));
    }
}
