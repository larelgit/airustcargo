//! A replay being played back writes nothing to disk, however the recorded session asked: an
//! exclusive system, a tool of the game, a `save_game` call.

use std::collections::BTreeMap;
use std::fs;
use std::time::{Duration, SystemTime};

use serde_json::json;
use shards::ai_core::replay::{self, ReplayEvent};
use shards::ai_core::{AiAppExt, Call, JournalKind, NoArgs, Scope, SimTick, ToolResult};
use shards::save::{flush_saves, save_now, SaveRequests, SaveWhat};
use shards::testing;

use shards::prelude::*;

use crate::common::*;

/// What `save_now` answered at each reward: written or not.
#[derive(Resource, Default)]
struct Rewards(Vec<bool>);

/// A reward at tick 4: an exclusive system saves the whole game at once.
fn reward(world: &mut World) {
    if world.resource::<SimTick>().0 == 4 {
        let written = save_now(world, SaveWhat::All, "reward").unwrap().iter().all(|r| r.written);
        world.resource_mut::<Rewards>().0.push(written);
    }
}

/// A tool of the game that saves, an action: it is in the recording.
fn bank(In(_): In<Call<NoArgs>>, world: &mut World) -> ToolResult {
    let reports = save_now(world, SaveWhat::World, "bank")?;
    Ok(format!("banked {}", reports.len()))
}

fn banking_game(dir: &TempDir) -> App {
    game_in(dir, |app| {
        app.init_resource::<Rewards>().add_systems(FixedUpdate, reward.in_set(SimSet::Act)).add_ai_tool("bank", Scope::Dev, bank);
    })
}

/// Every file of the save folder: name → (bytes, modified).
fn files_of(dir: &TempDir) -> BTreeMap<String, (Vec<u8>, SystemTime)> {
    let mut out = BTreeMap::new();
    for kind in ["world", "character"] {
        let Ok(entries) = fs::read_dir(dir.file(kind)) else { continue };
        for entry in entries {
            let entry = entry.unwrap();
            let modified = entry.metadata().unwrap().modified().unwrap();
            out.insert(format!("{kind}/{}", entry.file_name().to_string_lossy()), (fs::read(entry.path()).unwrap(), modified));
        }
    }
    out
}

/// The session of game A: a reward at tick 4, `bank` called at tick 2, 8 ticks; plus the
/// calls a hand-edited recording could hold, `save_game` and `save_damage`, at tick 5.
fn recording(dir: &TempDir) -> replay::Replay {
    let mut app = banking_game(dir);
    step(&mut app, 2);
    ok(&mut app, "bank", json!({}));
    step(&mut app, 6);
    assert_eq!(app.world().resource::<Rewards>().0, [true], "the recorded session wrote its reward");
    assert!(dir.file("world/slot1.save").exists() && dir.file("character/slot1.save").exists());
    let mut recording = replay::save(app.world());
    assert!(recording.events.iter().any(|e| matches!(e, ReplayEvent::Call { tool, .. } if tool == "bank")), "an action that saves is recorded");
    for (tool, args) in [("save_game", json!({"what": "all"})), ("save_damage", json!({"what": "world", "how": "flip"}))] {
        let at = recording.events.iter().position(|e| e.at() > 5).unwrap_or(recording.events.len());
        recording.events.insert(at, ReplayEvent::Call { at: 5, agent: "dev".into(), scope: Scope::Dev, subject: None, tool: tool.into(), args });
    }
    recording
}

#[test]
fn a_replay_that_saves_plays_back_with_every_slot_file_untouched() {
    let (dir_a, dir_b) = (TempDir::new("recorded"), TempDir::new("played"));
    let recording = recording(&dir_a);

    // The player's own saves, from earlier sessions of another state.
    let mut app = banking_game(&dir_b);
    app.world_mut().resource_mut::<Calendar>().day = 77;
    set_hero(&mut app, Hero { name: "Ada".into(), level: 9, ..Hero::default() });
    ok(&mut app, "save_game", json!({"what": "all", "label": "mine"}));
    ok(&mut app, "save_game", json!({"what": "all", "label": "mine again"}));
    let mine = files_of(&dir_b);
    assert_eq!(mine.len(), 4, "{:?}", mine.keys().collect::<Vec<_>>());
    std::thread::sleep(Duration::from_millis(40));

    let report = testing::verify(&mut app, recording).unwrap();
    assert!(report.identical(), "{report}");
    assert_eq!(files_of(&dir_b), mine, "bytes and modification times: nothing was written, rotated, damaged or removed");
    assert!(!dir_b.file("world/slot1.save.damaged").exists());

    // The calls answered, and the journal says what was not done.
    assert_eq!(app.world().resource::<Rewards>().0, [false], "save_now answers Ok, with written = false");
    for place in ["world/slot1, reward", "character/slot1, reward", "world/slot1, bank", "world/slot1, save_game"] {
        let said = lines(&app, JournalKind::Sim, &format!("save: not written during replay playback ({place})"));
        assert_eq!(said.len(), 1, "{place}: {:?}", journal(&app));
    }
    assert!(lines(&app, JournalKind::Error, "").is_empty(), "{:?}", journal(&app));
}

#[test]
fn the_tools_answer_that_nothing_was_written_while_a_replay_plays() {
    let (dir_a, dir_b) = (TempDir::new("answer_a"), TempDir::new("answer_b"));
    let recording = recording(&dir_a);
    let mut app = banking_game(&dir_b);
    ok(&mut app, "save_game", json!({"what": "world"}));
    let mine = files_of(&dir_b);
    replay::play(app.world_mut(), recording).unwrap();

    // A person at the controls while it plays: the control tools say no and write nothing.
    let err = fails(&mut app, "save_game", json!({"what": "world"}));
    assert!(err.starts_with("save: not written during replay playback: world/slot1 not written during replay playback (save_game)"), "{err}");
    let err = fails(&mut app, "save_damage", json!({"what": "world", "how": "flip"}));
    assert!(err.starts_with("save: not written during replay playback"), "{err}");
    // Reading is fine: `save_check` and `save_list` change nothing.
    ok(&mut app, "save_check", json!({"what": "world", "slot": "slot1"}));
    assert_eq!(files_of(&dir_b), mine);

    let slots = app.world().resource::<shards::save::SaveSlots>().clone();
    assert_eq!((slots.world.as_str(), slots.character.as_str()), ("slot1", "slot1"), "a refused save does not change the current slot");

    // The exit flush writes nothing either, and drops what was asked.
    app.world_mut().resource_mut::<SaveRequests>().request(SaveWhat::World, "camp");
    assert_eq!(flush_saves(app.world_mut()), 0);
    assert!(!app.world().resource::<SaveRequests>().is_pending());
    assert_eq!(files_of(&dir_b), mine);
    assert!(!lines(&app, JournalKind::Sim, "autosave (camp) skipped while a replay plays").is_empty(), "{:?}", journal(&app));
}
