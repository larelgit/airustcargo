//! Use by looking (F5) in the sandbox: the test room `use` (`assets/levels/use.gltf`, written by [`room`]), the
//! player with a first-person body (`viewer`) looking at its stations, and a game that answers `Used`.
//! Regenerate the room's file after changing [`room`]:
//!
//! ```text
//! SHARDS_UPDATE_FIXTURES=1 cargo test -p sandbox --test sandbox using::the_committed_room
//! ```
//!
//! `scenarios/use.scenario` plays the room (its reference replay is `tests/replays/use.json`); these tests say what a
//! scenario cannot, because it names the ids of spawned things: `interact target=eN`, a door a game opens when it is
//! used, a hold in a snapshot, and the replay in heavy mode.

use std::path::{Path, PathBuf};
use std::process::Command;

use shards::action::{UseProgress, Used};
use shards::ai_core::{replay, snapshot, AiIndex};
use shards::motion::{mover_go, use_target, Mover, MoverState, Usable};
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::{act, events, get, ok, step, text, PLAYER};
use crate::gltf::{Gltf, NodeSpec};

/// The room, in its own frame (the level `use` places it on the canyon floor): the viewer stands at x 0 on the
/// floor, looks west (yaw 90) and has its eye at y 1.6; each station is 3 m south of the one before it.
///
/// ```text
/// z -13.5  Lever          a bare marker: no mesh, no size, so no use area
/// z -10.5  Panel          1.5 m ahead: used
/// z  -7.5  FarPanel       3.5 m ahead: past the body's 2 m
/// z  -4.5  Console        3.5 m ahead, use_range 4: used
/// z  -1.5  Pane           glass 1 m ahead; BehindGlass 1.55 m ahead behind it
/// z   1.5  Valve          1.2 m ahead, hold_s 1: held
/// z   4.5  NearSwitch     1 m ahead; FarSwitch 1.8 m ahead on the same line
/// z   7.5  TwinA, TwinB   1.5 m ahead, the same box twice: the first in the file
/// z  10.5  Door           1.4 m ahead, solid, a `gate` mover that slides 3 m south
/// ```
pub fn room() -> Gltf {
    let mut g = Gltf::new();
    let panel = g.box_mesh("Panel", [-0.05, -0.2, -0.25], [0.05, 0.2, 0.25]);
    let pane = g.box_mesh("Pane", [-0.02, -1.0, -1.0], [0.02, 1.0, 1.0]);
    let valve = g.box_mesh("Valve", [-0.15, -0.2, -0.2], [0.15, 0.2, 0.2]);
    let slab = g.box_mesh("Slab", [-1.0, -1.25, -0.1], [1.0, 1.25, 0.1]);
    let usable = |name: &str| json!({"marker": "interactable", "name": name});
    g.root(NodeSpec::new("Lever").at([-1.5, 1.6, -13.5]).props(usable("lever")));
    g.root(NodeSpec::new("Panel").at([-1.55, 1.6, -10.5]).mesh(panel).props(usable("panel")));
    g.root(NodeSpec::new("FarPanel").at([-3.55, 1.6, -7.5]).mesh(panel).props(usable("far_panel")));
    g.root(NodeSpec::new("Console").at([-3.55, 1.6, -4.5]).mesh(panel).props(json!({
        "marker": "interactable", "name": "console", "use_range": 4.0,
    })));
    g.root(NodeSpec::new("Pane").at([-1.0, 1.0, -1.5]).mesh(pane).props(json!({"collider": "auto"})));
    g.root(NodeSpec::new("BehindGlass").at([-1.6, 1.6, -1.5]).mesh(panel).props(usable("behind_glass")));
    g.root(NodeSpec::new("Valve").at([-1.35, 1.6, 1.5]).mesh(valve).props(json!({
        "marker": "interactable", "name": "valve", "hold_s": 1.0, "prompt": "Turn the valve",
    })));
    g.root(NodeSpec::new("NearSwitch").at([-1.05, 1.6, 4.5]).mesh(panel).props(usable("near_switch")));
    g.root(NodeSpec::new("FarSwitch").at([-1.85, 1.6, 4.5]).mesh(panel).props(usable("far_switch")));
    g.root(NodeSpec::new("TwinA").at([-1.55, 1.6, 7.5]).mesh(panel).props(usable("twin_a")));
    g.root(NodeSpec::new("TwinB").at([-1.55, 1.6, 7.5]).mesh(panel).props(usable("twin_b")));
    // Turned a quarter, so the gate's key (3 m along its own -x) slides it south along its width.
    g.root(NodeSpec::new("Door").at([-1.5, 1.25, 10.5]).yaw(90.0).mesh(slab).props(json!({
        "marker": "interactable", "name": "door", "collider": "auto", "mover": "gate",
    })));
    g
}

