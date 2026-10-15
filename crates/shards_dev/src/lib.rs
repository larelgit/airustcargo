//! # shards_dev — the developer menu and console (E11.1)
//!
//! Whatever a developer reaches in the game window, an agent reaches through `shards`, and the
//! other way round: the menu and the console call the same tools through
//! [`call_tool`](ai_core::call_tool), in the window as agent [`CONSOLE_AGENT`] (dev scope).
//!
//! - **Menu** (F10 in a window; the `dev_menu` tool lists it and runs its entries): the entries
//!   the engine's modules register for their own tools
//!   ([`AiAppExt::add_dev_entry`](ai_core::AiAppExt::add_dev_entry)) — time (pause, slow
//!   motion, snapshots), world (the region and its seed, the generator's checks for it and for
//!   200 seeds, colliders, tiles), combat (immortality, hitboxes), AI (NPC minds, the navmesh),
//!   net (links, simulated lag), saves (the check) — then a game's own from content
//!   ([`DevMenuDef`], `content/**/*.devmenu.ron`), such as the sandbox's jump to the boss's
//!   second phase (a snapshot it loads). A switch runs the line of its other state
//!   (`npc_overlay on=false` while it is on); an entry that needs a value fills the console.
//! - **Console** (the key left of 1 in a window; the `console` tool for an agent): a line as
//!   `shards act` takes it, `tool key=value …`, answered with the result and the call's journal,
//!   the lines typed before (Up, Down) and completion of tool and argument names from the tools'
//!   schemas (Tab). While the console or the menu is open, the input map's `dev` context is on
//!   top: the character hears no key ([`has_keyboard`]).
//!
//! ```ron
//! // content/dev/sandbox.devmenu.ron
//! { "boss_phase_2": (group: "sandbox", label: "Boss phase 2", run: "snapshot_load path=snapshots/boss_phase2.json") }
//! ```
//!
//! Tools: `dev_menu {run?, open?}` and `console {line?, open?}` (dev queries; `open` shows or hides
//! the panel in a window, as its key does). Neither is recorded in replays itself: the calls they make
//! are, as the calling agent's.

#[cfg(feature = "render")]
mod window;

#[cfg(feature = "render")]
pub use window::{DevKeys, DEV_CONTEXT};

use std::collections::VecDeque;

use ai_core::dev::{entry_line, parse_line, run_line};
use ai_core::tools::{register_tool, ToolKind};
use ai_core::{AgentSessions, AiTools, Call, CallOutcome, DevCommand, DevEntry, DevMenu, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shards_content::{ContentAppExt, ContentDb, ContentDef, Refs};
use shards_core::EngineAppExt;

/// Version of the engine crates (they share one).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The agent the window's console and menu call tools as (dev scope).
pub const CONSOLE_AGENT: &str = "console";

/// Lines of output the console keeps.
const LOG_LINES: usize = 200;
/// Lines typed before that Up brings back.
const HISTORY: usize = 100;

/// What `help` answers in the console.
pub const HELP: &str = "a line is a tool call as `shards act` takes it: `god`, `speed x=0.25`, `teleport id=e5 x=3 z=4`, \
                        `move_to target='{\"x\": 3, \"z\": 4}'`; Tab completes tool and argument names, Up and Down bring back \
                        lines, `tools` lists the tools, `clear` empties the console, F10 opens the menu";

/// A game's entry of the developer menu (content kind `devmenu`, `content/**/*.devmenu.ron`).
/// An id the engine uses already replaces the engine's entry.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct DevMenuDef {
    /// The menu's section. Default `game`.
    #[serde(default = "game_group")]
    pub group: String,
    /// What the menu shows: `Boss phase 2`.
    pub label: String,
    /// The line it runs, as `shards act` takes it: `snapshot_load path=snapshots/boss.json`.
    #[serde(default)]
    pub run: Option<String>,
    /// Or the line it puts in the console to be finished and run there: `goto_tile x= y=`.
    #[serde(default)]
    pub fill: Option<String>,
}

fn game_group() -> String {
    "game".into()
}

impl ContentDef for DevMenuDef {
    const KIND: &'static str = "devmenu";

