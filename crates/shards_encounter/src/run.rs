//! Runtime state: the runs of the world's encounters, and the marker on their members.
//!
//! [`EncounterRuns`] is a snapshot resource, present only while there is a run or a remembered
//! key (as `PlacedLevels` is): a game without an encounter has the snapshots and replay hashes it
//! had before this module existed. Timers are whole ticks (`u32`), not seconds: a sum of
//! thousands of `f32` tick lengths drifts past the engine's epsilon, a count of ticks does not.

use ai_core::AiId;
use bevy::prelude::*;

/// Every encounter run of this world, and the `once` encounters it has won. Present only while
/// one of the lists is not empty. Snapshot resource; replicated to clients.
#[derive(Resource, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Resource, Default)]
pub struct EncounterRuns {
    /// Sorted by key.
    pub runs: Vec<Run>,
    /// Keys of won `once` encounters, sorted; kept when their region unloads.
    pub completed: Vec<String>,
    /// Anchors the journal has already said are wrong (a duplicate key, an encounter that is not
    /// in content), sorted: each is said once.
    pub reported: Vec<String>,
}

impl EncounterRuns {
    /// The run keyed `key`.
    pub fn get(&self, key: &str) -> Option<&Run> {
        self.runs.iter().find(|r| r.key == key)
    }

    pub(crate) fn index(&self, key: &str) -> Result<usize, usize> {
        self.runs.binary_search_by(|r| r.key.as_str().cmp(key))
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.runs.is_empty() && self.completed.is_empty() && self.reported.is_empty()
    }

    pub(crate) fn complete(&mut self, key: &str) {
        if let Err(at) = self.completed.binary_search_by(|k| k.as_str().cmp(key)) {
            self.completed.insert(at, key.to_owned());
        }
    }

    pub(crate) fn report(&mut self, text: &str) -> bool {
        match self.reported.binary_search_by(|k| k.as_str().cmp(text)) {
            Ok(_) => false,
            Err(at) => {
                self.reported.insert(at, text.to_owned());
                true
            }
        }
    }
}

/// One encounter's run.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
pub struct Run {
    /// `<region>/<anchor key>`.
    pub key: String,
    /// `encounter` content id.
    pub encounter: String,
    /// The anchor's position when the run was made.
    pub at: Vec3,
    /// The anchor's yaw (radians) when the run was made.
    pub yaw: f32,
    pub state: RunState,
    /// Waves started: 0 before the first, `waves.len()` once the last started. A placed first
    /// wave counts as started from the run's making.
    pub wave: u32,
    /// In `AiId` order.
    pub members: Vec<Member>,
    /// Ticks in a row with no standing participant within the radius (fighting states only).
    pub absent: u32,
    /// The lock prefab while it stands.
    pub lock: Option<AiId>,
    /// One-time notes already journaled (a bit each, see `NOTE_*`).
    pub noted: u8,
}

/// One member of a run, or an add the game adopted.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq)]
pub struct Member {
    pub id: AiId,
    /// 1-based wave that spawned it; 0 for an adopted add.
    pub wave: u32,
    /// Counted members end their wave; adds never do.
    pub counted: bool,
}

/// Where a run is.
#[derive(Reflect, Clone, Debug, Default, PartialEq)]
pub enum RunState {
    /// Made; waits for its start condition to be false once, so that a run made under a player's
    /// feet (a load, a dev reset) does not start at once.
    #[default]
    Idle,
    /// Waits for its start condition.
    Armed,
    /// The members of wave `wave` are fighting.
    Running,
    /// Wave `wave + 1` spawns when `left` ticks have run out (the start, a cleared wave).
    Between { left: u32 },
    /// Every participant is out. `resume` counts down (in ticks) once one stands again.
    /// `pulled_back` is whether this wipe pulled back the placed first wave (its survivors are
    /// alive and home): the resume then restarts at the start condition instead of spawning.
    Wiped { resume: Option<u32>, pulled_back: bool },
    /// Survivors walk home; when `left` ticks have run out, the rest are placed there.
    Resetting { left: u32 },
    /// Won; final.
    Won,
}

impl RunState {
    /// The states in which a fight is on.
    pub fn fighting(&self) -> bool {
        matches!(self, RunState::Running | RunState::Between { .. } | RunState::Wiped { .. } | RunState::Resetting { .. })
    }

    /// The states in which the absence clock runs.
    pub fn absence_counts(&self) -> bool {
        matches!(self, RunState::Running | RunState::Between { .. } | RunState::Wiped { .. })
    }

    /// The state's name for tools and the journal.
    pub fn name(&self) -> &'static str {
        match self {
            RunState::Idle => "Idle",
            RunState::Armed => "Armed",
            RunState::Running => "Running",
            RunState::Between { .. } => "Between",
            RunState::Wiped { .. } => "Wiped",
            RunState::Resetting { .. } => "Resetting",
            RunState::Won => "Won",
        }
    }
}

/// On every member (and adopted add) of a run: which run, which wave, whether it counts.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct EncounterMember {
    pub key: String,
    pub wave: u32,
    pub counted: bool,
    /// Set by a pull-back until the member is home (`encounter_returning`).
    pub returning: bool,
}

/// Note bits of [`Run::noted`]: things the journal says once per run.
pub(crate) const NOTE_NO_VOLUME: u8 = 1;
pub(crate) const NOTE_NO_CANDIDATES: u8 = 2;
pub(crate) const NOTE_ALL_BEHIND: u8 = 4;
pub(crate) const NOTE_TOO_FEW: u8 = 8;
pub(crate) const NOTE_CONTENT: u8 = 16;
pub(crate) const NOTE_NOT_WALKABLE: u8 = 32;
