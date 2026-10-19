//! The rules in the engine's saves and wiring (C3.1b): the world section `rules` (always written, a null mode
//! is a strict no-op, the load cases), the session count that leaves out a body waiting for a rejoin
//! (`NetLeft`), the guard `npc_engaged`, the UI sources `rules.*` and the net event. Every test writes its own
//! asset and save folders (modes `gentle`, `plain` and `harsh`, class `brute`) and removes them after.

use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};

use shards::action::Health;
use shards::ai_core::{AiId, AiJournal, CallOutcome, JournalKind};
use shards::net::NetLeft;
use shards::npc::{Npc, NpcBrain};
use shards::prelude::*;
use shards::rules::{ActiveRules, RulesCause, RulesChanged};
use shards::save::{SaveConfig, SaveFile, SaveKind, SectionData};
use shards::serde_json::{json, Value};
use shards::testing;

const RULES: &str = r#"{
    "default": (
        modes: ["gentle", "plain", "harsh"],
        start: "plain",
        classes: ["brute"],
        session: {"brute": (max_health: [1.0, 1.5])},
        blocked: "Rules change out of combat",
        host_only: "The host chooses",
    ),
}"#;

const SETS: &str = r#"{
    "gentle": (name: "Gentle", summary: "Half the damage", classes: {"player": (incoming_damage: 0.5)}),
    "plain": (name: "Plain"),
    "harsh": (name: "Harsh", classes: {"player": (incoming_damage: 1.5)}),
}"#;

/// An asset folder and a save folder of the test's own, removed when it is dropped.
struct Folders {
    assets: PathBuf,
    saves: PathBuf,
}

impl Folders {
    /// With the rules (`rules` true) or a game with an empty `content`.
    fn new(tag: &str, rules: bool) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let root = std::env::temp_dir().join(format!("shards_rules_save_{tag}_{}_{n}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let (assets, saves) = (root.join("assets"), root.join("saves"));
        fs::create_dir_all(assets.join("content")).unwrap();
        if rules {
            fs::create_dir_all(assets.join("content/rules")).unwrap();
            fs::write(assets.join("content/rules/game.rules.ron"), RULES).unwrap();
            fs::write(assets.join("content/rules/modes.rule_set.ron"), SETS).unwrap();
        }
        Self { assets, saves }
    }

    fn game(&self) -> App {
        self.game_with(|_| {})
    }

    fn game_with(&self, setup: impl FnOnce(&mut App)) -> App {
        let assets = self.assets.to_string_lossy().into_owned();
        let mut app = testing::test_app(move |config| config.asset_dir = assets, setup);
        app.world_mut().resource_mut::<SaveConfig>().dir = self.saves.clone();
        app
    }

    fn file(&self, kind: SaveKind, slot: &str) -> PathBuf {
        self.saves.join(kind.as_str()).join(format!("{slot}.save"))
    }

    fn read(&self, kind: SaveKind, slot: &str) -> SaveFile {
        SaveFile::decode(&fs::read(self.file(kind, slot)).unwrap()).unwrap()
    }

    /// Rewrites the world file `slot` with `edit` applied to its sections.
    fn edit(&self, slot: &str, edit: impl FnOnce(&mut SaveFile)) {
        let mut file = self.read(SaveKind::World, slot);
        edit(&mut file);
        fs::write(self.file(SaveKind::World, slot), file.encode()).unwrap();
    }
}

impl Drop for Folders {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(self.assets.parent().unwrap());
    }
}

fn text(out: &CallOutcome) -> String {
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

fn ok(app: &mut App, tool: &str, args: Value) -> String {
    text(&testing::ok(app, tool, args))
}

fn refused(app: &mut App, tool: &str, args: Value) -> String {
    let out = testing::call(app, tool, args);
    assert!(!out.ok, "{tool} should fail: {}", text(&out));
    text(&out)
}

fn step(app: &mut App, ticks: u64) {
    ok(app, "step", json!({ "ticks": ticks }));
}

fn rules(app: &App) -> Option<ActiveRules> {
    app.world().get_resource::<ActiveRules>().cloned()
}

fn playing(mode: &str, generation: u32, players: u32) -> Option<ActiveRules> {
    Some(ActiveRules { mode: mode.into(), generation, players })
}

fn lines(app: &App, kind: JournalKind, part: &str) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.filter(|e| e.kind == kind && e.text.contains(part)).map(|e| e.text.clone()).collect()
}

fn go(app: &mut App, state: EngineState) {
    app.world_mut().resource_mut::<NextState<EngineState>>().set(state);
    app.update();
    testing::wait_until_loaded(app, testing::LOAD_TIMEOUT).unwrap();
}

/// The `RulesChanged` events seen, in order.
#[derive(Resource, Default)]
struct Changes(Vec<RulesChanged>);

