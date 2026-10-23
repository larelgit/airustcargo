//! Movers (F6) in the sandbox: the test room `movers` (`assets/levels/movers.gltf`, written by
//! [`room`]; its movers in `content/world/movers.mover.ron`), and a client riding a host's lift.
//! Regenerate the room's file after changing [`room`]:
//!
//! ```text
//! SHARDS_UPDATE_FIXTURES=1 cargo test -p sandbox --test sandbox movers::the_committed_room
//! ```
//!
//! `scenarios/movers.scenario` plays the room (its reference replay is `tests/replays/movers.json`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use shards::ai_core::{sim, AiId};
use shards::motion::{Mover, MoverState};
use shards::net::NetCorrection;
use shards::prelude::*;
use shards::serde_json::json;

use crate::gltf::{Gltf, NodeSpec};
use crate::net::{body, desyncs, dev_guests, has_line, journal, lossy, position, stand_with, verify_host, C1, HOST};

/// The room, in its own frame (the level `movers` places it on the canyon floor): a floor 10 x
/// 30 m; a lift (`Lift`, a mover with no shape of its own: its platform and rail move with it) that
/// goes 4 m up to a deck on its west side; a gate 2 m wide that slides 3 m west to close; a crusher
/// whose bottom comes down from 3 m to 0.4 m; a turntable 3 m across.
pub fn room() -> Gltf {
    let mut g = Gltf::new();
    let floor = g.box_mesh("Floor", [-5.0, -0.5, -15.0], [5.0, 0.0, 15.0]);
    let platform = g.box_mesh("Platform", [-1.0, 0.0, -1.0], [1.0, 0.2, 1.0]);
    let rail = g.box_mesh("Rail", [-0.05, 0.0, -0.05], [0.05, 1.1, 0.05]);
    let deck = g.box_mesh("Deck", [-2.0, -0.2, -1.0], [2.0, 0.0, 1.0]);
    let gate = g.box_mesh("Gate", [-1.0, 0.0, -0.15], [1.0, 2.5, 0.15]);
    let crusher = g.box_mesh("Crusher", [-1.0, 0.0, -1.0], [1.0, 1.0, 1.0]);
    let turntable = g.box_mesh("Turntable", [-1.5, 0.0, -1.5], [1.5, 0.2, 1.5]);
    let solid = json!({"collider": "auto"});
    g.root(NodeSpec::new("Floor").mesh(floor).props(solid.clone()));
    let lift = g.root(NodeSpec::new("Lift").at([0.0, 0.0, -10.0]).props(json!({"mover": "lift"})));
    g.child(lift, NodeSpec::new("Platform").mesh(platform).props(solid.clone()));
    g.child(lift, NodeSpec::new("Rail").at([0.9, 0.2, 0.9]).mesh(rail).props(solid.clone()));
    g.root(NodeSpec::new("Deck").at([-3.0, 4.2, -10.0]).mesh(deck).props(solid.clone()));
    g.root(NodeSpec::new("Gate").at([3.0, 0.0, 0.0]).mesh(gate).props(json!({"mover": "gate", "collider": "auto"})));
    g.root(NodeSpec::new("Crusher").at([2.5, 3.0, 8.0]).mesh(crusher).props(json!({"mover": "crusher", "collider": "auto"})));
    g.root(NodeSpec::new("Turntable").at([2.5, 0.0, -4.0]).mesh(turntable).props(json!({"mover": "turntable", "collider": "cylinder"})));
    g
}

fn room_file() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/levels/movers.gltf")
}

#[test]
fn the_committed_room_is_what_the_builder_writes() {
    let want = room().gltf();
    if std::env::var_os("SHARDS_UPDATE_FIXTURES").is_some() {
        std::fs::create_dir_all(room_file().parent().unwrap()).unwrap();
        std::fs::write(room_file(), &want).unwrap();
    }
    let have = std::fs::read_to_string(room_file()).unwrap_or_else(|e| panic!("{}: {e}; run with SHARDS_UPDATE_FIXTURES=1", room_file().display()));
    assert!(have == want, "assets/levels/movers.gltf is out of date; regenerate it (see the module docs)");
}

/// Where the host had `id` after each of its ticks, and where c1 showed its own body after each
/// of its ticks: the prediction error is the distance at the same tick.
#[derive(Default)]
struct Track {
    host: BTreeMap<u64, Vec3>,
    shown: BTreeMap<u64, Vec3>,
}

