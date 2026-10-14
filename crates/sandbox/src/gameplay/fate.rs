//! What happens around fights and on the course: the boss's phases, the dead monsters'
//! remains, the lock-on marker that follows life and death, and the goal pad.

use shards::action::{Died, Healed};
use shards::motion::{LockOnTarget, Volume, VolumeEntered, VolumeKind};
use shards::prelude::*;

use super::{Boss, Player, Skeleton};

/// The name of the course's goal trigger (`Trigger(name: "goal")` in the layout).
pub const GOAL: &str = "goal";

pub(super) fn plugin(app: &mut App) {
    app.register_type::<Remains>()
        .register_type::<Corpse>()
        .add_observer(on_died)
        .add_observer(on_healed)
        .add_observer(on_volume_entered);
}

/// A boss changed phase (journaled: `e6 phase 1->2`).
#[derive(Event, Clone, Debug)]
pub struct PhaseChanged {
    pub boss: Entity,
    pub from: u8,
    pub to: u8,
}

/// A player entered the goal pad (journaled: `e5 reached goal`).
#[derive(Event, Clone, Debug)]
pub struct ReachedGoal {
    pub entity: Entity,
    pub goal: Entity,
}

/// A monster whose body goes `despawn_after` seconds after it dies. Without it the body
/// stays (the player, the warden) until a dev agent `heal`s it back.
#[derive(Component, Reflect, Clone, Copy, Debug, Default)]
#[reflect(Component, Default)]
pub struct Remains {
    pub despawn_after: f32,
}

/// A body on its way out: seconds since it died.
#[derive(Component, Reflect, Clone, Copy, Debug, Default)]
#[reflect(Component, Default)]
pub struct Corpse {
    pub for_s: f32,
}

/// `SimSet::Resolve`, after deaths: phase 2 at half health, however the health got there
/// (hits or a dev `set`); each phase fights with its own moveset (`Boss::movesets`).
pub fn boss_phases(mut commands: Commands, mut q: Query<(Entity, &mut Boss, &Health, &mut Fighter, Has<Dead>)>) {
    for (entity, mut boss, health, mut fighter, dead) in &mut q {
        if boss.phase == 1 && !dead && health.current <= health.max * 0.5 {
            boss.phase = 2;
            commands.trigger(PhaseChanged { boss: entity, from: 1, to: 2 });
        }
        let phase = boss.phase.max(1) as usize;
        if let Some(moveset) = boss.movesets.get(phase - 1).or(boss.movesets.last())
            && fighter.moveset != *moveset
        {
            fighter.moveset = moveset.clone();
        }
    }
}

/// `SimSet::Cleanup`: remains whose time is up are gone.
pub fn remove_corpses(time: Res<Time>, mut commands: Commands, mut q: Query<(Entity, &mut Corpse, &Remains)>) {
    for (entity, mut corpse, remains) in &mut q {
        corpse.for_s += time.delta_secs();
        if corpse.for_s >= remains.despawn_after {
            commands.entity(entity).despawn();
        }
    }
}

/// The dead are no lock-on targets (the camera lets go); bodies with [`Remains`] start
/// their countdown.
fn on_died(died: On<Died>, mut commands: Commands, q: Query<(Has<LockOnTarget>, Has<Remains>)>) {
    let entity = died.event().entity;
    let Ok((lockable, remains)) = q.get(entity) else { return };
    if lockable {
        commands.entity(entity).remove::<LockOnTarget>();
    }
    if remains {
        commands.entity(entity).insert(Corpse::default());
    }
}

/// A monster brought back (the dev `heal` tool) is a target again, and stays.
fn on_healed(
    healed: On<Healed>,
    mut commands: Commands,
    q: Query<(Has<Dead>, Has<LockOnTarget>, Has<Corpse>), Or<(With<Skeleton>, With<Boss>)>>,
) {
    let entity = healed.event().entity;
    let Ok((dead, lockable, corpse)) = q.get(entity) else { return };
    if dead {
        return;
    }
    if !lockable {
        commands.entity(entity).insert(LockOnTarget);
    }
    if corpse {
        commands.entity(entity).remove::<Corpse>();
    }
}

fn on_volume_entered(
    entered: On<VolumeEntered>,
    mut commands: Commands,
    volumes: Query<&Volume>,
    players: Query<(), With<Player>>,
) {
    let VolumeEntered { entity, volume } = *entered.event();
    let goal = volumes.get(volume).is_ok_and(|v| matches!(&v.kind, VolumeKind::Trigger { name } if name == GOAL));
    if goal && players.contains(entity) {
        commands.trigger(ReachedGoal { entity, goal: volume });
    }
}
