//! Scenes that keep the player's camera, and NPC sequences (F12), end to end on the sandbox: the test room
//! `scene` (`assets/levels/scene.gltf`, written by [`room`]; its guide, sequence and line in
//! `content/scene/`). The player is held (movement and actions locked, the look kept in a cone), the guide
//! walks to a marker beside them, turns to face them, nods, says a line with a subtitle and signals, and the
//! lock lifts. Regenerate the room's file after changing [`room`]:
//!
//! ```text
//! SHARDS_UPDATE_FIXTURES=1 cargo test -p sandbox --test sandbox sequence::the_committed_room
//! ```
//!
//! `scenarios/sequence.scenario` plays the scene (its reference replay is `tests/replays/sequence.json`).
//! What a scenario cannot say, because it names the ids of spawned bodies or counts ticks, is here: a game
//! that unlocks the player when the scene ends, the locks reaching the action machine and the controller, and
//! a game loaded at every step of the scene.

use std::path::PathBuf;

use shards::ai_core::snapshot::{self, differences};
use shards::ai_core::{sim, AiIndex};
use shards::core::Intent;
use shards::motion::{unlock_controls, ControlLocks};
use shards::npc::{NpcBrain, SequenceEnded, SequenceHold, Sequences};
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::{calm_skeletons, calm_warden, events, fails, lines, ok, step, text, PLAYER};
use crate::gltf::{Gltf, NodeSpec};

/// Where the room's origin is laid: the canyon floor, in the strip north of the goal's rock (x 118..140, z -16..-6), east
/// of the movers' room.
const ORIGIN: (f32, f32, f32) = (122.0, -10.0, -11.0);

/// The room, in its own frame: a marker 3 m east of the origin for the guide to walk to, and the guide itself 14 m
/// east of it (a node that places a prefab). No floor: the canyon has one.
pub fn room() -> Gltf {
    let mut g = Gltf::new();
    g.root(NodeSpec::new("SceneStand").at([3.0, 0.0, 0.0]).props(json!({"marker": "custom:scene", "name": "scene_stand"})));
    g.root(NodeSpec::new("Guide").at([14.0, 0.0, 0.0]).props(json!({"prefab": "scene_guide"})));
    g
}

fn room_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/levels/scene.gltf")
}

#[test]
fn the_committed_room_is_what_the_builder_writes() {
    let want = room().gltf();
    if std::env::var_os("SHARDS_UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(room_file().parent().unwrap()).unwrap();
        std::fs::write(room_file(), &want).unwrap();
    }
    let have = std::fs::read_to_string(room_file()).unwrap_or_else(|e| panic!("{}: {e}; run with SHARDS_UPDATE_FIXTURES=1", room_file().display()));
    assert!(have == want, "assets/levels/scene.gltf is out of date; regenerate it (see the module docs)");
}

/// The sandbox with the room laid and the player in it, looking north; nobody else in the way. The guide's brain has
/// not run yet: nothing has stepped.
fn scene() -> App {
    let mut app = sandbox::test_app(42);
    calm_skeletons(&mut app);
    calm_warden(&mut app);
    let (x, y, z) = ORIGIN;
    let placed = ok(&mut app, "spawn_level", json!({"id": "scene", "x": x, "y": y, "z": z}));
    assert!(text(&placed).contains("spawned level `scene`"), "{}", text(&placed));
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": x, "z": z, "y": y + 0.3}));
    ok(&mut app, "look", json!({"yaw": 0}));
    app
}

/// The guide's id: the one body named `Guide`.
fn guide(app: &mut App) -> AiId {
    let world = app.world_mut();
    let mut q = world.query::<(&AiId, &Name)>();
    let found: Vec<AiId> = q.iter(world).filter(|(_, n)| n.as_str() == "Guide").map(|(id, _)| *id).collect();
    assert_eq!(found.len(), 1, "one guide: {found:?}");
    found[0]
}

fn entity(app: &App, id: AiId) -> Entity {
    app.world().resource::<AiIndex>().get(id).unwrap()
}

fn player_entity(app: &App) -> Entity {
    entity(app, AiId::parse(PLAYER).unwrap())
}

/// The player looks only: movement and actions locked, the look within 35 degrees of the marker.
fn hold_the_player(app: &mut App) {
    let (x, y, z) = ORIGIN;
    let at = format!("{},{},{}", x + 3.0, y + 1.5, z);
    let said = ok(app, "lock_controls", json!({"reason": "scene", "movement": true, "actions": true, "at": at, "deg": 35}));
    assert!(text(&said).contains("controls locked (scene): movement, actions, look within 35°"), "{}", text(&said));
}

