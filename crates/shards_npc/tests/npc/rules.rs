//! The rules and NPCs (C3.4a): the start check an NPC's director, planner and chains share reads what
//! the rules make its attacks cost, so an NPC asks for no attack its stamina cannot pay for and the
//! machine never has to refuse one of its presses. A grunt of class `brute` whose jab costs 10 stamina,
//! with 15 and no regeneration: `plain` lets it jab once; `dear` doubles a brute's jab, and it never does.

use bevy::prelude::*;
use serde_json::json;
use shards_action::Stamina;
use shards_npc::Npc;
use shards_rules::{ActiveRules, RuleClass};

use crate::common::*;

const RULES: &str = r#"{"modes": ["plain", "dear"], "start": "plain", "classes": ["brute"]}"#;

/// Puts the rules in the app and plays `mode`.
fn play(app: &mut App, mode: &str) {
    let sets = [
        ("plain", json!({"name": "Plain"})),
        ("dear", json!({"name": "Dear", "classes": {"brute": {"stamina_cost": [{"tags": ["dear_jab"], "factor": 2.0}]}}})),
    ];
    let mut changes = vec![json!({"kind": "rules", "id": "default", "def": serde_json::from_str::<serde_json::Value>(RULES).unwrap()})];
    changes.extend(sets.iter().map(|(id, def)| json!({"kind": "rule_set", "id": id, "def": def})));
    ok(app, "content_apply", json!({"changes": changes}));
    app.world_mut().resource_mut::<ActiveRules>().mode = mode.to_owned();
}

/// The grunt 2 m from a player (in god mode), under `mode`, after `ticks`.
fn bout(mode: &str, ticks: u64) -> (App, Entity) {
    let mut app = app();
    let _hero = player(app.world_mut(), 0, Vec3::ZERO);
    let at = Vec3::new(0.0, 0.0, -2.0);
    let npc = Npc::new("grunt", "grunt").with_plan("jabber").with_director("default");
    let grunt = crate::common::npc(app.world_mut(), "spender", npc, at, -N);
    app.world_mut().entity_mut(grunt).insert((RuleClass("brute".to_owned()), Stamina::new(15.0, 0.0, 0.0)));
    play(&mut app, mode);
    step(&mut app, ticks);
    (app, grunt)
}

fn stamina(app: &App, e: Entity) -> f32 {
    app.world().get::<Stamina>(e).unwrap().current
}

#[test]
fn an_npc_asks_for_no_attack_the_rules_make_too_dear() {
    // Plain: the jab costs 10, the grunt jabs once and keeps 5.
    let (app, grunt) = bout("plain", 240);
    assert_eq!(stamina(&app, grunt), 5.0, "{:#?}", journal(&app));
    assert!(lines(&app, "rejected").is_empty(), "{:#?}", journal(&app));

    // Dear: a brute's jab costs 20; with 15 the grunt never presses it, so nothing is refused either.
    let (app, grunt) = bout("dear", 240);
    assert_eq!(stamina(&app, grunt), 15.0, "{:#?}", journal(&app));
    assert!(!attacking(&app, grunt));
    assert!(lines(&app, "rejected").is_empty(), "the planner pressed what the machine refused: {:#?}", journal(&app));
}
