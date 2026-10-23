//! The tool `camera_mode`: look through the first-person camera, or back through the orbit one, for
//! this session. A developer's (or an agent's) way to see what a first-person game shows without editing
//! the `camera` content: it sets [`CameraModeOverride`], which the camera mode system reads over the
//! content's `mode`. Presentation only: the tool is a query (no journal line, no replay record) and
//! nothing in the simulation reads the mode.

use ai_core::prelude::*;
use ai_core::tools::{register_tool, ToolKind};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{CameraMode, CameraModeOverride, FirstPersonCamera};
use crate::camera::CameraTuning;

pub(super) fn plugin(app: &mut App) {
    register_tool(app.world_mut(), "camera_mode", Scope::Dev, ToolKind::Query, camera_mode);
}

/// Look through the first-person camera (`mode=first`), back through the orbit camera (`mode=third`),
/// or let the `camera` content decide again (`mode=content`). Without `mode`, says which it is.
#[derive(Deserialize, JsonSchema)]
pub struct CameraModeArgs {
    /// `first`, `third` or `content`.
    pub mode: Option<String>,
}

fn camera_mode(
    In(call): In<Call<CameraModeArgs>>,
    mut chosen: ResMut<CameraModeOverride>,
    tuning: Res<CameraTuning>,
    cameras: Query<Has<super::FromMode>, With<FirstPersonCamera>>,
) -> ToolResult {
    if let Some(mode) = call.args.mode.as_deref() {
        chosen.0 = match mode {
            "first" | "first_person" => Some(CameraMode::FirstPerson),
            "third" | "third_person" => Some(CameraMode::ThirdPerson),
            "content" => None,
            other => return Err(format!("no mode `{other}`: say `first`, `third` or `content` (the camera content's own)")),
        };
    }
    let name = |mode: CameraMode| match mode {
        CameraMode::FirstPerson => "first person",
        CameraMode::ThirdPerson => "third person",
    };
    let now = chosen.0.unwrap_or(tuning.mode);
    let who = if chosen.0.is_some() { "this session's choice" } else { "the camera content" };
    // The mode system works in the next frame: say what the cameras are now.
    let looking = cameras.iter().count();
    Ok(format!(
        "camera mode: {} ({who}; the content says {}); {looking} first-person camera{} now{}",
        name(now),
        name(tuning.mode),
        if looking == 1 { "" } else { "s" },
        if chosen.is_changed() && call.args.mode.is_some() { " (the cameras change on the next frame)" } else { "" },
    ))
}