fn room_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/levels/use.gltf")
}

#[test]
fn the_committed_room_is_what_the_builder_writes() {
    let want = room().gltf();
    if std::env::var_os("SHARDS_UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(room_file().parent().unwrap()).unwrap();
        std::fs::write(room_file(), &want).unwrap();
    }
    let have = std::fs::read_to_string(room_file()).unwrap_or_else(|e| panic!("{}: {e}; run with SHARDS_UPDATE_FIXTURES=1", room_file().display()));
    assert!(have == want, "assets/levels/use.gltf is out of date; regenerate it (see the module docs)");
}

/// The arena as the game spawns it, the room laid on the canyon floor at x 100 and the player given the
/// first-person body.
fn in_the_room() -> App {
    let mut app = sandbox::test_app(42);
    for id in ["e6", "e7", "e8", "e14"] {
        ok(&mut app, "npc_set_state", json!({"id": id, "state": "calm"}));
    }
    ok(&mut app, "set", json!({"id": PLAYER, "component": "CharacterController", "path": "params", "value": "viewer"}));
    step(&mut app, 30);
    ok(&mut app, "spawn_level", json!({"id": "use", "x": 100.0, "y": -10.0, "z": 0.0}));
    step(&mut app, 2);
    app
}

/// The player at the room's station `z` (metres south of its middle), looking west.
fn stand_at(app: &mut App, z: f32) {
    ok(app, "teleport", json!({"id": PLAYER, "x": 100.0, "z": z, "y": -10.0}));
    ok(app, "look", json!({"yaw": 90, "pitch": 0}));
    step(app, 3);
}

/// The entity and id of the thing the room calls `name`.
fn thing(app: &mut App, name: &str) -> (Entity, String) {
    let world = app.world_mut();
    let mut q = world.query::<(Entity, &Usable, &AiId)>();
    q.iter(world).find(|(_, u, _)| u.name == name).map(|(e, _, id)| (e, id.to_string())).unwrap_or_else(|| panic!("no thing `{name}` in the room"))
}

fn player_entity(app: &App) -> Entity {
    app.world().resource::<AiIndex>().get(AiId::parse(PLAYER).unwrap()).unwrap()
}

/// The journal lines that say something was used.
fn uses(app: &App) -> Vec<String> {
    events(app).into_iter().filter(|l| l.contains(" uses ")).collect()
}

#[test]
fn the_use_scenario_plays_the_room_and_leaves_nothing_behind() {
    let mut app = sandbox::test_app(42);
    let report = shards::ai_core::scenario::run_file(app.world_mut(), crate::scenario_files::scenario("use.scenario"));
    assert!(report.passed, "{report}");
    let world = app.world_mut();
    assert_eq!(world.query::<&Usable>().iter(world).count(), 0, "no usable thing is left");
    assert_eq!(world.query::<&Mover>().iter(world).count(), 0, "nor a mover");
    assert_eq!(world.query::<&UseProgress>().iter(world).count(), 0, "nor a use under way");
}

