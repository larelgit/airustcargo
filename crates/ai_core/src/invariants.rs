//! Invariants: rules the game must keep, checked after every tick.
//!
//! A rule is defined by its violation: it fires when its condition becomes true.
//! - **engine rules**: the engine's own checks of its state, named `<module>.<rule>`
//!   (`action.resources_in_range`). Each module registers them with `add_engine_invariant`
//!   (shards_core), and `EngineConfig::engine_invariants` switches them off;
//! - **game rules**: [`add_ai_invariant`](crate::AiAppExt::add_ai_invariant), code that
//!   returns `Some(explanation)` while the rule is broken (`e5 outside the arena: x = 31.2`);
//! - **session rules**: `assert_add {name, when}` (`shards assert add hp 'field:6:Health.current<0'`)
//!   with any `step --until` condition, `warning` and `error` included.
//!
//! Rules run after every tick, in `step` and in real time. A violation takes the path of
//! a failed gameplay system: a journal error `invariant <name> violated at tick N:
//! <explanation>`, the game halts (pauses), the session is saved as
//! `replays/invariant-<name>-tick<N>.json`, and a `step` stops with `stopped: invariant
//! <name> violated at tick N (replay: …)`. The rule that fired stays off until
//! `assert_reset` re-arms it, with every other rule that fired.
//!
//! Rules never change the game and stay out of replay checks and snapshots. Changes to
//! the session's rules are recorded in the replay all the same, and playback makes them
//! at the same ticks: the replay saved at a violation meets it again at the same tick,
//! and `--verify` lists every violation on the way. Loading a snapshot keeps the rules
//! and re-arms them, and the recording that starts there begins with the session's
//! rules, so a replay from a snapshot checks them too.
//!
//! Rules run where the simulation runs: [`InvariantSystems`] is a set a game can gate
//! (the engine skips it on a network client, whose world follows the host's), and the four
//! `assert_*` tools are [`NetRoute::HostOnly`]: a client refuses them and says the rules are
//! checked on the host. (They are Dev tools, which a host caps for a guest to the player's
//! scope, so handing them over would be refused there while the client reported `ok`; and
//! `assert_list` is a query, which runs where it is called.)

use std::any::{Any, TypeId};
use std::cell::RefCell;
use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use bevy::ecs::query::{QueryData, QueryFilter};
use bevy::ecs::world::WorldId;

use bevy::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::builtin::{self, StopWhen};
use crate::id::AiId;
use crate::journal::{self, AiJournal, JournalKind};
use crate::replay;
use crate::session::Scope;
use crate::sim::{self, Halt};
use crate::tools::{register_tool, AiTools, Call, NetRoute, ToolKind, ToolResult};

/// Why a client refuses the `assert_*` tools ([`NetRoute::HostOnly`]).
pub const RULES_ON_THE_HOST: &str = "the rules are checked on the host: call it there";

/// A game rule: `Some(explanation)` while it is broken.
pub type Invariant = fn(&World) -> Option<String>;

enum Rule {
    /// [`AiInvariants::add_engine_rule`]: the engine's own, named `<module>.<rule>`.
    Engine(Invariant),
    /// [`add_ai_invariant`](crate::AiAppExt::add_ai_invariant).
    Game(Invariant),
    /// `assert_add`. Journal conditions look at entries after `seen`, so each entry once.
    Session { when: StopWhen, seen: u64 },
}

struct Entry {
    name: String,
    rule: Rule,
    /// The tick it fired at: off until `assert_reset`.
    fired: Option<u64>,
}

/// A rule found broken after a tick.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Violation {
    pub name: String,
    pub tick: u64,
    /// What the rule saw, e.g. `e5 outside the arena: x = 31.2`.
    pub text: String,
    /// The replay saved for it, relative to the game's working directory.
    pub replay: Option<String>,
}

impl Violation {
    /// `invariant hp violated at tick 12`
    pub fn headline(&self) -> String {
        format!("invariant {} violated at tick {}", self.name, self.tick)
    }
}

/// The rules (engine rules first, then game rules, then session rules, each in the order added) and every
/// violation this session.
#[derive(Resource)]
pub struct AiInvariants {
    rules: Vec<Entry>,
    violations: Vec<Violation>,
    /// Violations a `step` has already been told about.
    reported: usize,
    /// Save `replays/invariant-<name>-tick<N>.json` for each violation. `false` in tests
    /// and in `--verify`.
    pub replays: bool,
}