fn watch(app: &mut App) {
    app.init_resource::<Changes>().add_observer(|event: On<RulesChanged>, mut seen: ResMut<Changes>| seen.0.push((*event).clone()));
}

fn changes(app: &App) -> Vec<(String, String, RulesCause)> {
    app.world().resource::<Changes>().0.iter().map(|c| (c.from.clone(), c.to.clone(), c.cause)).collect()
}

fn body(app: &mut App, player: u8) -> Entity {
    app.world_mut()
        .spawn((Name::new(format!("player {player}")), AiKind::new("hero"), PlayerId(player), Health::new(40.0), Transform::default()))
        .id()
}

#[test]
fn the_registry_lists_the_rules_section_after_the_region() {
    let folders = Folders::new("registry", true);
    let mut app = folders.game();
    let out = testing::ok(&mut app, "save_registry", json!({})).result;
    let names: Vec<(&str, &str, u64)> =
        out["sections"].as_array().unwrap().iter().map(|s| (s["name"].as_str().unwrap(), s["kind"].as_str().unwrap(), s["version"].as_u64().unwrap())).collect();
    assert_eq!(&names[..2], [("region", "world", 2), ("rules", "world", 1)]);
}

#[test]
fn a_game_without_rules_writes_the_null_section_and_loading_it_changes_nothing() {
    let folders = Folders::new("null", false);
    let mut app = folders.game();
    watch(&mut app);
    let player = body(&mut app, 0);
    app.world_mut().get_mut::<Health>(player).unwrap().current = 17.5;
    assert_eq!(rules(&app), None, "no rules content: no rules");
    ok(&mut app, "save_game", json!({"what": "world", "slot": "a"}));
    let file = folders.read(SaveKind::World, "a");
    let section = &file.sections.0["rules"];
    assert_eq!((section.version, &section.data), (1, &json!({"mode": null})));

    // A strict no-op: no resource, no event, no journal line, the body as it was.
    app.world_mut().get_mut::<Health>(player).unwrap().current = 40.0;
    let health = app.world().get::<Health>(player).cloned();
    let out = ok(&mut app, "load_game", json!({"what": "world", "slot": "a"}));
    assert!(out.contains("world/a"), "{out}");
    assert_eq!(rules(&app), None);
    assert_eq!(changes(&app), []);
    assert_eq!(lines(&app, JournalKind::Event, "rules").len() + lines(&app, JournalKind::Warn, "rules").len(), 0);
    assert_eq!(app.world().get::<Health>(player).cloned(), health, "the world section touches no body");

    // A game without rules that is handed a mode: said, never refused.
    folders.edit("a", |file| {
        file.sections.0.insert("rules".into(), SectionData { version: 1, data: json!({"mode": "harsh"}) });
    });
    ok(&mut app, "load_game", json!({"what": "world", "slot": "a"}));
    assert_eq!(rules(&app), None);
    assert_eq!(lines(&app, JournalKind::Warn, "rules:"), ["rules: the save's rule set `harsh` is not in this game's content: playing no rules"]);
    // A game with rules names its mode in the section; a null mode in a file loads as a no-op there too.
    let with = Folders::new("null_neutral", true);
    let mut app = with.game();
    watch(&mut app);
    ok(&mut app, "save_game", json!({"what": "world", "slot": "n"}));
    assert_eq!(with.read(SaveKind::World, "n").sections.0["rules"].data, json!({"mode": "plain"}), "a world with rules in play names it");
    with.edit("n", |file| {
        file.sections.0.insert("rules".into(), SectionData { version: 1, data: json!({"mode": null}) });
    });
    ok(&mut app, "rules_set", json!({"mode": "harsh"}));
    ok(&mut app, "load_game", json!({"what": "world", "slot": "n"}));
    assert_eq!(rules(&app), playing("harsh", 2, 0), "a null mode: the world keeps the mode it has, generation untouched");
    assert_eq!(changes(&app), [("plain".into(), "harsh".into(), RulesCause::Call)], "no event from the load");
}

