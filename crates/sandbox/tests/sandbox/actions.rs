//! The combat prototype's kit, as the sandbox's content defines it and an agent uses it:
//! the light combo, the heavy attack, the dodge roll, the block and perfect block, the
//! flask; the warden's four attacks and its phases. Tools answer by the game's own rule:
//! `ok`, `ok: queued`, or why not and when.

use shards::action::InputBuffer;
use shards::ai_core::{AgentSessions, Scope};
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::*;

/// The player 3 m south of the warden, facing it; the warden calm, the skeletons too.
fn face_to_face() -> App {
    let mut app = sandbox::test_app(42);
    calm_skeletons(&mut app);
    calm_warden(&mut app);
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 0, "z": -3}));
    ok(&mut app, "lock_on", json!({"target": BOSS}));
    step(&mut app, 2);
    app
}

/// Presses `tool` on the warden's body, as an agent possessing it would.
fn as_warden(app: &mut App, tool: &str) -> String {
    app.world_mut().resource_mut::<AgentSessions>().configure("warden", Scope::Dev, Some(AiId(6)));
    let out = shards::ai_core::call_tool(app.world_mut(), "warden", tool, json!({}));
    assert!(out.ok, "{tool} as the warden: {}", out.result);
    text(&out).to_owned()
}

#[test]
fn a_dodge_without_stamina_is_refused_and_changes_nothing() {
    let mut app = face_to_face();
    ok(&mut app, "set", json!({"id": PLAYER, "component": "Stamina", "path": "current", "value": 10}));
    ok(&mut app, "move", json!({"dir": "S", "meters": 2}));
    // 20 needed, 45 back per second: 14 ticks, more than the 0.15 s a press may wait.
    assert_eq!(fails(&mut app, "dodge", json!({"dir": "W"})), "not enough stamina: 10/20 (ready in 0.2s)");
    let intent = player::<Intent>(&mut app).clone();
    assert!(intent.pressed.is_empty(), "a refused call leaves the intent alone: {intent:?}");
    assert_eq!(intent.orders.len(), 1, "and the orders: {intent:?}");
    step(&mut app, 1);
    assert_eq!(action(&app, PLAYER), "idle");
    assert!(player::<InputBuffer>(&mut app).queued.is_empty());
    assert!(lines(&app, "rejected").is_empty(), "{:?}", events(&app));
}

#[test]
fn a_light_windup_gives_way_to_a_dodge_a_heavy_one_does_not() {
    let mut app = face_to_face();
    assert_eq!(text(&ok(&mut app, "light_attack", json!({}))), "ok: light_1");
    step(&mut app, 3);
    assert_eq!(text(&ok(&mut app, "dodge", json!({"dir": "S"}))), "ok: roll (S)");
    step(&mut app, 1);
    assert_eq!(action(&app, PLAYER), "roll", "the light windup was cut short");

    let mut app = face_to_face();
    assert_eq!(text(&ok(&mut app, "heavy_attack", json!({}))), "ok: heavy");
    step(&mut app, 3);
    let busy = fails(&mut app, "dodge", json!({"dir": "S"}));
    assert_eq!(busy, "busy: windup heavy 0.50s (ready in 0.90s, when a cancel window opens)");
    let hits = |app: &App| lines(app, "e5 -> e6 heavy -55 (hp 545)").len();
    step(&mut app, 40);
    assert_eq!(hits(&app), 1, "the heavy attack lands: {:?}", events(&app));
}

#[test]
fn the_light_combo_is_three_hits_through_cancel_windows() {
    let mut app = face_to_face();
    assert_eq!(
        text(&ok(&mut app, "attack", json!({"target": BOSS, "repeat": true}))),
        "ok: attacking e6 with light until it dies"
    );
    let out = ok(&mut app, "step", json!({"ticks": 120, "until": [{"journal": "e5 -> e6 light_3"}]}));
    assert!(text(&out).contains("stopped: journal:"), "{}", text(&out));
    let hits: Vec<String> =
        lines(&app, "e5 -> e6").into_iter().map(|l| l.split_once(' ').unwrap().1.to_owned()).collect();
    assert_eq!(
        hits,
        ["e5 -> e6 light_1 -20 (hp 580)", "e5 -> e6 light_2 -22 (hp 558)", "e5 -> e6 light_3 -30 (hp 528)"]
    );
    assert_eq!(get::<Poise>(&app, BOSS).current, 140.0 - 12.0 - 14.0 - 24.0);
    // 8 + 8 + 12 stamina, with no time to regenerate in between.
    assert!(get::<Stamina>(&app, PLAYER).current <= 72.0 + 1e-3, "{:?}", get::<Stamina>(&app, PLAYER));
}

