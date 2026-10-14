//! Files on disk: slots, atomic writes, rotating backups, recovery.
//!
//! ```text
//! <save_dir>/world/slot1.save           the save
//!                 /slot1.save.bak1      the one before it
//!                 /slot1.save.bak2
//!                 /slot1.save.bak3      the oldest kept
//!                 /slot1.save.tmp       only while writing (a leftover means a crash)
//!                 /slot1.save.damaged   a damaged primary that a recovery replaced
//! <save_dir>/character/slot1.save ...
//! ```
//!
//! **Writing** never leaves a half file under the real name: the bytes go to `.tmp`, are
//! flushed to disk (`sync_all`), the older files shift one place along (`.save → .bak1 →
//! .bak2 → .bak3`, the last one dropped), `.tmp` is renamed to `.save`, and on Unix the
//! folder itself is flushed so the rename survives a power cut. A primary that fails its
//! own checksum is not promoted to a backup (a run of saves over a damaged file would
//! otherwise push every good copy out); it is kept as `.damaged`.
//!
//! **Reading** takes the first copy that passes everything ([`Store::read`]'s `accept`
//! decides what "everything" is: the envelope, then migration and reading each section).
//! When that is not the primary, the primary is rewritten from it and the caller is told
//! what was wrong, to put in the journal.
//!
//! **A newer save is not damage.** A copy that only fails because a newer version of the
//! game wrote it (the envelope is `shards-save/2`, or a section is at a version above this
//! build's: [`Fault::Newer`]) is refused with a clear error and *nothing is touched*: no
//! fallback to an older backup (that would quietly lose the newer progress), no `.damaged`,
//! no rewritten primary. Writing refuses the same way ([`Store::write_guarded`]): while any
//! copy of the slot is newer, this build saves nothing into that slot, so the newer file
//! can never be rotated away or renamed. The error says to use another slot or update the
//! game. (The other way out, keeping the newer file as one more backup, would lose it after
//! three more saves.)

use std::fs::{self, File};
use std::io::{ErrorKind, Write};
use std::path::PathBuf;

use crate::file::{is_intact, Fault, SaveFile};
use crate::section::SaveKind;

/// How many older saves are kept next to the newest.
pub const BACKUPS: u8 = 3;

/// Slot names are file names: `[a-z0-9_-]{1,32}`, never paths.
pub fn valid_slot(slot: &str) -> Result<(), String> {
    let ok = !slot.is_empty() && slot.len() <= 32 && slot.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(format!("slot name `{slot}` is not allowed: use 1 to 32 characters from a-z, 0-9, `_` and `-`, for example `slot1`"))
    }
}

/// One of the files of a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Which {
    /// `<slot>.save`
    Save,
    /// `<slot>.save.bak1` to `.bak3`: 1 is the newest.
    Backup(u8),
    /// `<slot>.save.tmp`
    Tmp,
    /// `<slot>.save.damaged`
    Damaged,
}

impl Which {
    fn suffix(self) -> String {
        match self {
            Which::Save => ".save".to_owned(),
            Which::Backup(n) => format!(".save.bak{n}"),
            Which::Tmp => ".save.tmp".to_owned(),
            Which::Damaged => ".save.damaged".to_owned(),
        }
    }

    /// `primary`, `backup 1`: for journal lines.
    pub fn describe(self) -> String {
        match self {
            Which::Save => "the save".to_owned(),
            Which::Backup(n) => format!("backup {n}"),
            Which::Tmp => "the temporary file".to_owned(),
            Which::Damaged => "the damaged copy".to_owned(),
        }
    }

    /// The copies a read tries, in order.
    pub fn copies() -> impl Iterator<Item = Which> {
        std::iter::once(Which::Save).chain((1..=BACKUPS).map(Which::Backup))
    }
}

/// `slot1.save.bak1`
fn file_name(slot: &str, which: Which) -> String {
    format!("{slot}{}", which.suffix())
}

/// How a message names a copy: `world/slot1`, `world/slot1 (backup 2)`.
fn subject(kind: SaveKind, slot: &str, which: Which) -> String {
    match which {
        Which::Save => format!("{kind}/{slot}"),
        other => format!("{kind}/{slot} ({})", other.describe()),
    }
}