    fn validate(&self, _: &str, refs: &mut Refs) {
        if self.label.trim().is_empty() {
            refs.error("`label` is empty: say what the entry does (`Boss phase 2`)");
        }
        if self.group.trim().is_empty() {
            refs.error("`group` is empty: name the menu's section, or leave it out for `game`");
        }
        let (field, line) = match (&self.run, &self.fill) {
            (Some(_), Some(_)) => return refs.error("give `run` or `fill`, not both"),
            (None, None) => {
                return refs.error("give `run` (a line it runs: `god`) or `fill` (a line finished in the console: `goto_tile x= y=`)");
            }
            (Some(line), None) => ("run", line),
            (None, Some(line)) => ("fill", line),
        };
        if let Err(why) = parse_line(line) {
            refs.error(format!("{field}: {why}"));
        }
    }
}

impl DevMenuDef {
    /// As an entry of the menu.
    pub fn entry(&self, id: &str) -> DevEntry {
        let command = match (&self.run, &self.fill) {
            (_, Some(line)) => DevCommand::Fill(line.clone()),
            (Some(line), None) => DevCommand::Run(line.clone()),
            (None, None) => DevCommand::Fill(String::new()),
        };
        DevEntry { group: self.group.clone(), id: id.to_owned(), label: self.label.clone(), command }
    }
}

/// The console and the menu in the window: open or not, the line being typed, what was typed
/// and answered. Not simulation state: no replay or snapshot holds it.
#[derive(Resource, Default, Debug)]
pub struct DevConsole {
    pub open: bool,
    pub menu_open: bool,
    /// The line being typed.
    pub input: String,
    /// Lines run, oldest first.
    pub history: Vec<String>,
    /// What the console shows, oldest first: `> god`, then its answer.
    pub log: VecDeque<String>,
    /// The line of `history` Up brought back; `None` while typing a new one.
    pub recall: Option<usize>,
    /// The menu's chosen entry.
    pub selected: usize,
    /// What Tab offered for the input last.
    pub offered: Vec<String>,
}

impl DevConsole {
    fn say(&mut self, text: &str) {
        for line in text.lines() {
            self.log.push_back(line.to_owned());
        }
        while self.log.len() > LOG_LINES {
            self.log.pop_front();
        }
    }
}

/// Whether the console or the menu has the keyboard: a game's own keys (its debug keys) wait
/// while it does.
pub fn has_keyboard(world: &World) -> bool {
    world.get_resource::<DevConsole>().is_some_and(|c| c.open || c.menu_open)
}

/// The developer menu and console: the `devmenu` content kind, the `dev_menu` and `console`
/// tools; with feature `render`, both in the window. Needs the engine core and content first;
/// `shards::EnginePlugins` adds it last, after every module has registered its entries.
pub struct DevPlugin;

impl Plugin for DevPlugin {
    fn build(&self, app: &mut App) {
        app.add_content::<DevMenuDef>().init_resource::<DevConsole>().add_engine_module("dev", VERSION);
        register_tool(app.world_mut(), "dev_menu", Scope::Dev, ToolKind::Query, dev_menu_tool);
        register_tool(app.world_mut(), "console", Scope::Dev, ToolKind::Query, console_tool);
        #[cfg(feature = "render")]
        window::plugin(app);
    }
}

/// The menu's entries: the engine's in the order they were registered, then the game's from
/// content by id; a game's entry with an engine entry's id takes its place.
pub fn menu_entries(world: &World) -> Vec<DevEntry> {
    let mut menu = world.get_resource::<DevMenu>().cloned().unwrap_or_default();
    if let Some(db) = world.get_resource::<ContentDb>() {
        for (id, def) in db.iter::<DevMenuDef>() {
            menu.add(def.entry(id));
        }
    }
    let mut groups: Vec<&str> = Vec::new();
    for entry in menu.entries() {
        if !groups.contains(&entry.group.as_str()) {
            groups.push(&entry.group);
        }
    }
    let rank = |e: &DevEntry| groups.iter().position(|g| *g == e.group).unwrap_or(usize::MAX);
    let mut entries = menu.entries().to_vec();
    // Stable: in a group, the order of registration.
    entries.sort_by_key(rank);
    entries
}

/// What the menu shows for an entry, for `agent`: the line it runs or fills, or why it does
/// nothing here (no such tool in this game, nothing for it to act on now).
pub fn entry_state(world: &mut World, agent: &str, entry: &DevEntry) -> Result<ai_core::DevLine, String> {
    let line = entry_line(world, agent, entry)?;
    let (tool, _) = parse_line(&line.line)?;
    if world.get_resource::<AiTools>().and_then(|t| t.get(&tool)).is_none() {
        return Err(format!("no tool `{tool}` in this game"));
    }
    Ok(line)
}

