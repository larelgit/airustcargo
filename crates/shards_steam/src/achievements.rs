//! Achievements as content (`content/**/*.achievement.ron`), unlocked by the game: an id, its name on
//! Steam, its name and description as messages the player reads in their language, and whether
//! Steam hides it until it is unlocked.
//!
//! ```ron
//! {
//!     "first_blood": (steam: "ACH_FIRST_BLOOD", name: "@ach-first-blood", description: "@ach-first-blood-desc"),
//!     "warden_down": (steam: "ACH_WARDEN", name: "@ach-warden", description: "@ach-warden-desc", hidden: true),
//! }
//! ```
//!
//! The game unlocks one where its simulation decides (the host's in co-op) by triggering
//! [`UnlockAchievement`] (for every player in the session, or one of them) or by calling
//! [`unlock_achievement`] (this machine's player). The host sends the event to its clients, and each
//! machine unlocks it for its own player on its own Steam account; a client never unlocks one by
//! itself. Nothing is unlocked while a replay plays (its unlocks were the recording's).
//!
//! Achievements are not the simulation's: they change nothing in the world, and neither the replay's
//! hash nor a snapshot holds them. The module keeps this run's record ([`Achievements`]), so a game
//! without Steam (tests, a build without the SDK) sees what it unlocked; Steam is told at once and
//! its store follows a second later (once for several unlocks), and on exit.

use std::time::{Duration, Instant};

use ai_core::journal::{self, JournalKind};
use ai_core::{sim, AiReplay};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::{ContentDb, ContentDef, Refs};
use shards_core::NetRole;
use shards_net::NetClient;

use crate::Steam;

/// An achievement (`content/**/*.achievement.ron`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AchievementDef {
    /// Its API name on Steam (Steamworks, Stats & Achievements): `ACH_FIRST_BLOOD`. Letters, digits
    /// and `_`; one name is one achievement.
    pub steam: String,
    /// Its name as the player reads it: a message (`@ach-first-blood`) in every language.
    pub name: String,
    /// What it asks of the player: a message too.
    pub description: String,
    /// Steam shows a hidden achievement's name and description only once it is unlocked.
    pub hidden: bool,
}

/// The longest API name Steam takes.
pub const MAX_STEAM_NAME: usize = 128;

impl ContentDef for AchievementDef {
    const KIND: &'static str = "achievement";

    fn validate(&self, id: &str, refs: &mut Refs) {
        if self.steam.is_empty() {
            refs.error("steam: the achievement's API name on Steam is missing (Steamworks, Stats & Achievements: `ACH_…`)");
        } else if self.steam.len() > MAX_STEAM_NAME || !self.steam.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            refs.error(format!("steam: `{}` is no Steam API name: letters, digits and `_`, up to {MAX_STEAM_NAME}", self.steam));
        }
        // One name on Steam is one achievement: the second definition is the mistake.
        let first = refs.iter::<AchievementDef>().filter(|(other, def)| *other != id && def.steam == self.steam && !self.steam.is_empty()).map(|(other, _)| other).min();
        if let Some(other) = first.filter(|other| *other < id) {
            refs.error(format!("steam: `{}` is achievement `{other}`'s already: one name on Steam is one achievement", self.steam));
        }
        for (field, text) in [("name", &self.name), ("description", &self.description)] {
            if shards_locale::message_ref(text).is_none() {
                refs.error(format!(
                    "{field}: the player reads it in their language: name a message, `@ach-{}` (assets/locale/<language>/*.ftl), not `{text}`",
                    id.replace('_', "-")
                ));
            }
        }
        shards_locale::check_messages(refs, [(self.name.as_str(), "name".to_owned()), (self.description.as_str(), "description".to_owned())]);
    }
}

/// Unlocks achievement `id` (`content/**/*.achievement.ron`): for player `player` only, or for every
/// player in the session (`None`; in a single game, the player). Trigger it where the game decides
/// (gameplay runs on the host in co-op): the host sends it to its clients, and each machine unlocks it
/// for its own player.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct UnlockAchievement {
    pub id: String,
    pub player: Option<u8>,
}

