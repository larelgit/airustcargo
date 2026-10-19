//! The `animation` tool and the `anim` status, headless: nothing is drawn, so the tool says what
//! the bridge would choose for a set it is told to preview. (The drawn bodies are tested in
//! `crates/shards/tests/action_render`.)

use shards::serde_json::json;

use crate::common::*;

#[test]
fn a_set_is_previewed_on_a_body_that_has_no_animated() {
    let mut app = sandbox::test_app(42);
    let said = text(&ok(&mut app, "animation", json!({"id": PLAYER, "animset": "humanoid"}))).to_owned();
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(
        lines[0],
        "e5 · animset humanoid (named: e5 has no Animated; nothing is drawn headless) · what the bridge would show, the pose is not applied"
    );
    assert!(lines[1].starts_with("  body: idle looping at x1.00 (idle)"), "{said}");
    assert!(said.contains("  layers: upper: none"), "{said}");
    assert!(said.contains("  carry (moveset player): sword at hand_r"), "{said}");
    assert!(said.contains("  props: sword at hand_r"), "{said}");

    // The warden carries nothing: its moveset has no `carry` entry but the empty `*`.
    let warden = text(&ok(&mut app, "animation", json!({"id": BOSS, "animset": "humanoid"}))).to_owned();
    assert!(warden.contains("carry (moveset warden): nothing"), "{warden}");
    assert!(!warden.contains("props:"), "{warden}");
}

#[test]
fn a_drink_puts_the_sword_on_the_hip_while_it_lasts() {
    let mut app = sandbox::test_app(42);
    assert_eq!(text(&ok(&mut app, "use_item", json!({"item": "flask"}))), "ok: drink");
    step(&mut app, 30);
    let said = text(&ok(&mut app, "animation", json!({"id": PLAYER, "animset": "humanoid"}))).to_owned();
    assert!(said.contains("  layers: upper: drink (wanted)"), "{said}");
    assert!(said.contains("  props: sword at hip (rules of drink at 0.50/1.30 s)"), "{said}");
    assert!(said.contains("body: idle looping"), "the drink is on the upper body only: {said}");
    // Before 0.1 s and after 0.9 s it is in the hand.
    step(&mut app, 40);
    let said = text(&ok(&mut app, "animation", json!({"id": PLAYER, "animset": "humanoid"}))).to_owned();
    assert!(said.contains("  props: sword at hand_r (rules of drink at 1.17/1.30 s)"), "{said}");
}

#[test]
fn the_tool_says_how_to_fix_a_bad_call() {
    let mut app = sandbox::test_app(42);
    assert_eq!(
        fails(&mut app, "animation", json!({"id": PLAYER})),
        "e5 is not animated: a game gives `Animated` to the characters it dresses in a window or with --offscreen; \
         pass animset=<id> to see what a set would choose"
    );
    assert_eq!(
        fails(&mut app, "animation", json!({"id": PLAYER, "animset": "humanoyd"})),
        "animset `humanoyd` is not in content; `content_list kind=animset` lists them"
    );
    assert_eq!(
        fails(&mut app, "animation", json!({"animset": "humanoid"})),
        "animset=<id> previews a body: name it with id=<body>"
    );
    assert_eq!(
        text(&ok(&mut app, "animation", json!({}))),
        "no animated body: a game gives `Animated` to the characters it dresses in a window or with --offscreen; \
         pass id=<body> animset=<id> to see what a set would choose"
    );
}

#[test]
fn the_status_counts_what_the_bridge_dresses_and_is_zero_headless() {
    let app = sandbox::test_app(42);
    let anim = shards::ai_core::status::status(app.world())["anim"].clone();
    assert_eq!(
        anim,
        json!({"animated": 0, "bound": 0, "props": 0, "plays": 0, "overlays_started": 0, "feet_solved": 0, "ms": 0.0})
    );
}

#[test]
fn the_prop_is_content_and_the_sandbox_has_a_menu_entry() {
    let mut app = sandbox::test_app(42);
    let sword = ok(&mut app, "content_get", json!({"kind": "prop", "id": "sword"})).result;
    assert_eq!(sword["def"]["size"], json!([0.05, 0.9, 0.02]));
    assert_eq!(sword["file"], "content/props/sandbox.prop.ron");
    let menu = text(&ok(&mut app, "dev_menu", json!({}))).to_owned();
    assert!(menu.contains("animation"), "{menu}");
}
