//! What counts as out (7.5): every death advances a wave, whatever caused it; an add never holds
//! a wave open; the adds are released on a win.

use shards::prelude::*;
use serde_json::json;
use shards::encounter::RunState;

use crate::common::*;

const FAR: Vec3 = Vec3::new(0.0, 0.0, 40.0);

/// `manual`, started: wave 1 (one member) running, a player standing near.
fn fight(app: &mut App) -> (String, Entity) {
    let p0 = player(app, 0, Vec3::new(0.0, 0.0, 6.0));
    let key = place(app, "manual", "m", Vec3::ZERO);
    step(app, 2);
    ok(app, "encounter_start", json!({"key": key}));
    run_until(app, 120, |a| run(a, &key).wave == 1);
    (key, p0)
}

fn advanced(app: &App, key: &str) -> bool {
    matches!(state(app, key), RunState::Between { .. }) && run(app, key).wave == 1
}

#[test]
fn a_dead_component_ends_the_wave_whoever_put_it_there() {
    // A hit with a killer, a status, a fall and a kill volume all end with `Dead`.
    let mut app = app(1);
    let (key, _p0) = fight(&mut app);
    let (_, member) = members(&app, &key)[0];
    kill(&mut app, member);
    tick(&mut app);
    assert!(advanced(&app, &key), "{:?}", state(&app, &key));
    assert!(run(&app, &key).members.is_empty(), "the corpse is pruned from the list");
    assert!(app.world().get::<shards::encounter::EncounterMember>(member).is_none(), "and loses its marker");
}

#[test]
fn a_dev_despawn_ends_the_wave() {
    let mut app = app(1);
    let (key, _p0) = fight(&mut app);
    let (id, _) = members(&app, &key)[0];
    ok(&mut app, "despawn", json!({"id": id.to_string()}));
    tick(&mut app);
    assert!(advanced(&app, &key), "{:?}", state(&app, &key));
}

#[test]
fn an_add_never_holds_a_wave_open_and_a_counted_one_does() {
    let mut app = app(1);
    let (key, _p0) = fight(&mut app);
    let (_, member) = members(&app, &key)[0];
    let add = app.world_mut().spawn((AiKind::new("Add"), Name::new("Add"))).id();
    shards::encounter::adopt(app.world_mut(), &key, add, false).expect("adopted");
    let held = app.world_mut().spawn((AiKind::new("Held"), Name::new("Held"))).id();
    shards::encounter::adopt(app.world_mut(), &key, held, true).expect("adopted");
    assert_eq!(run(&app, &key).members.len(), 3);
    kill(&mut app, member);
    tick(&mut app);
    assert_eq!(state(&app, &key), RunState::Running, "the counted adopt holds the wave");
    kill(&mut app, held);
    tick(&mut app);
    assert!(advanced(&app, &key), "{:?}", state(&app, &key));
    assert_eq!(run(&app, &key).members.len(), 1, "the living add stays listed through the clear");
    assert!(app.world().get::<shards::encounter::EncounterMember>(add).is_some());
}

#[test]
fn the_adds_are_released_on_a_win() {
    let mut app = app(1);
    let (key, _p0) = fight(&mut app);
    let add = app.world_mut().spawn((AiKind::new("Add"), Name::new("Add"))).id();
    shards::encounter::adopt(app.world_mut(), &key, add, false).expect("adopted");
    for wave in 1..=2u32 {
        run_until(&mut app, 300, |a| run(a, &key).wave == wave);
        for (_, e) in members(&app, &key) {
            if e != add {
                kill(&mut app, e);
            }
        }
        tick(&mut app);
    }
    assert_eq!(state(&app, &key), RunState::Won);
    assert!(app.world().get_entity(add).is_ok(), "released, not despawned");
    assert!(app.world().get::<shards::encounter::EncounterMember>(add).is_none());
    assert!(run(&app, &key).members.is_empty());
}

#[test]
fn adopting_teaches_the_fix() {
    let mut app = app(1);
    let (key, _p0) = fight(&mut app);
    let ghost = app.world_mut().spawn(Name::new("Ghost")).id();
    let text = shards::encounter::adopt(app.world_mut(), &key, ghost, false).unwrap_err();
    assert!(text.contains("no AiId (give it AiKind)"), "{text}");
    let text = shards::encounter::adopt(app.world_mut(), "world/mm", ghost, false).unwrap_err();
    assert!(text.contains("no encounter run `world/mm`") && text.contains("did you mean `world/m`?"), "{text}");
}

#[test]
fn a_member_gone_from_the_world_counts_as_out() {
    let mut app = app(1);
    let (key, _p0) = fight(&mut app);
    let (_, member) = members(&app, &key)[0];
    app.world_mut().despawn(member);
    tick(&mut app);
    assert!(advanced(&app, &key));
    let _ = FAR;
}

#[test]
fn a_prefab_without_an_ai_id_is_said_and_never_listed() {
    let mut app = app(1);
    let db = db_edited(
        &app,
        &[(
            "ghost.encounter.ron",
            r#"{ "haunt": (radius: 10.0, start: Manual, waves: [(members: [(prefab: "ghost", at: Offsets([(0.0, 0.0, -3.0)]))])]) }"#,
        )],
    );
    shards::content::install(app.world_mut(), db);
    let key = place(&mut app, "haunt", "h", Vec3::ZERO);
    step(&mut app, 2);
    ok(&mut app, "encounter_start", json!({"key": key}));
    step(&mut app, 3);
    assert!(journal_has(&app, "has no AiId (give it AiKind)"), "{:?}", journal(&app));
}