impl UnlockAchievement {
    /// For every player.
    pub fn everyone(id: impl Into<String>) -> Self {
        Self { id: id.into(), player: None }
    }

    /// For player `n` (0: the host's, or a single game's).
    pub fn player(id: impl Into<String>, n: u8) -> Self {
        Self { id: id.into(), player: Some(n) }
    }
}

/// An achievement this game unlocked in this run.
#[derive(Clone, Debug, PartialEq)]
pub struct Unlocked {
    pub id: String,
    /// The tick it was unlocked at.
    pub tick: u64,
    /// Whether Steam took it, or why not (no Steam: this run's record only).
    pub on_steam: Result<(), String>,
}

/// This run's achievements: what was unlocked, and Steam's stores.
#[derive(Resource, Debug, Default)]
pub struct Achievements {
    /// In the order they were unlocked.
    pub unlocked: Vec<Unlocked>,
    /// Stores sent to Steam.
    pub stores: u32,
    /// Why the last store failed, if it did.
    pub store_error: Option<String>,
    /// When Steam is to be told: a second after the first unlock not stored yet.
    store_due: Option<Instant>,
}

impl Achievements {
    pub fn is_unlocked(&self, id: &str) -> bool {
        self.unlocked.iter().any(|u| u.id == id)
    }

    /// Unlocks waiting for their store.
    pub fn store_pending(&self) -> bool {
        self.store_due.is_some()
    }
}

/// How long unlocks wait before Steam stores them: several unlocks at once make one store.
pub const STORE_AFTER: Duration = Duration::from_secs(1);

/// Unlocks achievement `id` for this machine's player now. Errors: no such achievement, a replay
/// plays, or this machine is a co-op client (its achievements come from the host's
/// [`UnlockAchievement`]). Answers what happened: `unlocked first_blood (First blood)`, or that it was
/// unlocked already.
pub fn unlock_achievement(world: &mut World, id: &str) -> Result<String, String> {
    if world.get_resource::<NetRole>().is_some_and(|r| !r.simulates()) {
        return Err(CLIENT.into());
    }
    grant(world, id)
}

/// Why a client does not unlock achievements itself.
pub const CLIENT: &str =
    "a co-op client does not unlock achievements itself: the host's game decides, and its UnlockAchievement reaches every player it names (call it on the host)";

/// This machine's player in the session: 0 on the host and in a single game, a client's own.
fn local_player(world: &World) -> Option<u8> {
    match world.get_resource::<NetClient>() {
        Some(client) => client.player,
        None => Some(0),
    }
}

/// [`UnlockAchievement`]: the achievement, for this machine's player when it is theirs. Runs on a
/// client too: there it is the host's decision, sent over.
pub(crate) fn on_unlock(event: On<UnlockAchievement>, mut commands: Commands) {
    let event = event.event().clone();
    commands.queue(move |world: &mut World| {
        earn(world, &event);
    });
}

/// The event's achievement for this machine's player, if it names them: the answer, `None` when it
/// is another player's. A mistake (an id no file defines) is journaled; a replay's or a second
/// unlock is not.
pub(crate) fn earn(world: &mut World, event: &UnlockAchievement) -> Option<Result<String, String>> {
    if event.player.is_some_and(|p| local_player(world) != Some(p)) {
        return None;
    }
    let result = grant(world, &event.id);
    if let Err(e) = &result
        && !replay_playing(world)
    {
        journal::emit(world, JournalKind::Warn, format!("achievement: {e}"));
    }
    Some(result)
}

fn replay_playing(world: &World) -> bool {
    world.get_resource::<AiReplay>().is_some_and(AiReplay::is_playing)
}

