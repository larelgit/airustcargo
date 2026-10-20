//! Compass categories (U3a): a landmark's `LandmarkCategory`, the nearest marks of each category,
//! the source `you.nearest:<category>`, the theme's `symbols` drawn on the compass, the checks.

use std::path::Path;

use shards::ai_core::session::SessionChange;
use shards::ai_core::{configure_session, LandmarkCategory, Scope};
use shards::bevy::a11y::AccessibilityNode;
use shards::bevy::window::Window;
use shards::prelude::*;
use shards::testing;
use shards::ui::{read_source, UiValue, UiWidget, DEFAULT_FONT};

use crate::common::*;

/// Around the hero (at the origin, looking north): a pillar 3 m south with no category, five
/// shrines north at 5 to 25 m, a camp 8 m east.
fn spawn_landmarks(mut commands: Commands) {
    commands.spawn((Name::new("Pillar"), AiLandmark, Transform::from_xyz(0.0, 0.0, 3.0)));
    for n in 1..=5 {
        let at = Transform::from_xyz(0.0, 0.0, -5.0 * n as f32);
        commands.spawn((Name::new(format!("Shrine {n}")), LandmarkCategory("shrine".into()), at));
    }
    commands.spawn((Name::new("Camp East"), LandmarkCategory("camp".into()), Transform::from_xyz(8.0, 0.0, 0.0)));
}

fn with_landmarks(app: &mut App) {
    app.add_systems(OnEnter(EngineState::InGame), spawn_landmarks);
}

fn marks(app: &App, bind: &str) -> Vec<(String, Option<String>, f32)> {
    match read_source(app.world(), bind) {
        Ok(Some(UiValue::Heading { marks, .. })) => marks.into_iter().map(|m| (m.label, m.category, m.distance)).collect(),
        other => panic!("`{bind}` gives {other:?}"),
    }
}

#[test]
fn a_compass_keeps_the_nearest_landmarks_of_each_category_and_says_their_category() {
    let mut app = game_with(with_landmarks);
    assert_eq!(
        line(&read(&mut app), "hud/compass"),
        "hud/compass  compass 0° N · Pillar S 3m · Shrine 1 (shrine) N 5m · Camp East (camp) E 8m · Shrine 2 (shrine) N 10m \
         · Shrine 3 (shrine) N 15m · Shrine 4 (shrine) N 20m ← you.heading",
        "four of each category, nearest first: the fifth shrine is left out, not the camp"
    );
    let one = marks(&app, "you.heading:1");
    let names: Vec<&str> = one.iter().map(|(label, ..)| label.as_str()).collect();
    assert_eq!(names, ["Pillar", "Shrine 1", "Camp East"], "one of each");
    assert_eq!(marks(&app, "you.heading:x").len(), 6, "a bad number keeps the default four");

    // How far the nearest of a category is: what a game pulses a sound or a needle by.
    let near = |app: &App, bind: &str| read_source(app.world(), bind).unwrap();
    assert_eq!(near(&app, "you.nearest:shrine"), Some(UiValue::Number(5.0)));
    assert_eq!(near(&app, "you.nearest:camp"), Some(UiValue::Number(8.0)));
    assert_eq!(near(&app, "you.nearest:tower"), Some(UiValue::None), "no tower: no value");
    let (camp, distance) = shards::nearest_landmark(app.world(), "camp").unwrap();
    assert_eq!((app.world().get::<Name>(camp).unwrap().as_str(), distance), ("Camp East", 8.0));
    assert_eq!(shards::nearest_landmark(app.world(), " camp "), Some((camp, 8.0)), "matched trimmed, as the source matches it");

    // The category is the landmark's identity, as `AiLandmark` is: not in the replay check.
    let before = shards::ai_core::replay::state_hash(app.world());
    let mut categories = app.world_mut().query::<&mut LandmarkCategory>();
    for mut c in categories.iter_mut(app.world_mut()) {
        c.0 = "renamed".into();
    }
    assert_eq!(shards::ai_core::replay::state_hash(app.world()), before);
}