#[test]
fn interact_with_a_target_turns_to_it_uses_it_and_says_what_stops_it() {
    let mut app = in_the_room();
    // Facing south, away from everything: the target brings the look round.
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 100.0, "z": -10.5, "y": -10.0}));
    ok(&mut app, "look", json!({"yaw": 180, "pitch": 0}));
    step(&mut app, 3);
    let (panel, panel_id) = thing(&mut app, "panel");
    let out = ok(&mut app, "interact", json!({"target": panel_id}));
    assert!(text(&out).starts_with(&format!("ok: using {panel_id} \"panel\" 1.5m")), "{}", text(&out));
    assert_eq!(get::<Intent>(&app, PLAYER).look.map(|l| l.yaw.to_degrees().round()), Some(90.0), "turned west");
    step(&mut app, 2);
    assert!(uses(&app).iter().any(|l| l.ends_with(&format!("uses {panel_id} \"panel\""))), "{:#?}", events(&app));
    assert!(app.world().get::<Usable>(panel).is_some());

    // A thing past the body's reach; one that the marker's own reach brings back.
    stand_at(&mut app, -7.5);
    let (_, far_id) = thing(&mut app, "far_panel");
    let err = act(&mut app, "interact", json!({"target": far_id}));
    assert!(!err.ok && text(&err).contains("\"far_panel\" is 3.5m away and can be used from 2.0m: walk closer"), "{}", text(&err));
    stand_at(&mut app, -4.5);
    let (_, console_id) = thing(&mut app, "console");
    let out = ok(&mut app, "interact", json!({"target": console_id}));
    assert!(text(&out).contains("\"console\" 3.5m"), "{}", text(&out));

    // Behind glass: the pane is what the refusal names.
    stand_at(&mut app, -1.5);
    let (_, behind_id) = thing(&mut app, "behind_glass");
    let err = act(&mut app, "interact", json!({"target": behind_id}));
    assert!(!err.ok && text(&err).contains("\"behind_glass\" is behind wall 1.0m ahead: a wall or glass blocks a use"), "{}", text(&err));

    // Two on one line: the nearer. Two at one distance: the lower collider, and the other says so.
    stand_at(&mut app, 4.5);
    let (_, far_switch) = thing(&mut app, "far_switch");
    let err = act(&mut app, "interact", json!({"target": far_switch}));
    assert!(!err.ok && text(&err).contains("\"near_switch\" is nearer on the same line (1.0m)"), "{}", text(&err));
    stand_at(&mut app, 7.5);
    let (_, twin_b) = thing(&mut app, "twin_b");
    let err = act(&mut app, "interact", json!({"target": twin_b}));
    assert!(!err.ok && text(&err).contains("\"twin_a\" is just as near on the same line (1.5m) and comes first"), "{}", text(&err));
    assert_eq!(get::<Intent>(&app, PLAYER).look.map(|l| l.yaw.to_degrees().round()), Some(90.0), "an error leaves the look as it was");

    // The bare marker has no use area at all.
    let (_, lever) = thing(&mut app, "lever");
    let err = act(&mut app, "interact", json!({"target": lever}));
    assert!(!err.ok && text(&err).contains("\"lever\" has no use area, so no look can meet it"), "{}", text(&err));

    // A hold has to be held.
    stand_at(&mut app, 1.5);
    let (_, valve_id) = thing(&mut app, "valve");
    let err = act(&mut app, "interact", json!({"target": valve_id}));
    assert!(!err.ok && text(&err).contains("takes 1.0s to use: hold the button"), "{}", text(&err));
    let out = ok(&mut app, "interact", json!({"target": valve_id, "hold": true}));
    assert!(text(&out).contains("ok: holding") && text(&out).contains("for 1.0s"), "{}", text(&out));
    step(&mut app, 80);
    assert!(uses(&app).iter().any(|l| l.ends_with("\"valve\"")), "{:#?}", events(&app));
    ok(&mut app, "interact", json!({"hold": false}));
}

/// A game answers `Used` with what it owns: the door named `door` goes to its key 1. The engine only says that it
/// was used; the sliding is the `gate` mover's.
fn open_doors(used: On<Used>, mut commands: Commands) {
    if used.name == "door" {
        let door = used.what;
        commands.queue(move |world: &mut World| {
            let _ = mover_go(world, door, 1);
        });
    }
}

#[test]
fn a_game_opens_the_door_when_it_is_used_and_the_use_area_goes_with_it() {
    let mut app = in_the_room();
    app.world_mut().add_observer(open_doors);
    let (door, door_id) = thing(&mut app, "door");
    stand_at(&mut app, 10.5);
    let me = player_entity(&app);
    let target = use_target(app.world(), me);
    assert!(target.is_some_and(|t| t.entity == door && (t.distance - 1.4).abs() < 0.01), "a door is a wall and a target");
    let out = ok(&mut app, "interact", json!({"target": door_id}));
    assert!(text(&out).starts_with("ok: using"), "{}", text(&out));
    ok(&mut app, "step", json!({"ticks": 400, "until": [{"named": "mover_arrived"}]}));
    let state = app.world().get::<MoverState>(door).expect("the door moved");
    assert_eq!((state.from, state.to), (1, 1), "it came to rest at its key 1");
    let z = app.world().get::<Transform>(door).unwrap().translation.z;
    assert!((z - 13.5).abs() < 0.02, "slid 3 m south: {z}");
    // Where it was there is nothing to use now; where it went, the use area went with it.
    assert!(use_target(app.world(), me).is_none(), "the gap is open");
    stand_at(&mut app, 13.5);
    assert!(use_target(app.world(), me).is_some_and(|t| t.entity == door), "the door's use area slid with it");
}

