//! Client prediction (C4): a client moves its own body and starts its actions at once; the
//! host's state confirms them or corrects the body. Stands at 150 ms one way, 2 % loss.

use std::collections::{BTreeMap, BTreeSet};

use shards::action::{ActionStarted, ActionState, PredictedAction};
use shards::ai_core::net_rules::NetRules;
use shards::ai_core::state::{StateEncoder, StateRule};
use shards::ai_core::{sim, SimTick};
use shards::core::{predicts, NetPrediction, NetSet, Predicted};
use shards::net::{NetClient, NetCorrection, NetOverlay, Stand};
use shards::prelude::*;
use shards::serde_json::{json, Value};

use crate::common::*;
use crate::net::{body, dev_guests, desyncs, entity, has_line, journal, lossy, position, run_until, stand_with, verify_host, C1, C2, HOST};

fn own(stand: &Stand, member: usize) -> Entity {
    entity(stand, member, &body(stand, member)).expect("the own body")
}

/// The NPCs stand still: a test moves only what it means to.
fn calm(stand: &mut Stand) {
    for id in SKELETONS.iter().chain([&BOSS]) {
        let out = stand.call(HOST, "npc_set_state", json!({"id": id, "state": "calm"}));
        assert!(out.ok, "{}", out.result);
    }
}

/// Stand ticks until `member` ran at least one tick more; its tick then.
fn next_tick(stand: &mut Stand, member: usize) -> u64 {
    let before = sim::tick(stand.world(member));
    run_until(stand, 20, "the member ticks", |s| sim::tick(s.world(member)) > before);
    sim::tick(stand.world(member))
}

fn correction(stand: &Stand, member: usize) -> &NetCorrection {
    stand.world(member).resource::<NetCorrection>()
}

fn prediction(stand: &Stand, member: usize) -> NetPrediction {
    stand.world(member).resource::<NetPrediction>().clone()
}

/// Where the host had `id` after each of its ticks, and where `member` showed its own body
/// after each of its ticks: the prediction error is the distance at the same tick.
#[derive(Default)]
struct Track {
    host: BTreeMap<u64, Vec3>,
    shown: BTreeMap<u64, Vec3>,
}

impl Track {
    fn run(&mut self, stand: &mut Stand, member: usize, id: &str, ticks: u64) {
        for _ in 0..ticks {
            stand.run(1);
            self.host.insert(sim::tick(stand.world(HOST)), position(stand, HOST, id));
            for s in correction(stand, member).history() {
                self.shown.insert(s.tick, s.feet);
            }
        }
    }

    /// Mean and largest error over the ticks both have from `from` on.
    fn error(&self, from: u64) -> (f32, f32, usize) {
        self.error_in(from..u64::MAX)
    }

    fn error_in(&self, ticks: std::ops::Range<u64>) -> (f32, f32, usize) {
        let errors: Vec<f32> = self.shown.range(ticks).filter_map(|(t, here)| self.host.get(t).map(|there| here.distance(*there))).collect();
        let max = errors.iter().copied().fold(0.0, f32::max);
        (errors.iter().sum::<f32>() / errors.len().max(1) as f32, max, errors.len())
    }
}

#[test]
fn a_client_moves_its_body_on_the_tick_it_presses() {
    let mut stand = stand_with(1, lossy());
    calm(&mut stand);
    stand.run(60);
    let (me, id) = (own(&stand, C1), body(&stand, C1));
    let (start, host_start) = (position(&stand, C1, &id), position(&stand, HOST, &id));
    assert!(start.distance(host_start) < 0.01, "{start} vs {host_start}");
    // The keyboard: forward held.
    stand.world_mut(C1).get_mut::<Intent>(me).unwrap().move_dir = Vec3::NEG_Z;
    let tick = next_tick(&mut stand, C1);
    let moved = position(&stand, C1, &id);
    assert!(moved.z < start.z - 0.005, "moved on its tick {tick}: {start} -> {moved}");
    assert!(position(&stand, HOST, &id).distance(host_start) < 1e-5, "150 ms away, the host has not seen the press yet");
    stand.run(40);
    stand.world_mut(C1).get_mut::<Intent>(me).unwrap().move_dir = Vec3::ZERO;
    stand.run(150);
    let (here, there) = (position(&stand, C1, &id), position(&stand, HOST, &id));
    assert!(there.distance(host_start) > 1.5, "the host moved it too: {host_start} -> {there}");
    assert!(here.distance(there) < 0.05, "after the correction settles: {here} vs {there}");
    assert_eq!(correction(&stand, C1).snaps, 0, "{:#?}", journal(&stand, C1));
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    verify_host(&stand);
}

/// The client's body put somewhere by the host, settled.
fn put(stand: &mut Stand, id: &str, x: f32, y: f32, z: f32) {
    let out = stand.call(HOST, "teleport", json!({"id": id, "x": x, "y": y, "z": z}));
    assert!(out.ok, "{}", out.result);
    stand.run(90);
}

#[test]
fn a_steady_run_along_the_course_is_predicted_without_snaps() {
    let mut stand = stand_with(1, lossy());
    calm(&mut stand);
    let id = body(&stand, C1);
    put(&mut stand, &id, 30.0, 0.0, 0.0);
    let snaps = correction(&stand, C1).snaps;
    let from = sim::tick(stand.world(C1));
    // Through the corridor and up the stairs to the plateau before the ledge.
    let out = stand.call(C1, "move", json!({"dir": "E", "meters": 21}));
    assert!(out.ok, "{}", out.result);
    let mut track = Track::default();
    track.run(&mut stand, C1, &id, 300);
    let there = position(&stand, HOST, &id);
    assert!(there.x > 50.0 && there.y > 1.9, "on the plateau: {there}");
    let (mean, max, n) = track.error(from);
    println!("steady run: prediction error mean {mean:.4} m, max {max:.4} m over {n} ticks");
    assert!(n > 200, "{n}");
    assert!(mean < 0.05, "mean error {mean} (max {max})");
    assert_eq!(correction(&stand, C1).snaps, snaps, "no snap: {:#?}", journal(&stand, C1));
    assert!(position(&stand, C1, &id).distance(there) < 0.05);
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
}

