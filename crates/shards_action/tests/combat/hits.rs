//! Hits and the damage pipeline: i-frames, guards, parries, poise, hyper-armor, the
//! hit-once rule, `AiId` order, statuses.

use bevy::prelude::*;
use serde_json::json;
use shards_action::prelude::*;
use shards_action::status::StatusEntry;
use shards_action::{ActionAppExt, DamageCtx, Outcome};
use shards_motion::{CharacterParams, Collider, ColliderId};

use crate::common::*;

/// The brute 2 m north of the hero, facing it; the hero faces the brute.
fn duel(moveset: &str) -> (App, Entity, Entity) {
    duel_with(moveset, |_| {})
}

fn duel_with(moveset: &str, setup: impl FnOnce(&mut App)) -> (App, Entity, Entity) {
    let mut app = app_with(setup);
    let world = app.world_mut();
    let hero = hero(world, moveset, Vec3::ZERO, N);
    let brute = brute(world, Vec3::new(0.0, 0.0, -2.0), S);
    (app, hero, brute)
}

#[test]
fn dodge_i_frames_evade_a_hit_and_a_late_dodge_is_hit() {
    // The swipe lands 30 ticks after the press. A sidestep's i-frames last 0.18 s.
    let (mut app, hero, brute) = duel("evader");
    let (h, b) = (id(&app, hero), id(&app, brute));
    press(&mut app, brute, "light");
    step(&mut app, 24);
    assert_eq!(ok(&mut app, "dodge", json!({"dir": "E"})), "ok: sidestep (E)");
    step(&mut app, 30);
    assert_eq!(lines(&app, "evaded"), [format!("30 {h} evaded {b} swipe")]);
    assert_eq!(hp(&app, hero), 100.0);

    step(&mut app, 30);
    press(&mut app, brute, "light");
    step(&mut app, 10);
    ok(&mut app, "dodge", json!({"dir": "E"}));
    step(&mut app, 30);
    assert_eq!(lines(&app, "evaded").len(), 1, "too early: the i-frames were over");
    assert_eq!(lines(&app, &format!("{b} -> {h} swipe")), [format!("114 {b} -> {h} swipe -15 (hp 85)")]);
}

#[test]
fn a_block_takes_stamina_instead_of_health_until_the_guard_breaks() {
    let (mut app, hero, brute) = duel("hero");
    let (h, b) = (id(&app, hero), id(&app, brute));
    assert_eq!(ok(&mut app, "block", json!({})), "ok: block (holding block)");
    press(&mut app, brute, "light");
    step(&mut app, 40);
    assert_eq!(action(&app, hero), "block", "held, the block loops");
    assert_eq!(lines(&app, "blocked"), [format!("30 {h} blocked {b} swipe: -2 hp, -15 stamina (hp 98)")]);
    assert!((hp(&app, hero) - 98.5).abs() < 1e-4, "10% gets through");

    // Not enough stamina for the next one: the guard breaks.
    step(&mut app, 30);
    let left = stamina(&app, hero);
    app.world_mut().get_mut::<Stamina>(hero).unwrap().spend(left - 10.0);
    press(&mut app, brute, "light");
    step(&mut app, 30);
    let broke = lines(&app, "'s guard");
    assert_eq!(broke, [format!("100 {b} broke {h}'s guard with swipe -2 (hp 97)")]);
    assert!(lines(&app, &format!("{h} staggered (guard broken)")).len() == 1, "{:#?}", journal(&app));
    assert_eq!(action(&app, hero), "stagger");
    assert_eq!(stamina(&app, hero), 0.0);
}

#[test]
fn a_guard_breaking_slam_breaks_even_a_full_guard() {
    let (mut app, hero, brute) = duel("hero");
    let (h, b) = (id(&app, hero), id(&app, brute));
    ok(&mut app, "block", json!({}));
    press(&mut app, brute, "heavy");
    step(&mut app, 50);
    assert_eq!(lines(&app, "'s guard"), [format!("48 {b} broke {h}'s guard with slam -4 (hp 96)")]);
    assert_eq!(lines(&app, "staggered"), [format!("48 {h} staggered (guard broken)")]);
    assert_eq!(action(&app, hero), "stagger");
}

