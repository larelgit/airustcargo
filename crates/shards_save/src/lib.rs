//! # shards_save — saves: sections, slots, migrations, safe files
//!
//! Two kinds of save: **character** (each player's progress, kept on that player's machine)
//! and **world** (the host's). Each is a set of *sections*, explicit structures the game
//! registers; nothing is saved that no section names ([`section`]). A save is the player's
//! progress applied onto a world the game built the usual way, not a snapshot of the
//! simulation (`snapshot_save` is that, for developers).
//!
//! ```ignore
//! app.save_component::<Bag>(SaveKind::Character, "bag", 1)       // on the player's body
//!    .save_component::<Opened>(SaveKind::World, "chests", 1)      // on entities with a SaveKey
//!    .save_resource::<Calendar>("calendar", 1)
//!    .add_save_section::<Quests>()                                // a struct that captures and applies itself
//!    .add_save_migration("bag", 1, |old| { /* JSON of version 1 -> version 2 */ Ok(old) })
//!    .autosave_on::<CampRested>(SaveWhat::All);                   // written at the end of the tick
//! ```
//!
//! - **File** ([`file`]): `<save_dir>/<kind>/<slot>.save`, one header line
//!   `shards-save/1 crc32=<hex> len=<bytes>` and canonical JSON (sorted keys, shortest
//!   numbers) with `{kind, slot, label, game, engine, tick, sections: {name: {version, data}}}`.
//!   Slot names are `[a-z0-9_-]{1,32}`, never paths.
//! - **Writing** ([`store`]) is atomic: a temporary file, flushed, the older saves shifted
//!   into three rotating backups (`.bak1` newest), a rename, a flush of the folder.
//! - **Reading** takes the primary, or on any damage (a bad checksum, a truncated or garbled
//!   file, JSON that does not read, a section from a newer version, a failed migration) the
//!   newest good backup, rewrites the primary from it and says so in the journal (`save:
//!   world/slot1.save is damaged (checksum mismatch); restored from backup 1 (tick 50110)`).
//!   When every copy is bad, the error names each copy's problem and nothing is applied.
//! - **Versions**: a section's version is in the file; an older one runs the chain of
//!   migrations (`add_save_migration`), a newer one is refused, a section the game does not
//!   register is skipped with a warning, one the file lacks gets its `missing()`. A chain with
//!   a gap panics when the app is finished.
//! - **When**: [`save_now`] writes at once; [`SaveRequests::request`] and
//!   [`autosave_on`](SaveAppExt::autosave_on) write at `FixedLast` of the tick, debounced per
//!   kind by `*.save.ron` content ([`SaveDef`]). Saving never changes the simulation.
//!
//! Tools: `save_game`, `load_game` (an action: the recording restarts from the loaded state),
//! `save_check`, `save_list`, `save_registry`, `save_damage` (QA of the recovery).
//! `ai/status` gets a `save` field (the folder, the current slots, how many sections).

pub mod autosave;
pub mod canon;
pub mod file;
pub mod ops;
pub mod section;
pub mod store;
mod tools;

use ai_core::replay::ReplaySystems;
use ai_core::AiAppExt;
use bevy::prelude::*;
use serde::de::DeserializeOwned;
use serde::Serialize;
use serde_json::{json, Value};
use shards_content::ContentAppExt;
use shards_core::EngineAppExt;

pub use autosave::{SaveDef, SaveRequests, DEFAULT_DEBOUNCE_SECS};
pub use file::{GameInfo, SaveFile, SectionData, SectionSet, FORMAT};
pub use ops::{
    apply_sections, capture_sections, check_parts, check_state, default_body, load_parts, load_slot, prepare, read_file, resolve_body, save_now,
    save_part, save_parts, save_slot, write_sections, Applied, ApplyError, CheckReport, FileCheck, LoadReport, Prepared, SaveConfig, SaveReport,
    SaveSlots, SaveWhat,
};
pub use section::{Migration, SaveKey, SaveKind, SaveRegistry, SaveSection, SectionEntry};
pub use store::{valid_slot, Damage, Recovery, Store, Which};

/// Version of the engine crates (they share one).
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The types a game module uses most.
pub mod prelude {
    pub use crate::{SaveAppExt, SaveKey, SaveKind, SaveRequests, SaveSection, SaveWhat};
}

/// The save module: registry, config, slots, autosave, tools. Needs `AiCorePlugin` first;
/// `shards::EnginePlugins` adds it (and the engine's own sections).
pub struct SavePlugin;

