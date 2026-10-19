//! # shards_rules — rule sets (G10, C3)
//!
//! A world plays one **rule set** at a time (a mode: `gentle`, `plain`, `harsh`), and it says how the
//! engine's numbers change for each **class** of body: the damage a body takes and deals, its maximum
//! health and poise, the stamina its actions cost, its guards' perfect window, the max of its named
//! charges; and, for every director, the attack tokens and the rest after an attack. A **session table**
//! scales a class by the number of players in the session.
//!
//! - **Content.** `*.rule_set.ron` ([`RuleSetDef`], one definition per mode) and `*.rules.ron`
//!   ([`RulesDef`], the one definition `default`: the modes in order, where a new world starts, the
//!   classes, the session table, the texts of a refused change). A game without `rules/default` has no
//!   rules: nothing is inserted, every read returns the engine's numbers, replays and snapshots keep their
//!   bytes.
//! - **The state.** [`ActiveRules`] (the mode in play, a generation that counts changes, the session size),
//!   a snapshot resource, replicated; [`RuleClass`] (authored: the class of a body; `player` for a body
//!   with `PlayerId`); [`RuleScaled`] (what the rules did to a body).
//! - **When it changes.** A new world plays `start` ([`start_rules`], in `RulesSystems::Start`); the main
//!   menu ends it ([`end_rules`]); a content change adds to the generation, or falls back to `start` when
//!   the mode is gone ([`rules_content_applied`]); a snapshot puts back what it holds (the prepare `rules`);
//!   `rules_set` changes it, out of combat ([`RulesGuards`]) and only on the host. [`RulesChanged`] says so.
//! - **Each tick** (`RulesSystems::Apply`, in `SimSet::Input` before the engine's other sets there, while
//!   `ActiveRules` exists), [`count_session`] counts the players seated in the session ([`SessionFilter`]).
//!   A game's own `SimSet::Input` system that reads the rules goes `.after(RulesSystems::Apply)`: nothing
//!   orders it against the set otherwise.
//! - **Agents.** `rules` (what the rules in play say), `rules_set` (change them), the condition
//!   `rules_changeable`, `ai/status` → `rules`, the engine rule `rules.mode_known`, the dev entry "Next
//!   rule set".

pub mod def;
pub mod guard;
pub mod read;
pub mod state;
pub mod tools;

use ai_core::journal::{self, JournalKind};
use ai_core::{AiAppExt, DevCommand, NetRoute, SimSet, Snapshot};
use bevy::prelude::*;
use serde_json::Value;
use shards_content::{ContentAppExt, ContentDb, PrefabDef};
use shards_core::{CoreSystems, EngineAppExt, EngineState, NetRole};

pub use def::{ClassRules, CostRule, RuleSetDef, RulesDef, SessionRow, TokenRule, NEUTRAL_CLASS};
pub use guard::{can_change, RulesAppExt, RulesGuard, RulesGuards};
pub use read::{
    active_set, charge_rules, class_of, class_rules, health_factor, incoming, outgoing, poise_factor, rest_factor, row_factor, rules_def,
    session_factor, session_size, token_limit, Column, RulesView,
};
pub use state::{ActiveRules, Owed, RuleClass, RuleKey, RuleScaled, Scaled, SessionFilter};
pub use tools::{RulesArgs, RulesSetArgs};

/// Version of the engine crates (they share one).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// The class of a body with `PlayerId` and no [`RuleClass`].
pub const PLAYER_CLASS: &str = "player";
/// The id of the one [`RulesDef`].
pub const RULES_ID: &str = "default";
/// Why a co-op guest cannot call `rules_set`.
pub const HOST_DECIDES: &str = "the host chooses the rules: call it on the host";

/// The rules' systems, for ordering against them.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RulesSystems {
    /// In `SimSet::Input`, before `CoreSystems::FollowOrders` and the world's zones (so before every engine
    /// set of `SimSet::Input`), while [`ActiveRules`] exists: the session count ([`count_session`]), then the
    /// bodies brought to the rules. A game's `SimSet::Input` system that reads the rules orders itself
    /// `.after(RulesSystems::Apply)`.
    Apply,
    /// In `OnEnter(EngineState::InGame)`: [`start_rules`]. A game whose own `OnEnter(InGame)` systems must
    /// see the new world's rules goes `.after(RulesSystems::Start)`.
    Start,
}