/// What a read had to do to get a save.
#[derive(Clone, Debug, PartialEq)]
pub struct Recovery {
    /// The backup that was good: 1 is the newest.
    pub backup: u8,
    /// What was wrong with the primary: `checksum mismatch (...)`, `missing`.
    pub problem: String,
    /// The tick of the backup.
    pub tick: u64,
    /// Whether the primary was written again from the backup (not on a read-only look).
    pub repaired: bool,
    /// Why rewriting the primary failed, if it did.
    pub repair_error: Option<String>,
}

/// A file read by [`Store::read`].
#[derive(Debug)]
pub struct Read<T> {
    /// What `accept` made of it.
    pub value: T,
    pub file: SaveFile,
    /// Bytes on disk.
    pub size: u64,
    /// `Some` when the primary was not usable.
    pub recovery: Option<Recovery>,
}

/// How one file of a slot is.
#[derive(Clone, Debug, PartialEq)]
pub enum FileState {
    Missing,
    Ok { tick: u64, label: String, size: u64, sections: usize },
    /// Fails a check: the reason.
    Damaged(String),
    /// A newer version of the game wrote it: what is newer. Not damaged, and left alone.
    Newer(String),
}

/// A file of a slot and its health.
#[derive(Clone, Debug, PartialEq)]
pub struct FileInfo {
    pub which: Which,
    /// `slot1.save.bak1`
    pub name: String,
    pub state: FileState,
}

/// A slot: its files and whether a damaged copy was kept.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotInfo {
    pub kind: SaveKind,
    pub slot: String,
    pub files: Vec<FileInfo>,
    /// Size of `.damaged`, when there is one.
    pub damaged_copy: Option<u64>,
}

/// How [`Store::damage`] breaks a file (the `save_damage` tool, for tests of the recovery).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Damage {
    /// One byte in the middle changes.
    Flip,
    /// The second half is cut off.
    Truncate,
    /// The whole file becomes noise of the same length.
    Garbage,
}

/// The folder saves live in.
#[derive(Clone, Debug)]
pub struct Store {
    /// `<save_dir>`.
    pub root: PathBuf,
    /// Flush files and the folder to disk when writing. Always on in a game; off only for
    /// volume tests, where it is the slow part.
    pub sync: bool,
}

