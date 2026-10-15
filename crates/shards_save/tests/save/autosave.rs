//! Saves the game decides on: an event (camp, beacon, boss) or a system (a reward) asks, the
//! file is written at the end of that tick, debounced, and the simulation never notices.

use std::fs;

use serde_json::json;
use shards::ai_core::{replay, snapshot, JournalKind, SimTick};
use shards::save::{SaveFile, SaveRequests, SaveWhat};
use shards::testing;

use shards::prelude::*;

use crate::common::*;

#[derive(Event, Clone, Debug)]
struct CampRested;

/// Every tick: the calendar's day and the hero's xp go up by one and the hero walks, so the
/// saved values say at which point of the tick they were read.
fn tick_up(mut calendar: ResMut<Calendar>, mut heroes: Query<(&mut Hero, &mut Transform)>) {
    calendar.day += 1;
    for (mut hero, mut tf) in &mut heroes {
        hero.xp += 1;
        tf.translation.x += 0.25;
    }
}

/// Ticks at which the hero rests at a camp.
const CAMPS: [u64; 3] = [5, 8, 200];

fn camp(mut commands: Commands, tick: Res<SimTick>) {
    if CAMPS.contains(&tick.0) {
        commands.trigger(CampRested);
    }
}

fn camp_game(dir: &TempDir, autosave: bool) -> App {
    game_in(dir, |app| {
        app.add_systems(FixedUpdate, (camp.in_set(SimSet::Act), tick_up.in_set(SimSet::Move)));
        if autosave {
            app.autosave_on::<CampRested>(SaveWhat::All);
        }
    })
}

fn day_in(dir: &TempDir) -> (u64, u64) {
    let world = SaveFile::decode(&fs::read(dir.file("world/slot1.save")).unwrap()).unwrap();
    let character = SaveFile::decode(&fs::read(dir.file("character/slot1.save")).unwrap()).unwrap();
    (world.sections.0["calendar"].data["day"].as_u64().unwrap(), character.sections.0["hero"].data["xp"].as_u64().unwrap())
}

#[test]
fn an_event_saves_at_the_end_of_that_tick() {
    let dir = TempDir::new("event");
    let mut app = camp_game(&dir, true);
    step(&mut app, 4);
    assert!(!dir.file("world/slot1.save").exists(), "nothing before the event");
    step(&mut app, 1);
    // The event came in the Act set of tick 5; the day went up in Move after it. The file has the state
    // at the end of the tick, written by FixedLast.
    assert_eq!(day_in(&dir), (5, 5));
    let world = SaveFile::decode(&fs::read(dir.file("world/slot1.save")).unwrap()).unwrap();
    assert_eq!((world.tick, world.label.as_str()), (5, "camp_rested"));
    let said = lines(&app, JournalKind::Sim, "save: world/slot1 saved (camp_rested, ");
    assert_eq!(said.len(), 1, "{:?}", journal(&app));
    assert!(said[0].ends_with(", tick 5)"), "{}", said[0]);
    assert_eq!(lines(&app, JournalKind::Sim, "save: character/slot1 saved (camp_rested, ").len(), 1);
}

#[test]
fn autosaves_do_not_change_the_simulation() {
    let (dir_a, dir_b) = (TempDir::new("with"), TempDir::new("without"));
    let (mut with, mut without) = (camp_game(&dir_a, true), camp_game(&dir_b, false));
    for app in [&mut with, &mut without] {
        step(app, 100);
    }
    assert!(dir_a.file("world/slot1.save").exists() && dir_a.file("character/slot1.save").exists());
    assert!(!dir_b.file("world").exists(), "the other game saved nothing");
    assert_eq!(replay::state_hash(with.world()), replay::state_hash(without.world()));
    let (a, b) = (snapshot::capture(with.world()).unwrap(), snapshot::capture(without.world()).unwrap());
    assert_eq!((a.tick, a.entities, a.resources), (b.tick, b.entities, b.resources));
    // Not even the walk of the hero differs, and it moved.
    let x = |app: &mut App| {
        let hero = hero_entity(app);
        app.world().get::<Transform>(hero).unwrap().translation.x
    };
    assert_eq!((x(&mut with), x(&mut without)), (25.0, 25.0));
}

#[test]
fn a_second_request_waits_for_the_debounce_window_and_is_not_lost() {
    // Content: the world is autosaved at most once a second (60 ticks), the character twice as often.
    let dir = TempDir::new("debounce");
    let mut app = camp_game(&dir, true);
    let db = app.world().resource::<shards::content::ContentDb>();
    assert_eq!(db.get::<shards::save::SaveDef>("world").map(|d| d.debounce_secs), Some(1.0));
    assert!(db.errors().next().is_none());

    step(&mut app, 8);
    assert_eq!(day_in(&dir), (5, 5), "the second camp, at tick 8, waits");
    step(&mut app, 26);
    assert_eq!(day_in(&dir), (5, 5), "tick 34: the character's 30 ticks are not over");
    step(&mut app, 1);
    assert_eq!(day_in(&dir), (5, 35), "tick 35: the character, as it is then");
    step(&mut app, 29);
    assert_eq!(day_in(&dir), (5, 35), "tick 64: the world's 60 ticks are not over");
    step(&mut app, 1);
    assert_eq!(day_in(&dir), (65, 35), "tick 65: the world, as it is then");
    // Nothing is left to write, and the third camp finds the windows over.
    step(&mut app, 134);
    assert_eq!(day_in(&dir), (65, 35));
    step(&mut app, 1);
    assert_eq!(day_in(&dir), (200, 200));
    assert_eq!(lines(&app, JournalKind::Sim, "saved (camp_rested").len(), 6);
}

