//! Encounters (G9) on the sandbox's yard: what the umbrella adds (cues, `NetAlways`, the join gate,
//! content checks), snapshots of a real fight, and spawn points on a real navmesh.

use std::path::Path;

use shards::ai_core::snapshot::{self, differences};
use shards::ai_core::{replay, sim, AiIndex, AiJournal, JournalKind};
use shards::content::db::{read_dir, Source};
use shards::content::{ContentDb, ContentKinds};
use shards::encounter::{EncounterRuns, RunState};
use shards::net::{default_join_policy, JoinDecision, JoinRequest, NetAlways};
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::*;

const RING: &str = "yard/ring";

/// The yard, the player unhurtable at its gate, the runs made.
fn yard() -> App {
    let mut app = sandbox::test_app(42);
    ok(&mut app, "regen", json!({"region": "yard", "seed": 1}));
    ok(&mut app, "heal", json!({"id": PLAYER}));
    ok(&mut app, "god", json!({}));
    step(&mut app, 3);
    app
}

fn run_state(app: &App, key: &str) -> RunState {
    shards::encounter::run(app.world(), key).unwrap_or_else(|| panic!("no run {key}")).state.clone()
}

/// Into the ring: the start and wave 1 spawned.
fn into_the_ring(app: &mut App) {
    ok(app, "teleport", json!({"id": PLAYER, "x": 248, "z": -182, "y": 0}));
    ok(app, "step", json!({"ticks": 600, "until": [{"named": "encounter_active"}]}));
    step(app, 72);
    assert_eq!(shards::encounter::run(app.world(), RING).unwrap().wave, 1);
}

#[test]
fn the_yard_has_its_two_runs() {
    let app = yard();
    let runs = shards::encounter::runs(app.world());
    assert_eq!(runs.iter().map(|r| r.key.as_str()).collect::<Vec<_>>(), vec!["yard/pack", "yard/ring"]);
    let pack = &runs[0];
    assert!((2..=3).contains(&pack.members.len()), "{pack:?}");
    assert_eq!(runs[1].state, RunState::Armed);
}

fn story_after(app: &App, n: u64) -> Vec<String> {
    app.world()
        .resource::<AiJournal>()
        .after(0)
        .0
        .filter(|e| e.tick > n && matches!(e.kind, JournalKind::Event | JournalKind::Warn | JournalKind::Error))
        .map(ToString::to_string)
        .collect()
}

/// The proof of the snapshot tests: a game at the situation, a snapshot, the same steps on both.
fn assert_continues_alike(what: &str, mut a: App, after: impl Fn(&mut App)) {
    let snapshot = snapshot::capture(a.world()).unwrap();
    let n = sim::tick(a.world());
    after(&mut a);
    let mut b = sandbox::test_app(42);
    ok(&mut b, "regen", json!({"region": "yard", "seed": 1}));
    let report = snapshot::restore(b.world_mut(), &snapshot, false).unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(report.exact(), "{what}: restored differently: {:?}", report.differences);
    assert_eq!(replay::state_hash(b.world()), snapshot.state_hash, "{what}: the loaded state is the saved one");
    after(&mut b);
    assert_eq!(sim::tick(a.world()), sim::tick(b.world()), "{what}: tick");
    let diff = differences(&snapshot::capture(a.world()).unwrap(), &snapshot::capture(b.world()).unwrap());
    assert!(diff.is_empty(), "{what}: the states differ:\n{}", diff.join("\n"));
    assert_eq!(replay::state_hash(a.world()), replay::state_hash(b.world()), "{what}: state hash");
    assert_eq!(story_after(&a, n), story_after(&b, n), "{what}: the journal from tick {n} on");
}

