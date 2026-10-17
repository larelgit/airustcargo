//! Telegraph decals on the headless engine counted as drawn: a decal over every zone the ground
//! shows, its fill growing as the hit nears, gone with the hit; the gizmos leave the fill to them;
//! the cameras get a depth prepass while they are on; off, it all goes back.

use serde_json::json;
use shards::action::draw::palette;
use shards::action::render::TelegraphSettings;
use shards::bevy::core_pipeline::prepass::DepthPrepass;
use shards::bevy::pbr::decal::ForwardDecal;
use shards::fx::{FxPrepass, TelegraphDecal, ZoneDecalMaterial};

use crate::*;

/// The decals shown: their place, scale and material.
fn decals(app: &mut App) -> Vec<(Transform, ZoneDecalMaterialSnapshot)> {
    let world = app.world_mut();
    let mut query = world.query_filtered::<(&Transform, &Visibility, &MeshMaterial3d<ZoneDecalMaterial>), With<TelegraphDecal>>();
    let shown: Vec<(Transform, Handle<ZoneDecalMaterial>)> =
        query.iter(world).filter(|(_, v, _)| **v != Visibility::Hidden).map(|(t, _, m)| (*t, m.0.clone())).collect();
    let materials = world.resource::<Assets<ZoneDecalMaterial>>();
    shown
        .into_iter()
        .map(|(t, m)| {
            let made = &materials.get(&m).unwrap().base;
            (t, ZoneDecalMaterialSnapshot { fill: made.state.x, kind: made.state.z, color: made.color })
        })
        .collect()
}

#[derive(Clone, Copy, Debug)]
struct ZoneDecalMaterialSnapshot {
    fill: f32,
    kind: f32,
    color: LinearRgba,
}

fn linear(c: [f32; 3]) -> LinearRgba {
    Color::srgb(c[0], c[1], c[2]).to_linear()
}

/// The hero (the local player) at the origin, the brute 2 m north facing it, and the AI's camera.
fn duel(assets: &Folder) -> (App, Entity, Entity, Entity) {
    let mut app = drawn(assets);
    let world = app.world_mut();
    let hero = fighter(world, "hero", "heroes", Vec3::ZERO, Vec3::NEG_Z, 100.0);
    world.entity_mut(hero).insert(LocalPlayer);
    let brute = fighter(world, "brute", "monsters", Vec3::new(0.0, 0.0, -2.0), Vec3::Z, 100.0);
    let camera = world.spawn((Camera3d::default(), AiCamera, Transform::from_xyz(0.0, 8.0, 8.0).looking_at(Vec3::ZERO, Vec3::Y))).id();
    app.update();
    (app, hero, brute, camera)
}

#[test]
fn a_telegraph_lays_a_decal_whose_fill_grows_until_the_hit() {
    let assets = Folder::new("slam");
    let (mut app, _hero, brute, camera) = duel(&assets);
    assert!(decals(&mut app).is_empty(), "no attack, no decal");
    assert!(app.world().get::<DepthPrepass>(camera).is_some(), "decals need the depth of the scene");
    assert!(app.world().get::<FxPrepass>(camera).is_some());
    assert!(app.world().resource::<TelegraphSettings>().style.decals, "the gizmos leave the fill to the decals");

    press(&mut app, brute, "light");
    step(&mut app, 1);
    app.update();
    let early = decals(&mut app);
    assert_eq!(early.len(), 1, "{early:?}");
    let (place, made) = early[0];
    // The slam: a 3 m circle round the brute, on the floor; the quad a little larger.
    assert!(place.translation.distance(Vec3::new(0.0, 0.0, -2.0)) < 0.05, "{place:?}");
    assert!((6.0..7.0).contains(&place.scale.x) && (place.scale.x - place.scale.z).abs() < 0.01, "{place:?}");
    assert_eq!(made.kind, 0.0, "a disc");
    assert!(made.fill < 0.5, "far from landing: {made:?}");
    assert_eq!(made.color, linear(palette::DANGER), "an enemy's: warm");
    // Bevy's forward decal, with the quad its hook gives it.
    let world = app.world_mut();
    let mut query = world.query_filtered::<(Has<ForwardDecal>, Has<Mesh3d>), With<TelegraphDecal>>();
    assert!(query.iter(world).all(|(decal, mesh)| decal && mesh));
    assert_eq!(status(&app)["decals"], 1);
    let said = ok(&mut app, "effects", json!({}));
    let brute_id = id(&app, brute);
    assert!(said.contains(&format!("  decal {brute_id} slam: disc ")) && said.contains(", hostile"), "{said}");

    step(&mut app, 40);
    app.update();
    let late = decals(&mut app);
    assert!(late[0].1.fill > made.fill + 0.3, "the fill grows as the hit nears: {} -> {}", made.fill, late[0].1.fill);

    // The slam is over: nothing on the ground.
    step(&mut app, 60);
    app.update();
    assert!(decals(&mut app).is_empty());
    assert_eq!(status(&app)["decals"], 0);
}

#[test]
fn a_cone_is_a_sector_and_an_allys_warning_is_cool() {
    let assets = Folder::new("sweep");
    let (mut app, hero, brute, _camera) = duel(&assets);
    // Seen from the brute: its own sweep is no threat, but it has a symbol, so it shows, cool.
    app.world_mut().entity_mut(hero).remove::<LocalPlayer>();
    app.world_mut().entity_mut(brute).insert(LocalPlayer);
    press(&mut app, brute, "heavy");
    step(&mut app, 1);
    app.update();
    let shown = decals(&mut app);
    assert_eq!(shown.len(), 1);
    let (place, made) = shown[0];
    assert_eq!(made.kind, 1.0, "a sector");
    assert_eq!(made.color, linear(palette::ALLY));
    // The 4 m cone faces south from the brute: the quad lies ahead of it, turned that way.
    assert!(place.translation.z > -2.0, "{place:?}");
    let forward = place.rotation * Vec3::NEG_Z;
    assert!(forward.distance(Vec3::Z) < 0.01, "the quad faces the cone's way: {forward}");
}

#[test]
fn off_the_gizmos_draw_the_fill_again_and_the_prepass_goes() {
    let assets = Folder::new("off");
    let (mut app, _hero, brute, camera) = duel(&assets);
    let theirs = app.world_mut().spawn((Camera3d::default(), AiCamera, DepthPrepass)).id();
    press(&mut app, brute, "light");
    step(&mut app, 1);
    app.update();
    assert_eq!(decals(&mut app).len(), 1);
    assert_eq!(ok(&mut app, "telegraph_decals", json!({"on": false})), "telegraph decals off: the gizmos draw the fill");
    app.update();
    app.update();
    assert!(decals(&mut app).is_empty());
    assert!(!app.world().resource::<TelegraphSettings>().style.decals);
    assert!(app.world().get::<DepthPrepass>(camera).is_none(), "the prepass the decals asked for goes");
    assert!(app.world().get::<DepthPrepass>(theirs).is_some(), "a camera's own stays");
    assert_eq!(ok(&mut app, "telegraph_decals", json!({})), "telegraph decals on: telegraphs on the ground now: 1");
    app.update();
    app.update();
    assert_eq!(decals(&mut app).len(), 1);
    assert!(app.world().get::<DepthPrepass>(camera).is_some());
    assert!(app.world().resource::<TelegraphSettings>().style.decals);
}
