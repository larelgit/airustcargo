//! Looking for lobbies (`steam_lobbies`): the lobbies of this game that anyone may join, found by
//! the data a lobby host writes ([`crate::backend::data`]): the game's name and version, the engine's
//! version, the content hash and the network rules hash: what a host's `hello` check compares, so a
//! lobby of this build is one whose host will let a client in.
//!
//! Steam answers a search later, by callback. The tool waits for it, running Steam's callbacks
//! itself ([`session::pump`]), and says so when the answer does not come in time: the search goes on,
//! and the next call has its answer. Steam's search lists `public` and `invisible` lobbies only; a
//! `friends` or `private` lobby never appears, the host's own included, and the answer says so then.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use ai_core::snapshot::ENGINE_VERSION;
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::SteamLobby;
use shards_net::NetGame;

use crate::backend::{data, LobbyInfo};
use crate::session::{self, SteamRole, SteamSession, SteamState};
use crate::Steam;

/// How long `steam_lobbies` waits for Steam's answer unless told otherwise, and the longest it may.
pub(crate) const DEFAULT_WAIT_MS: u32 = 5_000;
pub(crate) const MAX_WAIT_MS: u32 = 30_000;

/// How often a waiting search runs Steam's callbacks.
const POLL: Duration = Duration::from_millis(10);

/// What a client must share with a lobby's host to be let in: the game, the engine's version, the
/// content and the network rules (the checks of the session's `hello`, as text).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Build {
    pub game: String,
    pub engine: String,
    /// `ContentDb::hash` in hex.
    pub content: String,
    /// `net_rules::rules_hash` in hex.
    pub rules: String,
}

impl Build {
    /// The build of this game.
    pub fn of(world: &World) -> Self {
        Self {
            game: world.get_resource::<NetGame>().cloned().unwrap_or_default().0,
            engine: ENGINE_VERSION.to_owned(),
            content: format!("{:016x}", world.get_resource::<ContentDb>().map_or(0, ContentDb::hash)),
            rules: format!("{:016x}", ai_core::net_rules::rules_hash(world)),
        }
    }

