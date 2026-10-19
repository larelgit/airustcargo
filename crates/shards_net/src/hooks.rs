//! What a game decides about its players: who may join now, how a body is spawned and taken
//! away, and what is saved of a character (co-op saves plug in here). Plus `player_join` and
//! `player_leave`, the recorded actions a host runs for its peers, which work in single player
//! too (co-op game logic without a network).

use ai_core::journal::{self, JournalKind};
use ai_core::tools::{register_tool, ToolKind};
use ai_core::{sim, AiId, AiIndex, AiPlayer, Call, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use shards_content::spawn_prefab;
use shards_core::{player_body, Intent, LocalPlayer, PlayerId};

use crate::def::net_def;

/// A player who asks to join: their slot, what they call themselves, their token (a rejoin
/// with the same token keeps the slot) and the character they bring (save sections).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct JoinRequest {
    pub player: u8,
    pub name: String,
    pub token: String,
    pub character: Option<Value>,
}

/// A player who leaves, and why: `quit`, `timeout`, `kicked`, `session ended`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct LeaveRequest {
    pub player: u8,
    pub reason: String,
}

/// Whether someone may join now.
#[derive(Clone, Debug, PartialEq)]
pub enum JoinDecision {
    Accept,
    /// Not now (asked again every host tick); the reason is shown to the joining player:
    /// `a boss fight is on`.
    Wait(String),
    /// Never: the reason says why.
    Refuse(String),
}

/// A reason a joining player has to wait, or `None`: one gate of [`NetHooks::join_gates`].
pub type JoinGate = fn(&World, &JoinRequest) -> Option<String>;

/// The game's hooks into the session, with defaults that make co-op work for a game that sets
/// none. Co-op saves (`shards::coop`) set the character ones.
#[derive(Resource, Clone)]
pub struct NetHooks {
    /// Spawns a joining player's body. Default: `NetDef.player_prefab` at the region's spawn
    /// point for the player, else 2 m beside player 0, with `PlayerId(n)`.
    pub spawner: fn(&mut World, &JoinRequest) -> Result<Entity, String>,
    /// Asked on the host at every tick while a player waits to join, and for a rejoin too.
    /// Default: [`default_join_policy`], which consults `join_gates` and otherwise accepts.
    pub join_policy: fn(&World, &JoinRequest) -> JoinDecision,
    /// Modules that hold joiners while something is on (an encounter) add a gate here; the default
    /// `join_policy` waits with the first reason a gate gives, and accepts otherwise. A game that
    /// sets its own `join_policy` calls the gates it wants (`gates_say`). A player who already has a
    /// body (a rejoin within the grace) skips them: the gates keep newcomers out, not the players
    /// who are in. Default: none.
    pub join_gates: Vec<JoinGate>,
    /// Takes a leaving player's body away. Default: its `Intent` is cleared and it is despawned
    /// after `rejoin_grace_secs` (a rejoin within that time takes it back).
    pub remover: fn(&mut World, Entity, &LeaveRequest),
    /// Which character a joining player's body gets: asked on the host before the recorded
    /// `player_join`, whose arguments carry the answer (so a replay builds the same body).
    /// `request.character` is what they brought; the answer may be another (the host's own
    /// newer copy of it), `None` for none (the body keeps what it has). When the answer is
    /// not what they brought, they are sent the host's version (`CharacterSave`). Default:
    /// what they brought.
    pub join_character: fn(&mut World, &JoinRequest) -> Option<Value>,
    /// Puts a joining player's character (their save sections) onto their new body.
    pub on_join_character: fn(&mut World, Entity, &Value),
    /// A remote player's character as the host has it, to send them (`CharacterSave`): at
    /// their leave, at the end of the session and when the host overrode what they brought.
    pub capture_character: fn(&mut World, Entity) -> Option<Value>,
    /// A client got its character from the host (`CharacterSave`: the host tick it was
    /// captured at, why, the sections, and the slot the player asked for it in, `None` when the
    /// host sent it of its own accord): write it to that slot, or to the one it plays.
    pub on_character_save: fn(&mut World, u64, &str, &Value, Option<&str>),
    /// The host ends the session: save the world (before the clients are told). Answers the
    /// slot the world was saved in, which `SessionEnd` tells the clients.
    pub on_session_end: fn(&mut World) -> Option<String>,
    /// Where player `n` starts (the region's spawn point). Set by the engine when it has regions.
    pub spawn_point: fn(&World, u8) -> Option<Vec3>,
    /// After a client put the host's world in: what else must restart (collider numbers).
    pub after_restore: fn(&mut World),
    /// The world a client must build, for the join and the journal: `{"region": "wilds",
    /// "version": 3, "manifest": "8c1f…"}`.
    pub describe_world: fn(&World) -> Option<Value>,
}

