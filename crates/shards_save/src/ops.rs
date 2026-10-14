//! Saving, loading and checking: sections in, files out, and back.
//!
//! The functions here take `&mut World` and do the whole thing at once (capture, write,
//! journal); tools and autosave are thin callers. Pieces of them are public for co-op, which
//! captures a remote player's character on the host and applies one on join:
//! [`capture_sections`], [`apply_sections`].

use std::collections::BTreeMap;
use std::path::PathBuf;

use ai_core::journal::{self, JournalKind};
use ai_core::sim;
use bevy::prelude::*;
use serde_json::Value;
use shards_core::{EngineConfig, PlayerId};

use crate::canon;
use crate::file::{GameInfo, SaveFile, SectionData, SectionSet};
use crate::section::{plural, SaveKind, SaveRegistry};
use crate::store::{valid_slot, Recovery, Store, Which};

/// Where saves go and what they say about the game.
#[derive(Resource, Clone, Debug)]
pub struct SaveConfig {
    /// The folder `world/` and `character/` live in. Relative paths are relative to the
    /// game's folder, like `replays/`.
    pub dir: PathBuf,
    /// Written into every file next to the title (the `EngineConfig`'s); a game sets it
    /// from its own `CARGO_PKG_VERSION`.
    pub game_version: String,
    /// Flush files to disk when writing. Leave it on.
    pub sync: bool,
}

impl Default for SaveConfig {
    fn default() -> Self {
        Self { dir: PathBuf::from("saves"), game_version: "0.0.0".to_owned(), sync: true }
    }
}

impl SaveConfig {
    pub fn store(&self) -> Store {
        Store { root: self.dir.clone(), sync: self.sync }
    }
}

/// The slot each kind is in now: what [`save_now`] and autosaves write, and what a load or
/// an explicit save last named. Slots are the game's to present (`slot1`, `ford-camp`, ...).
#[derive(Resource, Clone, Debug, PartialEq, Eq)]
pub struct SaveSlots {
    pub world: String,
    pub character: String,
}

impl Default for SaveSlots {
    fn default() -> Self {
        Self { world: "slot1".to_owned(), character: "slot1".to_owned() }
    }
}

impl SaveSlots {
    pub fn get(&self, kind: SaveKind) -> &str {
        match kind {
            SaveKind::World => &self.world,
            SaveKind::Character => &self.character,
        }
    }

    pub fn set(&mut self, kind: SaveKind, slot: &str) {
        match kind {
            SaveKind::World => self.world = slot.to_owned(),
            SaveKind::Character => self.character = slot.to_owned(),
        }
    }
}

/// What to save or load.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SaveWhat {
    /// The world's sections.
    World,
    /// A character's sections: the body given, or `None` for the player's (the body with
    /// `PlayerId(0)`).
    Character(Option<Entity>),
    /// The world, then player 0's character.
    All,
}

impl SaveWhat {
    /// The kinds this covers, with their bodies, in the order a load applies them.
    pub fn parts(self) -> Vec<(SaveKind, Option<Entity>)> {
        match self {
            SaveWhat::World => vec![(SaveKind::World, None)],
            SaveWhat::Character(body) => vec![(SaveKind::Character, body)],
            SaveWhat::All => vec![(SaveKind::World, None), (SaveKind::Character, None)],
        }
    }

    pub fn includes(self, kind: SaveKind) -> bool {
        self.parts().iter().any(|(k, _)| *k == kind)
    }
}

/// The player's body: the entity with `PlayerId(0)`, else the one marked `AiPlayer`.
pub fn default_body(world: &World) -> Option<Entity> {
    let host = world.try_query::<(Entity, &PlayerId)>().and_then(|mut q| q.iter(world).find(|(_, id)| id.0 == 0).map(|(e, _)| e));
    host.or_else(|| ai_core::observe::player_entity(world))
}

/// The body a character save of `explicit` is about: that one if it exists, else the
/// player's.
pub fn resolve_body(world: &World, explicit: Option<Entity>) -> Option<Entity> {
    explicit.filter(|e| world.get_entity(*e).is_ok()).or_else(|| default_body(world))
}

