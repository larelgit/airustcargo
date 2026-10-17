//! The navigation overlay: walkable cells near the players (green, water blue, carved red),
//! links across tile edges (cyan), navlinks (yellow arrows) and the routes NPCs follow
//! (orange), drawn with gizmos. Toggle it with `nav_overlay`; it draws in the window only.
//!
//! What it draws is worked out first, from the navmesh, the players and the NPCs
//! ([`NavDraw::of`]), kept in the [`NavDraw`] resource, and only then handed to the gizmos: a
//! test (no GPU, no window) reads the shapes of a frame there, the way the combat overlay's
//! `CombatDraw` is read.

use ai_core::highlight::AiGizmos;
use ai_core::AiPlayer;
use bevy::gizmos::config::GizmoConfigStore;
use bevy::prelude::*;
use shards_core::PlayerId;
use shards_world::overlay::Seg;

use super::region::NavRegion;
use super::tools::NavOverlay;
use super::NavAgent;

/// Cells farther than this from every player are not drawn (m).
pub const NEAR: f32 = 24.0;

/// How far above the ground the cells float (m); links, navlinks and routes float higher.
const LIFT: f32 = 0.06;

/// A walkable cell.
pub const CELL_COLOR: Color = Color::srgba(0.3, 0.95, 0.4, 0.7);
/// A cell to swim in.
pub const WATER_COLOR: Color = Color::srgb(0.25, 0.55, 1.0);
/// A cell switched off by carving: a solid the bake never saw stands on it.
pub const CARVED_COLOR: Color = Color::srgb(1.0, 0.25, 0.2);
/// A link across a tile edge.
pub const STITCH_COLOR: Color = Color::srgb(0.2, 0.9, 0.95);
/// A navlink.
pub const NAVLINK_COLOR: Color = Color::srgb(1.0, 0.9, 0.2);
/// The route an NPC follows.
pub const ROUTE_COLOR: Color = Color::srgb(1.0, 0.55, 0.1);

/// A cell: a cross of two lines centered on `at`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Cell {
    pub at: Vec3,
    pub color: Color,
}

/// A route: connected points from the NPC through the waypoints it still walks to.
#[derive(Clone, Debug, PartialEq)]
pub struct Route {
    pub points: Vec<Vec3>,
    pub color: Color,
}

/// What the overlay handed to the gizmos in the last frame (nothing while it is off).
#[derive(Resource, Default, Clone, Debug, PartialEq)]
pub struct NavDraw {
    /// Half the length of a cell's cross (m).
    pub arm: f32,
    /// The walkable cells within [`NEAR`] of a player (every cell while no player stands).
    pub cells: Vec<Cell>,
    /// A line for each link across a tile edge that starts near a player.
    pub stitches: Vec<Seg>,
    /// An arrow from the start of each navlink to its end, for those that start near a player.
    pub navlinks: Vec<Seg>,
    /// One for each NPC that has waypoints left.
    pub routes: Vec<Route>,
}

impl NavDraw {
    /// The shapes for `nav` (`None`: no region, so only the routes), the players at `players`
    /// and the NPCs, each with where it stands and the waypoints it has left.
    pub fn of<'a>(nav: Option<&NavRegion>, players: &[Vec3], agents: impl IntoIterator<Item = (Vec3, &'a [Vec3])>) -> Self {
        let mut out = Self::default();
        let lift = Vec3::Y * LIFT;
        if let Some(nav) = nav {
            let close = |p: Vec3| players.is_empty() || players.iter().any(|q| Vec2::new(p.x - q.x, p.z - q.z).length() <= NEAR);
            out.arm = nav.cell * 0.3;
            for node in 0..nav.node_count() {
                let p = nav.node_pos(node);
                if !close(p) {
                    continue;
                }
                let color = if nav.is_disabled(node) {
                    CARVED_COLOR
                } else if nav.is_swim(node) {
                    WATER_COLOR
                } else {
                    CELL_COLOR
                };
                out.cells.push(Cell { at: p + lift, color });
            }
            for extra in &nav.extras {
                let (a, b) = (nav.node_pos(extra.from) + lift * 2.0, nav.node_pos(extra.to) + lift * 2.0);
                if !close(a) {
                    continue;
                }
                if extra.navlink {
                    out.navlinks.push(Seg { a: a + Vec3::Y * 0.3, b: b + Vec3::Y * 0.3, color: NAVLINK_COLOR });
                } else {
                    out.stitches.push(Seg { a, b, color: STITCH_COLOR });
                }
            }
        }
        for (at, left) in agents {
            if left.is_empty() {
                continue;
            }
            let points = std::iter::once(at).chain(left.iter().copied()).map(|p| p + lift * 4.0).collect();
            out.routes.push(Route { points, color: ROUTE_COLOR });
        }
        out
    }

    /// Hands the shapes to the gizmos.
    fn flush(&self, gizmos: &mut Gizmos<AiGizmos>) {
        for c in &self.cells {
            gizmos.line(c.at - Vec3::X * self.arm, c.at + Vec3::X * self.arm, c.color);
            gizmos.line(c.at - Vec3::Z * self.arm, c.at + Vec3::Z * self.arm, c.color);
        }
        for s in &self.stitches {
            gizmos.line(s.a, s.b, s.color);
        }
        for s in &self.navlinks {
            gizmos.arrow(s.a, s.b, s.color);
        }
        for route in &self.routes {
            for pair in route.points.windows(2) {
                gizmos.line(pair[0], pair[1], route.color);
            }
        }
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<NavDraw>();
    // Headless apps have no gizmo plugin: the tool still answers, nothing is drawn.
    if app.is_plugin_added::<bevy::gizmos::GizmoPlugin>() {
        app.add_systems(PostUpdate, (collect, flush).chain().run_if(resource_exists::<GizmoConfigStore>));
    }
}

type Players<'w, 's> = Query<'w, 's, &'static Transform, Or<(With<PlayerId>, With<AiPlayer>)>>;

/// `PostUpdate`: this frame's shapes.
fn collect(overlay: Res<NavOverlay>, nav: Option<Res<NavRegion>>, players: Players, agents: Query<(&NavAgent, &Transform)>, mut drawn: ResMut<NavDraw>) {
    let next = if overlay.enabled {
        let players: Vec<Vec3> = players.iter().map(|tf| tf.translation).collect();
        let routes = agents.iter().map(|(agent, tf)| (tf.translation, agent.path.get(agent.index as usize..).unwrap_or_default()));
        NavDraw::of(nav.as_deref(), &players, routes)
    } else {
        NavDraw::default()
    };
    drawn.set_if_neq(next);
}

fn flush(mut gizmos: Gizmos<AiGizmos>, drawn: Res<NavDraw>) {
    drawn.flush(&mut gizmos);
}
