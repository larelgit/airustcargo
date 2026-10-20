//! What an action costs under the rules (C3.4a): `cost.stamina` times the class's `stamina_cost` rules
//! (`shards_action::stamina_cost`), on a start, on a charge's turn, and in the start check every reader
//! shares (the machine, an agent's order, a tool's view). The rules content is applied to the running app
//! with `content_apply`: `plain` changes nothing; `cheap` halves a player's roll and gives the class
//! `brute` a roll at a fifth; `dear` makes a player's roll and charged heavy cost 1.2 times as much and
//! doubles its first light swing. Numbers against the hero's kit (`hero.action.ron`, `timeline.action.ron`).

use bevy::prelude::*;
use serde_json::json;
use shards_action::prelude::*;
use shards_action::{can_start, Blocker, FighterView};
use shards_core::PlayerId;
use shards_rules::{ActiveRules, RuleClass};

use crate::common::*;

const RULES: &str = r#"{"modes": ["plain", "cheap", "dear"], "start": "plain", "classes": ["brute"]}"#;
const DT: f32 = 1.0 / 60.0;

/// Puts the rules in the app and plays `mode`.
fn play(app: &mut App, mode: &str) {
    let sets = [
        ("plain", json!({"name": "Plain"})),
        (
            "cheap",
            json!({"name": "Cheap", "classes": {
                "player": {"stamina_cost": [{"tags": ["roll"], "factor": 0.5}]},
                "brute": {"stamina_cost": [{"tags": ["roll"], "factor": 0.2}]},
            }}),
        ),
        (
            "dear",
            json!({"name": "Dear", "classes": {"player": {"stamina_cost": [
                {"tags": ["roll", "t_heavy_full"], "factor": 1.2},
                {"tags": ["light_1"], "factor": 2.0},
            ]}}}),
        ),
    ];
    let mut changes = vec![json!({"kind": "rules", "id": "default", "def": serde_json::from_str::<serde_json::Value>(RULES).unwrap()})];
    changes.extend(sets.iter().map(|(id, def)| json!({"kind": "rule_set", "id": id, "def": def})));
    ok(app, "content_apply", json!({"changes": changes}));
    app.world_mut().resource_mut::<ActiveRules>().mode = mode.to_owned();
}

/// The hero with `moveset` at the origin facing north, a player's body (the class `player`), in an app
/// whose rules play `mode` (`None`: no rules content at all).
fn arena(mode: Option<&str>, moveset: &str) -> (App, Entity) {
    let mut app = app();
    let hero = hero(app.world_mut(), moveset, Vec3::ZERO, N);
    app.world_mut().entity_mut(hero).insert(PlayerId(0));
    if let Some(mode) = mode {
        play(&mut app, mode);
    }
    (app, hero)
}

