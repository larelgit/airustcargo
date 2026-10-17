//! Loading level files through the asset server, behind the loading gate, and keeping
//! spawned levels in step with their files.
//!
//! Every content `level` definition names a file; once the content database is built they
//! all start loading and the gate holds the simulation until they are in, so a level is
//! there when the first tick runs. A game that spawns levels the content does not list
//! (tiles from a manifest) asks [`LevelLibrary::request`] and waits for
//! [`LevelLibrary::asset`]. When a file changes on disk and the asset server reloads it,
//! the spawned instances of it are replaced through the `level_reload` tool, which is an
//! action: a replay records it and plays it back.

use std::collections::{BTreeMap, BTreeSet};

use ai_core::journal::{self, JournalKind};
use ai_core::{call_tool, sim};
use bevy::asset::{AssetEvent, LoadState};
use bevy::prelude::*;
use serde_json::json;
use shards_content::ContentDb;
use shards_core::LoadStatus;

use crate::asset::{check_relative, LevelAsset};
use crate::def::LevelDef;
use crate::spawn::LevelInstances;

/// The level files that are loading or loaded, by asset path.
#[derive(Resource, Default)]
pub struct LevelLibrary {
    handles: BTreeMap<String, Handle<LevelAsset>>,
    /// The content database was built and its levels requested.
    started: bool,
    /// Everything requested at start is in: the gate lets go and stays open (a file that
    /// reloads later does not stop the game).
    ready: bool,
    /// Load failures already journaled.
    reported: BTreeSet<String>,
    /// Files the asset server reloaded, waiting for their instances to be replaced.
    changed: BTreeSet<String>,
    /// Why they wait ([`shards_content::content_held`]), as the journal said it.
    held: Option<String>,
}

impl LevelLibrary {
    /// Starts loading `path` (once; a second request returns the same handle).
    pub fn request(&mut self, server: &AssetServer, path: &str) -> Handle<LevelAsset> {
        self.handles.entry(path.to_owned()).or_insert_with(|| server.load::<LevelAsset>(path.to_owned())).clone()
    }

    /// The handle of a file that was requested.
    pub fn handle(&self, path: &str) -> Option<&Handle<LevelAsset>> {
        self.handles.get(path)
    }

    fn availability<'a>(&self, world: &'a World, path: &str) -> Availability<'a> {
        let Some(handle) = self.handles.get(path) else { return Availability::Never };
        match (world.resource::<Assets<LevelAsset>>().get(handle), world.resource::<AssetServer>().load_state(handle)) {
            (Some(asset), _) => Availability::Ready(asset),
            (None, LoadState::Failed(why)) => Availability::Failed(why.to_string()),
            (None, _) => Availability::Loading,
        }
    }

    /// The loaded level at `path`, or why there is none: never requested, still loading, or
    /// failed (with the reason).
    pub fn asset<'a>(&self, world: &'a World, path: &str) -> Result<&'a LevelAsset, String> {
        match self.availability(world, path) {
            Availability::Ready(asset) => Ok(asset),
            Availability::Never => Err(never_requested(path)),
            Availability::Failed(why) => Err(format!("level file `{path}` failed to load: {why}")),
            Availability::Loading => Err(format!("level file `{path}` is still loading; wait with `shards wait`")),
        }
    }

    /// Whether `path` is loaded, as a loading-gate answer: a game that spawns levels the
    /// content does not list (the tiles of a region) requests them, then holds the
    /// simulation with `app.require_loaded("tiles", |world| ...)` until this is `Ready` for
    /// every file. A file that failed to load, or was never requested, is `Failed`.
    pub fn status(&self, world: &World, path: &str) -> LoadStatus {
        match self.availability(world, path) {
            Availability::Ready(_) => LoadStatus::Ready,
            Availability::Never => LoadStatus::Failed(never_requested(path)),
            Availability::Failed(why) => LoadStatus::Failed(format!("level file `{path}` failed to load: {why}")),
            Availability::Loading => LoadStatus::Pending(format!("level file `{path}`")),
        }
    }
}

enum Availability<'a> {
    Never,
    Loading,
    Failed(String),
    Ready(&'a LevelAsset),
}

fn never_requested(path: &str) -> String {
    format!("level file `{path}` was never requested: ask LevelLibrary::request, or list it in a content `level` definition")
}

/// Starts loading the level file `path` (an asset path like `tiles/ruin_a.gltf`) and returns
/// its handle; asking again returns the same one. Wait for it with
/// [`LevelLibrary::status`] in a loading-gate check, then spawn with
/// [`spawn_level_file`](crate::spawn_level_file).
pub fn request_level(world: &mut World, path: &str) -> Handle<LevelAsset> {
    let server = world.resource::<AssetServer>().clone();
    world.resource_mut::<LevelLibrary>().request(&server, path)
}

