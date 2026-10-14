//! Co-op saves (C8): characters travel with their players, the host keeps the world
//! (`shards::coop`). Stands of 150 ms and 2 % loss, every member with a save folder of its
//! own, removed when the test ends.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use shards::action::Health;
use shards::ai_core::Scope;
use shards::net::{LinkConditions, NetClient, NetDef, NetDefaults, NetLeft, Stand};
use shards::prelude::*;
use shards::save::{Damage, GameInfo, SaveConfig, SaveFile, SaveKey, SaveKind, SaveWhat, SectionData, SectionSet, Store, Which};
use shards::serde_json::{json, Value};

use super::net::*;
use crate::common::*;

// --- what the sandbox saves for these tests -------------------------------------------------

/// A character section of the tests: coins on the player's body.
#[derive(Component, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(crate = "shards::serde")]
struct Bag {
    coins: u32,
}

/// A world section of the tests: a lever on an entity with a `SaveKey`.
#[derive(Component, Reflect, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
#[serde(crate = "shards::serde")]
struct Lever {
    pulled: bool,
}

/// A camp: the game saves the characters when it is rested at.
#[derive(Event)]
struct Camp;

fn setup_for(dir: PathBuf, def: NetDef) -> impl Fn(&mut App) {
    move |app: &mut App| {
        app.add_plugins(sandbox::SandboxPlugin);
        app.insert_resource(NetDefaults(def.clone()));
        app.register_type::<Lever>()
            .save_component::<Bag>(SaveKind::Character, "bag", 1)
            .save_component::<Lever>(SaveKind::World, "lever", 1)
            .autosave_on::<Camp>(SaveWhat::Character(None));
        app.world_mut().resource_mut::<SaveConfig>().dir = dir.clone();
    }
}

/// Folders of a test, one per member, gone when it ends.
struct Folders(PathBuf);

impl Folders {
    fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("sandbox_coop_{tag}_{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = fs::remove_dir_all(&dir);
        Self(dir)
    }

    fn of(&self, member: &str) -> PathBuf {
        self.0.join(member)
    }
}

impl Drop for Folders {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn store(dir: &Path) -> Store {
    Store { root: dir.to_owned(), sync: false }
}

/// A character file in `dir`: this much health, this many coins, saved at host tick `tick`.
fn write_character(dir: &Path, slot: &str, health: f32, coins: u32, tick: u64) {
    let mut sections = BTreeMap::new();
    sections.insert(
        "vitals".to_owned(),
        SectionData { version: 1, data: json!({"health": {"current": health, "max": 100.0}, "stamina": null, "charges": []}) },
    );
    sections.insert("bag".to_owned(), SectionData { version: 1, data: json!({"coins": coins}) });
    let file = SaveFile {
        kind: SaveKind::Character,
        slot: slot.to_owned(),
        label: "test".into(),
        game: GameInfo::default(),
        engine: shards::core::VERSION.to_owned(),
        tick,
        sections: SectionSet(sections),
    };
    store(dir).write(SaveKind::Character, slot, &file.encode()).unwrap();
}

fn read_file(dir: &Path, kind: SaveKind, slot: &str) -> SaveFile {
    let path = dir.join(kind.as_str()).join(format!("{slot}.save"));
    SaveFile::decode(&fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))).unwrap()
}

fn coins_in(dir: &Path, slot: &str) -> u64 {
    read_file(dir, SaveKind::Character, slot).sections.0["bag"].data["coins"].as_u64().unwrap()
}

fn guests() -> NetDef {
    NetDef { guest_scope: Scope::Dev, ..NetDef::default() }
}

/// A host with its own folder, loaded, nobody joined.
fn host_stand(folders: &Folders, def: &NetDef) -> Stand {
    shards::testing::stand_host(2, &configure, &setup_for(folders.of("host"), def.clone()))
}

/// Client `name`, playing the character in its slot `name` of its own folder.
fn add_client(stand: &mut Stand, folders: &Folders, name: &str, def: &NetDef, links: LinkConditions) -> usize {
    let configure = |c: &mut shards::EngineConfig| {
        c.seed = 42;
        c.character_slot = Some(name.to_owned());
    };
    shards::testing::stand_client(stand, name, links, &configure, &setup_for(folders.of(name), def.clone()))
}