fn play(app: &mut App) {
    let said = ok(app, "sequence_play", json!({"id": "scene_intro", "role.guide": "Guide", "role.visitor": PLAYER}));
    assert!(text(&said).contains("(scene_intro) plays, 6 steps"), "{}", text(&said));
}

fn until(app: &mut App, ticks: u64, journal: &str) {
    let said = ok(app, "step", json!({"ticks": ticks, "until": [{"journal": journal}]}));
    assert!(text(&said).contains("stopped"), "`{journal}` did not come in {ticks} ticks: {}", text(&said));
}

#[test]
fn the_scene_scenario_holds_the_player_plays_the_scene_and_leaves_nothing_behind() {
    let mut app = sandbox::test_app(42);
    let report = shards::ai_core::scenario::run_file(app.world_mut(), crate::scenario_files::scenario("sequence.scenario"));
    assert!(report.passed, "{report}");
    let world = app.world_mut();
    assert!(world.get_resource::<Sequences>().is_none_or(|s| s.runs.is_empty()), "no run is left");
    assert_eq!(world.query::<&SequenceHold>().iter(world).count(), 0, "nor a hold");
    assert_eq!(world.query::<&ControlLocks>().iter(world).count(), 0, "nor a lock");
    let mut named = world.query::<&Name>();
    assert!(named.iter(world).all(|n| n.as_str() != "Guide"), "the guide went with the room");
}

#[test]
fn a_game_that_unlocks_on_the_end_of_the_scene_never_leaves_the_player_locked() {
    let mut app = scene();
    // The pattern: whoever locks the player lets go on `SequenceEnded`, finished or not.
    app.world_mut().add_observer(|_: On<SequenceEnded>, mut commands: Commands, player: Query<Entity, With<sandbox::gameplay::Player>>| {
        for entity in &player {
            commands.queue(move |world: &mut World| {
                unlock_controls(world, entity, "scene");
            });
        }
    });
    hold_the_player(&mut app);
    play(&mut app);
    let me = player_entity(&app);
    step(&mut app, 30);
    assert!(app.world().get::<ControlLocks>(me).is_some(), "locked while the scene plays");
    until(&mut app, 1200, "(scene_intro) ended");
    assert!(app.world().get::<ControlLocks>(me).is_none(), "and free when it ends");
    assert!(lines(&app, "controls unlocked (scene)").len() == 1, "{:#?}", events(&app));

    // A scene stopped half way: the game unlocks as well, and the guide is free.
    let mut app = scene();
    app.world_mut().add_observer(|_: On<SequenceEnded>, mut commands: Commands, player: Query<Entity, With<sandbox::gameplay::Player>>| {
        for entity in &player {
            commands.queue(move |world: &mut World| {
                unlock_controls(world, entity, "scene");
            });
        }
    });
    hold_the_player(&mut app);
    play(&mut app);
    step(&mut app, 20);
    ok(&mut app, "sequence_stop", json!({"all": true}));
    let me = player_entity(&app);
    assert!(app.world().get::<ControlLocks>(me).is_none());
    let guide = guide(&mut app);
    assert!(app.world().get::<SequenceHold>(entity(&app, guide)).is_none());
    step(&mut app, 30);
    assert!(lines(&app, "idle → follow").len() == 1, "the guide's brain takes up: {:#?}", events(&app));
}

#[test]
fn the_locks_reach_the_action_machine_and_the_controller_and_the_keys_come_back() {
    let mut app = sandbox::test_app(42);
    calm_skeletons(&mut app);
    calm_warden(&mut app);
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 0, "z": 14, "y": 0}));
    step(&mut app, 3);
    let me = player_entity(&app);
    let press = |app: &mut App, input: &str| {
        let me = player_entity(app);
        app.world_mut().get_mut::<Intent>(me).unwrap().press(input);
    };
    let ran = |app: &App| app.world().get::<ActionState>(player_entity(app)).unwrap().started;
    let before = ran(&app);

    // Actions: a press does nothing, the tool says why; movement is free.
    ok(&mut app, "lock_controls", json!({"reason": "a", "actions": true}));
    press(&mut app, "light");
    step(&mut app, 3);
    assert_eq!(ran(&app), before, "no action started");
    assert!(fails(&mut app, "light_attack", json!({})).contains("your actions are locked (a)"));
    ok(&mut app, "move", json!({"dir": "W", "meters": 2}));
    step(&mut app, 60);
    assert!(app.world().get::<Transform>(me).unwrap().translation.x < -1.0, "it walks");

    // Movement: the same Intent that walks (a key held; the keyboard writes it once) goes nowhere, and walks again at once when the lock lifts.
    unlock_controls(app.world_mut(), me, "a");
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 0, "z": 14, "y": 0}));
    app.world_mut().get_mut::<Intent>(me).unwrap().move_dir = Vec3::X;
    ok(&mut app, "lock_controls", json!({"reason": "b", "movement": true}));
    step(&mut app, 30);
    assert!(app.world().get::<Transform>(me).unwrap().translation.x.abs() < 0.1, "held still: {}", app.world().get::<Transform>(me).unwrap().translation);
    assert_eq!(app.world().get::<Intent>(me).unwrap().move_dir, Vec3::X, "the key is still down in the intent");
    press(&mut app, "light");
    step(&mut app, 3);
    assert!(ran(&app) > before, "actions are free under a movement lock");
    unlock_controls(app.world_mut(), me, "b");
    step(&mut app, 90);
    assert!(app.world().get::<Transform>(me).unwrap().translation.x > 2.0, "walking again, no key pressed anew: {}", app.world().get::<Transform>(me).unwrap().translation);
}

