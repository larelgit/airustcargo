//! Snapshots (E11.4) on the sandbox: motion, actions, statuses, cooldowns, input buffers
//! and the telegraph state all have to come back, so a session loaded from a snapshot
//! continues exactly like the one it was taken in. The proof is always the same: take a
//! snapshot at tick N and go on to tick M (state A); load it into a fresh game, give it
//! the same inputs, go on to M (state B); A and B are the same state, byte for byte, and
//! the journal from N to M reads the same.

use std::path::PathBuf;
use std::sync::MutexGuard;

use shards::action::InputBuffer;
use shards::ai_core::snapshot::{self, differences, Snapshot};
use shards::ai_core::{replay, sim, AgentSessions, AiJournal, JournalKind, Scope};
use shards::motion::{MotionMode, MotionState, Volume, VolumeKind};
use shards::prelude::*;
use shards::serde_json::{json, Value};

use crate::common::*;

fn tick(app: &App) -> u64 {
    sim::tick(app.world())
}

/// What happened after tick `n`: gameplay lines, warnings and errors (not the agents' own
/// echoes, and not the sim control lines a load adds).
fn story_after(app: &App, n: u64) -> Vec<String> {
    let journal = app.world().resource::<AiJournal>();
    journal
        .after(0)
        .0
        .filter(|e| e.tick > n && matches!(e.kind, JournalKind::Event | JournalKind::Warn | JournalKind::Error))
        .map(ToString::to_string)
        .collect()
}

fn assert_same_state(what: &str, a: &App, b: &App) {
    assert_eq!(tick(a), tick(b), "{what}: tick");
    let (sa, sb) = (snapshot::capture(a.world()).unwrap(), snapshot::capture(b.world()).unwrap());
    let diff = differences(&sa, &sb);
    assert!(diff.is_empty(), "{what}: the states differ:\n{}", diff.join("\n"));
    assert_eq!(replay::state_hash(a.world()), replay::state_hash(b.world()), "{what}: state hash");
}

/// The proof. `a` is a game in the situation to prove; `after` is what both games do next
/// (the same inputs, then time passing).
fn assert_continues_alike(what: &str, mut a: App, after: impl Fn(&mut App)) {
    let snapshot = snapshot::capture(a.world()).unwrap();
    let n = tick(&a);
    after(&mut a);

    let mut b = sandbox::test_app(42);
    let report = snapshot::restore(b.world_mut(), &snapshot, false).unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(report.exact(), "{what}: restored differently: {:?}", report.differences);
    assert_eq!(tick(&b), n);
    assert_eq!(replay::state_hash(b.world()), snapshot.state_hash, "{what}: the loaded state is the saved one");
    after(&mut b);

    assert_same_state(what, &a, &b);
    assert_eq!(story_after(&a, n), story_after(&b, n), "{what}: the journal from tick {n} on");
    assert!(tick(&a) > n, "{what}: the game went on");
}

/// The player 3 m south of the warden, facing it; the warden calm, the skeletons too.
fn face_to_face() -> App {
    let mut app = sandbox::test_app(42);
    calm_skeletons(&mut app);
    calm_warden(&mut app);
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 0, "z": -3}));
    ok(&mut app, "lock_on", json!({"target": BOSS}));
    step(&mut app, 2);
    app
}

/// Presses `tool` on the warden's body, as an agent possessing it would.
fn as_warden(app: &mut App, tool: &str) {
    app.world_mut().resource_mut::<AgentSessions>().configure("warden", Scope::Dev, Some(AiId(6)));
    let out = shards::ai_core::call_tool(app.world_mut(), "warden", tool, json!({}));
    assert!(out.ok, "{tool} as the warden: {}", out.result);
}

/// The player at `(x, y, 0)` standing, the skeletons and the warden calm.
fn on_the_course(x: f32, y: f32) -> App {
    let mut app = sandbox::test_app(42);
    calm_skeletons(&mut app);
    calm_warden(&mut app);
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": x, "z": 0, "y": y}));
    step(&mut app, 10);
    app
}

fn mode(app: &App) -> MotionMode {
    get::<MotionState>(app, PLAYER).mode
}

// --- the same game, from a snapshot ------------------------------------------------------

#[test]
fn mid_dodge() {
    let mut app = face_to_face();
    ok(&mut app, "dodge", json!({"dir": "S"}));
    step(&mut app, 4);
    assert_eq!(action(&app, PLAYER), "roll", "the roll is running");
    assert_continues_alike("mid-dodge", app, |app| {
        step(app, 90);
    });
}

/// The action timeline (B2a): a heavy held from the press, before it turns (its `RunExtras` hold the
/// hold), and a lunge whose whiff lengthened it.
#[test]
fn mid_charge_and_after_a_whiff() {
    let mut app = face_to_face();
    ok(&mut app, "set", json!({"id": PLAYER, "component": "Fighter", "path": "moveset", "value": "timeline"}));
    let out = ok(&mut app, "heavy_attack", json!({"hold": true}));
    assert_eq!(text(&out), "ok: spar_heavy (holding)");
    step(&mut app, 8);
    assert_eq!(action(&app, PLAYER), "spar_heavy", "held, not yet turned");
    assert!(get::<shards::action::RunExtras>(&app, PLAYER).held, "the hold is in the extras");
    assert_continues_alike("mid-charge", app, |app| {
        step(app, 10);
        assert_eq!(action(app, PLAYER), "spar_heavy_full", "turned after the load as before it");
        ok(app, "heavy_attack", json!({"hold": false}));
        step(app, 80);
    });

    let mut app = face_to_face();
    ok(&mut app, "set", json!({"id": PLAYER, "component": "Fighter", "path": "moveset", "value": "timeline"}));
    ok(&mut app, "lock_on", json!({"off": true}));
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 10, "z": 10}));
    step(&mut app, 2);
    ok(&mut app, "use_technique", json!({"slot": 1}));
    step(&mut app, 30);
    assert!(!lines(&app, "spar_lunge missed: +0.60s recovery").is_empty(), "{:?}", events(&app));
    assert_continues_alike("after a whiff", app, |app| {
        step(app, 60);
    });
}

#[test]
fn mid_combo() {
    let mut app = face_to_face();
    ok(&mut app, "attack", json!({"target": BOSS, "repeat": true}));
    ok(&mut app, "step", json!({"ticks": 200, "until": [{"journal": "e5 -> e6 light_2"}]}));
    step(&mut app, 1);
    assert!(action(&app, PLAYER).starts_with("light"), "{}", action(&app, PLAYER));
    assert_continues_alike("mid-combo", app, |app| {
        step(app, 150);
    });
}

/// The warden's slash is winding up, the player stands in its zone.
fn winding_up() -> App {
    let mut app = face_to_face();
    as_warden(&mut app, "light_attack");
    // The warning shows 0.1 s in; the slash lands at 0.6 s.
    step(&mut app, 12);
    assert!(!lines(&app, "e6 winds up").is_empty(), "the warning is out: {:?}", events(&app));
    assert_ne!(action(&app, BOSS), "idle");
    app
}

#[test]
fn during_a_telegraph() {
    // The player does nothing about it in one game, blocks in the other.
    assert_continues_alike("telegraph, hit taken", winding_up(), |app| {
        step(app, 60);
    });
    assert_continues_alike("telegraph, blocked", winding_up(), |app| {
        ok(app, "block", json!({}));
        step(app, 60);
    });
}

