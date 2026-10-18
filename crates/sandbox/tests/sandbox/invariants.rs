//! Invariants: rules checked after every tick. A broken rule halts the game the way a
//! failed gameplay system does: journal error, pause, replay file, `step` stops there.

use shards::ai_core::{self, replay, AiInvariants, AiJournal, AiReplay, CallOutcome, JournalKind};
use shards::bevy::time::TimeUpdateStrategy;
use shards::prelude::*;
use shards::serde_json::{self, json, Value};

use crate::common::{act, text, BOSS};

/// The engine's rules, in the order the modules register them.
pub(crate) const ENGINE_RULES: [&str; 8] = [
    "ai.index_consistent",
    "core.intent_sane",
    "motion.finite",
    "action.resources_in_range",
    "action.running_sane",
    "npc.state_known",
    "npc.strike_spacing",
    "npc.tokens_within_limit",
];

/// [`act`] that must succeed. Unlike `common::ok`, the game may halt during the call:
/// that is what these tests look at.
pub(crate) fn called(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    let out = act(app, tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    out
}

/// Runs up to `ticks` ticks; the reply.
pub(crate) fn step(app: &mut App, ticks: u64) -> String {
    text(&called(app, "step", json!({"ticks": ticks}))).to_owned()
}

pub(crate) fn journal(app: &App, kind: JournalKind) -> Vec<String> {
    let journal = app.world().resource::<AiJournal>();
    journal.after(0).0.filter(|e| e.kind == kind).map(ToString::to_string).collect()
}

fn boss_health(app: &mut App, value: f64) {
    called(app, "set", json!({"id": BOSS, "component": "Health", "path": "current", "value": value}));
}

/// The session rule the tests below break: the boss (600 max) below 300 health (a legal
/// value, so the game's own `health_in_range` stays quiet).
fn hurt(name: &str) -> Value {
    json!({"name": name, "when": {"field": {"id": BOSS, "component": "Health", "path": "current", "below": 300}}})
}

#[test]
fn a_broken_game_rule_stops_the_step_at_its_tick() {
    let mut app = sandbox::test_app(42);
    // Above the max. (Below 0 a living boss dies, and death writes its health back as 0:
    // the lower bound is tested on a dead one, below.)
    boss_health(&mut app, 700.0);
    let out = called(&mut app, "step", json!({"ticks": 10}));
    assert_eq!(text(&out), "ran 1 ticks (0 → 1), stopped: invariant action.resources_in_range violated at tick 1 (+1 more)");
    assert_eq!(ai_core::sim::tick(app.world()), 1);
    assert!(out.paused);
    let line = "1 [error] invariant action.resources_in_range violated at tick 1: e6 Health.current = 700 outside 0..=600";
    assert!(journal(&app, JournalKind::Error).iter().any(|l| l == line), "{:?}", journal(&app, JournalKind::Error));
    assert!(out.journal.iter().any(|l| l == line), "the step returns it: {:?}", out.journal);
    let halted = out.halted.expect("the outcome says the game halted");
    assert_eq!(halted.reason, "invariant action.resources_in_range violated at tick 1");
    // The game's own rule saw the same thing, and fired too.
    let own = "1 [error] invariant health_in_range violated at tick 1: e6 Health.current = 700 outside 0..=600";
    assert!(journal(&app, JournalKind::Error).iter().any(|l| l == own), "{:?}", journal(&app, JournalKind::Error));

    // The rule that fired stays off: the same state does not stop the next step.
    let next = called(&mut app, "step", json!({"ticks": 10}));
    assert_eq!(text(&next), "ran 10 ticks (1 → 11)");
    assert!(next.halted.is_none());
}

#[test]
fn health_below_zero_breaks_the_game_rule_of_a_dead_boss() {
    let mut app = sandbox::test_app(42);
    // A dead fighter keeps the health it is set to (death only rewrites a living one's), and
    // the boss leaves no remains: its body stays to carry a negative value.
    boss_health(&mut app, 0.0);
    assert_eq!(step(&mut app, 1), "ran 1 ticks (0 → 1)", "0 is in range: the boss dies, nothing halts");
    boss_health(&mut app, -5.0);
    let out = called(&mut app, "step", json!({"ticks": 10}));
    assert_eq!(text(&out), "ran 1 ticks (1 → 2), stopped: invariant action.resources_in_range violated at tick 2 (+1 more)");
    let line = "2 [error] invariant action.resources_in_range violated at tick 2: e6 Health.current = -5 outside 0..=600";
    assert!(journal(&app, JournalKind::Error).iter().any(|l| l == line), "{:?}", journal(&app, JournalKind::Error));
}

#[test]
fn real_time_play_pauses_on_a_violation() {
    let mut app = sandbox::test_app(42);
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1)); // one tick per frame
    boss_health(&mut app, 700.0);
    called(&mut app, "resume", json!({}));
    for _ in 0..10 {
        app.update();
    }
    assert!(ai_core::sim::is_paused(app.world()), "the violation must pause the game");
    assert_eq!(ai_core::sim::tick(app.world()), 1, "no tick after the one that broke the rule");
    let paused = "1 [sim] paused: invariant action.resources_in_range violated at tick 1";
    assert!(journal(&app, JournalKind::Sim).iter().any(|l| l == paused), "{:?}", journal(&app, JournalKind::Sim));
}

