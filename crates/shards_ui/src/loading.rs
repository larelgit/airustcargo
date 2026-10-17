//! The loading picture (feature `render`, with a window or offscreen): the game's
//! [`EngineConfig::loading_image`] full screen, over every screen, on every frame drawn while the
//! engine is `Loading` or `RegionLoading`.
//!
//! Content is not loaded yet when the picture is first needed, so the game names it in its config,
//! not in content (`--check-content` checks that the file is there). The picture covers the screen:
//! as large as it must be to fill it both ways, centred, cut at the sides or at the top and bottom
//! when the window is not of its proportions; black around it until its file is read.
//!
//! What is seen: the first load runs over many frames, and the picture is up from the frame its
//! file is read to the last frame of `Loading`. A region change (`regen`, `region_load`) goes
//! through `RegionLoading` inside the one call that makes it whenever nothing else must load, so no
//! frame is drawn in between: nothing is seen, and the next frame shows the new region. Only a change
//! that waits across frames for something to load (the loading gate holds `RegionLoading`) shows the
//! picture while it waits.

use bevy::prelude::*;
use bevy::ui::ComputedNode;
use shards_core::{EngineConfig, EngineState};

/// The picture's root: a black full-screen node over every screen.
#[derive(Component)]
pub struct LoadingScreen;

/// The picture inside it, sized to cover the screen.
#[derive(Component)]
struct LoadingPicture;

/// Above every screen (menus draw at 10 and up, modals at 100 and up).
const OVER_SCREENS: i32 = 10_000;

/// The picture, read once and kept: a region change that waits shows it again at once.
#[derive(Resource)]
struct LoadingImage(Handle<Image>);

pub(crate) fn plugin(app: &mut App) {
    app.add_systems(PostUpdate, show_loading.before(bevy::ui::UiSystems::Prepare).run_if(ai_core::display::drawn));
}

/// Whether the loading picture is up now: the engine loads and the game names one.
fn wanted(world: &World) -> Option<String> {
    let loading = matches!(world.get_resource::<State<EngineState>>().map(|s| *s.get()), Some(EngineState::Loading | EngineState::RegionLoading));
    world.get_resource::<EngineConfig>().and_then(|c| c.loading_image.clone()).filter(|_| loading)
}

/// `PostUpdate`, before Bevy's layout: the picture up while loading, gone after.
fn show_loading(world: &mut World) {
    let mut roots = world.query_filtered::<Entity, With<LoadingScreen>>();
    let shown: Vec<Entity> = roots.iter(world).collect();
    let Some(path) = wanted(world) else {
        for e in shown {
            world.entity_mut(e).despawn();
        }
        return;
    };
    if shown.is_empty() {
        let image = match world.get_resource::<LoadingImage>() {
            Some(image) => image.0.clone(),
            None => {
                let image: Handle<Image> = world.resource::<AssetServer>().load(path);
                world.insert_resource(LoadingImage(image.clone()));
                image
            }
        };
        let screen = Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), overflow: Overflow::clip(), ..default() };
        let root = world.spawn((Name::new("loading picture"), LoadingScreen, screen, BackgroundColor(Color::BLACK), GlobalZIndex(OVER_SCREENS))).id();
        world.spawn((LoadingPicture, Node { position_type: PositionType::Absolute, ..default() }, ImageNode::new(image), ChildOf(root)));
    }
    cover(world);
}

/// Sizes the picture to cover the screen, centred: from the screen's size of the last layout and
/// the picture's own, once its file is read.
fn cover(world: &mut World) {
    let Some(image) = world.get_resource::<LoadingImage>().map(|i| i.0.clone()) else { return };
    let Some(picture) = world.resource::<Assets<Image>>().get(&image).map(Image::size_f32) else { return };
    let mut roots = world.query_filtered::<&ComputedNode, With<LoadingScreen>>();
    let Some(screen) = roots.iter(world).next().map(|n| n.size() * n.inverse_scale_factor()) else { return };
    if picture.min_element() <= 0.0 || screen.min_element() <= 0.0 {
        return;
    }
    let size = picture * (screen.x / picture.x).max(screen.y / picture.y);
    let at = (screen - size) / 2.0;
    let mut pictures = world.query_filtered::<&mut Node, With<LoadingPicture>>();
    for mut node in pictures.iter_mut(world) {
        let (width, height, left, top) = (Val::Px(size.x), Val::Px(size.y), Val::Px(at.x), Val::Px(at.y));
        if (node.width, node.height, node.left, node.top) != (width, height, left, top) {
            node.width = width;
            node.height = height;
            node.left = left;
            node.top = top;
        }
    }
}
