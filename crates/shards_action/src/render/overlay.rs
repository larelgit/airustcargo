//! Ground telegraphs and the combat overlay, drawn with gizmos.
//!
//! Two gizmo groups: [`TelegraphGizmos`] for the warnings on the ground (depth tested, so a
//! wall hides them) and [`CombatOverlayGizmos`] for the developer overlay (always in
//! front: you want to see a hitbox through the model). Every frame an exclusive system asks
//! [`crate::draw`] for the two lists, from the world as the last tick left it; two small
//! systems hand them to the gizmos.
//!
//! The overlay is switched with the `combat_overlay` tool or a key ([`CombatOverlay::key`],
//! F2). Telegraphs are always on; [`TelegraphSettings`] can turn them off for a cutscene.

use ai_core::dev::toggle_line;
use ai_core::prelude::*;
use ai_core::tools::{register_tool, ToolKind};
use ai_core::DevCommand;
use bevy::gizmos::config::{GizmoConfig, GizmoConfigGroup, GizmoLineConfig};
use bevy::gizmos::AppGizmoBuilder;
use bevy::prelude::*;
use bevy::transform::TransformSystems;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::draw::{self, DrawList, OverlayParts, Prim, TelegraphStyle};

/// Gizmo group of the warnings on the ground.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct TelegraphGizmos;

/// Gizmo group of the developer overlay: drawn in front of everything.
#[derive(Default, Reflect, GizmoConfigGroup)]
pub struct CombatOverlayGizmos;

/// The combat overlay (TZ E11.2): hitboxes of open windows in red, hurtboxes in green, guard
/// arcs in blue (the perfect window brighter), i-frames as an outline round the body, every
/// zone on the ground with the time left. Switched by the `combat_overlay` tool and `key`.
#[derive(Resource, Clone, Debug)]
pub struct CombatOverlay {
    pub enabled: bool,
    pub parts: OverlayParts,
    /// Toggles `enabled` while a window has the keyboard. `None`: only the tool does.
    pub key: Option<KeyCode>,
}

impl Default for CombatOverlay {
    fn default() -> Self {
        Self { enabled: false, parts: OverlayParts::default(), key: Some(KeyCode::F2) }
    }
}

/// How ground telegraphs look. They are always on; a game turns them off for a
/// cutscene, never for play: danger has to read.
#[derive(Resource, Clone, Debug)]
pub struct TelegraphSettings {
    pub enabled: bool,
    pub style: TelegraphStyle,
}

impl Default for TelegraphSettings {
    fn default() -> Self {
        Self { enabled: true, style: TelegraphStyle::default() }
    }
}

/// This frame's drawings.
#[derive(Resource, Default, Debug)]
pub struct CombatDraw {
    pub telegraphs: DrawList,
    pub overlay: DrawList,
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<CombatOverlay>().init_resource::<TelegraphSettings>().init_resource::<CombatDraw>();
    register_tool(app.world_mut(), "combat_overlay", Scope::Dev, ToolKind::Query, overlay_tool);
    app.add_dev_entry(
        "combat",
        "combat_overlay",
        "Hitboxes, hurtboxes, attack zones (F2)",
        DevCommand::Live(|w, _| Ok(toggle_line("combat_overlay", w.resource::<CombatOverlay>().enabled))),
    );
    // Headless apps have no gizmo plugin: the tool still answers, nothing is drawn.
    if app.is_plugin_added::<bevy::gizmos::GizmoPlugin>() {
        app.insert_gizmo_config(
            TelegraphGizmos,
            GizmoConfig { depth_bias: -0.02, line: GizmoLineConfig { width: 3.0, ..default() }, ..default() },
        )
        .insert_gizmo_config(
            CombatOverlayGizmos,
            GizmoConfig { depth_bias: -1.0, line: GizmoLineConfig { width: 2.0, ..default() }, ..default() },
        )
        .add_systems(Update, toggle_with_key.run_if(ai_core::dev::keys_free))
        .add_systems(
            PostUpdate,
            (collect, (flush_telegraphs, flush_overlay)).chain().after(TransformSystems::Propagate).run_if(ai_core::display::drawn),
        );
    }
}

/// Draw hitboxes of open hit windows (red), hurtboxes (green), guard arcs (blue), i-frames
/// (outline) and every attack zone with its time left, in the game window. Answers how
/// many of each the world has now. The ground warnings of telegraphed attacks are always
/// drawn; this adds the rest.
#[derive(Deserialize, JsonSchema)]
pub struct OverlayArgs {
    /// Default true.
    pub on: Option<bool>,
}

fn overlay_tool(In(call): In<Call<OverlayArgs>>, world: &mut World) -> ToolResult {
    let on = call.args.on.unwrap_or(true);
    let parts = {
        let mut overlay = world.resource_mut::<CombatOverlay>();
        overlay.enabled = on;
        overlay.parts
    };
    if !on {
        return Ok("combat overlay off".into());
    }
    let counts = draw::overlay(world, parts, Quat::IDENTITY).1;
    Ok(format!("combat overlay on (drawn in the window or offscreen only): {}", counts.describe()))
}

fn toggle_with_key(keys: Option<Res<ButtonInput<KeyCode>>>, mut overlay: ResMut<CombatOverlay>) {
    let (Some(keys), Some(key)) = (keys, overlay.key) else { return };
    if keys.just_pressed(key) {
        overlay.enabled = !overlay.enabled;
    }
}

/// `PostUpdate`, after transforms: the drawings of this frame.
fn collect(world: &mut World) {
    let billboard = super::billboard(world);
    let time = world.get_resource::<Time<Real>>().map_or(0.0, |t| t.elapsed_secs());
    let settings = world.resource::<TelegraphSettings>().clone();
    let overlay = world.resource::<CombatOverlay>().clone();
    let telegraphs =
        if settings.enabled { draw::telegraphs(world, &settings.style, time, billboard) } else { DrawList::default() };
    let drawn = if overlay.enabled { draw::overlay(world, overlay.parts, billboard).0 } else { DrawList::default() };
    let mut out = world.resource_mut::<CombatDraw>();
    out.telegraphs = telegraphs;
    out.overlay = drawn;
}

fn flush_telegraphs(mut gizmos: Gizmos<TelegraphGizmos>, drawn: Res<CombatDraw>) {
    flush(&mut gizmos, &drawn.telegraphs);
}

fn flush_overlay(mut gizmos: Gizmos<CombatOverlayGizmos>, drawn: Res<CombatDraw>) {
    flush(&mut gizmos, &drawn.overlay);
}

/// Hands a list to the gizmos.
pub fn flush<G: GizmoConfigGroup>(gizmos: &mut Gizmos<G>, list: &DrawList) {
    for prim in &list.0 {
        match prim {
            Prim::Strip { points, color } => gizmos.linestrip(points.iter().copied(), *color),
            Prim::Line { a, b, color } => gizmos.line(*a, *b, *color),
            Prim::Text { at, rotation, text, size, color } => {
                gizmos.text(Isometry3d::new(*at, *rotation), text, *size, Vec2::ZERO, *color);
            }
        }
    }
}
