//! Languages as content (`*.locale.ron`) and their Fluent files (`locale/<language>/*.ftl`).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use fluent_bundle::FluentResource;
use fluent_syntax::ast;
use serde::{Deserialize, Serialize};
use shards_content::{ContentDef, Refs};
use unic_langid::LanguageIdentifier;

/// The folder of the Fluent files under the asset folder: `locale/<language>/*.ftl`.
pub const LOCALE_DIR: &str = "locale";

/// A language the game is played in (kind `locale`; the id is its tag: `en`, `ru`, `pt-BR`).
/// Its text is every `.ftl` file in `locale/<id>/`.
///
/// ```ron
/// {
///     "en": (name: "English"),
///     "ru": (name: "Русский", fallback: "en"),
/// }
/// ```
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct LocaleDef {
    /// The language's name in itself, for a language menu: `Русский`.
    pub name: String,
    /// The language a message missing here is taken from (`en`); without one, a missing
    /// message shows as its id.
    #[serde(default)]
    pub fallback: Option<String>,
}

impl ContentDef for LocaleDef {
    const KIND: &'static str = "locale";

    fn validate(&self, id: &str, refs: &mut Refs) {
        if let Err(e) = id.parse::<LanguageIdentifier>() {
            refs.error(format!("`{id}` is not a language tag (`en`, `ru`, `pt-BR`): {e}"));
        }
        if self.name.trim().is_empty() {
            refs.error("`name` is empty: the language's name in itself (`Deutsch`, `Русский`)");
        }
        if let Some(fallback) = &self.fallback {
            if fallback == id {
                refs.error("`fallback` is this language itself: name another one, or leave it out");
            } else if refs.require::<LocaleDef>(fallback, "fallback") {
                let mut chain = vec![id.to_owned(), fallback.clone()];
                while let Some(next) = refs.get::<LocaleDef>(chain.last().unwrap()).and_then(|d| d.fallback.clone()) {
                    let round = chain.contains(&next);
                    chain.push(next);
                    if round {
                        refs.error(format!("fallbacks go round and never end: {}", chain.join(" → ")));
                        break;
                    }
                }
            }
        }
        if let Some(root) = refs.asset_root() {
            match read_language(root, id) {
                Ok(files) if files.is_empty() => {
                    refs.error(format!("no text: put its Fluent files in {LOCALE_DIR}/{id}/ under the asset folder (`menus.ftl`)"))
                }
                Ok(files) => {
                    for problem in parse_language(&files).problems {
                        refs.error(problem);
                    }
                }
                Err(e) => refs.error(e),
            }
        }
    }
}

/// One Fluent file: its asset path (`locale/ru/menus.ftl`) and text.
#[derive(Clone, Debug, PartialEq)]
pub struct FtlFile {
    pub path: String,
    pub text: String,
}

