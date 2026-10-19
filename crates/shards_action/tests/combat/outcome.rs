//! Hit outcome, part two (B1): guards with per-hit costs and shares, perfect evades, walls,
//! the stamina overdraft, and what the events say. Content: `assets/content/reactions.*.ron`.

use bevy::prelude::*;
use serde_json::json;
use shards_action::prelude::*;
use shards_action::{ActionAppExt, DamageCtx, HitWeight, IgnoreReason, Knockback, Outcome};
use shards_core::Intent;
use ai_core::AiKind;
use shards_motion::{Collider, MotionState, Shape};

use crate::common::*;
use crate::reactions::{bout, bout_with, seen};

fn bleed(app: &App, e: Entity) -> f32 {
    app.world().get::<Statuses>(e).unwrap().0.iter().find(|s| s.id == "bleed").map_or(0.0, |s| s.buildup)
}

/// The victim holds its guard past the perfect window (0.18 s) and loops in the hold.
fn guarding(victim: &str) -> (App, Entity, Entity) {
    let (mut app, attacker, target) = bout(victim);
    hold(&mut app, target, "block", true);
    step(&mut app, 30);
    (app, attacker, target)
}

#[test]
fn guard_damage_sets_the_block_cost() {
    // The slam does 40 damage but asks 25 of the guard: 25 * 0.8 stamina, not 40 * 0.8.
    let (mut app, attacker, victim) = guarding("r_victim");
    let (a, v) = (id(&app, attacker), id(&app, victim));
    press(&mut app, attacker, "heavy");
    step(&mut app, 8);
    assert_eq!(lines(&app, "blocked"), [format!("36 {v} blocked {a} r_hit_heavy: -0 hp, -20 stamina (hp 100)")]);
    assert_eq!(stamina(&app, victim), 80.0);
}

#[test]
fn parry_without_stagger_leaves_the_attacker() {
    let (mut app, attacker, victim) = bout("r_victim");
    let (a, v) = (id(&app, attacker), id(&app, victim));
    hold(&mut app, victim, "block", true);
    press(&mut app, attacker, "heavy");
    step(&mut app, 8);
    // Half of 25 * 0.8, and a line that says so.
    assert_eq!(lines(&app, "parried"), [format!("6 {v} parried {a} r_hit_heavy, -10 stamina")]);
    assert_eq!(stamina(&app, victim), 90.0);
    assert!(lines(&app, "staggered").is_empty(), "the guard says the attacker is left alone: {:#?}", journal(&app));
    assert_eq!(action(&app, attacker), "r_hit_heavy");
    assert_eq!(hp(&app, victim), 100.0);
}

#[test]
fn a_perfect_guard_with_a_cost_breaks_when_unaffordable() {
    let (mut app, attacker, victim) = bout("r_victim");
    let (a, v) = (id(&app, attacker), id(&app, victim));
    app.world_mut().get_mut::<Stamina>(victim).unwrap().spend(95.0); // 5 left, the perfect cost is 10
    hold(&mut app, victim, "block", true);
    press(&mut app, attacker, "heavy");
    step(&mut app, 8);
    assert!(lines(&app, "parried").is_empty(), "{:#?}", journal(&app));
    // The break takes the share the guard names (half of 40) and all the stamina.
    assert_eq!(lines(&app, "'s guard"), [format!("6 {a} broke {v}'s guard with r_hit_heavy -20 (hp 80)")]);
    assert_eq!(stamina(&app, victim), 0.0);
    assert_eq!(action(&app, victim), "r_guard_broken", "the moveset's guard-broken action, not the stagger");
    assert_eq!(lines(&app, "staggered"), [format!("6 {v} staggered (guard broken)")]);
}

#[test]
fn guard_break_damage_share() {
    // A guard-breaking hit (40 damage, 60 bleed) against a guard that lets half through.
    let (mut app, attacker, victim) = guarding("r_victim");
    press(&mut app, attacker, "technique.6");
    step(&mut app, 8);
    assert_eq!(hp(&app, victim), 80.0, "half of 40");
    let buildup = bleed(&app, victim);
    assert!((29.0..=30.0).contains(&buildup), "half of 60 (less a little decay): {buildup}");
    assert_eq!(stamina(&app, victim), 0.0);
}

