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

/// The 3D world's own image, while it is drawn at a size other than the picture's
/// (`graphics.render_scale` not 1): the 3D cameras draw into it, and a camera of the picture's shows
/// it over the whole picture, the UI and the HUD at the picture's own resolution. The `shards`
/// crate keeps it; it is absent at scale 1, when the cameras draw into the picture itself.
#[derive(Resource, Clone, Debug)]
pub struct SceneImage {
    pub image: Handle<Image>,
    /// In pixels.
    pub size: UVec2,
    /// The scale asked for: `size` is the picture's size times this, rounded.
    pub scale: f32,
}

/// Something is drawn: there is a window, or the game draws offscreen. The run condition of
/// systems that only show things (the HUD, overlays, smoothing what is seen).
pub fn drawn(windows: Query<(), With<Window>>, offscreen: Option<Res<Offscreen>>) -> bool {
    offscreen.is_some() || !windows.is_empty()
}

/// The size, in physical pixels, of what `shot` captures: the primary window, else the offscreen
/// image; `None` headless.
pub fn picture_size(world: &World) -> Option<UVec2> {
    let window = world
        .try_query_filtered::<&Window, With<PrimaryWindow>>()
        .and_then(|mut q| q.iter(world).next().map(|w| UVec2::new(w.physical_width(), w.physical_height())));
    window.or_else(|| world.get_resource::<Offscreen>().map(|o| o.size))
}

/// `display` in `ai/status`: how the game shows itself (`window`, `offscreen` or `headless`), the
/// picture's size in pixels, the size the 3D world is drawn at (`scene`: `{"size": [w, h],
/// "scale": s}`, the picture's own at scale 1) and the GPU that draws it. `shot` works in a window
/// and offscreen.
pub fn status(world: &World) -> serde_json::Value {
    let window = world.try_query_filtered::<&Window, With<PrimaryWindow>>().is_some_and(|mut q| q.iter(world).next().is_some());
    let size = picture_size(world);
    let mode = match (window, size) {
        (true, _) => "window",
        (false, Some(_)) => "offscreen",
        (false, None) => "headless",
    };
    let scene = size.map(|picture| match world.get_resource::<SceneImage>() {
        Some(scene) => serde_json::json!({"size": [scene.size.x, scene.size.y], "scale": scene.scale}),
        None => serde_json::json!({"size": [picture.x, picture.y], "scale": 1.0}),
    });
    serde_json::json!({"mode": mode, "size": size.map(|s| [s.x, s.y]), "scene": scene, "gpu": crate::crash::gpu(world)})
}
