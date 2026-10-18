//! The reader guard (the voxel design, 6.3): a voxel field is no collider and has no box, so
//! code that reaches a collider or a collider's box (`.collider(`, `.colliders()`, `.aabb_min`,
//! `.aabb_max`) is wrong on a voxel world unless a field cannot be what it looks at. Every such
//! line in `crates/*/src` outside `shards_motion/src/physics/` is listed with its reason in
//! `tests/readers.allow`; this test fails on a line that is not listed and on a listed row that
//! matches no line. The `clippy.toml` entries for `PhysicsWorld::collider` and
//! `ColliderInfo::bounds` catch the same readers by type, in tests and examples too.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

const PATTERNS: [&str; 4] = [".collider(", ".colliders()", ".aabb_min", ".aabb_max"];

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

#[test]
fn every_reader_of_a_collider_box_is_listed_with_its_reason() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap();
    let root = crates.parent().unwrap();
    let mut found = BTreeSet::new();
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
                if !text.starts_with("//") && PATTERNS.iter().any(|p| text.contains(p)) {
                    found.insert((name.clone(), text.to_string()));
                }
            }
        }
    }
    assert!(found.len() > 5, "the scan found {} lines: it is not looking where the code is", found.len());

    let allow = fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/readers.allow")).unwrap();
    let mut listed = BTreeSet::new();
    for row in allow.lines().map(str::trim).filter(|r| !r.is_empty() && !r.starts_with('#')) {
        let parts: Vec<&str> = row.splitn(3, " :: ").collect();
        assert!(parts.len() == 3 && !parts[2].trim().is_empty(), "a row is `file :: line :: reason`: {row}");
        listed.insert((parts[0].to_string(), parts[1].to_string()));
    }
    let new: Vec<_> = found.difference(&listed).collect();
    let stale: Vec<_> = listed.difference(&found).collect();
    assert!(
        new.is_empty(),
        "reads a collider or its box, which a voxel field does not have: ask PhysicsWorld::extent_at, top_under or \
         solid_span, or add the line to tests/readers.allow with the reason a field cannot be what it reads:\n{new:#?}"
    );
    assert!(stale.is_empty(), "rows of tests/readers.allow that match no line any more (remove them):\n{stale:#?}");
}