#[test]
fn guard_passthrough_scales_status() {
    // The same 60 bleed against the same guard: a quarter of it through a block, half through a break.
    let (mut app, attacker, victim) = guarding("r_victim");
    press(&mut app, attacker, "technique.5");
    step(&mut app, 8);
    let blocked = bleed(&app, victim);
    let (mut app, attacker, victim) = guarding("r_victim");
    press(&mut app, attacker, "technique.6");
    step(&mut app, 8);
    let broken = bleed(&app, victim);
    assert!(broken > 1.9 * blocked && broken < 2.2 * blocked, "half versus a quarter: {broken} {blocked}");
}

#[test]
fn blocked_hit_applies_the_status_share() {
    // The old rule dropped a blocked hit's statuses; a share of a quarter lets 15 of 60 through.
    let (mut app, attacker, victim) = guarding("r_victim");
    press(&mut app, attacker, "technique.5");
    step(&mut app, 8);
    assert_eq!(lines(&app, "blocked").len(), 1, "{:#?}", journal(&app));
    let buildup = bleed(&app, victim);
    assert!((14.0..=15.0).contains(&buildup), "a quarter of 60 (less a little decay): {buildup}");
}

#[test]
fn guard_broken_push_scales_the_body_velocity() {
    let mut app = app_with(crate::reactions::record);
    let world = app.world_mut();
    let attacker = fighter(world, "Hero", "r_attacker", "heroes", Vec3::ZERO, N);
    let victim = fighter(world, "Dummy", "r_walker", "monsters", Vec3::new(0.0, 0.0, -2.0), S);
    hold(&mut app, victim, "block", true);
    app.world_mut().get_mut::<Intent>(victim).unwrap().move_dir = S * 0.1;
    step(&mut app, 30); // past the perfect window, walking
    let walking = app.world().get::<MotionState>(victim).unwrap().velocity;
    assert!(walking.z > 0.05, "the guard walks towards the attacker: {walking}");
    press(&mut app, attacker, "technique.6"); // a guard-breaking hit
    step(&mut app, 6);
    let push = app.world().get::<Knockback>(victim).expect("the guard break started a push");
    assert!(push.velocity.z > 0.0 && push.velocity.z <= walking.z * 0.2 + 1e-3 && push.velocity.y == 0.0, "{push:?} vs {walking}");
    let velocity = app.world().get::<MotionState>(victim).unwrap().velocity;
    assert!(velocity.z < walking.z * 0.2 + 1e-3, "the body's own momentum was replaced, not kept: {velocity} vs {walking}");
    assert_eq!(push.decel, 10.0);
    assert_eq!(action(&app, victim), "r_guard_broken");
}

#[test]
fn line_of_sight_hits_stop_at_walls_and_stay_open() {
    let (mut app, attacker, victim) = bout("r_victim");
    let (a, v) = (id(&app, attacker), id(&app, victim));
    let blocker = wall(app.world_mut(), Vec3::new(-3.0, 0.0, -1.2), Vec3::new(3.0, 3.0, -1.0));
    press(&mut app, attacker, "technique.3"); // a sphere, open from 0.1 s to 0.6 s
    step(&mut app, 14);
    assert_eq!(hp(&app, victim), 100.0, "the wall is in the way: {:#?}", journal(&app));
    assert!(lines(&app, &format!("{a} -> {v}")).is_empty());
    // The target was not marked: the wall goes, and the same window hits it.
    app.world_mut().despawn(blocker);
    step(&mut app, 4);
    assert_eq!(hp(&app, victim), 90.0);
    // Without the flag walls do not matter.
    let (mut app, attacker, victim) = bout("r_victim");
    wall(app.world_mut(), Vec3::new(-3.0, 0.0, -1.2), Vec3::new(3.0, 3.0, -1.0));
    press(&mut app, attacker, "technique.7");
    step(&mut app, 8);
    assert_eq!(hp(&app, victim), 90.0, "a hit without LineOfSight goes through walls");
}