fn journal_warn(world: &mut World, text: String) {
    journal::emit(world, JournalKind::Warn, text);
}

/// `812 B`, `14 KB`.
pub fn size_text(bytes: u64) -> String {
    if bytes < 1024 { format!("{bytes} B") } else { format!("{} KB", bytes.div_ceil(1024)) }
}

// --- sections <-> files -----------------------------------------------------------------

/// Reads every section of `kind` from the world. A character save needs `body`.
pub fn capture_sections(world: &World, kind: SaveKind, body: Option<Entity>) -> Result<SectionSet, String> {
    let registry = world.get_resource::<SaveRegistry>().ok_or("the save module is not installed (add SavePlugin)")?;
    if kind == SaveKind::Character && body.is_none() {
        return Err("there is no player body to take the character from (no entity with PlayerId(0) or AiPlayer)".to_owned());
    }
    let mut sections = BTreeMap::new();
    for entry in registry.of_kind(kind) {
        let data = entry.capture(world, body).map_err(|e| format!("section {}: {e}", entry.name))?;
        sections.insert(entry.name.clone(), SectionData { version: entry.version, data });
    }
    Ok(SectionSet(sections))
}

/// A set of sections made current: every section at this build's version and reading as its
/// struct, plus what was left out.
#[derive(Clone, Debug, Default)]
pub struct Prepared {
    /// Current-version data by section name.
    pub sections: BTreeMap<String, Value>,
    /// Sections the set has that this game does not register (skipped).
    pub unknown: Vec<String>,
}

/// Migrates `set` to the current versions and checks each section reads. Refuses a section
/// saved by a newer version (`section inventory was saved by a newer version: 7 > 5`), a
/// missing migration and a failed one.
pub fn prepare(registry: &SaveRegistry, kind: SaveKind, set: &SectionSet) -> Result<Prepared, String> {
    let mut prepared = Prepared::default();
    for (name, saved) in &set.0 {
        let Some(entry) = registry.get(name).filter(|e| e.kind == kind) else {
            prepared.unknown.push(name.clone());
            continue;
        };
        if saved.version > entry.version {
            return Err(format!("section {name} was saved by a newer version: {} > {}", saved.version, entry.version));
        }
        if saved.version == 0 {
            return Err(format!("section {name} has version 0; versions start at 1"));
        }
        let (mut data, mut version) = (saved.data.clone(), saved.version);
        while version < entry.version {
            let Some(migrate) = registry.migration(name, version) else {
                return Err(format!("section {name}: no migration from version {version} (this build is at {})", entry.version));
            };
            data = migrate(data).map_err(|e| format!("section {name}: migration from version {version} failed: {e}"))?;
            version += 1;
        }
        entry.check(&data).map_err(|e| format!("section {name} does not read: {e}"))?;
        prepared.sections.insert(name.clone(), data);
    }
    Ok(prepared)
}

/// What [`apply_sections`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Applied {
    /// Sections put into the world, in order.
    pub applied: Vec<String>,
    /// Sections the set lacked, whose `missing` ran.
    pub missing: Vec<String>,
    /// Sections the set had that this game does not register.
    pub unknown: Vec<String>,
}

/// A failed apply: how far it got.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApplyError {
    pub applied: Vec<String>,
    pub message: String,
}

fn apply_prepared(world: &mut World, kind: SaveKind, body: Option<Entity>, prepared: Prepared) -> Result<Applied, ApplyError> {
    let entries: Vec<_> = world.resource::<SaveRegistry>().of_kind(kind).cloned().collect();
    let mut done = Applied { unknown: prepared.unknown, ..Applied::default() };
    let mut sections = prepared.sections;
    for entry in entries {
        let result = match sections.remove(&entry.name) {
            Some(data) => entry.apply(world, body, data).inspect(|()| done.applied.push(entry.name.clone())),
            None => entry.missing(world, body).inspect(|()| done.missing.push(entry.name.clone())),
        };
        if let Err(message) = result {
            return Err(ApplyError { applied: done.applied, message: format!("section {}: {message}", entry.name) });
        }
    }
    Ok(done)
}

