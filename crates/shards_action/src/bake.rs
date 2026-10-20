//! The displacement bake: an action's `displacement` curve read off the root bone of its clip.
//!
//! An action names the clip it is baked from (`bake: (animset: "humanoid", clip: "light_1")`);
//! the animset names the root bone (`root_motion: "root"`). [`bake_action`] samples that bone's
//! translation at [`SAMPLE_HZ`] (and at the action's end, when that falls between ticks), keeps
//! the move along the model's forward axis (glTF +Z: the clip's own forward), drops the
//! vertical (a roll's bob is the picture's) and reports the sideways drift it cannot keep, then
//! thins the samples to the keys a straight line between neighbours does not already give.
//! [`bake_files`] writes the keys into the action's file in place (comments and the rest of the
//! file stay as they are): `--bake-motion [ACTIONS]` and the `motion_bake` tool run it. Same file,
//! same clip, same bytes.
//!
//! The clip must be timed to the action: its `phases` extra (Blender exports an action's
//! custom property `phases` as the animation's extras) names the same phases with the same
//! lengths, or, without one, it lasts as long as the action. A content check with the asset
//! folder bakes again and calls a curve that differs stale, naming the command.

use std::fmt;
use std::path::Path;

use ai_core::prelude::*;
use ai_core::tools::{register_tool, ToolKind};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_content::{asset_root, ContentDb, Refs};

use crate::anim::AnimSet;
use crate::clipfile::ClipFile;
use crate::def::{ActionDef, MotionBake, Phase};

/// Samples per second: the engine's default tick rate, so every tick of the action has a key.
pub const SAMPLE_HZ: f32 = 60.0;

/// A sample that a straight line between the keys around it passes this close to (m) is left
/// out.
const KEEP_WITHIN: f32 = 5e-4;

/// What a bake found.
#[derive(Clone, Debug, PartialEq)]
pub struct Baked {
    /// `(seconds, meters forward)`, from `(0, 0)` (left out), as `displacement` takes them.
    pub keys: Vec<(f32, f32)>,
    /// Meters forward at the end.
    pub forward: f32,
    /// The farthest the root strayed sideways (m): not kept, the curve is forward only.
    pub sideways: f32,
    /// The highest or lowest the root went (m): not kept, the body stays on the ground.
    pub vertical: f32,
    /// How many samples were read.
    pub samples: usize,
}

impl Baked {
    /// The keys as they go into the file: `[(0.2, 0.0), (0.2167, 0.008)]`.
    pub fn text(&self) -> String {
        keys_text(&self.keys)
    }
}

/// `[(t, m), ...]` with times to 0.1 ms and meters to the millimetre, trailing zeros left out.
pub fn keys_text(keys: &[(f32, f32)]) -> String {
    let pairs: Vec<String> = keys.iter().map(|&(t, m)| format!("({}, {})", number(t, 4), number(m, 3))).collect();
    format!("[{}]", pairs.join(", "))
}

/// `v` to `decimals` places, at least one, without trailing zeros; never `-0.0`.
pub(crate) fn number(v: f32, decimals: usize) -> String {
    let mut s = format!("{v:.decimals$}");
    while s.ends_with('0') && !s.ends_with(".0") {
        s.pop();
    }
    if s == "-0.0" { "0.0".to_owned() } else { s }
}

/// The phases as `windup 0.2 active 0.1`.
fn phases_text(phases: &[(String, f32)]) -> String {
    phases.iter().map(|(n, s)| format!("{n} {}", number(*s, 4).trim_end_matches(".0"))).collect::<Vec<_>>().join(" ")
}

/// Bakes the root bone `bone` of animation `animation` in `file` (shown as `source`) for an
/// action with `phases`.
pub fn bake_clip(file: &ClipFile, source: &str, animation: &str, bone: &str, phases: &[Phase]) -> Result<Baked, String> {
    if !file.has_animation(animation) {
        return Err(format!("`{source}` has no animation `{animation}`"));
    }
    if !file.has_node(bone) {
        return Err(format!("`{source}` has no root bone `{bone}`"));
    }
    if let Some(parent) = file.turned_parent(bone) {
        return Err(format!("root bone `{bone}` of `{source}` sits under node `{parent}`, which turns it"));
    }
    let length: f32 = phases.iter().map(|p| p.1).sum();
    let duration = file.duration(animation).unwrap_or(0.0);
    let half_tick = 0.5 / SAMPLE_HZ;
    let action: Vec<(String, f32)> = phases.iter().map(|p| (p.0.clone(), p.1)).collect();
    match file.phases(animation) {
        Some(Err(e)) => return Err(format!("clip `{animation}` of `{source}`: {e}")),
        Some(Ok(clip)) => {
            let same = clip.len() == action.len()
                && clip.iter().zip(&action).all(|(c, a)| c.0 == a.0 && (c.1 - a.1).abs() <= half_tick);
            if !same {
                return Err(format!(
                    "clip `{animation}` of `{source}` is timed to `{}`, the action to `{}`; make them match (the clip, \
                     or the action's phases) and bake again",
                    phases_text(&clip),
                    phases_text(&action)
                ));
            }
        }
        None if (duration - length).abs() > half_tick => {
            return Err(format!(
                "clip `{animation}` of `{source}` lasts {} s, the action {} s; make the clip as long as the action \
                 (or give it a `phases` extra) and bake again",
                number(duration, 4),
                number(length, 4)
            ));
        }
        None => {}
    }
    let scale = file.parent_scale(bone);
    let ticks = (length * SAMPLE_HZ).round() as usize;
    let track = file.translation(animation, bone);
    let at = |t: f32| track.map_or(Vec3::ZERO, |track| track.sample(t)) * scale;
    let origin = at(0.0);
    let (mut sideways, mut vertical) = (0.0f32, 0.0f32);
    // Every tick, and the action's own end when that is not on a tick (a last tick of 0.57 s ends at
    // 0.57 s, not at the tick before): the move the clip still makes there is the body's too.
    let mut times: Vec<f32> = (0..=ticks).map(|i| (i as f32 / SAMPLE_HZ).min(length)).collect();
    if times.last().is_some_and(|&t| t < length - 1e-4) {
        times.push(length);
    }
    let mut points = Vec::with_capacity(times.len());
    for t in times {
        let d = at(t) - origin;
        sideways = sideways.max(d.x.abs());
        vertical = vertical.max(d.y.abs());
        points.push((t, d.z));
    }
    let forward = points.last().map_or(0.0, |p| p.1);
    Ok(Baked { keys: thin(&points), forward, sideways, vertical, samples: points.len() })
}

