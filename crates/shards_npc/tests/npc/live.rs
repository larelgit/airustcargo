//! Live tags (N8d, the design's 12.2 run 8): an option's `forbid_while` and `solo_forbid_while`
//! refuse it while a tag is live: a combat object of the NPC's own (anywhere), a zone near the
//! target (within the plan's `zone_margin`), or a zone another NPC's attack in flight will lay at
//! the target's feet. Test content in `assets/content/live.*.ron`: `live_zone` is a 2 m capture
//! zone (`capture`, `long`), telegraphed 0.5 s, active 1 s, fading 0.5 s. Every test runs with the
//! engine's rules on, so a step that breaks one fails.

use bevy::prelude::*;
use serde_json::{json, Value};
use shards_action::{despawn_objects_of, spawn_objects, ActionState, CombatObject, SpawnDef};
use shards_npc::prelude::*;

use crate::common::*;

/// `npc_schedule` for `who`, parsed.
fn report(app: &mut App, who: Entity, extra: Value) -> Value {
    let mut args = json!({"id": id(app, who)});
    if let (Some(args), Some(extra)) = (args.as_object_mut(), extra.as_object()) {
        args.extend(extra.clone());
    }
    ok(app, "npc_schedule", args).result
}

/// The report's line for option `name`.
fn option<'a>(report: &'a Value, name: &str) -> &'a Value {
    report["options"].as_array().unwrap().iter().find(|o| o["name"] == name).unwrap_or_else(|| panic!("no option {name} in {report}"))
}

/// Why option `name` is refused, or "" when it is not.
fn refused(report: &Value, name: &str) -> String {
    option(report, name)["refused"].as_str().unwrap_or_default().to_owned()
}