#[test]
fn while_gliding() {
    let mut app = on_the_course(84.5, 8.0);
    ok(&mut app, "glide", json!({}));
    ok(&mut app, "move_to", json!({"target": "e13"}));
    ok(&mut app, "jump", json!({}));
    ok(&mut app, "step", json!({"ticks": 200, "until": [{"journal": "e5 air → glide"}]}));
    ok(&mut app, "step", json!({"ticks": 300, "until": [{"shows": {"id": PLAYER, "text": "glide ↑"}}]}));
    assert_eq!(mode(&app), MotionMode::Glide);
    let motion = get::<MotionState>(&app, PLAYER);
    assert!(motion.updraft.is_some(), "in the updraft: {motion:?}");
    // The updraft is a volume without an id: the character's reference to it must survive.
    let world = app.world_mut();
    let keyless = motion.volumes.iter().filter(|v| world.get::<AiId>(**v).is_none()).count();
    assert!(keyless >= 1, "a volume without an AiId is touched: {:?}", motion.volumes);

    assert_continues_alike("gliding", app, |app| {
        ok(app, "step", json!({"ticks": 900, "until": [{"journal": "glide → ground"}]}));
        step(app, 30);
    });
}

#[test]
fn while_swimming() {
    let mut app = on_the_course(60.0, 3.3);
    ok(&mut app, "step", json!({"ticks": 200, "until": [{"journal": "e5 air → water"}]}));
    step(&mut app, 20);
    assert_eq!(mode(&app), MotionMode::Water);
    assert_continues_alike("swimming", app, |app| {
        ok(app, "move_to", json!({"target": {"x": 65.5, "z": 0}, "within": 0.1}));
        step(app, 240);
    });
}

#[test]
fn input_buffer_cooldowns_and_stamina_come_back() {
    let mut app = face_to_face();
    // A press that waits in the buffer, a spent flask, stamina in recovery.
    ok(&mut app, "heavy_attack", json!({}));
    step(&mut app, 2);
    let queued = act(&mut app, "dodge", json!({"dir": "S"}));
    step(&mut app, 3);
    let stamina = get::<Stamina>(&app, PLAYER).current;
    assert!(stamina < 100.0, "{stamina} ({})", text(&queued));
    assert!(!get::<InputBuffer>(&app, PLAYER).queued.is_empty() || action(&app, PLAYER) != "idle");
    assert_continues_alike("buffer and stamina", app, |app| {
        act(app, "dodge", json!({"dir": "W"}));
        step(app, 120);
    });
}

#[test]
fn a_flask_in_the_making_with_charges_cooldowns_and_statuses() {
    let mut app = face_to_face();
    ok(&mut app, "set", json!({"id": PLAYER, "component": "Health", "path": "current", "value": 40}));
    assert_eq!(text(&ok(&mut app, "use_item", json!({"item": "flask"}))), "ok: drink");
    step(&mut app, 25);
    assert_eq!(action(&app, PLAYER), "drink", "the heal at 0.6 s has not happened yet");
    let statuses = json!([{"id": "bleed", "buildup": 40.0, "threshold": 100.0, "active": 0.0, "stacks": 0}]);
    ok(&mut app, "set", json!({"id": PLAYER, "component": "Statuses", "value": statuses}));
    ok(&mut app, "set", json!({"id": PLAYER, "component": "Cooldowns", "value": [{"action": "roll", "left": 3.0}]}));
    let charges = get::<Charges>(&app, PLAYER);
    assert_eq!(charges.0[0].current, 2, "{charges:?}");
    assert_continues_alike("flask, statuses, cooldowns", app, |app| {
        step(app, 150);
    });
}

/// The inputs of a fight: (tick, tool, arguments).
fn fight_inputs() -> Vec<(u64, &'static str, Value)> {
    vec![
        (0, "attack", json!({"target": BOSS, "repeat": true})),
        (240, "dodge", json!({"away_from": BOSS})),
        (330, "block", json!({})),
    ]
}

/// Plays `inputs` from the current tick to `until` (inputs at `until` are for the next call).
fn drive(app: &mut App, inputs: &[(u64, &str, Value)], until: u64) {
    loop {
        let now = tick(app);
        if now >= until {
            break;
        }
        for (_, tool, args) in inputs.iter().filter(|(at, ..)| *at == now) {
            act(app, tool, args.clone());
        }
        let next = inputs.iter().map(|(at, ..)| *at).filter(|at| *at > now).min().unwrap_or(until);
        step(app, next.min(until) - now);
    }
}

/// A fight loaded from snapshots taken at `offsets` ticks into it ends like the original.
/// `make` is the game at the start of the fight.
fn fight_loaded_at_every_checkpoint(make: fn() -> App, offsets: &[u64], needle: &str) {
    let inputs = fight_inputs();
    let mut original = make();
    let start = tick(&original);
    let shifted: Vec<(u64, &str, Value)> = inputs.iter().map(|(at, t, a)| (at + start, *t, a.clone())).collect();
    let end = start + 600;
    let mut snapshots = Vec::new();
    for &offset in offsets {
        drive(&mut original, &shifted, start + offset);
        snapshots.push((start + offset, snapshot::capture(original.world()).unwrap()));
    }
    drive(&mut original, &shifted, end);
    assert!(!lines(&original, needle).is_empty(), "the fight happened: {:?}", events(&original));

    for (at, snapshot) in snapshots {
        let mut copy = sandbox::test_app(42);
        let report = snapshot::restore(copy.world_mut(), &snapshot, false).unwrap();
        assert!(report.exact(), "tick {at}: {:?}", report.differences);
        drive(&mut copy, &shifted, end);
        assert_same_state(&format!("loaded at tick {at}"), &original, &copy);
        let after: Vec<String> = story_after(&original, at);
        assert_eq!(story_after(&copy, at), after, "journal after tick {at}");
    }
}

#[test]
fn a_fight_loaded_at_any_tick_ends_the_same() {
    fight_loaded_at_every_checkpoint(face_to_face, &[1, 45, 100, 190, 239, 240, 241, 335, 480], "e5 -> e6");
}

/// Nobody is calmed: the skeletons chase, the warden picks its attacks with `SimRng`; the
/// player is in god mode and lives through it.
fn lively() -> App {
    let mut app = sandbox::test_app(42);
    ok(&mut app, "god", json!({}));
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 0, "z": 4}));
    step(&mut app, 2);
    app
}

#[test]
fn brains_and_their_random_choices_go_on_the_same_after_a_load() {
    fight_loaded_at_every_checkpoint(lively, &[30, 90, 150, 210, 300, 400, 500], "winds up");
    let mut app = lively();
    let inputs = fight_inputs();
    drive(&mut app, &inputs, 600);
    let attacks: std::collections::BTreeSet<String> = lines(&app, "e6 winds up")
        .iter()
        .filter_map(|l| l.split("winds up ").nth(1)?.split(' ').next().map(str::to_owned))
        .collect();
    assert!(attacks.len() >= 2, "the warden chose between attacks (SimRng): {attacks:?}");
}

#[test]
fn loading_into_the_same_game_rewinds_it() {
    let inputs = fight_inputs();
    let mut app = face_to_face();
    let start = tick(&app);
    let shifted: Vec<(u64, &str, Value)> = inputs.iter().map(|(at, t, a)| (at + start, *t, a.clone())).collect();
    drive(&mut app, &shifted, start + 150);
    let snapshot = snapshot::capture(app.world()).unwrap();
    drive(&mut app, &shifted, start + 420);
    let end_state = snapshot::capture(app.world()).unwrap();
    assert!(!lines(&app, "e5 -> e6").is_empty());

    // Back to tick 150 in the very same game, and the same again.
    let report = snapshot::restore(app.world_mut(), &snapshot, false).unwrap();
    assert!(report.exact(), "{:?}", report.differences);
    assert_eq!(tick(&app), start + 150);
    drive(&mut app, &shifted, start + 420);
    let diff = differences(&end_state, &snapshot::capture(app.world()).unwrap());
    assert!(diff.is_empty(), "{}", diff.join("\n"));
}

