//! The look module's tools: `looks` (what each look is and draws) and `look_outlines` (show or
//! hide the outlines).

use ai_core::tools::{register_tool, ToolKind};
use ai_core::{Call, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_content::ContentDb;

use crate::def::LookDef;
use crate::LookSettings;

pub(crate) fn register(world: &mut World) {
    register_tool(world, "looks", Scope::Dev, ToolKind::Query, looks);
    register_tool(world, "look_outlines", Scope::Dev, ToolKind::Query, look_outlines);
}

/// The looks (`content/**/*.look.ron`): each one's colour, shading, flat faces, outline, glow and
/// glTF materials as drawn (with what it takes from the look it extends, and which that is), and
/// how many meshes it draws now; then the meshes drawn without a look and
/// those waiting for their files. Headless nothing is drawn: looks style meshes in a window or
/// with --offscreen.
#[derive(Deserialize, JsonSchema, Default)]
pub struct LooksArgs {}

fn looks(In(_): In<Call<LooksArgs>>, world: &World) -> ToolResult {
    let db = world.get_resource::<ContentDb>().ok_or("this game has no content")?;
    let usage = crate::usage(world);
    let outlines = if world.get_resource::<LookSettings>().is_none_or(|s| s.outlines) { "on" } else { "off" };
    let defs: Vec<(&str, &LookDef)> = db.iter::<LookDef>().collect();
    let mut out = if crate::draws(world) {
        format!(
            "{} looks · {} meshes styled ({} outlined) · {} without a look · outlines {outlines}\n",
            defs.len(),
            usage.total(),
            usage.outlined,
            usage.unstyled
        )
    } else {
        format!("{} looks · nothing is drawn headless: looks style meshes in a window or with --offscreen\n", defs.len())
    };
    if defs.is_empty() {
        out.push_str("no look defined: add one in content/**/*.look.ron\n");
    }
    let width = defs.iter().map(|(id, _)| id.len()).max().unwrap_or(0);
    for (id, def) in &defs {
        let count = usage.styled.get(*id).copied().unwrap_or(0);
        let meshes = match count {
            0 => String::new(),
            1 => " · 1 mesh".into(),
            n => format!(" · {n} meshes"),
        };
        // As drawn: what it takes from the looks it extends too.
        let drawn = def.resolve(|base| db.get::<LookDef>(base));
        out.push_str(&format!("  {id:<width$}  {}{meshes}\n", drawn.describe()));
    }
    match usage.waiting {
        0 => {}
        1 => out.push_str("1 mesh waits for its material or mesh to load\n"),
        n => out.push_str(&format!("{n} meshes wait for their material or mesh to load\n")),
    }
    Ok(out.trim_end().to_owned())
}

/// Show or hide the looks' outlines (they stay made: showing them again is free). Answers how many
/// there are.
#[derive(Deserialize, JsonSchema)]
pub struct LookOutlinesArgs {
    /// Default true.
    pub on: Option<bool>,
}

fn look_outlines(In(call): In<Call<LookOutlinesArgs>>, world: &mut World) -> ToolResult {
    let on = call.args.on.unwrap_or(true);
    world.resource_mut::<LookSettings>().outlines = on;
    let outlined = crate::usage(world).outlined;
    Ok(format!("outlines {}: {outlined} drawn with an outline", if on { "on" } else { "off" }))
}
