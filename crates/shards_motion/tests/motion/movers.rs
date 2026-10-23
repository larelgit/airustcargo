//! Movers (F6) on a headless engine: a lift carries what stands on it up and down, a turning
//! platform carries it round, a closing door pushes a body out of its way, a body that cannot be
//! pushed holds the mover by its `blocked` rule, and a snapshot taken mid-move goes on the same.

use ai_core::AiJournal;
use bevy::prelude::*;
use serde_json::json;
use shards_motion::*;

use crate::common::*;
use crate::engine::*;

const MOVERS: &str = r#"{
    "lift": (keys: [(), (at: (0.0, 4.0, 0.0))], speed: 1.2),
    "door": (keys: [(), (at: (-3.0, 0.0, 0.0))], speed: 1.5),
    "door_back": (keys: [(), (at: (-3.0, 0.0, 0.0))], speed: 1.5, blocked: Reverse),
    "press": (keys: [(), (at: (0.0, -2.6, 0.0))], speed: 1.0, blocked: Crush(damage: 10.0)),
    "spin": (keys: [(), (turn: (0.0, 360.0, 0.0), time: 8.0)], loop: Cycle),
    "shuttle": (keys: [(), (at: (4.0, 0.0, 0.0)), (at: (4.0, 0.0, 4.0))], speed: 2.0, wait: 0.5, loop: PingPong),
}"#;

/// Every mover event, in order.
#[derive(Resource, Default, Debug)]
struct Heard {
    started: Vec<MoverStarted>,
    arrived: Vec<MoverArrived>,
    blocked: Vec<MoverBlocked>,
}

fn movers_app(name: &str) -> App {
    movers_app_with(name, MOVERS)
}

fn movers_app_with(name: &str, movers: &str) -> App {
    let dir = temp_dir(name);
    write(&dir, "content/test.mover.ron", movers);
    let mut app = engine_app(&dir);
    app.init_resource::<Heard>()
        .add_observer(|e: On<MoverStarted>, mut heard: ResMut<Heard>| heard.started.push(*e.event()))
        .add_observer(|e: On<MoverArrived>, mut heard: ResMut<Heard>| heard.arrived.push(*e.event()))
        .add_observer(|e: On<MoverBlocked>, mut heard: ResMut<Heard>| heard.blocked.push(*e.event()));
    app
}

fn errors(app: &App) -> Vec<String> {
    app.world().resource::<shards_content::ContentDb>().errors().map(ToString::to_string).collect()
}

fn heard(app: &App) -> &Heard {
    app.world().resource::<Heard>()
}

/// A box mover named `name`, moving by `def`, centred at `center`.
fn mover(world: &mut World, name: &str, def: &str, center: Vec3, half: Vec3) -> Entity {
    world.spawn((Name::new(name.to_owned()), Mover { def: def.into() }, Collider::new(Shape::cuboid(half)), Transform::from_translation(center))).id()
}

fn mover_state_of(app: &App, e: Entity) -> MoverState {
    app.world().get::<MoverState>(e).cloned().expect("the mover has its state after its first tick")
}

fn top(app: &App, e: Entity, half_height: f32) -> f32 {
    app.world().get::<Transform>(e).unwrap().translation.y + half_height
}

fn journal(app: &App) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.map(|e| e.text.clone()).collect()
}

/// Steps until the mover rests at `key`, checking `each` every tick; returns the ticks.
fn ride(app: &mut App, lift: Entity, key: u32, max: u64, mut each: impl FnMut(&App)) -> u64 {
    for n in 1..=max {
        step(app, 1);
        each(app);
        let s = mover_state_of(app, lift);
        if s.from == key && s.to == key {
            return n;
        }
    }
    panic!("the mover did not reach key {key} in {max} ticks: {:?}", mover_state_of(app, lift));
}

#[test]
fn the_test_movers_are_sound_content() {
    let app = movers_app("movers_content");
    assert!(errors(&app).is_empty(), "{:?}", errors(&app));
}

