//! Co-op: no machine stops the others' clock. A host and a client hold only the pictures of the
//! bodies in the blow, by id, in real time; their clocks run on.

use shards::ai_core::{sim, ClockHold, PresentationHolds, SimTick};
use shards::bevy::time::TimeUpdateStrategy;
use shards::core::NetRole;
use shards::hitstop::{HitStops, Outcome};

use crate::*;

fn session(name: &str, role: NetRole) -> (App, Folder, Entity, Entity) {
    let assets = Folder::new(name);
    let mut app = drawn(&assets);
    let hero = fighter(app.world_mut(), "hero", "heroes", Vec3::ZERO, Vec3::NEG_Z, 100.0);
    let brute = fighter(app.world_mut(), "brute", "monsters", Vec3::new(0.0, 0.0, -1.5), Vec3::Z, 100.0);
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    app.update();
    sim::resume(app.world_mut());
    app.insert_resource(role);
    for _ in 0..10 {
        app.update();
    }
    (app, assets, hero, brute)
}

fn hit(app: &mut App, a: Entity, b: Entity, weight: HitWeight) {
    app.world_mut().trigger(damaged(a, b, weight, None));
    app.world_mut().flush();
}

#[test]
fn a_host_holds_the_pictures_of_the_bodies_and_never_its_clock() {
    let (mut app, _assets, hero, brute) = session("host", NetRole::Host);
    let ticks = app.world().resource::<SimTick>().0;
    hit(&mut app, hero, brute, HitWeight::Break);
    let ids = {
        let world = app.world();
        [*world.get::<AiId>(hero).unwrap(), *world.get::<AiId>(brute).unwrap()]
    };
    assert_eq!(app.world().resource::<HitStops>().log.back().unwrap().outcome, Outcome::Picture);
    assert!(!app.world().resource::<ClockHold>().is_pending(), "the clock is never held");
    let now = app.world().resource::<Time<Real>>().elapsed();
    let holds = app.world().resource::<PresentationHolds>();
    assert!(ids.iter().all(|id| holds.is_held(*id, now)), "both bodies are held");
    assert_eq!(holds.held(now).count(), 2);

    // The clock runs on through the hold: a tick a frame, the speed never touched.
    for _ in 0..6 {
        app.update();
        assert_eq!(app.world().resource::<Time<Virtual>>().relative_speed(), 1.0);
    }
    assert_eq!(app.world().resource::<SimTick>().0, ticks + 6);
    // And it is over after its real time: 0.10 s.
    for _ in 0..4 {
        app.update();
    }
    let now = app.world().resource::<Time<Real>>().elapsed();
    assert_eq!(app.world().resource::<PresentationHolds>().held(now).count(), 0);

    let said = ok(&mut app, "hitstop", json!({}));
    assert!(said.starts_with("hit stop: co-op host: holds the pictures of the bodies only, never the clock\n"), "{said}");
    assert!(said.contains("total: 1 blows · 0 clock holds · 1 picture holds · 0 not held"), "{said}");
    assert!(said.contains("poise break 0.10 s (damaged) on e"), "{said}");
    assert!(said.contains(": pictures held (0.10 s)"), "{said}");
}

#[test]
fn a_client_holds_the_pictures_too_and_the_tool_shows_them_while_they_last() {
    let (mut app, _assets, hero, brute) = session("client", NetRole::Client);
    hit(&mut app, hero, brute, HitWeight::Heavy);
    assert_eq!(app.world().resource::<HitStops>().log.back().unwrap().outcome, Outcome::Picture);
    assert!(!app.world().resource::<ClockHold>().is_pending());
    let said = ok(&mut app, "hitstop", json!({}));
    assert!(said.starts_with("hit stop: co-op client: holds the pictures of the bodies only, never the clock\n"), "{said}");
    assert!(said.contains("pictures held: e"), "{said}");
    assert_eq!(lines(&app, JournalKind::Local, "hitstop: hit 0.06 s").len(), 1);
}

