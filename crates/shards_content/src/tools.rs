//! What agents can do with content: list and read definitions, check the files on
//! disk, reload them, spawn prefabs.

use ai_core::journal::{self, JournalKind};
use ai_core::prelude::*;
use ai_core::tools::{register_tool, ToolKind};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::db::{closest, read_dir, ContentDb, ContentKinds, DefChange};
use crate::load::{asset_root, reload, shown};
use crate::prefab::spawn_prefab_tool;

pub(crate) fn register(world: &mut World) {
    use Scope::{Dev, Player};
    use ToolKind::{Action, Control, Query};
    register_tool(world, "content_list", Player, Query, content_list);
    register_tool(world, "content_get", Player, Query, content_get);
    register_tool(world, "content_check", Dev, Query, content_check);
    register_tool(world, "content_reload", Dev, Control, content_reload);
    register_tool(world, "content_apply", Dev, Action, content_apply);
    register_tool(world, "spawn_prefab", Dev, Action, spawn_prefab_tool);
}

/// The kind `kind`, or an error that lists the kinds there are.
fn known_kind(db: &ContentDb, kind: &str) -> Result<(), String> {
    if db.ids_of(kind).is_some() {
        return Ok(());
    }
    let kinds: Vec<&str> = db.kinds().collect();
    let hint = closest(kind, kinds.iter().copied()).map(|k| format!(" (did you mean `{k}`?)")).unwrap_or_default();
    Err(format!("no content kind `{kind}`{hint}; kinds: {}", kinds.join(", ")))
}

/// What content the game has: every kind with its number of definitions, or with
/// `kind`, the ids of that kind and the file each comes from.
#[derive(Deserialize, JsonSchema, Default)]
pub struct ContentListArgs {
    /// A content kind, e.g. "prefab".
    pub kind: Option<String>,
}

fn content_list(In(call): In<Call<ContentListArgs>>, db: Res<ContentDb>) -> ToolResult {
    if !db.is_loaded() {
        return Err("content is still loading; wait with `shards wait`".into());
    }
    let errors = db.errors().count();
    let Some(kind) = call.args.kind else {
        let summary = db.summary();
        let mut out = format!("content: {} definitions in {} files", summary.definitions, summary.files);
        if errors > 0 {
            out.push_str(&format!(", {errors} errors (content_check lists them)"));
        }
        for (kind, n) in &summary.kinds {
            out.push_str(&format!("\n  {kind}  {n}"));
        }
        return Ok(out);
    };
    known_kind(&db, &kind)?;
    let ids = db.ids_of(&kind).unwrap_or_default();
    let mut out = format!("{kind}: {} definitions", ids.len());
    for id in ids {
        out.push_str(&format!("\n  {id}  {}", shown(db.file(&kind, id).unwrap_or_default())));
    }
    Ok(out)
}

/// One content definition as JSON, with the file it comes from.
#[derive(Deserialize, JsonSchema)]
pub struct ContentGetArgs {
    /// Content kind, e.g. "prefab".
    pub kind: String,
    pub id: String,
}