#[test]
fn a_lift_carries_a_body_up_and_down_and_it_stays_on_through_the_stops() {
    let mut app = movers_app("movers_lift");
    floor(app.world_mut());
    let lift = mover(app.world_mut(), "Lift", "lift", Vec3::new(0.0, 0.1, 0.0), Vec3::new(1.0, 0.1, 1.0));
    let body = player(app.world_mut(), Vec3::new(0.2, 0.25, -0.1));
    step(&mut app, 30);
    assert!(state(&app, body).grounded, "it stands on the lift");
    let rest = feet(&app, body) - Vec3::Y * top(&app, lift, 0.1);
    let check = |app: &App| {
        let off = feet(app, body) - Vec3::Y * top(app, lift, 0.1);
        assert!((off.y - rest.y).abs() < 0.03, "the body keeps to the platform: {off} (at rest {rest})");
        assert!(Vec2::new(off.x - rest.x, off.z - rest.z).length() < 0.01, "and does not slide: {off} (at rest {rest})");
        assert!(state(app, body).grounded, "it stays on the ground: {:?}", state(app, body));
    };
    let out = ok(&mut app, "mover_go", json!({"name": "lift", "key": 1}));
    assert!(out.contains("goes to key 1") && out.contains("3.3s"), "{out}");
    let up = ride(&mut app, lift, 1, 400, check);
    // 4 m at 1.2 m/s.
    assert!((199..=202).contains(&up), "{up} ticks up");
    assert!((top(&app, lift, 0.1) - 4.2).abs() < 1e-4);
    // At rest at the top: it stays where it is.
    for _ in 0..30 {
        step(&mut app, 1);
        check(&app);
    }
    assert_eq!(app.world().get::<Body>(lift), Some(&Body::Static), "a mover at rest is laid geometry");
    let id = format!("e{}", app.world().get::<ai_core::AiId>(lift).unwrap().0);
    ok(&mut app, "mover_go", json!({"id": id, "key": 0}));
    step(&mut app, 1);
    assert_eq!(app.world().get::<Body>(lift), Some(&Body::Kinematic), "a moving one is kinematic");
    ride(&mut app, lift, 0, 400, check);
    assert_clear(&app, body);
    let lines = journal(&app);
    for want in ["Lift leaves key 0 for key 1", "Lift at key 1", "Lift leaves key 1 for key 0", "Lift at key 0"] {
        assert!(lines.iter().any(|l| l.contains(want)), "{want}: {lines:?}");
    }
    let heard = heard(&app);
    assert_eq!(heard.arrived.iter().map(|a| a.key).collect::<Vec<_>>(), [1, 0]);
    assert!(heard.blocked.is_empty(), "{:?}", heard.blocked);
}

#[test]
fn a_body_jumping_on_a_rising_lift_is_lifted_not_left_inside() {
    let mut app = movers_app("movers_lift_jump");
    floor(app.world_mut());
    let lift = mover(app.world_mut(), "Lift", "lift", Vec3::new(0.0, 0.1, 0.0), Vec3::new(1.0, 0.1, 1.0));
    let body = player(app.world_mut(), Vec3::new(0.0, 0.25, 0.0));
    step(&mut app, 30);
    ok(&mut app, "mover_go", json!({"name": "lift", "key": 1}));
    step(&mut app, 60);
    edit_intent(&mut app, body, |i| i.press(shards_core::input_names::JUMP));
    ride(&mut app, lift, 1, 400, |app| assert_clear(app, body));
    step(&mut app, 60);
    assert!((feet(&app, body).y - 4.2).abs() < 0.05, "it lands on the lift: {}", feet(&app, body));
}

#[test]
fn a_turning_platform_carries_a_body_round_and_turns_it_with_it() {
    let mut app = movers_app("movers_spin");
    floor(app.world_mut());
    let spin = app
        .world_mut()
        .spawn((Name::new("Turntable"), Mover { def: "spin".into() }, Collider::new(Shape::Cylinder { half_height: 0.1, radius: 3.0 }), Transform::from_xyz(0.0, 0.1, 0.0)))
        .id();
    let body = player(app.world_mut(), Vec3::new(2.0, 0.25, 0.0));
    step(&mut app, 30);
    let start = feet(&app, body);
    let facing = app.world().get::<Transform>(body).unwrap().rotation;
    let turned = mover_state_of(&app, spin).t;
    // A quarter turn (2 s of 8), counter-clockwise seen from above: +X goes to -Z.
    step(&mut app, 120);
    let t = mover_state_of(&app, spin).t - turned;
    assert!((t - 0.25).abs() < 1e-3, "{t}");
    let at = feet(&app, body);
    let angle = std::f32::consts::FRAC_PI_2;
    let want = Quat::from_rotation_y(angle) * Vec3::new(start.x, 0.0, start.z);
    assert!(Vec2::new(at.x - want.x, at.z - want.z).length() < 0.05, "carried round to {want}, found {at}");
    assert!((at.y - start.y).abs() < 0.02, "{at} vs {start}");
    let now = app.world().get::<Transform>(body).unwrap().rotation;
    assert!((now * facing.inverse()).angle_between(Quat::from_rotation_y(angle)) < 0.02, "the body turned with it");
    assert!(state(&app, body).grounded);
}

