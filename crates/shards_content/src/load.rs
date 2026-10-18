//! Loading content through the asset server, and applying edits as files change.
//!
//! At start the `content` folder is loaded asynchronously; the loading gate holds the
//! simulation until every file is in and the database is built, so loading never
//! shifts a tick. Afterwards (with the `hot_reload` feature) a changed, created or
//! deleted file is parsed and the whole database checked again: a clean result is
//! applied through the `content_apply` tool, which journals it and records it in the
//! replay at the tick it happened, so playback reproduces it; a broken one is journaled
//! and the previous version kept.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::Duration;

use ai_core::journal::{self, JournalKind};
use ai_core::replay::Replay;
use ai_core::{call_tool, sim, AiReplay};
use bevy::asset::io::Reader;
use bevy::asset::{AssetLoader, LoadContext, LoadState};
use bevy::prelude::*;
use serde_json::json;
use shards_core::{EngineConfig, LoadStatus};

use crate::db::{is_claimed, list_files, read_file, ContentDb, ContentDef, ContentKinds, DefChange, Source, CONTENT_DIR, NO_FOLDER_HINT};

/// One content file's text. Parsing happens when the database is built, so a broken
/// file still loads (and is reported with every other problem).
#[derive(Asset, TypePath, Debug)]
pub struct ContentText {
    /// The file's contents (invalid UTF-8 replaced).
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

/// How often hot reload lists the content folder for files created or deleted: the
/// file watcher only follows files the game loaded.
const RESCAN: Duration = Duration::from_secs(1);

/// The files being loaded, and what hot reload has seen of them since.
#[derive(Resource, Default)]
pub(crate) struct ContentLoading {
    started: bool,
    /// Every content file a registered kind claims, by asset path. The handles stay
    /// alive, so the file watcher reloads them when they change.
    files: Vec<(String, Handle<ContentText>)>,
    /// `.ron` files no kind claims, read from disk to be reported.
    unclaimed: Vec<Source>,
    /// Hot reload: every content file as last seen on disk (`None`: deleted), edits
    /// that were refused included. An edit is checked together with all of them, so
    /// one that spans files applies once its last file is saved.
    seen: BTreeMap<String, Option<String>>,
    /// `seen` changed since hot reload last tried to apply it.
    dirty: bool,
    /// When hot reload lists the folder next (real time).
    next_scan: Duration,
    /// Why an edit seen on disk waits ([`crate::content_held`]), as the journal said it.
    held: Option<String>,
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
        let text = format!("content: no content folder at {}: no content ({NO_FOLDER_HINT})", root.join(CONTENT_DIR).display());
        journal::emit(world, JournalKind::Warn, text);
    }
    world.resource_mut::<ContentKinds>().set_asset_root(root.clone());
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
/// the journal; definitions that parsed are used anyway. A replay playing already
/// gets its starting content now, before `OnEnter` spawns the world from it.
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
    crate::install(world, db);
    crate::edits::refresh(world);
    world.resource_mut::<ContentLoading>().seen = sources.into_iter().map(|s| (s.path, Some(s.text))).collect();
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
    restore_for_playback(world);
}

/// `Update`, with the file watcher on: every [`RESCAN`], content files created since
/// the game started load (and apply like an edit once in), and deleted ones count as
/// removed.
pub(crate) fn rescan_folder(world: &mut World) {
    if !world.resource::<AssetServer>().watching_for_changes() || !world.resource::<ContentDb>().is_loaded() {
        return;
    }
    let now = world.resource::<Time<Real>>().elapsed();
    if now < world.resource::<ContentLoading>().next_scan {
        return;
    }
    world.resource_mut::<ContentLoading>().next_scan = now + RESCAN;
    let root = asset_root(world);
    let Ok(paths) = list_files(&root) else { return };
    let kinds = world.resource::<ContentKinds>().clone();
    let server = world.resource::<AssetServer>().clone();
    let mut loading = world.resource_mut::<ContentLoading>();
    let loading = &mut *loading;
    for (path, text) in &mut loading.seen {
        if text.is_some() && !paths.contains(path) {
            *text = None;
            loading.dirty = true;
        }
    }
    for path in paths {
        // A file the game holds a handle to comes back through the watcher.
        if loading.seen.get(&path).is_some_and(Option::is_some) || loading.files.iter().any(|(p, _)| *p == path) {
            continue;
        }
        if is_claimed(&kinds, &path) {
            let handle = server.load::<ContentText>(path.clone());
            loading.files.push((path, handle));
        } else if let Ok(source) = read_file(&root, &path) {
            // Checked like any edit: refused, with what is wrong with its name.
            loading.seen.insert(source.path, Some(source.text));
            loading.dirty = true;
        }
    }
}

/// `Update`: content files the asset server (re)loaded because the file watcher saw
/// them change.
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
        if loading.seen.get(&path).and_then(Option::as_deref) != Some(text.text.as_str()) {
            loading.seen.insert(path, Some(text.text.clone()));
            loading.dirty = true;
        }
    }
}

