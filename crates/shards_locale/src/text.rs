//! The text of the language played: messages formatted with their parameters, a missing one
//! taken from the language's fallback.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use bevy::prelude::*;
use fluent_bundle::concurrent::FluentBundle;
use fluent_bundle::{FluentArgs, FluentError, FluentResource, FluentValue};
use serde_json::Value;
use unic_langid::LanguageIdentifier;

/// How deep messages put into messages (`Arg::Message`) may go: deeper is a loop.
const MAX_DEPTH: u8 = 4;

/// A value for a message's `$parameter`.
#[derive(Clone, Debug, PartialEq)]
pub enum Arg {
    /// Shown as it is: a name.
    Text(String),
    /// A number: Fluent picks the plural form by it (`{ $count -> [one] … *[other] … }`).
    Number(f64),
    /// Another message of the same language, a whole phrase put into this one (`$place` is
    /// `place-yard`): text is built from whole phrases, never from pieces glued in code.
    Message(String),
}

/// The parameters of a message: `Args::new().number("count", 2).text("name", "Ada")`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Args(pub Vec<(String, Arg)>);

impl Args {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn text(mut self, name: &str, value: impl Into<String>) -> Self {
        self.0.push((name.to_owned(), Arg::Text(value.into())));
        self
    }

    pub fn number(mut self, name: &str, value: impl Into<f64>) -> Self {
        self.0.push((name.to_owned(), Arg::Number(value.into())));
        self
    }

    pub fn message(mut self, name: &str, id: &str) -> Self {
        self.0.push((name.to_owned(), Arg::Message(id.to_owned())));
        self
    }

    /// From a JSON object, as tools take them: a number is a number, `"@id"` a message, any
    /// other value text (`{"count": 2, "place": "@place-yard", "name": "Ada"}`).
    pub fn from_json(value: &Value) -> Result<Self, String> {
        let Some(map) = value.as_object() else {
            return Err(format!("`args` is an object of parameters (`{{\"count\": 2}}`), not {value}"));
        };
        let mut args: Vec<(String, Arg)> = map
            .iter()
            .map(|(name, v)| {
                let arg = match v {
                    Value::Number(n) => Arg::Number(n.as_f64().unwrap_or_default()),
                    Value::String(s) => match s.strip_prefix('@') {
                        // `@@` is a text that starts with `@`.
                        Some(rest) if rest.starts_with('@') => Arg::Text(rest.to_owned()),
                        Some(id) => Arg::Message(id.to_owned()),
                        None => Arg::Text(s.clone()),
                    },
                    other => Arg::Text(other.to_string()),
                };
                (name.clone(), arg)
            })
            .collect();
        // By name whatever order the map keeps.
        args.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(Self(args))
    }
}

/// One language: what content says of it and its text.
pub struct Language {
    pub id: String,
    pub name: String,
    pub fallback: Option<String>,
    /// Message ids and the file of each.
    pub ids: BTreeMap<String, String>,
    pub(crate) bundle: FluentBundle<FluentResource>,
}

impl Language {
    pub(crate) fn new(id: &str, name: &str, fallback: Option<String>, resources: Vec<(String, FluentResource)>, ids: BTreeMap<String, String>) -> Self {
        let tag: LanguageIdentifier = id.parse().unwrap_or_default();
        let mut bundle = FluentBundle::new_concurrent(vec![tag]);
        // Unicode isolation marks around parameters: the UI's fonts draw them as boxes.
        bundle.set_use_isolating(false);
        for (_, resource) in resources {
            // A message in two files was reported when parsing; the first one stays.
            let _ = bundle.add_resource(resource);
        }
        Self { id: id.to_owned(), name: name.to_owned(), fallback, ids, bundle }
    }
}

/// What text could not do, journaled once each by the module (`text` only reads the world).
#[derive(Default)]
pub(crate) struct Notes {
    pub(crate) pending: Vec<String>,
    seen: BTreeSet<String>,
}

