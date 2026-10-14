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
fn noises_are_heard_within_their_radius() {
    let mut app = app();
    let (watcher, hero) = watcher_and_hero(&mut app, Vec3::new(0.0, 0.0, 6.0));
    step(&mut app, 1);
    assert!(knows(&app, watcher, hero).is_none(), "behind it, unseen");

    let at = feet(&app, hero);
    app.world_mut().trigger(Noise { position: at, radius: 4.0, source: Some(hero) });
    step(&mut app, 1);
    assert!(knows(&app, watcher, hero).is_none(), "a 4 m noise 6 m away is not heard");

    app.world_mut().trigger(Noise { position: at, radius: 8.0, source: Some(hero) });
    step(&mut app, 1);
    let k = knows(&app, watcher, hero).expect("an 8 m noise is");
    assert!(k.heard && !k.seen, "{k:?}");

    // A sound with no hostile source: something to investigate.
    let door = Vec3::new(3.0, 0.0, 3.0);
    app.world_mut().trigger(Noise { position: door, radius: 10.0, source: None });
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
