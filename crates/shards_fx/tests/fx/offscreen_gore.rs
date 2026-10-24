//! Gore on a GPU (`--offscreen`, F9): marks drawn as decals on a wall, a floor and a ceiling and kept off a body that
//! stands in front of them, and chunks of debris, a glTF mesh with no UVs each, drawn where they landed. Drawn into the
//! offscreen image and read back pixel by pixel, as `offscreen.rs` does for the telegraphs.

use serde_json::json;
use shards::bevy::render::view::{DebandDither, Msaa, Tonemapping};

use crate::offscreen::*;
use crate::*;

/// Three marks that show in their own colour whatever the light (`unlit`): magenta on a wall, green on a floor, cyan on a
/// ceiling, each 0.8 m across and round.
const MARKS: &str = r##"{
    "wall": (color: "#ff00ff", size: (0.8, 0.8), rotate: false, unlit: true, depth: 0.15),
    "floor": (color: "#00ff00", size: (0.8, 0.8), rotate: false, unlit: true, depth: 0.15),
    "ceiling": (color: "#00ffff", size: (0.8, 0.8), rotate: false, unlit: true, depth: 0.15),
}"##;

/// Chunks of a green that glows in any light, a mesh each, that bounce, land and stay.
const CHUNKS: &str = r##"{
    "chunk": (count: 8, mesh: "models/chunk.glb", collide: true, rest: UntilCleared, unlit: true, spread: 60.0, speed: (1.0, 2.5),
        gravity: -9.8, lifetime: (3.0, 3.0), size: (0.35, 0.35), size_end: 1.0, color: "#00ff00", color_end: "#00ff00", max_live: 8),
}"##;

fn cyan([r, g, b]: [u8; 3]) -> bool {
    g > r.saturating_add(80) && b > r.saturating_add(80) && g.abs_diff(b) < 60
}

/// A grey that shows as it is (`unlit`): the surfaces marks are laid on.
fn grey(world: &mut World) -> Handle<StandardMaterial> {
    world.resource_mut::<Assets<StandardMaterial>>().add(StandardMaterial { base_color: Color::srgb(0.5, 0.5, 0.5), unlit: true, ..default() })
}

/// A game that draws offscreen on a GPU, on an asset folder with `files` (text) and `blobs` (bytes) in it.
fn gore_app(name: &str, files: &[(&str, &str)], blobs: &[(&str, &[u8])]) -> (App, Folder) {
    let assets = Folder::empty(name);
    for (file, text) in files {
        assets.put(file, text);
    }
    for (file, bytes) in blobs {
        let path = assets.path().join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }
    let dir = assets.path().to_string_lossy().into_owned();
    let app = testing::test_app(
        move |config| {
            (config.headless, config.offscreen, config.window) = (false, true, SIZE);
            config.asset_dir = dir;
        },
        |app| {
            app.insert_resource(ClearColor(Color::srgb_u8(0, 0, 64)));
        },
    );
    (app, assets)
}