/// `Update`, after [`collect_changes`]: applies every file seen on disk that differs
/// from what is applied, checked together. Not while the game loads: then an edit
/// waits, since an action cannot run. Nor while a module holds content
/// ([`crate::ContentGuards`]): the journal says so once, and the edit applies when it lets go.
pub(crate) fn apply_changes(world: &mut World) {
    if !world.resource::<ContentLoading>().dirty || sim::gate_message(world).is_some() {
        return;
    }
    if let Some(why) = crate::content_held(world) {
        let mut loading = world.resource_mut::<ContentLoading>();
        if loading.held.as_deref() != Some(why.as_str()) {
            loading.held = Some(why.clone());
            journal::emit(world, JournalKind::Warn, format!("content: a file changed on disk and waits: a content reload {why}; it applies when that is over"));
        }
        return;
    }
    if world.resource_mut::<ContentLoading>().held.take().is_some() {
        journal::emit(world, JournalKind::Sim, "content: nothing holds content any more: applying what changed on disk");
    }
    world.resource_mut::<ContentLoading>().dirty = false;
    let loading = world.resource::<ContentLoading>();
    let db = world.resource::<ContentDb>();
    let mut changed = Vec::new();
    let mut removed = Vec::new();
    for (path, text) in &loading.seen {
        match text {
            Some(text) if db.file_text(path) != Some(text.as_str()) => changed.push(Source { path: path.clone(), text: text.clone() }),
            None if db.file_text(path).is_some() => removed.push(path.clone()),
            _ => {}
        }
    }
    if !changed.is_empty() || !removed.is_empty() {
        let _ = reload(world, changed, removed);
    }
}

/// `combat/attacks.attack.ron` for `content/combat/attacks.attack.ron`.
pub(crate) fn shown(path: &str) -> &str {
    path.strip_prefix(CONTENT_DIR).and_then(|p| p.strip_prefix('/')).unwrap_or(path)
}

/// Checks the database built from the applied files with `changed` files replaced and
/// `removed` ones gone; applies every definition that differs (also one `content_apply`
/// or a replay changed apart from its file) through `content_apply` when no new
/// problem appears and every definition of a changed file reads: a file applies whole, so
/// with one definition broken the previous version of all of them stays. The one path for
/// hot reload and `content_reload`. Returns what happened, for the journal or the caller.
pub(crate) fn reload(world: &mut World, changed: Vec<Source>, removed: Vec<String>) -> Result<String, String> {
    if let Some(why) = crate::content_held(world) {
        return Err(format!("refused: a content reload {why}"));
    }
    let kinds = world.resource::<ContentKinds>().clone();
    let current = world.resource::<ContentDb>();
    let mut texts = current.sources();
    // Files whose text changes, then every file a changed definition is in.
    let mut files: Vec<String> = Vec::new();
    for path in removed {
        if texts.remove(&path).is_some() {
            files.push(path);
        }
    }
    for source in changed {
        if texts.get(&source.path) != Some(&source.text) {
            files.push(source.path.clone());
        }
        texts.insert(source.path, source.text);
    }
    let sources: Vec<Source> = texts.into_iter().map(|(path, text)| Source { path, text }).collect();
    let candidate = ContentDb::build(&kinds, &sources);
    let changes = current.changes_to(&candidate);
    // The files whose text changed: any definition of them that does not read refuses the edit.
    let edited = files.clone();
    files.extend(changes.iter().filter_map(|c| c.file.clone()));
    files.sort();
    files.dedup();
    let problems: Vec<String> = candidate.refusals(current, &edited).into_iter().map(ToString::to_string).collect();
    let names: Vec<&str> = files.iter().map(|f| shown(f)).collect();
    if !problems.is_empty() {
        for problem in &problems {
            journal::emit(world, JournalKind::Error, format!("content: {problem}; kept the previous version"));
        }
        return Err(format!("{}; kept the previous version of {}", problems.join("; "), names.join(", ")));
    }
    if changes.is_empty() {
        world.resource_mut::<ContentDb>().adopt_files(&candidate);
        crate::edits::refresh(world);
        if names.is_empty() {
            return Ok("content: nothing changed on disk".into());
        }
        return Ok(format!("content: no definitions changed in {}", names.join(", ")));
    }
    let outcome = call_tool(world, "content", "content_apply", json!({"files": files, "changes": changes}));
    let text = outcome.result.as_str().unwrap_or_default().to_owned();
    if !outcome.ok {
        journal::emit(world, JournalKind::Error, format!("content: {text}"));
        return Err(text);
    }
    world.resource_mut::<ContentDb>().adopt_files(&candidate);
    crate::edits::refresh(world);
    Ok(text)
}

/// `FixedFirst` of the first tick: a playback started after loading starts from its
/// recorded content too (later than one started before: `OnEnter` has spawned the
/// world from the content on disk).
pub(crate) fn prepare_playback(world: &mut World) {
    if sim::tick(world) == 0 {
        restore_for_playback(world);
    }
}