/// The rules in play changed.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct RulesChanged {
    /// The mode before (empty: none) and after.
    pub from: String,
    pub to: String,
    pub cause: RulesCause,
}

/// What changed the rules.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq)]
pub enum RulesCause {
    /// `rules_set`.
    Call,
    /// A save's world section.
    Load,
    /// Content lost the mode in play: it fell back to `start`.
    Content,
}

/// The rules module: the content kinds `rule_set` and `rules`, [`ActiveRules`] and its snapshot, replay and
/// content wiring, the session count, the tools, the condition, the status, the engine rule and the dev
/// entry. Needs the engine core and content first; `shards::EnginePlugins` adds it after them.
pub struct RulesPlugin;

impl Plugin for RulesPlugin {
    fn build(&self, app: &mut App) {
        assert!(app.is_plugin_added::<ai_core::AiCorePlugin>(), "RulesPlugin needs AiCorePlugin added first (shards::EnginePlugins adds both)");
        app.register_type::<ActiveRules>()
            .register_type::<RuleClass>()
            .register_type::<RuleScaled>()
            .register_type::<RuleKey>()
            .register_type::<Scaled>()
            .register_type::<Owed>()
            .register_type::<RulesChanged>()
            .register_type::<RulesCause>()
            .add_content::<RuleSetDef>()
            .add_content::<RulesDef>()
            .add_content_check::<PrefabDef>("rule_class", def::check_prefab)
            .add_snapshot_resource::<ActiveRules>()
            .add_optional_replay_check("rules", replay_check)
            .add_snapshot_prepare_after("rules", "content", &[], prepare)
            .on_content_applied(rules_content_applied)
            .add_systems(OnEnter(EngineState::InGame), start_rules.in_set(RulesSystems::Start))
            .add_systems(OnExit(EngineState::InGame), end_rules)
            .configure_sets(
                FixedUpdate,
                RulesSystems::Apply.in_set(SimSet::Input).before(CoreSystems::FollowOrders).run_if(resource_exists::<ActiveRules>),
            )
            .add_systems(FixedUpdate, count_session.in_set(RulesSystems::Apply))
            .init_resource::<RulesGuards>()
            .insert_resource(SessionFilter::default())
            .add_engine_module("rules", VERSION)
            .add_engine_invariant("rules.mode_known", mode_known)
            .add_ai_condition("rules_changeable", tools::changeable)
            .add_ai_status("rules", status)
            .add_dev_entry("rules", "rules_next", "Next rule set", DevCommand::Live(dev_next))
            .journal_event::<RulesChanged>(journal_line);
        tools::register(app.world_mut());
        app.net_route("rules_set", NetRoute::HostOnly(HOST_DECIDES));
    }
}

/// The dev entry "Next rule set": `rules_set cycle=1 force=true`, or why it does nothing here (a game
/// without rules, a co-op client).
fn dev_next(world: &World, _: &ai_core::Caller) -> Result<String, String> {
    if world.get_resource::<ContentDb>().and_then(read::peek_rules_def).is_none() {
        return Err(tools::NO_RULES.to_owned());
    }
    if !decides(world) {
        return Err(HOST_DECIDES.to_owned());
    }
    Ok("rules_set cycle=1 force=true".to_owned())
}

/// Whether this machine decides the rules: not a co-op client.
pub(crate) fn decides(world: &World) -> bool {
    world.get_resource::<NetRole>() != Some(&NetRole::Client)
}

/// Whether this machine hosts a co-op session: there the rules end neutral (a removal does not travel to
/// its clients).
fn hosts(world: &World) -> bool {
    world.get_resource::<NetRole>() == Some(&NetRole::Host)
}

/// Ends the rules on this machine. A co-op host sets them neutral (the mode cleared, the generation kept).
/// Alone they are removed, and so is a client's copy: it is the host's, not this machine's, it goes with the
/// session, and nothing of it may stay to block the start of a later world of this machine's own.
fn end(world: &mut World) {
    if !hosts(world) {
        world.remove_resource::<ActiveRules>();
    } else if let Some(mut rules) = world.get_resource_mut::<ActiveRules>() {
        rules.mode.clear();
    }
}

