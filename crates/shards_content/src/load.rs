//! Loading content through the asset server, and applying edits as files change.
//!
//! At start the `content` folder is loaded asynchronously; the loading gate holds the
//! simulation until every file is in and the database is built, so loading never
//! shifts a tick. Afterwards (with the `hot_reload` feature) a changed file is parsed
//! and the whole database checked again: a clean result is applied through the
//! `content_apply` tool, which journals it and records it in the replay at the tick it
//! happened, so playback reproduces it; a broken one is journaled and the previous
//! version kept.

use std::path::PathBuf;

use ai_core::journal::{self, JournalKind};
use ai_core::{call_tool, sim, AiReplay};
use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext, LoadState};
use bevy::prelude::*;
use serde_json::json;
use shards_core::{EngineConfig, LoadStatus};

use crate::db::{is_claimed, list_files, read_file, ContentDb, ContentKinds, DefChange, Source, CONTENT_DIR};

/// One content file's text. Parsing happens when the database is built, so a broken
/// file still loads (and is reported with every other problem).
#[derive(Asset, TypePath, Debug)]
pub struct ContentText {
    pub text: String,
}

/// Loads `*.<kind>.ron` for every registered kind as [`ContentText`].
#[derive(TypePath)]
pub(crate) struct ContentLoader {
    extensions: Vec<&'static str>,
}

impl ContentLoader {
    pub(crate) fn new(kinds: &ContentKinds) -> Self {
        // A loader hands out `&[&str]`; kinds are registered once per app.
        let extensions = kinds.extensions().into_iter().map(|e| &*Box::leak(e.into_boxed_str())).collect();
        Self { extensions }
    }
}

impl AssetLoader for ContentLoader {
    type Asset = ContentText;
    type Settings = ();
    type Error = std::io::Error;

    async fn load(&self, reader: &mut dyn Reader, _: &(), _: &mut LoadContext<'_>) -> Result<ContentText, Self::Error> {
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await?;
        Ok(ContentText { text: String::from_utf8_lossy(&bytes).into_owned() })
    }

    fn extensions(&self) -> &[&str] {
        &self.extensions
    }
}

/// The files being loaded, and file changes waiting to be applied.
#[derive(Resource, Default)]
pub(crate) struct ContentLoading {
    started: bool,
    /// Every content file a registered kind claims, by asset path. The handles stay
    /// alive, so the file watcher reloads them when they change.
    files: Vec<(String, Handle<ContentText>)>,
    /// `.ron` files no kind claims, read from disk to be reported.
    unclaimed: Vec<Source>,
    /// Sources changed on disk since the last frame (hot reload).
    changed: Vec<Source>,
}

/// The asset folder on disk: where Bevy reads it, so synchronous reads see the same
/// files.
pub fn asset_root(world: &World) -> PathBuf {
    let dir = world.get_resource::<EngineConfig>().map_or_else(|| "assets".to_owned(), |c| c.asset_dir.clone());
    bevy::asset::io::file::FileAssetReader::get_base_path().join(dir)
}

/// `Startup`: lists the content folder and starts loading every file through the
/// asset server. (Not `load_folder`: in Bevy 0.20 one file without a loader fails the
/// whole folder, and a misnamed file should be reported, not break loading.)
pub(crate) fn start_loading(world: &mut World) {
    let root = asset_root(world);
    if !root.join(CONTENT_DIR).is_dir() {
        let hint = "games run from cargo find `assets` in the crate folder; elsewhere it is next to the executable, \
                    or where BEVY_ASSET_ROOT points";
        let text = format!("content: no content folder at {}: no content ({hint})", root.join(CONTENT_DIR).display());
        journal::emit(world, JournalKind::Warn, text);
    }
    let kinds = world.resource::<ContentKinds>().clone();
    let (paths, listing_error) = match list_files(&root) {
        Ok(paths) => (paths, None),
        Err(e) => (Vec::new(), Some(e)),
    };
    let server = world.resource::<AssetServer>().clone();
    let mut loading = world.resource_mut::<ContentLoading>();
    loading.started = true;
    for path in paths {
        if is_claimed(&kinds, &path) {
            let handle = server.load::<ContentText>(path.clone());
            loading.files.push((path, handle));
        } else if let Ok(source) = read_file(&root, &path) {
            loading.unclaimed.push(source);
        }
    }
    if let Some(error) = listing_error {
        journal::emit(world, JournalKind::Error, format!("content: {error}"));
    }
}

