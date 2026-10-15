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
    /// The player this agent plays as: its subject is that player's body, found again on
    /// every call (a body that died and respawned is the new one). Set instead of `subject`.
    pub player: Option<u8>,
    /// Journal cursor: the newest entry this agent has been shown.
    pub seen_seq: u64,
}

/// The body of player `n` right now, if it has one (see
/// [`AgentSessions::set_player_resolver`]).
pub type PlayerResolver = fn(&World, u8) -> Option<AiId>;

#[derive(Resource, Debug)]
pub struct AgentSessions {
    sessions: HashMap<String, AgentSession>,
    /// Scope for agents that were not configured explicitly (BRP is dev-only, so `Dev`).
    pub default_scope: Scope,
    /// How a player number becomes a body (`shards_core`: the body with that `PlayerId`).
    player_resolver: Option<PlayerResolver>,
}

impl Default for AgentSessions {
    fn default() -> Self {
        Self { sessions: HashMap::default(), default_scope: Scope::Dev, player_resolver: None }
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
            .or_insert_with(|| AgentSession { name: name.to_owned(), scope, subject: None, player: None, seen_seq: 0 })
    }

    /// Creates or reconfigures an agent (e.g. a fair playtester with `Scope::Player`). A
    /// subject replaces a player binding.
    pub fn configure(&mut self, name: &str, scope: Scope, subject: Option<AiId>) -> &mut AgentSession {
        let s = self.get_or_create(name);
        s.scope = scope;
        s.subject = subject;
        if subject.is_some() {
            s.player = None;
        }
        s
    }

    /// How sessions bound to a player find its body; set by the module that knows players.
    pub fn set_player_resolver(&mut self, resolver: PlayerResolver) {
        self.player_resolver = Some(resolver);
    }