/// Asks for a save from an ordinary system, twice in a tick.
fn found_the_ford(tick: Res<SimTick>, mut requests: ResMut<SaveRequests>) {
    if tick.0 == 3 {
        requests.request(SaveWhat::Character(None), "found the ford");
        requests.request(SaveWhat::Character(None), "found it again");
    }
}

#[test]
fn a_system_can_ask_for_a_save_and_the_same_ask_twice_is_one_file() {
    let dir = TempDir::new("request");
    let mut app = game_in(&dir, |app| {
        app.add_systems(FixedUpdate, (found_the_ford.in_set(SimSet::Act), tick_up.in_set(SimSet::Move)));
    });
    step(&mut app, 5);
    assert!(!dir.file("world/slot1.save").exists());
    let saved = lines(&app, JournalKind::Sim, "save: character/slot1 saved");
    assert_eq!(saved.len(), 1, "{:?}", journal(&app));
    assert!(saved[0].contains("(found the ford, ") && saved[0].ends_with(", tick 3)"), "{}", saved[0]);
    let list = act(&mut app, "save_list", json!({})).result;
    assert_eq!(list["slots"][0]["files"][0]["label"], "found the ford");
    assert_eq!(list["slots"][0]["files"][0]["tick"], 3);
}

#[test]
fn autosaves_write_nothing_while_a_replay_plays_and_the_replay_still_verifies() {
    let (dir_a, dir_b) = (TempDir::new("recorded"), TempDir::new("played"));
    let mut recorded = camp_game(&dir_a, true);
    step(&mut recorded, 70);
    let recording = replay::save(recorded.world());
    assert!(dir_a.file("world/slot1.save").exists(), "the recorded session did save");

    // The same game, built fresh, plays it back.
    let mut app = App::new();
    app.add_plugins(shards::EnginePlugins::new(shards::EngineConfig { asset_dir: ASSETS.into(), ..testing::config() }));
    app.init_resource::<Calendar>().add_systems(OnEnter(EngineState::InGame), spawn_world);
    register_saves(&mut app);
    app.add_systems(FixedUpdate, (camp.in_set(SimSet::Act), tick_up.in_set(SimSet::Move)));
    app.autosave_on::<CampRested>(SaveWhat::All);
    app.world_mut().resource_mut::<shards::save::SaveConfig>().dir = dir_b.path().to_owned();
    app.finish();
    app.cleanup();
    let report = testing::verify(&mut app, recording).unwrap();
    assert!(report.identical(), "{report}");
    assert!(!dir_b.file("world").exists() && !dir_b.file("character").exists(), "the player's saves are not overwritten by a replay");
    assert!(!lines(&app, JournalKind::Sim, "autosave (camp_rested) skipped while a replay plays").is_empty(), "{:?}", journal(&app));
}

#[test]
fn autosaves_still_waiting_are_written_when_the_game_exits() {
    let dir = TempDir::new("exit");
    let mut app = camp_game(&dir, true);
    step(&mut app, 8);
    // The camp at tick 5 was written; the one at tick 8 waits for the debounce window (30 and 60 ticks).
    assert_eq!(day_in(&dir), (5, 5));
    assert!(app.world().resource::<SaveRequests>().is_pending());
    assert!(
        app.world().resource::<shards::core::ExitHooks>().names().any(|n| n.starts_with("save:")),
        "the save module registers its exit hook"
    );

    // The game ends: an `AppExit`, as a closed window or `kill` makes one.
    app.world_mut().write_message(AppExit::Success);
    app.update();
    assert_eq!(day_in(&dir), (8, 8), "the file holds the newest state, not the one before the debounce");
    assert!(!app.world().resource::<SaveRequests>().is_pending());
    assert_eq!(lines(&app, JournalKind::Sim, "saved (camp_rested").len(), 4, "tick 5 and the flush, both kinds: {:?}", journal(&app));
    let (world, character) = (SaveFile::decode(&fs::read(dir.file("world/slot1.save")).unwrap()).unwrap(), SaveFile::decode(&fs::read(dir.file("character/slot1.save")).unwrap()).unwrap());
    assert_eq!((world.tick, character.tick), (8, 8));
    // The previous save is the first backup.
    assert!(dir.file("world/slot1.save.bak1").exists());
}

#[test]
fn flush_saves_is_there_for_a_games_own_way_out() {
    let dir = TempDir::new("flush");
    let mut app = camp_game(&dir, true);
    step(&mut app, 8);
    assert_eq!(shards::save::flush_saves(app.world_mut()), 2, "the world and the character");
    assert_eq!(day_in(&dir), (8, 8));
    assert_eq!(shards::save::flush_saves(app.world_mut()), 0, "nothing is left");
    // And the exit that follows has nothing to do.
    let before = fs::read(dir.file("world/slot1.save")).unwrap();
    app.world_mut().write_message(AppExit::Success);
    app.update();
    assert_eq!(fs::read(dir.file("world/slot1.save")).unwrap(), before);
}
