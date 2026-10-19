//! Snapshots and the rules: a load puts back what the snapshot holds (no rules included), a refused
//! load puts back the world's own, and neutral rules hash as none.

use shards::ai_core::{replay, snapshot, JournalKind};
use shards::motion::{Collider, Shape};
use shards::prelude::*;
use shards::rules::{replay_check, ActiveRules, RuleClass};

use crate::*;

/// Health `(current, max)` of the bodies of class `brute`.
fn brutes(app: &mut App) -> Vec<(f32, f32)> {
    let world = app.world_mut();
    world.query::<(&Health, &RuleClass)>().iter(world).filter(|(_, c)| c.0 == "brute").map(|(h, _)| (h.current, h.max)).collect()
}

#[test]
fn a_snapshot_with_rules_loads_back_equal_and_rescales_nothing() {
    let folder = Folder::new("snapshot");
    let mut app = game(&folder);
    body(&mut app, 0, 40.0);
    // `harsh` gives `brute` health ×1.5: the body that would show a second scale.
    app.world_mut().spawn((Name::new("brute"), AiKind::new("brute"), RuleClass("brute".into()), Health::new(40.0), Transform::default()));
    step(&mut app, 2);
    ok(&mut app, "rules_set", serde_json::json!({"mode": "harsh"}));
    // One tick at `harsh`: the bodies are brought to it (from C3.3a on) before the snapshot is taken.
    step(&mut app, 1);
    let taken = snapshot::capture(app.world()).unwrap();
    ok(&mut app, "rules_set", serde_json::json!({"mode": "gentle"}));
    step(&mut app, 3);
    assert_eq!(rules(&app), playing("gentle", 3, 1));

    let report = snapshot::restore_exact(app.world_mut(), &taken, false).unwrap();
    assert!(report.warnings.iter().any(|w| w == "rules: `harsh` from the snapshot"), "{:?}", report.warnings);
    assert_eq!(rules(&app), playing("harsh", 2, 1));
    step(&mut app, 10);
    let world = app.world_mut();
    let health: Vec<(f32, f32)> = world.query_filtered::<&Health, With<PlayerId>>().iter(world).map(|h| (h.current, h.max)).collect();
    assert_eq!(health, [(40.0, 40.0)], "no rescale");
    // C3.1a has no body pass yet: the brute keeps its 40. From C3.3a on the snapshot holds it at 60/60 (40 × 1.5,
    // scaled once at `harsh` with its key) and the load and the 10 ticks keep 60/60: C3.3a changes this to
    // [(60.0, 60.0)], never 90 (a second scale).
    assert_eq!(brutes(&mut app), [(40.0, 40.0)], "the snapshot's body, scaled no second time");
    assert_eq!(rules(&app), playing("harsh", 2, 1), "the same session, the same key");
    assert_eq!(lines(&app, JournalKind::Event, "(save)"), Vec::<String>::new(), "a snapshot load is no change of rules");
}

#[test]
fn a_snapshot_without_rules_removes_them_and_a_refused_load_puts_them_back() {
    let folder = Folder::new("without");
    let mut app = game(&folder);
    ok(&mut app, "rules_set", serde_json::json!({"mode": "harsh"}));
    let held = app.world_mut().remove_resource::<ActiveRules>().unwrap();
    let without = snapshot::capture(app.world()).unwrap();
    assert!(!without.resources.contains_key("shards_rules::state::ActiveRules"));
    app.world_mut().insert_resource(held.clone());

    // Alone, the load removes them.
    let report = snapshot::restore_exact(app.world_mut(), &without, false).unwrap();
    assert!(report.warnings.iter().any(|w| w == "rules: none, as the snapshot was taken without them"), "{:?}", report.warnings);
    assert_eq!(rules(&app), None);

    // A load refused after the prepares ran (a static collider the snapshot does not have) puts them back.
    app.world_mut().insert_resource(held.clone());
    app.world_mut().spawn((Collider::new(Shape::cuboid(Vec3::new(3.0, 0.5, 3.0))), Transform::from_xyz(0.0, -0.5, 0.0)));
    let refusal = snapshot::restore_exact(app.world_mut(), &without, false).unwrap_err();
    assert!(refusal.contains("static"), "{refusal}");
    assert_eq!(rules(&app), Some(held.clone()), "the rollback runs the prepare with the world's own snapshot");

    // A co-op host sets them neutral instead: a removal does not travel.
    let mut app = game(&folder);
    ok(&mut app, "rules_set", serde_json::json!({"mode": "harsh"}));
    let held = app.world_mut().remove_resource::<ActiveRules>().unwrap();
    let without = snapshot::capture(app.world()).unwrap();
    app.world_mut().insert_resource(held.clone());
    app.world_mut().insert_resource(NetRole::Host);
    snapshot::restore_exact(app.world_mut(), &without, false).unwrap();
    assert_eq!(rules(&app), playing("", held.generation, held.players));
}

#[test]
fn neutral_rules_hash_as_none() {
    let folder = Folder::new("neutral");
    let mut app = game(&folder);
    assert!(replay_check(app.world()).is_some(), "rules in play are hashed");
    let held = app.world_mut().remove_resource::<ActiveRules>().unwrap();
    let absent = replay::state_hash(app.world());
    app.world_mut().insert_resource(ActiveRules { mode: String::new(), ..held });
    assert_eq!(replay_check(app.world()), None);
    assert_eq!(replay::state_hash(app.world()), absent, "a neutral resource hashes as no resource");
}