impl AiInvariants {
    pub fn new(replays: bool) -> Self {
        Self { rules: Vec::new(), violations: Vec::new(), reported: 0, replays }
    }

    /// Every violation this session, oldest first.
    pub fn violations(&self) -> &[Violation] {
        &self.violations
    }

    /// Adds a game rule (prefer [`AiAppExt::add_ai_invariant`](crate::AiAppExt::add_ai_invariant));
    /// a game rule with the same name is replaced, an engine rule is never replaced.
    pub fn add_game_rule(&mut self, name: &str, rule: Invariant) -> Result<(), String> {
        check_name(name)?;
        let entry = Entry { name: name.to_owned(), rule: Rule::Game(rule), fired: None };
        match self.rules.iter().position(|e| e.name == name) {
            Some(i) if matches!(self.rules[i].rule, Rule::Engine(_)) => {
                return Err(format!("`{name}` is one of the engine's invariants; pick another name"));
            }
            Some(i) => self.rules[i] = entry,
            None => {
                // Game rules come before session rules.
                let at = self.rules.iter().position(|e| matches!(e.rule, Rule::Session { .. })).unwrap_or(self.rules.len());
                self.rules.insert(at, entry);
            }
        }
        Ok(())
    }

    /// Adds an engine rule, named `<module>.<rule>` (prefer `add_engine_invariant` from
    /// shards_core, which honours `EngineConfig::engine_invariants`). Engine rules come
    /// before the game's. A rule with the same name is replaced, so a plugin that is
    /// added twice registers once.
    pub fn add_engine_rule(&mut self, name: &str, rule: Invariant) -> Result<(), String> {
        check_engine_name(name)?;
        let entry = Entry { name: name.to_owned(), rule: Rule::Engine(rule), fired: None };
        match self.rules.iter().position(|e| e.name == name) {
            Some(i) => self.rules[i] = entry,
            None => {
                let at = self.rules.iter().position(|e| !matches!(e.rule, Rule::Engine(_))).unwrap_or(self.rules.len());
                self.rules.insert(at, entry);
            }
        }
        Ok(())
    }

    /// The names of the rules, in checking order (engine, game, session).
    pub fn rule_names(&self) -> Vec<&str> {
        self.rules.iter().map(|e| e.name.as_str()).collect()
    }

    fn position(&self, name: &str) -> Option<usize> {
        self.rules.iter().position(|e| e.name == name)
    }

    fn names(&self) -> String {
        let names: Vec<&str> = self.rules.iter().map(|e| e.name.as_str()).collect();
        if names.is_empty() { "none".to_owned() } else { names.join(", ") }
    }
}

impl Default for AiInvariants {
    fn default() -> Self {
        Self::new(true)
    }
}

/// Rule names end up in journal lines, `shards assert` commands and replay file names.
pub(crate) fn check_name(name: &str) -> Result<(), String> {
    let fits = !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if fits {
        Ok(())
    } else {
        Err(format!("bad rule name `{name}`: use up to 64 letters, digits, `_` and `-` (it names the replay file), e.g. boss_hp"))
    }
}

/// Runs `f` on a query state for `world` that is kept between calls, or returns `None` when
/// a component the query names is not registered in the world (the query state of a rule is
/// built again on the next call, and then it is kept).
///
/// The rules run after every tick and cannot hold state of their own (they are `fn(&World)`),
/// and building a `QueryState` walks every archetype of the world, which costs more than a rule
/// that finds nothing wrong. The state lives in a thread-local cache keyed by the world, so apps
/// side by side in one process (a stand) keep their own; it picks up new archetypes by itself.
pub fn with_query<D, F, R>(world: &World, f: impl FnOnce(&mut QueryState<D, F>) -> R) -> Option<R>
where
    D: QueryData + 'static,
    F: QueryFilter + 'static,
{
    thread_local! {
        static STATES: RefCell<HashMap<(TypeId, WorldId), Box<dyn Any>>> = RefCell::new(HashMap::new());
    }
    STATES.with(|states| {
        let key = (TypeId::of::<QueryState<D, F>>(), world.id());
        // Taken out while `f` runs: a rule may call `with_query` again inside, and a panic in
        // `f` drops the state, which the next call builds again.
        let kept = states.borrow_mut().remove(&key);
        let mut state = match kept.and_then(|b| b.downcast::<QueryState<D, F>>().ok()) {
            Some(state) => state,
            None => Box::new(world.try_query_filtered::<D, F>()?),
        };
        let out = f(&mut state);
        states.borrow_mut().insert(key, state);
        Some(out)
    })
}