#[test]
fn a_pillar_stops_the_predicted_body_where_it_stops_the_hosts() {
    let mut stand = stand_with(1, lossy());
    calm(&mut stand);
    let id = body(&stand, C1);
    // South of the pillar at (7, 7) (2 x 2 m), walking north into it.
    put(&mut stand, &id, 7.3, 0.0, 11.0);
    let snaps = correction(&stand, C1).snaps;
    let from = sim::tick(stand.world(C1));
    let out = stand.call(C1, "move", json!({"dir": "N", "meters": 6}));
    assert!(out.ok, "{}", out.result);
    let mut track = Track::default();
    track.run(&mut stand, C1, &id, 150);
    assert!(stand.call(C1, "stop", json!({})).ok);
    track.run(&mut stand, C1, &id, 60);
    let (here, there) = (position(&stand, C1, &id), position(&stand, HOST, &id));
    assert!(there.z > 8.0 && here.z > 8.0, "the pillar (z 6..8) held both: {here} / {there}");
    let (mean, max, n) = track.error(from);
    println!("pillar: prediction error mean {mean:.4} m, max {max:.4} m over {n} ticks");
    assert!(n > 100, "{n}");
    assert!(max < 0.1, "the contact is predicted as the host has it: max error {max}");
    assert!(here.distance(there) < 0.05, "{here} vs {there}");
    assert_eq!(correction(&stand, C1).snaps, snaps);
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
}

#[test]
fn a_path_of_legs_is_walked_without_a_stop_at_each_waypoint() {
    let mut stand = stand_with(1, lossy());
    calm(&mut stand);
    let id = body(&stand, C1);
    put(&mut stand, &id, 30.0, 0.0, 0.0);
    let snaps = correction(&stand, C1).snaps;
    let from = sim::tick(stand.world(C1));
    // A zigzag down the corridor (z -4..4), one order per leg.
    for (n, (x, z)) in [(33.0, 2.0), (36.0, -2.0), (39.0, 2.0), (36.0, 0.0)].into_iter().enumerate() {
        let out = stand.call(C1, "move_to", json!({"target": {"x": x, "z": z}, "queue": n > 0}));
        assert!(out.ok, "{}", out.result);
    }
    assert_eq!(stand.world(C1).get::<Intent>(own(&stand, C1)).unwrap().orders.len(), 4);
    let mut track = Track::default();
    track.run(&mut stand, C1, &id, 300);
    let there = position(&stand, HOST, &id);
    assert!((there.x - 36.0).abs() < 0.5 && there.z.abs() < 0.5, "walked every leg: {there}");
    let (mean, max, n) = track.error(from);
    println!("legs: prediction error mean {mean:.4} m, max {max:.4} m over {n} ticks");
    assert!(n > 200, "{n}");
    assert!(max < 0.05, "the turns are predicted as the host makes them: max error {max} (mean {mean})");
    assert_eq!(correction(&stand, C1).snaps, snaps, "{:#?}", journal(&stand, C1));
    let mine = stand.world(C1).get::<Intent>(own(&stand, C1)).unwrap();
    assert!(mine.orders.is_empty(), "the host's end of the path came back: {:?}", mine.orders);
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    verify_host(&stand);
}

#[test]
fn a_world_taken_again_while_walking_puts_the_body_back_on_the_hosts_time() {
    let mut stand = stand_with(1, lossy());
    calm(&mut stand);
    let id = body(&stand, C1);
    put(&mut stand, &id, 25.0, 0.0, 0.0);
    let out = stand.call(C1, "move", json!({"dir": "E", "meters": 16}));
    assert!(out.ok, "{}", out.result);
    stand.run(30);
    let snaps = correction(&stand, C1).snaps;
    assert!(stand.call(HOST, "net_resync", json!({})).ok);
    // The body comes back where the host had it at its tick, which this clock is ahead of:
    // the first comparison puts it on the host's time at once.
    let mut track = Track::default();
    run_until(&mut stand, 120, "the world again", |s| has_line(s, C1, "net: the host's world again (asked)"));
    let mut fixed_at = None;
    for _ in 0..120 {
        track.run(&mut stand, C1, &id, 1);
        if correction(&stand, C1).snaps > snaps {
            fixed_at = Some(sim::tick(stand.world(C1)));
            break;
        }
    }
    let fixed_at = fixed_at.unwrap_or_else(|| panic!("a snap: {:#?}", journal(&stand, C1)));
    let line = correction(&stand, C1).last_snap.clone().unwrap().1;
    assert!(line.ends_with("(snap: world taken)"), "{line}");
    track.run(&mut stand, C1, &id, 90);
    let (mean, max, n) = track.error(fixed_at);
    println!("after the world again: prediction error mean {mean:.4} m, max {max:.4} m over {n} ticks");
    assert!(n > 60, "{n}");
    assert!(max < 0.05, "no rubber band after it: max error {max} (mean {mean})");
    // To the end of the walk (x 41): where the host stops it.
    stand.run(150);
    let (here, there) = (position(&stand, C1, &id), position(&stand, HOST, &id));
    assert!((there.x - 41.0).abs() < 0.5, "walked on: {there}");
    assert!(here.distance(there) < 0.05, "{here} vs {there}");
    assert_eq!(correction(&stand, C1).snaps, snaps + 1, "{:#?}", journal(&stand, C1));
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
}

/// `ActionStarted` of the own body, as each member saw it: `(tick, action, predicted)`.
#[derive(Resource, Default)]
struct Started(Vec<(u64, String, bool)>);

fn watch_starts(stand: &mut Stand, member: usize, body: Entity) {
    let world = stand.world_mut(member);
    world.init_resource::<Started>();
    world.add_observer(move |e: On<ActionStarted>, tick: Res<SimTick>, mut seen: ResMut<Started>| {
        if e.event().entity == body {
            seen.0.push((tick.0, e.event().action.clone(), e.event().predicted));
        }
    });
}

#[test]
fn a_dodge_starts_on_the_client_at_once_and_the_host_confirms_it() {
    let mut stand = stand_with(1, lossy());
    calm(&mut stand);
    stand.run(40);
    let id = body(&stand, C1);
    let (mine, theirs) = (own(&stand, C1), entity(&stand, HOST, &id).unwrap());
    watch_starts(&mut stand, C1, mine);
    watch_starts(&mut stand, HOST, theirs);
    let dodged = stand.call(C1, "dodge", json!({"dir": "E"}));
    assert!(dodged.ok, "{}", dodged.result);
    assert_eq!(text(&dodged), "ok: roll (E) · predicted, host confirms");
    let asked = sim::tick(stand.world(C1));
    let tick = next_tick(&mut stand, C1);
    assert_eq!(stand.world(C1).resource::<Started>().0, [(asked + 1, "roll".to_owned(), true)], "shown on its tick");
    assert!(tick > asked);
    let shown = stand.world(C1).get::<PredictedAction>(mine).cloned().expect("the roll is shown");
    assert!(!shown.confirmed && shown.action == "roll");
    run_until(&mut stand, 120, "the host confirms the roll", |s| prediction(s, C1).confirmed == 1);
    assert_eq!(stand.world(HOST).resource::<Started>().0, [(asked + 1, "roll".to_owned(), false)], "the host rolled on the same tick");
    stand.run(60);
    let p = prediction(&stand, C1);
    assert_eq!((p.predicted, p.confirmed, p.mispredicted), (1, 1, 0));
    assert!(!has_line(&stand, C1, "not confirmed"), "{:#?}", journal(&stand, C1));
    assert!(stand.world(C1).get::<PredictedAction>(mine).is_none(), "the roll ended");
    let (here, there) = (position(&stand, C1, &id), position(&stand, HOST, &id));
    assert!(here.distance(there) < 0.05, "{here} vs {there}");
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    verify_host(&stand);
}