/// Puts the rules' `start` in play, as a new world does: inserts [`ActiveRules`] or overwrites a neutral one
/// (`generation` + 1, no players counted yet). Returns the mode, or `None` when it changed nothing (no
/// `rules/default`, or rules already in play).
fn begin(world: &mut World) -> Option<String> {
    let start = world.get_resource::<ContentDb>().and_then(rules_def)?.start.clone();
    let previous = match world.get_resource::<ActiveRules>() {
        Some(rules) if !rules.is_neutral() => return None,
        Some(rules) => rules.generation,
        None => 0,
    };
    world.insert_resource(ActiveRules { mode: start.clone(), generation: previous.wrapping_add(1), players: 0 });
    Some(start)
}

/// `OnEnter(EngineState::InGame)`: a new world plays the rules' `start` (never the previous world's mode).
/// Nothing on a co-op client (the host's resource replicates) or without `rules/default`. No event: a new
/// world shows no banner. A region trip does not run it (it hides `OnEnter`), so the mode crosses regions.
pub fn start_rules(world: &mut World) {
    if !decides(world) {
        return;
    }
    if let Some(start) = begin(world) {
        journal::emit(world, JournalKind::Event, format!("rules: a new world plays `{start}`"));
    }
}

/// `OnExit(EngineState::InGame)` (back to the main menu): a co-op host sets the resource neutral; alone it is
/// removed, and so is a client's copy of the host's. The next world starts at `start`.
pub fn end_rules(world: &mut World) {
    end(world);
}

/// After every content change a game applies while it runs (inside the call, so a replay runs it at the
/// same tick); nothing on a client. Rules in play add 1 to `generation` (every body is brought to the new
/// numbers at the next tick); a mode content no longer has falls back to `start` (neutral without
/// `rules/default`), with a warning and [`RulesChanged`] `Content`; rules content that appears in a world
/// without rules in play starts them, as [`start_rules`] does. A game without rules: nothing.
pub fn rules_content_applied(world: &mut World) -> Option<String> {
    if !decides(world) {
        return None;
    }
    const ANSWER: &str = "rules: content changed; bodies are brought to it at the next tick";
    let playing = world.get_resource::<ActiveRules>().filter(|r| !r.is_neutral()).map(|r| r.mode.clone());
    let Some(mode) = playing else {
        // No rules in play: content that now has them starts them, in a world.
        let in_game = world.get_resource::<State<EngineState>>().is_some_and(|s| *s.get() == EngineState::InGame);
        if !in_game {
            return None;
        }
        let start = begin(world)?;
        journal::emit(world, JournalKind::Event, format!("rules: content has rules now: the world plays `{start}`"));
        return Some(ANSWER.to_owned());
    };
    let (def_start, known) = {
        let db = world.get_resource::<ContentDb>();
        let def = db.and_then(rules_def);
        let known = def.is_some_and(|d| d.modes.contains(&mode)) && db.is_some_and(|db| db.get::<RuleSetDef>(&mode).is_some());
        (def.map(|d| d.start.clone()), known)
    };
    let to = if known { mode.clone() } else { def_start.unwrap_or_default() };
    {
        let mut rules = world.resource_mut::<ActiveRules>();
        rules.generation = rules.generation.wrapping_add(1);
        rules.mode.clone_from(&to);
    }
    if known {
        return Some(ANSWER.to_owned());
    }
    let playing = if to.is_empty() { "no rules now (content has no `rules/default`)".to_owned() } else { format!("playing `{to}`") };
    journal::emit(world, JournalKind::Warn, format!("rules: the rule set `{mode}` is gone from content: {playing}"));
    world.trigger(RulesChanged { from: mode, to, cause: RulesCause::Content });
    Some(ANSWER.to_owned())
}