#[test]
fn a_saved_mode_comes_back_in_this_world_and_in_a_new_one() {
    let folders = Folders::new("load", true);
    let mut app = folders.game();
    watch(&mut app);
    ok(&mut app, "rules_set", json!({"mode": "harsh"}));
    ok(&mut app, "save_game", json!({"what": "world", "slot": "a"}));
    assert_eq!(folders.read(SaveKind::World, "a").sections.0["rules"].data, json!({"mode": "harsh"}));

    // This world, at another mode: the load puts it back, once.
    ok(&mut app, "rules_set", json!({"mode": "gentle"}));
    assert_eq!(rules(&app), playing("gentle", 3, 0));
    ok(&mut app, "load_game", json!({"what": "world", "slot": "a"}));
    assert_eq!(rules(&app), playing("harsh", 4, 0), "the mode, and the generation moved once");
    assert_eq!(lines(&app, JournalKind::Event, "(save)"), ["rules: gentle → harsh (save)"]);
    assert_eq!(changes(&app).last(), Some(&("gentle".into(), "harsh".into(), RulesCause::Load)));

    // The mode in play: nothing changes, no event.
    let seen = changes(&app).len();
    ok(&mut app, "load_game", json!({"what": "world", "slot": "a"}));
    assert_eq!((rules(&app), changes(&app).len()), (playing("harsh", 4, 0), seen), "the same mode: no generation, no event");

    // A new world plays `start`; the load puts the saved mode in.
    go(&mut app, EngineState::MainMenu);
    go(&mut app, EngineState::InGame);
    assert_eq!(rules(&app), playing("plain", 1, 0));
    ok(&mut app, "load_game", json!({"what": "world", "slot": "a"}));
    assert_eq!(rules(&app), playing("harsh", 2, 0));

    // `continue_game` starts the game (at `start`) and then loads.
    go(&mut app, EngineState::MainMenu);
    ok(&mut app, "continue_game", json!({}));
    assert_eq!(rules(&app), playing("harsh", 2, 0), "start, then the world section sets the saved mode");
}

#[test]
fn a_save_without_the_section_or_with_an_unknown_mode_loads_at_the_mode_in_play() {
    let folders = Folders::new("old", true);
    let mut app = folders.game();
    watch(&mut app);
    ok(&mut app, "rules_set", json!({"mode": "harsh"}));
    ok(&mut app, "save_game", json!({"what": "world", "slot": "a"}));

    // A file written before the section existed: `missing`, a no-op. A new world keeps its `start`.
    folders.edit("a", |file| {
        file.sections.0.remove("rules");
    });
    go(&mut app, EngineState::MainMenu);
    ok(&mut app, "continue_game", json!({}));
    assert_eq!(rules(&app), playing("plain", 1, 0), "no section: the world keeps the start mode");
    assert_eq!(lines(&app, JournalKind::Warn, "rules:"), Vec::<String>::new());

    // A mode this game lost: the save is not refused, the mode in play stays, the journal warns.
    folders.edit("a", |file| {
        file.sections.0.insert("rules".into(), SectionData { version: 1, data: json!({"mode": "nightmare"}) });
    });
    let seen = changes(&app).len();
    ok(&mut app, "load_game", json!({"what": "world", "slot": "a"}));
    assert_eq!(rules(&app), playing("plain", 1, 0));
    assert_eq!(changes(&app).len(), seen, "no event");
    assert_eq!(lines(&app, JournalKind::Warn, "rules:"), ["rules: the save's rule set `nightmare` is not in this game's content: playing `plain`"]);

    // A mode that is not a name is damage: refused, nothing changes.
    for bad in ["x".repeat(65), "tab\there".to_owned()] {
        folders.edit("a", |file| {
            file.sections.0.insert("rules".into(), SectionData { version: 1, data: json!({"mode": bad}) });
        });
        let why = refused(&mut app, "load_game", json!({"what": "world", "slot": "a"}));
        assert!(why.contains("the rule set is named in at most 64 bytes without control characters"), "{why}");
        assert_eq!(rules(&app), playing("plain", 1, 0));
    }
}

#[test]
fn a_body_waiting_for_a_rejoin_is_not_in_the_session() {
    let folders = Folders::new("netleft", true);
    let mut app = folders.game();
    body(&mut app, 0);
    let guest = body(&mut app, 1);
    step(&mut app, 1);
    assert_eq!(rules(&app).map(|r| r.players), Some(2));
    app.world_mut().entity_mut(guest).insert(NetLeft { since: 1, until: 600 });
    step(&mut app, 1);
    assert_eq!(rules(&app).map(|r| r.players), Some(1), "the leaver stops counting at the leave, not when the grace ends");
    assert_eq!(lines(&app, JournalKind::Event, "player:"), ["rules: 1 player: brute health ×1"]);
    app.world_mut().entity_mut(guest).remove::<NetLeft>();
    step(&mut app, 1);
    assert_eq!(rules(&app).map(|r| r.players), Some(2), "a rejoin within the grace counts again");
}

/// Keeps every NPC fighting (`chase`, a target) until tick `CALM_AT`, then lets it be: an NPC the test spawns
/// has no behaviour content, so nothing else would keep its state.
const CALM_AT: u64 = 5;

fn fight_until_calm(tick: Res<shards::ai_core::SimTick>, mut brains: Query<&mut NpcBrain>) {
    for mut brain in &mut brains {
        let fighting = tick.0 < CALM_AT;
        brain.active = true;
        brain.state = if fighting { "chase" } else { "idle" }.to_owned();
        brain.target = fighting.then_some(AiId(1));
    }
}

