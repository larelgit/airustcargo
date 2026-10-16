//! Co-op saves: characters travel with their players, the host keeps the world.
//!
//! [`shards_net`] (the session) and [`shards_save`] (the files) know nothing of each other;
//! this is the glue. Two kinds of save meet here (E8.1, GDD 24.2):
//!
//! * **The world** is the host's. It is saved on the host (`--world SLOT`, the current
//!   world slot), at the end of the session and whenever the game saves it. A client never
//!   writes one: `save_game what=world` there answers `the host keeps the world`, and
//!   `load_game` is refused (the host loads, and every client is sent the new world).
//! * **A character** is its player's, in a file on their machine (`--character SLOT`). The
//!   host captures it from the authoritative body and sends it (`CharacterSave`, reliable);
//!   the client writes its slot through the normal store (atomic, three backups).
//!
//! How a character travels:
//!
//! | when | what happens |
//! |---|---|
//! | join | the client reads its slot (a damaged file is restored from its backup, the journal says so) and puts the sections in `Hello`; the host applies them to the new body inside the recorded `player_join`, so a replay builds the same body. A version older than this build's is migrated; an unreadable payload is journaled and the player starts fresh |
//! | the game saves (`save_now`, an autosave event, `save_game` for a remote player's body) | the host writes its own character and sends every remote player theirs |
//! | leave, kick, timeout | the host captures the body and sends it to the leaver's own link; the body waits for a rejoin |
//! | the session ends | the host saves the world and its character, then each client gets a final `CharacterSave`, then `SessionEnd` (which names the world slot) |
//!
//! The host also remembers each player's last character for the session
//! ([`SessionCharacters`], by token). When a player comes back and what they bring is a
//! character the host itself sent them earlier (a final save that never arrived, a file
//! that fell behind), the host's newer copy wins: the body keeps its state, or, if it is gone
//! (the rejoin grace passed), is given the remembered character; the player is sent it, so
//! their file catches up. What they bring that the host never sent (a fresh join, a game
//! played elsewhere meanwhile) is theirs and is applied.
//!
//! A character travels as one string: the canonical JSON of its sections
//! ([`character_value`]), which reads back exactly, floats included.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};

use ai_core::journal::{self, JournalKind};
use ai_core::tools::NetRoute;
use ai_core::{sim, AiIndex, AiTools};
use bevy::prelude::*;
use serde_json::Value;
use shards_core::{player_body, EngineAppExt, EngineConfig, EngineState, NetLaunch, NetRole, PlayerId};
use shards_net::host::{clip, MAX_CHARACTER_BYTES, MAX_PEER_TEXT};
use shards_net::{JoinRequest, NetClient, NetHooks, NetHost, NetLeft, NetSystems};
use shards_save::{
    apply_sections, canon, capture_sections, load_launch_slots, prepare, read_file, replay_playing, save_part, write_sections, LaunchLoaded, Routed,
    SaveConfig, SaveKind, SaveRegistry, SaveReport, SaveRouting, SaveSlots, SectionSet,
};

/// Why a client's `load_game` is refused (`NetRoute::HostOnly`).
pub const LOAD_ON_THE_HOST: &str = "the host keeps the world, and your character is loaded when you join (--character SLOT)";

/// How many of the characters the host sent a player it remembers as theirs: a file holding
/// one of them is the host's own copy.
const KNOWN_SENT: usize = 16;

/// Co-op saves: fills [`NetHooks`] and [`SaveRouting`]. Part of
/// [`EnginePlugins`](crate::EnginePlugins), after the net and save modules.
pub struct CoopPlugin;

