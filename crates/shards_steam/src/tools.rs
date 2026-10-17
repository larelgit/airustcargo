//! The Steam module's tools: `steam` (who this machine is on Steam, its lobby, the relay,
//! achievements, cloud), `steam_lobbies`, `steam_host`, `steam_join`, `steam_invite`,
//! `achievement_unlock`; the `ai/status` field `steam`.

use std::time::Duration;

use ai_core::tools::{register_tool, NetRoute, ToolKind};
use ai_core::{AiTools, Call, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use shards_content::ContentDb;
use shards_core::NetRole;
use shards_net::NetHost;

use crate::achievements::{self, AchievementDef, Achievements, UnlockAchievement};
use crate::backend::SteamId;
use crate::cloud::Cloud;
use crate::lobbies;
use crate::session::{self, SteamRole, SteamSession, SteamState};
use crate::Steam;

pub(crate) fn register(world: &mut World) {
    register_tool(world, "steam", Scope::Player, ToolKind::Query, steam);
    register_tool(world, "steam_lobbies", Scope::Player, ToolKind::Query, steam_lobbies);
    register_tool(world, "steam_host", Scope::Dev, ToolKind::Control, steam_host);
    register_tool(world, "steam_join", Scope::Dev, ToolKind::Control, steam_join);
    register_tool(world, "steam_invite", Scope::Player, ToolKind::Control, steam_invite);
    register_tool(world, "achievement_unlock", Scope::Dev, ToolKind::Control, achievement_unlock);
    let mut tools = world.resource_mut::<AiTools>();
    if let Err(e) = tools.set_route("achievement_unlock", NetRoute::HostOnly(achievements::CLIENT)) {
        warn!("steam: {e}");
    }
}

/// A Steam id as a number or as text (`109775240917319681`).
#[derive(Deserialize, JsonSchema, Clone, Debug)]
#[serde(untagged)]
pub enum IdArg {
    Number(u64),
    Text(String),
}

impl IdArg {
    fn id(&self, what: &str) -> Result<SteamId, String> {
        let id = match self {
            IdArg::Number(n) => Some(*n),
            IdArg::Text(text) => text.trim().trim_start_matches("steam:").parse().ok(),
        };
        id.filter(|id| *id != 0).ok_or_else(|| format!("{what}: give the Steam id, the number `steam` shows (`109775240917319681`)"))
    }
}

fn who(world: &World, user: SteamId) -> String {
    world.get_resource::<Steam>().map_or_else(|| user.to_string(), |s| session::who(s.backend.as_ref(), user))
}

fn bytes(n: u64) -> String {
    match n {
        0..1_000 => format!("{n} B"),
        1_000..1_000_000 => format!("{:.1} KB", n as f64 / 1e3),
        _ => format!("{:.1} MB", n as f64 / 1e6),
    }
}

/// Who this machine is on Steam (the client as a player, an anonymous game server, a fake of this
/// process, or none and why); its session (lobby and members, the game server's id, the host it
/// joined); the relay: what went over it, sessions refused, and the route to each peer (state,
/// whether a Steam relay carries it and which, ping); invites not followed; the achievements
/// unlocked in this run; what Steam Cloud did with the saves.
#[derive(Deserialize, JsonSchema, Default)]
pub struct SteamArgs {}

fn steam(In(_): In<Call<SteamArgs>>, world: &World) -> ToolResult {
    let steam = world.get_resource::<Steam>().ok_or("this game has no Steam module")?;
    let info = steam.backend.info();
    let named = |id: SteamId| if info.name.is_empty() { id.to_string() } else { format!("{} ({id})", info.name) };
    let app = info.app_id.map_or("?".into(), |a| a.to_string());
    let mut out = match (info.kind, info.user) {
        ("steam", Some(id)) => format!("steam: the Steam client · user {} · app {app}\n", named(id)),
        ("steam server", Some(id)) => format!("steam: an anonymous Steam game server · id {id} · app {app}\n"),
        ("steam server", None) => format!("steam: an anonymous Steam game server, logging on · app {app}\n"),
        ("fake", Some(id)) => format!("steam: a fake Steam in this process (this run does not talk to Steam) · user {}\n", named(id)),
        ("fake server", Some(id)) => format!("steam: a fake Steam game server in this process · id {id}\n"),
        _ => format!("steam: unavailable: {}\n", info.note.clone().unwrap_or_default()),
    };
    // A Steam that is there but stopped answering (its callbacks panicked).
    if let Some(note) = info.note.as_ref().filter(|_| steam.backend.available()) {
        out.push_str(&format!("problem: {note}\n"));
    }
    let s = world.resource::<SteamSession>();
    let members = |list: &[SteamId]| list.iter().map(|m| who(world, *m)).collect::<Vec<_>>().join(", ");
    let state = match &s.state {
        SteamState::None => "none".to_owned(),
        SteamState::ToCreate | SteamState::Creating => "lobby being created".to_owned(),
        SteamState::LoggingOn => "logging on to Steam".to_owned(),
        SteamState::Serving { id } => format!("serving as {id}: players join with --join steam:{id}"),
        SteamState::WaitingForInvite => "waiting for an invite (or steam_join lobby=ID)".to_owned(),
        SteamState::ToJoin(l) | SteamState::Joining(l) => format!("entering lobby {l}"),
        SteamState::ToConnect(host) => format!("joining {}", who(world, *host)),
        SteamState::InLobby { lobby, owner } => {
            let kind = s.lobby_type.map(|k| format!(" ({})", k.describe())).unwrap_or_default();
            format!("lobby {lobby}{kind}, host {} · {} in it: {}", who(world, *owner), s.members.len(), members(&s.members))
        }
        SteamState::Direct { host } => format!("on {} without a lobby", who(world, *host)),
        SteamState::Invalid(why) | SteamState::Failed(why) => format!("none: {why}"),
    };
    out.push_str(&match s.role {
        SteamRole::None => "session: none over Steam (start the game with --steam-host, --steam-server, or --join steam:ID)\n".to_owned(),
        SteamRole::Host => format!("session: host · {state}\n"),
        SteamRole::Server => format!("session: game server · {state}\n"),
        SteamRole::Client => format!("session: client · {state}\n"),
    });
    let mut relay = format!("relay: {}", steam.backend.relay_status());
    if info.relay_only {
        relay.push_str(" · relay only (direct routes off)");
    }
    let stats = s.relay_stats();
    if let Some(stats) = &stats {
        relay.push_str(&format!(" · sent {} ({}), received {} ({})", stats.sent, bytes(stats.sent_bytes), stats.received, bytes(stats.received_bytes)));
        if stats.errors > 0 {
            relay.push_str(&format!(", {} sends failed (last: {})", stats.errors, stats.last_error.clone().unwrap_or_default()));
        }
    }
    if s.refused > 0 {
        relay.push_str(&format!(" · {} sessions refused", s.refused));
    }
    out.push_str(&relay);
    out.push('\n');
    for peer in stats.map(|s| s.peers).unwrap_or_default() {
        let line = match steam.backend.route(peer) {
            Some(route) => {
                let mut line = route.state.clone();
                line.push_str(&match (route.relayed, &route.relay_pop) {
                    (true, Some(pop)) => format!(" · relayed via {pop}"),
                    (true, None) => " · relayed".to_owned(),
                    (false, _) => " · direct".to_owned(),
                });
                if let Some(ping) = route.ping_ms {
                    line.push_str(&format!(" · ping {ping} ms"));
                }
                line
            }
            None => "no session".to_owned(),
        };
        out.push_str(&format!("route to {}: {line}\n", who(world, peer)));
    }
    for (lobby, from) in &s.invites {
        out.push_str(&format!("invite not followed: lobby {lobby} from {}\n", who(world, *from)));
    }

    let defined = world.get_resource::<ContentDb>().map_or(0, |db| db.ids::<AchievementDef>().count());
    let a = world.resource::<Achievements>();
    let unlocked: Vec<String> = a
        .unlocked
        .iter()
        .map(|u| {
            let name = achievements::definition(world, &u.id).map(|d| achievements::display_name(world, &d)).unwrap_or_default();
            match &u.on_steam {
                Ok(()) => format!("{} ({name}) at tick {}", u.id, u.tick),
                Err(e) => format!("{} ({name}) at tick {}, this run only: {e}", u.id, u.tick),
            }
        })
        .collect();
    let mut line = format!("achievements: {defined} defined, {} unlocked in this run", unlocked.len());
    if !unlocked.is_empty() {
        line.push_str(&format!(": {}", unlocked.join("; ")));
    }
    line.push_str(&format!(" · stored on Steam {} times", a.stores));
    if a.store_pending() {
        line.push_str(", a store waits");
    }
    if let Some(e) = &a.store_error {
        line.push_str(&format!(", the last store failed: {e}"));
    }
    out.push_str(&line);
    out.push('\n');

    let cloud = world.resource::<Cloud>();
    let mut line = match &cloud.enabled {
        Some(Ok(())) => format!("cloud: on · {} saves copied as they were written", cloud.mirrored),
        Some(Err(why)) => format!("cloud: off ({why})"),
        None => "cloud: not checked yet".to_owned(),
    };
    if let (Some(Ok(())), Some((total, free))) = (&cloud.enabled, steam.backend.cloud_quota()) {
        let size = shards_save::ops::size_text;
        line.push_str(&format!(" · {} free of {}", size(free), size(total)));
    }
    for (what, list) in [
        ("taken from the cloud", &cloud.pulled),
        ("sent up", &cloud.pushed),
        ("kept both", &cloud.kept_both),
        ("from a newer version of the game, left alone", &cloud.newer),
    ] {
        if !list.is_empty() {
            line.push_str(&format!(" · at startup {what}: {}", list.join(", ")));
        }
    }
    if let Some(e) = &cloud.last_error {
        line.push_str(&format!(" · the last copy failed: {e}"));
    }
    out.push_str(&line);
    Ok(out)
}

/// Search Steam for lobbies of this game that a player can join: one with a free slot, of this
/// build (the same engine version, content and network rules: the host's checks), one line each:
/// the lobby's id, its host, how many are in it and how many fit, and whether its build matches.
/// Enter one with steam_join lobby=ID. Steam's search lists only public and invisible lobbies (the
/// host's --steam-lobby): a friends-only or private lobby never appears, this game's own included,
/// and the answer says so when its own is not found. Waits for Steam's answer.
#[derive(Deserialize, JsonSchema, Default)]
pub struct SteamLobbiesArgs {
    /// Also list the lobbies of this game on another build (they would refuse you), saying what differs.
    pub all: Option<bool>,
    /// How long to wait for Steam's answer, in milliseconds (default and at most 5000: the wait holds
    /// the game's frame). A search that outlasts it goes on, and the next call says its answer.
    pub wait_ms: Option<u32>,
}

fn steam_lobbies(In(call): In<Call<SteamLobbiesArgs>>, world: &mut World) -> ToolResult {
    let wait = call.args.wait_ms.unwrap_or(lobbies::WAIT_MS).min(lobbies::WAIT_MS);
    lobbies::search(world, call.args.all.unwrap_or(false), Duration::from_millis(u64::from(wait)))
}

/// Host in a Steam lobby: a game started with --steam-host makes its lobby again after a failure
/// (no Steam then, a lost connection); a game server (--steam-server) says whether it is on. A
/// running game does not become a host: start it with --steam-host or --steam-server.
#[derive(Deserialize, JsonSchema, Default)]
pub struct SteamHostArgs {}

fn steam_host(In(_): In<Call<SteamHostArgs>>, world: &mut World) -> ToolResult {
    session::host_again(world)
}

/// Enter Steam lobby `lobby`, or join host `host` by its Steam id (a game server: no lobby), and
/// play there: for a game started with --join steam: (waiting for an invite) or one that could not
/// get into its session.
#[derive(Deserialize, JsonSchema, Default)]
pub struct SteamJoinArgs {
    /// The lobby's id, as `steam` shows it on the host (`109775240917319681`) or `steam_lobbies` lists it.
    pub lobby: Option<IdArg>,
    /// The host's own Steam id, as `steam` shows it on a game server (`90071992547409921`).
    pub host: Option<IdArg>,
}

fn steam_join(In(call): In<Call<SteamJoinArgs>>, world: &mut World) -> ToolResult {
    let id = match (&call.args.lobby, &call.args.host) {
        (Some(lobby), None) => lobby.id("lobby")?,
        (None, Some(host)) => host.id("host")?,
        _ => return Err("name one: lobby=ID (a lobby) or host=ID (a game server's or player's Steam id)".into()),
    };
    session::join(world, id)
}

/// Invite a friend into this game's lobby: `friend=STEAMID` sends the invite, without it Steam's
/// invite dialog opens in the overlay (a window). The friend's game starts with the lobby, or a
/// running one enters it.
#[derive(Deserialize, JsonSchema, Default)]
pub struct SteamInviteArgs {
    /// The friend's Steam id (`76561197960265730`).
    pub friend: Option<IdArg>,
}

fn steam_invite(In(call): In<Call<SteamInviteArgs>>, world: &mut World) -> ToolResult {
    let friend = call.args.friend.map(|f| f.id("friend")).transpose()?;
    session::invite(world, friend)
}

/// Unlock achievement `id` (content/**/*.achievement.ron), as the game would: for every player of
/// the session, or for player `player` only. On the host the event goes to the clients too, and
/// each machine unlocks it on its own Steam account. Answers what happened here; nothing is
/// unlocked while a replay plays.
#[derive(Deserialize, JsonSchema)]
pub struct AchievementUnlockArgs {
    /// The achievement's id in its content file (`first_blood`), not its name on Steam.
    pub id: String,
    /// Only this player (0: the host's, or a single game's); default every player.
    pub player: Option<u8>,
}

fn achievement_unlock(In(call): In<Call<AchievementUnlockArgs>>, world: &mut World) -> ToolResult {
    if world.get_resource::<NetRole>().is_some_and(|r| !r.simulates()) {
        return Err(achievements::CLIENT.into());
    }
    let def = achievements::definition(world, &call.args.id)?;
    let event = UnlockAchievement { id: call.args.id.clone(), player: call.args.player };
    let here = achievements::earn(world, &event);
    let clients = world.get_resource::<NetHost>().map_or(0, |h| h.peers.len());
    if clients > 0 {
        // The clients' copies: the session sends the event (here it is unlocked already).
        world.trigger(event.clone());
        world.flush();
    }
    let name = achievements::display_name(world, &def);
    let mut answer = match here {
        Some(result) => result?,
        None => format!("{} ({name}) is player {}'s: not this machine's", call.args.id, call.args.player.unwrap_or_default()),
    };
    if clients > 0 {
        answer.push_str(&format!("; sent to the session's {clients} clients"));
    }
    Ok(answer)
}

/// `ai/status` field `steam`.
pub(crate) fn status(world: &World) -> Value {
    let Some(steam) = world.get_resource::<Steam>() else { return Value::Null };
    let info = steam.backend.info();
    let s = world.resource::<SteamSession>();
    let role = match s.role {
        SteamRole::None => "none",
        SteamRole::Host => "host",
        SteamRole::Server => "server",
        SteamRole::Client => "client",
    };
    let relay = s.relay_stats().map(|r| {
        let routes: Vec<Value> = r
            .peers
            .iter()
            .filter_map(|peer| {
                let route = steam.backend.route(*peer)?;
                Some(json!({"peer": peer.to_string(), "state": route.state, "relayed": route.relayed, "pop": route.relay_pop, "ping_ms": route.ping_ms}))
            })
            .collect();
        json!({"sent": r.sent, "received": r.received, "errors": r.errors, "relay_only": info.relay_only, "routes": routes})
    });
    json!({
        "kind": info.kind,
        "user": info.user.map(|u| u.to_string()),
        "note": info.note,
        "role": role,
        "lobby": s.lobby_id().map(|l| l.to_string()),
        "lobby_type": s.lobby_type.map(|k| k.as_str()),
        "host": s.host_id().map(|h| h.to_string()),
        "members": s.members.len(),
        "relay": relay,
        "achievements": world.resource::<Achievements>().unlocked.len(),
        "cloud": matches!(world.resource::<Cloud>().enabled, Some(Ok(()))),
    })
}
