//! `graphics.render_scale`: the resolution the 3D world is drawn at, as a multiple of the
//! picture's pixels (the window's, or the `--offscreen` image's). The UI, the HUD and the
//! overlay labels stay at the picture's own resolution.
//!
//! At 1.0 nothing is added: the 3D cameras draw into the window (or the offscreen image) as ever.
//! At any other scale
//!
//! * every `Camera3d` aimed at the picture draws into an [`Image`] of `scale` x the picture's size
//!   instead, resized with the window and with the setting (`SceneImage`, [`ai_core::display`]);
//! * one `Camera2d` aimed at the picture, above the 3D cameras, shows that image over the whole
//!   picture through a UI [`ImageNode`] under every screen, with linear filtering, and carries the UI.
//!
//! Bevy's `MainPassResolutionOverride` is not used: it only shrinks the main pass's viewport inside
//! a full-size texture and leaves the upscaling to an upscaler (DLSS), so without one the image
//! sits small in a corner of a picture that tone mapping and the rest still process in full.
//!
//! The target's scale factor is `scale x the picture's scale factor`, so a camera's *logical*
//! viewport is still the picture's logical size: `world_to_viewport`, the HUD's markers and the
//! `eN` marks of `shot` ask for the same coordinates as before.
//!
//! Only the picture changes: no entity here has an `AiId`, and nothing is in a snapshot or a replay.

use ai_core::display::{Offscreen, SceneImage};
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{Camera, Camera2d, Camera3d, ClearColorConfig, ImageRenderTarget, Projection, RenderTarget};
use bevy::image::ImageSampler;
use bevy::picking::Pickable;
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureFormat, TextureUsages};
use bevy::render::view::Msaa;
use bevy::window::{PrimaryWindow, Window};
use shards_settings::GraphicsSettings;

/// The plugin: one system, every frame, after the settings and before the cameras are worked out.
pub(crate) struct RenderScalePlugin;

impl Plugin for RenderScalePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            PostUpdate,
            apply.before(bevy::camera::CameraUpdateSystems).before(crate::offscreen::aim_changed_targets),
        );
    }
}

/// The longest side the scene image gets: wgpu's default texture limit, which every GPU the engine
/// draws on has. A larger window at a scale above 1 is drawn at this, and shown stretched.
pub const MAX_SIDE: u32 = 8192;

/// The size, in pixels, the 3D world is drawn at for a picture of `picture` pixels: `scale` times
/// it, rounded, at least 1x1 and at most [`MAX_SIDE`] a side (both sides shrink alike, so the
/// aspect holds).
pub fn scene_size(picture: UVec2, scale: f32) -> UVec2 {
    let longest = picture.max_element() as f32 * scale;
    let fit = if longest > MAX_SIDE as f32 { MAX_SIDE as f32 / longest } else { 1.0 };
    let scaled = |n: u32| ((n as f32 * scale * fit).round() as u32).clamp(1, MAX_SIDE);
    UVec2::new(scaled(picture.x), scaled(picture.y))
}

/// Whether the 3D world needs an image of its own at this scale: a scale that rounds to the
/// picture's own size draws straight into the picture.
pub fn needs_scene(picture: UVec2, scale: f32) -> bool {
    scene_size(picture, scale) != picture
}

/// The scale factor of the 3D target: the picture's, times how much smaller (or larger) the image
/// is. A camera's logical viewport, the target's pixels over this, is then the picture's logical
/// size.
pub fn target_scale_factor(scene: UVec2, picture: UVec2, picture_scale_factor: f32) -> f32 {
    picture_scale_factor * scene.x as f32 / picture.x.max(1) as f32
}

/// A camera that draws into the [`SceneImage`], and where it drew before.
#[derive(Component)]
struct Redirected(RenderTarget);

/// The camera that shows the scene image.
#[derive(Component)]
struct SceneCamera;

/// The node that shows the scene image, under every screen.
#[derive(Component)]
struct SceneBackdrop;