impl Track {
    fn run(&mut self, stand: &mut shards::net::Stand, id: &str, ticks: u64) {
        for _ in 0..ticks {
            stand.run(1);
            self.host.insert(sim::tick(stand.world(HOST)), position(stand, HOST, id));
            for s in stand.world(C1).resource::<NetCorrection>().history() {
                self.shown.insert(s.tick, s.feet);
            }
        }
    }

    /// Mean and largest error over the ticks both have.
    fn error(&self) -> (f32, f32, usize) {
        let errors: Vec<f32> = self.shown.iter().filter_map(|(t, here)| self.host.get(t).map(|there| here.distance(*there))).collect();
        let max = errors.iter().copied().fold(0.0, f32::max);
        (errors.iter().sum::<f32>() / errors.len().max(1) as f32, max, errors.len())
    }
}

/// A client rides the room's lift (150 ms one way, 2 % loss), sent up by a call from the client
/// that runs on the host: once the lift's start reaches it, its prediction runs the lift's
/// replicated state on to the tick it predicts and carries its body on it, so the ride is
/// predicted as the host has it, without a snap or a desync; the host's recording plays the same
/// in single player.
#[test]
fn a_client_rides_a_hosts_lift_as_the_host_has_it() {
    let mut stand = stand_with(1, lossy());
    let player = body(&stand, C1);
    for id in ["e6", "e7", "e8", "e14"] {
        assert!(stand.call(HOST, "npc_set_state", json!({"id": id, "state": "calm"})).ok);
    }
    stand.run(10);
    let placed = stand.call(HOST, "spawn_level", json!({"id": "movers", "x": 112.0, "y": -10.0, "z": 0.0}));
    assert!(placed.ok, "{}", placed.result);
    let lift = {
        let world = stand.world_mut(HOST);
        let mut q = world.query::<(&AiId, &Name, &Mover)>();
        q.iter(world).find(|(_, n, _)| n.as_str() == "Lift").map(|(id, _, _)| id.to_string()).expect("the room's lift")
    };
    assert!(stand.call(HOST, "teleport", json!({"id": player, "x": 112.0, "z": -10.0, "y": -9.7})).ok);
    stand.run(120);
    let snaps = stand.world(C1).resource::<NetCorrection>().snaps;
    // Sent from the client (a guest with dev calls): the call runs on the host.
    stand.world_mut(HOST).insert_resource(dev_guests());
    let sent = stand.call(C1, "mover_go", json!({"name": "Lift", "key": 1}));
    assert!(sent.ok, "{}", sent.result);
    let mut track = Track::default();
    track.run(&mut stand, &player, 300);
    let (mean, max, n) = track.error();
    println!("lift ride: prediction error mean {mean:.4} m, max {max:.4} m over {n} ticks");
    assert!(n > 200, "{n}");
    assert!(has_line(&stand, C1, "host: mover_go ok at host tick"), "{:#?}", journal(&stand, C1));
    for member in [HOST, C1] {
        let world = stand.world(member);
        let state = world.resource::<shards::ai_core::AiIndex>().get(AiId::parse(&lift).unwrap()).and_then(|e| world.get::<MoverState>(e));
        assert!(state.is_some_and(|s| s.from == 1 && s.to == 1), "member {member}: the lift is up: {state:?}");
        let at = position(&stand, member, &player);
        assert!((at.y + 5.8).abs() < 0.05 && (at.x - 112.0).abs() < 0.05, "member {member}: c1's body rode up: {at}");
    }
    // Until the lift's start reaches the client, it predicts its body standing still (the call is
    // the host's, at a tick the client does not know): the error grows for about a round trip,
    // then the prediction has the lift and the correction takes the body back smoothly.
    assert!(mean < 0.05, "mean error {mean} (max {max})");
    assert!(max < 0.6, "max error {max}");
    assert_eq!(stand.world(C1).resource::<NetCorrection>().snaps, snaps, "no snap: {:#?}", journal(&stand, C1));
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    verify_host(&stand);
}

/// The room's scenario on a fresh game, and nothing of the room is left after it (the scenarios
/// run one after another in one game).
#[test]
fn the_movers_scenario_rides_pushes_crushes_and_leaves_nothing_behind() {
    let mut app = sandbox::test_app(42);
    let report = shards::ai_core::scenario::run_file(app.world_mut(), crate::scenario_files::scenario("movers.scenario"));
    assert!(report.passed, "{report}");
    let world = app.world_mut();
    assert_eq!(world.query::<&Mover>().iter(world).count(), 0, "no mover is left");
    assert_eq!(world.query::<&shards::motion::MoverPart>().iter(world).count(), 0, "nor a part");
}
