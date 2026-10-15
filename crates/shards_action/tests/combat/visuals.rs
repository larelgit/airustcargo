//! What the render side puts on screen, checked as data in a headless arena: ground
//! telegraphs, the combat overlay, sound cues, off-screen markers. (The systems that hand
//! these lists to the gizmos live behind the `render` feature.)

use bevy::prelude::*;
use shards_action::draw::{edge_marker, overlay, telegraphs, OverlayParts, Prim, TelegraphStyle};
use shards_action::{threat_indicators, SoundCue};
use shards_core::LocalPlayer;

use crate::common::*;

#[derive(Resource, Default)]
struct Heard(Vec<SoundCue>);

fn listening(app: &mut App) {
    app.init_resource::<Heard>();
    app.add_observer(|cue: On<SoundCue>, mut heard: ResMut<Heard>| heard.0.push(cue.event().clone()));
}

/// The brute 2 m north of the hero, facing it; the hero faces the brute.
fn duel() -> (App, Entity, Entity) {
    let mut app = app_with(listening);
    let world = app.world_mut();
    let hero = hero(world, "hero", Vec3::ZERO, N);
    let brute = brute(world, Vec3::new(0.0, 0.0, -2.0), S);
    (app, hero, brute)
}

fn all_points(list: &shards_action::draw::DrawList) -> Vec<Vec3> {
    list.0
        .iter()
        .flat_map(|p| match p {
            Prim::Strip { points, .. } => points.clone(),
            Prim::Line { a, b, .. } => vec![*a, *b],
            Prim::Text { at, .. } => vec![*at],
        })
        .collect()
}