#[test]
fn a_session_rule_fires_once_until_it_is_reset() {
    let mut app = sandbox::test_app(42);
    let added = called(&mut app, "assert_add", hurt("hurt"));
    assert_eq!(text(&added), "added hurt: the game halts when field:e6:Health.current<300");
    boss_health(&mut app, 250.0);
    assert_eq!(step(&mut app, 10), "ran 1 ticks (0 → 1), stopped: invariant hurt violated at tick 1");
    let line = "1 [error] invariant hurt violated at tick 1: e6 Health.current = 250";
    assert!(journal(&app, JournalKind::Error).iter().any(|l| l == line), "{:?}", journal(&app, JournalKind::Error));

    assert_eq!(step(&mut app, 10), "ran 10 ticks (1 → 11)", "fired once, then off");
    assert_eq!(text(&called(&mut app, "assert_reset", json!({}))), "re-armed hurt");
    assert_eq!(step(&mut app, 10), "ran 1 ticks (11 → 12), stopped: invariant hurt violated at tick 12", "the same violation, again");

    boss_health(&mut app, 600.0);
    called(&mut app, "assert_reset", json!({}));
    assert_eq!(step(&mut app, 10), "ran 10 ticks (12 → 22)", "fixed: the step goes on");
    let fired: Vec<&str> = app.world().resource::<AiInvariants>().violations().iter().map(|v| v.name.as_str()).collect();
    assert_eq!(fired, ["hurt", "hurt"]);
}

