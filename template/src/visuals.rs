//! Window-only layer: meshes, light and the camera. Gameplay never depends on anything
//! here, so the same game runs headless (and plays the same: the replay of a windowed
//! session verifies without a window).
//!
//! Bodies are drawn from their colliders (a box, a capsule), so changing a number in
//! content changes what you see. Real art goes in the same place: an observer or a system
//! that dresses an entity once it exists.

use shards::motion::Shape;
use shards::prelude::*;

use crate::brain::Grunt;

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
/// not been dressed yet.
fn dress(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    undressed: Query<(Entity, &Collider, Has<PlayerId>, Has<Grunt>), Without<Visibility>>,
) {
    for (entity, collider, is_hero, is_grunt) in &undressed {
        let mesh = match collider.shape {
            Shape::Cuboid { half } => meshes.add(Cuboid::new(half.x * 2.0, half.y * 2.0, half.z * 2.0)),
            Shape::Sphere { radius } => meshes.add(Sphere::new(radius)),
            Shape::Capsule { half_height, radius } => meshes.add(Capsule3d::new(radius, half_height * 2.0)),
            Shape::Cylinder { half_height, radius } => meshes.add(Cylinder::new(radius, half_height * 2.0)),
            Shape::Cone { half_height, radius } => meshes.add(Cone::new(radius, half_height * 2.0)),
        };
        let color = if is_hero {
            Color::srgb(0.25, 0.55, 1.0)
        } else if is_grunt {
            Color::srgb(0.85, 0.35, 0.3)
        } else {
            Color::srgb(0.3, 0.32, 0.38)
        };
        let material = materials.add(StandardMaterial { base_color: color, perceptual_roughness: 0.8, ..default() });
        commands
            .entity(entity)
            .insert(Visibility::default())
            .with_child((Mesh3d(mesh), MeshMaterial3d(material), Transform::from_translation(collider.offset)));
    }
}

/// A body that died is not drawn (there is no death animation in the template).
fn hide_the_dead(mut dead: Query<&mut Visibility, Added<Dead>>) {
    for mut visibility in &mut dead {
        *visibility = Visibility::Hidden;
    }
}
