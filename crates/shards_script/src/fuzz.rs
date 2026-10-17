//! A fuzz run: seeded player inputs between short steps, and now and then a developer call that
//! shakes the world, until the game halts or the rounds run out (`shards fuzz`).
//!
//! Like a scenario and a soak, a fuzz run only moves JSON through a [`Driver`]: against a live
//! game over BRP (`shards fuzz`) or in-process under `cargo test`
//! (`ai_core::scenario::WorldDriver`). It has no model in it: the same seed, the same tool list
//! and the same game make the same calls.
//!
//! **Inputs** ([`Alphabet`]). `schema` (the default) plays every tool of scope `player` and kind
//! `action` that the game lists, sorted by name, with arguments made from the tool's own input
//! schema ([`crate::schema`]); `--weights move=3,dodge=2` tilts the choice, and `--invalid P`
//! makes that share of the calls break exactly one argument on purpose. `classic` keeps the
//! four tools and the exact random draws of the first fuzzer, so old seeds reproduce.
//!
//! **Chaos** (`--chaos P`). That share of the rounds makes one developer call instead of a
//! player input: a snapshot saved and loaded again, a region generated, content reloaded, a
//! game saved and loaded, a pause, a change of speed, a heal. The game must take all of them
//! between any two inputs.

use std::collections::BTreeMap;
use std::fmt;
use std::str::FromStr;

use serde::Serialize;
use serde_json::{json, Value};

use crate::inputs::{ids_in, random_input, SplitMix};
use crate::run::{Driver, Halted, Outcome};
use crate::schema::{invalid_args, schema_args, Hints};

/// A tool as `ai/tools` lists it (`shards tools --json`): what the fuzzer needs of it.
#[derive(Clone, Debug, PartialEq)]
pub struct ToolDesc {
    pub name: String,
    /// `player` or `dev`.
    pub scope: String,
    /// `query`, `action` or `control`.
    pub kind: String,
    /// The one-line form for errors: `move_to(target: id|{x,z}, run?: bool)`.
    pub signature: String,
    /// JSON Schema of the arguments.
    pub schema: Value,
}

impl ToolDesc {
    /// The tools of an `ai/tools` answer (an array); entries without a name are skipped.
    pub fn list_from_json(list: &Value) -> Vec<ToolDesc> {
        let text = |t: &Value, k: &str| t.get(k).and_then(Value::as_str).unwrap_or_default().to_owned();
        list.as_array()
            .into_iter()
            .flatten()
            .filter(|t| t.get("name").is_some_and(Value::is_string))
            .map(|t| ToolDesc {
                name: text(t, "name"),
                scope: text(t, "scope"),
                kind: text(t, "kind"),
                signature: text(t, "signature"),
                schema: t.get("input_schema").cloned().unwrap_or_else(|| json!({"type": "object", "properties": {}})),
            })
            .collect()
    }

    /// Whether a player may call it and it changes the world: what a fuzz run plays.
    pub fn is_player_action(&self) -> bool {
        self.scope == "player" && self.kind == "action"
    }
}

/// The tools a fuzz run plays: scope `player`, kind `action`, sorted by name.
pub fn alphabet(tools: &[ToolDesc]) -> Vec<&ToolDesc> {
    let mut out: Vec<&ToolDesc> = tools.iter().filter(|t| t.is_player_action()).collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Where the player inputs come from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Alphabet {
    /// Every player action tool of the game, with arguments from its schema.
    #[default]
    Schema,
    /// `move`, `attack`, `light_attack` and `dodge` as the first fuzzer drew them.
    Classic,
}

impl FromStr for Alphabet {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "schema" => Ok(Alphabet::Schema),
            "classic" => Ok(Alphabet::Classic),
            other => Err(format!("unknown alphabet `{other}`: use `schema` (every player action tool) or `classic` (move, attack, light_attack, dodge)")),
        }
    }
}

/// `move=3,dodge=2` as `[("move", 3), ("dodge", 2)]`.
pub fn parse_weights(text: &str) -> Result<Vec<(String, u32)>, String> {
    let mut out = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (name, weight) = part.split_once('=').ok_or_else(|| format!("weight `{part}` has no `=`: write tool=number, like move=3,dodge=2"))?;
        let weight: u32 = weight.trim().parse().map_err(|_| format!("weight of `{name}` is `{weight}`, not a whole number: write move=3"))?;
        out.push((name.trim().to_owned(), weight));
    }
    Ok(out)
}

/// What a fuzz run does.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FuzzPlan {
    /// Seed of the inputs (the game has its own): the same seed against the same fresh game
    /// makes the same calls.
    pub seed: u64,
    /// Rounds to play: each makes one input (or one chaos call), then steps.
    pub rounds: u32,
    /// Longest step after an input, in ticks.
    pub max_ticks: u64,
    pub alphabet: Alphabet,
    /// Tool name and weight; a tool that is not named weighs 1, and weight 0 drops it.
    pub weights: Vec<(String, u32)>,
    /// Share of the player inputs (0..=1) that break one argument on purpose (`schema` only).
    pub invalid: f64,
    /// Share of the rounds (0..=1) that make one developer call instead of a player input.
    pub chaos: f64,
    /// The agent that plays. It needs dev scope for the chaos calls (the default agent has it).
    pub agent: String,
    /// File stem of what the run writes (`snapshots/<stem>.json`, `replays/<stem>.json`).
    pub stem: String,
    /// Save the session as a replay at the end (or when the game halts).
    pub save_replay: bool,
}