/// Puts `set` into the world: migrates it, then applies each section of `kind` in
/// registration order (`missing` for the ones it lacks). Sections that are not registered
/// are skipped with a warning in the journal. Nothing is applied if the set does not
/// prepare; a section that fails when applied stops the rest.
pub fn apply_sections(world: &mut World, kind: SaveKind, body: Option<Entity>, set: &SectionSet) -> Result<Applied, ApplyError> {
    if kind == SaveKind::Character && body.is_none() {
        return Err(ApplyError { applied: Vec::new(), message: "there is no player body to put the character into".to_owned() });
    }
    let prepared = {
        let registry = world.get_resource::<SaveRegistry>().ok_or_else(|| ApplyError {
            applied: Vec::new(),
            message: "the save module is not installed (add SavePlugin)".to_owned(),
        })?;
        prepare(registry, kind, set).map_err(|message| ApplyError { applied: Vec::new(), message })?
    };
    for name in &prepared.unknown {
        journal_warn(world, format!("save: unknown {kind} section {name} skipped (this game does not save it)"));
    }
    apply_prepared(world, kind, body, prepared)
}

// --- writing -----------------------------------------------------------------------------

/// What one file's write did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SaveReport {
    pub kind: SaveKind,
    pub slot: String,
    pub tick: u64,
    /// Size of the file.
    pub bytes: usize,
    pub sections: usize,
    pub reason: String,
}

impl std::fmt::Display for SaveReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{} saved ({}, {}, tick {})", self.kind, self.slot, self.reason, size_text(self.bytes as u64), self.tick)
    }
}

fn game_info(world: &World) -> GameInfo {
    let title = world.get_resource::<EngineConfig>().map(|c| c.title.clone()).unwrap_or_default();
    let version = world.get_resource::<SaveConfig>().map(|c| c.game_version.clone()).unwrap_or_default();
    GameInfo { title, version }
}

fn store(world: &World) -> Result<Store, String> {
    world.get_resource::<SaveConfig>().map(SaveConfig::store).ok_or_else(|| "the save module is not installed (add SavePlugin)".to_owned())
}

/// Captures and writes one kind into `slot`: `save: world/slot1 saved (camp, 14 KB, tick
/// 51230)`. The state is read as it is now; nothing in the simulation changes.
pub fn save_part(world: &mut World, kind: SaveKind, body: Option<Entity>, slot: &str, label: &str, reason: &str) -> Result<SaveReport, String> {
    valid_slot(slot)?;
    let body = if kind == SaveKind::Character { resolve_body(world, body) } else { None };
    let sections = capture_sections(world, kind, body).inspect_err(|e| {
        journal::emit(world, JournalKind::Error, format!("save: {kind}/{slot} not saved: {e}"));
    })?;
    let tick = sim::tick(world);
    write_sections(world, kind, slot, label, reason, tick, sections)
}

/// Writes `sections` as the save of `kind/slot`, atomically, shifting the older ones into
/// the backups. For a character captured elsewhere (co-op: the host captures a remote player's
/// body and the player's machine writes the file): `tick` is the one it was captured at.
pub fn write_sections(world: &mut World, kind: SaveKind, slot: &str, label: &str, reason: &str, tick: u64, sections: SectionSet) -> Result<SaveReport, String> {
    valid_slot(slot)?;
    let store = store(world)?;
    let file = SaveFile {
        kind,
        slot: slot.to_owned(),
        label: if label.is_empty() { reason.to_owned() } else { label.to_owned() },
        game: game_info(world),
        engine: shards_core::VERSION.to_owned(),
        tick,
        sections,
    };
    let bytes = file.encode();
    store.write(kind, slot, &bytes).inspect_err(|e| {
        journal::emit(world, JournalKind::Error, format!("save: {kind}/{slot} not saved: {e}"));
    })?;
    let report = SaveReport { kind, slot: slot.to_owned(), tick, bytes: bytes.len(), sections: file.sections.len(), reason: reason.to_owned() };
    journal::log(world, JournalKind::Sim, format!("save: {report}"));
    Ok(report)
}

