//! What an agent sees and does: telegraphs, zones and advice, observations, orders.

use ai_core::observe::{observe, ObserveOptions};
use bevy::prelude::*;
use serde_json::json;
use shards_action::prelude::*;
use shards_action::{hit_zones, threat_indicators, HitShape};
use shards_core::{Intent, Order};

use crate::common::*;

#[test]
fn a_telegraph_journals_its_zone_and_the_advice_leads_out_of_it() {
    let mut app = app();
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let brute = brute(world, Vec3::new(0.0, 0.0, -2.0), S);
    let (h, b) = (id(&app, hero), id(&app, brute));
    press(&mut app, brute, "heavy");
    step(&mut app, 1);
    assert_eq!(lines(&app, "winds up"), [format!("1 {b} winds up slam (0.80s) ⚠ circle 4.5m")]);

    let zones = hit_zones(app.world());
    assert_eq!(zones.len(), 1);
    let zone = &zones[0];
    assert_eq!((zone.action.as_str(), zone.symbol.as_deref()), ("slam", Some("⚠")));
    assert!((zone.lands_in - (0.8 - DT)).abs() < 1e-4 && (zone.active_for - 0.1).abs() < 1e-4, "{zone:?}");
    assert!(matches!(zone.shape.shape, HitShape::Circle { radius, .. } if radius == 4.5));

    let seen = observe(app.world(), &ObserveOptions::default());
    let alert = seen.lines().nth(2).unwrap();
    assert_eq!(alert, format!("ALERT in hit zone of {b} slam (lands in 0.78s): move 2.9m S to leave it"), "{seen}");
    assert!(seen.contains(&format!("{b} Brute  hp 100/100  stam 100/100  poise 30/30  windup slam 0.78s (zone circle 4.5m) hyper-armor")), "{seen}");

    let ahead = threat_indicators(app.world(), hero, N);
    assert_eq!(ahead.len(), 1);
    assert!(ahead[0].angle_deg.abs() < 1e-3 && ahead[0].inside && ahead[0].symbol == "⚠", "{ahead:?}");
    let behind = threat_indicators(app.world(), hero, S);
    assert!((behind[0].angle_deg.abs() - 180.0).abs() < 1e-3, "{behind:?}");
    let east = threat_indicators(app.world(), hero, Vec3::X);
    assert!((east[0].angle_deg + 90.0).abs() < 1e-3, "north is to the left of east: {east:?}");

    // Do as told: run south.
    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = S;
    step(&mut app, 47);
    assert!(lines(&app, &format!("{b} -> {h}")).is_empty(), "the slam missed");
    assert!(feet(&app, hero).z > 2.9, "{}", feet(&app, hero));
    assert!(!observe(app.world(), &ObserveOptions::default()).contains("ALERT"));
}

#[test]
fn the_dodge_tool_rolls_out_of_the_zone_by_default() {
    let mut app = app();
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let brute = brute(world, Vec3::new(0.0, 0.0, -2.0), S);
    press(&mut app, brute, "heavy");
    step(&mut app, 20);
    assert_eq!(ok(&mut app, "dodge", json!({})), "ok: roll (S)");
    step(&mut app, 30);
    assert!(feet(&app, hero).z > 3.5, "rolled 4 m south: {}", feet(&app, hero));
    assert_eq!(hp(&app, hero), 100.0);
}

#[test]
fn an_attack_order_walks_up_and_combos_until_the_target_dies() {
    let mut app = app();
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let dummy = fighter(world, "Dummy", "dummy", "monsters", Vec3::new(3.0, 0.0, -6.0), S);
    let (h, d) = (id(&app, hero), id(&app, dummy));
    assert_eq!(ok(&mut app, "attack", json!({"target": d, "repeat": true})), format!("ok: attacking {d} with light until it dies"));
    step(&mut app, 300);
    let hits: Vec<String> = lines(&app, &format!("{h} -> {d}"));
    let actions: Vec<&str> = hits.iter().map(|l| l.split_whitespace().nth(4).unwrap()).collect();
    assert_eq!(&actions[..3], ["light_1", "light_2", "light_3"], "{hits:#?}");
    assert!(app.world().get::<Dead>(dummy).is_some(), "{:#?}", journal(&app));
    assert!(lines(&app, &format!("{d} died (killed by {h}")).len() == 1);
    assert!(app.world().get::<Intent>(hero).unwrap().orders.is_empty(), "the order ends with the target");
    assert!(lines(&app, "rejected").is_empty(), "orders press only what starts: {:#?}", journal(&app));
}

