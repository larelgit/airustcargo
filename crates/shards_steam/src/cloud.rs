//! Saves in Steam Cloud. Every save written ([`SaveWritten`]: the new primary of a slot) is copied to
//! the cloud as `<save dir>/<kind>/<slot>.save`; backups, `.tmp` and `.damaged` never are. At startup,
//! before anything loads a slot, each cloud copy is held against the file on this machine:
//!
//! - the same, or none in the cloud: nothing (a save only here is copied up);
//! - the cloud's is newer (written more than [`SLACK_SECS`] after this machine's): it becomes the
//!   save here, the old one goes to the first backup;
//! - this machine's is newer: it goes up;
//! - neither is clearly newer: this machine's stays, the cloud's is kept beside it as slot
//!   `<slot>-cloud` (which stays on this machine), and the journal says so.
//!
//! The files on disk stay the truth: a copy the cloud cannot take (quota, Steam Cloud off) changes
//! nothing here, and a cloud copy that is damaged or from a newer version of the game is never taken.

use std::path::Path;
use std::time::UNIX_EPOCH;

use ai_core::journal::{self, JournalKind};
use bevy::prelude::*;
use shards_core::EngineConfig;
use shards_save::{valid_slot, SaveConfig, SaveFile, SaveKind, SaveWritten, Store, Which};

use crate::Steam;

/// Seconds two writes may be apart and still count as the same moment.
pub const SLACK_SECS: i64 = 2;

/// What the cloud did in this run.
#[derive(Resource, Debug, Default, Clone)]
pub struct Cloud {
    /// Ok, or why saves are not mirrored (Steam Cloud off, no Steam).
    pub enabled: Option<Result<(), String>>,
    /// Saves copied up as they were written.
    pub mirrored: u32,
    /// At startup: taken from the cloud, sent up, kept both (`world/slot1`).
    pub pulled: Vec<String>,
    pub pushed: Vec<String>,
    pub kept_both: Vec<String>,
    /// The last copy that failed, and why.
    pub last_error: Option<String>,
}

/// What to do with a slot that has a cloud copy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reconcile {
    Same,
    Pull,
    Push,
    KeepBoth,
}

/// This machine's save (bytes, written at) against the cloud's.
pub fn reconcile(local: Option<(&[u8], i64)>, cloud: (&[u8], i64)) -> Reconcile {
    match local {
        None => Reconcile::Pull,
        Some((bytes, _)) if bytes == cloud.0 => Reconcile::Same,
        Some((_, at)) if cloud.1 > at + SLACK_SECS => Reconcile::Pull,
        Some((_, at)) if at > cloud.1 + SLACK_SECS => Reconcile::Push,
        Some(_) => Reconcile::KeepBoth,
    }
}

/// The folder of the saves as cloud names start: `saves`, or the game's own folder under it
/// (`--save-dir saves/test-run`). A folder outside the game's (absolute, `..`) is `saves`: a cloud
/// name never holds a path of this machine.
pub fn cloud_prefix(save_dir: &str) -> String {
    let dir = save_dir.replace('\\', "/");
    let dir = dir.trim_start_matches("./").trim_end_matches('/');
    let outside = dir.is_empty() || dir.starts_with('/') || dir.contains(':') || dir.split('/').any(|part| part == "..");
    if outside { "saves".to_owned() } else { dir.to_owned() }
}

fn prefix(world: &World) -> String {
    cloud_prefix(&world.get_resource::<EngineConfig>().map_or_else(|| "saves".to_owned(), |c| c.save_dir.clone()))
}

/// `saves/world/slot1.save`.
pub fn cloud_name(prefix: &str, kind: SaveKind, slot: &str) -> String {
    format!("{prefix}/{kind}/{slot}.save")
}

/// The kind and slot a cloud name stands for, under `prefix`.
pub fn parse_cloud_name(prefix: &str, name: &str) -> Option<(SaveKind, String)> {
    let rest = name.strip_prefix(prefix)?.strip_prefix('/')?;
    let (kind, file) = rest.split_once('/')?;
    let kind = SaveKind::ALL.into_iter().find(|k| k.as_str() == kind)?;
    let slot = file.strip_suffix(".save")?;
    valid_slot(slot).ok()?;
    Some((kind, slot.to_owned()))
}

/// What ends the slot a cloud copy is kept in beside this machine's.
const BESIDE: &str = "-cloud";

/// The slot the cloud's copy is kept in beside this machine's (never copied up at startup).
pub fn beside_slot(slot: &str) -> String {
    let keep = slot.len().min(32 - BESIDE.len());
    format!("{}{BESIDE}", &slot[..keep])
}