/// The state that counts, to compare a copy loaded from a snapshot with the original.
fn story_after(app: &App, tick: u64) -> Vec<String> {
    use shards::ai_core::{AiJournal, JournalKind};
    app.world()
        .resource::<AiJournal>()
        .after(0)
        .0
        .filter(|e| e.tick > tick && matches!(e.kind, JournalKind::Event | JournalKind::Warn | JournalKind::Error))
        .map(ToString::to_string)
        .collect()
}

#[test]
fn a_game_loaded_in_the_middle_of_the_scene_plays_it_to_the_same_end() {
    let mut original = scene();
    hold_the_player(&mut original);
    play(&mut original);
    // A snapshot whenever a step begins that is seen between ticks (the `Face` is done at once: the guide already
    // faces the player when it arrives, and the sound and the signal take no time), and part-way through every step.
    let mut saved: Vec<(u64, u32, shards::ai_core::Snapshot)> = Vec::new();
    let mut last = u32::MAX;
    for _ in 0..1500 {
        let at = original.world().get_resource::<Sequences>().and_then(|s| s.runs.first()).map(|r| (r.step, r.age));
        match at {
            Some((step_now, age)) if step_now != last || age % 50 == 25 => {
                last = step_now;
                saved.push((sim::tick(original.world()), step_now, snapshot::capture(original.world()).unwrap()));
            }
            None if last != u32::MAX => break,
            _ => {}
        }
        step(&mut original, 1);
    }
    assert!(saved.len() >= 8, "{:?}", saved.iter().map(|s| (s.0, s.1)).collect::<Vec<_>>());
    // The scene ends, and the lock is the game's to lift (nothing lifts it here): a few ticks on, both are still locked.
    step(&mut original, 20);
    let end = snapshot::capture(original.world()).unwrap();
    assert!(end.left_out.is_empty(), "{:?}", end.left_out);
    let finished = sim::tick(original.world());
    assert!(lines(&original, "(scene_intro) ended").len() == 1, "{:#?}", events(&original));

    for (at, step_at, snapshot) in saved {
        let mut copy = sandbox::test_app(42);
        let report = snapshot::restore(copy.world_mut(), &snapshot, false).unwrap_or_else(|e| panic!("tick {at} (step {step_at}): {e}"));
        assert!(report.exact(), "tick {at} (step {step_at}): {:?}", report.differences);
        assert_eq!(copy.world().resource::<Sequences>().runs.len(), 1, "tick {at}: the run came back");
        step(&mut copy, finished - at);
        let diff = differences(&end, &snapshot::capture(copy.world()).unwrap());
        assert!(diff.is_empty(), "loaded at tick {at} (step {step_at}):\n{}", diff.join("\n"));
        assert_eq!(story_after(&copy, at), story_after(&original, at), "journal after tick {at}");
        let me = player_entity(&copy);
        assert!(copy.world().get::<ControlLocks>(me).is_some(), "tick {at}: the lock came back");
        let brain_state = |app: &App, id| app.world().get::<NpcBrain>(entity(app, id)).unwrap().state.clone();
        let mut copy_app = copy;
        let id = guide(&mut copy_app);
        assert_eq!(brain_state(&copy_app, id), brain_state(&original, id), "tick {at}: the guide's brain is where it was");
    }
}