#[test]
fn a_closing_door_pushes_a_body_out_of_its_way() {
    let mut app = movers_app("movers_door");
    floor(app.world_mut());
    // A panel 2 m wide, sliding 3 m along -X: closed, it spans x -1..1.
    let door = mover(app.world_mut(), "Gate", "door", Vec3::new(3.0, 1.25, 0.0), Vec3::new(1.0, 1.25, 0.15));
    let body = player(app.world_mut(), Vec3::new(0.0, 0.0, 0.0));
    step(&mut app, 10);
    ok(&mut app, "mover_go", json!({"name": "gate", "key": 1}));
    ride(&mut app, door, 1, 300, |app| assert_clear(app, body));
    let at = feet(&app, body);
    let radius = CharacterParams::default().radius;
    assert!(at.x < -1.0 - radius + 0.02, "pushed ahead of the leading edge: {at}");
    assert!(at.z.abs() < 0.05, "straight ahead: {at}");
    assert!(heard(&app).blocked.is_empty(), "{:?}", heard(&app).blocked);
}

/// A door that closes on a body standing against a wall behind it.
fn pinned(name: &str, def: &str) -> (App, Entity, Entity) {
    let mut app = movers_app(name);
    floor(app.world_mut());
    block(app.world_mut(), Vec3::new(-0.8, 0.0, -2.0), Vec3::new(-0.6, 3.0, 2.0));
    let door = mover(app.world_mut(), "Gate", def, Vec3::new(3.0, 1.25, 0.0), Vec3::new(1.0, 1.25, 0.15));
    let body = player(app.world_mut(), Vec3::new(0.0, 0.0, 0.0));
    step(&mut app, 10);
    ok(&mut app, "mover_go", json!({"name": "gate", "key": 1}));
    (app, door, body)
}

#[test]
fn a_stop_door_holds_still_against_a_pinned_body_and_goes_on_once_it_is_gone() {
    let (mut app, door, body) = pinned("movers_stop", "door");
    step(&mut app, 120);
    let s = mover_state_of(&app, door);
    assert!(s.blocked > 0.5 && s.to == 1 && s.t < 1.0, "held: {s:?}");
    assert_clear(&app, body);
    let blocked = &heard(&app).blocked;
    assert_eq!(blocked.len(), 1, "told once when the hold begins: {blocked:?}");
    assert_eq!((blocked[0].mover, blocked[0].body, blocked[0].damage), (door, body, 0.0));
    let line = ok(&mut app, "observe", json!({}));
    assert!(line.lines().any(|l| l.contains("Mover") && l.contains("held")), "{line}");
    assert!(journal(&app).iter().any(|l| l.contains("Gate held by")), "{:?}", journal(&app));
    app.world_mut().get_mut::<Transform>(body).unwrap().translation = Vec3::new(0.0, 0.0, 6.0);
    ride(&mut app, door, 1, 300, |_| {});
    assert_eq!(mover_state_of(&app, door).blocked, 0.0);
}

#[test]
fn a_reverse_door_goes_back_to_the_key_it_came_from() {
    let (mut app, door, body) = pinned("movers_reverse", "door_back");
    ride(&mut app, door, 0, 300, |app| assert_clear(app, body));
    let heard = heard(&app);
    assert_eq!(heard.blocked.len(), 1, "{:?}", heard.blocked);
    assert_eq!(heard.arrived.iter().map(|a| a.key).collect::<Vec<_>>(), [0]);
}

