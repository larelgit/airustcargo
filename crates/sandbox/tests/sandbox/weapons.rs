//! Weapons (F4) on the sandbox's test weapons (`content/combat/weapons.*.ron`): a reload cut by a switch
//! loads nothing, a snapshot keeps magazine, reserve and slot, the `armed` prefab carries its arsenal, and the
//! engine rule `action.arsenal_sane` stops a state no call makes. The scenario `weapons.scenario` plays the
//! same weapons through the player's tools.

use shards::action::{Arsenal, Charges};
use shards::ai_core::snapshot::{self, differences};
use shards::ai_core::{self, replay, sim, AgentSessions, JournalKind, Scope};
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::*;
use crate::invariants::{called, journal};

/// The player given the two test weapons, the sidearm in the hands, in a calm arena.
fn armed_player() -> App {
    let mut app = sandbox::test_app(42);
    calm_skeletons(&mut app);
    calm_warden(&mut app);
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": 0, "z": 0}));
    ok(&mut app, "give_weapon", json!({"id": PLAYER, "moveset": "test_sidearm"}));
    ok(&mut app, "give_weapon", json!({"id": PLAYER, "moveset": "test_blaster"}));
    step(&mut app, 2);
    app
}

/// `(current, max)` of the player's charge.
fn held(app: &App, name: &str) -> (u32, u32) {
    let charges = get::<Charges>(app, PLAYER);
    let charge = charges.get(name).unwrap_or_else(|| panic!("no charge {name}: {charges:?}"));
    (charge.current, charge.max)
}

fn set_charge(app: &mut App, name: &str, current: u32) {
    let world = app.world_mut();
    let mut query = world.query_filtered::<&mut Charges, With<sandbox::gameplay::Player>>();
    query.single_mut(world).unwrap().get_mut(name).unwrap().current = current;
}

#[test]
fn the_given_weapons_bring_their_charges_and_a_moveset() {
    let app = armed_player();
    assert_eq!(get::<Fighter>(&app, PLAYER).moveset, "test_sidearm");
    let arsenal = get::<Arsenal>(&app, PLAYER);
    assert_eq!((arsenal.current, arsenal.bare.as_deref()), (Some(1), Some("player")), "the moveset it had is kept");
    assert_eq!(held(&app, "sidearm"), (6, 6));
    assert_eq!(held(&app, "sidearm.reserve"), (12, 24));
    assert_eq!(held(&app, "blaster"), (3, 3));
    assert_eq!(held(&app, "blaster.reserve"), (9, 9));
    assert_eq!(held(&app, "flask"), (3, 3), "its own charges stay");
}

#[test]
fn a_reload_cut_by_a_switch_loads_nothing_and_one_left_alone_loads_what_fits() {
    let mut app = armed_player();
    set_charge(&mut app, "sidearm", 1);
    assert_eq!(text(&ok(&mut app, "reload", json!({}))), "ok: arm_sidearm_reload");
    step(&mut app, 30); // 0.5 s into the 0.8 s draw
    assert!(text(&ok(&mut app, "weapon", json!({"slot": 2}))).starts_with("ok: switches to 2 (test_blaster)"));
    step(&mut app, 80);
    assert_eq!(get::<Fighter>(&app, PLAYER).moveset, "test_blaster");
    assert_eq!(held(&app, "sidearm"), (1, 6), "nothing was loaded");
    assert_eq!(held(&app, "sidearm.reserve"), (12, 24));
    assert!(lines(&app, "refilled sidearm").is_empty(), "{:?}", events(&app));

    ok(&mut app, "weapon", json!({"slot": 1}));
    step(&mut app, 2);
    ok(&mut app, "reload", json!({}));
    step(&mut app, 70);
    assert_eq!(held(&app, "sidearm"), (6, 6));
    assert_eq!(held(&app, "sidearm.reserve"), (7, 24), "five moved");
    assert_eq!(lines(&app, "refilled sidearm").len(), 1);
}

