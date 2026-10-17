//! The language a player plays in: a setting of their own (`locale.language`), changed with
//! `settings_set` like any other and kept in `settings.toml`.

use bevy::prelude::*;
use shards_content::ContentDb;
use shards_settings::prelude::*;

use crate::def::LocaleDef;

/// The player's language. Per machine, never part of the simulation: two players of one game
/// may read it in two languages.
#[derive(Resource, Reflect, SettingsGroup, Clone, Debug, Default, PartialEq)]
#[reflect(Resource, SettingsGroup, Default)]
#[settings_group(group = "locale")]
pub struct LocaleSettings {
    /// A language of the game's content (`en`, `ru`). Empty: the game's own, which is the
    /// first language (by id) that falls back to no other; a game that keeps its settings on disk
    /// fills it at its first run with the system's language, when it has that one.
    pub language: String,
}

/// `settings_set` takes only a language the game has.
pub(crate) fn known_language(_: &LocaleSettings, new: &LocaleSettings, world: &World) -> Result<(), String> {
    let wanted = new.language.trim();
    if wanted.is_empty() {
        return Ok(());
    }
    let db = world.resource::<ContentDb>();
    if db.get::<LocaleDef>(wanted).is_some() {
        return Ok(());
    }
    let all: Vec<String> = db.iter::<LocaleDef>().map(|(id, def)| format!("{id} ({})", def.name)).collect();
    if all.is_empty() {
        return Err(format!("`{wanted}`: this game has no languages (content `*.locale.ron`)"));
    }
    let hint = crate::def::closest_language(wanted, db.ids::<LocaleDef>()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
    Err(format!("`{wanted}` is not a language of this game{hint}: {}", all.join(", ")))
}