/// The snapshot prepare `rules` (after `content`): the world's [`ActiveRules`] becomes what the snapshot holds.
/// A snapshot with one puts it in; one without removes the world's (a co-op host sets it neutral: a removal
/// does not travel). A refused load runs it again with the world's own snapshot, which puts back what was.
pub fn prepare(world: &mut World, snapshot: &Snapshot) -> Result<Option<String>, String> {
    let saved = match snapshot.resources.get(<ActiveRules as TypePath>::type_path()) {
        Some(value) => Some(serde_json::from_value::<ActiveRules>(value.clone()).map_err(|e| format!("its rules do not read: {e}"))?),
        None => None,
    };
    let current = world.get_resource::<ActiveRules>().cloned();
    match saved {
        Some(saved) if current.as_ref() == Some(&saved) => Ok(None),
        Some(saved) => {
            let note = format!("`{}` from the snapshot", tools::shown(&saved.mode));
            world.insert_resource(saved);
            Ok(Some(note))
        }
        None => match current {
            None => Ok(None),
            Some(rules) if rules.is_neutral() && hosts(world) => Ok(None),
            Some(_) => {
                end(world);
                Ok(Some("none, as the snapshot was taken without them".to_owned()))
            }
        },
    }
}

/// FNV-1a, 64 bits: the replay check's hash (stable across builds and platforms).
fn fnv(bytes: &[u8], mut hash: u64) -> u64 {
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}

/// The optional replay check `rules`: the mode, the generation and the session size; `None` while there is
/// no resource or it is neutral (a game without rules hashes as one without the module).
pub fn replay_check(world: &World) -> Option<u64> {
    let rules = world.get_resource::<ActiveRules>().filter(|r| !r.is_neutral())?;
    let mut hash = fnv(rules.mode.as_bytes(), 0xcbf2_9ce4_8422_2325);
    hash = fnv(&[0xff], hash);
    hash = fnv(&rules.generation.to_le_bytes(), hash);
    Some(fnv(&rules.players.to_le_bytes(), hash))
}