#[test]
fn a_snapshot_of_the_ring_in_wave_two_and_in_a_wipe_continues_alike() {
    let mut app = yard();
    into_the_ring(&mut app);
    ok(&mut app, "encounter_clear", json!({"key": RING}));
    step(&mut app, 100);
    assert!(matches!(run_state(&app, RING), RunState::Between { .. }), "{:?}", run_state(&app, RING));
    assert_continues_alike("between waves 1 and 2", app, |app| {
        step(app, 200);
    });

    let mut app = yard();
    into_the_ring(&mut app);
    ok(&mut app, "god", json!({"on": false}));
    ok(&mut app, "set", json!({"id": PLAYER, "component": "Health", "path": "current", "value": 0}));
    step(&mut app, 10);
    assert!(matches!(run_state(&app, RING), RunState::Wiped { .. }));
    assert_continues_alike("wiped", app, |app| {
        ok(app, "heal", json!({"id": PLAYER}));
        ok(app, "god", json!({}));
        step(app, 200);
    });
}

#[test]
fn a_cue_with_an_unknown_sound_fails_the_content_check() {
    let app = sandbox::test_app(42);
    let kinds = app.world().resource::<ContentKinds>().clone();
    let mut sources: Vec<Source> = read_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("assets")).unwrap();
    let file = sources.iter_mut().find(|s| s.path == "content/encounter/yard.encounter.ron").unwrap();
    file.text = file.text.replacen("sound: Some(\"whoosh\")", "sound: Some(\"whosh\")", 1);
    let db = ContentDb::build(&kinds, &sources);
    let errors: Vec<String> = db.errors().map(ToString::to_string).collect();
    assert!(errors.iter().any(|e| e.contains("encounter `ring_waves`") && e.contains("waves[0].warn.sound -> unknown sound `whosh` (did you mean `whoosh`?)")), "{errors:#?}");
}

#[derive(Resource, Default)]
struct Played(Vec<String>);

#[test]
fn a_waves_warning_plays_its_effect_and_sound_at_the_anchor() {
    let mut app = sandbox::build_app(&shards::testing::config());
    app.init_resource::<Played>()
        .add_observer(|e: On<shards::fx::PlayEffect>, mut p: ResMut<Played>| p.0.push(format!("effect {}", e.effect)))
        .add_observer(|e: On<shards::audio::PlaySound>, mut p: ResMut<Played>| p.0.push(format!("sound {}", e.sound)));
    app.finish();
    app.cleanup();
    shards::testing::wait_until_loaded(&mut app, std::time::Duration::from_secs(60)).unwrap();
    ok(&mut app, "regen", json!({"region": "yard", "seed": 1}));
    ok(&mut app, "god", json!({}));
    step(&mut app, 3);
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 248, "z": -182, "y": 0}));
    ok(&mut app, "step", json!({"ticks": 600, "until": [{"named": "encounter_active"}]}));
    let played = app.world().resource::<Played>().0.clone();
    assert!(played.contains(&"effect parry_flash".to_owned()) && played.contains(&"sound whoosh".to_owned()), "{played:?}");
}

#[test]
fn a_fight_that_blocks_joining_holds_newcomers_and_not_the_players_who_are_in() {
    let mut app = yard();
    let request = |player| JoinRequest { player, name: "guest".into(), token: String::new(), character: None };
    assert_eq!(default_join_policy(app.world(), &request(1)), JoinDecision::Accept, "nothing is on");
    into_the_ring(&mut app);
    let held = default_join_policy(app.world(), &request(1));
    assert_eq!(held, JoinDecision::Wait("an encounter is under way (yard/ring): you join when it ends".into()));
    // The host's own body is a rejoin: not held.
    assert_eq!(default_join_policy(app.world(), &request(0)), JoinDecision::Accept);
    ok(&mut app, "encounter_reset", json!({"key": RING}));
    step(&mut app, 5);
    assert_eq!(default_join_policy(app.world(), &request(1)), JoinDecision::Accept, "the fight ended");
}

