//! The main menu (`main_menu: true`), pictures, and the loading picture: what opens after loading
//! in a run that shows the menu, how a press starts the game, and what a window draws while the
//! engine loads. Their own screens: `tests/ui/assets_main_menu`.

use std::time::Duration;

use accesskit::Role;
use serde_json::json;
use shards::ai_core::session::SessionChange;
use shards::ai_core::{configure_session, Scope};
use shards::bevy::a11y::AccessibilityNode;
use shards::bevy::window::Window;
use shards::content::{check_dir, ContentKinds};
use shards::prelude::*;
use shards::testing;
use shards::ui::{LoadingScreen, UiWidget};

use crate::common::*;

/// The main menu's screens and its picture.
const MENU_ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/ui/assets_main_menu");

/// The game with `config` changed by `configure`, its screens those of [`MENU_ASSETS`], loaded; a
/// hero spawns when the game starts.
fn menu_game(configure: impl FnOnce(&mut EngineConfig)) -> App {
    let mut app = testing::test_app(
        |config| {
            config.asset_dir = MENU_ASSETS.into();
            configure(config);
        },
        |app| {
            app.add_systems(OnEnter(EngineState::InGame), spawn_hero);
        },
    );
    configure_session(app.world_mut(), PLAYER, &SessionChange { scope: Some(Scope::Player), ..default() }).unwrap();
    app
}

/// A run that shows the main menu (`--menu`): after loading it is in the menu, no hero yet.
fn at_the_menu() -> App {
    menu_game(|config| {
        config.after_loading = EngineState::MainMenu;
        config.menu = MenuLaunch::Show;
    })
}

fn spawn_hero(mut commands: Commands) {
    commands.spawn((Name::new("Hero"), AiKind::new("Hero"), AiPlayer, Health::new(100.0), Stamina::default(), Transform::default()));
}

fn state(app: &App) -> EngineState {
    *app.world().resource::<State<EngineState>>().get()
}

fn heroes(app: &mut App) -> usize {
    app.world_mut().query_filtered::<Entity, With<AiPlayer>>().iter(app.world()).count()
}

fn open_screens(app: &App) -> Vec<String> {
    app.world().resource::<UiStack>().screens().map(|(name, _)| name.to_owned()).collect()
}

#[test]
fn the_main_menu_opens_after_loading_and_a_press_starts_the_game() {
    let mut app = at_the_menu();
    assert_eq!(state(&app), EngineState::MainMenu);
    assert_eq!(open_screens(&app), ["title"], "the HUD waits for the game");
    assert_eq!(heroes(&mut app), 0);
    let tree = read(&mut app);
    assert_eq!(line(&tree, "title/art"), "title/art  image \"A shard of glass\" ui/picture.png 200 wide");
    assert_eq!(line(&tree, "title/play"), "title/play  button \"Play\" → tool new_game ◀ focus");
    assert!(observe(&mut app).contains("UI  title"), "an agent sees the menu in observe");

    // The player presses Play, as a click does: the game starts inside the call.
    let pressed = ok(&mut app, "ui_click", json!({"id": "title/play"}));
    assert_eq!(state(&app), EngineState::InGame, "{pressed}");
    assert_eq!(heroes(&mut app), 1, "the world spawned as without a menu");
    assert_eq!(open_screens(&app), ["hud"], "the menu closed, the HUD opened");
    assert_eq!(journal(&app, "ui: title/play pressed").len(), 1);
}

#[test]
fn starting_the_game_closes_what_was_opened_over_the_menu() {
    let mut app = at_the_menu();
    ok(&mut app, "ui_click", json!({"id": "title/options"}));
    assert_eq!(open_screens(&app), ["title", "options"]);
    // `back` goes back to the menu, which it does not close.
    ok(&mut app, "ui_navigate", json!({"dir": "back"}));
    assert_eq!(open_screens(&app), ["title"]);
    let err = refused(&mut app, "ui_navigate", json!({"dir": "back"}));
    assert!(err.contains("title cannot be closed with back"), "{err}");

    ok(&mut app, "ui_click", json!({"id": "title/options"}));
    ok(&mut app, "new_game", json!({}));
    assert_eq!(state(&app), EngineState::InGame);
    assert_eq!(open_screens(&app), ["hud"]);
}

#[test]
fn a_run_without_the_menu_never_opens_it() {
    // A game with a main menu, headless and not asked for it: straight in.
    let mut app = menu_game(|config| config.after_loading = EngineState::MainMenu);
    assert_eq!(state(&app), EngineState::InGame);
    assert_eq!(open_screens(&app), ["hud"]);
    assert_eq!(heroes(&mut app), 1);
    let missing = refused(&mut app, "ui_click", json!({"id": "title/play"}));
    assert!(missing.contains("screen `title` is not open"), "{missing}");
    let again = refused(&mut app, "new_game", json!({}));
    assert!(again.contains("on already"), "{again}");
}