fn npc(app: &mut App, behavior: &str) -> Entity {
    app.world_mut().spawn((Name::new(behavior.to_owned()), Npc::new(behavior, ""), Transform::default())).id()
}

#[test]
fn the_rules_wait_for_an_npc_that_fights() {
    let folders = Folders::new("guard", true);
    let mut app = folders.game_with(|app| {
        app.add_systems(FixedUpdate, fight_until_calm.in_set(SimSet::Cleanup));
    });
    watch(&mut app);
    let skeleton = npc(&mut app, "skeleton");
    let raider = npc(&mut app, "raider");
    let (raider_id, skeleton_id) = (*app.world().get::<AiId>(raider).unwrap(), *app.world().get::<AiId>(skeleton).unwrap());
    assert!(skeleton_id < raider_id, "spawned first, numbered first");
    let changeable = app.world().resource::<shards::ai_core::AiConditions>().0["rules_changeable"];
    assert!(changeable(app.world()), "nobody fights yet");
    step(&mut app, 1);
    assert!(!changeable(app.world()), "an active NPC with a target in `chase` holds the rules");

    // The reason names the first such NPC in `AiId` order, and the guard.
    let reason = format!("{skeleton_id} `skeleton` is fighting (`chase`, target e1) [npc_engaged]");
    assert_eq!(
        refused(&mut app, "rules_set", json!({"mode": "harsh"})),
        format!("the rules change only out of combat: {reason}. Wait with `step --until named:rules_changeable`, or pass `force: true` from a dev session")
    );
    let view = testing::ok(&mut app, "rules", json!({})).result;
    assert_eq!(view["change"], format!("refused now: {reason}"));
    assert_eq!(rules(&app), playing("plain", 1, 0));
    assert_eq!(changes(&app), []);

    // A UI source says it for the player: the content text, not the agent's.
    let blocked = shards::ui::read_source(app.world(), "rules.blocked").unwrap();
    assert_eq!(blocked, Some(shards::ui::UiValue::Text("Rules change out of combat".into())));

    // `force` (a dev session) skips it, and is recorded.
    assert!(ok(&mut app, "rules_set", json!({"mode": "gentle", "force": true})).starts_with("rules: plain → gentle"));
    ok(&mut app, "rules_set", json!({"mode": "plain", "force": true}));

    // The NPC calms: the condition holds and the change goes through.
    let out = ok(&mut app, "step", json!({"ticks": 30, "until": [{"named": "rules_changeable"}]}));
    assert!(out.contains("rules_changeable"), "{out}");
    assert!(changeable(app.world()));
    assert!(app.world().resource::<shards::ai_core::SimTick>().0 <= CALM_AT + 1, "it stopped when the NPC calmed, not at the end");
    assert!(ok(&mut app, "rules_set", json!({"mode": "harsh"})).starts_with("rules: plain → harsh"));
    assert_eq!(shards::ui::read_source(app.world(), "rules.blocked").unwrap(), None, "nothing blocks now");
}

#[test]
fn the_ui_sources_read_the_rules() {
    let folders = Folders::new("ui", true);
    let mut app = folders.game();
    let source = |app: &App, name: &str| shards::ui::read_source(app.world(), name).unwrap();
    let text = |s: &str| Some(shards::ui::UiValue::Text(s.to_owned()));
    assert_eq!(source(&app, "rules.mode"), text("plain"));
    assert_eq!(source(&app, "rules.name"), text("Plain"));
    assert_eq!(source(&app, "rules.name:gentle"), text("Gentle"));
    assert_eq!(source(&app, "rules.summary"), None, "no summary: none");
    assert_eq!(source(&app, "rules.summary:gentle"), text("Half the damage"));
    assert_eq!(source(&app, "rules.name:nope"), None, "no such rule set");
    assert_eq!(source(&app, "rules.blocked"), None);
    app.world_mut().insert_resource(NetRole::Client);
    assert_eq!(source(&app, "rules.blocked"), text("The host chooses"), "a client reads the host's text");

    // A game without rules: nothing.
    let none = Folders::new("ui_none", false);
    let app = none.game();
    for name in ["rules.mode", "rules.name", "rules.summary", "rules.blocked"] {
        assert_eq!(source(&app, name), None, "{name}");
    }
}

/// A registration check only: that the event is sent by a host and triggered on a guest is the machinery of
/// `net_event` (its own tests); a stand with the rules comes with C3.6.
#[test]
fn rules_changed_travels_to_a_guest() {
    let folders = Folders::new("net", true);
    let app = folders.game();
    let events = app.world().resource::<shards::net::NetEvents>().names().collect::<Vec<_>>();
    assert!(events.contains(&<RulesChanged as bevy::reflect::TypePath>::type_path()), "the host sends it, a guest triggers it");
}