impl FuzzPlan {
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            rounds: 200,
            max_ticks: 30,
            alphabet: Alphabet::Schema,
            weights: Vec::new(),
            invalid: DEFAULT_INVALID,
            chaos: 0.0,
            agent: "fuzz".to_owned(),
            stem: format!("fuzz-seed{seed}"),
            save_replay: false,
        }
    }
}

/// The default share of calls with an argument broken on purpose.
pub const DEFAULT_INVALID: f64 = 0.05;

/// One round as it was played, for whoever replays or shrinks a run.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Round {
    /// The calls of the round in order: one input, or the chaos calls.
    pub calls: Vec<(String, Value)>,
    /// The step that ended the round, in ticks.
    pub ticks: u64,
}

/// How a fuzz run ended.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub enum Ended {
    /// Every round was played and nothing broke.
    Done,
    /// The game stopped itself: a failed gameplay system or a violated invariant.
    Halted { reason: String, replay: Option<String> },
    /// `step` was refused: the run cannot go on (the text says why).
    StepRefused(String),
    /// The second half of a chaos round trip was refused after the first half worked: a
    /// `snapshot_load` of the snapshot just saved, a `load_game` of the save just made. The
    /// world does not come back as it was: a finding, like a halt.
    ChaosBroke { call: String, why: String },
}

/// What a fuzz run did.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FuzzReport {
    pub plan: FuzzPlan,
    pub from_tick: u64,
    pub to_tick: u64,
    /// Rounds played (the one that halted the game counts).
    pub rounds: u32,
    /// Player inputs made, and how many of them the game refused (part of play).
    pub inputs: u64,
    pub refused: u64,
    /// Inputs that broke an argument on purpose, and how many of those the game took anyway.
    pub invalid: u64,
    pub invalid_taken: u64,
    /// Calls made per tool, player inputs and chaos calls alike.
    pub calls: BTreeMap<String, u64>,
    /// Chaos rounds played, and how many calls of them the game refused.
    pub chaos_rounds: u64,
    pub chaos_refused: u64,
    /// The first chaos call the game refused, as `tool args: answer`, so that a count of
    /// refusals is never just a number.
    pub first_chaos_refusal: Option<String>,
    pub ended: Ended,
    /// The replay of the session, when the plan saves one.
    pub replay: Option<String>,
    pub played: Vec<Round>,
}

impl FuzzReport {
    pub fn halted(&self) -> bool {
        matches!(self.ended, Ended::Halted { .. })
    }

    /// A chaos round trip failed on its second half.
    pub fn chaos_broke(&self) -> bool {
        matches!(self.ended, Ended::ChaosBroke { .. })
    }
}

impl fmt::Display for FuzzReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let seed = self.plan.seed;
        match &self.ended {
            Ended::Halted { reason, .. } => write!(
                f,
                "fuzz seed {seed}: the game halted after {} rounds (ticks {} → {}): {reason}",
                self.rounds, self.from_tick, self.to_tick
            ),
            Ended::StepRefused(why) => write!(f, "fuzz seed {seed}: step refused after {} rounds: {why}", self.rounds),
            Ended::ChaosBroke { call, why } => {
                write!(
                    f,
                    "fuzz seed {seed}: a chaos call broke the game after {} rounds (ticks {} → {}): {call} was refused: {why}",
                    self.rounds, self.from_tick, self.to_tick
                )?;
                // Other refusals came before it: the first is named too.
                match (&self.first_chaos_refusal, self.chaos_refused) {
                    (Some(first), n) if n > 1 => write!(f, "; {n} chaos calls refused, the first {first}"),
                    _ => Ok(()),
                }
            }
            Ended::Done => {
                write!(f, "fuzz seed {seed}: {} rounds, nothing broke (ticks {} → {})", self.rounds, self.from_tick, self.to_tick)?;
                match &self.first_chaos_refusal {
                    Some(first) => write!(f, "; {} chaos calls refused, the first {first}", self.chaos_refused),
                    None => Ok(()),
                }
            }
        }
    }
}

/// `p` of the draws is true (`p` outside 0..=1 is clamped). Draws nothing for 0.
fn chance(rng: &mut SplitMix, p: f64) -> bool {
    if p <= 0.0 {
        return false;
    }
    rng.below(1_000_000) < (p.clamp(0.0, 1.0) * 1_000_000.0) as u64
}

/// Whether the `observe` text says the game is paused (`tick 0 (0.00s) | paused | …`).
fn observed_paused(observation: &str) -> bool {
    observation.lines().next().is_some_and(|l| l.split(" | ").any(|part| part == "paused"))
}

