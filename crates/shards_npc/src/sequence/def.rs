//! The `sequence` content kind: a scripted scene for NPCs, as data.

use serde::{Deserialize, Serialize};
use shards_action::ActionDef;
use shards_content::db::closest;
use shards_content::{ContentDef, Refs};

/// A sequence (content kind `sequence`): the **roles** a game binds to bodies when it plays it, and
/// the **steps** that run one after another, each on a named role's NPC. Steps run in the simulation;
/// the next begins on the tick the last one finishes.
///
/// ```ron
/// // content/scenes/greeting.sequence.ron
/// {
///     "greeting": (
///         roles: ["guard", "visitor"],
///         steps: [
///             MoveTo(role: "guard", marker: "lectern", speed: 0.5),
///             Face(role: "guard", at: Role("visitor")),
///             Play(role: "guard", action: "wave"),
///             Sound(role: "guard", sound: "guard_greeting"),
///             Wait(secs: 1.5),
///             WaitFor(condition: "mover_arrived"),
///             Signal(name: "greeted"),
///         ],
///     ),
/// }
/// ```
///
/// A role is **bound** when the game calls `play_sequence`: to an NPC if some step acts on it
/// (`MoveTo`, `Face`, `Play`, `Sound`), to any body if the steps only look at it (the player, as
/// `Face(at: Role("visitor"))` does). The brains of the NPCs that act are dormant while it runs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct SequenceDef {
    /// The roles the steps name; the game binds each to a body at `play_sequence`. Every role is
    /// named by some step, and every step's role is here.
    pub roles: Vec<String>,
    /// What happens, in order.
    pub steps: Vec<Step>,
}

/// One step of a sequence ([`SequenceDef::steps`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub enum Step {
    /// The role's NPC walks to a level marker (its name), by the NPC's own route (the navmesh,
    /// avoiding bodies), and the step ends when it is within `within` meters. The marker is looked up
    /// when the step begins.
    MoveTo {
        role: String,
        /// The marker's name (`LevelMarker::name`).
        marker: String,
        /// Arrived within this many meters (default 0.5).
        #[serde(default = "default_within", skip_serializing_if = "is_default_within")]
        within: f32,
        /// The movement input: 0.5 walks, 1 runs (default 1).
        #[serde(default = "default_speed", skip_serializing_if = "is_default_speed")]
        speed: f32,
        /// Seconds before the step gives up and the sequence ends skipped (default 60).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<f32>,
    },
    /// The role's NPC turns to face a marker or another role's body, and keeps facing it (a body
    /// follows as it moves) until it faces something else or the sequence ends. The step ends when it
    /// faces it, within 5 degrees.
    Face {
        role: String,
        at: FaceTarget,
        /// Seconds before the step gives up and the sequence ends skipped (default 5).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<f32>,
    },
    /// The role's NPC runs an action (`ActionDef`) and the step ends when it has ended (finished,
    /// or cut short by a stagger).
    Play {
        role: String,
        action: String,
        /// Seconds before the step gives up and the sequence ends skipped (default 60).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<f32>,
    },
    /// The role's body says a sound: the audio module plays `sound` from it, with its subtitle and
    /// speaker (the sound's own definition). The step ends at once: follow it with a `Wait` as long
    /// as the line. Audio is a machine's, not the simulation's, so it never waits for a sound to end.
    Sound { role: String, sound: String },
    /// Nothing happens for `secs` seconds (counted in ticks, rounded to the nearest).
    Wait { secs: f32 },
    /// Nothing happens until the named condition holds (`add_ai_condition`: the names `step --until
    /// named:` takes; `!name` for its opposite).
    WaitFor {
        condition: String,
        /// Seconds before the step gives up and the sequence ends skipped (default: it waits for ever).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout: Option<f32>,
    },
    /// Tells the game something happened: the event `SequenceSignal { name }`, a journal line.
    Signal { name: String },
}

/// What a `Face` step faces.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum FaceTarget {
    /// A level marker, by name.
    Marker(String),
    /// Another role's body.
    Role(String),
}

fn default_within() -> f32 {
    0.5
}

fn is_default_within(within: &f32) -> bool {
    *within == default_within()
}