/// Collects the entities that break a rule, to report the lowest [`AiId`] first and
/// how many more there are, in the style `e6 Health.current = 700 outside 0..=600 (+2 more)`.
///
/// The text of a violation is built only for the lowest id seen so far, so a rule that
/// finds nothing allocates nothing, and the report does not depend on the order the
/// query visits entities in.
#[derive(Default)]
pub struct Offenders {
    count: usize,
    lowest: Option<(AiId, String)>,
}

impl Offenders {
    /// Notes that `id` breaks the rule; `what` is called only if it is the lowest so far.
    pub fn note(&mut self, id: AiId, what: impl FnOnce() -> String) {
        self.count += 1;
        if self.lowest.as_ref().is_none_or(|(low, _)| id < *low) {
            self.lowest = Some((id, what()));
        }
    }

    /// How many ids were noted.
    pub fn count(&self) -> usize {
        self.count
    }

    /// `None` when nothing was noted, else `e6 <what>` (plus ` (+N more)`).
    pub fn report(self) -> Option<String> {
        let (id, what) = self.lowest?;
        Some(if self.count > 1 { format!("{id} {what} (+{} more)", self.count - 1) } else { format!("{id} {what}") })
    }
}

/// Engine rule names are `<module>.<rule>`: lowercase letters, digits and `_` on both sides
/// of one dot. The name goes into replay file names, like any rule name.
pub(crate) fn check_engine_name(name: &str) -> Result<(), String> {
    let part = |s: &str| !s.is_empty() && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
    let fits = name.len() <= 64 && name.split_once('.').is_some_and(|(module, rule)| part(module) && part(rule));
    if fits {
        Ok(())
    } else {
        Err(format!("bad engine rule name `{name}`: use `<module>.<rule>` in lowercase letters, digits and `_`, e.g. action.resources_in_range"))
    }
}

/// `FixedLast`: the rules, after the tick's failures are collected.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InvariantSystems;

pub(crate) fn plugin(app: &mut App, replays: bool) {
    app.world_mut().get_resource_or_init::<AiInvariants>().replays = replays;
    // Last in the tick: its log lines and its failures are in the journal by now.
    app.configure_sets(FixedLast, InvariantSystems.after(crate::failures::collect_after_tick))
        .add_systems(FixedLast, check_after_tick.in_set(InvariantSystems));
    let world = app.world_mut();
    register_tool(world, "assert_add", Scope::Dev, ToolKind::Control, assert_add_tool);
    register_tool(world, "assert_list", Scope::Dev, ToolKind::Query, assert_list_tool);
    register_tool(world, "assert_remove", Scope::Dev, ToolKind::Control, assert_remove_tool);
    register_tool(world, "assert_reset", Scope::Dev, ToolKind::Control, assert_reset_tool);
    // The rules live where they are checked, on the host. A client does not hand the changes
    // over: the host caps a guest's forwarded calls to the player scope, so it would refuse
    // these Dev tools while the client's call reported `ok`. The list is a query, which never
    // goes to the host either. A client refuses all four and says where to call them.
    let mut tools = world.get_resource_or_init::<AiTools>();
    for name in ["assert_add", "assert_list", "assert_remove", "assert_reset"] {
        if let Err(why) = tools.set_route(name, NetRoute::HostOnly(RULES_ON_THE_HOST)) {
            panic!("invariants: {why}");
        }
    }
}

// --- checking --------------------------------------------------------------------

