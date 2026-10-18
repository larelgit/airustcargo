//! The engine's own invariant rules (`<module>.<rule>`): each one is shown to catch the bug it
//! exists for. A control test breaks the state on purpose and expects the stop
//! `stopped: invariant <name> violated at tick N`. A rule that cannot be shown to fire is not
//! a checker. The rules' own wiring (listing, removing, re-arming) is at the end.

use shards::ai_core::{self, AiInvariants, JournalKind};
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::{act, text, BOSS};
use crate::invariants::{called, journal, step, ENGINE_RULES};

/// Runs `corrupt` at the end of tick `at` (after the engine's own systems, before the rules),
/// the way a buggy system of a game would leave the world.
fn plant(app: &mut App, at: u64, corrupt: fn(&mut World)) {
    app.add_systems(
        FixedUpdate,
        (move |world: &mut World| {
            if ai_core::sim::tick(world) == at {
                corrupt(world);
            }
        })
        .in_set(SimSet::Cleanup),
    );
}

fn body(world: &World, id: u32) -> Entity {
    world.resource::<ai_core::AiIndex>().get(AiId(id)).unwrap()
}

/// Steps and expects the stop of engine rule `name` at tick `tick`; returns the journal's line.
fn stops_on(app: &mut App, name: &str, tick: u64) -> String {
    let out = called(app, "step", json!({"ticks": 20}));
    let want = format!("stopped: invariant {name} violated at tick {tick}");
    assert!(text(&out).contains(&want), "wanted `{want}`, got `{}`", text(&out));
    assert!(out.halted.is_some());
    journal(app, JournalKind::Error)
        .into_iter()
        .find(|l| l.contains(&format!("invariant {name} violated")))
        .expect("the journal has the line")
}

#[test]
fn a_stamina_or_poise_out_of_range_stops_the_game() {
    let mut app = sandbox::test_app(42);
    called(&mut app, "set", json!({"id": "e5", "component": "Stamina", "path": "current", "value": -5}));
    let line = stops_on(&mut app, "action.resources_in_range", 1);
    assert!(line.contains("e5 Stamina.current = -") && line.contains("outside 0..=100"), "{line}");

    let mut app = sandbox::test_app(42);
    called(&mut app, "set", json!({"id": BOSS, "component": "Poise", "path": "current", "value": 5000}));
    let line = stops_on(&mut app, "action.resources_in_range", 1);
    assert!(line.contains("e6 Poise.current = 5000 outside 0.."), "{line}");

    let mut app = sandbox::test_app(42);
    called(&mut app, "set", json!({"id": BOSS, "component": "Health", "path": "max", "value": 0}));
    let line = stops_on(&mut app, "action.resources_in_range", 1);
    assert!(line.contains("e6 Health.current = "), "{line}");
}

#[test]
fn an_intent_nobody_clamped_stops_the_game() {
    let mut app = sandbox::test_app(42);
    called(&mut app, "set", json!({"id": "e5", "component": "Intent", "path": "move_dir", "value": [5.0, 0.0, 0.0]}));
    let line = stops_on(&mut app, "core.intent_sane", 1);
    assert!(line.contains("e5 Intent.move_dir = ") && line.contains("is not finite or longer than 1"), "{line}");

    // NaN cannot travel through JSON.
    let mut app = sandbox::test_app(42);
    let player = body(app.world(), 5);
    app.world_mut().get_mut::<Intent>(player).unwrap().aim = Some(Vec3::splat(f32::NAN));
    let line = stops_on(&mut app, "core.intent_sane", 1);
    assert!(line.contains("e5 Intent.aim = ") && line.ends_with("is not finite"), "{line}");
}

