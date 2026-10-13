//! Let the agent point at things: rings and labels drawn with gizmos in the game
//! window, so you see what it is talking about. Plus a dev overlay with every `eNN`.

use bevy::camera::primitives::Aabb;
use bevy::gizmos::config::{GizmoConfig, GizmoConfigGroup, GizmoConfigStore, GizmoLineConfig};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::id::{AiId, AiVisible};
use crate::observe::AiCamera;
use crate::session::Scope;
use crate::tools::{register_tool, Call, Id, ToolKind, ToolResult};

struct Highlight {
    entity: Entity,
    id: AiId,
    label: String,
    color: Color,
    until: f64,
}

#[derive(Resource, Default)]
pub struct Highlights(Vec<Highlight>);

/// Show every AI-visible entity's id above it (F1 in the sandbox). Players never see it.
#[derive(Resource, Default, Clone, Copy)]
pub struct AiOverlay {
    pub enabled: bool,
    /// Marked screenshots in progress; each keeps the overlay on until it is saved.
    pub shots: u32,
}

impl AiOverlay {
    pub fn visible(&self) -> bool {
        self.enabled || self.shots > 0
    }
}

/// Gizmo group for marks: drawn on top of the scene, so a pillar never hides an id.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct AiGizmos;

/// Point at an entity in the game window: a ring plus a label for a few seconds.
/// Use it to show the human what you mean ("hitbox is here").
#[derive(Deserialize, JsonSchema)]
pub struct HighlightArgs {
    pub id: Id,
    /// Text above the entity. Default: its id.
    pub label: Option<String>,
    /// How long to show it. Default 4.
    pub seconds: Option<f32>,
    /// "red", "green", "yellow", "cyan" or "white". Default yellow.
    pub color: Option<String>,
}

fn highlight_tool(In(call): In<Call<HighlightArgs>>, world: &mut World) -> ToolResult {
    let entity = call.args.id.entity(world)?;
    let now = world.resource::<Time<Real>>().elapsed_secs_f64();
    let color = match call.args.color.as_deref().unwrap_or("yellow") {
        "red" => Color::srgb(1.0, 0.2, 0.2),
        "green" => Color::srgb(0.2, 1.0, 0.3),
        "cyan" => Color::srgb(0.2, 0.9, 1.0),
        "white" => Color::WHITE,
        _ => Color::srgb(1.0, 0.9, 0.1),
    };
    let label = call.args.label.unwrap_or_else(|| call.args.id.to_string());
    let seconds = call.args.seconds.unwrap_or(4.0).clamp(0.1, 120.0) as f64;
    world.resource_mut::<Highlights>().0.push(Highlight {
        entity,
        id: call.args.id.0,
        label: label.clone(),
        color,
        until: now + seconds,
    });
    Ok(format!("highlighted {} \"{label}\" for {seconds}s", call.args.id))
}

/// Toggle the id overlay over every AI-visible entity.
#[derive(Deserialize, JsonSchema)]
pub struct OverlayArgs {
    pub on: bool,
}

fn overlay_tool(In(call): In<Call<OverlayArgs>>, mut overlay: ResMut<AiOverlay>) -> ToolResult {
    overlay.enabled = call.args.on;
    Ok(format!("overlay {}", if overlay.enabled { "on" } else { "off" }))
}

/// Top of what is drawn for `entity` (its own and its children's meshes).
fn visual_top(entity: Entity, bounds: &Query<(&GlobalTransform, &Aabb)>, children: &Query<&Children>) -> Option<f32> {
    std::iter::once(entity)
        .chain(children.get(entity).into_iter().flatten().copied())
        .filter_map(|e| bounds.get(e).ok())
        .map(|(gt, aabb)| gt.transform_point(Vec3::from(aabb.center)).y + aabb.half_extents.y * gt.scale().y)
        .reduce(f32::max)
}

pub(crate) fn draw_highlights(
    mut gizmos: Gizmos<AiGizmos>,
    mut highlights: ResMut<Highlights>,
    overlay: Res<AiOverlay>,
    time: Res<Time<Real>>,
    targets: Query<&GlobalTransform>,
    visible: Query<(Entity, &AiId, &GlobalTransform), With<AiVisible>>,
    camera: Query<&GlobalTransform, With<AiCamera>>,
    bounds: Query<(&GlobalTransform, &Aabb)>,
    children: Query<&Children>,
) {
    let now = time.elapsed_secs_f64();
    highlights.0.retain(|h| h.until > now && targets.contains(h.entity));
    let facing = camera.iter().next().map_or(Quat::IDENTITY, |c| c.rotation());
    let ground = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    let top = |e: Entity, p: Vec3| visual_top(e, &bounds, &children).unwrap_or(p.y + 1.8);

    // Invisible, every frame: keeps this group's line pipelines compiled, so the first
    // marked screenshot does not wait for the GPU to build them.
    gizmos.line(Vec3::ZERO, Vec3::Y, Color::NONE);
    gizmos.circle(Isometry3d::IDENTITY, 0.1, Color::NONE);

    for h in &highlights.0 {
        let Ok(gt) = targets.get(h.entity) else { continue };
        let p = gt.translation();
        let y = top(h.entity, p);
        let pulse = 1.0 + 0.08 * ((now * 6.0).sin() as f32);
        gizmos.circle(Isometry3d::new(p + Vec3::Y * 0.05, ground), 1.1 * pulse, h.color);
        gizmos.line(p.with_y(y + 0.2), p.with_y(y + 0.8), h.color);
        gizmos.text(Isometry3d::new(p.with_y(y + 1.2), facing), &h.label, 0.45, Vec2::ZERO, h.color);
    }

    if overlay.visible() {
        let marked: Vec<AiId> = highlights.0.iter().map(|h| h.id).collect();
        for (entity, id, gt) in visible.iter().filter(|(_, id, _)| !marked.contains(id)) {
            let p = gt.translation();
            let at = Isometry3d::new(p.with_y(top(entity, p) + 0.5), facing);
            gizmos.text(at, &id.to_string(), 0.35, Vec2::ZERO, Color::srgb(0.6, 1.0, 0.6));
        }
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<Highlights>().init_resource::<AiOverlay>();
    // Headless apps have no gizmo plugin: the tools still answer, nothing is drawn.
    // (Add AiCorePlugin after DefaultPlugins.)
    if app.is_plugin_added::<bevy::gizmos::GizmoPlugin>() {
        let config = GizmoConfig { depth_bias: -1.0, line: GizmoLineConfig { width: 2.5, ..default() }, ..default() };
        app.insert_gizmo_config(AiGizmos, config)
            .add_systems(PostUpdate, draw_highlights.run_if(resource_exists::<GizmoConfigStore>));
    }
    register_tool(app.world_mut(), "highlight", Scope::Player, ToolKind::Query, highlight_tool);
    register_tool(app.world_mut(), "overlay", Scope::Player, ToolKind::Query, overlay_tool);
}
