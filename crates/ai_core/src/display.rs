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

/// Whether the engine's default font (`shards_ui::DEFAULT_FONT`, Fira Sans), which draws every
/// text that names no font, has a letter for `c`: printable ASCII, Latin-1 and Latin Extended-A,
/// Greek, Cyrillic, the common dashes, quotes and bullets, `№` and the four arrows. Text drawn
/// with it shows anything else as a stand-in.
pub fn default_font_has(c: char) -> bool {
    matches!(c,
        ' '..='~'
            | '\u{A0}'..='\u{17E}'
            | '\u{391}'..='\u{3A1}'
            | '\u{3A3}'..='\u{3C9}'
            | '\u{400}'..='\u{45F}'
            | '\u{2012}'..='\u{2016}'
            | '\u{2018}'..='\u{201A}'
            | '\u{201C}'..='\u{201E}'
            | '\u{2020}'..='\u{2022}'
            | '\u{2026}'
            | '\u{2116}'
            | '\u{2190}'..='\u{2193}')
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

#[cfg(test)]
mod tests {
    use super::default_font_has;

    #[test]
    fn the_default_font_has_the_letters_and_marks_the_engine_writes_and_nothing_else() {
        for c in "Az09 ÀéßŁœ ΑΩαω ЁЖжё —–‘’“”…•· № ←↑→↓".chars() {
            assert!(default_font_has(c), "{c:?}");
        }
        for c in ['\u{7F}', '\u{3A2}', '⚠', '✕', '中', '😀', '\u{460}'] {
            assert!(!default_font_has(c), "{c:?}");
        }
    }
}