/// The speed the observation says (`speed 2x`), 1 when it does not.
fn observed_speed(observation: &str) -> f64 {
    let header = observation.lines().next().unwrap_or_default();
    header.split(" | ").find_map(|p| p.strip_prefix("speed ")?.strip_suffix('x')?.parse().ok()).unwrap_or(1.0)
}

/// The ids of a `content_list kind=…` answer: `region: 2 definitions\n  wilds  file`.
fn listed_ids(answer: &str) -> Vec<String> {
    answer.lines().skip(1).filter_map(|l| l.split_whitespace().next()).map(str::to_owned).collect()
}

/// The most words a run keeps, and how many of them [`vocabulary`] takes from content: the rest is
/// room for what refusals teach ([`learn_words`]).
const MAX_WORDS: usize = 400;
const CONTENT_WORDS: usize = 300;

/// What the game calls things: the ids of its content of every kind (`content_list`), sorted,
/// without repeats. A text argument made from the schema alone ("a", "hello world") names
/// nothing the game knows, and `spawn_prefab` or `ui_open` refuse it before they do anything.
pub fn vocabulary(driver: &mut dyn Driver, agent: &str) -> Result<Vec<String>, String> {
    let kinds = driver.call(agent, "content_list", json!({}))?;
    if !kinds.ok {
        return Ok(Vec::new());
    }
    let mut words = std::collections::BTreeSet::new();
    for kind in listed_ids(&kinds.text()) {
        let ids = driver.call(agent, "content_list", json!({"kind": kind}))?;
        if ids.ok {
            words.extend(listed_ids(&ids.text()));
        }
    }
    Ok(words.into_iter().take(CONTENT_WORDS).collect())
}

/// Adds to `words` the names a refusal lists ("unknown prefab x; known: skeleton, pillar", "use N,
/// NE, E"): the words after a colon or a semicolon, which is where the engine's errors put what
/// would have been accepted. Without repeats, in the order met, at most [`MAX_WORDS`].
pub fn learn_words(words: &mut Vec<String>, refusal: &str) {
    for chunk in refusal.split([':', ';']).skip(1) {
        for token in chunk.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '.')) {
            let named = (2..=40).contains(&token.len()) && token.bytes().any(|b| b.is_ascii_alphabetic());
            if named && words.len() < MAX_WORDS && !words.iter().any(|w| w == token) {
                words.push(token.to_owned());
            }
        }
    }
}

/// The developer calls chaos picks from, by number.
const CHAOS_OPS: u64 = 7;

struct Run<'a> {
    driver: &'a mut dyn Driver,
    plan: &'a FuzzPlan,
    report: FuzzReport,
    /// Calls of the round in the making.
    round: Vec<(String, Value)>,
    halted: Option<Halted>,
    /// A chaos round trip that failed on its second half: the call and the answer.
    broke: Option<(String, String)>,
    /// The tick the newest call answered at.
    last_tick: Option<u64>,
    regions: Option<Vec<String>>,
}

impl Run<'_> {
    /// One call. A transport failure ends the run; a tool that refuses is part of play.
    fn call(&mut self, tool: &str, args: Value) -> Result<Outcome, String> {
        let out = self.driver.call(&self.plan.agent, tool, args.clone())?;
        *self.report.calls.entry(tool.to_owned()).or_default() += 1;
        self.round.push((tool.to_owned(), args));
        self.last_tick = out.tick.or(self.last_tick);
        if out.halted.is_some() && self.halted.is_none() {
            self.halted = out.halted.clone();
        }
        Ok(out)
    }

    /// A chaos call: a refusal is counted, not an error.
    fn chaos_call(&mut self, tool: &str, args: Value) -> Result<Outcome, String> {
        let out = self.call(tool, args)?;
        if !out.ok {
            self.report.chaos_refused += 1;
            if self.report.first_chaos_refusal.is_none() {
                let args = self.round.last().map(|(_, a)| a.to_string()).unwrap_or_default();
                self.report.first_chaos_refusal = Some(format!("{tool} {args}: {}", out.text()));
            }
        }
        Ok(out)
    }

    /// The second half of a round trip whose first half worked: a refusal is a finding.
    fn chaos_return(&mut self, tool: &str, args: Value) -> Result<Outcome, String> {
        let out = self.chaos_call(tool, args)?;
        if !out.ok && self.broke.is_none() {
            let args = self.round.last().map(|(_, a)| a.to_string()).unwrap_or_default();
            self.broke = Some((format!("{tool} {args}"), out.text()));
        }
        Ok(out)
    }

    /// One developer call, or a pair of them, drawn from the seed.
    fn chaos(&mut self, rng: &mut SplitMix, paused: bool, speed: f64) -> Result<(), String> {
        self.report.chaos_rounds += 1;
        match rng.below(CHAOS_OPS) {
            0 => {
                // A snapshot saved, then loaded again: the world must come back as it was.
                let path = format!("snapshots/{}.json", self.plan.stem);
                if self.chaos_call("snapshot_save", json!({"path": path}))?.ok {
                    self.chaos_return("snapshot_load", json!({"path": path}))?;
                }
            }
            1 => {
                let regions = self.regions()?;
                let seed = rng.below(1_000_000);
                let args = match regions.is_empty() {
                    true => json!({"seed": seed}),
                    false => json!({"region": regions[rng.below(regions.len() as u64) as usize], "seed": seed}),
                };
                self.chaos_call("regen", args)?;
            }
            2 => {
                self.chaos_call("content_reload", json!({}))?;
            }
            3 => {
                // A save in a slot of its own, so the player's slot is left alone, then loaded.
                let slot = "fuzz";
                if self.chaos_call("save_game", json!({"what": "all", "slot": slot}))?.ok {
                    self.chaos_return("load_game", json!({"what": "all", "slot": slot}))?;
                }
            }
            4 => {
                // A running game is paused and resumed; a paused one is only paused again: a
                // resume would let real time run ticks between two calls, and then the same seed
                // would not play the same game.
                self.chaos_call("pause", json!({}))?;
                if !paused {
                    self.chaos_call("resume", json!({}))?;
                }
            }
            5 => {
                // Real-time speed only (`step` ignores it); put back at once, to what it was.
                const SPEEDS: [f64; 5] = [0.25, 0.5, 2.0, 4.0, 1.0];
                let x = SPEEDS[rng.below(SPEEDS.len() as u64) as usize];
                self.chaos_call("speed", json!({"x": x}))?;
                self.chaos_call("speed", json!({"x": speed}))?;
            }
            _ => {
                self.chaos_call("heal", json!({}))?;
            }
        }
        Ok(())
    }

    /// The regions chaos may generate: the game's `region` content, asked once.
    fn regions(&mut self) -> Result<Vec<String>, String> {
        if let Some(known) = &self.regions {
            return Ok(known.clone());
        }
        let answer = self.driver.call(&self.plan.agent, "content_list", json!({"kind": "region"}))?;
        let ids = if answer.ok { listed_ids(&answer.text()) } else { Vec::new() };
        self.regions = Some(ids.clone());
        Ok(ids)
    }
}

