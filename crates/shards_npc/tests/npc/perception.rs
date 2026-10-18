//! Sight through the physics world, hearing as events, memory.

use bevy::prelude::*;
use shards_action::Damaged;
use shards_core::input_names::SPRINT;
use shards_core::Intent;
use shards_npc::prelude::*;

use crate::common::*;

fn watcher_and_hero(app: &mut App, hero_at: Vec3) -> (Entity, Entity) {
    let world = app.world_mut();
    let watcher = npc(world, "grunt", Npc::new("watcher", "eyes"), Vec3::ZERO, N);
    let hero = player(world, 0, hero_at);
    (watcher, hero)
}

fn knows(app: &App, npc: Entity, who: Entity) -> Option<shards_npc::Sensed> {
    app.world().get::<Perceived>(npc).unwrap().get(aid(app, who)).cloned()
}

fn sees(app: &App, npc: Entity, who: Entity) -> bool {
    knows(app, npc, who).is_some_and(|k| k.seen)
}

#[test]
fn sight_needs_range_field_of_view_and_a_clear_line() {
    let mut app = app();
    // `eyes`: 12 m, 120° (60° to either side), 1.5 m all round.
    let (watcher, hero) = watcher_and_hero(&mut app, Vec3::new(0.0, 0.0, -6.0));
    let look = |app: &mut App, at: Vec3| {
        teleport(app, hero, at);
        step(app, 1);
        sees(app, watcher, hero)
    };
    assert!(look(&mut app, Vec3::new(0.0, 0.0, -6.0)), "6 m ahead");
    assert!(!look(&mut app, Vec3::new(0.0, 0.0, 6.0)), "6 m behind");
    assert!(look(&mut app, Vec3::new(0.0, 0.0, 1.2)), "behind but within reach of its senses");
    assert!(look(&mut app, Vec3::new(4.0, 0.0, -5.0)), "39° off its facing");
    assert!(!look(&mut app, Vec3::new(6.0, 0.0, -2.0)), "72° off its facing");
    assert!(!look(&mut app, Vec3::new(0.0, 0.0, -12.5)), "out of range");
    wall(app.world_mut(), Vec3::new(-2.0, 0.0, -4.0), Vec3::new(2.0, 3.0, -3.5));
    assert!(!look(&mut app, Vec3::new(0.0, 0.0, -6.0)), "a wall in between");
    assert!(look(&mut app, Vec3::new(0.0, 0.0, -3.0)), "in front of the wall");
    // A low wall does not hide a standing body from eyes at 1.6 m.
    let world = app.world_mut();
    let low = wall(world, Vec3::new(-2.0, 0.0, -2.5), Vec3::new(2.0, 0.4, -2.2));
    assert!(look(&mut app, Vec3::new(0.0, 0.0, -3.0)), "over a low wall");
    app.world_mut().despawn(low);
    // The focus follows what it perceives.
    assert_eq!(app.world().get::<Perceived>(watcher).unwrap().focus, Some(aid(&app, hero)));
}

#[test]
fn what_is_out_of_sight_is_remembered_for_a_while() {
    let mut app = app();
    let (watcher, hero) = watcher_and_hero(&mut app, Vec3::new(0.0, 0.0, -6.0));
    step(&mut app, 1);
    assert!(sees(&app, watcher, hero));
    teleport(&mut app, hero, Vec3::new(0.0, 0.0, 8.0));
    step(&mut app, 30);
    let k = knows(&app, watcher, hero).expect("still known half a second later");
    assert!(!k.seen && (k.pos - Vec3::new(0.0, 0.0, -6.0)).length() < 0.1, "at its last known place: {k:?}");
    // `eyes` remembers 1 s.
    step(&mut app, 31);
    assert!(knows(&app, watcher, hero).is_none(), "forgotten after its memory");
    assert_eq!(app.world().get::<Perceived>(watcher).unwrap().focus, None);
}

#[test]
fn a_burrowed_body_is_not_perceived_but_remembered() {
    let mut app = app();
    let (watcher, hero) = watcher_and_hero(&mut app, Vec3::new(0.0, 0.0, -6.0));
    step(&mut app, 1);
    assert!(sees(&app, watcher, hero));
    // `hide`: under the ground from 0.3 s to 0.9 s, in plain view 6 m ahead.
    assert!(shards_action::force_action(app.world_mut(), hero, "hide"));
    step(&mut app, 30);
    let k = knows(&app, watcher, hero).expect("a burrowed body is hidden, not gone: still remembered");
    assert!(!k.seen, "not seen while under the ground: {k:?}");
    assert_eq!(app.world().get::<Perceived>(watcher).unwrap().focus, Some(aid(&app, hero)), "still its focus");
    // Out again: seen at once.
    step(&mut app, 30);
    assert!(sees(&app, watcher, hero));
}

