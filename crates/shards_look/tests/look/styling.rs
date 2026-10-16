//! The meshes the looks draw, on the headless engine counted as drawn: which look a mesh takes,
//! the material, flat copy and outline it gets, and what restyles it. The look systems run in
//! `PostUpdate`, so one `update` after a change shows its result (two after an asset changed in
//! place: its event goes out after them). Headless, where nothing is drawn, nothing is styled.

use serde_json::json;
use shards::bevy::camera::visibility::{NoFrustumCulling, RenderLayers, VisibilityRange};
use shards::bevy::gltf::GltfMaterialName;
use shards::bevy::light::NotShadowCaster;
use shards::bevy::mesh::skinning::SkinnedMesh;
use shards::look::{mesh, Look, Looked, OutlineHull, OutlineMaterial, ToonMaterial};

use crate::*;

/// Glass's glow (`#7fe0ff`): an unlit look has it in its colour.
fn glowing(color: Color) -> LinearRgba {
    let own = color.to_linear();
    LinearRgba { alpha: own.alpha, ..own + srgb(0x7fe0ff).to_linear() }
}

fn positions(app: &App, mesh: &Handle<Mesh>) -> Vec<[f32; 3]> {
    let mesh = app.world().resource::<Assets<Mesh>>().get(mesh).unwrap();
    mesh.try_attribute(Mesh::ATTRIBUTE_POSITION).unwrap().as_float3().unwrap().to_vec()
}

/// How far the mesh reaches from its centre.
fn reach(app: &App, mesh: &Handle<Mesh>) -> f32 {
    positions(app, mesh).iter().map(|p| Vec3::from(*p).length()).fold(0.0, f32::max)
}

fn mesh_of(app: &App, entity: Entity) -> Handle<Mesh> {
    app.world().get::<Mesh3d>(entity).unwrap().0.clone()
}

/// What a game draws before a look: a red and a green material, a sphere and a cube.
struct Kit {
    red: Handle<StandardMaterial>,
    green: Handle<StandardMaterial>,
    sphere: Handle<Mesh>,
    cube: Handle<Mesh>,
}

fn kit(app: &mut App) -> Kit {
    let world = app.world_mut();
    let mut materials = world.resource_mut::<Assets<StandardMaterial>>();
    let red = materials.add(StandardMaterial { base_color: Color::srgb(1.0, 0.0, 0.0), perceptual_roughness: 0.5, ..default() });
    let green = materials.add(Color::srgb(0.0, 1.0, 0.0));
    let mut meshes = world.resource_mut::<Assets<Mesh>>();
    Kit { red, green, sphere: meshes.add(Sphere::new(1.0)), cube: meshes.add(Cuboid::new(1.0, 1.0, 1.0)) }
}

/// A mesh drawn with `material`, with `extra` (a `Look`, a parent, a glTF material's name).
fn mesh_with(app: &mut App, mesh: &Handle<Mesh>, material: &Handle<StandardMaterial>, extra: impl Bundle) -> Entity {
    app.world_mut().spawn((Mesh3d(mesh.clone()), MeshMaterial3d(material.clone()), extra)).id()
}

/// An entity without a mesh, with `extra`.
fn node(app: &mut App, extra: impl Bundle) -> Entity {
    app.world_mut().spawn((Transform::default(), Visibility::default(), extra)).id()
}

fn looked(app: &App, entity: Entity) -> Option<Looked> {
    app.world().get::<Looked>(entity).cloned()
}

fn look_of(app: &App, entity: Entity) -> Option<String> {
    looked(app, entity).map(|looked| looked.look)
}

fn toon(app: &App, entity: Entity) -> ToonMaterial {
    let handle = app.world().get::<MeshMaterial3d<ToonMaterial>>(entity).expect("a toon material");
    app.world().resource::<Assets<ToonMaterial>>().get(&handle.0).unwrap().clone()
}

fn standard(app: &App, entity: Entity) -> (Handle<StandardMaterial>, StandardMaterial) {
    let handle = app.world().get::<MeshMaterial3d<StandardMaterial>>(entity).expect("a StandardMaterial").0.clone();
    let material = app.world().resource::<Assets<StandardMaterial>>().get(&handle).unwrap().clone();
    (handle, material)
}

fn hull(app: &App, entity: Entity) -> Entity {
    looked(app, entity).and_then(|looked| looked.hull).expect("an outline")
}

fn outlines(app: &App, entity: Entity) -> usize {
    let world = app.world();
    world.get::<Children>(entity).map_or(0, |children| children.iter().filter(|c| world.get::<OutlineHull>(*c).is_some()).count())
}

