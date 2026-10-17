//! How a game is started into a Steam session: `--steam-host` (a lobby), `--steam-server` (an
//! anonymous game server), `--join steam:<id>` (a lobby, or a host's own Steam id), and the
//! `+connect_lobby <lobby>` Steam adds to the command line when a friend's invite or "join game"
//! starts the game ([`connect_lobby_args`], which `shards::run` calls before it reads the line).

use std::ffi::OsString;

use crate::backend::SteamId;

/// What `--join steam:…` names. A Steam id says what it is (its account type), so one prefix is
/// never ambiguous: a lobby's id is a chat's, a host's is a user's or a game server's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SteamAddress {
    /// `steam:<lobby>`: enter this lobby; its owner is the host.
    Lobby(SteamId),
    /// `steam:<id>` of a user or a game server: that is the host, reached over the relay without a
    /// lobby (`--steam-server` prints its id).
    Host(SteamId),
    /// `steam:` alone: wait for an invite (or `steam_join`).
    Invite,
}

/// The prefix of a Steam address in `--join`.
pub const PREFIX: &str = "steam:";

/// What Steam puts on the command line of a game it starts into a lobby.
pub const CONNECT_LOBBY: &str = "+connect_lobby";

/// The account type of a Steam id (bits 52 to 55): 1 a user, 3 a game server, 4 an anonymous game
/// server, 8 a chat (lobbies are chats).
pub fn account_type(id: SteamId) -> u8 {
    ((id >> 52) & 0xF) as u8
}

/// What a Steam id is, as `--join steam:<id>` reads it.
pub fn classify(id: SteamId) -> Result<SteamAddress, String> {
    match account_type(id) {
        8 => Ok(SteamAddress::Lobby(id)),
        1 | 3 | 4 => Ok(SteamAddress::Host(id)),
        other => Err(format!("{id} is neither a lobby nor a player or game server (its account type is {other})")),
    }
}

/// `steam:109775240917319681` → a lobby, `steam:90071992547409921` → a game server to join directly,
/// `steam:` → waiting for an invite, anything that is not a Steam address → `None`.
pub fn parse_address(addr: &str) -> Option<Result<SteamAddress, String>> {
    let rest = addr.strip_prefix(PREFIX)?;
    let rest = rest.trim();
    if rest.is_empty() {
        return Some(Ok(SteamAddress::Invite));
    }
    let teach = "give the number `steam` shows on the host: its lobby (`steam:109775240917319681`) or its own id (a game server's, `steam:90071992547409921`); or `steam:` to wait for an invite";
    Some(match rest.parse::<SteamId>() {
        Ok(id) if id != 0 => classify(id).map_err(|e| format!("`{addr}`: {e}; {teach}")),
        _ => Err(format!("`{addr}` names no lobby or host: {teach}")),
    })
}

/// The command line with Steam's `+connect_lobby <lobby>` turned into `--join steam:<lobby>`, which
/// the game's own options understand. A `+connect_lobby` without a lobby number after it is an error.
/// Everything else stays as it was, in its place.
pub fn connect_lobby_args(args: impl IntoIterator<Item = OsString>) -> Result<Vec<OsString>, String> {
    let mut out = Vec::new();
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        if arg != CONNECT_LOBBY {
            out.push(arg);
            continue;
        }
        let lobby = args.next().ok_or_else(|| format!("{CONNECT_LOBBY} came without a lobby: Steam passes `{CONNECT_LOBBY} <lobby id>`"))?;
        let text = lobby.to_string_lossy().into_owned();
        match parse_address(&format!("{PREFIX}{text}")) {
            Some(Ok(SteamAddress::Lobby(id))) => {
                out.push("--join".into());
                out.push(format!("{PREFIX}{id}").into());
            }
            _ => return Err(format!("{CONNECT_LOBBY} {text}: that is no lobby id")),
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &[&str]) -> Vec<OsString> {
        line.iter().map(OsString::from).collect()
    }

    #[test]
    fn steam_addresses_name_a_lobby_a_host_or_wait_for_an_invite() {
        assert_eq!(parse_address("steam:109775240917319681"), Some(Ok(SteamAddress::Lobby(109775240917319681))));
        // A game server (anonymous: type 4) and a player (type 1) are hosts to join without a lobby.
        assert_eq!(parse_address("steam:90071992547409921"), Some(Ok(SteamAddress::Host(90071992547409921))));
        assert_eq!(parse_address("steam:76561197960265729"), Some(Ok(SteamAddress::Host(76561197960265729))));
        assert_eq!(parse_address("steam:"), Some(Ok(SteamAddress::Invite)));
        assert_eq!(parse_address("127.0.0.1:7777"), None);
        let bad = parse_address("steam:lobby").unwrap().unwrap_err();
        assert!(bad.contains("names no lobby or host") && bad.contains("steam:109775240917319681"), "{bad}");
        assert!(parse_address("steam:0").unwrap().is_err());
        // A clan (type 7) is neither.
        let clan = parse_address("steam:103582791429521408").unwrap().unwrap_err();
        assert!(clan.contains("account type is 7"), "{clan}");
        assert_eq!(account_type(crate::fake::FIRST_SERVER), 4);
        assert_eq!(account_type(crate::fake::LOBBY_BASE + 1), 8);
        assert_eq!(account_type(crate::fake::FIRST_USER), 1);
    }

    #[test]
    fn steam_s_connect_lobby_becomes_a_join_in_its_place() {
        let line = connect_lobby_args(args(&["game.exe", "+connect_lobby", "109775240917319681", "--headless"])).unwrap();
        assert_eq!(line, args(&["game.exe", "--join", "steam:109775240917319681", "--headless"]));
        // At the end too, and a line without it is left alone.
        let end = connect_lobby_args(args(&["game.exe", "--name", "b", "+connect_lobby", "109775240917319682"])).unwrap();
        assert_eq!(end, args(&["game.exe", "--name", "b", "--join", "steam:109775240917319682"]));
        // A player's or a game server's id is no lobby: Steam never passes one.
        assert!(connect_lobby_args(args(&["game.exe", "+connect_lobby", "90071992547409921"])).is_err());
        assert_eq!(connect_lobby_args(args(&["game.exe", "--seed", "3"])).unwrap(), args(&["game.exe", "--seed", "3"]));
    }

    #[test]
    fn a_connect_lobby_without_a_lobby_is_refused_with_what_steam_passes() {
        let missing = connect_lobby_args(args(&["game.exe", "+connect_lobby"])).unwrap_err();
        assert!(missing.contains("+connect_lobby <lobby id>"), "{missing}");
        let bad = connect_lobby_args(args(&["game.exe", "+connect_lobby", "--headless"])).unwrap_err();
        assert!(bad.contains("no lobby id"), "{bad}");
    }
}