fn join(stand: &mut Stand, member: usize) {
    run_until(stand, 400, "the client joins", |s| connected(s, member));
}

/// A stand with a host and `c1` playing the file `c1/character/c1.save` (if the folder has
/// one), joined.
fn joined(folders: &Folders, def: &NetDef) -> (Stand, usize, String) {
    let mut stand = host_stand(folders, def);
    let c1 = add_client(&mut stand, folders, "c1", def, lossy());
    join(&mut stand, c1);
    let me = body(&stand, c1);
    (stand, c1, me)
}

fn give(stand: &mut Stand, body: &str, coins: u32) {
    let entity = entity(stand, HOST, body).expect("the host has the body");
    stand.world_mut(HOST).entity_mut(entity).insert(Bag { coins });
}

fn bag(stand: &Stand, member: usize, body: &str) -> Option<u32> {
    stand.world(member).get::<Bag>(entity(stand, member, body)?).map(|b| b.coins)
}

fn health(stand: &Stand, member: usize, body: &str) -> f32 {
    stand.world(member).get::<Health>(entity(stand, member, body).expect("the body")).unwrap().current
}

fn call_args(stand: &Stand, tool: &str) -> Vec<Value> {
    host_calls(stand, tool).into_iter().map(|(_, _, args)| args).collect()
}

// --- a character file joins -----------------------------------------------------------------

#[test]
fn a_player_joins_with_the_character_in_their_file() {
    let folders = Folders::new("join");
    write_character(&folders.of("c1"), "c1", 61.5, 5, 900);
    let (mut stand, c1, me) = joined(&folders, &NetDef::default());
    // The body on the host, and on the client, has what the file said.
    assert_eq!(health(&stand, HOST, &me), 61.5);
    assert_eq!(bag(&stand, HOST, &me), Some(5));
    stand.run(30);
    assert_eq!(health(&stand, c1, &me), health(&stand, HOST, &me), "the client sees the same body");
    assert!(has_line(&stand, c1, "save: bringing character/c1 (tick 900) to the host"), "{:#?}", journal(&stand, c1));
    assert!(has_line(&stand, HOST, "save: p1 character applied (vitals, bag)"), "{:#?}", journal(&stand, HOST));
    // The character is in the recorded join (the token is not), so a replay builds the same body.
    let joins = call_args(&stand, "player_join");
    assert_eq!(joins.len(), 1, "{joins:?}");
    assert!(joins[0]["character"].is_string() && joins[0].get("token").is_none(), "{:?}", joins[0]);
    assert_eq!(desyncs(&stand, c1), 0, "{:#?}", journal(&stand, c1));
    verify_host(&stand);
}

#[test]
fn a_player_with_no_file_joins_as_a_new_player() {
    let folders = Folders::new("new");
    let (stand, c1, me) = joined(&folders, &NetDef::default());
    assert_eq!(bag(&stand, HOST, &me), None);
    assert!(has_line(&stand, c1, "no character yet, joining as a new player"), "{:#?}", journal(&stand, c1));
    assert!(call_args(&stand, "player_join")[0].get("character").is_none());
}

#[test]
fn a_damaged_file_is_restored_from_its_backup_and_used() {
    let folders = Folders::new("damaged");
    let dir = folders.of("c1");
    write_character(&dir, "c1", 50.0, 1, 100);
    write_character(&dir, "c1", 40.0, 2, 200);
    store(&dir).damage(SaveKind::Character, "c1", Which::Save, Damage::Flip).unwrap();
    let (stand, c1, me) = joined(&folders, &NetDef::default());
    assert!(has_line(&stand, c1, "restored from backup 1 (tick 100)"), "{:#?}", journal(&stand, c1));
    assert_eq!((bag(&stand, HOST, &me), health(&stand, HOST, &me)), (Some(1), 50.0), "the backup's character, not the damaged file's");
    assert_eq!(coins_in(&dir, "c1"), 1, "the file is the backup again");
}

