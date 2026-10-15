//! Tools: `save_game`, `load_game`, `save_check`, `save_list`, `save_registry`, `save_damage`.
//!
//! An agent drives saves the way a player's menu does (`save_game`, `load_game`), and checks
//! them the way a tester does: `save_check` compares the state with a file,
//! `save_damage` breaks a file so the recovery can be watched.

use ai_core::tools::{register_tool, ToolKind};
use ai_core::{Call, Caller, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use shards_core::PlayerId;

use crate::ops::{self, default_body, replay_playing, size_text, SaveSlots, NOT_WRITTEN_DURING_PLAYBACK};
use crate::section::{plural, SaveKind, SaveRegistry};
use crate::store::{valid_slot, Damage, FileState, Which};

pub(crate) fn register(world: &mut World) {
    register_tool(world, "save_game", Scope::Player, ToolKind::Control, save_game);
    register_tool(world, "load_game", Scope::Dev, ToolKind::Action, load_game);
    register_tool(world, "save_check", Scope::Dev, ToolKind::Query, save_check);
    register_tool(world, "save_list", Scope::Player, ToolKind::Query, save_list);
    register_tool(world, "save_registry", Scope::Player, ToolKind::Query, save_registry);
    register_tool(world, "save_damage", Scope::Dev, ToolKind::Control, save_damage);
}

/// What the tools can be pointed at.
#[derive(Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum What {
    /// The host's world (needs dev scope).
    World,
    /// Your character: the body you are bound to, or player 0's.
    Character,
    /// The world, then the character.
    #[default]
    All,
}

impl What {
    fn kinds(self) -> &'static [SaveKind] {
        match self {
            What::World => &[SaveKind::World],
            What::Character => &[SaveKind::Character],
            What::All => &[SaveKind::World, SaveKind::Character],
        }
    }
}

/// The body character saves are about for this caller: the body it is bound to when that
/// is a player's, else player 0's.
fn caller_body(world: &World, caller: &Caller) -> Option<Entity> {
    let is_player = |e: Entity| world.get::<PlayerId>(e).is_some() || world.get::<ai_core::AiPlayer>(e).is_some();
    caller.subject.filter(|e| is_player(*e)).or_else(|| default_body(world))
}

fn parts(world: &World, caller: &Caller, what: What) -> Vec<(SaveKind, Option<Entity>)> {
    what.kinds().iter().map(|&kind| (kind, (kind == SaveKind::Character).then(|| caller_body(world, caller)).flatten())).collect()
}

/// The slot named, or the current one of `kind`.
fn slot_for(world: &World, slot: Option<&str>, kind: SaveKind) -> String {
    slot.map(str::to_owned).unwrap_or_else(|| world.get_resource::<SaveSlots>().map(|s| s.get(kind).to_owned()).unwrap_or_default())
}

/// Save the game now: the world, your character, or both, into a slot (default: the slot
/// last used). Files go to the game's save folder (`saves/world/<slot>.save`,
/// `saves/character/<slot>.save`) with a checksum, and the three previous saves of the slot
/// are kept as backups. Saving never changes the game. The world is the host's, so it needs
/// dev scope; a character is yours to save. While a replay is played back nothing is written.
#[derive(Deserialize, JsonSchema, Default)]
pub struct SaveGameArgs {
    /// `world`, `character` or `all` (default).
    #[serde(default)]
    pub what: What,
    /// Slot name: 1 to 32 characters from a-z, 0-9, `_`, `-`. Default: the current slot.
    pub slot: Option<String>,
    /// What to call this save in `save_list`.
    pub label: Option<String>,
}