/// After every tick: runs the armed rules and fires the broken ones.
fn check_after_tick(world: &mut World) {
    let tick = sim::tick(world);
    let Some(invariants) = world.get_resource::<AiInvariants>() else {
        return;
    };
    // A failed system left this tick half-done, and the failure halted the game already.
    let broken: Vec<(usize, String)> = if crate::failures::failed_this_tick(world) {
        Vec::new()
    } else {
        invariants
            .rules
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.fired.is_none())
            .filter_map(|(i, entry)| broken(world, &entry.rule).map(|text| (i, text)))
            .collect()
    };
    let mut first = None;
    for (i, text) in broken {
        let violation = fire(world, i, tick, text);
        first.get_or_insert(Halt { reason: violation.headline(), replay: violation.replay });
    }
    if let Some(halt) = first {
        sim::halt(world, halt);
    }
    // Entries up to here are seen, the lines the rules just wrote included: an `error`
    // rule does not fire on another rule's violation.
    let seen = world.resource::<AiJournal>().last_seq();
    for entry in &mut world.resource_mut::<AiInvariants>().rules {
        if let Rule::Session { seen: s, .. } = &mut entry.rule {
            *s = seen;
        }
    }
}

fn broken(world: &World, rule: &Rule) -> Option<String> {
    match rule {
        Rule::Engine(rule) | Rule::Game(rule) => match catch_unwind(AssertUnwindSafe(|| rule(world))) {
            Ok(text) => text,
            Err(panic) => {
                let message = crate::tools::panic_message(&panic);
                // Reported as the rule's violation: no crash report.
                crate::crash::handled(&message);
                Some(format!("the rule itself panicked: {message}"))
            }
        },
        Rule::Session { when, seen } => builtin::condition_hit(world, when, *seen),
    }
}

fn fire(world: &mut World, index: usize, tick: u64, text: String) -> Violation {
    let name = {
        let mut invariants = world.resource_mut::<AiInvariants>();
        let entry = &mut invariants.rules[index];
        entry.fired = Some(tick);
        entry.name.clone()
    };
    let mut violation = Violation { name, tick, text, replay: None };
    journal::log(world, JournalKind::Error, format!("{}: {}", violation.headline(), violation.text));
    violation.replay = save_replay(world, &violation);
    world.resource_mut::<AiInvariants>().violations.push(violation.clone());
    violation
}

fn save_replay(world: &mut World, violation: &Violation) -> Option<String> {
    let recording = world.get_resource::<replay::AiReplay>().is_some_and(replay::AiReplay::is_recording);
    if !world.resource::<AiInvariants>().replays || !recording {
        return None;
    }
    let recording = replay::save(world);
    let path = format!("{}/invariant-{}-tick{}.json", replay::REPLAYS_DIR, violation.name, violation.tick);
    match replay::write(&recording, &path) {
        Ok(_) => {
            let text = format!("invariant replay saved: {path} (play it with --replay to reproduce)");
            journal::log(world, JournalKind::Sim, text);
            Some(path)
        }
        Err(e) => {
            journal::log(world, JournalKind::Error, format!("invariant replay not saved: {e}"));
            None
        }
    }
}

/// The recording restarts (a snapshot was loaded): every rule is armed again, journal
/// rules look at entries from now on. Returns the session's rules as `Add`s, in order,
/// for the new recording to start with.
pub(crate) fn restarted(world: &mut World) -> Vec<AssertOp> {
    let Some(seen) = world.get_resource::<AiJournal>().map(AiJournal::last_seq) else {
        return Vec::new();
    };
    let Some(mut invariants) = world.get_resource_mut::<AiInvariants>() else {
        return Vec::new();
    };
    let mut ops = Vec::new();
    for entry in &mut invariants.rules {
        entry.fired = None;
        if let Rule::Session { when, seen: s } = &mut entry.rule {
            *s = seen;
            ops.push(AssertOp::Add { name: entry.name.clone(), when: when.clone() });
        }
    }
    ops
}

/// The rules as they stand at one tick: the session's (in order) and which rules have fired.
/// A game that continues from a snapshot of that tick (heavy mode's twin) takes them over, so
/// it checks what the original checks from there on, and fires what the original fires.
#[derive(Clone, Debug, Default)]
pub(crate) struct RulesState {
    session: Vec<(String, StopWhen)>,
    fired: Vec<(String, u64)>,
}