#[test]
fn an_unreadable_character_is_journaled_and_the_player_starts_fresh() {
    let folders = Folders::new("unreadable");
    let mut stand = host_stand(&folders, &NetDef::default());
    stand.run(5);
    let garbage = stand.call(HOST, "player_join", json!({"player": 2, "name": "ann", "character": "{not json"}));
    assert!(garbage.ok, "the join goes on: {}", garbage.result);
    assert!(has_line(&stand, HOST, "save: p2 character could not be applied (bad JSON"), "{:#?}", journal(&stand, HOST));
    assert!(has_line(&stand, HOST, "; starting fresh"));
    // A save from a newer game is refused whole, nothing of it applied.
    let newer = stand.call(HOST, "player_join", json!({"player": 3, "name": "bob", "character": {"vitals": {"version": 9, "data": {}}}}));
    assert!(newer.ok, "{}", newer.result);
    assert!(has_line(&stand, HOST, "save: p3 character could not be applied (the save was saved by a newer version"), "{:#?}", journal(&stand, HOST));
    assert!(shards::core::player_body(stand.world(HOST), 2).is_some() && shards::core::player_body(stand.world(HOST), 3).is_some());
}

// --- progress goes back to its player -------------------------------------------------------

#[test]
fn an_autosave_event_on_the_host_updates_the_players_file() {
    let folders = Folders::new("autosave");
    write_character(&folders.of("c1"), "c1", 61.5, 5, 900);
    let (mut stand, c1, me) = joined(&folders, &NetDef::default());
    stand.run(20);
    // c1 gains progress on the host (a game system would; the test says it), the camp is rested at.
    give(&mut stand, &me, 11);
    stand.world_mut(HOST).trigger(Camp);
    stand.run(60);
    let file = read_file(&folders.of("c1"), SaveKind::Character, "c1");
    assert_eq!(file.sections.0["bag"].data["coins"], 11, "c1's own file has the progress");
    assert_eq!(file.label, "camp");
    assert!(file.tick >= 20, "the host's tick: {}", file.tick);
    assert!(has_line(&stand, c1, "save: character/c1 saved (camp"), "{:#?}", journal(&stand, c1));
    assert!(has_line(&stand, HOST, "save: character/p1 sent to p1's machine (camp"), "{:#?}", journal(&stand, HOST));
    // The host's own character is written on the host, and only there.
    assert!(folders.of("host").join("character/slot1.save").exists());
    assert_eq!(read_file(&folders.of("host"), SaveKind::Character, "slot1").sections.0.get("bag").map(|b| b.data.clone()), Some(Value::Null));
    assert!(!folders.of("host").join("character/c1.save").exists());
    verify_host(&stand);
}

#[test]
fn a_save_of_a_remote_players_body_goes_to_their_machine_not_the_hosts_slot() {
    let folders = Folders::new("remote");
    let (mut stand, c1, me) = joined(&folders, &guests());
    give(&mut stand, &me, 3);
    // The host's agent plays player 1 (a session bound to them): its save is theirs.
    shards::ai_core::configure_session(stand.world_mut(HOST), "dev", &shards::ai_core::SessionChange { player: Some(1), ..Default::default() }).unwrap();
    let saved = stand.call(HOST, "save_game", json!({"what": "character"}));
    assert!(saved.ok, "{}", saved.result);
    assert!(text(&saved).contains("character/p1 sent to p1's machine (save_game"), "{}", text(&saved));
    stand.run(60);
    assert_eq!(coins_in(&folders.of("c1"), "c1"), 3);
    assert!(!folders.of("host").join("character/slot1.save").exists(), "the host's own slot was not written for p1's character");
    assert!(connected(&stand, c1));
}

#[test]
fn a_player_who_leaves_has_the_latest_in_their_file_and_comes_back_to_it() {
    let folders = Folders::new("leave");
    write_character(&folders.of("c1"), "c1", 61.5, 5, 900);
    let (mut stand, c1, me) = joined(&folders, &NetDef::default());
    stand.run(20);
    give(&mut stand, &me, 7);
    // c1 quits: the host captures the body as it leaves and sends it to c1's link.
    shards::core::run_exit_hooks(stand.world_mut(c1));
    run_until(&mut stand, 90, "the host hears it", |s| has_line(s, HOST, "net: p1 left (quit)"));
    stand.run(60);
    assert_eq!(coins_in(&folders.of("c1"), "c1"), 7, "the file holds the latest");
    assert!(read_file(&folders.of("c1"), SaveKind::Character, "c1").label.starts_with("quit"));

    // The next run brings that file: the same slot, the same body, the same progress.
    let again = add_client(&mut stand, &folders, "c1", &NetDef::default(), lossy());
    join(&mut stand, again);
    assert_eq!(body(&stand, again), me);
    assert_eq!(bag(&stand, HOST, &me), Some(7));
    stand.run(30);
    let joins = call_args(&stand, "player_join");
    assert_eq!(joins.len(), 2, "{joins:?}");
    assert!(joins[1].get("character").is_none(), "the file is the host's own copy and the body stood: nothing is applied: {:?}", joins[1]);
    assert_eq!(coins_in(&folders.of("c1"), "c1"), 7);
    verify_host(&stand);
}

