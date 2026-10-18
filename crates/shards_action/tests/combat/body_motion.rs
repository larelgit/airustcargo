//! Body and motion (B2c): the leap and its landing search, the launch, walking during a span, the
//! mode gate (`only_in`), intangible spans and ghosts. Content in `assets/content/body_motion.*.ron`.

use bevy::prelude::*;
use serde_json::json;
use shards_action::{
    can_start, force_action, intangible, plan_leap, ActionDef, ActionEnd, ActionState, Blocker, Cooldowns, FighterView, LeapPlan, LeapRun,
    LeapSite,
};
use shards_content::ContentDb;
use shards_core::Intent;
use shards_motion::{Collider, CollisionLayers, Ghost, Launch, MotionMode, MotionState, Shape, Volume, VolumeKind};

use crate::common::*;
use crate::reactions::{record, seen};

fn arena() -> App {
    app_with(record)
}

fn me(app: &mut App) -> Entity {
    hero(app.world_mut(), "b_hero", Vec3::ZERO, N)
}

fn digger(app: &mut App, at: Vec3) -> Entity {
    fighter(app.world_mut(), "Digger", "b_digger", "monsters", at, S)
}

fn lock(app: &mut App, e: Entity, target: Entity) {
    let id = *app.world().get::<ai_core::AiId>(target).unwrap();
    app.world_mut().get_mut::<Intent>(e).unwrap().target = Some(id);
}

fn ends(app: &App, action: &str) -> Vec<ActionEnd> {
    seen(app).ended.iter().filter(|e| e.2 == action).map(|e| e.3).collect()
}

fn mode(app: &App, e: Entity) -> MotionMode {
    app.world().get::<MotionState>(e).unwrap().mode
}

/// The landing the search finds for `action` from where `e` stands.
fn plan(app: &App, e: Entity, action: &str) -> Option<LeapPlan> {
    let db = app.world().resource::<ContentDb>();
    let def = db.get::<ActionDef>(action).unwrap();
    let search = def.leap.as_ref().and_then(|l| l.search.as_ref()).unwrap();
    plan_leap(def, search, &LeapSite::of(app.world(), e).unwrap(), None)
}

/// A pool whose floor is 5 m down at x 80..120: a swimmer put in it is in water mode in a few ticks.
fn pool(app: &mut App) -> Vec3 {
    let world = app.world_mut();
    wall(world, Vec3::new(80.0, -6.0, -20.0), Vec3::new(120.0, -5.0, 20.0));
    world.spawn((
        Collider::new(Shape::cuboid(Vec3::new(20.0, 2.5, 20.0))).with_layers(CollisionLayers::VOLUME),
        Volume { kind: VolumeKind::Water },
        Transform::from_translation(Vec3::new(100.0, -2.5, 0.0)),
    ));
    Vec3::new(100.0, -1.1, 0.0)
}

/// Water `depth` deep over the floor between two distances north of the origin.
fn puddle(app: &mut App, near: f32, far: f32, depth: f32) {
    app.world_mut().spawn((
        Collider::new(Shape::cuboid(Vec3::new(5.0, depth * 0.5, (far - near) * 0.5))).with_layers(CollisionLayers::VOLUME),
        Volume { kind: VolumeKind::Water },
        Transform::from_translation(Vec3::new(0.0, depth * 0.5, -(near + far) * 0.5)),
    ));
}

// --- the leap --------------------------------------------------------------------------------

