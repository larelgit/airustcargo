//! Runtime state of sequences: the runs of the world and the hold on the NPCs that act in them.
//!
//! [`Sequences`] is a snapshot resource, present once a sequence has been played (as `EncounterRuns`
//! is): a game that never plays one has the snapshots and replay hashes it had before this module
//! existed. Timers are whole ticks (`u32`), not seconds: a sum of thousands of `f32` tick lengths
//! drifts past the engine's epsilon, a count of ticks does not.

use ai_core::AiId;
use bevy::prelude::*;

/// The number of a run of a sequence (`play_sequence` returns it, `sequence_stop` takes it). Numbers
/// ascend and are never used twice in a world; runs are visited in this order.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SequenceId(pub u32);

impl std::fmt::Display for SequenceId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.0.fmt(f)
    }
}

/// Every run of this world, and the number the next one takes. Simulation state (snapshots hold it, the
/// replay check `sequences` hashes it); created by the first `play_sequence`.
#[derive(Resource, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Resource, Default)]
pub struct Sequences {
    /// The number the next run takes (0 is never used: it counts from 1).
    pub next: u32,
    /// The runs playing, ascending by number.
    pub runs: Vec<Run>,
}

impl Sequences {
    /// The run numbered `id`.
    pub fn get(&self, id: SequenceId) -> Option<&Run> {
        self.runs.iter().find(|r| r.id == id.0)
    }

    pub(crate) fn index(&self, id: u32) -> Option<usize> {
        self.runs.iter().position(|r| r.id == id)
    }

    /// The run `body` acts in, if it acts in one.
    pub fn holding(&self, body: AiId) -> Option<&Run> {
        self.runs.iter().find(|r| r.actors.contains(&body))
    }
}

/// One run of a sequence.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
pub struct Run {
    /// Its number ([`SequenceId`]).
    pub id: u32,
    /// The `sequence` definition it plays.
    pub def: String,
    /// Role name to body, ascending by role.
    pub roles: Vec<(String, AiId)>,
    /// The bodies some step acts on: held, their brains dormant. Ascending by `AiId`.
    pub actors: Vec<AiId>,
    /// The step under way (0-based).
    pub step: u32,
    /// The step has begun: what it does once (forcing an action, a sound, a signal) has been done, so
    /// a game loaded in the middle of it does not do it twice.
    pub begun: bool,
    /// Ticks the step has run.
    pub age: u32,
    /// A `Wait`'s length in ticks.
    pub left: u32,
    /// A `Play`'s run of the action (`ActionState::started` when it began): the step ends when that run is over.
    pub instance: Option<u32>,
    /// Where the step's marker is (`MoveTo`), looked up when the step began.
    pub point: Option<Vec3>,
    /// What each actor faces, by role: kept after a `Face` step until it faces something else.
    pub faces: Vec<(String, Facing)>,
    /// The tick it was played at.
    pub started: u64,
}

impl Run {
    /// The body of role `role`.
    pub fn body(&self, role: &str) -> Option<AiId> {
        self.roles.iter().find(|(r, _)| r == role).map(|(_, id)| *id)
    }

    /// The role `body` plays.
    pub fn role_of(&self, body: AiId) -> Option<&str> {
        self.roles.iter().find(|(_, id)| *id == body).map(|(r, _)| r.as_str())
    }

    pub(crate) fn face_of(&self, role: &str) -> Option<Facing> {
        self.faces.iter().find(|(r, _)| r == role).map(|(_, f)| *f)
    }

    pub(crate) fn set_face(&mut self, role: &str, facing: Option<Facing>) {
        self.faces.retain(|(r, _)| r != role);
        if let Some(facing) = facing {
            self.faces.push((role.to_owned(), facing));
            self.faces.sort_by(|a, b| a.0.cmp(&b.0));
        }
    }
}

/// What an actor keeps facing.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub enum Facing {
    /// A place (a marker's).
    Point(Vec3),
    /// A body, followed as it moves.
    Body(AiId),
}

impl Default for Facing {
    fn default() -> Self {
        Facing::Point(Vec3::ZERO)
    }
}

/// On a body some step of a run acts on, for as long as the run plays: its brain is dormant (it does
/// not choose, ask for attack tokens or press) and the run drives it. State on an entity with an
/// `AiId`: snapshots hold it, and a game loaded mid-sequence resumes both.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct SequenceHold {
    /// The run's number.
    pub run: u32,
    /// The `sequence` definition.
    pub def: String,
    /// The role the body plays in it.
    pub role: String,
}

/// A sequence began (journaled: `sequence 3 (greeting) starts: guard=e7 visitor=e1`).
#[derive(Event, Clone, Debug, PartialEq)]
pub struct SequenceStarted {
    pub sequence: SequenceId,
    pub def: String,
    pub roles: Vec<(String, AiId)>,
}

/// A step began (journaled: `sequence 3 (greeting) 2/7: e7 turns to the visitor`). Triggered on the
/// tick it begins, in the simulation.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct SequenceStep {
    pub sequence: SequenceId,
    pub def: String,
    /// 0-based.
    pub index: u32,
    /// The steps in all.
    pub of: u32,
    /// `Play`, `Wait`, ...
    pub kind: &'static str,
    /// The body the step acts on.
    pub actor: Option<AiId>,
    /// What the step does, in words.
    pub text: String,
}

/// A `Signal` step: the sequence tells the game something happened (a game reacts with an observer,
/// and returns at once unless `shards::core::simulates`).
#[derive(Event, Clone, Debug, PartialEq)]
pub struct SequenceSignal {
    pub sequence: SequenceId,
    pub def: String,
    pub name: String,
}

/// A run is over: its last step finished (`skipped: false`), or it was stopped (`stop_sequence`) or
/// abandoned (a body gone, a step timed out: `why` says) before it did. The NPCs it held are free.
#[derive(Event, Clone, Debug, PartialEq)]
pub struct SequenceEnded {
    pub sequence: SequenceId,
    pub def: String,
    /// The step it reached (0-based): the number of steps for a run that finished.
    pub step: u32,
    /// The steps in all.
    pub of: u32,
    pub skipped: bool,
    /// Why it did not finish; `None` for a finished run and for one a game stopped.
    pub why: Option<String>,
}
