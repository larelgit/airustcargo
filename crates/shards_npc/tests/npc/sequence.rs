//! Sequences (F12): a scripted scene on NPCs, tick by tick. The brains are dormant while a run holds
//! them and go on after; a game loaded in the middle of a scene resumes it. The sandbox plays a scene
//! with a voiced line and a subtitle (`tests/sandbox/sequence.rs`).

use ai_core::snapshot::{self, differences};
use ai_core::{AiAppExt, AiJournal, JournalKind, SimTick};
use bevy::prelude::*;
use shards_action::{ActionSound, Dead};
use shards_content::db::{read_dir, Source};
use shards_content::{ContentDb, ContentKinds};
use shards_level::{LevelMarker, MarkerKind};
use shards_npc::{
    play_sequence, stop_sequence, NpcDrive, SequenceEnded, SequenceHold, SequenceId, SequenceSignal, SequenceStarted, SequenceStep, Sequences,
};
use shards_npc::prelude::*;

use crate::common::*;

/// The tick the condition `scene_go` starts to hold at.
const GO: u64 = 400;

/// What the observers saw, with the tick.
#[derive(Resource, Default)]
struct Log {
    started: Vec<(u64, SequenceStarted)>,
    steps: Vec<(u64, SequenceStep)>,
    signals: Vec<(u64, SequenceSignal)>,
    ended: Vec<(u64, SequenceEnded)>,
    sounds: Vec<(u64, ActionSound)>,
}

fn scene_app() -> App {
    app_with(|app| {
        app.init_resource::<Log>()
            .add_ai_condition("scene_go", |w| ai_core::sim::tick(w) >= GO)
            .add_observer(|e: On<SequenceStarted>, t: Res<SimTick>, mut log: ResMut<Log>| log.started.push((t.0, e.event().clone())))
            .add_observer(|e: On<SequenceStep>, t: Res<SimTick>, mut log: ResMut<Log>| log.steps.push((t.0, e.event().clone())))
            .add_observer(|e: On<SequenceSignal>, t: Res<SimTick>, mut log: ResMut<Log>| log.signals.push((t.0, e.event().clone())))
            .add_observer(|e: On<SequenceEnded>, t: Res<SimTick>, mut log: ResMut<Log>| log.ended.push((t.0, e.event().clone())))
            .add_observer(|e: On<ActionSound>, t: Res<SimTick>, mut log: ResMut<Log>| log.sounds.push((t.0, e.event().clone())));
    })
}

fn marker(world: &mut World, name: &str, at: Vec3) {
    world.spawn((LevelMarker { kind: MarkerKind::Custom("scene".into()), name: name.into(), to: None }, Transform::from_translation(at)));
}

/// The markers of the scenes, in the order the world makes them.
fn markers(world: &mut World) {
    marker(world, "mark_a", Vec3::new(6.0, 0.0, 0.0));
    marker(world, "mark_b", Vec3::new(0.0, 0.0, -10.0));
    marker(world, "mark_far", Vec3::new(50.0, 0.0, 0.0));
}

/// The listener (the player, e1) 8 m south of the first marker and the actor (e2), a watcher at the origin facing east.
fn stage(app: &mut App) -> (Entity, Entity) {
    let world = app.world_mut();
    markers(world);
    let listener = player(world, 0, Vec3::new(6.0, 0.0, 8.0));
    let actor = npc(world, "grunt", Npc::new("watcher", "grunt"), Vec3::ZERO, E);
    (actor, listener)
}

fn log(app: &App) -> &Log {
    app.world().resource::<Log>()
}

/// Runs ticks until `done` holds, at most `max`.
fn until(app: &mut App, max: u64, mut done: impl FnMut(&App) -> bool) -> u64 {
    for n in 0..max {
        if done(app) {
            return n;
        }
        step(app, 1);
    }
    panic!("not done in {max} ticks; journal: {:#?}", all_journal(app));
}

fn drive(app: &App, e: Entity) -> NpcDrive {
    app.world().get::<NpcDrive>(e).unwrap().clone()
}

fn facing(app: &App, e: Entity) -> Vec3 {
    let f = *app.world().get::<Transform>(e).unwrap().forward();
    Vec3::new(f.x, 0.0, f.z).normalize()
}