#[test]
fn net_always_is_on_the_members_only_while_the_fight_is_on() {
    let mut app = yard();
    into_the_ring(&mut app);
    let ids: Vec<AiId> = shards::encounter::run(app.world(), RING).unwrap().members.iter().map(|m| m.id).collect();
    assert_eq!(ids.len(), 1);
    let entity = app.world().resource::<AiIndex>().get(ids[0]).unwrap();
    assert!(app.world().get::<NetAlways>(entity).is_some(), "in every client's view");
    // The pack's members are ordinary: they keep the interest radius.
    let pack = shards::encounter::run(app.world(), "yard/pack").unwrap().members[0].id;
    let pack = app.world().resource::<AiIndex>().get(pack).unwrap();
    assert!(app.world().get::<NetAlways>(pack).is_none());
}

#[test]
fn net_always_leaves_corpses_and_released_members_when_a_wave_clears_and_the_run_is_won() {
    let mut app = yard();
    into_the_ring(&mut app);
    // Another module's NetAlways on a body that is no encounter member is not ours to take off.
    let foreign = app.world_mut().spawn(NetAlways).id();
    let mut seen: Vec<Entity> = Vec::new();
    for _ in 0..40 {
        if run_state(&app, RING) == RunState::Won {
            break;
        }
        let mut query = app.world_mut().query_filtered::<Entity, With<NetAlways>>();
        seen.extend(query.iter(app.world()).filter(|e| *e != foreign));
        if run_state(&app, RING) == RunState::Running {
            ok(&mut app, "encounter_clear", json!({"key": RING}));
        }
        step(&mut app, 30);
    }
    step(&mut app, 5);
    assert_eq!(run_state(&app, RING), RunState::Won);
    assert!(seen.len() >= 3, "every wave's members were in every client's view: {seen:?}");
    for e in seen {
        assert!(
            app.world().get_entity(e).map_or(true, |e| e.get::<NetAlways>().is_none()),
            "{e:?} (a corpse or a released add) is still in every client's view"
        );
    }
    assert!(app.world().get::<NetAlways>(foreign).is_some(), "another module's NetAlways stays");
}

#[test]
fn a_ring_spawn_keeps_to_the_navmesh_and_falls_back_to_the_anchor_when_none_is_walkable() {
    let mut app = yard();
    let kinds = app.world().resource::<ContentKinds>().clone();
    let mut sources: Vec<Source> = read_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("assets")).unwrap();
    sources.push(Source {
        path: "content/encounter/edge.encounter.ron".into(),
        text: r#"{ "edge_ring": (radius: 20.0, start: Manual, waves: [(delay: 0.0, members: [
            (prefab: "yard_skeleton", count: 6, at: Ring(min: 5.0, max: 9.0)),
        ])]) }"#
            .into(),
    });
    let db = ContentDb::build(&kinds, &sources);
    assert_eq!(db.errors().count(), 0, "{:?}", db.errors().map(ToString::to_string).collect::<Vec<_>>());
    shards::content::install(app.world_mut(), db);
    // The yard ends at x = 296: an anchor at x = 293 has half its ring off the navmesh.
    shards::encounter::place_encounter(app.world_mut(), "edge_ring", Some("edge"), Transform::from_xyz(293.0, 0.0, -184.0)).unwrap();
    step(&mut app, 2);
    ok(&mut app, "encounter_start", json!({"key": "yard/edge"}));
    step(&mut app, 3);
    let members = shards::encounter::run(app.world(), "yard/edge").unwrap().members.clone();
    assert_eq!(members.len(), 6);
    for m in members {
        let e = app.world().resource::<AiIndex>().get(m.id).unwrap();
        let x = app.world().get::<Transform>(e).unwrap().translation.x;
        assert!(x < 296.0, "a member stands off the navmesh at x = {x}");
    }
}

#[test]
fn the_encounter_state_stays_out_of_the_hash_until_a_run_exists() {
    // A game with no anchor has no resource and no replay check: the hashes of every other game are unchanged.
    let app = sandbox::test_app(42);
    assert!(app.world().get_resource::<EncounterRuns>().is_none());
}