// --- what a load keeps and what it brings back ---------------------------------------------

#[test]
fn a_load_brings_back_the_ids_the_counters_and_the_references() {
    let mut app = sandbox::test_app(42);
    step(&mut app, 30);
    // A body that is gone and a spawn that is new, so the counters moved.
    ok(&mut app, "despawn", json!({"id": "e8"}));
    ok(&mut app, "spawn_prefab", json!({"id": "skeleton", "x": 3, "z": 3}));
    let snapshot = snapshot::capture(app.world()).unwrap();
    let ids_then = app.world().resource::<shards::ai_core::AiIndex>().ids();
    let next_then = app.world().resource::<shards::ai_core::AiIndex>().next_id();
    ok(&mut app, "spawn_prefab", json!({"id": "skeleton", "x": -3, "z": 3}));
    ok(&mut app, "despawn", json!({"id": "e7"}));

    let report = snapshot::restore(app.world_mut(), &snapshot, false).unwrap();
    assert!(report.exact(), "{:?}", report.differences);
    let index = app.world().resource::<shards::ai_core::AiIndex>();
    assert_eq!(index.ids(), ids_then);
    assert_eq!(index.next_id(), next_then);
    assert!(index.resolve(AiId(8)).unwrap_err().contains("no longer exists"), "e8 is still gone");
    // The next collider and the next entity get the numbers they would have had.
    assert_eq!(snapshot::capture(app.world()).unwrap().resources, snapshot.resources);
    let next = ok(&mut app, "spawn_prefab", json!({"id": "skeleton", "x": -3, "z": 3}));
    assert!(text(&next).contains(&format!("e{next_then}")), "{}", text(&next));
}

#[test]
fn what_a_window_adds_to_the_entities_does_not_get_in() {
    let mut app = sandbox::test_app(42);
    step(&mut app, 5);
    let before = snapshot::capture(app.world()).unwrap();
    let ids: Vec<Entity> = {
        let index = app.world().resource::<shards::ai_core::AiIndex>();
        index.ids().into_iter().filter_map(|id| index.get(id)).collect()
    };
    for entity in ids {
        app.world_mut().entity_mut(entity).insert((Visibility::default(), GlobalTransform::from_xyz(1.0, 2.0, 3.0)));
        app.world_mut().spawn((Transform::default(), ChildOf(entity)));
    }
    let with_window = snapshot::capture(app.world()).unwrap();
    assert_eq!(differences(&before, &with_window), Vec::<String>::new());
    let engine_types: Vec<&String> = with_window.entities[0]
        .components
        .keys()
        .filter(|k| k.starts_with("bevy_") && !k.contains("Transform") && !k.contains("Name"))
        .collect();
    assert!(engine_types.is_empty(), "{engine_types:?}");
}

#[test]
fn a_snapshot_is_the_same_bytes_every_time() {
    let (mut a, mut b) = (sandbox::test_app(42), sandbox::test_app(42));
    for app in [&mut a, &mut b] {
        ok(app, "attack", json!({"target": BOSS, "repeat": true}));
        step(app, 120);
    }
    let text = |app: &App| shards::serde_json::to_string_pretty(&snapshot::capture(app.world()).unwrap()).unwrap();
    assert_eq!(text(&a), text(&b));
    // And through the file text and back.
    let parsed: Snapshot = shards::serde_json::from_str(&text(&a)).unwrap();
    assert_eq!(parsed, snapshot::capture(a.world()).unwrap());
}

#[test]
fn save_load_save_gives_the_same_observation() {
    let mut app = face_to_face();
    ok(&mut app, "attack", json!({"target": BOSS, "repeat": true}));
    step(&mut app, 90);
    let seen = |app: &mut App| text(&ok(app, "observe", json!({"since_seq": u64::MAX >> 1}))).to_owned();
    let before = seen(&mut app);
    let snapshot = snapshot::capture(app.world()).unwrap();
    step(&mut app, 100);
    assert_ne!(seen(&mut app), before);
    snapshot::restore(app.world_mut(), &snapshot, false).unwrap();
    assert_eq!(seen(&mut app), before);
    let mut fresh = sandbox::test_app(42);
    snapshot::restore(fresh.world_mut(), &snapshot, false).unwrap();
    assert_eq!(seen(&mut fresh), before);
}

#[test]
fn volumes_a_character_touches_are_found_again() {
    let mut app = on_the_course(84.5, 8.0);
    ok(&mut app, "glide", json!({}));
    ok(&mut app, "move_to", json!({"target": "e13"}));
    ok(&mut app, "jump", json!({}));
    ok(&mut app, "step", json!({"ticks": 300, "until": [{"shows": {"id": PLAYER, "text": "glide ↑"}}]}));
    let snapshot = snapshot::capture(app.world()).unwrap();
    let motion = &snapshot.entities.iter().find(|e| e.id == 5).unwrap().components["shards_motion::controller::MotionState"];
    let volumes = motion["volumes"].as_array().unwrap();
    assert!(volumes.iter().any(|v| v.as_str().unwrap().starts_with('c')), "a keyless volume is written as c<n>: {volumes:?}");

    let mut copy = sandbox::test_app(42);
    snapshot::restore(copy.world_mut(), &snapshot, false).unwrap();
    // The reference leads to the updraft of the copy, the same collider.
    let touched = get::<MotionState>(&copy, PLAYER).volumes;
    let world = copy.world_mut();
    let updrafts: Vec<Entity> = touched
        .iter()
        .copied()
        .filter(|v| world.get::<Volume>(*v).is_some_and(|v| matches!(v.kind, VolumeKind::Updraft { .. })))
        .collect();
    assert_eq!(updrafts.len(), 1, "{touched:?}");
}

// --- when a snapshot does not fit ----------------------------------------------------------

#[test]
fn a_snapshot_of_other_content_or_geometry_is_refused() {
    let mut app = sandbox::test_app(42);
    step(&mut app, 10);
    let good = snapshot::capture(app.world()).unwrap();
    assert!(good.checks.contains_key("content") && good.checks.contains_key("static_colliders"), "{:?}", good.checks);

    let before = snapshot::capture(app.world()).unwrap();
    let mut other = good.clone();
    other.checks.insert("content".into(), "0000000000000000".into());
    let err = snapshot::restore(app.world_mut(), &other, false).unwrap_err();
    assert!(err.contains("content differs") && err.contains("force=true"), "{err}");
    assert_eq!(snapshot::capture(app.world()).unwrap(), before, "nothing changed");

    // A static collider moved: the snapshot cannot say where it was.
    let wall = {
        let world = app.world_mut();
        let mut q = world.query_filtered::<(Entity, &Transform), (With<shards::motion::Collider>, Without<AiId>)>();
        q.iter(world).next().map(|(e, _)| e).unwrap()
    };
    app.world_mut().get_mut::<Transform>(wall).unwrap().translation.y += 1.0;
    let err = snapshot::restore(app.world_mut(), &good, false).unwrap_err();
    assert!(err.contains("static_colliders differs"), "{err}");

    // Forced, it loads and says what it ignored.
    let report = snapshot::restore(app.world_mut(), &good, true).unwrap();
    assert!(report.warnings.iter().any(|w| w.contains("loaded anyway: static_colliders differs")), "{:?}", report.warnings);
}