#[test]
fn the_second_weapon_reloads_itself_and_the_first_says_what_to_do() {
    let mut app = armed_player();
    set_charge(&mut app, "sidearm", 0);
    assert_eq!(fails(&mut app, "light_attack", json!({})), "sidearm empty (0/6): reload");
    set_charge(&mut app, "sidearm.reserve", 0);
    assert_eq!(fails(&mut app, "reload", json!({})), "no sidearm.reserve left to refill sidearm with; find more");
    ok(&mut app, "weapon", json!({"slot": 2}));
    step(&mut app, 2);
    set_charge(&mut app, "blaster", 0);
    assert_eq!(text(&ok(&mut app, "light_attack", json!({}))), "ok: arm_blaster_reload");
    step(&mut app, 40);
    assert_eq!((held(&app, "blaster"), held(&app, "blaster.reserve")), ((3, 3), (6, 9)));
}

/// Snapshot, then the same inputs in this game and in a fresh one that loaded it: the states are alike.
fn assert_continues_alike(what: &str, mut a: App, after: impl Fn(&mut App)) {
    let snapshot = snapshot::capture(a.world()).unwrap();
    let n = sim::tick(a.world());
    after(&mut a);

    let mut b = sandbox::test_app(42);
    let report = snapshot::restore(b.world_mut(), &snapshot, false).unwrap_or_else(|e| panic!("{what}: {e}"));
    assert!(report.exact(), "{what}: restored differently: {:?}", report.differences);
    assert_eq!(replay::state_hash(b.world()), snapshot.state_hash, "{what}: the loaded state is the saved one");
    after(&mut b);

    let (sa, sb) = (snapshot::capture(a.world()).unwrap(), snapshot::capture(b.world()).unwrap());
    let diff = differences(&sa, &sb);
    assert!(diff.is_empty(), "{what}: the states differ:\n{}", diff.join("\n"));
    assert_eq!(replay::state_hash(a.world()), replay::state_hash(b.world()), "{what}: state hash");
    let story = |app: &App| -> Vec<String> {
        let journal = app.world().resource::<shards::ai_core::AiJournal>();
        journal
            .after(0)
            .0
            .filter(|e| e.tick > n && matches!(e.kind, JournalKind::Event | JournalKind::Warn | JournalKind::Error))
            .map(ToString::to_string)
            .collect()
    };
    assert_eq!(story(&a), story(&b), "{what}: the journal from tick {n} on");
    assert!(sim::tick(a.world()) > n, "{what}: the game went on");
}

#[test]
fn a_snapshot_keeps_the_magazine_the_reserve_the_slot_and_a_switch_that_waits() {
    let mut app = armed_player();
    set_charge(&mut app, "sidearm", 2);
    ok(&mut app, "reload", json!({}));
    step(&mut app, 20);
    // A second switch asked for in the middle of a draw: it waits only while no window is open, and the draw has one,
    // so ask in the seat phase, which has none.
    step(&mut app, 35);
    assert_eq!(action(&app, PLAYER), "arm_sidearm_reload");
    ok(&mut app, "weapon", json!({"slot": 2}));
    // Not yet taken: the press is in the intent, the switch not asked for. One tick makes it a pending one.
    step(&mut app, 1);
    let arsenal = get::<Arsenal>(&app, PLAYER);
    assert_eq!((arsenal.current, arsenal.pending), (Some(1), Some(2)), "the seat phase has no window: the switch waits");
    assert_eq!((held(&app, "sidearm"), held(&app, "sidearm.reserve")), ((6, 6), (8, 24)), "the rounds moved at 0.8 s");
    assert_continues_alike("magazine, reserve, slot and a waiting switch", app, |app| {
        step(app, 40);
        let arsenal = get::<Arsenal>(app, PLAYER);
        assert_eq!((arsenal.current, arsenal.pending), (Some(2), None));
        ok(app, "light_attack", json!({}));
        step(app, 30);
    });
}

