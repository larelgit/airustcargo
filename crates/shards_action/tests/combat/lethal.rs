//! The lethal hook and the downed state (B5): a game's veto before a gameplay death (die, survive,
//! go down), the downed body (hits ignored, no actions, its bleed-out), the revive (a paired action
//! held next to a downed ally), and what a fallen attacker leaves its pair's victim.

use bevy::prelude::*;
use serde_json::json;
use shards_action::prelude::*;
use shards_action::{
    bleed_out, go_down, kill, ActionAppExt, Downed, Fate, Grace, Lethal, LethalCause, Paired, Pairing, Statuses,
};
use shards_core::PlayerId;

use crate::common::*;

// --- the hooks a test answers with --------------------------------------------------------------

/// What each registered hook answers, by registration order (`Die` past the end).
#[derive(Resource, Default)]
struct Answers(Vec<Fate>);

/// Who was asked: the hook's index, the cause and the action of the blow.
#[derive(Resource, Default)]
struct Asked(Vec<(usize, LethalCause, Option<String>)>);

fn ask(world: &mut World, lethal: &Lethal, i: usize) -> Fate {
    world.get_resource_or_init::<Asked>().0.push((i, lethal.cause, lethal.action.clone()));
    world.get_resource::<Answers>().and_then(|a| a.0.get(i).copied()).unwrap_or(Fate::Die)
}

fn first(world: &mut World, lethal: &Lethal) -> Fate {
    ask(world, lethal, 0)
}

fn second(world: &mut World, lethal: &Lethal) -> Fate {
    ask(world, lethal, 1)
}

fn third(world: &mut World, lethal: &Lethal) -> Fate {
    ask(world, lethal, 2)
}

/// The hero (`l_hero`) at the origin facing north and the foe (`l_foe`) 1.8 m north of it facing
/// it; with `answers`, three lethal hooks answer them in order.
fn arena(answers: Option<&[Fate]>) -> (App, Entity, Entity) {
    let answers = answers.map(<[Fate]>::to_vec);
    let mut app = app_with(move |app| {
        if let Some(answers) = answers {
            app.insert_resource(Answers(answers)).init_resource::<Asked>().add_lethal_hook(first).add_lethal_hook(second).add_lethal_hook(third);
        }
    });
    let world = app.world_mut();
    let hero = hero(world, "l_hero", Vec3::ZERO, N);
    world.entity_mut(hero).insert(PlayerId(0));
    let foe = fighter(world, "Foe", "l_foe", "monsters", Vec3::new(0.0, 0.0, -1.8), S);
    step(&mut app, 2);
    (app, hero, foe)
}

/// A second player of the hero's team, 1 m east of it.
fn mate(app: &mut App, moveset: &str) -> Entity {
    let world = app.world_mut();
    let e = fighter(world, "Mate", moveset, "heroes", Vec3::new(1.0, 0.0, 0.0), N);
    world.entity_mut(e).insert(PlayerId(1));
    step(app, 1);
    e
}

/// The foe's lethal blow lands on the hero (pressed now, it lands 0.2 s later); the foe's swing is
/// over when this returns.
fn smash(app: &mut App, foe: Entity) {
    press(app, foe, "heavy");
    step(app, 40);
}

fn asked(app: &App) -> Vec<(usize, LethalCause, Option<String>)> {
    app.world().get_resource::<Asked>().map(|a| a.0.clone()).unwrap_or_default()
}

/// The tick of the first journal line containing `needle`.
fn tick_of(app: &App, needle: &str) -> u64 {
    let line = lines(app, needle).into_iter().next().unwrap_or_else(|| panic!("no `{needle}` in {:#?}", journal(app)));
    line.split(' ').next().unwrap().parse().unwrap()
}

fn downed(app: &App, e: Entity) -> Option<Downed> {
    app.world().get::<Downed>(e).copied()
}

// --- the hook --------------------------------------------------------------------------------------

