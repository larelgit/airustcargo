//! Spawned combat objects (B4): spawn patterns, projectiles, zones, credit, owners, `then`, the
//! content check against runaway spawners, the caps and the prefab cache.

use ai_core::{AiId, AiIndex, SimTick};
use bevy::prelude::*;
use serde_json::json;
use shards_action::objects::{
    despawn_objects_of, live_tags_at, objects_of, spawn_objects, CombatObject, ProjectileFlight, Repeats, SpawnDef,
};
use shards_action::zone::zones_threatening;
use shards_action::{hit_zones, kill, threat_indicators, ActionDef, ActionEnded, ActionSound, ActionStarted, Damaged, Died, Invulnerable, Team};
use shards_content::{ContentDb, ContentKinds, PrefabCache, PrefabDef};
use shards_core::Intent;

use crate::common::*;

/// What the observers saw: `(tick, attacker, target, action, amount, source)` of every `Damaged`,
/// `(tick, entity, sound)` of every `ActionSound`, `(killer, source)` of every `Died`, and
/// `{tick} started {action}` / `{tick} ended {action}` in order.
#[derive(Resource, Default)]
struct Seen {
    damaged: Vec<(u64, Entity, Entity, String, f32, Option<Entity>)>,
    sounds: Vec<(u64, Entity, String)>,
    died: Vec<(Option<Entity>, Option<Entity>)>,
    log: Vec<String>,
}

fn record(app: &mut App) {
    app.init_resource::<Seen>();
    app.add_observer(|e: On<ActionStarted>, tick: Res<SimTick>, mut seen: ResMut<Seen>| {
        seen.log.push(format!("{} started {}", tick.0, e.event().action));
    });
    app.add_observer(|e: On<ActionEnded>, tick: Res<SimTick>, mut seen: ResMut<Seen>| {
        seen.log.push(format!("{} ended {}", tick.0, e.event().action));
    });
    app.add_observer(|e: On<Damaged>, tick: Res<SimTick>, mut seen: ResMut<Seen>| {
        let e = e.event();
        seen.damaged.push((tick.0, e.attacker, e.target, e.action.clone(), e.amount, e.source));
    });
    app.add_observer(|e: On<ActionSound>, tick: Res<SimTick>, mut seen: ResMut<Seen>| {
        let e = e.event();
        seen.sounds.push((tick.0, e.entity, e.sound.clone()));
    });
    app.add_observer(|e: On<Died>, mut seen: ResMut<Seen>| {
        let e = e.event();
        seen.died.push((e.killer, e.source));
    });
}

fn arena() -> App {
    app_with(record)
}

fn seen(app: &App) -> &Seen {
    app.world().resource::<Seen>()
}

/// The ticks `target` was damaged on.
fn hit_ticks(app: &App, target: Entity) -> Vec<u64> {
    seen(app).damaged.iter().filter(|d| d.2 == target).map(|d| d.0).collect()
}

fn spawn_def(ron: &str) -> SpawnDef {
    ron::Options::default()
        .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
        .from_str(ron)
        .unwrap_or_else(|e| panic!("{e}: {ron}"))
}

fn spawn(app: &mut App, spawner: Entity, ron: &str) -> Vec<Entity> {
    spawn_objects(app.world_mut(), spawner, "test", &spawn_def(ron), None)
}

/// Spawns `ron` at `point`, facing `facing`.
fn spawn_at(app: &mut App, spawner: Entity, ron: &str, point: Vec3, facing: Vec3) -> Vec<Entity> {
    spawn_objects(app.world_mut(), spawner, "test", &spawn_def(ron), Some((point, facing)))
}

fn tick(app: &App) -> u64 {
    app.world().resource::<SimTick>().0
}

fn alive(app: &App, e: Entity) -> bool {
    app.world().get_entity(e).is_ok()
}

fn object(app: &App, e: Entity) -> CombatObject {
    app.world().get::<CombatObject>(e).unwrap().clone()
}

fn flight(app: &App, e: Entity) -> ProjectileFlight {
    app.world().get::<ProjectileFlight>(e).unwrap().clone()
}

/// Every combat object in the world, in `AiId` order: its id and the action it started with.
fn all_objects(app: &mut App) -> Vec<(AiId, String)> {
    let world = app.world_mut();
    let mut q = world.query::<(&AiId, &CombatObject)>();
    let mut list: Vec<(AiId, String)> = q.iter(world).map(|(id, o)| (*id, o.action.clone())).collect();
    list.sort();
    list
}

fn ai_id(app: &App, e: Entity) -> AiId {
    *app.world().get::<AiId>(e).unwrap()
}

fn lock(app: &mut App, who: Entity, on: Entity) {
    let target = ai_id(app, on);
    app.world_mut().get_mut::<Intent>(who).unwrap().target = Some(target);
}

/// Degrees of `v` from north on the ground, positive to the east (right of north).
fn bearing(v: Vec3) -> f32 {
    ops::atan2(v.x, -v.z).to_degrees()
}

// --- spawning ----------------------------------------------------------------------------------

#[test]
fn spawn_indices_pick_prefab_and_action() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let spawned = spawn(
        &mut app,
        hero,
        r#"(prefab: ["t_mark", "t_zone"], action: ["t_wait", "t_once", "t_wait"], count: 4, pattern: Line(start: 1, step: 1))"#,
    );
    assert_eq!(spawned.len(), 4);
    let kinds: Vec<(String, String)> = spawned
        .iter()
        .map(|&e| (app.world().get::<ai_core::AiKind>(e).unwrap().0.to_string(), object(&app, e).action))
        .collect();
    let want = [("mark", "t_wait"), ("zone", "t_once"), ("zone", "t_wait"), ("zone", "t_wait")];
    assert_eq!(kinds, want.map(|(k, a)| (k.to_owned(), a.to_owned())), "the last entry of a list repeats");
    for &e in &spawned {
        let o = object(&app, e);
        assert_eq!(o.owner, ai_id(&app, hero), "the spawner owns them");
        assert_eq!(app.world().get::<Team>(e), app.world().get::<Team>(hero), "with the owner's team");
        assert_eq!(action(&app, e), o.action, "started at once: no delay");
    }
    let ids: Vec<u32> = spawned.iter().map(|&e| ai_id(&app, e).0).collect();
    assert!(ids.windows(2).all(|w| w[1] == w[0] + 1), "ids in index order: {ids:?}");
    assert!(lines(&app, "spawns 4 x t_wait").len() == 1, "{:?}", journal(&app));
}

