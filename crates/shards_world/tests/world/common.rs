//! Test games on the whole engine (`shards::testing`), with the content next to this file:
//! an 8×8 meadow of 16 m tiles, a walled region, an impossible one with a fallback, a crypt.
//! The game spawns its player at the region's spawn when it enters `InGame`, as games do.

#![allow(dead_code)]

use std::path::Path;

use serde_json::Value;
use shards::content::db::{read_dir, Source};
use shards::content::{ContentDb, ContentKinds};
use shards::prelude::*;
use shards::testing;
use shards::world::WorldGen;

pub const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/world/assets");

/// The player marker of the test game.
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct Hero;

/// The test game: a hero at the region's spawn point.
pub fn game(app: &mut App) {
    app.register_type::<Hero>().add_systems(OnEnter(EngineState::InGame), spawn_hero);
}

fn spawn_hero(mut commands: Commands, region: Option<Res<ActiveRegion>>) {
    let at = region.map_or(Vec3::ZERO, |r| r.spawn_point(0));
    commands.spawn((Hero, character("", at), AiPlayer, PlayerId(0), AiKind::new("Player"), Name::new("Player")));
}

pub fn config(seed: u64) -> EngineConfig {
    EngineConfig { seed, asset_dir: ASSETS.into(), ..testing::config() }
}

/// A loaded test game at tick 0, with `start` generated while it loaded.
pub fn app(seed: u64, start: Option<&str>) -> App {
    app_with(seed, start, |_| {})
}

pub fn app_with(seed: u64, start: Option<&str>, setup: impl FnOnce(&mut App)) -> App {
    let app = testing::test_app(
        |c| *c = config(seed),
        |app| {
            if let Some(region) = start {
                app.start_region(region);
            }
            game(app);
            setup(app);
        },
    );
    let errors: Vec<String> = app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "test content has errors: {errors:#?}");
    app
}

/// The same game built but not loaded: for playing a replay on it.
pub fn fresh(seed: u64, start: Option<&str>) -> App {
    let mut app = App::new();
    app.add_plugins(EnginePlugins::new(config(seed)));
    if let Some(region) = start {
        app.start_region(region);
    }
    game(&mut app);
    app.finish();
    app.cleanup();
    app
}

pub fn ok(app: &mut App, tool: &str, args: Value) -> String {
    let out = testing::ok(app, tool, args);
    match out.result {
        Value::String(s) => s,
        other => other.to_string(),
    }
}

pub fn err(app: &mut App, tool: &str, args: Value) -> String {
    let out = testing::call(app, tool, args.clone());
    assert!(!out.ok, "{tool} {args} should fail: {}", out.result);
    out.result.as_str().unwrap_or_default().to_owned()
}

pub fn db(app: &App) -> &ContentDb {
    app.world().resource::<ContentDb>()
}

pub fn worldgen(app: &App) -> WorldGen {
    app.world().resource::<WorldGen>().clone()
}

/// The content on disk, with `edit` applied to the text of the file at `path` (relative to
/// `content/`), built with the app's kinds.
pub fn edited_db(app: &App, path: &str, edit: impl Fn(&str) -> String) -> ContentDb {
    let kinds = app.world().resource::<ContentKinds>().clone();
    let mut sources: Vec<Source> = read_dir(Path::new(ASSETS)).expect("test content");
    let wanted = format!("content/{path}");
    let file = sources.iter_mut().find(|s| s.path == wanted).unwrap_or_else(|| panic!("no {wanted}"));
    file.text = edit(&file.text);
    ContentDb::build(&kinds, &sources)
}

/// Content `files` (path under content/ → text) on top of the test content, built.
pub fn db_with(app: &App, files: &[(&str, &str)]) -> ContentDb {
    let kinds = app.world().resource::<ContentKinds>().clone();
    let mut sources: Vec<Source> = read_dir(Path::new(ASSETS)).expect("test content");
    for (path, text) in files {
        sources.push(Source { path: format!("content/{path}"), text: (*text).to_owned() });
    }
    ContentDb::build(&kinds, &sources)
}

pub fn hero(app: &mut App) -> Entity {
    app.world_mut().query_filtered::<Entity, With<Hero>>().single(app.world()).expect("one hero")
}

pub fn heroes(app: &mut App) -> usize {
    app.world_mut().query_filtered::<Entity, With<Hero>>().iter(app.world()).count()
}

pub fn pos(app: &App, entity: Entity) -> Vec3 {
    app.world().get::<Transform>(entity).expect("a transform").translation
}

pub fn journal(app: &App) -> Vec<String> {
    app.world().resource::<ai_core::AiJournal>().after(0).0.map(|e| e.text.clone()).collect()
}

pub fn region(app: &App) -> &ActiveRegion {
    app.world().get_resource::<ActiveRegion>().expect("a region in the world")
}
