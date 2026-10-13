//! Agent sessions: who is calling, with which rights, as which entity.
//!
//! Agents are external (a coding agent driving `shards`, a script, CI). By default
//! an agent gets [`Scope::Dev`] and the player's character; a playtester that must
//! play fair gets [`Scope::Player`] (`shards session --scope player`).

use bevy::platform::collections::HashMap;
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::id::{AiId, AiIndex};
use crate::observe::player_entity;

/// Permission level. `Dev` includes everything `Player` can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Scope {
    /// Plays fair: acts only through `Intent`, the same input a human or a network
    /// client uses.
    Player,
    /// Spawns, teleports, edits components via reflection, controls time.
    Dev,
}

impl Scope {
    pub fn allows(self, needed: Scope) -> bool {
        self >= needed
    }
}

impl std::fmt::Display for Scope {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Scope::Player => "player",
            Scope::Dev => "dev",
        })
    }
}

#[derive(Clone, Debug)]
pub struct AgentSession {
    pub name: String,
    pub scope: Scope,
    /// The entity this agent embodies ("YOU"). `None`: the [`AiPlayer`](crate::AiPlayer) entity.
    pub subject: Option<AiId>,
    /// Journal cursor: the newest entry this agent has been shown.
    pub seen_seq: u64,
}

#[derive(Resource, Debug)]
pub struct AgentSessions {
    sessions: HashMap<String, AgentSession>,
    /// Scope for agents that were not configured explicitly (BRP is dev-only, so `Dev`).
    pub default_scope: Scope,
}

impl Default for AgentSessions {
    fn default() -> Self {
        Self { sessions: HashMap::default(), default_scope: Scope::Dev }
    }
}

impl AgentSessions {
    pub fn get(&self, name: &str) -> Option<&AgentSession> {
        self.sessions.get(name)
    }

    pub fn get_or_create(&mut self, name: &str) -> &mut AgentSession {
        let scope = self.default_scope;
        self.sessions
            .entry(name.to_owned())
            .or_insert_with(|| AgentSession { name: name.to_owned(), scope, subject: None, seen_seq: 0 })
    }

    /// Creates or reconfigures an agent (e.g. a fair playtester with `Scope::Player`).
    pub fn configure(&mut self, name: &str, scope: Scope, subject: Option<AiId>) -> &mut AgentSession {
        let s = self.get_or_create(name);
        s.scope = scope;
        s.subject = subject;
        s
    }

    pub fn names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.sessions.keys().cloned().collect();
        names.sort();
        names
    }
}

/// Who is calling a tool. Passed to every tool system inside [`Call`](crate::Call).
#[derive(Clone, Debug)]
pub struct Caller {
    pub agent: String,
    pub scope: Scope,
    /// The entity this agent acts as, if it exists right now.
    pub subject: Option<Entity>,
    pub subject_id: Option<AiId>,
}

impl Caller {
    /// The subject, or an error an agent can act on.
    pub fn require_subject(&self) -> Result<Entity, String> {
        self.subject.ok_or_else(|| match self.subject_id {
            Some(id) => format!("your body {id} no longer exists"),
            None => "you have no body (no subject and no AiPlayer entity)".to_owned(),
        })
    }

    /// Dev-only operations on entities other than your own body.
    pub fn require_dev(&self, what: &str) -> Result<(), String> {
        if self.scope.allows(Scope::Dev) {
            Ok(())
        } else {
            Err(format!("{what} needs dev scope; agent `{}` has {} scope", self.agent, self.scope))
        }
    }
}

pub(crate) fn caller(world: &mut World, agent: &str) -> Caller {
    let session = world.resource_mut::<AgentSessions>().get_or_create(agent).clone();
    let (subject, subject_id) = match session.subject {
        Some(id) => (world.resource::<AiIndex>().get(id), Some(id)),
        None => {
            let e = player_entity(world);
            (e, e.and_then(|e| world.get::<AiId>(e).copied()))
        }
    };
    Caller { agent: session.name, scope: session.scope, subject, subject_id }
}