    /// The lobby data of a host with this build (the host's own name comes with it, from Steam).
    pub fn lobby_data(&self) -> [(&'static str, &str); 4] {
        [(data::GAME, &self.game), (data::ENGINE, &self.engine), (data::CONTENT, &self.content), (data::RULES, &self.rules)]
    }

    /// The pairs a search of this build's lobbies filters on; the game's alone for any build.
    fn filter(&self, any_build: bool) -> Vec<(&'static str, &str)> {
        let mut pairs = self.lobby_data().to_vec();
        if any_build {
            pairs.truncate(1);
        }
        pairs
    }

    /// Where a lobby's data differs from this build, as `engine 0.8.0 (yours 0.9.0)`; nothing when
    /// the lobby is of this build.
    fn differences(&self, lobby: &BTreeMap<String, String>) -> Vec<String> {
        self.lobby_data()
            .into_iter()
            .filter_map(|(key, ours)| match lobby.get(key) {
                Some(theirs) if theirs == ours => None,
                Some(theirs) => Some(format!("{key} {theirs} (yours {ours})")),
                None => Some(format!("{key} not set (yours {ours})")),
            })
            .collect()
    }
}

/// A lobby search: asked of Steam, and the last answer.
#[derive(Debug, Default)]
pub(crate) struct Search {
    /// Asked and not answered yet, with whether it listed lobbies of any build.
    pub running: Option<bool>,
    /// What Steam answered last.
    pub answer: Option<Result<Vec<LobbyInfo>, String>>,
}

/// Steam answered the search (a stray answer, with none asked for, is dropped).
pub(crate) fn answered(world: &mut World, answer: Result<Vec<LobbyInfo>, String>) {
    let mut session = world.resource_mut::<SteamSession>();
    if session.search.running.take().is_some() {
        session.search.answer = Some(answer);
    }
}

/// `steam_lobbies`: searches (or joins the search that waits), runs Steam's callbacks until it is
/// answered or `wait` is over, and says what was found.
pub(crate) fn search(world: &mut World, any_build: bool, wait: Duration) -> Result<String, String> {
    let steam = world.get_resource::<Steam>().ok_or("this game has no Steam module")?;
    if let Some(why) = session::unavailable(steam.backend.as_ref()) {
        return Err(why);
    }
    // The content's hash is the build's: until the content is loaded it is a partial one, and no lobby matches.
    if !world.get_resource::<ContentDb>().is_some_and(ContentDb::is_loaded) {
        return Err("this game is still loading its content, so its build is not known yet: ask again when it is loaded (`shards wait`)".into());
    }
    let build = Build::of(world);
    match world.resource::<SteamSession>().search.running {
        Some(earlier) if earlier != any_build => {
            return Err(format!("an earlier search (all={earlier}) still waits for Steam's answer: ask again with all={earlier}, or in a moment"));
        }
        Some(_) => {}
        None => {
            world.resource_mut::<Steam>().backend.find_lobbies(&build.filter(any_build)).map_err(|e| format!("Steam could not search for lobbies: {e}"))?;
            let search = &mut world.resource_mut::<SteamSession>().search;
            search.running = Some(any_build);
            search.answer = None;
        }
    }
    let started = Instant::now();
    loop {
        session::pump(world);
        if world.resource::<SteamSession>().search.running.is_none() {
            break;
        }
        if started.elapsed() >= wait {
            return Err(format!(
                "Steam has not answered the lobby search within {} ms; it goes on: call steam_lobbies again (wait_ms=… waits longer)",
                wait.as_millis()
            ));
        }
        std::thread::sleep(POLL);
    }
    match world.resource_mut::<SteamSession>().search.answer.take() {
        Some(Ok(found)) => Ok(describe(world, &build, any_build, &found)),
        Some(Err(why)) => Err(why),
        None => Err("the lobby search ended without an answer".into()),
    }
}

/// `1 lobby`, `2 lobbies`.
fn lobbies(n: usize) -> String {
    format!("{n} {}", if n == 1 { "lobby" } else { "lobbies" })
}

/// The answer as text: a line per lobby, and what the search cannot show.
fn describe(world: &World, build: &Build, any_build: bool, found: &[LobbyInfo]) -> String {
    let backend = world.resource::<Steam>().backend.as_ref();
    let session = world.resource::<SteamSession>();
    let own = session.lobby_id();
    let scope = if any_build { "with a free slot, of any build" } else { "of this build with a free slot" };
    let mut out = String::new();
    if found.is_empty() {
        out.push_str(&format!("no lobby of `{}` {scope} answers the search\n", build.game));
    } else {
        out.push_str(&format!("{} of `{}` {scope}:\n", lobbies(found.len()), build.game));
    }
    let mut joinable = false;
    for lobby in found {
        let host = match (lobby.data.get(data::HOST), lobby.owner) {
            (Some(name), Some(owner)) => format!("{name} ({owner})"),
            (Some(name), None) => name.clone(),
            (None, Some(owner)) => session::who(backend, owner),
            (None, None) => "unknown".to_owned(),
        };
        let differences = build.differences(&lobby.data);
        let build_says = if differences.is_empty() {
            joinable = true;
            "build matches".to_owned()
        } else {
            format!("build differs: {}", differences.join(", "))
        };
        let here = if Some(lobby.lobby) == own { " · this game's lobby" } else { "" };
        out.push_str(&format!("lobby {} · host {host} · {} of {} · {build_says}{here}\n", lobby.lobby, lobby.members, lobby.max));
    }
    if joinable && session.role == SteamRole::Client {
        out.push_str("enter one with steam_join lobby=ID\n");
    }
    if found.is_empty() {
        out.push_str("Steam's search lists public and invisible lobbies only: a host started with --steam-lobby friends (the default) or private is never in it\n");
        if !any_build {
            out.push_str("all=true lists the lobbies of other builds of this game too\n");
        }
    }
    if let Some(note) = own_lobby_note(session, found) {
        out.push_str(&note);
        out.push('\n');
    }
    out.truncate(out.trim_end().len());
    out
}

/// Why the lobby this game hosts is not in the answer, when it is not.
fn own_lobby_note(session: &SteamSession, found: &[LobbyInfo]) -> Option<String> {
    let SteamState::InLobby { lobby, .. } = session.state else { return None };
    if session.role != SteamRole::Host || found.iter().any(|l| l.lobby == lobby) {
        return None;
    }
    let kind: SteamLobby = session.lobby_type?;
    Some(if kind.in_search() {
        format!(
            "note: this game's own lobby {lobby} ({}) is not in the answer: it has no free slot ({} in it), or Steam's list has not caught up with it yet",
            kind.as_str(),
            session.members.len()
        )
    } else {
        format!(
            "note: this game's own lobby {lobby} ({}) is not in the answer: Steam's search never lists friends-only or private lobbies; host with --steam-lobby public or invisible to be found",
            kind.as_str()
        )
    })
}
