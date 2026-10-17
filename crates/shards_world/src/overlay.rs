//! The tile overlay: every tile's bounds, the sockets along its edges (a color per kind,
//! roads orange), the points of interest and the roads or doors between them, drawn with
//! gizmos. Toggle it with the `world_overlay` tool; it draws in the window only.
//!
//! What it draws is worked out first, from the manifest and the content ([`TileDraw::of`]),
//! kept in the [`TileDraw`] resource, and only then handed to the gizmos: a test (no GPU, no
//! window) reads the shapes of a frame there, the way the combat overlay's `CombatDraw` is read.

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
use crate::manifest::{fnv, Layout, RegionManifest};
use crate::region::ActiveRegion;

/// How far above the ground the shapes float, so the ground does not hide them (m).
const LIFT: f32 = 0.05;

/// The outline of a tile or room.
pub const BOUNDS_COLOR: Color = Color::srgba(0.8, 0.8, 0.85, 0.6);
/// The socket marks of an edge part that is a road.
pub const ROAD_COLOR: Color = Color::srgb(1.0, 0.6, 0.15);
/// The ball on a point of interest.
pub const POI_COLOR: Color = Color::srgb(1.0, 0.85, 0.2);
/// The line along a road or through a door.
pub const LINK_COLOR: Color = Color::srgb(1.0, 0.45, 0.1);
/// Radius of the ball on a point of interest (m).
pub const POI_RADIUS: f32 = 0.6;

/// Whether the tile overlay is drawn.
#[derive(Resource, Default, Clone, Copy, Debug)]
pub struct TileOverlay {
    pub enabled: bool,
}

/// A straight line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Seg {
    pub a: Vec3,
    pub b: Vec3,
    pub color: Color,
}

/// A closed outline on the ground: four corners, clockwise from the north-west one.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Outline {
    pub corners: [Vec3; 4],
    pub color: Color,
}

/// A ball.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ball {
    pub at: Vec3,
    pub radius: f32,
    pub color: Color,
}

/// What the overlay handed to the gizmos in the last frame (nothing while it is off).
#[derive(Resource, Default, Clone, Debug, PartialEq)]
pub struct TileDraw {
    /// One outline per placed tile or room, in the manifest's order.
    pub bounds: Vec<Outline>,
    /// A mark along the edge for each socket part of each tile, a color per socket kind and
    /// orange for roads. Grids only: rooms have doors, not sockets.
    pub sockets: Vec<Seg>,
    /// A ball on each point of interest.
    pub pois: Vec<Ball>,
    /// A line from cell to cell along each road, or through each door.
    pub links: Vec<Seg>,
}

impl TileDraw {
    /// The shapes for `manifest`, with the sockets of its tiles and the rules of its region
    /// from `db`.
    pub fn of(m: &RegionManifest, db: &ContentDb) -> Self {
        let mut out = Self::default();
        let s = m.tile_size;
        let lift = Vec3::Y * LIFT;
        let rules = db.get::<crate::content::RegionDef>(&m.region).and_then(|r| db.get::<crate::content::SocketRulesDef>(&r.rules));
        let rules = rules.map(crate::sockets::Rules::new);
        for t in &m.tiles {
            let min = m.corner() + Vec3::new(t.at[0] as f32 * s, 0.0, t.at[1] as f32 * s) + lift;
            let max = min + Vec3::new(t.size[0] as f32 * s, 0.0, t.size[1] as f32 * s);
            let corners = [min, Vec3::new(max.x, min.y, min.z), max, Vec3::new(min.x, min.y, max.z)];
            out.bounds.push(Outline { corners, color: BOUNDS_COLOR });
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
                let out_of = Vec3::new(dx as f32, 0.0, dz as f32);
                // Clockwise along this side: N runs west→east, E north→south, and so on.
                let along = Vec3::new(-out_of.z, 0.0, out_of.x);
                let start = center + out_of * inset - along * inset;
                let part = 2.0 * inset / edge.len() as f32;
                for (i, socket) in edge.iter().enumerate() {
                    let road = rules.as_ref().is_some_and(|r| r.has_road(std::slice::from_ref(socket)));
                    let a = start + along * (part * i as f32 + 0.2);
                    let b = start + along * (part * (i + 1) as f32 - 0.2);
                    out.sockets.push(Seg { a, b, color: kind_color(&socket.kind, road) });
                }
            }
        }
        for poi in &m.pois {
            out.pois.push(Ball { at: poi.position() + Vec3::Y * 0.5, radius: POI_RADIUS, color: POI_COLOR });
        }
        for link in &m.links {
            for pair in link.cells.windows(2) {
                let (a, b) = (m.cell_center(pair[0]) + lift * 4.0, m.cell_center(pair[1]) + lift * 4.0);
                out.links.push(Seg { a, b, color: LINK_COLOR });
            }
        }
        out
    }

    /// Hands the shapes to the gizmos.
    fn flush(&self, gizmos: &mut Gizmos<AiGizmos>) {
        for o in &self.bounds {
            for i in 0..4 {
                gizmos.line(o.corners[i], o.corners[(i + 1) % 4], o.color);
            }
        }
        for s in self.sockets.iter().chain(&self.links) {
            gizmos.line(s.a, s.b, s.color);
        }
        for b in &self.pois {
            gizmos.sphere(Isometry3d::from_translation(b.at), b.radius, b.color);
        }
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<TileOverlay>().init_resource::<TileDraw>();
    register_tool(app.world_mut(), "world_overlay", Scope::Dev, ToolKind::Query, overlay_tool);
    app.add_dev_entry(
        "world",
        "world_overlay",
        "Tile bounds, sockets, roads",
        DevCommand::Live(|w, _| Ok(toggle_line("world_overlay", w.resource::<TileOverlay>().enabled))),
    );
    // Headless apps have no gizmo plugin: the tool still answers, nothing is drawn.
    if app.is_plugin_added::<bevy::gizmos::GizmoPlugin>() {
        app.add_systems(PostUpdate, (collect, flush).chain().run_if(resource_exists::<GizmoConfigStore>));
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
pub fn kind_color(kind: &str, road: bool) -> Color {
    if road {
        return ROAD_COLOR;
    }
    let hue = (fnv(kind.as_bytes()) % 360) as f32;
    Color::hsl(hue, 0.7, 0.55)
}

/// `PostUpdate`: this frame's shapes.
fn collect(overlay: Res<TileOverlay>, region: Option<Res<ActiveRegion>>, db: Res<ContentDb>, mut drawn: ResMut<TileDraw>) {
    let next = match (overlay.enabled, region) {
        (true, Some(region)) => TileDraw::of(&region.manifest, &db),
        _ => TileDraw::default(),
    };
    drawn.set_if_neq(next);
}

fn flush(mut gizmos: Gizmos<AiGizmos>, drawn: Res<TileDraw>) {
    drawn.flush(&mut gizmos);
}
