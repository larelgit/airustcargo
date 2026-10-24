//! The effects module's tools: `effects` (the effects, their pools, the marks, the debris, the decals),
//! `effect_play` (play one now), `mark_spawn` (lay a mark), `fx_clear` (take marks and debris away) and
//! `telegraph_decals` (decals or the gizmo fill under telegraphs). Effects are this machine's picture,
//! so all of them run where they are called, a co-op client too.

use ai_core::tools::{register_tool, ToolKind};
use ai_core::{Call, Id, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_content::ContentDb;

use crate::def::EffectDef;
use crate::gore::{clear, clear_all, spawn_mark, surface_along};
use crate::mark::MarkDef;
use crate::{debris_counts, draws, fx_def, play_with, EffectAt, FxSettings, GoreStats, Marks, Particles, PlayOptions};

pub(crate) fn register(world: &mut World) {
    register_tool(world, "effects", Scope::Dev, ToolKind::Query, effects);
    register_tool(world, "effect_play", Scope::Dev, ToolKind::Control, effect_play);
    register_tool(world, "telegraph_decals", Scope::Dev, ToolKind::Query, telegraph_decals);
    register_tool(world, "mark_spawn", Scope::Dev, ToolKind::Control, mark_spawn);
    register_tool(world, "fx_clear", Scope::Dev, ToolKind::Control, fx_clear);
}

/// The effects (`content/**/*.effect.ron`): each one's particles, colours and life, and its pool
/// (alive now / its size, and how many of those that land lie at rest); the engine events and clip cues
/// that play them (`*.fx.ron`) and the marks they lay; the marks (`content/**/*.mark.ron`) alive per kind;
/// the debris at rest and the rays it cast; the telegraph decals on the ground now (shape, size, how far
/// the fill has grown, whose). Headless nothing is drawn: effects play and marks are laid in a window or
/// with --offscreen.
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
            Some(playing) if playing.def.collide => {
                let resting = playing.pool.slots.iter().flatten().filter(|p| p.body.is_some_and(|b| b.rested)).count();
                format!("pool {}/{} ({resting} at rest)", playing.pool.live(), playing.pool.size())
            }
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
            for (event, links) in &fx.marks {
                let list: Vec<String> = links.iter().map(|link| format!("{} {:?}", link.mark, link.place)).collect();
                out.push_str(&format!("marks on {event}: {}\n", list.join(" · ")));
            }
        }
        None => out.push_str("no table of effects (`*.fx.ron`): engine events play none\n"),
    }
    out.push_str(&gore_lines(world, drawn));
    #[cfg(feature = "render")]
    for line in crate::render::decal_lines(world) {
        out.push_str(&format!("  {line}\n"));
    }
    Ok(out.trim_end().to_owned())
}

/// The marks and the debris: what each kind has alive of its cap, the landing particles and the rays they cast. Nothing
/// for a game that has neither marks nor effects that land.
fn gore_lines(world: &World, drawn: bool) -> String {
    let Some(db) = world.get_resource::<ContentDb>() else { return String::new() };
    let defs: Vec<(&str, &MarkDef)> = db.iter::<MarkDef>().collect();
    let lands = db.iter::<EffectDef>().any(|(_, def)| def.collide);
    if defs.is_empty() && !lands {
        return String::new();
    }
    let marks = world.get_resource::<Marks>();
    let mut out = String::new();
    if !defs.is_empty() {
        out = if drawn {
            format!("{} marks · {} alive\n", defs.len(), marks.map_or(0, Marks::live))
        } else {
            format!("{} marks · nothing is drawn headless: marks are laid in a window or with --offscreen\n", defs.len())
        };
    }
    let width = defs.iter().map(|(id, _)| id.len()).max().unwrap_or(0);
    for (id, def) in &defs {
        let alive = match marks.and_then(|m| m.kinds.get(*id)).filter(|_| drawn) {
            Some(pool) => format!("alive {}/{}", pool.live(), def.cap()),
            None if drawn => format!("alive 0/{}", def.cap()),
            None => format!("up to {}", def.cap()),
        };
        out.push_str(&format!("  {id:<width$}  {} · {alive}\n", def.describe()));
    }
    if drawn {
        let (resting, flying) = debris_counts(world);
        let stats = world.get_resource::<GoreStats>().copied().unwrap_or_default();
        out.push_str(&format!(
            "debris: {resting} at rest, {flying} in flight · rays last frame {}, most {}, budget {}\n",
            stats.rays, stats.rays_max, stats.budget
        ));
    }
    out
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
    /// attacker): `[1, 0, 0]`. For `aim: Surface`, the surface's normal.
    pub toward: Option<[f32; 3]>,
    /// How strong the play is (default 1): the effect's `strength` response scales its particles
    /// (by default their number and size): `1.5`.
    pub strength: Option<f32>,
    /// A velocity (m/s) every particle takes its share of (the effect's `push` range): `[0, 0, 3]`.
    pub push: Option<[f32; 3]>,
}