/// The keys of `points` (the first one at `(0, 0)`) that a line between neighbours does not
/// give within [`KEEP_WITHIN`]; the first point, and a hold after the last move, left out.
fn thin(points: &[(f32, f32)]) -> Vec<(f32, f32)> {
    if points.len() < 2 {
        return Vec::new();
    }
    let fits = |a: usize, b: usize| {
        let ((t0, d0), (t1, d1)) = (points[a], points[b]);
        (a + 1..b).all(|i| {
            let (t, d) = points[i];
            let s = if t1 - t0 > 0.0 { (t - t0) / (t1 - t0) } else { 1.0 };
            (d0 + (d1 - d0) * s - d).abs() <= KEEP_WITHIN
        })
    };
    let mut kept = vec![0];
    let mut a = 0;
    while a + 1 < points.len() {
        let mut b = a + 1;
        while b + 1 < points.len() && fits(a, b + 1) {
            b += 1;
        }
        kept.push(b);
        a = b;
    }
    let mut keys: Vec<(f32, f32)> = kept.into_iter().map(|i| points[i]).collect();
    // The curve holds its last key: a flat stretch at the end says nothing.
    while keys.len() >= 2 && (keys[keys.len() - 1].1 - keys[keys.len() - 2].1).abs() <= KEEP_WITHIN {
        keys.pop();
    }
    keys.remove(0);
    keys
}

/// Where the bake of `bake` reads: the set, the file and the animation in it, the root bone.
struct Source<'a> {
    file: &'a str,
    animation: &'a str,
    bone: &'a str,
}

fn source<'a>(bake: &'a MotionBake, set: &'a AnimSet) -> Result<Source<'a>, String> {
    let bone = set.root_motion.as_deref().filter(|b| !b.is_empty()).ok_or_else(|| {
        format!("animset `{}` names no root bone to bake from; give it `root_motion: \"root\"`", bake.animset)
    })?;
    if !set.clips.contains_key(&bake.clip) {
        let names: Vec<&str> = set.clips.keys().map(String::as_str).collect();
        let hint = shards_content::db::closest(&bake.clip, names.iter().copied())
            .map(|c| format!(" (did you mean `{c}`?)"))
            .unwrap_or_default();
        return Err(format!("animset `{}` has no clip `{}`{hint}; clips: {}", bake.animset, bake.clip, names.join(", ")));
    }
    Ok(Source { file: set.source_of(&bake.clip), animation: set.file_name(&bake.clip), bone })
}

/// Bakes action `def` from the clip its `bake` names, reading the files under `root`.
pub fn bake_action(def: &ActionDef, db: &ContentDb, root: &Path) -> Result<Baked, String> {
    let bake = def.bake.as_ref().ok_or("it has no `bake`; name its clip: `bake: (animset: \"humanoid\", clip: \"roll\")`")?;
    let set = db.get::<AnimSet>(&bake.animset).ok_or_else(|| format!("there is no animset `{}`", bake.animset))?;
    let src = source(bake, set)?;
    let file = ClipFile::read(&root.join(src.file)).map_err(|e| format!("`{}` {e}", src.file))?;
    bake_clip(&file, src.file, src.animation, src.bone, &def.phases)
}

