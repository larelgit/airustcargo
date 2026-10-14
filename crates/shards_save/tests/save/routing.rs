//! `SaveRouting`: a networked game (`shards::coop`) says where a save goes. The save module
//! asks it first and writes nothing here when it answers; it tells it which saves the game
//! decided on were written here.

use shards::ai_core::WorldRebuilt;
use shards::save::{SaveKind, SaveReport, SaveRequests, SaveRouting, SaveWhat, save_now};
use shards::serde_json::json;

use shards::prelude::*;

use crate::common::*;

/// What the routing was asked, `kind:reason`.
#[derive(Resource, Default)]
struct Asked(Vec<String>);

/// What it was told was saved here, `kind:reason`.
#[derive(Resource, Default)]
struct Saved(Vec<String>);

/// Every character save goes to "elsewhere" (a remote player's machine), the world is written here.
fn elsewhere(world: &mut World, kind: SaveKind, _: Option<Entity>, reason: &str) -> Option<Result<SaveReport, String>> {
    world.get_resource_or_init::<Asked>().0.push(format!("{kind}:{reason}"));
    (kind == SaveKind::Character).then(|| {
        Ok(SaveReport { kind, slot: "p1".into(), tick: 7, bytes: 10, sections: 1, reason: reason.to_owned(), written: true, target: Some("elsewhere".into()) })
    })
}

fn saved(world: &mut World, kind: SaveKind, reason: &str) {
    world.get_resource_or_init::<Saved>().0.push(format!("{kind}:{reason}"));
}

fn routed(dir: &TempDir) -> App {
    let mut app = game(dir);
    app.insert_resource(SaveRouting { route: elsewhere, saved });
    app
}

fn told(app: &App) -> Vec<String> {
    app.world().get_resource::<Saved>().map(|s| s.0.clone()).unwrap_or_default()
}

#[test]
fn a_routed_save_is_answered_by_the_route_and_no_file_is_written_here() {
    let dir = TempDir::new("routed");
    let mut app = routed(&dir);
    let out = ok(&mut app, "save_game", json!({"what": "all", "slot": "camp"}));
    assert!(out.contains("character/p1 sent to elsewhere (save_game, tick 7)") && out.contains("world/camp saved (save_game"), "{out}");
    assert!(dir.file("world/camp.save").exists());
    assert!(!dir.file("character").exists(), "the character went elsewhere");
    // The slot the character was routed from is not "current": nothing was written to it.
    let slots = shards::save::SaveSlots::default();
    assert_eq!(app.world().resource::<shards::save::SaveSlots>().character, slots.character);
    assert_eq!(app.world().resource::<shards::save::SaveSlots>().world, "camp");
    assert_eq!(app.world().resource::<Asked>().0, ["world:save_game", "character:save_game"]);
    assert!(told(&app).is_empty(), "`save_game` is the player's menu, not the game deciding");
}

#[test]
fn the_route_hears_which_saves_the_game_decided_on_were_written_here() {
    let dir = TempDir::new("decided");
    let mut app = routed(&dir);
    save_now(app.world_mut(), SaveWhat::All, "camp").unwrap();
    assert_eq!(told(&app), ["world:camp"], "the character was routed, only the world was written here");

    // An autosave tells it the same, at the end of the tick.
    app.world_mut().resource_mut::<SaveRequests>().request(SaveWhat::World, "beacon");
    step(&mut app, 1);
    assert_eq!(told(&app), ["world:camp", "world:beacon"]);
}

#[test]
fn a_loaded_world_is_a_rebuilt_world_for_whoever_mirrors_it() {
    #[derive(Resource, Default)]
    struct Rebuilt(Vec<String>);
    let dir = TempDir::new("rebuilt");
    let mut app = game(&dir);
    app.init_resource::<Rebuilt>().add_observer(|e: On<WorldRebuilt>, mut seen: ResMut<Rebuilt>| seen.0.push(e.reason.clone()));
    ok(&mut app, "save_game", json!({"what": "all", "slot": "camp"}));
    ok(&mut app, "load_game", json!({"what": "character", "slot": "camp"}));
    assert!(app.world().resource::<Rebuilt>().0.is_empty(), "a character is no world");
    ok(&mut app, "load_game", json!({"what": "world", "slot": "camp"}));
    assert_eq!(app.world().resource::<Rebuilt>().0, ["save camp loaded"]);
}