#[test]
fn the_local_filter_holds_only_the_blows_the_local_player_is_in() {
    let (mut app, _assets, hero, brute) = session("local", NetRole::Host);
    app.world_mut().entity_mut(hero).insert(LocalPlayer);
    let other = fighter(app.world_mut(), "brute", "monsters", Vec3::new(5.0, 0.0, 0.0), Vec3::Z, 100.0);
    ok(&mut app, "settings_set", json!({"group": "accessibility", "key": "hit_stop_local_only", "value": "true"}));
    hit(&mut app, brute, other, HitWeight::Light);
    assert!(matches!(app.world().resource::<HitStops>().log.back().unwrap().outcome, Outcome::Skipped(_)));
    hit(&mut app, brute, hero, HitWeight::Light);
    assert_eq!(app.world().resource::<HitStops>().log.back().unwrap().outcome, Outcome::Picture);
    let now = app.world().resource::<Time<Real>>().elapsed();
    let held: Vec<_> = app.world().resource::<PresentationHolds>().held(now).collect();
    assert_eq!(held.len(), 2, "the local player and the one that hit them");
}

#[test]
fn a_solo_rule_matches_alone_and_a_machine_in_co_op_falls_through_to_the_next() {
    let assets = Folder::new("solo_rule");
    assets.put(
        "content/feel/test.hitstop.ron",
        r#"{ "t": (max_seconds: 0.12, rules: [
            (name: "slow dodge", when: (event: PerfectEvaded, solo: true), seconds: 0.10),
            (name: "dodge", when: (event: PerfectEvaded), seconds: 0.04),
        ]) }"#,
    );
    let mut app = drawn(&assets);
    let hero = fighter(app.world_mut(), "hero", "heroes", Vec3::ZERO, Vec3::NEG_Z, 100.0);
    let brute = fighter(app.world_mut(), "brute", "monsters", Vec3::new(0.0, 0.0, -1.5), Vec3::Z, 100.0);
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    app.update();
    sim::resume(app.world_mut());
    for _ in 0..10 {
        app.update();
    }
    let (h, b) = (id(&app, hero), id(&app, brute));
    let play = format!("{{\"event\": \"perfect_evaded\", \"attacker\": \"{b}\", \"target\": \"{h}\"}}");
    let play: Value = serde_json::from_str(&play).unwrap();
    // Alone: the first rule.
    let alone = ok(&mut app, "hitstop_play", play.clone());
    assert!(alone.starts_with("slow dodge 0.10 s (perfect_evaded)"), "{alone}");
    // Hosting is co-op even with nobody joined, and a client is in co-op: the rule is passed over.
    for role in [NetRole::Host, NetRole::Client] {
        app.insert_resource(role);
        let coop = ok(&mut app, "hitstop_play", play.clone());
        assert!(coop.starts_with("dodge 0.04 s (perfect_evaded): pictures held"), "{role:?}: {coop}");
    }
}

#[test]
fn the_play_tool_says_when_the_only_rule_is_held_to_solo_play() {
    let assets = Folder::new("solo_only");
    assets.put(
        "content/feel/test.hitstop.ron",
        r#"{ "t": (max_seconds: 0.12, rules: [(name: "slow dodge", when: (event: PerfectEvaded, solo: true), seconds: 0.10)]) }"#,
    );
    let mut app = drawn(&assets);
    app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    app.update();
    sim::resume(app.world_mut());
    for _ in 0..10 {
        app.update();
    }
    let play = json!({"event": "perfect_evaded"});
    assert!(ok(&mut app, "hitstop_play", play.clone()).starts_with("slow dodge 0.10 s"));
    app.insert_resource(NetRole::Host);
    let coop = ok(&mut app, "hitstop_play", play.clone());
    assert!(coop.contains("only in solo play") && coop.contains("in co-op"), "{coop}");
    // A rule held to solo play and to blows of the local player: the probe counts the local
    // player in the blow, as a real blow does.
    let assets = Folder::new("solo_local");
    assets.put(
        "content/feel/test.hitstop.ron",
        r#"{ "t": (max_seconds: 0.12, rules: [(name: "own dodge", when: (event: PerfectEvaded, solo: true, involves_local: true), seconds: 0.10)]) }"#,
    );
    let mut local = drawn(&assets);
    let hero = fighter(local.world_mut(), "hero", "heroes", Vec3::ZERO, Vec3::NEG_Z, 100.0);
    local.world_mut().entity_mut(hero).insert(LocalPlayer);
    local.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
    local.update();
    sim::resume(local.world_mut());
    for _ in 0..10 {
        local.update();
    }
    local.insert_resource(NetRole::Host);
    let own = json!({"event": "perfect_evaded", "target": id(&local, hero).to_string()});
    let said = ok(&mut local, "hitstop_play", own);
    assert!(said.contains("only in solo play"), "{said}");
    // An event no rule names is still just missing.
    let other = ok(&mut app, "hitstop_play", json!({"event": "parried"}));
    assert!(other.contains("no rule of the table gives a hold for parried"), "{other}");
}