/// The content check of an action's `bake`: the references, and with an asset folder, a curve
/// that matches what its clip bakes to.
pub(crate) fn check(def: &ActionDef, id: &str, bake: &MotionBake, refs: &mut Refs) {
    if !refs.require::<AnimSet>(&bake.animset, "bake.animset") {
        return;
    }
    let Some(set) = refs.get::<AnimSet>(&bake.animset) else { return };
    let src = match source(bake, set) {
        Ok(src) => src,
        Err(e) => return refs.error(format!("bake: {e}")),
    };
    let Some(root) = refs.asset_root() else { return };
    let Ok(file) = ClipFile::read(&root.join(src.file)) else {
        return; // the animset's own check names the file
    };
    match bake_clip(&file, src.file, src.animation, src.bone, &def.phases) {
        Err(e) => refs.error(format!("bake: {e}")),
        Ok(baked) if baked.text() != keys_text(&def.displacement) => refs.error(format!(
            "displacement is stale: clip `{}` of `{}` bakes to {} {} ({} m forward); bake it again with \
             `--bake-motion {id}` (or the motion_bake tool)",
            bake.clip,
            src.file,
            baked.keys.len(),
            if baked.keys.len() == 1 { "key" } else { "keys" },
            number(baked.forward, 3),
        )),
        Ok(_) => {}
    }
}

// --- writing ---------------------------------------------------------------------------

/// What [`bake_files`] did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MotionBakeReport {
    /// One line per action baked: `light_1: 9 keys, 0.3 m forward ... -> content/...`.
    pub baked: Vec<String>,
    /// `(action, why)` of the actions that could not be baked.
    pub failed: Vec<(String, String)>,
}

impl MotionBakeReport {
    pub fn ok(&self) -> bool {
        self.failed.is_empty()
    }
}

impl fmt::Display for MotionBakeReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let n = self.baked.len();
        write!(f, "baked the displacement of {n} {} at {SAMPLE_HZ} Hz", if n == 1 { "action" } else { "actions" })?;
        for line in &self.baked {
            write!(f, "\n  {line}")?;
        }
        for (id, why) in &self.failed {
            write!(f, "\n  {id}: FAILED: {why}")?;
        }
        Ok(())
    }
}

/// Bakes the displacement of actions into their content files: `only` names action ids
/// (comma separated), `None` or `all` every action with a `bake`. The asset folder is the
/// game's; an action that cannot be baked is reported and its file left alone.
pub fn bake_files(world: &World, only: Option<&str>) -> Result<MotionBakeReport, String> {
    let db = world.get_resource::<ContentDb>().ok_or("content is not loaded")?;
    if !db.is_loaded() {
        return Err("content is still loading; wait with `shards wait`".into());
    }
    let root = asset_root(world);
    bake_db(db, &root, only)
}