#[test]
fn the_scene_is_checked_with_the_content_a_wrong_sound_is_an_error_and_the_good_one_passes() {
    use crate::content::{app_on, assets_copy};
    let errors = |dir: &std::path::Path| -> Vec<String> {
        let mut app = app_on(dir);
        let out = crate::common::act(&mut app, "content_check", json!({}));
        let report = out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned);
        report.lines().map(str::to_owned).collect()
    };
    let clean = errors(&assets_copy("sequence_clean"));
    assert!(clean.iter().all(|l| l.starts_with("ok:")), "{clean:#?}");
    // A sound nobody defines, and one that loops (an emitter's).
    let dir = assets_copy("sequence_sounds");
    let file = dir.join("content/scene/scene.sequence.ron");
    let text_of = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, text_of.replace("sound: \"scene_line\"", "sound: \"scene_lin\"")).unwrap();
    let wrong = errors(&dir);
    assert!(wrong.iter().any(|l| l.contains("steps[3].sound") && l.contains("scene_lin")), "{wrong:#?}");
    std::fs::write(&file, text_of.replace("sound: \"scene_line\"", "sound: \"room_hum\"")).unwrap();
    let looping = errors(&dir);
    assert!(looping.iter().any(|l| l.contains("steps[3].sound") && l.contains("loops")), "{looping:#?}");
}


/// The reference replay in heavy mode: a twin loaded from the snapshot of every 10th tick plays on beside the
/// original and must keep its state, the scene's runs, holds and locks included.
#[test]
fn the_sequence_replay_continues_alike_from_its_snapshots() {
    let file = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/replays/sequence.json");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_sandbox"))
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
    assert!(checked >= 50, "a twin every 10 ticks: {verdict}");
}

/// The use room (F5, `tests/sandbox/using.rs`) with the player at its station `z`, looking west with a first-person body.
fn at_a_station(z: f32) -> App {
    let mut app = sandbox::test_app(42);
    calm_skeletons(&mut app);
    calm_warden(&mut app);
    ok(&mut app, "set", json!({"id": PLAYER, "component": "CharacterController", "path": "params", "value": "viewer"}));
    step(&mut app, 30);
    ok(&mut app, "spawn_level", json!({"id": "use", "x": 100.0, "y": -10.0, "z": 0.0}));
    step(&mut app, 2);
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 100.0, "z": z, "y": -10.0}));
    ok(&mut app, "look", json!({"yaw": 90, "pitch": 0}));
    step(&mut app, 3);
    app
}

fn uses(app: &App) -> usize {
    events(app).iter().filter(|l| l.contains(" uses ")).count()
}

#[test]
fn a_lock_on_actions_blocks_a_use_and_a_hold_and_a_lock_on_movement_does_not() {
    use shards::action::UseProgress;
    // A press: the panel 1.5 m ahead is used with the button, not with the actions locked.
    let mut app = at_a_station(-10.5);
    let me = player_entity(&app);
    ok(&mut app, "lock_controls", json!({"reason": "a", "actions": true}));
    app.world_mut().get_mut::<Intent>(me).unwrap().press("interact");
    step(&mut app, 3);
    assert_eq!(uses(&app), 0, "no use under an actions lock: {:#?}", events(&app));
    assert!(fails(&mut app, "interact", json!({})).contains("your actions are locked (a)"));
    ok(&mut app, "unlock_controls", json!({"reason": "a"}));
    // A movement lock leaves it be: using is not moving.
    ok(&mut app, "lock_controls", json!({"reason": "m", "movement": true}));
    app.world_mut().get_mut::<Intent>(me).unwrap().press("interact");
    step(&mut app, 3);
    assert_eq!(uses(&app), 1, "a movement lock does not stop a use: {:#?}", events(&app));
    ok(&mut app, "unlock_controls", json!({"reason": "m"}));

    // A hold under way (the valve, 1 s): the lock takes the button, so the hold is cancelled at once and never finishes.
    let mut app = at_a_station(1.5);
    let me = player_entity(&app);
    ok(&mut app, "interact", json!({"hold": true}));
    step(&mut app, 20);
    assert!(app.world().get::<UseProgress>(me).is_some_and(|p| p.elapsed > 0.2), "the hold is under way");
    ok(&mut app, "lock_controls", json!({"reason": "a", "actions": true}));
    step(&mut app, 120);
    assert!(app.world().get::<UseProgress>(me).is_none(), "the lock let go of the button for the body: the hold is cancelled");
    assert_eq!(uses(&app), 0, "and the valve was never turned: {:#?}", events(&app));
    // The key is still down in the intent: it comes back with the lock, and a hold that began before does not begin again by itself.
    ok(&mut app, "unlock_controls", json!({"reason": "a"}));
    assert!(app.world().get::<Intent>(me).unwrap().is_held("interact"));
    step(&mut app, 120);
    assert_eq!(uses(&app), 0, "a held button alone starts nothing: only a press does");
}
