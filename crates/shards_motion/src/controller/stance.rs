//! Crouching (F2): the stance of a character, kept in [`Crouch`], and the rules that move it.
//!
//! A body whose `locomotion` has a `crouch_height` lowers its capsule while the `crouch` input is
//! held (`Intent::held`, so the keyboard, the `crouch` tool, a brain and a replay all write the same)
//! and raises it again when it is let go. [`Crouch::depth`] goes from 0 (standing) to 1 (crouched) at
//! an even rate of `1 / crouch_time` a second, and the capsule's height (and with it the body's
//! hurtbox and eye) is [`CharacterParams::height_at`] that depth.
//!
//! - **Which end moves.** On the ground the feet stay and the head comes down; in the air the head
//!   stays and the feet are pulled up, so a crouch in the middle of a jump clears a ledge a plain jump
//!   misses, as in *Half-Life*. The air shift is added to the body's feet in the tick it happens.
//! - **Standing up waits for room.** A stand-up is a shape cast of the standing capsule: upward on the
//!   ground, downward in the air (the feet would drop). While it is blocked the body stays as it is,
//!   [`Crouch::blocked`] says so, and nothing is half-stood in a ceiling or grown into a floor.
//! - **The stance is state.** [`Crouch`] is a reflected component, in snapshots and the replay hash, put
//!   on a body by the controller the first time it crouches (and never on one that cannot or does not:
//!   its hash is what it was before the stance existed). A client predicts it for its own body.
//! - **Events.** [`Crouched`] when a body leaves the standing height, [`Stood`] when it is back at it.
//!
//! The controller's casts, the body's collider, the movers that carry it and `clearance` all ask for the
//! size of the body through [`CharacterParams::at_depth`], so every one of them sees the stance.

use bevy::prelude::*;

use super::mover::Mover;
use super::params::CharacterParams;
use super::MotionMode;
use crate::physics::{CollisionLayers, PhysicsWorld};

/// A stand-up needs the standing capsule's cast to come up short by more than this (m) to count as
/// blocked: rounding is not a ceiling.
const ROOM_SLACK: f32 = 1e-4;

/// How crouched a character is. Put on a body by the controller the first time it crouches (its
/// `locomotion` must give a `crouch_height`); a body without it is standing.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct Crouch {
    /// 0 standing, 1 crouched, in between while the capsule moves between the two heights. A body
    /// is *crouched* while this is above 0: it stands only when the capsule is back at its full
    /// height.
    pub depth: f32,
    /// The stance asked for: the `crouch` input was held on the last tick the body could use it. It
    /// keeps its value while the body is rooted (a scene that took the movement away).
    pub wanted: bool,
    /// The body wants to stand and the space it would stand in is not free (a ceiling above, or in
    /// the air the floor below), so it stays as it is.
    pub blocked: bool,
}

impl Crouch {
    /// The capsule is below its full height.
    pub fn is_crouched(&self) -> bool {
        self.depth > 0.0
    }
}

/// A character left its standing height: it started to crouch. The event of a body that cannot be seen
/// to crouch again before it has [`Stood`].
#[derive(Event, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crouched {
    pub entity: Entity,
}

/// A character that was crouched is at its full height again (a stand-up blocked by a ceiling fires
/// nothing until it succeeds).
#[derive(Event, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stood {
    pub entity: Entity,
}

/// What the stance did this tick.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Settled {
    /// The stance after the tick.
    pub crouch: Crouch,
    /// How far the feet move up this tick (negative: down): the capsule shrank or grew from its
    /// bottom, in the air. 0 on the ground.
    pub lift: f32,
    /// The body left its standing height this tick.
    pub crouched: bool,
    /// The body came back to its standing height this tick.
    pub stood: bool,
}

/// How much of the way to the standing capsule a body `depth` into a crouch has room for, and how far it
/// has to go: `(room, lacking)` in meters. The crouched capsule cast up by what it lacks sweeps out
/// exactly the standing capsule; in the `air` it is cast down instead (the feet would drop that far).
/// The body stands when `room` reaches `lacking`.
pub(crate) fn room_to_stand(
    physics: &PhysicsWorld,
    params: &CharacterParams,
    who: (Entity, CollisionLayers),
    air: bool,
    feet: Vec3,
    depth: f32,
) -> (f32, f32) {
    let lacking = params.height - params.height_at(depth);
    let now = params.at_depth(depth);
    let mover = Mover::new(physics, &now, who.0, who.1);
    (mover.free(feet, if air { Vec3::NEG_Y } else { Vec3::Y }, lacking), lacking)
}

