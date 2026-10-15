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
    let root = canonical(&root).map_err(|e| format!("cannot open {}: {e}", root.display()))?;
    let parent = folders.iter().fold(root.clone(), |p, f| p.join(f));
    // Check the part that exists before creating the rest: `create_dir_all` would follow a
    // symlink out of the folder.
    let mut existing = parent.as_path();
    while std::fs::symlink_metadata(existing).is_err() {
        existing = existing.parent().unwrap_or(root.as_path());
    }
    let real = canonical(existing).map_err(|_| refuse("goes through a broken symlink"))?;
    if !real.starts_with(&root) {
        return Err(refuse("leads outside through a symlink"));
    }
    std::fs::create_dir_all(&parent).map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    let path = canonical(&parent).map_err(|e| format!("cannot open {}: {e}", parent.display()))?.join(name);
    // Writing (or removing) through a symlink would touch its target.
    if std::fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
        return Err(refuse("is a symlink"));
    }
    Ok(path)
}

/// `path` made absolute with every link followed, written the way people write it: on Windows
/// without the `\\?\` that [`Path::canonicalize`] puts in front (`\\?\C:\game` → `C:\game`,
/// `\\?\UNC\server\share` → `\\server\share`), so the answers of tools show a path one can paste.
pub fn canonical(path: &Path) -> std::io::Result<PathBuf> {
    path.canonicalize().map(|p| without_verbatim(&p))
}

/// `path` without Windows' verbatim prefix, where the plain form means the same: a drive or a
/// share. Other paths are left as they are.
pub fn without_verbatim(path: &Path) -> PathBuf {
    let text = path.as_os_str().to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        // `C:\…`: a drive letter, a colon, a separator.
        Some(rest) if rest.len() >= 3 && rest.as_bytes()[0].is_ascii_alphabetic() && &rest[1..3] == r":\" => PathBuf::from(rest),
        _ => path.to_path_buf(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verbatim_prefix_goes_where_the_plain_path_means_the_same() {
        let plain = |p: &str| without_verbatim(Path::new(p)).display().to_string();
        assert_eq!(plain(r"\\?\E:\game\replays\a.json"), r"E:\game\replays\a.json");
        assert_eq!(plain(r"\\?\UNC\server\share\a.json"), r"\\server\share\a.json");
        // A device or a volume by its GUID has no plain form.
        assert_eq!(plain(r"\\?\Volume{1234}\a.json"), r"\\?\Volume{1234}\a.json");
        assert_eq!(plain("/home/me/game/replays/a.json"), "/home/me/game/replays/a.json");
    }

    fn base(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ai_core_paths_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        canonical(&dir).unwrap()
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