#[test]
fn a_dodge_without_the_stamina_is_refused_at_once() {
    let mut stand = stand_with(1, lossy());
    calm(&mut stand);
    let id = body(&stand, C1);
    for (path, value) in [("regen", 0.0), ("current", 5.0)] {
        let out = stand.call(HOST, "set", json!({"id": id, "component": "Stamina", "path": path, "value": value}));
        assert!(out.ok, "{}", out.result);
    }
    stand.run(60);
    let dodged = stand.call(C1, "dodge", json!({"dir": "E"}));
    assert!(!dodged.ok, "{}", dodged.result);
    assert!(text(&dodged).contains("not enough stamina: 5/20"), "{}", text(&dodged));
    stand.run(30);
    assert_eq!(prediction(&stand, C1).predicted, 0, "nothing predicted");
    assert!(stand.world(C1).get::<PredictedAction>(own(&stand, C1)).is_none());
}

/// An order the raised guard holds off for as long as it is held (the flask: no cancel from
/// the block into it) is refused as in a single game, by the block shown here.
#[test]
fn an_attack_order_a_raised_guard_cannot_start_is_refused_at_once() {
    let mut stand = stand_with(1, lossy());
    calm(&mut stand);
    stand.run(40);
    let me = own(&stand, C1);
    assert!(stand.call(C1, "block", json!({})).ok);
    // Raised here (0.15 s), not yet in the host's state that came back.
    stand.run(14);
    let shown = stand.world(C1).get::<PredictedAction>(me).cloned().expect("the block is shown");
    assert_eq!(shown.action, "block");
    assert!(stand.world(C1).get::<ActionState>(me).unwrap().current.is_none(), "the host's state is older");
    let out = stand.call(C1, "attack", json!({"target": BOSS, "input": "use_item"}));
    assert!(!out.ok, "{}", out.result);
    assert!(text(&out).contains("while `block` is held; release it first"), "{}", text(&out));
}

#[test]
fn a_stagger_on_the_host_during_a_predicted_roll_is_corrected() {
    let mut stand = stand_with(1, lossy());
    calm(&mut stand);
    stand.run(40);
    let id = body(&stand, C1);
    let fixes = correction(&stand, C1).blends + u64::from(correction(&stand, C1).snaps);
    let dodged = stand.call(C1, "dodge", json!({"dir": "E"}));
    assert!(dodged.ok, "{}", dodged.result);
    // The host staggers the body before the press reaches it: the roll never happens there.
    let staggered = stand.call(HOST, "stagger", json!({"id": id}));
    assert!(staggered.ok, "{}", staggered.result);
    stand.run(180);
    assert!(has_line(&stand, C1, "net: predicted roll"), "{:#?}", journal(&stand, C1));
    let fixed = correction(&stand, C1).blends + u64::from(correction(&stand, C1).snaps);
    assert!(fixed > fixes, "the body was corrected");
    assert_eq!(prediction(&stand, C1).mispredicted, 1, "{:#?}", journal(&stand, C1));
    let (here, there) = (position(&stand, C1, &id), position(&stand, HOST, &id));
    assert!(here.distance(there) < 0.05, "{here} vs {there}");
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    verify_host(&stand);
}

#[test]
fn a_forwarded_teleport_snaps_the_predicted_body() {
    let mut stand = stand_with(1, lossy());
    stand.world_mut(HOST).insert_resource(dev_guests());
    calm(&mut stand);
    stand.run(30);
    let id = body(&stand, C1);
    let sent = stand.call(C1, "teleport", json!({"id": id, "x": 4.0, "z": -2.0}));
    assert!(sent.ok, "{}", sent.result);
    stand.run(60);
    let snap = journal(&stand, C1).into_iter().find(|l| l.contains(&format!("net: {id} corrected by")));
    let snap = snap.unwrap_or_else(|| panic!("a snap line: {:#?}", journal(&stand, C1)));
    assert!(snap.ends_with("m (snap)"), "{snap}");
    assert_eq!(correction(&stand, C1).snaps, 1);
    let (here, there) = (position(&stand, C1, &id), position(&stand, HOST, &id));
    assert!((there.x - 4.0).abs() < 0.01 && (there.z + 2.0).abs() < 0.01, "{there}");
    assert!(here.distance(there) < 0.05, "{here} vs {there}");
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
}

#[test]
fn others_are_where_the_hosts_newest_state_has_them() {
    let mut stand = stand_with(2, lossy());
    let (c2, boss) = (body(&stand, C2), BOSS.to_owned());
    // The warden fights c2, who walks around: both move every tick on the host.
    assert!(stand.call(C2, "move", json!({"dir": "E", "meters": 12})).ok);
    let mut host: BTreeMap<u64, BTreeMap<String, Transform>> = BTreeMap::new();
    let mut checked = 0;
    for _ in 0..240 {
        stand.run(1);
        let tick = sim::tick(stand.world(HOST));
        for id in [&c2, &boss] {
            let tf = *stand.world(HOST).get::<Transform>(entity(&stand, HOST, id).unwrap()).unwrap();
            host.entry(tick).or_default().insert(id.clone(), tf);
        }
        let Some(applied) = stand.world(C1).resource::<NetClient>().ring.last_applied else { continue };
        for id in [&c2, &boss] {
            let here = *stand.world(C1).get::<Transform>(entity(&stand, C1, id).unwrap()).unwrap();
            assert_eq!(here, host[&applied][id], "{id} on c1 at host tick {applied}");
            checked += 1;
        }
    }
    assert!(checked > 400, "{checked}");
    let moved = position(&stand, HOST, &c2);
    assert!(moved.distance(host.values().next().unwrap()[&c2].translation) > 3.0, "c2 moved");
}