#[test]
fn another_seed_or_engine_version_is_refused_with_the_fix() {
    let mut app = sandbox::test_app(42);
    let mut snapshot = snapshot::capture(app.world()).unwrap();
    snapshot.seed = 7;
    snapshot.engine = "0.1.0".into();
    let err = snapshot::restore(app.world_mut(), &snapshot, false).unwrap_err();
    assert!(err.contains("start the game with --seed 7"), "{err}");
    assert!(err.contains("written by engine 0.1.0"), "{err}");
    snapshot.tick_hz = 30.0;
    assert!(snapshot::restore(app.world_mut(), &snapshot, true).unwrap_err().contains("--hz 30"));
}

// --- through the tools and the files -------------------------------------------------------

// Tests that write into `snapshots/` take turns (`snapshot_file_turn`, shared with the
// scenarios): the last one out removes the folder.

/// A file in `snapshots/` (of the working directory) that goes away with the test.
pub(crate) struct Scratch(pub(crate) PathBuf, pub(crate) String, #[allow(dead_code, reason = "held until the test is over")] MutexGuard<'static, ()>);

impl Scratch {
    pub(crate) fn new(name: &str) -> Scratch {
        // The scenarios' own snapshot (`snapshot.scenario`) is in the same folder.
        let turn = crate::scenario_files::snapshot_file_turn();
        let relative = format!("snapshots/sandbox_test_{}_{name}.json", std::process::id());
        Scratch(PathBuf::from(&relative), relative, turn)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_dir("snapshots");
    }
}

#[test]
fn the_tools_save_and_load_and_keep_to_their_folder() {
    let file = Scratch::new("tools");
    let mut app = face_to_face();
    ok(&mut app, "attack", json!({"target": BOSS, "repeat": true}));
    step(&mut app, 60);
    let saved = ok(&mut app, "snapshot_save", json!({"path": file.1}));
    assert!(text(&saved).contains("saved ") && text(&saved).contains(&format!("tick {}", tick(&app))), "{}", text(&saved));
    assert!(file.0.exists());
    let state_then = snapshot::capture(app.world()).unwrap();
    step(&mut app, 120);

    let loaded = ok(&mut app, "snapshot_load", json!({"path": file.1}));
    assert!(text(&loaded).contains("loaded ") && text(&loaded).contains("the recording now starts at tick"), "{}", text(&loaded));
    let diff = differences(&state_then, &snapshot::capture(app.world()).unwrap());
    assert!(diff.is_empty(), "{}", diff.join("\n"));
    let journal = app.world().resource::<AiJournal>();
    assert!(journal.after(0).0.any(|e| e.kind == JournalKind::Sim && e.text.starts_with("snapshot loaded:")));

    for bad in ["/tmp/x.json", "snapshots/../x.json", "x.json", "replays/x.json"] {
        for tool in ["snapshot_save", "snapshot_load"] {
            let err = fails(&mut app, tool, json!({"path": bad}));
            assert!(err.starts_with("path must be inside snapshots/, e.g. snapshots/boss_phase2.json"), "{tool} {bad}: {err}");
        }
    }
    let missing = fails(&mut app, "snapshot_load", json!({"path": "snapshots/nowhere.json"}));
    assert!(missing.contains("cannot read") && missing.contains("nowhere.json"), "{missing}");
}

#[test]
fn the_tools_tell_what_does_not_fit() {
    let file = Scratch::new("refused");
    let mut app = sandbox::test_app(42);
    let mut snapshot = snapshot::capture(app.world()).unwrap();
    snapshot.checks.insert("content".into(), "00000000000000ff".into());
    std::fs::create_dir_all("snapshots").unwrap();
    std::fs::write(&file.0, shards::serde_json::to_string(&snapshot).unwrap()).unwrap();
    let err = fails(&mut app, "snapshot_load", json!({"path": file.1}));
    assert!(err.contains("content differs") && err.contains("force=true"), "{err}");
    let forced = ok(&mut app, "snapshot_load", json!({"path": file.1, "force": true}));
    assert!(text(&forced).contains("loaded anyway: content differs"), "{}", text(&forced));
}

/// State outside the entities that no snapshot brings back: a load cannot be exact.
#[derive(Resource, Default)]
struct Weather(u64);

#[test]
fn a_load_that_cannot_restore_exactly_fails_and_changes_nothing() {
    let file = Scratch::new("inexact");
    let mut app = face_to_face();
    app.init_resource::<Weather>().add_replay_check("weather", |w| w.resource::<Weather>().0);
    ok(&mut app, "attack", json!({"target": BOSS, "repeat": true}));
    step(&mut app, 30);
    let saved_at = tick(&app);
    ok(&mut app, "snapshot_save", json!({"path": file.1}));
    app.world_mut().resource_mut::<Weather>().0 = 7;
    step(&mut app, 40);

    let (state, tick_then, recorded) = (snapshot::capture(app.world()).unwrap(), tick(&app), replay::save(app.world()));
    let err = fails(&mut app, "snapshot_load", json!({"path": file.1}));
    assert!(err.contains("the restored state differs from the snapshot") && err.contains("add_snapshot_resource"), "{err}");
    // Failed means unchanged: the world, the tick, the recording that would have restarted.
    let diff = differences(&state, &snapshot::capture(app.world()).unwrap());
    assert!(diff.is_empty(), "{}", diff.join("\n"));
    assert_eq!(tick(&app), tick_then);
    let after = replay::save(app.world());
    assert_eq!((after.start.clone(), after.events.len()), (recorded.start.clone(), recorded.events.len()), "the recording goes on");
    let journal: Vec<String> = app.world().resource::<AiJournal>().after(0).0.map(|e| e.text.clone()).collect();
    assert!(!journal.iter().any(|l| l.starts_with("snapshot loaded:")), "{journal:#?}");
    assert!(journal.iter().any(|l| l.starts_with(&format!("snapshot of tick {saved_at} not loaded (the restored state differs"))), "{journal:#?}");

    // The game plays on from where it was.
    step(&mut app, 10);
    assert_eq!(tick(&app), tick_then + 10);
}

#[test]
fn go_to_boss_phase_two_is_a_snapshot_loaded_by_name() {
    // Recorded once (here by a test; by hand with `shards save-snapshot snapshots/boss_phase2.json`
    // when the fight stands where you want it): the warden at half health, in its second phase.
    let file = Scratch::new("boss_phase2");
    let mut recorder = sandbox::test_app(42);
    calm_skeletons(&mut recorder);
    ok(&mut recorder, "god", json!({}));
    ok(&mut recorder, "teleport", json!({"id": PLAYER, "x": 0, "z": 12}));
    ok(&mut recorder, "teleport", json!({"id": BOSS, "x": 0, "z": -6}));
    ok(&mut recorder, "set", json!({"id": BOSS, "component": "Health", "path": "current", "value": 295}));
    ok(&mut recorder, "step", json!({"ticks": 300, "until": [{"named": "boss_phase_2"}]}));
    assert!(text(&ok(&mut recorder, "observe", json!({}))).contains("phase 2"));
    ok(&mut recorder, "snapshot_save", json!({"path": file.1}));

    // Any later session, on a fresh game: one call, and the fight is in phase 2.
    let mut game = sandbox::test_app(42);
    assert!(text(&ok(&mut game, "observe", json!({}))).contains("phase 1"));
    ok(&mut game, "snapshot_load", json!({"path": file.1}));
    assert!(text(&ok(&mut game, "observe", json!({}))).contains("phase 2"));
    assert_eq!(get::<Fighter>(&game, BOSS).moveset, "warden_enraged");
    let out = ok(&mut game, "step", json!({"ticks": 900, "until": [{"journal": "e6 winds up"}]}));
    assert!(text(&out).contains("stopped: journal:") && text(&out).contains("_enraged"), "{}", text(&out));
}

// --- replays that start from a snapshot ----------------------------------------------------

/// A session recorded after loading a snapshot: the replay starts from it.
fn recorded_from_a_snapshot(file: &Scratch) -> (App, replay::Replay) {
    let mut origin = face_to_face();
    ok(&mut origin, "attack", json!({"target": BOSS, "repeat": true}));
    step(&mut origin, 70);
    ok(&mut origin, "snapshot_save", json!({"path": file.1}));

    let mut app = sandbox::test_app(42);
    // Something recorded before the load: it is not part of the new replay.
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 5, "z": 5}));
    step(&mut app, 3);
    let loaded = ok(&mut app, "snapshot_load", json!({"path": file.1}));
    assert!(text(&loaded).contains("2 earlier events dropped") || text(&loaded).contains("earlier events dropped"), "{}", text(&loaded));
    // The session after the snapshot: inputs and calls.
    step(&mut app, 40);
    ok(&mut app, "dodge", json!({"away_from": BOSS}));
    step(&mut app, 80);
    let recording = replay::save(app.world());
    // Through JSON, like a file.
    let recording = shards::serde_json::from_str(&shards::serde_json::to_string(&recording).unwrap()).unwrap();
    (app, recording)
}