#[test]
fn no_hook_kills_as_before() {
    let (mut plain, hero, foe) = arena(None);
    smash(&mut plain, foe);
    let (h, f) = (id(&plain, hero), id(&plain, foe));
    assert!(plain.world().get::<Dead>(hero).is_some() && downed(&plain, hero).is_none());
    assert_eq!(hp(&plain, hero), 0.0);
    assert_eq!(action(&plain, hero), "l_death");
    assert_eq!(lines(&plain, &format!("{h} died (killed by {f} with l_smash)")).len(), 1, "{:#?}", journal(&plain));
    // Hooks that all answer `Die` change nothing: the same journal, line for line.
    let (mut vetoless, hero2, foe2) = arena(Some(&[Fate::Die, Fate::Die, Fate::Die]));
    smash(&mut vetoless, foe2);
    assert_eq!(journal(&plain), journal(&vetoless));
    assert_eq!(asked(&vetoless).len(), 3, "every hook is asked when each says Die");
    assert!(vetoless.world().get::<Dead>(hero2).is_some());
}

#[test]
fn first_non_die_answer_wins() {
    let answers = [Fate::Die, Fate::Survive { health: 10.0, grace: 0.5 }, Fate::Down { bleed: None }];
    let (mut app, hero, foe) = arena(Some(&answers));
    smash(&mut app, foe);
    let blow = Some("l_smash".to_owned());
    assert_eq!(asked(&app), [(0, LethalCause::Hit, blow.clone()), (1, LethalCause::Hit, blow)], "the third hook is never asked");
    assert_eq!(hp(&app, hero), 10.0);
    assert!(app.world().get::<Dead>(hero).is_none() && downed(&app, hero).is_none());
}

#[test]
fn survive_keeps_the_hits_rest() {
    let (mut app, hero, foe) = arena(Some(&[Fate::Survive { health: 1.0, grace: 1.0 }]));
    smash(&mut app, foe);
    let (h, f) = (id(&app, hero), id(&app, foe));
    assert_eq!(hp(&app, hero), 1.0);
    assert_eq!(lines(&app, &format!("{h} survived a lethal hit (hp 1)")).len(), 1, "{:#?}", journal(&app));
    assert!(lines(&app, "died").is_empty());
    // The rest of the blow: its status, its poise (the break staggers).
    let slow = app.world().get::<Statuses>(hero).and_then(|s| s.get("slow")).map(|e| e.active);
    assert!(slow.is_some_and(|a| a > 0.0), "the blow's status applies: {slow:?}");
    assert!(!lines(&app, &format!("{h} staggered (poise broken)")).is_empty(), "{:#?}", journal(&app));
    let grace = app.world().get::<Grace>(hero).map(|g| g.left).unwrap();
    assert!(grace > 0.4 && grace < 1.0, "grace {grace}");
    // Inside the grace a blow does nothing.
    press(&mut app, foe, "light");
    step(&mut app, 16);
    assert!(!lines(&app, &format!("{f} -> {h} l_slash -0 (hp 1, grace)")).is_empty(), "{:#?}", journal(&app));
    assert_eq!(hp(&app, hero), 1.0);
    // It runs out a second after the survive.
    step(&mut app, 40);
    assert!(app.world().get::<Grace>(hero).is_none());
    let shown = ok(&mut app, "inspect", json!({"id": h}));
    assert!(!shown.contains("grace"), "{shown}");
}

