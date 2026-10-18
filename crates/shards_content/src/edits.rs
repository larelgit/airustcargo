//! What a game's content holds apart from its files, as snapshot state.
//!
//! The content database is built from files, and a snapshot only checks it (a hash, the
//! `content` check): a load refuses where the files differ. But `content_apply` can set
//! definitions no file says (an agent's JSON, a replayed call), and those belong to the session
//! a snapshot is taken in. [`ContentEdits`] names them, the snapshot saves it, and a load puts
//! the saved edits over the files of the game it loads into before the `content` check compares
//! the two databases ([`snapshot_prepare`]). A replay that starts from such a snapshot plays in a
//! fresh game the same way.
//!
//! A hot reload adopts its file, so its change is not an edit: a fresh game reads the same file.

use std::collections::BTreeSet;

use ai_core::Snapshot;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};

use crate::db::{ContentDb, ContentKinds, DefChange, Source};

/// The definitions the content database has apart from its files, in kind and id order.
/// Present while there is one, absent while the database is its files: a snapshot of a game
/// without edits is what it was before edits were saved. Snapshot state, kept on each machine
/// of a co-op session (content is held there, and a client's content is its own files).
#[derive(Resource, Reflect, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[reflect(Resource, Default)]
pub struct ContentEdits {
    pub edits: Vec<ContentEdit>,
}

/// One definition as the database has it and its file does not say. Plain strings, so the
/// snapshot's JSON reads back with serde as well as through reflection.
#[derive(Reflect, Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ContentEdit {
    /// Content kind: `action`.
    pub kind: String,
    pub id: String,
    /// The file the definition is in (was in, when the edit removed it); empty for one no file
    /// has.
    pub file: String,
    /// The definition as JSON; empty when the edit removed it.
    pub def: String,
}

impl ContentEdit {
    fn of(change: DefChange) -> ContentEdit {
        ContentEdit {
            kind: change.kind,
            id: change.id,
            file: change.file.unwrap_or_default(),
            def: change.def.map(|d| d.to_string()).unwrap_or_default(),
        }
    }

    /// The change that makes the definition what this edit says.
    pub(crate) fn change(&self) -> Result<DefChange, String> {
        let def = match self.def.as_str() {
            "" => None,
            text => Some(serde_json::from_str(text).map_err(|e| format!("{} `{}` does not read: {e}", self.kind, self.id))?),
        };
        let file = (!self.file.is_empty()).then(|| self.file.clone());
        Ok(DefChange { kind: self.kind.clone(), id: self.id.clone(), file, def, before: None })
    }
}

/// The database `db`'s files build, with no checks: what the edits are made over.
fn files_of(kinds: &ContentKinds, db: &ContentDb) -> ContentDb {
    let sources: Vec<Source> = db.sources().into_iter().map(|(path, text)| Source { path, text }).collect();
    ContentDb::build_unchecked(kinds, &sources)
}

/// Makes [`ContentEdits`] say how the database differs from its files: inserted while it does,
/// removed while it does not. Called wherever the database changes.
pub(crate) fn refresh(world: &mut World) {
    let edits = {
        let Some(db) = world.get_resource::<ContentDb>().filter(|db| db.is_loaded()) else { return };
        let kinds = world.resource::<ContentKinds>();
        files_of(kinds, db).changes_to(db).into_iter().map(ContentEdit::of).collect::<Vec<_>>()
    };
    if edits.is_empty() {
        if world.contains_resource::<ContentEdits>() {
            world.remove_resource::<ContentEdits>();
        }
    } else if world.get_resource::<ContentEdits>().is_none_or(|now| now.edits != edits) {
        world.insert_resource(ContentEdits { edits });
    }
}

/// Snapshot prepare `content`: the database becomes this game's files with the snapshot's edits
/// over them (none when it has none), so the `content` check after it compares what the two
/// sessions played with. Files that differ still differ, and the check refuses the load. A load
/// refused later runs this again with the world as it was, which puts its own edits back.
pub(crate) fn snapshot_prepare(world: &mut World, snapshot: &Snapshot) -> Result<Option<String>, String> {
    let saved = match snapshot.resources.get(<ContentEdits as TypePath>::type_path()) {
        Some(value) => serde_json::from_value::<ContentEdits>(value.clone()).map_err(|e| format!("its content edits do not read: {e}"))?,
        None => ContentEdits::default(),
    };
    let current = world.get_resource::<ContentEdits>().cloned().unwrap_or_default();
    if saved == current {
        put_back_reads(world, snapshot);
        return Ok(None);
    }
    let changes = saved.edits.iter().map(ContentEdit::change).collect::<Result<Vec<_>, _>>()?;
    let next = {
        let kinds = world.resource::<ContentKinds>();
        files_of(kinds, world.resource::<ContentDb>()).with_changes(kinds, &[], &changes)?
    };
    crate::install(world, next);
    refresh(world);
    put_back_reads(world, snapshot);
    Ok(Some(match changes.len() {
        0 => "the definitions are the content files' again".to_owned(),
        1 => "1 definition as the snapshot's session had it, over the content files".to_owned(),
        n => format!("{n} definitions as the snapshot's session had them, over the content files"),
    }))
}

/// What the simulation has read becomes what it had read in the session the snapshot was taken
/// in, so the `content` check after the prepare compares those definitions, and the session goes
/// on reading what that one did (a playback from the snapshot, a heavy-mode twin, a refused load
/// putting the world back). A snapshot without the parts was taken when the check hashed every
/// definition: what is read does not matter to it, and stays as it is.
fn put_back_reads(world: &World, snapshot: &Snapshot) {
    let Some(parts) = snapshot.check_parts.get("content") else { return };
    let names: BTreeSet<&str> = parts.keys().map(String::as_str).collect();
    world.resource::<ContentDb>().set_used(&names);
}

/// The edits a snapshot carries, as changes (for a module that looks ahead at what a replay's
/// start will put in: the files its definitions name).
pub(crate) fn changes_in(snapshot: &Snapshot) -> Vec<DefChange> {
    let Some(value) = snapshot.resources.get(<ContentEdits as TypePath>::type_path()) else { return Vec::new() };
    let Ok(saved) = serde_json::from_value::<ContentEdits>(value.clone()) else { return Vec::new() };
    saved.edits.iter().filter_map(|edit| edit.change().ok()).collect()
}
