//! # shards_locale — localization (E9.4)
//!
//! A game's text is in Fluent files, one folder per language: `locale/<language>/*.ftl` under
//! the asset folder. Content says which languages there are (`content/**/*.locale.ron`, kind
//! `locale`: a name in the language itself and the language its missing messages come from).
//! The player picks one with the `locale.language` setting, and the game switches to it while it
//! runs: everything drawn with text is drawn again.
//!
//! - **Messages, never pieces** ([`Locale::text`], [`text`]): a message is a whole phrase with
//!   parameters (`leave = Leave { $place }?`). A number picks the language's plural form (`1
//!   трофей`, `3 трофея`, `5 трофеев`); a parameter can be another message of the same language
//!   ([`Arg::Message`]), so procedural text is built from whole localized phrases.
//! - **Content names messages** as `@menu-resume` ([`resolve`]): screens and widgets show that
//!   message in the language played; `@@` starts a text with `@`.
//! - **Fallbacks**: a message missing in the language played comes from its `fallback`, and
//!   failing that shows as its id. Each case is journaled once (`locale: ru has no message
//!   `hud-hint`; it is shown in en`), as is a parameter left out.
//! - **Checks**: `--check-content` parses every language's files (`locale/ru/menus.ftl:3:5:
//!   …`), finds a message in two files of one language and fallbacks that go round; screens
//!   check that every `@id` they name is in each language or its fallbacks.
//! - **Hot reload**: with the file watcher on, an edited `.ftl` file applies within a second
//!   (`locale: text reloaded (locale/ru/menus.ftl)`).
//! - **Agents**: `locale` lists the languages, the one played, and what each lacks; `text`
//!   formats a message with parameters in any language. `ai/status` has `locale: {language,
//!   chain, languages}`.
//! - **Not the simulation's**: the language is this machine's setting, outside snapshots, replay
//!   checks and the network; two players of one co-op game read it in two languages.

pub mod def;
pub mod settings;
pub mod text;
mod tools;

use std::collections::BTreeMap;
use std::time::Duration;

use ai_core::journal::{self, JournalKind};
use ai_core::AiAppExt;
use bevy::prelude::*;
use serde_json::{json, Value};
use shards_content::{ContentAppExt, ContentDb};
use shards_core::{EngineAppExt, LoadStatus};
use shards_settings::{SettingsAppExt, SettingsChanged};

pub use def::{check_messages, LocaleDef, Messages, LOCALE_DIR};
pub use settings::LocaleSettings;
pub use text::{message_ref, resolve, text, Arg, Args, Language, Locale};
pub use tools::{LocaleArgs, TextArgs};

/// Version of the engine crates (they share one).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// How often the text files are looked at for edits, with the file watcher on (real time).
const RESCAN: Duration = Duration::from_secs(1);

/// The types a game module uses most.
pub mod prelude {
    pub use crate::{Args, Locale, LocaleSettings, TextChanged};
}

/// The locale systems other modules order against.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LocaleSystems {
    /// `Update`: languages built from content and their files, the language played followed
    /// from the setting, edited files read again. What draws text runs after it.
    Build,
}

/// Text reads differently now: another language is played, or its files were edited. What shows
/// text says it again (the UI does, before the call that switched returns).
#[derive(Event, Clone, Debug, PartialEq)]
pub struct TextChanged {
    /// The language played now.
    pub language: Option<String>,
}

/// What the languages were built from: content's definitions and the files as read.
#[derive(Resource, Default)]
struct LocaleSource {
    defs: Vec<(String, def::LocaleDef)>,
    files: BTreeMap<String, Vec<def::FtlFile>>,
    /// The setting as last followed.
    wanted: Option<String>,
    next_scan: Duration,
    /// Edits the last reload refused (see [`reload_text`]).
    refused: BTreeMap<String, String>,
}

/// Languages as content and their Fluent text, the `locale.language` setting, the tools and the
/// `locale` field of `ai/status`. Needs `AiCorePlugin` and content first; `shards::EnginePlugins`
/// adds it before the UI.
pub struct LocalePlugin;

impl Plugin for LocalePlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<ai_core::AiCorePlugin>(),
            "LocalePlugin needs AiCorePlugin added first (shards::EnginePlugins adds both)"
        );
        app.add_content::<LocaleDef>()
            .add_settings_group::<LocaleSettings>()
            .add_settings_world_check::<LocaleSettings>(settings::known_language)
            .init_resource::<Locale>()
            .init_resource::<LocaleSource>()
            .add_engine_module("locale", VERSION)
            .add_ai_status("locale", status)
            // The first screen is drawn in the language played, not in message ids.
            .require_loaded("locale", loaded)
            .add_systems(Update, (build_languages, reread_files, follow_setting).chain().in_set(LocaleSystems::Build))
            // After everything that formats text this frame.
            .add_systems(Last, journal_notes)
            // `settings_set group=locale` switches before it returns.
            .add_observer(on_settings_changed);
        tools::register(app.world_mut());
    }
}

