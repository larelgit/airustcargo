//! The NPC overlay: what each NPC near the players has on its mind. In the game window (F7,
//! `render` feature) it draws, with gizmos, its state above it, a line to its target (red
//! while it holds an attack token), its sight cone and near circle, where it last knew
//! targets and heard noises, and its leash around home (red when it stands beyond). The
//! `npc_overlay` tool answers the same as lines, one per NPC, anywhere.

use ai_core::dev::toggle_line;
use ai_core::spatial::{fmt_pos, relative};
use ai_core::tools::{register_tool, ToolKind};
use ai_core::{AiAppExt, AiId, AiPlayer, Call, DevCommand, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_core::{flat, PlayerId};

use crate::ai::npc_line;
use crate::brain::{Home, Npc, NpcBrain};
use crate::director::AttackToken;
use crate::nav::NavAgent;
use crate::perception::Perceived;

/// NPCs farther than this from every player are left out (m).
pub const NEAR: f32 = 40.0;

/// Whether the NPC overlay is drawn, and the key that toggles it in a window.
#[derive(Resource, Clone, Copy, Debug)]
pub struct NpcOverlay {
    pub enabled: bool,
    pub key: Option<KeyCode>,
}

impl Default for NpcOverlay {
    fn default() -> Self {
        Self { enabled: false, key: Some(KeyCode::F7) }
    }
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<NpcOverlay>();
    register_tool(app.world_mut(), "npc_overlay", Scope::Dev, ToolKind::Query, overlay_tool);
    app.add_dev_entry(
        "ai",
        "npc_overlay",
        "NPC minds: state, senses, leash (F7)",
        DevCommand::Live(|w, _| Ok(toggle_line("npc_overlay", w.resource::<NpcOverlay>().enabled))),
    )
    .add_dev_entry(
        "ai",
        "nav_overlay",
        "Navmesh and NPC routes",
        DevCommand::Live(|w, _| Ok(toggle_line("nav_overlay", w.resource::<crate::nav::NavOverlay>().enabled))),
    )
    .add_dev_entry("ai", "npc_state", "One NPC's mind...", DevCommand::Fill("npc_state id=".into()))
    .add_dev_entry("ai", "nav_info", "Navmesh of this region", DevCommand::Run("nav_info".into()));
    #[cfg(feature = "render")]
    window::plugin(app);
}

/// Show what the NPCs near the players have on their minds, in the game window (F7): state
/// and target above each, a line to the target (red with an attack token), sight cone, last
/// known places, leash around home. Answers the same as lines, one per NPC within 40 m of a
/// player: `e7 grunt: fight e5 · token · leash 6/15m · sees e5 4.2m NE`.
#[derive(Deserialize, JsonSchema)]
pub struct NpcOverlayArgs {
    /// Default true.
    pub on: Option<bool>,
}

fn overlay_tool(In(call): In<Call<NpcOverlayArgs>>, world: &mut World) -> ToolResult {
    let on = call.args.on.unwrap_or(true);
    world.resource_mut::<NpcOverlay>().enabled = on;
    if !on {
        return Ok("npc overlay off".into());
    }
    let lines = overlay_lines(world);
    let lines = if lines.is_empty() { format!("no NPC within {NEAR:.0}m of a player") } else { lines.join("\n") };
    Ok(format!("npc overlay on\n{lines}"))
}

/// Players' bodies: the overlay shows what is near them.
type Players = Or<(With<PlayerId>, With<AiPlayer>)>;

/// Whether `at` is within [`NEAR`] of a player (anywhere, when there is none).
fn near_players(players: &[Vec3], at: Vec3) -> bool {
    players.is_empty() || players.iter().any(|p| flat(*p - at).length() <= NEAR)
}

/// The overlay's lines: one per NPC near a player, in id order. `e7 grunt: fight e5 · token
/// · leash 6/15m · sees e5 4.2m NE · detour, 3 waypoints left`.
pub fn overlay_lines(world: &World) -> Vec<String> {
    let players: Vec<Vec3> = world
        .try_query_filtered::<&Transform, Players>()
        .map(|mut q| q.iter(world).map(|tf| tf.translation).collect())
        .unwrap_or_default();
    let Some(mut npcs) = world.try_query::<(
        &AiId,
        &Npc,
        &NpcBrain,
        &Transform,
        Option<&AttackToken>,
        Option<&Home>,
        Option<&Perceived>,
        Option<&NavAgent>,
    )>() else {
        return Vec::new();
    };
    let mut rows: Vec<_> = npcs.iter(world).filter(|row| near_players(&players, row.3.translation)).collect();
    rows.sort_by_key(|row| *row.0);
    rows.into_iter()
        .map(|(id, npc, brain, tf, token, home, perceived, agent)| {
            let here = tf.translation;
            let mut parts = vec![npc_line(brain, token, home, Some(tf)).unwrap_or_else(|| "(not started)".into())];
            if brain.active {
                parts.extend(perceived.map(|p| senses(here, p)).unwrap_or_default());
                parts.extend(agent.and_then(route));
            }
            format!("{id} {}: {}", npc.behavior, parts.join(" · "))
        })
        .collect()
}

/// What an NPC knows of its targets and the last noise: `sees e5 4.2m NE`, `hears e5 8.0m
/// S`, `lost e5 2.1s ago at (3.0, 4.0)`, `a noise 1.2s ago 6.0m W`.
fn senses(here: Vec3, perceived: &Perceived) -> Vec<String> {
    let mut out: Vec<String> = perceived
        .known
        .iter()
        .map(|k| match (k.seen, k.heard) {
            (true, _) => format!("sees {} {}", k.id, relative(here, k.pos)),
            (false, true) => format!("hears {} {}", k.id, relative(here, k.pos)),
            _ => format!("lost {} {:.1}s ago at {}", k.id, k.age, fmt_pos(k.pos)),
        })
        .collect();
    if let Some(noise) = perceived.noise {
        out.push(format!("a noise {:.1}s ago {}", noise.age, relative(here, noise.pos)));
    }
    out
}

/// Its way to its goal, when it is not a straight walk: `detour, 3 waypoints left`, `no
/// route`.
fn route(agent: &NavAgent) -> Option<String> {
    if agent.goal.is_none() || agent.direct || agent.arrived {
        return None;
    }
    let left = agent.path.len().saturating_sub(agent.index as usize);
    match left {
        0 if agent.path.is_empty() => Some("no route".into()),
        0 => None,
        n => Some(format!("detour, {n} waypoint{} left", if n == 1 { "" } else { "s" })),
    }
}

#[cfg(feature = "render")]
mod window {
    use std::f32::consts::FRAC_PI_2;

    use ai_core::highlight::AiGizmos;
    use ai_core::{AiCamera, AiIndex};
    use bevy::gizmos::config::GizmoConfigStore;
    use bevy::prelude::*;
    use shards_content::ContentDb;
    use shards_core::flat;

    use super::{near_players, NpcOverlay, Players};
    use crate::ai::{npc_line, LEASH_SLACK};
    use crate::brain::{Home, Npc, NpcBrain};
    use crate::def::PerceptionDef;
    use crate::director::AttackToken;
    use crate::perception::Perceived;

    pub(super) fn plugin(app: &mut App) {
        app.add_systems(Update, toggle_with_key.run_if(any_with_component::<Window>));
        // Headless apps have no gizmo plugin: the tool still answers, nothing is drawn.
        if app.is_plugin_added::<bevy::gizmos::GizmoPlugin>() {
            app.add_systems(PostUpdate, draw.run_if(resource_exists::<GizmoConfigStore>));
        }
    }

    fn toggle_with_key(keys: Option<Res<ButtonInput<KeyCode>>>, mut overlay: ResMut<NpcOverlay>) {
        let (Some(keys), Some(key)) = (keys, overlay.key) else { return };
        if keys.just_pressed(key) {
            overlay.enabled = !overlay.enabled;
        }
    }

    type Npcs<'w, 's> = Query<
        'w,
        's,
        (
            &'static Npc,
            &'static NpcBrain,
            &'static Transform,
            Option<&'static AttackToken>,
            Option<&'static Home>,
            Option<&'static Perceived>,
        ),
    >;

    const LIFT: Vec3 = Vec3::new(0.0, 0.08, 0.0);
    const CHEST: Vec3 = Vec3::new(0.0, 1.2, 0.0);

    #[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
    fn draw(
        mut gizmos: Gizmos<AiGizmos>,
        overlay: Res<NpcOverlay>,
        db: Option<Res<ContentDb>>,
        index: Option<Res<AiIndex>>,
        players: Query<&Transform, Players>,
        bodies: Query<&Transform>,
        npcs: Npcs,
        camera: Query<&GlobalTransform, With<AiCamera>>,
    ) {
        if !overlay.enabled {
            return;
        }
        let near: Vec<Vec3> = players.iter().map(|tf| tf.translation).collect();
        let facing = camera.iter().next().map_or(Quat::IDENTITY, |c| c.rotation());
        let ground = Quat::from_rotation_x(-FRAC_PI_2);
        let grey = Color::srgb(0.6, 0.6, 0.6);
        for (npc, brain, tf, token, home, perceived) in &npcs {
            let here = tf.translation;
            if !near_players(&near, here) {
                continue;
            }
            // The gizmo font is ASCII only.
            let label = npc_line(brain, token, home, Some(tf)).unwrap_or_default().replace('·', "|");
            let holds_token = token.is_some_and(|t| t.held.is_some());
            let text = if !brain.active { grey } else if holds_token { Color::srgb(1.0, 0.35, 0.3) } else { Color::srgb(1.0, 0.85, 0.4) };
            // Above the id the F1 overlay shows.
            gizmos.text(Isometry3d::new(here + Vec3::Y * 3.0, facing), &label, 0.28, Vec2::ZERO, text);
            if !brain.active {
                continue;
            }
            let def = perception_of(db.as_deref(), npc);
            let sees = perceived.is_some_and(|p| p.known.iter().any(|k| k.seen));
            let sight = if sees { Color::srgba(1.0, 0.9, 0.2, 0.8) } else { Color::srgba(0.4, 0.8, 1.0, 0.5) };
            cone(&mut gizmos, here + LIFT, *tf.forward(), &def, sight);
            gizmos.circle(Isometry3d::new(here + LIFT, ground), def.near, sight);
            if let Some(target) = brain.target
                && let Some(entity) = index.as_ref().and_then(|i| i.get(target))
                && let Ok(there) = bodies.get(entity)
            {
                let color = if holds_token { Color::srgb(1.0, 0.2, 0.2) } else { Color::srgb(1.0, 0.6, 0.15) };
                gizmos.line(here + CHEST, there.translation + CHEST, color);
            }
            for known in perceived.iter().flat_map(|p| p.known.iter()).filter(|k| !k.seen) {
                let p = known.pos + LIFT;
                let magenta = Color::srgb(0.95, 0.3, 0.9);
                gizmos.line(p - Vec3::new(0.3, 0.0, 0.3), p + Vec3::new(0.3, 0.0, 0.3), magenta);
                gizmos.line(p - Vec3::new(0.3, 0.0, -0.3), p + Vec3::new(0.3, 0.0, -0.3), magenta);
            }
            if let Some(noise) = perceived.and_then(|p| p.noise) {
                gizmos.circle(Isometry3d::new(noise.pos + LIFT, ground), 0.5, Color::srgb(0.7, 0.4, 1.0));
            }
            if let Some(home) = home
                && let Some(at) = home.pos
            {
                let beyond = flat(here - at).length() > home.leash + LEASH_SLACK;
                let color = if beyond { Color::srgb(1.0, 0.2, 0.2) } else { Color::srgba(0.3, 1.0, 0.4, 0.6) };
                gizmos.circle(Isometry3d::new(at + LIFT, ground), home.leash, color);
                gizmos.line(at + LIFT, at + Vec3::Y * 1.0, color);
            }
        }
    }

    /// How an NPC senses: its perception's numbers, the engine's defaults without one.
    fn perception_of(db: Option<&ContentDb>, npc: &Npc) -> PerceptionDef {
        db.and_then(|db| db.get::<PerceptionDef>(&npc.perception)).cloned().unwrap_or_default()
    }

    /// The sight cone: its two edges and the arc at the sight range (a circle when it sees
    /// all around).
    fn cone(gizmos: &mut Gizmos<AiGizmos>, at: Vec3, forward: Vec3, def: &PerceptionDef, color: Color) {
        const SEGMENTS: usize = 24;
        let ahead = flat(forward).normalize_or(Vec3::NEG_Z);
        let half = (def.fov_deg.clamp(0.0, 360.0) * 0.5).to_radians();
        let point = |k: usize| {
            let angle = -half + 2.0 * half * k as f32 / SEGMENTS as f32;
            at + Quat::from_rotation_y(angle) * ahead * def.sight_range
        };
        let arc: Vec<Vec3> = (0..=SEGMENTS).map(point).collect();
        if def.fov_deg < 360.0 {
            gizmos.line(at, arc[0], color);
            gizmos.line(at, arc[SEGMENTS], color);
        }
        gizmos.linestrip(arc, color);
    }
}