#[test]
fn a_crusher_holds_on_a_pinned_body_and_deals_its_damage_every_second() {
    let mut app = movers_app("movers_crush");
    floor(app.world_mut());
    // Its bottom comes down from 3.0 to 0.4: a body 1.8 m tall stands in the way.
    let press = mover(app.world_mut(), "Crusher", "press", Vec3::new(0.0, 3.5, 0.0), Vec3::new(1.0, 0.5, 1.0));
    let body = player(app.world_mut(), Vec3::new(0.0, 0.0, 0.0));
    step(&mut app, 10);
    ok(&mut app, "mover_go", json!({"name": "crusher", "key": 1}));
    // About 1.2 s to reach the head, then 2.5 s held: three crushes.
    step(&mut app, 222);
    assert_clear(&app, body);
    let blocked = &heard(&app).blocked;
    assert_eq!(blocked.iter().map(|b| b.damage).collect::<Vec<_>>(), [10.0, 10.0, 10.0], "{blocked:?}");
    assert!(blocked.iter().all(|b| b.mover == press && b.body == body));
    assert!(journal(&app).iter().any(|l| l.contains("Crusher crushes")), "{:?}", journal(&app));
    assert!(mover_state_of(&app, press).blocked > 2.0);
}

#[test]
fn a_ping_pong_mover_runs_by_itself_and_its_starts_stay_out_of_the_journal() {
    let mut app = movers_app("movers_loop");
    floor(app.world_mut());
    let shuttle = mover(app.world_mut(), "Shuttle", "shuttle", Vec3::new(10.0, 0.1, 0.0), Vec3::new(1.0, 0.1, 1.0));
    // 2 s a way, 0.5 s at each key: 1, 2, 1, 0 in 10 s.
    step(&mut app, 600);
    assert_eq!(heard(&app).arrived.iter().map(|a| a.key).collect::<Vec<_>>(), [1, 2, 1, 0]);
    assert_eq!(app.world().get::<Transform>(shuttle).unwrap().translation, Vec3::new(10.0, 0.1, 0.0));
    assert!(!journal(&app).iter().any(|l| l.contains("Shuttle")), "a loop's routine is not news: {:?}", journal(&app));
}

#[test]
fn a_snapshot_taken_mid_ride_goes_on_the_same() {
    let mut app = movers_app("movers_snapshot");
    floor(app.world_mut());
    mover(app.world_mut(), "Lift", "lift", Vec3::new(0.0, 0.1, 0.0), Vec3::new(1.0, 0.1, 1.0));
    player(app.world_mut(), Vec3::new(0.3, 0.25, 0.0));
    step(&mut app, 30);
    ok(&mut app, "mover_go", json!({"name": "lift", "key": 1}));
    step(&mut app, 75);
    let saved = ai_core::snapshot::capture(app.world_mut()).expect("capture");
    let run = |app: &mut App| {
        step(app, 150);
        let lift = app.world_mut().query_filtered::<Entity, With<Mover>>().single(app.world()).unwrap();
        let body = app.world_mut().query_filtered::<Entity, With<CharacterController>>().single(app.world()).unwrap();
        (mover_state_of(app, lift), app.world().get::<Transform>(lift).copied().unwrap(), feet(app, body))
    };
    let first = run(&mut app);
    ai_core::snapshot::restore(app.world_mut(), &saved, true).expect("restore");
    let second = run(&mut app);
    assert_eq!(first, second, "the restored ride ends where the first one did");
    assert!(first.0.from == 1 && first.0.to == 1, "{:?}", first.0);
}

#[test]
fn the_describer_says_where_a_mover_is_and_how_it_moves() {
    let mut app = movers_app("movers_describe");
    floor(app.world_mut());
    let lift = mover(app.world_mut(), "Lift", "lift", Vec3::new(0.0, 0.1, 0.0), Vec3::new(1.0, 0.1, 1.0));
    let id = format!("e{}", app.world().get::<ai_core::AiId>(lift).unwrap().0);
    step(&mut app, 2);
    let line = |app: &mut App| ok(app, "observe", json!({})).lines().find(|l| l.contains(&id)).map(str::to_owned).unwrap_or_default();
    assert!(line(&mut app).contains("Mover") && line(&mut app).contains("at key 0"), "{}", line(&mut app));
    ok(&mut app, "mover_go", json!({"id": id, "key": 1}));
    step(&mut app, 10);
    let moving = line(&mut app);
    assert!(moving.contains("key 0→1, moving up 1.2m/s"), "{moving}");
}

