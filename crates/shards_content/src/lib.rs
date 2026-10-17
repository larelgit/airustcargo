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
//!   with file, line and id; definitions that parsed are used anyway, also those next to
//!   a definition that did not (only a broken map, a syntax error or a bad id, drops its file).
//! - **Reading**: [`ContentDb::get`] or the [`Content`] system param. Gameplay never
//!   writes content.
//! - **Hot reload** (`hot_reload` feature): an edited file is checked against the whole
//!   database; a clean edit is applied through the `content_apply` tool, recorded in
//!   the replay at its tick, and journaled: `content reloaded: combat/attacks.attack.ron
//!   (1 definition changed)`. A broken edit is journaled and the previous version kept: a
//!   file applies whole, so one broken definition keeps the previous version of all of its file.
//!   While a module holds content ([`ContentGuards`]: a co-op session) an edit waits and
//!   the tools refuse.
//! - **Checking without a game**: [`check_dir`] for a `--check-content` CI step.
//! - **Prefabs**: entities defined in content ([`PrefabDef`], [`spawn_prefab`]).
//!
//! Tools: `content_list`, `content_get`, `content_check`, `content_reload`,
//! `content_apply`, `spawn_prefab`.

pub mod color;
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

pub use color::parse_color;
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

    /// A check of kind `T`'s definitions that a module other than the kind's own adds: it
    /// runs after the kind's `validate`, wherever content is validated (see
    /// [`ContentKinds::add_check`]).
    fn add_content_check<T: ContentDef>(&mut self, name: &'static str, check: fn(&T, &str, &mut Refs)) -> &mut Self;

    /// A module that cannot have this game's content change under it at times says when, and
    /// why ([`ContentGuards`]).
    fn add_content_guard(&mut self, guard: fn(&World) -> Option<String>) -> &mut Self;
}

/// Why the content of this game must not change right now, from the modules that may say so:
/// while a guard gives a reason, `content_apply`, `content_reload` and a level's reload refuse
/// with it, and a file changed on disk waits (said once in the journal) until no guard holds.
/// A co-op session holds content while other machines play it (`shards_net`): changed on one of
/// them only, it would desync them. The reason finishes the sentence "a content reload …":
/// `during a co-op session would desync its 2 clients; …`.
#[derive(Resource, Default, Clone)]
pub struct ContentGuards(Vec<fn(&World) -> Option<String>>);

/// The first reason a guard gives for content to stay as it is now ([`ContentGuards`]).
pub fn content_held(world: &World) -> Option<String> {
    world.get_resource::<ContentGuards>()?.0.iter().find_map(|guard| guard(world))
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

    fn add_content_check<T: ContentDef>(&mut self, name: &'static str, check: fn(&T, &str, &mut Refs)) -> &mut Self {
        self.world_mut().get_resource_or_init::<ContentKinds>().add_check::<T>(name, check);
        self
    }

    fn add_content_guard(&mut self, guard: fn(&World) -> Option<String>) -> &mut Self {
        self.world_mut().get_resource_or_init::<ContentGuards>().0.push(guard);
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
            // Content is not in a snapshot (it is files); a snapshot loads where it is the same.
            .add_snapshot_check(
                "content",
                "the content files differ from the ones the snapshot was taken with, or a content edit came before \
                 it: put those files back, or take the snapshot again",
                |w| w.get_resource::<ContentDb>().map_or(0, ContentDb::hash),
            )
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