/// The game's languages and the text of the one played. `text` formats a message; a missing
/// one comes from the fallbacks, and failing those is its id.
#[derive(Resource, Default)]
pub struct Locale {
    pub(crate) languages: BTreeMap<String, Language>,
    /// The language played, then the ones its missing messages come from.
    pub(crate) chain: Vec<String>,
    /// Goes up whenever text may read differently: another language, an edited file.
    pub(crate) version: u64,
    pub(crate) built: bool,
    pub(crate) notes: Mutex<Notes>,
}

impl Locale {
    /// The language played (`ru`); `None` when the game has no languages.
    pub fn language(&self) -> Option<&str> {
        self.chain.first().map(String::as_str)
    }

    /// The language played and its fallbacks, in order.
    pub fn chain(&self) -> &[String] {
        &self.chain
    }

    /// Every language, by id.
    pub fn languages(&self) -> impl Iterator<Item = &Language> {
        self.languages.values()
    }

    pub fn get(&self, id: &str) -> Option<&Language> {
        self.languages.get(id)
    }

    /// Goes up whenever text may read differently (what was drawn with it is stale).
    pub fn version(&self) -> u64 {
        self.version
    }

    /// Message `id` (`menu-resume`, or an attribute: `menu-resume.tooltip`) in the language
    /// played, with `args`. Missing everywhere: the id itself (and a journal line).
    pub fn text(&self, id: &str, args: &Args) -> String {
        let chain = self.chain.clone();
        let mut notes = Vec::new();
        let text = self.format_in(&chain, id, args, 0, &mut notes);
        for note in notes {
            self.note(note);
        }
        text.unwrap_or_else(|| {
            self.note(format!("{} has no message `{id}`{}", self.language().unwrap_or("the game"), self.or_fallbacks(&chain)));
            id.to_owned()
        })
    }

