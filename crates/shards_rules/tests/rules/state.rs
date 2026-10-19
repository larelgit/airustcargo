//! The rules' state: a new world, the main menu, a region trip, `rules_set` and its refusals,
//! content edits, the session count.

use serde_json::json;
use shards::ai_core::{configure_session, AiConditions, AiReplay, JournalKind, Scope, SessionChange};
use shards::prelude::*;
use shards::rules::tools::FORCE_IS_DEV;
use shards::rules::{replay_check, ActiveRules, RulesGuards, SessionFilter};

use crate::*;

const HARSH: &str = "player: incoming damage ×1.5, charge flask 2; brute: health ×1.5; attack tokens +1 (at least 1); attack rest ×0.5";

#[test]
fn a_new_world_plays_start_and_the_main_menu_ends_it() {
    let folder = Folder::new("start");
    let mut app = game(&folder);
    assert_eq!(rules(&app), playing("plain", 1, 0));
    assert_eq!(lines(&app, JournalKind::Event, "rules:"), ["rules: a new world plays `plain`"]);
    assert_eq!(status(&app), json!("plain · session 0 · generation 1"));
    assert!(ok(&mut app, "rules_set", json!({"mode": "harsh"})).starts_with("rules: plain → harsh"));

    go(&mut app, EngineState::MainMenu);
    assert_eq!(rules(&app), None, "alone, the main menu removes the rules");
    assert_eq!(status(&app), json!("none"));
    go(&mut app, EngineState::InGame);
    assert_eq!(rules(&app), playing("plain", 1, 0), "the next world starts at `start`, never at the last world's mode");
}

#[test]
fn a_host_sets_the_rules_neutral_at_the_main_menu_and_a_client_starts_none() {
    let folder = Folder::new("host");
    let mut app = game(&folder);
    app.world_mut().insert_resource(NetRole::Host);
    go(&mut app, EngineState::MainMenu);
    assert_eq!(rules(&app), playing("", 1, 0), "a removal does not travel to clients: neutral");
    assert_eq!(replay_check(app.world()), None, "neutral hashes as no rules");
    assert_eq!(status(&app), json!("none"));
    go(&mut app, EngineState::InGame);
    assert_eq!(rules(&app), playing("plain", 2, 0), "a neutral resource is overwritten, its generation moved on");

    // A client takes the host's rules; it starts none of its own.
    go(&mut app, EngineState::MainMenu);
    app.world_mut().remove_resource::<ActiveRules>();
    app.world_mut().insert_resource(NetRole::Client);
    go(&mut app, EngineState::InGame);
    assert_eq!(rules(&app), None);
}

#[test]
fn a_region_trip_keeps_the_rules() {
    let folder = Folder::new("trip");
    let mut app = game(&folder);
    ok(&mut app, "rules_set", json!({"mode": "harsh"}));
    go(&mut app, EngineState::RegionLoading);
    assert_eq!(state(&app), EngineState::InGame, "the trip came back");
    assert_eq!(rules(&app), playing("harsh", 2, 0), "the world keeps its mode across regions");
    assert_eq!(lines(&app, JournalKind::Event, "a new world plays").len(), 1, "a trip is no new world");
}

fn busy(_: &World) -> Option<String> {
    Some("a test holds the rules".to_owned())
}