impl Plugin for CoopPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<shards_net::NetPlugin>() || !app.is_plugin_added::<shards_save::SavePlugin>() {
            // Co-op saves are the meeting of both; a game with one of them has nothing to join.
            return;
        }
        app.add_engine_module("coop", env!("CARGO_PKG_VERSION"))
            .init_resource::<SessionCharacters>()
            .add_systems(Startup, bring_character)
            .add_systems(RunFixedMainLoop, load_host_launch.in_set(RunFixedMainLoopSystems::BeforeFixedMainLoop))
            // What was queued while the session's links were out of the world goes out before they flush.
            .add_systems(FixedLast, send_outbox.before(NetSystems::Send).run_if(resource_exists::<NetHost>))
            .add_systems(Last, send_outbox.before(NetSystems::Pump).run_if(resource_exists::<NetHost>));
        let mut hooks = app.world_mut().get_resource_or_init::<NetHooks>();
        hooks.join_character = join_character;
        hooks.on_join_character = on_join_character;
        hooks.capture_character = capture_character;
        hooks.on_character_save = on_character_save;
        hooks.on_session_end = on_session_end;
        app.insert_resource(SaveRouting { route, saved });
        // A client loads nothing: the host's world is the world.
        if let Err(e) = app.world_mut().resource_mut::<AiTools>().set_route("load_game", NetRoute::HostOnly(LOAD_ON_THE_HOST)) {
            warn!("coop: {e}");
        }
    }
}

// --- what the host remembers -----------------------------------------------------------

/// The last character the host captured of each player of this session, by the token that
/// says who they are: what a rejoin restores when the player's own file fell behind.
#[derive(Resource, Default, Debug)]
pub struct SessionCharacters {
    /// Who holds each player number now (learned when they join).
    tokens: BTreeMap<u8, String>,
    saved: BTreeMap<String, Kept>,
    /// Characters to send once the session is in reach: a forwarded call, a join or a leave runs
    /// while the host's session is out of the world, and a save made there cannot send at once.
    outbox: Vec<Outgoing>,
}

/// A character waiting to be sent to a player.
#[derive(Debug)]
struct Outgoing {
    player: u8,
    tick: u64,
    reason: String,
    character: Value,
}

#[derive(Debug, Default)]
struct Kept {
    /// The host tick of the capture.
    tick: u64,
    /// The character, as [`character_value`].
    character: Value,
    /// Fingerprints of the characters the host sent this player (the newest last): a file
    /// that holds one of them is the host's own copy, however old.
    sent: Vec<u64>,
}

impl SessionCharacters {
    /// Players with a remembered character.
    pub fn len(&self) -> usize {
        self.saved.len()
    }

    pub fn is_empty(&self) -> bool {
        self.saved.is_empty()
    }

    /// The remembered character of `token`, and the host tick it was captured at.
    pub fn get(&self, token: &str) -> Option<(u64, &Value)> {
        self.saved.get(token).map(|k| (k.tick, &k.character))
    }

    /// The character a joining player gets. `brought` is the character they bring, `body`
    /// whether their body is still in the world (waiting for them), `token` who they are, `now`
    /// the host's tick. `None`: the body keeps what it has (it is newer than any file), or there
    /// is nothing to apply.
    ///
    /// A character the host knows is one it sent the player or applied at their first join: a
    /// file that still holds one is the host's own copy however old, and the host's newest wins.
    /// Any other file was made elsewhere, is theirs, and becomes what the host knows.
    fn choose(&mut self, token: &str, brought: Option<&Value>, body: bool, now: u64) -> Option<Value> {
        if token.is_empty() {
            return brought.cloned();
        }
        // A body older than what the host kept (the world was taken back to before the capture)
        // is not the newest there is.
        let stands = |kept: &Kept| body && kept.tick <= now;
        match (self.saved.get(token), brought) {
            (Some(kept), None) => (!stands(kept)).then(|| kept.character.clone()),
            (Some(kept), Some(brought)) if kept.sent.contains(&fingerprint(brought)) => (!stands(kept)).then(|| kept.character.clone()),
            (_, Some(brought)) => {
                self.remember_token(token, now, brought);
                Some(brought.clone())
            }
            (None, None) => None,
        }
    }

    fn remember(&mut self, player: u8, tick: u64, character: &Value) {
        let Some(token) = self.tokens.get(&player).cloned() else { return };
        self.remember_token(&token, tick, character);
    }

    fn remember_token(&mut self, token: &str, tick: u64, character: &Value) {
        if token.is_empty() {
            return;
        }
        let kept = self.saved.entry(token.to_owned()).or_default();
        let print = fingerprint(character);
        kept.tick = tick;
        kept.character = character.clone();
        kept.sent.retain(|p| *p != print);
        kept.sent.push(print);
        if kept.sent.len() > KNOWN_SENT {
            kept.sent.remove(0);
        }
    }