/// Every `.ftl` file of language `id` under the asset folder `root`, sorted by path. A missing
/// folder is no files.
pub fn read_language(root: &Path, id: &str) -> Result<Vec<FtlFile>, String> {
    let dir = root.join(LOCALE_DIR).join(id);
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let entries = std::fs::read_dir(&dir).map_err(|e| format!("cannot list {LOCALE_DIR}/{id}/: {e}"))?;
    let mut files = Vec::new();
    for entry in entries {
        let path = entry.map_err(|e| format!("cannot list {LOCALE_DIR}/{id}/: {e}"))?.path();
        if path.extension().is_none_or(|e| e != "ftl") {
            continue;
        }
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        let asset = format!("{LOCALE_DIR}/{id}/{name}");
        let bytes = std::fs::read(&path).map_err(|e| format!("{asset}: cannot read: {e}"))?;
        // Strictly: a lossy read would turn a corrupt file into other text and other message ids.
        let text = String::from_utf8(bytes).map_err(|e| format!("{asset}: not UTF-8 ({e}); save the file as UTF-8"))?;
        files.push(FtlFile { path: asset, text });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// A language's files parsed: what parsed of each (a file with a mistake keeps the messages
/// around it), the message ids by file, and every problem as `file:line:col: what`.
pub(crate) struct Parsed {
    pub(crate) resources: Vec<(String, FluentResource)>,
    /// Message ids (`menu-resume`, and `menu-resume.tooltip` for attributes) and the file of each.
    pub(crate) ids: BTreeMap<String, String>,
    pub(crate) problems: Vec<String>,
}

/// Parses `files`; a message in two files is a problem (the second is left out).
pub(crate) fn parse_language(files: &[FtlFile]) -> Parsed {
    let mut parsed = Parsed { resources: Vec::new(), ids: BTreeMap::new(), problems: Vec::new() };
    for file in files {
        let (resource, problems) = parse_file(file);
        parsed.problems.extend(problems);
        for entry in resource.entries() {
            let ast::Entry::Message(message) = entry else { continue };
            let name = message.id.name.to_owned();
            let ids = std::iter::once(name.clone()).chain(message.attributes.iter().map(|a| format!("{name}.{}", a.id.name)));
            for id in ids {
                if let Some(first) = parsed.ids.get(&id) {
                    parsed.problems.push(format!("{}: message `{id}` is in {first} too: a message is in one file", file.path));
                } else {
                    parsed.ids.insert(id, file.path.clone());
                }
            }
        }
        parsed.resources.push((file.path.clone(), resource));
    }
    parsed
}

/// What of `file` parsed, and its syntax mistakes as `file:line:col: what`.
fn parse_file(file: &FtlFile) -> (FluentResource, Vec<String>) {
    match FluentResource::try_new(file.text.clone()) {
        Ok(resource) => (resource, Vec::new()),
        Err((resource, errors)) => {
            let problems = errors
                .into_iter()
                .map(|e| {
                    let (line, col) = line_col(&file.text, e.pos.start);
                    format!("{}:{line}:{col}: {e}", file.path)
                })
                .collect();
            (resource, problems)
        }
    }
}

/// The syntax mistakes of `file` (`locale/ru/menus.ftl:2:14: …`), none when it parses.
pub(crate) fn syntax_problems(file: &FtlFile) -> Vec<String> {
    parse_file(file).1
}

/// 1-based line and column of byte `at` in `text`.
fn line_col(text: &str, at: usize) -> (usize, usize) {
    let before = &text[..at.min(text.len())];
    let line = before.matches('\n').count() + 1;
    let col = before.rsplit('\n').next().map_or(0, |l| l.chars().count()) + 1;
    (line, col)
}

/// The language id closest to `wanted`, for a hint. Two-letter tags are a letter or two from any
/// other (`de`, `en`), so one counts only when it starts the same (`ry` → `ru`, `rus` → `ru`).
pub(crate) fn closest_language<'a>(wanted: &str, ids: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    shards_content::db::closest(wanted, ids).filter(|c| c.chars().next() == wanted.chars().next())
}

/// Every language's message ids as its files on disk have them, and the fallbacks: what a
/// definition that names messages (`@menu-resume` on a screen) checks them against.
pub struct Messages {
    ids: BTreeMap<String, BTreeSet<String>>,
    fallbacks: BTreeMap<String, Option<String>>,
}

impl Messages {
    /// The languages of content and their files; `None` when there is no asset folder to read
    /// (a check of content files alone).
    pub fn read(refs: &Refs) -> Option<Self> {
        let root = refs.asset_root()?;
        let fallbacks: BTreeMap<String, Option<String>> = refs.iter::<LocaleDef>().map(|(id, d)| (id.to_owned(), d.fallback.clone())).collect();
        let ids = fallbacks
            .keys()
            .map(|id| {
                let ids = read_language(root, id).map(|files| parse_language(&files).ids.into_keys().collect()).unwrap_or_default();
                (id.clone(), ids)
            })
            .collect();
        Some(Self { ids, fallbacks })
    }

    /// From languages given directly: `(id, fallback, message ids)`.
    pub fn new<'a>(languages: impl IntoIterator<Item = (&'a str, Option<&'a str>, Vec<&'a str>)>) -> Self {
        let mut messages = Self { ids: BTreeMap::new(), fallbacks: BTreeMap::new() };
        for (id, fallback, ids) in languages {
            messages.fallbacks.insert(id.to_owned(), fallback.map(str::to_owned));
            messages.ids.insert(id.to_owned(), ids.into_iter().map(str::to_owned).collect());
        }
        messages
    }

    /// Whether the game has no languages.
    pub fn is_empty(&self) -> bool {
        self.fallbacks.is_empty()
    }

    /// The languages in which message `id` would show as its id: neither they nor their
    /// fallbacks have it.
    pub fn lacking(&self, id: &str) -> Vec<&str> {
        self.fallbacks.keys().filter(|lang| !self.chain(lang).iter().any(|l| self.ids.get(*l).is_some_and(|ids| ids.contains(id)))).map(String::as_str).collect()
    }

    /// The message id of any language closest to `id`, for a hint.
    pub fn closest(&self, id: &str) -> Option<&str> {
        let all: BTreeSet<&str> = self.ids.values().flatten().map(String::as_str).collect();
        shards_content::db::closest(id, all.into_iter())
    }

    /// `lang` and its fallbacks, while they do not come round.
    fn chain<'a>(&'a self, lang: &'a str) -> Vec<&'a str> {
        let mut chain = vec![lang];
        while let Some(next) = self.fallbacks.get(*chain.last().unwrap()).and_then(Option::as_deref) {
            if chain.contains(&next) {
                break;
            }
            chain.push(next);
        }
        chain
    }
}