#[test]
fn down_ignores_hits_and_blocks_actions() {
    let (mut app, hero, foe) = arena(Some(&[Fate::Down { bleed: None }]));
    shards_action::add_status_buildup(app.world_mut(), hero, "slow", 200.0);
    step(&mut app, 1);
    smash(&mut app, foe);
    let (h, f) = (id(&app, hero), id(&app, foe));
    let d = downed(&app, hero).expect("down");
    assert_eq!(d.bleed, None);
    assert_eq!(d.by, app.world().get::<ai_core::AiId>(foe).copied());
    assert!(app.world().get::<Dead>(hero).is_none());
    assert_eq!(hp(&app, hero), 0.0);
    assert_eq!(action(&app, hero), "l_down");
    assert_eq!(lines(&app, &format!("{h} is down")).len(), 1, "{:#?}", journal(&app));
    assert!(lines(&app, "died").is_empty());
    // Its statuses are gone (the active one expired), and the blow's own never applied.
    assert!(!lines(&app, &format!("{h} slow expired")).is_empty(), "{:#?}", journal(&app));
    assert!(app.world().get::<Statuses>(hero).is_none_or(|s| s.0.is_empty()));
    // Nothing starts, the body does not walk, hits pass over it.
    let refused = fails(&mut app, "light_attack", json!({}));
    assert!(refused.contains("you are down"), "{refused}");
    let loco = app.world().get::<shards_motion::LocomotionInput>(hero).unwrap();
    assert!(loco.locked, "the downed pose owns the body");
    press(&mut app, foe, "light");
    step(&mut app, 16);
    assert!(!lines(&app, &format!("{f} -> {h} l_slash -0 (hp 0, downed)")).is_empty(), "{:#?}", journal(&app));
    assert!(lines(&app, &format!("{h} reacts")).is_empty());
    let shown = ok(&mut app, "inspect", json!({"id": h}));
    assert!(shown.contains("downed"), "{shown}");
    // A moveset without a downed pose: its action is cut.
    let other = mate(&mut app, "l_plain");
    press(&mut app, other, "light");
    step(&mut app, 2);
    assert!(go_down(app.world_mut(), other, Some(3.0), None));
    assert_eq!(action(&app, other), "idle");
    assert!(lines(&app, &format!("{} is down (bleeds out in 3.0s)", id(&app, other))).len() == 1);
}

#[test]
fn bleed_out_on_the_tick() {
    let (mut app, hero, foe) = arena(Some(&[Fate::Down { bleed: Some(1.0) }]));
    smash(&mut app, foe);
    let (h, f) = (id(&app, hero), id(&app, foe));
    let down = tick_of(&app, &format!("{h} is down (bleeds out in 1.0s)"));
    step(&mut app, 70);
    let died = tick_of(&app, &format!("{h} died (killed by {f})"));
    assert_eq!(died, down + 60, "{:#?}", journal(&app));
    assert!(app.world().get::<Dead>(hero).is_some() && downed(&app, hero).is_none());
    assert_eq!(action(&app, hero), "l_death");
    // A bleed-out asks no hook: it is the death the downed state stood for.
    assert_eq!(asked(&app).len(), 1);
}

#[test]
fn long_timers_end_on_their_tick() {
    // 20 s of bleed-out and 20 s of grace: 1200 ticks each, whatever the drift of the count.
    let (mut app, hero, foe) = arena(Some(&[Fate::Down { bleed: Some(20.0) }]));
    smash(&mut app, foe);
    let h = id(&app, hero);
    let down = tick_of(&app, &format!("{h} is down (bleeds out in 20.0s)"));
    step(&mut app, 1200);
    assert_eq!(tick_of(&app, &format!("{h} died")), down + 1200, "{:#?}", journal(&app));
    // Granted between ticks: the next 1200 ticks.
    shards_action::grant_grace(app.world_mut(), foe, 20.0);
    step(&mut app, 1199);
    assert!(app.world().get::<Grace>(foe).is_some(), "still there on its last tick");
    step(&mut app, 1);
    assert!(app.world().get::<Grace>(foe).is_none());
}

#[test]
fn kill_from_tools_skips_the_hook() {
    let (mut app, hero, foe) = arena(Some(&[Fate::Down { bleed: None }]));
    kill(app.world_mut(), foe, None, None);
    assert!(app.world().get::<Dead>(foe).is_some());
    assert!(asked(&app).is_empty(), "kill is outright");
    // Killing a downed body outright: dead, no longer down.
    smash_on(&mut app, hero);
    assert!(downed(&app, hero).is_some());
    kill(app.world_mut(), hero, None, None);
    assert!(app.world().get::<Dead>(hero).is_some() && downed(&app, hero).is_none());
    step(&mut app, 2);
}

/// Brings `e` down with no blow (a game's own `go_down`).
fn smash_on(app: &mut App, e: Entity) {
    assert!(go_down(app.world_mut(), e, None, None));
    step(app, 1);
}