/// What the prediction systems changed of the own body's replicated components.
#[derive(Resource, Default)]
struct Probe {
    before: Option<BTreeMap<String, Value>>,
    changed: BTreeSet<String>,
    ticks: u32,
}

fn own_values(world: &mut World) -> Option<BTreeMap<String, Value>> {
    let mut q = world.query_filtered::<Entity, With<Predicted>>();
    let body = q.iter(world).next()?;
    StateEncoder::new(world, StateRule::Replicated).entity(body).map(|(values, _)| values)
}

fn probe_before(world: &mut World) {
    let values = own_values(world);
    world.resource_mut::<Probe>().before = values;
}

fn probe_after(world: &mut World) {
    let after = own_values(world);
    let mut probe = world.resource_mut::<Probe>();
    let (Some(before), Some(after)) = (probe.before.take(), after) else { return };
    probe.ticks += 1;
    let paths: BTreeSet<&String> = before.keys().chain(after.keys()).collect();
    let changed: Vec<String> = paths.into_iter().filter(|p| before.get(*p) != after.get(*p)).cloned().collect();
    probe.changed.extend(changed);
}

#[test]
fn prediction_changes_only_the_predicted_components() {
    let mut stand = stand_with(1, lossy());
    let app = stand.app(C1);
    app.init_resource::<Probe>().add_systems(
        FixedUpdate,
        (probe_before.before(NetSet::PredictInput).run_if(predicts), probe_after.after(NetSet::PredictEnd).run_if(predicts)),
    );
    let id = body(&stand, C1);
    let calls = [
        ("move", json!({"dir": "N", "meters": 4})),
        ("jump", json!({})),
        ("dodge", json!({"dir": "W"})),
        ("light_attack", json!({})),
        ("light_attack", json!({})),
        ("block", json!({})),
        ("block", json!({"on": false})),
        ("sprint", json!({})),
        ("move_to", json!({"target": BOSS})),
        ("glide", json!({})),
        ("stop", json!({})),
    ];
    for (tool, args) in calls {
        let _ = stand.call(C1, tool, args);
        stand.run(25);
    }
    let predicted: BTreeSet<String> = {
        let world = stand.world(C1);
        let rules = world.resource::<NetRules>();
        let registry = world.resource::<AppTypeRegistry>().read();
        rules.predicted().filter_map(|t| registry.get(t).map(|r| r.type_info().type_path().to_owned())).collect()
    };
    let probe = stand.world(C1).resource::<Probe>();
    assert!(probe.ticks > 200, "{}", probe.ticks);
    let outside: Vec<&String> = probe.changed.difference(&predicted).collect();
    assert!(outside.is_empty(), "prediction changed {outside:?}, which are not net_predicted ({predicted:?})");
    for short in ["Transform", "MotionState", "LocomotionInput"] {
        assert!(probe.changed.iter().any(|p| p.ends_with(short)), "{short} was predicted: {:?}", probe.changed);
    }
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    let _ = id;
}

/// The walk of an `Order::Attack` is predicted; the swing is the host's (its lunge comes with
/// the host's state and is blended in).
#[test]
fn an_attack_order_walks_the_predicted_body_as_the_host_walks_it() {
    let mut stand = stand_with(1, lossy());
    calm(&mut stand);
    let id = body(&stand, C1);
    // Nine meters south of the warden (0, 0, -6), nothing between.
    put(&mut stand, &id, 0.0, 0.0, 3.0);
    let theirs = entity(&stand, HOST, &id).unwrap();
    watch_starts(&mut stand, HOST, theirs);
    let snaps = correction(&stand, C1).snaps;
    let from = sim::tick(stand.world(C1));
    let out = stand.call(C1, "attack", json!({"target": BOSS}));
    assert!(out.ok, "{}", out.result);
    let mut track = Track::default();
    track.run(&mut stand, C1, &id, 240);
    let (there, warden) = (position(&stand, HOST, &id), position(&stand, HOST, BOSS));
    assert!(there.z < -3.0, "walked up to the warden at {warden}: {there}");
    let swung = stand.world(HOST).resource::<Started>().0.first().cloned().expect("the host swung");
    let (mean, max, n) = track.error_in(from..swung.0);
    println!("attack order walk: prediction error mean {mean:.4} m, max {max:.4} m over {n} ticks");
    assert!(n > 60 && max < 0.01, "the walk is predicted: mean error {mean} (max {max}, {n} ticks)");
    let (mean, max, _) = track.error(swung.0);
    println!("attack order swing (the host's): prediction error mean {mean:.4} m, max {max:.4} m");
    assert!(max < 0.5, "the lunge is blended in: max error {max}");
    assert_eq!(correction(&stand, C1).snaps, snaps, "{:#?}", journal(&stand, C1));
    let (here, there) = (position(&stand, C1, &id), position(&stand, HOST, &id));
    assert!(here.distance(there) < 0.05, "settled: {here} vs {there}");
    let host_orders = stand.world(HOST).get::<Intent>(theirs).unwrap().orders.len();
    assert_eq!(host_orders, 0, "the one swing ended the order");
    let mine = stand.world(C1).get::<Intent>(own(&stand, C1)).unwrap();
    assert!(mine.orders.is_empty(), "the host's end of the order came back: {:?}", mine.orders);
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    verify_host(&stand);
}

#[test]
fn the_net_overlay_names_the_links_and_the_prediction() {
    let mut stand = stand_with(1, lossy());
    stand.run(120);
    let client = stand.call(C1, "net_overlay", json!({}));
    assert!(client.ok, "{}", client.result);
    let lines = text(&client);
    assert!(lines.starts_with("net overlay on\nNET client p"), "{lines}");
    for part in ["ping ", "loss ", "in ", "kbps", "margin ", "desyncs 0", "prediction: error ", "mispredicted 0/0"] {
        assert!(lines.contains(part), "{part:?} in {lines}");
    }
    assert!(stand.world(C1).resource::<NetOverlay>().enabled);
    let host = stand.call(HOST, "net_overlay", json!({"on": true}));
    let host = text(&host);
    assert!(host.starts_with("net overlay on\nNET host") && host.contains("ping ") && host.contains("margin "), "{host}");
    let off = stand.call(C1, "net_overlay", json!({"on": false}));
    assert_eq!(text(&off), "net overlay off");
    assert!(!stand.world(C1).resource::<NetOverlay>().enabled);
}

// --- the action timeline (B2a) ---------------------------------------------------------------

