//! Co-op for two: the host's hero and a guest's, on a link of 150 ms one way with 2 % loss. The
//! guest's hero is the prefab `content/session.net.ron` names; what it earns (`Trophies`, a
//! character section) is its player's, saved on the guest's machine and brought back at the
//! next join. The yard (`Fallen`, a world section) stays with the host.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use my_game::progress::Trophies;
use shards::ai_core::{AiIndex, AiJournal};
use shards::bevy::ecs::message::Messages;
use shards::net::{LinkConditions, NetDesync, NetState, Stand};
use shards::prelude::*;
use shards::save::{read_file, SaveConfig, SaveKind, SaveSlots};
use shards::serde_json::json;

use crate::SEED;

const HOST: usize = 0;
const GUEST: usize = 1;

fn lossy() -> LinkConditions {
    LinkConditions { latency_ms: 150, loss: 0.02, ..LinkConditions::NONE }
}

fn seeded(config: &mut EngineConfig) {
    config.seed = SEED;
}

fn game(app: &mut App) {
    app.add_plugins(my_game::GamePlugin);
}

/// A save folder for the guest that outlives one stand, removed when the test ends.
struct GuestFolder(PathBuf);

impl GuestFolder {
    fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("my_game_coop_{tag}_{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        Self(dir)
    }
}

impl Drop for GuestFolder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The yard for two: the host (with a save folder of its own) and the guest saving into `guest`,
/// joined at the host's tick 0.
fn two_heroes(guest: &Path) -> Stand {
    let mut stand = shards::testing::stand_host(1, &seeded, &game);
    let guest = guest.to_owned();
    let guest_game = move |app: &mut App| {
        game(app);
        app.world_mut().resource_mut::<SaveConfig>().dir = guest.clone();
    };
    shards::testing::stand_client(&mut stand, "c1", lossy(), &seeded, &guest_game);
    if let Err(e) = stand.settle() {
        panic!("{e}");
    }
    stand
}

/// The guest's body, as an `eN` (the host and the guest name it the same).
fn guest_body(stand: &Stand) -> String {
    match stand.world(GUEST).resource::<NetState>() {
        NetState::Connected { body: Some(b), .. } => format!("e{b}"),
        other => panic!("the guest is not playing: {other:?}"),
    }
}

fn entity(stand: &Stand, member: usize, id: &str) -> Entity {
    let id = AiId::parse(id).unwrap();
    stand.world(member).resource::<AiIndex>().get(id).unwrap_or_else(|| panic!("member {member} has no {id}"))
}

fn trophies(stand: &Stand, member: usize, id: &str) -> u32 {
    stand.world(member).get::<Trophies>(entity(stand, member, id)).unwrap_or_else(|| panic!("{id} has no Trophies")).defeated
}

fn down(stand: &Stand, member: usize, id: &str) -> bool {
    stand.world(member).get::<Dead>(entity(stand, member, id)).is_some()
}

fn journal(stand: &Stand, member: usize) -> Vec<String> {
    stand.world(member).resource::<AiJournal>().after(0).0.map(ToString::to_string).collect()
}

fn has_line(stand: &Stand, member: usize, needle: &str) -> bool {
    journal(stand, member).iter().any(|line| line.contains(needle))
}

fn act(stand: &mut Stand, member: usize, tool: &str, args: shards::serde_json::Value) {
    let out = stand.call(member, tool, args.clone());
    assert!(out.ok, "{tool} {args} on member {member}: {}", out.result);
}

/// Runs the stand until `done`, at most `max` ticks.
fn run_until(stand: &mut Stand, max: u64, what: &str, done: impl Fn(&Stand) -> bool) {
    for _ in 0..max {
        if done(stand) {
            return;
        }
        stand.run(1);
    }
    assert!(done(stand), "{what}: not after {max} stand ticks\nhost: {:#?}", journal(stand, HOST));
}

/// The guest walks up to the grunt and fights it until it falls.
fn guest_defeats_the_grunt(stand: &mut Stand) {
    act(stand, GUEST, "move_to", json!({"target": "e2", "within": 3}));
    act(stand, GUEST, "attack", json!({"target": "e2", "repeat": true}));
    run_until(stand, 2400, "the grunt falls", |s| down(s, HOST, "e2"));
    // The clear yard saves at the end of that tick; the guest's character crosses the link.
    stand.run(120);
}