    /// Message `id` in language `lang` and its fallbacks, and what it could not do (a fallback
    /// used, a parameter left out); `Err` says what is missing. Nothing goes to the journal.
    pub fn text_in(&self, lang: &str, id: &str, args: &Args) -> Result<(String, Vec<String>), String> {
        if !self.languages.contains_key(lang) {
            let all: Vec<&str> = self.languages.keys().map(String::as_str).collect();
            return Err(format!("no language `{lang}`; languages: {}", if all.is_empty() { "none".into() } else { all.join(", ") }));
        }
        let chain = self.chain_from(lang);
        let mut notes = Vec::new();
        let text = self.format_in(&chain, id, args, 0, &mut notes);
        text.map(|t| (t, notes)).ok_or_else(|| {
            let mut ids: Vec<&str> = self.languages.values().flat_map(|l| l.ids.keys().map(String::as_str)).collect();
            ids.sort_unstable();
            ids.dedup();
            let hint = shards_content::db::closest(id, ids.into_iter()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
            format!("{lang} has no message `{id}`{}{hint}", self.or_fallbacks(&chain))
        })
    }

    /// Whether message `id` is in the language played or its fallbacks.
    pub fn has(&self, id: &str) -> bool {
        self.chain.iter().any(|l| self.languages.get(l).is_some_and(|lang| lang.ids.contains_key(id)))
    }

    /// `lang`, then its fallbacks while they are languages and do not come round.
    pub(crate) fn chain_from(&self, lang: &str) -> Vec<String> {
        let mut chain = Vec::new();
        let mut at = Some(lang.to_owned());
        while let Some(id) = at.take() {
            if chain.contains(&id) || !self.languages.contains_key(&id) {
                break;
            }
            at = self.languages[&id].fallback.clone();
            chain.push(id);
        }
        chain
    }

    fn or_fallbacks(&self, chain: &[String]) -> String {
        match chain {
            [] | [_] => String::new(),
            [_, rest @ ..] => format!(" (nor has {})", rest.join(", ")),
        }
    }

    fn format_in(&self, chain: &[String], id: &str, args: &Args, depth: u8, notes: &mut Vec<String>) -> Option<String> {
        let (message_id, attribute) = id.split_once('.').map_or((id, None), |(m, a)| (m, Some(a)));
        for (n, lang) in chain.iter().enumerate() {
            let Some(language) = self.languages.get(lang) else { continue };
            let Some(message) = language.bundle.get_message(message_id) else { continue };
            let pattern = match attribute {
                Some(a) => message.get_attribute(a).map(|a| a.value()),
                None => message.value(),
            };
            let Some(pattern) = pattern else { continue };
            if n > 0 {
                notes.push(format!("{} has no message `{id}`; it is shown in {lang}", chain[0]));
            }
            let mut fluent = FluentArgs::new();
            for (name, arg) in &args.0 {
                let value = match arg {
                    Arg::Text(t) => FluentValue::from(t.clone()),
                    Arg::Number(x) => FluentValue::from(*x),
                    Arg::Message(m) if depth < MAX_DEPTH => {
                        FluentValue::from(self.format_in(chain, m, &Args::new(), depth + 1, notes).unwrap_or_else(|| {
                            notes.push(format!("{} has no message `{m}`{} (put into `{id}`)", chain[0], self.or_fallbacks(chain)));
                            m.clone()
                        }))
                    }
                    Arg::Message(m) => FluentValue::from(m.clone()),
                };
                fluent.set(name.clone(), value);
            }
            let mut errors = Vec::new();
            let text = language.bundle.format_pattern(pattern, Some(&fluent), &mut errors).into_owned();
            for e in errors {
                let what = match e {
                    FluentError::ResolverError(e) => e.to_string(),
                    other => other.to_string(),
                };
                notes.push(format!("{lang} `{id}`: {what}"));
            }
            return Some(text);
        }
        None
    }

    /// Message `id` of language `lang` alone, as it reads with no parameters (`{$count}` where
    /// one goes), noting nothing: for listing a language's text.
    pub fn plain(&self, lang: &str, id: &str) -> Option<String> {
        let language = self.languages.get(lang)?;
        let (message_id, attribute) = id.split_once('.').map_or((id, None), |(m, a)| (m, Some(a)));
        let message = language.bundle.get_message(message_id)?;
        let pattern = match attribute {
            Some(a) => message.get_attribute(a)?.value(),
            None => message.value()?,
        };
        Some(language.bundle.format_pattern(pattern, None, &mut Vec::new()).into_owned())
    }

    fn note(&self, text: String) {
        if let Ok(mut notes) = self.notes.lock()
            && notes.seen.insert(text.clone())
        {
            notes.pending.push(text);
        }
    }

    /// What text could not do since the last call, each once.
    pub(crate) fn take_notes(&self) -> Vec<String> {
        self.notes.lock().map(|mut n| std::mem::take(&mut n.pending)).unwrap_or_default()
    }

    /// Text reads differently now: what it could not do is told again when it happens again.
    pub(crate) fn forget_notes(&mut self) {
        if let Ok(notes) = self.notes.get_mut() {
            notes.seen.clear();
        }
    }
}

/// Message `id` in the language played, with `args`; its id when the game has no such message
/// (or no languages).
pub fn text(world: &World, id: &str, args: &Args) -> String {
    world.get_resource::<Locale>().map_or_else(|| id.to_owned(), |l| l.text(id, args))
}

/// Text as content writes it: `@menu-resume` is that message in the language played, `@@` a
/// text starting with `@`, anything else itself.
pub fn resolve(world: &World, text: &str) -> String {
    match text.strip_prefix('@') {
        Some(rest) if rest.starts_with('@') => rest.to_owned(),
        Some(id) => self::text(world, id, &Args::new()),
        None => text.to_owned(),
    }
}

/// The message a content text names (`@menu-resume` → `menu-resume`), if it names one.
pub fn message_ref(text: &str) -> Option<&str> {
    text.strip_prefix('@').filter(|id| !id.is_empty() && !id.starts_with('@'))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::{parse_language, FtlFile};

    fn language(id: &str, fallback: Option<&str>, text: &str) -> Language {
        let parsed = parse_language(&[FtlFile { path: format!("locale/{id}/a.ftl"), text: text.into() }]);
        assert!(parsed.problems.is_empty(), "{:?}", parsed.problems);
        Language::new(id, id, fallback.map(str::to_owned), parsed.resources, parsed.ids)
    }

    fn locale(playing: &str) -> Locale {
        let en = language(
            "en",
            None,
            "trophies = { $count ->\n    [one] { $count } trophy\n   *[other] { $count } trophies\n}\nplace-yard = the yard\nleave = Leave { $place }?\nonly-en = English only\n",
        );
        let ru = language(
            "ru",
            Some("en"),
            "trophies = { $count ->\n    [one] { $count } трофей\n    [few] { $count } трофея\n   *[many] { $count } трофеев\n}\nplace-yard = двор\nleave = Покинуть { $place }?\n",
        );
        let mut locale = Locale { languages: [("en".into(), en), ("ru".into(), ru)].into(), ..Locale::default() };
        locale.chain = locale.chain_from(playing);
        locale
    }

    fn pending(locale: &Locale) -> Vec<String> {
        std::mem::take(&mut locale.notes.lock().unwrap().pending)
    }

    #[test]
    fn plurals_follow_each_language() {
        let en = locale("en");
        assert_eq!(en.text("trophies", &Args::new().number("count", 1)), "1 trophy");
        assert_eq!(en.text("trophies", &Args::new().number("count", 5)), "5 trophies");
        let ru = locale("ru");
        let count = |n: u32| ru.text("trophies", &Args::new().number("count", n));
        assert_eq!([count(1), count(3), count(5), count(21)], ["1 трофей", "3 трофея", "5 трофеев", "21 трофей"]);
    }

    #[test]
    fn a_phrase_goes_into_a_phrase_whole_and_in_the_same_language() {
        let args = Args::new().message("place", "place-yard");
        assert_eq!(locale("en").text("leave", &args), "Leave the yard?");
        assert_eq!(locale("ru").text("leave", &args), "Покинуть двор?");
    }

    #[test]
    fn a_missing_message_comes_from_the_fallback_and_failing_that_is_its_id() {
        let ru = locale("ru");
        assert_eq!(ru.text("only-en", &Args::new()), "English only");
        assert_eq!(ru.text("nowhere", &Args::new()), "nowhere");
        ru.text("nowhere", &Args::new());
        assert_eq!(pending(&ru), ["ru has no message `only-en`; it is shown in en", "ru has no message `nowhere` (nor has en)"], "each once");
        let (text, notes) = ru.text_in("ru", "only-en", &Args::new()).unwrap();
        assert_eq!((text.as_str(), notes.as_slice()), ("English only", ["ru has no message `only-en`; it is shown in en".to_owned()].as_slice()));
        assert!(pending(&ru).is_empty(), "text_in journals nothing");
        assert_eq!(ru.text_in("ru", "trophie", &Args::new()).unwrap_err(), "ru has no message `trophie` (nor has en) (did you mean `trophies`?)");
        assert_eq!(ru.text_in("de", "trophies", &Args::new()).unwrap_err(), "no language `de`; languages: en, ru");
    }

    #[test]
    fn a_parameter_left_out_is_noted() {
        let en = locale("en");
        assert_eq!(en.text("leave", &Args::new()), "Leave {$place}?");
        assert_eq!(pending(&en), ["en `leave`: Unknown variable: $place"]);
    }

    #[test]
    fn json_parameters_read_as_tools_send_them() {
        let args = Args::from_json(&serde_json::json!({"count": 2, "place": "@place-yard", "name": "Ada", "mail": "@@ada"})).unwrap();
        assert_eq!(
            args.0,
            [
                ("count".to_owned(), Arg::Number(2.0)),
                ("mail".to_owned(), Arg::Text("@ada".into())),
                ("name".to_owned(), Arg::Text("Ada".into())),
                ("place".to_owned(), Arg::Message("place-yard".into())),
            ]
        );
        assert!(Args::from_json(&serde_json::json!([1])).is_err());
        assert_eq!(message_ref("@menu-resume"), Some("menu-resume"));
        assert_eq!(message_ref("@@literal"), None);
        assert_eq!(message_ref("Resume"), None);
    }
}
