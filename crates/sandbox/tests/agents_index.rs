//! `AGENTS.md` is a short core with an index; the detail lives in `docs/agents/*.md`. Keeps the two
//! in step: every topic file is linked from the index, and every link in the core exists.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The `docs/agents/<name>.md` targets of the Markdown links in `text`.
fn topic_links(text: &str) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut rest = text;
    while let Some(i) = rest.find("](docs/agents/") {
        let tail = &rest[i + 2..];
        let end = tail.find(')').expect("a link is closed");
        found.insert(tail[..end].to_string());
        rest = &tail[end..];
    }
    found
}

#[test]
fn every_topic_file_is_linked_from_the_agents_index_and_every_link_exists() {
    let root = root();
    let core = fs::read_to_string(root.join("AGENTS.md")).expect("AGENTS.md");
    let linked = topic_links(&core);
    assert!(!linked.is_empty(), "AGENTS.md has no index links");

    let mut files = BTreeSet::new();
    for entry in fs::read_dir(root.join("docs/agents")).expect("docs/agents") {
        let path = entry.unwrap().path();
        if path.extension().is_some_and(|e| e == "md") {
            files.insert(format!("docs/agents/{}", path.file_name().unwrap().to_string_lossy()));
        }
    }
    for f in &files {
        assert!(linked.contains(f), "{f} is not linked from the index in AGENTS.md");
    }
    for l in &linked {
        assert!(root.join(l).is_file(), "AGENTS.md links {l}, which does not exist");
    }
}

#[test]
fn the_agents_core_stays_short() {
    let core = fs::read_to_string(root().join("AGENTS.md")).expect("AGENTS.md");
    assert!(
        core.len() <= 9 * 1024,
        "AGENTS.md is {} bytes: move detail to docs/agents/*.md and keep the core short",
        core.len()
    );
}