#[test]
fn a_transform_or_motion_state_with_nan_stops_the_game() {
    let mut app = sandbox::test_app(42);
    plant(&mut app, 3, |world| {
        let boss = body(world, 6);
        world.get_mut::<Transform>(boss).unwrap().translation.y = f32::NAN;
    });
    let line = stops_on(&mut app, "motion.finite", 3);
    assert!(line.contains("e6 Transform.translation = ") && line.ends_with("is not finite"), "{line}");

    // An unnormalised quaternion (a prop a level file placed so) is judged on a bare world:
    // in a running game, Bevy's own `Dir3` assertions panic on it first.
    let mut world = World::new();
    world.init_resource::<ai_core::AiIndex>();
    world.spawn((AiVisible, Transform { rotation: Quat::from_xyzw(0.0, 0.0, 0.0, 1.05), ..default() }));
    let text = shards::motion::motion_finite(&world).expect("1.05 is not a rotation");
    assert!(text.starts_with("e1 Transform.rotation = ") && text.contains("not normalised"), "{text}");
    world.clear_entities();
    world.spawn((AiVisible, Transform { rotation: Quat::from_xyzw(0.0, 0.0, 0.0, 1.0004), ..default() }));
    assert_eq!(shards::motion::motion_finite(&world), None, "within the tolerance");


    let mut app = sandbox::test_app(42);
    plant(&mut app, 3, |world| {
        let player = body(world, 5);
        world.get_mut::<shards::motion::MotionState>(player).unwrap().velocity.x = f32::INFINITY;
    });
    let line = stops_on(&mut app, "motion.finite", 3);
    assert!(line.contains("e5 MotionState.velocity = ") && line.ends_with("is not finite"), "{line}");
}

#[test]
fn an_id_written_in_place_breaks_the_index() {
    let mut app = sandbox::test_app(42);
    let boss = body(app.world(), 6);
    // `AiId` is managed by the engine: an in-place write skips the hooks that keep the index.
    app.world_mut().get_mut::<AiId>(boss).unwrap().0 = 77;
    let line = stops_on(&mut app, "ai.index_consistent", 1);
    assert!(line.contains("e6 is in the index for") && line.contains("(+1 more)"), "{line}");
}

#[test]
fn a_running_action_the_machine_would_not_have_stops_the_game() {
    use shards::action::ActionState;
    let start = |app: &mut App| {
        called(app, "light_attack", json!({}));
        step(app, 1);
    };
    // Its phase is not one of the action's phases.
    let mut app = sandbox::test_app(42);
    start(&mut app);
    plant(&mut app, 2, |world| {
        let player = body(world, 5);
        world.get_mut::<ActionState>(player).unwrap().current.as_mut().expect("still swinging").phase = "bogus".into();
    });
    let line = stops_on(&mut app, "action.running_sane", 2);
    assert!(line.contains("e5 action `") && line.contains("is in phase `bogus`"), "{line}");

    // Its clock ran away.
    let mut app = sandbox::test_app(42);
    start(&mut app);
    plant(&mut app, 2, |world| {
        let player = body(world, 5);
        world.get_mut::<ActionState>(player).unwrap().current.as_mut().expect("still swinging").t = 99.0;
    });
    let line = stops_on(&mut app, "action.running_sane", 2);
    assert!(line.contains("e5 action `") && line.contains("t = 99 outside 0.."), "{line}");

    // It is not in content.
    let mut app = sandbox::test_app(42);
    start(&mut app);
    plant(&mut app, 2, |world| {
        let player = body(world, 5);
        world.get_mut::<ActionState>(player).unwrap().current.as_mut().expect("still swinging").action = "ghost".into();
    });
    let line = stops_on(&mut app, "action.running_sane", 2);
    assert!(line.contains("e5 runs action `ghost`, which is not in content"), "{line}");
}

#[test]
fn an_npc_in_a_state_its_behavior_lacks_stops_the_game() {
    use shards::npc::NpcBrain;
    let mut app = sandbox::test_app(42);
    plant(&mut app, 3, |world| {
        let boss = body(world, 6);
        let mut brain = world.get_mut::<NpcBrain>(boss).unwrap();
        brain.state = "bogus".into();
        brain.active = true;
    });
    let line = stops_on(&mut app, "npc.state_known", 3);
    assert!(line.contains("e6 NpcBrain.state = `bogus` is not a state of behavior `"), "{line}");

    // A carve-out: an inactive NPC keeps a stale state until it wakes.
    let mut app = sandbox::test_app(42);
    plant(&mut app, 3, |world| {
        let boss = body(world, 6);
        let mut brain = world.get_mut::<NpcBrain>(boss).unwrap();
        brain.state = "bogus".into();
        brain.active = false;
    });
    assert_eq!(step(&mut app, 10), "ran 10 ticks (0 → 10)");
}