#[test]
fn rules_are_listed_and_checked_before_they_are_added() {
    let mut app = sandbox::test_app(42);
    let list = text(&called(&mut app, "assert_list", json!({}))).to_owned();
    assert!(list.starts_with("9 rules, checked after every tick"), "{list}");
    for engine in ENGINE_RULES {
        assert!(list.contains(&format!("  {engine}  engine rule  armed")), "{engine}: {list}");
    }
    assert!(list.contains("  health_in_range  game rule  armed") && list.contains("  bodies_in_world  game rule  armed"), "{list}");

    let mut refuse = |tool: &str, args: Value| {
        let out = act(&mut app, tool, args);
        assert!(!out.ok, "{tool} must refuse");
        text(&out).to_owned()
    };
    let name = refuse("assert_add", json!({"name": "boss hp", "when": "error"}));
    assert!(name.contains("bad rule name `boss hp`") && name.contains("letters, digits"), "{name}");
    let nobody = refuse("assert_add", json!({"name": "x", "when": {"field": {"id": 99, "component": "Health", "path": "current", "below": 1}}}));
    assert!(nobody.contains("e99 does not exist"), "{nobody}");
    let open = refuse("assert_add", json!({"name": "x", "when": {"field": {"id": 6, "component": "Health", "path": "current"}}}));
    assert!(open.contains("needs `below` or `above`"), "{open}");
    let taken = refuse("assert_add", json!({"name": "health_in_range", "when": "error"}));
    assert!(taken.contains("one of the game's invariants"), "{taken}");
    let stays = refuse("assert_remove", json!({"name": "bodies_in_world"}));
    assert!(stays.contains("one of the game's invariants and stays"), "{stays}");
    let unknown = refuse("assert_remove", json!({"name": "nope"}));
    assert!(unknown.contains(&format!("no rule `nope`; rules: {}, health_in_range, bodies_in_world", ENGINE_RULES.join(", "))), "{unknown}");

    called(&mut app, "assert_add", hurt("hurt"));
    let replaced = called(&mut app, "assert_add", json!({"name": "hurt", "when": {"gone": 7}}));
    assert_eq!(text(&replaced), "replaced hurt: the game halts when gone:e7");
    called(&mut app, "despawn", json!({"id": "e7"}));
    assert!(step(&mut app, 5).ends_with("stopped: invariant hurt violated at tick 1"));
    let list = text(&called(&mut app, "assert_list", json!({}))).to_owned();
    assert!(list.contains("  hurt  when gone:e7  fired at tick 1, off until assert_reset"), "{list}");
    assert!(list.ends_with("violations so far: 1 (last: invariant hurt violated at tick 1)"), "{list}");
    assert_eq!(text(&called(&mut app, "assert_remove", json!({"name": "hurt"}))), "removed hurt");
    assert_eq!(text(&called(&mut app, "assert_reset", json!({}))), "no rule has fired; nothing to re-arm");
}

/// Writes straight into the journal, the way a game's own warnings and errors land there.
fn creaking_ice(tick: Res<SimTick>, mut journal: ResMut<AiJournal>) {
    match tick.0 {
        30 | 60 => {
            journal.push(tick.0, JournalKind::Warn, "the ice creaks");
        }
        45 | 90 => {
            journal.push(tick.0, JournalKind::Error, "the ice cracks");
        }
        _ => {}
    }
}

#[test]
fn warnings_and_errors_stop_a_step_and_break_rules() {
    let mut app = sandbox::test_app(42);
    app.add_systems(FixedUpdate, creaking_ice.in_set(SimSet::Resolve));
    let out = called(&mut app, "step", json!({"ticks": 100, "until": ["warning"]}));
    assert_eq!(text(&out), "ran 30 ticks (0 → 30), stopped: journal: 30 [warn] the ice creaks");
    let out = called(&mut app, "step", json!({"ticks": 100, "until": ["error"]}));
    assert_eq!(text(&out), "ran 15 ticks (30 → 45), stopped: journal: 45 [error] the ice cracks");

    called(&mut app, "assert_add", json!({"name": "calm", "when": "error"}));
    called(&mut app, "assert_add", json!({"name": "quiet", "when": "warning"}));
    assert_eq!(step(&mut app, 100), "ran 15 ticks (45 → 60), stopped: invariant quiet violated at tick 60");
    let line = "60 [error] invariant quiet violated at tick 60: journal: 60 [warn] the ice creaks";
    assert!(journal(&app, JournalKind::Error).iter().any(|l| l == line), "{:?}", journal(&app, JournalKind::Error));
    // `calm` does not fire on the error `quiet` wrote, only on the game's next one.
    assert_eq!(step(&mut app, 100), "ran 30 ticks (60 → 90), stopped: invariant calm violated at tick 90");
}

/// A movement bug: e7 ends the tick far outside every layout and region at tick 5.
fn leaky_wall(tick: Res<SimTick>, index: Res<ai_core::AiIndex>, mut q: Query<&mut Transform>) {
    if tick.0 == 5
        && let Some(mut tf) = index.get(AiId(7)).and_then(|e| q.get_mut(e).ok())
    {
        tf.translation.x = 200.0;
        tf.translation.z = 0.0;
    }
}