    /// The resolver, if a module set one.
    pub fn player_resolver(&self) -> Option<PlayerResolver> {
        self.player_resolver
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
    /// The player this agent is bound to ([`AgentSession::player`]).
    pub player: Option<u8>,
}

impl Caller {
    /// The subject, or an error an agent can act on.
    pub fn require_subject(&self) -> Result<Entity, String> {
        self.subject.ok_or_else(|| match (self.player, self.subject_id) {
            (Some(n), Some(id)) => format!("you are bound to player {n}, whose body {id} no longer exists"),
            (Some(n), None) => format!("you are bound to player {n}, who has no body right now"),
            (None, Some(id)) => format!("your body {id} no longer exists"),
            (None, None) => "you have no body (no subject and no AiPlayer entity)".to_owned(),
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

/// The caller a call of `agent` would have now: its scope and subject (its session is made on
/// first use).
pub fn caller(world: &mut World, agent: &str) -> Caller {
    let session = world.resource_mut::<AgentSessions>().get_or_create(agent).clone();
    let resolver = world.resource::<AgentSessions>().player_resolver;
    let (subject, subject_id) = match (session.player, session.subject) {
        (Some(n), _) => {
            let id = resolver.and_then(|resolve| resolve(world, n));
            (id.and_then(|id| world.resource::<AiIndex>().get(id)), id)
        }
        (None, Some(id)) => (world.resource::<AiIndex>().get(id), Some(id)),
        (None, None) => {
            let e = player_entity(world);
            (e, e.and_then(|e| world.get::<AiId>(e).copied()))
        }
    };
    Caller { agent: session.name, scope: session.scope, subject, subject_id, player: session.player }
}

/// What `ai/session` (and the scenario `session` command) changes for an agent; what is not
/// given stays as it is.
#[derive(Clone, Debug, Default)]
pub struct SessionChange {
    pub scope: Option<Scope>,
    /// Act as this entity (ends a player binding).
    pub subject: Option<AiId>,
    /// Act as this player's body, whichever entity that is at the time (ends a subject).
    pub player: Option<u8>,
    /// Back to the default: the `AiPlayer` entity, no subject, no player.
    pub clear_subject: bool,
}

/// Applies `change` to `agent`'s session, or says why not (an entity or a player that does not
/// exist now) without changing anything.
pub fn configure_session(world: &mut World, agent: &str, change: &SessionChange) -> Result<AgentSession, String> {
    if change.subject.is_some() && change.player.is_some() {
        return Err("give a subject or a player, not both".to_owned());
    }
    if let Some(id) = change.subject {
        world.resource::<AiIndex>().resolve(id)?;
    }
    if let Some(n) = change.player {
        check_player(world, n)?;
    }
    let mut sessions = world.resource_mut::<AgentSessions>();
    let current = sessions.get_or_create(agent).clone();
    let (subject, player) = match (change.clear_subject, change.subject, change.player) {
        (true, ..) => (None, None),
        (_, Some(id), _) => (Some(id), None),
        (_, _, Some(n)) => (None, Some(n)),
        _ => (current.subject, current.player),
    };
    let session = sessions.configure(agent, change.scope.unwrap_or(current.scope), subject);
    session.player = player;
    Ok(session.clone())
}

/// Player `n` exists now, or the error lists the players that do.
fn check_player(world: &World, n: u8) -> Result<(), String> {
    let Some(resolve) = world.resource::<AgentSessions>().player_resolver else {
        return Err("this game has no players to bind to (no module resolves player numbers)".to_owned());
    };
    if resolve(world, n).is_some() {
        return Ok(());
    }
    let players: Vec<String> = (0..=u8::MAX).filter_map(|p| resolve(world, p).map(|id| format!("{p} ({id})"))).collect();
    let players = if players.is_empty() { "none".to_owned() } else { players.join(", ") };
    Err(format!("there is no player {n} with a body now; players: {players}"))
}

/// `{agent, scope, subject, player, seen_seq}`: what `ai/session` answers.
pub fn session_json(session: &AgentSession) -> serde_json::Value {
    serde_json::json!({
        "agent": session.name,
        "scope": session.scope,
        "subject": session.subject.map(|i| i.to_string()),
        "player": session.player,
        "seen_seq": session.seen_seq,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Component)]
    struct Seat(u8);

    /// The newest body with seat `n`.
    fn seat(world: &World, n: u8) -> Option<AiId> {
        let mut q = world.try_query::<(&Seat, &AiId)>()?;
        q.iter(world).filter(|(s, _)| s.0 == n).map(|(_, id)| *id).max()
    }

    #[test]
    fn a_session_bound_to_a_player_follows_its_body_through_a_respawn() {
        let mut w = World::new();
        w.init_resource::<AiIndex>();
        w.init_resource::<AgentSessions>();
        let err = configure_session(&mut w, "p1", &SessionChange { player: Some(1), ..Default::default() }).unwrap_err();
        assert!(err.contains("no players to bind to"), "{err}");
        w.resource_mut::<AgentSessions>().set_player_resolver(seat);

        let first = w.spawn((crate::AiVisible, Seat(1))).id();
        let session = configure_session(&mut w, "p1", &SessionChange { player: Some(1), ..Default::default() }).unwrap();
        assert_eq!((session.player, session.subject), (Some(1), None));
        assert_eq!(session_json(&session)["player"], 1);
        assert_eq!(caller(&mut w, "p1").subject, Some(first));

        w.despawn(first);
        let gone = caller(&mut w, "p1").require_subject().unwrap_err();
        assert_eq!(gone, "you are bound to player 1, who has no body right now");
        let respawned = w.spawn((crate::AiVisible, Seat(1))).id();
        let now = caller(&mut w, "p1");
        assert_eq!((now.subject, now.subject_id, now.player), (Some(respawned), Some(AiId(2)), Some(1)));

        let err = configure_session(&mut w, "p1", &SessionChange { player: Some(3), ..Default::default() }).unwrap_err();
        assert_eq!(err, "there is no player 3 with a body now; players: 1 (e2)");
        let session = configure_session(&mut w, "p1", &SessionChange { subject: Some(AiId(2)), ..Default::default() }).unwrap();
        assert_eq!((session.player, session.subject), (None, Some(AiId(2))), "a subject ends the binding");
    }
}
