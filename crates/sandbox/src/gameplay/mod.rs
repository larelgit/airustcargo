//! Headless-safe gameplay: no meshes, no windows, only data and `FixedUpdate` systems.
//! Rendering attaches itself to these entities from `visuals` when a window exists.
//! The arena is spawned when the engine enters `InGame`: content (the attack numbers)
//! exists only after loading.

pub mod brains;
pub mod combat;
pub mod intent;

use shards::core::{flat, CoreSystems, Intent};
use shards::prelude::*;

use brains::{BossBrain, SkeletonBrain};
use combat::{Action, AttackDef, AttackKind, Body, Health, Hit, InputBuffer, Stamina, Team};

/// Half size of the square arena (meters).
pub const ARENA_HALF: f32 = 20.0;

#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct Player;

#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct Skeleton;

#[derive(Component, Reflect, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct Boss {
    pub phase: u8,
}

#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct Pillar;

pub struct GameplayPlugin;

impl Plugin for GameplayPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Hit>()
            .add_content::<AttackDef>()
            // One-shot presses wait in the buffer until the character can act on them.
            .register_required_components::<Intent, InputBuffer>()
            .add_systems(OnEnter(EngineState::InGame), (report_missing_attacks, spawn_arena).chain());
        app.add_systems(
            FixedUpdate,
            (
                intent::attack_orders.in_set(SimSet::Input).after(CoreSystems::FollowOrders),
                (brains::skeleton_brains, brains::boss_brains).chain().in_set(SimSet::Think),
                (combat::start_actions, combat::tick_actions).chain().in_set(SimSet::Act),
                movement.in_set(SimSet::Move),
                (combat::apply_hits, combat::boss_phases, combat::regen_stamina).chain().in_set(SimSet::Resolve),
                combat::remove_corpses.in_set(SimSet::Cleanup),
            ),
        );
    }
}

/// An attack the game uses but content does not define does nothing; say so once.
fn report_missing_attacks(db: Res<ContentDb>) {
    let missing: Vec<&str> = AttackKind::ALL.iter().map(|k| k.id()).filter(|id| db.get::<AttackDef>(id).is_none()).collect();
    if !missing.is_empty() {
        error!("content: attacks {} are not defined (content/combat/attacks.attack.ron); they do nothing", missing.join(", "));
    }
}

fn spawn_arena(mut commands: Commands) {
    for (x, z) in [(-7.0, -7.0), (7.0, -7.0), (-7.0, 7.0), (7.0, 7.0)] {
        spawn_pillar(&mut commands, Vec3::new(x, 0.0, z));
    }
    spawn_player(&mut commands, Vec3::new(0.0, 0.0, 12.0));
    spawn_boss(&mut commands, Vec3::new(0.0, 0.0, -6.0));
    spawn_skeleton(&mut commands, Vec3::new(-12.0, 0.0, 2.0));
    spawn_skeleton(&mut commands, Vec3::new(12.0, 0.0, -2.0));
}

pub fn spawn_pillar(commands: &mut Commands, at: Vec3) -> Entity {
    commands
        .spawn((
            Name::new("Pillar"),
            Pillar,
            AiKind::new("Pillar"),
            AiLandmark,
            AiOccluder::standing(Vec3::new(1.0, 2.0, 1.0)),
            Transform::from_translation(at),
        ))
        .id()
}

pub fn spawn_player(commands: &mut Commands, at: Vec3) -> Entity {
    commands
        .spawn((
            Name::new("Player"),
            Player,
            AiPlayer,
            AiKind::new("Player"),
            Team::Hero,
            Health::new(100.0),
            Stamina { current: 80.0, max: 80.0, regen: 15.0 },
            Body { radius: 0.45, speed: 6.0 },
            Intent::default(),
            Action::default(),
            Transform::from_translation(at).looking_to(Vec3::NEG_Z, Vec3::Y),
        ))
        .id()
}

pub fn spawn_skeleton(commands: &mut Commands, at: Vec3) -> Entity {
    commands
        .spawn((
            Name::new("Skeleton"),
            Skeleton,
            AiKind::new("Skeleton"),
            AiImportance(1.0),
            Team::Monster,
            Health::new(40.0),
            Body { radius: 0.45, speed: 3.6 },
            SkeletonBrain::default(),
            Intent::default(),
            Action::default(),
            Transform::from_translation(at),
        ))
        .id()
}