/// The client's body gets the `timeline` moveset (a heavy that charges, a lunge with miss recovery,
/// a channel that lasts while held) on the host, and the change reaches the client.
fn timeline_stand() -> (Stand, String, Entity, Entity) {
    let mut stand = stand_with(1, lossy());
    calm(&mut stand);
    let id = body(&stand, C1);
    let out = stand.call(HOST, "set", json!({"id": id, "component": "Fighter", "path": "moveset", "value": "timeline"}));
    assert!(out.ok, "{}", out.result);
    stand.run(60);
    let (mine, theirs) = (own(&stand, C1), entity(&stand, HOST, &id).unwrap());
    assert_eq!(stand.world(C1).get::<shards::action::Fighter>(mine).unwrap().moveset, "timeline");
    (stand, id, mine, theirs)
}

fn shown(stand: &Stand, body: Entity) -> Option<PredictedAction> {
    stand.world(C1).get::<PredictedAction>(body).cloned()
}

#[test]
fn predicted_charge_matches_the_host() {
    let (mut stand, id, mine, theirs) = timeline_stand();
    watch_starts(&mut stand, C1, mine);
    watch_starts(&mut stand, HOST, theirs);
    let out = stand.call(C1, "heavy_attack", json!({"hold": true}));
    assert_eq!(text(&out), "ok: spar_heavy (holding) · predicted, host confirms");
    let asked = sim::tick(stand.world(C1));
    run_until(&mut stand, 200, "the host turns the charge", |s| s.world(HOST).resource::<Started>().0.len() == 2);
    // Held 0.25 s from the press: the turn falls on the 15th tick of the run, here and there.
    let turned = [(asked + 1, "spar_heavy".to_owned()), (asked + 15, "spar_heavy_full".to_owned())];
    // What each one started itself: the client also sees the host's starts, forwarded (`predicted: false`).
    let seen = |member: usize| -> Vec<(u64, String)> {
        stand.world(member).resource::<Started>().0.iter().filter(|(_, _, p)| *p == (member == C1)).map(|(t, a, _)| (*t, a.clone())).collect()
    };
    assert_eq!(seen(C1), turned, "predicted on its ticks");
    assert_eq!(seen(HOST), turned, "the host turned it on the same tick");
    // The host's start of the heavy reaches the client after its own turn: it confirms the start
    // and leaves the turn shown (no going back to the heavy and turning again).
    assert!(!has_line(&stand, C1, "cut short"), "{:#?}", journal(&stand, C1));
    let out = stand.call(C1, "heavy_attack", json!({"hold": false}));
    assert!(out.ok, "{}", out.result);
    stand.run(120);
    let p = prediction(&stand, C1);
    assert_eq!((p.predicted, p.confirmed, p.mispredicted), (2, 2, 0), "{:#?}", journal(&stand, C1));
    assert!(shown(&stand, mine).is_none(), "the heavy ended");
    let (here, there) = (position(&stand, C1, &id), position(&stand, HOST, &id));
    assert!(here.distance(there) < 0.05, "{here} vs {there}");
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    verify_host(&stand);
}

#[test]
fn predicted_charge_keeps_its_time() {
    let (mut stand, _, mine, _) = timeline_stand();
    assert!(stand.call(C1, "heavy_attack", json!({"hold": true})).ok);
    // The shown run's clock, every client tick until it ends: the turn and the host's word on it
    // never set it back.
    let mut clock: Vec<(String, f32)> = Vec::new();
    for _ in 0..120 {
        let before = sim::tick(stand.world(C1));
        run_until(&mut stand, 20, "the client ticks", |s| sim::tick(s.world(C1)) > before);
        if let Some(s) = shown(&stand, mine) {
            clock.push((s.action.clone(), s.run.t));
        }
    }
    assert!(clock.iter().any(|(a, _)| a == "spar_heavy") && clock.iter().any(|(a, _)| a == "spar_heavy_full"), "{clock:?}");
    for pair in clock.windows(2) {
        assert!(pair[1].1 > pair[0].1, "the clock went back: {pair:?} in {clock:?}");
    }
    let p = prediction(&stand, C1);
    assert_eq!(p.mispredicted, 0, "{:#?}", journal(&stand, C1));
}

#[test]
fn predicted_charge_refused_by_the_host_reverts() {
    let (mut stand, id, mine, _) = timeline_stand();
    // Enough for the heavy (15), not for the turn (20 more), and none comes back.
    for (path, value) in [("regen", 0.0), ("current", 25.0)] {
        let out = stand.call(HOST, "set", json!({"id": id, "component": "Stamina", "path": path, "value": value}));
        assert!(out.ok, "{}", out.result);
    }
    stand.run(60);
    assert!(stand.call(C1, "heavy_attack", json!({"hold": true})).ok);
    run_until(&mut stand, 60, "the client turns it", |s| shown(s, mine).is_some_and(|p| p.action == "spar_heavy_full"));
    // The host never turns it; once the charge could no longer turn there, the client goes back to
    // the host's heavy at the host's time, not to idle.
    run_until(&mut stand, 200, "the client takes the host's heavy back", |s| {
        shown(s, mine).is_some_and(|p| p.action == "spar_heavy")
    });
    assert!(has_line(&stand, C1, "net: predicted spar_heavy_full not confirmed"), "{:#?}", journal(&stand, C1));
    let host = stand.world(HOST).get::<ActionState>(entity(&stand, HOST, &id).unwrap()).unwrap().current.clone();
    assert_eq!(host.map(|r| r.action), Some("spar_heavy".to_owned()), "the host is still in its heavy");
    assert!(stand.call(C1, "heavy_attack", json!({"hold": false})).ok);
    stand.run(120);
    assert_eq!(prediction(&stand, C1).mispredicted, 1, "{:#?}", journal(&stand, C1));
    assert!(shown(&stand, mine).is_none());
    let (here, there) = (position(&stand, C1, &id), position(&stand, HOST, &id));
    assert!(here.distance(there) < 0.05, "{here} vs {there}");
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    verify_host(&stand);
}