/// Plays `plan` on the game `driver` reaches, with the tools `tools` (`ai/tools`; `classic`
/// needs none). `Err` when the game cannot be reached, or the plan cannot be played (a weight
/// that names no tool, an alphabet that is empty): the error says how to fix it. A game that
/// halts is not an error: the report says so.
pub fn run(driver: &mut dyn Driver, plan: &FuzzPlan, tools: &[ToolDesc]) -> Result<FuzzReport, String> {
    let alphabet = alphabet(tools);
    let weighted = weighted(plan, &alphabet)?;
    let mut rng = SplitMix(plan.seed);
    let report = FuzzReport {
        plan: plan.clone(),
        from_tick: 0,
        to_tick: 0,
        rounds: 0,
        inputs: 0,
        refused: 0,
        invalid: 0,
        invalid_taken: 0,
        calls: BTreeMap::new(),
        chaos_rounds: 0,
        chaos_refused: 0,
        first_chaos_refusal: None,
        ended: Ended::Done,
        replay: None,
        played: Vec::new(),
    };
    let mut run = Run { driver, plan, report, round: Vec::new(), halted: None, broke: None, last_tick: None, regions: None };
    // What the game calls things, to give text arguments names that mean something to it.
    let mut words = match plan.alphabet {
        Alphabet::Schema => vocabulary(&mut *run.driver, &plan.agent)?,
        Alphabet::Classic => Vec::new(),
    };
    let mut first = true;
    for round in 1..=plan.rounds {
        run.round.clear();
        let seen = run.driver.call(&plan.agent, "observe", json!({}))?;
        if first {
            run.report.from_tick = seen.tick.unwrap_or(0);
            first = false;
        }
        let text = seen.text();
        let ids = ids_in(&text);
        if chance(&mut rng, plan.chaos) {
            run.chaos(&mut rng, observed_paused(&text), observed_speed(&text))?;
            if let Some((call, why)) = run.broke.take() {
                run.report.rounds = round;
                let calls = std::mem::take(&mut run.round);
                run.report.played.push(Round { calls, ticks: 0 });
                run.report.to_tick = run.last_tick.unwrap_or(run.report.to_tick);
                run.report.ended = Ended::ChaosBroke { call, why };
                break;
            }
        } else {
            let (tool, args, broken) = match plan.alphabet {
                Alphabet::Classic => {
                    let (tool, args) = random_input(&mut rng, &ids);
                    (tool.to_owned(), args, false)
                }
                Alphabet::Schema => {
                    let hints = Hints { ids: &ids, words: &words };
                    let tool = weighted[pick_weighted(&mut rng, &weighted)].0;
                    if chance(&mut rng, plan.invalid) {
                        (tool.name.clone(), invalid_args(&tool.schema, &mut rng, hints).args, true)
                    } else {
                        (tool.name.clone(), schema_args(&tool.schema, &mut rng, hints), false)
                    }
                }
            };
            let out = run.call(&tool, args)?;
            if !out.ok && plan.alphabet == Alphabet::Schema {
                learn_words(&mut words, &out.text());
            }
            run.report.inputs += 1;
            run.report.refused += u64::from(!out.ok);
            run.report.invalid += u64::from(broken);
            run.report.invalid_taken += u64::from(broken && out.ok);
        }
        run.report.rounds = round;
        let ticks = 1 + rng.below(plan.max_ticks);
        let step = run.call("step", json!({"ticks": ticks}))?;
        let calls = std::mem::take(&mut run.round);
        run.report.played.push(Round { calls, ticks });
        if let Some(tick) = step.tick {
            run.report.to_tick = tick;
        }
        if !step.ok {
            run.report.ended = Ended::StepRefused(step.text());
            break;
        }
        if let Some(halted) = run.halted.take() {
            run.report.ended = Ended::Halted { reason: halted.reason, replay: halted.replay };
            break;
        }
    }
    if run.report.rounds == plan.rounds && matches!(run.report.ended, Ended::Done) {
        let seen = run.driver.call(&plan.agent, "observe", json!({}))?;
        run.report.to_tick = seen.tick.unwrap_or(run.report.to_tick);
    }
    if plan.save_replay {
        run.report.replay = save_replay(&mut run)?;
    }
    Ok(run.report)
}