#[test]
#[ignore = "needs a GPU, or a software one (WGPU_FORCE_FALLBACK_ADAPTER=1): cargo test -p shards_fx --test fx -- --ignored offscreen"]
fn offscreen_marks_lie_on_a_wall_a_floor_and_a_ceiling_and_stay_off_a_body_in_front_of_them() {
    let (mut app, _assets) = gore_app("offscreen_marks", &[("content/fx/marks.mark.ron", MARKS)], &[]);
    let world = app.world_mut();
    let grey = grey(world);
    // A floor, a wall 4 m ahead of the camera along -Z, a ceiling 4 m up, and a pillar a metre in front of the wall, over
    // the spot a second wall mark is laid on.
    let mut spawn = |size: Vec3, at: Vec3| {
        let mesh = world.resource_mut::<Assets<Mesh>>().add(Cuboid::new(size.x, size.y, size.z));
        world.spawn((Mesh3d(mesh), MeshMaterial3d(grey.clone()), Transform::from_translation(at)));
    };
    spawn(Vec3::new(40.0, 0.2, 40.0), Vec3::new(0.0, -0.1, 0.0));
    spawn(Vec3::new(40.0, 8.0, 0.2), Vec3::new(0.0, 4.0, -4.1));
    spawn(Vec3::new(40.0, 0.2, 40.0), Vec3::new(0.0, 4.1, 0.0));
    spawn(Vec3::new(1.2, 3.0, 0.4), Vec3::new(2.0, 1.5, -3.0));
    let camera = (Camera3d::default(), AiCamera, Tonemapping::None, DebandDither::Disabled);
    let camera = world.spawn((camera, Transform::from_xyz(0.0, 2.0, 3.0).looking_at(Vec3::new(0.0, 2.0, -4.0), Vec3::Y))).id();
    let clear = capture_until(&mut app, "the room", |p| p.find(floor_px).len() > 6_000);
    assert!(clear.find(magenta).is_empty() && clear.find(green).is_empty() && clear.find(cyan).is_empty(), "nothing laid, nothing coloured");

    let wall = Vec3::new(-1.5, 2.0, -4.0);
    let behind_pillar = Vec3::new(2.0, 2.0, -4.0);
    let floor_at = Vec3::new(-1.0, 0.0, -3.0);
    let ceiling_at = Vec3::new(0.0, 4.0, -2.0);
    for (mark, at, normal) in [
        ("wall", wall, [0, 0, 1]),
        ("wall", behind_pillar, [0, 0, 1]),
        ("floor", floor_at, [0, 1, 0]),
        ("ceiling", ceiling_at, [0, -1, 0]),
    ] {
        ok(&mut app, "mark_spawn", json!({"mark": mark, "point": [at.x, at.y, at.z], "normal": normal}));
    }
    // The decals' pipelines compile in the background: a few dozen frames in, they are there.
    let drawn = capture_until(&mut app, "the marks", |p| p.find(magenta).len() > 100 && p.find(green).len() > 100 && p.find(cyan).len() > 100);
    let near = |pixels: &[Vec2], at: Vec2, radius: f32| pixels.iter().filter(|p| p.distance(at) < radius).count();
    // Each colour is a patch round its point, and nothing of it is anywhere else.
    for (what, pixels, at) in [
        ("wall", drawn.find(magenta), wall),
        ("floor", drawn.find(green), floor_at),
        ("ceiling", drawn.find(cyan), ceiling_at),
    ] {
        let on = on_screen(&mut app, camera, at);
        let (inside, all) = (near(&pixels, on, 22.0), pixels.len());
        eprintln!("{what}: {all} pixels, {inside} within 22 px of {on}, centre {}", mean(&pixels));
        assert!(inside * 10 >= all * 9, "{what} mark where it was laid: {inside} of {all} within 22 px of {on}");
        assert!(inside > 100, "{what} mark drawn: {inside} pixels");
    }
    // The wall's second mark is behind the pillar a metre in front of it: the pillar is not painted (the mark's depth is
    // 0.15 m), and the wall's first mark is the only magenta there is.
    let pillar = on_screen(&mut app, camera, behind_pillar);
    let painted = drawn.find(magenta).iter().filter(|p| p.distance(pillar) < 10.0).count();
    assert_eq!(painted, 0, "no paint on the pillar that stands in front of the mark: {painted} pixels near {pillar}");
    let said = ok(&mut app, "effects", json!({}));
    assert!(said.contains("alive 2/64") && said.contains("floor") && said.contains("ceiling"), "{said}");

    // Clearing takes them off the picture.
    assert_eq!(ok(&mut app, "fx_clear", json!({"all": true})), "cleared 4 marks and 0 particles of every kind");
    let gone = capture_until(&mut app, "the marks gone", |p| p.find(magenta).is_empty() && p.find(green).is_empty() && p.find(cyan).is_empty());
    assert!(gone.find(floor_px).len() > 6_000);
}

#[test]
#[ignore = "needs a GPU, or a software one (WGPU_FORCE_FALLBACK_ADAPTER=1): cargo test -p shards_fx --test fx -- --ignored offscreen"]
fn offscreen_chunks_of_a_mesh_without_uvs_lie_on_the_floor_where_they_landed() {
    // The sandbox's chunk of debris (`cargo run -p sandbox --example make_gore`): a binary glTF with positions and
    // normals and no UVs, which the particle shader has to draw all the same.
    let chunk: &[u8] = include_bytes!("../../../sandbox/assets/models/chunk.glb");
    let (mut app, _assets) = gore_app("offscreen_chunks", &[("content/fx/chunks.effect.ron", CHUNKS)], &[("models/chunk.glb", chunk)]);
    let world = app.world_mut();
    floor(world);
    let grey = grey(world);
    let plane = world.resource_mut::<Assets<Mesh>>().add(Plane3d::default().mesh().size(40.0, 40.0));
    world.spawn((Mesh3d(plane), MeshMaterial3d(grey), Transform::default()));
    let camera = (Camera3d::default(), AiCamera, Msaa::Off, Tonemapping::None, DebandDither::Disabled);
    let camera = world.spawn((camera, Transform::from_xyz(0.0, 6.0, 5.0).looking_at(Vec3::new(0.0, 0.0, 0.0), Vec3::Y))).id();
    step(&mut app, 2);
    capture_until(&mut app, "the floor", |p| p.find(floor_px).len() > 20_000);
    let said = ok(&mut app, "effect_play", json!({"effect": "chunk", "point": [0.0, 1.5, 0.0]}));
    assert!(said.contains("8 particles"), "{said}");
    // Four seconds on the simulation's clock: they have flown, bounced and lain down.
    step(&mut app, 240);
    let lying = capture_until(&mut app, "the chunks", |p| p.find(green).len() > 200);
    let (resting, flying) = shards::fx::debris_counts(app.world());
    assert_eq!((resting, flying), (8, 0), "all at rest");
    let pool = &app.world().resource::<shards::fx::Particles>().effects["chunk"].pool;
    let spots: Vec<Vec3> = pool.slots.iter().flatten().map(|p| p.body.unwrap().to).collect();
    assert!(spots.iter().all(|p| (p.y - 0.175).abs() < 0.02), "on the floor, half a size up: {spots:?}");
    let on_floor: Vec<Vec2> = spots.iter().map(|p| on_screen(&mut app, camera, *p)).collect();
    let green_px = lying.find(green);
    eprintln!("chunks: {} green pixels; {} chunks on the floor, the first at {}", green_px.len(), on_floor.len(), on_floor[0]);
    // Every green pixel is next to a chunk that lies on the floor, and every chunk has green by it.
    assert!(green_px.iter().all(|p| on_floor.iter().any(|at| at.distance(*p) < 30.0)), "the green is where the chunks lie");
    assert!(on_floor.iter().all(|at| green_px.iter().any(|p| at.distance(*p) < 12.0)), "each chunk is drawn");
    // They stay: a long time on, in the same places.
    step(&mut app, 3000);
    let later = capture(&mut app);
    assert!(later.find(green).len() >= green_px.len() * 9 / 10, "{} green pixels later, {} before", later.find(green).len(), green_px.len());
    // Cleared by tag, they are gone.
    ok(&mut app, "fx_clear", json!({"all": true}));
    capture_until(&mut app, "the chunks gone", |p| p.find(green).is_empty());
}