pub(crate) fn rules_state(world: &World) -> RulesState {
    let Some(invariants) = world.get_resource::<AiInvariants>() else {
        return RulesState::default();
    };
    let session = invariants
        .rules
        .iter()
        .filter_map(|entry| match &entry.rule {
            Rule::Session { when, .. } => Some((entry.name.clone(), when.clone())),
            Rule::Game(_) => None,
        })
        .collect();
    let fired = invariants.rules.iter().filter_map(|entry| Some((entry.name.clone(), entry.fired?))).collect();
    RulesState { session, fired }
}

/// Puts `state` in: the session's rules are `state`'s (looking at the journal from now on),
/// and a rule has fired when it had there. The game's own rules are this game's.
pub(crate) fn set_rules_state(world: &mut World, state: &RulesState) {
    let Some(seen) = world.get_resource::<AiJournal>().map(AiJournal::last_seq) else {
        return;
    };
    let Some(mut invariants) = world.get_resource_mut::<AiInvariants>() else {
        return;
    };
    invariants.rules.retain(|entry| matches!(entry.rule, Rule::Game(_)));
    for (name, when) in &state.session {
        invariants.rules.push(Entry { name: name.clone(), rule: Rule::Session { when: when.clone(), seen }, fired: None });
    }
    for entry in &mut invariants.rules {
        entry.fired = state.fired.iter().find(|(name, _)| *name == entry.name).map(|(_, tick)| *tick);
    }
}

/// Violations since the last call, for the `step` that is running (or a stand's driver).
pub fn take_unreported(world: &mut World) -> Vec<Violation> {
    let Some(mut invariants) = world.get_resource_mut::<AiInvariants>() else {
        return Vec::new();
    };
    let new = invariants.violations[invariants.reported..].to_vec();
    invariants.reported = invariants.violations.len();
    new
}

// --- session rules ---------------------------------------------------------------

/// A change to the session's rules, as the `assert_*` tools make it and replays store it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AssertOp {
    Add { name: String, when: StopWhen },
    Remove { name: String },
    Reset,
}

/// Makes the change and records it in the replay: playback makes it again at this tick.
pub(crate) fn apply(world: &mut World, op: &AssertOp) -> Result<String, String> {
    let done = match op {
        AssertOp::Add { name, when } => add(world, name, when)?,
        AssertOp::Remove { name } => remove(world, name)?,
        AssertOp::Reset => reset(world),
    };
    replay::record_assert(world, op.clone());
    Ok(done)
}

fn add(world: &mut World, name: &str, when: &StopWhen) -> Result<String, String> {
    let invariants = world.resource::<AiInvariants>();
    if invariants.position(name).is_some_and(|i| matches!(invariants.rules[i].rule, Rule::Engine(_))) {
        return Err(format!("`{name}` is one of the engine's invariants; pick another name"));
    }
    check_name(name)?;
    builtin::validate(world, when)?;
    let seen = world.resource::<AiJournal>().last_seq();
    let mut invariants = world.resource_mut::<AiInvariants>();
    let entry = Entry { name: name.to_owned(), rule: Rule::Session { when: when.clone(), seen }, fired: None };
    match invariants.position(name) {
        Some(i) if matches!(invariants.rules[i].rule, Rule::Game(_)) => {
            Err(format!("`{name}` is one of the game's invariants; pick another name"))
        }
        Some(i) => {
            invariants.rules[i] = entry;
            Ok(format!("replaced {name}: the game halts when {when}"))
        }
        None => {
            invariants.rules.push(entry);
            Ok(format!("added {name}: the game halts when {when}"))
        }
    }
}

fn remove(world: &mut World, name: &str) -> Result<String, String> {
    let mut invariants = world.resource_mut::<AiInvariants>();
    match invariants.position(name) {
        Some(i) if matches!(invariants.rules[i].rule, Rule::Engine(_)) => Err(format!(
            "`{name}` is one of the engine's invariants and stays; after it fires, `assert_reset` re-arms it"
        )),
        Some(i) if matches!(invariants.rules[i].rule, Rule::Game(_)) => Err(format!(
            "`{name}` is one of the game's invariants and stays; after it fires, `assert_reset` re-arms it"
        )),
        Some(i) => {
            invariants.rules.remove(i);
            Ok(format!("removed {name}"))
        }
        None => Err(format!("no rule `{name}`; rules: {}", invariants.names())),
    }
}