/// The replay of the session: the one the game saved when it halted, else a file of our own.
fn save_replay(run: &mut Run) -> Result<Option<String>, String> {
    if let Ended::Halted { replay: Some(path), .. } = &run.report.ended {
        return Ok(Some(path.clone()));
    }
    let path = match &run.report.ended {
        Ended::Halted { .. } | Ended::ChaosBroke { .. } => format!("replays/{}-tick{}.json", run.plan.stem, run.report.to_tick),
        _ => format!("replays/{}.json", run.plan.stem),
    };
    let out = run.driver.call(&run.plan.agent, "replay_save", json!({"path": path}))?;
    match out.text().strip_prefix("saved ") {
        // "saved PATH: 120 ticks, …"
        Some(text) if out.ok => Ok(Some(text.split_once(": ").map_or(text, |(saved, _)| saved).to_owned())),
        _ => Err(format!("replay_save: {}", out.text())),
    }
}

/// The alphabet with its weights; the plan's weights must name tools of it.
fn weighted<'t>(plan: &FuzzPlan, alphabet: &[&'t ToolDesc]) -> Result<Vec<(&'t ToolDesc, u64)>, String> {
    if plan.alphabet == Alphabet::Classic {
        // The classic draws take no weights; say so rather than ignore them.
        return match plan.weights.is_empty() {
            true => Ok(Vec::new()),
            false => Err("--weights goes with the `schema` alphabet: `classic` draws its four tools as the first fuzzer did".to_owned()),
        };
    }
    if alphabet.is_empty() {
        return Err("the game lists no tool of scope player and kind action to play: is `ai/tools` answering? (`--alphabet classic` plays move, attack, light_attack and dodge)".to_owned());
    }
    for (name, _) in &plan.weights {
        if !alphabet.iter().any(|t| &t.name == name) {
            let names: Vec<&str> = alphabet.iter().map(|t| t.name.as_str()).collect();
            return Err(format!("--weights names `{name}`, which is not a player action tool; use one of: {}", names.join(", ")));
        }
    }
    let weighted: Vec<(&ToolDesc, u64)> = alphabet
        .iter()
        .map(|t| (*t, plan.weights.iter().rev().find(|(n, _)| *n == t.name).map_or(1, |(_, w)| u64::from(*w))))
        .collect();
    if weighted.iter().all(|(_, w)| *w == 0) {
        return Err("every weight is 0, so there is nothing to play: give at least one tool a weight above 0".to_owned());
    }
    Ok(weighted)
}

