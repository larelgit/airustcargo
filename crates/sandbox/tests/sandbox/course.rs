//! The obstacle course (`assets/content/world/course.layout.ron`) is a course only if no
//! plain jump skips an obstacle and the gap needs the glide and its updraft. The way
//! through is `scenarios/obstacle_course.scenario`; here, the ways that must fail.

use shards::motion::{Volume, VolumeKind};
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::*;

/// The player at `(x, y, 0)` facing east, standing, the skeletons and the warden calm.
fn at(x: f32, y: f32) -> App {
    let mut app = sandbox::test_app(42);
    calm_skeletons(&mut app);
    calm_warden(&mut app);
    ok(&mut app, "teleport", json!({"id": PLAYER, "x": x, "z": 0, "y": y}));
    step(&mut app, 10);
    app
}

fn feet(app: &mut App) -> Vec3 {
    player::<Transform>(app).translation
}

#[test]
fn a_running_jump_does_not_clear_the_pool() {
    let mut app = at(52.5, 3.3);
    ok(&mut app, "sprint", json!({}));
    ok(&mut app, "move", json!({"dir": "E", "meters": 20}));
    // Jump at the edge of the deck (x = 56), at full sprint.
    let at_edge = json!({"field": {"id": PLAYER, "component": "Transform", "path": "translation.x", "above": 55.4}});
    let edge = ok(&mut app, "step", json!({"ticks": 120, "until": [at_edge]}));
    assert!(text(&edge).contains("stopped"), "{}", text(&edge));
    assert_eq!(text(&ok(&mut app, "jump", json!({}))), "ok: jumping");
    let out = ok(&mut app, "step", json!({"ticks": 120, "until": [{"journal": "→ water"}]}));
    assert!(text(&out).contains("stopped: journal:"), "{}", text(&out));
    assert!(feet(&mut app).x < 64.0, "lands in the water, short of the far deck at 66: {}", feet(&mut app));
}

#[test]
fn a_jump_off_the_cliff_without_a_glide_falls_into_the_canyon() {
    let mut app = at(84.5, 8.0);
    ok(&mut app, "sprint", json!({}));
    ok(&mut app, "move_to", json!({"target": "e13"}));
    ok(&mut app, "step", json!({"ticks": 12}));
    ok(&mut app, "jump", json!({}));
    let out = ok(&mut app, "step", json!({"ticks": 600, "until": [{"journal": "e5 landed"}]}));
    assert!(text(&out).contains("stopped: journal:"), "{}", text(&out));
    assert!(feet(&mut app).y < -9.0, "down on the canyon floor: {}", feet(&mut app));
}

#[test]
fn a_glide_without_the_updraft_falls_short_of_the_goal() {
    let mut app = at(84.5, 8.0);
    // The same gap with the updraft gone.
    let world = app.world_mut();
    let updraft: Vec<Entity> = world
        .query::<(Entity, &Volume)>()
        .iter(world)
        .filter(|(_, v)| matches!(v.kind, VolumeKind::Updraft { .. }))
        .map(|(e, _)| e)
        .collect();
    assert_eq!(updraft.len(), 1);
    world.despawn(updraft[0]);
    ok(&mut app, "glide", json!({}));
    ok(&mut app, "move_to", json!({"target": "e13"}));
    ok(&mut app, "jump", json!({}));
    let out =
        ok(&mut app, "step", json!({"ticks": 900, "until": [{"journal": "glide → ground"}, {"journal": "e5 landed"}]}));
    assert!(text(&out).contains("stopped: journal:"), "{}", text(&out));
    step(&mut app, 120);
    assert!(feet(&mut app).y < -9.0, "short of the rock, down in the canyon: {}", feet(&mut app));
    assert!(lines(&app, "reached goal").is_empty());
}

#[test]
fn the_glide_with_the_updraft_lands_on_the_goal_rock() {
    let mut app = at(84.5, 8.0);
    ok(&mut app, "glide", json!({}));
    ok(&mut app, "move_to", json!({"target": "e13"}));
    let out = ok(&mut app, "step", json!({"ticks": 900, "until": [{"journal": "glide → ground"}]}));
    assert!(text(&out).contains("stopped: journal:"), "{}", text(&out));
    let landed = feet(&mut app);
    assert!(landed.y > 5.9 && landed.x > 118.0 && landed.x < 136.0, "on the rock: {landed}");
    assert_eq!(lines(&app, "e5 reached goal").len(), 1, "{:?}", events(&app));
}
