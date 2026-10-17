//! The language of the player's system, which a game takes at its first run: Windows names the
//! user's interface languages, Linux and macOS the `LANGUAGE`, `LC_ALL`, `LC_MESSAGES` and `LANG`
//! variables (the first that is set wins).

use ai_core::journal::{self, JournalKind};
use bevy::prelude::*;
use shards_settings::SettingsStorage;
use unic_langid::LanguageIdentifier;

use crate::settings::LocaleSettings;

/// The system's interface languages, most preferred first, as it names them (`ru-RU`, `en-US`).
/// Read instead of the system's when present: a test puts in the languages it plays a player of.
#[derive(Resource, Clone, Debug, Default, PartialEq, Eq)]
pub struct SystemLanguages(pub Vec<String>);

/// [`SystemLanguages`] when present, else the system's own.
fn system_languages(world: &World) -> Vec<String> {
    match world.get_resource::<SystemLanguages>() {
        Some(languages) => languages.0.clone(),
        None => sys_locale::get_locales().collect(),
    }
}

/// The first system language the game has, as `(the system's tag, the game's language)`: the same
/// tag (case and `_` aside), else a language of the game with the same language subtag, the one
/// without a region first (`ru-RU` plays `ru`, `pt-PT` plays `pt-BR` when that is all there is).
pub(crate) fn matching<'a>(system: &[String], game: &[&'a str]) -> Option<(String, &'a str)> {
    let base = |tag: &str| tag.replace('_', "-").parse::<LanguageIdentifier>().ok().map(|id| id.language);
    system.iter().find_map(|tag| {
        let same = game.iter().find(|id| id.replace('_', "-").eq_ignore_ascii_case(&tag.replace('_', "-")));
        let found = match same {
            Some(id) => id,
            None => {
                let language = base(tag)?;
                // Without a region or script before with one, then by id.
                game.iter().filter(|id| base(id) == Some(language)).min_by_key(|id| (id.contains(['-', '_']), **id))?
            }
        };
        Some((tag.clone(), *found))
    })
}

/// At a start whose settings are kept on disk and name no language yet: the first system
/// language the game has becomes the setting, which the store keeps (the next start reads it as
/// the player's own choice). A test, an agent's headless run and `--no-settings` keep no settings,
/// and play the game's own language, whatever the machine's is.
pub(crate) fn adopt(world: &mut World, game: &[&str]) {
    if !world.contains_resource::<SettingsStorage>() || game.is_empty() {
        return;
    }
    if world.get_resource::<LocaleSettings>().is_none_or(|s| !s.language.trim().is_empty()) {
        return;
    }
    let system = system_languages(world);
    let Some((tag, id)) = matching(&system, game) else {
        if !system.is_empty() {
            let text = format!("locale: the system's languages ({}) are none of this game's; playing its own", system.join(", "));
            journal::emit(world, JournalKind::Local, text);
        }
        return;
    };
    world.resource_mut::<LocaleSettings>().language = id.to_owned();
    shards_settings::save_soon_in(world);
    journal::emit(world, JournalKind::Local, format!("locale: the system's language is {tag}; playing {id} (kept in the settings)"));
}

#[cfg(test)]
mod tests {
    use super::matching;

    fn tags(tags: &[&str]) -> Vec<String> {
        tags.iter().map(|t| (*t).to_owned()).collect()
    }

    #[test]
    fn a_system_language_plays_the_games_language_of_the_same_tag_or_language() {
        let game = ["en", "pt-BR", "ru"];
        assert_eq!(matching(&tags(&["ru-RU", "en-US"]), &game), Some(("ru-RU".into(), "ru")));
        assert_eq!(matching(&tags(&["RU"]), &game), Some(("RU".into(), "ru")));
        assert_eq!(matching(&tags(&["pt_BR"]), &game), Some(("pt_BR".into(), "pt-BR")));
        assert_eq!(matching(&tags(&["pt-PT"]), &game), Some(("pt-PT".into(), "pt-BR")));
        // The first of the system's that the game has, in the system's order.
        assert_eq!(matching(&tags(&["de-DE", "en-GB", "ru-RU"]), &game), Some(("en-GB".into(), "en")));
        // POSIX's `C` and languages the game has not: none.
        assert_eq!(matching(&tags(&["C", "de-DE"]), &game), None);
        assert_eq!(matching(&[], &game), None);
        // Without a region before with one.
        assert_eq!(matching(&tags(&["en-AU"]), &["en-US", "en"]), Some(("en-AU".into(), "en")));
    }
}