impl Plugin for SavePlugin {
    fn build(&self, app: &mut App) {
        assert!(
            app.is_plugin_added::<ai_core::AiCorePlugin>(),
            "SavePlugin needs AiCorePlugin added first (shards::EnginePlugins adds both)"
        );
        // SEAM(C1): the folder comes from `EngineConfig.save_dir` once C1 adds it
        // (`--save-dir`); until then the default `saves/` next to `replays/`.
        app.register_type::<SaveKey>()
            .init_resource::<SaveRegistry>()
            .init_resource::<SaveConfig>()
            .init_resource::<SaveSlots>()
            .init_resource::<SaveRequests>()
            .add_content::<SaveDef>()
            .add_engine_module("save", VERSION)
            .add_ai_status("save", status)
            .add_systems(FixedLast, autosave::write_requests.after(ReplaySystems::Remember));
        tools::register(app.world_mut());
    }

    fn finish(&self, app: &mut App) {
        check_registry(app);
    }

    fn cleanup(&self, app: &mut App) {
        // Sections a game registered in its own `finish`.
        check_registry(app);
    }
}

fn check_registry(app: &App) {
    if let Err(e) = app.world().resource::<SaveRegistry>().check_chains() {
        panic!("{e}");
    }
}

/// `ai/status` → `save: {dir, slots, sections}`.
fn status(world: &World) -> Value {
    let config = world.get_resource::<SaveConfig>();
    let slots = world.get_resource::<SaveSlots>();
    let sections = world.get_resource::<SaveRegistry>().map_or(0, |r| r.sections().len());
    json!({
        "dir": config.map(|c| c.dir.display().to_string()),
        "slots": slots.map(|s| json!({"world": s.world, "character": s.character})),
        "sections": sections,
    })
}

/// Registration API for games and engine modules.
pub trait SaveAppExt {
    /// Registers a section ([`SaveSection`]). One with the same name replaces the first.
    fn add_save_section<S: SaveSection>(&mut self) -> &mut Self;

    /// Saves component `C`: for [`SaveKind::Character`] the one on the player's body (absent
    /// is saved as such and applied as nothing); for [`SaveKind::World`] a map from
    /// [`SaveKey`] to the component over every entity with a key.
    fn save_component<C>(&mut self, kind: SaveKind, name: &'static str, version: u32) -> &mut Self
    where
        C: Component<Mutability = bevy::ecs::component::Mutable> + Serialize + DeserializeOwned + Clone;

    /// Saves resource `R` in the world save.
    fn save_resource<R>(&mut self, name: &'static str, version: u32) -> &mut Self
    where
        R: Resource + Serialize + DeserializeOwned + Clone;

    /// How to read data saved at `from_version` of section `name` as the next version: old
    /// JSON in, new JSON out (old structs need not exist). Every version below the
    /// section's current one needs one, or the app panics when it is finished.
    fn add_save_migration(&mut self, name: &str, from_version: u32, migrate: Migration) -> &mut Self;

    /// Saves `what` at the end of the tick whenever event `E` is triggered. The journal
    /// names the reason as the event's name in snake case (`camp_rested`).
    fn autosave_on<E: Event>(&mut self, what: SaveWhat) -> &mut Self;

    /// [`autosave_on`](Self::autosave_on) with a reason of your own.
    fn autosave_on_as<E: Event>(&mut self, what: SaveWhat, reason: &str) -> &mut Self;
}

impl SaveAppExt for App {
    fn add_save_section<S: SaveSection>(&mut self) -> &mut Self {
        self.world_mut().get_resource_or_init::<SaveRegistry>().add(SectionEntry::of::<S>());
        self
    }

    fn save_component<C>(&mut self, kind: SaveKind, name: &'static str, version: u32) -> &mut Self
    where
        C: Component<Mutability = bevy::ecs::component::Mutable> + Serialize + DeserializeOwned + Clone,
    {
        let entry = match kind {
            SaveKind::Character => SectionEntry::character_component::<C>(name, version),
            SaveKind::World => SectionEntry::world_component::<C>(name, version),
        };
        self.world_mut().get_resource_or_init::<SaveRegistry>().add(entry);
        self
    }

    fn save_resource<R>(&mut self, name: &'static str, version: u32) -> &mut Self
    where
        R: Resource + Serialize + DeserializeOwned + Clone,
    {
        self.world_mut().get_resource_or_init::<SaveRegistry>().add(SectionEntry::resource::<R>(name, version));
        self
    }

    fn add_save_migration(&mut self, name: &str, from_version: u32, migrate: Migration) -> &mut Self {
        self.world_mut().get_resource_or_init::<SaveRegistry>().add_migration(name, from_version, migrate);
        self
    }

    fn autosave_on<E: Event>(&mut self, what: SaveWhat) -> &mut Self {
        let name = std::any::type_name::<E>().rsplit("::").next().unwrap_or("event");
        let reason = autosave::snake_case(name.split('<').next().unwrap_or(name));
        self.autosave_on_as::<E>(what, &reason)
    }

    fn autosave_on_as<E: Event>(&mut self, what: SaveWhat, reason: &str) -> &mut Self {
        let reason = reason.to_owned();
        self.add_observer(move |_: On<E>, mut requests: ResMut<SaveRequests>| requests.request(what, reason.clone()));
        self
    }
}
