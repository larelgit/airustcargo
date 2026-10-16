//! The `render` feature's systems in a headless arena that has what they need and nothing
//! more: the gizmo plugin, a `Window` entity, a camera whose size is filled in by hand. The
//! GPU-free half of a window: what is handed to the gizmos, and the UI nodes of the markers.

use bevy::camera::{CameraProjection, ComputedCameraValues, RenderTargetInfo};
use bevy::gizmos::GizmoPlugin;
use bevy::prelude::*;
use bevy::transform::TransformPlugin;
use bevy::window::Window;
use serde_json::json;
use shards_action::render::overlay::CombatDraw;
use shards_action::render::CombatOverlay;
use shards_core::LocalPlayer;

use crate::common::*;

const SCREEN: UVec2 = UVec2::new(1280, 720);

/// A window's worth of an arena: gizmos and transforms, a `Window`, the hero as the local player
/// and a camera at `eye` looking at `target`. The brute stands 2 m north of the hero.
fn windowed(eye: Vec3, target: Vec3) -> (App, Entity, Entity, Entity) {
    let mut app = app_with_plugins(
        |app| {
            // The assets the gizmo plugin's helper systems (bounds, frusta) look at.
            app.init_asset::<Mesh>().init_asset::<bevy::mesh::skinning::SkinnedMeshInverseBindposes>();
            app.add_plugins((TransformPlugin, GizmoPlugin));
        },
        |_| {},
    );
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let brute = brute(world, Vec3::new(0.0, 0.0, -2.0), S);
    world.entity_mut(hero).insert(LocalPlayer);
    world.spawn(Window::default());
    let projection = PerspectiveProjection { aspect_ratio: SCREEN.x as f32 / SCREEN.y as f32, ..default() };
    let camera = Camera {
        computed: ComputedCameraValues {
            clip_from_view: projection.get_clip_from_view(),
            target_info: Some(RenderTargetInfo { physical_size: SCREEN, scale_factor: 1.0 }),
            ..default()
        },
        ..default()
    };
    let camera = world
        .spawn((Camera3d::default(), camera, Projection::Perspective(projection), Transform::from_translation(eye).looking_at(target, Vec3::Y)))
        .id();
    (app, hero, brute, camera)
}

/// Frames the window would draw: the world's transforms settle, the systems run. No system
/// may fail meanwhile (a failure would also be reported by whichever test ticks next: the
/// failure queue is global).
fn frames(app: &mut App, n: usize) {
    for _ in 0..n {
        app.update();
    }
    let errors: Vec<String> = app
        .world()
        .resource::<ai_core::AiJournal>()
        .after(0)
        .0
        .filter(|e| e.kind == ai_core::JournalKind::Error)
        .map(|e| e.to_string())
        .collect();
    assert!(errors.is_empty(), "{errors:#?}");
}

fn drawn(app: &App) -> (usize, usize) {
    let draw = app.world().resource::<CombatDraw>();
    (draw.telegraphs.0.len(), draw.overlay.0.len())
}

#[test]
fn a_telegraph_is_handed_to_the_gizmos_every_frame_and_the_overlay_only_when_asked() {
    let (mut app, _hero, brute, _camera) = windowed(Vec3::new(0.0, 6.0, 9.0), Vec3::new(0.0, 0.0, -1.0));
    frames(&mut app, 2);
    assert_eq!(drawn(&app), (0, 0), "nothing to warn about");

    press(&mut app, brute, "heavy");
    step(&mut app, 3);
    frames(&mut app, 2);
    let (telegraph, overlay) = drawn(&app);
    assert!(telegraph > 10, "the slam's warning: {telegraph}");
    assert_eq!(overlay, 0, "the overlay is off until asked for");

    let said = ok(&mut app, "combat_overlay", json!({}));
    assert!(said.starts_with("combat overlay on (drawn in the window or offscreen only): "), "{said}");
    assert!(said.contains("2 hurtboxes") && said.contains("1 zone"), "{said}");
    frames(&mut app, 2);
    assert!(drawn(&app).1 > 20, "hurtboxes, the zone and its label: {:?}", drawn(&app));

    assert_eq!(ok(&mut app, "combat_overlay", json!({"on": false})), "combat overlay off");
    frames(&mut app, 2);
    assert_eq!(drawn(&app).1, 0);
    assert!(drawn(&app).0 > 10, "telegraphs stay on");
}