#[test]
fn a_scene_walks_turns_plays_says_waits_and_signals_in_order_and_frees_the_npc() {
    let mut app = scene_app();
    let (actor, listener) = stage(&mut app);
    let run = play_sequence(app.world_mut(), "scene", &[("actor", actor), ("listener", listener)]).unwrap();
    assert_eq!(run, SequenceId(1));
    let hold = app.world().get::<SequenceHold>(actor).cloned().unwrap();
    assert_eq!((hold.run, hold.def.as_str(), hold.role.as_str()), (1, "scene", "actor"));
    assert!(app.world().get::<SequenceHold>(listener).is_none(), "a body that is only looked at is not held");
    assert_eq!(log(&app).started.len(), 1);

    // 1. It walks to the first marker, at a walk, by its own navigation.
    step(&mut app, 1);
    assert_eq!(log(&app).steps.len(), 1);
    let first = &log(&app).steps[0].1;
    assert_eq!((first.index, first.of, first.kind), (0, 7, "MoveTo"));
    assert_eq!(first.actor, Some(aid(&app, actor)));
    let d = drive(&app, actor);
    assert_eq!((d.goal, d.speed, d.within), (Some(Vec3::new(6.0, 0.0, 0.0)), 0.5, 0.4));
    assert!(!d.request_token && !d.strike && d.press.is_empty() && d.face.is_none());
    until(&mut app, 900, |a| log(a).steps.len() >= 2);
    assert!(flat_distance(feet(&app, actor), Vec3::new(6.0, 0.0, 0.0)) <= 0.5, "arrived: {}", feet(&app, actor));
    assert_eq!(drive(&app, actor).goal, None, "and stopped asking to go anywhere");

    // 2. It turns to face the listener, 8 m south, and keeps facing it.
    assert_eq!(log(&app).steps[1].1.kind, "Face");
    assert_eq!(drive(&app, actor).face, Some(feet(&app, listener)));
    until(&mut app, 300, |a| log(a).steps.len() >= 3);
    // Within 5 degrees of the listener from where it stands, which is within 0.4 m of the marker.
    assert!(facing(&app, actor).dot(Vec3::Z) > 0.99, "faces south: {}", facing(&app, actor));

    // 3. It gestures: the action runs, and the step ends when it has.
    assert_eq!(log(&app).steps[2].1.kind, "Play");
    assert_eq!(action(&app, actor).as_deref(), Some("gesture"));
    until(&mut app, 120, |a| log(a).steps.len() >= 4);
    let played = log(&app).steps[3].0 - log(&app).steps[2].0;
    assert!((17..=21).contains(&played), "the action takes 0.3 s: {played} ticks");

    // 4. It says its line: the event the audio module plays an action's sound by, from the actor.
    assert_eq!(log(&app).steps[3].1.kind, "Sound");
    let [(_, said)] = &log(&app).sounds[..] else { panic!("one sound: {:?}", log(&app).sounds) };
    assert_eq!((said.entity, said.sound.as_str(), said.action.as_str()), (actor, "line", "sequence scene"));

    // 5. Half a second, to the tick; then it waits for the condition, which holds at tick GO.
    until(&mut app, 60, |a| log(a).steps.len() >= 6);
    assert_eq!(log(&app).steps[5].0 - log(&app).steps[4].0, 30);
    assert_eq!(log(&app).steps[4].1.kind, "Wait");
    assert_eq!(log(&app).steps[5].1.kind, "WaitFor");
    assert!(ai_core::sim::tick(app.world()) < GO, "the test is quick enough to wait: {}", ai_core::sim::tick(app.world()));
    step(&mut app, 20);
    assert_eq!(log(&app).steps.len(), 6, "still waiting");
    assert!(log(&app).ended.is_empty());

    // 6. The signal, and the end: the hold goes and the actor is its own again.
    until(&mut app, 600, |a| !log(a).ended.is_empty());
    let [(at, ended)] = &log(&app).ended[..] else { panic!("{:?}", log(&app).ended) };
    assert_eq!((ended.skipped, ended.why.clone(), ended.step, ended.of), (false, None, 7, 7));
    assert!(*at >= GO && *at <= GO + 1, "ended when the condition held: {at}");
    let [(signalled, signal)] = &log(&app).signals[..] else { panic!("{:?}", log(&app).signals) };
    assert_eq!((signal.name.as_str(), *signalled), ("done", *at));
    assert!(app.world().get::<SequenceHold>(actor).is_none());
    assert!(app.world().resource::<Sequences>().runs.is_empty());
    assert_eq!(drive(&app, actor), NpcDrive::default(), "nothing of the scene's is left in what it is told");

    // The journal tells it.
    let said = journal(&app);
    for line in [
        "sequence 1 (scene) starts: actor=e2 listener=e1",
        "sequence 1 (scene) 1/7: e2 walks to `mark_a`",
        "sequence 1 (scene) 2/7: e2 turns to the listener",
        "sequence 1 (scene) 3/7: e2 plays `gesture`",
        "sequence 1 (scene) 4/7: e2 says `line`",
        "sequence 1 (scene) 5/7: waits 0.5 s",
        "sequence 1 (scene) 6/7: waits for `scene_go`",
        "sequence 1 (scene) 7/7: signals `done`",
        "sequence 1 (scene) ended",
    ] {
        assert!(said.iter().any(|l| l.ends_with(line)), "no `{line}` in {said:#?}");
    }
    // The next run takes the next number.
    let again = play_sequence(app.world_mut(), "stand", &[("actor", actor)]).unwrap();
    assert_eq!(again, SequenceId(2));
}