#[test]
fn check_deaths_ignores_downed() {
    let (mut app, hero, foe) = arena(Some(&[Fate::Die]));
    smash_on(&mut app, hero);
    step(&mut app, 120);
    assert!(downed(&app, hero).is_some() && app.world().get::<Dead>(hero).is_none(), "{:#?}", journal(&app));
    assert!(asked(&app).is_empty(), "a downed body is not dying");
    // Health run out some other way asks the hooks with its cause.
    app.world_mut().get_mut::<Health>(foe).unwrap().current = 0.0;
    step(&mut app, 1);
    assert_eq!(asked(&app), [(0, LethalCause::Health, None), (1, LethalCause::Health, None), (2, LethalCause::Health, None)]);
    assert!(app.world().get::<Dead>(foe).is_some());
}

#[test]
fn deaths_chain_is_ordered() {
    // The bleed-out comes before `check_deaths` in the same tick, whatever the ids: the foe (the
    // lower id) has no health left in the tick the mate bleeds out, and dies after it.
    let (mut app, _hero, foe) = arena(None);
    let mate = mate(&mut app, "l_hero");
    assert!(go_down(app.world_mut(), mate, Some(0.0), None));
    app.world_mut().get_mut::<Health>(foe).unwrap().current = 0.0;
    step(&mut app, 1);
    let all = journal(&app);
    let at = |needle: String| all.iter().position(|l| l.ends_with(&needle)).unwrap_or_else(|| panic!("no `{needle}` in {all:#?}"));
    assert!(at(format!("{} died", id(&app, mate))) < at(format!("{} died", id(&app, foe))));
    assert!(app.world().get::<ai_core::AiId>(foe) < app.world().get::<ai_core::AiId>(mate));
}

// --- the revive ------------------------------------------------------------------------------------

/// The mate goes down next to the hero; the hero holds interact: the revive takes it.
fn reviving(answers: &[Fate]) -> (App, Entity, Entity, Entity) {
    let (mut app, hero, foe) = arena(Some(answers));
    let mate = mate(&mut app, "l_hero");
    smash_on(&mut app, mate);
    let out = ok(&mut app, "interact", json!({"hold": true}));
    assert!(out.contains("ok: l_revive"), "{out}");
    step(&mut app, 2);
    assert_eq!(action(&app, hero), "l_revive");
    assert_eq!(app.world().get::<Pairing>(hero).map(|p| p.victim), app.world().get::<ai_core::AiId>(mate).copied());
    assert!(app.world().get::<Paired>(mate).is_some());
    (app, hero, foe, mate)
}

#[test]
fn revive_pair_restores_health_and_grace() {
    let (mut app, hero, _foe, mate) = reviving(&[]);
    let (h, m) = (id(&app, hero), id(&app, mate));
    assert_eq!(lines(&app, &format!("{h} takes {m} with l_revive")).len(), 1, "{:#?}", journal(&app));
    step(&mut app, 60);
    assert_eq!(lines(&app, &format!("{m} revived by {h}")).len(), 1, "{:#?}", journal(&app));
    assert!(downed(&app, mate).is_none() && app.world().get::<Dead>(mate).is_none());
    assert!((hp(&app, mate) - 30.0).abs() < 1e-3, "30 % of 100: {}", hp(&app, mate));
    let grace = app.world().get::<Grace>(mate).map(|g| g.left).unwrap();
    assert!(grace > 0.9 && grace <= 1.0, "grace {grace}");
    assert_ne!(action(&app, mate), "l_down", "the pose is cut");
    assert_eq!(app.world().get::<Poise>(mate).map(|p| p.current), app.world().get::<Poise>(mate).map(|p| p.max));
    step(&mut app, 2);
    assert!(app.world().get::<Pairing>(hero).is_none() && app.world().get::<Paired>(mate).is_none());
    assert!(!lines(&app, &format!("{h} lets go of {m} (finished)")).is_empty(), "{:#?}", journal(&app));
}

#[test]
fn releasing_interact_aborts() {
    let (mut app, hero, _foe, mate) = reviving(&[]);
    step(&mut app, 20);
    let out = ok(&mut app, "interact", json!({"hold": false}));
    assert!(out.contains("released"), "{out}");
    step(&mut app, 2);
    assert_ne!(action(&app, hero), "l_revive");
    assert!(!lines(&app, &format!("{} lets go of {} (cut)", id(&app, hero), id(&app, mate))).is_empty(), "{:#?}", journal(&app));
    step(&mut app, 60);
    assert!(downed(&app, mate).is_some() && hp(&app, mate) == 0.0);
    assert!(lines(&app, "revived").is_empty());
}

