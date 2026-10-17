//! The window's icon: `<assets>/icon.png`, when the game has one. (The program file's own icon,
//! the one Explorer shows, is put in by `shards package`.)

use bevy::asset::RenderAssetUsages;
use bevy::ecs::system::NonSendMarker;
use bevy::image::{CompressedImageFormats, ImageSampler, ImageType};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;
use bevy::window::PrimaryWindow;
use bevy::winit::WINIT_WINDOWS;

/// The icon, in the asset folder (beside `content/`). A PNG, square: 256×256 serves every size.
pub const ICON_FILE: &str = "icon.png";

pub(crate) struct WindowIconPlugin;

impl Plugin for WindowIconPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, read_icon).add_systems(Update, set_icon.run_if(resource_exists::<PendingIcon>));
    }
}

/// The icon as read at start, until the window it goes on exists.
#[derive(Resource)]
struct PendingIcon {
    rgba: Vec<u8>,
    width: u32,
    height: u32,
}

/// `Startup`: the icon file read and decoded. None: the platform's own icon.
fn read_icon(world: &mut World) {
    let path = shards_content::asset_root(world).join(ICON_FILE);
    let Ok(bytes) = std::fs::read(&path) else { return };
    match decode(&bytes) {
        Ok(icon) => world.insert_resource(icon),
        Err(e) => warn!("{} is not the window's icon: {e}", path.display()),
    }
}

fn decode(bytes: &[u8]) -> Result<PendingIcon, String> {
    let image = Image::from_buffer(
        bytes,
        ImageType::Extension("png"),
        CompressedImageFormats::NONE,
        true,
        ImageSampler::Default,
        RenderAssetUsages::default(),
    )
    .map_err(|e| e.to_string())?;
    let rgba = image.convert(TextureFormat::Rgba8UnormSrgb).ok_or("its pixels do not convert to RGBA")?;
    let (width, height) = (rgba.width(), rgba.height());
    let rgba = rgba.data.ok_or("it has no pixels")?;
    Ok(PendingIcon { rgba, width, height })
}

/// `Update`, on the main thread (winit's windows live there): the window gets the icon once it
/// exists, in its title bar and, on Windows, the taskbar.
fn set_icon(mut commands: Commands, window: Single<Entity, With<PrimaryWindow>>, icon: Res<PendingIcon>, _main_thread: NonSendMarker) {
    let set = WINIT_WINDOWS.with_borrow(|windows| {
        let winit = windows.get_window(*window)?;
        let set = winit::window::Icon::from_rgba(icon.rgba.clone(), icon.width, icon.height).map(|icon| {
            #[cfg(windows)]
            winit::platform::windows::WindowExtWindows::set_taskbar_icon(&**winit, Some(icon.clone()));
            winit.set_window_icon(Some(icon));
        });
        Some(set)
    });
    match set {
        // The window is not there yet.
        None => {}
        Some(Ok(())) => commands.remove_resource::<PendingIcon>(),
        Some(Err(e)) => {
            warn!("{ICON_FILE} is not the window's icon: {e}");
            commands.remove_resource::<PendingIcon>();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_icon_decodes_to_rgba_and_anything_else_says_why_not() {
        // A 1×1 opaque red PNG, RGBA.
        let png: &[u8] = &[
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00,
            0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x01, 0x73, 0x52, 0x47, 0x42, 0x00, 0xae, 0xce,
            0x1c, 0xe9, 0x00, 0x00, 0x00, 0x04, 0x67, 0x41, 0x4d, 0x41, 0x00, 0x00, 0xb1, 0x8f, 0x0b, 0xfc, 0x61, 0x05, 0x00, 0x00, 0x00, 0x09,
            0x70, 0x48, 0x59, 0x73, 0x00, 0x00, 0x0e, 0xc3, 0x00, 0x00, 0x0e, 0xc3, 0x01, 0xc7, 0x6f, 0xa8, 0x64, 0x00, 0x00, 0x00, 0x0d, 0x49,
            0x44, 0x41, 0x54, 0x18, 0x57, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0xa6, 0x5c, 0x9b, 0x5d, 0x00, 0x00,
            0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ];
        let icon = decode(png).unwrap();
        assert_eq!((icon.width, icon.height), (1, 1));
        assert_eq!(icon.rgba, [255, 0, 0, 255]);
        assert!(decode(b"not a png").is_err());
    }
}