#[test]
fn mover_go_names_what_is_wrong_with_a_call() {
    let mut app = movers_app("movers_tool");
    floor(app.world_mut());
    mover(app.world_mut(), "Lift", "lift", Vec3::new(0.0, 0.1, 0.0), Vec3::new(1.0, 0.1, 1.0));
    mover(app.world_mut(), "Gate", "door", Vec3::new(5.0, 1.25, 0.0), Vec3::new(1.0, 1.25, 0.15));
    let crate_ = block(app.world_mut(), Vec3::new(8.0, 0.0, 0.0), Vec3::new(9.0, 1.0, 1.0));
    app.world_mut().entity_mut(crate_).insert(ai_core::AiKind::new("Crate"));
    step(&mut app, 2);
    let err = fails(&mut app, "mover_go", json!({"name": "lfit", "key": 1}));
    assert!(err.contains("no mover named `lfit`") && err.contains("did you mean `Lift`") && err.contains("Gate"), "{err}");
    let err = fails(&mut app, "mover_go", json!({"name": "lift", "key": 2}));
    assert!(err.contains("no key 2") && err.contains("0..=1"), "{err}");
    let crate_id = format!("e{}", app.world().get::<ai_core::AiId>(crate_).unwrap().0);
    let err = fails(&mut app, "mover_go", json!({"id": crate_id, "key": 1}));
    assert!(err.contains("is no mover"), "{err}");
    let err = fails(&mut app, "mover_go", json!({"key": 1}));
    assert!(err.contains("name the mover"), "{err}");
    let out = ok(&mut app, "mover_go", json!({"name": "lift", "key": 0}));
    assert!(out.contains("at key 0 already"), "{out}");
}

#[test]
fn the_condition_mover_arrived_holds_on_the_tick_a_mover_comes_to_rest() {
    let mut app = movers_app("movers_condition");
    floor(app.world_mut());
    let lift = mover(app.world_mut(), "Lift", "lift", Vec3::new(0.0, 0.1, 0.0), Vec3::new(1.0, 0.1, 1.0));
    step(&mut app, 2);
    let holds = |app: &App| app.world().resource::<ai_core::AiConditions>().0.get("mover_arrived").map(|f| f(app.world()));
    assert_eq!(holds(&app), Some(false));
    ok(&mut app, "mover_go", json!({"name": "lift", "key": 1}));
    let ticks = ride(&mut app, lift, 1, 400, |app| {
        let s = app.world().get::<MoverState>(lift).unwrap();
        assert_eq!(holds(app), Some(s.resting()), "{s:?}");
    });
    assert!(ticks > 100);
    step(&mut app, 1);
    assert_eq!(holds(&app), Some(false), "only on the tick it arrives");
}

#[test]
fn a_bad_mover_definition_is_an_error_naming_its_file_and_field() {
    let app = movers_app_with(
        "movers_bad",
        r#"{
            "one": (keys: [()]),
            "slow": (keys: [(), (at: (0.0, 1.0, 0.0))]),
            "still": (keys: [(), ()], speed: 1.0),
            "open": (keys: [(), (turn: (0.0, 90.0, 0.0))], turn_speed: 30.0, loop: Cycle),
            "first": (keys: [(time: 1.0), (at: (1.0, 0.0, 0.0))], speed: 1.0),
            "hurt": (keys: [(), (at: (1.0, 0.0, 0.0))], speed: 1.0, blocked: Crush(damage: 0.0)),
        }"#,
    );
    let errors = errors(&app);
    for needle in [
        "keys: a mover needs at least 2 keys",
        "keys[1]: the way from key 0 takes no time; it moves 1.00 m: give the mover a `speed`",
        "keys[1]: the way from key 0 takes no time; it stays where key 0 is: give key 1 a `time`",
        "loop: Cycle goes from the last key back to key 0 at once, so keys[1] must be where key 0 is",
        "keys[0].time",
        "Crush damage must be above 0",
    ] {
        assert!(errors.iter().any(|e| e.contains("content/test.mover.ron") && e.contains(needle)), "{needle}: {errors:?}");
    }
}

#[test]
fn a_typo_in_a_mover_definition_is_an_error() {
    let app = movers_app_with("movers_typo", r#"{ "typo": (keys: [(), (at: (1.0, 0.0, 0.0))], sped: 1.0) }"#);
    let errors = errors(&app);
    assert!(errors.iter().any(|e| e.contains("content/test.mover.ron") && e.contains("sped")), "{errors:?}");
}