#[test]
fn moving_away_aborts() {
    let (mut app, hero, _foe, mate) = reviving(&[]);
    step(&mut app, 10);
    let tf = Transform::from_xyz(4.0, 0.0, 0.0);
    app.world_mut().entity_mut(mate).insert(tf);
    step(&mut app, 2);
    assert!(!lines(&app, &format!("{} lets go of {} (lost)", id(&app, hero), id(&app, mate))).is_empty(), "{:#?}", journal(&app));
    assert_ne!(action(&app, hero), "l_revive", "the revive is cut");
    step(&mut app, 60);
    assert!(downed(&app, mate).is_some());
}

#[test]
fn damage_aborts() {
    let (mut app, hero, foe, mate) = reviving(&[]);
    press(&mut app, foe, "light");
    step(&mut app, 16);
    let (h, f) = (id(&app, hero), id(&app, foe));
    assert!(!lines(&app, &format!("{f} -> {h} l_slash -10")).is_empty(), "{:#?}", journal(&app));
    assert!(!lines(&app, &format!("{h} lets go of {} (cut)", id(&app, mate))).is_empty(), "{:#?}", journal(&app));
    assert_ne!(action(&app, hero), "l_revive");
    step(&mut app, 60);
    assert!(downed(&app, mate).is_some());
}

#[test]
fn no_downed_ally_no_revive() {
    // Interact binds the revive only while a downed ally lies within its reach.
    let (mut app, _hero, _foe) = arena(None);
    let refused = fails(&mut app, "interact", json!({"hold": true}));
    assert!(refused.contains("has no `interact`"), "{refused}");
    let mate = mate(&mut app, "l_hero");
    smash_on(&mut app, mate);
    assert!(ok(&mut app, "interact", json!({"hold": true})).contains("ok: l_revive"));
    // A downed body is no finisher's victim and no lock-on target.
    let m = id(&app, mate);
    let refused = fails(&mut app, "attack", json!({"target": m}));
    assert!(refused.contains("is down"), "{refused}");
}

#[test]
fn heal_stands_a_downed_body_up_and_bleed_out_kills_it() {
    let (mut app, hero, _foe) = arena(None);
    let mate = mate(&mut app, "l_hero");
    smash_on(&mut app, mate);
    let m = id(&app, mate);
    let out = ok(&mut app, "heal", json!({"id": m}));
    assert!(out.contains("revived"), "{out}");
    assert!(downed(&app, mate).is_none() && hp(&app, mate) == 100.0);
    assert_eq!(lines(&app, &format!("{} revived", id(&app, mate))).len(), 1);
    // The dev tool, and a game's bleed-out.
    let out = ok(&mut app, "down", json!({"id": m, "bleed": 4}));
    assert!(out.contains("is down (bleeds out in 4.0s)"), "{out}");
    assert!(bleed_out(app.world_mut(), mate));
    assert!(app.world().get::<Dead>(mate).is_some() && downed(&app, mate).is_none());
    assert!(!bleed_out(app.world_mut(), hero), "not down: nothing happens");
}