/// The entries the menu has, grouped, with what each does now (`dev_menu`).
fn listing(world: &mut World, agent: &str) -> String {
    let entries = menu_entries(world);
    let id_width = entries.iter().map(|e| e.id.len()).max().unwrap_or(0);
    let label_width = entries.iter().map(|e| e.label.chars().count()).max().unwrap_or(0);
    let mut out = vec![format!(
        "dev menu: {} entries (F10 in a window); `dev_menu run=<id>` runs one, `console line=...` runs any line",
        entries.len()
    )];
    let mut group = "";
    for entry in &entries {
        if entry.group != group {
            group = &entry.group;
            out.push(group.to_owned());
        }
        let does = match entry_state(world, agent, entry) {
            Ok(line) if line.run => line.line,
            Ok(line) => format!("{} (finished in the console)", line.line),
            Err(why) => format!("(not here: {why})"),
        };
        out.push(format!("  {:id_width$}  {:label_width$}  {does}", entry.id, entry.label));
    }
    out.join("\n")
}

/// The developer menu (F10 in a window): its entries by section, with the line each runs —
/// switches show the line of their other state. With `run`, does what choosing that entry
/// does, and answers as the console would.
#[derive(Deserialize, JsonSchema, Default)]
pub struct DevMenuArgs {
    /// An entry's id (the first column of the list): run it.
    pub run: Option<String>,
    /// Show (true) or hide (false) the menu in the game window, as F10 does.
    pub open: Option<bool>,
}

fn dev_menu_tool(In(call): In<Call<DevMenuArgs>>, world: &mut World) -> ToolResult {
    let agent = call.caller.agent.clone();
    if let Some(open) = call.args.open {
        let mut console = world.resource_mut::<DevConsole>();
        console.menu_open = open;
        if open {
            console.open = false;
        }
    }
    let Some(id) = call.args.run else { return Ok(listing(world, &agent)) };
    let entries = menu_entries(world);
    let Some(entry) = entries.iter().find(|e| e.id == id) else {
        let ids: Vec<&str> = entries.iter().map(|e| e.id.as_str()).collect();
        return Err(format!("no entry `{id}` in the dev menu; entries: {}", ids.join(", ")));
    };
    let line = entry_state(world, &agent, entry).map_err(|why| format!("`{id}` does nothing here: {why}"))?;
    if !line.run {
        return Err(format!(
            "`{id}` needs values: run `{}` with them filled in (`console line=...`; in a window the menu puts it in the console)",
            line.line
        ));
    }
    run_console_line(world, &agent, &line.line)
}

/// Run a line in the developer console, as typed there: a tool call as `shards act` takes it
/// (`god`, `speed x=0.25`, `teleport id=e5 x=3 z=4`), or `help`, `clear`. Answers with the
/// result and the call's journal; the window's console shows both.
#[derive(Deserialize, JsonSchema, Default)]
pub struct ConsoleArgs {
    /// The line.
    pub line: Option<String>,
    /// Show (true) or hide (false) the console in the game window, as its key does.
    pub open: Option<bool>,
}

fn console_tool(In(call): In<Call<ConsoleArgs>>, world: &mut World) -> ToolResult {
    if let Some(open) = call.args.open {
        let mut console = world.resource_mut::<DevConsole>();
        console.open = open;
        if open {
            console.menu_open = false;
        }
    }
    match (call.args.line, call.args.open) {
        (Some(line), _) => submit(world, &call.caller.agent, &line),
        (None, Some(open)) => Ok(format!("console {}", if open { "open" } else { "closed" })),
        (None, None) => Err("give `line` (a tool call: `god`, `speed x=0.25`) or `open`".into()),
    }
}

/// Runs a console line for `agent` as typed: kept in the history, written into the console with
/// its answer.
pub fn submit(world: &mut World, agent: &str, line: &str) -> ToolResult {
    let line = line.trim().to_owned();
    {
        let mut console = world.resource_mut::<DevConsole>();
        if !line.is_empty() && console.history.last() != Some(&line) {
            console.history.push(line.clone());
            if console.history.len() > HISTORY {
                console.history.remove(0);
            }
        }
        console.recall = None;
        console.offered.clear();
        console.say(&format!("> {line}"));
    }
    let answer = match line.as_str() {
        "help" => Ok(HELP.to_owned()),
        "clear" => {
            world.resource_mut::<DevConsole>().log.clear();
            return Ok("cleared".into());
        }
        _ => run_console_line(world, agent, &line),
    };
    let shown = match &answer {
        Ok(text) => text.clone(),
        Err(why) => format!("error: {why}"),
    };
    world.resource_mut::<DevConsole>().say(&shown);
    answer
}