fn texts(list: &shards_action::draw::DrawList) -> Vec<String> {
    list.0
        .iter()
        .filter_map(|p| match p {
            Prim::Text { text, .. } => Some(text.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn a_telegraph_shows_on_the_ground_until_the_hit_is_over() {
    let (mut app, hero, brute) = duel();
    app.world_mut().entity_mut(hero).insert(LocalPlayer);
    let style = TelegraphStyle::default();
    assert!(telegraphs(app.world(), &style, 0.0, Quat::IDENTITY).is_empty(), "nothing before the windup");

    press(&mut app, brute, "heavy");
    step(&mut app, 1);
    let early = telegraphs(app.world(), &style, 0.0, Quat::IDENTITY);
    let (strips, lines, _) = early.counts();
    assert!(strips >= 2 && lines > 3, "outline, fill ring, hatch, symbol: {:?}", early.counts());
    // The slam is a 4.5 m circle around the brute at z = -2, drawn at the floor.
    let on_floor: Vec<Vec3> = all_points(&early).into_iter().filter(|p| p.y < 0.5).collect();
    let (lo, hi) = on_floor.iter().fold((Vec3::MAX, Vec3::MIN), |(l, h), p| (l.min(*p), h.max(*p)));
    assert!((hi.x - 4.5).abs() < 0.1 && (lo.x + 4.5).abs() < 0.1, "{lo} {hi}");
    assert!((hi.z - 2.5).abs() < 0.1 && (lo.z + 6.5).abs() < 0.1, "{lo} {hi}");
    // The symbol floats above it.
    assert!(all_points(&early).iter().any(|p| p.y > 2.0));

    // The nearer the hit, the denser the hatch.
    step(&mut app, 40);
    let late = telegraphs(app.world(), &style, 0.0, Quat::IDENTITY);
    assert!(late.counts().1 > lines * 2, "{lines} -> {}", late.counts().1);

    // Once the slam is over there is nothing to warn about.
    step(&mut app, 60);
    assert!(telegraphs(app.world(), &style, 0.0, Quat::IDENTITY).is_empty());
}

#[test]
fn the_players_own_swings_are_no_warning_but_a_zone_to_the_overlay() {
    let (mut app, hero, brute) = duel();
    app.world_mut().entity_mut(hero).insert(LocalPlayer);
    press(&mut app, hero, "light");
    step(&mut app, 4);
    assert!(telegraphs(app.world(), &TelegraphStyle::default(), 0.0, Quat::IDENTITY).is_empty());
    let (list, counts) = overlay(app.world(), OverlayParts::default(), Quat::IDENTITY);
    assert_eq!(counts.zones, 1, "the hero's swing is a zone to the overlay");
    assert!(texts(&list).iter().any(|t| t.starts_with("light_1 0.")), "{:?}", texts(&list));

    // Seen from the brute's side, the same swing is aimed at the local player: it is drawn
    // on the ground even though the action has no telegraph, with a bang.
    app.world_mut().entity_mut(hero).remove::<LocalPlayer>();
    app.world_mut().entity_mut(brute).insert(LocalPlayer);
    let warned = telegraphs(app.world(), &TelegraphStyle::default(), 0.0, Quat::IDENTITY);
    assert!(warned.counts().0 >= 2 + 4, "outline, fill ring and the bang: {:?}", warned.counts());
    assert!(warned.counts().1 > 2, "hatch: {:?}", warned.counts());
}

#[test]
fn the_overlay_shows_hitboxes_only_while_a_window_is_open() {
    let (mut app, hero, _brute) = duel();
    let off = OverlayParts { hitboxes: false, hurtboxes: false, guards: false, iframes: false, zones: false };
    step(&mut app, 1);
    assert!(overlay(app.world(), off, Quat::IDENTITY).0.is_empty());

    let (_, idle) = overlay(app.world(), OverlayParts::default(), Quat::IDENTITY);
    assert_eq!((idle.hurtboxes, idle.hitboxes, idle.zones), (2, 0, 0), "both bodies are hurtboxes, the floor is not");

    press(&mut app, hero, "light");
    step(&mut app, 8);
    let (_, windup) = overlay(app.world(), OverlayParts::default(), Quat::IDENTITY);
    assert_eq!((windup.hitboxes, windup.zones), (0, 1), "the window is still to come: only its zone shows");

    step(&mut app, 6);
    let (list, open) = overlay(app.world(), OverlayParts::default(), Quat::IDENTITY);
    assert_eq!((open.hitboxes, open.zones), (1, 1));
    assert!(texts(&list).iter().any(|t| t.starts_with("light_1 now")), "{:?}", texts(&list));

    step(&mut app, 10);
    let (_, spent) = overlay(app.world(), OverlayParts::default(), Quat::IDENTITY);
    assert_eq!((spent.hitboxes, spent.zones), (0, 0), "recovery: nothing left to show");
}

#[test]
fn the_overlay_draws_guards_and_i_frames_while_their_windows_are_open() {
    let (mut app, hero, _brute) = duel();
    hold(&mut app, hero, "block", true);
    step(&mut app, 3);
    let (list, c) = overlay(app.world(), OverlayParts::default(), Quat::IDENTITY);
    assert_eq!(c.guards, 1, "{c:?}");
    // The first moments of a block are the perfect window: a second, inner arc.
    let arcs = list.0.iter().filter(|p| matches!(p, Prim::Strip { points, .. } if points.len() > 10 && points[0].y > 0.5 && points[0].y < 1.0)).count();
    assert_eq!(arcs, 2, "perfect guard: two arcs");
    step(&mut app, 12);
    let (list, c) = overlay(app.world(), OverlayParts::default(), Quat::IDENTITY);
    assert_eq!(c.guards, 1);
    let arcs = list.0.iter().filter(|p| matches!(p, Prim::Strip { points, .. } if points.len() > 10 && points[0].y > 0.5 && points[0].y < 1.0)).count();
    assert_eq!(arcs, 1, "after the perfect window: one arc");
    hold(&mut app, hero, "block", false);
    step(&mut app, 30);

    press(&mut app, hero, "dodge");
    step(&mut app, 3);
    let (_, c) = overlay(app.world(), OverlayParts::default(), Quat::IDENTITY);
    assert_eq!(c.iframes, 1, "i-frames of the roll");
    step(&mut app, 20);
    let (_, c) = overlay(app.world(), OverlayParts::default(), Quat::IDENTITY);
    assert_eq!(c.iframes, 0, "the roll's i-frames are over");
}

#[test]
fn god_mode_and_the_dead_show_in_the_overlay() {
    let (mut app, _hero, brute) = duel();
    step(&mut app, 1);
    ok(&mut app, "god", serde_json::json!({}));
    let (list, c) = overlay(app.world(), OverlayParts::default(), Quat::IDENTITY);
    let outlines = list.counts().0;
    assert_eq!(c.iframes, 0, "god mode is no i-frame window");
    assert_eq!(outlines, 3 * 10, "two hurtboxes and a dim outline round the god, ten strokes each");
    let before = outlines;
    // A dead hurtbox is still drawn (grey), so you can see why a hit passes through it.
    shards_action::kill(app.world_mut(), brute, None, None);
    step(&mut app, 1);
    let (list, c) = overlay(app.world(), OverlayParts::default(), Quat::IDENTITY);
    assert_eq!(c.hurtboxes, 2);
    assert_eq!(list.counts().0, before);
}

#[test]
fn telegraph_sounds_become_cues_at_the_zone() {
    let (mut app, _hero, brute) = duel();
    press(&mut app, brute, "light");
    step(&mut app, 1);
    {
        let heard = &app.world().resource::<Heard>().0;
        assert_eq!(heard.len(), 1, "{heard:?}");
        assert_eq!((heard[0].entity, heard[0].cue.as_str()), (brute, "brute_swipe"));
        // The swipe's cone reaches 2.5 m south of the brute at z = -2; its center is half way.
        assert!((heard[0].position - Vec3::new(0.0, 0.0, -0.75)).length() < 0.05, "{}", heard[0].position);
    }
    step(&mut app, 80);
    // A telegraph without a sound makes no cue.
    press(&mut app, brute, "technique.1");
    step(&mut app, 1);
    assert_eq!(app.world().resource::<Heard>().0.len(), 1);
}

#[test]
fn a_threat_behind_the_camera_gets_a_marker_on_the_bottom_edge() {
    let (mut app, hero, brute) = duel();
    press(&mut app, brute, "heavy");
    step(&mut app, 2);
    // The camera looks south, away from the brute.
    let seen = threat_indicators(app.world(), hero, S);
    assert_eq!(seen.len(), 1);
    let marker = edge_marker(seen[0].angle_deg, Vec2::new(1280.0, 720.0), 48.0);
    assert!((marker.position - Vec2::new(640.0, 672.0)).length() < 1.0, "{:?}", marker.position);
    // Looking at it, the marker is at the top.
    let ahead = threat_indicators(app.world(), hero, N);
    let marker = edge_marker(ahead[0].angle_deg, Vec2::new(1280.0, 720.0), 48.0);
    assert!((marker.position - Vec2::new(640.0, 48.0)).length() < 1.0, "{:?}", marker.position);
}
