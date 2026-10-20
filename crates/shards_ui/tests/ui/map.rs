//! The map widget (U3b): a game's places on a picture of the world, in words for agents and screen
//! readers, drawn as marks where they are (a rumour on the edge, toward it), the theme's colours,
//! the checks.

use std::path::{Path, PathBuf};

use shards::ai_core::session::SessionChange;
use shards::ai_core::{configure_session, Scope};
use shards::bevy::a11y::AccessibilityNode;
use shards::bevy::window::Window;
use shards::prelude::*;
use shards::testing;
use shards::ui::{MapPoint, MapState, UiAppExt, UiValue, UiWidget, DEFAULT_FONT};

use crate::common::*;

/// The places the test's game knows of.
#[derive(Resource)]
struct Places(Vec<MapPoint>);

fn place(label: &str, x: f32, z: f32, state: MapState, category: Option<&str>) -> MapPoint {
    MapPoint { label: label.into(), at: Vec2::new(x, z), state, category: category.map(str::to_owned) }
}

/// A shrine found north-north-east, ruins heard of to the north-east, a gate done with.
fn places() -> Places {
    Places(vec![
        place("Old Shrine", 12.0, -40.0, MapState::Discovered, Some("shrine")),
        place("Ruins", 60.0, -60.0, MapState::Rumor, None),
        place("Gate", 5.0, 5.0, MapState::Completed, None),
    ])
}

/// The test assets and a HUD with a 200 × 200 map of the 200 m around the origin; `theme`, if
/// given, is the theme `default`.
fn assets(name: &str, theme: Option<&str>) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("shards-ui-map-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(Path::new(ASSETS), &dir);
    std::fs::create_dir_all(dir.join("fonts")).unwrap();
    std::fs::write(dir.join("fonts/symbols.ttf"), DEFAULT_FONT).unwrap();
    let map = r#"{ "atlas": (layer: Hud, open_at_start: true, widgets: [
        Map(id: "map", label: "Valley", bind: "quests.places", area: (min: (-100.0, -100.0), max: (100.0, 100.0)), width: 200.0, height: 200.0),
    ]) }"#;
    std::fs::write(dir.join("content/ui/atlas.screen.ron"), map).unwrap();
    if let Some(theme) = theme {
        std::fs::write(dir.join("content/ui/look.theme.ron"), theme).unwrap();
    }
    dir
}

/// The engine on `dir` with the game's source of places; a window entity when `windowed`.
fn game_on(dir: PathBuf, windowed: bool) -> App {
    let mut app = testing::test_app(
        |config| config.asset_dir = dir.to_string_lossy().into_owned(),
        |app| {
            app.insert_resource(places()).add_ui_source("quests.places", |world, _| {
                let places = world.get_resource::<Places>()?;
                Some(UiValue::Map { points: places.0.clone(), you: None })
            });
            app.add_systems(OnEnter(EngineState::InGame), |mut commands: Commands| {
                commands.spawn((Name::new("Hero"), AiKind::new("Hero"), AiPlayer, Health::new(100.0), Transform::default()));
            });
        },
    );
    configure_session(app.world_mut(), PLAYER, &SessionChange { scope: Some(Scope::Player), ..default() }).unwrap();
    if windowed {
        app.world_mut().spawn(Window::default());
    }
    app.update();
    app
}

#[test]
fn agents_read_the_places_in_words_and_a_rumour_as_a_direction() {
    let mut app = game_on(assets("words", None), false);
    assert_eq!(
        line(&read(&mut app), "atlas/map"),
        "atlas/map  map \"Valley\" you (0, 0) facing N · Old Shrine (shrine) discovered (12, -40) · Ruins rumor NE \
         · Gate completed (5, 5) ← quests.places"
    );
    // The viewer moves and turns: the rumour's direction is from where it stands now.
    let hero = {
        let mut q = app.world_mut().query_filtered::<Entity, With<AiPlayer>>();
        q.single(app.world()).unwrap()
    };
    *app.world_mut().get_mut::<Transform>(hero).unwrap() = Transform::from_xyz(60.0, 0.0, 0.0).looking_to(Vec3::X, Vec3::Y);
    let now = line(&read(&mut app), "atlas/map").to_owned();
    assert!(now.contains("you (60, 0) facing E") && now.contains("Ruins rumor N "), "{now}");
    app.world_mut().resource_mut::<Places>().0.clear();
    assert!(line(&read(&mut app), "atlas/map").contains("facing E · no places"));
}