#[test]
fn refusals_record_nothing_and_a_same_mode_call_changes_nothing() {
    let folder = Folder::new("refusals");
    let mut app = game(&folder);
    let recorded = |app: &App| app.world().resource::<AiReplay>().recorded_events();
    let before = recorded(&app);
    let modes = "gentle, plain, harsh";
    let neither = format!("say `mode` (one of {modes}) or `cycle` (1: the next in `modes`, -1: the one before), not both and not neither");
    assert_eq!(refused(&mut app, "rules_set", json!({})), neither);
    assert_eq!(refused(&mut app, "rules_set", json!({"mode": "harsh", "cycle": 1})), neither);
    assert_eq!(refused(&mut app, "rules_set", json!({"cycle": 2})), "`cycle` is 1 (the next rule set in `modes`) or -1 (the one before), not 2");
    assert_eq!(refused(&mut app, "rules_set", json!({"cycle": 0})), "`cycle` is 1 (the next rule set in `modes`) or -1 (the one before), not 0");
    assert_eq!(
        refused(&mut app, "rules_set", json!({"mode": "gentel"})),
        format!("unknown rule set `gentel` (did you mean `gentle`?); this game's rule sets: {modes}")
    );
    configure_session(app.world_mut(), "player", &SessionChange { scope: Some(Scope::Player), ..default() }).unwrap();
    let forced = shards::ai_core::call_tool(app.world_mut(), "player", "rules_set", json!({"mode": "harsh", "force": true}));
    assert!(!forced.ok);
    assert_eq!(text(&forced), FORCE_IS_DEV);

    app.world_mut().resource_mut::<RulesGuards>().0.push(("busy", busy));
    assert_eq!(
        refused(&mut app, "rules_set", json!({"mode": "harsh"})),
        "the rules change only out of combat: a test holds the rules [busy]. Wait with `step --until named:rules_changeable`, or pass \
         `force: true` from a dev session"
    );
    let changeable = app.world().resource::<AiConditions>().0["rules_changeable"];
    assert!(!changeable(app.world()), "a guard holds");
    let view = testing::ok(&mut app, "rules", json!({})).result;
    assert_eq!(view["change"], "refused now: a test holds the rules [busy]");
    assert_eq!(recorded(&app), before, "a refusal records nothing");
    assert_eq!(rules(&app), playing("plain", 1, 0), "and changes nothing");

    let answer = ok(&mut app, "rules_set", json!({"mode": "harsh", "force": true}));
    assert_eq!(answer, format!("rules: plain → harsh at tick 0: bodies are brought to it at the start of the next tick; {HARSH}"));
    assert_eq!(recorded(&app), before + 1, "a dev session forces it, and it is recorded");
    app.world_mut().resource_mut::<RulesGuards>().0.clear();
    assert!(changeable(app.world()));
    assert_eq!(ok(&mut app, "rules_set", json!({"mode": "harsh"})), "rules: `harsh` already; nothing changed");
    assert_eq!(rules(&app), playing("harsh", 2, 0), "the same mode changes nothing");
}

#[test]
fn cycle_wraps_both_ways() {
    let folder = Folder::new("cycle");
    let mut app = game(&folder);
    assert_eq!(
        ok(&mut app, "rules_set", json!({"cycle": 1})),
        format!("rules: plain → harsh at tick 0: bodies are brought to it at the start of the next tick; {HARSH}")
    );
    step(&mut app, 1);
    assert!(ok(&mut app, "rules_set", json!({"cycle": 1})).starts_with("rules: harsh → gentle at tick 1: "), "past the last: the first");
    assert!(ok(&mut app, "rules_set", json!({"cycle": -1})).starts_with("rules: gentle → harsh at tick 1: "), "before the first: the last");
    assert!(ok(&mut app, "rules_set", json!({"cycle": -1})).starts_with("rules: harsh → plain at tick 1: "));
    assert_eq!(rules(&app).unwrap().generation, 5);
    assert_eq!(
        lines(&app, JournalKind::Event, " (call)"),
        ["rules: plain → harsh (call)", "rules: harsh → gentle (call)", "rules: gentle → harsh (call)", "rules: harsh → plain (call)"]
    );
}

/// `content_get`'s definition of `kind/id`.
fn def(app: &mut App, kind: &str, id: &str) -> serde_json::Value {
    testing::ok(app, "content_get", json!({"kind": kind, "id": id})).result["def"].clone()
}