/// Runs `line` for `agent` and answers as the console shows it: the result, then the journal
/// lines of the call, indented.
pub fn run_console_line(world: &mut World, agent: &str, line: &str) -> ToolResult {
    let (tool, _) = parse_line(line)?;
    if tool == "console" || tool == "dev_menu" {
        return Err(format!("`{tool}` runs lines itself: type the tool you want (`god`, `region_info`)"));
    }
    // The console has dev scope whatever a new agent gets by default.
    if agent == CONSOLE_AGENT {
        world.resource_mut::<AgentSessions>().get_or_create(CONSOLE_AGENT).scope = Scope::Dev;
    }
    let outcome = run_line(world, agent, line)?;
    let text = outcome_text(&outcome);
    if outcome.ok { Ok(text) } else { Err(text) }
}

/// A call's answer as text: its result (JSON pretty-printed), then its journal lines.
fn outcome_text(outcome: &CallOutcome) -> String {
    let mut text = match &outcome.result {
        Value::String(s) => s.clone(),
        Value::Null => "ok".into(),
        other => serde_json::to_string_pretty(other).unwrap_or_else(|_| other.to_string()),
    };
    for line in &outcome.journal {
        text.push_str("\n  ");
        text.push_str(line);
    }
    text
}

/// Tab in the console: completes the last word of `input` from what fits there — a tool's name
/// first, then the names of its arguments not given yet (`key=`). Returns the input after it and
/// what fits; of several, the start they share is filled in.
pub fn complete(world: &World, input: &str) -> (String, Vec<String>) {
    let Some(tools) = world.get_resource::<AiTools>() else { return (input.to_owned(), Vec::new()) };
    let (head, last) = match input.rfind(char::is_whitespace) {
        Some(at) => input.split_at(at + 1),
        None => ("", input),
    };
    let first_word = head.trim().is_empty();
    let candidates: Vec<String> = if first_word {
        tools.names(Scope::Dev).into_iter().filter(|name| name.starts_with(last)).collect()
    } else if last.contains('=') {
        Vec::new()
    } else {
        let tool = head.split_whitespace().next().unwrap_or_default();
        let given: Vec<&str> = head.split_whitespace().skip(1).filter_map(|w| w.split_once('=').map(|(k, _)| k)).collect();
        let keys = tools.get(tool).and_then(|t| t.spec.input_schema.get("properties").and_then(Value::as_object).map(|p| p.keys().cloned().collect::<Vec<_>>()));
        keys.unwrap_or_default().into_iter().filter(|k| k.starts_with(last) && !given.contains(&k.as_str())).map(|k| format!("{k}=")).collect()
    };
    let completed = match candidates.as_slice() {
        [] => last.to_owned(),
        [only] if first_word => format!("{only} "),
        [only] => only.clone(),
        several => shared_start(several),
    };
    (format!("{head}{completed}"), candidates)
}

/// The longest start all of `words` share.
fn shared_start(words: &[String]) -> String {
    let first = &words[0];
    let mut end = first.len();
    for word in &words[1..] {
        end = first.char_indices().zip(word.chars()).take_while(|((_, a), b)| a == b).map(|((i, a), _)| i + a.len_utf8()).last().unwrap_or(0).min(end);
    }
    first[..end].to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn several_words_share_their_start() {
        let words = |w: &[&str]| w.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(shared_start(&words(&["nav_info", "nav_overlay", "nav_path"])), "nav_");
        assert_eq!(shared_start(&words(&["god", "goto_tile"])), "go");
        assert_eq!(shared_start(&words(&["heal", "speed"])), "");
    }

    #[test]
    fn a_game_entry_runs_or_fills_its_line() {
        let def = |run: Option<&str>, fill: Option<&str>| DevMenuDef {
            group: "game".into(),
            label: "x".into(),
            run: run.map(Into::into),
            fill: fill.map(Into::into),
        };
        assert!(matches!(def(Some("god"), None).entry("a").command, DevCommand::Run(ref l) if l == "god"));
        assert!(matches!(def(None, Some("goto_tile x= y=")).entry("a").command, DevCommand::Fill(ref l) if l == "goto_tile x= y="));
    }
}
