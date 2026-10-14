//! Graphics settings applied to the primary window.
//!
//! Only when a window exists: a headless game has none and keeps the values. The window is
//! touched when the settings change (and once at the start), never every frame, so a player
//! who drags the window to another size is not fought.

use bevy::prelude::*;
use bevy::window::{MonitorSelection, PresentMode, PrimaryWindow, Window, WindowMode};

use crate::GraphicsSettings;

/// `Update`: fullscreen, vsync, resolution and scale onto the primary window.
pub fn apply_graphics(settings: Res<GraphicsSettings>, mut windows: Query<&mut Window, With<PrimaryWindow>>, mut applied: Local<Option<GraphicsSettings>>) {
    let Ok(mut window) = windows.single_mut() else {
        // No window (yet): try again when one exists.
        *applied = None;
        return;
    };
    if applied.as_ref() == Some(&*settings) {
        return;
    }
    let before = applied.replace(settings.clone());

    let mode = if settings.fullscreen { WindowMode::BorderlessFullscreen(MonitorSelection::Current) } else { WindowMode::Windowed };
    if window.mode != mode {
        window.mode = mode;
    }
    let present = if settings.vsync { PresentMode::AutoVsync } else { PresentMode::AutoNoVsync };
    if window.present_mode != present {
        window.present_mode = present;
    }
    // The scale factor first, so a size set after it is the logical size the player asked for
    // (Bevy sets the physical size to logical x factor). A change of the factor alone keeps the
    // logical size where it was: the window gets more or fewer pixels, not more or less room.
    let scale = settings.scale();
    let wanted = (scale != 1.0).then_some(scale);
    let (logical_w, logical_h) = (window.resolution.width(), window.resolution.height());
    let scale_changed = window.resolution.scale_factor_override() != wanted;
    if scale_changed {
        window.resolution.set_scale_factor_override(wanted);
    }
    // The size is the player's to set once it was chosen: only a changed `resolution` resizes.
    let resolution_changed = before.as_ref().is_none_or(|b| b.resolution != settings.resolution);
    match settings.window_size() {
        Some((w, h)) if resolution_changed && !settings.fullscreen => window.resolution.set(w as f32, h as f32),
        _ if scale_changed && !settings.fullscreen => window.resolution.set(logical_w, logical_h),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app_with(settings: GraphicsSettings) -> App {
        let mut app = App::new();
        app.insert_resource(settings).add_systems(Update, apply_graphics);
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app
    }

    fn window(app: &mut App) -> Window {
        app.world_mut().query_filtered::<&Window, With<PrimaryWindow>>().single(app.world()).unwrap().clone()
    }

    #[test]
    fn the_settings_reach_the_window_when_they_change() {
        let mut app = app_with(GraphicsSettings { resolution: "1600x900".into(), vsync: false, render_scale: 0.75, ..default() });
        app.update();
        let w = window(&mut app);
        assert_eq!((w.resolution.width(), w.resolution.height()), (1600.0, 900.0), "the logical size asked for");
        assert_eq!((w.resolution.physical_width(), w.resolution.physical_height()), (1200, 675), "at the scale factor's pixels");
        assert_eq!(w.present_mode, PresentMode::AutoNoVsync);
        assert_eq!(w.resolution.scale_factor_override(), Some(0.75));
        assert_eq!(w.mode, WindowMode::Windowed);

        app.world_mut().resource_mut::<GraphicsSettings>().fullscreen = true;
        app.update();
        assert!(matches!(window(&mut app).mode, WindowMode::BorderlessFullscreen(_)));
    }

    #[test]
    fn a_window_the_player_resized_is_left_alone() {
        let mut app = app_with(GraphicsSettings { resolution: "1600x900".into(), ..default() });
        app.update();
        let id = app.world_mut().query_filtered::<Entity, With<PrimaryWindow>>().single(app.world()).unwrap();
        app.world_mut().get_mut::<Window>(id).unwrap().resolution.set(800.0, 600.0);
        // Another setting changes: the size stays where the player put it.
        app.world_mut().resource_mut::<GraphicsSettings>().vsync = false;
        app.update();
        assert_eq!(window(&mut app).resolution.width(), 800.0);
    }

    #[test]
    fn without_a_window_nothing_happens() {
        let mut app = App::new();
        app.init_resource::<GraphicsSettings>().add_systems(Update, apply_graphics);
        app.update();
        app.world_mut().resource_mut::<GraphicsSettings>().fullscreen = true;
        app.update();
        // A window that appears later gets the settings.
        app.world_mut().spawn((Window::default(), PrimaryWindow));
        app.update();
        assert!(matches!(window(&mut app).mode, WindowMode::BorderlessFullscreen(_)));
    }
}