#[test]
fn a_held_brain_chooses_nothing_and_goes_on_from_where_it_was() {
    // The control: the grunt sees the player 8 m away, asks for a token and fights.
    let fight = |hold: bool| {
        let mut app = scene_app();
        markers(app.world_mut());
        let hero = player(app.world_mut(), 0, Vec3::new(0.0, 0.0, 30.0));
        let me = grunt(app.world_mut(), "grunt", "grunt", "default", Vec3::ZERO);
        step(&mut app, 3);
        assert_eq!(state(&app, me), "idle", "it has not seen anyone yet");
        let before = app.world().get::<NpcBrain>(me).unwrap().clone();
        if hold {
            play_sequence(app.world_mut(), "stand", &[("actor", me)]).unwrap();
        }
        teleport(&mut app, hero, Vec3::new(0.0, 0.0, 6.0));
        (app, me, before)
    };
    let (mut free, free_grunt, _) = fight(false);
    step(&mut free, 20);
    assert_eq!(state(&free, free_grunt), "fight", "{:#?}", journal(&free));

    let (mut held, me, before) = fight(true);
    // Face + 3 s: the actor's brain sees the player, and does nothing about it.
    step(&mut held, 100);
    let brain = held.world().get::<NpcBrain>(me).unwrap().clone();
    assert!(held.world().get::<Perceived>(me).unwrap().known.iter().any(|k| k.seen), "perception goes on");
    assert_eq!(brain.target, before.target, "what it acts on is the brain's, frozen with it");
    assert_eq!((brain.state.as_str(), brain.since, brain.timer, brain.step), (before.state.as_str(), before.since, before.timer, before.step), "frozen");
    assert!(!has_token(&held, me), "no attack token");
    let d = drive(&held, me);
    assert!(!d.request_token && !d.strike && !d.waiting && d.press.is_empty());
    assert!(!attacking(&held, me), "{:?}", action(&held, me));
    assert!(lines(&held, "attack token").is_empty(), "{:#?}", journal(&held));
    // Held it stays where it stands and faces the marker it was told to face, away from the player.
    assert!(flat_distance(feet(&held, me), Vec3::ZERO) < 0.2, "{}", feet(&held, me));
    assert!(facing(&held, me).dot(Vec3::NEG_Z) > 0.99, "{}", facing(&held, me));
    // The scene ends (3 s after the turn) and the brain takes up its fight.
    until(&mut held, 400, |a| !log(a).ended.is_empty());
    assert!(!app_has_hold(&held, me));
    step(&mut held, 20);
    assert_eq!(state(&held, me), "fight", "{:#?}", journal(&held));
    assert!(!lines(&held, "idle → fight").is_empty());
}

fn app_has_hold(app: &App, e: Entity) -> bool {
    app.world().get::<SequenceHold>(e).is_some()
}

