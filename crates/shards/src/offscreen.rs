//! `--offscreen` ([`EngineConfig::offscreen`](shards_core::EngineConfig::offscreen)): no window,
//! but the GPU draws. One image of the window's size stands in for the window
//! ([`ai_core::display::Offscreen`]): every camera aimed at the primary window (a camera's default)
//! draws into it, the camera that draws last shows the UI, as it would in the window, and `shot`
//! captures it. Nothing reads the image back but `shot`.

use ai_core::display::Offscreen;
use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::render_resource::{PollType, TextureFormat};
use bevy::render::renderer::RenderDevice;
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::ui::IsDefaultUiCamera;
use bevy::window::WindowRef;

/// The image and what aims the cameras at it.
pub(crate) struct OffscreenPlugin {
    /// In pixels ([`EngineConfig::window`](shards_core::EngineConfig::window)).
    pub size: (u32, u32),
}

impl Plugin for OffscreenPlugin {
    fn build(&self, app: &mut App) {
        let size = UVec2::new(self.size.0.max(1), self.size.1.max(1));
        let mut image = Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None);
        // Only the GPU reads it (`shot` copies it there): no copy in the main world.
        image.asset_usage = RenderAssetUsages::RENDER_WORLD;
        let image = app.world_mut().resource_mut::<Assets<Image>>().add(image);
        app.insert_resource(Offscreen { image, size })
            .add_observer(aim_at_the_image)
            .add_systems(PostUpdate, show_the_ui.before(bevy::ui::UiSystems::Prepare));
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.add_systems(Render, wait_for_the_gpu.in_set(RenderSystems::Cleanup));
        }
    }
}

/// No window means no swapchain, and nothing else makes a frame wait for the one before it: a GPU
/// slower than the run loop (a software one) would be handed frames faster than it draws them,
/// until it runs out of memory. Each frame waits until the GPU has drawn it.
fn wait_for_the_gpu(device: Res<RenderDevice>) {
    if let Err(e) = device.poll(PollType::wait_indefinitely()) {
        warn!("offscreen: waiting for the GPU failed: {e}");
    }
}

/// A camera aimed at the primary window is aimed at the image instead.
fn aim_at_the_image(insert: On<Insert<RenderTarget>>, mut targets: Query<&mut RenderTarget>, offscreen: Res<Offscreen>) {
    let Ok(mut target) = targets.get_mut(insert.entity) else {
        return;
    };
    if matches!(*target, RenderTarget::Window(WindowRef::Primary)) {
        *target = RenderTarget::Image(offscreen.image.clone().into());
    }
}

/// `IsDefaultUiCamera` the engine put on a camera, not the game.
#[derive(Component)]
struct UiShownHere;

/// A camera, where it draws, and whether it shows the UI: by the game's choice or the engine's.
type UiCamera<'a> = (Entity, &'a Camera, &'a RenderTarget, Has<IsDefaultUiCamera>, Has<UiShownHere>);

/// The UI goes to the camera that draws last into the image (the highest `order`), as Bevy
/// gives it to the one that draws last into the primary window, unless the game chose a camera
/// for it (`IsDefaultUiCamera`).
fn show_the_ui(mut commands: Commands, cameras: Query<UiCamera>, offscreen: Res<Offscreen>) {
    let chosen = cameras.iter().any(|(_, _, _, default, ours)| default && !ours);
    let last = cameras
        .iter()
        .filter(|(_, _, target, ..)| !chosen && matches!(target, RenderTarget::Image(i) if i.handle == offscreen.image))
        .max_by_key(|(entity, camera, ..)| (camera.order, *entity))
        .map(|(entity, ..)| entity);
    for (entity, _, _, _, ours) in &cameras {
        if ours && Some(entity) != last {
            commands.entity(entity).remove::<(IsDefaultUiCamera, UiShownHere)>();
        } else if !ours && Some(entity) == last {
            commands.entity(entity).insert((IsDefaultUiCamera, UiShownHere));
        }
    }
}
