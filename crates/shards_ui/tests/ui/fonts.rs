//! Font spans (U1): a theme's named fonts, `[font=name]…[/font]` in a `Text` or `Hint` value drawn
//! as `TextSpan` runs in them, the text without markup for agents and screen readers, the checks.

use std::path::{Path, PathBuf};

use shards::ai_core::session::SessionChange;
use shards::ai_core::{configure_session, Scope};
use shards::bevy::a11y::AccessibilityNode;
use shards::bevy::window::Window;
use shards::prelude::*;
use shards::testing;
use shards::ui::{UiAppExt, UiValue, UiWidget, DEFAULT_FONT};

use crate::common::*;

/// The line a bound text shows: changed by a test to see the runs drawn again in place.
#[derive(Resource)]
struct Lore(String);

/// The test screens, a theme with the font `symbols` (the engine's font written under another
/// name: what is checked is which handle a run has), and a HUD screen with a marked text.
fn assets() -> PathBuf {
    let dir = std::env::temp_dir().join(format!("shards-ui-fonts-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(Path::new(ASSETS), &dir);
    std::fs::create_dir_all(dir.join("fonts")).unwrap();
    std::fs::write(dir.join("fonts/symbols.ttf"), DEFAULT_FONT).unwrap();
    std::fs::write(dir.join("content/ui/look.theme.ron"), r#"{ "default": (fonts: {"symbols": "fonts/symbols.ttf"}) }"#).unwrap();
    let lore = r#"{
        "lore": (layer: Hud, open_at_start: true, widgets: [
            Text(id: "line", text: "the door said [font=symbols]ᚱᚢᚾ[/font]."),
            Hint(id: "bound", bind: "lore.line", text: ""),
        ]),
    }"#;
    std::fs::write(dir.join("content/ui/lore.screen.ron"), lore).unwrap();
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The engine on [`assets`], with a window entity (drawing, without a GPU).
fn windowed() -> App {
    let dir = assets();
    let mut app = testing::test_app(
        |config| config.asset_dir = dir.to_string_lossy().into_owned(),
        |app| {
            app.insert_resource(Lore("plain".into()));
            app.add_ui_source("lore.line", |world, _| world.get_resource::<Lore>().map(|l| UiValue::Text(l.0.clone())));
        },
    );
    configure_session(app.world_mut(), PLAYER, &SessionChange { scope: Some(Scope::Player), ..default() }).unwrap();
    app.world_mut().spawn(Window::default());
    app.update();
    app
}

fn widget(app: &mut App, path: &str) -> Entity {
    let mut q = app.world_mut().query::<(Entity, &UiWidget)>();
    q.iter(app.world()).find(|(_, w)| w.path == path).map(|(e, _)| e).unwrap_or_else(|| panic!("no widget {path}"))
}

/// The widget's own text, and its runs: each one's text and whether it draws in the theme's font
/// (`None`) or in `symbols` (`Some(true)`), or another (`Some(false)`).
fn runs(app: &mut App, path: &str) -> (String, Vec<(String, Option<bool>)>) {
    let e = widget(app, path);
    let symbols: Handle<Font> = app.world().resource::<AssetServer>().load("fonts/symbols.ttf");
    let base = app.world().get::<TextFont>(e).unwrap().font.clone();
    let own = app.world().get::<Text>(e).unwrap().0.clone();
    let children: Vec<Entity> = app.world().get::<Children>(e).map(|c| c.iter().collect()).unwrap_or_default();
    let runs = children
        .into_iter()
        .filter_map(|c| {
            let span = app.world().get::<TextSpan>(c)?;
            let font = &app.world().get::<TextFont>(c).unwrap().font;
            let which = if *font == base { None } else { Some(*font == symbols.clone().into()) };
            Some((span.0.clone(), which))
        })
        .collect();
    (own, runs)
}

#[test]
fn a_marked_text_draws_its_runs_in_the_themes_named_font() {
    let mut app = windowed();
    let (own, line) = runs(&mut app, "lore/line");
    assert_eq!(own, "", "the runs carry the text");
    assert_eq!(line, [("the door said ".into(), None), ("ᚱᚢᚾ".into(), Some(true)), (".".into(), None)]);
    // Screen readers and agents read the text without the markup.
    let e = widget(&mut app, "lore/line");
    assert_eq!(app.world().get::<AccessibilityNode>(e).unwrap().0.label(), Some("the door said ᚱᚢᚾ."));
    assert!(read(&mut app).contains("lore/line  text \"the door said ᚱᚢᚾ.\""), "{}", read(&mut app));

    // A bound value: plain, then marked, then plain again, drawn in place each time.
    assert_eq!(runs(&mut app, "lore/bound"), ("plain".into(), vec![]));
    app.world_mut().resource_mut::<Lore>().0 = "[font=symbols]ᚦ[/font] and [font=nope]x[/font]".into();
    app.update();
    let (own, bound) = runs(&mut app, "lore/bound");
    assert_eq!(own, "");
    assert_eq!(bound, [("ᚦ".into(), Some(true)), (" and ".into(), None), ("x".into(), None)], "a font the theme lacks draws in its own");
    assert_eq!(journal(&app, "`[font=nope]` names no font of the theme").len(), 1);
    app.update();
    assert_eq!(journal(&app, "`[font=nope]` names no font of the theme").len(), 1, "once");
    app.world_mut().resource_mut::<Lore>().0 = "plain [again".into();
    app.update();
    assert_eq!(runs(&mut app, "lore/bound"), ("plain [again".into(), vec![]), "the runs go");
}

#[test]
fn the_content_check_names_a_span_font_the_theme_lacks() {
    use shards::content::db::Source;
    use shards::content::{ContentDb, ContentKinds};
    use shards::ui::{ScreenDef, ThemeDef};
    let mut kinds = ContentKinds::default();
    kinds.register::<ThemeDef>();
    kinds.register::<ScreenDef>();
    let theme = r#"{ "default": (fonts: {"symbols": "fonts/symbols.ttf", "bad name": "fonts/x.ttf"}) }"#;
    let screen = r#"{ "s": (widgets: [
        Text(id: "a", text: "[font=runes]x[/font] [font=symbols]y[/font]"),
        Hint(id: "b", text: "[font=runes]never closed"),
        Button(id: "c", label: "[font=runes]a label is drawn as written[/font]", action: "close"),
    ]) }"#;
    let sources = [
        Source { path: "content/t.theme.ron".into(), text: theme.into() },
        Source { path: "content/s.screen.ron".into(), text: screen.into() },
    ];
    let db = ContentDb::build(&kinds, &sources);
    let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
    let has = |needle: &str| assert!(errors.iter().any(|e| e.contains(needle)), "no `{needle}` in {errors:#?}");
    has("widget `a` text: `[font=runes]` names no font of the theme; fonts: bad name, symbols");
    has("fonts: `bad name` is not a font name");
    assert_eq!(errors.len(), 2, "an unclosed tag and a label are text: {errors:#?}");
}