#[test]
fn f2_toggles_the_overlay() {
    let (mut app, ..) = windowed(Vec3::new(0.0, 6.0, 9.0), Vec3::ZERO);
    app.init_resource::<ButtonInput<KeyCode>>();
    frames(&mut app, 1);
    assert!(!app.world().resource::<CombatOverlay>().enabled);
    app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::F2);
    frames(&mut app, 1);
    assert!(app.world().resource::<CombatOverlay>().enabled);
    app.world_mut().resource_mut::<ButtonInput<KeyCode>>().clear();
    frames(&mut app, 1);
    assert!(app.world().resource::<CombatOverlay>().enabled, "one press, one toggle");
    app.world_mut().resource_mut::<CombatOverlay>().key = None;
    app.world_mut().resource_mut::<ButtonInput<KeyCode>>().release(KeyCode::F2);
    app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::F2);
    frames(&mut app, 1);
    assert!(app.world().resource::<CombatOverlay>().enabled, "a game can unbind it");
}

#[test]
fn without_a_window_nothing_is_drawn() {
    // The same arena minus the window, like a headless engine run with the render plugins.
    let (mut app, _hero, brute, camera) = windowed(Vec3::new(0.0, 6.0, 9.0), Vec3::ZERO);
    let window = app.world_mut().query_filtered::<Entity, With<Window>>().iter(app.world()).next().unwrap();
    app.world_mut().despawn(window);
    app.world_mut().despawn(camera);
    press(&mut app, brute, "heavy");
    step(&mut app, 3);
    frames(&mut app, 2);
    assert_eq!(drawn(&app), (0, 0));
    ok(&mut app, "combat_overlay", json!({}));
    frames(&mut app, 2);
    assert_eq!(drawn(&app), (0, 0));
}

/// The text of every marker and where its node sits: `(text, left, top, shown)`.
fn markers(app: &mut App) -> Vec<(String, f32, f32, bool)> {
    let world = app.world_mut();
    let mut q = world.query::<(&Text, &ChildOf)>();
    let pairs: Vec<(String, Entity)> = q.iter(world).map(|(t, p)| (t.0.clone(), p.parent())).collect();
    pairs
        .into_iter()
        .filter_map(|(text, parent)| {
            let node = world.get::<Node>(parent)?;
            let px = |v: Val| if let Val::Px(p) = v { p } else { f32::NAN };
            Some((text, px(node.left), px(node.top), node.display != Display::None))
        })
        .collect()
}

#[test]
fn an_attack_behind_the_camera_gets_a_marker_on_the_bottom_edge_until_it_is_in_view() {
    // The camera looks south; the brute stands north of the hero, behind it.
    let (mut app, _hero, brute, camera) = windowed(Vec3::new(0.0, 3.0, 4.0), Vec3::new(0.0, 1.0, 10.0));
    press(&mut app, brute, "heavy");
    step(&mut app, 3);
    frames(&mut app, 2);
    let shown = markers(&mut app);
    assert_eq!(shown.len(), 1, "{shown:?}");
    let (text, left, top, visible) = &shown[0];
    assert_eq!(text, "!", "the warning symbol, as ASCII for the default font");
    assert!(*visible);
    // Centered on the bottom edge, inset by the margin: the node is `size` wide.
    let size = 52.0 * (0.85 + 0.35 * shards_action::draw::intensity(0.7, 3.0).max(0.0)) ;
    assert!((left + size * 0.5 - 640.0).abs() < 8.0 && (top + size * 0.5 - (720.0 - 56.0)).abs() < 8.0, "{left} {top}");

    // The player turns to face it: the zone is in view, so no marker.
    let transform = Transform::from_xyz(0.0, 3.0, 4.0).looking_at(Vec3::new(0.0, 0.0, -2.0), Vec3::Y);
    app.world_mut().entity_mut(camera).insert(transform);
    frames(&mut app, 2);
    let shown = markers(&mut app);
    assert_eq!(shown.len(), 1, "the pooled marker stays");
    assert!(!shown[0].3, "hidden: {shown:?}");

    // It turns away to the east: the marker moves round to the left of the screen.
    let transform = Transform::from_xyz(0.0, 3.0, 4.0).looking_at(Vec3::new(8.0, 1.0, 4.0), Vec3::Y);
    app.world_mut().entity_mut(camera).insert(transform);
    frames(&mut app, 2);
    let shown = markers(&mut app);
    assert!(shown[0].3 && shown[0].1 < 100.0, "to the left: {shown:?}");

    // The slam is over.
    step(&mut app, 80);
    frames(&mut app, 2);
    assert!(markers(&mut app).iter().all(|m| !m.3), "no threat, no markers");
}

#[test]
fn friendly_attacks_raise_no_marker() {
    let (mut app, hero, _brute, _camera) = windowed(Vec3::new(0.0, 3.0, 4.0), Vec3::new(0.0, 1.0, 10.0));
    press(&mut app, hero, "heavy");
    step(&mut app, 3);
    frames(&mut app, 2);
    assert!(markers(&mut app).is_empty(), "your own swing is no threat to you");
}