impl Store {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into(), sync: true }
    }

    pub fn dir(&self, kind: SaveKind) -> PathBuf {
        self.root.join(kind.as_str())
    }

    /// The path of one file. `slot` must be valid ([`valid_slot`]).
    pub fn path(&self, kind: SaveKind, slot: &str, which: Which) -> PathBuf {
        self.dir(kind).join(file_name(slot, which))
    }

    /// `world/slot1.save.bak1`: how messages name a file.
    pub fn name(&self, kind: SaveKind, slot: &str, which: Which) -> String {
        format!("{kind}/{}", file_name(slot, which))
    }

    /// Whether the slot has any file at all.
    pub fn exists(&self, kind: SaveKind, slot: &str) -> bool {
        Which::copies().any(|w| self.path(kind, slot, w).exists())
    }

    /// Writes `bytes` (an encoded [`SaveFile`]) as the slot's save, shifting the older ones
    /// into the backups. Atomic: whatever happens, the slot keeps a complete save. Refuses,
    /// writing nothing, when a copy of the slot is from a newer format ([`write_guarded`](Self::write_guarded)
    /// also looks at what the game's sections say).
    pub fn write(&self, kind: SaveKind, slot: &str, bytes: &[u8]) -> Result<(), String> {
        self.write_guarded(kind, slot, bytes, &|_| None)
    }

    /// [`write`](Self::write), but first looks at every copy of the slot that is there: one
    /// from a newer format, or one `newer` says is newer (it gets the decoded file and returns
    /// what is newer, `section inventory v7 > v5`) makes the write refuse, changing nothing.
    pub fn write_guarded(&self, kind: SaveKind, slot: &str, bytes: &[u8], newer: &dyn Fn(&SaveFile) -> Option<String>) -> Result<(), String> {
        self.refuse_newer(kind, slot, newer)?;
        let dir = self.dir(kind);
        fs::create_dir_all(&dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
        let (primary, tmp) = (self.path(kind, slot, Which::Save), self.path(kind, slot, Which::Tmp));
        self.write_tmp(&tmp, bytes)?;
        let shifted = self.shift(kind, slot, &primary);
        if let Err(e) = shifted.and_then(|()| fs::rename(&tmp, &primary).map_err(|e| format!("cannot move {} into place: {e}", tmp.display()))) {
            let _ = fs::remove_file(&tmp);
            return Err(e);
        }
        self.sync_dir(&dir)
    }

    /// Errors when a copy of the slot is from a newer version: writing would rotate it into
    /// the backups and, three saves later, lose it.
    fn refuse_newer(&self, kind: SaveKind, slot: &str, newer: &dyn Fn(&SaveFile) -> Option<String>) -> Result<(), String> {
        for which in Which::copies() {
            let Ok(bytes) = fs::read(self.path(kind, slot, which)) else {
                continue;
            };
            let detail = match SaveFile::decode(&bytes) {
                Err(Fault::Newer(detail)) => Some(detail),
                Err(Fault::Damaged(_)) => None,
                Ok(file) => newer(&file),
            };
            if let Some(detail) = detail {
                return Err(format!(
                    "{} was saved by a newer version of the game ({detail}), so this build does not overwrite it and nothing was written. \
                     Save into another slot, or update the game",
                    subject(kind, slot, which)
                ));
            }
        }
        Ok(())
    }

    fn write_tmp(&self, tmp: &PathBuf, bytes: &[u8]) -> Result<(), String> {
        let fail = |what: &str, e: std::io::Error| format!("cannot {what} {}: {e}", tmp.display());
        let mut file = File::create(tmp).map_err(|e| fail("create", e))?;
        file.write_all(bytes).map_err(|e| fail("write", e))?;
        if self.sync {
            file.sync_all().map_err(|e| fail("flush", e))?;
        }
        Ok(())
    }

    /// Makes room for a new primary: the old one becomes `.bak1` (the others move up, the
    /// oldest goes), or `.damaged` when it fails its checksum.
    fn shift(&self, kind: SaveKind, slot: &str, primary: &PathBuf) -> Result<(), String> {
        let old = match fs::read(primary) {
            Ok(old) => old,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(format!("cannot read {}: {e}", primary.display())),
        };
        let rename = |from: PathBuf, to: PathBuf| match fs::rename(&from, &to) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("cannot move {} to {}: {e}", from.display(), to.display())),
        };
        if !is_intact(&old) {
            return rename(primary.clone(), self.path(kind, slot, Which::Damaged));
        }
        let backup = |n: u8| self.path(kind, slot, Which::Backup(n));
        for n in (1..BACKUPS).rev() {
            rename(backup(n), backup(n + 1))?;
        }
        rename(primary.clone(), backup(1))
    }

    #[cfg(unix)]
    fn sync_dir(&self, dir: &PathBuf) -> Result<(), String> {
        if !self.sync {
            return Ok(());
        }
        File::open(dir).and_then(|d| d.sync_all()).map_err(|e| format!("cannot flush {}: {e}", dir.display()))
    }

    #[cfg(not(unix))]
    fn sync_dir(&self, _: &PathBuf) -> Result<(), String> {
        Ok(())
    }

    /// Reads the slot: the primary, else the first good backup. `accept` is everything a
    /// file must pass beyond its own envelope (migration, reading each section) and turns
    /// it into the value the caller wants; an `Err` from it makes the copy damaged.
    /// With `repair`, a primary that was not usable is written again from the backup that
    /// was. A leftover `.tmp` (a crash while writing) is removed.
    ///
    /// A copy that is [`Fault::Newer`] ends the read at once: the error says so and nothing
    /// is changed (see the module docs).
    pub fn read<T>(
        &self,
        kind: SaveKind,
        slot: &str,
        repair: bool,
        mut accept: impl FnMut(&SaveFile) -> Result<T, Fault>,
    ) -> Result<Read<T>, String> {
        let mut newer = false;
        let result = self.read_copies(kind, slot, repair, &mut accept, &mut newer);
        if repair && !newer {
            let _ = fs::remove_file(self.path(kind, slot, Which::Tmp));
        }
        result
    }

    fn read_copies<T>(
        &self,
        kind: SaveKind,
        slot: &str,
        repair: bool,
        accept: &mut dyn FnMut(&SaveFile) -> Result<T, Fault>,
        newer: &mut bool,
    ) -> Result<Read<T>, String> {
        let mut problems: Vec<(Which, String)> = Vec::new();
        let mut found_any = false;
        for which in Which::copies() {
            let path = self.path(kind, slot, which);
            let bytes = match fs::read(&path) {
                Ok(bytes) => bytes,
                Err(e) if e.kind() == ErrorKind::NotFound => {
                    problems.push((which, "missing".to_owned()));
                    continue;
                }
                Err(e) => {
                    found_any = true;
                    problems.push((which, format!("cannot be read: {e}")));
                    continue;
                }
            };
            found_any = true;
            let outcome = SaveFile::decode(&bytes).and_then(|file| {
                if file.kind != kind {
                    return Err(Fault::Damaged(format!("it is a {} save, not a {kind} save", file.kind)));
                }
                accept(&file).map(|value| (file, value))
            });
            let (file, value) = match outcome {
                Ok(ok) => ok,
                Err(fault @ Fault::Newer(_)) => {
                    *newer = true;
                    let before: Vec<String> = problems.iter().map(|(w, p)| format!("{}: {p}", file_name(slot, *w))).collect();
                    let also = if before.is_empty() { String::new() } else { format!(" (the copies before it are unusable: {})", before.join("; ")) };
                    return Err(format!("{} was {fault}; nothing was changed{also}. Update the game to read it, or use another slot", subject(kind, slot, which)));
                }
                Err(Fault::Damaged(problem)) => {
                    problems.push((which, problem));
                    continue;
                }
            };
            let size = bytes.len() as u64;
            let Which::Backup(n) = which else {
                return Ok(Read { value, file, size, recovery: None });
            };
            let problem = problems.first().map(|(_, p)| p.clone()).unwrap_or_default();
            let mut recovery = Recovery { backup: n, problem, tick: file.tick, repaired: false, repair_error: None };
            if repair {
                match self.restore_primary(kind, slot, &bytes) {
                    Ok(()) => recovery.repaired = true,
                    Err(e) => recovery.repair_error = Some(e),
                }
            }
            return Ok(Read { value, file, size, recovery: Some(recovery) });
        }
        if !found_any {
            return Err(format!(
                "there is no {kind} save in slot `{slot}` (looked for {}); `save_list` shows the slots there are",
                self.path(kind, slot, Which::Save).display()
            ));
        }
        let each: Vec<String> = problems.iter().map(|(which, problem)| format!("{}: {problem}", file_name(slot, *which))).collect();
        Err(format!("cannot load {kind}/{slot}, every copy is unusable, nothing was applied: {}", each.join("; ")))
    }

    /// Puts `bytes` (a good backup) back as the primary, keeping a damaged primary as
    /// `.damaged`. The backups stay as they are.
    fn restore_primary(&self, kind: SaveKind, slot: &str, bytes: &[u8]) -> Result<(), String> {
        let (primary, tmp) = (self.path(kind, slot, Which::Save), self.path(kind, slot, Which::Tmp));
        self.write_tmp(&tmp, bytes)?;
        if primary.exists() {
            let kept = fs::rename(&primary, self.path(kind, slot, Which::Damaged));
            if let Err(e) = kept {
                let _ = fs::remove_file(&tmp);
                return Err(format!("cannot keep the damaged {}: {e}", primary.display()));
            }
        }
        if let Err(e) = fs::rename(&tmp, &primary) {
            let _ = fs::remove_file(&tmp);
            return Err(format!("cannot move {} into place: {e}", tmp.display()));
        }
        self.sync_dir(&self.dir(kind))
    }

    /// The slots of a kind, sorted: a slot is a name with any of its files.
    pub fn slots(&self, kind: SaveKind) -> Vec<String> {
        let Ok(entries) = fs::read_dir(self.dir(kind)) else {
            return Vec::new();
        };
        let mut slots: Vec<String> = entries
            .filter_map(Result::ok)
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                Which::copies().find_map(|w| name.strip_suffix(w.suffix().as_str()).filter(|slot| valid_slot(slot).is_ok()).map(str::to_owned))
            })
            .collect();
        slots.sort();
        slots.dedup();
        slots
    }

    /// How each copy of a slot is, without changing anything.
    pub fn inspect(&self, kind: SaveKind, slot: &str) -> SlotInfo {
        self.inspect_with(kind, slot, &|_| None)
    }

    /// [`inspect`](Self::inspect), where `newer` (see [`write_guarded`](Self::write_guarded))
    /// also says which copies are from a newer version of the game.
    pub fn inspect_with(&self, kind: SaveKind, slot: &str, newer: &dyn Fn(&SaveFile) -> Option<String>) -> SlotInfo {
        let files = Which::copies()
            .map(|which| {
                let name = file_name(slot, which);
                let state = match fs::read(self.path(kind, slot, which)) {
                    Err(e) if e.kind() == ErrorKind::NotFound => FileState::Missing,
                    Err(e) => FileState::Damaged(format!("cannot be read: {e}")),
                    Ok(bytes) => match SaveFile::decode(&bytes) {
                        Ok(f) if f.kind == kind => match newer(&f) {
                            Some(detail) => FileState::Newer(detail),
                            None => FileState::Ok { tick: f.tick, label: f.label, size: bytes.len() as u64, sections: f.sections.len() },
                        },
                        Ok(f) => FileState::Damaged(format!("it is a {} save, not a {kind} save", f.kind)),
                        Err(Fault::Newer(detail)) => FileState::Newer(detail),
                        Err(Fault::Damaged(e)) => FileState::Damaged(e),
                    },
                };
                FileInfo { which, name, state }
            })
            .collect();
        let damaged_copy = fs::metadata(self.path(kind, slot, Which::Damaged)).ok().map(|m| m.len());
        SlotInfo { kind, slot: slot.to_owned(), files, damaged_copy }
    }

    /// Breaks one existing file of a slot on purpose, so the recovery can be tried
    /// (`save_damage`). Deterministic. Returns what was done.
    pub fn damage(&self, kind: SaveKind, slot: &str, which: Which, how: Damage) -> Result<String, String> {
        let path = self.path(kind, slot, which);
        let name = self.name(kind, slot, which);
        let mut bytes = fs::read(&path).map_err(|e| match e.kind() {
            ErrorKind::NotFound => format!("{name} does not exist, so there is nothing to damage; `save_list` shows the files"),
            _ => format!("cannot read {name}: {e}"),
        })?;
        let what = match how {
            Damage::Flip => {
                let header = bytes.iter().position(|b| *b == b'\n').map_or(0, |n| n + 1);
                let at = (header + (bytes.len() - header) / 2).min(bytes.len().saturating_sub(1));
                if let Some(b) = bytes.get_mut(at) {
                    *b ^= 0x01;
                }
                format!("flipped one bit of byte {at}")
            }
            Damage::Truncate => {
                bytes.truncate(bytes.len() / 2);
                format!("cut it to {} bytes", bytes.len())
            }
            Damage::Garbage => {
                let mut x: u64 = 0x2545_F491_4F6C_DD1D ^ bytes.len() as u64;
                for b in &mut bytes {
                    x ^= x << 13;
                    x ^= x >> 7;
                    x ^= x << 17;
                    *b = (x >> 24) as u8;
                }
                format!("overwrote it with {} bytes of noise", bytes.len())
            }
        };
        fs::write(&path, &bytes).map_err(|e| format!("cannot write {name}: {e}"))?;
        Ok(format!("{name}: {what}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file::{GameInfo, SectionData, SectionSet};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shards_save_store_{name}_{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn file(tick: u64) -> SaveFile {
        let mut sections = BTreeMap::new();
        sections.insert("s".to_owned(), SectionData { version: 1, data: json!({"n": tick}) });
        SaveFile {
            kind: SaveKind::World,
            slot: "a".into(),
            label: format!("save {tick}"),
            game: GameInfo::default(),
            engine: "0".into(),
            tick,
            sections: SectionSet(sections),
        }
    }

    fn ticks(store: &Store) -> Vec<Option<u64>> {
        store
            .inspect(SaveKind::World, "a")
            .files
            .iter()
            .map(|f| match f.state {
                FileState::Ok { tick, .. } => Some(tick),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn slot_names_are_file_names() {
        for good in ["a", "slot1", "my-save_2", &"x".repeat(32)] {
            assert!(valid_slot(good).is_ok(), "{good}");
        }
        for bad in ["", "Slot", "a b", "../x", "a/b", "a.b", "é", &"x".repeat(33), "a\0"] {
            assert!(valid_slot(bad).unwrap_err().contains("is not allowed"), "{bad:?}");
        }
    }

    #[test]
    fn three_backups_rotate_and_no_more() {
        let store = Store::new(dir("rotate"));
        for tick in 1..=6 {
            store.write(SaveKind::World, "a", &file(tick).encode()).unwrap();
        }
        assert_eq!(ticks(&store), [Some(6), Some(5), Some(4), Some(3)]);
        let names: Vec<String> = fs::read_dir(store.dir(SaveKind::World)).unwrap().map(|e| e.unwrap().file_name().into_string().unwrap()).collect();
        assert_eq!(names.len(), 4, "{names:?}");
        assert!(!store.path(SaveKind::World, "a", Which::Tmp).exists());
        assert_eq!(store.slots(SaveKind::World), ["a"]);
        let _ = fs::remove_dir_all(&store.root);
    }

    #[test]
    fn the_first_good_copy_is_read_and_the_primary_is_repaired() {
        let store = Store::new(dir("recover"));
        for tick in 1..=3 {
            store.write(SaveKind::World, "a", &file(tick).encode()).unwrap();
        }
        let read = |repair| store.read(SaveKind::World, "a", repair, |f| Ok(f.tick));
        assert_eq!(read(true).unwrap().recovery, None);

        store.damage(SaveKind::World, "a", Which::Save, Damage::Flip).unwrap();
        let looked = read(false).unwrap();
        let recovery = looked.recovery.unwrap();
        assert_eq!((looked.value, recovery.backup, recovery.repaired), (2, 1, false));
        assert!(recovery.problem.starts_with("checksum mismatch"), "{}", recovery.problem);
        assert_eq!(ticks(&store)[0], None, "a read-only look repairs nothing");

        let repaired = read(true).unwrap().recovery.unwrap();
        assert!(repaired.repaired && repaired.repair_error.is_none());
        assert_eq!(ticks(&store), [Some(2), Some(2), Some(1), None]);
        assert!(store.inspect(SaveKind::World, "a").damaged_copy.is_some(), "the damaged file is kept");
        assert_eq!(read(true).unwrap().recovery, None);

        // Primary and bak1 gone: bak2.
        store.damage(SaveKind::World, "a", Which::Save, Damage::Garbage).unwrap();
        store.damage(SaveKind::World, "a", Which::Backup(1), Damage::Truncate).unwrap();
        let r = read(true).unwrap();
        assert_eq!((r.value, r.recovery.unwrap().backup), (1, 2));

        // Everything damaged: every problem is listed, and nothing is invented.
        for which in Which::copies() {
            let _ = store.damage(SaveKind::World, "a", which, Damage::Garbage);
        }
        let err = read(true).unwrap_err();
        for part in ["cannot load world/a", "nothing was applied", "a.save: no header", "a.save.bak1:", "a.save.bak2:", "a.save.bak3: missing"] {
            assert!(err.contains(part), "{part}: {err}");
        }
        let _ = fs::remove_dir_all(&store.root);
    }

    #[test]
    fn a_copy_that_accept_refuses_counts_as_damaged() {
        let store = Store::new(dir("accept"));
        store.write(SaveKind::World, "a", &file(1).encode()).unwrap();
        store.write(SaveKind::World, "a", &file(2).encode()).unwrap();
        let r = store.read(SaveKind::World, "a", true, |f| if f.tick == 2 { Err(Fault::Damaged("section s does not read".to_owned())) } else { Ok(f.tick) }).unwrap();
        assert_eq!(r.value, 1);
        assert_eq!(r.recovery.unwrap().problem, "section s does not read");
        let _ = fs::remove_dir_all(&store.root);
    }

    /// The bytes of every file in the slot's folder, by name.
    fn snapshot(store: &Store) -> BTreeMap<String, Vec<u8>> {
        fs::read_dir(store.dir(SaveKind::World)).unwrap().map(|e| e.unwrap()).map(|e| (e.file_name().into_string().unwrap(), fs::read(e.path()).unwrap())).collect()
    }

    fn newer_than(file: &SaveFile) -> Option<String> {
        (file.tick >= 100).then(|| format!("section s v{} > v1", file.tick))
    }

    #[test]
    fn a_newer_save_is_refused_and_nothing_is_touched() {
        let store = Store::new(dir("newer"));
        store.write(SaveKind::World, "a", &file(1).encode()).unwrap();
        store.write(SaveKind::World, "a", &file(2).encode()).unwrap();
        // The primary is from a newer build (here: tick 100 stands for section version 100).
        store.write(SaveKind::World, "a", &file(100).encode()).unwrap();
        let before = snapshot(&store);
        let read = |repair| {
            store.read(SaveKind::World, "a", repair, |f| match newer_than(f) {
                Some(detail) => Err(Fault::Newer(detail)),
                None => Ok(f.tick),
            })
        };
        for repair in [false, true] {
            let err = read(repair).unwrap_err();
            assert!(
                err.starts_with("world/a was saved by a newer version of the game (section s v100 > v1); nothing was changed"),
                "{err}"
            );
        }
        assert_eq!(snapshot(&store), before, "no fallback, no .damaged, no rewrite");
        // `inspect` calls it newer, not damaged.
        let info = store.inspect(SaveKind::World, "a");
        assert!(info.files.iter().all(|f| !matches!(f.state, FileState::Damaged(_))));

        // A newer format reads as newer too, and so does one that is only a header: nothing else is checked.
        fs::write(store.path(SaveKind::World, "a", Which::Save), b"shards-save/2 crc32=00000000 len=0\n").unwrap();
        let before = snapshot(&store);
        let err = store.read(SaveKind::World, "a", true, |f| Ok(f.tick)).unwrap_err();
        assert!(err.starts_with("world/a was saved by a newer version of the game (save format shards-save/2 > shards-save/1); nothing was changed"), "{err}");
        assert_eq!(snapshot(&store), before);
        assert_eq!(store.inspect(SaveKind::World, "a").files[0].state, FileState::Newer("save format shards-save/2 > shards-save/1".to_owned()));
        let _ = fs::remove_dir_all(&store.root);
    }

    #[test]
    fn a_newer_backup_behind_a_damaged_primary_is_not_skipped() {
        let store = Store::new(dir("newer_backup"));
        for tick in [1, 100, 2] {
            store.write(SaveKind::World, "a", &file(tick).encode()).unwrap();
        }
        // Primary 2, bak1 100 (newer), bak2 1. The primary is damaged: the newest good copy
        // is the newer one, and the older bak2 would lose its progress, so the read stops.
        store.damage(SaveKind::World, "a", Which::Save, Damage::Flip).unwrap();
        let before = snapshot(&store);
        let err = store
            .read(SaveKind::World, "a", true, |f| match newer_than(f) {
                Some(detail) => Err(Fault::Newer(detail)),
                None => Ok(f.tick),
            })
            .unwrap_err();
        assert!(err.starts_with("world/a (backup 1) was saved by a newer version of the game (section s v100 > v1); nothing was changed"), "{err}");
        assert!(err.contains("the copies before it are unusable: a.save: checksum mismatch"), "{err}");
        assert_eq!(snapshot(&store), before);
        let _ = fs::remove_dir_all(&store.root);
    }

    #[test]
    fn writing_never_rotates_a_newer_save_away() {
        let store = Store::new(dir("newer_write"));
        store.write(SaveKind::World, "a", &file(1).encode()).unwrap();
        store.write(SaveKind::World, "a", &file(100).encode()).unwrap();
        let before = snapshot(&store);
        // Section versions are the game's to judge: the guard says the primary is newer.
        let err = store.write_guarded(SaveKind::World, "a", &file(3).encode(), &newer_than).unwrap_err();
        assert!(err.starts_with("world/a was saved by a newer version of the game (section s v100 > v1), so this build does not overwrite it"), "{err}");
        assert!(err.contains("Save into another slot"), "{err}");
        assert_eq!(snapshot(&store), before, "not even a .tmp is left");

        // So does a newer backup behind a primary that is damaged: the shift would push it out.
        store.write_guarded(SaveKind::World, "b", &file(100).encode(), &|_| None).unwrap();
        store.write_guarded(SaveKind::World, "b", &file(2).encode(), &|_| None).unwrap();
        fs::write(store.path(SaveKind::World, "b", Which::Save), b"damaged").unwrap();
        let err = store.write_guarded(SaveKind::World, "b", &file(3).encode(), &newer_than).unwrap_err();
        assert!(err.starts_with("world/b (backup 1) was saved by a newer version"), "{err}");

        // A newer format is refused by plain `write` too, whatever the sections say.
        fs::write(store.path(SaveKind::World, "c", Which::Save), b"shards-save/7 whatever\n").unwrap();
        let err = store.write(SaveKind::World, "c", &file(1).encode()).unwrap_err();
        assert!(err.contains("(save format shards-save/7 > shards-save/1)"), "{err}");
        assert_eq!(fs::read(store.path(SaveKind::World, "c", Which::Save)).unwrap(), b"shards-save/7 whatever\n");
        assert!(!store.path(SaveKind::World, "c", Which::Damaged).exists());

        // A slot whose copies are only damaged is written as before.
        fs::write(store.path(SaveKind::World, "d", Which::Save), b"junk").unwrap();
        store.write_guarded(SaveKind::World, "d", &file(4).encode(), &newer_than).unwrap();
        let _ = fs::remove_dir_all(&store.root);
    }

    #[test]
    fn a_leftover_tmp_or_a_missing_primary_is_survived() {
        let store = Store::new(dir("crash"));
        store.write(SaveKind::World, "a", &file(1).encode()).unwrap();
        store.write(SaveKind::World, "a", &file(2).encode()).unwrap();
        // Crashed while writing: the new bytes sit in .tmp, the primary is untouched.
        let tmp = store.path(SaveKind::World, "a", Which::Tmp);
        fs::write(&tmp, b"half a fi").unwrap();
        let r = store.read(SaveKind::World, "a", true, |f| Ok(f.tick)).unwrap();
        assert_eq!((r.value, r.recovery), (2, None));
        assert!(!tmp.exists(), "the leftover is removed");
        // Crashed between the shift and the rename: no primary, the previous save is bak1.
        let p = |w| store.path(SaveKind::World, "a", w);
        fs::rename(p(Which::Backup(1)), p(Which::Backup(2))).unwrap();
        fs::rename(p(Which::Save), p(Which::Backup(1))).unwrap();
        fs::write(p(Which::Tmp), b"the new save, complete but never renamed").unwrap();
        let r = store.read(SaveKind::World, "a", true, |f| Ok(f.tick)).unwrap();
        let recovery = r.recovery.unwrap();
        assert_eq!((r.value, recovery.backup, recovery.problem.as_str(), recovery.repaired), (2, 1, "missing", true));
        assert!(!p(Which::Tmp).exists() && p(Which::Save).exists());
        // No file at all is not damage: there is simply no save.
        let err = store.read(SaveKind::World, "zz", true, |f| Ok(f.tick)).unwrap_err();
        assert!(err.starts_with("there is no world save in slot `zz`"), "{err}");
        let _ = fs::remove_dir_all(&store.root);
    }

    #[test]
    fn a_damaged_primary_is_not_made_a_backup() {
        let store = Store::new(dir("nopromote"));
        store.write(SaveKind::World, "a", &file(1).encode()).unwrap();
        store.write(SaveKind::World, "a", &file(2).encode()).unwrap();
        store.damage(SaveKind::World, "a", Which::Save, Damage::Flip).unwrap();
        store.write(SaveKind::World, "a", &file(3).encode()).unwrap();
        assert_eq!(ticks(&store), [Some(3), Some(1), None, None], "the good copy stays, the damaged one is set aside");
        assert!(store.inspect(SaveKind::World, "a").damaged_copy.is_some());
        let _ = fs::remove_dir_all(&store.root);
    }

    #[test]
    fn a_save_of_the_other_kind_is_not_loaded() {
        let store = Store::new(dir("kind"));
        let mut other = file(1);
        other.kind = SaveKind::Character;
        fs::create_dir_all(store.dir(SaveKind::World)).unwrap();
        fs::write(store.path(SaveKind::World, "a", Which::Save), other.encode()).unwrap();
        let err = store.read(SaveKind::World, "a", false, |f| Ok(f.tick)).unwrap_err();
        assert!(err.contains("it is a character save, not a world save"), "{err}");
        let _ = fs::remove_dir_all(&store.root);
    }

    #[test]
    fn damage_needs_a_file() {
        let store = Store::new(dir("nodamage"));
        let err = store.damage(SaveKind::World, "a", Which::Save, Damage::Flip).unwrap_err();
        assert!(err.contains("world/a.save does not exist"), "{err}");
        let _ = fs::remove_dir_all(&store.root);
    }
}