#[test]
fn a_replay_recorded_after_a_load_starts_from_the_snapshot_and_plays_back() {
    let file = Scratch::new("replay");
    let (original, recording) = recorded_from_a_snapshot(&file);
    assert_eq!(recording.format, shards::ai_core::replay::FORMAT);
    let start = recording.start.as_ref().expect("the replay carries its start");
    assert_eq!(start.tick, 70 + 2, "the snapshot is the start (the two ticks of the face-to-face setup, then 70)");
    assert_eq!(recording.end.tick, start.tick + 120);
    assert!(recording.events.iter().all(|e| e.at() >= start.tick), "{:?}", recording.events);
    assert!(
        recording.events.iter().all(|e| !matches!(e, replay::ReplayEvent::Call { tool, .. } if tool == "teleport" || tool.starts_with("snapshot"))),
        "what came before the load, and the load itself, are not events: {:?}",
        recording.events
    );
    assert!(recording.events.iter().any(|e| matches!(e, replay::ReplayEvent::Call { tool, .. } if tool == "dodge")));

    // Played in a fresh game, loaded already: the snapshot goes in before the first tick.
    let mut copy = sandbox::test_app(42);
    replay::play(copy.world_mut(), recording.clone()).unwrap();
    ok(&mut copy, "step", json!({"ticks": recording.end.tick - start.tick}));
    copy.update();
    let report = copy.world().resource::<shards::ai_core::AiReplay>().report().cloned().expect("playback finished");
    assert!(report.identical(), "{report}");
    assert_eq!(report.matched as usize, recording.checks.len() + 1);
    assert_same_state("the playback", &original, &copy);
    // A branch from the playback saves as one replay with the same start.
    assert_eq!(replay::save(copy.world()).start, recording.start);

    // And started before it has loaded, as `--replay` does.
    let mut early = shards::testing::test_app(|c| c.seed = 42, |app| {
        app.add_plugins(sandbox::SandboxPlugin);
        replay::play(app.world_mut(), recording.clone()).unwrap();
    });
    ok(&mut early, "step", json!({"ticks": recording.end.tick - start.tick}));
    early.update();
    let report = early.world().resource::<shards::ai_core::AiReplay>().report().cloned().expect("playback finished");
    assert!(report.identical(), "{report}");
}

#[test]
fn a_replay_from_a_snapshot_names_the_first_diverging_tick_and_refuses_other_content() {
    let file = Scratch::new("diverge");
    let (_, mut recording) = recorded_from_a_snapshot(&file);
    // The boss with less health than the recording says: the first check after the start differs.
    for event in &mut recording.events {
        if let replay::ReplayEvent::Call { args, tool, .. } = event
            && tool == "dodge"
        {
            // As if the boss had less health in a newer build.
            *tool = "set".into();
            *args = json!({"id": BOSS, "component": "Health", "path": "current", "value": 299});
        }
    }
    let mut copy = sandbox::test_app(42);
    replay::play(copy.world_mut(), recording.clone()).unwrap();
    ok(&mut copy, "step", json!({"ticks": recording.end.tick - recording.start.as_ref().unwrap().tick}));
    copy.update();
    let report = copy.world().resource::<shards::ai_core::AiReplay>().report().cloned().unwrap();
    assert!(report.diverged_at.is_some(), "{report}");

    // A snapshot of other content cannot be played at all.
    let (_, mut recording) = recorded_from_a_snapshot(&file);
    recording.start.as_mut().unwrap().checks.insert("content".into(), "00000000000000ff".into());
    let mut copy = sandbox::test_app(42);
    replay::play(copy.world_mut(), recording).unwrap();
    copy.update();
    let report = copy.world().resource::<shards::ai_core::AiReplay>().report().cloned().expect("playback ended at once");
    let why = report.interrupted.expect("interrupted");
    assert!(why.contains("cannot start from its snapshot") && why.contains("content differs"), "{why}");
}

#[test]
fn a_replay_whose_start_does_not_restore_exactly_says_so_in_its_headline() {
    let file = Scratch::new("inexact");
    let (_, mut recording) = recorded_from_a_snapshot(&file);
    // A newer build that hashes state the snapshot cannot carry: the restore finds the world
    // is not what the snapshot says, though every entity matches.
    recording.start.as_mut().unwrap().state_hash ^= 1;
    let start_tick = recording.start.as_ref().unwrap().tick;
    let mut copy = sandbox::test_app(42);
    replay::play(copy.world_mut(), recording.clone()).unwrap();
    ok(&mut copy, "step", json!({"ticks": recording.end.tick - start_tick}));
    copy.update();
    let report = copy.world().resource::<shards::ai_core::AiReplay>().report().cloned().expect("playback finished");
    assert_eq!(report.diverged_at, Some(start_tick), "{report}");
    let headline = report.headline();
    let expected = format!("DIVERGED at tick {start_tick}: no details: the start does not restore exactly: the state hash is ");
    assert!(headline.contains(&expected), "`--verify` prints the headline, so it carries the reason: {headline}");
}

#[test]
fn loading_a_snapshot_ends_a_playback_and_starts_a_new_recording() {
    let file = Scratch::new("takeover");
    let (_, recording) = recorded_from_a_snapshot(&file);
    let mut app = sandbox::test_app(42);
    replay::play(app.world_mut(), recording).unwrap();
    ok(&mut app, "step", json!({"ticks": 30}));
    assert!(app.world().resource::<shards::ai_core::AiReplay>().is_playing());
    ok(&mut app, "snapshot_load", json!({"path": file.1}));
    let report = app.world().resource::<shards::ai_core::AiReplay>().report().cloned().unwrap();
    assert!(report.interrupted.as_deref().is_some_and(|w| w.contains("a snapshot was loaded")), "{report}");
    assert!(!app.world().resource::<shards::ai_core::AiReplay>().is_playing());
    // The recording is the snapshot's now.
    let now = replay::save(app.world());
    assert_eq!(now.start.as_ref().map(|s| s.tick), Some(tick(&app)));
    assert!(now.events.is_empty());
}

