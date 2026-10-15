//! The developer menu's entries and the console's lines: both are tool calls written the way
//! `shards act` takes them (`god`, `speed x=0.25`, `npc_overlay on=false`), run through
//! [`call_tool`], so the menu, the console and an agent reach the same thing the same way.
//!
//! Each module registers the entries of its own tools
//! ([`AiAppExt::add_dev_entry`](crate::AiAppExt::add_dev_entry)); `shards_dev` shows them in
//! the game window, lists them with `dev_menu` and adds a game's own from content.

use bevy::prelude::*;
use serde_json::Value;

use crate::session::{caller, Caller};
use crate::tools::{call_tool, CallOutcome};

/// What a menu entry does.
#[derive(Clone, Debug)]
pub enum DevCommand {
    /// Runs this line.
    Run(String),
    /// Puts this line in the console to be finished (an id, a path) and run from there.
    Fill(String),
    /// Runs the line this gives now, for the caller: a toggle (the overlay's opposite state),
    /// a call about the region loaded now. `Err` says why the entry does nothing here.
    Live(fn(&World, &Caller) -> Result<String, String>),
}

/// One entry of the developer menu.
#[derive(Clone, Debug)]
pub struct DevEntry {
    /// The menu's section: `world`, `combat`, `ai`, `net`, `saves`, `time`, or a game's own.
    pub group: String,
    /// Unique in the menu: `god`, `npc_overlay`. A later entry with the same id replaces it.
    pub id: String,
    /// What the menu shows: `Immortality`.
    pub label: String,
    pub command: DevCommand,
}

/// The line an entry stands for now, and whether choosing it runs that line (`false`: it goes
/// to the console to be finished).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DevLine {
    pub line: String,
    pub run: bool,
}

impl DevEntry {
    /// What choosing the entry does for `caller` now, or why it does nothing here.
    pub fn line(&self, world: &World, caller: &Caller) -> Result<DevLine, String> {
        match &self.command {
            DevCommand::Run(line) => Ok(DevLine { line: line.clone(), run: true }),
            DevCommand::Fill(line) => Ok(DevLine { line: line.clone(), run: false }),
            DevCommand::Live(make) => make(world, caller).map(|line| DevLine { line, run: true }),
        }
    }
}

/// The entries the engine's modules registered, in order.
#[derive(Resource, Default, Clone, Debug)]
pub struct DevMenu {
    entries: Vec<DevEntry>,
}

impl DevMenu {
    /// Adds `entry`, or replaces the one with its id where that one stands.
    pub fn add(&mut self, entry: DevEntry) {
        match self.entries.iter_mut().find(|e| e.id == entry.id) {
            Some(old) => *old = entry,
            None => self.entries.push(entry),
        }
    }

    pub fn entries(&self) -> &[DevEntry] {
        &self.entries
    }
}

/// Whether the console or the menu has the keyboard (`shards_dev` keeps it): the engine's
/// debug keys (the overlays' F2, F6, F7) wait while it does, so a key typed into a panel
/// toggles nothing.
#[derive(Resource, Default, Clone, Copy, Debug, PartialEq, Eq)]
pub struct DevKeyboard(pub bool);

/// A run condition for a debug key's system: no panel has the keyboard.
pub fn keys_free(taken: Option<Res<DevKeyboard>>) -> bool {
    !taken.is_some_and(|t| t.0)
}

/// The line of a switch's entry: `{tool} on=false` while it is `on`, else `{tool} on=true`.
pub fn toggle_line(tool: &str, on: bool) -> String {
    format!("{tool} on={}", !on)
}

/// The engine core's entries: time and snapshots.
pub(crate) fn register(app: &mut App) {
    use crate::AiAppExt;
    let run = |line: &str| DevCommand::Run(line.into());
    app.init_resource::<DevKeyboard>();
    app.add_dev_entry("time", "pause", "Pause / resume", DevCommand::Live(|w, _| Ok(if crate::sim::is_paused(w) { "resume" } else { "pause" }.into())))
        .add_dev_entry("time", "step", "One tick (while paused)", run("step ticks=1"))
        .add_dev_entry("time", "slow", "Slow motion x0.25", run("speed x=0.25"))
        .add_dev_entry("time", "normal_speed", "Normal speed", run("speed x=1"))
        .add_dev_entry("time", "snapshot_save", "Save a snapshot of this tick", run("snapshot_save"))
        .add_dev_entry("time", "snapshot_load", "Load a snapshot...", DevCommand::Fill("snapshot_load path=snapshots/".into()));
}

/// A line as `shards act` takes it: the tool, then `key=value` words or one JSON object.
pub fn parse_line(line: &str) -> Result<(String, Value), String> {
    let words = shards_script::split_words(line)?.words;
    let Some((tool, rest)) = words.split_first() else {
        return Err("an empty line: type a tool and its arguments, like `god` or `speed x=0.25` (`tools` lists them)".into());
    };
    Ok((tool.clone(), shards_script::parse_args(rest)?))
}

/// Runs `line` as `agent` through [`call_tool`], or says why it is not a call.
pub fn run_line(world: &mut World, agent: &str, line: &str) -> Result<CallOutcome, String> {
    let (tool, args) = parse_line(line)?;
    Ok(call_tool(world, agent, &tool, args))
}

/// What choosing `entry` does for `agent` now (see [`DevEntry::line`]).
pub fn entry_line(world: &mut World, agent: &str, entry: &DevEntry) -> Result<DevLine, String> {
    let caller = caller(world, agent);
    entry.line(world, &caller)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn a_line_is_a_tool_and_its_arguments() {
        assert_eq!(parse_line("god").unwrap(), ("god".into(), json!({})));
        assert_eq!(parse_line("speed x=0.25").unwrap(), ("speed".into(), json!({"x": 0.25})));
        assert_eq!(
            parse_line(r#"move_to target='{"x": 3, "z": 4}' path=true"#).unwrap(),
            ("move_to".into(), json!({"target": {"x": 3, "z": 4}, "path": true}))
        );
        assert_eq!(parse_line(r#"step '{"ticks": 60}'"#).unwrap(), ("step".into(), json!({"ticks": 60})));
        assert!(parse_line("   ").unwrap_err().starts_with("an empty line"));
        assert_eq!(parse_line("teleport x").unwrap_err(), "expected key=value, got `x`");
        assert_eq!(parse_line("say text='hi").unwrap_err(), "unclosed `'` quote");
    }

    #[test]
    fn an_entry_with_the_same_id_replaces_the_first_where_it_stood() {
        let entry = |id: &str, line: &str| DevEntry {
            group: "time".into(),
            id: id.into(),
            label: id.into(),
            command: DevCommand::Run(line.into()),
        };
        let mut menu = DevMenu::default();
        menu.add(entry("slow", "speed x=0.25"));
        menu.add(entry("normal", "speed x=1"));
        menu.add(entry("slow", "speed x=0.1"));
        let lines: Vec<String> = menu
            .entries()
            .iter()
            .map(|e| match &e.command {
                DevCommand::Run(line) => line.clone(),
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(lines, ["speed x=0.1", "speed x=1"]);
    }
}