#[test]
fn leap_picks_the_farthest_landing() {
    let mut app = arena();
    let hero = me(&mut app);
    step(&mut app, 1);
    // Open ground: the farthest candidate, 6 m.
    let open = plan(&app, hero, "b_leap").expect("open ground");
    assert!((open.distance - 6.0).abs() < 1e-4 && open.dir.abs_diff_eq(N, 1e-4), "{open:?}");
    // A wall 4.7 m north: 6, 5.5 and 5 m are behind it, 4.5 m overlaps it; 4 clears it.
    wall(app.world_mut(), Vec3::new(-5.0, 0.0, -5.2), Vec3::new(5.0, 3.0, -4.7));
    step(&mut app, 1);
    let blocked = plan(&app, hero, "b_leap").expect("a landing short of the wall");
    assert!((blocked.distance - 4.0).abs() < 1e-4, "{blocked:?}");
    // The run goes there: the body ends the leap about 4 m north.
    press(&mut app, hero, "technique.1");
    step(&mut app, 80);
    let landed = feet(&app, hero);
    assert!((landed.z + 4.0).abs() < 0.15 && landed.x.abs() < 0.05 && landed.y.abs() < 0.05, "{landed}");
    assert_eq!(action(&app, hero), "idle");
}

#[test]
fn leap_without_landing_is_rejected_unpaid() {
    let mut app = arena();
    let hero = me(&mut app);
    // `b_leap_far` lands 3 m away or not at all; a wall 2.5 m north stops it.
    wall(app.world_mut(), Vec3::new(-5.0, 0.0, -3.0), Vec3::new(5.0, 3.0, -2.5));
    step(&mut app, 1);
    let view = FighterView::of(app.world(), hero).unwrap();
    let err = can_start(&view, "technique.4", DT).unwrap_err();
    assert_eq!(err, Blocker::NoLanding { min: 3.0, max: 3.0 });
    assert_eq!(err.ready_in(), None, "waiting will not do");
    let refused = fails(&mut app, "use_technique", json!({"slot": 4}));
    assert!(refused.contains("no landing spot within 3-3m"), "{refused}");
    // A press made anyway (a keyboard) waits in the buffer like any press the machine cannot start (the host and a client judge the landing alike), then is rejected and nothing is paid.
    let before = stamina(&app, hero);
    press(&mut app, hero, "technique.4");
    step(&mut app, 40);
    assert_eq!(action(&app, hero), "idle");
    assert_eq!(stamina(&app, hero), before);
    assert_eq!(app.world().get::<Cooldowns>(hero).unwrap().left("b_leap_far"), 0.0);
    let rejected = lines(&app, "technique.4 rejected");
    assert_eq!(rejected.len(), 1, "{:#?}", journal(&app));
    assert!(rejected[0].contains("no landing spot within 3-3m"), "{rejected:?}");
    // Open ground: it starts.
    let mut app = arena();
    let hero = me(&mut app);
    step(&mut app, 1);
    let view = FighterView::of(app.world(), hero).unwrap();
    assert!(can_start(&view, "technique.4", DT).is_ok());
}

#[test]
fn leap_launches_once_on_the_ground() {
    let mut app = arena();
    let hero = me(&mut app);
    press(&mut app, hero, "technique.1");
    let mut top: f32 = 0.0;
    let mut risen = 0;
    let mut was_rising = false;
    for tick in 1..=80 {
        step(&mut app, 1);
        top = top.max(feet(&app, hero).y);
        let rising = app.world().get::<MotionState>(hero).unwrap().velocity.y > 0.5;
        if rising && !was_rising {
            risen += 1;
        }
        was_rising = rising;
        assert!(app.world().get::<Launch>(hero).is_none(), "the launch is taken the tick it is put (tick {tick})");
        if tick == 20 {
            let leap = app.world().get::<LeapRun>(hero).copied().expect("a leap run");
            assert!(leap.launched && (leap.scale - 1.0).abs() < 1e-4, "{leap:?}: open ground, the authored 6 m");
        }
    }
    assert!((top - 1.2).abs() < 0.04, "peaked at {top}");
    assert_eq!(risen, 1, "one launch");
    assert!(app.world().get::<LeapRun>(hero).is_none(), "taken out with the run");
    assert!((feet(&app, hero).z + 6.0).abs() < 0.15, "{}", feet(&app, hero));
}

