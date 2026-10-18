//! What stopped a fuzz run, and when two stops are "the same" for minimisation.
//!
//! A shrunk session is only worth keeping if it breaks the same thing as the long one: the same
//! invariant, the same failing system, a heavy check at about the same tick, the same chaos round
//! trip. [`Stop::of`] reads that from how a run ended. The game's halt reason is the only text
//! there is (`invariant NAME violated at tick N`, `system NAME panicked at tick N`, `heavy check
//! failed at tick T`: the `headline` of each in `ai_core`), so it is parsed here in one place.

use crate::fuzz::Ended;

/// How far apart two heavy checks may be and still be the same stop: a shorter session reaches the
/// same defect at another tick.
pub const HEAVY_TICKS: u64 = 60;

/// What a run stopped on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    /// A rule (the game's or the engine's) was violated.
    Invariant { name: String, tick: u64 },
    /// A gameplay system (or another named part) failed.
    Failure { kind: String, name: String, tick: u64 },
    /// A heavy check found a twin that went another way.
    Heavy { tick: u64 },
    /// The second half of a chaos round trip (`snapshot_load`, `load_game`) was refused.
    Chaos { tool: String },
}

impl Stop {
    /// The stop a halt reason names; `None` for text this does not know.
    pub fn from_reason(reason: &str) -> Option<Stop> {
        let reason = reason.lines().next()?.trim();
        let (head, tick) = reason.rsplit_once(" at tick ")?;
        let tick: u64 = tick.split(|c: char| !c.is_ascii_digit()).next()?.parse().ok()?;
        if head == "heavy check failed" {
            return Some(Stop::Heavy { tick });
        }
        let words: Vec<&str> = head.split_whitespace().collect();
        match words.as_slice() {
            ["invariant", name, "violated"] => Some(Stop::Invariant { name: (*name).to_owned(), tick }),
            [kind, name, "panicked" | "failed"] => Some(Stop::Failure { kind: (*kind).to_owned(), name: (*name).to_owned(), tick }),
            _ => None,
        }
    }

    /// The stop a finished run ended on: a halt, or a chaos round trip that did not come back.
    pub fn of(ended: &Ended) -> Option<Stop> {
        match ended {
            Ended::Halted { reason, .. } => Stop::from_reason(reason),
            Ended::ChaosBroke { call, .. } => Some(Stop::Chaos { tool: call.split_whitespace().next()?.to_owned() }),
            Ended::Done | Ended::StepRefused(_) => None,
        }
    }

    /// Whether `other` is this same stop: the same rule or system by name, a heavy check within
    /// [`HEAVY_TICKS`], the same tool refused. The tick of a rule does not matter: a shorter
    /// session breaks it earlier.
    pub fn same_as(&self, other: &Stop) -> bool {
        match (self, other) {
            (Stop::Invariant { name: a, .. }, Stop::Invariant { name: b, .. }) => a == b,
            (Stop::Failure { kind: k1, name: a, .. }, Stop::Failure { kind: k2, name: b, .. }) => k1 == k2 && a == b,
            (Stop::Heavy { tick: a }, Stop::Heavy { tick: b }) => a.abs_diff(*b) <= HEAVY_TICKS,
            (Stop::Chaos { tool: a }, Stop::Chaos { tool: b }) => a == b,
            _ => false,
        }
    }

    /// A word for a file name: `invariant-action.resources_in_range`, `system-game--hits`, `heavy`,
    /// `chaos-snapshot_load`.
    pub fn slug(&self) -> String {
        match self {
            Stop::Invariant { name, .. } => format!("invariant-{name}"),
            Stop::Failure { kind, name, .. } => format!("{kind}-{}", name.replace("::", "--")),
            Stop::Heavy { .. } => "heavy".to_owned(),
            Stop::Chaos { tool } => format!("chaos-{tool}"),
        }
    }

    /// The first line of a header: `invariant action.resources_in_range`.
    pub fn title(&self) -> String {
        match self {
            Stop::Invariant { name, .. } => format!("invariant {name}"),
            Stop::Failure { kind, name, .. } => format!("{kind} {name}"),
            Stop::Heavy { .. } => "heavy check".to_owned(),
            Stop::Chaos { tool } => format!("chaos {tool} refused"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn halt_reasons_are_read_back_into_stops() {
        assert_eq!(
            Stop::from_reason("invariant action.resources_in_range violated at tick 212"),
            Some(Stop::Invariant { name: "action.resources_in_range".into(), tick: 212 })
        );
        assert_eq!(
            Stop::from_reason("system game::combat::apply_hits panicked at tick 50"),
            Some(Stop::Failure { kind: "system".into(), name: "game::combat::apply_hits".into(), tick: 50 })
        );
        assert_eq!(Stop::from_reason("heavy check failed at tick 90"), Some(Stop::Heavy { tick: 90 }));
        assert_eq!(Stop::from_reason("invariant hp violated at tick 7\nmore text"), Some(Stop::Invariant { name: "hp".into(), tick: 7 }));
        assert_eq!(Stop::from_reason("the game wandered off"), None);
        assert_eq!(Stop::from_reason(""), None);
    }

    #[test]
    fn a_run_that_ended_well_or_refused_a_step_has_no_stop() {
        assert_eq!(Stop::of(&Ended::Done), None);
        assert_eq!(Stop::of(&Ended::StepRefused("no".into())), None);
        let halted = Ended::Halted { reason: "heavy check failed at tick 60".into(), replay: None };
        assert_eq!(Stop::of(&halted), Some(Stop::Heavy { tick: 60 }));
        let chaos = Ended::ChaosBroke { call: "snapshot_load {\"path\":\"x\"}".into(), why: "differs".into() };
        assert_eq!(Stop::of(&chaos), Some(Stop::Chaos { tool: "snapshot_load".into() }));
    }

    #[test]
    fn the_same_stop_is_the_same_rule_system_tool_or_a_heavy_check_near_in_time() {
        let inv = |name: &str, tick| Stop::Invariant { name: name.into(), tick };
        assert!(inv("a", 10).same_as(&inv("a", 900)));
        assert!(!inv("a", 10).same_as(&inv("b", 10)));
        let fail = |name: &str| Stop::Failure { kind: "system".into(), name: name.into(), tick: 1 };
        assert!(fail("x").same_as(&fail("x")) && !fail("x").same_as(&fail("y")));
        assert!(Stop::Heavy { tick: 300 }.same_as(&Stop::Heavy { tick: 360 }));
        assert!(Stop::Heavy { tick: 300 }.same_as(&Stop::Heavy { tick: 240 }));
        assert!(!Stop::Heavy { tick: 300 }.same_as(&Stop::Heavy { tick: 361 }));
        let chaos = |tool: &str| Stop::Chaos { tool: tool.into() };
        assert!(chaos("snapshot_load").same_as(&chaos("snapshot_load")) && !chaos("snapshot_load").same_as(&chaos("load_game")));
        assert!(!inv("a", 1).same_as(&Stop::Heavy { tick: 1 }));
    }

    #[test]
    fn slugs_are_file_name_words() {
        assert_eq!(Stop::Invariant { name: "action.resources_in_range".into(), tick: 1 }.slug(), "invariant-action.resources_in_range");
        assert_eq!(Stop::Failure { kind: "system".into(), name: "game::hits".into(), tick: 1 }.slug(), "system-game--hits");
        assert_eq!(Stop::Heavy { tick: 1 }.slug(), "heavy");
    }
}