/// The region of the given seed, the player (unhurtable) in the pillar hall north of the
/// colonnade, the den guard on its way around the columns to it.
fn in_the_hall() -> App {
    let mut app = sandbox::test_app(42);
    ok(&mut app, "regen", json!({"region": "wilds", "seed": 42}));
    ok(&mut app, "god", json!({}));
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 24, "z": 130}));
    let out = ok(&mut app, "step", json!({"ticks": 600, "until": [{"named": "npc_detour"}]}));
    assert!(text(&out).contains("stopped: condition npc_detour"), "{}", text(&out));
    ok(&mut app, "step", json!({"ticks": 45}));
    app
}

#[test]
fn a_snapshot_in_the_generated_region_goes_on_the_same_in_a_fresh_arena() {
    // Taken mid-walk around the columns (a navmesh route in its nav agent), loaded into a game
    // that has no region: the wilds are built again from the snapshot's manifest, with the
    // same ids, and the fight goes on as it did.
    let a = in_the_hall();
    assert!(story_after(&a, 0).iter().any(|l| l.contains("idle → fight e5")), "{:#?}", story_after(&a, 0));
    assert_continues_alike("in the region", a, |app| {
        ok(app, "step", json!({"ticks": 240}));
        ok(app, "move_to", json!({"target": {"x": 24, "z": 112}, "within": 0.5}));
        ok(app, "step", json!({"ticks": 300}));
    });

    // The snapshot tool on the same: the region comes back where the player went home to the
    // arena (another region loaded, then none).
    let file = Scratch::new("wilds");
    let mut app = in_the_hall();
    ok(&mut app, "snapshot_save", json!({"path": file.1}));
    let before = tick(&app);
    ok(&mut app, "regen", json!({"region": "wilds", "seed": 7}));
    ok(&mut app, "step", json!({"ticks": 30}));
    let loaded = text(&ok(&mut app, "snapshot_load", json!({"path": file.1}))).to_owned();
    assert!(loaded.contains("region: wilds seed 42 v1 built again from its manifest"), "{loaded}");
    assert_eq!(tick(&app), before);
    let region = app.world().resource::<ActiveRegion>();
    assert_eq!((region.manifest.seed, region.manifest.region.as_str()), (42, "wilds"));
}

// --- loads across regions and placed levels ----------------------------------------------

/// The instance number in `spawn_level`'s answer (`spawned level `ruin` as instance 5 at …`).
fn instance_of(out: &shards::ai_core::CallOutcome) -> u32 {
    let text = text(out);
    let after = text.split("as instance ").nth(1).unwrap_or_else(|| panic!("no instance in: {text}"));
    after.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or_else(|| panic!("no instance in: {text}"))
}

/// The ruin of the content as an agent places it (not a tile of the region), east of everything.
fn place_ruin(app: &mut App, z: f32) -> u32 {
    instance_of(&ok(app, "spawn_level", json!({"id": "ruin", "x": 300, "z": z})))
}

/// The instances of the levels agents placed (by id; tiles go by their file).
fn agent_levels(app: &App) -> Vec<u32> {
    let instances = app.world().resource::<shards::level::LevelInstances>();
    instances.iter().filter(|i| i.label == "ruin").map(|i| i.id).collect()
}

/// Entities of the agent's level instance `n`.
fn level_entities(app: &mut App, n: u32) -> usize {
    let world = app.world_mut();
    world.query::<&FromLevel>().iter(world).filter(|f| f.instance == n && f.level == "ruin").count()
}

#[test]
fn a_load_refused_after_its_region_was_built_leaves_the_world_as_it_was() {
    // Taken in wilds 42 with a ruin an agent placed; loaded where the game went on to wilds 43
    // and placed another ruin. The region and the snapshot's ruin are built again first, and only
    // then do the static colliders compare: the file says other ones (as from another build of
    // the game), the load is refused, and the world must be what it was before the call (region,
    // its ruin, ids, counters, hash, journal), so the session's recording still plays back.
    let file = Scratch::new("refused_late");
    let mut app = sandbox::test_app(42);
    ok(&mut app, "regen", json!({"region": "wilds", "seed": 42}));
    place_ruin(&mut app, 0.0);
    step(&mut app, 10);
    ok(&mut app, "snapshot_save", json!({"path": file.1}));
    let mut saved: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&file.0).unwrap()).unwrap();
    saved["checks"]["static_colliders"] = json!("0000000000000000");
    std::fs::write(&file.0, saved.to_string()).unwrap();
    ok(&mut app, "regen", json!({"region": "wilds", "seed": 43}));
    place_ruin(&mut app, 40.0);
    ok(&mut app, "move", json!({"dir": "N", "meters": 3}));
    step(&mut app, 20);

    let before = snapshot::capture(app.world()).unwrap();
    let seen = app.world().resource::<AiJournal>().last_seq();
    let err = fails(&mut app, "snapshot_load", json!({"path": file.1}));
    assert!(err.contains("static_colliders differs"), "{err}");
    let after = snapshot::capture(app.world()).unwrap();
    let diff = differences(&before, &after);
    assert!(diff.is_empty(), "the refused load changed the world:\n{}", diff.join("\n"));
    assert_eq!(app.world().resource::<ActiveRegion>().manifest.seed, 43);

    // One line says what happened: no region came and went as far as the journal tells.
    let journal = app.world().resource::<AiJournal>();
    let lines: Vec<String> =
        journal.after(seen).0.filter(|e| e.kind != JournalKind::Agent).map(ToString::to_string).collect();
    assert_eq!(lines.len(), 1, "{lines:#?}");
    assert!(lines[0].contains("snapshot of tick 10 not loaded") && lines[0].contains("as it was"), "{}", lines[0]);

    // The session goes on, and its recording (never restarted: the load did not happen) plays.
    ok(&mut app, "move", json!({"dir": "E", "meters": 4}));
    step(&mut app, 70);
    let recording = replay::save(app.world());
    assert!(recording.start.is_none());
    let report = shards::testing::verify(&mut sandbox::test_app(42), recording).expect("plays");
    assert!(report.identical(), "{report}");
}

#[test]
fn a_level_placed_after_a_load_gets_the_number_it_got_in_the_original_session() {
    // The original session: wilds 42, a ruin placed (after the tiles' instances), then another.
    let mut original = sandbox::test_app(42);
    ok(&mut original, "regen", json!({"region": "wilds", "seed": 42}));
    let first = place_ruin(&mut original, 0.0);
    let snapshot = snapshot::capture(original.world()).unwrap();
    let second = place_ruin(&mut original, 40.0);
    assert_eq!(second, first + 1);

    // The same session that went to wilds 43 before loading the snapshot: the region's tiles
    // are numbered again from where they were, and the next level still gets `second`.
    let mut app = sandbox::test_app(42);
    ok(&mut app, "regen", json!({"region": "wilds", "seed": 42}));
    assert_eq!(place_ruin(&mut app, 0.0), first);
    ok(&mut app, "regen", json!({"region": "wilds", "seed": 43}));
    let report = snapshot::restore(app.world_mut(), &snapshot, false).unwrap();
    assert!(report.exact(), "{:?}", report.differences);
    assert_eq!(place_ruin(&mut app, 40.0), second, "the number the original session gave");

    // Tiles never take an agent's number, so later region changes leave the ruins standing.
    let entities = (level_entities(&mut app, first), level_entities(&mut app, second));
    assert!(entities.0 > 0 && entities.0 == entities.1, "{entities:?}");
    ok(&mut app, "regen", json!({"region": "wilds", "seed": 44}));
    ok(&mut app, "regen", json!({"region": "wilds", "seed": 45}));
    assert_eq!(agent_levels(&app), [first, second]);
    assert_eq!((level_entities(&mut app, first), level_entities(&mut app, second)), entities);
}