/// A def whose host forgets a silent client after a second (and a body after `grace` seconds).
fn quick(grace: f32) -> NetDef {
    NetDef { timeout_secs: 1.0, rejoin_grace_secs: grace, ..NetDef::default() }
}

/// c1 had 7 coins in its file; the host's c1 has 9 and never told it (the link died).
fn timed_out_with_unsaved_progress(folders: &Folders, def: &NetDef) -> (Stand, String) {
    write_character(&folders.of("c1"), "c1", 61.5, 5, 900);
    let (mut stand, c1, me) = joined(folders, def);
    stand.run(20);
    give(&mut stand, &me, 7);
    stand.world_mut(HOST).trigger(Camp);
    stand.run(60);
    assert_eq!(coins_in(&folders.of("c1"), "c1"), 7);
    give(&mut stand, &me, 9);
    stand.set_link(c1, "loss=1".parse().unwrap()).unwrap();
    stand.run(100);
    assert!(has_line(&stand, HOST, "net: p1 left (timeout)"), "{:#?}", journal(&stand, HOST));
    assert_eq!(coins_in(&folders.of("c1"), "c1"), 7, "the final save never arrived");
    (stand, me)
}

#[test]
fn a_rejoin_within_the_grace_keeps_the_body_and_sends_the_file_what_it_missed() {
    let folders = Folders::new("grace");
    let def = quick(120.0);
    let (mut stand, me) = timed_out_with_unsaved_progress(&folders, &def);
    let waiting = entity(&stand, HOST, &me).expect("the body waits");
    assert!(stand.world(HOST).get::<NetLeft>(waiting).is_some());

    // c1 comes back with its stale file (what the host sent it, 7 coins).
    let again = add_client(&mut stand, &folders, "c1", &def, lossy());
    join(&mut stand, again);
    assert_eq!(body(&stand, again), me);
    assert_eq!(bag(&stand, HOST, &me), Some(9), "the body kept what it had");
    stand.run(90);
    let joins = call_args(&stand, "player_join");
    assert!(joins.last().unwrap().get("character").is_none(), "{joins:?}");
    assert_eq!(coins_in(&folders.of("c1"), "c1"), 9, "the host sent c1 its progress when it came back");
    assert!(read_file(&folders.of("c1"), SaveKind::Character, "c1").label.starts_with("rejoined"));
}

#[test]
fn a_rejoin_after_the_grace_restores_the_character_the_host_kept() {
    let folders = Folders::new("kept");
    let def = quick(1.0);
    let (mut stand, me) = timed_out_with_unsaved_progress(&folders, &def);
    stand.run(120);
    assert!(entity(&stand, HOST, &me).is_none(), "the body is gone after the grace");

    let again = add_client(&mut stand, &folders, "c1", &def, lossy());
    join(&mut stand, again);
    let now = body(&stand, again);
    assert_eq!(bag(&stand, HOST, &now), Some(9), "built from what the host kept of c1, not from the stale file");
    stand.run(90);
    let joins = call_args(&stand, "player_join");
    assert!(joins.last().unwrap()["character"].is_string(), "the recorded join carries what the host chose: {joins:?}");
    assert_eq!(coins_in(&folders.of("c1"), "c1"), 9, "and c1's file caught up");
    verify_host_with(&stand, &def);
}

/// [`verify_host`] for a session whose host had numbers of its own (`def`).
fn verify_host_with(stand: &Stand, def: &NetDef) {
    let recording = shards::ai_core::replay::save(stand.world(HOST));
    let mut config = shards::testing::config();
    config.seed = recording.seed;
    let mut app = sandbox::build_app(&config);
    app.insert_resource(NetDefaults(def.clone()));
    app.finish();
    app.cleanup();
    let report = shards::testing::verify(&mut app, recording).unwrap();
    assert!(report.identical(), "the host's replay does not play the same in single player: {report}");
}

