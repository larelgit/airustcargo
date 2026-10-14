//! `testing::ok` fails a test whose step a gameplay panic stopped. Its own file (its own
//! process): Bevy's error handler queues failures globally.

use shards::prelude::*;
use shards::serde_json::json;

fn cracking_ice() {
    panic!("the ice gave way");
}

#[test]
#[should_panic(expected = "stopped: gameplay system failures::cracking_ice panicked at tick 1")]
fn ok_fails_a_step_that_a_gameplay_panic_stopped() {
    let mut app = shards::testing::test_app(|_| {}, |app| {
        app.add_systems(FixedUpdate, cracking_ice.in_set(SimSet::Resolve));
    });
    shards::testing::ok(&mut app, "step", json!({"ticks": 5}));
}