#[test]
fn predicted_run_adopts_host_miss_recovery() {
    let (mut stand, id, mine, _) = timeline_stand();
    // Far from everyone: the lunge hits nothing.
    put(&mut stand, &id, 30.0, 0.0, 0.0);
    let out = stand.call(C1, "use_technique", json!({"slot": 1}));
    assert_eq!(text(&out), "ok: spar_lunge · predicted, host confirms");
    let start = sim::tick(stand.world(C1));
    // 0.7 s as authored, 1.3 s with the whiff: on the client too it is still the lunge after 1 s.
    run_until(&mut stand, 200, "a second of the lunge on the client", |s| sim::tick(s.world(C1)) >= start + 60);
    let s = shown(&stand, mine).expect("still lunging after its authored length");
    assert_eq!(s.action, "spar_lunge");
    assert!((s.run.length - 1.3).abs() < 1e-3, "the host's longer recovery: {}", s.run.length);
    assert!(has_line(&stand, HOST, "spar_lunge missed: +0.60s recovery"), "{:#?}", journal(&stand, HOST));
    run_until(&mut stand, 120, "the lunge ends", |s| shown(s, mine).is_none());
    assert!(sim::tick(stand.world(C1)) >= start + 77, "it ended with the host's run, not before");
    let (here, there) = (position(&stand, C1, &id), position(&stand, HOST, &id));
    assert!(here.distance(there) < 0.05, "{here} vs {there}");
    assert_eq!(prediction(&stand, C1).mispredicted, 0, "{:#?}", journal(&stand, C1));
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
}

#[test]
fn host_cut_drops_the_shown_run() {
    let (mut stand, id, mine, theirs) = timeline_stand();
    let out = stand.call(C1, "use_technique", json!({"slot": 3, "hold": true}));
    assert!(out.ok, "{}", out.result);
    run_until(&mut stand, 120, "the host channels", |s| {
        s.world(HOST).get::<ActionState>(theirs).is_some_and(|a| a.is("spar_channel"))
    });
    stand.run(20);
    assert!(shown(&stand, mine).is_some_and(|p| p.action == "spar_channel" && p.confirmed));
    // The host ends the run by itself, with no new run and no event (as a pair's release will).
    let world = stand.world_mut(HOST);
    world.get_mut::<ActionState>(theirs).unwrap().current = None;
    world.entity_mut(theirs).remove::<shards::action::RunExtras>();
    // The client still holds the button: only the host's word can end it there.
    run_until(&mut stand, 120, "the client drops the channel", |s| shown(s, mine).is_none());
    assert!(stand.world(C1).get::<Intent>(mine).unwrap().is_held("technique.3"));
    stand.run(30);
    assert!(shown(&stand, mine).is_none(), "it does not come back");
    assert!(stand.call(C1, "use_technique", json!({"slot": 3, "hold": false})).ok);
    stand.run(30);
    let (here, there) = (position(&stand, C1, &id), position(&stand, HOST, &id));
    assert!(here.distance(there) < 0.05, "{here} vs {there}");
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
}

// --- context and paired actions (B3) ---------------------------------------------------------

/// The client's body gets the `contextual` moveset (light is a finisher next to a broken enemy, a
/// counter after a parry) and the skeleton e7 the `sparring` one; e7 stands 1.8 m north of the
/// body, the body locked on to it. The host's agent `skeleton` possesses e7.
fn context_stand() -> (Stand, String, Entity) {
    let mut stand = stand_with(1, lossy());
    calm(&mut stand);
    let id = body(&stand, C1);
    for (who, moveset) in [(id.as_str(), "contextual"), ("e7", "sparring")] {
        let out = stand.call(HOST, "set", json!({"id": who, "component": "Fighter", "path": "moveset", "value": moveset}));
        assert!(out.ok, "{}", out.result);
    }
    stand.world_mut(HOST).resource_mut::<shards::ai_core::AgentSessions>().configure("skeleton", shards::ai_core::Scope::Dev, Some(AiId(7)));
    stand.run(60);
    let mine = own(&stand, C1);
    assert_eq!(stand.world(C1).get::<shards::action::Fighter>(mine).unwrap().moveset, "contextual");
    face_off(&mut stand, &id);
    let out = stand.call(C1, "lock_on", json!({"target": "e7"}));
    assert!(out.ok, "{}", out.result);
    let out = stand.call_as(HOST, "skeleton", "lock_on", json!({"target": id}));
    assert!(out.ok, "{}", out.result);
    stand.run(30);
    (stand, id, mine)
}

/// The body at the origin and e7 1.8 m north of it, both healed, settled.
fn face_off(stand: &mut Stand, id: &str) {
    for (who, z) in [(id, 0.0), ("e7", -1.8)] {
        for (tool, args) in [("heal", json!({"id": who})), ("teleport", json!({"id": who, "x": 0, "y": 0, "z": z}))] {
            let out = stand.call(HOST, tool, args);
            assert!(out.ok, "{tool}: {}", out.result);
        }
    }
    stand.run(90);
}

/// Whether the client shows its light attack bound to a finisher (its copy of the host's context).
fn finisher_shown(stand: &Stand, mine: Entity) -> bool {
    stand.world(C1).get::<shards::action::ContextInputs>(mine).is_some_and(|c| c.bound("light").is_some_and(|b| b.victim.is_some()))
}

/// A client presses light as soon as its copy shows the finisher (a few ticks of reaction at most),
/// twelve times over a 150 ms, 2 % loss link. A paired action is not predicted (13a, "Context
/// staleness"; see `context_staleness_around_the_break_is_counted`): the host resolves the press on
/// its own tick and the client takes the finisher over, so nothing the client shows is ever wrong.
#[test]
fn client_finishers_are_confirmed_by_the_host() {
    let (mut stand, id, mine) = context_stand();
    let trials = 12u64;
    let mut missed = Vec::new();
    for trial in 0..trials {
        face_off(&mut stand, &id);
        let before = prediction(&stand, C1);
        let out = stand.call(HOST, "stagger", json!({"id": "e7", "poise": true}));
        assert!(out.ok, "{}", out.result);
        run_until(&mut stand, 120, "the client shows the finisher", |s| finisher_shown(s, mine));
        // A reaction of 0 to 9 ticks (up to 150 ms).
        stand.run(trial % 4 * 3);
        let out = stand.call(C1, "light_attack", json!({}));
        assert_eq!(text(&out), "ok: spar_finisher · the host starts it (a paired action is not predicted)", "trial {trial}");
        let takes = format!("{id} takes e7 with spar_finisher");
        run_until(&mut stand, 120, "the host takes e7", |s| journal(s, HOST).iter().filter(|l| l.contains(&takes)).count() as u64 > trial);
        stand.run(150);
        let after = prediction(&stand, C1);
        assert_eq!(after.predicted, before.predicted, "trial {trial}: a paired action is not predicted");
        if after.mispredicted > before.mispredicted {
            missed.push(trial);
        }
    }
    println!("B3 stand: {} client finisher misprediction(s) in {trials} trials at latency 150 ms, loss 2 % (none predicted)", missed.len());
    assert!(missed.is_empty(), "mispredicted in trials {missed:?}: {:#?}", journal(&stand, C1));
    assert!(!has_line(&stand, C1, "not confirmed"), "{:#?}", journal(&stand, C1));
    let downs = journal(&stand, HOST).iter().filter(|l| l.contains("e7 reacts knockdown")).count() as u64;
    assert_eq!(downs, trials, "every finisher knocked e7 down: {:#?}", journal(&stand, HOST));
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    verify_host(&stand);
}

