//! A real window (software Vulkan under Xvfb), by hand: `--ignored`.

use std::time::Duration;

use shards::action::render::{CombatOverlay, ThreatHud};
use shards::bevy::render::view::screenshot::{save_to_disk, Screenshot};
use shards::bevy::winit::WinitPlugin;
use shards::motion::{character, Collider, Shape};
use shards::prelude::*;

use crate::fixtures::assets;

/// Two brutes wind up a slam, one in front of the camera and one behind it; the hero holds a
/// block. The first picture is a fifth of a second before the front slam lands: its ground
/// telegraph with the warning symbol, the overlay's zone, hurtboxes and the hero's guard arc,
/// and a marker on the bottom edge for the brute behind the camera. The second is the moment
/// the slam is active, with the hero in the i-frames of a roll: the red hitbox, the outline.
///
/// Ignored: it needs a display and a Vulkan driver. Look at `shot-windup.png` and
/// `shot-active.png` in the folder it prints.
#[test]
#[ignore = "needs a display and a Vulkan driver: xvfb-run -a cargo test -p shards --test action_render -- --ignored"]
fn the_window_shows_telegraphs_overlay_and_markers() {
    let dir = assets("action_render_window");
    let (first, second) = (dir.join("shot-windup.png"), dir.join("shot-active.png"));
    let config = EngineConfig {
        title: "action render".into(),
        headless: false,
        testing: false,
        paused: true,
        port: None,
        crash_replays: false,
        hot_reload: false,
        asset_dir: dir.to_string_lossy().into_owned(),
        window: (960, 540),
        ..default()
    };
    let mut app = App::new();
    app.add_plugins(EnginePlugins::new(config).build().set(WinitPlugin { run_on_any_thread: true }));
    app.insert_resource(CombatOverlay { enabled: true, ..default() });
    app.insert_resource(ThreatHud::default());
    app.add_systems(OnEnter(EngineState::InGame), arena);

    let (paths, mut frame) = ((first.clone(), second.clone()), 0u32);
    app.add_systems(Update, move |world: &mut World| {
        if *world.resource::<State<EngineState>>().get() != EngineState::InGame {
            return;
        }
        frame += 1;
        let step = |world: &mut World, ticks: u64| {
            shards::ai_core::sim::step(world, ticks, Duration::from_secs(20), |_| None).unwrap();
        };
        let intents = |world: &mut World, f: &dyn Fn(&str, &mut Intent)| {
            let mut q = world.query::<(&Name, &mut Intent)>();
            for (name, mut intent) in q.iter_mut(world) {
                f(name.as_str(), &mut intent);
            }
        };
        match frame {
            // Both brutes wind up, the hero raises his guard; 36 ticks on, the slams are 0.2 s
            // from landing.
            3 => {
                intents(world, &|name, intent| match name {
                    "hero" => intent.set_held("block", true),
                    _ => intent.press("light"),
                });
                step(world, 36);
            }
            // Pipelines take a while to build in software; then the picture.
            120 => {
                world.spawn(Screenshot::primary_window()).observe(save_to_disk(paths.0.clone()));
            }
            // Four ticks later the hero lets go and rolls away; ten ticks after that the slam is
            // active and the roll is in its i-frames.
            200.. if paths.0.exists() && frame < 1000 => {
                intents(world, &|name, intent| {
                    if name == "hero" {
                        intent.set_held("block", false);
                    }
                });
                step(world, 4);
                intents(world, &|name, intent| {
                    if name == "hero" {
                        intent.press_dir("dodge", Vec3::Z);
                    }
                });
                step(world, 10);
                frame = 1000;
            }
            1030 => {
                world.spawn(Screenshot::primary_window()).observe(save_to_disk(paths.1.clone()));
            }
            1100.. if paths.1.exists() => {
                world.write_message(AppExit::Success);
            }
            1400 => panic!("no screenshot arrived"),
            _ => {}
        }
    });
    let exit = app.run();
    assert!(exit.is_success(), "{exit:?}");
    for shot in [&first, &second] {
        assert!(shot.metadata().unwrap().len() > 5_000, "{} is a picture, not an empty file", shot.display());
    }
    eprintln!("pictures: {} and {}", first.display(), second.display());
}

/// The ground, a camera, a light, the hero (the local player) and two brutes, as capsules.
fn arena(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let ground =
        materials.add(StandardMaterial { base_color: Color::srgb(0.35, 0.4, 0.35), perceptual_roughness: 1.0, ..default() });
    commands.spawn((
        Collider::new(Shape::cuboid(Vec3::new(30.0, 0.5, 30.0))),
        Mesh3d(meshes.add(Cuboid::new(60.0, 1.0, 60.0))),
        MeshMaterial3d(ground),
        Transform::from_xyz(0.0, -0.5, 0.0),
    ));
    commands.spawn((
        Camera3d::default(),
        AiCamera,
        Transform::from_xyz(0.0, 7.0, 8.0).looking_at(Vec3::new(0.0, 0.0, -2.0), Vec3::Y),
    ));
    commands.spawn((DirectionalLight::default(), Transform::from_xyz(3.0, 8.0, 5.0).looking_at(Vec3::ZERO, Vec3::Y)));
    let body = meshes.add(Capsule3d::new(0.4, 1.0));
    let mut fighter = |name: &'static str, team: &'static str, at: Vec3, facing: Vec3, color: Color| {
        let (controller, intent, transform) = character("", at);
        let mut entity = commands.spawn((
            Name::new(name),
            AiKind::new(name),
            Fighter::new(if team == "heroes" { "hero" } else { "brute" }),
            Team::new(team),
            Health::new(100.0),
            Stamina::new(100.0, 30.0, 0.8),
            controller,
            intent,
            transform.looking_to(facing, Vec3::Y),
            Visibility::default(),
        ));
        entity.with_child((Mesh3d(body.clone()), MeshMaterial3d(materials.add(color)), Transform::from_xyz(0.0, 0.9, 0.0)));
        if team == "heroes" {
            entity.insert((AiPlayer, LocalPlayer));
        }
    };
    fighter("hero", "heroes", Vec3::new(0.0, 0.0, 3.0), Vec3::NEG_Z, Color::srgb(0.3, 0.5, 0.9));
    fighter("front", "monsters", Vec3::new(0.0, 0.0, -2.0), Vec3::Z, Color::srgb(0.8, 0.3, 0.2));
    fighter("behind", "monsters", Vec3::new(2.0, 0.0, 12.0), Vec3::Z, Color::srgb(0.8, 0.3, 0.2));
}