fn written_at(path: &Path) -> Option<i64> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    Some(modified.duration_since(UNIX_EPOCH).ok()?.as_secs() as i64)
}

/// [`SaveWritten`]: the new save, copied to the cloud.
pub(crate) fn on_save_written(event: On<SaveWritten>, mut commands: Commands) {
    let SaveWritten { kind, slot, bytes } = event.event().clone();
    commands.queue(move |world: &mut World| mirror(world, kind, &slot, &bytes));
}

fn mirror(world: &mut World, kind: SaveKind, slot: &str, bytes: &[u8]) {
    let name = cloud_name(&prefix(world), kind, slot);
    let mut steam = world.resource_mut::<Steam>();
    if steam.backend.cloud_enabled().is_err() {
        return;
    }
    let written = steam.backend.cloud_write(&name, bytes);
    let mut cloud = world.resource_mut::<Cloud>();
    match written {
        Ok(()) => cloud.mirrored += 1,
        Err(e) => {
            cloud.last_error = Some(format!("{name}: {e}"));
            journal::emit(world, JournalKind::Warn, format!("steam: the cloud copy of {kind}/{slot} was not written: {e}; the save on this machine is fine"));
        }
    }
}

/// `PreStartup`, before any slot is loaded: the cloud's saves held against this machine's.
pub(crate) fn pull(world: &mut World) {
    let enabled = world.resource::<Steam>().backend.cloud_enabled();
    world.resource_mut::<Cloud>().enabled = Some(enabled.clone());
    if enabled.is_err() {
        return;
    }
    let Some(store) = world.get_resource::<SaveConfig>().map(SaveConfig::store) else { return };
    let prefix = prefix(world);
    let mut files: Vec<(String, u64)> = world.resource::<Steam>().backend.cloud_files();
    files.sort();
    let mut in_cloud = Vec::new();
    for (name, _) in files {
        let Some((kind, slot)) = parse_cloud_name(&prefix, &name) else { continue };
        in_cloud.push((kind, slot.clone()));
        pull_one(world, &store, &prefix, &name, kind, &slot);
    }
    // Saves only here go up: the cloud gets what this machine had before it had Steam.
    for kind in SaveKind::ALL {
        let Ok(entries) = std::fs::read_dir(store.dir(kind)) else { continue };
        let mut slots: Vec<String> = entries
            .filter_map(|e| e.ok()?.file_name().to_str()?.strip_suffix(".save").map(str::to_owned))
            // A cloud copy kept beside a slot stays on this machine: the cloud has its own.
            .filter(|slot| valid_slot(slot).is_ok() && !slot.ends_with(BESIDE) && !in_cloud.contains(&(kind, slot.clone())))
            .collect();
        slots.sort();
        for slot in slots {
            if let Ok(bytes) = std::fs::read(store.path(kind, &slot, Which::Save))
                && SaveFile::decode(&bytes).is_ok()
            {
                push(world, &prefix, kind, &slot, &bytes);
            }
        }
    }
}

fn pull_one(world: &mut World, store: &Store, prefix: &str, name: &str, kind: SaveKind, slot: &str) {
    let (read, at) = {
        let steam = world.resource::<Steam>();
        (steam.backend.cloud_read(name), steam.backend.cloud_timestamp(name).unwrap_or(0))
    };
    let subject = format!("{kind}/{slot}");
    let cloud_bytes = match read {
        Ok(bytes) => bytes,
        Err(e) => {
            journal::emit(world, JournalKind::Warn, format!("steam: the cloud copy of {subject} could not be read: {e}; the save on this machine is used"));
            return;
        }
    };
    let path = store.path(kind, slot, Which::Save);
    let local = std::fs::read(&path).ok().filter(|bytes| SaveFile::decode(bytes).is_ok());
    if let Err(fault) = SaveFile::decode(&cloud_bytes) {
        journal::emit(world, JournalKind::Warn, format!("steam: the cloud copy of {subject} is not used ({fault}); the save on this machine stays"));
        // A good save here repairs the cloud's.
        if let Some(bytes) = local {
            push(world, prefix, kind, slot, &bytes);
        }
        return;
    }
    let local_at = written_at(&path).unwrap_or(0);
    match reconcile(local.as_deref().map(|b| (b, local_at)), (&cloud_bytes, at)) {
        Reconcile::Same => {}
        Reconcile::Pull => match store.write(kind, slot, &cloud_bytes) {
            Ok(()) => {
                let kept = if local.is_some() { format!(", this machine's older one kept as {subject}.save.bak1") } else { String::new() };
                journal::log(world, JournalKind::Local, format!("steam: {subject} taken from Steam Cloud (newer){kept}"));
                world.resource_mut::<Cloud>().pulled.push(subject);
            }
            Err(e) => {
                journal::emit(world, JournalKind::Warn, format!("steam: the cloud copy of {subject} was not taken: {e}"));
            }
        },
        Reconcile::Push => {
            if let Some(bytes) = local {
                push(world, prefix, kind, slot, &bytes);
            }
        }
        Reconcile::KeepBoth => {
            let beside = beside_slot(slot);
            if let Err(e) = store.write(kind, &beside, &cloud_bytes) {
                journal::emit(world, JournalKind::Warn, format!("steam: the cloud copy of {subject} could not be kept beside it: {e}"));
                return;
            }
            journal::emit(
                world,
                JournalKind::Warn,
                format!(
                    "steam: {subject} differs between this machine and Steam Cloud, and neither is clearly newer: this machine's stays, \
                     the cloud's is kept as {kind}/{beside}"
                ),
            );
            world.resource_mut::<Cloud>().kept_both.push(subject);
            // The cloud gets this machine's, so the next start finds them the same.
            if let Some(bytes) = local {
                push(world, prefix, kind, slot, &bytes);
            }
        }
    }
}