    /// Whether `player` is a player of the session (they joined through the host).
    fn has_player(&self, player: u8) -> bool {
        self.tokens.contains_key(&player)
    }
}

/// What identifies a character: the hash of its canonical text.
fn fingerprint(character: &Value) -> u64 {
    match character {
        Value::String(text) => canon::fnv(text.as_bytes()),
        other => canon::fnv(canon::to_string(other).as_bytes()),
    }
}

/// A character as it travels (`Hello`, `player_join`, `CharacterSave`): the canonical text
/// of its sections, which reads back to exactly the same data.
pub fn character_value(sections: &SectionSet) -> Value {
    Value::String(sections.canonical())
}

/// The sections in a character that travelled ([`character_value`]), or an object of them
/// (what an agent types into `player_join`).
pub fn parse_character(value: &Value) -> Result<SectionSet, String> {
    match value {
        Value::String(text) => SectionSet::from_canonical(text),
        other => serde_json::from_value(other.clone()).map_err(|e| format!("not a set of sections: {e}")),
    }
}

// --- joining ---------------------------------------------------------------------------

/// `NetHooks::join_character`: which character the body of a joining player gets.
fn join_character(world: &mut World, request: &JoinRequest) -> Option<Value> {
    let body = player_body(world, request.player).is_some();
    let now = sim::tick(world);
    let mut sessions = world.get_resource_or_init::<SessionCharacters>();
    sessions.tokens.insert(request.player, request.token.clone());
    sessions.choose(&request.token, request.character.as_ref(), body, now)
}

/// A character's sections split into the ones this game saves as a character, and the names of
/// the rest (another kind's, or none): a character that comes from another machine is applied
/// or written only as far as this game would have saved it.
fn split_character(world: &World, set: SectionSet) -> (SectionSet, Vec<String>) {
    let registry = world.resource::<SaveRegistry>();
    let (ours, foreign): (Vec<_>, Vec<_>) = set.0.into_iter().partition(|(name, _)| registry.get(name).is_some_and(|entry| entry.kind == SaveKind::Character));
    (SectionSet(ours.into_iter().collect()), foreign.into_iter().map(|(name, _)| name).collect())
}

/// `NetHooks::on_join_character`: the character's sections onto the new body. What a peer
/// sends is not trusted with the journal: its text is clipped, and sections this game does
/// not save as characters (another kind's, or none) never reach the apply.
fn on_join_character(world: &mut World, body: Entity, value: &Value) {
    let who = world.get::<PlayerId>(body).map_or_else(|| "a player".to_owned(), |p| format!("p{}", p.0));
    let peer = |text: &str| clip(text, MAX_PEER_TEXT);
    let set = match parse_character(value) {
        Ok(set) => set,
        Err(e) => {
            journal::emit(world, JournalKind::Warn, format!("save: {who} character could not be applied ({}); starting fresh", peer(&e)));
            return;
        }
    };
    let (set, foreign) = split_character(world, set);
    if !foreign.is_empty() {
        journal::emit(world, JournalKind::Warn, format!("save: {who} brought sections this game does not save: {}; skipped", peer(&foreign.join(", "))));
    }
    // A section that panics on what a peer sent must not take the join with it (the body is made
    // already): the player starts with what the body has.
    let applied = catch_unwind(AssertUnwindSafe(|| apply_sections(world, SaveKind::Character, Some(body), &set)));
    match applied {
        Ok(Ok(done)) => {
            journal::log(world, JournalKind::Sim, format!("save: {who} character applied ({})", done.applied.join(", ")));
        }
        // A section's `check` refused: nothing ran, the body is as it was.
        Ok(Err(e)) if !e.changed_anything() => {
            journal::emit(world, JournalKind::Warn, format!("save: {who} character could not be applied ({}); starting fresh", peer(&e.message)));
        }
        // An `apply` (or `missing`) failed: it may have changed the body before it did.
        Ok(Err(e)) => {
            let ran: Vec<&str> = e.applied.iter().chain(&e.missing).map(String::as_str).collect();
            let before = if ran.is_empty() { "nothing before it".to_owned() } else { ran.join(", ") };
            journal::emit(
                world,
                JournalKind::Warn,
                format!(
                    "save: {who} character could only be applied in part ({}); applied: {before}; the failed section may have changed the body, which keeps the rest as it was",
                    peer(&e.message)
                ),
            );
        }
        Err(panic) => {
            // The journal says it and the join goes on: no crash report.
            ai_core::crash::handled(&ai_core::crash::panic_text(panic.as_ref()));
            journal::emit(world, JournalKind::Warn, format!("save: {who} character could not be applied (a section panicked); the body keeps what it has"));
        }
    }
}