#[test]
fn line_of_sight_ignores_the_ends_of_the_ray() {
    // Both fighters have a WORLD-layer collider (the default layers): the ray must not count them.
    let mut app = app();
    let world = app.world_mut();
    let still = |world: &mut World, moveset: &str, at: Vec3| {
        world
            .spawn((
                Name::new(moveset.to_owned()),
                AiKind::new(moveset.to_owned()),
                Fighter::new(moveset),
                Health::new(100.0),
                Team::new(moveset.to_owned()),
                Collider::new(Shape::cuboid(Vec3::new(0.4, 0.9, 0.4))).with_offset(Vec3::Y * 0.9),
                Transform::from_translation(at).looking_to(Vec3::NEG_Z, Vec3::Y),
            ))
            .id()
    };
    let attacker = still(world, "r_attacker", Vec3::ZERO);
    let victim = still(world, "r_victim", Vec3::new(0.0, 0.0, -2.0));
    press(&mut app, attacker, "technique.3");
    step(&mut app, 10);
    assert_eq!(hp(&app, victim), 90.0, "{:#?}", journal(&app));
}

#[test]
fn overdraft_starts_on_any_stamina_and_drains_to_zero() {
    let mut app = app();
    let hero = hero(app.world_mut(), "r_victim", Vec3::ZERO, N);
    app.world_mut().get_mut::<Stamina>(hero).unwrap().spend(95.0); // 5 left, the roll costs 22
    let started = ok(&mut app, "dodge", json!({"dir": "E"}));
    assert!(started.starts_with("ok: r_roll"), "{started}");
    step(&mut app, 2);
    assert_eq!(action(&app, hero), "r_roll");
    assert_eq!(stamina(&app, hero), 0.0, "the cost drained it to 0, no further");
    // With nothing at all it does not start, and says what the roll costs.
    step(&mut app, 40);
    app.world_mut().get_mut::<Stamina>(hero).unwrap().spend(100.0);
    let refused = fails(&mut app, "dodge", json!({"dir": "E"}));
    assert!(refused.starts_with("not enough stamina: 0/22 (ready in "), "{refused}");
}

/// The roll starts in tick 1 (its i-frames 0.07-0.25 s open in tick 5, its perfect window
/// 0.105-0.215 s, sampled at the instant of each tick, in ticks 7 to 12); the attacker is pressed after `wait` ticks and its hit lands
/// 5 ticks after its start.
fn rolled_into(input: &str, wait: u64) -> (App, Entity, Entity) {
    let (mut app, attacker, victim) = bout("r_victim");
    press(&mut app, victim, "dodge");
    step(&mut app, wait);
    press(&mut app, attacker, input);
    step(&mut app, 12);
    (app, attacker, victim)
}

#[test]
fn perfect_iframes_send_perfect_evaded_once_per_swing() {
    let (app, attacker, victim) = rolled_into("technique.7", 2);
    let (a, v) = (id(&app, attacker), id(&app, victim));
    // The hit lands in tick 8, inside the perfect window; its own window stays open for six ticks.
    assert_eq!(plain_evades(&app), [format!("8 {v} evaded {a} r_hit_plain")]);
    assert_eq!(lines(&app, "perfectly"), [format!("8 {v} perfectly evaded {a} r_hit_plain")]);
    assert_eq!(hp(&app, victim), 100.0);
    // Too early: in the i-frames, before the perfect window.
    let (app, attacker, victim) = rolled_into("technique.7", 0);
    let (a, v) = (id(&app, attacker), id(&app, victim));
    assert_eq!(plain_evades(&app), [format!("6 {v} evaded {a} r_hit_plain")]);
    assert!(lines(&app, "perfectly").is_empty());
    // A hit that does no damage is not a perfect evade.
    let (app, _, _) = rolled_into("technique.8", 2);
    assert!(lines(&app, "perfectly").is_empty() && plain_evades(&app).len() == 1);
}

