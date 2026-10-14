//! The navigation overlay: walkable cells near the players (green, water blue, carved red),
//! links across tile edges (cyan), navlinks (yellow arrows) and the routes NPCs follow
//! (orange), drawn with gizmos. Toggle it with `nav_overlay`; it draws in the window only.

use ai_core::highlight::AiGizmos;
use ai_core::AiPlayer;
use bevy::gizmos::config::GizmoConfigStore;
use bevy::prelude::*;
use shards_core::PlayerId;

use super::region::NavRegion;
use super::tools::NavOverlay;
use super::NavAgent;

/// Cells farther than this from every player are not drawn (m).
const NEAR: f32 = 24.0;

pub(crate) fn plugin(app: &mut App) {
    // Headless apps have no gizmo plugin: the tool still answers, nothing is drawn.
    if app.is_plugin_added::<bevy::gizmos::GizmoPlugin>() {
        app.add_systems(PostUpdate, draw_nav.run_if(resource_exists::<GizmoConfigStore>));
    }
}

type Players<'w, 's> = Query<'w, 's, &'static Transform, Or<(With<PlayerId>, With<AiPlayer>)>>;

fn draw_nav(mut gizmos: Gizmos<AiGizmos>, overlay: Res<NavOverlay>, nav: Option<Res<NavRegion>>, players: Players, agents: Query<(&NavAgent, &Transform)>) {
    if !overlay.enabled {
        return;
    }
    let lift = Vec3::Y * 0.06;
    if let Some(nav) = nav {
        let near: Vec<Vec3> = players.iter().map(|tf| tf.translation).collect();
        let close = |p: Vec3| near.is_empty() || near.iter().any(|q| Vec2::new(p.x - q.x, p.z - q.z).length() <= NEAR);
        let half = nav.cell * 0.3;
        for node in 0..nav.node_count() {
            let p = nav.node_pos(node);
            if !close(p) {
                continue;
            }
            let color = if nav.is_disabled(node) {
                Color::srgb(1.0, 0.25, 0.2)
            } else if nav.is_swim(node) {
                Color::srgb(0.25, 0.55, 1.0)
            } else {
                Color::srgba(0.3, 0.95, 0.4, 0.7)
            };
            let p = p + lift;
            gizmos.line(p - Vec3::X * half, p + Vec3::X * half, color);
            gizmos.line(p - Vec3::Z * half, p + Vec3::Z * half, color);
        }
        for extra in &nav.extras {
            let (a, b) = (nav.node_pos(extra.from) + lift * 2.0, nav.node_pos(extra.to) + lift * 2.0);
            if !close(a) {
                continue;
            }
            if extra.navlink {
                gizmos.arrow(a + Vec3::Y * 0.3, b + Vec3::Y * 0.3, Color::srgb(1.0, 0.9, 0.2));
            } else {
                gizmos.line(a, b, Color::srgb(0.2, 0.9, 0.95));
            }
        }
    }
    for (agent, tf) in &agents {
        let mut at = tf.translation + lift * 4.0;
        for p in agent.path.iter().skip(agent.index as usize) {
            let next = *p + lift * 4.0;
            gizmos.line(at, next, Color::srgb(1.0, 0.55, 0.1));
            at = next;
        }
    }
}
