//! What agents see and do: the NPC's line, `npc_state`, `nav_path`, `npc_set_state`.

use bevy::prelude::*;
use serde_json::json;

use crate::common::*;

#[test]
fn npc_state_tells_the_mind_and_the_line_reads_short() {
    let mut app = app();
    let hero = player(app.world_mut(), 0, Vec3::ZERO);
    let g = grunt(app.world_mut(), "grunt", "grunt", "default", Vec3::new(0.0, 0.0, -6.0));
    home(app.world_mut(), g, Vec3::new(0.0, 0.0, -6.0), 15.0);
    let (gid, hid) = (id(&app, g), id(&app, hero));
    let mut with_token = None;
    for _ in 0..300 {
        step(&mut app, 1);
        if has_token(&app, g) {
            with_token = Some(shards_npc::ai::describe_npc(app.world().entity(g)).unwrap());
            break;
        }
    }
    let line = with_token.expect("it got a token");
    assert!(line.starts_with(&format!("fight {hid} · token · leash ")) && line.ends_with("/15m"), "{line}");

    let out = ok(&mut app, "npc_state", json!({"id": gid}));
    let s = &out.result;
    assert_eq!(s["state"], "fight");
    assert_eq!(s["behavior"], "grunt");
    assert_eq!(s["target"], hid);
    assert_eq!(s["token"]["target"], hid);
    assert_eq!(s["perceived"][0]["id"], hid);
    assert_eq!(s["perceived"][0]["seen"], true);
    assert_eq!(s["home"]["leash"], 15.0);
    assert!(s["plan"]["history"].is_array() && s["nav"].is_object(), "{s}");

    let err = fails(&mut app, "npc_state", json!({"id": hid}));
    assert!(err.contains("is not an NPC"), "{err}");
}

#[test]
fn npc_overlay_answers_a_line_per_npc_near_the_players() {
    let mut app = app();
    let hero = player(app.world_mut(), 0, Vec3::ZERO);
    let g = grunt(app.world_mut(), "grunt", "grunt", "default", Vec3::new(0.0, 0.0, -6.0));
    home(app.world_mut(), g, Vec3::new(0.0, 0.0, -6.0), 15.0);
    // Far from the player: no line for it.
    let far = grunt(app.world_mut(), "grunt", "grunt", "default", Vec3::new(0.0, 0.0, 80.0));
    let (gid, hid, fid) = (id(&app, g), id(&app, hero), id(&app, far));
    for _ in 0..300 {
        step(&mut app, 1);
        if has_token(&app, g) {
            break;
        }
    }
    // A fallen player's body is nobody to be near: the far grunt stays out.
    let body = player(app.world_mut(), 1, Vec3::new(0.0, 0.0, 78.0));
    app.world_mut().entity_mut(body).insert(shards_action::Dead);
    let out = ok(&mut app, "npc_overlay", json!({}));
    let lines: Vec<String> = text(&out).lines().map(str::to_owned).collect();
    assert_eq!(lines[0], "npc overlay on");
    assert_eq!(lines.len(), 2, "{lines:?}");
    let line = &lines[1];
    assert!(line.starts_with(&format!("{gid} grunt: fight {hid} · token · leash ")), "{line}");
    assert!(line.contains(&format!(" · sees {hid} ")), "{line}");
    assert!(!text(&out).contains(&fid.to_string()), "{}", text(&out));
    assert!(app.world().resource::<shards_npc::NpcOverlay>().enabled);
    // The hero out of its sight: it still knows where it was a moment ago.
    teleport(&mut app, hero, Vec3::new(0.0, 0.0, -30.0));
    step(&mut app, 10);
    let lost = text(&ok(&mut app, "npc_overlay", json!({})));
    assert!(lost.contains(&format!("lost {hid} 0.")) && lost.contains("s ago at "), "{lost}");
    assert_eq!(text(&ok(&mut app, "npc_overlay", json!({"on": false}))), "npc overlay off");
    assert!(!app.world().resource::<shards_npc::NpcOverlay>().enabled);
}

#[test]
fn npc_set_state_puts_it_there_or_lists_the_states() {
    let mut app = app();
    let g = grunt(app.world_mut(), "grunt", "grunt", "default", Vec3::new(0.0, 0.0, -6.0));
    step(&mut app, 2);
    let gid = id(&app, g);
    let out = ok(&mut app, "npc_set_state", json!({"id": gid, "state": "fight"}));
    assert_eq!(text(&out), format!("ok: {gid} idle → fight"));
    assert!(out.journal.iter().any(|l| l.contains(&format!("{gid} idle → fight (npc_set_state)"))), "{:?}", out.journal);
    assert_eq!(state(&app, g), "fight");
    let err = fails(&mut app, "npc_set_state", json!({"id": gid, "state": "figth"}));
    assert_eq!(err, format!("{gid}: behavior `grunt` has no state `figth` (did you mean `fight`?); states: fight, idle"));
    // No target to fight: its transition takes it back the next tick.
    step(&mut app, 1);
    assert_eq!(state(&app, g), "idle");
}

#[test]
fn agents_step_until_the_npcs_attack_or_calm_down() {
    let mut app = app();
    let hero = player(app.world_mut(), 0, Vec3::ZERO);
    let g = grunt(app.world_mut(), "grunt", "jabber", "default", Vec3::new(0.0, 0.0, -6.0));
    let run = ok(&mut app, "step", json!({"ticks": 600, "until": [{"named": "npc_attacking"}]}));
    assert!(text(&run).contains("stopped: condition npc_attacking"), "{}", text(&run));
    assert!(attacking(&app, g) || action(&app, g).is_some(), "it swings");
    // The hero goes far away for good: the grunt forgets it.
    teleport(&mut app, hero, Vec3::new(0.0, 0.0, 50.0));
    let run = ok(&mut app, "step", json!({"ticks": 900, "until": [{"named": "npcs_calm"}]}));
    assert!(text(&run).contains("stopped: condition npcs_calm"), "{}", text(&run));
    assert_eq!(state(&app, g), "idle");
}

#[test]
fn nav_path_answers_with_the_route_around_a_wall() {
    let mut app = app();
    player(app.world_mut(), 0, Vec3::new(-4.0, 0.0, 0.0));
    wall(app.world_mut(), Vec3::new(-0.5, 0.0, -3.0), Vec3::new(0.5, 3.0, 3.0));
    let out = ok(&mut app, "nav_path", json!({"to": {"x": 4.0, "z": 0.0}}));
    let r = &out.result;
    assert_eq!(r["provider"], "local grid");
    let distance = r["distance"].as_f64().unwrap();
    assert!(distance > 9.0, "around the 6 m wall, not through it: {r}");
    assert!(r["waypoints"].as_array().unwrap().len() > 1, "{r}");
    let last = &r["waypoints"].as_array().unwrap().last().unwrap().clone();
    assert_eq!((last["x"].as_f64(), last["z"].as_f64()), (Some(4.0), Some(0.0)));
    let err = fails(&mut app, "nav_path", json!({"to": {"x": 0.0, "z": 0.0}, "radius": 0.4}));
    assert!(err.starts_with("no route from (-4.0, 0.0) to (0.0, 0.0)"), "{err}");
}