/// The achievement's definition, or an error that names the ones there are.
pub(crate) fn definition(world: &World, id: &str) -> Result<AchievementDef, String> {
    let db = world.get_resource::<ContentDb>().ok_or("this game has no content")?;
    if let Some(def) = db.get::<AchievementDef>(id) {
        return Ok(def.clone());
    }
    let ids: Vec<&str> = db.ids::<AchievementDef>().collect();
    if ids.is_empty() {
        return Err(format!("no achievement `{id}`: this game defines none (content/**/*.achievement.ron)"));
    }
    let hint = shards_content::db::closest(id, ids.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
    Err(format!("no achievement `{id}`{hint}; there are: {}", ids.join(", ")))
}

/// The achievement's name in the language played.
pub(crate) fn display_name(world: &World, def: &AchievementDef) -> String {
    shards_locale::resolve(world, &def.name)
}

/// Unlocks `id` here: this run's record, then Steam (stored a moment later).
fn grant(world: &mut World, id: &str) -> Result<String, String> {
    let def = definition(world, id)?;
    if replay_playing(world) {
        return Err(format!("`{id}` is not unlocked while a replay plays: its unlocks were the recording's"));
    }
    let name = display_name(world, &def);
    if world.resource::<Achievements>().is_unlocked(id) {
        return Ok(format!("{id} ({name}) was unlocked already"));
    }
    let tick = sim::tick(world);
    let on_steam = {
        let mut steam = world.resource_mut::<Steam>();
        if steam.backend.achievement_get(&def.steam) == Some(true) {
            Ok(None)
        } else {
            steam.backend.achievement_set(&def.steam).map(Some)
        }
    };
    let mut achievements = world.resource_mut::<Achievements>();
    let (record, answer) = match on_steam {
        // Steam had it from an earlier run: nothing to store.
        Ok(None) => (Ok(()), format!("unlocked {id} ({name}); Steam had it already")),
        Ok(Some(())) => {
            achievements.store_due.get_or_insert_with(|| Instant::now() + STORE_AFTER);
            (Ok(()), format!("unlocked {id} ({name}) on Steam as {}", def.steam))
        }
        Err(e) => (Err(e.clone()), format!("unlocked {id} ({name}) in this run only: {e}")),
    };
    achievements.unlocked.push(Unlocked { id: id.to_owned(), tick, on_steam: record });
    journal::log(world, JournalKind::Local, format!("achievement unlocked: {name} ({id})"));
    Ok(answer)
}

/// `Last`: Steam stores what was unlocked once a second has passed since the first unlock.
pub(crate) fn store_when_due(world: &mut World) {
    if world.get_resource::<Achievements>().and_then(|a| a.store_due).is_some_and(|due| Instant::now() >= due) {
        store(world);
    }
}

/// Stores the unlocks waiting for it on Steam now (the exit does).
pub fn flush_achievements(world: &mut World) {
    if world.get_resource::<Achievements>().is_some_and(Achievements::store_pending) {
        store(world);
    }
}

fn store(world: &mut World) {
    match world.resource_mut::<Steam>().backend.store_stats() {
        Ok(()) => {
            let mut achievements = world.resource_mut::<Achievements>();
            achievements.store_due = None;
            achievements.stores += 1;
            achievements.store_error = None;
        }
        Err(e) => refused(world, e),
    }
}

/// What Steam said of a store.
pub(crate) fn stored(world: &mut World, result: Result<(), String>) {
    if let Err(e) = result {
        refused(world, e);
    }
}

/// A store Steam refused is tried again a moment later, and at exit. The journal warns once per reason, not
/// once a second while Steam keeps refusing.
fn refused(world: &mut World, e: String) {
    let mut achievements = world.resource_mut::<Achievements>();
    achievements.store_due = Some(Instant::now() + STORE_AFTER);
    if achievements.store_error.as_ref() != Some(&e) {
        achievements.store_error = Some(e.clone());
        journal::emit(world, JournalKind::Warn, format!("steam: achievements not stored: {e}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_events_say_whose_achievement_it_is() {
        assert_eq!(UnlockAchievement::everyone("win"), UnlockAchievement { id: "win".into(), player: None });
        assert_eq!(UnlockAchievement::player("win", 2).player, Some(2));
    }
}
