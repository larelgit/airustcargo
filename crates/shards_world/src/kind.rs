//! Region kinds: content kinds another crate adds, built behind the same region API.
//!
//! The world module knows two layouts of its own, a grid of tiles (`region` content) and a
//! dungeon of rooms (`dungeon` content). A crate adds a third without this crate depending on it
//! by registering a [`RegionKind`] ([`WorldAppExt::add_region_kind`](crate::WorldAppExt::add_region_kind)):
//! its content kind (`voxel_region`), its [`Layout`] and the functions that generate, check,
//! build and forget a region of it. Everything else is the world module's and the same for every
//! kind: `regen`, `region_load` and [`change_region`](crate::change_region) (through
//! `EngineState::RegionLoading`), the [`RegionManifest`] and its hash, the points of interest
//! ([`Poi`](crate::Poi)), the markers ([`Marker`](crate::Marker)), the spawn and where players
//! appear ([`ActiveRegion::spawn_point`](crate::ActiveRegion::spawn_point)), the active radius,
//! snapshots (by [`RegionBuild`](crate::RegionBuild)), saves (the `region` section), the
//! worldgen report and `region_info`. A game picks the kind in content (`regen region=<id>`
//! names a definition of any kind), so swapping one kind of region for another needs no change
//! to gameplay code.
//!
//! **The rule for games:** what is particular to a kind (a voxel volume's cells, its palette)
//! is for the kind's generation steps only. Gameplay code reads the world through the region
//! API above and through physics, never through the kind's own types, so the region can become
//! another kind by content.

use std::any::Any;
use std::fmt;
use std::sync::Arc;

use bevy::prelude::*;
use shards_content::ContentDb;

use crate::generate::{GenError, Generated, WorldGen};
use crate::manifest::{Layout, RegionManifest};

/// Generates region `id` of the kind for `seed`: a pure function of the seed, the content and
/// the code registered at build time (the kind's [`data`](RegionKind::data)).
pub type KindGenerateFn = fn(&ContentDb, &WorldGen, &str, u64) -> Result<Generated, GenError>;

/// Whether content can build `manifest` now, before anything changes: the definition, what it
/// is made from and the code that made it (a generation step of another version is refused,
/// naming the step).
pub type KindCheckFn = fn(&World, &RegionManifest) -> Result<(), String>;

/// Builds what is particular to the kind for `manifest` (its terrain, water), as top-level
/// entities with a world `Transform` where they have one, and returns them in spawn order. The
/// world module tags them [`RegionPart`](crate::RegionPart) and spawns the points of interest
/// after them. On an error it leaves nothing behind (its own entities and resources).
pub type KindSpawnFn = fn(&mut World, &RegionManifest) -> Result<Vec<Entity>, String>;

/// A region content kind of another crate.
#[derive(Clone)]
pub struct RegionKind {
    /// Its content kind: `voxel_region`.
    pub content: &'static str,
    /// The layout its manifests have (one the world module does not build itself).
    pub layout: Layout,
    /// What its manifest's cells are, for messages: `chunks`.
    pub noun: &'static str,
    /// The ids of its definitions in `db`, sorted.
    pub ids: fn(&ContentDb) -> Vec<String>,
    /// The generator version of definition `id` (saves say when it changed).
    pub version: fn(&ContentDb, &str) -> Option<u32>,
    pub generate: KindGenerateFn,
    pub check: KindCheckFn,
    pub spawn: KindSpawnFn,
    /// Removes what [`spawn`](Self::spawn) kept outside its entities (a resource), after the
    /// region's entities are gone. Called whenever a region is unloaded, of any kind: it
    /// removes only its own and does nothing when there is none.
    pub unload: fn(&mut World),
    /// The active radius definition `id` gives (`None`: the world settings').
    pub active_radius: fn(&ContentDb, &str) -> Option<f32>,
    /// What the `region loaded` journal line says of the region in the world, after its
    /// title: `128 x 32 x 128, 3 water boxes`.
    pub summary: fn(&World) -> String,
    /// What `--check-content` finds by generating definition `id` (a seed every step fails
    /// on, a fallback that does not pass): one line each, empty when it generates. Run on
    /// content that validated, never inside validation (which runs on every reload).
    pub content_check: fn(&ContentDb, &WorldGen, &str) -> Vec<String>,
    /// Code the kind's functions read that a game registers at build time (a step registry),
    /// set with [`WorldGen::set_kind_data`]: the generator is cloned into reports and saves,
    /// and takes it along.
    pub data: Option<Arc<dyn Any + Send + Sync>>,
}

impl RegionKind {
    /// The [`data`](Self::data) as `T`.
    pub fn data<T: Any>(&self) -> Option<&T> {
        self.data.as_ref()?.downcast_ref::<T>()
    }

    /// Whether definition `id` is of this kind in `db`.
    pub fn defines(&self, db: &ContentDb, id: &str) -> bool {
        (self.ids)(db).iter().any(|known| known == id)
    }
}

impl fmt::Debug for RegionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RegionKind").field("content", &self.content).field("layout", &self.layout).field("data", &self.data.is_some()).finish_non_exhaustive()
    }
}

/// What generating every definition of every region kind finds ([`RegionKind::content_check`]),
/// as content errors naming the file: `--check-content` runs it after content validates.
pub fn generation_problems(db: &ContentDb, worldgen: &WorldGen) -> Vec<shards_content::ContentError> {
    let mut out = Vec::new();
    for kind in worldgen.kinds() {
        for id in (kind.ids)(db) {
            for message in (kind.content_check)(db, worldgen, &id) {
                let file = db.file(kind.content, &id).map(str::to_owned);
                out.push(shards_content::ContentError { file, position: None, kind: Some(kind.content.to_owned()), id: Some(id.clone()), message });
            }
        }
    }
    out
}

impl WorldGen {
    /// Registers `kind`; a kind of the same content kind is replaced. The layout must be one
    /// the world module does not build itself.
    pub fn add_kind(&mut self, kind: RegionKind) {
        assert!(!matches!(kind.layout, Layout::Grid | Layout::Rooms), "region kind `{}`: grids and rooms are the world module's own layouts", kind.content);
        self.kinds.retain(|k| k.content != kind.content);
        self.kinds.push(kind);
    }

    /// The registered region kinds, in registration order.
    pub fn kinds(&self) -> &[RegionKind] {
        &self.kinds
    }

    /// The kind of content kind `content`.
    pub fn kind(&self, content: &str) -> Option<&RegionKind> {
        self.kinds.iter().find(|k| k.content == content)
    }

    /// Sets what kind `content`'s functions read ([`RegionKind::data`]). A kind not registered
    /// changes nothing.
    pub fn set_kind_data<T: Any + Send + Sync>(&mut self, content: &str, data: T) {
        if let Some(kind) = self.kinds.iter_mut().find(|k| k.content == content) {
            kind.data = Some(Arc::new(data));
        }
    }

    /// The kind whose content defines region `id`, the first registered first.
    pub fn kind_of(&self, db: &ContentDb, id: &str) -> Option<&RegionKind> {
        self.kinds.iter().find(|k| k.defines(db, id))
    }

    /// The kind that builds `manifest`: the one of its layout that defines its region, else the
    /// first of its layout (whose check then says the definition is gone). `None` for tiles
    /// and rooms, which the world module builds.
    pub fn kind_for(&self, db: &ContentDb, manifest: &RegionManifest) -> Option<&RegionKind> {
        let of_layout = || self.kinds.iter().filter(|k| k.layout == manifest.layout);
        of_layout().find(|k| k.defines(db, &manifest.region)).or_else(|| of_layout().next())
    }
}
