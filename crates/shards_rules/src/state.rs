//! The rules' runtime state: the resource [`ActiveRules`] (the rule set a world plays), the body
//! components [`RuleClass`] (authored) and [`RuleScaled`] (the rules' bookkeeping of a body), and the
//! registry [`SessionFilter`] (which player bodies count toward the session size).

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// The rule set a world plays, and the key bodies are brought to. Inserted only where content has
/// `rules/default` (a game without it never has one); a snapshot resource, replicated to clients.
/// Serde reads it too: the snapshot prepare `rules` reads it from a snapshot as the reflected JSON
/// it is written in.
#[derive(Resource, Reflect, Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[reflect(Resource, Default)]
pub struct ActiveRules {
    /// The rule set in play; empty: none (neutral: every hook reads 1, and bodies go back to their base).
    pub mode: String,
    /// Counts changes of what the rules say (a mode change, a content change, a load): a body scaled at
    /// another generation is brought to this one at the next apply.
    pub generation: u32,
    /// The session size bodies are scaled for: the number of distinct `PlayerId`s among the bodies seated in the
    /// session ([`SessionFilter`]; a body waiting for a rejoin is not), dead or downed ones included.
    pub players: u32,
}

impl ActiveRules {
    /// No rule set in play: every hook reads the engine's numbers.
    pub fn is_neutral(&self) -> bool {
        self.mode.is_empty()
    }

    /// The (generation, session size) a body is brought to.
    pub fn key(&self) -> RuleKey {
        RuleKey { generation: self.generation, players: self.players }
    }
}

/// The class the rules know this body by (`brute`). A body with `PlayerId` and no class is `player`;
/// a body with neither is untouched by the rules. Authored in prefabs (`"RuleClass": ("brute")`), never
/// written by the engine.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[reflect(Component, Default)]
pub struct RuleClass(pub String);

/// What the rules did to this body: the bases they scale from and the key of the last apply.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct RuleScaled {
    /// The (generation, session size) the body was last brought to; `None`: at the next apply.
    pub applied: Option<RuleKey>,
    /// `Health::max` without the rules, and the factor applied to it; `None`: health is not rule-owned.
    pub health: Option<Scaled>,
    /// The same for `Poise::max`.
    pub poise: Option<Scaled>,
    /// Charges a mode change could not take back yet; only entries above 0, sorted by name.
    pub owed: Vec<Owed>,
}

/// The (generation, session size) of [`ActiveRules`] a body was brought to.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RuleKey {
    pub generation: u32,
    pub players: u32,
}

/// A rule-owned maximum: the base the rules scale from and the factor applied to it.
#[derive(Reflect, Clone, Copy, Debug, Default, PartialEq)]
pub struct Scaled {
    pub base: f32,
    pub factor: f32,
}

/// A charge a mode change still owes: spent before the change, more than the new max had room for.
#[derive(Reflect, Clone, Debug, Default, PartialEq, Eq)]
pub struct Owed {
    pub charge: String,
    pub count: u32,
}

/// Says whether a body with `PlayerId` is seated in the session, so counts toward the session size
/// ([`session_size`](crate::read::session_size)). Set at build time: the default seats every body; the
/// umbrella's net wiring leaves out a body that waits for its player's rejoin.
#[derive(Resource, Clone, Copy)]
pub struct SessionFilter(pub fn(&World, Entity) -> bool);

impl Default for SessionFilter {
    fn default() -> Self {
        SessionFilter(seat_every_body)
    }
}

/// The default [`SessionFilter`]: every body with `PlayerId` is seated.
pub fn seat_every_body(_: &World, _: Entity) -> bool {
    true
}