#[test]
fn a_raised_guard_blocks_and_one_raised_just_in_time_parries() {
    // The slash lands 36 ticks after it starts (0.60 s); a guard raised long before blocks.
    let mut app = face_to_face();
    as_warden(&mut app, "light_attack");
    step(&mut app, 2);
    assert_eq!(text(&ok(&mut app, "block", json!({}))), "ok: block (holding block)");
    step(&mut app, 40);
    let blocked = lines(&app, "e5 blocked e6 slash");
    assert_eq!(blocked.len(), 1, "{:?}", events(&app));
    assert!(blocked[0].contains("-18 stamina"), "{blocked:?}");
    assert!(get::<Health>(&app, PLAYER).current > 95.0 && get::<Health>(&app, PLAYER).current < 100.0);
    assert!(lines(&app, "parried").is_empty());

    // Raised 6 ticks before the hit, the guard's first 0.15 s take it: a parry. The warden
    // staggers, and nothing reaches the player.
    let mut app = face_to_face();
    as_warden(&mut app, "light_attack");
    step(&mut app, 30);
    ok(&mut app, "block", json!({}));
    step(&mut app, 12);
    assert_eq!(lines(&app, "e5 parried e6 slash").len(), 1, "{:?}", events(&app));
    assert_eq!(lines(&app, "e6 staggered (parried)").len(), 1);
    assert_eq!(action(&app, BOSS), "warden_stagger");
    assert_eq!(get::<Health>(&app, PLAYER).current, 100.0);
    assert_eq!(get::<Stamina>(&app, PLAYER).current, 100.0, "a perfect block costs nothing");
}

#[test]
fn the_roll_evades_with_its_i_frames() {
    // Rolling into the slash just as it lands: the first 0.18 s of the roll are i-frames.
    let mut app = face_to_face();
    as_warden(&mut app, "light_attack");
    step(&mut app, 34);
    ok(&mut app, "dodge", json!({"dir": "N"}));
    step(&mut app, 10);
    assert_eq!(lines(&app, "e5 evaded e6 slash").len(), 1, "{:?}", events(&app));
    assert_eq!(get::<Health>(&app, PLAYER).current, 100.0);
}

#[test]
fn the_flask_heals_three_times() {
    let mut app = face_to_face();
    ok(&mut app, "set", json!({"id": PLAYER, "component": "Health", "path": "current", "value": 40}));
    assert_eq!(text(&ok(&mut app, "use_item", json!({"item": "flask"}))), "ok: drink");
    step(&mut app, 30);
    assert_eq!(get::<Health>(&app, PLAYER).current, 40.0, "it heals at 0.7 s");
    step(&mut app, 30);
    assert_eq!(get::<Health>(&app, PLAYER).current, 85.0);
    assert_eq!(lines(&app, "e5 healed +45 (hp 85)").len(), 1, "{:?}", events(&app));
    assert!(text(&ok(&mut app, "observe", json!({}))).contains("flask 2/3"));
    let refused = fails(&mut app, "light_attack", json!({}));
    assert!(refused.starts_with("busy: recover drink"), "drinking locks the body: {refused}");

    ok(&mut app, "set", json!({"id": PLAYER, "component": "Charges", "path": "0[0].current", "value": 0}));
    step(&mut app, 30);
    assert_eq!(fails(&mut app, "use_item", json!({"item": "flask"})), "no flask charges left (0/3)");
    assert_eq!(fails(&mut app, "use_item", json!({"item": "bomb"})), "no item `bomb`; you have: flask");
}

#[test]
fn an_input_that_runs_out_is_journaled_as_rejected() {
    let mut app = face_to_face();
    ok(&mut app, "set", json!({"id": PLAYER, "component": "Stamina", "path": "current", "value": 0}));
    // Straight into the intent, the way the keyboard writes it: no tool to refuse it.
    {
        let world = app.world_mut();
        let mut q = world.query_filtered::<&mut Intent, With<sandbox::gameplay::Player>>();
        q.single_mut(world).unwrap().press_dir("dodge", Vec3::Z);
    }
    step(&mut app, 6);
    assert!(!player::<InputBuffer>(&mut app).queued.is_empty(), "it waits in the buffer");
    assert!(lines(&app, "rejected").is_empty());
    let out = step(&mut app, 6);
    let rejected: Vec<&String> = out.journal.iter().filter(|l| l.contains("e5 dodge rejected: stamina")).collect();
    assert_eq!(rejected.len(), 1, "{:?}", out.journal);
    assert_eq!(action(&app, PLAYER), "idle");
    assert!(player::<InputBuffer>(&mut app).queued.is_empty());
}