#[test]
fn a_leap_on_a_body_that_is_not_grounded_launches_nothing() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "b_hero", Vec3::new(0.0, 3.0, 0.0), N);
    // `b_hop` has no search, so it starts in the air; its launch at 0.1 s finds no ground.
    press(&mut app, hero, "technique.8");
    let mut top: f32 = 0.0;
    for _ in 0..12 {
        step(&mut app, 1);
        top = top.max(feet(&app, hero).y);
    }
    assert!(app.world().get::<LeapRun>(hero).is_some_and(|l| l.launched), "the launch was asked for");
    assert!(top <= 3.0 + 1e-3 && app.world().get::<MotionState>(hero).unwrap().velocity.y < 0.0, "still falling: {top}");
}

#[test]
fn leap_toward_the_target_lands_at_its_edge() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "b_hero", Vec3::ZERO, E);
    let target = digger(&mut app, Vec3::new(0.0, 0.0, -5.0));
    lock(&mut app, hero, target);
    step(&mut app, 1);
    let p = plan(&app, hero, "b_leap_to").expect("a landing");
    // 5 m between them, 0.4 m of body each: 4.2 m, pointing north though the body faces east.
    assert!((p.distance - 4.2).abs() < 1e-3 && p.dir.abs_diff_eq(N, 1e-4), "{p:?}");
    press(&mut app, hero, "technique.2");
    step(&mut app, 2);
    let facing = app.world().get::<ActionState>(hero).unwrap().current.as_ref().unwrap().facing;
    assert!(facing.abs_diff_eq(N, 1e-3), "the run points at the target: {facing}");
    step(&mut app, 70);
    let landed = feet(&app, hero);
    assert!((landed.z + 4.2).abs() < 0.2, "{landed}");
}

#[test]
fn leap_search_refuses_a_landing_in_deep_water() {
    let mut app = arena();
    let hero = me(&mut app);
    // Water 2 m deep from 4.2 m north to 8 m: 6, 5.5, 5 and 4.5 m come down in it.
    puddle(&mut app, 4.2, 8.0, 2.0);
    step(&mut app, 1);
    let dry = plan(&app, hero, "b_leap_wet").expect("a dry landing");
    assert!((dry.distance - 4.0).abs() < 1e-4, "{dry:?}");
    // The plain leap does not mind.
    let wet = plan(&app, hero, "b_leap").expect("a landing");
    assert!((wet.distance - 6.0).abs() < 1e-4, "{wet:?}");
    // Water 0.5 m deep is no reason.
    let mut app = arena();
    let hero = me(&mut app);
    puddle(&mut app, 4.2, 8.0, 0.5);
    step(&mut app, 1);
    assert!((plan(&app, hero, "b_leap_wet").unwrap().distance - 6.0).abs() < 1e-4);
    // Deep water everywhere between 1 and 6 m: no landing at all.
    let mut app = arena();
    let hero = me(&mut app);
    puddle(&mut app, 0.8, 8.0, 2.0);
    step(&mut app, 1);
    assert!(plan(&app, hero, "b_leap_wet").is_none());
    let view = FighterView::of(app.world(), hero).unwrap();
    assert!(matches!(can_start(&view, "technique.3", DT), Err(Blocker::NoLanding { .. })));
}

#[test]
fn a_cut_leap_takes_its_state_with_it() {
    let mut app = arena();
    let hero = me(&mut app);
    press(&mut app, hero, "technique.1");
    step(&mut app, 15);
    assert!(app.world().get::<LeapRun>(hero).is_some());
    assert!(force_action(app.world_mut(), hero, "b_stagger"));
    assert!(app.world().get::<LeapRun>(hero).is_none());
    assert_eq!(ends(&app, "b_leap"), [ActionEnd::Interrupted]);
}

// --- walking during a span -----------------------------------------------------------------