#[test]
fn a_perfect_guard_parries_and_staggers_the_attacker() {
    let (mut app, hero, brute) = duel("hero");
    let (h, b) = (id(&app, hero), id(&app, brute));
    press(&mut app, brute, "light");
    step(&mut app, 26);
    assert_eq!(ok(&mut app, "parry", json!({})), "ok: parry");
    step(&mut app, 4);
    assert_eq!(lines(&app, &format!("{h} parried")), [format!("30 {h} parried {b} swipe")]);
    assert_eq!(lines(&app, "staggered"), [format!("30 {b} staggered (parried)")]);
    assert_eq!(action(&app, brute), "brute_stagger");
    assert_eq!(hp(&app, hero), 100.0);

    // A raised block's first moments parry too; a later block only blocks.
    step(&mut app, 70);
    press(&mut app, brute, "light");
    step(&mut app, 27);
    hold(&mut app, hero, "block", true);
    step(&mut app, 3);
    assert_eq!(lines(&app, &format!("{h} parried")).len(), 2, "{:#?}", journal(&app));
}

#[test]
fn a_shockwave_cannot_be_parried_only_blocked() {
    let (mut app, hero, brute) = duel("hero");
    let (h, b) = (id(&app, hero), id(&app, brute));
    press(&mut app, brute, "technique.2");
    step(&mut app, 40);
    ok(&mut app, "parry", json!({}));
    step(&mut app, 5);
    assert!(lines(&app, &format!("{h} parried")).is_empty());
    assert_eq!(lines(&app, "blocked"), [format!("42 {h} blocked {b} shockwave: -0 hp, -20 stamina (hp 100)")]);
}

#[test]
fn a_poise_break_interrupts_and_staggers() {
    let (mut app, hero, brute) = duel("hero");
    let (h, b) = (id(&app, hero), id(&app, brute));
    press(&mut app, brute, "technique.2"); // shockwave: lands at 0.7 s
    ok(&mut app, "heavy_attack", json!({})); // 40 poise at 0.5 s
    step(&mut app, 30);
    assert_eq!(lines(&app, &format!("{h} -> {b}")), [format!("30 {h} -> {b} heavy -45 (hp 55)")]);
    assert_eq!(lines(&app, "staggered"), [format!("30 {b} staggered (poise broken)")]);
    assert_eq!(action(&app, brute), "brute_stagger");
    let poise = *app.world().get::<Poise>(brute).unwrap();
    assert!(poise.is_broken() && poise.current == 0.0, "{poise:?}");
    step(&mut app, 60);
    assert_eq!(hp(&app, hero), 100.0, "the shockwave was cut short");
    let poise = *app.world().get::<Poise>(brute).unwrap();
    assert!(!poise.is_broken() && poise.current == poise.max, "full again after the stagger: {poise:?}");
}

#[test]
fn hyper_armor_ignores_poise() {
    let (mut app, hero, brute) = duel("hero");
    let (h, b) = (id(&app, hero), id(&app, brute));
    press(&mut app, brute, "heavy"); // slam: hyper-armor through its windup
    ok(&mut app, "heavy_attack", json!({}));
    step(&mut app, 30);
    assert_eq!(lines(&app, &format!("{h} -> {b}")), [format!("30 {h} -> {b} heavy -45 (hp 55)")]);
    assert!(lines(&app, &format!("{b} staggered")).is_empty());
    assert_eq!(app.world().get::<Poise>(brute).unwrap().current, 30.0, "poise untouched");
    assert_eq!(action(&app, brute), "slam");
    step(&mut app, 20);
    assert_eq!(lines(&app, &format!("{b} -> {h}")), [format!("48 {b} -> {h} slam -40 (hp 60)")]);
    assert_eq!(lines(&app, "staggered"), [format!("48 {h} staggered (poise broken)")], "the hero has no armor left");
}

#[test]
fn a_flinch_without_hyper_armor() {
    let (mut app, hero, brute) = duel("hero");
    ok(&mut app, "light_attack", json!({}));
    step(&mut app, 12);
    assert_eq!(action(&app, brute), "brute_flinch", "10 poise does not break 30, it flinches");
    let _ = hero;
}