// --- the host captures -----------------------------------------------------------------

/// `NetHooks::capture_character`: the character of a remote player's body as the host has it,
/// remembered for their rejoin and about to be sent to them.
fn capture_character(world: &mut World, body: Entity) -> Option<Value> {
    let player = world.get::<PlayerId>(body)?.0;
    let set = match capture_sections(world, SaveKind::Character, Some(body)) {
        Ok(set) => set,
        Err(e) => {
            journal::emit(world, JournalKind::Warn, format!("save: p{player}'s character could not be captured ({e}); it is not sent"));
            return None;
        }
    };
    let character = character_value(&set);
    let tick = sim::tick(world);
    world.get_resource_or_init::<SessionCharacters>().remember(player, tick, &character);
    Some(character)
}

/// Captures `player`'s body and sends it to their machine: at once when the session is in reach,
/// else (a forwarded call, a join or a leave is running) with the next send.
fn send_to_player(world: &mut World, player: u8, body: Entity, reason: &str) -> Result<SaveReport, String> {
    let character = capture_character(world, body).ok_or_else(|| format!("p{player}'s character could not be captured; the journal says why"))?;
    let tick = sim::tick(world);
    let (bytes, sections) = match &character {
        Value::String(text) => (text.len(), SectionSet::from_canonical(text).map_or(0, |s| s.len())),
        other => (other.to_string().len(), 0),
    };
    // `Some(sent)` when the session is in reach, `None` when it is out of the world.
    let sent = world.get_resource_mut::<NetHost>().map(|mut host| host.send_character(player, tick, reason, character.clone()));
    let target = match sent {
        Some(true) => format!("p{player}'s machine"),
        Some(false) => format!("p{player} (away; kept for their return)"),
        None => {
            world.get_resource_or_init::<SessionCharacters>().outbox.push(Outgoing { player, tick, reason: reason.to_owned(), character });
            format!("p{player}'s machine (when the session next sends)")
        }
    };
    let report = SaveReport { kind: SaveKind::Character, slot: format!("p{player}"), tick, bytes, sections, reason: reason.to_owned(), written: true, target: Some(target) };
    journal::log(world, JournalKind::Sim, format!("save: {report}"));
    Ok(report)
}

/// `FixedLast` / `Last`, before the session sends: the characters queued while it was out of the
/// world go to their players.
fn send_outbox(world: &mut World) {
    let queued = match world.get_resource_mut::<SessionCharacters>() {
        Some(mut sessions) if !sessions.outbox.is_empty() => std::mem::take(&mut sessions.outbox),
        _ => return,
    };
    let Some(mut host) = world.get_resource_mut::<NetHost>() else {
        world.resource_mut::<SessionCharacters>().outbox = queued;
        return;
    };
    for Outgoing { player, tick, reason, character } in queued {
        // A player who is gone has their character in the host's memory for their return.
        host.send_character(player, tick, &reason, character);
    }
}

/// The remote players who have a body in the world now, with it, by player number. A player is
/// remote when they joined through the host; whether their link is there is for the send to say.
fn remote_players(world: &World) -> Vec<(u8, Entity)> {
    let Some(sessions) = world.get_resource::<SessionCharacters>() else { return Vec::new() };
    let index = world.resource::<AiIndex>();
    sessions
        .tokens
        .keys()
        .copied()
        .filter(|player| *player != 0)
        .filter_map(|player| {
            let body = index.get(player_body(world, player)?)?;
            world.get::<NetLeft>(body).is_none().then_some((player, body))
        })
        .collect()
}