#[test]
fn a_run_that_cannot_be_played_says_what_to_fix_and_changes_nothing() {
    let mut app = scene_app();
    let (actor, listener) = stage(&mut app);
    let dead = npc(app.world_mut(), "grunt", Npc::new("watcher", "grunt"), Vec3::new(-5.0, 0.0, 0.0), E);
    app.world_mut().entity_mut(dead).insert(Dead);
    step(&mut app, 2);
    let play = |app: &mut App, id: &str, roles: &[(&str, Entity)]| play_sequence(app.world_mut(), id, roles).unwrap_err();
    let both = [("actor", actor), ("listener", listener)];

    let e = play(&mut app, "scen", &both);
    assert!(e.contains("no sequence `scen` (did you mean `scene`?)") && e.contains("sequences: "), "{e}");
    let e = play(&mut app, "scene", &[("actor", actor), ("watcher", listener)]);
    assert!(e.contains("has no role `watcher`") && e.contains("its roles: actor, listener"), "{e}");
    let e = play(&mut app, "scene", &[("actor", actor)]);
    assert!(e.contains("role `listener` is not bound") && e.contains("role.actor=eN role.listener=eN"), "{e}");
    let e = play(&mut app, "scene", &[("actor", actor), ("actor", listener), ("listener", listener)]);
    assert!(e.contains("bound twice"), "{e}");
    let e = play(&mut app, "scene", &[("actor", actor), ("listener", actor)]);
    assert!(e.contains("is bound to both `actor` and `listener`"), "{e}");
    let e = play(&mut app, "scene", &[("actor", listener), ("listener", actor)]);
    assert!(e.contains("e1 (`actor`) is no NPC"), "{e}");
    let e = play(&mut app, "scene", &[("actor", dead), ("listener", listener)]);
    assert!(e.contains("(`actor`) is dead"), "{e}");
    let nobody = app.world_mut().spawn_empty().id();
    let e = play(&mut app, "scene", &[("actor", actor), ("listener", nobody)]);
    assert!(e.contains("has no `AiId`"), "{e}");
    // The world lacks a marker: the closest name and the list.
    let mark_a = app.world_mut().query::<(Entity, &LevelMarker)>().iter(app.world()).find(|(_, m)| m.name == "mark_a").map(|(e, _)| e).unwrap();
    app.world_mut().despawn(mark_a);
    let e = play(&mut app, "scene", &both);
    assert!(e.contains("no marker `mark_a` in the world") && e.contains("markers: mark_b, mark_far"), "{e}");
    marker(app.world_mut(), "mark_a", Vec3::new(6.0, 0.0, 0.0));
    // A condition nobody registered: the registered ones (the game's `scene_go` among them) are listed.
    let kinds = app.world().resource::<ContentKinds>().clone();
    let source = Source { path: "content/x.sequence.ron".into(), text: r#"{"waits": (roles: ["actor"], steps: [Face(role: "actor", at: Marker("mark_a")), WaitFor(condition: "scene_gone")])}"#.into() };
    let mut sources = vec![source];
    sources.extend(read_dir(std::path::Path::new(ASSETS)).unwrap());
    app.world_mut().insert_resource(ContentDb::build(&kinds, &sources));
    let e = play(&mut app, "waits", &[("actor", actor)]);
    assert!(e.contains("waits on condition `scene_gone`") && e.contains("did you mean `scene_go`?"), "{e}");
    // Nothing was started by any of it.
    assert!(app.world().get_resource::<Sequences>().is_none(), "no run, no resource");
    assert!(!app_has_hold(&app, actor));
    // A body one run holds is not another's.
    play_sequence(app.world_mut(), "stand", &[("actor", actor)]).unwrap();
    let e = play(&mut app, "stand", &[("actor", actor)]);
    assert!(e.contains("is already in sequence 1 (stand)") && e.contains("sequence_stop run=1"), "{e}");
}

#[test]
fn an_order_an_agent_gave_the_actor_is_dropped_when_the_scene_takes_it() {
    use shards_core::{Intent, Order, Target};
    let mut app = scene_app();
    let (actor, _) = stage(&mut app);
    // An agent sent it off the other way.
    {
        let mut intent = app.world_mut().get_mut::<Intent>(actor).unwrap();
        intent.orders.push_back(Order::MoveTo { target: Target::Point(Vec3::new(-20.0, 0.0, 0.0)), within: 0.5 });
    }
    step(&mut app, 10);
    assert!(feet(&app, actor).x < -0.5, "it walks west on the order: {}", feet(&app, actor));
    play_sequence(app.world_mut(), "stand", &[("actor", actor)]).unwrap();
    assert!(app.world().get::<Intent>(actor).unwrap().orders.is_empty(), "the scene has it now");
    let here = feet(&app, actor);
    step(&mut app, 30);
    assert!(flat_distance(feet(&app, actor), here) < 0.3, "it stands for the scene: {} -> {}", here, feet(&app, actor));
}

#[test]
fn stopping_a_run_frees_its_bodies_at_once_and_says_skipped() {
    let mut app = scene_app();
    let (actor, _) = stage(&mut app);
    play_sequence(app.world_mut(), "stand", &[("actor", actor)]).unwrap();
    step(&mut app, 10);
    assert!(app_has_hold(&app, actor));
    assert!(drive(&app, actor).face.is_some());
    let began = log(&app).steps.len();
    assert!(stop_sequence(app.world_mut(), SequenceId(1)));
    assert!(!app_has_hold(&app, actor), "free in the same breath");
    assert_eq!(drive(&app, actor), NpcDrive::default());
    assert!(!stop_sequence(app.world_mut(), SequenceId(1)), "it is not playing any more");
    let [(_, ended)] = &log(&app).ended[..] else { panic!("{:?}", log(&app).ended) };
    assert!(ended.skipped && ended.why.is_none(), "stopped by the game: {ended:?}");
    assert!(!lines(&app, "sequence 1 (stand) stopped at step ").is_empty(), "{:#?}", journal(&app));
    // The run is gone for good: nothing more happens on later ticks.
    step(&mut app, 200);
    assert_eq!(log(&app).ended.len(), 1);
    assert_eq!(log(&app).steps.len(), began);
    // And the tools: by number, by definition, all.
    let a = ok(&mut app, "sequence_play", serde_json::json!({"id": "stand", "role.actor": "e2"}));
    assert!(text(&a).contains("sequence 2 (stand) plays, 2 steps: actor=e2"), "{}", text(&a));
    assert!(fails(&mut app, "sequence_stop", serde_json::json!({"run": 9})).contains("no sequence 9 is playing; playing: 2 (stand)"));
    assert!(fails(&mut app, "sequence_stop", serde_json::json!({})).contains("name one thing to stop"));
    assert!(fails(&mut app, "sequence_stop", serde_json::json!({"id": "scene"})).contains("no run of `scene` is playing"));
    assert!(text(&ok(&mut app, "sequence_stop", serde_json::json!({"id": "stand"}))).contains("stopped sequence 2"));
    assert!(fails(&mut app, "sequence_stop", serde_json::json!({"all": true})).contains("no sequence is playing"));
}

#[test]
fn a_body_that_dies_or_goes_ends_the_run_with_a_warning_and_a_step_that_takes_too_long_does_too() {
    // A death.
    let mut app = scene_app();
    let (actor, _) = stage(&mut app);
    play_sequence(app.world_mut(), "stand", &[("actor", actor)]).unwrap();
    step(&mut app, 10);
    app.world_mut().entity_mut(actor).insert(Dead);
    step(&mut app, 1);
    let [(_, ended)] = &log(&app).ended[..] else { panic!("{:?}", log(&app).ended) };
    assert!(ended.skipped && ended.why.as_deref().is_some_and(|w| w.contains("e2 (actor) is dead")), "{ended:?}");
    let warned: Vec<String> = all_journal(&app).into_iter().filter(|l| l.contains("abandoned")).collect();
    assert!(warned.len() == 1 && warned[0].contains("sequence 1 (stand) abandoned at step ") && warned[0].contains("/2: e2 (actor) is dead"), "{warned:#?}");
    assert!(app.world().resource::<AiJournal>().after(0).0.any(|e| e.kind == JournalKind::Warn && e.text.contains("abandoned")));
    assert!(!app_has_hold(&app, actor));

    // A body that is gone.
    let mut app = scene_app();
    let (actor, _) = stage(&mut app);
    play_sequence(app.world_mut(), "stand", &[("actor", actor)]).unwrap();
    step(&mut app, 10);
    app.world_mut().despawn(actor);
    step(&mut app, 1);
    let [(_, ended)] = &log(&app).ended[..] else { panic!("{:?}", log(&app).ended) };
    assert!(ended.skipped && ended.why.as_deref().is_some_and(|w| w.contains("(actor) is gone")), "{ended:?}");

    // A walk that gets nowhere in its second.
    let mut app = scene_app();
    let (actor, _) = stage(&mut app);
    play_sequence(app.world_mut(), "far", &[("actor", actor)]).unwrap();
    step(&mut app, 80);
    let [(_, ended)] = &log(&app).ended[..] else { panic!("{:?}", log(&app).ended) };
    assert!(ended.skipped && ended.why.as_deref().is_some_and(|w| w.contains("walks to `mark_far`) did not finish in 1.0 s") || w.contains("did not finish in 1.0 s")), "{ended:?}");
    assert!(!app_has_hold(&app, actor));
    assert!(feet(&app, actor).x > 0.5 && feet(&app, actor).x < 49.0, "it set off: {}", feet(&app, actor));

    // An action that is cut short (the actor is staggered or told another) ends the Play step; one that cannot be played ends the run.
    let mut app = scene_app();
    let (actor, _) = stage(&mut app);
    play_sequence(app.world_mut(), "long", &[("actor", actor)]).unwrap();
    step(&mut app, 5);
    assert_eq!(action(&app, actor).as_deref(), Some("long_gesture"));
    assert!(shards_action::force_action(app.world_mut(), actor, "gesture"));
    step(&mut app, 2);
    assert_eq!(log(&app).signals.len(), 1, "the long action was replaced: the step is over");
}

#[test]
fn a_scene_with_no_bodies_runs_its_steps_through_in_a_tick() {
    let mut app = scene_app();
    let run = play_sequence(app.world_mut(), "tell", &[]).unwrap();
    step(&mut app, 1);
    assert_eq!(run, SequenceId(1));
    let names: Vec<&str> = log(&app).signals.iter().map(|(_, s)| s.name.as_str()).collect();
    assert_eq!(names, ["hello", "again"]);
    assert_eq!(log(&app).ended.len(), 1);
    assert!(app.world().resource::<Sequences>().runs.is_empty());
    assert_eq!(app.world().resource::<Sequences>().next, 1, "the number is not used twice");
    assert_eq!(call(&mut app, "sequences", serde_json::json!({})).result.as_str().map(|s| s.contains("0 playing")), Some(true));
}

/// What the story says after tick `n`.
fn story_after(app: &App, n: u64) -> Vec<String> {
    app.world()
        .resource::<AiJournal>()
        .after(0)
        .0
        .filter(|e| e.tick > n && matches!(e.kind, JournalKind::Event | JournalKind::Warn | JournalKind::Error))
        .map(ToString::to_string)
        .collect()
}

#[test]
fn a_scene_loaded_at_any_tick_ends_the_same() {
    let mut original = scene_app();
    let (actor, listener) = stage(&mut original);
    play_sequence(original.world_mut(), "scene", &[("actor", actor), ("listener", listener)]).unwrap();
    // A snapshot at the start of every step, and one part-way through the walk and the wait.
    let mut snapshots: Vec<(u64, u32, ai_core::Snapshot)> = Vec::new();
    let mut last_step = u32::MAX;
    for _ in 0..GO + 20 {
        let (step_now, age) = original.world().get_resource::<Sequences>().and_then(|s| s.runs.first()).map_or((u32::MAX, 0), |r| (r.step, r.age));
        if step_now != u32::MAX && (step_now != last_step || (step_now <= 5 && age == 12)) {
            last_step = step_now;
            snapshots.push((ai_core::sim::tick(original.world()), step_now, snapshot::capture(original.world()).unwrap()));
        }
        step(&mut original, 1);
    }
    assert!(snapshots.len() >= 9, "{} snapshots: {:?}", snapshots.len(), snapshots.iter().map(|s| (s.0, s.1)).collect::<Vec<_>>());
    assert!(original.world().resource::<Sequences>().runs.is_empty(), "the original finished");
    let end = snapshot::capture(original.world()).unwrap();
    assert!(end.left_out.is_empty(), "{:?}", end.left_out);

    for (at, step_at, saved) in snapshots {
        // A fresh arena with the markers (static, like the floor); the bodies and the run are the snapshot's.
        let mut copy = scene_app();
        markers(copy.world_mut());
        let report = snapshot::restore(copy.world_mut(), &saved, false).unwrap_or_else(|e| panic!("tick {at}: {e}"));
        assert!(report.exact(), "tick {at} (step {step_at}): {:?}", report.differences);
        let runs = &copy.world().resource::<Sequences>().runs;
        assert_eq!(runs.len(), 1, "tick {at}: the run came back");
        let me = *copy.world().resource::<ai_core::AiIndex>().get(ai_core::AiId(2)).as_ref().unwrap();
        assert!(app_has_hold(&copy, me), "tick {at}: and so did the hold");
        step(&mut copy, GO + 20 - at);
        let diff = differences(&end, &snapshot::capture(copy.world()).unwrap());
        assert!(diff.is_empty(), "loaded at tick {at} (step {step_at}):\n{}", diff.join("\n"));
        assert_eq!(story_after(&copy, at), story_after(&original, at), "journal after tick {at}");
        // A step already begun is not begun again: its sound is said once, by the original or by this copy, never twice.
        let said = if at < original_sound_tick(&original) { 1 } else { 0 };
        assert_eq!(log(&copy).sounds.len(), said, "tick {at} (step {step_at}): the line is said {said} time(s) after the load");
    }
}

/// The tick the original said its line at.
fn original_sound_tick(original: &App) -> u64 {
    log(original).sounds[0].0
}

#[test]
fn a_snapshot_without_a_run_takes_the_world_s_run_away() {
    let mut app = scene_app();
    let (actor, listener) = stage(&mut app);
    step(&mut app, 3);
    let before = snapshot::capture(app.world()).unwrap();
    assert!(!before.resources.keys().any(|k| k.contains("Sequences")), "a world that never played one saves none");
    play_sequence(app.world_mut(), "scene", &[("actor", actor), ("listener", listener)]).unwrap();
    step(&mut app, 30);
    assert!(app_has_hold(&app, actor));
    let report = snapshot::restore(app.world_mut(), &before, false).unwrap();
    assert!(report.exact(), "{:?}", report.differences);
    assert!(app.world().get_resource::<Sequences>().is_none(), "the run is not the snapshot's");
    let actor = *app.world().resource::<ai_core::AiIndex>().get(ai_core::AiId(2)).as_ref().unwrap();
    assert!(!app_has_hold(&app, actor));
    step(&mut app, 5);
}

/// The errors of a `sequence` file, built with the test's actions.
fn errors_of(text: &str) -> Vec<String> {
    let app = scene_app();
    let kinds = app.world().resource::<ContentKinds>().clone();
    let mut sources = vec![Source { path: "content/bad.sequence.ron".into(), text: text.into() }];
    sources.extend(read_dir(std::path::Path::new(ASSETS)).unwrap().into_iter().filter(|s| s.path.ends_with(".action.ron")));
    ContentDb::build(&kinds, &sources).errors().map(ToString::to_string).collect()
}

#[test]
fn a_sequence_file_is_checked_before_it_plays() {
    let errors = errors_of(
        r#"{
            "empty": (roles: [], steps: []),
            "names": (roles: ["a b", "x", "x"], steps: [Signal(name: "go")]),
            "undeclared": (roles: ["guard"], steps: [MoveTo(role: "gard", marker: "m"), Face(role: "guard", at: Role("visitor"))]),
            "unused": (roles: ["guard", "extra"], steps: [Wait(secs: 1.0), Face(role: "guard", at: Marker("m"))]),
            "numbers": (roles: ["g"], steps: [
                MoveTo(role: "g", marker: "", within: -1.0, speed: 2.0, timeout: 0.0),
                Wait(secs: -3.0),
                WaitFor(condition: "!!x"),
                WaitFor(condition: ""),
                Signal(name: " "),
                Sound(role: "g", sound: ""),
            ]),
            "action": (roles: ["g"], steps: [Play(role: "g", action: "gestur")]),
            "fine": (roles: ["g"], steps: [Play(role: "g", action: "gesture"), Wait(secs: 0.0)]),
        }"#,
    );
    let all = errors.join("\n");
    for needle in [
        "sequence `empty`: `steps` is empty",
        "roles[0] `a b`: a role is a word",
        "roles[2] `x` is declared twice",
        "steps[0] MoveTo: role `gard` is not declared in `roles` (did you mean `guard`?); declared: guard",
        "steps[1] Face: role `visitor` is not declared in `roles`",
        "role `extra` is declared but no step names it",
        "steps[0] MoveTo: `marker` is empty",
        "`within` must be a number of meters, 0 or more (is -1)",
        "`speed` is the movement input, above 0 and at most 1 (0.5 walks, 1 runs), not 2",
        "`timeout` must be a number of seconds above 0 (is 0)",
        "steps[1] Wait: `secs` must be a number of seconds, 0 or more (is -3)",
        "steps[2] WaitFor: `condition` names no condition",
        "steps[3] WaitFor: `condition` names no condition",
        "steps[4] Signal: `name` is empty",
        "steps[5] Sound: `sound` is empty",
        "unknown action `gestur` (did you mean `gesture`?)",
    ] {
        assert!(all.contains(needle), "no `{needle}` in:\n{all}");
    }
    assert!(!all.contains("sequence `fine`"), "the good one passes:\n{all}");
    let app = scene_app();
    assert!(app.world().resource::<ContentDb>().errors().next().is_none());
}

