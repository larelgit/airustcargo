//! The scatter module's tools: `scatter` (what each scatter is and has strewn) and `scatter_set`
//! (take it all away, or strew it again).

use ai_core::tools::{register_tool, ToolKind};
use ai_core::{Call, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_content::ContentDb;

use crate::def::ScatterDef;
use crate::ScatterState;

pub(crate) fn register(world: &mut World) {
    register_tool(world, "scatter", Scope::Dev, ToolKind::Query, scatter);
    register_tool(world, "scatter_set", Scope::Dev, ToolKind::Query, scatter_set);
}

const HEADLESS: &str = "nothing is strewn headless: scatter is drawn in a window or with --offscreen";

/// The scatters (`content/**/*.scatter.ron`): each one's density, levels of detail, looks and
/// tiles, and how many instances it has strewn on how many tiles now, with those that found no
/// place (ground out of the height band or inside a solid, too steep, too close to a solid).
/// Headless nothing is strewn: scatter is drawn in a window or with --offscreen.
#[derive(Deserialize, JsonSchema, Default)]
pub struct ScatterArgs {}

fn scatter(In(_): In<Call<ScatterArgs>>, world: &World) -> ToolResult {
    let db = world.get_resource::<ContentDb>().ok_or("this game has no content")?;
    let state = world.get_resource::<ScatterState>().ok_or("this game has no scatter module")?;
    let defs: Vec<(&str, &ScatterDef)> = db.iter::<ScatterDef>().collect();
    let s = |n: usize| if n == 1 { "" } else { "s" };
    let strewn = crate::strewn(world);
    let mut out = format!("{} scatter{} · ", defs.len(), s(defs.len()));
    if !crate::draws(world) {
        out.push_str(HEADLESS);
    } else if world.get_resource::<shards_world::ActiveRegion>().is_none() {
        out.push_str("no region is loaded: scatter is strewn over a region's tiles (`regen`)");
    } else {
        let total = strewn.total();
        out.push_str(&format!(
            "{total} instance{} ({} mesh entities) on {} tiles · foliage {} · scatter {}",
            s(total),
            strewn.entities,
            strewn.tiles(),
            state.foliage,
            if state.on { "on" } else { "off" }
        ));
    }
    out.push('\n');
    if defs.is_empty() {
        out.push_str("no scatter defined: add one in content/**/*.scatter.ron\n");
    }
    let width = defs.iter().map(|(id, _)| id.len()).max().unwrap_or(0);
    for (id, def) in &defs {
        out.push_str(&format!("  {id:<width$}  {}", def.describe()));
        let instances = strewn.instances.get(*id).copied().unwrap_or(0);
        if instances > 0 {
            let tiles = strewn.tiles.get(*id).map_or(0, |t| t.len());
            out.push_str(&format!(" · {instances} on {tiles} tile{}", s(tiles)));
        }
        if let Some(skipped) = state.skipped.get(*id).filter(|k| k.total() > 0) {
            let mut why = Vec::new();
            for (n, what) in [(skipped.ground, "no ground in the band"), (skipped.steep, "too steep"), (skipped.crowded, "too close to a solid")] {
                if n > 0 {
                    why.push(format!("{n} {what}"));
                }
            }
            out.push_str(&format!(" · {} found no place ({})", skipped.total(), why.join(", ")));
        }
        out.push('\n');
    }
    Ok(out.trim_end().to_owned())
}

/// Take every scatter away (`on=false`), or strew them again (`on=true`, the default). How dense
/// they are is the player's setting: `settings_set group=graphics key=foliage value=0.5`.
#[derive(Deserialize, JsonSchema)]
pub struct ScatterSetArgs {
    /// Default true.
    pub on: Option<bool>,
}

fn scatter_set(In(call): In<Call<ScatterSetArgs>>, world: &mut World) -> ToolResult {
    let on = call.args.on.unwrap_or(true);
    let mut state = world.resource_mut::<ScatterState>();
    state.on = on;
    state.redo = true;
    let word = if on { "on" } else { "off" };
    if !crate::draws(world) {
        return Ok(format!("scatter {word}: {HEADLESS}"));
    }
    #[cfg(feature = "render")]
    crate::render::place(world);
    let strewn = crate::strewn(world);
    Ok(format!("scatter {word}: {} instances on {} tiles", strewn.total(), strewn.tiles()))
}
