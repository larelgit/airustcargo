//! `AGENTS.md` is a short core with an index; the detail lives in `docs/agents/*.md`. Keeps the two
//! in step: every topic file is linked from the index, and every link in the core exists.
//!
//! The skills keep in step too: each `.agents/skills/<name>/SKILL.md` (the Agent Skills format, where
//! Codex reads it) has a name and a description an agent can load, an entry in `.claude/skills/` with
//! the same two that sends Claude Code to it, a row in the index `.agents/skills/README.md` that
//! `AGENTS.md` points to, and only paths that exist.

use std::collections::{BTreeMap, BTreeSet};
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

/// The skills, by name: the folders of `.agents/skills/` that hold a `SKILL.md`.
fn skills() -> BTreeMap<String, PathBuf> {
    let dir = root().join(".agents/skills");
    let skills: BTreeMap<String, PathBuf> = fs::read_dir(&dir)
        .expect(".agents/skills")
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.join("SKILL.md").is_file())
        .map(|path| (path.file_name().unwrap().to_string_lossy().into_owned(), path))
        .collect();
    assert!(!skills.is_empty(), "no skills in {}", dir.display());
    skills
}

/// The front matter of a `SKILL.md` (`---`, `key: value` lines, `---`) and the body after it.
fn front_matter(file: &Path) -> (BTreeMap<String, String>, String) {
    let text = fs::read_to_string(file).unwrap_or_else(|e| panic!("{}: {e}", file.display()));
    let rest = text.strip_prefix("---\n").unwrap_or_else(|| panic!("{}: the first line must be `---`", file.display()));
    let (front, body) = rest.split_once("\n---\n").unwrap_or_else(|| panic!("{}: the front matter has no closing `---`", file.display()));
    let fields = front
        .lines()
        .map(|line| {
            let (key, value) = line.split_once(": ").unwrap_or_else(|| panic!("{}: `{line}` is not `key: value`", file.display()));
            (key.to_owned(), value.to_owned())
        })
        .collect();
    (fields, body.to_owned())
}

/// Every Markdown file under `dir`, recursively.
fn markdown_in(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(markdown_in(&path));
        } else if path.extension().is_some_and(|e| e == "md") {
            files.push(path);
        }
    }
    files.sort();
    files
}

#[test]
fn every_skill_has_a_name_and_a_description_an_agent_can_load() {
    for (name, dir) in skills() {
        let file = dir.join("SKILL.md");
        let (fields, body) = front_matter(&file);
        assert_eq!(fields.get("name").map(String::as_str), Some(name.as_str()), "{}: `name` must be the folder's name", file.display());
        let well_formed = !name.is_empty()
            && name.len() <= 64
            && name.split('-').all(|word| !word.is_empty() && word.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit()));
        assert!(well_formed, "skill `{name}`: lowercase letters, digits and single hyphens, at most 64");
        let description = fields.get("description").unwrap_or_else(|| panic!("{}: no `description`", file.display()));
        assert!(
            !description.is_empty() && description.chars().count() <= 1024,
            "{}: the description is what agents choose a skill by; 1 to 1024 characters (is {})",
            file.display(),
            description.chars().count()
        );
        // A plain YAML scalar: `: ` or ` #` would end it early, and tags are not allowed in it.
        for bad in [": ", " #", "<", ">"] {
            assert!(!description.contains(bad), "{}: the description must not contain `{bad}`", file.display());
        }
        let lines = fs::read_to_string(&file).unwrap().lines().count();
        assert!(lines <= 500, "{}: {lines} lines; keep a skill under 500 and move detail to references/", file.display());
        assert!(!body.trim().is_empty(), "{}: no instructions after the front matter", file.display());
    }
}