fn loaded(world: &World) -> LoadStatus {
    match world.get_resource::<Locale>() {
        Some(locale) if locale.built => LoadStatus::Ready,
        _ => LoadStatus::Pending("languages".into()),
    }
}

/// `ai/status` → `locale: {language, chain, languages}`.
fn status(world: &World) -> Value {
    let Some(locale) = world.get_resource::<Locale>() else { return Value::Null };
    let languages: Vec<&str> = locale.languages().map(|l| l.id.as_str()).collect();
    json!({"language": locale.language(), "chain": locale.chain(), "languages": languages})
}

/// `Update`: (re)builds every language when content's languages change, reading their files.
fn build_languages(world: &mut World) {
    let built = world.resource::<Locale>().built;
    if built && !world.is_resource_changed::<ContentDb>() {
        return;
    }
    let db = world.resource::<ContentDb>();
    if !db.is_loaded() {
        return;
    }
    let defs: Vec<(String, LocaleDef)> = db.iter::<LocaleDef>().map(|(id, d)| (id.to_owned(), d.clone())).collect();
    if built && world.resource::<LocaleSource>().defs == defs {
        return;
    }
    let root = shards_content::load::asset_root(world);
    let mut files = BTreeMap::new();
    for (id, _) in &defs {
        // Content's check reported a folder that cannot be read; the language has no text.
        files.insert(id.clone(), def::read_language(&root, id).unwrap_or_default());
    }
    // Content's check reported the files' mistakes already.
    install(world, defs, files);
}

/// `Update`, with the file watcher on: every [`RESCAN`], [`reload_text`].
fn reread_files(world: &mut World) {
    if !world.resource::<Locale>().built || !world.get_resource::<AssetServer>().is_some_and(AssetServer::watching_for_changes) {
        return;
    }
    let now = world.resource::<Time<Real>>().elapsed();
    if now < world.resource::<LocaleSource>().next_scan {
        return;
    }
    world.resource_mut::<LocaleSource>().next_scan = now + RESCAN;
    reload_text(world);
}

/// Reads every language's files again and, where they changed, builds the languages anew: what
/// the file watcher does every second. The files that changed (none: nothing was done); the journal
/// says `locale: text reloaded (…)` and the mistakes in them. An edit that does not parse is
/// refused, as content's are: the file keeps its previous version (a new file waits) until it
/// does, and a language whose files cannot be read now (an editor mid-save) keeps the text it
/// had. Each refusal is journaled once.
pub fn reload_text(world: &mut World) -> Vec<String> {
    if !world.get_resource::<Locale>().is_some_and(|l| l.built) {
        return Vec::new();
    }
    let root = shards_content::load::asset_root(world);
    let source = world.resource::<LocaleSource>();
    let mut files = BTreeMap::new();
    let mut changed = Vec::new();
    let mut refusals = Refusals { before: &source.refused, now: BTreeMap::new(), problems: Vec::new() };
    for (id, before) in &source.files {
        let now = match def::read_language(&root, id) {
            Ok(now) => keep_where_broken(before, now, &mut refusals),
            Err(e) => {
                refusals.refuse(format!("{}/{id}/", def::LOCALE_DIR), e.clone(), vec![format!("{e}; {id} keeps the text it had")]);
                before.clone()
            }
        };
        changed.extend(edited(before, &now));
        files.insert(id.clone(), now);
    }
    let Refusals { now: refused, mut problems, .. } = refusals;
    let defs = source.defs.clone();
    world.resource_mut::<LocaleSource>().refused = refused;
    if !changed.is_empty() {
        problems.extend(files.values().flat_map(|f| def::parse_language(f).problems));
        install(world, defs, files);
        journal::emit(world, JournalKind::Event, format!("locale: text reloaded ({})", changed.join(", ")));
    }
    for problem in problems {
        journal::emit(world, JournalKind::Error, format!("locale: {problem}"));
    }
    changed
}

/// What a reload refused: by file (or `locale/<id>/` for a folder that could not be read), the
/// text or error refused, so the same one is journaled once.
struct Refusals<'a> {
    before: &'a BTreeMap<String, String>,
    now: BTreeMap<String, String>,
    problems: Vec<String>,
}

impl Refusals<'_> {
    fn refuse(&mut self, key: String, what: String, problems: Vec<String>) {
        if self.before.get(&key) != Some(&what) {
            self.problems.extend(problems);
        }
        self.now.insert(key, what);
    }
}

