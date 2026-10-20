//! Encounters (G9) on the sandbox's yard: what the umbrella adds (cues, `NetAlways`, the join gate,
//! content checks), snapshots of a real fight, spawn points on a real navmesh, and a game's reset of
//! its NPCs' plans when a fight is lost (N8c).

use std::path::Path;

use shards::ai_core::snapshot::{self, differences};
use shards::ai_core::{replay, sim, AiId, AiIndex, AiJournal, JournalKind};
use shards::content::db::{read_dir, Source};
use shards::content::{ContentDb, ContentKinds};
use shards::core::NetRole;
use shards::encounter::{EncounterReset, EncounterRuns, EncounterWiped, RunState};
use shards::net::{default_join_policy, JoinDecision, JoinRequest, NetAlways};
use shards::npc::{reset_plan, BehaviorDef, Npc, NpcBrain, NpcPlan, NpcStateChanged};
use shards::prelude::*;
use shards::serde_json::{json, Value};

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

/// Issue #72 (the nightly fuzz campaign's heavy checks): wave 1 of the ring, a `net_always` fight, is
/// on, and its member carries `NetAlways` with the umbrella's marker that it put it there. A game
/// loaded from a snapshot of that must take `NetAlways` off the member when the run lets it go, as the
/// original does. The marker was not in snapshots: the loaded game left the corpse in every client's
/// view, and its snapshot had nothing to leave out (`left_out differs`). The between-waves snapshot
/// above does not see it: by then wave 1 is gone, and both games put the marker on wave 2 themselves.
#[test]
fn a_snapshot_inside_a_net_always_fight_lets_its_members_go_as_the_original_does() {
    let mut app = yard();
    into_the_ring(&mut app);
    let member = shards::encounter::run(app.world(), RING).unwrap().members[0].id;
    let entity = app.world().resource::<AiIndex>().get(member).unwrap();
    assert!(app.world().get::<NetAlways>(entity).is_some(), "in every client's view");
    assert_continues_alike("wave 1 of a net_always fight, until its member is let go", app, |app| {
        ok(app, "encounter_clear", json!({"key": RING}));
        step(app, 10);
        let Some(entity) = app.world().resource::<AiIndex>().get(member) else { return };
        assert!(app.world().get::<NetAlways>(entity).is_none(), "{member}, let go by the run, is still in every client's view");
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

// --- N8c: a game resets its NPCs' plans when a fight is lost --------------------------------------------

/// What a game does to reset its bosses' plans (N8c; the game's `reset_boss` also heals them and sets
/// their first phase). A member that is home again after its walk (it leaves a state of kind
/// `return_home`) forgets its plan. On a wipe or a reset, so does a member placed home at once
/// instead of walking. A walking member is left to the first observer, so each path resets once.
/// Every observer returns at once on a machine that does not simulate.
fn reset_plans_like_a_game(app: &mut App) {
    app.add_observer(
        |e: On<NpcStateChanged>, npcs: Query<&Npc>, db: Res<ContentDb>, role: Option<Res<NetRole>>, mut commands: Commands| {
            if !shards::core::simulates(role) {
                return;
            }
            let walked = npcs.get(e.entity).ok().and_then(|n| db.get::<BehaviorDef>(&n.behavior)).is_some_and(|b| b.kind_of(&e.from) == "return_home");
            if walked {
                let entity = e.entity;
                commands.queue(move |w: &mut World| {
                    reset_plan(w, entity, "home").unwrap();
                });
            }
        },
    );
    app.add_observer(|e: On<EncounterWiped>, role: Option<Res<NetRole>>, mut commands: Commands| {
        if shards::core::simulates(role) {
            let key = e.key.clone();
            commands.queue(move |w: &mut World| reset_members(w, &key, "encounter wiped"));
        }
    });
    app.add_observer(|e: On<EncounterReset>, role: Option<Res<NetRole>>, mut commands: Commands| {
        if shards::core::simulates(role) {
            let key = e.key.clone();
            commands.queue(move |w: &mut World| reset_members(w, &key, "encounter reset"));
        }
    });
}

/// Resets the plans of run `key`'s members that are not walking home.
fn reset_members(world: &mut World, key: &str, why: &str) {
    let Some(run) = shards::encounter::run(world, key) else { return };
    let ids: Vec<AiId> = run.members.iter().map(|m| m.id).collect();
    for id in ids {
        let Some(e) = world.resource::<AiIndex>().get(id) else { continue };
        let Some(npc) = world.get::<Npc>(e) else { continue };
        let state = world.get::<NpcBrain>(e).map(|b| b.state.clone()).unwrap_or_default();
        let db = world.resource::<ContentDb>();
        let walking = db.get::<BehaviorDef>(&npc.behavior).is_some_and(|b| b.kind_of(&state) == "return_home");
        if !walking && npc.plan.is_some() {
            reset_plan(world, e, why).unwrap();
        }
    }
}

const PACK: &str = "yard/pack";

/// The pack's members by id, with their entities.
fn pack(app: &App) -> Vec<(AiId, Entity)> {
    let index = app.world().resource::<AiIndex>();
    shards::encounter::run(app.world(), PACK).unwrap().members.iter().filter_map(|m| Some((m.id, index.get(m.id)?))).collect()
}

fn plan_of(app: &App, e: Entity) -> NpcPlan {
    app.world().get::<NpcPlan>(e).unwrap().clone()
}

/// The yard with the game's resets, the player beside the pack until a member has attacked, then fallen:
/// the members that remembered something just before the wipe.
fn wipe_the_pack(app: &mut App) -> Vec<AiId> {
    reset_plans_like_a_game(app);
    let members = pack(app);
    let at = app.world().get::<Transform>(members[0].1).unwrap().translation;
    ok(app, "teleport", json!({"id": PLAYER, "x": at.x, "z": at.z + 2.5, "y": at.y}));
    for _ in 0..60 {
        step(app, 30);
        if members.iter().any(|(_, e)| !plan_of(app, *e).history.is_empty()) {
            break;
        }
    }
    let remembered: Vec<AiId> = members.iter().filter(|(_, e)| !plan_of(app, *e).history.is_empty()).map(|(id, _)| *id).collect();
    assert!(!remembered.is_empty(), "nobody in the pack attacked");
    ok(app, "god", json!({"on": false}));
    ok(app, "set", json!({"id": PLAYER, "component": "Health", "path": "current", "value": 0}));
    step(app, 10);
    assert!(matches!(run_state(app, PACK), RunState::Wiped { pulled_back: true, .. }), "{:?}", run_state(app, PACK));
    remembered
}

/// The reset lines: who, and why.
fn resets(app: &App) -> Vec<(String, String)> {
    lines(app, " plan reset (")
        .iter()
        .map(|l| {
            let (who, rest) = l.split_once(' ').unwrap().1.split_once(" plan reset (").unwrap();
            (who.to_owned(), rest.split_once(')').unwrap().0.to_owned())
        })
        .collect()
}

/// Every member's plan is empty; every reset line says `why`; nobody is reset twice; and every member
/// that remembered something before the wipe is among them. (A member whose swing was under way at the
/// sample remembers it too, so the lines may name more than `remembered`.)
fn assert_reset_once(app: &App, remembered: &[AiId], why: &str) -> usize {
    for (id, e) in pack(app) {
        let plan = plan_of(app, e);
        assert!(plan.history.is_empty() && plan.cooldowns.is_empty(), "{id}: {plan:?}");
    }
    let got = resets(app);
    assert!(got.iter().all(|(_, w)| w == why), "every reset is `{why}`: {got:?}");
    let mut who: Vec<&String> = got.iter().map(|(w, _)| w).collect();
    who.sort();
    who.dedup();
    assert_eq!(who.len(), got.len(), "once each: {got:?}");
    for id in remembered {
        assert!(got.iter().any(|(w, _)| *w == id.to_string()), "{id} remembered and was not reset: {got:?}");
    }
    got.len()
}

#[test]
fn a_wiped_pack_walks_home_and_forgets_its_plans_on_arrival_once_each() {
    let mut app = yard();
    let remembered = wipe_the_pack(&mut app);
    step(&mut app, 600);
    for (id, e) in pack(&app) {
        assert_eq!(app.world().get::<NpcBrain>(e).unwrap().state, "idle", "{id} is home");
    }
    assert_reset_once(&app, &remembered, "home");
}

#[test]
fn a_wiped_pack_placed_home_at_once_forgets_its_plans_on_the_wipe() {
    let mut app = yard();
    // The same pack without a state to walk home in: the wipe places its survivors home.
    let before = ok(&mut app, "content_get", json!({"kind": "encounter", "id": "yard_pack"})).result["def"].clone();
    let mut def = before.clone();
    def["pull_back"]["state"] = Value::Null;
    def["pull_back"]["deadline"] = json!(0.0);
    ok(&mut app, "content_apply", json!({"changes": [{"kind": "encounter", "id": "yard_pack", "before": before, "def": def}]}));
    let remembered = wipe_the_pack(&mut app);
    let n = assert_reset_once(&app, &remembered, "encounter wiped");
    // Home already, they leave nothing to forget when their own walk home ends.
    step(&mut app, 600);
    assert_eq!(resets(&app).len(), n, "{:?}", resets(&app));
}