#[test]
fn noises_are_heard_within_their_radius() {
    let mut app = app();
    let (watcher, hero) = watcher_and_hero(&mut app, Vec3::new(0.0, 0.0, 6.0));
    step(&mut app, 1);
    assert!(knows(&app, watcher, hero).is_none(), "behind it, unseen");

    let at = feet(&app, hero);
    app.world_mut().trigger(Noise { position: at, radius: 4.0, source: Some(hero), origin: None });
    step(&mut app, 1);
    assert!(knows(&app, watcher, hero).is_none(), "a 4 m noise 6 m away is not heard");

    app.world_mut().trigger(Noise { position: at, radius: 8.0, source: Some(hero), origin: None });
    step(&mut app, 1);
    let k = knows(&app, watcher, hero).expect("an 8 m noise is");
    assert!(k.heard && !k.seen, "{k:?}");

    // A sound with no hostile source: something to investigate.
    let door = Vec3::new(3.0, 0.0, 3.0);
    app.world_mut().trigger(Noise { position: door, radius: 10.0, source: None, origin: None });
    step(&mut app, 1);
    assert_eq!(app.world().get::<Perceived>(watcher).unwrap().noise.map(|n| n.pos), Some(door));
}

#[test]
fn hits_and_sprints_make_noise() {
    let mut app = app();
    let (watcher, hero) = watcher_and_hero(&mut app, Vec3::new(0.0, 0.0, 6.0));
    let victim = fighter(app.world_mut(), "Dummy", "grunt", "monsters", Vec3::new(1.0, 0.0, 5.0), N);
    step(&mut app, 1);
    // A hit (the engine's `Damaged`) is heard around the one hit, made by the attacker.
    let hit = Damaged {
        attacker: hero,
        target: victim,
        action: "hero_swing".into(),
        amount: 5.0,
        poise: 0.0,
        hp_left: 95.0,
        invulnerable: false,
        ignore_reason: Default::default(),
        weight: Default::default(),
        reaction: None,
        absorbed: 0.0,
        source: None,
    };
    app.world_mut().trigger(hit);
    // The hit's noise is a command of the observer: heard on the first tick that applies it.
    step(&mut app, 2);
    let k = knows(&app, watcher, hero).expect("the hit gives the attacker away");
    assert!(!k.seen && k.age <= DT * 1.5, "{k:?}");

    // Sprinting is heard within `sprint_noise` (8 m).
    step(&mut app, 70);
    assert!(knows(&app, watcher, hero).is_none(), "forgotten again");
    let mut intent = app.world_mut().get_mut::<Intent>(hero).unwrap();
    intent.move_dir = E;
    intent.set_held(SPRINT, true);
    let mut heard = false;
    for _ in 0..20 {
        step(&mut app, 1);
        heard |= knows(&app, watcher, hero).is_some_and(|k| k.heard);
    }
    assert!(heard, "a sprint 6 m behind it is heard");
}

/// A hit heard by the one it hit does not move what it sees: the attacker stays where it
/// stands, not at the victim's own feet.
#[test]
fn a_hit_heard_does_not_move_an_attacker_in_sight() {
    let mut app = app();
    let (watcher, hero) = watcher_and_hero(&mut app, Vec3::new(0.0, 0.0, -3.0));
    step(&mut app, 1);
    assert!(sees(&app, watcher, hero));
    let hit = Damaged {
        attacker: hero,
        target: watcher,
        action: "hero_swing".into(),
        amount: 5.0,
        poise: 0.0,
        hp_left: 95.0,
        invulnerable: false,
        ignore_reason: Default::default(),
        weight: Default::default(),
        reaction: None,
        absorbed: 0.0,
        source: None,
    };
    app.world_mut().trigger(hit);
    let mut heard = false;
    for _ in 0..3 {
        step(&mut app, 1);
        let k = knows(&app, watcher, hero).expect("in sight");
        assert!(k.seen, "{k:?}");
        assert!((k.pos - feet(&app, hero)).length() < 0.05, "where it stands, not at the victim's feet: {k:?}");
        heard |= k.heard;
    }
    assert!(heard, "the hit was heard all the same");
}

/// A hit heard by the one it hit, from an attacker it does not see (behind it, or beyond its
/// sight), tells where the attacker stands, not the victim's own feet. The noise is still
/// heard around the one hit: a shot from 14 m away reaches a victim the 10 m noise would
/// not reach from the attacker.
#[test]
fn a_hit_from_an_attacker_out_of_sight_tells_where_it_stands() {
    for hero_at in [Vec3::new(0.0, 0.0, 6.0), Vec3::new(0.0, 0.0, 14.0)] {
        let mut app = app();
        let (watcher, hero) = watcher_and_hero(&mut app, hero_at);
        step(&mut app, 1);
        assert!(knows(&app, watcher, hero).is_none(), "{hero_at}: unseen");
        let hit = Damaged {
            attacker: hero,
            target: watcher,
            action: "hero_shot".into(),
            amount: 5.0,
            poise: 0.0,
            hp_left: 95.0,
            invulnerable: false,
            ignore_reason: Default::default(),
            weight: Default::default(),
            reaction: None,
            absorbed: 0.0,
            source: None,
        };
        app.world_mut().trigger(hit);
        step(&mut app, 2);
        let k = knows(&app, watcher, hero).unwrap_or_else(|| panic!("{hero_at}: the hit gives the attacker away"));
        assert!(!k.seen, "{hero_at}: {k:?}");
        assert!((k.pos - feet(&app, hero)).length() < 0.05, "{hero_at}: where the attacker stands, not at the victim's feet: {k:?}");
        assert_eq!(app.world().get::<Perceived>(watcher).unwrap().focus, Some(aid(&app, hero)), "{hero_at}: it takes the attacker for its target");
    }
}