#[test]
fn two_planned_strikes_closer_than_the_spacing_stop_the_game() {
    use shards::npc::NpcStrike;
    // The skeletons' director spaces strikes 0.7 s apart; two of them are found to have begun
    // planned strikes 10 ticks apart. The rule's own cases are tested where it lives (shards_npc).
    let mut app = sandbox::test_app(42);
    called(
        &mut app,
        "content_apply",
        json!({"changes": [{"kind": "director", "id": "default", "def": {
            "tokens": {"1": 2, "2": 3, "3": 4},
            "waiting": [{"role": "Circle", "distance": 3.5, "orbit": 1.2}],
            "patience": 3.0,
            "min_spacing": 0.7
        }}]}),
    );
    plant(&mut app, 3, |world| {
        for (id, at) in [(7, 100), (8, 110)] {
            let e = body(world, id);
            world.entity_mut(e).insert(NpcStrike { last_start: at, gated: true });
        }
    });
    let line = stops_on(&mut app, "npc.strike_spacing", 3);
    assert!(line.contains("e8 NpcStrike: began a strike on tick 110, 10 ticks after e7's on tick 100"), "{line}");
}

#[test]
fn a_token_audit_that_found_trouble_stops_the_game() {
    // The audit itself is tested where the director's books are (shards_npc); here the rule
    // that reports it stops the game.
    let mut app = sandbox::test_app(42);
    plant(&mut app, 2, |world| {
        world.resource_mut::<shards::npc::TokenAudit>().0 = Some("e6 AttackToken: planted".into());
    });
    let line = stops_on(&mut app, "npc.tokens_within_limit", 2);
    assert!(line.ends_with("e6 AttackToken: planted"), "{line}");
}

#[test]
fn engine_rules_stay_and_are_rearmed_like_game_rules() {
    let mut app = sandbox::test_app(42);
    let stays = act(&mut app, "assert_remove", json!({"name": "motion.finite"}));
    assert!(!stays.ok);
    assert!(text(&stays).contains("`motion.finite` is one of the engine's invariants and stays"), "{}", text(&stays));
    let taken = act(&mut app, "assert_add", json!({"name": "motion.finite", "when": "error"}));
    assert!(!taken.ok);
    assert!(text(&taken).contains("one of the engine's invariants"), "{}", text(&taken));
    assert!(app.world_mut().resource_mut::<AiInvariants>().add_game_rule("motion.finite", |_| None).is_err());

    let list = text(&called(&mut app, "assert_list", json!({}))).to_owned();
    assert!(list.contains("  action.running_sane  engine rule  armed"), "{list}");

    called(&mut app, "set", json!({"id": "e5", "component": "Stamina", "path": "current", "value": -5}));
    assert!(text(&called(&mut app, "step", json!({"ticks": 10}))).contains("stopped: invariant action.resources_in_range violated at tick 1"));
    let list = text(&called(&mut app, "assert_list", json!({}))).to_owned();
    assert!(list.contains("  action.resources_in_range  engine rule  fired at tick 1, off until assert_reset"), "{list}");
    assert_eq!(step(&mut app, 5), "ran 5 ticks (1 → 6)", "off until re-armed");
    called(&mut app, "set", json!({"id": "e5", "component": "Stamina", "path": "current", "value": -5}));
    assert!(text(&called(&mut app, "assert_reset", json!({}))).starts_with("re-armed action.resources_in_range"));
    assert!(text(&called(&mut app, "step", json!({"ticks": 5}))).contains("stopped: invariant action.resources_in_range violated at tick 7"));
}

#[test]
fn the_engine_registers_its_rules_first_and_the_switch_turns_them_off() {
    let app = sandbox::test_app(42);
    let names = app.world().resource::<AiInvariants>().rule_names().join(",");
    assert_eq!(names, format!("{},health_in_range,bodies_in_world", ENGINE_RULES.join(",")));

    let mut app = sandbox::test_app_with(|config| config.engine_invariants = false);
    let names = app.world().resource::<AiInvariants>().rule_names().join(",");
    assert_eq!(names, "health_in_range,bodies_in_world", "the game's rules stay");
    assert!(app.world().get_resource::<shards::npc::TokenAudit>().is_none(), "and no audit system runs");
    called(&mut app, "set", json!({"id": "e5", "component": "Stamina", "path": "current", "value": -5}));
    assert_eq!(step(&mut app, 5), "ran 5 ticks (0 → 5)", "nothing judges a stamina below 0");
}

#[test]
fn the_rules_keep_their_query_states_in_the_world() {
    let mut app = sandbox::test_app(42);
    assert!(app.world().resource::<ai_core::RuleQueries>().is_empty());
    step(&mut app, 3);
    assert!(app.world().resource::<ai_core::RuleQueries>().len() >= 7, "the engine rules' queries are kept");
    let again = sandbox::test_app(42);
    assert!(again.world().resource::<ai_core::RuleQueries>().is_empty(), "another app starts empty: nothing is shared");
}