/// The changes of every `content_apply` a recording made, in order.
fn recorded_changes(replay: &Replay) -> Vec<DefChange> {
    let mut changes = Vec::new();
    for event in &replay.events {
        let ai_core::replay::ReplayEvent::Call { tool, args, .. } = event else { continue };
        if tool == "content_apply" {
            let applied: Vec<DefChange> = args.get("changes").and_then(|c| serde_json::from_value(c.clone()).ok()).unwrap_or_default();
            changes.extend(applied);
        }
    }
    changes
}

/// Every definition of kind `T` that `replay` will apply on its way: the edits of the snapshot
/// it starts from ([`ContentEdits`](crate::ContentEdits)) and what its `content_apply` calls set. A module whose definitions name files (a tile's level file)
/// asks for those files while the game loads, so a replayed call right after the change (in
/// the same tick, where a live session had frames between) finds them loaded. Definitions
/// that do not read as `T` are left out: the replayed call says what is wrong with them.
pub fn defs_ahead<T: ContentDef>(replay: &Replay) -> Vec<T> {
    let start = replay.start.as_ref().map(crate::edits::changes_in).unwrap_or_default();
    start
        .into_iter()
        .chain(recorded_changes(replay))
        .filter(|change| change.kind == T::KIND)
        .filter_map(|change| T::deserialize(change.def?).ok())
        .collect()
}

/// Definitions the playing recording changed with `content_apply` go back to the
/// versions it started from (the `before` of the first change to each, which
/// `content_apply` checked), so the replay plays the session it recorded even after the
/// files changed on disk.
fn restore_for_playback(world: &mut World) {
    let Some(replay) = world.get_resource::<AiReplay>().and_then(AiReplay::playing) else { return };
    let mut first: Vec<DefChange> = Vec::new();
    for change in recorded_changes(replay) {
        if !first.iter().any(|c| c.kind == change.kind && c.id == change.id) {
            first.push(DefChange { def: change.before.clone(), file: change.file.clone(), ..change });
        }
    }
    let kinds = world.resource::<ContentKinds>().clone();
    let current = world.resource::<ContentDb>();
    let restore: Vec<DefChange> = first
        .into_iter()
        .filter(|c| current.json_of(&c.kind, &c.id) != c.def.as_ref().and_then(|d| kinds.normalize(&c.kind, d)).as_ref())
        .collect();
    if restore.is_empty() {
        return;
    }
    match current.with_changes(&kinds, &[], &restore) {
        Ok(db) => {
            world.insert_resource(db);
            crate::edits::refresh(world);
            let n = restore.len();
            let defs = if n == 1 { "definition" } else { "definitions" };
            journal::emit(world, JournalKind::Event, format!("content: the replay starts from its recorded version of {n} {defs}"));
        }
        Err(e) => {
            journal::emit(world, JournalKind::Error, format!("content: cannot restore the replay's starting content: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ai_core::AiJournal;

    /// Whether the test's "session" holds content.
    #[derive(Resource)]
    struct Session(bool);

    fn in_session(world: &World) -> Option<String> {
        world.resource::<Session>().0.then(|| "during a co-op session would desync its client".to_owned())
    }

    fn lines(world: &World) -> Vec<String> {
        world.resource::<AiJournal>().after(0).0.map(ToString::to_string).collect()
    }

    #[test]
    fn an_edit_seen_on_disk_waits_while_content_is_held_says_so_once_and_goes_ahead_after() {
        let mut world = World::new();
        world.init_resource::<ai_core::SimTick>();
        world.init_resource::<AiJournal>();
        world.init_resource::<ContentDb>();
        world.insert_resource(Session(true));
        world.insert_resource(crate::ContentGuards(vec![in_session]));
        world.insert_resource(ContentLoading { dirty: true, ..Default::default() });
        apply_changes(&mut world);
        apply_changes(&mut world);
        assert!(world.resource::<ContentLoading>().dirty, "the edit waits");
        let said: Vec<String> = lines(&world).into_iter().filter(|l| l.contains("waits")).collect();
        assert_eq!(said.len(), 1, "{said:#?}");
        assert!(said[0].ends_with("content: a file changed on disk and waits: a content reload during a co-op session would desync its client; it applies when that is over"), "{}", said[0]);
        let refused = reload(&mut world, Vec::new(), Vec::new()).unwrap_err();
        assert_eq!(refused, "refused: a content reload during a co-op session would desync its client");
        // The session is over: what changed goes ahead (here nothing differs, so nothing is applied).
        world.resource_mut::<Session>().0 = false;
        apply_changes(&mut world);
        assert!(!world.resource::<ContentLoading>().dirty);
        assert!(lines(&world).iter().any(|l| l.ends_with("content: nothing holds content any more: applying what changed on disk")), "{:#?}", lines(&world));
        assert_eq!(crate::content_held(&world), None);
    }
}