/// Checks that each `@id` in `texts` (content text as written: `@menu-resume`, `Resume`) names a
/// message every language has, itself or through its fallbacks. Nothing is checked without an
/// asset folder to read.
pub fn check_messages<'t>(refs: &mut Refs, texts: impl IntoIterator<Item = (&'t str, String)>) {
    let mut named: Vec<(&str, String)> = texts.into_iter().filter_map(|(text, field)| crate::text::message_ref(text).map(|id| (id, field))).collect();
    if named.is_empty() {
        return;
    }
    let Some(messages) = Messages::read(refs) else { return };
    named.dedup();
    for (id, field) in named {
        if messages.is_empty() {
            refs.error(format!("{field} names message `{id}`, but this game has no languages (content `*.locale.ron` and {LOCALE_DIR}/<language>/*.ftl)"));
            continue;
        }
        let lacking = messages.lacking(id);
        let who = match lacking.as_slice() {
            [one] => format!("{one} has not, nor its fallbacks"),
            [rest @ .., last] => format!("{} and {last} have not, nor their fallbacks", rest.join(", ")),
            [] => continue,
        };
        let hint = messages.closest(id).map(|c| format!(" (did you mean `@{c}`?)")).unwrap_or_default();
        refs.error(format!("{field} names message `{id}`, which {who}{hint}"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, text: &str) -> FtlFile {
        FtlFile { path: path.into(), text: text.into() }
    }

    #[test]
    fn mistakes_name_their_file_line_and_column() {
        let parsed = parse_language(&[file("locale/en/menus.ftl", "menu-resume = Resume\nmenu-quit Quit\nhud-hint = [Esc] Menu\n")]);
        assert_eq!(parsed.problems.len(), 1, "{:?}", parsed.problems);
        assert!(parsed.problems[0].starts_with("locale/en/menus.ftl:2:"), "{:?}", parsed.problems);
        let ids: Vec<&str> = parsed.ids.keys().map(String::as_str).collect();
        assert_eq!(ids, ["hud-hint", "menu-resume"], "what is around a mistake still counts");
    }

    #[test]
    fn a_message_in_two_files_is_a_problem_and_attributes_are_messages_too() {
        let parsed = parse_language(&[
            file("locale/en/a.ftl", "title = Paused\n    .tooltip = The game waits\n"),
            file("locale/en/b.ftl", "title = Pause\n"),
        ]);
        assert_eq!(parsed.problems, ["locale/en/b.ftl: message `title` is in locale/en/a.ftl too: a message is in one file"]);
        assert_eq!(parsed.ids.keys().collect::<Vec<_>>(), ["title", "title.tooltip"]);
    }

    #[test]
    fn a_message_a_fallback_has_is_not_lacking() {
        let messages = Messages::new([
            ("de", None, vec!["menu-resume"]),
            ("en", None, vec!["menu-resume", "menu-quit"]),
            ("ru", Some("en"), vec!["menu-resume"]),
        ]);
        assert!(messages.lacking("menu-resume").is_empty());
        assert_eq!(messages.lacking("menu-quit"), ["de"], "ru takes it from en");
        assert_eq!(messages.lacking("menu-pause"), ["de", "en", "ru"]);
        assert_eq!(messages.closest("menu-resum"), Some("menu-resume"));
    }

    #[test]
    fn a_language_hint_starts_like_the_tag_asked_for() {
        let ids = ["en", "ru"];
        assert_eq!(closest_language("ry", ids.into_iter()), Some("ru"));
        assert_eq!(closest_language("rus", ids.into_iter()), Some("ru"));
        assert_eq!(closest_language("de", ids.into_iter()), None, "`en` is as far from `de` as any two-letter tag");
    }

    #[test]
    fn lines_and_columns_count_from_one() {
        assert_eq!(line_col("ab\ncd", 0), (1, 1));
        assert_eq!(line_col("ab\ncd", 4), (2, 2));
        assert_eq!(line_col("ab\nцd", 5), (2, 2), "columns count characters, not bytes");
    }
}