#[test]
fn a_downed_body_is_refused_the_movement_tools_and_loses_its_orders() {
    // As a dead body is (FU2): the order it had ends when it goes down, and the movement tools,
    // which the motion module owns, refuse it the way its action tools do.
    let (mut app, hero, _foe) = arena(None);
    ok(&mut app, "move_to", json!({"target": {"x": 5.0, "z": 0.0}}));
    assert_eq!(app.world().get::<shards_core::Intent>(hero).unwrap().orders.len(), 1);
    assert!(go_down(app.world_mut(), hero, None, None));
    let intent = app.world().get::<shards_core::Intent>(hero).unwrap();
    assert!(intent.orders.is_empty() && intent.move_dir == Vec3::ZERO, "going down drops the orders: {intent:?}");
    let tools = [
        ("move_to", json!({"target": {"x": 5.0, "z": 0.0}})),
        ("move", json!({"dir": "E", "meters": 3})),
        ("stop", json!({})),
        ("jump", json!({})),
        ("sprint", json!({})),
        ("glide", json!({})),
    ];
    for (tool, args) in tools {
        let refused = fails(&mut app, tool, args);
        assert!(refused.starts_with("you are down"), "{tool}: {refused}");
    }
    assert!(app.world().get::<shards_core::Intent>(hero).unwrap().orders.is_empty());
    // Stood up, nothing runs on its own and the tools work again.
    let h = id(&app, hero);
    ok(&mut app, "heal", json!({"id": h}));
    step(&mut app, 10);
    assert!(app.world().get::<Transform>(hero).unwrap().translation.x.abs() < 1e-3, "nothing ran after the heal");
    ok(&mut app, "move", json!({"dir": "E", "meters": 3}));
    assert_eq!(app.world().get::<shards_core::Intent>(hero).unwrap().orders.len(), 1);
}

// --- a fallen attacker and its pair's victim (13a, B3 as built) -------------------------------------

/// The `c_hero` finisher has taken the broken `c_foe` (its poise broken, held in `c_held`), a few ticks
/// before its strike.
fn finisher_holding() -> (App, Entity, Entity) {
    let mut app = app();
    let world = app.world_mut();
    let hero = hero(world, "c_hero", Vec3::ZERO, N);
    let foe = fighter(world, "Foe", "c_foe", "monsters", Vec3::new(0.0, 0.0, -1.8), S);
    step(&mut app, 2);
    let f = id(&app, foe);
    let out = ok(&mut app, "stagger", json!({"id": f, "poise": true}));
    assert!(out.contains("poise broken"), "{out}");
    step(&mut app, 1);
    press(&mut app, hero, "light");
    step(&mut app, 1);
    assert_eq!(action(&app, hero), "c_finisher");
    assert_eq!(action(&app, foe), "c_held");
    step(&mut app, 3);
    (app, hero, foe)
}

fn the_victim_keeps_the_rest_of_its_break(fall: fn(&mut World, Entity)) {
    let (mut app, hero, foe) = finisher_holding();
    let broken = app.world().get::<Poise>(foe).unwrap().broken;
    let staggers = lines(&app, &format!("{} staggered", id(&app, foe))).len();
    fall(app.world_mut(), hero);
    // What the observers queued runs at the end of the system that killed it (here, now).
    app.world_mut().flush();
    // Back in its stagger, at once, for exactly the rest of the break: no fresh break, no early end.
    assert_eq!(action(&app, foe), "c_stagger", "{:#?}", journal(&app));
    let poise = *app.world().get::<Poise>(foe).unwrap();
    assert_eq!(poise.broken, broken, "the break is the one it had");
    let run = app.world().get::<ActionState>(foe).unwrap().current.clone().unwrap();
    assert!((run.length - run.t - broken).abs() < 1e-4, "the stagger ends with the break: {} left, {broken} broken", run.length - run.t);
    assert!(app.world().get::<Paired>(foe).is_none());
    assert!(!lines(&app, &format!("{} lets go of {} (cut)", id(&app, hero), id(&app, foe))).is_empty(), "{:#?}", journal(&app));
    assert_eq!(lines(&app, &format!("{} staggered", id(&app, foe))).len(), staggers, "no new stagger");
    // The two end on the same tick.
    let ticks = (broken / DT).round() as u64;
    step(&mut app, ticks - 1);
    assert_eq!(action(&app, foe), "c_stagger");
    assert!(app.world().get::<Poise>(foe).unwrap().is_broken());
    step(&mut app, 2);
    assert_ne!(action(&app, foe), "c_stagger");
    assert!(!app.world().get::<Poise>(foe).unwrap().is_broken());
}

#[test]
fn the_attacker_dies_mid_pair() {
    the_victim_keeps_the_rest_of_its_break(|world, hero| kill(world, hero, None, None));
}

#[test]
fn the_attacker_goes_down_mid_pair() {
    the_victim_keeps_the_rest_of_its_break(|world, hero| {
        assert!(go_down(world, hero, None, None));
    });
}
