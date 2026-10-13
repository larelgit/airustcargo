//! Monster brains. They write `Intent`, exactly like the player's keyboard does.

use ai_core::prelude::*;
use bevy::prelude::*;

use super::combat::{Action, AttackKind};
use super::intent::{flat, Intent};
use super::{Boss, Player};

const SALT_ATTACK: u64 = 1;
const SALT_COOLDOWN: u64 = 2;

#[derive(Component, Reflect, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct SkeletonBrain {
    pub aggro: f32,
    pub cooldown: f32,
}

impl Default for SkeletonBrain {
    fn default() -> Self {
        Self { aggro: 18.0, cooldown: 0.0 }
    }
}

#[derive(Component, Reflect, Clone, Copy, Debug, Default)]
#[reflect(Component)]
pub struct BossBrain {
    pub cooldown: f32,
}

fn nearest_living_player(players: &Query<(&Transform, &Action), With<Player>>, from: Vec3) -> Option<Vec3> {
    players
        .iter()
        .filter(|(_, a)| !matches!(a, Action::Dead { .. }))
        .map(|(tf, _)| tf.translation)
        .min_by(|a, b| a.distance(from).total_cmp(&b.distance(from)))
}

pub fn skeleton_brains(
    time: Res<Time>,
    rng: SimRng,
    players: Query<(&Transform, &Action), With<Player>>,
    mut q: Query<(&AiId, &Transform, &Action, &mut SkeletonBrain, &mut Intent), Without<Player>>,
) {
    let dt = time.delta_secs();
    for (id, tf, action, mut brain, mut intent) in &mut q {
        brain.cooldown = (brain.cooldown - dt).max(0.0);
        intent.move_dir = Vec3::ZERO;
        if *action != Action::Idle {
            continue;
        }
        let Some(target) = nearest_living_player(&players, tf.translation) else { continue };
        let to = flat(target - tf.translation);
        if to.length() > brain.aggro {
            continue;
        }
        intent.aim = Some(target);
        if to.length() > AttackKind::Claw.spec().reach() * 0.9 {
            intent.move_dir = to.normalize_or_zero();
        } else if brain.cooldown <= 0.0 {
            intent.attack = Some(AttackKind::Claw);
            brain.cooldown = 1.4 + rng.stream(*id, SALT_COOLDOWN).range(0.0, 0.5);
        }
    }
}

pub fn boss_brains(
    time: Res<Time>,
    rng: SimRng,
    players: Query<(&Transform, &Action), With<Player>>,
    mut q: Query<(&AiId, &Transform, &Action, &Boss, &mut BossBrain, &mut Intent), Without<Player>>,
) {
    let dt = time.delta_secs();
    for (id, tf, action, boss, mut brain, mut intent) in &mut q {
        brain.cooldown = (brain.cooldown - dt).max(0.0);
        intent.move_dir = Vec3::ZERO;
        if *action != Action::Idle {
            continue;
        }
        let Some(target) = nearest_living_player(&players, tf.translation) else { continue };
        let to = flat(target - tf.translation);
        let dist = to.length();
        intent.aim = Some(target);
        if brain.cooldown > 0.0 {
            if dist > 2.6 {
                intent.move_dir = to.normalize_or_zero();
            }
            continue;
        }
        let mut roll = rng.stream(*id, SALT_ATTACK);
        let choice = if boss.phase >= 2 && dist <= 7.0 && roll.chance(0.3) {
            Some(AttackKind::FrostNova)
        } else if dist <= 3.0 {
            Some(if roll.chance(0.55) { AttackKind::FrostSlash } else { AttackKind::IceSlam })
        } else if dist <= 4.0 {
            Some(AttackKind::IceSlam)
        } else {
            None
        };
        match choice {
            Some(kind) => {
                intent.attack = Some(kind);
                brain.cooldown = if boss.phase >= 2 { 0.6 } else { 1.0 };
            }
            None => intent.move_dir = to.normalize_or_zero(),
        }
    }
}