#[test]
fn one_action_hits_a_target_once_however_long_it_overlaps() {
    let (mut app, hero, brute) = duel("hero");
    let (h, b) = (id(&app, hero), id(&app, brute));
    ok(&mut app, "god", json!({"id": b}));
    ok(&mut app, "heavy_attack", json!({}));
    step(&mut app, 80); // its hit window spans 9 ticks of overlap
    assert_eq!(lines(&app, &format!("{h} -> {b}")), [format!("30 {h} -> {b} heavy -0 (hp 100, god mode)")]);
}

#[test]
fn the_last_tick_of_an_action_still_hits_and_still_protects() {
    // The jab's hit window and the dash's i-frames both reach the end of their action,
    // and the tick that ends an action is still part of it.
    let mut app = app();
    let world = app.world_mut();
    let dasher = fighter(world, "Hero", "edges", "heroes", Vec3::new(-1.0, 0.0, 0.0), N);
    let idle = fighter(world, "Hero", "edges", "heroes", Vec3::new(1.0, 0.0, 0.0), N);
    let foe = fighter(world, "Dummy", "edges", "monsters", Vec3::new(0.0, 0.0, -1.5), S);
    let (d, i, f) = (id(&app, dasher), id(&app, idle), id(&app, foe));
    press(&mut app, foe, "light");
    press(&mut app, dasher, "dodge");
    step(&mut app, 14);
    assert!(lines(&app, "jab").is_empty(), "nothing before the last tick: {:#?}", journal(&app));
    step(&mut app, 1);
    assert_eq!(action(&app, foe), "idle", "the jab is over after its last tick");
    // Targets in `AiId` order: the dasher first.
    assert_eq!(lines(&app, "jab"), [format!("15 {d} evaded {f} jab"), format!("15 {f} -> {i} jab -7 (hp 93)")]);
    assert_eq!(hp(&app, dasher), 100.0);
}

/// Spawns `n` fighters so that a query visits them in the opposite of `AiId` order.
fn spawn_out_of_order(app: &mut App, spots: &[(Vec3, Vec3, &str, &str)]) -> Vec<Entity> {
    #[derive(Component)]
    struct Late;
    let world = app.world_mut();
    // An archetype for the later ids, created before the earlier ids' archetype.
    let scout = fighter(world, "Scout", "dummy", "nobody", Vec3::new(50.0, 0.0, 50.0), N);
    world.entity_mut(scout).insert(Late);
    world.despawn(scout);
    let mut spawned = Vec::new();
    for (i, &(at, facing, moveset, team)) in spots.iter().enumerate() {
        let e = fighter(world, "Fighter", moveset, team, at, facing);
        if i % 2 == 0 {
            world.entity_mut(e).insert(Late);
        }
        spawned.push(e);
    }
    spawned
}