/// The test assets with a theme whose `symbols` give shrines `⛩` and camps a rune in the font
/// `symbols` (the engine's font under another name: what is checked is which handle a run has).
fn windowed() -> App {
    let dir = std::env::temp_dir().join(format!("shards-ui-compass-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(Path::new(ASSETS), &dir);
    std::fs::create_dir_all(dir.join("fonts")).unwrap();
    std::fs::write(dir.join("fonts/symbols.ttf"), DEFAULT_FONT).unwrap();
    let theme = r#"{ "default": (fonts: {"symbols": "fonts/symbols.ttf"}, symbols: {"shrine": "⛩", "camp": "[font=symbols]ᚲ[/font]"}) }"#;
    std::fs::write(dir.join("content/ui/look.theme.ron"), theme).unwrap();
    let mut app = testing::test_app(
        |config| config.asset_dir = dir.to_string_lossy().into_owned(),
        |app| {
            app.add_systems(OnEnter(EngineState::InGame), spawn_hero);
            with_landmarks(app);
        },
    );
    configure_session(app.world_mut(), PLAYER, &SessionChange { scope: Some(Scope::Player), ..default() }).unwrap();
    app.world_mut().spawn(Window::default());
    app.update();
    app
}

#[test]
fn the_theme_draws_each_category_as_its_symbol_and_screen_readers_hear_its_name() {
    let mut app = windowed();
    let e = {
        let mut q = app.world_mut().query::<(Entity, &UiWidget)>();
        q.iter(app.world()).find(|(_, w)| w.path == "hud/compass").map(|(e, _)| e).unwrap()
    };
    let symbols: Handle<Font> = app.world().resource::<AssetServer>().load("fonts/symbols.ttf");
    let children: Vec<Entity> = app.world().get::<Children>(e).map(|c| c.iter().collect()).unwrap_or_default();
    let runs: Vec<(String, bool)> = children
        .into_iter()
        .filter_map(|c| {
            let span = app.world().get::<TextSpan>(c)?;
            Some((span.0.clone(), app.world().get::<TextFont>(c).unwrap().font == symbols.clone().into()))
        })
        .collect();
    let drawn: String = runs.iter().map(|(t, _)| t.as_str()).collect();
    assert!(drawn.contains("|  ⛩ Shrine 1 N 5m  |  ᚲ Camp East E 8m  |  ⛩ Shrine 2"), "{drawn}");
    assert!(drawn.contains("|  Pillar S 3m"), "no category, no symbol: {drawn}");
    let in_symbols: Vec<&str> = runs.iter().filter(|(_, s)| *s).map(|(t, _)| t.as_str()).collect();
    assert_eq!(in_symbols, ["ᚲ"], "the camp's rune is drawn in its font");
    let value = app.world().get::<AccessibilityNode>(e).unwrap().0.value().unwrap_or_default().to_owned();
    assert!(value.contains("Shrine 1 (shrine) N 5m") && value.contains("Camp East (camp) E 8m"), "{value}");
}

#[test]
fn the_content_check_names_bad_categories_and_symbols() {
    use shards::content::db::Source;
    use shards::content::{ContentDb, ContentKinds};
    use shards::ui::ThemeDef;
    use shards::world::PoiDef;
    let mut kinds = ContentKinds::default();
    kinds.register::<ThemeDef>();
    kinds.register::<PoiDef>();
    let theme = r#"{ "default": (symbols: {"shrine": "⛩", "big camp": "C", "ruin": " ", "rune": "[font=runes]ᚱ[/font]", "camp": "[font=runes]C"}) }"#;
    let pois = r#"{ "shrine": (category: "shrine"), "camp": (category: "camp site") }"#;
    let sources = [
        Source { path: "content/t.theme.ron".into(), text: theme.into() },
        Source { path: "content/p.poi.ron".into(), text: pois.into() },
    ];
    let db = ContentDb::build(&kinds, &sources);
    let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
    let has = |needle: &str| assert!(errors.iter().any(|e| e.contains(needle)), "no `{needle}` in {errors:#?}");
    has("symbols: `big camp` is not a category name: letters, digits, `_` and `-`");
    has("symbols.ruin: empty; leave the category out to draw none");
    has("symbols.rune: `[font=runes]` names no font of the theme (`fonts`)");
    has("category `camp site`: letters, digits, `_` and `-` (the UI theme's `symbols` name it)");
    has("symbols.camp: `[font=runes]` pairs with no tag and would be drawn as text; close each `[font=name]` with `[/font]`");
    assert_eq!(errors.len(), 5, "{errors:#?}");
}