/// What the picture is, in pixels, and its scale factor: the primary window, or the offscreen image.
fn picture(windows: &Query<&Window, With<PrimaryWindow>>, offscreen: Option<&Offscreen>) -> Option<(UVec2, f32)> {
    match windows.single() {
        Ok(window) => {
            let size = UVec2::new(window.resolution.physical_width(), window.resolution.physical_height());
            // A minimized window has no size: the scene waits as it is.
            (size.x > 0 && size.y > 0).then_some((size, window.resolution.scale_factor()))
        }
        Err(_) => offscreen.map(|o| (o.size, 1.0)),
    }
}

/// Where a camera aimed at the picture draws.
fn picture_target(offscreen: Option<&Offscreen>) -> RenderTarget {
    match offscreen {
        Some(offscreen) => RenderTarget::Image(offscreen.image.clone().into()),
        None => RenderTarget::default(),
    }
}

/// Whether `target` is the picture: the primary window, or the offscreen image.
fn is_picture(target: &RenderTarget, offscreen: Option<&Offscreen>) -> bool {
    match target {
        RenderTarget::Window(bevy::window::WindowRef::Primary) => true,
        RenderTarget::Image(image) => offscreen.is_some_and(|o| image.handle == o.image),
        _ => false,
    }
}

/// The 3D target of an image at `scale_factor`.
fn scene_target(image: &Handle<Image>, scale_factor: f32) -> RenderTarget {
    RenderTarget::Image(ImageRenderTarget { handle: image.clone(), scale_factor })
}

/// The scene image: a target texture nothing is written to by the CPU (no data: it has no copy
/// there), sampled linearly where it is shown, and readable for a `shot` of it.
fn new_scene_image(size: UVec2) -> Image {
    let mut image = Image::new_target_texture(size.x, size.y, TextureFormat::Rgba8UnormSrgb, None);
    image.data = None;
    image.texture_descriptor.usage |= TextureUsages::COPY_SRC;
    image.sampler = ImageSampler::linear();
    image
}

