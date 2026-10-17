//! The effects module's tools: `effects` (the effects, their pools, the decals), `effect_play`
//! (play one now) and `telegraph_decals` (decals or the gizmo fill under telegraphs). Effects are
//! this machine's picture, so all three run where they are called, a co-op client too.

use ai_core::tools::{register_tool, ToolKind};
use ai_core::{Call, Id, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_content::ContentDb;

use crate::def::EffectDef;
use crate::{draws, fx_def, play, EffectAt, FxSettings, Particles};

pub(crate) fn register(world: &mut World) {
    register_tool(world, "effects", Scope::Dev, ToolKind::Query, effects);
    register_tool(world, "effect_play", Scope::Dev, ToolKind::Control, effect_play);
    register_tool(world, "telegraph_decals", Scope::Dev, ToolKind::Query, telegraph_decals);
}

/// The effects (`content/**/*.effect.ron`): each one's particles, colours and life, and its pool
/// (alive now / its size); the engine events and clip cues that play them (`*.fx.ron`); the
/// telegraph decals on the ground now (shape, size, how far the fill has grown, whose). Headless
/// nothing is drawn: effects play in a window or with --offscreen.
#[derive(Deserialize, JsonSchema, Default)]
pub struct EffectsArgs {}

fn effects(In(_): In<Call<EffectsArgs>>, world: &World) -> ToolResult {
    let db = world.get_resource::<ContentDb>().ok_or("this game has no content")?;
    let defs: Vec<(&str, &EffectDef)> = db.iter::<EffectDef>().collect();
    let particles = world.get_resource::<Particles>();
    let decals_on = world.get_resource::<FxSettings>().is_none_or(|s| s.decals);
    let drawn = draws(world);
    let mut out = if drawn {
        let live = particles.map_or(0, Particles::live);
        let pools: usize = particles.map_or(0, |p| p.effects.values().map(|e| e.pool.size()).sum());
        let decals = if decals_on { format!("telegraph decals on, on the ground: {}", crate::decals(world)) } else { "telegraph decals off".into() };
        format!("{} effects · {live} particles alive in pools of {pools} · {decals}\n", defs.len())
    } else {
        let decals = if decals_on { "on" } else { "off" };
        format!("{} effects · nothing is drawn headless: effects play in a window or with --offscreen · telegraph decals {decals}\n", defs.len())
    };
    if defs.is_empty() {
        out.push_str("no effect defined: add one in content/**/*.effect.ron\n");
    }
    let width = defs.iter().map(|(id, _)| id.len()).max().unwrap_or(0);
    for (id, def) in &defs {
        let pool = match particles.and_then(|p| p.effects.get(*id)).filter(|_| drawn) {
            Some(playing) => format!("pool {}/{}", playing.pool.live(), playing.pool.size()),
            None => format!("pool {}", def.pool_size()),
        };
        out.push_str(&format!("  {id:<width$}  {} · {pool}\n", def.describe()));
    }
    match fx_def(world) {
        Some(fx) => {
            let pairs = |map: &std::collections::BTreeMap<String, String>| {
                map.iter().map(|(from, to)| format!("{from} → {to}")).collect::<Vec<_>>().join(" · ")
            };
            if !fx.effects.is_empty() {
                out.push_str(&format!("events: {}\n", pairs(&fx.effects)));
            }
            if !fx.cues.is_empty() {
                out.push_str(&format!("cues: {}\n", pairs(&fx.cues)));
            }
        }
        None => out.push_str("no table of effects (`*.fx.ron`): engine events play none\n"),
    }
    #[cfg(feature = "render")]
    for line in crate::render::decal_lines(world) {
        out.push_str(&format!("  {line}\n"));
    }
    Ok(out.trim_end().to_owned())
}

/// Play an effect now, as the game would: its burst at an entity (`at=e6`, its feet plus the
/// effect's offset) or a point (`point=[4, 0, -2]`), and what its rate adds while it lasts. Needs
/// a picture (a window or --offscreen). Answers how many particles started and how full its pool
/// is.
#[derive(Deserialize, JsonSchema)]
pub struct EffectPlayArgs {
    /// The effect (`*.effect.ron` id): `hit_spark`.
    pub effect: String,
    /// At this entity: `e6`.
    pub at: Option<Id>,
    /// At this point: `[4.0, 0.0, -2.0]`.
    pub point: Option<[f32; 3]>,
    /// The way an effect with `aim: Hit` sends its particles (for a hit, back towards the
    /// attacker): `[1, 0, 0]`.
    pub toward: Option<[f32; 3]>,
}

fn effect_play(In(call): In<Call<EffectPlayArgs>>, world: &mut World) -> ToolResult {
    let EffectPlayArgs { effect, at, point, toward } = call.args;
    let (at, label) = match (at, point) {
        (Some(_), Some(_)) => return Err("give `at` (an entity) or `point` (x, y, z), not both".into()),
        (Some(id), None) => (EffectAt::Entity(id.entity(world)?), id.to_string()),
        (None, Some([x, y, z])) => (EffectAt::Point(Vec3::new(x, y, z)), "a point".to_owned()),
        (None, None) => return Err("give `at` (an entity: `at=e6`) or `point` (`point=[4, 0, -2]`)".into()),
    };
    let toward = toward.map(Vec3::from);
    let played = play(world, &effect, at, toward)?;
    let p = played.position;
    let later = if played.later > 0 { format!(" (+{} as it goes on)", played.later) } else { String::new() };
    Ok(format!(
        "played {effect} at {label} ({:.1}, {:.1}, {:.1}): {} particles{later}, {}/{} alive in its pool",
        p.x, p.y, p.z, played.particles, played.live, played.pool
    ))
}

/// Draw the ground telegraphs' fill as decals (on, the default) or with gizmo lines (`on=false`:
/// the hatch and the ring that grows). The outline, the symbol and its stem are gizmos either
/// way. Answers how many decals are on the ground.
#[derive(Deserialize, JsonSchema)]
pub struct TelegraphDecalsArgs {
    /// Default true.
    pub on: Option<bool>,
}

fn telegraph_decals(In(call): In<Call<TelegraphDecalsArgs>>, world: &mut World) -> ToolResult {
    let on = call.args.on.unwrap_or(true);
    world.resource_mut::<FxSettings>().decals = on;
    if !draws(world) {
        return Ok(format!("telegraph decals {} (nothing is drawn headless)", if on { "on" } else { "off" }));
    }
    Ok(if on {
        let shown = shards_action::draw::shown_telegraphs(world).len();
        format!("telegraph decals on: telegraphs on the ground now: {shown}")
    } else {
        "telegraph decals off: the gizmos draw the fill".to_owned()
    })
}