fn set_stamina(app: &mut App, e: Entity, value: f32) {
    app.world_mut().get_mut::<Stamina>(e).unwrap().current = value;
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

/// The stamina the start check asks of `input` now, as a tool sees the hero (`FighterView::of`); `None`
/// when it would start.
fn needs(app: &App, e: Entity, input: &str) -> Option<f32> {
    let view = FighterView::of(app.world(), e).unwrap();
    match can_start(&view, input, DT) {
        Ok(_) => None,
        Err(Blocker::Stamina { need, .. }) => Some(need),
        Err(other) => panic!("blocked by something else: {other:?}"),
    }
}

#[test]
fn a_rule_scales_what_starting_an_action_costs() {
    // (mode, the roll's 25, the first light swing's 10)
    for (mode, roll, light) in [(None, 25.0, 10.0), (Some("plain"), 25.0, 10.0), (Some("cheap"), 12.5, 10.0), (Some("dear"), 30.0, 20.0)] {
        for (input, want, cost) in [("dodge", "roll", roll), ("light", "light_1", light)] {
            let (mut app, hero) = arena(mode, "hero");
            press(&mut app, hero, input);
            step(&mut app, 1);
            assert_eq!(action(&app, hero), want, "{mode:?}");
            assert!(close(stamina(&app, hero), 100.0 - cost), "{mode:?} {want}: {} left, {cost} paid", stamina(&app, hero));
        }
    }
}

#[test]
fn the_class_decides_which_rules_apply() {
    // A `RuleClass` names the class, even on a player's body; without one a player's body is `player`;
    // a body with neither has no class and pays the action's own cost.
    type Setup = fn(&mut App, Entity);
    let cases: [(&str, Setup, f32); 3] = [
        ("brute", |app, e| _ = app.world_mut().entity_mut(e).insert(RuleClass("brute".to_owned())), 5.0),
        ("player", |_, _| {}, 12.5),
        ("none", |app, e| _ = app.world_mut().entity_mut(e).remove::<PlayerId>(), 25.0),
    ];
    for (name, setup, cost) in cases {
        let (mut app, hero) = arena(Some("cheap"), "hero");
        setup(&mut app, hero);
        press(&mut app, hero, "dodge");
        step(&mut app, 1);
        assert_eq!(action(&app, hero), "roll", "{name}");
        assert!(close(stamina(&app, hero), 100.0 - cost), "{name}: {} left, {cost} paid", stamina(&app, hero));
    }
}

#[test]
fn the_start_waits_for_the_cost_the_rules_make() {
    // Dear: 28 is enough for the roll's 25, not for the rules' 30. The press waits, and a tool's view
    // says what it waits for.
    let (mut app, hero) = arena(Some("dear"), "hero");
    set_stamina(&mut app, hero, 28.0);
    assert!(needs(&app, hero, "dodge").is_some_and(|need| close(need, 30.0)), "{:?}", needs(&app, hero, "dodge"));
    press(&mut app, hero, "dodge");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "idle");

    // Plain: the same 28 rolls.
    let (mut app, hero) = arena(Some("plain"), "hero");
    set_stamina(&mut app, hero, 28.0);
    assert_eq!(needs(&app, hero, "dodge"), None);

    // Cheap: 13 is short of 25 but covers 12.5; the roll starts and leaves half a point.
    let (mut app, hero) = arena(Some("cheap"), "hero");
    set_stamina(&mut app, hero, 13.0);
    assert_eq!(needs(&app, hero, "dodge"), None);
    press(&mut app, hero, "dodge");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "roll");
    assert!(close(stamina(&app, hero), 0.5), "{}", stamina(&app, hero));
}

#[test]
fn a_charge_turns_on_the_difference_of_the_costs_the_rules_make() {
    // `t_heavy` (20) turns into `t_heavy_full` (50, and 60 when dear) after 0.2 s held: the turn costs the
    // difference, 30 plain and 40 dear. From 55 the start leaves 35.
    let (mut app, hero) = arena(Some("plain"), "t_fighter");
    set_stamina(&mut app, hero, 55.0);
    press(&mut app, hero, "heavy");
    hold(&mut app, hero, "heavy", true);
    step(&mut app, 14);
    assert_eq!(action(&app, hero), "t_heavy_full");
    assert!(close(stamina(&app, hero), 5.0), "{}", stamina(&app, hero));

    let (mut app, hero) = arena(Some("dear"), "t_fighter");
    set_stamina(&mut app, hero, 55.0);
    press(&mut app, hero, "heavy");
    hold(&mut app, hero, "heavy", true);
    step(&mut app, 14);
    assert_eq!(action(&app, hero), "t_heavy", "35 left, 40 needed: it waits");
    assert!(close(stamina(&app, hero), 35.0), "a refused turn takes nothing: {}", stamina(&app, hero));
    set_stamina(&mut app, hero, 40.0);
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "t_heavy_full");
    assert!(close(stamina(&app, hero), 0.0), "{}", stamina(&app, hero));
}

#[test]
fn an_agents_order_presses_only_what_the_rules_let_start() {
    // Dear: the first light swing costs 20. From 10 the order waits for the stamina instead of pressing a
    // swing the machine would refuse, then swings.
    let (mut app, hero) = arena(Some("dear"), "hero");
    let dummy = fighter(app.world_mut(), "Dummy", "dummy", "monsters", Vec3::new(0.0, 0.0, -1.5), S);
    set_stamina(&mut app, hero, 10.0);
    let d = id(&app, dummy);
    ok(&mut app, "attack", json!({"target": d}));
    step(&mut app, 90);
    assert!(lines(&app, "rejected").is_empty(), "orders press only what starts: {:#?}", journal(&app));
    assert!(hp(&app, dummy) < 100.0, "the swing landed once the stamina was there: {:#?}", journal(&app));
}