#[test]
fn the_guest_defeats_the_grunt_and_takes_the_trophy_home() {
    let folder = GuestFolder::new("trophy");
    let mut stand = two_heroes(&folder.0);
    let guest = guest_body(&stand);
    // The arena's characters and walls come first (e1 hero, e2 grunt, walls), the guest's hero after them.
    assert_eq!(guest, "e7", "{:#?}", journal(&stand, HOST));
    assert_eq!((trophies(&stand, HOST, "e1"), trophies(&stand, HOST, &guest)), (0, 0));

    guest_defeats_the_grunt(&mut stand);

    // The host decided: the trophy is the guest's, the host's hero has none.
    assert_eq!((trophies(&stand, HOST, "e1"), trophies(&stand, HOST, &guest)), (0, 1));
    assert!(down(&stand, GUEST, "e2"), "the guest sees the grunt down");
    assert!(has_line(&stand, HOST, "save: world/slot1 saved (yard_cleared"), "{:#?}", journal(&stand, HOST));
    // A stand client plays the character slot of its name.
    let slot = stand.world(GUEST).resource::<SaveSlots>().character.clone();
    assert_eq!(slot, "c1");
    assert!(has_line(&stand, GUEST, "save: character/c1 saved (yard_cleared"), "{:#?}", journal(&stand, GUEST));
    assert!(!has_line(&stand, GUEST, "skipped on a client"), "the client has nothing to autosave: {:#?}", journal(&stand, GUEST));
    // The guest's file has its trophy; the host's world file, the fallen grunt.
    let file = read_file(stand.world_mut(GUEST), SaveKind::Character, &slot).unwrap();
    assert_eq!(file.sections.0["trophies"].data, json!({"defeated": 1}));
    let world = read_file(stand.world_mut(HOST), SaveKind::World, "slot1").unwrap();
    assert_eq!(world.sections.0["fallen"].data, json!({"grunts": ["yard.gate_grunt"]}));
    let own = read_file(stand.world_mut(HOST), SaveKind::Character, "slot1").unwrap();
    assert_eq!(own.sections.0["trophies"].data, json!({"defeated": 0}), "the host's hero earned nothing");

    // Nobody's copy of the yard drifted from the host's on that link.
    assert_eq!(stand.world(GUEST).get_resource::<NetDesync>().map_or(0, |d| d.count), 0);
    assert!(!journal(&stand, GUEST).iter().any(|l| l.contains("desync")), "{:#?}", journal(&stand, GUEST));
}

#[test]
fn a_guest_brings_their_trophies_to_the_next_session() {
    let folder = GuestFolder::new("again");
    let mut first = two_heroes(&folder.0);
    guest_defeats_the_grunt(&mut first);
    assert_eq!(first.shut_down(5000), AppExit::Success);
    drop(first);

    // A new yard, the grunt on its feet again; the guest joins with the file from last time.
    let second = two_heroes(&folder.0);
    let guest = guest_body(&second);
    assert!(!down(&second, HOST, "e2"));
    assert_eq!((trophies(&second, HOST, "e1"), trophies(&second, HOST, &guest)), (0, 1));
    assert_eq!(trophies(&second, GUEST, &guest), 1, "the guest's own copy of its body");
}

#[test]
fn a_guest_saves_and_quits_by_leaving() {
    let folder = GuestFolder::new("quit");
    let mut stand = two_heroes(&folder.0);
    act(&mut stand, GUEST, "ui_open", json!({"screen": "pause"}));
    act(&mut stand, GUEST, "ui_click", json!({"id": "pause/quit"}));
    let out = stand.call(GUEST, "ui_click", json!({"id": "confirm_quit/yes"}));
    let done = out.result.as_str().unwrap_or_default().to_owned();
    assert!(out.ok && done.ends_with("the host saves your hero as you leave; the game ends"), "{done}");
    let exits = stand.world(GUEST).resource::<Messages<AppExit>>();
    assert_eq!(exits.iter_current_update_messages().count(), 1, "the guest's game is asked to end");
    assert!(!folder.0.join("world").exists(), "a guest writes no world");
}

#[test]
fn the_hosts_replay_of_a_coop_fight_plays_the_same_alone() {
    let folder = GuestFolder::new("replay");
    let mut stand = two_heroes(&folder.0);
    guest_defeats_the_grunt(&mut stand);
    let recording = shards::ai_core::replay::save(stand.world(HOST));
    let mut config = shards::testing::config();
    config.seed = recording.seed;
    let mut app = my_game::build_app(&config);
    app.finish();
    app.cleanup();
    let report = shards::testing::verify(&mut app, recording).unwrap();
    assert!(report.identical(), "the host's replay does not play the same in single player: {report}");
}