/// A client's guard press against the host's skeleton slam: fresh, so the guard is perfect and
/// parries; the counter the parry binds to light is pressed from the client's copy of the context
/// and confirmed.
#[test]
fn a_client_parries_and_counters() {
    let (mut stand, id, _) = context_stand();
    let before = prediction(&stand, C1);
    let out = stand.call_as(HOST, "skeleton", "heavy_attack", json!({}));
    assert!(out.ok, "{}", out.result);
    // The slam starts on the host's next tick; the client's guard is to start 24 ticks into it (0.4 s,
    // the raise ends after its hit lands). A press is stamped with the client's tick, which runs ahead.
    let slam = sim::tick(stand.world(HOST)) + 1;
    assert!(sim::tick(stand.world(C1)) < slam + 23, "the client is less than 0.38 s ahead");
    run_until(&mut stand, 60, "the client reaches the guard's tick", |s| sim::tick(s.world(C1)) + 1 >= slam + 24);
    let out = stand.call(C1, "block", json!({}));
    assert!(out.ok, "{}", out.result);
    let parried = format!("{id} parried e7 spar_slam");
    for _ in 0..120 {
        if has_line(&stand, HOST, &parried) {
            break;
        }
        stand.run(1);
    }
    assert!(has_line(&stand, HOST, &parried), "slam at {slam}: {:#?}", journal(&stand, HOST).iter().rev().take(25).collect::<Vec<_>>());
    run_until(&mut stand, 60, "the client shows the counter", |s| {
        let mine = own(s, C1);
        s.world(C1).get::<shards::action::ContextInputs>(mine).and_then(|c| c.bound("light")).is_some_and(|b| b.action == "spar_counter")
    });
    let out = stand.call(C1, "light_attack", json!({}));
    assert_eq!(text(&out), "ok: spar_counter · predicted, host confirms");
    assert!(stand.call(C1, "block", json!({"on": false})).ok);
    let counter = format!("{id} -> e7 spar_counter -14");
    run_until(&mut stand, 120, "the host's counter", |s| has_line(s, HOST, &counter));
    stand.run(90);
    let after = prediction(&stand, C1);
    println!(
        "B3 stand: guard and counter from a client: {} predicted, {} confirmed, {} mispredicted",
        after.predicted - before.predicted,
        after.confirmed - before.confirmed,
        after.mispredicted - before.mispredicted
    );
    assert_eq!(after.mispredicted, before.mispredicted, "{:#?}", journal(&stand, C1));
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    verify_host(&stand);
}

/// One staleness trial: the body presses light `k` ticks after (negative: before) the host breaks
/// e7's poise, over the lossy link. Returns what the client predicted and how it fared.
fn staleness_trial(stand: &mut Stand, id: &str, k: i64) -> Staleness {
    face_off(stand, id);
    let before = prediction(stand, C1);
    let broken = |stand: &mut Stand| {
        let out = stand.call(HOST, "stagger", json!({"id": "e7", "poise": true}));
        assert!(out.ok, "{}", out.result);
    };
    let take = |stand: &mut Stand| {
        let out = stand.call(C1, "light_attack", json!({}));
        assert!(out.ok, "{}", out.result);
        text(&out).to_owned()
    };
    let said = if k >= 0 {
        broken(stand);
        stand.run(k as u64);
        take(stand)
    } else {
        let said = take(stand);
        stand.run((-k) as u64);
        broken(stand);
        said
    };
    stand.run(180);
    let after = prediction(stand, C1);
    let takes = format!("{id} takes e7 with spar_finisher");
    Staleness {
        k,
        said,
        mispredicted: after.mispredicted - before.mispredicted,
        host_finisher: journal(stand, HOST).iter().filter(|l| l.contains(&takes)).count(),
    }
}

#[derive(Debug)]
struct Staleness {
    k: i64,
    said: String,
    mispredicted: u64,
    host_finisher: usize,
}

/// 13a "Context staleness": a press in the window before the client's copy of the host's context
/// shows the finisher (one way latency plus a tick after the break), and one near the end of the
/// stagger, when the host's break may lapse before the stamped tick. The counts are reported
/// (`--nocapture`); a predicted finisher the host does not confirm, or a predicted jab the host
/// turned into a finisher, is a misprediction.
#[test]
fn context_staleness_around_the_break_is_counted() {
    let (mut stand, id, _) = context_stand();
    let mut rows = Vec::new();
    for k in (-9..=9).chain([30, 34, 36, 38, 40, 41, 42, 43, 44, 46]) {
        rows.push(staleness_trial(&mut stand, &id, k));
    }
    let mut finisher_predicted = 0;
    let mut finisher_missed = 0;
    let mut other_answered_differently = 0;
    for r in &rows {
        println!("B3 staleness k={:>3}: {:<48} mispredicted {} host finishers so far {}", r.k, r.said, r.mispredicted, r.host_finisher);
        if r.said.contains("spar_finisher") {
            finisher_predicted += 1;
            finisher_missed += r.mispredicted;
        } else {
            other_answered_differently += r.mispredicted;
        }
    }
    println!(
        "B3 stand: staleness over {} trials at latency 150 ms, loss 2 %: {finisher_predicted} finisher presses ({finisher_missed} predicted and not confirmed), {other_answered_differently} other predictions the host answered differently",
        rows.len()
    );
    // The finisher is never predicted, so it is never wrong; the jab predicted from a context that is
    // a round trip stale is replaced by the host's finisher (counted above, reported in the PR).
    assert_eq!(finisher_missed, 0, "{rows:#?}");
    assert!(rows.iter().all(|r| !(r.said.contains("spar_finisher") && r.said.contains("predicted, host confirms"))), "{rows:#?}");
    assert_eq!(desyncs(&stand, C1), 0, "{:#?}", journal(&stand, C1));
    verify_host(&stand);
}