/// A white quad, plain and with a picture (the sandbox's bullet hole: a dark pit in the middle), 1 m across, that stay put.
const STAMPS: &str = r##"{
    "plain": (count: 1, speed: (0.0, 0.0), lifetime: (30.0, 30.0), size: (1.0, 1.0), size_end: 1.0, color: "#ffffff",
        color_end: "#ffffff", spin: 0.0, max_live: 1),
    "stamp": (count: 1, image: "textures/hole.png", speed: (0.0, 0.0), lifetime: (30.0, 30.0), size: (1.0, 1.0), size_end: 1.0,
        color: "#ffffff", color_end: "#ffffff", spin: 0.0, max_live: 1),
}"##;

#[test]
#[ignore = "needs a GPU, or a software one (WGPU_FORCE_FALLBACK_ADAPTER=1): cargo test -p shards_fx --test fx -- --ignored offscreen"]
fn offscreen_a_quad_wears_its_picture_and_a_plain_one_stays_a_white_spot() {
    let hole: &[u8] = include_bytes!("../../../sandbox/assets/textures/marks/bullet_hole.png");
    let (mut app, _assets) = gore_app("offscreen_stamps", &[("content/fx/stamps.effect.ron", STAMPS)], &[("textures/hole.png", hole)]);
    let world = app.world_mut();
    let grey = grey(world);
    let plane = world.resource_mut::<Assets<Mesh>>().add(Plane3d::default().mesh().size(40.0, 40.0));
    world.spawn((Mesh3d(plane), MeshMaterial3d(grey), Transform::default()));
    let camera = (Camera3d::default(), AiCamera, Msaa::Off, Tonemapping::None, DebandDither::Disabled);
    let camera = world.spawn((camera, Transform::from_xyz(0.0, 5.0, 4.0).looking_at(Vec3::ZERO, Vec3::Y))).id();
    ok(&mut app, "effect_play", json!({"effect": "plain", "point": [-1.5, 0.5, 0.0]}));
    ok(&mut app, "effect_play", json!({"effect": "stamp", "point": [1.5, 0.5, 0.0]}));
    let bright = |p: &Picture, at: Vec2| p.pixels[(at.y as u32 * p.width + at.x as u32) as usize][0];
    capture(&mut app);
    let (plain_at, stamp_at) = (on_screen(&mut app, camera, Vec3::new(-1.5, 0.5, 0.0)), on_screen(&mut app, camera, Vec3::new(1.5, 0.5, 0.0)));
    // The picture loads and its pipeline compiles a few frames in: until then the quad is the plain spot or nothing.
    let drawn = capture_until(&mut app, "the picture", |p| bright(p, plain_at) > 240 && bright(p, stamp_at) < 80);
    eprintln!("the plain spot's middle {}, the stamp's {}", bright(&drawn, plain_at), bright(&drawn, stamp_at));
    // Round the pit the picture's dust ring is faint, so the stamp has far less white than the plain spot.
    let white = |p: &Picture, at: Vec2| p.find(|[r, g, b]| r > 200 && g > 200 && b > 200).iter().filter(|q| q.distance(at) < 40.0).count();
    assert!(white(&drawn, stamp_at) * 2 < white(&drawn, plain_at), "{} against {}", white(&drawn, stamp_at), white(&drawn, plain_at));
}
