//! Co-op saves (C8): characters travel with their players, the host keeps the world
//! (`shards::coop`). Stands of 150 ms and 2 % loss, every member with a save folder of its
//! own, removed when the test ends.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use shards::action::Health;
use shards::ai_core::{sim, snapshot, AiIndex, AiJournal, NoArgs, Scope, ToolResult};
use shards::net::host::{CHARACTER_PART_BYTES, MAX_CHARACTER_BYTES, MAX_CHARACTER_PARTS, MAX_INLINE_CHARACTER, MAX_JOINING_MESSAGE};
use shards::net::msg::{Hello, Msg, PROTO};
use shards::net::{LinkConditions, NetClient, NetDef, NetDefaults, NetHooks, NetLeft, NetSeat, NetState, Stand};
use shards::prelude::*;
use shards::save::{
    save_now, Damage, GameInfo, LaunchLoaded, SaveConfig, SaveFile, SaveKey, SaveKind, SaveSlots, SaveWhat, SectionData, SectionSet, Store,
    Which,
};
use shards::serde_json::{json, Value};

use super::net::*;
use crate::common::*;

// --- what the sandbox saves for these tests -------------------------------------------------

/// A character section of the tests: coins on the player's body. Reflected, so `set` changes it
/// as a recorded call and the replay's hash covers it.
#[derive(Component, Reflect, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
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

/// A game's reward: it saves the caller's character at once (`save_now`, from a tool that
/// runs on the host for a guest).
fn reward(In(call): In<Call<NoArgs>>, world: &mut World) -> ToolResult<String> {
    let body = call.caller.require_subject()?;
    let reports = save_now(world, SaveWhat::Character(Some(body)), "reward")?;
    Ok(reports.iter().map(ToString::to_string).collect::<Vec<_>>().join("; "))
}

/// The sections, the camp and the reward: what every member of these tests (and the host's replay) registers.
fn sections(app: &mut App) {
    app.register_type::<Bag>()
        .register_type::<Lever>()
        // `bag` is at version 2: version 1 called the coins `gold`.
        .save_component::<Bag>(SaveKind::Character, "bag", 2)
        .add_save_migration("bag", 1, |old| Ok(if old.is_null() { old } else { json!({"coins": old["gold"]}) }))
        .save_component::<Lever>(SaveKind::World, "lever", 1)
        .autosave_on::<Camp>(SaveWhat::Character(None))
        .add_ai_tool("reward", Scope::Dev, reward);
}