#[test]
fn a_hold_under_way_is_in_the_snapshot_and_completes_after_the_load() {
    let mut a = in_the_room();
    stand_at(&mut a, 1.5);
    ok(&mut a, "interact", json!({"hold": true}));
    step(&mut a, 25);
    let held = a.world().get::<UseProgress>(player_entity(&a)).cloned();
    assert!(held.as_ref().is_some_and(|h| h.elapsed > 0.3 && h.elapsed < 0.5), "{held:?}");
    let saved = snapshot::capture(a.world()).unwrap();
    let mut b = sandbox::test_app(42);
    let report = snapshot::restore(b.world_mut(), &saved, false).unwrap_or_else(|e| panic!("{e}"));
    assert!(report.exact(), "restored differently: {:?}", report.differences);
    assert_eq!(b.world().get::<UseProgress>(player_entity(&b)).cloned(), held, "the hold came back");
    for app in [&mut a, &mut b] {
        step(app, 60);
        assert!(uses(app).iter().any(|l| l.ends_with("\"valve\"")), "{:#?}", events(app));
    }
    assert_eq!(replay::state_hash(a.world()), replay::state_hash(b.world()), "the same state after");
}

#[test]
fn the_use_replay_continues_alike_from_its_snapshots_through_holds() {
    let file = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/use.json");
    let out = Command::new(env!("CARGO_BIN_EXE_sandbox"))
        .arg("--replay")
        .arg(file)
        .args(["--verify", "--heavy", "10"])
        .env("BEVY_ASSET_ROOT", env!("CARGO_MANIFEST_DIR"))
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "{stdout}{stderr}");
    let verdict = stdout.lines().find(|l| l.starts_with("replay ")).unwrap_or_default();
    assert!(verdict.contains("identical to the recording"), "{stdout}");
    assert!(verdict.contains(", 0 skipped"), "{verdict}");
    let checked: u64 = verdict.split("heavy: ").nth(1).and_then(|r| r.split(' ').next()).and_then(|n| n.parse().ok()).unwrap_or(0);
    assert!(checked >= 40, "a twin every 10 ticks, some of them in the middle of a hold: {verdict}");
}

#[test]
fn a_hud_asks_what_the_look_would_use_and_how_far_a_hold_is() {
    use shards::ui::{read_source, UiValue};
    let mut app = in_the_room();
    let read = |app: &App, source: &str| read_source(app.world(), source).expect("the engine gives the source");
    // Nothing in reach: no value. A panel with no prompt of its own shows its name.
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 100.0, "z": -7.5, "y": -10.0}));
    ok(&mut app, "look", json!({"yaw": 90, "pitch": 0}));
    step(&mut app, 3);
    assert_eq!((read(&app, "you.use_target"), read(&app, "you.use_progress")), (None, None), "3.5 m is past the 2 m reach");
    stand_at(&mut app, -10.5);
    assert_eq!(read(&app, "you.use_target"), Some(UiValue::Text("panel".into())));
    assert_eq!(read(&app, "you.use_progress"), None);
    // The valve's own prompt, and a bar while it is held.
    stand_at(&mut app, 1.5);
    assert_eq!(read(&app, "you.use_target"), Some(UiValue::Text("Turn the valve".into())));
    ok(&mut app, "interact", json!({"hold": true}));
    step(&mut app, 31);
    let Some(UiValue::Ratio { value, max }) = read(&app, "you.use_progress") else { panic!("a bar: {:?}", read(&app, "you.use_progress")) };
    assert!(max == 1.0 && value > 0.3 && value < 0.6, "{value}/{max}");
    ok(&mut app, "interact", json!({"hold": false}));
    step(&mut app, 2);
    assert_eq!(read(&app, "you.use_progress"), None, "let go: no bar");
}