fn srgb(hex: u32) -> Color {
    Color::srgb_u8((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

#[test]
fn a_mesh_takes_its_own_look_else_the_nearest_above_it_else_its_gltf_material_s() {
    let assets = Folder::new("which");
    let mut app = drawn(&assets);
    let kit = kit(&mut app);
    let hero = node(&mut app, Look::new("hero"));
    let body = mesh_with(&mut app, &kit.sphere, &kit.red, ChildOf(hero));
    let visor = mesh_with(&mut app, &kit.cube, &kit.red, (Look::new("glass"), ChildOf(hero)));
    let arm = node(&mut app, ChildOf(hero));
    let hand = mesh_with(&mut app, &kit.cube, &kit.red, ChildOf(arm));
    let rock = mesh_with(&mut app, &kit.cube, &kit.red, GltfMaterialName("Rock.001".into()));
    let moss = mesh_with(&mut app, &kit.cube, &kit.red, GltfMaterialName("Moss".into()));
    let wall = node(&mut app, Look::new("stone"));
    let mossy = mesh_with(&mut app, &kit.cube, &kit.red, (GltfMaterialName("Moss".into()), ChildOf(wall)));
    let grass = mesh_with(&mut app, &kit.cube, &kit.red, GltfMaterialName("Grass".into()));
    let plain = mesh_with(&mut app, &kit.cube, &kit.red, ());
    let ghost = mesh_with(&mut app, &kit.cube, &kit.red, Look::new("ghost"));
    app.update();

    let looks = [body, visor, hand, rock, moss, mossy, grass, plain, ghost].map(|e| look_of(&app, e));
    let wanted = [Some("hero"), Some("glass"), Some("hero"), Some("stone"), Some("Moss"), Some("stone"), None, None, None];
    assert_eq!(looks, wanted.map(|look| look.map(str::to_owned)));
    // Without a look the game's material stays; a look no content defines is the content check's.
    for e in [grass, plain, ghost] {
        assert_eq!(app.world().get::<MeshMaterial3d<StandardMaterial>>(e).map(|m| &m.0), Some(&kit.red));
    }
    assert_eq!(status(&app), json!({"looks": 5, "styled": 6, "outlined": 4, "outlines": true}));
    let said = ok(&mut app, "looks", json!({}));
    assert!(said.contains("\n  hero   #408cff · toon 3 bands · flat · outline 2px #000000 · 2 meshes\n"), "{said}");
    assert!(said.contains("\n  Moss   matte · 1 mesh\n") && !said.contains("wait"), "{said}");
}

#[test]
fn a_look_draws_with_a_material_made_from_the_mesh_s_own() {
    let assets = Folder::new("materials");
    let mut app = drawn(&assets);
    let kit = kit(&mut app);
    let texture = app.world_mut().resource_mut::<Assets<Image>>().add(Image::default());
    let textured = StandardMaterial { base_color_texture: Some(texture.clone()), ..default() };
    let textured = app.world_mut().resource_mut::<Assets<StandardMaterial>>().add(textured);
    let stone = mesh_with(&mut app, &kit.cube, &textured, Look::new("stone"));
    let stone_too = mesh_with(&mut app, &kit.sphere, &textured, Look::new("stone"));
    let red_stone = mesh_with(&mut app, &kit.cube, &kit.red, Look::new("stone"));
    let glass = mesh_with(&mut app, &kit.cube, &kit.red, Look::new("glass"));
    let metal = mesh_with(&mut app, &kit.cube, &kit.red, Look::new("metal"));
    let moss = mesh_with(&mut app, &kit.cube, &kit.red, Look::new("Moss"));
    // A glTF file's material may be unlit (KHR_materials_unlit); the look says how light falls.
    let unlit = StandardMaterial { unlit: true, ..default() };
    let unlit = app.world_mut().resource_mut::<Assets<StandardMaterial>>().add(unlit);
    let lit_metal = mesh_with(&mut app, &kit.cube, &unlit, Look::new("metal"));
    let lit_stone = mesh_with(&mut app, &kit.cube, &unlit, Look::new("stone"));
    app.update();

    // Toon: the look's colour over the mesh's texture, no shine, three bands; the game's material
    // is kept to put back.
    assert!(app.world().get::<MeshMaterial3d<StandardMaterial>>(stone).is_none(), "drawn with the toon material only");
    let drawn = toon(&app, stone);
    assert_eq!((drawn.base.base_color, drawn.base.base_color_texture), (srgb(0x6b6f78), Some(texture)));
    let shine = (drawn.base.perceptual_roughness, drawn.base.metallic, drawn.base.reflectance);
    assert_eq!((shine, drawn.extension.bands), ((1.0, 0.0, 0.0), 3));
    assert_eq!(looked(&app, stone).unwrap().source, textured);
    // One material a look and a material of the game's.
    let handle = |e| app.world().get::<MeshMaterial3d<ToonMaterial>>(e).unwrap().0.clone();
    assert_eq!(handle(stone), handle(stone_too));
    assert_ne!(handle(stone), handle(red_stone));
    // Unlit: the mesh's own colour as it is, and the glow in it (Bevy shows no emissive light on
    // an unlit surface).
    let (glass_handle, glass_drawn) = standard(&app, glass);
    assert_ne!(glass_handle, kit.red);
    assert_eq!((glass_drawn.unlit, glass_drawn.base_color.to_linear()), (true, glowing(Color::srgb(1.0, 0.0, 0.0))));
    assert_eq!(glass_drawn.emissive, LinearRgba::BLACK);
    let (_, metal_drawn) = standard(&app, metal);
    assert_eq!((metal_drawn.perceptual_roughness, metal_drawn.metallic, metal_drawn.unlit), (0.4, 1.0, false));
    assert!(!standard(&app, lit_metal).1.unlit && !toon(&app, lit_stone).base.unlit, "lit, as the looks say");
    let (_, moss_drawn) = standard(&app, moss);
    let matte = (moss_drawn.base_color, moss_drawn.perceptual_roughness, moss_drawn.metallic, moss_drawn.reflectance);
    assert_eq!(matte, (Color::srgb(1.0, 0.0, 0.0), 1.0, 0.0, 0.0));
    // The game's material itself is as it was.
    let red = app.world().resource::<Assets<StandardMaterial>>().get(&kit.red).unwrap();
    assert_eq!((red.unlit, red.perceptual_roughness), (false, 0.5));
}

#[test]
fn a_flat_look_draws_a_flat_copy_and_an_outline_a_hull_as_the_mesh_s_child() {
    let assets = Folder::new("hull");
    let mut app = drawn(&assets);
    let kit = kit(&mut app);
    let body = mesh_with(&mut app, &kit.sphere, &kit.red, Look::new("hero"));
    let twin = mesh_with(&mut app, &kit.sphere, &kit.green, Look::new("hero"));
    let skin = SkinnedMesh { inverse_bindposes: Handle::default(), joints: Vec::new() };
    let skinned = mesh_with(&mut app, &kit.cube, &kit.red, (Look::new("stone"), skin.clone()));
    app.update();

    let styled = looked(&app, body).unwrap();
    let world = app.world();
    let meshes = world.resource::<Assets<Mesh>>();
    let source = meshes.get(&kit.sphere).unwrap();
    // Flat: a copy, every triangle its own three vertices; the mesh is kept to put back.
    let drawn = &world.get::<Mesh3d>(body).unwrap().0;
    assert_ne!(drawn, &kit.sphere);
    assert_eq!(styled.flattened.as_ref(), Some(&kit.sphere));
    let flat = meshes.get(drawn).unwrap();
    assert!(flat.try_indices_option().unwrap().is_none());
    assert_eq!(flat.count_vertices(), mesh::drawn_vertices(source));
    assert_eq!(&world.get::<Mesh3d>(twin).unwrap().0, drawn, "one flat copy a mesh");
    // The outline: the hull of the mesh (not of its flat copy), a child that casts no shadow, in
    // the look's colour and width.
    let outline = world.entity(styled.hull.expect("an outline"));
    assert!(outline.contains::<OutlineHull>() && outline.contains::<NotShadowCaster>());
    assert!(!outline.contains::<SkinnedMesh>() && !outline.contains::<Looked>());
    assert_eq!(outline.get::<ChildOf>().map(ChildOf::parent), Some(body));
    assert_eq!((outline.get::<Transform>(), outline.get::<Visibility>()), (Some(&Transform::IDENTITY), Some(&Visibility::Inherited)));
    let hull_mesh = meshes.get(&outline.get::<Mesh3d>().unwrap().0).unwrap();
    assert_eq!(mesh::normals(hull_mesh), mesh::normals(&mesh::hull(source).unwrap()));
    assert_eq!(mesh::drawn_vertices(hull_mesh), mesh::drawn_vertices(source));
    let material = &outline.get::<MeshMaterial3d<OutlineMaterial>>().unwrap().0;
    let material = world.resource::<Assets<OutlineMaterial>>().get(material).unwrap();
    assert_eq!((material.color, material.width), (Color::BLACK.to_linear(), 2.0));
    assert_eq!(world.get::<Mesh3d>(hull(&app, twin)).unwrap().0, outline.get::<Mesh3d>().unwrap().0, "one hull a mesh");
    // A skinned mesh's outline bends with it.
    let bent = world.get::<SkinnedMesh>(hull(&app, skinned)).expect("the skin of its mesh");
    assert_eq!((&bent.inverse_bindposes, &bent.joints), (&skin.inverse_bindposes, &skin.joints));
    assert_eq!(status(&app)["outlined"], 3);
}

#[test]
fn look_outlines_hides_and_shows_the_hulls_and_new_ones_follow() {
    let assets = Folder::new("outlines");
    let mut app = drawn(&assets);
    let kit = kit(&mut app);
    let first = mesh_with(&mut app, &kit.cube, &kit.red, Look::new("stone"));
    app.update();
    let shown = |app: &App, e: Entity| *app.world().get::<Visibility>(hull(app, e)).unwrap();
    assert_eq!(shown(&app, first), Visibility::Inherited);
    assert_eq!(ok(&mut app, "look_outlines", json!({"on": false})), "outlines off: 1 drawn with an outline");
    app.update();
    assert_eq!(shown(&app, first), Visibility::Hidden);
    let second = mesh_with(&mut app, &kit.sphere, &kit.red, Look::new("stone"));
    app.update();
    assert_eq!(shown(&app, second), Visibility::Hidden, "made hidden while they are off");
    assert_eq!(status(&app), json!({"looks": 5, "styled": 2, "outlined": 2, "outlines": false}));
    assert_eq!(ok(&mut app, "look_outlines", json!({"on": true})), "outlines on: 2 drawn with an outline");
    app.update();
    assert_eq!((shown(&app, first), shown(&app, second)), (Visibility::Inherited, Visibility::Inherited));
}

#[test]
fn taking_a_look_off_puts_the_meshes_back_and_changing_it_restyles_them() {
    let assets = Folder::new("change");
    let mut app = drawn(&assets);
    let kit = kit(&mut app);
    let hero = node(&mut app, Look::new("hero"));
    let body = mesh_with(&mut app, &kit.sphere, &kit.red, ChildOf(hero));
    let visor = mesh_with(&mut app, &kit.cube, &kit.red, (Look::new("glass"), ChildOf(hero)));
    app.update();
    let outline = hull(&app, body);

    app.world_mut().entity_mut(hero).remove::<Look>();
    app.update();
    let world = app.world();
    assert!(world.get::<Looked>(body).is_none() && world.get::<MeshMaterial3d<ToonMaterial>>(body).is_none());
    assert_eq!(world.get::<MeshMaterial3d<StandardMaterial>>(body).map(|m| &m.0), Some(&kit.red));
    assert_eq!(world.get::<Mesh3d>(body).map(|m| &m.0), Some(&kit.sphere), "its own mesh again, not the flat copy");
    assert!(world.get_entity(outline).is_err(), "the outline is gone");
    assert_eq!(look_of(&app, visor).as_deref(), Some("glass"), "a mesh's own look stays");

    app.world_mut().entity_mut(hero).insert(Look::new("metal"));
    app.update();
    let styled = looked(&app, body).unwrap();
    assert_eq!((styled.look.as_str(), styled.flattened, styled.hull), ("metal", None, None));
    assert_eq!(app.world().get::<Mesh3d>(body).map(|m| &m.0), Some(&kit.sphere));
    assert_eq!(standard(&app, body).1.metallic, 1.0);
    assert_eq!(look_of(&app, visor).as_deref(), Some("glass"));

    app.world_mut().entity_mut(visor).insert(Look::new("stone"));
    app.update();
    assert_eq!(look_of(&app, visor).as_deref(), Some("stone"));
    assert_eq!(toon(&app, visor).base.base_color, srgb(0x6b6f78));
    assert!(app.world().get::<MeshMaterial3d<StandardMaterial>>(visor).is_none());
    assert_eq!((outlines(&app, body), outlines(&app, visor)), (0, 1), "one outline, none left over");
    assert_eq!(status(&app)["styled"], 2);
}

#[test]
fn a_material_the_game_gives_a_styled_mesh_is_styled_anew() {
    let assets = Folder::new("regiven");
    let mut app = drawn(&assets);
    let kit = kit(&mut app);
    let glass = mesh_with(&mut app, &kit.cube, &kit.red, Look::new("glass"));
    let stone = mesh_with(&mut app, &kit.cube, &kit.red, Look::new("stone"));
    app.update();
    let drawn = standard(&app, glass).0;
    app.update();
    assert_eq!(standard(&app, glass).0, drawn, "the look's own material is not taken for the game's");

    app.world_mut().entity_mut(glass).insert(MeshMaterial3d(kit.green.clone()));
    app.world_mut().entity_mut(stone).insert(MeshMaterial3d(kit.green.clone()));
    app.update();
    let (handle, material) = standard(&app, glass);
    assert_eq!(looked(&app, glass).unwrap().source, kit.green);
    assert!(handle != drawn && handle != kit.green);
    assert_eq!((material.unlit, material.base_color.to_linear()), (true, glowing(Color::srgb(0.0, 1.0, 0.0))));
    assert_eq!(looked(&app, stone).unwrap().source, kit.green);
    assert!(app.world().get::<MeshMaterial3d<StandardMaterial>>(stone).is_none(), "toon again, the game's material put away");
    assert_eq!(toon(&app, stone).base.base_color, srgb(0x6b6f78));
    assert_eq!(outlines(&app, stone), 1);
}

#[test]
fn a_content_change_restyles_every_mesh_as_hot_reload_does() {
    let assets = Folder::new("reload");
    let mut app = drawn(&assets);
    let kit = kit(&mut app);
    let rock = mesh_with(&mut app, &kit.cube, &kit.red, GltfMaterialName("Rock".into()));
    let grass = mesh_with(&mut app, &kit.cube, &kit.red, GltfMaterialName("Grass".into()));
    let body = mesh_with(&mut app, &kit.sphere, &kit.red, Look::new("hero"));
    let visor = mesh_with(&mut app, &kit.cube, &kit.red, Look::new("glass"));
    app.update();
    let old_outline = hull(&app, body);

    // Stone turns red and draws `Grass` instead of `Rock.001`, the hero is no longer flat, glass is gone.
    let edited = LOOKS
        .replace("#6b6f78", "#ff0000")
        .replace("\"Rock.001\"", "\"Grass\"")
        .replace("flat: true, ", "")
        .replace("\"glass\": (shading: Unlit, glow: Some(\"#7fe0ff\")),", "");
    assert_ne!(edited, LOOKS);
    assets.put("content/look/test.look.ron", &edited);
    ok(&mut app, "content_reload", json!({}));
    app.update();

    assert_eq!(toon(&app, rock).base.base_color, srgb(0xff0000));
    assert_eq!(look_of(&app, grass).as_deref(), Some("stone"));
    let styled = looked(&app, body).unwrap();
    assert_eq!((styled.look.as_str(), styled.flattened), ("hero", None));
    assert_eq!(app.world().get::<Mesh3d>(body).map(|m| &m.0), Some(&kit.sphere));
    assert!(app.world().get_entity(old_outline).is_err() && outlines(&app, body) == 1, "a new outline, the old one gone");
    assert!(looked(&app, visor).is_none(), "a look content no longer defines draws nothing");
    assert_eq!(app.world().get::<MeshMaterial3d<StandardMaterial>>(visor).map(|m| &m.0), Some(&kit.red));
    assert_eq!(status(&app), json!({"looks": 4, "styled": 3, "outlined": 3, "outlines": true}));
    let said = ok(&mut app, "looks", json!({}));
    assert!(said.contains("\n  stone  #ff0000 · toon 3 bands · outline 1.5px #16171c · materials Rock, Grass · 2 meshes"), "{said}");
}

#[test]
fn a_mesh_whose_material_is_still_loading_waits_for_it() {
    let assets = Folder::new("waiting");
    let mut app = drawn(&assets);
    let kit = kit(&mut app);
    let later = app.world().resource::<Assets<StandardMaterial>>().reserve_handle();
    let crate_ = mesh_with(&mut app, &kit.cube, &later, Look::new("stone"));
    app.update();
    assert!(looked(&app, crate_).is_none());
    let said = ok(&mut app, "looks", json!({}));
    assert!(said.ends_with("\n1 mesh waits for its material or mesh to load"), "{said}");

    app.world_mut().resource_mut::<Assets<StandardMaterial>>().insert(&later, StandardMaterial::from(Color::WHITE)).unwrap();
    app.update();
    assert_eq!(look_of(&app, crate_).as_deref(), Some("stone"));
    assert!(!ok(&mut app, "looks", json!({})).contains("wait"));
}

#[test]
fn a_mesh_the_game_gives_a_styled_entity_is_styled_anew() {
    let assets = Folder::new("reshaped");
    let mut app = drawn(&assets);
    let kit = kit(&mut app);
    let body = mesh_with(&mut app, &kit.cube, &kit.red, Look::new("hero"));
    let rock = mesh_with(&mut app, &kit.cube, &kit.red, Look::new("stone"));
    app.update();
    let flat_cube = app.world().get::<Mesh3d>(body).unwrap().0.clone();
    app.update();
    assert_eq!(app.world().get::<Mesh3d>(body).unwrap().0, flat_cube, "the look's own flat copy is not taken for the game's mesh");

    for e in [body, rock] {
        app.world_mut().entity_mut(e).insert(Mesh3d(kit.sphere.clone()));
    }
    app.update();
    // The flat look flattens the new mesh, and both outlines take its shape.
    assert_eq!(looked(&app, body).unwrap().flattened, Some(kit.sphere.clone()));
    let drawn = app.world().get::<Mesh3d>(body).unwrap().0.clone();
    assert!(drawn != kit.sphere && drawn != flat_cube);
    let meshes = app.world().resource::<Assets<Mesh>>();
    let sphere = mesh::drawn_vertices(meshes.get(&kit.sphere).unwrap());
    assert_eq!(meshes.get(&drawn).unwrap().count_vertices(), sphere);
    assert_eq!(app.world().get::<Mesh3d>(rock).unwrap().0, kit.sphere);
    for e in [body, rock] {
        let outline = &app.world().get::<Mesh3d>(hull(&app, e)).unwrap().0;
        assert_eq!(mesh::drawn_vertices(meshes.get(outline).unwrap()), sphere, "the outline of the new shape");
        assert_eq!(outlines(&app, e), 1);
    }
    // Taken off, the look leaves the game's mesh as the game gave it.
    app.world_mut().entity_mut(body).remove::<Look>();
    app.update();
    assert_eq!(app.world().get::<Mesh3d>(body).unwrap().0, kit.sphere);
}

#[test]
fn what_the_looks_made_for_a_mesh_and_a_material_goes_when_they_go() {
    let assets = Folder::new("forget");
    let mut app = drawn(&assets);
    let counts = |app: &App| {
        let world = app.world();
        let meshes = world.resource::<Assets<Mesh>>().len();
        let materials = world.resource::<Assets<StandardMaterial>>().len();
        (meshes, materials, world.resource::<Assets<ToonMaterial>>().len(), world.resource::<Assets<OutlineMaterial>>().len())
    };
    let before = counts(&app);
    let sphere = app.world_mut().resource_mut::<Assets<Mesh>>().add(Sphere::new(1.0));
    let red = app.world_mut().resource_mut::<Assets<StandardMaterial>>().add(Color::srgb(1.0, 0.0, 0.0));
    let body = mesh_with(&mut app, &sphere, &red, Look::new("hero"));
    let visor = mesh_with(&mut app, &sphere, &red, Look::new("glass"));
    app.update();
    // The sphere, its flat copy and its hull; red and glass's copy of it; hero's toon; hero's outline.
    assert_eq!(counts(&app), (before.0 + 3, before.1 + 2, before.2 + 1, before.3 + 1));

    drop((sphere, red));
    app.world_mut().entity_mut(body).despawn();
    app.world_mut().entity_mut(visor).despawn();
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(counts(&app), (before.0, before.1, before.2, before.3 + 1), "only the look's outline material stays");
}

#[test]
fn a_material_or_a_mesh_changed_in_place_restyles_what_is_drawn_from_it() {
    let assets = Folder::new("modified");
    let mut app = drawn(&assets);
    let kit = kit(&mut app);
    let glass = mesh_with(&mut app, &kit.cube, &kit.red, Look::new("glass"));
    let body = mesh_with(&mut app, &kit.sphere, &kit.green, Look::new("hero"));
    let other = mesh_with(&mut app, &kit.cube, &kit.green, Look::new("glass"));
    app.update();
    let old_glass = standard(&app, glass).0;
    let (old_flat, old_outline) = (mesh_of(&app, body), hull(&app, body));
    let old_hull = mesh_of(&app, old_outline);
    let old_other = standard(&app, other).0;

    // The game recolours its material and reshapes its mesh where they are (as a glTF file
    // reloaded does).
    app.world_mut().resource_mut::<Assets<StandardMaterial>>().get_mut(&kit.red).unwrap().base_color = Color::srgb(0.0, 0.0, 1.0);
    {
        let mut meshes = app.world_mut().resource_mut::<Assets<Mesh>>();
        let mut sphere = meshes.get_mut(&kit.sphere).unwrap();
        let positions = sphere.try_attribute(Mesh::ATTRIBUTE_POSITION).unwrap().as_float3().unwrap();
        let twice: Vec<[f32; 3]> = positions.iter().map(|p| p.map(|c| c * 2.0)).collect();
        sphere.insert_attribute(Mesh::ATTRIBUTE_POSITION, twice);
    }
    // Their events go out after the look systems: the frame after shows them.
    app.update();
    app.update();
    let (new_glass, material) = standard(&app, glass);
    assert_ne!(new_glass, old_glass);
    assert_eq!(material.base_color.to_linear(), glowing(Color::srgb(0.0, 0.0, 1.0)), "the new colour");
    let new_flat = mesh_of(&app, body);
    assert!(new_flat != old_flat && new_flat != kit.sphere);
    assert!((reach(&app, &new_flat) - 2.0).abs() < 1e-4, "a flat copy of the new shape");
    let new_outline = hull(&app, body);
    let new_hull = mesh_of(&app, new_outline);
    assert!(new_hull != old_hull && (reach(&app, &new_hull) - 2.0).abs() < 1e-4, "the hull of the new shape");
    assert!(app.world().get_entity(old_outline).is_err() && outlines(&app, body) == 1, "the old outline gone");
    assert_eq!(standard(&app, other).0, old_other, "what is drawn from neither stays");
    // The looks' own new meshes and materials restyle nothing: what is drawn stays.
    for _ in 0..3 {
        app.update();
    }
    assert_eq!((standard(&app, glass).0, mesh_of(&app, body), hull(&app, body)), (new_glass, new_flat, new_outline));
}

#[test]
fn a_see_through_mesh_keeps_its_alpha_and_has_no_outline() {
    let assets = Folder::new("alpha");
    let mut app = drawn(&assets);
    let kit = kit(&mut app);
    let mut materials = app.world_mut().resource_mut::<Assets<StandardMaterial>>();
    let glassy = materials.add(StandardMaterial { base_color: Color::srgba(1.0, 1.0, 1.0, 0.4), alpha_mode: AlphaMode::Blend, ..default() });
    let leafy = materials.add(StandardMaterial { alpha_mode: AlphaMode::Mask(0.5), ..default() });
    let window = mesh_with(&mut app, &kit.cube, &glassy, Look::new("stone"));
    let leaves = mesh_with(&mut app, &kit.cube, &leafy, Look::new("hero"));
    let wall = mesh_with(&mut app, &kit.cube, &kit.red, Look::new("stone"));
    app.update();

    // The look's colour, the glass's alpha and blending.
    let drawn = toon(&app, window).base;
    assert_eq!((drawn.base_color, drawn.alpha_mode), (srgb(0x6b6f78).with_alpha(0.4), AlphaMode::Blend));
    // An outline would show through the glass, and go round the whole leaf card.
    assert_eq!((outlines(&app, window), outlines(&app, leaves), outlines(&app, wall)), (0, 0, 1));
    assert!(looked(&app, window).unwrap().hull.is_none());
    assert_eq!(looked(&app, leaves).unwrap().flattened, Some(kit.cube.clone()), "flat all the same");
    assert_eq!(status(&app)["outlined"], 1);
}

#[test]
fn an_outline_is_drawn_in_its_mesh_s_render_layers_and_follows_them() {
    let assets = Folder::new("layers");
    let mut app = drawn(&assets);
    let kit = kit(&mut app);
    let range = VisibilityRange::abrupt(0.0, 50.0);
    let body = mesh_with(&mut app, &kit.sphere, &kit.red, (Look::new("stone"), RenderLayers::layer(2), NoFrustumCulling, range.clone()));
    let plain = mesh_with(&mut app, &kit.cube, &kit.red, Look::new("stone"));
    app.update();
    let layers = |app: &App, e: Entity| app.world().get::<RenderLayers>(hull(app, e)).cloned();
    let outline = app.world().entity(hull(&app, body));
    assert!(outline.contains::<NoFrustumCulling>() && outline.get::<VisibilityRange>() == Some(&range));
    assert_eq!(layers(&app, body), Some(RenderLayers::layer(2)));
    let bare = app.world().entity(hull(&app, plain));
    assert!(!bare.contains::<RenderLayers>() && !bare.contains::<NoFrustumCulling>() && !bare.contains::<VisibilityRange>());

    app.world_mut().entity_mut(body).insert(RenderLayers::layer(3));
    app.update();
    assert_eq!(layers(&app, body), Some(RenderLayers::layer(3)));
    app.world_mut().entity_mut(body).remove::<RenderLayers>();
    app.update();
    assert_eq!(layers(&app, body), None);
    assert_eq!(outlines(&app, body), 1, "one outline, none left over");
    let outline = hull(&app, body);
    app.update();
    assert_eq!(hull(&app, body), outline, "and it stays");
}

#[test]
fn an_outline_is_placed_shown_and_bounded_in_the_frame_it_appears() {
    let assets = Folder::new("placed");
    let mut app = drawn(&assets);
    let kit = kit(&mut app);
    let body = mesh_with(&mut app, &kit.sphere, &kit.red, (Look::new("stone"), Transform::from_xyz(3.0, 0.0, 0.0)));
    app.update();
    // The look systems run before transforms propagate, and so before visibility and bounds.
    let outline = app.world().entity(hull(&app, body));
    assert_eq!(outline.get::<GlobalTransform>().map(GlobalTransform::translation), Some(Vec3::new(3.0, 0.0, 0.0)));
    assert_eq!(outline.get::<InheritedVisibility>().map(|v| v.get()), Some(true));
    assert!(outline.contains::<shards::bevy::camera::primitives::Aabb>(), "bounded");
}

#[test]
fn a_content_change_keeps_the_flat_copy_and_hull_of_a_mesh_whose_data_went_to_the_gpu() {
    let assets = Folder::new("gpu_only");
    let mut app = drawn(&assets);
    let kit = kit(&mut app);
    let body = mesh_with(&mut app, &kit.sphere, &kit.red, Look::new("hero"));
    app.update();
    let (flat, outline) = (mesh_of(&app, body), mesh_of(&app, hull(&app, body)));
    // What the render world does to a mesh only it keeps (`RenderAssetUsages::RENDER_WORLD`):
    // takes its data, unseen.
    app.world_mut().resource_mut::<Assets<Mesh>>().get_mut_untracked(&kit.sphere).unwrap().take_gpu_data().unwrap();
    let sphere = app.world().resource::<Assets<Mesh>>().get(&kit.sphere).unwrap();
    assert!(mesh::flat(sphere).is_none() && mesh::hull(sphere).is_none(), "they could not be made again");

    assets.put("content/look/test.look.ron", &LOOKS.replace("#408cff", "#ff0000"));
    ok(&mut app, "content_reload", json!({}));
    app.update();
    assert_eq!(toon(&app, body).base.base_color, srgb(0xff0000), "restyled");
    assert_eq!(mesh_of(&app, body), flat, "the same flat copy");
    assert_eq!(mesh_of(&app, hull(&app, body)), outline, "the same hull");
}

/// A preset, a look that extends it and one that extends that one.
const PRESETS: &str = r##"{
    "low_poly": (flat: true, outline: Some((width: 2.0, color: "#0b0f1a"))),
    "hero": (extends: "low_poly", color: Some("#408cff")),
    "boss": (extends: "hero", outline: Some((width: 4.0, color: "#ff0000"))),
}"##;

#[test]
fn editing_a_preset_restyles_the_looks_that_extend_it() {
    let assets = Folder::empty("presets");
    assets.put("content/look/presets.look.ron", PRESETS);
    let mut app = drawn(&assets);
    let kit = kit(&mut app);
    let hero = mesh_with(&mut app, &kit.sphere, &kit.red, Look::new("hero"));
    let boss = mesh_with(&mut app, &kit.sphere, &kit.red, Look::new("boss"));
    app.update();
    let line = |app: &App, e: Entity| {
        let material = &app.world().get::<MeshMaterial3d<OutlineMaterial>>(hull(app, e)).unwrap().0;
        let material = app.world().resource::<Assets<OutlineMaterial>>().get(material).unwrap();
        (material.width, material.color)
    };
    // Hero: its own colour, the preset's flat faces and outline. The boss: hero's colour and the
    // preset's flat faces through hero, its own outline.
    for e in [hero, boss] {
        assert_eq!(toon(&app, e).base.base_color, srgb(0x408cff));
        assert_eq!(looked(&app, e).unwrap().flattened, Some(kit.sphere.clone()));
    }
    assert_eq!((line(&app, hero), line(&app, boss)), ((2.0, srgb(0x0b0f1a).to_linear()), (4.0, srgb(0xff0000).to_linear())));
    let said = ok(&mut app, "looks", json!({}));
    assert!(said.contains("\n  boss      #408cff · toon 3 bands · flat · outline 4px #ff0000 · extends hero · 1 mesh\n"), "{said}");
    assert!(said.contains("\n  hero      #408cff · toon 3 bands · flat · outline 2px #0b0f1a · extends low_poly · 1 mesh\n"), "{said}");
    assert!(said.ends_with("\n  low_poly  toon 3 bands · flat · outline 2px #0b0f1a"), "{said}");

    // The preset trades its flat faces for two bands: both looks follow.
    assets.put("content/look/presets.look.ron", &PRESETS.replace("(flat: true,", "(shading: Toon(bands: 2),"));
    ok(&mut app, "content_reload", json!({}));
    app.update();
    for e in [hero, boss] {
        assert_eq!((looked(&app, e).unwrap().flattened, mesh_of(&app, e)), (None, kit.sphere.clone()));
        assert_eq!((toon(&app, e).extension.bands, outlines(&app, e)), (2, 1));
    }
    assert_eq!(line(&app, hero), (2.0, srgb(0x0b0f1a).to_linear()));
}