/// Index into `weighted`, by weight.
fn pick_weighted(rng: &mut SplitMix, weighted: &[(&ToolDesc, u64)]) -> usize {
    let total: u64 = weighted.iter().map(|(_, w)| w).sum();
    let mut at = rng.below(total);
    for (i, (_, w)) in weighted.iter().enumerate() {
        if at < *w {
            return i;
        }
        at -= w;
    }
    weighted.len() - 1
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::run::{JournalRead, Status};
    use crate::schema::ENTITY_ID_FORMAT;
    use crate::script::SessionConfig;

    const OBSERVATION: &str = "tick 0 (0.00s) | paused | speed 1x | seed 42 | you e5\n\
                               YOU  e5 Player  hp 100/100\n\
                               SEEN (by threat)\n  e6 Boss \"Frost Warden\"  hp 4000/4000  18.0m N\n  e7 Skeleton  12.6m W\n\
                               SINCE seq 0\n";

    /// A game that answers every call and remembers them.
    #[derive(Default)]
    struct Recorder {
        calls: Vec<(String, Value)>,
        tick: u64,
        /// `step` halts at this tick.
        halt_at: Option<u64>,
        /// Tools that are refused.
        refuse: Vec<&'static str>,
        /// What `observe` answers instead of [`OBSERVATION`].
        observation: Option<String>,
    }

    impl Driver for Recorder {
        fn call(&mut self, _agent: &str, tool: &str, args: Value) -> Result<Outcome, String> {
            self.calls.push((tool.to_owned(), args.clone()));
            let mut out = match tool {
                "observe" => Outcome::ok(self.observation.clone().unwrap_or_else(|| OBSERVATION.to_owned())),
                "content_list" if args.get("kind").is_none() => Outcome::ok("content: 3 definitions in 2 files\n  prefab  1\n  region  2"),
                "content_list" if args["kind"] == "region" => Outcome::ok("region: 2 definitions\n  dunes  a.ron\n  wilds  b.ron"),
                "content_list" => Outcome::ok("prefab: 1 definitions\n  wolf  w.ron"),
                "replay_save" => Outcome::ok(format!("saved {}: 1 ticks", args["path"].as_str().unwrap())),
                _ if self.refuse.contains(&tool) => Outcome::err("refused"),
                _ => Outcome::ok("ok"),
            };
            if tool == "step" {
                self.tick += args["ticks"].as_u64().unwrap();
                if self.halt_at.is_some_and(|at| self.tick >= at) {
                    out.halted = Some(Halted { reason: "invariant x violated".into(), replay: Some("replays/x.json".into()) });
                }
            }
            out.tick = Some(self.tick);
            Ok(out)
        }

        fn session(&mut self, _: &str, _: &SessionConfig) -> Result<Outcome, String> {
            unreachable!()
        }

        fn status(&mut self) -> Result<Status, String> {
            unreachable!()
        }

        fn journal_since(&mut self, _: u64) -> Result<JournalRead, String> {
            unreachable!()
        }
    }

    fn tool(name: &str, scope: &str, kind: &str, schema: Value) -> ToolDesc {
        ToolDesc { name: name.into(), scope: scope.into(), kind: kind.into(), signature: format!("{name}()"), schema }
    }

    fn id() -> Value {
        json!({"type": ["integer", "string"], "format": ENTITY_ID_FORMAT})
    }

    fn tools() -> Vec<ToolDesc> {
        vec![
            tool("zap", "player", "action", json!({"type": "object", "properties": {"target": id(), "power": {"type": "integer", "minimum": 1, "maximum": 3}}, "required": ["target"]})),
            tool("aim", "player", "action", json!({"type": "object", "properties": {}})),
            tool("peek", "player", "query", json!({"type": "object", "properties": {}})),
            tool("spawn", "dev", "action", json!({"type": "object", "properties": {}})),
            tool("step", "dev", "control", json!({"type": "object", "properties": {}})),
        ]
    }

    fn plan(seed: u64) -> FuzzPlan {
        FuzzPlan { rounds: 40, ..FuzzPlan::new(seed) }
    }

    fn calls_of(plan: &FuzzPlan, tools: &[ToolDesc]) -> Vec<(String, Value)> {
        let mut game = Recorder::default();
        run(&mut game, plan, tools).unwrap();
        game.calls
    }

    #[test]
    fn the_alphabet_is_the_player_actions_sorted_by_name() {
        let tools = tools();
        let names: Vec<&str> = alphabet(&tools).iter().map(|t| t.name.as_str()).collect();
        assert_eq!(names, ["aim", "zap"]);
    }

    #[test]
    fn tools_are_read_from_the_ai_tools_answer() {
        let list = json!([{"name": "move", "scope": "player", "kind": "action", "signature": "move(dir)", "description": "d",
            "input_schema": {"type": "object", "properties": {"dir": {"type": "string"}}}}, {"oops": 1}]);
        let tools = ToolDesc::list_from_json(&list);
        assert_eq!(tools.len(), 1);
        assert!(tools[0].is_player_action() && tools[0].signature == "move(dir)");
        assert_eq!(tools[0].schema["properties"]["dir"]["type"], "string");
    }

    #[test]
    fn the_same_seed_and_tools_make_the_same_calls() {
        let tools = tools();
        let plan = FuzzPlan { chaos: 0.2, ..plan(7) };
        assert_eq!(calls_of(&plan, &tools), calls_of(&plan, &tools));
        assert_ne!(calls_of(&plan, &tools), calls_of(&FuzzPlan { seed: 8, ..plan.clone() }, &tools));
    }

    #[test]
    fn schema_inputs_play_every_player_action_with_arguments_of_its_schema() {
        let tools = tools();
        let plan = FuzzPlan { rounds: 300, invalid: 0.0, ..plan(3) };
        let calls = calls_of(&plan, &tools);
        let played: Vec<&(String, Value)> = calls.iter().filter(|(t, _)| !["observe", "step", "content_list"].contains(&t.as_str())).collect();
        assert!(played.iter().all(|(t, _)| t == "aim" || t == "zap"), "only player actions: {played:?}");
        let zaps: Vec<&Value> = played.iter().filter(|(t, _)| t == "zap").map(|(_, a)| a).collect();
        assert!(zaps.len() > 100 && played.iter().any(|(t, _)| t == "aim"));
        for args in &zaps {
            assert!(args["target"].is_string(), "{args}");
            assert!(args.get("power").is_none_or(|p| (1..=3).contains(&p.as_i64().unwrap())), "{args}");
        }
        assert!(zaps.iter().any(|a| a["target"] == "e6") && zaps.iter().any(|a| a["target"] == "e7"));
    }

    #[test]
    fn weights_tilt_the_choice_and_zero_drops_a_tool() {
        let tools = tools();
        let count = |weights: &str| {
            let plan = FuzzPlan { rounds: 400, weights: parse_weights(weights).unwrap(), invalid: 0.0, ..plan(5) };
            let calls = calls_of(&plan, &tools);
            (calls.iter().filter(|(t, _)| t == "zap").count(), calls.iter().filter(|(t, _)| t == "aim").count())
        };
        let (zap, aim) = count("zap=9,aim=1");
        assert!(zap > aim * 4, "{zap} zaps to {aim} aims");
        assert_eq!(count("aim=0").1, 0);
        assert!(count("aim=0").0 > 300);
        let mut game = Recorder::default();
        let weights = |text: &str| FuzzPlan { weights: parse_weights(text).unwrap(), ..plan(1) };
        assert!(run(&mut game, &weights("jump=2"), &tools).unwrap_err().contains("not a player action tool; use one of: aim, zap"));
        assert!(run(&mut game, &weights("aim=0,zap=0"), &tools).unwrap_err().contains("every weight is 0"));
        assert!(parse_weights("zap").unwrap_err().contains("write tool=number"));
        assert!(parse_weights("zap=x").unwrap_err().contains("not a whole number"));
    }

    #[test]
    fn a_share_of_the_calls_break_one_argument_on_purpose() {
        let tools = tools();
        let mut game = Recorder::default();
        let report = run(&mut game, &FuzzPlan { rounds: 500, invalid: 0.2, ..plan(9) }, &tools).unwrap();
        assert!((60..=140).contains(&report.invalid), "about 20% of 500, got {}", report.invalid);
        let mut game = Recorder::default();
        assert_eq!(run(&mut game, &FuzzPlan { rounds: 200, invalid: 0.0, ..plan(9) }, &tools).unwrap().invalid, 0);
    }

    #[test]
    fn the_classic_alphabet_makes_the_calls_of_the_first_fuzzer() {
        // What `shards fuzz --seed 1` drew before the alphabet came from schemas: an input from
        // `random_input`, then a step of 1 + below(max_ticks) ticks, from one generator.
        let ids = ids_in(OBSERVATION);
        let mut rng = SplitMix(1);
        let mut expected = Vec::new();
        for _ in 0..60 {
            let (tool, args) = random_input(&mut rng, &ids);
            expected.push((tool.to_owned(), args));
            expected.push(("step".to_owned(), json!({"ticks": 1 + rng.below(30)})));
        }
        let plan = FuzzPlan { alphabet: Alphabet::Classic, rounds: 60, ..FuzzPlan::new(1) };
        let calls: Vec<(String, Value)> = calls_of(&plan, &[]).into_iter().filter(|(t, _)| t != "observe").collect();
        assert_eq!(calls, expected);
        assert!(run(&mut Recorder::default(), &FuzzPlan { weights: parse_weights("move=2").unwrap(), ..plan }, &[]).unwrap_err().contains("--weights goes with"));
    }

    #[test]
    fn chaos_makes_developer_calls_in_the_share_asked_for() {
        let tools = tools();
        let mut game = Recorder::default();
        let report = run(&mut game, &FuzzPlan { rounds: 600, chaos: 0.1, ..plan(2) }, &tools).unwrap();
        assert!((30..=100).contains(&report.chaos_rounds), "{}", report.chaos_rounds);
        assert_eq!(report.inputs + report.chaos_rounds, 600);
        for name in ["snapshot_save", "snapshot_load", "regen", "content_reload", "save_game", "load_game", "pause", "speed", "heal"] {
            assert!(report.calls.contains_key(name), "{name} never called: {:?}", report.calls);
        }
        // The game was paused (the observation says so): resume would let real time run ticks.
        assert!(!report.calls.contains_key("resume"), "{:?}", report.calls);
        // Regions come from the game's content, snapshots go to a file of the run's own.
        let regens: Vec<&Value> = game.calls.iter().filter(|(t, _)| t == "regen").map(|(_, a)| a).collect();
        assert!(regens.iter().all(|a| ["dunes", "wilds"].contains(&a["region"].as_str().unwrap())), "{regens:?}");
        assert!(game.calls.iter().filter(|(t, _)| t == "snapshot_save").all(|(_, a)| a["path"] == "snapshots/fuzz-seed2.json"));
        assert_eq!(game.calls.iter().filter(|(t, a)| t == "content_list" && a["kind"] == "region").count(), 2, "once for the words, once for chaos");
        // A snapshot that could not be saved is not loaded.
        let mut game = Recorder { refuse: vec!["snapshot_save", "save_game"], ..Recorder::default() };
        run(&mut game, &FuzzPlan { rounds: 600, chaos: 0.1, ..plan(2) }, &tools).unwrap();
        assert!(game.calls.iter().all(|(t, _)| t != "snapshot_load" && t != "load_game"));
        // Without chaos, none of it.
        let mut game = Recorder::default();
        assert_eq!(run(&mut game, &plan(2), &tools).unwrap().chaos_rounds, 0);
    }

    #[test]
    fn chaos_puts_the_speed_back_to_what_it_was() {
        assert_eq!(observed_speed("tick 0 (0.00s) | paused | speed 0.5x | seed 42"), 0.5);
        assert_eq!(observed_speed("no header"), 1.0);
        let slow = OBSERVATION.replace("speed 1x", "speed 0.5x");
        let mut game = Recorder { observation: Some(slow), ..Recorder::default() };
        run(&mut game, &FuzzPlan { rounds: 300, chaos: 0.3, ..plan(6) }, &tools()).unwrap();
        let speeds: Vec<f64> = game.calls.iter().filter(|(t, _)| t == "speed").map(|(_, a)| a["x"].as_f64().unwrap()).collect();
        assert!(speeds.len() >= 2 && speeds.len().is_multiple_of(2), "{speeds:?}");
        assert!(speeds.chunks(2).all(|pair| pair[1] == 0.5), "each change is put back to 0.5: {speeds:?}");
    }

    #[test]
    fn a_halt_ends_the_run_and_names_its_replay() {
        let tools = tools();
        let mut game = Recorder { halt_at: Some(50), ..Recorder::default() };
        let report = run(&mut game, &FuzzPlan { save_replay: true, ..plan(4) }, &tools).unwrap();
        assert!(report.halted() && report.rounds < 40, "{report}");
        assert_eq!(report.replay.as_deref(), Some("replays/x.json"));
        assert_eq!(report.played.len() as u32, report.rounds);
        assert!(report.to_string().contains("the game halted after"), "{report}");
        // Nothing broke: the run saves a replay of its own.
        let mut game = Recorder::default();
        let report = run(&mut game, &FuzzPlan { save_replay: true, ..plan(4) }, &tools).unwrap();
        assert_eq!(report.ended, Ended::Done);
        assert_eq!(report.replay.as_deref(), Some("replays/fuzz-seed4.json"));
        // A refused step ends it too.
        let mut game = Recorder { refuse: vec!["step"], ..Recorder::default() };
        let report = run(&mut game, &plan(4), &tools).unwrap();
        assert_eq!((report.ended, report.rounds), (Ended::StepRefused("refused".into()), 1));
    }


    #[test]
    fn refusals_teach_the_names_they_list() {
        let mut words = vec!["pillar".to_owned()];
        learn_words(&mut words, "unknown prefab `humanoid`; known: camp_skeleton, den_guard, pillar");
        learn_words(&mut words, "unknown direction `x`; use N, NE, E or W");
        learn_words(&mut words, "nothing listed here, and 12 is no name");
        assert_eq!(words, ["pillar", "known", "camp_skeleton", "den_guard", "use", "NE", "or"]);
    }

    #[test]
    fn a_refused_return_of_a_round_trip_is_a_finding_and_other_refusals_are_named() {
        let tools = tools();
        for (refused, call) in [("snapshot_load", "snapshot_load {\"path\":\"snapshots/fuzz-seed2.json\"}"), ("load_game", "load_game")] {
            let mut game = Recorder { refuse: vec![refused], ..Recorder::default() };
            let report = run(&mut game, &FuzzPlan { rounds: 600, chaos: 0.1, save_replay: true, ..plan(2) }, &tools).unwrap();
            let Ended::ChaosBroke { call: broke, why } = &report.ended else { panic!("{refused}: {report}") };
            assert!(broke.starts_with(call.split_whitespace().next().unwrap()) && why == "refused", "{broke}: {why}");
            assert!(report.chaos_broke() && report.rounds < 600, "the run ends there: {report}");
            assert!(report.to_string().contains("was refused: refused"), "{report}");
            assert_eq!(report.played.len() as u32, report.rounds);
            assert!(report.replay.as_deref().is_some_and(|r| r.starts_with("replays/fuzz-seed2-tick")), "{:?}", report.replay);
        }
        // An earlier refusal is named beside the broken return.
        let mut game = Recorder { refuse: vec!["snapshot_load", "heal", "speed"], ..Recorder::default() };
        let report = run(&mut game, &FuzzPlan { rounds: 600, chaos: 0.3, ..plan(2) }, &tools).unwrap();
        assert!(report.chaos_broke(), "{report}");
        assert!(report.chaos_refused > 1 && report.to_string().contains("chaos calls refused, the first "), "{report}");
        assert!(report.first_chaos_refusal.is_some());
        // A refusal elsewhere is counted, and the first one is named in the summary.
        let mut game = Recorder { refuse: vec!["heal"], ..Recorder::default() };
        let report = run(&mut game, &FuzzPlan { rounds: 600, chaos: 0.1, ..plan(2) }, &tools).unwrap();
        assert_eq!(report.ended, Ended::Done);
        assert!(report.chaos_refused > 0, "{report:?}");
        assert!(report.first_chaos_refusal.as_deref().is_some_and(|f| f.starts_with("heal ") && f.ends_with(": refused")), "{:?}", report.first_chaos_refusal);
        assert!(report.to_string().contains("the first heal"), "{report}");
    }
    #[test]
    fn a_game_without_player_actions_is_told_what_to_do() {
        let err = run(&mut Recorder::default(), &plan(1), &[]).unwrap_err();
        assert!(err.contains("--alphabet classic"), "{err}");
    }
}
