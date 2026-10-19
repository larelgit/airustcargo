//! What happens around fights and on the course: the boss's phases, the dead monsters'
//! remains, the lock-on marker that follows life and death, who goes down instead of dying
//! ([`spare`]), and the goal pad.

use shards::action::{ActionAppExt, ActionDef, Died, Fate, Healed, Lethal, LethalCause, MovesetDef, Standing};
use shards::content::ContentDb;
use shards::motion::{LockOnTarget, Volume, VolumeEntered, VolumeKind};
use shards::prelude::*;

use super::{Boss, Player, Skeleton};

/// The name of the course's goal trigger (`Trigger(name: "goal")` in the layout).
pub const GOAL: &str = "goal";

pub(super) fn plugin(app: &mut App) {
    app.register_type::<Remains>()
        .register_type::<Corpse>()
        .add_lethal_hook(spare)
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
/// (hits or a dev `set`); each phase fights with its own moveset and attack plan
/// (`Boss::movesets`, `Boss::plans`).
pub fn boss_phases(mut commands: Commands, mut q: Query<(Entity, &mut Boss, &Health, &mut Fighter, Option<&mut Npc>, Has<Dead>)>) {
    for (entity, mut boss, health, mut fighter, npc, dead) in &mut q {
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
        if let (Some(mut npc), Some(plan)) = (npc, boss.plans.get(phase - 1).or(boss.plans.last()))
            && npc.plan.as_ref() != Some(plan)
        {
            npc.plan = Some(plan.clone());
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

/// The sandbox's lethal hook (B5): a fighter whose moveset has a `downed` pose goes down instead of
/// dying to a hit while another player still stands (has health, neither dead nor down), and
/// bleeds out when the pose's time runs out (the `partner` moveset's `spar_down`: 20 s). Any other
/// death (no pose, no partner standing, health run out some other way) is a death: a fighter
/// with no `downed` pose, every one before B5, dies exactly as before.
fn spare(world: &mut World, lethal: &Lethal) -> Fate {
    if lethal.cause != LethalCause::Hit {
        return Fate::Die;
    }
    let db = world.resource::<ContentDb>();
    let Some(pose) = world.get::<Fighter>(lethal.entity).and_then(|f| db.get::<MovesetDef>(&f.moveset)).and_then(|m| m.downed.clone()) else {
        return Fate::Die;
    };
    let Some(bleed) = db.get::<ActionDef>(&pose).map(ActionDef::length) else { return Fate::Die };
    let partner = world
        .try_query_filtered::<(Entity, &Health), (With<PlayerId>, Standing)>()
        .is_some_and(|mut q| q.iter(world).any(|(e, h)| e != lethal.entity && h.current > 0.0));
    if partner { Fate::Down { bleed: Some(bleed) } } else { Fate::Die }
}