#[test]
fn the_armed_prefab_carries_its_arsenal_and_is_driven_by_the_tools() {
    let mut app = sandbox::test_app(42);
    calm_skeletons(&mut app);
    calm_warden(&mut app);
    ok(&mut app, "spawn_prefab", json!({"id": "armed", "x": 8, "z": 8, "y": 0}));
    step(&mut app, 3);
    let world = app.world_mut();
    let mut query = world.query::<(&AiId, &Arsenal)>();
    let (id, arsenal) = query.iter(world).find(|(_, a)| a.slots.len() == 2 && a.bare.is_none()).map(|(i, a)| (*i, a.clone())).expect("the armed body");
    assert_eq!((arsenal.current, arsenal.list()), (Some(1), "1 test_sidearm, 2 test_blaster".to_owned()));
    let entity = app.world().resource::<ai_core::AiIndex>().get(id).unwrap();
    let names: Vec<String> = app.world().get::<Charges>(entity).unwrap().0.iter().map(|c| c.name.clone()).collect();
    assert_eq!(names, ["blaster", "blaster.reserve", "sidearm", "sidearm.reserve"]);

    // A dev agent possessing it plays its weapons through the player tools.
    app.world_mut().resource_mut::<AgentSessions>().configure("armed", Scope::Dev, Some(id));
    let out = ai_core::call_tool(app.world_mut(), "armed", "weapon", json!({"slot": "next"}));
    assert!(out.ok, "{}", out.result);
    step(&mut app, 2);
    assert_eq!(app.world().get::<Fighter>(entity).unwrap().moveset, "test_blaster");
    let described = text(&ok(&mut app, "inspect", json!({"id": id.to_string()}))).to_owned();
    assert!(described.contains("weapon test_blaster 3/3 (9) · slots 1 test_sidearm, 2 test_blaster"), "{described}");
}

/// A change that leaves the world as a buggy system of a game would.
type Corrupt = fn(&mut World);

/// Runs `corrupt` at the end of tick `at`, the way a buggy system of a game would leave the world.
fn plant(app: &mut App, at: u64, corrupt: Corrupt) {
    app.add_systems(
        FixedUpdate,
        (move |world: &mut World| {
            if sim::tick(world) == at {
                corrupt(world);
            }
        })
        .in_set(SimSet::Cleanup),
    );
}

#[test]
fn an_arsenal_no_call_could_make_stops_the_game() {
    let broken: [(Corrupt, &str); 3] = [
        (|world| {
            let player = world.resource::<ai_core::AiIndex>().get(AiId(5)).unwrap();
            world.get_mut::<Arsenal>(player).unwrap().current = Some(7);
        }, "e5 Arsenal.current is slot 7, which holds no weapon (1 test_sidearm, 2 test_blaster)"),
        (|world| {
            let player = world.resource::<ai_core::AiIndex>().get(AiId(5)).unwrap();
            world.get_mut::<Arsenal>(player).unwrap().slots.reverse();
        }, "e5 Arsenal slots 2 test_blaster, 1 test_sidearm are not ascending numbers from 1 to 9"),
        (|world| {
            let player = world.resource::<ai_core::AiIndex>().get(AiId(5)).unwrap();
            world.get_mut::<Charges>(player).unwrap().get_mut("sidearm").unwrap().current = 9;
        }, "e5 charge sidearm holds 9 of at most 6"),
    ];
    for (corrupt, wanted) in broken {
        let mut app = armed_player();
        let now = sim::tick(app.world());
        plant(&mut app, now + 2, corrupt);
        let out = called(&mut app, "step", json!({"ticks": 20}));
        let want = format!("stopped: invariant action.arsenal_sane violated at tick {}", now + 2);
        assert!(text(&out).contains(&want), "wanted `{want}`, got `{}`", text(&out));
        let line = journal(&app, JournalKind::Error).into_iter().find(|l| l.contains("invariant action.arsenal_sane violated")).expect("the journal has the line");
        assert!(line.contains(wanted), "wanted `{wanted}` in `{line}`");
    }
}