/// `now`, where each file that changed since `before` and does not parse is its version in
/// `before` again (or left out, when it is new).
fn keep_where_broken(before: &[def::FtlFile], now: Vec<def::FtlFile>, refusals: &mut Refusals) -> Vec<def::FtlFile> {
    let mut kept = Vec::new();
    for file in now {
        let was = before.iter().find(|f| f.path == file.path);
        let mistakes = if was.is_some_and(|w| w.text == file.text) { Vec::new() } else { def::syntax_problems(&file) };
        if mistakes.is_empty() {
            kept.push(file);
            continue;
        }
        let outcome = if was.is_some() { "kept the previous version" } else { "left out until it parses" };
        refusals.refuse(file.path.clone(), file.text, mistakes.into_iter().map(|m| format!("{m}; {outcome}")).collect());
        kept.extend(was.cloned());
    }
    kept
}

/// Files of one language added, changed or removed between two reads.
fn edited(before: &[def::FtlFile], now: &[def::FtlFile]) -> Vec<String> {
    let mut paths: Vec<&str> = before.iter().chain(now).map(|f| f.path.as_str()).collect();
    paths.sort_unstable();
    paths.dedup();
    let text = |files: &[def::FtlFile], path: &str| files.iter().find(|f| f.path == path).map(|f| f.text.clone());
    paths.into_iter().filter(|p| text(before, p) != text(now, p)).map(str::to_owned).collect()
}

/// Builds the languages from `defs` and `files` and plays the one the setting names.
fn install(world: &mut World, defs: Vec<(String, LocaleDef)>, files: BTreeMap<String, Vec<def::FtlFile>>) {
    let mut languages = BTreeMap::new();
    for (id, def) in &defs {
        let parsed = def::parse_language(files.get(id).map_or(&[][..], Vec::as_slice));
        languages.insert(id.clone(), Language::new(id, &def.name, def.fallback.clone(), parsed.resources, parsed.ids));
    }
    let wanted = world.get_resource::<LocaleSettings>().map(|s| s.language.trim().to_owned()).unwrap_or_default();
    let mut locale = world.resource_mut::<Locale>();
    locale.languages = languages;
    locale.chain = pick(&locale, &wanted);
    locale.version += 1;
    locale.built = true;
    locale.forget_notes();
    let mut source = world.resource_mut::<LocaleSource>();
    source.defs = defs;
    source.files = files;
    source.wanted = Some(wanted);
    told(world);
}

/// `Update`: the language played follows the `locale.language` setting.
fn follow_setting(world: &mut World) {
    if !world.resource::<Locale>().built {
        return;
    }
    let wanted = world.get_resource::<LocaleSettings>().map(|s| s.language.trim().to_owned()).unwrap_or_default();
    if world.resource::<LocaleSource>().wanted.as_deref() == Some(wanted.as_str()) {
        return;
    }
    world.resource_mut::<LocaleSource>().wanted = Some(wanted.clone());
    let mut locale = world.resource_mut::<Locale>();
    let chain = pick(&locale, &wanted);
    if chain == locale.chain {
        return;
    }
    let before = locale.language().unwrap_or("none").to_owned();
    locale.chain = chain;
    locale.version += 1;
    locale.forget_notes();
    let after = locale.language().unwrap_or("none").to_owned();
    if !wanted.is_empty() && after != wanted {
        let text = format!("locale: the setting names `{wanted}`, which this game has not; playing {after}");
        journal::emit(world, JournalKind::Warn, text);
    }
    journal::emit(world, JournalKind::Event, format!("locale: language {before} → {after}"));
    told(world);
}

fn on_settings_changed(changed: On<SettingsChanged>, mut commands: Commands) {
    if changed.group == "locale" {
        commands.queue(follow_setting);
    }
}

/// Tells what shows text that it reads differently now, and lets it say it again at once.
fn told(world: &mut World) {
    let language = world.resource::<Locale>().language().map(str::to_owned);
    world.trigger(TextChanged { language });
    world.flush();
    // What saying it again could not do (a message the language lacks) is in the journal too.
    journal_notes(world);
}

/// The language `wanted` and its fallbacks; empty or not a language: the game's own, the first
/// language (by id) that falls back to no other, or failing that the first.
fn pick(locale: &Locale, wanted: &str) -> Vec<String> {
    if locale.get(wanted).is_some() {
        return locale.chain_from(wanted);
    }
    let own = locale.languages().find(|l| l.fallback.is_none()).or_else(|| locale.languages().next());
    own.map(|l| locale.chain_from(&l.id)).unwrap_or_default()
}

/// What text could not do (a message the language lacks, a parameter left out), in the journal
/// once each: every frame in `Last`, and by what shows text right after it said it.
pub fn journal_notes(world: &mut World) {
    let Some(notes) = world.get_resource::<Locale>().map(Locale::take_notes) else { return };
    for note in notes {
        journal::emit(world, JournalKind::Warn, format!("locale: {note}"));
    }
}

