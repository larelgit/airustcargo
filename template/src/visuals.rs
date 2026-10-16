//! Picture-only layer (a window or `--offscreen`): meshes, light and the camera. Gameplay never depends on anything
//! here, so the same game runs headless (and plays the same: the replay of a windowed
//! session verifies without a window).
//!
//! Bodies are drawn from their colliders (a box, a capsule), so changing a number in
//! content changes what you see. Real art goes in the same place: an observer or a system
//! that dresses an entity once it exists.

use shards::look::Look;
use shards::motion::Shape;
use shards::prelude::*;
use shards::world::RegionEdge;

pub struct VisualsPlugin;

impl Plugin for VisualsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(Color::srgb(0.05, 0.06, 0.09)))
            .insert_resource(GlobalAmbientLight { brightness: 300.0, ..default() })
            .add_systems(Startup, setup_scene)
            .add_systems(Update, (dress, hide_the_dead));
    }
}

/// Light, and the camera that follows the entity with `LocalPlayer` (the hero). The
/// engine drives it from the mouse and the right stick; `AiCamera` lets the AI layer
/// say where things are on screen (`scr(x,y)` in `observe`, labels in `shards shot`).
fn setup_scene(mut commands: Commands) {
    commands.spawn((
        DirectionalLight { illuminance: 9000.0, shadow_maps_enabled: true, ..default() },
        Transform::from_xyz(8.0, 18.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((Camera3d::default(), AiCamera, ThirdPersonCamera::default()));
}

/// Gives every collider a mesh of its own shape, once: an entity without `Visibility` has
/// not been dressed yet. The wall around a region's edge stays invisible. The engine's looks
/// (content/looks.look.ron) give the colour and the light: a body's prefab names its look,
/// everything else is stone.
fn dress(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut white: Local<Option<Handle<StandardMaterial>>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    undressed: Query<(Entity, &Collider, Has<Look>), (Without<Visibility>, Without<RegionEdge>)>,
) {
    let white = white.get_or_insert_with(|| materials.add(StandardMaterial::default())).clone();
    for (entity, collider, looked) in &undressed {
        let mesh = match collider.shape {
            Shape::Cuboid { half } => meshes.add(Cuboid::new(half.x * 2.0, half.y * 2.0, half.z * 2.0)),
            Shape::Sphere { radius } => meshes.add(Sphere::new(radius)),
            Shape::Capsule { half_height, radius } => meshes.add(Capsule3d::new(radius, half_height * 2.0)),
            Shape::Cylinder { half_height, radius } => meshes.add(Cylinder::new(radius, half_height * 2.0)),
            Shape::Cone { half_height, radius } => meshes.add(Cone::new(radius, half_height * 2.0)),
        };
        let body = (Mesh3d(mesh), MeshMaterial3d(white.clone()), Transform::from_translation(collider.offset));
        let mut e = commands.entity(entity);
        e.insert(Visibility::default());
        // On the mesh, not the body: the picture adds nothing the simulation keeps.
        if looked { e.with_child(body) } else { e.with_child((body, Look::new("stone"))) };
    }
}

/// A body that died is not drawn (there is no death animation in the template).
fn hide_the_dead(mut dead: Query<&mut Visibility, Added<Dead>>) {
    for mut visibility in &mut dead {
        *visibility = Visibility::Hidden;
    }
}