/// One tick of a body's stance: the input `held` (or, `rooted`, the stance it already wanted) moves
/// the depth at `1 / crouch_time` a second, and a stand-up is cast first. `mode` says which end of the
/// capsule moves; a climb holds the stance as it is.
#[allow(clippy::too_many_arguments, reason = "everything one body's stance reads, named once")]
pub(crate) fn settle(
    physics: &PhysicsWorld,
    params: &CharacterParams,
    who: (Entity, CollisionLayers),
    mode: MotionMode,
    feet: Vec3,
    before: Crouch,
    held: bool,
    rooted: bool,
    dt: f32,
) -> Settled {
    let unchanged = Settled { crouch: before, lift: 0.0, crouched: false, stood: false };
    if !params.can_crouch() {
        // A body whose params lost the crouch (a hot reload, another locomotion) stands as it is.
        return match before {
            Crouch { depth, .. } if depth > 0.0 => Settled { crouch: Crouch::default(), stood: true, ..unchanged },
            _ => Settled { crouch: Crouch::default(), ..unchanged },
        };
    }
    if mode == MotionMode::Mantle {
        return unchanged;
    }
    let wanted = if rooted { before.wanted } else { held };
    let step = if params.crouch_time > 0.0 { dt / params.crouch_time } else { 1.0 };
    let air = matches!(mode, MotionMode::Air | MotionMode::Glide);
    let mut crouch = Crouch { wanted, blocked: false, ..before };
    if wanted {
        crouch.depth = (before.depth + step).min(1.0);
    } else if before.depth > 0.0 {
        // Room for the standing capsule: the crouched one cast up by what it lacks (it sweeps out exactly
        // the standing capsule), or, in the air, down by the same (the feet would drop that far).
        let (room, lacking) = room_to_stand(physics, params, who, air, feet, before.depth);
        if room + ROOM_SLACK < lacking {
            crouch.blocked = true;
        } else {
            crouch.depth = (before.depth - step).max(0.0);
        }
    }
    // In the air the head stays: a shrinking capsule pulls the feet up, a growing one lets them down.
    let lift = if air { params.height_at(before.depth) - params.height_at(crouch.depth) } else { 0.0 };
    Settled { crouch, lift, crouched: before.depth <= 0.0 && crouch.depth > 0.0, stood: before.depth > 0.0 && crouch.depth <= 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::physics::{Collider, ColliderId, PhysicsWorld};

    fn crouching() -> CharacterParams {
        CharacterParams { crouch_height: Some(1.0), crouch_time: 0.2, ..default() }
    }

    fn empty() -> (PhysicsWorld, Entity) {
        (PhysicsWorld::default(), Entity::from_raw_u32(1).unwrap())
    }

    fn settle_in(physics: &PhysicsWorld, who: Entity, mode: MotionMode, before: Crouch, held: bool) -> Settled {
        settle(physics, &crouching(), (who, CollisionLayers::CHARACTER), mode, Vec3::ZERO, before, held, false, 0.05)
    }

    #[test]
    fn holding_crouch_goes_down_at_one_over_crouch_time_and_letting_go_comes_back() {
        let (physics, who) = empty();
        let mut now = Crouch::default();
        for tick in 1..=4 {
            let s = settle_in(&physics, who, MotionMode::Ground, now, true);
            assert_eq!(s.crouched, tick == 1, "the event is the tick it leaves standing");
            assert_eq!(s.lift, 0.0, "on the ground the feet stay");
            now = s.crouch;
        }
        assert_eq!(now, Crouch { depth: 1.0, wanted: true, blocked: false }, "0.2 s at 0.05 s a tick is four ticks");
        let mut stood = false;
        for _ in 0..4 {
            let s = settle_in(&physics, who, MotionMode::Ground, now, false);
            stood |= s.stood;
            now = s.crouch;
        }
        assert!(stood && now == Crouch::default(), "{now:?}");
    }

    #[test]
    fn the_air_shift_pulls_the_feet_up_by_what_the_capsule_lost_and_lets_them_down_again() {
        let (physics, who) = empty();
        let down = settle_in(&physics, who, MotionMode::Air, Crouch::default(), true);
        // A quarter of 0.8 m in one 0.05 s step of a 0.2 s crouch.
        assert!((down.lift - 0.2).abs() < 1e-6, "{down:?}");
        let up = settle_in(&physics, who, MotionMode::Air, down.crouch, false);
        assert!((up.lift + 0.2).abs() < 1e-6, "{up:?}");
    }

    #[test]
    fn a_climb_holds_the_stance() {
        let (physics, who) = empty();
        let s = settle_in(&physics, who, MotionMode::Mantle, Crouch::default(), true);
        assert_eq!(s.crouch, Crouch::default());
    }

    #[test]
    fn a_rooted_body_keeps_what_it_wanted() {
        let (physics, who) = empty();
        let before = Crouch { depth: 0.5, wanted: true, blocked: false };
        let s = settle(&physics, &crouching(), (who, CollisionLayers::CHARACTER), MotionMode::Ground, Vec3::ZERO, before, false, true, 0.05);
        assert!(s.crouch.wanted && s.crouch.depth > 0.5, "it goes on to the stance it wanted: {s:?}");
    }

    #[test]
    fn a_body_that_cannot_crouch_stands() {
        let (physics, who) = empty();
        let params = CharacterParams::default();
        let before = Crouch { depth: 0.6, wanted: true, blocked: false };
        let s = settle(&physics, &params, (who, CollisionLayers::CHARACTER), MotionMode::Ground, Vec3::ZERO, before, true, false, 0.05);
        assert_eq!((s.crouch, s.stood), (Crouch::default(), true));
        let none = settle(&physics, &params, (who, CollisionLayers::CHARACTER), MotionMode::Ground, Vec3::ZERO, Crouch::default(), true, false, 0.05);
        assert_eq!((none.crouch, none.stood, none.crouched), (Crouch::default(), false, false), "a press does nothing");
    }

    #[test]
    fn in_the_air_a_stand_up_is_held_back_by_a_floor_just_below_the_feet() {
        let mut physics = PhysicsWorld::default();
        let floor = Entity::from_raw_u32(7).unwrap();
        let slab = Collider::new(crate::physics::Shape::Cuboid { half: Vec3::new(20.0, 0.5, 20.0) });
        physics.upsert((floor, &ColliderId(1), &slab, &Transform::from_xyz(0.0, -0.5, 0.0), None, None));
        let who = Entity::from_raw_u32(1).unwrap();
        let crouched = Crouch { depth: 1.0, wanted: false, blocked: false };
        let from = |y: f32| settle(&physics, &crouching(), (who, CollisionLayers::CHARACTER), MotionMode::Air, Vec3::new(0.0, y, 0.0), crouched, false, false, 0.05);
        // 0.4 m up, growing the 0.8 m the capsule lacks would put the feet 0.4 m into the floor: it waits.
        let low = from(0.4);
        assert!(low.crouch.blocked && low.crouch.depth == 1.0 && low.lift == 0.0, "{low:?}");
        // A metre up there is room, and the feet go down as the capsule grows from its bottom.
        let high = from(1.0);
        assert!(!high.crouch.blocked && high.crouch.depth < 1.0 && high.lift < 0.0, "{high:?}");
    }

    #[test]
    fn a_ceiling_holds_the_body_down_until_it_is_clear() {
        let mut physics = PhysicsWorld::default();
        let ceiling = Entity::from_raw_u32(7).unwrap();
        // A slab 1.3 m over the floor: room for the 1.0 m crouch (and its skin), not for the 1.8 m stand.
        let slab = Collider::new(crate::physics::Shape::Cuboid { half: Vec3::new(2.0, 0.2, 2.0) });
        physics.upsert((ceiling, &ColliderId(1), &slab, &Transform::from_xyz(0.0, 1.5, 0.0), None, None));
        let who = Entity::from_raw_u32(1).unwrap();
        let crouched = Crouch { depth: 1.0, wanted: true, blocked: false };
        let feet = Vec3::new(0.0, 0.02, 0.0);
        let held = settle(&physics, &crouching(), (who, CollisionLayers::CHARACTER), MotionMode::Ground, feet, crouched, false, false, 0.05);
        assert_eq!(held.crouch, Crouch { depth: 1.0, wanted: false, blocked: true }, "{held:?}");
        assert!(!held.stood);
        // Walked out from under it: the same cast finds room.
        let clear = settle(&physics, &crouching(), (who, CollisionLayers::CHARACTER), MotionMode::Ground, feet + Vec3::X * 5.0, crouched, false, false, 0.05);
        assert!(!clear.crouch.blocked);
        assert!(clear.crouch.depth < 1.0, "{clear:?}");
    }
}
