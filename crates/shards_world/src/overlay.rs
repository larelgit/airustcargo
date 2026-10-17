//! The tile overlay: every tile's bounds, the sockets along its edges (a color per kind,
//! roads orange), the points of interest and the roads or doors between them, drawn with
//! gizmos. Toggle it with the `world_overlay` tool; it draws in the window only.

use ai_core::dev::toggle_line;
use ai_core::highlight::AiGizmos;
use ai_core::prelude::*;
use ai_core::tools::{register_tool, ToolKind};
use ai_core::DevCommand;
use bevy::gizmos::config::GizmoConfigStore;
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_content::ContentDb;

use crate::content::TileDef;
use crate::grid::{turns_of, Dir};
use crate::manifest::{fnv, Layout};
use crate::region::ActiveRegion;

/// Whether the tile overlay is drawn.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct TileOverlay {
    pub enabled: bool,
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<TileOverlay>();
    register_tool(app.world_mut(), "world_overlay", Scope::Dev, ToolKind::Query, overlay_tool);
    app.add_dev_entry(
        "world",
        "world_overlay",
        "Tile bounds, sockets, roads",
        DevCommand::Live(|w, _| Ok(toggle_line("world_overlay", w.resource::<TileOverlay>().enabled))),
    );
    // Headless apps have no gizmo plugin: the tool still answers, nothing is drawn.
    if app.is_plugin_added::<bevy::gizmos::GizmoPlugin>() {
        app.add_systems(PostUpdate, draw_tiles.run_if(resource_exists::<GizmoConfigStore>));
    }
}

/// Draw the region in the game window: tile bounds, edge sockets (a color per kind, roads
/// orange), points of interest, roads and doors.
#[derive(Deserialize, JsonSchema)]
pub struct WorldOverlayArgs {
    /// Default true.
    pub on: Option<bool>,
}

fn overlay_tool(In(call): In<Call<WorldOverlayArgs>>, mut overlay: ResMut<TileOverlay>) -> ToolResult {
    overlay.enabled = call.args.on.unwrap_or(true);
    Ok(format!("world overlay {}", if overlay.enabled { "on" } else { "off" }))
}

/// One color per socket kind, the same every run.
fn kind_color(kind: &str, road: bool) -> Color {
    if road {
        return Color::srgb(1.0, 0.6, 0.15);
    }
    let hue = (fnv(kind.as_bytes()) % 360) as f32;
    Color::hsl(hue, 0.7, 0.55)
}

fn draw_tiles(mut gizmos: Gizmos<AiGizmos>, overlay: Res<TileOverlay>, region: Option<Res<ActiveRegion>>, db: Res<ContentDb>) {
    let (true, Some(region)) = (overlay.enabled, region) else { return };
    let m = &region.manifest;
    let s = m.tile_size;
    let lift = Vec3::Y * 0.05;
    let bounds = Color::srgba(0.8, 0.8, 0.85, 0.6);
    let rules = db.get::<crate::content::RegionDef>(&m.region).and_then(|r| db.get::<crate::content::SocketRulesDef>(&r.rules));
    let rules = rules.map(crate::sockets::Rules::new);
    for t in &m.tiles {
        let min = m.corner() + Vec3::new(t.at[0] as f32 * s, 0.0, t.at[1] as f32 * s) + lift;
        let max = min + Vec3::new(t.size[0] as f32 * s, 0.0, t.size[1] as f32 * s);
        let corners = [min, Vec3::new(max.x, min.y, min.z), max, Vec3::new(min.x, min.y, max.z)];
        for i in 0..4 {
            gizmos.line(corners[i], corners[(i + 1) % 4], bounds);
        }
        if m.layout != Layout::Grid {
            continue;
        }
        let (Some(def), Some(turns)) = (db.get::<TileDef>(&t.tile), turns_of(t.rot)) else { continue };
        let Ok(edges) = def.sockets.parse() else { continue };
        let center = m.tile_center(t) + lift;
        let inset = s * 0.5 - 0.4;
        for dir in Dir::ALL {
            let edge = &edges[(dir.index() + 4 - turns as usize) % 4];
            let (dx, dz) = dir.offset();
            let out = Vec3::new(dx as f32, 0.0, dz as f32);
            // Clockwise along this side: N runs west→east, E north→south, and so on.
            let along = Vec3::new(-out.z, 0.0, out.x);
            let start = center + out * inset - along * inset;
            let part = 2.0 * inset / edge.len() as f32;
            for (i, socket) in edge.iter().enumerate() {
                let road = rules.as_ref().is_some_and(|r| r.has_road(std::slice::from_ref(socket)));
                let a = start + along * (part * i as f32 + 0.2);
                let b = start + along * (part * (i + 1) as f32 - 0.2);
                gizmos.line(a, b, kind_color(&socket.kind, road));
            }
        }
    }
    for poi in &m.pois {
        gizmos.sphere(Isometry3d::from_translation(poi.position() + Vec3::Y * 0.5), 0.6, Color::srgb(1.0, 0.85, 0.2));
    }
    for link in &m.links {
        for pair in link.cells.windows(2) {
            let (a, b) = (m.cell_center(pair[0]) + lift * 4.0, m.cell_center(pair[1]) + lift * 4.0);
            gizmos.line(a, b, Color::srgb(1.0, 0.45, 0.1));
        }
    }
}
