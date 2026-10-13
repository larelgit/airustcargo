//! Attacks with telegraphs, dodges with i-frames, damage, deaths, boss phases.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use super::intent::{flat, Intent};
use super::{Boss, Player};

#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Eq)]
#[reflect(Component)]
pub enum Team {
    Hero,
    Monster,
}

#[derive(Component, Reflect, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

impl Health {
    pub fn new(max: f32) -> Self {
        Self { current: max, max }
    }
}

#[derive(Component, Reflect, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct Stamina {
    pub current: f32,
    pub max: f32,
    pub regen: f32,
}

/// Collision radius and walking speed.
#[derive(Component, Reflect, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct Body {
    pub radius: f32,
    pub speed: f32,
}

#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AttackKind {
    Sword,
    Claw,
    FrostSlash,
    IceSlam,
    FrostNova,
}

#[derive(Clone, Copy, Debug)]
pub enum Shape {
    /// In front of the attacker.
    Cone { range: f32, half_angle_deg: f32 },
    /// Around the attacker.
    Circle { radius: f32 },
}

#[derive(Clone, Copy, Debug)]
pub struct AttackSpec {
    /// Telegraph time before the hit lands.
    pub windup: f32,
    /// Vulnerable time after the hit.
    pub recover: f32,
    pub damage: f32,
    pub shape: Shape,
    pub stamina: f32,
}

impl AttackSpec {
    pub fn reach(&self) -> f32 {
        match self.shape {
            Shape::Cone { range, .. } => range,
            Shape::Circle { radius } => radius,
        }
    }
}

impl AttackKind {
    pub fn spec(self) -> AttackSpec {
        use Shape::*;
        let (windup, recover, damage, shape, stamina) = match self {
            AttackKind::Sword => (0.15, 0.25, 25.0, Cone { range: 2.4, half_angle_deg: 60.0 }, 10.0),
            AttackKind::Claw => (0.45, 0.6, 8.0, Cone { range: 1.8, half_angle_deg: 50.0 }, 0.0),
            AttackKind::FrostSlash => (0.55, 0.7, 18.0, Cone { range: 3.2, half_angle_deg: 70.0 }, 0.0),
            AttackKind::IceSlam => (1.0, 1.2, 30.0, Circle { radius: 4.5 }, 0.0),
            AttackKind::FrostNova => (1.3, 1.0, 22.0, Circle { radius: 7.5 }, 0.0),
        };
        AttackSpec { windup, recover, damage, shape, stamina }
    }

    /// Long windups are worth a journal line ("e42 winds up IceSlam").
    pub fn is_telegraphed(self) -> bool {
        self.spec().windup >= 0.5
    }
}

pub const DODGE_TIME: f32 = 0.35;
pub const DODGE_SPEED: f32 = 12.0;
pub const DODGE_STAMINA: f32 = 25.0;
const CORPSE_TIME: f32 = 1.5;

/// What a character is doing right now.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub enum Action {
    #[default]
    Idle,
    /// Telegraph: the hit lands when `left` reaches 0. Facing is locked.
    Windup { attack: AttackKind, left: f32 },
    /// After a hit; cannot act.
    Recover { left: f32 },
    /// Moving fast, immune to hits.
    Dodge { dir: Vec3, left: f32 },
    Dead { for_s: f32 },
}

// --- events (journaled by the AI layer) -------------------------------------------

#[derive(Event, Clone, Debug)]
pub struct Telegraph {
    pub attacker: Entity,
    pub attack: AttackKind,
    pub windup: f32,
}

#[derive(Event, Clone, Debug)]
pub struct Damaged {
    pub source: Entity,
    pub target: Entity,
    pub attack: AttackKind,
    pub amount: f32,
    pub hp_left: f32,
}

#[derive(Event, Clone, Debug)]
pub struct Dodged {
    pub source: Entity,
    pub target: Entity,
    pub attack: AttackKind,
}

#[derive(Event, Clone, Debug)]
pub struct Died {
    pub entity: Entity,
    pub killer: Option<Entity>,
}

#[derive(Event, Clone, Debug)]
pub struct PhaseChanged {
    pub boss: Entity,
    pub from: u8,
    pub to: u8,
}

/// A hit that landed geometrically; i-frames and health are resolved later in the tick.
#[derive(Message, Clone, Debug)]
pub struct Hit {
    pub source: Entity,
    pub target: Entity,
    pub attack: AttackKind,
    pub damage: f32,
}

// --- systems ----------------------------------------------------------------------

fn windup_scale(boss: Option<&Boss>) -> f32 {
    match boss {
        Some(b) if b.phase >= 2 => 0.8,
        _ => 1.0,
    }
}