/// The first reason a gate of [`NetHooks::join_gates`] gives for `request` to wait, if any. A game
/// with a `join_policy` of its own calls this to honour the gates modules registered.
pub fn gates_say(world: &World, request: &JoinRequest) -> Option<String> {
    hooks(world).join_gates.iter().find_map(|gate| gate(world, request))
}

/// The default join policy: a player who already has a body (a rejoin) is accepted; a newcomer
/// waits with the reason of the first gate that gives one ([`NetHooks::join_gates`]), else is
/// accepted.
pub fn default_join_policy(world: &World, request: &JoinRequest) -> JoinDecision {
    if player_body(world, request.player).is_some() {
        return JoinDecision::Accept;
    }
    match gates_say(world, request) {
        Some(reason) => JoinDecision::Wait(reason),
        None => JoinDecision::Accept,
    }
}

impl Default for NetHooks {
    fn default() -> Self {
        Self {
            spawner: default_spawner,
            join_policy: default_join_policy,
            join_gates: Vec::new(),
            remover: default_remover,
            join_character: |_, request| request.character.clone(),
            on_join_character: |_, _, _| {},
            capture_character: |_, _| None,
            on_character_save: |_, _, _, _, _| {},
            on_session_end: |_| None,
            spawn_point: |_, _| None,
            after_restore: |_| {},
            describe_world: |_| None,
        }
    }
}

/// The hooks, or the defaults.
pub fn hooks(world: &World) -> NetHooks {
    world.get_resource::<NetHooks>().cloned().unwrap_or_default()
}

/// A player's body was spawned (or taken back) for them.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct PlayerJoined {
    pub player: u8,
    pub body: Entity,
    pub name: String,
}

/// A player left: `reason` is `quit`, `timeout`, `kicked: …` or `session ended`.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct PlayerLeft {
    pub player: u8,
    pub body: Entity,
    pub reason: String,
}

/// The body of a player who left: it waits for a rejoin until tick `until`, then goes.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct NetLeft {
    pub since: u64,
    pub until: u64,
}

/// A body the session holds for a remote player: joined for a peer (`player_join` called by
/// the host's session), or that player's new body. A world rebuilt (a snapshot load, also in
/// a new host process) while no peer of that player is connected sends it through
/// `player_leave`. A body the host joined itself (co-op on its own machine) has none and
/// stays, whichever number a guest held before.
#[derive(Component, Reflect, Clone, Copy, Debug, Default)]
#[reflect(Component, Default)]
pub struct NetSeat;

/// Spawns `NetDef.player_prefab` for the player at their spawn point (or beside player 0).
pub fn default_spawner(world: &mut World, request: &JoinRequest) -> Result<Entity, String> {
    let def = net_def(world);
    let n = request.player;
    let at = (hooks(world).spawn_point)(world, n)
        .or_else(|| {
            let zero = player_body(world, 0).and_then(|id| world.resource::<AiIndex>().get(id))?;
            world.get::<Transform>(zero).map(|t| t.translation + Vec3::new(2.0 * n as f32, 0.0, 0.0))
        })
        .unwrap_or(Vec3::new(2.0 * n as f32, 0.0, 0.0));
    let body = spawn_prefab(world, &def.player_prefab, Transform::from_translation(at), None)?;
    world.entity_mut(body).insert(PlayerId(n)).remove::<(AiPlayer, LocalPlayer)>();
    Ok(body)
}

/// Clears the body's `Intent` and despawns it after the rejoin grace.
pub fn default_remover(world: &mut World, body: Entity, _: &LeaveRequest) {
    let grace = (net_def(world).rejoin_grace_secs / sim::tick_seconds(world).max(1e-6)).round() as u64;
    let now = sim::tick(world);
    if let Some(mut intent) = world.get_mut::<Intent>(body) {
        *intent = Intent::default();
    }
    world.entity_mut(body).insert(NetLeft { since: now, until: now + grace });
}

/// `SimSet::Cleanup`: bodies whose rejoin grace is over go.
pub fn despawn_left(mut commands: Commands, left: Query<(Entity, &NetLeft)>, tick: Res<ai_core::SimTick>) {
    for (entity, left) in &left {
        if tick.0 >= left.until {
            commands.entity(entity).despawn();
        }
    }
}

// --- tools -------------------------------------------------------------------------------

/// Spawn (or take back) a player's body, as a host does when someone joins. Works without a
/// network too: co-op game logic with one machine.
#[derive(Deserialize, JsonSchema, Default)]
pub struct PlayerJoinArgs {
    /// Player number (1, 2, …). Default: the lowest without a body.
    pub player: Option<u8>,
    /// The name others see. Default: `player N`.
    pub name: Option<String>,
    /// Who the player is across reconnects.
    pub token: Option<String>,
    /// The character they bring (save sections).
    pub character: Option<Value>,
}