/// The camera that shows the scene (never a 3D one).
type Displays<'w, 's> = Query<'w, 's, (Entity, &'static mut Camera), (With<SceneCamera>, Without<Camera3d>)>;

type Cameras3d<'w, 's> = Query<'w, 's, (Entity, &'static Camera, &'static mut RenderTarget, &'static mut Projection, Option<&'static Redirected>), With<Camera3d>>;

#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
fn apply(
    mut commands: Commands,
    settings: Option<Res<GraphicsSettings>>,
    windows: Query<&Window, With<PrimaryWindow>>,
    offscreen: Option<Res<Offscreen>>,
    scene: Option<ResMut<SceneImage>>,
    mut images: ResMut<Assets<Image>>,
    mut cameras: Cameras3d,
    mut shown: Displays,
    backdrop: Query<Entity, With<SceneBackdrop>>,
) {
    let offscreen = offscreen.as_deref();
    let Some((picture_size, picture_sf)) = picture(&windows, offscreen) else {
        return;
    };
    let scale = settings.map_or(1.0, |s| s.scale());
    let size = scene_size(picture_size, scale);

    // The 3D cameras that draw the picture: those aimed at it, and those already redirected whose
    // target is still the scene image (a game that aims one elsewhere keeps it there).
    let image = scene.as_ref().map(|s| s.image.clone());
    let ours = |target: &RenderTarget, image: &Option<Handle<Image>>| matches!((target, image), (RenderTarget::Image(t), Some(i)) if t.handle == *i);
    let drawing = cameras
        .iter()
        .filter(|(.., target, _, redirected)| if redirected.is_some() { ours(target, &image) } else { is_picture(target, offscreen) })
        .count();

    if size == picture_size || drawing == 0 {
        // Native: every redirected camera is aimed back, and what showed the scene goes.
        for (_, _, mut target, mut projection, redirected) in &mut cameras {
            let Some(Redirected(before)) = redirected else { continue };
            if ours(&target, &image) {
                *target = before.clone();
                projection.set_changed();
            }
        }
        for (entity, _, _, _, redirected) in cameras.iter() {
            if redirected.is_some() {
                commands.entity(entity).remove::<Redirected>();
            }
        }
        for (entity, _) in &shown {
            commands.entity(entity).despawn();
        }
        for entity in &backdrop {
            commands.entity(entity).despawn();
        }
        if scene.is_some() {
            commands.remove_resource::<SceneImage>();
        }
        return;
    }

    let scale_factor = target_scale_factor(size, picture_size, picture_sf);
    let (handle, resized) = match &image {
        Some(handle) => {
            let resized = images.get(handle).is_some_and(|i| i.size() != size);
            if resized && let Some(mut i) = images.get_mut(handle) {
                i.resize(Extent3d { width: size.x, height: size.y, depth_or_array_layers: 1 });
            }
            (handle.clone(), resized)
        }
        None => (images.add(new_scene_image(size)), true),
    };
    let target = scene_target(&handle, scale_factor);
    let mut order = isize::MIN;
    for (entity, camera, mut current, mut projection, redirected) in &mut cameras {
        match redirected {
            None if is_picture(&current, offscreen) => {
                commands.entity(entity).insert(Redirected(current.clone()));
                *current = target.clone();
                projection.set_changed();
            }
            Some(_) if ours(&current, &image) => {
                // Only a change is written, so a steady scale leaves the camera alone.
                let stale = !matches!(&*current, RenderTarget::Image(t) if t.scale_factor == scale_factor);
                if stale {
                    *current = target.clone();
                }
                if stale || resized {
                    projection.set_changed();
                }
            }
            Some(_) => {
                // The game aimed it elsewhere since: it is the game's again.
                commands.entity(entity).remove::<Redirected>();
                continue;
            }
            None => continue,
        }
        order = order.max(camera.order);
    }

    // What shows it: above the 3D cameras, at the picture.
    let above = order.saturating_add(1);
    match shown.single_mut() {
        Ok((_, mut camera)) => {
            if camera.order != above {
                camera.order = above;
            }
        }
        Err(_) => {
            let camera = Camera { order: above, clear_color: ClearColorConfig::Custom(Color::BLACK), ..default() };
            // No render layers: it shows an image and the UI, and sees no mesh (its orthographic box
            // would otherwise count the 3D meshes near the origin as visible to it).
            let display = commands.spawn((Name::new("scene display"), SceneCamera, Camera2d, camera, picture_target(offscreen), Msaa::Off, RenderLayers::none())).id();
            let node = Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                ..default()
            };
            let image = ImageNode { image_mode: NodeImageMode::Stretch, ..ImageNode::new(handle.clone()) };
            commands.spawn((Name::new("scene backdrop"), SceneBackdrop, node, image, UiTargetCamera(display), GlobalZIndex(i32::MIN), Pickable::IGNORE));
        }
    }

    let now = SceneImage { image: handle, size, scale };
    match scene {
        Some(mut scene) => {
            if scene.size != size || scene.scale != scale {
                *scene = now;
            }
        }
        None => commands.insert_resource(now),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_scene_is_the_picture_times_the_scale_rounded_and_never_empty() {
        let hd = UVec2::new(1280, 720);
        assert_eq!(scene_size(hd, 0.5), UVec2::new(640, 360));
        assert_eq!(scene_size(hd, 0.75), UVec2::new(960, 540));
        assert_eq!(scene_size(hd, 2.0), UVec2::new(2560, 1440));
        assert_eq!(scene_size(UVec2::new(1001, 3), 0.5), UVec2::new(501, 2), "rounded, not cut");
        assert_eq!(scene_size(UVec2::new(1, 1), 0.5), UVec2::ONE, "a window of one pixel still has a picture");
        assert_eq!(scene_size(UVec2::new(1920, 1080), 0.333), UVec2::new(639, 360));
    }

    #[test]
    fn a_scene_never_outgrows_the_texture_limit_and_keeps_its_aspect() {
        assert_eq!(scene_size(UVec2::new(3840, 2160), 2.0), UVec2::new(7680, 4320), "a 4K window supersampled fits");
        let big = scene_size(UVec2::new(7680, 4320), 2.0);
        assert_eq!(big, UVec2::new(MAX_SIDE, 4608), "the long side at the limit, the short one in proportion");
        assert!(!needs_scene(UVec2::new(MAX_SIDE, MAX_SIDE), 2.0), "a picture at the limit has no room to supersample: drawn straight into it");
    }

    #[test]
    fn a_scale_that_changes_no_pixel_needs_no_scene_of_its_own() {
        let hd = UVec2::new(1280, 720);
        assert!(!needs_scene(hd, 1.0));
        assert!(!needs_scene(hd, 1.0001), "rounds to the picture's own size");
        assert!(needs_scene(hd, 0.5) && needs_scene(hd, 1.25) && needs_scene(hd, 2.0));
    }

    #[test]
    fn the_target_has_the_pictures_logical_size_at_any_scale() {
        // A 2560x1440 window at the system scale 2 is 1280x720 logical; the scene half the pixels.
        let (picture, scene) = (UVec2::new(2560, 1440), UVec2::new(1280, 720));
        let factor = target_scale_factor(scene, picture, 2.0);
        assert_eq!(factor, 1.0);
        assert_eq!(scene.as_vec2() / factor, Vec2::new(1280.0, 720.0));
        // Supersampled: twice the pixels of a window at the scale 1.
        let factor = target_scale_factor(UVec2::new(2560, 1440), UVec2::new(1280, 720), 1.0);
        assert_eq!(UVec2::new(2560, 1440).as_vec2() / factor, Vec2::new(1280.0, 720.0));
    }

    /// A game of the scale `scale` with a picture of `size` (a window when `window`, the offscreen
    /// image otherwise) and one 3D camera of order 3 aimed at it.
    fn game(scale: f64, size: (u32, u32), window: bool) -> (App, Entity) {
        let mut app = App::new();
        let mut images = Assets::<Image>::default();
        let picture = images.add(Image::default());
        app.insert_resource(images).insert_resource(GraphicsSettings { render_scale: scale, ..default() }).add_systems(PostUpdate, apply);
        let target = if window {
            app.world_mut().spawn((Window { resolution: size.into(), ..default() }, PrimaryWindow));
            RenderTarget::default()
        } else {
            app.insert_resource(Offscreen { image: picture.clone(), size: size.into() });
            RenderTarget::Image(picture.into())
        };
        let camera = app.world_mut().spawn((Camera3d::default(), Camera { order: 3, ..default() }, target)).id();
        (app, camera)
    }

    fn set_scale(app: &mut App, scale: f64) {
        app.world_mut().resource_mut::<GraphicsSettings>().render_scale = scale;
    }

    fn count<F: bevy::ecs::query::QueryFilter>(app: &mut App) -> usize {
        app.world_mut().query_filtered::<Entity, F>().iter(app.world()).count()
    }

    fn scene(app: &App) -> Option<SceneImage> {
        app.world().get_resource::<SceneImage>().cloned()
    }

    fn drawn_into(app: &App, camera: Entity) -> RenderTarget {
        app.world().get::<RenderTarget>(camera).unwrap().clone()
    }

    #[test]
    fn at_scale_one_nothing_is_added_and_the_cameras_draw_into_the_picture() {
        let (mut app, camera) = game(1.0, (1280, 720), false);
        app.update();
        assert!(scene(&app).is_none());
        assert_eq!(count::<With<SceneCamera>>(&mut app) + count::<With<SceneBackdrop>>(&mut app), 0, "no extra camera, no extra node");
        assert!(app.world().get::<Redirected>(camera).is_none());
        assert_eq!(app.world().resource::<Assets<Image>>().len(), 1, "no image but the picture's own");
        let RenderTarget::Image(target) = drawn_into(&app, camera) else { panic!("aimed at the picture") };
        assert_eq!(target.handle, app.world().resource::<Offscreen>().image);
    }

    #[test]
    fn another_scale_draws_the_world_into_an_image_of_its_own_and_shows_it_over_the_picture() {
        let (mut app, camera) = game(0.5, (1280, 720), false);
        let picture = app.world().resource::<Offscreen>().image.clone();
        app.update();
        let scene = scene(&app).expect("the scene image");
        assert_eq!((scene.size, scene.scale), (UVec2::new(640, 360), 0.5));
        let RenderTarget::Image(target) = drawn_into(&app, camera) else { panic!("aimed at the scene image") };
        assert_eq!((target.handle.clone(), target.scale_factor), (scene.image.clone(), 0.5), "640 pixels over 0.5 are the picture's 1280 logical ones");
        let image = app.world().resource::<Assets<Image>>().get(&scene.image).unwrap().clone();
        assert_eq!(image.size(), UVec2::new(640, 360));
        assert!(image.data.is_none(), "nothing writes it from the CPU");
        assert!(matches!(image.sampler, ImageSampler::Descriptor(_)), "its own sampler: linear");
        assert_eq!(app.world().get::<Redirected>(camera).map(|r| matches!(&r.0, RenderTarget::Image(t) if t.handle == picture)), Some(true));

        // One camera shows it, above the 3D one and at the picture; one node under every screen.
        let shown = app.world_mut().query_filtered::<(&Camera, &RenderTarget), With<SceneCamera>>().single(app.world()).map(|(c, t)| (c.order, t.clone())).unwrap();
        assert_eq!(shown.0, 4);
        let layers = app.world_mut().query_filtered::<&RenderLayers, With<SceneCamera>>().single(app.world()).unwrap().clone();
        assert_eq!(layers, RenderLayers::none(), "it sees no 3D mesh, so the mesh counters count the 3D camera's alone");
        assert!(matches!(shown.1, RenderTarget::Image(t) if t.handle == picture), "the picture, which is the offscreen image here");
        let (node, ui_image, z, display) = app
            .world_mut()
            .query_filtered::<(&Node, &ImageNode, &GlobalZIndex, &UiTargetCamera), With<SceneBackdrop>>()
            .single(app.world())
            .map(|(n, i, z, t)| (n.clone(), i.clone(), *z, t.0))
            .unwrap();
        assert_eq!((node.width, node.height, node.position_type), (Val::Percent(100.0), Val::Percent(100.0), PositionType::Absolute));
        assert_eq!((ui_image.image, ui_image.image_mode), (scene.image.clone(), NodeImageMode::Stretch));
        assert_eq!(z, GlobalZIndex(i32::MIN), "under every screen");
        assert!(app.world().get::<SceneCamera>(display).is_some(), "the node is drawn by the display camera");

        // A steady scale changes nothing: no new entity, the camera's target is left alone.
        app.world_mut().clear_trackers();
        app.update();
        assert_eq!(count::<With<SceneCamera>>(&mut app), 1);
        assert!(!app.world().entity(camera).get_ref::<RenderTarget>().unwrap().is_changed());
    }

    #[test]
    fn a_window_that_resizes_or_changes_its_scale_factor_resizes_the_scene_and_keeps_the_logical_size() {
        let (mut app, camera) = game(0.5, (1280, 720), true);
        app.update();
        let image = scene(&app).unwrap().image;
        let target = |app: &App| match drawn_into(app, camera) {
            RenderTarget::Image(t) => (t.handle == image, t.scale_factor),
            other => panic!("{other:?}"),
        };
        assert_eq!(target(&app), (true, 0.5));
        let window = app.world_mut().query_filtered::<Entity, With<PrimaryWindow>>().single(app.world()).unwrap();
        let size = |app: &App| app.world().resource::<Assets<Image>>().get(&image).unwrap().size();

        app.world_mut().get_mut::<Window>(window).unwrap().resolution.set(801.0, 600.0);
        app.update();
        assert_eq!(size(&app), UVec2::new(401, 300), "half of the window's pixels, rounded");
        assert_eq!(scene(&app).unwrap().size, UVec2::new(401, 300));

        // The system's own scale (a high-DPI display): twice the pixels for the same logical size.
        {
            let mut window = app.world_mut().get_mut::<Window>(window).unwrap();
            window.resolution.set_scale_factor_override(Some(2.0));
            window.resolution.set(801.0, 600.0);
        }
        app.update();
        assert_eq!(size(&app), UVec2::new(801, 600), "half of the window's 1602x1200 pixels");
        let (_, factor) = target(&app);
        assert_eq!(factor, 1.0, "the system's 2 times the scene's 0.5: 801 pixels are 801 logical ones");

        // The scale changes while the game plays: the same image, resized.
        set_scale(&mut app, 2.0);
        app.update();
        assert_eq!(size(&app), UVec2::new(3204, 2400), "twice the window's 1602x1200");
        assert_eq!(scene(&app).unwrap().image, image);
    }

    #[test]
    fn back_at_scale_one_the_cameras_are_aimed_where_they_were_and_the_display_goes() {
        for window in [false, true] {
            let (mut app, camera) = game(0.75, (800, 600), window);
            let before = drawn_into(&app, camera);
            app.update();
            assert!(matches!(drawn_into(&app, camera), RenderTarget::Image(_)) && scene(&app).is_some());
            set_scale(&mut app, 1.0);
            app.update();
            assert_eq!(format!("{:?}", drawn_into(&app, camera)), format!("{before:?}"), "window {window}");
            assert!(scene(&app).is_none() && app.world().get::<Redirected>(camera).is_none());
            app.update();
            assert_eq!(count::<With<SceneCamera>>(&mut app) + count::<With<SceneBackdrop>>(&mut app), 0, "despawned: an inactive camera would still take the UI");
            // And again.
            set_scale(&mut app, 0.5);
            app.update();
            assert!(scene(&app).is_some() && matches!(drawn_into(&app, camera), RenderTarget::Image(_)));
        }
    }

    #[test]
    fn a_camera_the_game_aimed_elsewhere_stays_where_the_game_put_it() {
        let (mut app, camera) = game(0.5, (800, 600), false);
        let other = app.world_mut().resource_mut::<Assets<Image>>().add(Image::default());
        let own = app.world_mut().spawn((Camera3d::default(), RenderTarget::Image(other.clone().into()))).id();
        app.update();
        assert!(matches!(drawn_into(&app, camera), RenderTarget::Image(t) if t.handle == scene(&app).unwrap().image));
        assert!(matches!(drawn_into(&app, own), RenderTarget::Image(t) if t.handle == other), "a minimap's camera has its own image");
        // The game takes the redirected camera for its own: the engine lets go of it.
        *app.world_mut().get_mut::<RenderTarget>(camera).unwrap() = RenderTarget::Image(other.clone().into());
        app.update();
        assert!(app.world().get::<Redirected>(camera).is_none());
        assert!(matches!(drawn_into(&app, camera), RenderTarget::Image(t) if t.handle == other));
        set_scale(&mut app, 1.0);
        app.update();
        assert!(matches!(drawn_into(&app, camera), RenderTarget::Image(t) if t.handle == other), "not aimed back at the picture");
    }

    #[test]
    fn without_a_3d_camera_to_show_nothing_is_drawn_or_added() {
        let (mut app, camera) = game(0.5, (800, 600), false);
        app.world_mut().despawn(camera);
        app.update();
        assert!(scene(&app).is_none());
        assert_eq!(count::<With<SceneCamera>>(&mut app), 0);
    }
}
