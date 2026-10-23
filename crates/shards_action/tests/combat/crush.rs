//! A mover that crushes (F6, `Blocked::Crush`) hurts the body it pins through `deal_damage`: when
//! the hold begins and every second it lasts.

use bevy::prelude::*;
use shards_action::prelude::*;
use shards_motion::{mover_go, Collider, Mover, Shape};

use crate::common::*;

#[test]
fn a_press_deals_its_damage_to_the_fighter_it_pins_every_second() {
    let mut app = app();
    let brute = brute(app.world_mut(), Vec3::ZERO, N);
    // Its bottom comes down from 3.0 m to 0.4 m: the brute (1.8 m) holds it.
    let press = app
        .world_mut()
        .spawn((Name::new("Press"), Mover { def: "press".into() }, Collider::new(Shape::cuboid(Vec3::new(1.0, 0.5, 1.0))), Transform::from_xyz(0.0, 3.5, 0.0)))
        .id();
    step(&mut app, 5);
    mover_go(app.world_mut(), press, 1).unwrap();
    // About 1.2 s down to the head, then 2.5 s held: hurt at the start and after one and two seconds.
    step(&mut app, 222);
    assert_eq!(app.world().get::<Health>(brute).unwrap().current, 70.0);
    // It lets go once sent back up, and hurts no more.
    mover_go(app.world_mut(), press, 0).unwrap();
    step(&mut app, 120);
    assert_eq!(app.world().get::<Health>(brute).unwrap().current, 70.0);
}