fn setup_for(dir: PathBuf, def: NetDef) -> impl Fn(&mut App) {
    move |app: &mut App| {
        app.add_plugins(sandbox::SandboxPlugin);
        app.insert_resource(NetDefaults(def.clone()));
        sections(app);
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

/// A character file in `dir` from these sections: saved at host tick `tick`.
fn write_sections(dir: &Path, slot: &str, tick: u64, sections: Vec<(&str, u32, Value)>) {
    let sections = sections.into_iter().map(|(name, version, data)| (name.to_owned(), SectionData { version, data })).collect::<BTreeMap<_, _>>();
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

fn vitals(health: f32) -> Value {
    json!({"health": {"current": health, "max": 100.0}, "stamina": null, "charges": []})
}

/// A character file in `dir`: this much health, this many coins, saved at host tick `tick`.
fn write_character(dir: &Path, slot: &str, health: f32, coins: u32, tick: u64) {
    write_sections(dir, slot, tick, vec![("vitals", 1, vitals(health)), ("bag", 2, json!({"coins": coins}))]);
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
    host_stand_with(folders, def, |_| {})
}

/// [`host_stand`] whose game also does what `tweak` says.
fn host_stand_with(folders: &Folders, def: &NetDef, tweak: impl Fn(&mut App)) -> Stand {
    let base = setup_for(folders.of("host"), def.clone());
    shards::testing::stand_host(2, &configure, &|app: &mut App| {
        base(app);
        tweak(app);
    })
}

/// Client `name`, playing the character in its slot `name` of its own folder.
fn add_client(stand: &mut Stand, folders: &Folders, name: &str, def: &NetDef, links: LinkConditions) -> usize {
    add_client_in(stand, folders.of(name), name, def, links, |c| c.character_slot = Some(name.to_owned()))
}

/// Client `name` saving into `dir`, started as `tweak` says.
fn add_client_in(stand: &mut Stand, dir: PathBuf, name: &str, def: &NetDef, links: LinkConditions, tweak: impl Fn(&mut shards::EngineConfig)) -> usize {
    let configure = |c: &mut shards::EngineConfig| {
        c.seed = 42;
        tweak(c);
    };
    shards::testing::stand_client(stand, name, links, &configure, &setup_for(dir, def.clone()))
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

/// [`joined`] with a file of 61.5 health and no coins, for a test that gives the body progress.
fn joined_with_file(folders: &Folders, def: &NetDef) -> (Stand, usize, String) {
    write_character(&folders.of("c1"), "c1", 61.5, 0, 900);
    joined(folders, def)
}

/// The progress of a player on the host, as a recorded call.
fn give(stand: &mut Stand, body: &str, coins: u32) {
    let set = stand.call(HOST, "set", json!({"id": body, "component": "Bag", "path": "coins", "value": coins}));
    assert!(set.ok, "{}", set.result);
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

/// The host's replay played in single player, with the game's sections registered there too (the
/// body's `Bag` is part of what the replay's hash covers): it must come out the same.
fn verify(stand: &Stand, def: &NetDef) {
    let recording = shards::ai_core::replay::save(stand.world(HOST));
    let mut config = shards::testing::config();
    config.seed = recording.seed;
    let mut app = sandbox::build_app(&config);
    app.insert_resource(NetDefaults(def.clone()));
    sections(&mut app);
    app.finish();
    app.cleanup();
    let report = shards::testing::verify(&mut app, recording).unwrap();
    assert!(report.identical(), "the host's replay does not play the same in single player: {report}");
}

/// A journal entry of `member` that a forged line could hide in: none has a raw newline.
fn no_forged_lines(stand: &Stand, member: usize) {
    assert!(journal(stand, member).iter().all(|l| !l.contains('\n')), "{:#?}", journal(stand, member));
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
    assert_eq!(bag(&stand, c1, &me), Some(5), "and its bag");
    assert!(has_line(&stand, c1, "save: bringing character/c1 (tick 900) to the host"), "{:#?}", journal(&stand, c1));
    assert!(has_line(&stand, HOST, "save: p1 character applied (vitals, bag)"), "{:#?}", journal(&stand, HOST));
    // The character is in the recorded join (the token is not), so a replay builds the same body.
    let joins = call_args(&stand, "player_join");
    assert_eq!(joins.len(), 1, "{joins:?}");
    assert!(joins[0]["character"].is_string() && joins[0].get("token").is_none(), "{:?}", joins[0]);
    assert_eq!(desyncs(&stand, c1), 0, "{:#?}", journal(&stand, c1));
    verify(&stand, &NetDef::default());
}

#[test]
fn a_file_of_an_older_version_is_migrated_when_it_joins() {
    let folders = Folders::new("migrate");
    // `bag` was saved at version 1, when the coins were called `gold`.
    write_sections(&folders.of("c1"), "c1", 900, vec![("vitals", 1, vitals(61.5)), ("bag", 1, json!({"gold": 5}))]);
    let (stand, _, me) = joined(&folders, &NetDef::default());
    assert_eq!(bag(&stand, HOST, &me), Some(5));
    assert!(has_line(&stand, HOST, "save: p1 character applied (vitals, bag)"), "{:#?}", journal(&stand, HOST));
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
fn a_file_with_no_good_copy_joins_as_a_new_player_and_is_left_alone() {
    let folders = Folders::new("allbad");
    let dir = folders.of("c1");
    fs::create_dir_all(dir.join("character")).unwrap();
    fs::write(dir.join("character/c1.save"), b"junk").unwrap();
    let (stand, c1, me) = joined(&folders, &NetDef::default());
    assert!(has_line(&stand, c1, "save: character/c1 could not be read, joining as a new player"), "{:#?}", journal(&stand, c1));
    assert_eq!(bag(&stand, HOST, &me), None);
    assert!(call_args(&stand, "player_join")[0].get("character").is_none());
}

#[test]
fn a_client_ignores_the_world_it_is_told_to_load() {
    let folders = Folders::new("clientworld");
    let mut stand = host_stand(&folders, &NetDef::default());
    let c1 = add_client_in(&mut stand, folders.of("c1"), "c1", &NetDef::default(), lossy(), |c| c.world_slot = Some("camp".into()));
    join(&mut stand, c1);
    assert!(has_line(&stand, c1, "save: --world camp ignored: the host keeps the world"), "{:#?}", journal(&stand, c1));
}

// --- what a peer sends is not trusted --------------------------------------------------------

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

    // Sections of another kind, or that this game does not save, are skipped before anything applies, and
    // what a peer names never forges a journal line.
    let foreign = json!({
        "region": {"version": 1, "data": {"active": null}},
        "evil\n0 [error] forged": {"version": 1, "data": {}},
        "vitals": {"version": 1, "data": {"health": {"current": 7.0, "max": 100.0}, "stamina": null, "charges": []}}
    });
    let sent = stand.call(HOST, "player_join", json!({"player": 4, "name": "eve", "character": foreign}));
    assert!(sent.ok, "{}", sent.result);
    let line = journal(&stand, HOST).into_iter().find(|l| l.contains("p4 brought sections this game does not save")).expect("the skipped sections are said");
    assert!(line.contains("evil\\n0 [error] forged") && line.contains("region"), "{line}");
    no_forged_lines(&stand, HOST);
    let p4 = shards::core::player_body(stand.world(HOST), 4).unwrap().to_string();
    assert_eq!(health(&stand, HOST, &p4), 7.0, "what is ours was applied");
}

#[test]
fn numbers_that_no_body_has_join_with_defaults_and_leave_nothing_behind() {
    let folders = Folders::new("hostile");
    // c1's file says its health tops out at -5; c2's that it stands at infinity (1e39 does not fit an f32).
    write_sections(&folders.of("c1"), "c1", 900, vec![("vitals", 1, json!({"health": {"current": 1.0, "max": -5.0}, "stamina": null, "charges": []})), ("bag", 2, json!({"coins": 3}))]);
    write_sections(&folders.of("c2"), "c2", 900, vec![("position", 1, json!({"feet": [1e39, 0.0, 0.0], "rotation": [0.0, 0.0, 0.0, 1.0], "region": null}))]);
    let def = NetDef::default();
    let mut stand = host_stand(&folders, &def);
    let c1 = add_client(&mut stand, &folders, "c1", &def, lossy());
    let c2 = add_client(&mut stand, &folders, "c2", &def, lossy());
    join(&mut stand, c1);
    join(&mut stand, c2);
    let (b1, b2) = (body(&stand, c1), body(&stand, c2));
    // Both are in, with what their prefab gives them, and the host says why their files were not applied.
    assert!(has_line(&stand, HOST, "save: p1 character could not be applied (section vitals: health 1 of -5 is not a pool"), "{:#?}", journal(&stand, HOST));
    assert!(has_line(&stand, HOST, "save: p2 character could not be applied (section position: feet"), "{:#?}", journal(&stand, HOST));
    let max = stand.world(HOST).get::<Health>(entity(&stand, HOST, &b1).unwrap()).unwrap().max;
    assert!(max > 0.0, "the ceiling is the prefab's, not -5: {max}");
    assert!(position(&stand, HOST, &b2).is_finite(), "{}", position(&stand, HOST, &b2));
    // The game goes on: no invariant halts it, and no slot is held by a body nobody plays.
    stand.run(60);
    assert!(connected(&stand, c1) && connected(&stand, c2));
    assert_eq!(shards::core::player_body(stand.world(HOST), 1).map(|b| b.to_string()), Some(b1));
}

#[test]
fn a_join_that_fails_after_its_body_was_made_takes_the_body_back() {
    let folders = Folders::new("orphan");
    write_character(&folders.of("c1"), "c1", 61.5, 5, 900);
    // The game's own section code panics on what it is given (a bug the engine's checks do not know of).
    let mut stand = host_stand_with(&folders, &NetDef::default(), |app| {
        app.world_mut().resource_mut::<NetHooks>().on_join_character = |_, _, _| panic!("a section of this game panics");
    });
    let c1 = add_client(&mut stand, &folders, "c1", &NetDef::default(), lossy());
    run_until(&mut stand, 400, "the join is refused", |s| matches!(net_state(s, c1), NetState::Ended(_)));
    assert!(matches!(net_state(&stand, c1), NetState::Ended(why) if why.contains("cannot spawn your body")), "{:?}", net_state(&stand, c1));
    // The body the failed join made is not live: it waits as a leaver's does, instead of holding the slot for good.
    let orphan = shards::core::player_body(stand.world(HOST), 1).and_then(|id| entity(&stand, HOST, &id.to_string())).expect("the body is there");
    assert!(stand.world(HOST).get::<NetLeft>(orphan).is_some(), "a live body nobody plays holds the slot");
    // It keeps the seat the join gave it, as a leaver's body does: the session's, waiting.
    assert!(stand.world(HOST).get::<NetSeat>(orphan).is_some());
}

#[test]
fn a_rejoin_that_fails_leaves_the_body_waiting_for_the_player_again() {
    let folders = Folders::new("orphan_back");
    write_character(&folders.of("c1"), "c1", 61.5, 5, 900);
    let (mut stand, c1, me) = joined(&folders, &NetDef::default());
    stand.run(20);
    shards::core::run_exit_hooks(stand.world_mut(c1));
    run_until(&mut stand, 90, "the host hears it", |s| has_line(s, HOST, "net: p1 left (quit)"));
    stand.run(30);
    let waiting = entity(&stand, HOST, &me).expect("the body waits");
    assert!(stand.world(HOST).get::<NetLeft>(waiting).is_some());
    // c1 comes back with a file the host never saw, and the game's section code panics on it: the join
    // takes the body off its wait before it fails, and must put it back.
    write_character(&folders.of("c1"), "c1", 20.0, 40, 5);
    stand.world_mut(HOST).resource_mut::<NetHooks>().on_join_character = |_, _, _| panic!("a section of this game panics");
    let again = add_client(&mut stand, &folders, "c1", &NetDef::default(), lossy());
    run_until(&mut stand, 400, "the join is refused", |s| matches!(net_state(s, again), NetState::Ended(_)));
    assert!(matches!(net_state(&stand, again), NetState::Ended(why) if why.contains("cannot spawn your body")), "{:?}", net_state(&stand, again));
    assert!(stand.world(HOST).get::<NetLeft>(waiting).is_some(), "a body nobody plays is live and holds the slot");
    assert!(stand.world(HOST).get::<NetSeat>(waiting).is_some());
}

/// Character sections whose text, as the host measures it (the JSON of the string it travels as),
/// grows one byte with each byte of `pad`.
fn padded(pad: usize) -> Vec<(&'static str, u32, Value)> {
    vec![("vitals", 1, vitals(61.5)), ("bag", 2, json!({"coins": 5, "pad": letters(pad)}))]
}

/// The bytes of [`padded`]`(pad)` as a character travels.
fn measured(pad: usize) -> usize {
    let set = SectionSet(padded(pad).into_iter().map(|(n, v, d)| (n.to_owned(), SectionData { version: v, data: d })).collect());
    serde_json::to_string(&shards::coop::character_value(&set)).unwrap().len()
}

/// Letters that hardly compress: a character of this size is that many bytes on the wire too.
fn letters(len: usize) -> String {
    let mut x = 0x9E37_79B9_7F4A_7C15u64;
    (0..len)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            char::from(b'a' + (x % 26) as u8)
        })
        .collect()
}

#[test]
fn the_largest_hello_and_the_largest_part_of_a_character_fit_under_the_pre_play_cap() {
    // The largest character a hello carries itself, with every other field as long as the host takes it.
    let hello = Msg::Hello(Hello {
        proto: PROTO,
        engine: "0.2.0-with-a-long-suffix".repeat(3),
        game: "g".repeat(200),
        content_hash: u64::MAX,
        net_rules_hash: u64::MAX,
        name: "n".repeat(32),
        token: "t".repeat(64),
        character: Some(shards::serde_json::from_str(&format!("\"{}\"", "x".repeat(MAX_INLINE_CHARACTER - 2))).unwrap()),
        character_parts: 0,
        character_bytes: 0,
        tick_hz: 60.0,
    });
    let size = shards::net::msg::encode(&hello).len();
    assert!(size < MAX_JOINING_MESSAGE, "a Hello with the largest inline character is {size} bytes, over the {MAX_JOINING_MESSAGE} the host reads before play");
    // A part of the text that escapes the most: every byte a quote, twice its size once escaped.
    let part = Msg::CharacterPart { index: MAX_CHARACTER_PARTS, text: "\"".repeat(CHARACTER_PART_BYTES) };
    let size = shards::net::msg::encode(&part).len();
    assert!(size < MAX_JOINING_MESSAGE, "the largest part is {size} bytes, over the {MAX_JOINING_MESSAGE} the host reads before play");
    // The parts of the largest character are as many as a host takes.
    let largest = Value::String(letters(MAX_CHARACTER_BYTES - 2));
    let parts = shards::net::msg::split_character(&largest, CHARACTER_PART_BYTES);
    assert!(parts.len() as u32 <= MAX_CHARACTER_PARTS && parts.iter().all(|p| p.len() <= CHARACTER_PART_BYTES), "{} parts", parts.len());
    assert_eq!(parts.concat(), serde_json::to_string(&largest).unwrap());
}

#[test]
fn a_character_too_big_for_the_hello_comes_in_parts_over_a_lossy_link_and_one_byte_over_the_cap_is_refused() {
    let folders = Folders::new("largest");
    let pad = MAX_CHARACTER_BYTES - measured(0);
    assert_eq!(measured(pad), MAX_CHARACTER_BYTES);
    let def = NetDef::default();
    let mut stand = host_stand(&folders, &def);
    let big = "c".repeat(32);
    write_sections(&folders.of(&big), &big, 900, padded(pad));
    write_sections(&folders.of("c2"), "c2", 900, padded(pad + 1));
    // c1's character is the largest a host takes: 33 parts after its hello, over 150 ms and 2 % loss.
    let (c1, c2) = (add_client(&mut stand, &folders, &big, &def, lossy()), add_client(&mut stand, &folders, "c2", &def, lossy()));
    // A part is a message of its own, each a round trip of 300 ms at least.
    run_until(&mut stand, 2400, "both are answered", |s| connected(s, c1) && matches!(net_state(s, c2), NetState::Ended(_)));
    let me = body(&stand, c1);
    assert_eq!(bag(&stand, HOST, &me), Some(5), "the largest character is applied");
    let why = match net_state(&stand, c2) {
        NetState::Ended(why) => why,
        other => panic!("{other:?}"),
    };
    assert!(why.contains("the character is 1025 KB, over the 1024 KB a host takes"), "one byte more: {why}");
    // Refused at its hello: it held no seat.
    assert!(!has_line(&stand, HOST, "(c2) accepted"), "{:#?}", journal(&stand, HOST));
}

// --- progress goes back to its player -------------------------------------------------------

#[test]
fn an_autosave_event_on_the_host_updates_the_players_file() {
    let folders = Folders::new("autosave");
    write_character(&folders.of("c1"), "c1", 61.5, 5, 900);
    let (mut stand, c1, me) = joined(&folders, &NetDef::default());
    stand.run(20);
    // c1 gains progress on the host (a recorded call: a game system would), the camp is rested at.
    give(&mut stand, &me, 11);
    stand.world_mut(HOST).trigger(Camp);
    stand.run(60);
    let file = read_file(&folders.of("c1"), SaveKind::Character, "c1");
    assert_eq!(file.sections.0["bag"].data["coins"], 11, "c1's own file has the progress");
    assert_eq!(file.label, "camp");
    assert!(file.tick >= 20, "the host's tick: {}", file.tick);
    let said = journal(&stand, c1).into_iter().find(|l| l.contains("save: character/c1 saved (camp, ")).expect("c1 says it saved");
    assert!(said.contains(", tick "), "{said}");
    assert!(has_line(&stand, HOST, "save: character/p1 sent to p1's machine (camp"), "{:#?}", journal(&stand, HOST));
    // The host's own character is written on the host, and only there.
    assert!(folders.of("host").join("character/slot1.save").exists());
    assert_eq!(read_file(&folders.of("host"), SaveKind::Character, "slot1").sections.0.get("bag").map(|b| b.data.clone()), Some(Value::Null));
    assert!(!folders.of("host").join("character/c1.save").exists());
    verify(&stand, &NetDef::default());
}

#[test]
fn a_save_of_a_remote_players_body_goes_to_their_machine_not_the_hosts_slot() {
    let folders = Folders::new("remote");
    let (mut stand, c1, me) = joined_with_file(&folders, &guests());
    give(&mut stand, &me, 3);
    // The host's agent plays player 1 (a session bound to them): its save is theirs.
    shards::ai_core::configure_session(stand.world_mut(HOST), "dev", &shards::ai_core::SessionChange { player: Some(1), ..Default::default() }).unwrap();
    let saved = stand.call(HOST, "save_game", json!({"what": "character"}));
    assert!(saved.ok, "{}", saved.result);
    assert!(text(&saved).contains("character/p1 sent to p1's machine (save_game"), "{}", text(&saved));
    stand.run(60);
    assert_eq!(coins_in(&folders.of("c1"), "c1"), 3);
    assert!(!folders.of("host").join("character/slot1.save").exists(), "the host's own slot was not written for p1's character");
    // So is a save the game decides on for their body.
    give(&mut stand, &me, 4);
    let entity_of_p1 = entity(&stand, HOST, &me).unwrap();
    let reports = save_now(stand.world_mut(HOST), SaveWhat::Character(Some(entity_of_p1)), "reward").unwrap();
    assert!(reports[0].target.is_some(), "{reports:?}");
    stand.run(60);
    assert_eq!(coins_in(&folders.of("c1"), "c1"), 4);
    assert!(!folders.of("host").join("character/slot1.save").exists());
    assert!(connected(&stand, c1));
}

#[test]
fn a_save_a_guests_tool_makes_goes_to_the_guest_not_into_the_hosts_slot() {
    let folders = Folders::new("reward");
    let (mut stand, c1, me) = joined_with_file(&folders, &guests());
    give(&mut stand, &me, 7);
    // The game's `reward` tool runs on the host for the guest, while the host's session is out of its world.
    let rewarded = stand.call(c1, "reward", json!({}));
    assert!(rewarded.ok, "{}", rewarded.result);
    stand.run(90);
    let file = read_file(&folders.of("c1"), SaveKind::Character, "c1");
    assert_eq!((file.sections.0["bag"].data["coins"].as_u64(), file.label.as_str()), (Some(7), "reward"), "{:#?}", journal(&stand, HOST));
    assert!(!folders.of("host").join("character/slot1.save").exists(), "p1's character was not written into the host's own slot");
}

#[test]
fn guests_are_sent_their_characters_even_when_the_hosts_own_cannot_be_written() {
    let folders = Folders::new("hostfails");
    let (mut stand, _, me) = joined_with_file(&folders, &NetDef::default());
    // The host's own character slot holds a save of a newer game: this build never overwrites it.
    write_sections(&folders.of("host"), "slot1", 5, vec![("vitals", 9, json!({}))]);
    stand.run(20);
    give(&mut stand, &me, 8);
    stand.world_mut(HOST).trigger(Camp);
    stand.run(60);
    assert!(has_line(&stand, HOST, "so this build does not overwrite it"), "the host's own save failed: {:#?}", journal(&stand, HOST));
    assert_eq!(coins_in(&folders.of("c1"), "c1"), 8, "c1 got its character all the same");
}

#[test]
fn a_player_who_leaves_has_the_latest_in_their_file_and_comes_back_to_it() {
    let folders = Folders::new("leave");
    let (mut stand, c1, me) = joined_with_file(&folders, &NetDef::default());
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
    verify(&stand, &NetDef::default());
}

/// A def whose host forgets a silent client after a second (and a body after `grace` seconds).
fn quick(grace: f32) -> NetDef {
    NetDef { timeout_secs: 1.0, rejoin_grace_secs: grace, ..NetDef::default() }
}

/// c1 joined with 5 coins in its file and has 9 on the host; the link died, and the host's last
/// word (its leave-time capture) never reached c1. With `camp`, c1's file had 7 before that.
fn timed_out_with_unsaved_progress(folders: &Folders, def: &NetDef, camp: bool) -> (Stand, String) {
    write_character(&folders.of("c1"), "c1", 61.5, 5, 900);
    let (mut stand, c1, me) = joined(folders, def);
    stand.run(20);
    if camp {
        give(&mut stand, &me, 7);
        stand.world_mut(HOST).trigger(Camp);
        stand.run(60);
        assert_eq!(coins_in(&folders.of("c1"), "c1"), 7);
    }
    give(&mut stand, &me, 9);
    stand.set_link(c1, "loss=1".parse().unwrap()).unwrap();
    stand.run(100);
    assert!(has_line(&stand, HOST, "net: p1 left (timeout)"), "{:#?}", journal(&stand, HOST));
    assert_eq!(coins_in(&folders.of("c1"), "c1"), if camp { 7 } else { 5 }, "the final save never arrived");
    (stand, me)
}

#[test]
fn a_rejoin_within_the_grace_keeps_the_body_and_sends_the_file_what_it_missed() {
    let folders = Folders::new("grace");
    let def = quick(120.0);
    let (mut stand, me) = timed_out_with_unsaved_progress(&folders, &def, true);
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
    let (mut stand, me) = timed_out_with_unsaved_progress(&folders, &def, true);
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
    verify(&stand, &def);
}

#[test]
fn the_file_a_player_joined_with_is_the_hosts_own_copy_when_nothing_was_saved_since() {
    // c1 plays on and the link dies before the host sent it any save: its file is still the one it joined with.
    for grace in [120.0, 1.0] {
        let folders = Folders::new("baseline");
        let def = quick(grace);
        let (mut stand, me) = timed_out_with_unsaved_progress(&folders, &def, false);
        let standing = grace > 1.0;
        if !standing {
            stand.run(120);
            assert!(entity(&stand, HOST, &me).is_none(), "the body is gone after the grace");
        }
        let again = add_client(&mut stand, &folders, "c1", &def, lossy());
        join(&mut stand, again);
        let now = body(&stand, again);
        assert_eq!(bag(&stand, HOST, &now), Some(9), "the progress the host holds is not rolled back to the file's 5 (grace {grace})");
        stand.run(90);
        assert_eq!(coins_in(&folders.of("c1"), "c1"), 9, "and c1's file catches up (grace {grace})");
        assert_eq!(call_args(&stand, "player_join").last().unwrap().get("character").is_some(), !standing, "(grace {grace})");
    }
}

#[test]
fn a_file_the_host_never_sent_is_the_players_own_even_when_the_host_remembers_them() {
    let folders = Folders::new("elsewhere");
    let def = quick(1.0);
    let (mut stand, me) = timed_out_with_unsaved_progress(&folders, &def, true);
    stand.run(120);
    assert!(entity(&stand, HOST, &me).is_none());
    // c1 played another world meanwhile: its file is not one the host sent.
    write_character(&folders.of("c1"), "c1", 20.0, 40, 5);
    let again = add_client(&mut stand, &folders, "c1", &def, lossy());
    join(&mut stand, again);
    let now = body(&stand, again);
    assert_eq!((bag(&stand, HOST, &now), health(&stand, HOST, &now)), (Some(40), 20.0), "theirs wins, nothing is rolled back");
}

#[test]
fn a_player_whose_slot_was_taken_meanwhile_comes_back_to_what_the_host_kept() {
    let folders = Folders::new("retired");
    let def = quick(1.0);
    let (mut stand, _) = timed_out_with_unsaved_progress(&folders, &def, true);
    stand.run(120);
    // Another player takes c1's slot (the lowest free one), then c1 comes back with its token.
    let c2 = add_client(&mut stand, &folders, "c2", &def, lossy());
    join(&mut stand, c2);
    assert!(matches!(net_state(&stand, c2), NetState::Connected { player: 1, .. }), "{:?}", net_state(&stand, c2));
    let again = add_client(&mut stand, &folders, "c1", &def, lossy());
    join(&mut stand, again);
    assert!(matches!(net_state(&stand, again), NetState::Connected { player: 2, .. }), "a slot of its own: {:?}", net_state(&stand, again));
    let now = body(&stand, again);
    assert_eq!(bag(&stand, HOST, &now), Some(9), "its token is still its own: the host's copy, not the stale file's 7");
}

#[test]
fn a_rebuilt_world_gives_a_player_the_newest_character_the_host_has() {
    let folders = Folders::new("rebase");
    let def = NetDef::default();
    let mut stand = host_stand(&folders, &def);
    // The world as it is before c1 joined: a snapshot of it comes back later.
    let before = snapshot::capture(stand.world(HOST)).unwrap();
    write_character(&folders.of("c1"), "c1", 61.5, 5, 900);
    let c1 = add_client(&mut stand, &folders, "c1", &def, lossy());
    join(&mut stand, c1);
    let me = body(&stand, c1);
    stand.run(20);
    give(&mut stand, &me, 7);
    stand.world_mut(HOST).trigger(Camp);
    stand.run(60);
    assert_eq!(coins_in(&folders.of("c1"), "c1"), 7);
    // The host goes back to a world without c1's body: it is joined again, from what the host holds.
    snapshot::restore(stand.world_mut(HOST), &before, false).unwrap();
    stand.run(90);
    let now = body(&stand, c1);
    assert_eq!(bag(&stand, HOST, &now), Some(7), "not the 5 of the file c1 first joined with: {:#?}", journal(&stand, HOST));
    assert_eq!(coins_in(&folders.of("c1"), "c1"), 7);
}

// --- the client writes what the host sends -------------------------------------------------

/// What the host sends c1 as a `CharacterSave`, handed to c1's hook as the session does.
fn tell_client(stand: &mut Stand, member: usize, reason: &str, sections: Value) {
    let world = stand.world_mut(member);
    let hook = shards::net::hooks::hooks(world).on_character_save;
    hook(world, 99, reason, &sections, None);
}

#[test]
fn a_client_writes_only_a_character_it_can_read_and_keeps_its_file_otherwise() {
    let folders = Folders::new("clientsave");
    write_character(&folders.of("c1"), "c1", 61.5, 5, 900);
    let (mut stand, c1, _) = joined(&folders, &NetDef::default());
    let set = |sections: Vec<(&str, u32, Value)>| shards::coop::character_value(&SectionSet(sections.into_iter().map(|(n, v, d)| (n.to_owned(), SectionData { version: v, data: d })).collect()));
    let unchanged = |stand: &Stand| assert_eq!(coins_in(&folders.of("c1"), "c1"), 5, "c1's file is as it was: {:#?}", journal(stand, c1));
    let said = |stand: &Stand, text: &str| assert!(has_line(stand, c1, text), "{text}: {:#?}", journal(stand, c1));

    tell_client(&mut stand, c1, "camp", json!("{not json"));
    said(&stand, "save: the host's character for you could not be read (bad JSON");
    tell_client(&mut stand, c1, "camp", set(vec![("vitals", 9, json!({}))]));
    said(&stand, "is not one this game reads");
    tell_client(&mut stand, c1, "camp", set(vec![("x", 1, json!({}))]));
    said(&stand, "has none of the sections this game saves");
    tell_client(&mut stand, c1, "camp", set(vec![]));
    tell_client(&mut stand, c1, "camp", set(vec![("bag", 2, json!({"coins": 1, "pad": "x".repeat(MAX_CHARACTER_BYTES)}))]));
    said(&stand, "is over 1024 KB, which no host takes");
    unchanged(&stand);

    // A good one goes in: its foreign sections are left out, and the reason is no forged line.
    tell_client(&mut stand, c1, "x\n0 [error] forged", set(vec![("bag", 2, json!({"coins": 6})), ("evil\nname", 1, json!({}))]));
    assert_eq!(coins_in(&folders.of("c1"), "c1"), 6);
    let file = read_file(&folders.of("c1"), SaveKind::Character, "c1");
    assert!(!file.sections.0.keys().any(|k| k.contains("evil")), "{:?}", file.sections.0.keys().collect::<Vec<_>>());
    assert!(has_line(&stand, c1, "sections this game does not save: evil\\nname; skipped"), "{:#?}", journal(&stand, c1));
    no_forged_lines(&stand, c1);
}

#[test]
fn a_client_does_not_write_a_character_from_its_own_autosave() {
    let folders = Folders::new("clientauto");
    write_character(&folders.of("c1"), "c1", 61.5, 5, 900);
    let (mut stand, c1, _) = joined(&folders, &NetDef::default());
    stand.run(20);
    // A camp event that reached the client (a presentation event) asks for a save there.
    stand.world_mut(c1).trigger(Camp);
    stand.run(10);
    assert!(has_line(&stand, c1, "save: character autosave (camp) skipped on a client"), "{:#?}", journal(&stand, c1));
    assert_eq!(coins_in(&folders.of("c1"), "c1"), 5, "the file is the host's to send");
    assert!(!folders.of("c1").join("world").exists());
}

// --- two games in one folder ---------------------------------------------------------------

#[test]
fn a_host_and_a_client_started_from_one_folder_keep_their_own_characters() {
    let folders = Folders::new("shared");
    let shared = folders.of("shared");
    let def = NetDef::default();
    let mut stand = shards::testing::stand_host(1, &configure, &setup_for(shared.clone(), def.clone()));
    // c1 names no character: it plays the slot named after it, not the host's `slot1`.
    write_character(&shared, "c1", 61.5, 0, 900);
    let c1 = add_client_in(&mut stand, shared.clone(), "c1", &def, lossy(), |_| {});
    join(&mut stand, c1);
    assert!(has_line(&stand, c1, "save: bringing character/c1 (tick 900)"), "{:#?}", journal(&stand, c1));
    let me = body(&stand, c1);
    stand.run(20);
    // Write the host's slot as a single game would have, so that a client reading `slot1` would take it.
    give(&mut stand, &me, 3);
    stand.world_mut(HOST).trigger(Camp);
    stand.run(60);
    assert!(stand.call(HOST, "net_end", json!({})).ok);
    stand.run(120);
    let host = read_file(&shared, SaveKind::Character, "slot1");
    assert_eq!(host.label, "session ended", "the host's own file");
    assert_eq!(host.sections.0["bag"].data, Value::Null, "holds the host's character, not p1's");
    assert_eq!(coins_in(&shared, "c1"), 3, "c1's file holds c1's");
}

// --- the end of a session -------------------------------------------------------------------

#[test]
fn the_hosts_end_saves_the_world_and_every_character() {
    let folders = Folders::new("end");
    write_character(&folders.of("c1"), "c1", 61.5, 5, 900);
    write_character(&folders.of("c2"), "c2", 70.0, 6, 900);
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
    // Nothing twice: the world once, the host's character once, a character to each player once (with `SessionEnd`,
    // not again by the fan-out of a save the game decides on), and a player's leave sends none of its own.
    let count = |member: usize, needle: &str| journal(&stand, member).iter().filter(|l| l.contains(needle)).count();
    assert_eq!(count(HOST, "save: world/slot1 saved (session ended"), 1);
    assert_eq!(count(HOST, "save: character/slot1 saved (session ended"), 1);
    assert_eq!(count(HOST, "sent to"), 0, "{:#?}", journal(&stand, HOST));
    assert_eq!(count(HOST, "net: session ended ("), 1);
    for (member, name) in [(c1, "c1"), (c2, "c2")] {
        assert_eq!(count(member, &format!("save: character/{name} saved (")), 1, "{:#?}", journal(&stand, member));
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
    let (mut stand, c1, me) = joined_with_file(&folders, &NetDef::default());
    stand.run(20);
    give(&mut stand, &me, 8);
    assert_eq!(stand.shut_down(5000), AppExit::Success);
    assert!(folders.of("host").join("world/slot1.save").exists());
    assert_eq!(coins_in(&folders.of("c1"), "c1"), 8);
    assert!(has_line(&stand, c1, "net: session ended by host (the host quit), world saved: slot1"), "{:#?}", journal(&stand, c1));
}

#[test]
fn the_worlds_slot_is_told_though_the_hosts_own_character_could_not_be_saved() {
    let folders = Folders::new("worldslot");
    let (mut stand, c1, _) = joined_with_file(&folders, &NetDef::default());
    write_sections(&folders.of("host"), "slot1", 5, vec![("vitals", 9, json!({}))]);
    stand.run(20);
    assert!(stand.call(HOST, "net_end", json!({})).ok);
    stand.run(90);
    assert!(folders.of("host").join("world/slot1.save").exists(), "the world was written");
    assert!(has_line(&stand, c1, "world saved: slot1"), "{:#?}", journal(&stand, c1));
}

#[test]
fn a_host_that_quit_before_its_slots_were_loaded_does_not_save_over_them() {
    let folders = Folders::new("unloaded");
    {
        let mut single = sandbox::test_app(42);
        single.world_mut().resource_mut::<SaveConfig>().dir = folders.of("host");
        ok(&mut single, "set", json!({"id": PLAYER, "component": "Health", "path": "current", "value": 33.0}));
        ok(&mut single, "save_game", json!({"what": "all", "slot": "camp"}));
    }
    let before = (fs::read(folders.of("host").join("world/camp.save")).unwrap(), fs::read(folders.of("host").join("character/camp.save")).unwrap());
    let launch = |c: &mut shards::EngineConfig| {
        c.seed = 42;
        c.world_slot = Some("camp".into());
        c.character_slot = Some("camp".into());
    };
    let mut stand = shards::testing::stand_host(1, &launch, &setup_for(folders.of("host"), NetDef::default()));
    // The launch's load did not run (the game was stopped while it loaded): the slots hold the only copy of the game.
    stand.world_mut(HOST).resource_mut::<LaunchLoaded>().0 = false;
    assert_eq!(stand.shut_down(1000), AppExit::Success);
    assert_eq!(before, (fs::read(folders.of("host").join("world/camp.save")).unwrap(), fs::read(folders.of("host").join("character/camp.save")).unwrap()));
    assert!(!folders.of("host").join("world/camp.save.bak1").exists(), "nothing was rotated into a backup");
    assert!(has_line(&stand, HOST, "nothing was saved"), "{:#?}", journal(&stand, HOST));
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
    // ... and leaves. The world is the host's: its save at the end of the session has what the guest did.
    shards::core::run_exit_hooks(stand.world_mut(c1));
    run_until(&mut stand, 90, "the host hears it", |s| has_line(s, HOST, "net: p1 left (quit)"));
    assert!(stand.call(HOST, "net_end", json!({})).ok);
    stand.run(30);
    let world = read_file(&folders.of("host"), SaveKind::World, "slot1");
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
fn a_client_saves_its_character_by_asking_the_host_for_the_body_it_plays() {
    let folders = Folders::new("asked");
    let (mut stand, c1, me) = joined_with_file(&folders, &NetDef::default());
    // Progress on the host: the body there is the one that is saved.
    give(&mut stand, &me, 9);
    stand.run(20);
    let asked = stand.call(c1, "save_game", json!({"what": "character", "slot": "mine"}));
    assert!(asked.ok, "{}", text(&asked));
    assert!(text(&asked).contains("ok: character/mine asked of the host host: written here when it comes (save_game, tick "), "{}", text(&asked));
    run_until(&mut stand, 90, "the file is written", |_| folders.of("c1").join("character/mine.save").exists());
    let file = read_file(&folders.of("c1"), SaveKind::Character, "mine");
    assert_eq!(file.sections.0["bag"].data["coins"], 9, "what the host's body has");
    assert_eq!(file.label, "save_game");
    assert!(has_line(&stand, c1, "save: character/mine saved (save_game, "), "{:#?}", journal(&stand, c1));
    assert!(has_line(&stand, HOST, "net: p1 asked for its character: sent ("), "{:#?}", journal(&stand, HOST));
    // The slot it named is its slot now: what the host sends next goes there too.
    assert_eq!(stand.world(c1).resource::<SaveSlots>().character, "mine");
    give(&mut stand, &me, 12);
    stand.world_mut(HOST).trigger(Camp);
    stand.run(60);
    assert_eq!(coins_in(&folders.of("c1"), "mine"), 12);
    // At most once a second: asked twice at once, the second is told to wait (and nothing more).
    assert!(stand.call(c1, "save_game", json!({"what": "character"})).ok);
    assert!(stand.call(c1, "save_game", json!({"what": "character"})).ok);
    assert!(stand.call(c1, "save_game", json!({"what": "character"})).ok);
    stand.run(30);
    let waits = journal(&stand, c1).into_iter().filter(|l| l.contains("save: the host did not send your character: it was sent less than a second ago; ask again in ")).count();
    assert_eq!(waits, 1, "{:#?}", journal(&stand, c1));
    // Asking saves nothing on the host and changes nothing of the game.
    assert!(!folders.of("host").join("character/mine.save").exists());
    verify(&stand, &NetDef::default());
}

#[test]
fn each_ask_for_a_character_is_written_into_the_slot_it_named() {
    let folders = Folders::new("asked_two");
    let (mut stand, c1, me) = joined_with_file(&folders, &NetDef::default());
    give(&mut stand, &me, 9);
    stand.run(20);
    // Two asks before the first answer comes: the second slot is the current one by then (and the
    // host tells that ask to wait).
    assert!(stand.call(c1, "save_game", json!({"what": "character", "slot": "first"})).ok);
    assert!(stand.call(c1, "save_game", json!({"what": "character", "slot": "second"})).ok);
    assert_eq!(stand.world(c1).resource::<SaveSlots>().character, "second");
    run_until(&mut stand, 90, "the first is written", |_| folders.of("c1").join("character/first.save").exists());
    stand.run(30);
    assert_eq!(coins_in(&folders.of("c1"), "first"), 9);
    assert!(!folders.of("c1").join("character/second.save").exists(), "the first's answer is not the second's: {:#?}", journal(&stand, c1));
    let told = "save: the host did not send your character: it was sent less than a second ago; ask again in ";
    let waits: Vec<String> = journal(&stand, c1).into_iter().filter(|l| l.contains(told)).collect();
    assert!(waits.len() == 1 && waits[0].ends_with("(character/second is as it was)"), "{waits:#?}");
}

#[test]
fn a_client_that_is_not_in_a_session_cannot_ask_for_its_character() {
    let folders = Folders::new("asked_late");
    let (mut stand, c1, _) = joined_with_file(&folders, &NetDef::default());
    assert!(stand.call(HOST, "net_end", json!({})).ok);
    run_until(&mut stand, 90, "the session ends", |s| matches!(net_state(s, c1), NetState::Ended(_)));
    let asked = stand.call(c1, "save_game", json!({"what": "character"}));
    assert!(!asked.ok && text(&asked).contains("not in a session any more (session ended by host"), "{}", text(&asked));
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
    assert_eq!(sim::tick(stand.world(HOST)), 0);
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

#[test]
fn a_world_another_generator_made_loads_on_the_host_from_its_manifest_and_its_clients_get_it() {
    let folders = Folders::new("manifest");
    // A game of one whose wilds are at another generator version saves a world in them.
    let saved = {
        let updated = crate::saves::Updated::new("coop");
        let mut single = updated.game_in(&folders.of("host"));
        ok(&mut single, "regen", json!({"region": "wilds", "seed": 42}));
        ok(&mut single, "save_game", json!({"what": "world", "slot": "camp"}));
        single.world().resource::<ActiveRegion>().hash
    };
    let launch = |c: &mut shards::EngineConfig| {
        c.seed = 42;
        c.world_slot = Some("camp".into());
    };
    let mut stand = shards::testing::stand_host(1, &launch, &setup_for(folders.of("host"), NetDef::default()));
    let hash = |stand: &Stand, member: usize| stand.world(member).get_resource::<ActiveRegion>().map(|r| r.hash);
    assert_eq!(hash(&stand, HOST), Some(saved), "the host built the saved world, not what its generator makes");
    assert!(has_line(&stand, HOST, "region wilds seed 42 v2: built from the save's manifest (the generator is v1 now)"), "{:#?}", journal(&stand, HOST));
    // The recording starts from the loaded state: its replay builds that world from the manifest too.
    verify_host(&stand);

    // A guest joins the loaded world: it takes the region from the host, it does not generate it.
    let c1 = add_client(&mut stand, &folders, "c1", &NetDef::default(), lossy());
    join(&mut stand, c1);
    stand.run(60);
    assert_eq!(hash(&stand, c1), Some(saved));
    assert!(!has_line(&stand, c1, "region generated"), "{:#?}", journal(&stand, c1));
    assert_eq!(desyncs(&stand, c1), 0, "{:#?}", journal(&stand, c1));

    // The host's next save keeps that world.
    assert!(stand.call(HOST, "save_game", json!({"what": "world", "slot": "camp"})).ok);
    let world = read_file(&folders.of("host"), SaveKind::World, "camp");
    assert_eq!(world.sections.0["region"].data["active"]["manifest_hash"], json!(format!("{saved:016x}")));
}

// --- over a real socket ---------------------------------------------------------------------

/// Save folders inside the game's folder (as `--save-dir` must be), removed when the test ends.
struct Gone(Vec<String>);

impl Gone {
    fn new(tag: &str) -> Self {
        let pid = std::process::id();
        let dirs = vec![format!("saves/test-coop-{tag}-host-{pid}"), format!("saves/test-coop-{tag}-client-{pid}")];
        for dir in &dirs {
            let _ = fs::remove_dir_all(dir);
        }
        Self(dirs)
    }
}

impl Drop for Gone {
    fn drop(&mut self) {
        for dir in &self.0 {
            let _ = fs::remove_dir_all(dir);
        }
    }
}

/// Raises a flag when it is dropped, so that a thread that runs until it is raised stops when the
/// test panics too (a scope waits for its threads).
struct Raise<'a>(&'a AtomicBool);

impl Drop for Raise<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

/// A host and a client over UDP loopback, joined: the client plays a file of 61.5 health, and what
/// the host holds of it is 12.5 now.
fn udp_session(gone: &Gone) -> (App, App) {
    // The client is called `udp`, so it plays the slot `udp` (not the host's `slot1`).
    write_character(Path::new(&gone.0[1]), "udp", 61.5, 5, 900);
    let (started, deadline) = (Instant::now(), Duration::from_secs(20));
    let mut host = udp_member(shards::core::NetLaunch::Host { port: 0 }, "host", Some(&gone.0[0]));
    shards::testing::wait_until_loaded(&mut host, deadline).unwrap();
    let address = host.world().resource::<shards::net::NetEndpoint>().transport.local_addr().expect("a bound socket");
    let join = shards::core::NetLaunch::Join { addr: format!("127.0.0.1:{}", address.rsplit(':').next().unwrap()) };
    let mut client = udp_member(join, "udp", Some(&gone.0[1]));
    assert!(play_over_udp(&mut host, &mut client, 30, started, deadline).is_some(), "not connected within {deadline:?}");
    let NetState::Connected { body: Some(body), .. } = client.world().resource::<NetState>().clone() else { panic!("not playing") };
    // What the player did since their file was written, on the host.
    let on_host = host.world().resource::<AiIndex>().get(AiId(body)).expect("the body on the host");
    host.world_mut().get_mut::<Health>(on_host).unwrap().current = 12.5;
    assert_eq!(read_file(Path::new(&gone.0[1]), SaveKind::Character, "udp").sections.0["vitals"].data["health"]["current"], 61.5);
    (host, client)
}

/// A client that quits over UDP waits, up to 300 ms, for the character its host captures as it
/// leaves: its file holds the progress made since the last save. (The host plays on in a thread
/// of its own, as a real host is another process.)
#[test]
fn over_udp_a_quitting_client_waits_for_the_character_its_host_sends_as_it_leaves() {
    let gone = Gone::new("quit");
    let (mut host, mut client) = udp_session(&gone);
    let stop = AtomicBool::new(false);
    let host_world = host.world_mut();
    std::thread::scope(|scope| {
        let _raise = Raise(&stop);
        scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) {
                sim::run_tick(host_world).unwrap();
                std::thread::sleep(Duration::from_millis(4));
            }
        });
        shards::core::run_exit_hooks(client.world_mut());
    });
    let file = read_file(Path::new(&gone.0[1]), SaveKind::Character, "udp");
    assert_eq!(file.sections.0["vitals"].data["health"]["current"], 12.5, "the last word of the host is in the file");
    assert!(file.label.starts_with("quit"), "{}", file.label);
}

/// A host that quits over UDP gives its client its character and the end of the session, and
/// waits for them to be acknowledged before it goes (its 500 ms): the client's file and journal
/// have both. (The client plays on in a thread of its own, as a real client is another process.)
#[test]
fn over_udp_the_hosts_exit_reaches_the_client_with_its_character_and_the_world_slot() {
    let gone = Gone::new("exit");
    let (mut host, mut client) = udp_session(&gone);
    // A game started with `--exit-with-host`: a session the host ended is exit code 0.
    client.world_mut().resource_mut::<shards::EngineConfig>().exit_with_host = true;
    let stop = AtomicBool::new(false);
    let client_world = client.world_mut();
    std::thread::scope(|scope| {
        let _raise = Raise(&stop);
        scope.spawn(|| {
            while !stop.load(Ordering::Relaxed) && !matches!(client_world.resource::<NetState>(), NetState::Ended(_)) {
                let _ = sim::run_tick(client_world);
                std::thread::sleep(Duration::from_millis(4));
            }
        });
        // Back from the host's exit: its 500 ms ended with the client's acknowledgements (or ran out).
        shards::core::run_exit_hooks(host.world_mut());
        std::thread::sleep(Duration::from_millis(150));
    });
    let file = read_file(Path::new(&gone.0[1]), SaveKind::Character, "udp");
    assert_eq!(file.sections.0["vitals"].data["health"]["current"], 12.5);
    assert_eq!(file.label, "session ended");
    assert!(read_file(Path::new(&gone.0[0]), SaveKind::World, "slot1").label == "session ended");
    let said: Vec<String> = client.world().resource::<AiJournal>().after(0).0.map(ToString::to_string).collect();
    assert!(said.iter().any(|l| l.contains("net: session ended by host (the host quit), world saved: slot1")), "{said:#?}");
    assert_eq!(client.should_exit(), Some(AppExit::Success), "the host ended the session: exit code 0");
}
