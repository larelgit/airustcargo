//! The reader guard (the voxel design, 6.3): a voxel field is no collider and has no box, so
//! code that reaches a collider or a collider's box is wrong on a voxel world unless a field
//! cannot be what it looks at. The scan looks for `.collider(`, `.colliders()`, `.by_id(`,
//! `.aabb_min`, `.aabb_max`, the same methods called as `PhysicsWorld::…`, and the `Collider`
//! component read by type (`&Collider`, `&'static Collider`, `&mut Collider`, `get::<Collider>`):
//! every such line in `crates/*/src` outside `shards_motion/src/physics/` is listed with its reason
//! in `tests/readers.allow`, once for each time the same line is in the file. This test fails on
//! a line that is not listed (or is there more times than listed) and on a listed row that
//! matches no line. The `clippy.toml` entries for `PhysicsWorld::collider`, `PhysicsWorld::by_id`
//! and `ColliderInfo::bounds` catch the same readers by type, in tests and examples too.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Found anywhere in a line.
const PATTERNS: [&str; 9] = [
    "get::<Collider>",
    ".collider(",
    ".colliders()",
    ".by_id(",
    ".aabb_min",
    ".aabb_max",
    "PhysicsWorld::collider",
    "PhysicsWorld::colliders",
    "PhysicsWorld::by_id",
];

/// The `Collider` component by type: found when the name ends there (not `&ColliderId`,
/// `&ColliderInfo`).
const TYPES: [&str; 3] = ["&Collider", "&'static Collider", "&mut Collider"];

fn sources(dir: &Path, out: &mut Vec<PathBuf>) {
    let mut entries: Vec<_> = fs::read_dir(dir).unwrap().map(|e| e.unwrap().path()).collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            sources(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Whether `text` holds `name` as a whole word at its end (the next character, if any, is not
/// part of an identifier).
fn names_type(text: &str, name: &str) -> bool {
    text.match_indices(name).any(|(at, _)| {
        text[at + name.len()..].chars().next().is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
    })
}

fn reads(text: &str) -> bool {
    PATTERNS.iter().any(|p| text.contains(p)) || TYPES.iter().any(|t| names_type(text, t))
}

#[test]
fn the_scan_tells_a_component_from_its_id() {
    assert!(reads("q: Query<(&Transform, &Collider)>,") && reads("w.get::<Collider>(e)") && reads("Query<&mut Collider, With<A>>"));
    assert!(!reads("ids: Query<&ColliderId>") && !reads("fn f(c: &ColliderInfo)") && !reads("Option<&'static ColliderId>"));
    assert!(reads("physics.by_id(id)") && reads("PhysicsWorld::collider(&w, e)") && !reads("session.lobby_id()"));
}

#[test]
fn every_reader_of_a_collider_box_is_listed_with_its_reason() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let root = crates.parent().unwrap();
    // (file, the line's text, trimmed) and how many times the file has it.
    let mut found: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut names: Vec<_> = fs::read_dir(crates).unwrap().map(|e| e.unwrap().path()).collect();
    names.sort();
    for krate in names {
        let src = krate.join("src");
        if !src.is_dir() {
            continue;
        }
        let mut files = Vec::new();
        sources(&src, &mut files);
        for file in files {
            let name = file.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
            if name.starts_with("crates/shards_motion/src/physics/") {
                continue;
            }
            for line in fs::read_to_string(&file).unwrap().lines() {
                let text = line.trim();
                if !text.starts_with("//") && reads(text) {
                    *found.entry((name.clone(), text.to_string())).or_default() += 1;
                }
            }
        }
    }
    assert!(found.len() > 5, "the scan found {} lines: it is not looking where the code is", found.len());

    let allow = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/readers.allow")).unwrap();
    let mut listed: BTreeMap<(String, String), usize> = BTreeMap::new();
    for row in allow.lines().map(str::trim).filter(|r| !r.is_empty() && !r.starts_with('#')) {
        let parts: Vec<&str> = row.splitn(3, " :: ").collect();
        assert!(parts.len() == 3 && !parts[2].trim().is_empty(), "a row is `file :: line :: reason`: {row}");
        *listed.entry((parts[0].to_string(), parts[1].to_string())).or_default() += 1;
    }
    let new: Vec<_> = found
        .iter()
        .filter(|(k, n)| listed.get(*k).is_none_or(|m| m < n))
        .map(|(k, n)| format!("{} :: {} (in the file {n} times, listed {})", k.0, k.1, listed.get(k).copied().unwrap_or(0)))
        .collect();
    let stale: Vec<_> = listed
        .iter()
        .filter(|(k, n)| found.get(*k).is_none_or(|m| m < n))
        .map(|(k, n)| format!("{} :: {} (listed {n} times, in the file {})", k.0, k.1, found.get(k).copied().unwrap_or(0)))
        .collect();
    assert!(
        new.is_empty(),
        "reads a collider or its box, which a voxel field does not have: ask PhysicsWorld::extent_at, top_under or \
         solid_span, or add the line to tests/readers.allow (a row each time the file has it) with the reason a field \
         cannot be what it reads:\n{new:#?}"
    );
    assert!(stale.is_empty(), "rows of tests/readers.allow that match no line any more (remove them):\n{stale:#?}");
}