/// Consumes one-shot intents: dodges and attacks start here.
pub fn start_actions(
    mut commands: Commands,
    mut q: Query<(Entity, &mut Intent, &mut Action, &mut Transform, Option<&mut Stamina>, Option<&Boss>)>,
) {
    for (entity, mut intent, mut action, mut tf, mut stamina, boss) in &mut q {
        let dodge = intent.dodge.take();
        let attack = intent.attack.take();
        // Dodging cancels recovery (not a windup: attacks are commitments).
        let can_dodge = matches!(*action, Action::Idle | Action::Recover { .. });
        if let Some(dir) = dodge.filter(|_| can_dodge) {
            let enough = stamina.as_ref().is_none_or(|s| s.current >= DODGE_STAMINA);
            if enough {
                if let Some(s) = stamina.as_mut() {
                    s.current -= DODGE_STAMINA;
                }
                let dir = flat(dir).try_normalize().unwrap_or(*tf.forward());
                *action = Action::Dodge { dir, left: DODGE_TIME };
                continue;
            }
        }
        if *action != Action::Idle {
            continue;
        }
        if let Some(kind) = attack {
            let spec = kind.spec();
            if stamina.as_ref().is_some_and(|s| s.current < spec.stamina) {
                continue;
            }
            if let Some(s) = stamina.as_mut() {
                s.current -= spec.stamina;
            }
            if let Some(aim) = intent.aim {
                let to = aim - tf.translation;
                face(&mut tf, to);
            }
            let windup = spec.windup * windup_scale(boss);
            *action = Action::Windup { attack: kind, left: windup };
            if kind.is_telegraphed() {
                commands.trigger(Telegraph { attacker: entity, attack: kind, windup });
            }
        }
    }
}

/// Advances timers; a windup that reaches zero produces `Hit`s.
pub fn tick_actions(
    time: Res<Time>,
    mut hits: MessageWriter<Hit>,
    mut actors: Query<(Entity, &mut Action, &Transform, &Team, &Body)>,
) {
    let dt = time.delta_secs();
    // Targets as they were at the start of this phase, so hit order never depends on
    // which attacker the query happens to visit first.
    let targets: Vec<(Entity, Vec3, Team, f32)> = actors
        .iter()
        .filter(|(_, a, ..)| !matches!(a, Action::Dead { .. }))
        .map(|(e, _, tf, team, body)| (e, tf.translation, *team, body.radius))
        .collect();
    for (entity, mut action, tf, team, _) in &mut actors {
        match *action {
            Action::Windup { attack, left } if left - dt <= 0.0 => {
                let spec = attack.spec();
                for &(target, pos, tteam, radius) in &targets {
                    if tteam != *team && in_shape(tf, spec.shape, pos, radius) {
                        hits.write(Hit { source: entity, target, attack, damage: spec.damage });
                    }
                }
                *action = Action::Recover { left: spec.recover };
            }
            Action::Windup { attack, left } => *action = Action::Windup { attack, left: left - dt },
            Action::Recover { left } => {
                *action = if left - dt <= 0.0 { Action::Idle } else { Action::Recover { left: left - dt } }
            }
            Action::Dodge { dir, left } => {
                *action = if left - dt <= 0.0 { Action::Idle } else { Action::Dodge { dir, left: left - dt } }
            }
            Action::Dead { for_s } => *action = Action::Dead { for_s: for_s + dt },
            Action::Idle => {}
        }
    }
}

fn in_shape(attacker: &Transform, shape: Shape, point: Vec3, radius: f32) -> bool {
    let to = flat(point - attacker.translation);
    match shape {
        Shape::Circle { radius: r } => to.length() <= r + radius,
        Shape::Cone { range, half_angle_deg } => {
            if to.length() > range + radius {
                return false;
            }
            if to.length() < radius {
                return true;
            }
            let fwd = flat(*attacker.forward()).normalize_or_zero();
            fwd.angle_between(to.normalize_or_zero()).to_degrees() <= half_angle_deg
        }
    }
}

pub fn face(tf: &mut Transform, dir: Vec3) {
    let dir = flat(dir);
    if dir.length_squared() > 1e-6 {
        let target = tf.translation + dir;
        tf.look_at(target, Vec3::Y);
    }
}

/// Applies hits: dodging characters are immune; otherwise damage, maybe death.
pub fn apply_hits(
    mut commands: Commands,
    mut hits: MessageReader<Hit>,
    mut q: Query<(&mut Health, &mut Action)>,
) {
    for hit in hits.read() {
        let Ok((mut health, mut action)) = q.get_mut(hit.target) else { continue };
        match *action {
            Action::Dead { .. } => continue,
            Action::Dodge { .. } => {
                commands.trigger(Dodged { source: hit.source, target: hit.target, attack: hit.attack });
                continue;
            }
            _ => {}
        }
        health.current = (health.current - hit.damage).max(0.0);
        commands.trigger(Damaged {
            source: hit.source,
            target: hit.target,
            attack: hit.attack,
            amount: hit.damage,
            hp_left: health.current,
        });
        if health.current <= 0.0 {
            *action = Action::Dead { for_s: 0.0 };
            commands.trigger(Died { entity: hit.target, killer: Some(hit.source) });
        }
    }
}

/// Phase 2 at half health, however the health got there (hits or a dev `set`).
pub fn boss_phases(mut commands: Commands, mut q: Query<(Entity, &mut Boss, &Health)>) {
    for (entity, mut boss, health) in &mut q {
        if boss.phase == 1 && health.current <= health.max * 0.5 {
            boss.phase = 2;
            commands.trigger(PhaseChanged { boss: entity, from: 1, to: 2 });
        }
    }
}

pub fn regen_stamina(time: Res<Time>, mut q: Query<(&mut Stamina, &Action)>) {
    for (mut s, action) in &mut q {
        if !matches!(action, Action::Dodge { .. } | Action::Dead { .. }) {
            s.current = (s.current + s.regen * time.delta_secs()).min(s.max);
        }
    }
}

/// Monster corpses disappear; the player stays down until revived.
pub fn remove_corpses(mut commands: Commands, q: Query<(Entity, &Action), Without<Player>>) {
    for (entity, action) in &q {
        if let Action::Dead { for_s } = action
            && *for_s >= CORPSE_TIME
        {
            commands.entity(entity).despawn();
        }
    }
}