// --- where a save goes -----------------------------------------------------------------

/// `SaveRouting::route`: a client writes no world and no character of its own; the host sends
/// a remote player's character to their machine instead of writing it into its own slot.
fn route(world: &mut World, kind: SaveKind, body: Option<Entity>, reason: &str) -> Routed {
    match world.get_resource::<NetRole>().copied().unwrap_or_default() {
        NetRole::Single => None,
        NetRole::Client => {
            let host = world.get_resource::<NetClient>().map_or_else(|| "the host".to_owned(), |c| c.address.clone());
            let why = match kind {
                SaveKind::World => format!("the host keeps the world; ask the host ({host})"),
                SaveKind::Character => format!(
                    "your character is saved by the host and sent here (at camps and rewards, when you leave, when the session ends); \
                     the host asks for one now with save_game what=character bound to your player ({host})"
                ),
            };
            if reason != "save_game" {
                journal::log(world, JournalKind::Sim, format!("save: {kind} autosave ({reason}) skipped on a client: {why}"));
            }
            Some(Err(why))
        }
        NetRole::Host => {
            if kind != SaveKind::Character {
                return None;
            }
            let body = body?;
            let player = world.get::<PlayerId>(body)?.0;
            // Player 0 is the host's own; a player the host joined itself has no machine. Who is a
            // session's player is what the host learned at their join, not whether its session is in
            // the world now: it is not while a forwarded call runs.
            if player == 0 || !world.get_resource::<SessionCharacters>().is_some_and(|s| s.has_player(player)) {
                return None;
            }
            if replay_playing(world) {
                let tick = sim::tick(world);
                return Some(Ok(SaveReport {
                    kind,
                    slot: format!("p{player}"),
                    tick,
                    bytes: 0,
                    sections: 0,
                    reason: reason.to_owned(),
                    written: false,
                    target: None,
                }));
            }
            Some(send_to_player(world, player, body, reason))
        }
    }
}

/// `SaveRouting::saved`: the host saved its own character because the game decided to; every
/// remote player's goes to their machine with it.
fn saved(world: &mut World, kind: SaveKind, reason: &str) {
    if kind != SaveKind::Character || world.get_resource::<NetRole>().copied() != Some(NetRole::Host) || replay_playing(world) {
        return;
    }
    for (player, body) in remote_players(world) {
        if let Err(e) = send_to_player(world, player, body, reason) {
            journal::emit(world, JournalKind::Warn, format!("save: p{player}'s character was not sent ({e})"));
        }
    }
}

// --- the client ------------------------------------------------------------------------

/// `NetHooks::on_character_save`: the host's capture of this player's character, into the
/// slot this machine plays. Nothing is written that this build could not read: a bad
/// capture must not replace a good file.
fn on_character_save(world: &mut World, tick: u64, reason: &str, sections: &Value) {
    let Some(slot) = world.get_resource::<SaveSlots>().map(|s| s.character.clone()) else { return };
    let refuse = |world: &mut World, why: String| {
        journal::emit(world, JournalKind::Error, format!("save: the host's character for you {}; character/{slot} is as it was", clip(&why, MAX_PEER_TEXT)));
    };
    // The size a host takes of a joining character is the size a client keeps: a file that is
    // over it could never be brought to a host again.
    let size = serde_json::to_string(sections).map_or(usize::MAX, |text| text.len());
    if size > MAX_CHARACTER_BYTES {
        return refuse(world, format!("is over {} KB, which no host takes", MAX_CHARACTER_BYTES / 1024));
    }
    let set = match parse_character(sections) {
        Ok(set) => set,
        Err(e) => return refuse(world, format!("could not be read ({e})")),
    };
    let (set, foreign) = split_character(world, set);
    if !foreign.is_empty() {
        journal::emit(world, JournalKind::Warn, format!("save: the host's character for you has sections this game does not save: {}; skipped", clip(&foreign.join(", "), MAX_PEER_TEXT)));
    }
    let has_sections = world.get_resource::<SaveRegistry>().is_some_and(|registry| registry.of_kind(SaveKind::Character).next().is_some());
    if set.is_empty() && has_sections {
        return refuse(world, "has none of the sections this game saves".to_owned());
    }
    if let Some(Err(fault)) = world.get_resource::<SaveRegistry>().map(|registry| prepare(registry, SaveKind::Character, &set)) {
        return refuse(world, format!("is not one this game reads ({fault})"));
    }
    // Failures are in the journal (Error) already.
    let _ = write_sections(world, SaveKind::Character, &slot, "", &clip(reason, 40), tick, set);
}

