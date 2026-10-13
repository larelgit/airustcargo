//! Files the game writes when a tool asks: only inside the game's own folders.
//!
//! `replay_save` and crash replays write into `replays/`, `shot` into `shots/`, both
//! relative to the game's working directory. An absolute path, `..`, or a symlink that
//! leads out of the folder is refused with an example of a good path, so a tool call
//! can never overwrite (or, for `shot`, delete) a file elsewhere on the machine.

use std::path::{Component, Path, PathBuf};

/// `requested` (relative, starting with `dir`: `replays/boss.json`) as an absolute path
/// inside `<working directory>/<dir>`, with its parent folders created. `example` is a
/// good path to show in the error.
pub fn inside(dir: &str, requested: &str, example: &str) -> Result<PathBuf, String> {
    let base = std::env::current_dir().map_err(|e| format!("no working directory: {e}"))?;
    inside_of(&base, dir, requested, example)
}

/// [`inside`], relative to `base` instead of the working directory.
pub fn inside_of(base: &Path, dir: &str, requested: &str, example: &str) -> Result<PathBuf, String> {
    let refuse = |why: &str| format!("path must be inside {dir}/, e.g. {example} (`{requested}` {why})");
    let mut parts = Vec::new();
    for part in Path::new(requested).components() {
        match part {
            Component::Normal(p) => parts.push(p),
            Component::CurDir => {}
            Component::ParentDir => return Err(refuse("goes up with `..`")),
            Component::RootDir | Component::Prefix(_) => return Err(refuse("is absolute")),
        }
    }
    let Some((first, rest)) = parts.split_first().filter(|(first, _)| **first == dir) else {
        return Err(refuse(&format!("is outside {dir}/")));
    };
    let Some((name, folders)) = rest.split_last() else {
        return Err(refuse("names no file"));
    };

    let root = base.join(first);
    std::fs::create_dir_all(&root).map_err(|e| format!("cannot create {}: {e}", root.display()))?;
    // The folder itself may be a symlink (replays kept on another disk); what is inside
    // it must stay inside it.
    let root = root.canonicalize().map_err(|e| format!("cannot open {}: {e}", root.display()))?;
    let parent = folders.iter().fold(root.clone(), |p, f| p.join(f));
    // Check the part that exists before creating the rest: `create_dir_all` would follow a
    // symlink out of the folder.
    let mut existing = parent.as_path();
    while std::fs::symlink_metadata(existing).is_err() {
        existing = existing.parent().unwrap_or(root.as_path());
    }
    let real = existing.canonicalize().map_err(|_| refuse("goes through a broken symlink"))?;
    if !real.starts_with(&root) {
        return Err(refuse("leads outside through a symlink"));
    }
    std::fs::create_dir_all(&parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    let path = parent.canonicalize().map_err(|e| format!("cannot open {}: {e}", parent.display()))?.join(name);
    // Writing (or removing) through a symlink would touch its target.
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(refuse("is a symlink"));
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn base(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ai_core_paths_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    #[test]
    fn paths_stay_inside_their_folder() {
        let base = base("inside");
        let replay = |p: &str| inside_of(&base, "replays", p, "replays/boss-fight.json");

        assert_eq!(replay("replays/a.json").unwrap(), base.join("replays/a.json"));
        assert_eq!(replay("./replays/./b.json").unwrap(), base.join("replays/b.json"));
        assert_eq!(replay("replays/fights/boss/c.json").unwrap(), base.join("replays/fights/boss/c.json"));
        assert!(base.join("replays/fights/boss").is_dir(), "nested folders are created");

        for (bad, why) in [
            ("/tmp/a.json", "is absolute"),
            ("replays/../a.json", "goes up with `..`"),
            ("replays/fights/../../a.json", "goes up with `..`"),
            ("a.json", "is outside replays/"),
            ("shots/a.json", "is outside replays/"),
            ("replays", "names no file"),
        ] {
            let err = replay(bad).unwrap_err();
            assert!(err.starts_with("path must be inside replays/, e.g. replays/boss-fight.json"), "{err}");
            assert!(err.contains(why), "{bad}: {err}");
        }
        assert!(!base.join("a.json").exists());
        let _ = std::fs::remove_dir_all(base);
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_do_not_lead_out() {
        use std::os::unix::fs::symlink;
        let base = base("symlinks");
        let outside = base.join("elsewhere");
        std::fs::create_dir_all(&outside).unwrap();
        std::fs::create_dir_all(base.join("shots")).unwrap();
        symlink(&outside, base.join("shots/out")).unwrap();
        symlink(outside.join("x.png"), base.join("shots/link.png")).unwrap();
        let shot = |p: &str| inside_of(&base, "shots", p, "shots/boss.png");

        assert!(shot("shots/out/x.png").unwrap_err().contains("leads outside through a symlink"));
        assert!(shot("shots/out/new/x.png").unwrap_err().contains("leads outside through a symlink"));
        assert!(!outside.join("new").exists(), "nothing is created outside");
        assert!(shot("shots/link.png").unwrap_err().contains("is a symlink"));
        assert!(shot("shots/fine.png").is_ok());
        let _ = std::fs::remove_dir_all(base);
    }
}