fn default_speed() -> f32 {
    1.0
}

fn is_default_speed(speed: &f32) -> bool {
    *speed == default_speed()
}

/// Seconds a `MoveTo` or a `Play` waits when its definition names no `timeout`.
pub const DEFAULT_LONG_TIMEOUT: f32 = 60.0;
/// Seconds a `Face` waits when its definition names no `timeout`.
pub const DEFAULT_FACE_TIMEOUT: f32 = 5.0;
/// How near the facing must be to the direction (degrees) for a `Face` to be done.
pub const FACE_TOLERANCE_DEG: f32 = 5.0;

impl Step {
    /// The role it acts on, if it acts on one.
    pub fn actor(&self) -> Option<&str> {
        match self {
            Step::MoveTo { role, .. } | Step::Face { role, .. } | Step::Play { role, .. } | Step::Sound { role, .. } => Some(role),
            Step::Wait { .. } | Step::WaitFor { .. } | Step::Signal { .. } => None,
        }
    }

    /// Every role it names, the actor and a role it faces.
    pub fn roles(&self) -> impl Iterator<Item = &str> {
        let faced = match self {
            Step::Face { at: FaceTarget::Role(r), .. } => Some(r.as_str()),
            _ => None,
        };
        self.actor().into_iter().chain(faced)
    }

    /// `MoveTo`, `Face`, ...
    pub fn kind(&self) -> &'static str {
        match self {
            Step::MoveTo { .. } => "MoveTo",
            Step::Face { .. } => "Face",
            Step::Play { .. } => "Play",
            Step::Sound { .. } => "Sound",
            Step::Wait { .. } => "Wait",
            Step::WaitFor { .. } => "WaitFor",
            Step::Signal { .. } => "Signal",
        }
    }

    /// The seconds the step may take, if it has a limit.
    pub fn limit(&self) -> Option<f32> {
        match self {
            Step::MoveTo { timeout, .. } | Step::Play { timeout, .. } => Some(timeout.unwrap_or(DEFAULT_LONG_TIMEOUT)),
            Step::Face { timeout, .. } => Some(timeout.unwrap_or(DEFAULT_FACE_TIMEOUT)),
            Step::WaitFor { timeout, .. } => *timeout,
            Step::Wait { .. } | Step::Sound { .. } | Step::Signal { .. } => None,
        }
    }

    /// What the step does, in a few words, for the journal: `e7 walks to `lectern``.
    pub fn text(&self, who: &str) -> String {
        match self {
            Step::MoveTo { marker, speed, .. } => format!("{who} {} to `{marker}`", if *speed <= 0.5 { "walks" } else { "runs" }),
            Step::Face { at: FaceTarget::Marker(m), .. } => format!("{who} turns to `{m}`"),
            Step::Face { at: FaceTarget::Role(r), .. } => format!("{who} turns to the {r}"),
            Step::Play { action, .. } => format!("{who} plays `{action}`"),
            Step::Sound { sound, .. } => format!("{who} says `{sound}`"),
            Step::Wait { secs } => format!("waits {secs:.1} s"),
            Step::WaitFor { condition, .. } => format!("waits for `{condition}`"),
            Step::Signal { name } => format!("signals `{name}`"),
        }
    }
}

impl ContentDef for SequenceDef {
    const KIND: &'static str = "sequence";