/// `Startup` (a client): reads the character slot this machine plays and puts it in the
/// `Hello`, so the host builds the body from it. A slot with no file is a new player.
fn bring_character(world: &mut World) {
    let Some(config) = world.get_resource::<EngineConfig>() else { return };
    if !matches!(config.net, NetLaunch::Join { .. }) || !world.contains_resource::<NetClient>() {
        return;
    }
    if let Some(slot) = config.world_slot.clone() {
        journal::emit(world, JournalKind::Warn, format!("save: --world {slot} ignored: the host keeps the world"));
    }
    let slot = world.resource::<SaveSlots>().character.clone();
    let Some(config) = world.get_resource::<SaveConfig>() else { return };
    if !config.store().exists(SaveKind::Character, &slot) {
        journal::emit(world, JournalKind::Sim, format!("save: character/{slot}: no character yet, joining as a new player (the host sends it back as you play)"));
        return;
    }
    match read_file(world, SaveKind::Character, &slot) {
        Ok(file) => {
            journal::emit(world, JournalKind::Sim, format!("save: bringing character/{slot} (tick {}) to the host", file.tick));
            world.resource_mut::<NetClient>().character = Some(character_value(&file.sections));
        }
        Err(e) => {
            journal::emit(world, JournalKind::Error, format!("save: character/{slot} could not be read, joining as a new player: {e}"));
        }
    }
}

// --- the host --------------------------------------------------------------------------

/// `RunFixedMainLoop`, before the fixed loop (a host): once the game is in play and nothing
/// holds the simulation, loads the slots the launch names (`--world`, `--character`) like a
/// single game does: a `load_game` call of the recording's own, so a replay of the session
/// starts from the loaded state. A client loads none (the host's world is the world).
fn load_host_launch(world: &mut World) {
    if world.resource::<LaunchLoaded>().0 {
        return;
    }
    let host = world.get_resource::<EngineConfig>().is_some_and(|c| matches!(c.net, NetLaunch::Host { .. } | NetLaunch::Stand { .. }));
    let in_game = world.get_resource::<State<EngineState>>().is_some_and(|s| *s.get() == EngineState::InGame);
    if !host || !in_game || sim::gate_message(world).is_some() {
        return;
    }
    world.resource_mut::<LaunchLoaded>().0 = true;
    if !replay_playing(world) {
        load_launch_slots(world);
    }
}