#[test]
fn with_a_window_a_picture_is_an_image_named_for_screen_readers() {
    let mut app = at_the_menu();
    app.world_mut().spawn(Window::default());
    app.update();
    app.update();
    let mut q = app.world_mut().query::<(Entity, &UiWidget)>();
    let art = q.iter(app.world()).find(|(_, w)| w.path == "title/art").map(|(e, _)| e).expect("title/art");
    let node = app.world().get::<Node>(art).expect("drawn");
    assert_eq!((node.width, node.height), (Val::Px(200.0), Val::Auto), "the height keeps the picture's proportions");
    let a11y = &app.world().get::<AccessibilityNode>(art).expect("named for screen readers").0;
    assert_eq!((a11y.role(), a11y.label()), (Role::Image, Some("A shard of glass")));
    let children: Vec<Entity> = app.world().get::<Children>(art).map(|c| c.iter().collect()).unwrap_or_default();
    let picture = children.into_iter().find(|c| app.world().get::<ImageNode>(*c).is_some()).expect("the picture is a part");
    let image = app.world().get::<ImageNode>(picture).unwrap();
    assert_eq!(image.image.path().map(ToString::to_string).as_deref(), Some("ui/picture.png"));
    assert!(app.world().get::<AccessibilityNode>(picture).unwrap().0.is_hidden(), "screen readers hear the widget once");
}

fn loading_screens(app: &mut App) -> usize {
    app.world_mut().query_filtered::<Entity, With<LoadingScreen>>().iter(app.world()).count()
}

/// The engine with `config` (the menu's assets), built but not loaded yet.
fn unloaded(config: EngineConfig) -> App {
    let mut app = App::new();
    app.add_plugins(EnginePlugins::new(EngineConfig { asset_dir: MENU_ASSETS.into(), ..config }));
    app.finish();
    app.cleanup();
    app
}

#[test]
fn with_a_window_the_loading_picture_covers_loading_and_a_region_that_loads_across_frames() {
    let mut app = unloaded(EngineConfig { loading_image: Some("ui/picture.png".into()), ..testing::config() });
    app.world_mut().spawn(Window::default());
    app.update();
    assert_eq!(state(&app), EngineState::Loading);
    assert_eq!(loading_screens(&mut app), 1, "up from the first frame of loading");
    testing::wait_until_loaded(&mut app, Duration::from_secs(30)).unwrap();
    app.update();
    assert_eq!(loading_screens(&mut app), 0, "gone in play");

    // A region that loads across frames (the gate holds `RegionLoading`) shows it again.
    app.world_mut().resource_mut::<NextState<EngineState>>().set(EngineState::RegionLoading);
    app.update();
    assert_eq!(state(&app), EngineState::RegionLoading);
    assert_eq!(loading_screens(&mut app), 1);
    testing::wait_until_loaded(&mut app, Duration::from_secs(30)).unwrap();
    app.update();
    assert_eq!(state(&app), EngineState::InGame);
    assert_eq!(loading_screens(&mut app), 0);
}

#[test]
fn without_a_window_or_a_picture_nothing_is_drawn_while_loading() {
    let mut app = unloaded(EngineConfig { loading_image: Some("ui/picture.png".into()), ..testing::config() });
    app.update();
    assert_eq!(state(&app), EngineState::Loading);
    assert_eq!(loading_screens(&mut app), 0, "headless: nothing is drawn");

    let mut app = unloaded(testing::config());
    app.world_mut().spawn(Window::default());
    app.update();
    assert_eq!(loading_screens(&mut app), 0, "a game that names no picture shows none");
}

#[test]
fn the_content_check_names_a_missing_picture_and_a_hud_for_a_main_menu() {
    let dir = std::env::temp_dir().join(format!("shards_ui_main_menu_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("content")).unwrap();
    std::fs::create_dir_all(dir.join("ui")).unwrap();
    std::fs::copy(format!("{MENU_ASSETS}/ui/picture.png"), dir.join("ui/picture.png")).unwrap();
    std::fs::write(
        dir.join("content/menu.screen.ron"),
        r#"{
            "fine": (main_menu: true, widgets: [Image(id: "art", path: "ui/picture.png", height: 120.0)]),
            "lost": (widgets: [Image(id: "art", path: "ui/missing.png")]),
            "squashed": (widgets: [Image(id: "art", path: "ui/picture.png", width: 0.0, height: -3.0)]),
            "hud_menu": (layer: Hud, main_menu: true, widgets: [Text(text: "hi")]),
        }"#,
    )
    .unwrap();
    let app = game();
    let kinds = app.world().resource::<ContentKinds>().clone();
    let errors: Vec<String> = check_dir(&dir, &kinds).unwrap_err().iter().map(ToString::to_string).collect();
    let _ = std::fs::remove_dir_all(&dir);
    let has = |needle: &str| assert!(errors.iter().any(|e| e.contains(needle)), "no `{needle}` in:\n{}", errors.join("\n"));
    has("screen `lost`: image `art`: no file `ui/missing.png` under the asset folder");
    has("screen `squashed`: image `art`: `width` is a size above 0, not 0");
    has("screen `squashed`: image `art`: `height` is a size above 0, not -3");
    has("screen `hud_menu`: a HUD takes no input, so it cannot be the main menu");
    assert_eq!(errors.len(), 4, "{}", errors.join("\n"));
}