#[test]
fn a_file_the_host_never_sent_is_the_players_own_even_when_the_host_remembers_them() {
    let folders = Folders::new("elsewhere");
    let def = quick(1.0);
    let (mut stand, me) = timed_out_with_unsaved_progress(&folders, &def);
    stand.run(120);
    assert!(entity(&stand, HOST, &me).is_none());
    // c1 played another world meanwhile: its file is not one the host sent.
    write_character(&folders.of("c1"), "c1", 20.0, 40, 5);
    let again = add_client(&mut stand, &folders, "c1", &def, lossy());
    join(&mut stand, again);
    let now = body(&stand, again);
    assert_eq!((bag(&stand, HOST, &now), health(&stand, HOST, &now)), (Some(40), 20.0), "theirs wins, nothing is rolled back");
}

// --- the end of a session -------------------------------------------------------------------

#[test]
fn the_hosts_end_saves_the_world_and_every_character() {
    let folders = Folders::new("end");
    write_character(&folders.of("c1"), "c1", 61.5, 5, 900);
    let def = guests();
    let mut stand = host_stand(&folders, &def);
    let c1 = add_client(&mut stand, &folders, "c1", &def, lossy());
    let c2 = add_client(&mut stand, &folders, "c2", &def, lossy());
    join(&mut stand, c1);
    join(&mut stand, c2);
    let (b1, b2) = (body(&stand, c1), body(&stand, c2));
    stand.run(20);
    // The world has a lever on the boss; progress of every kind happened.
    let boss = entity(&stand, HOST, BOSS).unwrap();
    stand.world_mut(HOST).entity_mut(boss).insert((SaveKey::new("boss"), Lever { pulled: true }));
    give(&mut stand, &b1, 3);
    give(&mut stand, &b2, 4);

    let ended = stand.call(HOST, "net_end", json!({"reason": "the raid is over"}));
    assert!(ended.ok, "{}", ended.result);
    stand.run(120);
    let world = read_file(&folders.of("host"), SaveKind::World, "slot1");
    assert_eq!(world.sections.0["lever"].data, json!({"boss": {"pulled": true}}));
    assert_eq!(world.label, "session ended");
    assert!(folders.of("host").join("character/slot1.save").exists(), "the host's own character");
    for (name, coins) in [("c1", 3), ("c2", 4)] {
        let file = read_file(&folders.of(name), SaveKind::Character, name);
        assert_eq!((file.sections.0["bag"].data["coins"].as_u64(), file.label.as_str()), (Some(coins), "session ended"), "{name}'s file");
    }
    // The journals of all three members say what happened.
    assert!(has_line(&stand, HOST, "save: world/slot1 saved (session ended"), "{:#?}", journal(&stand, HOST));
    for (member, name) in [(c1, "c1"), (c2, "c2")] {
        assert!(has_line(&stand, member, &format!("save: character/{name} saved (session ended")), "{:#?}", journal(&stand, member));
        assert!(
            has_line(&stand, member, "net: session ended by host (the raid is over), world saved: slot1"),
            "{:#?}",
            journal(&stand, member)
        );
    }
}

#[test]
fn a_hosts_quit_saves_the_same_way() {
    let folders = Folders::new("quit");
    let (mut stand, c1, me) = joined(&folders, &NetDef::default());
    stand.run(20);
    give(&mut stand, &me, 8);
    assert_eq!(stand.shut_down(5000), AppExit::Success);
    assert!(folders.of("host").join("world/slot1.save").exists());
    assert_eq!(coins_in(&folders.of("c1"), "c1"), 8);
    assert!(has_line(&stand, c1, "net: session ended by host (the host quit), world saved: slot1"), "{:#?}", journal(&stand, c1));
}