fn player_join_tool(In(call): In<Call<PlayerJoinArgs>>, world: &mut World) -> ToolResult<Value> {
    let args = call.args;
    let player = match args.player {
        Some(n) => n,
        None => (1..=u8::MAX).find(|n| player_body(world, *n).is_none()).ok_or("no free player number")?,
    };
    let name = args.name.unwrap_or_else(|| format!("player {player}"));
    let request = JoinRequest { player, name: name.clone(), token: args.token.unwrap_or_default(), character: args.character };
    let existing = player_body(world, player).and_then(|id| world.resource::<AiIndex>().get(id));
    let (body, back) = match existing {
        Some(body) if world.get::<NetLeft>(body).is_some() => {
            world.entity_mut(body).remove::<NetLeft>();
            (body, true)
        }
        Some(body) => {
            let id = world.get::<AiId>(body).copied().unwrap_or(AiId(0));
            return Err(format!("player {player} has a body already ({id}); leave first with player_leave player={player}"));
        }
        None => ((hooks(world).spawner)(world, &request)?, false),
    };
    if world.get::<PlayerId>(body).is_none() {
        world.entity_mut(body).insert(PlayerId(player));
    }
    // Who holds the player now: the session for a peer, or the host itself.
    if call.caller.agent == crate::session::NET_AGENT {
        world.entity_mut(body).insert(NetSeat);
    } else {
        world.entity_mut(body).remove::<NetSeat>();
    }
    if let Some(character) = &request.character {
        (hooks(world).on_join_character)(world, body, character);
    }
    world.trigger(PlayerJoined { player, body, name });
    let id = world.get::<AiId>(body).map_or_else(|| "?".to_owned(), ToString::to_string);
    Ok(json!({ "player": player, "body": id, "back": back }))
}

/// Take a player's body away, as a host does when someone leaves (it waits `rejoin_grace_secs`
/// for them to come back).
#[derive(Deserialize, JsonSchema, Default)]
pub struct PlayerLeaveArgs {
    pub player: u8,
    /// `quit`, `timeout`, `kicked`. Default: `quit`.
    pub reason: Option<String>,
}

fn player_leave_tool(In(call): In<Call<PlayerLeaveArgs>>, world: &mut World) -> ToolResult<Value> {
    let PlayerLeaveArgs { player, reason } = call.args;
    if player == 0 {
        return Err("player 0 is the host's own; the host ends the session with net_end".into());
    }
    let reason = reason.unwrap_or_else(|| "quit".into());
    let body = player_body(world, player)
        .and_then(|id| world.resource::<AiIndex>().get(id))
        .ok_or_else(|| format!("player {player} has no body"))?;
    if world.get::<NetLeft>(body).is_some() {
        return Err(format!("player {player} has left already"));
    }
    let request = LeaveRequest { player, reason: reason.clone() };
    (hooks(world).remover)(world, body, &request);
    world.trigger(PlayerLeft { player, body, reason: reason.clone() });
    let id = world.get::<AiId>(body).map_or_else(|| "?".to_owned(), ToString::to_string);
    Ok(json!({ "player": player, "body": id, "reason": reason }))
}

pub(crate) fn register(app: &mut App) {
    use ai_core::AiAppExt;
    app.register_type::<NetLeft>()
        .register_type::<NetSeat>()
        // Session bookkeeping (a respawned guest body gets it outside any call): a replay in
        // single player has no session to hold it.
        .replay_ignore::<NetSeat>()
        .register_type::<PlayerJoined>()
        .register_type::<PlayerLeft>()
        .journal_event::<PlayerJoined>(|e, w| {
            let id = w.get::<AiId>(e.body).map_or_else(|| "?".to_owned(), ToString::to_string);
            Some(format!("p{} joined as {id} ({})", e.player, e.name))
        })
        .journal_event::<PlayerLeft>(|e, w| {
            let id = w.get::<AiId>(e.body).map_or_else(|| "?".to_owned(), ToString::to_string);
            Some(format!("p{} left ({}), body {id}", e.player, e.reason))
        });
    let world = app.world_mut();
    register_tool(world, "player_join", Scope::Dev, ToolKind::Action, player_join_tool);
    register_tool(world, "player_leave", Scope::Dev, ToolKind::Action, player_leave_tool);
}

/// The `net:` journal line, at most once a second per `kind` (by tick, so the same in every
/// run).
pub fn net_line(world: &mut World, kind: &str, level: JournalKind, text: impl Into<String>) {
    let tick = sim::tick(world);
    let hz = (1.0 / sim::tick_seconds(world).max(1e-6)).round() as u64;
    let mut limits = world.get_resource_or_init::<LineLimits>();
    if let Some(last) = limits.0.get(kind)
        && tick < last + hz
        && tick >= *last
    {
        return;
    }
    limits.0.insert(kind.to_owned(), tick);
    journal::emit(world, level, format!("net: {}", text.into()));
}

/// When each kind of `net:` line was written last.
#[derive(Resource, Default)]
pub struct LineLimits(pub std::collections::BTreeMap<String, u64>);