fn push(world: &mut World, prefix: &str, kind: SaveKind, slot: &str, bytes: &[u8]) {
    let name = cloud_name(prefix, kind, slot);
    let written = world.resource_mut::<Steam>().backend.cloud_write(&name, bytes);
    match written {
        Ok(()) => world.resource_mut::<Cloud>().pushed.push(format!("{kind}/{slot}")),
        Err(e) => {
            world.resource_mut::<Cloud>().last_error = Some(format!("{name}: {e}"));
            journal::emit(world, JournalKind::Warn, format!("steam: {kind}/{slot} was not copied to Steam Cloud: {e}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_newer_side_wins_and_a_tie_keeps_both() {
        let (a, b): (&[u8], &[u8]) = (b"a", b"b");
        assert_eq!(reconcile(None, (a, 100)), Reconcile::Pull);
        assert_eq!(reconcile(Some((a, 100)), (a, 500)), Reconcile::Same, "the same bytes, whatever the times");
        assert_eq!(reconcile(Some((a, 100)), (b, 103)), Reconcile::Pull);
        assert_eq!(reconcile(Some((a, 103)), (b, 100)), Reconcile::Push);
        assert_eq!(reconcile(Some((a, 100)), (b, 102)), Reconcile::KeepBoth, "{SLACK_SECS} s apart is the same moment");
        assert_eq!(reconcile(Some((a, 102)), (b, 100)), Reconcile::KeepBoth);
    }

    #[test]
    fn cloud_names_are_the_save_folder_kind_and_slot_and_nothing_else_is_one() {
        assert_eq!(cloud_name("saves", SaveKind::World, "slot1"), "saves/world/slot1.save");
        assert_eq!(parse_cloud_name("saves", "saves/world/slot1.save"), Some((SaveKind::World, "slot1".into())));
        assert_eq!(parse_cloud_name("saves", "saves/character/my-run.save"), Some((SaveKind::Character, "my-run".into())));
        for other in ["saves/world/slot1.save.bak1", "saves/world/slot1.save.tmp", "other/world/slot1.save", "saves/maps/a.save", "saves/world/BAD.save", "saves/world/x/y.save"] {
            assert_eq!(parse_cloud_name("saves", other), None, "{other}");
        }
        assert_eq!(parse_cloud_name("saves/test-run", "saves/test-run/world/a.save"), Some((SaveKind::World, "a".into())));
    }

    #[test]
    fn the_cloud_s_copy_beside_a_slot_is_still_a_slot() {
        assert_eq!(beside_slot("slot1"), "slot1-cloud");
        let long = "a".repeat(32);
        let beside = beside_slot(&long);
        assert_eq!(beside.len(), 32);
        assert!(valid_slot(&beside).is_ok() && beside.ends_with("-cloud"));
    }

    #[test]
    fn a_cloud_name_never_holds_a_path_of_this_machine() {
        assert_eq!(cloud_prefix("saves"), "saves");
        assert_eq!(cloud_prefix("./saves/test-run/"), "saves/test-run");
        assert_eq!(cloud_prefix("saves\\test-run"), "saves/test-run", "the same name from Windows");
        for outside in ["", "/home/me/saves", "C:\\Users\\me\\saves", "C:/saves", "../saves", "saves/../../x"] {
            assert_eq!(cloud_prefix(outside), "saves", "{outside}");
        }
    }
}