    fn validate(&self, id: &str, refs: &mut Refs) {
        let _ = id;
        if self.steps.is_empty() {
            refs.error("`steps` is empty: a sequence with nothing to do (MoveTo, Face, Play, Sound, Wait, WaitFor, Signal)");
        }
        for (i, role) in self.roles.iter().enumerate() {
            if role.is_empty() || role.chars().any(|c| c.is_whitespace() || c == '.' || c == '=') {
                refs.error(format!("roles[{i}] `{role}`: a role is a word with no blanks, dots or `=` (`role.NAME=eN` binds it)"));
            }
            if self.roles[..i].contains(role) {
                refs.error(format!("roles[{i}] `{role}` is declared twice"));
            }
        }
        let mut used = vec![false; self.roles.len()];
        for (i, step) in self.steps.iter().enumerate() {
            let at = format!("steps[{i}] {}", step.kind());
            for role in step.roles() {
                match self.roles.iter().position(|r| r == role) {
                    Some(n) => used[n] = true,
                    None => {
                        let hint = closest(role, self.roles.iter().map(String::as_str)).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                        let declared = if self.roles.is_empty() { "none".to_owned() } else { self.roles.join(", ") };
                        refs.error(format!("{at}: role `{role}` is not declared in `roles`{hint}; declared: {declared}"));
                    }
                }
            }
            check_step(step, &at, refs);
            if let Some(limit) = step.limit()
                && !(limit > 0.0 && limit.is_finite())
            {
                refs.error(format!("{at}: `timeout` must be a number of seconds above 0 (is {limit})"));
            }
        }
        for (role, used) in self.roles.iter().zip(used) {
            if !used {
                refs.error(format!("role `{role}` is declared but no step names it; leave it out of `roles`"));
            }
        }
    }
}

fn check_step(step: &Step, at: &str, refs: &mut Refs) {
    let empty = |refs: &mut Refs, field: &str, value: &str| {
        let bad = value.trim().is_empty();
        if bad {
            refs.error(format!("{at}: `{field}` is empty"));
        }
        bad
    };
    match step {
        Step::MoveTo { marker, within, speed, .. } => {
            empty(refs, "marker", marker);
            if !(within.is_finite() && *within >= 0.0) {
                refs.error(format!("{at}: `within` must be a number of meters, 0 or more (is {within})"));
            }
            if !(*speed > 0.0 && *speed <= 1.0) {
                refs.error(format!("{at}: `speed` is the movement input, above 0 and at most 1 (0.5 walks, 1 runs), not {speed}"));
            }
        }
        Step::Face { at: target, .. } => match target {
            FaceTarget::Marker(m) => {
                empty(refs, "at: Marker", m);
            }
            FaceTarget::Role(r) => {
                empty(refs, "at: Role", r);
            }
        },
        Step::Play { action, .. } => {
            if !empty(refs, "action", action) {
                refs.require::<ActionDef>(action, &format!("{at}.action"));
            }
        }
        Step::Sound { sound, .. } => {
            empty(refs, "sound", sound);
        }
        Step::Wait { secs } => {
            if !(secs.is_finite() && *secs >= 0.0) {
                refs.error(format!("{at}: `secs` must be a number of seconds, 0 or more (is {secs})"));
            }
        }
        Step::WaitFor { condition, .. } => {
            let name = condition.strip_prefix('!').unwrap_or(condition).trim();
            if name.is_empty() || name.starts_with('!') {
                refs.error(format!("{at}: `condition` names no condition (`mover_arrived`, `!npcs_calm`; one `!` at most), not `{condition}`"));
            }
        }
        Step::Signal { name } => {
            empty(refs, "name", name);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_definition_reads_from_ron() {
        let text = r#"(
            roles: ["guard", "visitor"],
            steps: [
                MoveTo(role: "guard", marker: "lectern", speed: 0.5),
                Face(role: "guard", at: Role("visitor")),
                Play(role: "guard", action: "wave"),
                Sound(role: "guard", sound: "hello"),
                Wait(secs: 1.5),
                WaitFor(condition: "!npcs_calm", timeout: 5),
                Signal(name: "done"),
            ],
        )"#;
        let def: SequenceDef = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME).from_str(text).unwrap();
        assert_eq!(def.steps.len(), 7);
        assert_eq!(def.steps[0], Step::MoveTo { role: "guard".into(), marker: "lectern".into(), within: 0.5, speed: 0.5, timeout: None });
        assert_eq!(def.steps[1].roles().collect::<Vec<_>>(), ["guard", "visitor"]);
        assert_eq!(def.steps[5].limit(), Some(5.0));
        assert_eq!(def.steps[2].limit(), Some(DEFAULT_LONG_TIMEOUT));
        assert_eq!(def.steps[4].limit(), None);
        // Defaults are left out when written back: a hash of a definition that never names them is unchanged.
        let written = ron::to_string(&def.steps[0]).unwrap();
        assert!(!written.contains("within") && !written.contains("timeout"), "{written}");
    }
}
