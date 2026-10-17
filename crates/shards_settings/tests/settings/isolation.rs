//! Settings are per machine: not in snapshots, replays, the replay hash or what a host replicates.

use std::any::TypeId;

use serde_json::json;
use shards::ai_core::net_rules::{replicated_resources, NetRules};
use shards::ai_core::{replay, snapshot};
use shards::settings::{AccessibilitySettings, AudioSettings, ControlSettings, GraphicsSettings, SettingsRegistry};

use crate::common::*;

#[test]
fn settings_are_not_simulation_state() {
    let mut app = game();
    let hash = replay::state_hash(app.world());
    let snap = serde_json::to_value(snapshot::capture(app.world()).unwrap()).unwrap();
    assert!(!snap.to_string().contains("Settings"), "no settings resource in a snapshot");

    for (group, key, value) in [
        ("graphics", "fullscreen", "true"),
        ("audio", "master", "0.1"),
        ("controls", "invert_y", "true"),
        ("accessibility", "fov_deg", "110"),
        ("accessibility", "hold_to_toggle", "true"),
    ] {
        set(&mut app, group, key, value);
    }
    app.update();

    assert_eq!(replay::state_hash(app.world()), hash, "the replay hash does not see settings");
    let after = serde_json::to_value(snapshot::capture(app.world()).unwrap()).unwrap();
    assert_eq!(after, snap, "a snapshot is the same whatever the player's settings are");
}

#[test]
fn a_host_does_not_replicate_settings() {
    let mut app = game();
    let world = app.world_mut();
    let replicated = replicated_resources(world);
    assert!(replicated.iter().all(|name| !name.contains("Settings")), "{replicated:?}");
    let rules = world.resource::<NetRules>();
    for id in [TypeId::of::<GraphicsSettings>(), TypeId::of::<AudioSettings>(), TypeId::of::<ControlSettings>(), TypeId::of::<AccessibilitySettings>()] {
        assert!(rules.is_resource_ignored(id), "named as per machine, even if a game snapshots everything one day");
    }
    // And the protocol check does not depend on whether the player changed them.
    let rules_hash = shards::ai_core::net_rules::rules_hash(world);
    set(&mut app, "audio", "master", "0.3");
    assert_eq!(shards::ai_core::net_rules::rules_hash(app.world()), rules_hash);
}

#[test]
fn the_engine_groups_are_registered_in_order() {
    let app = game();
    // The four of this crate, then the language of `shards_locale`, which the engine adds after it.
    assert_eq!(app.world().resource::<SettingsRegistry>().names(), ["graphics", "audio", "controls", "accessibility", "locale"]);
    let status = shards::ai_core::status::status(app.world());
    assert_eq!(status["settings"]["groups"], json!(["graphics", "audio", "controls", "accessibility", "locale"]));
    assert_eq!(status["settings"]["store"], json!(null), "no store in a test app");
    assert!(status["engine"]["modules"].to_string().contains("settings"), "{status}");
}