fn reset(world: &mut World) -> String {
    // A re-armed rule looks at the journal from now on, not at what it missed while off.
    let seen = world.resource::<AiJournal>().last_seq();
    let mut rearmed = Vec::new();
    for entry in &mut world.resource_mut::<AiInvariants>().rules {
        if entry.fired.take().is_some() {
            rearmed.push(entry.name.clone());
            if let Rule::Session { seen: s, .. } = &mut entry.rule {
                *s = seen;
            }
        }
    }
    // Heavy mode stops checking after a failed check, like a rule that fired.
    if crate::heavy::rearm(world) {
        rearmed.push("heavy".to_owned());
    }
    if rearmed.is_empty() { "no rule has fired; nothing to re-arm".to_owned() } else { format!("re-armed {}", rearmed.join(", ")) }
}

/// Add a rule the game must keep. After every tick (in `step` and in real time) the game
/// halts when `when` becomes true: an error in the journal, a pause, the session saved as
/// replays/invariant-<name>-tick<N>.json, and `step` stops there. A rule that fired stays
/// off until `assert_reset`. Rules are recorded in replays, which then catch the same
/// violations at the same ticks.
#[derive(Deserialize, JsonSchema)]
pub struct AssertAddArgs {
    /// Letters, digits, `_` and `-` (it names the replay file). A session rule with
    /// this name is replaced.
    pub name: String,
    /// A `step --until` condition: {"field": {"id": 6, "component": "Health", "path":
    /// "current", "below": 0}}, {"gone": 7}, "warning", "error", ….
    pub when: StopWhen,
}

fn assert_add_tool(In(call): In<Call<AssertAddArgs>>, world: &mut World) -> ToolResult {
    let AssertAddArgs { name, when } = call.args;
    apply(world, &AssertOp::Add { name, when })
}

/// Remove a rule added with `assert_add` (the engine's and the game's own rules stay).
#[derive(Deserialize, JsonSchema)]
pub struct AssertRemoveArgs {
    pub name: String,
}

fn assert_remove_tool(In(call): In<Call<AssertRemoveArgs>>, world: &mut World) -> ToolResult {
    apply(world, &AssertOp::Remove { name: call.args.name })
}

/// Re-arm every rule that fired, the engine's, the game's and the session's: each fires again after
/// the next tick that finds it broken.
#[derive(Deserialize, JsonSchema, Default)]
pub struct AssertResetArgs {}

fn assert_reset_tool(In(_): In<Call<AssertResetArgs>>, world: &mut World) -> ToolResult {
    apply(world, &AssertOp::Reset)
}

/// The rules checked after every tick: the engine's, the game's own and this session's, armed or fired.
#[derive(Deserialize, JsonSchema, Default)]
pub struct AssertListArgs {}