/// Reads `kind/slot` as a file: the primary, or the newest good backup (rewriting the
/// primary and saying so in the journal). Its sections are as written, at their saved
/// versions; [`apply_sections`] migrates them. For a client that sends its character to the
/// host in co-op.
pub fn read_file(world: &mut World, kind: SaveKind, slot: &str) -> Result<SaveFile, String> {
    read_slot(world, kind, slot, true).map(|read| read.file)
}

/// Saves `what` into `slot` (the same name for each kind), and makes it the current slot of
/// those kinds. Every kind is tried; if some fail the error lists them and what was saved.
pub fn save_slot(world: &mut World, what: SaveWhat, slot: &str, label: &str, reason: &str) -> Result<Vec<SaveReport>, String> {
    let slots = SaveSlots { world: slot.to_owned(), character: slot.to_owned() };
    save_parts(world, &what.parts(), &slots, label, reason)
}

/// Saves `what` into the current slots ([`SaveSlots`]) now, for rewards and discoveries
/// the game decides on in a tool, an exclusive system or after a tick. From an ordinary
/// system use [`SaveRequests`](crate::SaveRequests), which writes at the end of the tick and
/// is debounced. `reason` is journaled (`camp`, `found the ford`).
pub fn save_now(world: &mut World, what: SaveWhat, reason: &str) -> Result<Vec<SaveReport>, String> {
    let slots = world.get_resource::<SaveSlots>().cloned().unwrap_or_default();
    save_parts(world, &what.parts(), &slots, "", reason)
}

/// Saves each of `parts` into its kind's slot of `slots`; the slots that were written become
/// the current ones. Every part is tried: if some fail, the error lists them and what was
/// saved.
pub fn save_parts(
    world: &mut World,
    parts: &[(SaveKind, Option<Entity>)],
    slots: &SaveSlots,
    label: &str,
    reason: &str,
) -> Result<Vec<SaveReport>, String> {
    let (mut reports, mut failures) = (Vec::new(), Vec::new());
    for &(kind, body) in parts {
        match save_part(world, kind, body, slots.get(kind), label, reason) {
            Ok(report) => {
                if let Some(mut current) = world.get_resource_mut::<SaveSlots>() {
                    current.set(kind, &report.slot);
                }
                reports.push(report);
            }
            Err(e) => failures.push(format!("{kind}: {e}")),
        }
    }
    if failures.is_empty() {
        return Ok(reports);
    }
    let saved: Vec<String> = reports.iter().map(ToString::to_string).collect();
    let also = if saved.is_empty() { String::new() } else { format!(" (saved: {})", saved.join(", ")) };
    Err(format!("{}{also}", failures.join("; ")))
}

// --- reading -----------------------------------------------------------------------------

/// A slot read and made current, before anything is applied.
struct Read {
    kind: SaveKind,
    slot: String,
    file: SaveFile,
    prepared: Prepared,
    recovery: Option<Recovery>,
}

/// Reads `kind/slot`: the primary or the first good backup, every section migrated and
/// read. `repair` rewrites a primary that was not usable.
fn read_slot(world: &mut World, kind: SaveKind, slot: &str, repair: bool) -> Result<Read, String> {
    valid_slot(slot)?;
    let store = store(world)?;
    let registry = world.get_resource::<SaveRegistry>().cloned().ok_or("the save module is not installed (add SavePlugin)")?;
    let read = store.read(kind, slot, repair, |file| prepare(&registry, kind, &file.sections))?;
    // A look (`save_check`) says nothing; a load tells what it had to do.
    if repair && let Some(r) = &read.recovery {
        let name = store.name(kind, slot, Which::Save);
        let fixed = r.repair_error.as_ref().map(|e| format!("; the file could not be rewritten: {e}")).unwrap_or_default();
        journal_warn(world, format!("save: {name} is {}; restored from backup {} (tick {}){fixed}", damage_text(&r.problem), r.backup, r.tick));
    }
    Ok(Read { kind, slot: slot.to_owned(), file: read.file, prepared: read.value, recovery: read.recovery })
}

/// `damaged (checksum mismatch)` or `missing`.
fn damage_text(problem: &str) -> String {
    if problem == "missing" { "missing".to_owned() } else { format!("damaged ({problem})") }
}

