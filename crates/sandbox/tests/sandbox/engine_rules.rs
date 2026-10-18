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

/// A block of solid voxel cells under the canyon floor (inside the world's bounds, away from the
/// arena): 16 m of 1 m cells around (120, -20, 0).
fn block(app: &mut App) {
    use shards::motion::{VoxelCollider, VoxelField};
    app.world_mut().spawn(VoxelCollider::new(VoxelField::new(Vec3::new(112.0, -28.0, -8.0), 1.0, UVec3::splat(16), |_| true)));
}

/// Teleports the player into the block: its feet at the block's middle, its capsule centre in cell
/// (8, 8, 8), placed by something other than the controller (as `teleport` does).
fn bury(world: &mut World) {
    let player = body(world, 5);
    world.get_mut::<Transform>(player).unwrap().translation = Vec3::new(120.5, -20.0, 0.5);
}

/// The sandbox with `voxel.body_in_solid` on (`--engine-invariant voxel.body_in_solid`), or not.
fn voxel_rule(on: bool) -> App {
    sandbox::test_app_with(|config| {
        config.seed = 42;
        if on {
            config.opt_in_invariants = vec![shards::motion::BODY_IN_SOLID.into()];
        }
    })
}

#[test]
fn a_body_in_a_solid_voxel_cell_stops_the_game_once_the_rule_is_on() {
    use shards::motion::{body_in_solid, BODY_IN_SOLID};
    let names = |app: &App| app.world().resource::<AiInvariants>().rule_names().join(",");
    // Off by default: not even listed, and a body in rock goes unjudged.
    let mut app = voxel_rule(false);
    assert!(!names(&app).contains(BODY_IN_SOLID), "{}", names(&app));
    block(&mut app);
    plant(&mut app, 3, bury);
    let out = called(&mut app, "step", json!({"ticks": 20}));
    assert!(out.halted.is_none(), "{}", text(&out));

    // On. A world without fields is not judged.
    let app = voxel_rule(true);
    assert!(names(&app).starts_with("ai.index_consistent") && names(&app).contains(BODY_IN_SOLID), "{}", names(&app));
    assert_eq!(body_in_solid(app.world()), None);

    // Teleported into the rock: not on that tick (the controller did not put it there); on the
    // next, the controller has run and placed the body where it is, still deep in a block no
    // depenetration gets out of in a tick: a real controller tick that leaves the centre in a
    // solid cell, named with the body, its centre and the cell.
    let mut app = voxel_rule(true);
    block(&mut app);
    plant(&mut app, 3, bury);
    let line = stops_on(&mut app, BODY_IN_SOLID, 4);
    assert!(line.contains("e5 capsule centre [") && line.contains("] is in solid cell [") && line.contains(" of field "), "{line}");
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
fn a_protection_timer_that_is_not_a_time_stops_the_game() {
    use shards::action::Protection;
    // A NaN stays a NaN through the countdown and would shield its fighter for ever.
    let mut app = sandbox::test_app(42);
    plant(&mut app, 2, |world| {
        let player = body(world, 5);
        world.entity_mut(player).insert(Protection { reactions: f32::NAN, poise: 0.0 });
    });
    let line = stops_on(&mut app, "action.protection_sane", 2);
    assert!(line.contains("e5 Protection reactions = NaN, poise = 0"), "{line}");

    // A negative timer.
    let mut app = sandbox::test_app(42);
    plant(&mut app, 2, |world| {
        let boss = body(world, 6);
        world.entity_mut(boss).insert(Protection { reactions: 0.5, poise: -1.0 });
    });
    let line = stops_on(&mut app, "action.protection_sane", 2);
    assert!(line.contains("e6 Protection reactions = 0.5, poise = -1"), "{line}");
}

#[test]
fn a_barrier_that_is_not_a_pool_stops_the_game() {
    use shards::action::Barrier;
    // A negative pool would heal what it absorbs.
    let mut app = sandbox::test_app(42);
    plant(&mut app, 2, |world| {
        let player = body(world, 5);
        world.entity_mut(player).insert(Barrier { amount: -1.0, left: 5.0 });
    });
    let line = stops_on(&mut app, "action.barrier_sane", 2);
    assert!(line.contains("e5 Barrier amount = -1, left = 5"), "{line}");

    // A NaN time never runs out.
    let mut app = sandbox::test_app(42);
    plant(&mut app, 2, |world| {
        let boss = body(world, 6);
        world.entity_mut(boss).insert(Barrier { amount: 10.0, left: f32::NAN });
    });
    let line = stops_on(&mut app, "action.barrier_sane", 2);
    assert!(line.contains("e6 Barrier amount = 10, left = NaN"), "{line}");
}

#[test]
fn damage_taken_that_is_not_sorted_unique_and_finite_stops_the_game() {
    use shards::action::{DamageTaken, TakenMult};
    fn mult(key: &str, mult: f32) -> TakenMult {
        TakenMult { key: key.into(), mult, left: None }
    }
    // Two entries under one key.
    let mut app = sandbox::test_app(42);
    plant(&mut app, 2, |world| {
        let player = body(world, 5);
        world.entity_mut(player).insert(DamageTaken(vec![mult("core", 1.35), mult("core", 1.2)]));
    });
    let line = stops_on(&mut app, "action.damage_taken_sane", 2);
    assert!(line.contains("e5 DamageTaken keys `core`, `core` are not sorted and unique"), "{line}");

    // Keys out of order.
    let mut app = sandbox::test_app(42);
    plant(&mut app, 2, |world| {
        let boss = body(world, 6);
        world.entity_mut(boss).insert(DamageTaken(vec![mult("core", 1.35), mult("armor", 0.5)]));
    });
    let line = stops_on(&mut app, "action.damage_taken_sane", 2);
    assert!(line.contains("e6 DamageTaken keys `core`, `armor`"), "{line}");

    // A multiplier that is not a number, and a negative one.
    let mut app = sandbox::test_app(42);
    plant(&mut app, 2, |world| {
        let player = body(world, 5);
        world.entity_mut(player).insert(DamageTaken(vec![mult("core", f32::NAN)]));
    });
    let line = stops_on(&mut app, "action.damage_taken_sane", 2);
    assert!(line.contains("e5 DamageTaken `core` mult = NaN"), "{line}");
    let mut app = sandbox::test_app(42);
    plant(&mut app, 2, |world| {
        let player = body(world, 5);
        world.entity_mut(player).insert(DamageTaken(vec![mult("core", -0.5)]));
    });
    let line = stops_on(&mut app, "action.damage_taken_sane", 2);
    assert!(line.contains("e5 DamageTaken `core` mult = -0.5"), "{line}");
}

#[test]
fn a_push_that_is_not_flat_or_never_slows_stops_the_game() {
    use shards::action::Knockback;
    // A vertical component.
    let mut app = sandbox::test_app(42);
    plant(&mut app, 2, |world| {
        let player = body(world, 5);
        world.entity_mut(player).insert(Knockback { velocity: Vec3::Y, decel: 12.0 });
    });
    let line = stops_on(&mut app, "action.knockback_sane", 2);
    assert!(line.contains("e5 Knockback velocity = ") && line.contains("decel = 12"), "{line}");

    // No deceleration: it would never stop.
    let mut app = sandbox::test_app(42);
    plant(&mut app, 2, |world| {
        let boss = body(world, 6);
        world.entity_mut(boss).insert(Knockback { velocity: Vec3::X, decel: 0.0 });
    });
    let line = stops_on(&mut app, "action.knockback_sane", 2);
    assert!(line.contains("e6 Knockback velocity = ") && line.contains("decel = 0"), "{line}");
}

#[test]
fn run_extras_of_another_run_stop_the_game() {
    use shards::action::RunExtras;
    // Extras left behind by a run that is over (nothing runs now).
    let mut app = sandbox::test_app(42);
    plant(&mut app, 2, |world| {
        let player = body(world, 5);
        world.entity_mut(player).insert(RunExtras { instance: 999, ..default() });
    });
    let line = stops_on(&mut app, "action.extras_follow_run", 2);
    assert!(line.contains("e5 RunExtras of instance 999 while it runs no action"), "{line}");

    // A recovery made shorter than nothing.
    let mut app = sandbox::test_app(42);
    plant(&mut app, 2, |world| {
        let player = body(world, 5);
        assert!(shards::action::force_action(world, player, "player_stagger"));
        let instance = world.get::<shards::action::ActionState>(player).unwrap().current.as_ref().unwrap().instance;
        world.entity_mut(player).insert(RunExtras { instance, extra: -1.0, ..default() });
    });
    let line = stops_on(&mut app, "action.extras_follow_run", 2);
    assert!(line.contains("e5 RunExtras extra = -1, closed = 0: finite numbers >= 0"), "{line}");
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
        json!({"changes": [{"kind": "director", "id": "default",
            "before": {"patience": 3.0, "tokens": {"1": 2, "2": 3, "3": 4}, "waiting": [
                {"distance": 3.5, "orbit": 1.2, "period": 2.0, "role": "Circle", "speed": 0.5},
                {"distance": 3.0, "orbit": 1.0, "period": 1.5, "role": "Strafe", "speed": 0.5},
                {"distance": 5.0, "orbit": 1.0, "period": 2.0, "role": "Hold", "speed": 0.5}
            ]},
            "def": {"tokens": {"1": 2, "2": 3, "3": 4}, "waiting": [{"role": "Circle", "distance": 3.5, "orbit": 1.2}], "patience": 3.0, "min_spacing": 0.7}}]}),
    );
    plant(&mut app, 3, |world| {
        for (id, at) in [(7, 100), (8, 110)] {
            let e = body(world, id);
            world.entity_mut(e).insert(NpcStrike { last_start: at, gated: true, pool: "default".into() });
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