/// A `live_zone` laid for `owner` at `at`.
fn lay(app: &mut App, owner: Entity, at: Vec3) {
    let def: SpawnDef = ron::from_str(r#"(prefab: ["zone"], action: ["live_zone"])"#).unwrap();
    assert_eq!(spawn_objects(app.world_mut(), owner, "probe", &def, Some((at, N))).len(), 1);
}

/// A caster that never acts (no brain), of the monsters, far off: the owner of the zones a test lays.
fn caster(app: &mut App) -> Entity {
    fighter(app.world_mut(), "Caster", "live", "monsters", Vec3::new(0.0, 0.0, -30.0), S)
}

/// An NPC with `plan` 1.2 m south of the hero, facing it, on director `sched_arena` (two tokens).
fn judged(app: &mut App, plan: &str, at: Vec3) -> Entity {
    npc(app.world_mut(), "live", Npc::new("grunt", "grunt").with_plan(plan).with_director("sched_arena"), at, (-at).normalize())
}

/// The capture zones whose hit window is still to come or open (the telegraph and the active phase).
fn live_zones(app: &mut App) -> usize {
    let world = app.world_mut();
    let mut q = world.query::<(&CombatObject, &ActionState)>();
    q.iter(world).filter(|(_, s)| s.current.as_ref().is_some_and(|r| r.action == "live_zone" && r.t < 1.5 - 1e-4)).count()
}

/// The jabs `who` started, by tick.
fn started(app: &App, who: Entity, action: &str) -> Vec<u64> {
    app.world().resource::<Started>().0.iter().filter(|(_, e, a)| *e == who && a == action).map(|(t, ..)| *t).collect()
}

fn tick(app: &App) -> u64 {
    app.world().resource::<ai_core::SimTick>().0
}

/// A capture zone at the target's feet refuses the option while it telegraphs and while it is
/// active, as the report says and as the planner does; in its fade it may be chosen again.
#[test]
fn a_capture_at_the_target_refuses_the_option_until_its_fade() {
    let mut app = app_seeded(5, record_starts);
    player(app.world_mut(), 0, Vec3::ZERO);
    let owner = caster(&mut app);
    let e = judged(&mut app, "live_trap", Vec3::new(0.0, 0.0, 1.2));
    lay(&mut app, owner, Vec3::ZERO);
    let laid = tick(&app);
    step(&mut app, 10);
    let telegraph = report(&mut app, e, json!({}));
    assert_eq!(refused(&telegraph, "trap"), "live: capture is live (forbid_while)", "{telegraph}");
    assert_eq!(telegraph["live"], json!(["capture", "long"]));
    step(&mut app, 50);
    let active = report(&mut app, e, json!({}));
    assert_eq!(refused(&active, "trap"), "live: capture is live (forbid_while)", "{active}");
    // Through the telegraph and the active phase (1.5 s, 90 ticks) it never jabs.
    step(&mut app, 35);
    let fade = report(&mut app, e, json!({}));
    assert_eq!(option(&fade, "trap")["eligible"], true, "{fade}");
    assert!(fade.get("live").is_none(), "{fade}");
    let first = started(&app, e, "jab").first().copied();
    assert!(first.is_none_or(|t| t > laid + 90), "a jab at tick {first:?}, the zone laid at {laid}");
    step(&mut app, 120);
    assert!(!started(&app, e, "jab").is_empty(), "it jabs once the zone fades");
    // The what-if sees through it: no live tag, and it may be chosen; a named one refuses.
    lay(&mut app, owner, Vec3::ZERO);
    step(&mut app, 5);
    let none = report(&mut app, e, json!({"live": []}));
    assert_eq!(none["what_if"], true);
    assert!(!refused(&none, "trap").starts_with("live:"), "{none}");
    let named = report(&mut app, e, json!({"live": ["capture"]}));
    assert_eq!(refused(&named, "trap"), "live: capture is live (forbid_while)");
    let who = id(&app, e);
    assert_eq!(fails(&mut app, "npc_schedule", json!({"id": who, "live": [""]})), "live lists tags, e.g. live=[\"capture\"] (an empty tag names nothing)");
}

/// A zone of any owner counts when the target stands within the plan's `zone_margin` of its
/// footprint: a 2 m zone 2.8 m off refuses with a metre of margin and not without; 3.5 m off, neither.
#[test]
fn a_zone_near_the_target_counts_within_the_margin() {
    let mut app = app_seeded(5, record_starts);
    player(app.world_mut(), 0, Vec3::ZERO);
    let owner = caster(&mut app);
    let margin = judged(&mut app, "live_margin", Vec3::new(0.0, 0.0, 1.2));
    let none = judged(&mut app, "live_trap", Vec3::new(-1.2, 0.0, 0.0));
    lay(&mut app, owner, Vec3::new(2.8, 0.0, 0.0));
    step(&mut app, 2);
    let near = report(&mut app, margin, json!({}));
    assert_eq!(refused(&near, "trap"), "live: capture is live (forbid_while)", "{near}");
    let without = report(&mut app, none, json!({}));
    assert!(!refused(&without, "trap").starts_with("live:"), "no margin: 2.8 m is outside a 2 m zone: {without}");
    let owner_id = aid(&app, owner);
    despawn_objects_of(app.world_mut(), owner_id);
    lay(&mut app, owner, Vec3::new(3.5, 0.0, 0.0));
    step(&mut app, 2);
    let far = report(&mut app, margin, json!({}));
    assert!(!refused(&far, "trap").starts_with("live:"), "3.5 m is outside 2 m and a metre: {far}");
}

/// An NPC's own zone counts wherever it is: 20 m away it refuses `solo_forbid_while: ["long"]`
/// while one player lives, and not once a second one does.
#[test]
fn an_own_zone_far_off_refuses_a_solo_forbid_only_alone() {
    let mut app = app_seeded(5, record_starts);
    player(app.world_mut(), 0, Vec3::ZERO);
    let e = judged(&mut app, "live_solo", Vec3::new(0.0, 0.0, 1.2));
    lay(&mut app, e, Vec3::new(20.0, 0.0, 0.0));
    step(&mut app, 2);
    let alone = report(&mut app, e, json!({}));
    assert_eq!(refused(&alone, "vines"), "live: long is live with one player (solo_forbid_while)", "{alone}");
    assert_eq!(alone["players"], 1);
    let two = report(&mut app, e, json!({"players": 2}));
    assert!(!refused(&two, "vines").starts_with("live:"), "{two}");
    player(app.world_mut(), 1, Vec3::new(-3.0, 0.0, 0.0));
    step(&mut app, 1);
    let together = report(&mut app, e, json!({}));
    assert_eq!(together["players"], 2);
    assert!(!refused(&together, "vines").starts_with("live:"), "{together}");
    assert_eq!(together["live"], json!(["capture", "long"]), "still live, but only a solo player is spared it");
}

/// Another NPC's attack in flight whose action lays a capture zone at the target (`Spawn` with
/// `aim: AtTarget`) makes `capture` live from its press until the zone fades; a swing tagged
/// `capture` that lays nothing does not.
#[test]
fn an_attack_in_flight_that_lays_a_capture_at_the_target_refuses_from_its_press() {
    let mut app = app_seeded(5, record_starts);
    player(app.world_mut(), 0, Vec3::ZERO);
    let e = judged(&mut app, "live_trap", Vec3::new(-1.2, 0.0, 0.0));
    let boss = judged(&mut app, "live_caster", Vec3::new(0.0, 0.0, 3.0));
    let pressed = |app: &App| app.world().get::<AttackToken>(boss).unwrap().held.as_ref().is_some_and(|g| g.pressed.is_some());
    let mut press = None;
    for _ in 0..600 {
        step(&mut app, 1);
        if pressed(&app) {
            press = Some(tick(&app));
            break;
        }
        // Before the press nothing is live.
        let before = report(&mut app, e, json!({}));
        assert!(!refused(&before, "trap").starts_with("live:"), "{before}");
    }
    let press = press.expect("the boss presses its cast");
    // From the press until the zone it lays fades, the trap is refused on every tick: first for the
    // cast in flight, then for the zone. (The boss's option has a 5 s cooldown: it casts no other.)
    let mut zone_seen = false;
    while !zone_seen || live_zones(&mut app) > 0 {
        let now = report(&mut app, e, json!({}));
        assert_eq!(refused(&now, "trap"), "live: capture is live (forbid_while)", "{} ticks after the press: {now}", tick(&app) - press);
        zone_seen |= live_zones(&mut app) > 0;
        step(&mut app, 1);
        assert!(tick(&app) < press + 400, "never fades");
    }
    // 0.4 s to the spawn, then 1.5 s of telegraph and active phase.
    assert!(tick(&app) - press >= 110, "refused for {} ticks from the press", tick(&app) - press);
    let faded = report(&mut app, e, json!({}));
    assert!(!refused(&faded, "trap").starts_with("live:") && faded.get("live").is_none(), "{faded}");

    // A capture swing lays nothing: the trap is never refused for it.
    let mut app = app_seeded(5, record_starts);
    player(app.world_mut(), 0, Vec3::ZERO);
    let e = judged(&mut app, "live_trap", Vec3::new(-1.2, 0.0, 0.0));
    let grabber = judged(&mut app, "live_grabber", Vec3::new(0.0, 0.0, 1.2));
    let mut in_flight = 0;
    for _ in 0..300 {
        step(&mut app, 1);
        in_flight += usize::from(app.world().get::<AttackToken>(grabber).unwrap().held.as_ref().is_some_and(|g| g.in_flight()));
        let now = report(&mut app, e, json!({}));
        assert!(!refused(&now, "trap").starts_with("live:"), "{now}");
    }
    assert!(in_flight > 30 && !started(&app, grabber, "live_grab").is_empty(), "the grab was in flight for {in_flight} ticks");
}

/// Two casters with one capture option, granted on one target and deciding on one tick: only the
/// first in `AiId` order presses it (the planner's running list of attacks in flight), and at most
/// one capture zone is live on the target at any tick.
#[test]
fn two_casters_deciding_together_lay_one_capture_at_a_time() {
    let mut app = app_seeded(5, record_starts);
    player(app.world_mut(), 0, Vec3::ZERO);
    let first = judged(&mut app, "live_caster", Vec3::new(0.0, 0.0, 3.0));
    let second = judged(&mut app, "live_caster", Vec3::new(3.0, 0.0, 0.0));
    assert!(aid(&app, first) < aid(&app, second));
    let pressed = |app: &App, e: Entity| app.world().get::<AttackToken>(e).unwrap().held.as_ref().is_some_and(|g| g.pressed.is_some());
    let mut first_press = None;
    for _ in 0..1200 {
        step(&mut app, 1);
        assert!(live_zones(&mut app) <= 1, "two capture zones at tick {}", tick(&app));
        if first_press.is_none() && (pressed(&app, first) || pressed(&app, second)) {
            first_press = Some((pressed(&app, first), pressed(&app, second)));
        }
    }
    assert_eq!(first_press, Some((true, false)), "on the tick both decide, only the first presses");
    let casts = started(&app, first, "live_cast").len() + started(&app, second, "live_cast").len();
    assert!(casts >= 3, "{casts} casts in 20 s");
}