#[test]
fn every_skill_has_a_claude_code_entry_that_matches_it() {
    let skills = skills();
    for (name, dir) in &skills {
        let entry = root().join(".claude/skills").join(name).join("SKILL.md");
        assert!(entry.is_file(), "{} is missing: Claude Code finds skills only in .claude/skills/", entry.display());
        let (theirs, body) = front_matter(&entry);
        let (ours, _) = front_matter(&dir.join("SKILL.md"));
        assert_eq!(theirs, ours, "{}: name and description must be those of .agents/skills/{name}/SKILL.md", entry.display());
        let canonical = format!(".agents/skills/{name}/SKILL.md");
        assert!(body.contains(&canonical), "{}: the entry must send the agent to {canonical}", entry.display());
    }
    for entry in fs::read_dir(root().join(".claude/skills")).expect(".claude/skills") {
        let name = entry.unwrap().file_name().to_string_lossy().into_owned();
        assert!(skills.contains_key(&name), ".claude/skills/{name} has no .agents/skills/{name}: write the skill there, where every agent reads it");
    }
}

#[test]
fn the_skills_index_lists_every_skill_and_agents_md_points_to_it() {
    let skills = skills();
    let index = fs::read_to_string(root().join(".agents/skills/README.md")).expect(".agents/skills/README.md");
    for name in skills.keys() {
        assert!(index.contains(&format!("| `{name}` |")), ".agents/skills/README.md has no row for the skill `{name}`: agents choose a skill from that index");
    }
    for row in index.lines().filter_map(|line| line.strip_prefix("| `")) {
        let name = &row[..row.find('`').expect("a closed name")];
        assert!(skills.contains_key(name), ".agents/skills/README.md lists `{name}`, which has no .agents/skills/{name}/SKILL.md");
    }
    let core = fs::read_to_string(root().join("AGENTS.md")).expect("AGENTS.md");
    assert!(core.contains(".agents/skills/README.md"), "AGENTS.md must point to the skills index: agents without skills would never hear of them");
}

/// The paths from the repository root that `text` names: words that start with one of these folders,
/// up to the first character a path does not have. A word that runs into a placeholder
/// (`crates/<crate>/…`, `net_*.rs`) names no single path and is left out.
fn repository_paths(text: &str) -> Vec<String> {
    const ROOTS: [&str; 9] = [".agents/", ".claude/", ".github/", ".config/", ".cargo/", "docs/", "crates/", "template/", "tools/"];
    let path_char = |c: char| c.is_ascii_alphanumeric() || "_./-".contains(c);
    let mut found = Vec::new();
    for (i, _) in text.char_indices() {
        let boundary = text[..i].chars().next_back().is_none_or(|c| c.is_whitespace() || "`'\"([".contains(c));
        if !boundary || !ROOTS.iter().any(|r| text[i..].starts_with(r)) {
            continue;
        }
        let end = text[i..].find(|c: char| !path_char(c)).map_or(text.len(), |n| i + n);
        if text[end..].starts_with(['<', '*', '{', '…', '$', '[']) {
            continue;
        }
        found.push(text[i..end].trim_end_matches('.').to_owned());
    }
    found
}

#[test]
fn every_path_a_skill_names_exists() {
    let root = root();
    let mut files = markdown_in(&root.join(".agents/skills"));
    files.extend(markdown_in(&root.join(".claude/skills")));
    let mut missing = Vec::new();
    for file in &files {
        let text = fs::read_to_string(file).unwrap();
        for path in repository_paths(&text) {
            if !root.join(&path).exists() {
                missing.push(format!("{} names {path}", file.strip_prefix(&root).unwrap().display()));
            }
        }
    }
    assert!(missing.is_empty(), "paths that do not exist (a file moved? fix the skill):\n{}", missing.join("\n"));

    // Every reference and script of a skill is named by a SKILL.md, so an agent can reach it.
    let skills = skills();
    let instructions: Vec<String> = skills.values().map(|dir| fs::read_to_string(dir.join("SKILL.md")).unwrap()).collect();
    for (name, dir) in &skills {
        for folder in ["references", "scripts"] {
            let Ok(entries) = fs::read_dir(dir.join(folder)) else { continue };
            // Files only: a run of a script may leave a cache folder beside it.
            for entry in entries.map(Result::unwrap).filter(|e| e.file_type().is_ok_and(|t| t.is_file())) {
                let file = entry.file_name().to_string_lossy().into_owned();
                let named = format!(".agents/skills/{name}/{folder}/{file}");
                assert!(instructions.iter().any(|text| text.contains(&named)), "{named} is named by no SKILL.md: an agent would never open it");
            }
        }
    }
}
