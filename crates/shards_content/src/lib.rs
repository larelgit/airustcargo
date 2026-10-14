//! # shards_content — the content database
//!
//! A game's numbers and definitions live in files, not code: attacks, weapons,
//! enemies, prefabs. Each kind is a Rust type implementing [`ContentDef`], registered
//! with [`ContentAppExt::add_content`]; its definitions live in
//! `<assets>/content/**/*.<kind>.ron`, one map of stable ids per file:
//!
//! ```ron
//! // assets/content/combat/attacks.attack.ron
//! {
//!     "sword": (windup: 0.15, recover: 0.25, damage: 25.0),
//!     "claw": (windup: 0.45, recover: 0.6, damage: 8.0),
//! }
//! ```
//!
//! - **Loading**: the folder loads through the asset server while the loading gate holds
//!   the simulation; then every file is parsed and the whole database validated
//!   (schema, duplicate ids, references between definitions). Problems are journaled
//!   with file, line and id; definitions that parsed are used anyway.
//! - **Reading**: [`ContentDb::get`] or the [`Content`] system param. Gameplay never
//!   writes content.
//! - **Hot reload** (`hot_reload` feature): an edited file is checked against the whole
//!   database; a clean edit is applied through the `content_apply` tool, recorded in
//!   the replay at its tick, and journaled: `content reloaded: combat/attacks.attack.ron
//!   (1 definition changed)`. A broken edit is journaled and the previous version kept.
//! - **Checking without a game**: [`check_dir`] for a `--check-content` CI step.
//! - **Prefabs**: entities defined in content ([`PrefabDef`], [`spawn_prefab`]).
//!
//! Tools: `content_list`, `content_get`, `content_check`, `content_reload`,
//! `content_apply`, `spawn_prefab`.

pub mod db;
pub mod load;
pub mod prefab;
mod tools;

use std::marker::PhantomData;

use ai_core::replay::ReplaySystems;
use ai_core::AiAppExt;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use shards_core::EngineAppExt;

pub use db::{check_dir, ContentDb, ContentDef, ContentError, ContentKinds, DefChange, Refs, Summary};
pub use load::{asset_root, ContentText};
pub use prefab::{spawn_prefab, PrefabDef};

/// The types a game module uses most.
pub mod prelude {
    pub use crate::{Content, ContentAppExt, ContentDb, ContentDef, ContentPlugin, PrefabDef, Refs};
}

/// Registration API for content kinds.
pub trait ContentAppExt {
    /// Registers content kind `T`: its files (`content/**/*.<KIND>.ron`) load into the
    /// [`ContentDb`]. Call it while building the app (before `App::finish`).
    fn add_content<T: ContentDef>(&mut self) -> &mut Self;

    /// Ids of kind `T` the game's code uses by name: content without one of them has an
    /// error saying `by` (`AttackKind`) needs it. Reported at load and by
    /// `--check-content`, and a hot edit that deletes one is refused.
    fn require_content<T: ContentDef>(&mut self, ids: &[&str], by: &str) -> &mut Self;
}

impl ContentAppExt for App {
    fn add_content<T: ContentDef>(&mut self) -> &mut Self {
        self.world_mut().get_resource_or_init::<ContentKinds>().register::<T>();
        self
    }

    fn require_content<T: ContentDef>(&mut self, ids: &[&str], by: &str) -> &mut Self {
        let mut kinds = self.world_mut().get_resource_or_init::<ContentKinds>();
        for id in ids {
            kinds.require::<T>(id, by);
        }
        self
    }
}

/// Typed read access to one content kind, for systems:
///
/// ```ignore
/// fn swing(attacks: Content<AttackDef>, ...) {
///     let Some(sword) = attacks.get("sword") else { return };
/// }
/// ```
#[derive(SystemParam)]
pub struct Content<'w, T: ContentDef> {
    db: Res<'w, ContentDb>,
    marker: PhantomData<fn() -> T>,
}

impl<'w, T: ContentDef> Content<'w, T> {
    /// Definition `id`.
    pub fn get(&self, id: &str) -> Option<&T> {
        self.db.get::<T>(id)
    }

    /// Ids, sorted.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.db.ids::<T>()
    }

    /// Definitions by id, sorted.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &T)> {
        self.db.iter::<T>()
    }

    /// The whole database.
    pub fn db(&self) -> &ContentDb {
        &self.db
    }
}

/// The content database, its loading, hot reload, tools and the `prefab` kind. Needs
/// the asset plugin and `CorePlugin` (`shards::EnginePlugins` adds all three).
pub struct ContentPlugin;

impl Plugin for ContentPlugin {
    fn build(&self, app: &mut App) {
        let registry = app.world().resource::<AppTypeRegistry>().clone();
        app.world_mut().get_resource_or_init::<ContentKinds>().set_type_registry(registry);
        app.init_asset::<ContentText>()
            .add_content::<PrefabDef>()
            .init_resource::<ContentDb>()
            .init_resource::<load::ContentLoading>()
            .require_loaded("content", load::content_status)
            .add_engine_module("content", env!("CARGO_PKG_VERSION"))
            .add_replay_check("content", |w| w.get_resource::<ContentDb>().map_or(0, ContentDb::hash))
            .add_systems(Startup, load::start_loading)
            .add_systems(
                Update,
                (load::finish_loading, load::rescan_folder, load::collect_changes, load::apply_changes).chain(),
            )
            .add_systems(FixedFirst, load::prepare_playback.before(ReplaySystems::Calls));
        tools::register(app.world_mut());
    }

    fn finish(&self, app: &mut App) {
        // Every kind is registered by now: one loader claims all their extensions.
        let loader = load::ContentLoader::new(app.world().resource::<ContentKinds>());
        app.register_asset_loader(loader);
    }
}