fn save_game(In(call): In<Call<SaveGameArgs>>, world: &mut World) -> ToolResult {
    let SaveGameArgs { what, slot, label } = call.args;
    if what != What::Character {
        call.caller.require_dev("saving the world").map_err(|e| format!("{e}; save your character with what=character"))?;
    }
    if let Some(slot) = &slot {
        valid_slot(slot)?;
    }
    let slots = SaveSlots {
        world: slot_for(world, slot.as_deref(), SaveKind::World),
        character: slot_for(world, slot.as_deref(), SaveKind::Character),
    };
    let parts = parts(world, &call.caller, what);
    let reports = ops::save_parts(world, &parts, &slots, label.as_deref().unwrap_or(""), "save_game")?;
    if reports.iter().any(|r| !r.written) {
        // The files are as they were; a played-back replay must not look like it saved.
        let skipped: Vec<String> = reports.iter().map(ToString::to_string).collect();
        return Err(format!("{NOT_WRITTEN_DURING_PLAYBACK}: {}. Stop the replay (or let it finish) to save", skipped.join("; ")));
    }
    Ok(format!("ok: {}", reports.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")))
}

/// Load a save into the running game: the world, your character, or both, from a slot
/// (default: the slot last used). A save is the player's progress applied onto the world
/// the game built, not a snapshot of the simulation: use `snapshot_load` for that. Every
/// file is read and checked first; a damaged primary is restored from its newest good
/// backup (the journal says so) and a slot with no good copy changes nothing. The
/// recording restarts from the loaded state.
#[derive(Deserialize, JsonSchema, Default)]
pub struct LoadGameArgs {
    /// `world`, `character` or `all` (default).
    #[serde(default)]
    pub what: What,
    /// Slot name. Default: the current slot.
    pub slot: Option<String>,
}

fn load_game(In(call): In<Call<LoadGameArgs>>, world: &mut World) -> ToolResult {
    let LoadGameArgs { what, slot } = call.args;
    // A refusal that changes nothing is not part of the recording.
    let refuse = |world: &mut World, message: String| {
        ai_core::replay::forget_call(world);
        message
    };
    if let Some(slot) = &slot
        && let Err(e) = valid_slot(slot)
    {
        return Err(refuse(world, e));
    }
    // Without a slot each kind loads from its own current one.
    let slots = SaveSlots { world: slot_for(world, slot.as_deref(), SaveKind::World), character: slot_for(world, slot.as_deref(), SaveKind::Character) };
    let parts = parts(world, &call.caller, what);
    let reports = ops::load_parts(world, &parts, &slots)?;
    Ok(format!("ok: {}", reports.iter().map(ToString::to_string).collect::<Vec<_>>().join("; ")))
}

/// Hash the saveable state (what a save would hold) and, with a slot, compare it with that
/// slot's file section by section: `same` when nothing differs, otherwise the sections that
/// do. After `load_game` the answer is `same`. A damaged primary is compared through its
/// backup (and left alone).
#[derive(Deserialize, JsonSchema, Default)]
pub struct SaveCheckArgs {
    /// `world`, `character` or `all` (default).
    #[serde(default)]
    pub what: What,
    /// Compare with this slot's file. Omit to only hash.
    pub slot: Option<String>,
}

fn save_check(In(call): In<Call<SaveCheckArgs>>, world: &mut World) -> ToolResult<Value> {
    let SaveCheckArgs { what, slot } = call.args;
    if let Some(slot) = &slot {
        valid_slot(slot)?;
    }
    let mut out = serde_json::Map::new();
    let mut summary = Vec::new();
    for part in parts(world, &call.caller, what) {
        let name = slot.clone();
        let report = ops::check_parts(world, &[part], name.as_deref())?.remove(0);
        let mut entry = json!({"hash": format!("{:016x}", report.hash), "sections": report.sections});
        let mut line = format!("{}: {:016x} ({})", report.kind, report.hash, plural(report.sections, "section", "sections"));
        if let Some(file) = &report.file {
            entry["file"] = json!({
                "slot": file.slot, "tick": file.tick, "label": file.label, "same": file.same(),
                "differing": file.differing, "only_here": file.only_here, "only_in_file": file.only_in_file,
                "from_backup": file.from_backup,
            });
            if file.same() {
                line.push_str(&format!("; same as {}/{} (tick {})", report.kind, file.slot, file.tick));
            } else {
                let mut why = Vec::new();
                if !file.differing.is_empty() {
                    why.push(format!("differs in {}", file.differing.join(", ")));
                }
                if !file.only_here.is_empty() {
                    why.push(format!("the file lacks {}", file.only_here.join(", ")));
                }
                if !file.only_in_file.is_empty() {
                    why.push(format!("the file has {} that this game does not save", file.only_in_file.join(", ")));
                }
                line.push_str(&format!("; {}/{} (tick {}) {}", report.kind, file.slot, file.tick, why.join("; ")));
            }
            if let Some(n) = file.from_backup {
                line.push_str(&format!(" [read from backup {n}: the primary is not usable]"));
            }
        }
        out.insert(report.kind.to_string(), entry);
        summary.push(line);
    }
    out.insert("summary".to_owned(), json!(summary));
    Ok(Value::Object(out))
}

/// The slots there are, what is in them and how each file is: the save, then the backups
/// (`bak1` is the newest), each `ok` with its tick and label, `damaged` with the reason, or
/// `missing`.
#[derive(Deserialize, JsonSchema, Default)]
pub struct SaveListArgs {}

fn save_list(In(_): In<Call<SaveListArgs>>, world: &mut World) -> ToolResult<Value> {
    let config = world.get_resource::<crate::SaveConfig>().ok_or("the save module is not installed")?.clone();
    let store = config.store();
    let current = world.get_resource::<SaveSlots>().cloned().unwrap_or_default();
    let mut slots = Vec::new();
    let mut lines = Vec::new();
    for kind in SaveKind::ALL {
        for slot in store.slots(kind) {
            let info = store.inspect_with(kind, &slot, &ops::newer_check(world, kind));
            let mut files = Vec::new();
            let mut parts = Vec::new();
            for file in &info.files {
                let (state, text) = match &file.state {
                    FileState::Missing => (json!({"state": "missing"}), "missing".to_owned()),
                    FileState::Ok { tick, label, size, sections } => (
                        json!({"state": "ok", "tick": tick, "label": label, "size": size, "sections": sections}),
                        format!("ok (tick {tick}, {}{})", size_text(*size), if label.is_empty() { String::new() } else { format!(", \"{label}\"") }),
                    ),
                    FileState::Damaged(problem) => (json!({"state": "damaged", "problem": problem}), format!("damaged ({problem})")),
                    FileState::Newer(what) => (json!({"state": "newer", "problem": what}), format!("from a newer version of the game ({what}); left alone")),
                };
                let mut entry = state;
                entry["file"] = json!(file.name);
                files.push(entry);
                let short = match file.which {
                    Which::Save => "save".to_owned(),
                    other => other.describe().replace("backup ", "bak"),
                };
                parts.push(format!("{short} {text}"));
            }
            let marker = if current.get(kind) == slot { " (current)" } else { "" };
            lines.push(format!("{kind}/{slot}{marker}: {}", parts.join(", ")));
            slots.push(json!({"kind": kind, "slot": slot, "current": current.get(kind) == slot, "files": files, "damaged_copy": info.damaged_copy}));
        }
    }
    if lines.is_empty() {
        lines.push(format!("no saves yet in {}", config.dir.display()));
    }
    Ok(json!({"dir": config.dir.display().to_string(), "current": {"world": current.world, "character": current.character}, "slots": slots, "summary": lines}))
}

/// What the game saves: every section with its kind, version, the crate or game that
/// registered it and the versions it can migrate from, in the order sections are applied.
#[derive(Deserialize, JsonSchema, Default)]
pub struct SaveRegistryArgs {}

fn save_registry(In(_): In<Call<SaveRegistryArgs>>, registry: Res<SaveRegistry>) -> ToolResult<Value> {
    let sections: Vec<Value> = registry
        .sections()
        .iter()
        .map(|s| json!({"name": s.name, "kind": s.kind, "version": s.version, "source": s.source, "migrations_from": registry.migrations_of(&s.name)}))
        .collect();
    let summary: Vec<String> =
        registry.sections().iter().map(|s| format!("{} {} v{} ({})", s.kind, s.name, s.version, s.source)).collect();
    Ok(json!({"sections": sections, "summary": summary}))
}

/// How `save_damage` breaks a file.
#[derive(Deserialize, JsonSchema, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum How {
    /// One bit of a byte in the middle changes: the checksum fails.
    Flip,
    /// The second half is cut off.
    Truncate,
    /// The file becomes noise of the same length.
    Garbage,
}

/// Which copy of the slot.
#[derive(Deserialize, JsonSchema, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SlotFile {
    /// `<slot>.save`
    #[default]
    Save,
    Bak1,
    Bak2,
    Bak3,
}

