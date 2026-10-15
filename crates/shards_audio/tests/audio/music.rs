//! The music director: layers by the game's conditions, the hold before a calmer one, fades,
//! `music_set`, the journal and snapshots.

use serde_json::json;
use shards::ai_core::{snapshot, JournalKind};
use shards::prelude::*;

use crate::*;

fn fight(app: &mut App, on: bool) {
    app.world_mut().resource_mut::<Fighting>().0 = on;
}

#[test]
fn the_start_track_plays_its_calm_layer_and_a_fight_turns_it_to_combat_at_once() {
    let assets = Folder::new("combat");
    let mut app = game(&assets);
    assert_eq!(music(&app).track, None, "nothing before the first tick");
    step(&mut app, 1);
    assert_eq!((music(&app).track.as_deref(), music(&app).layer.as_deref()), (Some("theme"), Some("explore")));
    assert_eq!(journal(&app, JournalKind::Local, "music:"), ["music: theme plays explore"]);
    wait(&mut app, 0.5);
    assert_eq!(stems(&app), [("music/calm.wav".to_owned(), 1.0), ("music/drums.wav".to_owned(), 0.0)], "the calm stem came in over `fade_in`");

    fight(&mut app, true);
    step(&mut app, 1);
    assert_eq!(music(&app).layer.as_deref(), Some("combat"));
    assert_eq!(journal(&app, JournalKind::Local, "→"), ["music: theme explore → combat"]);
    let quarter = ticks(&app, 0.25);
    step(&mut app, quarter - 1);
    near(stems(&app)[1].1, 0.5);
    wait(&mut app, 0.25);
    near(stems(&app)[1].1, 1.0);
}

#[test]
fn a_calmer_layer_waits_for_the_hold_and_a_fight_on_the_way_starts_it_again() {
    let assets = Folder::new("hold");
    let mut app = game(&assets);
    fight(&mut app, true);
    wait(&mut app, 1.0);
    fight(&mut app, false);
    wait(&mut app, 1.0);
    let said = ok(&mut app, "audio", json!({}));
    assert!(said.contains("music theme: combat for 2.0s; explore wanted 1.0/2.0s"), "{said}");

    // A blow on the way: the calm counts from nothing again.
    fight(&mut app, true);
    step(&mut app, 1);
    fight(&mut app, false);
    wait(&mut app, 1.5);
    assert_eq!(music(&app).layer.as_deref(), Some("combat"));
    wait(&mut app, 0.5);
    assert_eq!(music(&app).layer.as_deref(), Some("explore"));
    assert_eq!(journal(&app, JournalKind::Local, "→"), ["music: theme combat → explore"]);
    // The drums go over `fade_out` (the tick of the change took them down already), the calm
    // stem stays.
    let half = ticks(&app, 0.5);
    step(&mut app, half - 1);
    near(stems(&app)[1].1, 0.5);
    assert_eq!(stems(&app)[0].1, 1.0);
}

#[test]
fn music_set_holds_a_layer_changes_the_track_and_explains_a_wrong_call() {
    let assets = Folder::new("set");
    let mut app = game(&assets);
    step(&mut app, 1);
    assert_eq!(ok(&mut app, "music_set", json!({"layer": "combat"})), "theme keeps to combat until `music_set layer=auto`");
    step(&mut app, 1);
    assert_eq!(music(&app).layer.as_deref(), Some("combat"), "whatever the game does");
    assert!(ok(&mut app, "observe", json!({})).contains("MUSIC  theme: combat for 0.0s (music_set)"));
    assert_eq!(ok(&mut app, "music_set", json!({"layer": "auto"})), "theme follows the game again");
    wait(&mut app, 2.0);
    assert_eq!(music(&app).layer.as_deref(), Some("explore"), "after the hold");

    assert_eq!(ok(&mut app, "music_set", json!({"track": "quiet"})), "theme → quiet");
    wait(&mut app, 0.5);
    let tracks: Vec<(String, String)> = music(&app).stems.iter().map(|s| (s.track.clone(), s.file.clone())).collect();
    assert_eq!(tracks.len(), 3, "theme's calm stem fades out while quiet's comes in: {tracks:?}");
    wait(&mut app, 1.0);
    assert_eq!(stems(&app), [("music/calm.wav".to_owned(), 1.0)], "theme's stems are gone");
    assert_eq!(ok(&mut app, "music_set", json!({"track": "none"})), "quiet fades out");
    wait(&mut app, 2.5);
    assert!(music(&app).stems.is_empty());
    assert!(!ok(&mut app, "observe", json!({})).contains("MUSIC"), "no music, no line");
    assert_eq!(journal(&app, JournalKind::Local, "music:"), ["music: theme plays explore", "music: theme explore → combat", "music: theme combat → explore", "music: theme → quiet", "music: quiet plays explore", "music: quiet fades out"]);

    assert_eq!(refused(&mut app, "music_set", json!({"layer": "combat"})), "no music plays: start a track first (`music_set track=<id>`)");
    assert_eq!(refused(&mut app, "music_set", json!({"track": "thme"})), "no track `thme` (did you mean `theme`?); tracks: quiet, theme");
    ok(&mut app, "music_set", json!({"track": "theme"}));
    assert_eq!(refused(&mut app, "music_set", json!({"layer": "combt"})), "theme has no layer `combt` (did you mean `combat`?); layers: explore, combat");
    assert!(refused(&mut app, "music_set", json!({})).starts_with("give `track` (a track, or none) or `layer` (a layer, or auto); now: theme: "));
}

#[test]
fn the_music_is_this_machines_and_no_gameplay_event() {
    let assets = Folder::new("local");
    let mut app = game(&assets);
    fight(&mut app, true);
    step(&mut app, 2);
    assert_eq!(journal(&app, JournalKind::Local, "music:").len(), 1);
    assert!(journal(&app, JournalKind::Event, "music").is_empty(), "a co-op host sends its events to clients; not its music");
    let before = shards::ai_core::replay::state_hash(app.world());
    ok(&mut app, "music_set", json!({"layer": "explore"}));
    step(&mut app, 0);
    assert_eq!(shards::ai_core::replay::state_hash(app.world()), before, "nothing of it is in the replay's hash");
}

#[test]
fn a_snapshot_brings_back_the_layer_its_time_and_the_fades() {
    let assets = Folder::new("snapshot");
    let mut app = game(&assets);
    fight(&mut app, true);
    wait(&mut app, 0.25);
    let saved = snapshot::capture(app.world()).unwrap();
    let then = music(&app);
    fight(&mut app, false);
    wait(&mut app, 3.0);
    assert_eq!(music(&app).layer.as_deref(), Some("explore"));
    snapshot::restore(app.world_mut(), &saved, false).unwrap();
    assert_eq!(music(&app), then);
    // And on from there by the simulation's ticks, not by the jump back.
    fight(&mut app, true);
    wait(&mut app, 0.25);
    near(music(&app).since, then.since + 0.25);
    near(stems(&app)[1].1, 1.0);
}