#[test]
fn a_snapshot_from_before_any_level_loads_exactly_after_one_and_back() {
    // Taken in the arena before any level was placed; loaded after a visit to the wilds,
    // whose ruin tiles are level files. The state hash may not remember the visit.
    let file = Scratch::new("before_levels");
    let mut app = sandbox::test_app(42);
    step(&mut app, 5);
    ok(&mut app, "snapshot_save", json!({"path": file.1}));
    let arena = snapshot::capture(app.world()).unwrap();
    ok(&mut app, "regen", json!({"region": "wilds", "seed": 42}));
    step(&mut app, 10);
    let loaded = ok(&mut app, "snapshot_load", json!({"path": file.1}));
    assert!(text(&loaded).contains("region: wilds seed 42 v1 unloaded"), "{}", text(&loaded));
    assert_eq!(replay::state_hash(app.world()), arena.state_hash);
    assert!(app.world().get_resource::<ActiveRegion>().is_none());

    // A recording restarted from the load plays in a fresh game, which never had a level.
    ok(&mut app, "move", json!({"dir": "S", "meters": 4}));
    step(&mut app, 70);
    let report = shards::testing::verify(&mut sandbox::test_app(42), replay::save(app.world())).expect("plays");
    assert!(report.identical(), "{report}");

    // The other way: taken in the arena after the visit, loaded into a fresh game.
    let back = snapshot::capture(app.world()).unwrap();
    let mut fresh = sandbox::test_app(42);
    let report = snapshot::restore(fresh.world_mut(), &back, false).unwrap();
    assert!(report.exact(), "{:?}", report.differences);
    assert_eq!(replay::state_hash(fresh.world()), back.state_hash);
}

// --- content edits made before the snapshot -------------------------------------------------

/// The first light attack as the game has it now (JSON).
fn light_1(app: &App) -> Value {
    app.world().resource::<shards::content::ContentDb>().get_json("action", "light_1").unwrap().clone()
}

/// `content_apply` arguments that make the first light attack hit for 90 instead of its file's
/// 20: a definition no file says.
fn harder_light(app: &App) -> Value {
    let before = light_1(app);
    let mut def = before.clone();
    def["hits"][0]["damage"] = json!(90);
    json!({"changes": [{"kind": "action", "id": "light_1", "def": def, "before": before}]})
}

fn edits_in(snapshot: &Snapshot) -> Option<&Value> {
    snapshot.resources.iter().find(|(path, _)| path.ends_with("::ContentEdits")).map(|(_, edits)| edits)
}

#[test]
fn a_content_edit_made_before_the_snapshot_comes_back_with_it_and_goes_again() {
    let plain = snapshot::capture(face_to_face().world()).unwrap();
    assert_eq!(edits_in(&plain), None, "a game without edits saves none: its snapshots are what they were");

    let mut a = face_to_face();
    let edit = harder_light(&a);
    ok(&mut a, "content_apply", edit);
    step(&mut a, 3);
    let edited = snapshot::capture(a.world()).unwrap();
    let saved = edits_in(&edited).expect("the edit is in the snapshot");
    assert_eq!(saved["edits"][0]["kind"], "action");
    assert_eq!(saved["edits"][0]["id"], "light_1");

    // A fresh game reads the files; the load puts the edit over them, and says so.
    let mut b = sandbox::test_app(42);
    assert_eq!(light_1(&b)["hits"][0]["damage"].as_f64(), Some(20.0));
    let report = snapshot::restore(b.world_mut(), &edited, false).unwrap();
    assert!(report.exact(), "{:?}", report.differences);
    assert_eq!(light_1(&b)["hits"][0]["damage"].as_f64(), Some(90.0));
    assert!(
        report.warnings.iter().any(|w| w == "content: 1 definition as the snapshot's session had it, over the content files"),
        "{:?}",
        report.warnings
    );
    assert!(!report.warnings.iter().any(|w| w.contains("ContentEdits")), "the saved edits load: {:?}", report.warnings);

    // A load refused after the prepares put the world back, its content too.
    let mut broken = plain.clone();
    broken.checks.insert("static_colliders".into(), "0000000000000000".into());
    let err = snapshot::restore(b.world_mut(), &broken, false).unwrap_err();
    assert!(err.contains("static_colliders differs"), "{err}");
    assert_eq!(light_1(&b)["hits"][0]["damage"].as_f64(), Some(90.0), "the edit is still there");
    assert_eq!(edits_in(&snapshot::capture(b.world()).unwrap()), Some(saved));

    // A snapshot without edits takes them away: the files' version again.
    let report = snapshot::restore(b.world_mut(), &plain, false).unwrap();
    assert!(report.exact(), "{:?}", report.differences);
    assert_eq!(light_1(&b)["hits"][0]["damage"].as_f64(), Some(20.0));
    assert!(report.warnings.iter().any(|w| w == "content: the definitions are the content files' again"), "{:?}", report.warnings);
    assert_eq!(edits_in(&snapshot::capture(b.world()).unwrap()), None);
}

#[test]
fn a_fight_after_a_content_edit_goes_on_the_same_from_a_snapshot() {
    let mut a = face_to_face();
    let edit = harder_light(&a);
    ok(&mut a, "content_apply", edit);
    step(&mut a, 3);
    assert_continues_alike("after a content edit", a, |app| {
        ok(app, "attack", json!({"target": BOSS, "repeat": true}));
        step(app, 120);
    });
}

#[test]
fn a_replay_from_a_snapshot_taken_after_a_content_edit_plays_in_a_fresh_game() {
    let file = Scratch::new("edited");
    let mut origin = face_to_face();
    let edit = harder_light(&origin);
    ok(&mut origin, "content_apply", edit);
    ok(&mut origin, "attack", json!({"target": BOSS, "repeat": true}));
    step(&mut origin, 30);
    ok(&mut origin, "snapshot_save", json!({"path": file.1}));

    // Another game goes on from the snapshot and records from it.
    let mut app = sandbox::test_app(42);
    ok(&mut app, "snapshot_load", json!({"path": file.1}));
    assert_eq!(light_1(&app)["hits"][0]["damage"].as_f64(), Some(90.0));
    step(&mut app, 90);
    let recording = replay::save(app.world());
    assert!(recording.start.as_ref().and_then(edits_in).is_some(), "the replay's start carries the edit");

    // A fresh game plays it: the start puts the edit over its files first.
    let report = shards::testing::verify(&mut sandbox::test_app(42), recording).expect("plays");
    assert!(report.identical(), "{report}");
}