/// What a load did for one kind.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadReport {
    pub kind: SaveKind,
    pub slot: String,
    /// The tick the save was made at.
    pub tick: u64,
    pub label: String,
    pub applied: usize,
    pub missing: usize,
    pub unknown: Vec<String>,
    /// `Some` when the primary was damaged or missing and a backup was used.
    pub recovery: Option<Recovery>,
}

impl std::fmt::Display for LoadReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{} loaded (saved at tick {}, {}", self.kind, self.slot, self.tick, plural(self.applied, "section", "sections"))?;
        if self.missing > 0 {
            write!(f, ", {} missing from the file", self.missing)?;
        }
        if let Some(r) = &self.recovery {
            write!(f, ", from backup {}", r.backup)?;
        }
        f.write_str(")")
    }
}

/// Loads `what` from `slot` into the world (a world built afresh, the usual way). Every
/// file is read and checked before anything is applied; a slot whose copies are all damaged
/// changes nothing and the error names each copy's problem. Then the sections apply in
/// registration order, and the recording restarts from the loaded state, so a replay saved
/// afterwards verifies. A section that fails when applied leaves the world partly loaded
/// and says how far it got.
pub fn load_slot(world: &mut World, what: SaveWhat, slot: &str) -> Result<Vec<LoadReport>, String> {
    let slots = SaveSlots { world: slot.to_owned(), character: slot.to_owned() };
    load_parts(world, &what.parts(), &slots)
}

/// [`load_slot`] for the parts given, each from its kind's slot of `slots`; a character part
/// names its body.
pub fn load_parts(world: &mut World, parts: &[(SaveKind, Option<Entity>)], slots: &SaveSlots) -> Result<Vec<LoadReport>, String> {
    // Nothing is applied before every file is read: a refusal changes nothing, and the call
    // that made it is not part of the recording.
    let refuse = |world: &mut World, message: String| {
        ai_core::replay::forget_call(world);
        message
    };
    let mut reads = Vec::new();
    for &(kind, _) in parts {
        match read_slot(world, kind, slots.get(kind), true) {
            Ok(read) => reads.push(read),
            Err(e) => return Err(refuse(world, e)),
        }
    }
    let bodies: Vec<Option<Entity>> =
        parts.iter().map(|(kind, body)| if *kind == SaveKind::Character { resolve_body(world, *body) } else { None }).collect();
    if let Some(i) = reads.iter().zip(&bodies).position(|(r, body)| r.kind == SaveKind::Character && body.is_none()) {
        let message = format!("cannot load {}/{}: there is no player body to put the character into", reads[i].kind, reads[i].slot);
        return Err(refuse(world, message));
    }
    let mut reports = Vec::new();
    let mut applied_anything = false;
    for (read, body) in reads.into_iter().zip(bodies) {
        let Read { kind, slot, file, prepared, recovery } = read;
        for name in &prepared.unknown {
            journal_warn(world, format!("save: unknown {kind} section {name} in {kind}/{slot} skipped (this game does not save it)"));
        }
        let unknown = prepared.unknown.clone();
        match apply_prepared(world, kind, body, prepared) {
            Ok(done) => {
                applied_anything |= !done.applied.is_empty() || !done.missing.is_empty();
                reports.push(LoadReport { kind, slot, tick: file.tick, label: file.label, applied: done.applied.len(), missing: done.missing.len(), unknown, recovery });
            }
            Err(e) => {
                applied_anything |= !e.applied.is_empty();
                journal::emit(world, JournalKind::Error, format!("save: loading {kind}/{slot} failed: {}", e.message));
                let done = if e.applied.is_empty() { "nothing of it was applied".to_owned() } else { format!("applied before it: {}", e.applied.join(", ")) };
                if applied_anything {
                    restart_recording(world, "load_game");
                } else {
                    ai_core::replay::forget_call(world);
                }
                return Err(format!(
                    "loading {kind}/{slot} failed at {}; {done}. The world is partly loaded: fix the cause and load again, or go back with snapshot_load",
                    e.message
                ));
            }
        }
    }
    for report in &reports {
        world.resource_mut::<SaveSlots>().set(report.kind, &report.slot);
        journal::log(world, JournalKind::Sim, format!("save: {report}"));
    }
    restart_recording(world, "load_game");
    Ok(reports)
}