#[test]
fn the_observe_section_the_npc_fragment_and_npc_state_say_where_a_scene_is() {
    let mut app = scene_app();
    let (actor, listener) = stage(&mut app);
    play_sequence(app.world_mut(), "scene", &[("actor", actor), ("listener", listener)]).unwrap();
    step(&mut app, 10);
    let observed = ai_core::observe::observe(app.world(), &ai_core::ObserveOptions::default());
    assert!(observed.lines().any(|l| l == "SEQUENCE 1 scene step 1/7: e2 walks to `mark_a`"), "{observed}");
    assert!(observed.contains("in sequence 1 scene (actor), step 1/7"), "the actor's line says so:\n{observed}");
    let state = ok(&mut app, "npc_state", serde_json::json!({"id": "e2"}));
    let sequence = &state.result["sequence"];
    assert_eq!((sequence["run"].as_u64(), sequence["def"].as_str(), sequence["role"].as_str(), sequence["step"].as_u64(), sequence["of"].as_u64()), (Some(1), Some("scene"), Some("actor"), Some(1), Some(7)), "{state:?}");
    assert_eq!(sequence["doing"].as_str(), Some("e2 walks to `mark_a`"));
    let listed = text(&ok(&mut app, "sequences", serde_json::json!({})));
    assert!(listed.contains("5 sequences") && listed.contains("scene: roles actor listener, 7 steps") && listed.contains("1 playing\n  1 scene step 1/7"), "{listed}");
    let one = text(&ok(&mut app, "sequences", serde_json::json!({"run": 1})));
    assert!(one.contains("actor = e2 (held)") && one.contains("listener = e1\n") && one.contains(" > 1. e2 walks to `mark_a`") && one.contains("   2. e2 turns to the listener"), "{one}");
    assert!(fails(&mut app, "sequences", serde_json::json!({"run": 5})).contains("no sequence 5 is playing"));
    // `step --until named:sequence_playing`.
    let playing = app.world().resource::<ai_core::AiConditions>().0["sequence_playing"];
    assert!(playing(app.world()));
    ok(&mut app, "sequence_stop", serde_json::json!({"all": true}));
    assert!(!playing(app.world()));
}

#[test]
fn a_run_an_observer_stops_while_it_is_being_stepped_does_nothing_more() {
    let mut app = scene_app();
    // The game reacts to the first signal by stopping the run (a normal thing to do with a signal).
    app.world_mut().add_observer(|e: On<SequenceSignal>, mut commands: Commands| {
        let sequence = e.sequence;
        if e.name == "hello" {
            commands.queue(move |world: &mut World| {
                stop_sequence(world, sequence);
            });
        }
    });
    play_sequence(app.world_mut(), "tell", &[]).unwrap();
    step(&mut app, 5);
    let names: Vec<&str> = log(&app).signals.iter().map(|(_, s)| s.name.as_str()).collect();
    assert_eq!(names, ["hello"], "the second signal was never given: the run was over");
    let [(_, ended)] = &log(&app).ended[..] else { panic!("{:?}", log(&app).ended) };
    assert!(ended.skipped && ended.why.is_none(), "{ended:?}");
    assert_eq!(log(&app).steps.len(), 1, "and no step began after it");
    assert!(app.world().resource::<Sequences>().runs.is_empty());
}
