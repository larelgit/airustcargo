//! The content check: what a broken table is told, file and all.

use serde_json::json;
use shards::hitstop::{HitstopDef, MAX_HOLD};

use crate::*;

fn errors(app: &mut App) -> String {
    let out = shards::testing::call(app, "content_check", json!({}));
    text(&out)
}

#[test]
fn the_test_table_is_clean_and_not_simulation_content() {
    let assets = Folder::new("clean");
    let mut app = game(&assets);
    let said = errors(&mut app);
    assert!(said.starts_with("ok: "), "{said}");
    let feel: serde_json::Value = serde_json::from_str(&ok(&mut app, "content_get", json!({"kind": "hitstop", "id": "feel"}))).unwrap();
    assert_eq!(feel["file"].as_str(), Some("content/feel/test.hitstop.ron"));
    assert_eq!(feel["def"]["rules"].as_array().map(Vec::len), Some(8));
    // Only the picture and the real-time clock read it: no replay's content hash changes with it.
    const { assert!(!<HitstopDef as shards::content::ContentDef>::SIMULATION) };
}

#[test]
fn a_broken_table_says_what_and_where() {
    let assets = Folder::new("broken");
    assets.put(
        "content/feel/test.hitstop.ron",
        r#"{"feel": (
            max_seconds: 0.1,
            cooldown: -1.0,
            rules: [
                (name: "hit", when: (event: Damaged), seconds: 0.06),
                (name: "break", when: (event: Damaged, weight_at_least: Break), seconds: 0.5),
                (name: "block", when: (event: Blocked, reaction_at_least: 2), seconds: 0.06),
                (name: "parry", when: (event: Parried, weight_at_least: Heavy), seconds: 0.06),
                (name: "stagger", when: (event: Damaged, cause: PoiseBroken), seconds: 0.06),
                (when: (action: ""), seconds: 0.06),
            ],
        )}"#,
    );
    assets.put("content/feel/more.hitstop.ron", r#"{"more": (rules: [])}"#);
    let mut app = game(&assets);
    let said = errors(&mut app);
    for wanted in [
        "a game has one hit stop table, and this one has more: more; keep one",
        "cooldown -1: a number from 0 to 5 seconds",
        "rules[1] (break): never matches, rule 1 (hit) before it matches everything it does; put the narrower rule first",
        "rules[1] (break): seconds 0.5 is longer than max_seconds 0.1 and would be cut",
        "rules[2] (block): reaction_at_least is read from a hit that landed (event: Damaged), not from blocked",
        "rules[3] (parry): parried carries no weight, so a weight_at_least would never match",
        "rules[4] (stagger): cause is read from a stagger (event: Staggered), not from damaged",
        "rules[5] (rule 6 any): an empty action",
    ] {
        assert!(said.contains(wanted), "`{wanted}` in\n{said}");
    }
    assert!(said.contains("test.hitstop.ron"), "{said}");
}

#[test]
fn a_typo_in_a_name_is_a_parse_error_with_the_file() {
    let assets = Folder::new("typo");
    assets.put("content/feel/test.hitstop.ron", r#"{"feel": (rules: [(when: (event: Damage), seconds: 0.06)])}"#);
    let mut app = game(&assets);
    let said = errors(&mut app);
    assert!(said.contains("test.hitstop.ron") && said.contains("Damage"), "{said}");
    // And a ceiling above the engine's is refused, not obeyed.
    let assets = Folder::new("ceiling");
    assets.put("content/feel/test.hitstop.ron", r#"{"feel": (max_seconds: 3.0, rules: [(when: (event: Damaged), seconds: 2.0)])}"#);
    let mut app = game(&assets);
    let said = errors(&mut app);
    assert!(said.contains(&format!("at most {MAX_HOLD}")), "{said}");
}
