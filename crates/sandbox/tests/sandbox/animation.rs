//! The `animation` tool and the `anim` status, headless: nothing is drawn, so the tool says what
//! the bridge would choose for a set it is told to preview. (The drawn bodies are tested in
//! `crates/shards/tests/action_render`.)

use shards::prelude::*;
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
fn a_dead_body_with_nothing_running_says_its_clip_plays_once() {
    let mut app = sandbox::test_app(42);
    let boss = app.world().resource::<shards::ai_core::AiIndex>().get(shards::ai_core::AiId::parse(BOSS).unwrap()).unwrap();
    shards::action::kill(app.world_mut(), boss, None, None);
    step(&mut app, 600);
    let said = text(&ok(&mut app, "animation", json!({"id": BOSS, "animset": "humanoid"}))).to_owned();
    let body = said.lines().nth(1).unwrap_or_default();
    assert!(body.contains(" played once at x") && (body.ends_with("(dead)") || body.contains("(dead) ·")), "{said}");
    assert!(!body.contains("looping"), "{said}");
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
fn a_swimmer_has_its_sword_on_the_back_and_a_stroke_keeps_it_in_the_hand() {
    let mut app = sandbox::test_app(42);
    ok(&mut app, "set", json!({"id": PLAYER, "component": "Fighter", "path": "moveset", "value": "mover"}));
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 61, "z": 0, "y": 3.2}));
    step(&mut app, 40);
    let said = text(&ok(&mut app, "animation", json!({"id": PLAYER, "animset": "humanoid"}))).to_owned();
    for part in [
        "  mode: water (as the controller says) · @mode:water applies",
        "layers off: upper",
        "drop 0.15 m over 0.22 s when an action runs",
    ] {
        assert!(said.contains(part), "{part}: {said}");
    }
    assert!(said.contains("  props: sword at back"), "{said}");
    // A stroke is a swing afloat: the sword is in the hand for it, and the model goes down.
    let player = shards::ai_core::AiId::parse(PLAYER).unwrap();
    let entity = app.world().resource::<shards::ai_core::AiIndex>().get(player).unwrap();
    assert!(shards::action::force_action(app.world_mut(), entity, "spar_stroke"));
    step(&mut app, 10);
    let said = text(&ok(&mut app, "animation", json!({"id": PLAYER, "animset": "humanoid"}))).to_owned();
    assert!(said.contains("@mode:water steps aside (an action runs, yield_after 1.2 s)"), "{said}");
    assert!(said.contains("  props: sword at hand_r"), "{said}");
    // On the ground there is nothing of it.
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 0, "z": 0, "y": 0}));
    step(&mut app, 120);
    let said = text(&ok(&mut app, "animation", json!({"id": PLAYER, "animset": "humanoid"}))).to_owned();
    assert!(said.contains("  mode: ground (as the controller says) · no @mode rules"), "{said}");
    assert!(said.contains("  props: sword at hand_r"), "{said}");
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
fn an_event_overlay_is_previewed_and_a_bad_event_is_named() {
    let mut app = sandbox::test_app(42);
    let said = text(&ok(&mut app, "animation", json!({"id": PLAYER, "animset": "humanoid", "event": "damaged"}))).to_owned();
    let lines: Vec<&str> = said.lines().collect();
    assert_eq!(
        lines[0],
        "overlay damaged of humanoid: flinch on upper, in 0.04 s, out 0.15-0.35 s, 60 %; e5 is not drawn (a window or \
         --offscreen draws it), so nothing plays"
    );
    assert!(
        said.contains(
            "  overlays: blocked: flinch on upper, in 0.04 s, out 0.10-0.25 s, 35 % · damaged: flinch on upper, in 0.04 s, \
             out 0.15-0.35 s, 60 %"
        ),
        "{said}"
    );
    assert_eq!(
        fails(&mut app, "animation", json!({"id": PLAYER, "animset": "humanoid", "event": "damage"})),
        "event `damage` is no engine event an overlay answers (did you mean `damaged`?); events: blocked, parried, damaged, \
         guard_broken, staggered, part_broken, landed"
    );
    assert_eq!(
        fails(&mut app, "animation", json!({"id": PLAYER, "animset": "humanoid", "event": "landed"})),
        "animset `humanoid` has no overlay for `landed`; its overlays: blocked, damaged"
    );
    // An overlay plays on one body, named, even with no body drawn; a dead body starts none.
    assert_eq!(
        fails(&mut app, "animation", json!({"event": "damaged"})),
        "event=<name> plays an overlay on one body: name it with id=<body>"
    );
    let boss = app.world().resource::<shards::ai_core::AiIndex>().get(shards::ai_core::AiId::parse(BOSS).unwrap()).unwrap();
    shards::action::kill(app.world_mut(), boss, None, None);
    step(&mut app, 2);
    let said = text(&ok(&mut app, "animation", json!({"id": BOSS, "animset": "humanoid", "event": "damaged"}))).to_owned();
    assert!(said.starts_with("overlay damaged of humanoid: flinch on upper, in 0.04 s, out 0.15-0.35 s, 60 %; e"), "{said}");
    assert!(said.lines().next().unwrap().ends_with(" is dead, so nothing plays"), "{said}");
}

#[test]
fn an_attack_is_seeked_ahead_by_the_overstep_as_the_bridge_seeks_it() {
    let mut app = sandbox::test_app(42);
    assert_eq!(text(&ok(&mut app, "light_attack", json!({}))), "ok: light_1");
    step(&mut app, 6);
    let seeked = |app: &mut App| {
        let said = text(&ok(app, "animation", json!({"id": PLAYER, "animset": "humanoid"}))).to_owned();
        let body = said.lines().nth(1).unwrap_or_default().to_owned();
        let at = body.split_once(" seeked to ").and_then(|(_, r)| r.split_once(" %")).map(|(p, _)| p.trim().parse::<f32>());
        (at.and_then(Result::ok).unwrap_or_else(|| panic!("{said}")), said)
    };
    let (on_tick, _) = seeked(&mut app);
    let half = app.world().resource::<Time<Fixed>>().timestep() / 2;
    app.world_mut().resource_mut::<Time<Fixed>>().accumulate_overstep(half);
    let (ahead, said) = seeked(&mut app);
    assert!(ahead > on_tick, "half a tick of overstep seeks the clip ahead, as the bridge does: {on_tick} % then {said}");
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