#[test]
fn an_edit_that_rebuilt_the_arenas_walls_before_the_snapshot_is_refused_by_the_geometry_check() {
    // The sandbox builds its walls from `layout` content (no `AiId`, static colliders): the edit
    // comes back with the snapshot, the walls it rebuilt do not, and the check says so.
    let mut a = sandbox::test_app(42);
    let before = a.world().resource::<shards::content::ContentDb>().get_json("layout", "arena").unwrap().clone();
    let mut def = before.clone();
    def["solids"][1]["max"][1] = json!(5.0);
    ok(&mut a, "content_apply", json!({"changes": [{"kind": "layout", "id": "arena", "def": def, "before": before}]}));
    step(&mut a, 2);
    let edited = snapshot::capture(a.world()).unwrap();
    assert!(edits_in(&edited).is_some());

    let mut b = sandbox::test_app(42);
    let err = snapshot::restore(b.world_mut(), &edited, false).unwrap_err();
    assert!(err.contains("static_colliders differs"), "{err}");
    let arena = b.world().resource::<shards::content::ContentDb>().get_json("layout", "arena").unwrap().clone();
    assert_eq!(arena["solids"][1]["max"][1].as_f64(), Some(3.0), "the content is the files' again");
}

/// The player (a duelist) 1.8 m south of the skeleton e7 (a sparring partner), facing it; both calm.
fn sparring_pair() -> App {
    let mut app = sandbox::test_app(42);
    calm_skeletons(&mut app);
    calm_warden(&mut app);
    ok(&mut app, "set", json!({"id": PLAYER, "component": "Fighter", "path": "moveset", "value": "duelist"}));
    ok(&mut app, "set", json!({"id": "e7", "component": "Fighter", "path": "moveset", "value": "sparring"}));
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 0, "z": 0}));
    ok(&mut app, "teleport", json!({"id": "e7", "x": 0, "z": -1.8}));
    ok(&mut app, "lock_on", json!({"target": "e7"}));
    step(&mut app, 2);
    app
}

#[test]
fn a_reaction_with_its_push_and_timers_comes_back() {
    use shards::action::{Knockback, Protection};
    // The tick after a tier 2 reaction: the push is live, and so is the immunity.
    let mut app = sparring_pair();
    ok(&mut app, "heavy_attack", json!({}));
    ok(&mut app, "step", json!({"ticks": 80, "until": [{"journal": "e7 reacts heavy"}]}));
    step(&mut app, 1);
    assert_eq!(action(&app, "e7"), "spar_react_heavy");
    assert!(get::<Knockback>(&app, "e7").velocity.length() > 1.0, "the push is running");
    assert!(get::<Protection>(&app, "e7").reactions > 0.0, "the immunity is running");
    assert_continues_alike("a heavy reaction", app, |app| {
        step(app, 60);
    });
}

#[test]
fn a_break_and_the_protection_after_it_come_back() {
    use shards::action::Protection;
    // A poise break that has just run out: the poise protection is live, and the reaction protection
    // the stagger's end granted with it.
    let mut app = sparring_pair();
    ok(&mut app, "set", json!({"id": "e7", "component": "Poise", "path": "current", "value": 1}));
    ok(&mut app, "light_attack", json!({}));
    ok(&mut app, "step", json!({"ticks": 80, "until": [{"journal": "e7 staggered (poise broken)"}]}));
    step(&mut app, 50);
    assert_eq!(action(&app, "e7"), "idle");
    let protection = get::<Protection>(&app, "e7");
    assert!(protection.poise > 1.0 && protection.reactions > 0.0, "{protection:?}");
    assert_continues_alike("after a break", app, |app| {
        step(app, 200);
    });
}

/// The entity of `id`.
fn entity(app: &App, id: &str) -> Entity {
    app.world().resource::<shards::ai_core::AiIndex>().get(AiId::parse(id).unwrap()).unwrap()
}

/// The player with the `contextual` moveset (B3) 1.8 m south of the skeleton e7, locked on to it;
/// e7 with the `sparring` moveset, calm, its poise broken.
fn next_to_a_broken_skeleton() -> App {
    let mut app = sandbox::test_app(42);
    calm_skeletons(&mut app);
    calm_warden(&mut app);
    ok(&mut app, "set", json!({"id": PLAYER, "component": "Fighter", "path": "moveset", "value": "contextual"}));
    ok(&mut app, "set", json!({"id": "e7", "component": "Fighter", "path": "moveset", "value": "sparring"}));
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 0, "z": 0, "y": 0}));
    ok(&mut app, "teleport", json!({"id": "e7", "x": 0, "z": -1.8, "y": 0}));
    step(&mut app, 2);
    ok(&mut app, "lock_on", json!({"target": "e7"}));
    ok(&mut app, "stagger", json!({"id": "e7", "poise": true}));
    step(&mut app, 1);
    app
}

/// Context and paired actions (B3): taken mid-pair (the finisher holds the skeleton, its victim
/// action runs, the context and the memories are live), and with a guard's press armed.
#[test]
fn mid_pair_and_with_a_guard_press_armed() {
    use shards::action::{ContextInputs, GuardArm, Paired, Pairing};
    let mut app = next_to_a_broken_skeleton();
    assert!(app.world().get::<ContextInputs>(entity(&app, PLAYER)).is_some(), "the finisher is bound");
    ok(&mut app, "light_attack", json!({}));
    step(&mut app, 4);
    assert_eq!(action(&app, PLAYER), "spar_finisher");
    assert!(app.world().get::<Pairing>(entity(&app, PLAYER)).is_some());
    assert!(app.world().get::<Paired>(entity(&app, "e7")).is_some());
    assert_eq!(action(&app, "e7"), "spar_finished");
    assert_continues_alike("mid-pair", app, |app| {
        step(app, 90);
    });

    let mut app = next_to_a_broken_skeleton();
    ok(&mut app, "heal", json!({"id": "e7"}));
    ok(&mut app, "block", json!({}));
    step(&mut app, 3);
    assert!(app.world().get::<GuardArm>(entity(&app, PLAYER)).is_some_and(|a| a.run.is_some()), "the guard took a fresh press");
    assert_continues_alike("guard armed", app, |app| {
        ok(app, "block", json!({"on": false}));
        step(app, 6);
        ok(app, "block", json!({}));
        step(app, 40);
    });
}

#[test]
fn a_barrier_a_timed_multiplier_and_a_proc_gate_come_back() {
    use shards::action::{Barrier, DamageTaken, ProcGates};
    // The player (a `bodystate` kit) has a barrier of 30 and the skeleton takes x1.35 for 3 s; the
    // player's frost jab has just procced the chill on the skeleton, so its proc gate runs.
    let mut app = sparring_pair();
    ok(&mut app, "set", json!({"id": PLAYER, "component": "Fighter", "path": "moveset", "value": "bodystate"}));
    ok(&mut app, "barrier", json!({"id": PLAYER, "amount": 30, "seconds": 5}));
    ok(&mut app, "damage_taken", json!({"id": "e7", "key": "core", "mult": 1.35, "seconds": 3}));
    ok(&mut app, "use_technique", json!({"slot": 2}));
    ok(&mut app, "step", json!({"ticks": 60, "until": [{"journal": "e7 spar_chill applied"}]}));
    step(&mut app, 3);
    assert_eq!(get::<Barrier>(&app, PLAYER).amount, 30.0);
    assert!(get::<DamageTaken>(&app, "e7").get("core").is_some_and(|m| m.mult == 1.35 && m.left.is_some()));
    assert!(get::<ProcGates>(&app, "e7").gated("spar_chill"), "the gate is closed");
    assert_continues_alike("a barrier, a multiplier and a gate", app, |app| {
        // The frost jab again inside the gate (swallowed), then time past all three timers.
        step(app, 20);
        ok(app, "use_technique", json!({"slot": 2}));
        step(app, 30);
        step(app, 250);
    });
}