#[test]
fn hits_resolve_in_ai_id_order_whatever_the_query_order() {
    let mut app = app();
    // Two attackers swing at one target in the same tick; the target has 30 hp.
    let f = spawn_out_of_order(
        &mut app,
        &[
            (Vec3::new(0.0, 0.0, 1.5), N, "hero", "heroes"),
            (Vec3::new(1.5, 0.0, 0.0), Vec3::NEG_X, "hero", "heroes"),
            (Vec3::ZERO, N, "dummy", "monsters"),
        ],
    );
    let (a1, a2, target) = (f[0], f[1], f[2]);
    let mut q = app.world_mut().query_filtered::<Entity, With<Fighter>>();
    let visited: Vec<Entity> = q.iter(app.world()).filter(|e| [a1, a2].contains(e)).collect();
    assert_eq!(visited, [a2, a1], "the query visits the attackers against id order");
    app.world_mut().get_mut::<Health>(target).unwrap().current = 30.0;
    press(&mut app, a1, "light");
    press(&mut app, a2, "light");
    step(&mut app, 13);
    let (i1, i2, t) = (id(&app, a1), id(&app, a2), id(&app, target));
    let story: Vec<String> = journal(&app).into_iter().filter(|l| l.contains(&t)).collect();
    assert_eq!(
        story,
        [
            format!("12 {i1} -> {t} light_1 -20 (hp 10)"),
            format!("12 {i2} -> {t} light_1 -20 (hp 0)"),
            format!("12 {t} died (killed by {i2} with light_1)")
        ]
    );

    // One attacker, two targets in its cone: targets in id order too, though the
    // physics layer lists the second one first (its collider is older).
    let mut app = crate::common::app();
    let world = app.world_mut();
    let f = [
        fighter(world, "Dummy", "dummy", "monsters", Vec3::new(-0.6, 0.0, -1.5), S),
        fighter(world, "Dummy", "dummy", "monsters", Vec3::new(0.6, 0.0, -1.5), S),
        fighter(world, "Hero", "hero", "heroes", Vec3::ZERO, N),
    ];
    world.entity_mut(f[0]).remove::<Collider>();
    world.entity_mut(f[0]).insert(CharacterParams::default().collider());
    let cid = |e: Entity| *world.get::<ColliderId>(e).unwrap();
    assert!(cid(f[0]) > cid(f[1]));
    press(&mut app, f[2], "light");
    step(&mut app, 13);
    let (t1, t2, a) = (id(&app, f[0]), id(&app, f[1]), id(&app, f[2]));
    assert_eq!(
        lines(&app, &format!("{a} ->")),
        [format!("12 {a} -> {t1} light_1 -20 (hp 80)"), format!("12 {a} -> {t2} light_1 -20 (hp 80)")]
    );
}

fn bleed_applied(world: &mut World, e: Entity) {
    world.get_mut::<Health>(e).unwrap().max -= 10.0;
}

fn bleed_tick(world: &mut World, e: Entity) {
    world.get_mut::<Health>(e).unwrap().current -= 0.5;
}

fn bleed_expired(world: &mut World, e: Entity) {
    world.get_mut::<Health>(e).unwrap().max += 10.0;
}

#[test]
fn statuses_build_up_from_hits_and_run_the_game_hooks() {
    let (mut app, hero, brute) = duel_with("hero", |app| {
        app.add_status_effect("bleed", bleed_applied, bleed_tick, bleed_expired);
    });
    let h = id(&app, hero);
    app.world_mut().entity_mut(hero).insert((Poise::new(1000.0, 0.0, 1.0), Health::new(500.0)));
    press(&mut app, brute, "heavy");
    step(&mut app, 48);
    let statuses = app.world().get::<Statuses>(hero).unwrap();
    assert_eq!(shards_action::status::describe_statuses(statuses), "bleed 60%");
    step(&mut app, 60);
    press(&mut app, brute, "heavy");
    step(&mut app, 48);
    assert_eq!(lines(&app, "bleed"), [format!("156 {h} bleed applied")]);
    assert_eq!(app.world().get::<Health>(hero).unwrap().max, 490.0, "on_apply ran");
    let before = hp(&app, hero);
    step(&mut app, 70);
    assert_eq!(lines(&app, "bleed").last().unwrap(), &format!("215 {h} bleed expired"));
    assert_eq!(app.world().get::<Health>(hero).unwrap().max, 500.0, "on_expire ran");
    // 60 ticks of on_tick in all (1 s), the first one before `before`.
    assert!((before - hp(&app, hero) - 0.5 * 59.0).abs() < 1e-3, "on_tick ran every tick: {}", before - hp(&app, hero));
}

fn bleeding(buildup: f32, active: f32) -> Statuses {
    Statuses(vec![StatusEntry { id: "bleed".into(), buildup, threshold: 100.0, active, stacks: u32::from(active > 0.0) }])
}