#[test]
fn walk_span_unlocks_the_body() {
    let travelled = |action_input: Option<&str>| {
        let mut app = arena();
        let hero = me(&mut app);
        if let Some(input) = action_input {
            press(&mut app, hero, input);
        }
        app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = N;
        // Windup: the action owns the body (0.2 s = 12 ticks); the spin walks (0.2-1.2 s).
        step(&mut app, 10);
        let in_windup = -feet(&app, hero).z;
        step(&mut app, 50);
        (in_windup, -feet(&app, hero).z - in_windup)
    };
    let (windup, spin) = travelled(Some("technique.5"));
    let (free_windup, free) = travelled(None);
    assert!(windup < 0.05, "locked in the windup: {windup}");
    assert!(free_windup > 0.5, "the same input walks a free body: {free_windup}");
    // The spin walks at 35% of the input: slower than a free body over the same ticks, but it walks.
    assert!(spin > 0.3 && spin < 0.6 * free, "spin {spin}, free {free}");
}

// --- the mode gate ---------------------------------------------------------------------------

#[test]
fn afloat_gate_blocks_on_water() {
    let mut app = arena();
    let at = pool(&mut app);
    let hero = hero(app.world_mut(), "b_hero", at, N);
    step(&mut app, 30);
    assert_eq!(mode(&app, hero), MotionMode::Water);
    let view = FighterView::of(app.world(), hero).unwrap();
    let err = can_start(&view, "light", DT).unwrap_err();
    assert!(matches!(&err, Blocker::Mode { mode: MotionMode::Water, tags, .. } if tags == &["afloat"]), "{err:?}");
    assert_eq!(err.ready_in(), None);
    assert_eq!(err.reason(), "not while in water: only actions tagged afloat");
    // An action with the tag starts.
    assert!(can_start(&view, "technique.7", DT).is_ok());
    let refused = fails(&mut app, "light_attack", json!({}));
    assert!(refused.contains("only actions tagged afloat"), "{refused}");
    press(&mut app, hero, "technique.7");
    step(&mut app, 2);
    assert_eq!(action(&app, hero), "b_stroke");
    // On the ground the same press starts the swing.
    let mut app = arena();
    let hero = me(&mut app);
    let view = FighterView::of(app.world(), hero).unwrap();
    assert!(can_start(&view, "light", DT).is_ok());
}

#[test]
fn only_in_cuts_a_running_action_on_entering_water() {
    let mut app = arena();
    let at = pool(&mut app);
    let hero = me(&mut app);
    press(&mut app, hero, "dodge");
    step(&mut app, 6);
    assert_eq!(action(&app, hero), "b_roll");
    let cooldown = app.world().get::<Cooldowns>(hero).unwrap().left("b_roll");
    // Into the water (a teleport stands for walking in): in a few ticks the body swims and the roll is gone.
    app.world_mut().get_mut::<Transform>(hero).unwrap().translation = at;
    step(&mut app, 3);
    assert_eq!(mode(&app, hero), MotionMode::Water);
    assert_eq!(action(&app, hero), "idle");
    assert_eq!(ends(&app, "b_roll"), [ActionEnd::Interrupted]);
    // The windup was cut, and still the cooldown keeps no share: the gate is not an interruption.
    let after = app.world().get::<Cooldowns>(hero).unwrap().left("b_roll");
    assert!(after > cooldown - 0.2 && after > 1.5, "{cooldown} -> {after}");
    // A forced action (a stagger) is never cut by the gate.
    assert!(force_action(app.world_mut(), hero, "b_stagger"));
    step(&mut app, 5);
    assert_eq!(action(&app, hero), "b_stagger");
}

// --- intangible and ghost ---------------------------------------------------------------------