/// The marks drawn on `map`, in the order drawn: each one's centre (pixels from the map's top
/// left) and its dot's colour, or the text of its symbol.
fn marks(app: &mut App) -> Vec<(Vec2, Result<Srgba, String>)> {
    let e = {
        let mut q = app.world_mut().query::<(Entity, &UiWidget)>();
        q.iter(app.world()).find(|(_, w)| w.path == "atlas/map").map(|(e, _)| e).unwrap()
    };
    let px = |v: Val| match v {
        Val::Px(p) => p,
        other => panic!("{other:?}"),
    };
    let children: Vec<Entity> = app.world().get::<Children>(e).unwrap().iter().collect();
    children
        .into_iter()
        .filter(|c| app.world().get::<ImageNode>(*c).is_none())
        .map(|c| {
            let node = app.world().get::<Node>(c).unwrap();
            let centre = Vec2::new(px(node.left) + px(node.width) * 0.5, px(node.top) + px(node.height) * 0.5);
            let inside = app.world().get::<Children>(c).unwrap()[0];
            let look = match app.world().get::<BackgroundColor>(inside) {
                Some(bg) => Ok(bg.0.to_srgba()),
                None => {
                    let own = app.world().get::<Text>(inside).unwrap().0.clone();
                    let runs: String = app
                        .world()
                        .get::<Children>(inside)
                        .map(|r| r.iter().filter_map(|s| app.world().get::<TextSpan>(s).map(|s| s.0.clone())).collect())
                        .unwrap_or_default();
                    Err(own + &runs)
                }
            };
            (centre, look)
        })
        .collect()
}

#[test]
fn places_are_marks_where_they_are_a_rumour_on_the_edge_and_the_viewer_last() {
    let theme = r#"{ "default": (
        fonts: {"symbols": "fonts/symbols.ttf"},
        symbols: {"shrine": "[font=symbols]ᛋ[/font]"},
        map_colors: {"completed": "#00ff00", "you": "#ff0000"},
    ) }"#;
    let mut app = game_on(assets("marks", Some(theme)), true);
    let drawn = marks(&mut app);
    let near = |a: Vec2, b: Vec2| (a - b).length() < 0.01;
    assert_eq!(drawn.len(), 4, "{drawn:?}");
    assert!(near(drawn[0].0, Vec2::new(112.0, 60.0)), "the shrine, 12 m east and 40 m north of the middle: {drawn:?}");
    assert_eq!(drawn[0].1, Err("ᛋ".into()), "drawn as its category's symbol");
    assert!(near(drawn[1].0, Vec2::new(200.0, 0.0)), "the ruins heard of to the north-east: the corner that way, not where they are");
    assert!(near(drawn[2].0, Vec2::new(105.0, 105.0)));
    assert_eq!(drawn[2].1, Ok(Srgba::rgb_u8(0, 255, 0)), "the theme's colour for a place done with");
    assert!(near(drawn[3].0, Vec2::new(100.0, 100.0)) && drawn[3].1 == Ok(Srgba::rgb_u8(255, 0, 0)), "the viewer, last, in its colour");

    // A place found moves its mark; the viewer walking redraws the marks in place.
    app.world_mut().resource_mut::<Places>().0[1].state = MapState::Discovered;
    app.update();
    assert!(near(marks(&mut app)[1].0, Vec2::new(160.0, 40.0)), "the ruins found: where they are");
    let e = {
        let mut q = app.world_mut().query::<(Entity, &UiWidget)>();
        q.iter(app.world()).find(|(_, w)| w.path == "atlas/map").map(|(e, _)| e).unwrap()
    };
    let value = app.world().get::<AccessibilityNode>(e).unwrap().0.value().unwrap_or_default().to_owned();
    assert!(value.contains("Ruins discovered (60, -60)"), "screen readers hear the places: {value}");
}

#[test]
fn the_content_check_names_a_map_that_cannot_be_drawn() {
    use shards::content::db::Source;
    use shards::content::{ContentDb, ContentKinds};
    use shards::ui::{ScreenDef, ThemeDef};
    let mut kinds = ContentKinds::default();
    kinds.register::<ThemeDef>();
    kinds.register::<ScreenDef>();
    let screen = r#"{ "s": (widgets: [
        Map(id: "flat", bind: "places", area: (min: (0.0, 0.0), max: (10.0, 0.0))),
        Map(id: "tiny", bind: " ", area: (min: (0.0, 0.0), max: (1.0, 1.0)), width: 0.0),
    ]) }"#;
    let theme = r#"{ "default": (map_colors: {"lost": "#ffffff", "rumor": "grey"}) }"#;
    let sources = [
        Source { path: "content/s.screen.ron".into(), text: screen.into() },
        Source { path: "content/t.theme.ron".into(), text: theme.into() },
    ];
    let db = ContentDb::build(&kinds, &sources);
    let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
    let has = |needle: &str| assert!(errors.iter().any(|e| e.contains(needle)), "no `{needle}` in {errors:#?}");
    has("map `flat`: area: `max` is east and south of `min`");
    has("map `tiny`: `width` is a size above 0, not 0");
    has("map `tiny`: `bind` names the game's source of places");
    has("map_colors: `lost` is none of rumor, discovered, completed, you");
    has("map_colors.rumor: ");
    assert_eq!(errors.len(), 5, "{errors:#?}");
}