#[test]
fn zone_flag_is_never_a_perfect_evade() {
    let (app, attacker, victim) = rolled_into("technique.4", 2);
    let (a, v) = (id(&app, attacker), id(&app, victim));
    assert_eq!(plain_evades(&app), [format!("8 {v} evaded {a} r_hit_zone")]);
    assert!(lines(&app, "perfectly").is_empty());
}

#[test]
fn damaged_carries_weight_and_reaction() {
    for (input, weight, reaction) in [
        ("technique.7", HitWeight::Light, None),
        ("light", HitWeight::Light, Some(1)),
        ("heavy", HitWeight::Heavy, Some(2)),
        ("technique.1", HitWeight::Heavy, Some(3)),
        // 100 poise breaks it: the heavier weight, and no reaction (it staggers).
        ("technique.2", HitWeight::Break, None),
    ] {
        let (mut app, attacker, _) = bout("r_victim");
        press(&mut app, attacker, input);
        step(&mut app, 8);
        let hit = seen(&app).damaged.last().expect("a hit").clone();
        assert_eq!((hit.weight, hit.reaction), (weight, reaction), "{input}");
    }
}

#[derive(Resource, Default)]
struct Staggers(u32);

#[test]
fn a_blow_that_kills_is_weighed_without_the_break_it_never_makes() {
    // `r_hit_poise` does 1 damage, 100 poise and asks a heavy reaction. On a fresh victim the poise
    // breaks (the weight is `Break`, one `Staggered`); on one the blow kills, the poise is never
    // touched: the weight is what it would be without the break (`Heavy`, from the reaction tier).
    let record_staggers = |app: &mut App| {
        app.init_resource::<Staggers>();
        app.add_observer(|_: On<shards_action::Staggered>, mut n: ResMut<Staggers>| n.0 += 1);
    };
    let (mut app, attacker, victim) = bout_with("r_victim", record_staggers);
    press(&mut app, attacker, "technique.2");
    step(&mut app, 8);
    assert_eq!(seen(&app).damaged.last().unwrap().weight, HitWeight::Break);
    assert_eq!(app.world().resource::<Staggers>().0, 1, "the break staggers");
    assert!(app.world().get::<shards_action::Dead>(victim).is_none());

    let (mut app, attacker, victim) = bout_with("r_victim", record_staggers);
    app.world_mut().get_mut::<shards_action::Health>(victim).unwrap().current = 1.0;
    let poise_before = app.world().get::<shards_action::Poise>(victim).unwrap().current;
    press(&mut app, attacker, "technique.2");
    step(&mut app, 8);
    let hit = seen(&app).damaged.last().unwrap().clone();
    assert_eq!((hit.hp_left, hit.weight), (0.0, HitWeight::Heavy), "{hit:?}");
    assert!(app.world().get::<shards_action::Dead>(victim).is_some());
    assert_eq!(app.world().resource::<Staggers>().0, 0, "a corpse does not stagger");
    assert_eq!(app.world().get::<shards_action::Poise>(victim).unwrap().current, poise_before, "nor does it take poise");
}
#[test]
fn ignored_hits_print_their_reason() {
    fn ignore(ctx: &mut DamageCtx, _: &World) {
        match ctx.action.as_str() {
            "r_hit_plain" => (ctx.outcome, ctx.ignore_reason) = (Outcome::Ignored, IgnoreReason::Grace),
            "r_hit_light" => (ctx.outcome, ctx.ignore_reason) = (Outcome::Ignored, IgnoreReason::Hook),
            // No reason set: the default says the hook ignored it.
            "r_hit_heavy" => ctx.outcome = Outcome::Ignored,
            _ => {}
        }
    }
    let (mut app, attacker, victim) = bout_with("r_victim", |app| {
        app.add_damage_hook(ignore);
    });
    let (a, v) = (id(&app, attacker), id(&app, victim));
    for input in ["technique.7", "light", "heavy"] {
        press(&mut app, attacker, input);
        step(&mut app, 30);
    }
    let hits = lines(&app, &format!("{a} -> {v}"));
    assert_eq!(hits.len(), 3, "{hits:?}");
    assert!(hits[0].ends_with("r_hit_plain -0 (hp 100, grace)"), "{}", hits[0]);
    assert!(hits[1].ends_with("r_hit_light -0 (hp 100, ignored)"), "{}", hits[1]);
    assert!(hits[2].ends_with("r_hit_heavy -0 (hp 100, ignored)"), "{}", hits[2]);
    assert!(lines(&app, "reacts").is_empty(), "an ignored hit reacts to nothing");
}