fn content_get(In(call): In<Call<ContentGetArgs>>, db: Res<ContentDb>) -> ToolResult<Value> {
    let ContentGetArgs { kind, id } = call.args;
    known_kind(&db, &kind)?;
    let Some(def) = db.get_json(&kind, &id) else {
        let ids = db.ids_of(&kind).unwrap_or_default();
        let hint = closest(&id, ids.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        return Err(format!("no {kind} `{id}`{hint}; `content_list kind={kind}` lists them"));
    };
    Ok(json!({"kind": kind, "id": id, "file": db.file(&kind, &id), "def": def}))
}

/// Read every content file from disk again, parse and validate it, and report:
/// `ok: 14 definitions in 5 files` or every problem with its file. Applies nothing.
fn content_check(In(_): In<Call<NoArgs>>, world: &mut World) -> ToolResult {
    let kinds = world.resource::<ContentKinds>().clone();
    match crate::db::check_dir(&asset_root(world), &kinds) {
        Ok(summary) => Ok(summary.to_string()),
        Err(errors) => {
            let lines: Vec<String> = errors.iter().map(|e| format!("  {e}")).collect();
            Ok(format!("{} errors:\n{}", errors.len(), lines.join("\n")))
        }
    }
}

/// Re-read content from disk now and apply it, as hot reload does when a file
/// changes: everything, one `file` ("combat/attacks.attack.ron"), or one `kind`. A
/// version with new problems is refused and the previous one kept. What changed is
/// applied (and recorded for replays) through `content_apply`.
#[derive(Deserialize, JsonSchema, Default)]
pub struct ContentReloadArgs {
    /// One file, relative to content/ (or the full asset path).
    pub file: Option<String>,
    /// Every file of one kind.
    pub kind: Option<String>,
}

fn content_reload(In(call): In<Call<ContentReloadArgs>>, world: &mut World) -> ToolResult {
    let on_disk = read_dir(&asset_root(world)).map_err(|e| e.to_string())?;
    let db = world.resource::<ContentDb>();
    let wanted = |path: &str| -> bool {
        match (&call.args.file, &call.args.kind) {
            (Some(file), _) => shown(path) == shown(file),
            (None, Some(kind)) => path.ends_with(&format!(".{kind}.ron")),
            (None, None) => true,
        }
    };
    if let Some(kind) = &call.args.kind {
        known_kind(db, kind)?;
    }
    let changed: Vec<_> = on_disk.iter().filter(|s| wanted(&s.path) && db.file_text(&s.path) != Some(s.text.as_str())).cloned().collect();
    let removed: Vec<String> =
        db.files().filter(|f| wanted(f) && !on_disk.iter().any(|s| s.path == *f)).map(str::to_owned).collect();
    if let Some(file) = &call.args.file
        && !on_disk.iter().any(|s| shown(&s.path) == shown(file))
        && removed.is_empty()
    {
        let all: Vec<&str> = on_disk.iter().map(|s| shown(&s.path)).collect();
        let hint = closest(shown(file), all.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        return Err(format!("no content file `{file}`{hint}; files: {}", all.join(", ")));
    }
    if changed.is_empty() && removed.is_empty() {
        return Ok("content: nothing changed on disk".into());
    }
    reload(world, changed, removed)
}

/// Apply content definitions given as JSON: what hot reload and `content_reload` do
/// with a checked edit, and what replays record and play back. Prefer
/// `content_reload`.
#[derive(Deserialize, JsonSchema)]
pub struct ContentApplyArgs {
    /// Files the change comes from, e.g. ["content/combat/attacks.attack.ron"].
    #[serde(default)]
    pub files: Vec<String>,
    /// Definitions added, replaced (`def`) or removed (no `def`).
    pub changes: Vec<DefChange>,
}

fn content_apply(In(call): In<Call<ContentApplyArgs>>, world: &mut World) -> ToolResult {
    let ContentApplyArgs { files, changes } = call.args;
    let kinds = world.resource::<ContentKinds>().clone();
    let current = world.resource::<ContentDb>();
    let next = current.with_changes(&kinds, &files, &changes)?;
    let problems: Vec<String> = next.new_errors(current).into_iter().map(ToString::to_string).collect();
    if !problems.is_empty() {
        return Err(format!("refused, it would break content: {}", problems.join("; ")));
    }
    world.insert_resource(next);
    let n = changes.len();
    let defs = if n == 1 { "definition" } else { "definitions" };
    let names: Vec<&str> = files.iter().map(|f| shown(f)).collect();
    let from = if names.is_empty() { String::new() } else { format!(" {}", names.join(", ")) };
    let text = format!("content reloaded:{from} ({n} {defs} changed)");
    journal::emit(world, JournalKind::Event, text.clone());
    Ok(text)
}