/// The recording starts again from the state just loaded, so a replay saved after a load
/// verifies (a load is not something playback could redo: it reads a file).
///
/// SEAM(C1): once `ai_core::replay::restart_from_current(world, reason)` exists, this
/// function is that call and nothing else. Until then the call that made the load is taken
/// out of the recording (it must not play back) and the journal says the recording is not
/// restarted.
fn restart_recording(world: &mut World, reason: &str) {
    ai_core::replay::forget_call(world);
    journal_warn(
        world,
        format!(
            "save: the recording was not restarted after {reason} (SEAM(C1): ai_core::replay::restart_from_current is not \
             there yet), so a replay saved now will not verify"
        ),
    );
}

// --- checking ----------------------------------------------------------------------------

/// How the current state compares with a file.
#[derive(Clone, Debug, PartialEq)]
pub struct FileCheck {
    pub slot: String,
    /// The tick the file was saved at.
    pub tick: u64,
    pub label: String,
    /// Sections present in both whose data differs.
    pub differing: Vec<String>,
    /// Sections this game saves that the file lacks (`missing` would run).
    pub only_here: Vec<String>,
    /// Sections in the file this game does not save (skipped on load).
    pub only_in_file: Vec<String>,
    /// The backup that was read because the primary is not usable.
    pub from_backup: Option<u8>,
}

impl FileCheck {
    pub fn same(&self) -> bool {
        self.differing.is_empty() && self.only_here.is_empty() && self.only_in_file.is_empty()
    }
}

/// The saveable state of one kind, hashed, and compared with a file when there is one.
#[derive(Clone, Debug, PartialEq)]
pub struct CheckReport {
    pub kind: SaveKind,
    /// FNV-1a of the canonical JSON of the sections: equal states, equal hashes.
    pub hash: u64,
    pub sections: usize,
    pub file: Option<FileCheck>,
}

/// Hashes the saveable state of `what` and, with a `slot`, compares it section by section
/// with that slot's file (migrated to the current versions; a damaged primary is compared
/// through its backup, and nothing is rewritten).
pub fn check_state(world: &mut World, what: SaveWhat, slot: Option<&str>) -> Result<Vec<CheckReport>, String> {
    check_parts(world, &what.parts(), slot)
}

/// [`check_state`] for the parts given: a character part names its body.
pub fn check_parts(world: &mut World, parts: &[(SaveKind, Option<Entity>)], slot: Option<&str>) -> Result<Vec<CheckReport>, String> {
    let mut reports = Vec::new();
    for &(kind, body) in parts {
        let body = if kind == SaveKind::Character { resolve_body(world, body) } else { None };
        let current = capture_sections(world, kind, body)?;
        let hash = canon::fnv(current.canonical().as_bytes());
        let file = match slot {
            None => None,
            Some(slot) => {
                let read = read_slot(world, kind, slot, false)?;
                Some(compare(world, kind, &current, read))
            }
        };
        reports.push(CheckReport { kind, hash, sections: current.len(), file });
    }
    Ok(reports)
}

fn compare(world: &World, kind: SaveKind, current: &SectionSet, read: Read) -> FileCheck {
    let registry = world.resource::<SaveRegistry>();
    let mut check = FileCheck {
        slot: read.slot,
        tick: read.file.tick,
        label: read.file.label,
        differing: Vec::new(),
        only_here: Vec::new(),
        only_in_file: read.prepared.unknown.clone(),
        from_backup: read.recovery.map(|r| r.backup),
    };
    for entry in registry.of_kind(kind) {
        match (current.0.get(&entry.name), read.prepared.sections.get(&entry.name)) {
            (Some(here), Some(there)) if canon::to_string(&here.data) != canon::to_string(there) => check.differing.push(entry.name.clone()),
            (Some(_), None) => check.only_here.push(entry.name.clone()),
            _ => {}
        }
    }
    check
}