/// A fall through the floor: e7 far below the ground at tick 5.
fn trapdoor(tick: Res<SimTick>, index: Res<ai_core::AiIndex>, mut q: Query<&mut Transform>) {
    if tick.0 == 5
        && let Some(mut tf) = index.get(AiId(7)).and_then(|e| q.get_mut(e).ok())
    {
        tf.translation.y = -80.0;
    }
}

#[test]
fn the_world_rule_catches_a_body_outside_the_world() {
    let mut app = sandbox::test_app(42);
    app.add_systems(FixedUpdate, leaky_wall.in_set(SimSet::Cleanup));
    assert_eq!(step(&mut app, 10), "ran 5 ticks (0 → 5), stopped: invariant bodies_in_world violated at tick 5");
    let line = "5 [error] invariant bodies_in_world violated at tick 5: e7 outside the world: x = 200.0, z = 0.0";
    assert!(journal(&app, JournalKind::Error).iter().any(|l| l == line), "{:?}", journal(&app, JournalKind::Error));

    let mut app = sandbox::test_app(42);
    app.add_systems(FixedUpdate, trapdoor.in_set(SimSet::Cleanup));
    assert_eq!(step(&mut app, 10), "ran 5 ticks (0 → 5), stopped: invariant bodies_in_world violated at tick 5");
    let line = "5 [error] invariant bodies_in_world violated at tick 5: e7 fell out of the world: y = -80.0";
    assert!(journal(&app, JournalKind::Error).iter().any(|l| l == line), "{:?}", journal(&app, JournalKind::Error));
}

#[test]
fn the_world_rule_covers_the_corridor_the_course_and_the_region() {
    let mut app = sandbox::test_app(42);
    // The corridor and the obstacle course east of the arena, then a generated region.
    for (x, z) in [(30.0, 0.0), (90.0, 0.0)] {
        called(&mut app, "teleport", json!({"id": "e5", "x": x, "z": z}));
        assert!(!step(&mut app, 2).contains("stopped"), "e5 at ({x}, {z}) is in the world");
    }
    called(&mut app, "regen", json!({"region": "wilds", "seed": 42}));
    assert!(!step(&mut app, 30).contains("stopped"), "the player at the region's gate is in the world");
    assert!(app.world().resource::<AiInvariants>().violations().is_empty());
}

#[test]
fn the_world_rule_leaves_out_the_voids_beside_the_corridor_and_the_lane() {
    // Where bodies belong: the corridor, the lane of the course, the air over the canyon.
    let mut app = sandbox::test_app(42);
    for (x, z, y) in [(30.0, 0.0, 1.0), (60.0, 0.0, 1.0), (110.0, 14.0, 12.0)] {
        called(&mut app, "teleport", json!({"id": "e5", "x": x, "z": z, "y": y}));
        assert!(!step(&mut app, 1).contains("stopped"), "e5 at ({x}, {z}, {y}) is in the world");
    }

    // The bounding box of the arena layout covers the void beside the corridor, and the
    // course's the void beside its lane: neither is part of the world.
    for (x, z, line_x, line_z) in [(30.0, 15.0, "30.0", "15.0"), (60.0, -12.0, "60.0", "-12.0")] {
        let mut app = sandbox::test_app(42);
        called(&mut app, "teleport", json!({"id": "e5", "x": x, "z": z, "y": 1.0}));
        let out = step(&mut app, 10);
        assert_eq!(out, "ran 1 ticks (0 → 1), stopped: invariant bodies_in_world violated at tick 1");
        let line = format!("1 [error] invariant bodies_in_world violated at tick 1: e5 outside the world: x = {line_x}, z = {line_z}");
        assert!(journal(&app, JournalKind::Error).contains(&line), "{:?}", journal(&app, JournalKind::Error));
    }
}