#[test]
fn intangible_targets_are_not_hit_or_marked() {
    let mut app = arena();
    let hero = me(&mut app);
    let victim = digger(&mut app, Vec3::new(0.0, 0.0, -1.5));
    press(&mut app, victim, "technique.6");
    step(&mut app, 20);
    assert!(intangible(app.world(), victim), "under: {}", action(&app, victim));
    // The sweep (1.5 s of window) starts while the victim is underground.
    press(&mut app, hero, "heavy");
    step(&mut app, 25);
    assert_eq!(hp(&app, victim), 100.0, "nothing to hit while under");
    assert!(app.world().get::<ActionState>(hero).unwrap().current.as_ref().unwrap().hit.is_empty(), "and no mark");
    // It comes out at 0.9 s (tick 54): the sweep is still open, so it hits then.
    step(&mut app, 12);
    assert!(!intangible(app.world(), victim));
    assert_eq!(hp(&app, victim), 90.0, "hit once it is out");
    step(&mut app, 20);
    assert_eq!(hp(&app, victim), 90.0, "once");
}

#[test]
fn ghost_passes_through_bodies() {
    let gap_after = |input: &str| {
        let mut app = arena();
        let walker = me(&mut app);
        let victim = digger(&mut app, Vec3::new(0.0, 0.0, -3.0));
        press(&mut app, victim, input);
        // The walker goes straight for the burrower's spot, arriving while it is under (0.3-0.9 s).
        app.world_mut().get_mut::<Intent>(walker).unwrap().move_dir = N;
        let (mut closest, mut ghost) = (f32::MAX, false);
        for _ in 0..60 {
            step(&mut app, 1);
            let (a, b) = (feet(&app, walker), feet(&app, victim));
            closest = closest.min(Vec3::new(a.x - b.x, 0.0, a.z - b.z).length());
            ghost |= app.world().get::<Ghost>(victim).is_some();
        }
        (closest, ghost)
    };
    let (through, ghost) = gap_after("technique.6");
    assert!(ghost, "a burrow passes through bodies while it is under");
    assert!(through < 0.7, "walked into the burrower: {through}");
    let (stopped, ghost) = gap_after("technique.9");
    assert!(!ghost);
    assert!(stopped > 0.75, "an intangible body that is not a ghost is still pushed: {stopped}");
}

#[test]
fn a_ghost_is_solid_again_when_its_run_is_cut() {
    let mut app = arena();
    let victim = digger(&mut app, Vec3::new(0.0, 0.0, -1.5));
    press(&mut app, victim, "technique.6");
    step(&mut app, 25);
    assert!(app.world().get::<Ghost>(victim).is_some());
    assert!(force_action(app.world_mut(), victim, "b_stagger"));
    assert!(app.world().get::<Ghost>(victim).is_none());
    assert!(!intangible(app.world(), victim));
    // And when it emerges by itself.
    let mut app = arena();
    let victim = digger(&mut app, Vec3::new(0.0, 0.0, -1.5));
    press(&mut app, victim, "technique.6");
    step(&mut app, 60);
    assert!(app.world().get::<Ghost>(victim).is_none(), "out of the ground at 0.9 s");
    step(&mut app, 30);
    assert_eq!(action(&app, victim), "idle");
    assert!(app.world().get::<Ghost>(victim).is_none());
}

#[test]
fn a_leap_with_no_landing_does_not_take_the_start_slot() {
    let mut app = arena();
    let hero = me(&mut app);
    // A wall 2.5 m north: `b_leap_far` (3 m or nothing) has no landing, and `light` (a slam) is
    // pressed after it in the same tick.
    wall(app.world_mut(), Vec3::new(-5.0, 0.0, -3.0), Vec3::new(5.0, 3.0, -2.5));
    step(&mut app, 1);
    let before = stamina(&app, hero);
    press(&mut app, hero, "technique.4");
    press(&mut app, hero, "light");
    step(&mut app, 3);
    assert_eq!(action(&app, hero), "b_slam", "the press behind the leap started: {:#?}", journal(&app));
    assert!(stamina(&app, hero) <= before);
    assert_eq!(app.world().get::<Cooldowns>(hero).unwrap().left("b_leap_far"), 0.0, "the leap paid nothing");
}