#[test]
fn fan_spread_turns_left_first() {
    // `spread * (i - (count - 1) / 2)` degrees, a positive angle turning left: the first index is
    // the rightmost, the middle one straight ahead, the last the leftmost.
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let bolts = spawn(&mut app, hero, r#"(prefab: ["t_bolt"], action: ["t_fly_plain"], at: (forward: 1, up: 1), count: 3, pattern: Fan(spread: 10))"#);
    let bearings: Vec<f32> = bolts.iter().map(|&b| bearing(flight(&app, b).dir)).collect();
    for (got, want) in bearings.iter().zip([10.0, 0.0, -10.0]) {
        assert!((got - want).abs() < 1e-3, "bearings {bearings:?}");
    }
    for &b in &bolts {
        assert!((feet(&app, b) - Vec3::new(0.0, 1.0, -1.0)).length() < 1e-5, "all at `at`");
        assert!(flight(&app, b).dir.y.abs() < 1e-6, "aimed along the facing: flat");
    }
}

#[test]
fn line_and_radial_place_by_step() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let line = spawn(&mut app, hero, r#"(prefab: ["t_mark"], action: ["t_wait"], count: 3, pattern: Line(start: 2, step: 1.5))"#);
    let at: Vec<Vec3> = line.iter().map(|&e| feet(&app, e)).collect();
    for (got, z) in at.iter().zip([-2.0, -3.5, -5.0]) {
        assert!((*got - Vec3::new(0.0, 0.0, z)).length() < 1e-4, "{at:?}");
    }
    // Four lines turned 90° left each (N, W, S, E), two on each at 1 and 3 m, line by line.
    let radial =
        spawn(&mut app, hero, r#"(prefab: ["t_mark"], action: ["t_wait"], count: 2, pattern: Radial(lines: 4, start: 1, step: 2))"#);
    let at: Vec<Vec3> = radial.iter().map(|&e| feet(&app, e)).collect();
    let want = [(0.0, -1.0), (0.0, -3.0), (-1.0, 0.0), (-3.0, 0.0), (0.0, 1.0), (0.0, 3.0), (1.0, 0.0), (3.0, 0.0)];
    assert_eq!(at.len(), 8);
    for (got, (x, z)) in at.iter().zip(want) {
        assert!((*got - Vec3::new(x, 0.0, z)).length() < 1e-4, "{at:?}");
    }
}

#[test]
fn random_turn_comes_from_simrng() {
    let turned = |seed_bump: bool| {
        let mut app = arena();
        if seed_bump {
            app.world_mut().resource_mut::<ai_core::SimSeed>().0 += 1;
        }
        let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
        let marks = spawn(
            &mut app,
            hero,
            r#"(prefab: ["t_mark"], action: ["t_wait"], count: 1, random_turn: true, pattern: Radial(lines: 4, start: 2, step: 0))"#,
        );
        marks.iter().map(|&m| bearing(feet(&app, m))).collect::<Vec<f32>>()
    };
    let (a, b, other) = (turned(false), turned(false), turned(true));
    assert_eq!(a, b, "the same seed and tick turn it the same");
    assert_ne!(a, other, "another seed turns it otherwise");
    assert!(a[0].abs() > 1e-3, "turned: {a:?}");
    // The pattern turns as a whole: its lines stay 90° apart.
    for w in a.windows(2) {
        let gap = (w[0] - w[1]).rem_euclid(360.0);
        assert!((gap - 90.0).abs() < 1e-2, "{a:?}");
    }
}

#[test]
fn objects_act_from_the_next_tick() {
    // A zone laid by an action's effect on tick N over a brute: it hits on tick N + 1.
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let brute = brute(app.world_mut(), Vec3::new(0.0, 0.0, -1.5), S);
    assert!(shards_action::force_action(app.world_mut(), hero, "t_lay"));
    let mut laid = None;
    for _ in 0..20 {
        step(&mut app, 1);
        if let Some((id, _)) = all_objects(&mut app).first() {
            laid = Some((tick(&app), *id));
            break;
        }
    }
    let (n, _) = laid.expect("the zone was laid");
    assert_eq!(hp(&app, brute), 100.0, "not on the tick it was spawned");
    step(&mut app, 1);
    assert_eq!(hit_ticks(&app, brute), [n + 1]);
}

#[test]
fn delayed_objects_start_late() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let marks = spawn(&mut app, hero, r#"(prefab: ["t_mark"], action: ["t_wait"], count: 3, delay: 0.25, pattern: Line(start: 2, step: 1))"#);
    let starts = |app: &App| marks.iter().map(|&m| action(app, m) == "t_wait").collect::<Vec<_>>();
    assert_eq!(starts(&app), [true, false, false], "index 0 at once");
    assert!((object(&app, marks[1]).start_in - 0.25).abs() < 1e-6);
    // 0.25 s: 15 ticks of waiting, then it starts.
    step(&mut app, 14);
    assert_eq!(starts(&app), [true, false, false]);
    step(&mut app, 1);
    assert_eq!(starts(&app), [true, true, false]);
    step(&mut app, 15);
    assert_eq!(starts(&app), [true, true, true]);
}

// --- projectiles -------------------------------------------------------------------------------

#[test]
fn projectile_flies_and_stops_at_walls() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    wall(app.world_mut(), Vec3::new(-3.0, 0.0, -5.5), Vec3::new(3.0, 3.0, -5.0));
    let bolt = spawn(&mut app, hero, r#"(prefab: ["t_bolt"], action: ["t_fly"], at: (forward: 0.5, up: 1))"#)[0];
    step(&mut app, 1);
    // 15 m/s: 0.25 m a tick, from the tick after its spawn.
    assert!((feet(&app, bolt).z - -0.75).abs() < 1e-4, "{}", feet(&app, bolt));
    step(&mut app, 4);
    assert!((feet(&app, bolt).z - -1.75).abs() < 1e-4);
    let mut stopped = None;
    for _ in 0..30 {
        step(&mut app, 1);
        if flight(&app, bolt).dir == Vec3::ZERO {
            stopped = Some(feet(&app, bolt));
            break;
        }
    }
    let at = stopped.expect("the wall stopped it");
    assert!((at.z - -4.98).abs() < 1e-3 && (at.y - 1.0).abs() < 1e-4, "just off the wall: {at}");
    assert_eq!(action(&app, bolt), "t_burst", "where it stops it runs its impact");
    step(&mut app, 3);
    assert!((feet(&app, bolt).z - -4.98).abs() < 1e-3, "it stays stopped");
}

#[test]
fn homing_turns_by_weight() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let brute = brute(app.world_mut(), Vec3::new(6.0, 0.0, -16.0), S);
    lock(&mut app, hero, brute);
    let bolt = spawn(&mut app, hero, r#"(prefab: ["t_bolt_homing"], action: ["t_fly_plain"], at: (up: 0.9))"#)[0];
    assert_eq!(flight(&app, bolt).target, Some(ai_id(&app, brute)), "it flies at the spawner's target");
    let start = feet(&app, bolt);
    step(&mut app, 1);
    // One tick turns it by homing * dt (3/60) of the angle to the aim point, then it flies.
    let want = (Vec3::new(6.0, 0.9, -16.0) - start).normalize();
    let turned = flight(&app, bolt).dir;
    let angle = N.angle_between(want);
    assert!((N.angle_between(turned) - angle * 3.0 / 60.0).abs() < 1e-4, "{turned}");
    assert!((turned.angle_between(want) - angle * (1.0 - 3.0 / 60.0)).abs() < 1e-4);
    assert!((turned.length() - 1.0).abs() < 1e-5);
    assert!((feet(&app, bolt) - (start + turned * 0.25)).length() < 1e-4);
    // It keeps turning toward the target: the angle off the line to its aim point shrinks.
    let off = |app: &App| flight(app, bolt).dir.angle_between(Vec3::new(6.0, 0.9, -16.0) - feet(app, bolt));
    let mut last = off(&app);
    for _ in 0..30 {
        step(&mut app, 1);
        let now = off(&app);
        assert!(now < last, "{now} after {last}");
        last = now;
    }
}

#[test]
fn projectile_ends_at_lifetime_without_impact() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let bolt = spawn(&mut app, hero, r#"(prefab: ["t_bolt_short"], action: ["t_fly"], at: (up: 1))"#)[0];
    step(&mut app, 29);
    assert!(alive(&app, bolt) && action(&app, bolt) == "t_fly");
    step(&mut app, 1);
    assert!(!alive(&app, bolt), "0.5 s of flight and it is gone");
    assert!(all_objects(&mut app).is_empty(), "no impact: nothing laid");
}

#[test]
fn first_target_consumes_it_whatever_the_outcome() {
    // A brute in god mode: the hit is ignored, and the bolt is spent all the same.
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let brute = brute(app.world_mut(), Vec3::new(0.0, 0.0, -4.0), S);
    app.world_mut().entity_mut(brute).insert(Invulnerable);
    let bolt = spawn(&mut app, hero, r#"(prefab: ["t_bolt"], action: ["t_fly"], at: (forward: 0.5, up: 1))"#)[0];
    step(&mut app, 30);
    let flight: Vec<f32> = seen(&app).damaged.iter().filter(|d| d.3 == "t_fly").map(|d| d.4).collect();
    assert_eq!(flight, [0.0], "the hit, ignored");
    assert!(!alive(&app, bolt) || action(&app, bolt) == "t_burst", "it impacted");
    assert!(all_objects(&mut app).iter().any(|(_, a)| a == "t_puddle"), "its impact laid a puddle");

    // Two brutes in one tick's flight (2 m a tick): the first along the way takes it, whatever
    // their ids; a piercing bolt hits both.
    for (prefab, hit) in [("t_bolt_fast", 1), ("t_bolt_pierce", 2)] {
        let mut app = arena();
        let hero = crate::common::hero(app.world_mut(), "hero", Vec3::ZERO, N);
        let far = crate::common::brute(app.world_mut(), Vec3::new(0.0, 0.0, -4.4), S);
        let near = crate::common::brute(app.world_mut(), Vec3::new(0.0, 0.0, -3.5), S);
        spawn(&mut app, hero, &format!(r#"(prefab: ["{prefab}"], action: ["t_fly_plain"], at: (forward: 0.5, up: 1))"#));
        step(&mut app, 40);
        let targets: Vec<Entity> = seen(&app).damaged.iter().map(|d| d.2).collect();
        match hit {
            1 => assert_eq!(targets, [near], "only the nearer one, though {far:?} has the lower id"),
            _ => assert_eq!(targets.len(), 2, "{targets:?}"),
        }
    }
}

#[test]
fn a_fast_projectile_hits_what_it_passes_between_two_ticks() {
    // 120 m/s: 2 m a tick. The body stands between two end positions; the swept segment meets it.
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let brute = brute(app.world_mut(), Vec3::new(0.0, 0.0, -3.5), S);
    let bolt = spawn(&mut app, hero, r#"(prefab: ["t_bolt_fast"], action: ["t_fly_plain"], at: (forward: 0.5, up: 1))"#)[0];
    step(&mut app, 1);
    assert!((feet(&app, bolt).z - -2.5).abs() < 1e-4);
    step(&mut app, 1);
    assert_eq!(hit_ticks(&app, brute).len(), 1, "swept from -2.5 to -4.5 through the body at -3.5");
}

#[test]
fn impact_runs_its_action_on_the_ground() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    wall(app.world_mut(), Vec3::new(-3.0, 0.0, -5.5), Vec3::new(3.0, 3.0, -5.0));
    spawn(&mut app, hero, r#"(prefab: ["t_bolt"], action: ["t_fly"], at: (forward: 0.5, up: 1.5))"#);
    step(&mut app, 25);
    let world = app.world_mut();
    let mut q = world.query::<(&CombatObject, &Transform)>();
    let puddle: Vec<Vec3> = q.iter(world).filter(|(o, _)| o.action == "t_puddle").map(|(_, t)| t.translation).collect();
    assert_eq!(puddle.len(), 1, "the burst laid one puddle");
    assert!(puddle[0].y.abs() < 1e-3 && (puddle[0].z - -4.98).abs() < 1e-3, "on the floor at the wall: {}", puddle[0]);
}

#[test]
fn the_impact_action_hits_from_the_next_tick() {
    // The blast hits a brute by the wall that the flight passes by: from the tick after the impact.
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    wall(app.world_mut(), Vec3::new(-3.0, 0.0, -5.5), Vec3::new(3.0, 3.0, -5.0));
    let brute = brute(app.world_mut(), Vec3::new(1.6, 0.0, -4.2), S);
    let bolt = spawn(&mut app, hero, r#"(prefab: ["t_bolt"], action: ["t_fly_blast"], at: (forward: 0.5, up: 1))"#)[0];
    let mut impact = None;
    for _ in 0..30 {
        step(&mut app, 1);
        if action(&app, bolt) == "t_blast" {
            impact = Some(tick(&app));
            break;
        }
    }
    let n = impact.expect("it hit the wall");
    assert!(hit_ticks(&app, brute).is_empty(), "nothing on the impact tick");
    step(&mut app, 1);
    assert_eq!(hit_ticks(&app, brute), [n + 1]);
    assert_eq!(seen(&app).damaged[0].3, "t_blast");
}

#[test]
fn a_bolt_does_not_hit_on_its_spawn_tick() {
    // Shot by an action's effect from inside reach of the brute: the first hit test is the sweep
    // of the next tick, which starts at the spawn point.
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let brute = brute(app.world_mut(), Vec3::new(0.0, 0.0, -1.0), S);
    assert!(shards_action::force_action(app.world_mut(), hero, "t_shoot"));
    let mut shot = None;
    for _ in 0..20 {
        step(&mut app, 1);
        if !all_objects(&mut app).is_empty() {
            shot = Some(tick(&app));
            break;
        }
    }
    let n = shot.expect("shot");
    assert!(hit_ticks(&app, brute).is_empty());
    step(&mut app, 1);
    assert_eq!(hit_ticks(&app, brute), [n + 1]);
}

#[test]
fn an_owner_without_a_team_is_not_hit_by_its_own_bolt() {
    let mut app = arena();
    let loner = fighter(app.world_mut(), "Loner", "hero", "x", Vec3::ZERO, N);
    app.world_mut().entity_mut(loner).remove::<Team>();
    let target = fighter(app.world_mut(), "Other", "dummy", "x", Vec3::new(0.0, 0.0, -4.0), S);
    app.world_mut().entity_mut(target).remove::<Team>();
    // Shot from inside its own body.
    spawn(&mut app, loner, r#"(prefab: ["t_bolt"], action: ["t_fly_plain"], at: (up: 1))"#);
    step(&mut app, 30);
    let targets: Vec<Entity> = seen(&app).damaged.iter().map(|d| d.2).collect();
    assert_eq!(targets, [target], "team-less bodies are fair game, but never the owner");
    assert_eq!(hp(&app, loner), 100.0);
}

#[test]
fn credit_goes_to_the_owner() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let brute = brute(app.world_mut(), Vec3::new(0.0, 0.0, -3.0), S);
    app.world_mut().get_mut::<shards_action::Health>(brute).unwrap().current = 5.0;
    let bolt = spawn(&mut app, hero, r#"(prefab: ["t_bolt"], action: ["t_fly_plain"], at: (forward: 0.5, up: 1))"#)[0];
    step(&mut app, 20);
    let d = &seen(&app).damaged[0];
    assert_eq!((d.1, d.2, d.5), (hero, brute, Some(bolt)), "the owner hit, with the bolt");
    assert_eq!(seen(&app).died, [(Some(hero), Some(bolt))], "and killed");
    let line = lines(&app, "died");
    assert!(line[0].ends_with(&format!("{} died (killed by {} with t_fly_plain)", id(&app, brute), id(&app, hero))), "{line:?}");
}

// --- zones -------------------------------------------------------------------------------------

#[test]
fn zone_repeats_per_target() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::new(0.0, 0.0, 8.0), N);
    let a = brute(app.world_mut(), Vec3::new(0.5, 0.0, 0.0), S);
    let b = brute(app.world_mut(), Vec3::new(6.0, 0.0, 0.0), S);
    let zone = spawn_at(&mut app, hero, r#"(prefab: ["t_zone"], action: ["t_puddle"])"#, Vec3::ZERO, N)[0];
    let n = tick(&app);
    step(&mut app, 10);
    app.world_mut().get_mut::<Transform>(b).unwrap().translation = Vec3::new(-0.6, 0.0, 0.0);
    step(&mut app, 100);
    // Every 0.8 s (48 ticks) per target, each from its own first hit.
    assert_eq!(hit_ticks(&app, a), [n + 1, n + 49, n + 97]);
    assert_eq!(hit_ticks(&app, b), [n + 11, n + 59, n + 107]);
    let marks = &app.world().get::<Repeats>(zone).unwrap().0;
    assert_eq!(marks.iter().map(|m| m.target).collect::<Vec<_>>(), [ai_id(&app, a), ai_id(&app, b)], "sorted by target");
    // The zone's 3 s run out, then its fade, and it goes.
    step(&mut app, 120);
    assert!(!alive(&app, zone));
    assert_eq!(hit_ticks(&app, a).len(), 4, "hits only while active: {:?}", hit_ticks(&app, a));
}

#[test]
fn zone_hits_once_without_repeat() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::new(0.0, 0.0, 8.0), N);
    let brute = brute(app.world_mut(), Vec3::new(0.5, 0.0, 0.0), S);
    spawn_at(&mut app, hero, r#"(prefab: ["t_zone"], action: ["t_once"])"#, Vec3::ZERO, N);
    step(&mut app, 130);
    assert_eq!(hit_ticks(&app, brute).len(), 1);
}

#[test]
fn hit_zones_list_objects() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::new(0.0, 0.0, 8.0), N);
    let brute = brute(app.world_mut(), Vec3::new(3.0, 0.0, 0.0), S);
    let ring = spawn_at(&mut app, hero, r#"(prefab: ["t_zone"], action: ["t_ring"])"#, Vec3::new(3.0, 0.0, 0.0), N)[0];
    step(&mut app, 6);
    let zones = hit_zones(app.world());
    let zone = zones.iter().find(|z| z.attacker == ring).expect("the ring is listed");
    assert_eq!(zone.owner, Some(ai_id(&app, hero)));
    assert_eq!(zone.tags, ["ring"]);
    assert!((zone.lands_in - 0.4).abs() < 1e-3, "{}", zone.lands_in);
    let json = serde_json::to_value(zone).unwrap();
    assert_eq!(json["owner"], json!(ai_id(&app, hero).0), "{json}");
    assert_eq!(live_tags_at(app.world(), Vec3::new(3.5, 0.0, 0.0), 0.4), ["ring"]);
    assert!(live_tags_at(app.world(), Vec3::new(8.0, 0.0, 0.0), 0.4).is_empty());
    // A fighter's own zone (or its object's) never threatens it.
    let at = feet(&app, brute);
    assert_eq!(zones_threatening(app.world(), brute, at, 0.4).len(), 1, "the brute stands in the hero's ring");
    assert!(zones_threatening(app.world(), hero, Vec3::new(3.0, 0.0, 0.0), 0.4).is_empty(), "the hero's own");
    // No team on the hero: only the owner keeps its own ring off its warnings.
    assert!(threat_indicators(app.world(), hero, N).iter().all(|i| i.source != ring), "the hero's own ring is no warning");
    step(&mut app, 60);
    assert!(hit_zones(app.world()).iter().all(|z| z.attacker != ring), "landed and over");
    assert_eq!(hit_ticks(&app, brute).len(), 1);
}

// --- owners ------------------------------------------------------------------------------------

#[test]
fn owner_gone_despawns_objects() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let brute = brute(app.world_mut(), Vec3::new(0.0, 0.0, -6.0), S);
    let marks = spawn(&mut app, brute, r#"(prefab: ["t_mark"], action: ["t_wait"], count: 2, stay: true, pattern: Line(start: 1, step: 1))"#);
    let mine = spawn(&mut app, hero, r#"(prefab: ["t_mark"], action: ["t_wait"], stay: true)"#);
    let ids: Vec<AiId> = marks.iter().map(|&m| ai_id(&app, m)).collect();
    let owner = ai_id(&app, brute);
    step(&mut app, 2);
    app.world_mut().despawn(brute);
    step(&mut app, 1);
    assert!(marks.iter().all(|&m| !alive(&app, m)), "a freed owner's objects go");
    assert!(alive(&app, mine[0]), "not another's");
    // Objects leave no tombstone; the brute does.
    let index = app.world().resource::<AiIndex>();
    assert!(ids.iter().all(|id| index.tombstone(*id).is_none()));
    assert!(index.tombstone(owner).is_some());
}

#[test]
fn dead_owner_clears_them() {
    // A dead owner's zones and projectiles go with it.
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::new(0.0, 0.0, 8.0), N);
    let brute = brute(app.world_mut(), Vec3::new(0.5, 0.0, 0.0), S);
    let puddle = spawn_at(&mut app, hero, r#"(prefab: ["t_zone"], action: ["t_puddle"])"#, Vec3::ZERO, N)[0];
    let bolt = spawn(&mut app, hero, r#"(prefab: ["t_bolt"], action: ["t_fly_plain"], at: (up: 1))"#)[0];
    step(&mut app, 5);
    assert_eq!(hit_ticks(&app, brute).len(), 1);
    kill(app.world_mut(), hero, None, None);
    assert!(!alive(&app, puddle) && !alive(&app, bolt), "gone with the death");
    step(&mut app, 100);
    assert_eq!(hit_ticks(&app, brute).len(), 1, "no more damage");

    // A zone whose owner is dead deals nothing, however the owner died.
    let mut app = arena();
    let hero = crate::common::hero(app.world_mut(), "hero", Vec3::new(0.0, 0.0, 8.0), N);
    let brute = crate::common::brute(app.world_mut(), Vec3::new(0.5, 0.0, 0.0), S);
    let puddle = spawn_at(&mut app, hero, r#"(prefab: ["t_zone"], action: ["t_once"])"#, Vec3::ZERO, N)[0];
    app.world_mut().entity_mut(hero).insert(shards_action::Dead);
    step(&mut app, 3);
    assert!(hit_ticks(&app, brute).is_empty());
    assert!(!alive(&app, puddle));
    // And a dead owner lays nothing down.
    assert!(spawn_at(&mut app, hero, r#"(prefab: ["t_zone"], action: ["t_once"])"#, Vec3::ZERO, N).is_empty(), "a dead owner spawns nothing");
}

#[test]
fn despawn_objects_of_clears_them() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let brute = brute(app.world_mut(), Vec3::new(0.0, 0.0, -6.0), S);
    spawn(&mut app, hero, r#"(prefab: ["t_mark"], action: ["t_wait"], count: 3, stay: true, pattern: Line(start: 1, step: 1))"#);
    let theirs = spawn(&mut app, brute, r#"(prefab: ["t_mark"], action: ["t_wait"], count: 2, stay: true, pattern: Line(start: 1, step: 1))"#);
    let owner = ai_id(&app, hero);
    assert_eq!(objects_of(app.world(), owner).len(), 3);
    assert_eq!(despawn_objects_of(app.world_mut(), owner), 3);
    assert!(objects_of(app.world(), owner).is_empty());
    assert_eq!(objects_of(app.world(), ai_id(&app, brute)), theirs, "in id order, the other owner's kept");
}

// --- then --------------------------------------------------------------------------------------

#[test]
fn then_chains_a_summon_pulse() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let summon = spawn(&mut app, hero, r#"(prefab: ["t_zone"], action: ["t_pulse"], stay: true)"#)[0];
    let n = tick(&app);
    step(&mut app, 95);
    let pulses: Vec<u64> = seen(&app).sounds.iter().filter(|s| s.1 == summon).map(|s| s.0).collect();
    assert_eq!(pulses, [n, n + 30, n + 60, n + 90], "a pulse at its start, then each time the action ends");
    assert!(alive(&app, summon) && action(&app, summon) == "t_pulse", "it stays");
}

#[test]
fn then_loop_period_is_ceil_length() {
    // A `then` loop of an action of L seconds repeats every ceil(60 L) ticks (the engine's timer
    // rule: the new run starts in the tick the old one ends, and steps from the next).
    for (action, period) in [("t_pulse", 30), ("t_pulse_odd", 31)] {
        let mut app = arena();
        let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
        let summon = spawn(&mut app, hero, &format!(r#"(prefab: ["t_zone"], action: ["{action}"], stay: true)"#))[0];
        step(&mut app, 4 * period);
        let pulses: Vec<u64> = seen(&app).sounds.iter().filter(|s| s.1 == summon).map(|s| s.0).collect();
        let gaps: Vec<u64> = pulses.windows(2).map(|w| w[1] - w[0]).collect();
        assert_eq!(gaps, [period; 4], "{action}: {pulses:?}");
    }
}

#[test]
fn at_zero_effects_fire_on_a_then_start() {
    // On an object its pulse's `at: 0` sound comes on every start (above); on a fighter the chain
    // starts in the tick the first action ends, after its end, with its `at: 0` sound.
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let n = tick(&app);
    assert!(shards_action::force_action(app.world_mut(), hero, "t_chain_a"));
    step(&mut app, 11);
    assert_eq!(action(&app, hero), "t_chain_a");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "t_chain_b", "0.2 s: 12 ticks");
    let log: Vec<&String> = seen(&app).log.iter().filter(|l| l.contains("t_chain")).collect();
    let want = [format!("{n} started t_chain_a"), format!("{} ended t_chain_a", n + 12), format!("{} started t_chain_b", n + 12)];
    assert_eq!(log, want.iter().collect::<Vec<_>>());
    let chimes: Vec<u64> = seen(&app).sounds.iter().filter(|s| s.2 == "chain").map(|s| s.0).collect();
    assert_eq!(chimes, [n + 12]);
    step(&mut app, 18);
    assert_eq!(action(&app, hero), "idle", "t_chain_b has no then");
}

// --- content and caps --------------------------------------------------------------------------

fn errors_of(actions: &str) -> Vec<String> {
    errors_with(actions, r#"{"p": (components: {})}"#)
}

fn errors_with(actions: &str, prefabs: &str) -> Vec<String> {
    let mut kinds = ContentKinds::default();
    kinds.register::<ActionDef>();
    kinds.register::<PrefabDef>();
    kinds.register::<shards_action::StatusDef>();
    let sources = [
        shards_content::db::Source { path: "content/a.action.ron".into(), text: actions.into() },
        shards_content::db::Source { path: "content/p.prefab.ron".into(), text: prefabs.into() },
    ];
    ContentDb::build(&kinds, &sources).errors().map(|e| e.to_string()).collect()
}

#[test]
fn spawn_cycle_fails_validation() {
    let actions = r#"{
        "a": (net_id: 1, phases: [("x", 0.1)], effects: [(at: 0.0, effect: Spawn((prefab: ["p"], action: ["b"])))]),
        "b": (net_id: 2, phases: [("x", 0.1)], effects: [(at: 0.0, effect: Spawn((prefab: ["p"], action: ["a"])))]),
        "fly": (net_id: 3, phases: [("x", 0.1)], impact: "burst"),
        "burst": (net_id: 4, phases: [("x", 0.1)], then: "fly"),
        "pulse": (net_id: 5, phases: [("x", 0.1)], then: "pulse"),
        "ping": (net_id: 6, phases: [("x", 0.1)], then: "pong"),
        "pong": (net_id: 7, phases: [("x", 0.1)], then: "ping"),
        "typo": (net_id: 8, phases: [("x", 0.1)], then: "pulze", impact: "nope",
                 effects: [(at: 0.0, effect: Spawn((prefab: ["q"], action: [], count: 0, delay: -1, ground: 0)))],
                 hits: [(span: Phase("x"), shape: Sphere(radius: 1), damage: 1, repeat: 0.02)]),
    }"#;
    let errors = errors_of(actions);
    let expected = [
        "content/a.action.ron: action `a`: spawns and impacts go round in a circle (a -> Spawn b -> Spawn a): each would start the next for ever; break the circle",
        "content/a.action.ron: action `b`: spawns and impacts go round in a circle (b -> Spawn a -> Spawn b): each would start the next for ever; break the circle",
        "content/a.action.ron: action `burst`: spawns and impacts go round in a circle (burst -> then fly -> impact burst): each would start the next for ever; break the circle",
        "content/a.action.ron: action `fly`: spawns and impacts go round in a circle (fly -> impact burst -> then fly): each would start the next for ever; break the circle",
        "content/a.action.ron: action `typo`: then -> unknown action `pulze` (did you mean `pulse`?)",
        "content/a.action.ron: action `typo`: impact -> unknown action `nope`",
        "content/a.action.ron: action `typo`: hits[0].repeat must be 0 (once per run) or at least 0.05 s (is 0.02)",
        "content/a.action.ron: action `typo`: effects[0] Spawn.prefab -> unknown prefab `q` (did you mean `p`?)",
        "content/a.action.ron: action `typo`: effects[0] Spawn.action is empty; name the action each object runs",
        "content/a.action.ron: action `typo`: effects[0] Spawn.count must be at least 1",
        "content/a.action.ron: action `typo`: effects[0] Spawn.delay must be a number >= 0 (is -1)",
        "content/a.action.ron: action `typo`: effects[0] Spawn.ground must be a number above 0 (is 0)",
    ];
    for line in expected {
        assert!(errors.iter().any(|e| e == line), "missing `{line}` in {errors:#?}");
    }
    assert_eq!(errors.len(), expected.len(), "a `then` loop alone passes: {errors:#?}");
}

/// Sets the object caps in the combat tuning the arena loaded.
fn caps(app: &mut App, all: u32, per_owner: u32) {
    let before = json!({"input_buffer": 0.15, "lock_on_range": 20.0, "max_statuses": 2, "sprint_stamina_per_s": 30.0});
    let mut def = before.clone();
    def["max_objects"] = json!(all);
    def["max_objects_per_owner"] = json!(per_owner);
    ok(app, "content_apply", json!({"changes": [{"kind": "combat_tuning", "id": "default", "def": def, "before": before}]}));
}

#[test]
fn objects_stop_at_the_cap() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let brute = brute(app.world_mut(), Vec3::new(0.0, 0.0, -9.0), S);
    caps(&mut app, 5, 3);
    let line = r#"(prefab: ["t_mark"], action: ["t_wait"], count: 4, stay: true, pattern: Line(start: 1, step: 1))"#;
    assert_eq!(spawn(&mut app, hero, line).len(), 3, "3 per owner");
    let warning = format!("{} spawn capped: 3 of 4 t_wait (max_objects 5, max_objects_per_owner 3)", id(&app, hero));
    let warnings: Vec<String> = app
        .world()
        .resource::<ai_core::AiJournal>()
        .after(0)
        .0
        .filter(|e| e.kind == ai_core::JournalKind::Warn)
        .map(|e| e.text.clone())
        .collect();
    assert!(warnings.contains(&warning), "{warnings:?}");
    assert_eq!(spawn(&mut app, brute, line).len(), 2, "5 in all");
    assert!(spawn(&mut app, brute, line).is_empty());
    assert_eq!(all_objects(&mut app).len(), 5);
}

#[test]
fn a_sweep_hits_max_targets_lowest_id_first() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let brutes: Vec<Entity> = [(-1.5, -1.0), (1.5, -1.0), (0.0, 1.8)].into_iter().map(|(x, z)| brute(app.world_mut(), Vec3::new(x, 0.0, z), S)).collect();
    assert!(shards_action::force_action(app.world_mut(), hero, "t_sweep"));
    let n = tick(&app);
    step(&mut app, 3);
    let targets: Vec<(u64, Entity)> = seen(&app).damaged.iter().map(|d| (d.0, d.2)).collect();
    // A cap of a tick's results, as a physics query's: the third comes on the next tick.
    assert_eq!(targets, [(n + 1, brutes[0]), (n + 1, brutes[1]), (n + 2, brutes[2])], "two a tick, lowest id first");
}

#[test]
fn prefab_build_is_cached() {
    let mut app = arena();
    let caster = fighter(app.world_mut(), "Caster", "t_caster", "monsters", Vec3::ZERO, N);
    let hero = hero(app.world_mut(), "hero", Vec3::new(0.0, 0.0, -8.0), S);
    lock(&mut app, caster, hero);
    let builds = |app: &App| app.world().resource::<PrefabCache>().builds();
    let before = builds(&app);
    press(&mut app, caster, "light");
    step(&mut app, 20);
    assert_eq!(builds(&app) - before, 2, "t_bolt and t_bolt_homing, once each for three bolts");
    let bolts = all_objects(&mut app).len();
    assert_eq!(bolts, 3);
    step(&mut app, 60);
    press(&mut app, caster, "light");
    step(&mut app, 20);
    assert_eq!(builds(&app) - before, 2, "built from the cache");
    // A change to the prefab is built again.
    let old = call(&mut app, "content_get", json!({"kind": "prefab", "id": "t_bolt"})).result["def"].clone();
    let mut faster = old.clone();
    faster["components"]["Projectile"] = json!("(speed: 30.0, lifetime: 2.0, aim_height: 0.9)");
    ok(&mut app, "content_apply", json!({"changes": [{"kind": "prefab", "id": "t_bolt", "def": faster, "before": old}]}));
    step(&mut app, 60);
    press(&mut app, caster, "light");
    step(&mut app, 8);
    assert_eq!(builds(&app) - before, 3, "t_bolt again");
    let world = app.world_mut();
    let mut q = world.query::<&shards_action::objects::Projectile>();
    assert!(q.iter(world).any(|p| p.speed == 30.0), "the new numbers");
}

#[test]
fn a_volley_homes_its_middle_shot_at_the_target() {
    let mut app = arena();
    let caster = fighter(app.world_mut(), "Caster", "t_caster", "monsters", Vec3::ZERO, N);
    let hero = hero(app.world_mut(), "hero", Vec3::new(0.0, 0.0, -8.0), S);
    lock(&mut app, caster, hero);
    press(&mut app, caster, "light");
    step(&mut app, 8);
    let world = app.world_mut();
    let mut q = world.query::<(&AiId, &shards_action::objects::Projectile, &ProjectileFlight)>();
    let mut shots: Vec<(AiId, f32, Vec3)> = q.iter(world).map(|(id, p, f)| (*id, p.homing, f.dir)).collect();
    shots.sort_by_key(|s| s.0);
    assert_eq!(shots.iter().map(|s| s.1 > 0.0).collect::<Vec<_>>(), [false, true, false], "the middle one homes");
    // Aimed at the target's chest from 1 m up, then turned by the fan.
    assert!(shots[1].2.z < -0.99 && shots[1].2.y < 0.0, "{:?}", shots[1].2);
    step(&mut app, 40);
    assert!(!hit_ticks(&app, hero).is_empty(), "the volley found the hero");
    assert_eq!(seen(&app).damaged.iter().filter(|d| d.2 == hero).count(), 1, "the middle shot; the others fly wide");
}

// --- review fixes ------------------------------------------------------------------------------

#[test]
fn a_prefab_with_bad_flight_numbers_is_refused_by_name() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let spawned = spawn(&mut app, hero, r#"(prefab: ["t_bolt_bad"], action: ["t_fly"], at: (forward: 0.5, up: 1))"#);
    assert!(spawned.is_empty());
    assert!(all_objects(&mut app).is_empty(), "no half-built object is left");
    let errors: Vec<String> = app
        .world()
        .resource::<ai_core::AiJournal>()
        .after(0)
        .0
        .filter(|e| e.kind == ai_core::JournalKind::Error)
        .map(|e| e.text.clone())
        .collect();
    assert!(errors.iter().any(|e| e.contains("prefab `t_bolt_bad` has a Projectile with speed = -5 (a number >= 0)")), "{errors:?}");
    step(&mut app, 3);
}

#[test]
fn spawn_content_that_cannot_work_is_caught_by_the_check() {
    let prefabs = r#"{"p": (components: {}), "proj": (components: {"Projectile": (speed: 1.0, lifetime: 1.0)}), "proj2": (extends: "proj", components: {})}"#;
    let actions = r#"{
        "fly_capsule": (net_id: 1, phases: [("x", 0.1)], hits: [(span: Phase("x"), shape: Capsule(radius: 0.3, length: 1.0), damage: 1)]),
        "fly_ball": (net_id: 2, phases: [("x", 0.1)], hits: [(span: Phase("x"), shape: Sphere(radius: 0.3), damage: 1)]),
        "zone_box": (net_id: 3, phases: [("x", 0.1)], hits: [(span: Phase("x"), shape: Box(width: 1, height: 1, depth: 1), damage: 1)]),
        "shoots": (net_id: 4, phases: [("x", 0.1)], effects: [
            (at: 0.0, effect: Spawn((prefab: ["proj2"], action: ["fly_capsule"]))),
            (at: 0.0, effect: Spawn((prefab: ["proj2"], action: ["fly_ball"]))),
            (at: 0.0, effect: Spawn((prefab: ["p"], action: ["zone_box"]))),
        ]),
        "flood": (net_id: 5, phases: [("x", 0.1)], effects: [
            (at: 0.0, effect: Spawn((prefab: ["p"], action: ["zone_box"], count: 255, pattern: Radial(lines: 255, start: 1, step: 1)))),
        ]),
    }"#;
    let errors = errors_with(actions, prefabs);
    let expected = [
        "content/a.action.ron: action `shoots`: effects[0] Spawn: `fly_capsule` is the flight of the projectile `proj2`, and its hits[0] is not a Sphere; only spheres sweep the segment a projectile flies (use a Sphere, or an `impact` action for a burst)",
        "content/a.action.ron: action `flood`: effects[0] Spawn spawns 65025 objects at once, over the caps of the combat tuning (96 = the smaller of max_objects and max_objects_per_owner); lower count or lines",
    ];
    for line in expected {
        assert!(errors.iter().any(|e| e == line), "missing `{line}` in {errors:#?}");
    }
    assert_eq!(errors.len(), expected.len(), "a ball flight and a box zone pass: {errors:#?}");
    // The cap does not make the spawn allocate or place 65025 objects.
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let spawned = spawn(
        &mut app,
        hero,
        r#"(prefab: ["t_mark"], action: ["t_wait"], count: 255, stay: true, pattern: Radial(lines: 255, start: 1, step: 0.1))"#,
    );
    assert_eq!(spawned.len(), 96, "the per-owner cap");
}

#[test]
fn a_flight_with_two_hit_windows_meets_one_target() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let left = brute(app.world_mut(), Vec3::new(-0.5, 0.0, -4.0), S);
    let right = brute(app.world_mut(), Vec3::new(0.5, 0.0, -4.0), S);
    spawn(&mut app, hero, r#"(prefab: ["t_bolt"], action: ["t_fly_twin"], at: (forward: 0.5, up: 1))"#);
    step(&mut app, 40);
    let hit: Vec<Entity> = seen(&app).damaged.iter().map(|d| d.2).collect();
    assert_eq!(hit.len(), 1, "one target in all, not one per window: {hit:?} ({left:?}, {right:?})");
}

#[test]
fn two_turned_spawns_in_one_tick_turn_differently() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let line = r#"(prefab: ["t_mark"], action: ["t_wait"], count: 1, random_turn: true, pattern: Radial(lines: 1, start: 2, step: 0))"#;
    let first = spawn(&mut app, hero, line);
    let second = spawn(&mut app, hero, line);
    let (a, b) = (bearing(feet(&app, first[0])), bearing(feet(&app, second[0])));
    assert!((a - b).abs() > 1e-3, "two casts of one tick, one angle: {a} {b}");
}

#[test]
fn a_delayed_projectile_flies_from_the_tick_after_it_starts() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let bolts = spawn(&mut app, hero, r#"(prefab: ["t_bolt"], action: ["t_fly_plain"], count: 2, delay: 0.1, at: (up: 1), pattern: Line(start: 1, step: 0))"#);
    let late = bolts[1];
    let start = feet(&app, late);
    let mut started = None;
    for _ in 0..20 {
        step(&mut app, 1);
        if action(&app, late) != "idle" {
            started = Some(tick(&app));
            break;
        }
    }
    started.expect("the delayed bolt started");
    assert_eq!(feet(&app, late), start, "it has not flown on the tick it starts");
    step(&mut app, 1);
    assert!((feet(&app, late).z - (start.z - 0.25)).abs() < 1e-4, "{} from {}", feet(&app, late), start);
}

#[test]
fn a_bolt_spawned_behind_a_wall_stops_at_it() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    wall(app.world_mut(), Vec3::new(-3.0, 0.0, -0.7), Vec3::new(3.0, 3.0, -0.5));
    let brute = brute(app.world_mut(), Vec3::new(0.0, 0.0, -3.0), S);
    step(&mut app, 1);
    let bolt = spawn(&mut app, hero, r#"(prefab: ["t_bolt"], action: ["t_fly"], at: (forward: 0.8, up: 1))"#)[0];
    assert_eq!(flight(&app, bolt).dir, Vec3::ZERO, "stopped where it was made");
    assert!(feet(&app, bolt).z > -0.5, "in front of the wall: {}", feet(&app, bolt));
    step(&mut app, 2);
    assert_eq!(action(&app, bolt), "t_burst", "it burst at the wall");
    step(&mut app, 30);
    assert!(hit_ticks(&app, brute).is_empty(), "nothing went through the wall");
}

#[test]
fn an_aimed_spawn_with_no_target_says_so() {
    let mut app = arena();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let spawned = spawn(&mut app, hero, r#"(prefab: ["t_bolt"], action: ["t_fly"], aim: Target(height: 0.9))"#);
    assert!(spawned.is_empty());
    let lines: Vec<String> = app.world().resource::<ai_core::AiJournal>().after(0).0.map(|e| e.text.clone()).collect();
    assert!(
        lines.iter().any(|l| l.contains("spawn of test skipped: it aims at the lock-on target and there is none (lock_on first)")),
        "{lines:?}"
    );
}

#[derive(Resource, Default)]
struct BleedSources(Vec<Option<AiId>>);

fn note_bleed_source(world: &mut World, _: Entity) {
    let source = world.resource::<shards_action::StatusSource>().0;
    world.resource_mut::<BleedSources>().0.push(source);
}

fn nothing(_: &mut World, _: Entity) {}

#[test]
fn a_status_built_up_by_an_object_names_its_owner() {
    use shards_action::ActionAppExt as _;
    let mut app = app_with(|app| {
        record(app);
        app.init_resource::<BleedSources>();
        app.add_status_effect("bleed", note_bleed_source, nothing, nothing);
    });
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let brute = brute(app.world_mut(), Vec3::new(0.0, 0.0, -3.0), S);
    spawn(&mut app, hero, r#"(prefab: ["t_bolt"], action: ["t_fly_bleed"], at: (forward: 0.5, up: 1))"#);
    step(&mut app, 20);
    let _ = brute;
    assert_eq!(app.world().resource::<BleedSources>().0, [Some(ai_id(&app, hero))], "the owner, not the bolt");
}