#[test]
fn a_rule_that_panics_counts_as_broken() {
    let mut app = sandbox::test_app(42);
    app.add_ai_invariant("fragile", |_| panic!("the rule has a bug"));
    assert_eq!(step(&mut app, 10), "ran 1 ticks (0 → 1), stopped: invariant fragile violated at tick 1");
    let line = "1 [error] invariant fragile violated at tick 1: the rule itself panicked: the rule has a bug";
    assert!(journal(&app, JournalKind::Error).iter().any(|l| l == line), "{:?}", journal(&app, JournalKind::Error));
    assert_eq!(step(&mut app, 10), "ran 10 ticks (1 → 11)", "off until assert_reset, like any rule");
}

#[test]
fn the_replay_saved_at_a_violation_breaks_the_rule_at_the_same_tick() {
    let mut app = sandbox::test_app(42);
    app.world_mut().resource_mut::<AiInvariants>().replays = true;
    called(&mut app, "assert_add", hurt("fuse"));
    called(&mut app, "attack", json!({"target": BOSS, "repeat": true}));
    called(&mut app, "step", json!({"ticks": 30}));
    boss_health(&mut app, 250.0);
    let file = "replays/invariant-fuse-tick31.json";
    assert_eq!(step(&mut app, 100), format!("ran 1 ticks (30 → 31), stopped: invariant fuse violated at tick 31 (replay: {file})"));
    assert!(journal(&app, JournalKind::Sim).iter().any(|l| l.contains(&format!("invariant replay saved: {file}"))));
    let path = std::env::current_dir().unwrap().join(file);
    let recording = replay::load(&path).unwrap();
    assert_eq!(recording.format, "shards-replay/3");
    assert_eq!(recording.end.tick, 31);
    let ops = serde_json::to_value(&recording.events[0]).unwrap();
    assert_eq!(ops["assert"]["op"]["add"]["name"], "fuse", "the rule travels with the replay: {ops}");

    // A fresh game knows nothing of `fuse`: the replay adds it at tick 0 again.
    let mut again = sandbox::test_app(42);
    replay::play(again.world_mut(), recording).unwrap();
    let out = called(&mut again, "step", json!({"ticks": 100}));
    assert_eq!(text(&out), "ran 31 ticks (0 → 31), stopped: invariant fuse violated at tick 31");
    again.update();
    let report = again.world().resource::<AiReplay>().report().cloned().unwrap();
    assert!(report.identical(), "rules do not change the game: {report}");

    // `--verify` lists it and fails.
    let verify = std::process::Command::new(env!("CARGO_BIN_EXE_sandbox")).arg("--replay").arg(&path).arg("--verify").output().unwrap();
    let _ = std::fs::remove_file(&path);
    let stdout = String::from_utf8_lossy(&verify.stdout);
    assert_eq!(verify.status.code(), Some(1), "{stdout}{}", String::from_utf8_lossy(&verify.stderr));
    assert!(stdout.contains("invariant fuse violated at tick 31: e6 Health.current = 250"), "{stdout}");
    assert!(stdout.contains("identical to the recording (1 checks); 1 invariant violated"), "{stdout}");
}

#[test]
fn a_snapshot_restart_rearms_the_rules_and_keeps_the_session_ones() {
    let mut app = sandbox::test_app(42);
    called(&mut app, "assert_add", hurt("fuse"));
    boss_health(&mut app, 250.0);
    assert!(step(&mut app, 10).ends_with("stopped: invariant fuse violated at tick 1"));
    let file = crate::snapshots::Scratch::new("rules");
    called(&mut app, "snapshot_save", json!({"path": file.1}));
    // Loading the snapshot it was taken in: the session rule is still there, armed again.
    called(&mut app, "snapshot_load", json!({"path": file.1}));
    let list = text(&called(&mut app, "assert_list", json!({}))).to_owned();
    assert!(list.contains("  fuse  when field:e6:Health.current<300  armed"), "{list}");
    assert!(step(&mut app, 10).ends_with("stopped: invariant fuse violated at tick 2"), "armed again");
    // The recording from the snapshot adds it again, so its playback halts there too.
    let recording = replay::save(app.world());
    let ops = serde_json::to_value(&recording.events[0]).unwrap();
    assert_eq!(ops["assert"]["op"]["add"]["name"], "fuse", "{ops}");
}