/// [`bake_files`] on a content database and an asset folder.
pub fn bake_db(db: &ContentDb, root: &Path, only: Option<&str>) -> Result<MotionBakeReport, String> {
    let baked: Vec<&str> = db.iter::<ActionDef>().filter(|(_, d)| d.bake.is_some()).map(|(id, _)| id).collect();
    let wanted: Vec<String> = match only.map(str::trim).filter(|o| !o.is_empty() && *o != "all") {
        None => baked.iter().map(|s| (*s).to_owned()).collect(),
        Some(list) => list.split(',').map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect(),
    };
    for id in &wanted {
        if db.get::<ActionDef>(id).is_none() {
            let hint = shards_content::db::closest(id, db.ids::<ActionDef>()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
            return Err(format!("no action `{id}`{hint}"));
        }
        if !baked.contains(&id.as_str()) {
            return Err(format!(
                "action `{id}` has no `bake`; name its clip in it (`bake: (animset: \"humanoid\", clip: \"{id}\")`) first; actions \
                 with one: {}",
                if baked.is_empty() { "none".to_owned() } else { baked.join(", ") }
            ));
        }
    }
    let mut report = MotionBakeReport::default();
    for id in &wanted {
        let def = db.get::<ActionDef>(id).expect("checked above");
        let result = bake_action(def, db, root).and_then(|baked| {
            let path = db.file_of::<ActionDef>(id).ok_or("its file is unknown")?;
            let file = root.join(path);
            let text = std::fs::read_to_string(&file).map_err(|e| format!("cannot read {}: {e}", file.display()))?;
            let new = with_displacement(&text, id, &baked.text())?;
            let changed = new != text;
            if changed {
                std::fs::write(&file, &new).map_err(|e| format!("cannot write {}: {e}", file.display()))?;
            }
            let bake = def.bake.as_ref().expect("baked actions have one");
            let mut line = format!(
                "{id}: clip `{}`, {} {}, {} m forward by {} s",
                bake.clip,
                baked.keys.len(),
                if baked.keys.len() == 1 { "key" } else { "keys" },
                number(baked.forward, 3),
                baked.keys.last().map_or_else(|| "0.0".to_owned(), |k| number(k.0, 4)),
            );
            if baked.sideways > KEEP_WITHIN {
                line.push_str(&format!(" (sideways drift {} m left out)", number(baked.sideways, 3)));
            }
            line.push_str(&if changed { format!(" -> {path}") } else { format!(" (unchanged, {path})") });
            Ok(line)
        });
        match result {
            Ok(line) => report.baked.push(line),
            Err(why) => report.failed.push((id.clone(), why)),
        }
    }
    Ok(report)
}

/// Bake the displacement curves of actions from their clips' root bone into their content
/// files, as `--bake-motion` does: one action, several, or every action with a `bake`. The game
/// takes the new curves as it reloads the files.
#[derive(Deserialize, JsonSchema, Default)]
pub struct MotionBakeArgs {
    /// An action id, or several separated by commas. Default: every action with a `bake`.
    pub action: Option<String>,
}

fn motion_bake(In(call): In<Call<MotionBakeArgs>>, world: &mut World) -> ToolResult {
    let report = bake_files(world, call.args.action.as_deref())?;
    if report.ok() { Ok(report.to_string()) } else { Err(report.to_string()) }
}

pub(crate) fn register(app: &mut App) {
    register_tool(app.world_mut(), "motion_bake", Scope::Dev, ToolKind::Control, motion_bake);
}

// --- the file, in place -------------------------------------------------------------------

/// `text` (a RON map of actions) with action `id`'s `displacement` set to `keys`; a missing
/// field goes in front of its `bake`. Comments and layout stay.
pub fn with_displacement(text: &str, id: &str, keys: &str) -> Result<String, String> {
    let s = text.as_bytes();
    let (open, close) = entry(s, id).ok_or_else(|| format!("cannot find action `{id}` in its file"))?;
    let fields = fields(s, open + 1, close);
    let mut out = text.to_owned();
    if let Some(f) = fields.iter().find(|f| f.name == "displacement") {
        out.replace_range(f.value.0..f.value.1, keys);
        return Ok(out);
    }
    if keys == "[]" {
        return Ok(out); // no move, no field
    }
    let bake = fields.iter().find(|f| f.name == "bake").ok_or_else(|| format!("action `{id}` has no `bake` field in its file"))?;
    let line_start = text[..bake.start].rfind('\n').map_or(0, |i| i + 1);
    let indent = &text[line_start..bake.start];
    let insert = if indent.trim().is_empty() {
        format!("displacement: {keys},\n{indent}")
    } else {
        format!("displacement: {keys}, ")
    };
    out.insert_str(bake.start, &insert);
    Ok(out)
}

struct Field {
    name: String,
    /// Where the field's name starts.
    start: usize,
    /// The value, trimmed: `[..]` of `displacement: [..],`.
    value: (usize, usize),
}

/// Whitespace and comments from `i` on skipped.
fn trivia(s: &[u8], mut i: usize) -> usize {
    loop {
        while i < s.len() && s[i].is_ascii_whitespace() {
            i += 1;
        }
        if s[i..].starts_with(b"//") {
            while i < s.len() && s[i] != b'\n' {
                i += 1;
            }
        } else if s[i..].starts_with(b"/*") {
            let mut depth = 0;
            while i < s.len() {
                if s[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if s[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                    if depth == 0 {
                        break;
                    }
                } else {
                    i += 1;
                }
            }
        } else {
            return i;
        }
    }
}

/// The end of the string or char literal starting at `i` (one past its closing quote).
fn literal(s: &[u8], i: usize) -> usize {
    if s[i] == b'r' {
        let hashes = s[i + 1..].iter().take_while(|&&c| c == b'#').count();
        let mut close = vec![b'"'];
        close.extend(std::iter::repeat_n(b'#', hashes));
        let body = i + 2 + hashes;
        return s[body..].windows(close.len()).position(|w| w == close.as_slice()).map_or(s.len(), |p| body + p + close.len());
    }
    let quote = s[i];
    let mut j = i + 1;
    while j < s.len() && s[j] != quote {
        j += if s[j] == b'\\' { 2 } else { 1 };
    }
    (j + 1).min(s.len())
}

/// The end of the value starting at `i`: the `,` or closing bracket after it at its own depth.
fn value_end(s: &[u8], mut i: usize) -> usize {
    let mut depth = 0usize;
    while i < s.len() {
        match s[i] {
            b'"' | b'\'' => {
                i = literal(s, i);
                continue;
            }
            b'r' if s.get(i + 1).is_some_and(|&c| c == b'"' || c == b'#') && (i == 0 || !s[i - 1].is_ascii_alphanumeric()) => {
                i = literal(s, i);
                continue;
            }
            b'/' if matches!(s.get(i + 1), Some(b'/' | b'*')) => {
                i = trivia(s, i);
                continue;
            }
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' if depth == 0 => return i,
            b')' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => return i,
            _ => {}
        }
        i += 1;
    }
    s.len()
}

/// `value` trimmed of trailing whitespace and comments: its last byte is the value's.
fn trimmed(s: &[u8], start: usize, end: usize) -> usize {
    let mut last = start;
    let mut i = start;
    while i < end {
        let next = trivia(s, i);
        if next > i {
            i = next;
            continue;
        }
        let v = match s[i] {
            b'"' | b'\'' => literal(s, i),
            _ => i + 1,
        };
        last = v;
        i = v;
    }
    last
}

/// The bracket that closes the one at `open`.
fn close_of(s: &[u8], open: usize) -> usize {
    let mut i = open + 1;
    loop {
        let end = value_end(s, i);
        if s.get(end) != Some(&b',') {
            return end;
        }
        i = end + 1;
    }
}

/// The brackets `(..)` of top-level entry `"id": (..)`: the index of `(` and of `)`.
fn entry(s: &[u8], id: &str) -> Option<(usize, usize)> {
    let mut i = trivia(s, 0);
    if s.get(i) != Some(&b'{') {
        return None;
    }
    i += 1;
    loop {
        i = trivia(s, i);
        if i >= s.len() || s[i] != b'"' {
            return None;
        }
        let key_end = literal(s, i);
        let key = &s[i + 1..key_end - 1];
        i = trivia(s, key_end);
        if s.get(i) != Some(&b':') {
            return None;
        }
        let start = trivia(s, i + 1);
        let end = value_end(s, start);
        if key == id.as_bytes() {
            let open = start + s[start..end].iter().position(|&c| c == b'(')?;
            return Some((open, close_of(s, open)));
        }
        i = end;
        if s.get(i) == Some(&b',') {
            i += 1;
        }
    }
}

/// The fields of a struct body between `start` and `end` (inside its brackets).
fn fields(s: &[u8], start: usize, end: usize) -> Vec<Field> {
    let mut out = Vec::new();
    let mut i = start;
    loop {
        i = trivia(s, i);
        if i >= end {
            return out;
        }
        let name_end = i + s[i..end].iter().take_while(|c| c.is_ascii_alphanumeric() || **c == b'_').count();
        if name_end == i {
            return out;
        }
        let name = String::from_utf8_lossy(&s[i..name_end]).into_owned();
        let colon = trivia(s, name_end);
        if s.get(colon) != Some(&b':') {
            return out;
        }
        let value_start = trivia(s, colon + 1);
        let stop = value_end(s, value_start).min(end);
        out.push(Field { name, start: i, value: (value_start, trimmed(s, value_start, stop)) });
        i = stop;
        if s.get(i) == Some(&b',') {
            i += 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clipfile::tests::glb;

    fn phases(list: &[(&str, f32)]) -> Vec<Phase> {
        list.iter().map(|(n, s)| Phase((*n).to_owned(), *s)).collect()
    }

    /// A lunge: still for 0.2 s, 0.6 m forward (glTF +Z) by 0.3 s with a sideways wobble and a
    /// hop, held to 0.6 s.
    fn lunge(extras: Option<&str>) -> ClipFile {
        let bytes = glb(
            serde_json::json!({}),
            &[(
                "lunge",
                extras,
                &[0.0, 0.2, 0.25, 0.3, 0.6],
                &[[0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.02, 0.1, 0.3], [0.0, 0.0, 0.6], [0.0, 0.0, 0.6]],
            )],
        );
        ClipFile::parse(&bytes, None).unwrap()
    }

    #[test]
    fn a_lunge_bakes_to_its_forward_move() {
        let file = lunge(Some("windup 0.2 active 0.1 recovery 0.3"));
        let baked =
            bake_clip(&file, "h.glb", "lunge", "root", &phases(&[("windup", 0.2), ("active", 0.1), ("recovery", 0.3)])).unwrap();
        assert_eq!(baked.text(), "[(0.2, 0.0), (0.3, 0.6)]", "linear stretches thin to their ends; the hold is left out");
        assert_eq!(baked.samples, 37, "every tick of 0.6 s and its start");
        assert!((baked.sideways - 0.02).abs() < 1e-6 && (baked.vertical - 0.1).abs() < 1e-6, "{baked:?}");
        assert_eq!(baked.forward, 0.6);
        // The same clip, the same bytes.
        let again = bake_clip(&file, "h.glb", "lunge", "root", &phases(&[("windup", 0.2), ("active", 0.1), ("recovery", 0.3)]));
        assert_eq!(again.unwrap(), baked);
    }

    #[test]
    fn an_action_that_ends_between_ticks_is_sampled_at_its_end() {
        // The root ramps to 0.5 m by 0.57 s; 0.57 s is 34.2 ticks, which rounds down to 34 and a
        // last tick sample at 0.5667 s, 0.49 m, 0.0033 s short of the end.
        let bytes = glb(serde_json::json!({}), &[("ramp", None, &[0.0, 0.4, 0.57], &[[0.0; 3], [0.0; 3], [0.0, 0.0, 0.5]])]);
        let file = ClipFile::parse(&bytes, None).unwrap();
        let baked = bake_clip(&file, "h.glb", "ramp", "root", &phases(&[("go", 0.57)])).unwrap();
        assert_eq!(baked.samples, 36, "35 ticks and the end");
        assert_eq!(baked.forward, 0.5, "{baked:?}");
        assert_eq!(baked.text(), "[(0.4, 0.0), (0.57, 0.5)]");
        // A length on a tick gets no sample beyond its last tick.
        let aligned = lunge(Some("windup 0.2 active 0.1 recovery 0.3"));
        let every = phases(&[("windup", 0.2), ("active", 0.1), ("recovery", 0.3)]);
        assert_eq!(bake_clip(&aligned, "h.glb", "lunge", "root", &every).unwrap().samples, 37, "no extra sample at 0.6 s");
    }

    #[test]
    fn a_clip_timed_otherwise_is_refused() {
        let file = lunge(Some("windup 0.2 active 0.1 recovery 0.3"));
        let e = bake_clip(&file, "h.glb", "lunge", "root", &phases(&[("windup", 0.25), ("active", 0.1), ("recovery", 0.25)]))
            .unwrap_err();
        assert!(e.contains("is timed to `windup 0.2 active 0.1 recovery 0.3`, the action to `windup 0.25 active 0.1 recovery 0.25`"), "{e}");
        let bare = lunge(None);
        let e = bake_clip(&bare, "h.glb", "lunge", "root", &phases(&[("roll", 0.4)])).unwrap_err();
        assert!(e.contains("lasts 0.6 s, the action 0.4 s"), "{e}");
        assert!(bake_clip(&bare, "h.glb", "lunge", "root", &phases(&[("a", 0.3), ("b", 0.3)])).is_ok(), "same length");
        assert!(bake_clip(&bare, "h.glb", "nope", "root", &[]).unwrap_err().contains("has no animation `nope`"));
        assert!(bake_clip(&bare, "h.glb", "lunge", "pelvis", &[]).unwrap_err().contains("has no root bone `pelvis`"));
    }

    #[test]
    fn the_parents_scale_counts() {
        let bytes = glb(serde_json::json!({"scale": [2.0, 2.0, 2.0]}), &[("m", None, &[0.0, 0.5], &[[0.0; 3], [0.0, 0.0, 1.0]])]);
        let file = ClipFile::parse(&bytes, None).unwrap();
        let baked = bake_clip(&file, "h.glb", "m", "root", &phases(&[("go", 0.5)])).unwrap();
        assert_eq!(baked.text(), "[(0.5, 2.0)]");
    }

    #[test]
    fn a_curve_keeps_the_keys_a_line_does_not_give() {
        // Eased: 0, 0.1, 0.4, 0.9, 1.0 m at 0..4 ticks.
        let points = [(0.0, 0.0), (1.0, 0.1), (2.0, 0.4), (3.0, 0.9), (4.0, 1.0), (5.0, 1.0)];
        assert_eq!(thin(&points), [(1.0, 0.1), (2.0, 0.4), (3.0, 0.9), (4.0, 1.0)]);
        assert!(thin(&[(0.0, 0.0), (1.0, 0.0), (2.0, 0.0)]).is_empty(), "no move: no keys");
        assert_eq!(keys_text(&[(0.21666667, 0.0081), (0.35, -0.0000001)]), "[(0.2167, 0.008), (0.35, 0.0)]");
    }

    const FILE: &str = r#"// The kit.
{
    "light_1": (
        net_id: 1,
        phases: [("windup", 0.2), ("active", 0.1)],
        // How far the lunge goes.
        displacement: [(0.2, 0.0), (0.3, 0.3)], // by hand
        bake: (animset: "humanoid", clip: "light_1"),
    ),
    "roll": (net_id: 5, phases: [("roll", 0.4)], steer: Move, bake: (animset: "h", clip: "roll")),
    "drink": (
        net_id: 7,
        phases: [("drink", 1.0)],
        bake: (animset: "humanoid", clip: "drink"),
    ),
    "odd": (net_id: 9, phases: [("a", 1.0)], tags: ["x,]"], displacement: [(1.0, 1.0)]),
}
"#;

    #[test]
    fn the_curve_is_replaced_in_place() {
        let out = with_displacement(FILE, "light_1", "[(0.2, 0.0), (0.3, 0.5)]").unwrap();
        assert_eq!(out, FILE.replace("(0.3, 0.3)], // by hand", "(0.3, 0.5)], // by hand"));
        let out = with_displacement(FILE, "odd", "[(1.0, 2.0)]").unwrap();
        assert_eq!(out, FILE.replace("displacement: [(1.0, 1.0)]", "displacement: [(1.0, 2.0)]"), "a `]` in a string is no bracket");
    }

    #[test]
    fn a_missing_curve_goes_in_front_of_its_bake() {
        let out = with_displacement(FILE, "drink", "[(0.5, 1.0)]").unwrap();
        assert!(out.contains("        phases: [(\"drink\", 1.0)],\n        displacement: [(0.5, 1.0)],\n        bake: (animset: \"humanoid\", clip: \"drink\"),"), "{out}");
        let out = with_displacement(FILE, "roll", "[(0.35, 4.0)]").unwrap();
        assert!(out.contains("steer: Move, displacement: [(0.35, 4.0)], bake: (animset: \"h\", clip: \"roll\"))"), "{out}");
        assert!(with_displacement(FILE, "nope", "[]").unwrap_err().contains("cannot find action `nope`"));
        assert!(with_displacement(FILE, "odd", "[]").is_ok());
        assert_eq!(with_displacement(FILE, "drink", "[]").unwrap(), FILE, "no move: no field to add");
    }

    /// An asset folder with `models/h.glb` (a lunge timed to light_1's phases, an idle) and
    /// content files `files`.
    fn assets(name: &str, files: &[(&str, &str)]) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("shards_action_bake_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("models")).unwrap();
        std::fs::create_dir_all(root.join("content")).unwrap();
        let glb = glb(
            serde_json::json!({}),
            &[
                (
                    "lunge",
                    Some("windup 0.2 active 0.1 recovery 0.3"),
                    &[0.0, 0.2, 0.3, 0.6],
                    &[[0.0; 3], [0.0; 3], [0.01, 0.0, 0.6], [0.0, 0.0, 0.6]],
                ),
                ("idle", None, &[0.0, 2.0], &[[0.0; 3], [0.0; 3]]),
            ],
        );
        std::fs::write(root.join("models/h.glb"), glb).unwrap();
        for (path, text) in files {
            std::fs::write(root.join(path), text).unwrap();
        }
        root
    }

    fn build(root: &Path) -> ContentDb {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<ActionDef>();
        kinds.register::<AnimSet>();
        kinds.register::<crate::def::MovesetDef>();
        kinds.register::<crate::status::StatusDef>();
        kinds.set_asset_root(root.to_owned());
        ContentDb::build(&kinds, &shards_content::db::read_dir(root).unwrap())
    }

    fn errors(db: &ContentDb) -> Vec<String> {
        db.errors().map(ToString::to_string).collect()
    }

    const SET: &str = r#"{ "h": (source: "models/h.glb", clips: {"lunge": (), "idle": ()}, root_motion: "root") }"#;

    #[test]
    fn the_animset_files_are_checked_with_an_asset_folder() {
        let set = r#"{ "h": (source: "models/h.glb", clips: {"idle": (), "slash": (name: "Slash"),
            "bow": (source: "models/g.glb")}, root_motion: "pelvis") }"#;
        let root = assets("animset", &[("content/h.animset.ron", set)]);
        let errors = errors(&build(&root));
        assert_eq!(errors.len(), 3, "{errors:#?}");
        assert!(errors[2].starts_with("content/h.animset.ron: animset `h`: a clip's source `models/g.glb` cannot be read ("), "{errors:#?}");
        assert_eq!(
            errors[..2],
            [
                "content/h.animset.ron: animset `h`: clips.slash -> `models/h.glb` has no animation `Slash`; it has: idle, lunge",
                "content/h.animset.ron: animset `h`: root_motion -> `models/h.glb` has no node `pelvis`; name its root bone",
            ]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_clips_phases_must_be_the_files() {
        // `lunge` carries `windup 0.2 active 0.1 recovery 0.3`; `idle` carries none and lasts 2 s.
        let set = r#"{ "h": (source: "models/h.glb", clips: {
            "lunge": (phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)]),
            "near": (name: "lunge", phases: [("windup", 0.205), ("active", 0.1), ("recovery", 0.3)]),
            "late": (name: "lunge", phases: [("windup", 0.25), ("active", 0.1), ("recovery", 0.3)]),
            "idle": (phases: [("loop", 2.0)]),
            "short": (name: "idle", phases: [("loop", 1.5)]),
        }) }"#;
        let root = assets("clip_phases", &[("content/h.animset.ron", set)]);
        assert_eq!(
            errors(&build(&root)),
            [
                "content/h.animset.ron: animset `h`: clips.late.phases (windup 0.25, active 0.1, recovery 0.3) are not the \
                 phases animation `lunge` of `models/h.glb` was timed to (windup 0.2, active 0.1, recovery 0.3); write the file's",
                "content/h.animset.ron: animset `h`: clips.short.phases last 1.5 s in all, animation `idle` of `models/h.glb` \
                 2.0 s; make them add up to the clip's length",
            ],
            "within half a tick is the same, as for a bake"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_layer_must_start_at_a_bone_of_the_model() {
        // The model has `root` and `hips` (the helper's skeleton).
        let set = r#"{ "h": (source: "models/h.glb", clips: {"idle": (), "lunge": ()},
            layers: {"upper": (root: "spine"), "legs": (root: "hips"), "blank": (root: "")},
            actions: {"light_1": (clip: "lunge", layer: "upper")}) }"#;
        let actions = r#"{ "light_1": (net_id: 1, phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)]) }"#;
        let root = assets("layers", &[("content/h.animset.ron", set), ("content/a.action.ron", actions)]);
        let errors = errors(&build(&root));
        assert_eq!(
            errors,
            [
                "content/h.animset.ron: animset `h`: layers.blank.root is empty; name the bone the layer starts at, \
                 e.g. `layers: {\"blank\": (root: \"spine\")}`",
                "content/h.animset.ron: animset `h`: layers.upper.root -> `models/h.glb` has no node `spine`; \
                 name the bone the layer starts at",
            ],
            "`hips` is a node of the file; an empty root is reported once, by the field check"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_cues_bone_must_be_a_node_of_the_model() {
        // The model has `root` and `hips` (the helper's skeleton); `models/g.glb` is not there at all.
        let set = r#"{ "h": (source: "models/h.glb", clips: {
            "idle": (cues: [(at: 0.5, cue: "breath", bone: "hips"), (at: 1.0, cue: "breath", bone: "tail"), (at: 1.5, cue: "breath")]),
            "lunge": (cues: [(at: 0.1, cue: "step", bone: "foot.L", strength: 0.5), (at: 0.2, cue: "step", bone: "")]),
        }) }"#;
        let root = assets("cue_bones", &[("content/h.animset.ron", set)]);
        let errors = errors(&build(&root));
        assert_eq!(
            errors,
            [
                "content/h.animset.ron: animset `h`: clips.lunge.cues[1].bone is empty; name a bone of the model \
                 (`bone: \"foot.L\"`) or leave it out",
                "content/h.animset.ron: animset `h`: clips.idle.cues[1].bone -> `models/h.glb` has no node `tail`; \
                 name a bone of the model",
                "content/h.animset.ron: animset `h`: clips.lunge.cues[0].bone -> `models/h.glb` has no node `foot.L`; \
                 name a bone of the model",
            ],
            "`hips` is a node of the file; an empty bone is reported once, by the field check"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_cue_on_a_clip_from_another_file_names_a_bone_of_the_model() {
        // `models/g.glb` holds clips for the same skeleton and a helper node `tail` the model lacks; the
        // model has `hips`, the gestures file does not. The bridge finds a cue's bone under the model.
        let set = r#"{ "h": (source: "models/h.glb", clips: {"idle": (),
            "bow": (source: "models/g.glb", name: "lunge", cues: [(at: 0.1, cue: "creak", bone: "hips"), (at: 0.2, cue: "swish", bone: "tail")]),
        }) }"#;
        let root = assets("cue_bones_file", &[("content/h.animset.ron", set)]);
        let mut gestures = std::fs::read(root.join("models/h.glb")).unwrap();
        let at = gestures.windows(6).position(|w| w == b"\"hips\"").unwrap();
        gestures[at..at + 6].copy_from_slice(b"\"tail\"");
        std::fs::write(root.join("models/g.glb"), gestures).unwrap();
        let errors = errors(&build(&root));
        assert_eq!(
            errors,
            ["content/h.animset.ron: animset `h`: clips.bow.cues[1].bone -> `models/h.glb` has no node `tail`; name a bone of the model"],
            "`hips` is the model's, whatever the clip's own file has"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_baked_curve_must_match_its_clip() {
        let actions = r#"{
            "fresh": (net_id: 1, phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)],
                displacement: [(0.2, 0.0), (0.3, 0.6)], bake: (animset: "h", clip: "lunge")),
            "stale": (net_id: 2, phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)],
                displacement: [(0.3, 0.5)], bake: (animset: "h", clip: "lunge")),
            "timed": (net_id: 3, phases: [("roll", 0.6)], bake: (animset: "h", clip: "lunge")),
            "unknown": (net_id: 4, phases: [("a", 0.6)], bake: (animset: "h", clip: "lunj")),
        }"#;
        let root = assets("check", &[("content/h.animset.ron", SET), ("content/a.action.ron", actions)]);
        let errors = errors(&build(&root));
        assert_eq!(
            errors,
            [
                "content/a.action.ron: action `stale`: displacement is stale: clip `lunge` of `models/h.glb` bakes to 2 keys \
                 (0.6 m forward); bake it again with `--bake-motion stale` (or the motion_bake tool)",
                "content/a.action.ron: action `timed`: bake: clip `lunge` of `models/h.glb` is timed to `windup 0.2 active 0.1 \
                 recovery 0.3`, the action to `roll 0.6`; make them match (the clip, or the action's phases) and bake again",
                "content/a.action.ron: action `unknown`: bake: animset `h` has no clip `lunj` (did you mean `lunge`?); clips: idle, lunge",
            ]
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn baking_writes_the_curve_and_checks_pass() {
        let actions = r#"{
    "light_1": (
        net_id: 1,
        phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)],
        bake: (animset: "h", clip: "lunge"),
    ),
    "still": (net_id: 2, phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)]),
}"#;
        let root = assets("write", &[("content/h.animset.ron", SET), ("content/a.action.ron", actions)]);
        let db = build(&root);
        assert_eq!(errors(&db).len(), 1, "no curve yet: stale {:#?}", errors(&db));
        let report = bake_db(&db, &root, None).unwrap();
        assert!(report.ok(), "{report}");
        assert_eq!(
            report.to_string(),
            "baked the displacement of 1 action at 60 Hz\n  light_1: clip `lunge`, 2 keys, 0.6 m forward by 0.3 s (sideways drift \
             0.01 m left out) -> content/a.action.ron"
        );
        let text = std::fs::read_to_string(root.join("content/a.action.ron")).unwrap();
        assert_eq!(text, actions.replace("        bake:", "        displacement: [(0.2, 0.0), (0.3, 0.6)],\n        bake:"));
        let db = build(&root);
        assert!(errors(&db).is_empty(), "{:#?}", errors(&db));
        let again = bake_db(&db, &root, Some("light_1")).unwrap();
        assert!(again.baked[0].ends_with("(unchanged, content/a.action.ron)"), "{again}");
        assert_eq!(std::fs::read_to_string(root.join("content/a.action.ron")).unwrap(), text, "the same bytes");
        let e = bake_db(&db, &root, Some("still")).unwrap_err();
        assert!(e.starts_with("action `still` has no `bake`"), "{e}");
        assert_eq!(bake_db(&db, &root, Some("ligth_1")).unwrap_err(), "no action `ligth_1` (did you mean `light_1`?)");
        let _ = std::fs::remove_dir_all(root);
    }
}