/// The loaded level at `path` (see [`LevelLibrary::asset`]).
pub fn level_asset<'a>(world: &'a World, path: &str) -> Result<&'a LevelAsset, String> {
    match world.get_resource::<LevelLibrary>() {
        Some(library) => library.asset(world, path),
        None => Err("LevelPlugin is not added".to_owned()),
    }
}

/// Why no level may be spawned from `path` now: the file changed on disk while content is held
/// ([`shards_content::content_held`], a co-op session). The asset server has read the new version
/// already, the other machines play the old one: what is spawned from it would desync them. What
/// was spawned from it before stays as it was until the change applies.
pub fn held_change(world: &World, path: &str) -> Option<String> {
    if !world.get_resource::<LevelLibrary>()?.changed.contains(&path.replace('\\', "/")) {
        return None;
    }
    let why = shards_content::content_held(world)?;
    Some(format!("refused: `{path}` changed on disk, and a level spawn from the changed file {why}"))
}

/// The loading gate's view: ready once the content database is built and its levels are in.
pub(crate) fn level_status(world: &World) -> LoadStatus {
    let library = world.resource::<LevelLibrary>();
    if library.ready {
        return LoadStatus::Ready;
    }
    if !library.started {
        return LoadStatus::Pending("waiting for content".into());
    }
    let server = world.resource::<AssetServer>();
    let left = library.handles.values().filter(|h| !is_done(server, h)).count();
    LoadStatus::Pending(match left {
        1 => "1 file left".into(),
        n => format!("{n} files left"),
    })
}

fn is_done(server: &AssetServer, handle: &Handle<LevelAsset>) -> bool {
    matches!(server.load_state(handle), LoadState::Loaded | LoadState::Failed(_))
}

/// `Update`: once the content database is built, starts loading every level it lists
/// (also ones a hot edit adds later), journals the files that fail, and opens the gate
/// when all are in.
pub(crate) fn request_levels(world: &mut World) {
    if !world.resource::<ContentDb>().is_loaded() {
        return;
    }
    let files: Vec<String> = world.resource::<ContentDb>().iter::<LevelDef>().map(|(_, def)| def.file.clone()).collect();
    let server = world.resource::<AssetServer>().clone();
    let mut library = world.resource_mut::<LevelLibrary>();
    for file in files.iter().filter(|f| check_relative(f).is_ok()) {
        library.request(&server, file);
    }
    library.started = true;

    let mut failures = Vec::new();
    let mut all_done = true;
    for (path, handle) in &library.handles {
        match server.load_state(handle) {
            LoadState::Failed(why) => failures.push((path.clone(), why.to_string())),
            LoadState::Loaded => {}
            _ => all_done = false,
        }
    }
    library.ready |= all_done;
    let fresh: Vec<(String, String)> = failures.into_iter().filter(|(path, _)| library.reported.insert(path.clone())).collect();
    for (path, why) in fresh {
        journal::emit(world, JournalKind::Error, format!("level: cannot load `{path}`: {why}"));
    }
}

/// `Update`: files the asset server reloaded because they changed on disk.
pub(crate) fn collect_changed(mut events: MessageReader<AssetEvent<LevelAsset>>, server: Res<AssetServer>, mut library: ResMut<LevelLibrary>) {
    for event in events.read() {
        if let AssetEvent::Modified { id } = event
            && let Some(path) = server.get_path(*id)
        {
            library.changed.insert(path.path().to_string_lossy().replace('\\', "/"));
        }
    }
}

/// `Update`, after [`collect_changed`]: replaces the instances of a changed file through
/// `level_reload`. Not while the game loads: an action cannot run then, so the change waits.
pub(crate) fn apply_changed(world: &mut World) {
    if world.resource::<LevelLibrary>().changed.is_empty() || sim::gate_message(world).is_some() {
        return;
    }
    // A module holds content (a co-op session): the change waits, said once.
    if let Some(why) = shards_content::content_held(world) {
        let mut library = world.resource_mut::<LevelLibrary>();
        if library.held.as_deref() != Some(why.as_str()) {
            library.held = Some(why.clone());
            journal::emit(world, JournalKind::Warn, format!("level: a level file changed on disk and waits: a level reload {why}; it applies when that is over"));
        }
        return;
    }
    world.resource_mut::<LevelLibrary>().held = None;
    let changed = std::mem::take(&mut world.resource_mut::<LevelLibrary>().changed);
    for file in changed {
        let used = world.get_resource::<LevelInstances>().is_some_and(|i| i.iter().any(|i| i.path == file));
        if used {
            let outcome = call_tool(world, "level", "level_reload", json!({"file": file}));
            if !outcome.ok {
                let why = outcome.result.as_str().unwrap_or_default().to_owned();
                journal::emit(world, JournalKind::Error, format!("level: {why}"));
            }
        }
    }
}
