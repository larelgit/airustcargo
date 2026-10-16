//! Where the picture goes: a window, an offscreen image (`--offscreen`), or nowhere (headless).
//!
//! Offscreen, the game has a GPU but no window: every camera that would draw into the primary
//! window draws into one image instead (the engine's `shards` crate sets that up), the HUD and the
//! overlays go into it too, and [`shot`](crate::shot) captures it. CI and agents on a machine with
//! a GPU (or a software one) get pictures without a display.

use bevy::prelude::*;
use bevy::window::{PrimaryWindow, Window};

/// The image the game draws into instead of a window. Present only offscreen.
#[derive(Resource, Clone, Debug)]
pub struct Offscreen {
    pub image: Handle<Image>,
    /// In pixels: the window size the game was configured with.
    pub size: UVec2,
}

/// Something is drawn: there is a window, or the game draws offscreen. The run condition of
/// systems that only show things (the HUD, overlays, smoothing what is seen).
pub fn drawn(windows: Query<(), With<Window>>, offscreen: Option<Res<Offscreen>>) -> bool {
    offscreen.is_some() || !windows.is_empty()
}

/// `display` in `ai/status`: how the game shows itself (`window`, `offscreen` or `headless`), the
/// picture's size in pixels and the GPU that draws it. `shot` works in a window and offscreen.
pub fn status(world: &World) -> serde_json::Value {
    let window = world
        .try_query_filtered::<&Window, With<PrimaryWindow>>()
        .and_then(|mut q| q.iter(world).next().map(|w| [w.physical_width(), w.physical_height()]));
    let offscreen = world.get_resource::<Offscreen>().map(|o| [o.size.x, o.size.y]);
    let (mode, size) = match (window, offscreen) {
        (Some(size), _) => ("window", Some(size)),
        (None, Some(size)) => ("offscreen", Some(size)),
        (None, None) => ("headless", None),
    };
    serde_json::json!({"mode": mode, "size": size, "gpu": crate::crash::gpu(world)})
}