fn assert_list_tool(
    In(_): In<Call<AssertListArgs>>,
    invariants: Res<AiInvariants>,
    heavy: Option<Res<crate::heavy::AiHeavy>>,
) -> ToolResult {
    let heavy = heavy.map(|h| format!("\n{}", h.summary())).unwrap_or_default();
    if invariants.rules.is_empty() {
        return Ok(format!("no rules; add one with assert_add {{name, when}}{heavy}"));
    }
    let mut out = format!("{} rules, checked after every tick", invariants.rules.len());
    for entry in &invariants.rules {
        let what = match &entry.rule {
            Rule::Engine(_) => "engine rule".to_owned(),
            Rule::Game(_) => "game rule".to_owned(),
            Rule::Session { when, .. } => format!("when {when}"),
        };
        let state = match entry.fired {
            Some(tick) => format!("fired at tick {tick}, off until assert_reset"),
            None => "armed".to_owned(),
        };
        out.push_str(&format!("\n  {}  {what}  {state}", entry.name));
    }
    if let Some(last) = invariants.violations.last() {
        out.push_str(&format!("\nviolations so far: {} (last: {})", invariants.violations.len(), last.headline()));
    }
    out.push_str(&heavy);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_names_fit_a_file_name() {
        assert!(check_name("boss_hp-2").is_ok());
        for bad in ["", "boss hp", "../x", "hp/x", &"x".repeat(65)] {
            let err = check_name(bad).unwrap_err();
            assert!(err.contains("letters, digits"), "{err}");
        }
    }

    #[test]
    fn game_rules_come_first() {
        let mut rules = AiInvariants::new(false);
        rules.rules.push(Entry { name: "mine".into(), rule: Rule::Session { when: StopWhen::Error, seen: 0 }, fired: None });
        rules.add_game_rule("health", |_| None).unwrap();
        rules.add_game_rule("arena", |_| None).unwrap();
        rules.add_game_rule("health", |_| Some("replaced".into())).unwrap();
        let names: Vec<&str> = rules.rules.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["health", "arena", "mine"]);
        assert!(rules.add_game_rule("no spaces", |_| None).is_err());
    }

    #[test]
    fn engine_rules_have_their_own_names_and_come_first() {
        for ok in ["ai.index_consistent", "a1.b_2"] {
            assert!(check_engine_name(ok).is_ok(), "{ok}");
        }
        for bad in ["", "plain", "a.b.c", ".b", "a.", "Ai.x", "a.b-c", "a b.c", "../x.y"] {
            assert!(check_engine_name(bad).unwrap_err().contains("<module>.<rule>"), "{bad}");
        }
        let mut rules = AiInvariants::new(false);
        rules.rules.push(Entry { name: "mine".into(), rule: Rule::Session { when: StopWhen::Error, seen: 0 }, fired: None });
        rules.add_game_rule("health", |_| None).unwrap();
        rules.add_engine_rule("ai.one", |_| None).unwrap();
        rules.add_engine_rule("core.two", |_| None).unwrap();
        rules.add_engine_rule("ai.one", |_| None).unwrap();
        assert_eq!(rules.rule_names(), ["ai.one", "core.two", "health", "mine"]);
        assert!(rules.add_game_rule("ai.one", |_| None).is_err());
        assert!(rules.add_game_rule("no.dots.here", |_| None).is_err());
    }

    #[test]
    fn a_kept_query_sees_new_entities_and_belongs_to_its_world() {
        let count = |world: &World| with_query::<&AiId, (), _>(world, |q| q.iter(world).count());
        let mut a = World::new();
        a.init_resource::<crate::AiIndex>();
        let mut b = World::new();
        b.init_resource::<crate::AiIndex>();
        a.spawn(crate::AiVisible);
        assert_eq!(count(&a), Some(1));
        a.spawn(crate::AiVisible);
        a.spawn((crate::AiVisible, Name::new("in a new archetype")));
        assert_eq!(count(&a), Some(3), "the kept state follows new entities and archetypes");
        assert_eq!(count(&b), None, "an AiId nobody spawned in b is not registered there");
        b.spawn(crate::AiVisible);
        assert_eq!(count(&b), Some(1), "another world has its own state");
        // A component the world does not know: nothing to judge, and no state is kept.
        assert_eq!(with_query::<&crate::replay::AiReplay, (), _>(&b, |_| ()), None);
    }

    #[test]
    fn a_rule_may_nest_queries_and_a_panic_inside_loses_only_the_kept_state() {
        let mut w = World::new();
        w.init_resource::<crate::AiIndex>();
        w.spawn(crate::AiVisible);
        let nested = with_query::<&AiId, (), _>(&w, |outer| {
            outer.iter(&w).count() + with_query::<&AiId, (), _>(&w, |inner| inner.iter(&w).count()).unwrap()
        });
        assert_eq!(nested, Some(2));
        let panicked = catch_unwind(AssertUnwindSafe(|| with_query::<&AiId, (), _>(&w, |_| panic!("the rule has a bug"))));
        assert!(panicked.is_err());
        assert_eq!(with_query::<&AiId, (), _>(&w, |q| q.iter(&w).count()), Some(1), "built again");
    }

    #[test]
    fn ops_have_a_readable_json_shape() {
        let add = AssertOp::Add { name: "hp".into(), when: StopWhen::Error };
        assert_eq!(serde_json::to_value(&add).unwrap(), serde_json::json!({"add": {"name": "hp", "when": "error"}}));
        assert_eq!(serde_json::to_value(AssertOp::Reset).unwrap(), serde_json::json!("reset"));
        let back: AssertOp = serde_json::from_value(serde_json::json!({"remove": {"name": "hp"}})).unwrap();
        assert_eq!(back, AssertOp::Remove { name: "hp".into() });
    }
}