pub fn spawn_boss(commands: &mut Commands, at: Vec3) -> Entity {
    commands
        .spawn((
            Name::new("Frost Warden"),
            Boss { phase: 1 },
            AiKind::new("Boss"),
            AiImportance(10.0),
            Team::Monster,
            Health::new(4000.0),
            Body { radius: 1.0, speed: 3.2 },
            BossBrain::default(),
            Intent::default(),
            Action::default(),
            Transform::from_translation(at).looking_to(Vec3::Z, Vec3::Y),
        ))
        .id()
}

/// Integrates movement, then resolves collisions with the arena, pillars and bodies.
/// Bodies are processed in `AiId` order so the result never depends on query order.
fn movement(
    time: Res<Time>,
    pillars: Query<(&Transform, &AiOccluder), Without<Body>>,
    mut bodies: Query<(Entity, &AiId, &mut Transform, &Intent, &Action, &Body, Option<&Boss>)>,
) {
    let dt = time.delta_secs();
    for (_, _, mut tf, intent, action, body, boss) in &mut bodies {
        let speed_scale = if boss.is_some_and(|b| b.phase >= 2) { 1.25 } else { 1.0 };
        let velocity = match action {
            Action::Dodge { dir, .. } => *dir * combat::DODGE_SPEED,
            Action::Idle => flat(intent.move_dir).clamp_length_max(1.0) * body.speed * speed_scale,
            _ => Vec3::ZERO,
        };
        tf.translation += velocity * dt;
        if matches!(action, Action::Idle) {
            match intent.aim {
                Some(aim) if velocity.length_squared() < 1e-6 => {
                    let to = aim - tf.translation;
                    combat::face(&mut tf, to);
                }
                _ => combat::face(&mut tf, velocity),
            }
        }
    }

    // Body-body separation, deterministic order.
    let mut all: Vec<(AiId, Vec3, f32, bool, Entity)> = bodies
        .iter()
        .map(|(e, id, tf, _, action, body, _)| (*id, tf.translation, body.radius, matches!(action, Action::Dead { .. }), e))
        .collect();
    all.sort_by_key(|b| b.0);
    let mut push = vec![Vec3::ZERO; all.len()];
    for i in 0..all.len() {
        for j in (i + 1)..all.len() {
            let (a, b) = (&all[i], &all[j]);
            if a.3 || b.3 {
                continue;
            }
            let d = flat(b.1 - a.1);
            let overlap = a.2 + b.2 - d.length();
            if overlap > 0.0 {
                let n = d.try_normalize().unwrap_or(Vec3::X);
                push[i] -= n * overlap * 0.5;
                push[j] += n * overlap * 0.5;
            }
        }
    }
    for (k, &(.., entity)) in all.iter().enumerate() {
        if let Ok((_, _, mut tf, _, _, body, _)) = bodies.get_mut(entity) {
            tf.translation += push[k];
            let limit = ARENA_HALF - body.radius;
            tf.translation.x = tf.translation.x.clamp(-limit, limit);
            tf.translation.z = tf.translation.z.clamp(-limit, limit);
            tf.translation.y = 0.0;
            for (ptf, occ) in &pillars {
                push_out_of_box(&mut tf.translation, ptf.translation, occ.half_extents, body.radius);
            }
        }
    }
}

/// Circle (on the ground plane) vs axis-aligned box.
fn push_out_of_box(p: &mut Vec3, center: Vec3, half: Vec3, radius: f32) {
    let local = *p - center;
    let closest = Vec3::new(local.x.clamp(-half.x, half.x), 0.0, local.z.clamp(-half.z, half.z));
    let d = Vec3::new(local.x - closest.x, 0.0, local.z - closest.z);
    let dist = d.length();
    if dist < radius {
        let n = if dist > 1e-5 {
            d / dist
        } else {
            // Center inside the box: leave through the nearest face.
            let (dx, dz) = (half.x - local.x.abs(), half.z - local.z.abs());
            if dx < dz { Vec3::X * local.x.signum() } else { Vec3::Z * local.z.signum() }
        };
        let target = center + closest + n * radius;
        p.x = target.x;
        p.z = target.z;
    }
}