/// The attacker's place is for those who take it for a hostile fighter: a hit by one of its
/// own side is still somewhere to investigate, the place of the hit.
#[test]
fn a_hit_by_a_friend_is_a_place_to_look_at_not_the_friends_position() {
    let mut app = app();
    let (watcher, hero) = watcher_and_hero(&mut app, Vec3::new(0.0, 0.0, 6.0));
    let friend = fighter(app.world_mut(), "Grunt", "grunt", "monsters", Vec3::new(0.0, 0.0, 12.0), N);
    step(&mut app, 1);
    let hit = Damaged {
        attacker: friend,
        target: hero,
        action: "grunt_swing".into(),
        amount: 5.0,
        poise: 0.0,
        hp_left: 95.0,
        invulnerable: false,
        ignore_reason: Default::default(),
        weight: Default::default(),
        reaction: None,
        absorbed: 0.0,
        source: None,
    };
    app.world_mut().trigger(hit);
    step(&mut app, 2);
    assert!(knows(&app, watcher, friend).is_none(), "no hostile to know of");
    let noise = app.world().get::<Perceived>(watcher).unwrap().noise.expect("the hit is something to look at");
    assert!((noise.pos - feet(&app, hero)).length() < 0.05, "at the hit, not at the attacker: {noise:?}");
}

// --- keep range ----------------------------------------------------------------------------

/// A known target stays seen (its place refreshed) out to `keep_range`; a new one is noticed
/// only within `sight_range`; without `keep_range` nothing changes.
#[test]
fn a_known_target_stays_seen_out_to_the_keep_range() {
    for (perception, kept) in [("keeper", true), ("plain", false)] {
        let mut app = app();
        let world = app.world_mut();
        let watcher = npc(world, "grunt", Npc::new("watcher", perception), Vec3::ZERO, N);
        let hero = player(world, 0, Vec3::new(0.0, 0.0, -14.0));
        step(&mut app, 2);
        assert!(!sees(&app, watcher, hero), "{perception}: 14 m is beyond the 10 m it notices anew");
        assert!(knows(&app, watcher, hero).is_none());
        teleport(&mut app, hero, Vec3::new(0.0, 0.0, -9.0));
        step(&mut app, 1);
        assert!(sees(&app, watcher, hero), "{perception}: noticed at 9 m");
        // Out to 14 m: a keeper sees it and follows it; the plain eyes lose it at once and
        // chase a point where it was.
        teleport(&mut app, hero, Vec3::new(0.0, 0.0, -14.0));
        step(&mut app, 1);
        assert_eq!(sees(&app, watcher, hero), kept, "{perception}: 14 m, known");
        let k = knows(&app, watcher, hero).unwrap();
        assert_eq!(k.age == 0.0 && (k.pos.z + 14.0).abs() < 0.01, kept, "{perception}: place refreshed only when kept: {k:?}");
        // Beyond the keep range it is gone from sight either way, and fades with the memory.
        teleport(&mut app, hero, Vec3::new(0.0, 0.0, -17.0));
        step(&mut app, 1);
        assert!(!sees(&app, watcher, hero), "{perception}: 17 m is beyond the keep range");
        step(&mut app, 125);
        assert!(knows(&app, watcher, hero).is_none(), "{perception}: forgotten after the memory");
        // Forgotten, it has to come within the sight range again.
        teleport(&mut app, hero, Vec3::new(0.0, 0.0, -14.0));
        step(&mut app, 1);
        assert!(!sees(&app, watcher, hero), "{perception}: a forgotten target is noticed anew within 10 m only");
    }
}

#[test]
fn a_kept_target_is_still_hidden_by_a_wall() {
    let mut app = app();
    let world = app.world_mut();
    let watcher = npc(world, "grunt", Npc::new("watcher", "keeper"), Vec3::ZERO, N);
    let hero = player(world, 0, Vec3::new(0.0, 0.0, -9.0));
    step(&mut app, 1);
    assert!(sees(&app, watcher, hero));
    wall(app.world_mut(), Vec3::new(-3.0, 0.0, -11.0), Vec3::new(3.0, 3.0, -10.5));
    teleport(&mut app, hero, Vec3::new(0.0, 0.0, -14.0));
    step(&mut app, 1);
    assert!(!sees(&app, watcher, hero), "the ray is blocked: not kept");
    assert!(knows(&app, watcher, hero).is_some(), "but remembered");
}