#[test]
fn the_warden_warns_half_a_second_ahead_and_never_does_two_zone_attacks_in_a_row() {
    let mut app = sandbox::test_app(42);
    calm_skeletons(&mut app);
    ok(&mut app, "god", json!({}));
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 0, "z": -2}));
    step(&mut app, 3600);
    let warnings: Vec<String> = lines(&app, "e6 winds up");
    assert!(warnings.len() >= 15, "about one every 3 s: {warnings:#?}");
    let attack = |l: &String| l.split_whitespace().nth(4).unwrap().to_owned();
    let zone = |a: &str| !a.starts_with("slash");
    let kinds: Vec<String> = warnings.iter().map(attack).collect();
    assert!(kinds.windows(2).all(|w| !(zone(&w[0]) && zone(&w[1]))), "{kinds:?}");
    assert!(warnings.iter().all(|l| l.contains("(0.50s)")), "{warnings:#?}");
    for name in ["slash", "slam", "sweep", "shockwave"] {
        assert!(kinds.iter().any(|k| k == name), "never used {name}: {kinds:?}");
    }
    assert!(warnings.iter().any(|l| l.ends_with("⚠ circle 4m")), "{warnings:#?}");
}

#[test]
fn at_half_health_the_warden_turns_enraged() {
    let mut app = sandbox::test_app(42);
    calm_skeletons(&mut app);
    ok(&mut app, "set", json!({"id": BOSS, "component": "Health", "path": "current", "value": 299}));
    step(&mut app, 1);
    assert_eq!(lines(&app, "e6 phase 1->2").len(), 1, "{:?}", events(&app));
    assert_eq!(get::<Fighter>(&app, BOSS).moveset, "warden_enraged");
    // Set back by hand (a scenario resetting the arena): the calm moveset again.
    ok(&mut app, "set", json!({"id": BOSS, "component": "Boss", "path": "phase", "value": 1}));
    ok(&mut app, "heal", json!({"id": BOSS}));
    step(&mut app, 1);
    assert_eq!(get::<Fighter>(&app, BOSS).moveset, "warden");
}

#[test]
fn the_dead_leave_or_lie_and_a_dev_heal_brings_them_back() {
    let mut app = sandbox::test_app(42);
    calm_skeletons(&mut app);
    calm_warden(&mut app);
    for id in ["e6", "e7"] {
        ok(&mut app, "set", json!({"id": id, "component": "Health", "path": "current", "value": 0}));
    }
    step(&mut app, 1);
    assert_eq!(lines(&app, " died").len(), 2, "{:?}", events(&app));
    assert!(!app.world().entity(lookup(&app, "e6")).contains::<LockOnTarget>(), "the dead are no lock-on targets");
    // A skeleton's remains go after 1.5 s; the warden's stay.
    step(&mut app, 90);
    assert!(fails(&mut app, "inspect", json!({"id": "e7"})).contains("e7 no longer exists"));
    assert!(text(&ok(&mut app, "inspect", json!({"id": BOSS}))).contains("dead"));
    assert_eq!(text(&ok(&mut app, "heal", json!({"id": BOSS}))), "healed e6 to 600, revived");
    step(&mut app, 1);
    assert!(app.world().entity(lookup(&app, "e6")).contains::<LockOnTarget>(), "a target again");
}

fn lookup(app: &App, id: &str) -> Entity {
    app.world().resource::<shards::ai_core::AiIndex>().get(AiId::parse(id).unwrap()).unwrap()
}

/// The dev menu's downed entries (B5): `down_me` brings the caller's own body down until it is
/// revived, `heal` revives it, and `down` puts its line (with a bleed-out) in the console for an id.
#[test]
fn the_dev_menu_brings_the_player_down_and_heal_stands_it_up() {
    let mut app = sandbox::test_app(42);
    step(&mut app, 2);
    let list = text(&ok(&mut app, "dev_menu", json!({}))).to_owned();
    let line = |id: &str| list.lines().find(|l| l.trim_start().starts_with(&format!("{id} "))).unwrap_or_else(|| panic!("no `{id}`:\n{list}"));
    assert!(line("down_me").ends_with(&format!("down id={PLAYER}")), "{list}");
    assert!(line("down").ends_with("down id= bleed=30 (finished in the console)"), "{list}");
    // An action answers its first line, then the journal lines it caused.
    let down = text(&ok(&mut app, "dev_menu", json!({"run": "down_me"}))).to_owned();
    assert_eq!(down.lines().next(), Some(format!("{PLAYER} is down").as_str()), "{down}");
    step(&mut app, 1);
    assert!(text(&ok(&mut app, "inspect", json!({"id": PLAYER}))).contains("Downed"));
    let healed = text(&ok(&mut app, "dev_menu", json!({"run": "heal"}))).to_owned();
    assert!(healed.lines().next().is_some_and(|l| l.ends_with(", revived")), "{healed}");
    let fill = fails(&mut app, "dev_menu", json!({"run": "down"}));
    assert!(fill.starts_with("`down` needs values: run `down id= bleed=30`"), "{fill}");
}