/// `NetHooks::on_session_end`: the world and the host's own character are saved before the
/// clients hear that the session is over (each of them gets a final character, then
/// `SessionEnd`, which names the world slot). Answers that slot.
fn on_session_end(world: &mut World) -> Option<String> {
    if replay_playing(world) {
        return None;
    }
    let slots = world.get_resource::<SaveSlots>().cloned()?;
    let in_game = world.get_resource::<State<EngineState>>().is_some_and(|s| *s.get() == EngineState::InGame);
    // A host that quit before the game was in play, or before the slots it named were loaded, has
    // nothing of its own to put in them: they hold the real thing, and a save now would push it
    // into the backups.
    let named = world.get_resource::<EngineConfig>().is_some_and(|c| c.world_slot.is_some() || c.character_slot.is_some());
    if !in_game || (named && !world.get_resource::<LaunchLoaded>().is_some_and(|l| l.0)) {
        journal::emit(world, JournalKind::Sim, "save: the session ended before the game was in play (or its --world / --character loaded): nothing was saved");
        return None;
    }
    // Not `save_now`: the clients' characters go out with `SessionEnd`, not twice. Each part on its
    // own: the world's slot is told to the clients whatever became of the host's character.
    let world_saved = save_part(world, SaveKind::World, None, &slots.world, "", "session ended");
    // A failure is in the journal (Error) already.
    let _ = save_part(world, SaveKind::Character, None, &slots.character, "", "session ended");
    world_saved.ok().filter(|report| report.written).map(|report| report.slot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sessions() -> SessionCharacters {
        let mut sessions = SessionCharacters::default();
        sessions.tokens.insert(1, "tok".into());
        sessions
    }

    #[test]
    fn a_character_travels_as_exact_text_and_reads_back() {
        let mut set = SectionSet::default();
        set.0.insert("vitals".into(), shards_save::SectionData { version: 1, data: json!({"hp": 0.1f32, "max": 100.0}) });
        let value = character_value(&set);
        assert!(value.is_string());
        assert_eq!(parse_character(&value).unwrap(), set);
        // An agent may type the sections as an object.
        assert_eq!(parse_character(&json!({"vitals": {"version": 1, "data": {"hp": 0.1f32, "max": 100.0}}})).unwrap().len(), 1);
        assert!(parse_character(&json!(42)).unwrap_err().contains("not a set of sections"));
        assert!(parse_character(&json!("{not json")).is_err());
    }

    #[test]
    fn what_a_player_brings_is_theirs_unless_the_host_sent_it() {
        let mut sessions = sessions();
        let (old, new, other) = (json!("old"), json!("new"), json!("played elsewhere"));
        // A first join: nothing remembered, the file wins; no file, nothing.
        assert_eq!(sessions.choose("tok", None, false, 0), None);
        assert_eq!(sessions.choose("tok", Some(&old), false, 1), Some(old.clone()));
        // ... and what the host applied is what it knows of them: their file is its copy until
        // it captures more. Its body stands: nothing is applied again.
        assert_eq!(sessions.get("tok"), Some((1, &old)));
        assert_eq!(sessions.choose("tok", Some(&old), true, 5), None, "the host's copy, and the body is the newest there is");

        // The host captured `new` (which never arrived).
        sessions.remember(1, 100, &new);
        assert_eq!(sessions.get("tok"), Some((100, &new)));
        // They come back with the file they joined with: the host's newest wins. Its body is gone: it is given.
        assert_eq!(sessions.choose("tok", Some(&old), false, 150), Some(new.clone()));
        // Its body still stands: it has the newest state already, nothing is applied.
        assert_eq!(sessions.choose("tok", Some(&old), true, 150), None);
        // ... unless the world was taken back to before the capture: the body is older than what the host kept.
        assert_eq!(sessions.choose("tok", Some(&old), true, 50), Some(new.clone()));
        // They bring nothing: the same.
        assert_eq!(sessions.choose("tok", None, false, 150), Some(new.clone()));
        assert_eq!(sessions.choose("tok", None, true, 150), None);
        // A file the host never had was made elsewhere: it is theirs, body or not, and then it is what the host knows.
        assert_eq!(sessions.choose("tok", Some(&other), true, 200), Some(other.clone()));
        assert_eq!(sessions.get("tok"), Some((200, &other)));
        assert_eq!(sessions.choose("tok", Some(&other), true, 210), None);
        // Another player's remembered character is not theirs to get, and no token is no one.
        assert_eq!(sessions.choose("someone-else", None, false, 0), None);
        assert_eq!(sessions.choose("", Some(&other), false, 0), Some(other));
        assert_eq!(sessions.len(), 1);
    }

    #[test]
    fn only_a_player_with_a_token_is_remembered_and_the_known_ones_are_bounded() {
        let mut sessions = SessionCharacters::default();
        sessions.remember(1, 5, &json!("x"));
        assert!(sessions.is_empty(), "nobody joined as p1 yet");
        sessions.tokens.insert(1, "tok".into());
        for n in 0..(KNOWN_SENT as u64 + 10) {
            sessions.remember(1, n, &json!(format!("save {n}")));
        }
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions.saved["tok"].sent.len(), KNOWN_SENT);
        // The same character sent again is one fingerprint.
        sessions.remember(1, 99, &json!("save 30"));
        assert_eq!(sessions.saved["tok"].sent.iter().filter(|p| **p == fingerprint(&json!("save 30"))).count(), 1);
    }
}
