//! Determinism: the same seed gives the same decisions and journal; a recorded NPC fight
//! plays back identically; every system has a total order, with a game's brain beside ours.

use ai_core::replay::{self, Replay};
use ai_core::{AiId, AiReplay, SimSet};
use bevy::ecs::schedule::{LogLevel, ScheduleBuildSettings};
use bevy::prelude::*;
use serde_json::json;
use shards_action::prelude::*;
use shards_action::ActionState;
use shards_core::Intent;
use shards_npc::prelude::*;
use shards_npc::NpcPlan;

use crate::common::*;

/// A hero, three grunts and a warden.
fn arena(app: &mut App) -> (Entity, Vec<Entity>) {
    let world = app.world_mut();
    let hero = player(world, 0, Vec3::ZERO);
    let mut npcs: Vec<Entity> = [Vec3::new(5.0, 0.0, 1.0), Vec3::new(-4.0, 0.0, 4.0), Vec3::new(0.0, 0.0, -6.0)]
        .into_iter()
        .map(|at| grunt(world, "grunt", "grunt", "default", at))
        .collect();
    let post = Vec3::new(8.0, 0.0, -8.0);
    let warden = npc(world, "warden", Npc::new("warden", "boss").with_plan("warden").with_director("boss"), post, S);
    home(world, warden, post, 12.0);
    npcs.push(warden);
    (hero, npcs)
}

/// The hero plays through tools and the keyboard: walks, rolls, swings.
fn play(app: &mut App, hero: Entity) {
    step(app, 60);
    ok(app, "light_attack", json!({}));
    step(app, 90);
    ok(app, "move", json!({"dir": "E", "meters": 3}));
    step(app, 120);
    ok(app, "dodge", json!({"dir": "N"}));
    step(app, 60);
    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = Vec3::new(-0.5, 0.0, 0.0);
    step(app, 90);
    app.world_mut().get_mut::<Intent>(hero).unwrap().move_dir = Vec3::ZERO;
    ok(app, "light_attack", json!({}));
    step(app, 300);
}

/// Every NPC's gameplay state, bit-exact.
fn npc_state(app: &mut App) -> Vec<String> {
    let world = app.world_mut();
    let mut q = world.query::<(&AiId, &Transform, &NpcBrain, &AttackToken, &NpcPlan, &NavAgent, &Perceived, &ActionState)>();
    let mut out: Vec<String> = q.iter(world).map(|c| format!("{c:?}")).collect();
    out.sort();
    out
}

#[test]
fn the_same_seed_gives_the_same_decisions() {
    let (mut a, mut b) = (app(), app());
    let (ha, _) = arena(&mut a);
    let (hb, _) = arena(&mut b);
    play(&mut a, ha);
    play(&mut b, hb);
    assert_eq!(npc_state(&mut a), npc_state(&mut b));
    assert_eq!(journal(&a), journal(&b));
    let story = journal(&a);
    for needle in ["takes an attack token", "returns its attack token", "idle → fight", "guard → fight"] {
        assert!(story.iter().any(|l| l.contains(needle)), "no `{needle}` in {story:#?}");
    }
    // Another seed: other rolls, another fight.
    let mut c = app_seeded(8, |_| {});
    let (hc, _) = arena(&mut c);
    play(&mut c, hc);
    assert_ne!(journal(&a), journal(&c), "the decisions come from the seed");
}

#[test]
fn a_recorded_npc_fight_plays_back_identically() {
    let mut original = app();
    let (hero, _) = arena(&mut original);
    play(&mut original, hero);
    let recording = replay::save(original.world());
    let recording: Replay = serde_json::from_str(&serde_json::to_string(&recording).unwrap()).unwrap();
    assert!(recording.checks.len() >= 10, "a check every second: {}", recording.checks.len());

    let mut copy = app();
    arena(&mut copy);
    replay::play(copy.world_mut(), recording.clone()).unwrap();
    step(&mut copy, recording.end.tick);
    copy.update();
    let report = copy.world().resource::<AiReplay>().report().cloned().expect("playback finished");
    assert!(report.identical(), "{report}");
    assert_eq!(npc_state(&mut original), npc_state(&mut copy));
    assert_eq!(journal(&original), journal(&copy));
    // Every NPC component goes into the checks: none is left out for not serializing.
    let left_out: Vec<String> = all_journal(&original).into_iter().filter(|l| l.contains("replay checks leave out")).collect();
    assert!(left_out.is_empty(), "{left_out:#?}");
}

/// The checks see the NPCs' minds: a brain changed between recording and playback diverges.
#[test]
fn a_changed_npc_mind_diverges() {
    let mut original = app();
    let (hero, _) = arena(&mut original);
    play(&mut original, hero);
    let recording = replay::save(original.world());
    let mut copy = app();
    let (_, npcs) = arena(&mut copy);
    copy.world_mut().get_mut::<NpcPlan>(npcs[0]).unwrap().rest = 0.5;
    replay::play(copy.world_mut(), recording.clone()).unwrap();
    step(&mut copy, recording.end.tick);
    copy.update();
    let report = copy.world().resource::<AiReplay>().report().cloned().unwrap();
    assert_eq!(report.diverged_at, Some(60), "{report}");
}

/// A game's own brain in `SimSet::Think` that reads the NPCs and writes `Intent`s, and a
/// game system after the hits: no ambiguity with the NPC module.
fn game_brain(npcs: Query<&NpcBrain>, mut intents: Query<&mut Intent, Without<Npc>>) {
    let fighting = npcs.iter().filter(|b| b.state == "fight").count();
    for mut intent in &mut intents {
        if fighting > 99 {
            intent.move_dir = Vec3::ZERO;
        }
    }
}

fn game_aftermath(mut q: Query<&mut Health>) {
    for mut h in &mut q {
        if h.current < 0.0 {
            h.current = 0.0;
        }
    }
}

#[test]
fn gameplay_systems_have_a_total_order() {
    let mut app = app_with(|app| {
        app.add_systems(
            FixedUpdate,
            (game_brain.in_set(SimSet::Think), game_aftermath.in_set(SimSet::Resolve).after(ActionSystems::Deaths)),
        );
    });
    let strict = ScheduleBuildSettings { ambiguity_detection: LogLevel::Error, ..default() };
    let result = app.world_mut().schedule_scope(FixedUpdate, |world, schedule| {
        schedule.set_build_settings(strict);
        schedule.initialize(world).map(|_| ()).map_err(|e| e.to_string(schedule.graph(), world))
    });
    if let Err(e) = result {
        panic!("ambiguous FixedUpdate systems make replays diverge:\n{e}");
    }
}