/// The loading gate's view: ready once the database is built.
pub(crate) fn content_status(world: &World) -> LoadStatus {
    if world.resource::<ContentDb>().is_loaded() {
        return LoadStatus::Ready;
    }
    let loading = world.resource::<ContentLoading>();
    if !loading.started {
        return LoadStatus::Pending("starting".into());
    }
    let server = world.resource::<AssetServer>();
    let left = loading.files.iter().filter(|(_, h)| !matches!(server.load_state(h), LoadState::Loaded | LoadState::Failed(_))).count();
    LoadStatus::Pending(match left {
        0 => "building the database".into(),
        1 => "1 file left".into(),
        n => format!("{n} files left"),
    })
}

/// `Update`: once every file is in, builds and validates the database. Problems go to
/// the journal; definitions that parsed are used anyway.
pub(crate) fn finish_loading(world: &mut World) {
    let loading = world.resource::<ContentLoading>();
    if world.resource::<ContentDb>().is_loaded() || !loading.started {
        return;
    }
    let server = world.resource::<AssetServer>();
    let texts = world.resource::<Assets<ContentText>>();
    let mut sources = loading.unclaimed.clone();
    let mut failed = Vec::new();
    for (path, handle) in &loading.files {
        match server.load_state(handle) {
            LoadState::Loaded => {
                let Some(text) = texts.get(handle) else { return };
                sources.push(Source { path: path.clone(), text: text.text.clone() });
            }
            LoadState::Failed(e) => failed.push(format!("{path}: cannot load: {e}")),
            _ => return,
        }
    }
    let kinds = world.resource::<ContentKinds>().clone();
    let db = ContentDb::build(&kinds, &sources);
    let summary = db.summary();
    let errors: Vec<String> = failed.into_iter().chain(db.errors().map(ToString::to_string)).collect();
    world.insert_resource(db);
    for error in &errors {
        journal::emit(world, JournalKind::Error, format!("content: {error}"));
    }
    let defs = if summary.definitions == 1 { "definition" } else { "definitions" };
    let files = if summary.files == 1 { "file" } else { "files" };
    let mut text = format!("content loaded: {} {defs} in {} {files}", summary.definitions, summary.files);
    if !errors.is_empty() {
        text.push_str(&format!(", {} with errors (content_check lists them)", errors.len()));
    }
    journal::emit(world, JournalKind::Event, text);
}

/// `Update`: content files the asset server reloaded (the file watcher saw them
/// change) whose text differs from what is applied.
pub(crate) fn collect_changes(
    mut events: MessageReader<AssetEvent<ContentText>>,
    server: Res<AssetServer>,
    texts: Res<Assets<ContentText>>,
    db: Res<ContentDb>,
    mut loading: ResMut<ContentLoading>,
) {
    for event in events.read() {
        let (AssetEvent::Added { id } | AssetEvent::Modified { id }) = event else { continue };
        if !db.is_loaded() {
            continue;
        }
        let (Some(path), Some(text)) = (server.get_path(*id), texts.get(*id)) else { continue };
        let path = path.path().to_string_lossy().replace('\\', "/");
        if db.file_text(&path) == Some(text.text.as_str()) || loading.changed.iter().any(|s| s.path == path && s.text == text.text) {
            continue;
        }
        loading.changed.retain(|s| s.path != path);
        loading.changed.push(Source { path, text: text.text.clone() });
    }
}

/// `Update`, after [`collect_changes`]: checks and applies them.
pub(crate) fn apply_changes(world: &mut World) {
    let changed = std::mem::take(&mut world.resource_mut::<ContentLoading>().changed);
    if !changed.is_empty() {
        let _ = reload(world, changed, Vec::new());
    }
}