#[test]
fn a_killing_hit_applies_no_status() {
    let (mut app, hero, brute) = duel_with("hero", |app| {
        app.add_status_effect("bleed", bleed_applied, bleed_tick, bleed_expired);
    });
    let (h, b) = (id(&app, hero), id(&app, brute));
    // 30 hp, bleed at 50%: the slam (40 damage, 60 bleed) would tip it over, but kills first.
    app.world_mut().entity_mut(hero).insert((Health::new(30.0), bleeding(50.0, 0.0)));
    press(&mut app, brute, "heavy");
    step(&mut app, 50);
    assert_eq!(lines(&app, "died"), [format!("48 {h} died (killed by {b} with slam)")]);
    assert!(lines(&app, "bleed").is_empty(), "no status for the dead: {:#?}", journal(&app));
    assert_eq!(app.world().get::<Health>(hero).unwrap().max, 30.0, "on_apply never ran");
}

#[test]
fn a_corpse_runs_no_status_hooks() {
    let (mut app, hero, brute) = duel_with("hero", |app| {
        app.add_status_effect("bleed", bleed_applied, bleed_tick, bleed_expired);
    });
    app.world_mut().entity_mut(hero).insert((Health::new(30.0), bleeding(0.0, 5.0)));
    press(&mut app, brute, "heavy");
    step(&mut app, 48);
    assert!(app.world().get::<Dead>(hero).is_some());
    assert_eq!(hp(&app, hero), 0.0);
    step(&mut app, 20);
    assert_eq!(hp(&app, hero), 0.0, "its bleeding stopped with it");
}

fn double_backstabs(ctx: &mut DamageCtx, world: &World) {
    // A game's hook: hits from behind deal double.
    let (Some(a), Some(t)) = (world.get::<Transform>(ctx.attacker), world.get::<Transform>(ctx.target)) else { return };
    if ctx.outcome == Outcome::Hit && t.forward().dot(a.translation - t.translation) < 0.0 {
        ctx.damage *= 2.0;
    }
}

#[test]
fn damage_hooks_run_after_the_engine_modifiers() {
    let mut app = app_with(|app| {
        app.add_damage_hook(double_backstabs);
    });
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let front = fighter(world, "Dummy", "dummy", "monsters", Vec3::new(-0.5, 0.0, -1.5), S);
    let back = fighter(world, "Dummy", "dummy", "monsters", Vec3::new(0.5, 0.0, -1.5), N);
    let armored = fighter(world, "Dummy", "dummy", "monsters", Vec3::new(0.0, 0.0, -1.0), N);
    world.entity_mut(armored).insert(Armor { percent: 0.5, flat: 2.0 });
    ok(&mut app, "light_attack", json!({}));
    step(&mut app, 13);
    assert_eq!(hp(&app, front), 80.0);
    assert_eq!(hp(&app, back), 60.0, "hit from behind");
    assert_eq!(hp(&app, armored), 84.0, "20 halved, less 2, then doubled from behind: armor comes first");
    let _ = hero;
}

/// Whether `T` is in the app's type registry.
fn registered<T: 'static>(app: &App) -> bool {
    app.world().resource::<AppTypeRegistry>().read().contains(std::any::TypeId::of::<T>())
}

#[test]
fn combat_events_reflect_for_the_network_layer() {
    use shards_action::{
        ActionRejected, ActionStarted, Blocked, Damaged, Died, Evaded, GuardBroken, Healed, HitShape, Parried, Staggered,
        Telegraph, WorldShape,
    };
    let app = app();
    let all = [
        registered::<ActionStarted>(&app),
        registered::<ActionRejected>(&app),
        registered::<Telegraph>(&app),
        registered::<Healed>(&app),
        registered::<Damaged>(&app),
        registered::<Blocked>(&app),
        registered::<Parried>(&app),
        registered::<Evaded>(&app),
        registered::<GuardBroken>(&app),
        registered::<Staggered>(&app),
        registered::<Died>(&app),
    ];
    assert!(all.iter().all(|r| *r), "{all:?}");
    let shape = WorldShape { shape: HitShape::Circle { radius: 4.0, height: 1.0 }, origin: Vec3::X, facing: Vec3::NEG_Z };
    let telegraph = Telegraph {
        attacker: Entity::PLACEHOLDER,
        action: "slam".into(),
        lands_in: 0.5,
        zone: Some(shape),
        symbol: "!".into(),
        sound: None,
    };
    let copy = telegraph.reflect_clone().unwrap().take::<Telegraph>().unwrap();
    assert_eq!(copy, telegraph);
}