#[test]
fn a_content_edit_adds_to_the_generation_and_a_vanished_mode_falls_back() {
    const ANSWER: &str = "rules: content changed; bodies are brought to it at the next tick";
    let folder = Folder::new("content");
    let mut app = game(&folder);
    let gentle = def(&mut app, "rule_set", "gentle");
    let mut softer = gentle.clone();
    softer["summary"] = json!("A third of the damage");
    let answer = ok(&mut app, "content_apply", json!({"changes": [{"kind": "rule_set", "id": "gentle", "def": softer, "before": gentle}]}));
    assert!(answer.contains(ANSWER), "{answer}");
    assert_eq!(rules(&app), playing("plain", 2, 0), "every body is brought to the new numbers at the next tick");

    ok(&mut app, "rules_set", json!({"mode": "harsh"}));
    let rules_def = def(&mut app, "rules", "default");
    let mut fewer = rules_def.clone();
    fewer["modes"] = json!(["gentle", "plain"]);
    let harsh = def(&mut app, "rule_set", "harsh");
    let changes = json!([
        {"kind": "rules", "id": "default", "def": fewer, "before": rules_def},
        {"kind": "rule_set", "id": "harsh", "before": harsh},
    ]);
    let answer = ok(&mut app, "content_apply", json!({"changes": changes}));
    assert!(answer.contains(ANSWER), "{answer}");
    assert_eq!(rules(&app), playing("plain", 4, 0), "back to `start`");
    assert_eq!(lines(&app, JournalKind::Warn, "rules:"), ["rules: the rule set `harsh` is gone from content: playing `plain`"]);
    assert_eq!(lines(&app, JournalKind::Event, "→ plain"), Vec::<String>::new(), "the warning says it; no `(call)` line");
}

#[test]
fn content_without_rules_starts_none_and_rules_that_appear_start_them() {
    let folder = Folder::empty("none");
    let mut app = game(&folder);
    assert_eq!(rules(&app), None, "a game without `rules/default` has no rules");
    assert_eq!(status(&app), json!("none"));
    let refusal = refused(&mut app, "rules", json!({}));
    assert!(refusal.starts_with("this game has no rules: add `content/**/*.rules.ron`"), "{refusal}");
    step(&mut app, 3);
    assert_eq!(rules(&app), None, "and none appear by themselves");
    let rules_def: serde_json::Value = json!({"modes": ["plain"], "start": "plain"});
    let plain = json!({"name": "Plain"});
    let changes = json!([{"kind": "rules", "id": "default", "def": rules_def}, {"kind": "rule_set", "id": "plain", "def": plain}]);
    let answer = ok(&mut app, "content_apply", json!({"changes": changes}));
    assert!(answer.contains("rules: content changed"), "{answer}");
    assert_eq!(rules(&app), playing("plain", 1, 0), "content that has rules now starts them in a world");
}

fn leave_out_player_one(world: &World, e: Entity) -> bool {
    world.get::<PlayerId>(e).is_some_and(|p| p.0 != 1)
}

#[test]
fn the_session_counts_the_players_the_filter_seats() {
    let folder = Folder::new("session");
    let mut app = game(&folder);
    body(&mut app, 0, 40.0);
    body(&mut app, 1, 40.0);
    // A corpse keeps its PlayerId beside the player's new body: one player.
    let corpse = body(&mut app, 1, 40.0);
    app.world_mut().get_mut::<Health>(corpse).unwrap().current = 0.0;
    step(&mut app, 1);
    assert_eq!(rules(&app), playing("plain", 1, 2));
    assert_eq!(lines(&app, JournalKind::Event, "players:"), ["rules: 2 players: brute health ×1.5"]);
    app.world_mut().insert_resource(SessionFilter(leave_out_player_one));
    step(&mut app, 1);
    assert_eq!(rules(&app), playing("plain", 1, 1), "a body the filter leaves out is not counted");
    assert_eq!(lines(&app, JournalKind::Event, "player:"), ["rules: 1 player: brute health ×1"]);
    assert_eq!(status(&app), json!("plain · session 1 · generation 1"));
    step(&mut app, 3);
    assert_eq!(lines(&app, JournalKind::Event, "player").len(), 2, "an unchanged count journals nothing");
}