/// `combat/attacks.attack.ron` for `content/combat/attacks.attack.ron`.
pub(crate) fn shown(path: &str) -> &str {
    path.strip_prefix(CONTENT_DIR).and_then(|p| p.strip_prefix('/')).unwrap_or(path)
}

/// Checks the database with `changed` files replaced and `removed` ones gone; applies
/// it through `content_apply` when no new problem appears. The one path for hot reload
/// and `content_reload`. Returns what happened, for the journal or the caller.
pub(crate) fn reload(world: &mut World, changed: Vec<Source>, removed: Vec<String>) -> Result<String, String> {
    let kinds = world.resource::<ContentKinds>().clone();
    let current = world.resource::<ContentDb>();
    let mut texts = current.sources();
    for path in &removed {
        texts.remove(path);
    }
    let mut files: Vec<String> = removed;
    for source in changed {
        files.push(source.path.clone());
        texts.insert(source.path, source.text);
    }
    files.sort();
    files.dedup();
    let sources: Vec<Source> = texts.into_iter().map(|(path, text)| Source { path, text }).collect();
    let candidate = ContentDb::build(&kinds, &sources);
    let problems: Vec<String> = candidate.new_errors(current).into_iter().map(ToString::to_string).collect();
    let names: Vec<&str> = files.iter().map(|f| shown(f)).collect();
    if !problems.is_empty() {
        for problem in &problems {
            journal::emit(world, JournalKind::Error, format!("content: {problem}; kept the previous version"));
        }
        return Err(format!("{}; kept the previous version of {}", problems.join("; "), names.join(", ")));
    }
    let changes = current.changes_to(&candidate);
    if changes.is_empty() {
        world.resource_mut::<ContentDb>().adopt_files(&candidate);
        return Ok(format!("content: no definitions changed in {}", names.join(", ")));
    }
    let outcome = call_tool(world, "content", "content_apply", json!({"files": files, "changes": changes}));
    let text = outcome.result.as_str().unwrap_or_default().to_owned();
    if !outcome.ok {
        journal::emit(world, JournalKind::Error, format!("content: {text}"));
        return Err(text);
    }
    world.resource_mut::<ContentDb>().adopt_files(&candidate);
    Ok(text)
}

/// `FixedFirst` of the first tick of a playback: definitions the recording changed
/// with `content_apply` go back to the versions it started from, so the replay plays
/// the session it recorded even after the files changed on disk.
pub(crate) fn prepare_playback(world: &mut World) {
    if sim::tick(world) != 0 {
        return;
    }
    let Some(replay) = world.get_resource::<AiReplay>().and_then(AiReplay::playing) else { return };
    let mut first: Vec<DefChange> = Vec::new();
    for event in &replay.events {
        let ai_core::replay::ReplayEvent::Call { tool, args, .. } = event else { continue };
        if tool != "content_apply" {
            continue;
        }
        let changes: Vec<DefChange> = args.get("changes").and_then(|c| serde_json::from_value(c.clone()).ok()).unwrap_or_default();
        for change in changes {
            if !first.iter().any(|c| c.kind == change.kind && c.id == change.id) {
                first.push(DefChange { def: change.before.clone(), file: change.file.clone(), ..change });
            }
        }
    }
    let kinds = world.resource::<ContentKinds>().clone();
    let current = world.resource::<ContentDb>();
    let restore: Vec<DefChange> =
        first.into_iter().filter(|c| current.get_json(&c.kind, &c.id) != c.def.as_ref()).collect();
    if restore.is_empty() {
        return;
    }
    match current.with_changes(&kinds, &[], &restore) {
        Ok(db) => {
            world.insert_resource(db);
            let n = restore.len();
            let defs = if n == 1 { "definition" } else { "definitions" };
            journal::emit(world, JournalKind::Event, format!("content: the replay starts from its recorded version of {n} {defs}"));
        }
        Err(e) => {
            journal::emit(world, JournalKind::Error, format!("content: cannot restore the replay's starting content: {e}"));
        }
    }
}