/// Damage a save file on purpose, to watch the recovery (QA of the save path). Only files in
/// the save folder can be touched, and not while a replay is played back. Load the slot
/// afterwards: a damaged save is restored from the newest good backup and the journal says so.
#[derive(Deserialize, JsonSchema)]
pub struct SaveDamageArgs {
    /// `world`, `character` or `all`.
    pub what: What,
    /// `flip`, `truncate` or `garbage`.
    pub how: How,
    /// Slot name. Default: the current slot.
    pub slot: Option<String>,
    /// `save` (default), `bak1`, `bak2` or `bak3`.
    #[serde(default)]
    pub copy: SlotFile,
}

fn save_damage(In(call): In<Call<SaveDamageArgs>>, world: &mut World) -> ToolResult {
    let SaveDamageArgs { what, how, slot, copy } = call.args;
    if let Some(slot) = &slot {
        valid_slot(slot)?;
    }
    if replay_playing(world) {
        return Err(format!("{NOT_WRITTEN_DURING_PLAYBACK}: save_damage changes files on disk, and a played-back replay touches none"));
    }
    let store = world.get_resource::<crate::SaveConfig>().ok_or("the save module is not installed")?.store();
    let which = match copy {
        SlotFile::Save => Which::Save,
        SlotFile::Bak1 => Which::Backup(1),
        SlotFile::Bak2 => Which::Backup(2),
        SlotFile::Bak3 => Which::Backup(3),
    };
    let how = match how {
        How::Flip => Damage::Flip,
        How::Truncate => Damage::Truncate,
        How::Garbage => Damage::Garbage,
    };
    let mut done = Vec::new();
    for &kind in what.kinds() {
        let name = slot_for(world, slot.as_deref(), kind);
        done.push(store.damage(kind, &name, which, how)?);
    }
    Ok(format!("ok: {}", done.join("; ")))
}