/// The `evaded` lines that are not the `perfectly evaded` ones.
fn plain_evades(app: &App) -> Vec<String> {
    lines(app, " evaded ").into_iter().filter(|l| !l.contains("perfectly")).collect()
}

#[test]
fn the_perfect_window_is_sampled_at_the_tick_not_over_it() {
    // 0.105-0.215 s: tick 12 (t = 0.2) is inside, tick 13 (t = 0.2167) is not, though the tick
    // covers (0.2, 0.2167] and the window ends at 0.215.
    let (app, attacker, victim) = rolled_into("technique.7", 6);
    let (a, v) = (id(&app, attacker), id(&app, victim));
    assert_eq!(lines(&app, "perfectly"), [format!("12 {v} perfectly evaded {a} r_hit_plain")], "{:#?}", journal(&app));
    let (app, attacker, victim) = rolled_into("technique.7", 7);
    let (a, v) = (id(&app, attacker), id(&app, victim));
    assert_eq!(plain_evades(&app), [format!("13 {v} evaded {a} r_hit_plain")], "{:#?}", journal(&app));
    assert!(lines(&app, "perfectly").is_empty());
}

#[test]
fn a_hook_credit_names_the_combat_object_as_source() {
    #[derive(Resource, Default)]
    struct Log {
        damaged: Vec<(Entity, Option<Entity>)>,
        died: Vec<(Option<Entity>, Option<Entity>)>,
        owner: Option<Entity>,
    }
    fn credit(ctx: &mut DamageCtx, world: &World) {
        ctx.credit = world.resource::<Log>().owner.unwrap();
        if ctx.action == "r_hit_heavy" {
            ctx.damage = 1000.0;
        }
    }
    let (mut app, attacker, victim) = bout_with("r_victim", |app| {
        app.init_resource::<Log>();
        app.add_damage_hook(credit);
        app.add_observer(|e: On<shards_action::Damaged>, mut log: ResMut<Log>| log.damaged.push((e.event().attacker, e.event().source)));
        app.add_observer(|e: On<shards_action::Died>, mut log: ResMut<Log>| {
            log.died.push((e.event().killer, e.event().source));
        });
    });
    let owner = fighter(app.world_mut(), "Owner", "r_plain", "heroes", Vec3::new(30.0, 0.0, 0.0), N);
    app.world_mut().resource_mut::<Log>().owner = Some(owner);
    press(&mut app, attacker, "technique.7");
    step(&mut app, 8);
    assert_eq!(app.world().resource::<Log>().damaged, [(owner, Some(attacker))]);
    step(&mut app, 20);
    press(&mut app, attacker, "heavy");
    step(&mut app, 8);
    assert_eq!(app.world().resource::<Log>().died, [(Some(owner), Some(attacker))], "{:#?}", journal(&app));
    let _ = victim;
}

#[test]
fn a_hit_is_not_ignored_and_an_own_hit_has_no_source() {
    let (mut app, attacker, _) = bout("r_victim");
    press(&mut app, attacker, "technique.7");
    step(&mut app, 8);
    let hit = seen(&app).damaged.last().unwrap();
    assert_eq!((hit.ignore_reason, hit.source), (None, None));
}