#[test]
fn world_progress_a_guest_made_is_in_the_hosts_world_save_after_the_guest_left() {
    let folders = Folders::new("world");
    let (mut stand, c1, _) = joined(&folders, &guests());
    let boss = entity(&stand, HOST, BOSS).unwrap();
    stand.world_mut(HOST).entity_mut(boss).insert((SaveKey::new("boss"), Lever::default()));
    stand.run(30);
    // The guest pulls the lever (a dev call that runs on the host)...
    let pulled = stand.call(c1, "set", json!({"id": BOSS, "component": "Lever", "path": "pulled", "value": true}));
    assert!(pulled.ok, "{}", pulled.result);
    stand.run(60);
    assert!(stand.world(HOST).get::<Lever>(boss).unwrap().pulled);
    // ... and leaves. The world is the host's: it keeps what the guest did.
    shards::core::run_exit_hooks(stand.world_mut(c1));
    run_until(&mut stand, 90, "the host hears it", |s| has_line(s, HOST, "net: p1 left (quit)"));
    let saved = stand.call(HOST, "save_game", json!({"what": "world", "slot": "after"}));
    assert!(saved.ok, "{}", saved.result);
    let world = read_file(&folders.of("host"), SaveKind::World, "after");
    assert_eq!(world.sections.0["lever"].data, json!({"boss": {"pulled": true}}));
}

// --- who may save and load what -------------------------------------------------------------

#[test]
fn a_client_cannot_save_the_world_or_load_anything_and_a_loaded_world_goes_to_its_clients() {
    let folders = Folders::new("refusals");
    let (mut stand, c1, _) = joined(&folders, &NetDef::default());
    stand.run(20);
    let world = stand.call(c1, "save_game", json!({"what": "world"}));
    assert!(!world.ok && text(&world).contains("the host keeps the world; ask the host (host)"), "{}", text(&world));
    let character = stand.call(c1, "save_game", json!({"what": "character"}));
    assert!(!character.ok && text(&character).contains("your character is saved by the host"), "{}", text(&character));
    let load = stand.call(c1, "load_game", json!({"what": "world"}));
    assert!(!load.ok && text(&load).contains("runs on the host only") && text(&load).contains("the host keeps the world"), "{}", text(&load));
    assert!(!folders.of("c1").join("world").exists(), "no world was written on the client");

    // On the host a world load goes to every client whole.
    assert!(stand.call(HOST, "save_game", json!({"what": "world", "slot": "w"})).ok);
    let loaded = stand.call(HOST, "load_game", json!({"what": "world", "slot": "w"}));
    assert!(loaded.ok, "{}", loaded.result);
    stand.run(90);
    assert!(has_line(&stand, c1, "net: the host's world again (save w loaded)"), "{:#?}", journal(&stand, c1));
    assert_eq!(stand.world(c1).resource::<NetClient>().rebases, 1);
    assert_eq!(desyncs(&stand, c1), 0, "{:#?}", journal(&stand, c1));
    assert!(connected(&stand, c1));
}

#[test]
fn a_hosts_launch_slots_load_before_its_first_tick_as_recorded_calls() {
    let folders = Folders::new("launch");
    // A game of one saves a character and a world to the host's folder.
    {
        let mut single = sandbox::test_app(42);
        single.world_mut().resource_mut::<SaveConfig>().dir = folders.of("host");
        ok(&mut single, "set", json!({"id": PLAYER, "component": "Health", "path": "current", "value": 33.0}));
        ok(&mut single, "save_game", json!({"what": "all", "slot": "camp"}));
    }
    let launch = |c: &mut shards::EngineConfig| {
        c.seed = 42;
        c.world_slot = Some("camp".into());
        c.character_slot = Some("camp".into());
    };
    let mut stand = shards::testing::stand_host(1, &launch, &setup_for(folders.of("host"), NetDef::default()));
    assert_eq!(shards::ai_core::sim::tick(stand.world(HOST)), 0);
    assert_eq!(health(&stand, HOST, PLAYER), 33.0, "the host's own character, loaded");
    // Two `load_game` calls of the launch's own; the recording starts from the state they made, so a
    // replay of the session needs no file.
    let calls: Vec<String> = journal(&stand, HOST).into_iter().filter(|l| l.contains("launch: load_game")).collect();
    assert_eq!(calls.len(), 2, "{:#?}", journal(&stand, HOST));
    assert!(shards::ai_core::replay::save(stand.world(HOST)).start.is_some());
    verify_host(&stand);
    // A guest joins the loaded world.
    let c1 = add_client(&mut stand, &folders, "c1", &NetDef::default(), lossy());
    join(&mut stand, c1);
    stand.run(30);
    assert_eq!(health(&stand, c1, PLAYER), 33.0);
    assert_eq!(desyncs(&stand, c1), 0, "{:#?}", journal(&stand, c1));
}
