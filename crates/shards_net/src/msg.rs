//! What host and client say to each other: JSON over the link. Everything is reliable and
//! ordered except [`StateDelta`] (host → client) and [`Inputs`] (client → host), which go on
//! the latest-wins channel.

use std::collections::BTreeMap;

use ai_core::snapshot::Snapshot;
use ai_core::state::ComponentValues;
use ai_core::Scope;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::inputs::InputFrame;

/// Session protocol version: a client with another one is refused.
pub const PROTO: u32 = 1;

/// A reliable message.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Msg {
    Hello(Hello),
    Accept(Accept),
    /// The game says "not now"; asked again every host tick.
    Wait { reason: String },
    /// The join is over: the reason says what to fix.
    Refused { reason: String },
    /// The client built the world from the seed: the hashes of what it is built around.
    Ready { checks: BTreeMap<String, String> },
    Welcome(Welcome),
    /// The whole world again (a region change, a snapshot or save load, a resync).
    Rebase(Welcome),
    /// The client leaves.
    Bye { reason: String },
    /// The host ended the session.
    SessionEnd { reason: String, saved_slot: Option<String> },
    /// A line of the host's journal (events: damage, deaths, telegraphs).
    Journal { tick: u64, kind: String, text: String },
    /// A typed event (`net_event`), for presentation.
    Event { tick: u64, type_path: String, value: Value },
    /// A client's call that runs on the host.
    Call(CallMsg),
    CallReply { id: u64, ok: bool, text: String, tick: u64 },
    /// The client's copy differs from the host's.
    Desync { tick: u64, here: u64, host: u64, first: String },
    /// A player's character, captured by the host (co-op saves).
    CharacterSave { tick: u64, reason: String, sections: Value },
    Paused(bool),
}

/// The first thing a client says.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Hello {
    pub proto: u32,
    /// Engine version.
    pub engine: String,
    /// Game title and version, as the game names itself.
    pub game: String,
    /// `ContentDb::hash`.
    pub content_hash: u64,
    /// `net_rules::rules_hash`.
    pub net_rules_hash: u64,
    pub name: String,
    /// Who this player is across reconnects: a rejoin with the same token keeps the slot.
    pub token: String,
    /// The character the player brings (`--character`), as the game's save sections.
    pub character: Option<Value>,
    /// Tick rate, so a mismatch is named before the seed is adopted.
    pub tick_hz: f64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Accept {
    pub seed: u64,
    pub tick_hz: f64,
    pub session: u32,
    pub player: u8,
    /// The region the host is in (id, generator version, manifest hash), when the game has
    /// regions.
    pub region: Option<Value>,
}

/// The world as the host has it at `tick`, and whose body is whose.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Welcome {
    pub player: u8,
    pub body: Option<u32>,
    pub tick: u64,
    pub reason: String,
    pub snapshot: Snapshot,
    /// The host's region description (as in [`Accept`]).
    pub region: Option<Value>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CallMsg {
    pub id: u64,
    pub agent: String,
    pub scope: Scope,
    pub subject: Option<u32>,
    pub tool: String,
    pub args: Value,
}

/// One entity's change against the baseline.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct EntityDelta {
    pub id: u32,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub set: ComponentValues,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub removed: Vec<String>,
}

/// The own body's predicted components and the newest input the host applied for it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct OwnState {
    pub body: u32,
    pub applied_input_tick: u64,
    pub components: ComponentValues,
}

/// The client's view at `tick`, against the view it acknowledged (`baseline`; `None`: whole).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct StateDelta {
    pub tick: u64,
    pub baseline: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub entities: Vec<EntityDelta>,
    /// Entities out of view (or gone) since the baseline.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub gone: Vec<u32>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub resources: BTreeMap<String, Value>,
    pub own: Option<OwnState>,
    pub paused: bool,
    /// The desync hash of the view at `tick` (every `desync_every` ticks).
    pub hash: Option<u64>,
    /// The newest input frame tick the host has from this client (what to stop repeating).
    pub inputs_ack: u64,
    /// `(client tick of the newest Inputs packet, its margin in ticks)`: how early it came.
    pub margin: Option<(u64, i64)>,
    /// Rebases sent to this client before this state: a state of an older world is dropped.
    #[serde(default)]
    pub epoch: u32,
}

/// A client's inputs: every frame the host has not acknowledged.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Inputs {
    /// The client's tick when it sent this.
    pub tick: u64,
    pub frames: Vec<InputFrame>,
    /// The newest state the client applied.
    pub state_ack: u64,
    /// Rebases the client has taken: frames of an older world are dropped.
    #[serde(default)]
    pub epoch: u32,
}

pub fn encode<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).expect("net messages serialize")
}

pub fn decode<T: for<'de> Deserialize<'de>>(bytes: &[u8]) -> Result<T, String> {
    serde_json::from_slice(bytes).map_err(|e| format!("undecodable message ({} bytes): {e}", bytes.len()))
}