/// Two players of one team on the host's e7: C1 sees the finisher (C2 stands far), then C2 steps
/// 0.8 m nearer to e7 than C1 (more than the finisher's `slack`) `j` ticks around C1's press, so
/// the host's choice of who takes e7 changes under C1's finger (C2 presses as it steps in). No
/// finisher is predicted: the counts are the finishers each player got and C1's mispredictions.
#[test]
fn the_slack_handoff_between_two_players_is_counted() {
    let mut stand = stand_with(2, lossy());
    calm(&mut stand);
    let (a, b) = (body(&stand, C1), body(&stand, C2));
    for (who, moveset) in [(a.as_str(), "contextual"), (b.as_str(), "contextual"), ("e7", "sparring")] {
        let out = stand.call(HOST, "set", json!({"id": who, "component": "Fighter", "path": "moveset", "value": moveset}));
        assert!(out.ok, "{}", out.result);
    }
    stand.run(60);
    let mine = own(&stand, C1);
    let mut rows = Vec::new();
    for j in [-12, -9, -6, -3, 0, 3, 6, 9, 12, 18, 24, 30] {
        for (who, x, z) in [(a.as_str(), 0.0, 0.0), ("e7", 0.0, -1.8), (b.as_str(), 6.0, 6.0)] {
            for (tool, args) in [("heal", json!({"id": who})), ("teleport", json!({"id": who, "x": x, "y": 0, "z": z}))] {
                assert!(stand.call(HOST, tool, args).ok);
            }
        }
        stand.run(90);
        let before = (prediction(&stand, C1), prediction(&stand, C2));
        let (takes_a, takes_b) = (format!("{a} takes e7 with spar_finisher"), format!("{b} takes e7 with spar_finisher"));
        let seen = |stand: &Stand, needle: &str| journal(stand, HOST).iter().filter(|l| l.contains(needle)).count();
        let (had_a, had_b) = (seen(&stand, &takes_a), seen(&stand, &takes_b));
        assert!(stand.call(HOST, "stagger", json!({"id": "e7", "poise": true})).ok);
        run_until(&mut stand, 120, "C1 shows the finisher", |s| finisher_shown(s, mine));
        stand.run(2);
        // j ticks after C1's press (negative: before), C2 steps in.
        let step_in = |stand: &mut Stand| {
            assert!(stand.call(HOST, "teleport", json!({"id": b, "x": 0, "y": 0, "z": -0.9})).ok);
            assert!(stand.call(C2, "light_attack", json!({})).ok);
        };
        let press_a = |stand: &mut Stand| assert!(stand.call(C1, "light_attack", json!({})).ok);
        if j >= 0 {
            press_a(&mut stand);
            stand.run(j as u64);
            step_in(&mut stand);
        } else {
            step_in(&mut stand);
            stand.run((-j) as u64);
            press_a(&mut stand);
        }
        stand.run(180);
        let after = (prediction(&stand, C1), prediction(&stand, C2));
        rows.push((j, seen(&stand, &takes_a) - had_a, seen(&stand, &takes_b) - had_b, after.0.mispredicted - before.0.mispredicted));
    }
    for (j, a_took, b_took, missed) in &rows {
        println!("B3 slack j={j:>3}: C1 took e7 {a_took}x, C2 {b_took}x, C1 mispredicted {missed}");
    }
    let predicted_finishers: u64 = rows.iter().map(|r| r.3).sum();
    println!("B3 stand: slack handoff over {} trials, 2 clients, latency 150 ms, loss 2 %: {predicted_finishers} client mispredictions", rows.len());
    assert!(rows.iter().all(|r| r.1 + r.2 <= 1), "one finisher per break: {rows:?}");
    assert_eq!(desyncs(&stand, C1) + desyncs(&stand, C2), 0);
    verify_host(&stand);
}

/// The client's guard against the host's: a fresh press opens the perfect window on the same ticks
/// on both machines, and a press too soon after the last opens it on neither. The client runs its
/// own press rule on its own presses (\`ActionPredictor::arm\`), not the host's arm a round trip late.
#[test]
fn a_predicted_guard_shows_the_hosts_perfect_window() {
    let (mut stand, id, mine) = context_stand();
    let theirs = entity(&stand, HOST, &id).unwrap();
    let mut host: BTreeMap<u64, bool> = BTreeMap::new();
    let mut client: BTreeMap<u64, bool> = BTreeMap::new();
    let sample = |stand: &mut Stand, ticks: u64, host: &mut BTreeMap<u64, bool>, client: &mut BTreeMap<u64, bool>| {
        for _ in 0..ticks {
            stand.run(1);
            let h = stand.world(HOST).get::<ActionState>(theirs).and_then(|s| s.current.as_ref().filter(|r| r.action == "spar_parry_guard").map(|r| r.windows.perfect));
            if let Some(w) = h {
                host.insert(sim::tick(stand.world(HOST)), w);
            }
            if let Some(s) = shown(stand, mine).filter(|s| s.action == "spar_parry_guard") {
                client.insert(sim::tick(stand.world(C1)), s.run.windows.perfect);
            }
        }
    };
    // A fresh press from rest.
    assert!(stand.call(C1, "block", json!({})).ok);
    sample(&mut stand, 40, &mut host, &mut client);
    assert!(stand.call(C1, "block", json!({"on": false})).ok);
    sample(&mut stand, 90, &mut host, &mut client);
    let fresh_ticks = client.values().filter(|w| **w).count();
    // The same guard raised again 6 ticks after it was let go: inside the gap, no perfect window.
    assert!(stand.call(C1, "block", json!({})).ok);
    sample(&mut stand, 4, &mut host, &mut client);
    assert!(stand.call(C1, "block", json!({"on": false})).ok);
    sample(&mut stand, 4, &mut host, &mut client);
    assert!(stand.call(C1, "block", json!({})).ok);
    sample(&mut stand, 40, &mut host, &mut client);
    assert!(stand.call(C1, "block", json!({"on": false})).ok);
    sample(&mut stand, 60, &mut host, &mut client);
    let both: Vec<u64> = client.keys().filter(|t| host.contains_key(t)).copied().collect();
    let differing: Vec<(u64, bool, bool)> = both.iter().filter(|t| client[t] != host[t]).map(|t| (*t, client[t], host[t])).collect();
    println!(
        "B3 stand: predicted guard windows: {} ticks on both, {} with a perfect window on the client, {} on the host, {} differ",
        both.len(),
        client.values().filter(|w| **w).count(),
        host.values().filter(|w| **w).count(),
        differing.len()
    );
    assert!(fresh_ticks > 0, "a fresh press opens a perfect window on the client: {client:?}");
    assert!(both.len() > 40, "{} shared ticks", both.len());
    assert!(differing.is_empty(), "client vs host perfect window: {differing:?}");
    assert_eq!(desyncs(&stand, C1), 0);
    verify_host(&stand);
}
