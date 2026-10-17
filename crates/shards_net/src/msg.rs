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

/// Session protocol version: a client with another one is refused. 2: the join's cookie, and a
/// character too big for the hello in parts after it.
pub const PROTO: u32 = 2;

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
    /// The client's copy differs from the host's: the state of host tick `tick`, whose hash
    /// was `host`, in the world of the client's rebase `epoch`.
    Desync {
        tick: u64,
        here: u64,
        host: u64,
        first: String,
        #[serde(default)]
        epoch: u32,
    },
    /// A player's character, captured by the host (co-op saves). `asked`: the number of the
    /// player's [`Msg::CharacterAsk`] it answers; none when the host sends it of its own accord.
    CharacterSave { tick: u64, reason: String, sections: Value, asked: Option<u32> },
    Paused(bool),
    /// Part `index` of a character too big for the hello ([`Hello::character_parts`]): a piece of
    /// its JSON text, in order.
    CharacterPart { index: u32, text: String },
    /// A player asks for its character now (`save_game what=character` on a client): the host
    /// answers with a [`Msg::CharacterSave`] (reason `save_game`), or why not, either carrying
    /// `id` back: the player's number for the ask, which says the slot the answer goes into.
    CharacterAsk { id: u32 },
    /// Why the host did not send the character a player asked for (ask `asked`).
    CharacterNotSent { reason: String, asked: Option<u32> },
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
    /// The token the host issued this player at an earlier join (kept beside the client's
    /// saves), empty the first time: a rejoin with it keeps the slot and the body.
    pub token: String,
    /// The character the player brings (`--character`), as the game's save sections, when its
    /// JSON is small enough to come in the hello (`host::MAX_INLINE_CHARACTER`).
    pub character: Option<Value>,
    /// A bigger character: its JSON comes after the hello in this many [`Msg::CharacterPart`]s.
    pub character_parts: u32,
    /// The bytes of that JSON, all parts together.
    pub character_bytes: u64,
    /// Tick rate, so a mismatch is named before the seed is adopted.
    pub tick_hz: f64,
}

/// The JSON text of `character` cut into pieces of at most `part` bytes, at character boundaries:
/// what a character too big for the hello travels in. Joined, they decode to it again.
pub fn split_character(character: &Value, part: usize) -> Vec<String> {
    let text = serde_json::to_string(character).expect("a JSON value serializes");
    let mut parts = Vec::new();
    let mut rest = text.as_str();
    while !rest.is_empty() {
        let mut at = rest.len().min(part.max(4));
        while !rest.is_char_boundary(at) {
            at -= 1;
        }
        let (piece, tail) = rest.split_at(at);
        parts.push(piece.to_owned());
        rest = tail;
    }
    parts
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
    /// Who this player is to the host from now on: random, issued at the first join; the
    /// client keeps it to take its slot back after a restart.
    #[serde(default)]
    pub token: String,
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
    /// The host's ticks did not move (a resync, a new body): the client keeps the inputs it
    /// sent after `tick` and its own intent, and the host keeps the ones it has.
    #[serde(default)]
    pub carry_inputs: bool,
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