fn effect_play(In(call): In<Call<EffectPlayArgs>>, world: &mut World) -> ToolResult {
    let EffectPlayArgs { effect, at, point, toward, strength, push } = call.args;
    let (at, label) = match (at, point) {
        (Some(_), Some(_)) => return Err("give `at` (an entity) or `point` (x, y, z), not both".into()),
        (Some(id), None) => (EffectAt::Entity(id.entity(world)?), id.to_string()),
        (None, Some([x, y, z])) => (EffectAt::Point(Vec3::new(x, y, z)), "a point".to_owned()),
        (None, None) => return Err("give `at` (an entity: `at=e6`) or `point` (`point=[4, 0, -2]`)".into()),
    };
    let strength = strength.unwrap_or(1.0);
    if !(strength.is_finite() && strength >= 0.0) {
        return Err(format!("strength: a number, 0 or more (1 plays the effect as it is), not {strength}"));
    }
    let push = push.map_or(Vec3::ZERO, Vec3::from);
    if !push.is_finite() {
        return Err(format!("push: three numbers (m/s), not {push}"));
    }
    let played = play_with(world, &effect, at, PlayOptions { toward: toward.map(Vec3::from), strength, push })?;
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

/// Lay a mark (`content/**/*.mark.ron`) on a surface now, as the game would: at a `point` with the
/// surface's `normal`, or on the first world surface a ray finds from `from` along `toward` within
/// `reach` metres (default 20). Needs a picture (a window or --offscreen). Answers where it lies, how
/// big it is and how full its kind is.
#[derive(Deserialize, JsonSchema)]
pub struct MarkSpawnArgs {
    /// The mark (`*.mark.ron` id): `bullet_hole`.
    pub mark: String,
    /// Where it lies: `[4.0, 1.5, -2.0]`. Give it with `normal`.
    pub point: Option<[f32; 3]>,
    /// The surface's normal, facing the viewer: `[0, 0, 1]`.
    pub normal: Option<[f32; 3]>,
    /// Instead of `point` and `normal`: where the search for a surface starts.
    pub from: Option<[f32; 3]>,
    /// The way the search goes: `[0, -1, 0]` finds the floor.
    pub toward: Option<[f32; 3]>,
    /// How far the search reaches (m, default 20).
    pub reach: Option<f32>,
}

fn mark_spawn(In(call): In<Call<MarkSpawnArgs>>, world: &mut World) -> ToolResult {
    let MarkSpawnArgs { mark, point, normal, from, toward, reach } = call.args;
    let (point, normal) = match (point, normal, from, toward) {
        (Some(p), Some(n), None, None) => (Vec3::from(p), Vec3::from(n)),
        (None, None, Some(from), Some(toward)) => {
            let reach = reach.unwrap_or(20.0);
            if !(reach > 0.0 && reach.is_finite()) {
                return Err(format!("reach: metres above 0, not {reach}"));
            }
            let (from, toward) = (Vec3::from(from), Vec3::from(toward));
            surface_along(world, from, toward, reach).ok_or_else(|| {
                format!("no world surface within {reach} m from ({:.1}, {:.1}, {:.1}) along ({:.2}, {:.2}, {:.2})", from.x, from.y, from.z, toward.x, toward.y, toward.z)
            })?
        }
        _ => return Err("give `point` and `normal`, or `from` and `toward` (a search for the surface): `mark_spawn mark=blood point=[4,0,1] normal=[0,1,0]`".into()),
    };
    let marked = spawn_mark(world, &mark, point, normal)?;
    let n = normal.normalize_or_zero();
    let recycled = if marked.recycled { ", the oldest was taken" } else { "" };
    Ok(format!(
        "laid {mark} at ({:.2}, {:.2}, {:.2}) on a surface facing ({:.2}, {:.2}, {:.2}), {:.2} m across: {}/{} alive{recycled}",
        point.x, point.y, point.z, n.x, n.y, n.z, marked.size, marked.live, marked.cap
    ))
}

/// Take marks and landing particles away: those whose kind or effect carries `tag` (`tags` in the
/// content), or everything with `all=true`. Works headless too (nothing is there). Answers how many went.
#[derive(Deserialize, JsonSchema)]
pub struct FxClearArgs {
    /// A tag: `gore`.
    pub tag: Option<String>,
    /// Everything.
    pub all: Option<bool>,
}

fn fx_clear(In(call): In<Call<FxClearArgs>>, world: &mut World) -> ToolResult {
    let FxClearArgs { tag, all } = call.args;
    let (cleared, what) = match (tag, all.unwrap_or(false)) {
        (Some(_), true) => return Err("give a `tag` or `all=true`, not both".into()),
        (Some(tag), false) => (clear(world, &tag), format!("tagged `{tag}`")),
        (None, true) => (clear_all(world), "of every kind".to_owned()),
        (None, false) => return Err("give a `tag` (`fx_clear tag=gore`) or `all=true`".into()),
    };
    Ok(format!("cleared {} marks and {} particles {what}", cleared.marks, cleared.particles))
}