/// `RulesSystems::Apply`, first: the session size is the number of distinct players seated in the session
/// ([`session_size`]); when it moves it is written into [`ActiveRules::players`], and, if the rules have a
/// session table, journaled with the table's factors at the new size (`rules: 2 players: brute health ×1.5`).
/// The query state is the system's own, kept between ticks (building one walks every archetype).
pub fn count_session(world: &mut World, bodies: &mut QueryState<(Entity, &'static shards_core::PlayerId)>) {
    let n = read::count_seated(world, bodies);
    match world.get_resource_mut::<ActiveRules>() {
        Some(mut rules) if rules.players != n => rules.players = n,
        _ => return,
    }
    let line = world.get_resource::<ContentDb>().and_then(rules_def).and_then(|def| session_line(def, n));
    if let Some(line) = line {
        journal::emit(world, JournalKind::Event, line);
    }
}

/// `rules: 2 players: brute health ×1.5`: the session table's factors at `players`; `None` when the table
/// gives none (no row, or only rows without a column, which validation refuses).
fn session_line(def: &RulesDef, players: u32) -> Option<String> {
    let factors: Vec<String> = def
        .session
        .keys()
        .flat_map(|class| {
            Column::ALL.into_iter().filter_map(move |column| {
                let row = column.of(def, class);
                (!row.is_empty()).then(|| format!("{class} {} ×{}", column.label(), row_factor(row, players)))
            })
        })
        .collect();
    if factors.is_empty() {
        return None;
    }
    let noun = if players == 1 { "player" } else { "players" };
    Some(format!("rules: {players} {noun}: {}", factors.join(", ")))
}

/// The engine rule `rules.mode_known`: while [`ActiveRules`] exists, its mode is empty or a rule set in
/// `rules/default` `modes` that content has.
pub fn mode_known(world: &World) -> Option<String> {
    let rules = world.get_resource::<ActiveRules>().filter(|r| !r.is_neutral())?;
    let db = world.get_resource::<ContentDb>()?;
    let known = read::peek_rules_def(db).is_some_and(|d| d.modes.contains(&rules.mode)) && db.peek::<RuleSetDef>(&rules.mode).is_some();
    (!known).then(|| format!("ActiveRules plays `{}`, which content does not have", rules.mode))
}

/// `ai/status` → `rules`: `gentle · session 2 · generation 4`, or `none`.
fn status(world: &World) -> Value {
    match world.get_resource::<ActiveRules>().filter(|r| !r.is_neutral()) {
        Some(rules) => Value::String(format!("{} · session {} · generation {}", rules.mode, rules.players, rules.generation)),
        None => Value::String("none".to_owned()),
    }
}

/// `rules: plain → gentle (call)`, `(save)`; a fall back from content is journaled as a warning instead.
fn journal_line(event: &RulesChanged, _: &World) -> Option<String> {
    let why = match event.cause {
        RulesCause::Call => "call",
        RulesCause::Load => "save",
        RulesCause::Content => return None,
    };
    Some(format!("rules: {} → {} ({why})", tools::shown(&event.from), tools::shown(&event.to)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_replay_check_is_absent_without_rules_in_play() {
        let mut world = World::new();
        assert_eq!(replay_check(&world), None);
        world.insert_resource(ActiveRules::default());
        assert_eq!(replay_check(&world), None, "neutral: absent, as without the module");
        world.insert_resource(ActiveRules { mode: "plain".into(), generation: 1, players: 1 });
        let plain = replay_check(&world).unwrap();
        world.resource_mut::<ActiveRules>().players = 2;
        assert_ne!(replay_check(&world), Some(plain), "the session size is hashed");
        world.resource_mut::<ActiveRules>().players = 1;
        world.resource_mut::<ActiveRules>().generation = 2;
        assert_ne!(replay_check(&world), Some(plain), "the generation is hashed");
        world.resource_mut::<ActiveRules>().generation = 1;
        assert_eq!(replay_check(&world), Some(plain));
    }

    #[test]
    fn the_journal_says_who_changed_the_rules() {
        let world = World::new();
        let line = |from: &str, to: &str, cause| journal_line(&RulesChanged { from: from.into(), to: to.into(), cause }, &world);
        assert_eq!(line("plain", "gentle", RulesCause::Call).as_deref(), Some("rules: plain → gentle (call)"));
        assert_eq!(line("plain", "harsh", RulesCause::Load).as_deref(), Some("rules: plain → harsh (save)"));
        assert_eq!(line("", "harsh", RulesCause::Call).as_deref(), Some("rules: none → harsh (call)"));
        assert_eq!(line("x", "plain", RulesCause::Content), None, "the warning says it");
    }

    #[test]
    fn the_session_line_gives_the_table_at_the_new_size() {
        let mut def = RulesDef { modes: vec!["plain".into()], start: "plain".into(), classes: vec!["brute".into()], session: default(), blocked: String::new(), host_only: String::new() };
        assert_eq!(session_line(&def, 2), None, "no table: no line");
        def.session.insert("brute".into(), SessionRow { max_health: vec![1.0, 1.5], outgoing_damage: vec![1.0, 1.25, 1.5], ..default() });
        // A row without a column (validation refuses it) adds nothing to the line.
        def.session.insert("ghost".into(), SessionRow::default());
        assert_eq!(session_line(&def, 2).as_deref(), Some("rules: 2 players: brute health ×1.5, brute damage ×1.25"));
        assert_eq!(session_line(&def, 1).as_deref(), Some("rules: 1 player: brute health ×1, brute damage ×1"));
        def.session.remove("brute");
        assert_eq!(session_line(&def, 2), None, "only rows without a column: no line");
    }

    #[test]
    fn the_dev_entry_says_why_it_does_nothing_here() {
        let mut world = World::new();
        let caller = ai_core::Caller { agent: "dev".into(), scope: ai_core::Scope::Dev, subject: None, subject_id: None, player: None };
        assert_eq!(dev_next(&world, &caller), Err(tools::NO_RULES.to_owned()), "no content");
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<RuleSetDef>();
        kinds.register::<RulesDef>();
        let sources = [
            shards_content::db::Source { path: "content/r.rules.ron".into(), text: r#"{"default": (modes: ["plain"], start: "plain")}"#.into() },
            shards_content::db::Source { path: "content/s.rule_set.ron".into(), text: r#"{"plain": (name: "Plain")}"#.into() },
        ];
        world.insert_resource(ContentDb::build(&kinds, &sources));
        assert_eq!(dev_next(&world, &caller).as_deref(), Ok("rules_set cycle=1 force=true"));
        world.insert_resource(NetRole::Client);
        assert_eq!(dev_next(&world, &caller), Err(HOST_DECIDES.to_owned()), "a client: the host chooses");
    }
}