#[test]
fn lock_on_picks_the_nearest_enemy_and_attacks_follow_it() {
    let mut app = app();
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let far = fighter(world, "Dummy", "dummy", "monsters", Vec3::new(0.0, 0.0, -8.0), S);
    let near = fighter(world, "Dummy", "dummy", "monsters", Vec3::new(1.5, 0.0, 0.0), S);
    let _friend = fighter(world, "Hero", "hero", "heroes", Vec3::new(-1.0, 0.0, 0.0), S);
    let (n, f) = (id(&app, near), id(&app, far));
    assert_eq!(ok(&mut app, "lock_on", json!({})), format!("ok: locked on {n} (1.5m E)"));
    assert_eq!(ok(&mut app, "light_attack", json!({})), "ok: light_1");
    step(&mut app, 13);
    assert_eq!(hp(&app, near), 80.0, "the attack turned to the locked target");
    assert_eq!(ok(&mut app, "lock_on", json!({"target": f})), format!("ok: locked on {f} (8.0m N)"));
    assert_eq!(ok(&mut app, "lock_on", json!({"off": true})), "ok: lock-on released");
    assert_eq!(app.world().get::<Intent>(hero).unwrap().target, None);
    let too_far = {
        let mut app = crate::common::app();
        hero_alone(&mut app);
        fails(&mut app, "lock_on", json!({}))
    };
    assert_eq!(too_far, "no enemy within 20m to lock on to; pass `target`");
}

fn hero_alone(app: &mut App) {
    let world = app.world_mut();
    hero(world, "hero", Vec3::ZERO, N);
    fighter(world, "Dummy", "dummy", "monsters", Vec3::new(30.0, 0.0, 0.0), S);
}

#[test]
fn the_block_tool_holds_and_waits_for_a_busy_fighter() {
    let mut app = app();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    ok(&mut app, "heavy_attack", json!({}));
    step(&mut app, 30);
    // Busy for longer than a press could wait; a held block simply rises later.
    assert_eq!(ok(&mut app, "block", json!({})), "ok: holding block; it rises in 0.65s");
    step(&mut app, 38);
    assert_eq!(action(&app, hero), "heavy");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "idle", "the heavy attack ends this tick");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "block", "0.65 s after the call");
    assert_eq!(ok(&mut app, "block", json!({})), "ok: already blocking");
    assert_eq!(ok(&mut app, "block", json!({"on": false})), "ok: block lowered");
    step(&mut app, 30);
    assert_eq!(action(&app, hero), "idle", "lowered, the block runs out");
}

#[test]
fn describe_lines_read_like_a_status_bar() {
    let mut app = app();
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let h = id(&app, hero);
    ok(&mut app, "dodge", json!({"dir": "W"}));
    step(&mut app, 3);
    app.world_mut().get_mut::<Intent>(hero).unwrap().press("light");
    step(&mut app, 1);
    let seen = observe(app.world(), &ObserveOptions::default());
    let you = seen.lines().nth(1).unwrap();
    assert!(you.starts_with(&format!("YOU  {h} Hero  hp 100/100  stam 75/100  poise 30/30  roll roll 0.33s i-frames  queued light 0.13s  flask 2/2")), "{you}");
    ok(&mut app, "god", json!({}));
    let seen = observe(app.world(), &ObserveOptions::default());
    assert!(seen.lines().nth(1).unwrap().contains("god mode"), "{seen}");
    let mut intent = app.world_mut().get_mut::<Intent>(hero).unwrap();
    intent.orders.push_back(Order::Attack { target: ai_core::AiId(99), input: "light".into(), repeat: false });
    step(&mut app, 1);
    assert!(app.world().get::<Intent>(hero).unwrap().orders.is_empty(), "an order on a missing target ends");
}

#[test]
fn every_tool_documents_itself() {
    let app = app();
    let tools = app.world().resource::<ai_core::AiTools>();
    for name in ["light_attack", "heavy_attack", "block", "parry", "dodge", "use_technique", "use_item", "lock_on", "attack", "god", "heal", "stagger"] {
        let tool = tools.get(name).unwrap_or_else(|| panic!("no tool {name}"));
        assert!(!tool.spec.description.is_empty(), "{name} has no description");
    }
    assert_eq!(tools.get("dodge").unwrap().spec.signature, "dodge(away_from?: integer|string, dir?: string)");
    assert_eq!(tools.get("god").unwrap().spec.scope, ai_core::Scope::Dev);
    assert_eq!(tools.get("block").unwrap().spec.scope, ai_core::Scope::Player);
}
