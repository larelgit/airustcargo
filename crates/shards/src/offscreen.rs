//! `--offscreen` ([`EngineConfig::offscreen`](shards_core::EngineConfig::offscreen)): no window,
//! but the GPU draws. One image of the window's size stands in for the window
//! ([`ai_core::display::Offscreen`]): every camera aimed at the primary window (a camera's default)
//! draws into it, the camera that draws last shows the UI, as it would in the window, and `shot`
//! captures it. Nothing reads the image back but `shot`.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use ai_core::display::Offscreen;
use bevy::asset::RenderAssetUsages;
use bevy::camera::RenderTarget;
use bevy::prelude::*;
use bevy::render::render_resource::{PollType, TextureFormat};
use bevy::render::renderer::RenderDevice;
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::ui::IsDefaultUiCamera;
use bevy::window::WindowRef;

/// The longest the GPU may take to draw a frame. Past it the GPU is stuck (a hung driver, a lost
/// device it never reports), and the game ends instead of waiting forever.
const FRAME_LIMIT: Duration = Duration::from_secs(30);

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
        let stuck = GpuStuck::default();
        app.insert_resource(Offscreen { image, size })
            .insert_resource(stuck.clone())
            .add_observer(aim_at_the_image)
            .add_systems(
                PostUpdate,
                (
                    aim_changed_targets.before(bevy::camera::CameraUpdateSystems),
                    show_the_ui.after(aim_changed_targets).before(bevy::ui::UiSystems::Prepare),
                ),
            )
            .add_systems(Last, end_when_stuck);
        if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
            render_app.insert_resource(stuck).add_systems(Render, wait_for_the_gpu.in_set(RenderSystems::Cleanup));
        }
    }
}

/// Set by the render world when a frame did not finish within [`FRAME_LIMIT`]; read by the main
/// world, which ends the game.
#[derive(Resource, Clone, Default)]
struct GpuStuck(Arc<AtomicBool>);

/// No window means no swapchain, and nothing else makes a frame wait for the one before it: a GPU
/// slower than the run loop (a software one) would be handed frames faster than it draws them,
/// until it runs out of memory. Each frame waits until the GPU has drawn it, for
/// [`FRAME_LIMIT`] at most: going on after that would pile frames up again.
fn wait_for_the_gpu(device: Res<RenderDevice>, stuck: Res<GpuStuck>) {
    if device.poll(PollType::Wait { submission_index: None, timeout: Some(FRAME_LIMIT) }).is_err() {
        stuck.0.store(true, Ordering::Relaxed);
    }
}

/// A stuck GPU draws no more frames, and `shot` would wait for one forever: the game says why and
/// ends (exit 1).
fn end_when_stuck(stuck: Res<GpuStuck>, mut exit: MessageWriter<AppExit>, mut ended: Local<bool>) {
    if *ended || !stuck.0.load(Ordering::Relaxed) {
        return;
    }
    *ended = true;
    error!("offscreen: the GPU did not finish a frame in {}s: the game ends", FRAME_LIMIT.as_secs());
    exit.write(AppExit::error());
}

/// A camera aimed at the primary window is aimed at the image instead, from the moment its target
/// is inserted.
fn aim_at_the_image(insert: On<Insert<RenderTarget>>, mut targets: Query<&mut RenderTarget>, offscreen: Res<Offscreen>) {
    if let Ok(target) = targets.get_mut(insert.entity) {
        aim(target, &offscreen);
    }
}

/// A target a game system changed in place (no insert, so no observer) is aimed the same way,
/// before Bevy works out where each camera draws.
fn aim_changed_targets(mut targets: Query<&mut RenderTarget, Changed<RenderTarget>>, offscreen: Res<Offscreen>) {
    for target in &mut targets {
        aim(target, &offscreen);
    }
}

fn aim(mut target: Mut<RenderTarget>, offscreen: &Offscreen) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stuck_gpu_ends_the_game() {
        let mut app = App::new();
        let stuck = GpuStuck::default();
        app.insert_resource(stuck.clone()).add_systems(Last, end_when_stuck);
        app.update();
        assert_eq!(app.should_exit(), None, "a GPU that keeps up");
        stuck.0.store(true, Ordering::Relaxed);
        app.update();
        assert_eq!(app.should_exit(), Some(AppExit::error()));
    }

    /// A camera aimed at the window, on insert or by a game system changing its target later,
    /// draws into the image; one aimed at an image of its own keeps it.
    #[test]
    fn every_camera_aimed_at_the_window_draws_into_the_image() {
        let mut app = App::new();
        let mut images = Assets::<Image>::default();
        let (image, own) = (images.add(Image::default()), images.add(Image::default()));
        app.insert_resource(Offscreen { image: image.clone(), size: UVec2::ONE })
            .add_observer(aim_at_the_image)
            .add_systems(PostUpdate, aim_changed_targets);
        let drawn = |app: &App, camera: Entity| match app.world().get::<RenderTarget>(camera) {
            Some(RenderTarget::Image(target)) => Some(target.handle.clone()),
            _ => None,
        };
        let inserted = app.world_mut().spawn(RenderTarget::Window(WindowRef::Primary)).id();
        let retargeted = app.world_mut().spawn(RenderTarget::Image(own.clone().into())).id();
        assert_eq!(drawn(&app, inserted).as_ref(), Some(&image), "on insert");
        app.update();
        *app.world_mut().get_mut::<RenderTarget>(retargeted).unwrap() = RenderTarget::Window(WindowRef::Primary);
        app.update();
        assert_eq!(drawn(&app, retargeted).as_ref(), Some(&image), "changed in place");
        let kept = app.world_mut().spawn(RenderTarget::Image(own.clone().into())).id();
        app.update();
        assert_eq!(drawn(&app, kept), Some(own));
    }
}
