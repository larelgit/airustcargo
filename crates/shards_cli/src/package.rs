//! `shards package`: the game built for players, then checked the way a clean machine runs it.
//!
//! The game (the package in this folder, `--manifest-path` or `--package`) is built with the
//! profile `dist` when its workspace defines one (else `release`) and, when the game has a
//! feature `dist`, with `--no-default-features --features dist`: no `shards` server, no hot
//! reload, no developer keys. On Windows (MSVC) the C runtime is linked in (`+crt-static`: no
//! Visual C++ redistributable to install) and `assets/icon.png` becomes the program's icon.
//! The program and `assets/` go to `dist/<name>-<version>-<os>-<arch>/` beside the game's
//! `Cargo.toml`, with the dynamic libraries build scripts link beside the program (the Steamworks
//! SDK's `steam_api64.dll`, feature `steam_sdk`) and never a `steam_appid.txt`. Then that folder is
//! checked: on Windows the DLLs it imports are the system's or shipped beside it;
//! a copy of it in a temporary folder, started with an environment as bare as a player's (no
//! cargo, no `BEVY_ASSET_ROOT`), passes `--check-content` and `--smoke`, and writes nothing into
//! its own folder.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use serde_json::Value;

use crate::say;

/// What the command line asked for.
pub struct Options {
    pub manifest_path: Option<PathBuf>,
    pub package: Option<String>,
    pub bin: Option<String>,
    pub profile: Option<String>,
    pub out: Option<PathBuf>,
    pub smoke: u64,
    pub check: bool,
}

/// The icon file, in the game's asset folder (the engine reads the same one for the window).
const ICON: &str = "icon.png";

/// The asset folder, beside the game's `Cargo.toml`, and beside the program in the package.
const ASSETS: &str = "assets";

/// How long the package's program may take to check its content or run its smoke test.
const RUN_TIMEOUT: Duration = Duration::from_secs(300);

/// The game as `cargo metadata` describes it.
struct Game {
    name: String,
    version: String,
    manifest: PathBuf,
    dir: PathBuf,
    bin: String,
    /// It has a feature `dist`.
    dist_feature: bool,
    target_dir: PathBuf,
    workspace_root: PathBuf,
}

/// Exit 0 (built, every check passed), 1 (a check failed: printed) or an error (2: the build
/// failed, no such package, …).
pub fn run(options: &Options) -> Result<ExitCode, String> {
    let game = find_game(options)?;
    let profile = options.profile.clone().unwrap_or_else(|| default_profile(&game.workspace_root));
    let Built { exe, libraries } = build(&game, &profile)?;
    let folder = options.out.clone().unwrap_or_else(|| game.dir.join("dist")).join(format!(
        "{}-{}-{}-{}",
        game.name,
        game.version,
        std::env::consts::OS,
        std::env::consts::ARCH
    ));
    let placed = place(&game, &exe, &libraries, &folder)?;
    say(format!("package: {} ({})", folder.display(), placed));
    if !options.check {
        return Ok(ExitCode::SUCCESS);
    }
    let program = folder.join(exe.file_name().ok_or("the build named no program")?);
    let mut passed = true;
    if cfg!(windows) {
        passed &= check_imports(&program)?;
    }
    passed &= check_clean_run(&folder, &program, options.smoke)?;
    Ok(if passed { ExitCode::SUCCESS } else { ExitCode::from(crate::exit::TOOL_ERROR) })
}

fn cargo(dir: &Path) -> Command {
    let mut cargo = Command::new("cargo");
    cargo.current_dir(dir);
    // Run by `cargo shards`, this process got the toolchain of the engine's workspace from
    // rustup; the game's folder may pin its own (`rust-toolchain.toml`).
    if std::env::var_os("CARGO").is_some() {
        cargo.env_remove("RUSTUP_TOOLCHAIN");
    }
    cargo
}

fn find_game(options: &Options) -> Result<Game, String> {
    let here = std::env::current_dir().map_err(|e| format!("this folder: {e}"))?;
    let mut metadata = cargo(&here);
    metadata.args(["metadata", "--format-version", "1", "--no-deps"]);
    if let Some(manifest) = &options.manifest_path {
        metadata.arg("--manifest-path").arg(manifest);
    }
    let metadata: Value = serde_json::from_slice(&output(&mut metadata, "cargo metadata")?)
        .map_err(|e| format!("cargo metadata: {e}"))?;
    let packages = metadata["packages"].as_array().cloned().unwrap_or_default();
    let with_bins: Vec<&Value> = packages.iter().filter(|p| !bins(p).is_empty()).collect();
    let package = match &options.package {
        Some(name) => packages.iter().find(|p| p["name"] == name.as_str()).ok_or_else(|| {
            format!("no package `{name}` in this workspace; packages with a program: {}", names(&with_bins))
        })?,
        None => {
            // The package whose folder this is (or holds this one).
            let mut locate = cargo(&here);
            locate.args(["locate-project", "--message-format", "plain"]);
            if let Some(manifest) = &options.manifest_path {
                locate.arg("--manifest-path").arg(manifest);
            }
            let located = String::from_utf8_lossy(&output(&mut locate, "cargo locate-project")?).trim().to_owned();
            packages.iter().find(|p| p["manifest_path"].as_str().is_some_and(|m| same_file(m, &located))).ok_or_else(|| {
                format!(
                    "{located} is a workspace without a package of its own: name the game with --package (packages with a program: {})",
                    names(&with_bins)
                )
            })?
        }
    };
    let name = package["name"].as_str().unwrap_or_default().to_owned();
    let package_bins = bins(package);
    let bin = match (&options.bin, package_bins.as_slice()) {
        (Some(bin), _) if package_bins.contains(bin) => bin.clone(),
        (Some(bin), _) => return Err(format!("`{name}` has no program `{bin}`; it has: {}", list_or_none(&package_bins))),
        (None, [only]) => only.clone(),
        (None, []) => return Err(format!("`{name}` has no program (`[[bin]]`) to package")),
        (None, _) => match package["default_run"].as_str() {
            Some(run) => run.to_owned(),
            None => return Err(format!("`{name}` has several programs: name one with --bin ({})", package_bins.join(", "))),
        },
    };
    let manifest = PathBuf::from(package["manifest_path"].as_str().unwrap_or_default());
    let dir = manifest.parent().map(Path::to_path_buf).ok_or("the package's manifest has no folder")?;
    Ok(Game {
        name,
        version: package["version"].as_str().unwrap_or("0.0.0").to_owned(),
        dir,
        manifest,
        bin,
        dist_feature: package["features"].get("dist").is_some(),
        target_dir: PathBuf::from(metadata["target_directory"].as_str().unwrap_or("target")),
        workspace_root: PathBuf::from(metadata["workspace_root"].as_str().unwrap_or(".")),
    })
}

fn bins(package: &Value) -> Vec<String> {
    let targets = package["targets"].as_array().map(Vec::as_slice).unwrap_or_default();
    targets
        .iter()
        .filter(|t| t["kind"].as_array().is_some_and(|k| k.iter().any(|k| k == "bin")))
        .filter_map(|t| t["name"].as_str().map(str::to_owned))
        .collect()
}

fn names(packages: &[&Value]) -> String {
    list_or_none(&packages.iter().filter_map(|p| p["name"].as_str().map(str::to_owned)).collect::<Vec<_>>())
}

fn list_or_none(items: &[String]) -> String {
    if items.is_empty() { "none".to_owned() } else { items.join(", ") }
}

/// Whether two paths name one file (`cargo` writes `\\?\` and drive letters as it likes).
fn same_file(a: &str, b: &str) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => a == b,
    }
}

/// `dist` when the workspace's `Cargo.toml` defines that profile, else `release`.
fn default_profile(workspace_root: &Path) -> String {
    let text = std::fs::read_to_string(workspace_root.join("Cargo.toml")).unwrap_or_default();
    // The table header, however it is spelled: `[profile.dist]  # for players`, `[ profile."dist" ]`.
    let header = |line: &str| {
        let before_comment = line.split('#').next().unwrap_or_default();
        before_comment.chars().filter(|c| !c.is_whitespace() && *c != '"').collect::<String>()
    };
    let defined = text.lines().any(|line| header(line) == "[profile.dist]");
    if defined { "dist" } else { "release" }.to_owned()
}

/// What a build made: the program, and the dynamic libraries it needs beside it.
struct Built {
    exe: PathBuf,
    /// The libraries build scripts link dynamically (`dylib=`), found where they link them from:
    /// the Steamworks SDK's `steam_api64.dll` (feature `steam_sdk` of `shards`).
    libraries: Vec<PathBuf>,
}

/// Builds the game's program, says how, and returns it with the libraries it needs.
fn build(game: &Game, profile: &str) -> Result<Built, String> {
    let mut cargo = cargo(&game.dir);
    let mut how = vec![format!("profile {profile}")];
    let windows = cfg!(all(windows, target_env = "msvc"));
    if windows {
        // `cargo rustc`: the icon is an argument for the program's link alone.
        cargo.arg("rustc");
    } else {
        cargo.arg("build");
    }
    cargo.arg("--manifest-path").arg(&game.manifest).args(["--bin", &game.bin, "--profile", profile]);
    cargo.args(["--message-format", "json-render-diagnostics"]);
    if game.dist_feature {
        cargo.args(["--no-default-features", "--features", "dist"]);
        how.push("feature dist".into());
    } else {
        how.push("default features (the game has no feature `dist`)".into());
    }
    if windows {
        // With `--target`, the flags reach the game's crates and not build scripts.
        cargo.args(["--target", &format!("{}-pc-windows-msvc", std::env::consts::ARCH)]);
        add_rustflag(&mut cargo, "-Ctarget-feature=+crt-static");
        how.push("C runtime linked in".into());
        let icon = game.dir.join(ASSETS).join(ICON);
        if icon.is_file() {
            let png = std::fs::read(&icon).map_err(|e| format!("{}: {e}", icon.display()))?;
            let res = icon_res(&png).map_err(|e| format!("{}: {e}", icon.display()))?;
            // Named after its contents: a new icon is a new link argument, so the program is linked again.
            let file = game.target_dir.join("shards-package").join(format!("icon-{:016x}.res", fnv1a(&res)));
            std::fs::create_dir_all(file.parent().unwrap_or(&game.target_dir)).map_err(|e| format!("{}: {e}", file.display()))?;
            std::fs::write(&file, &res).map_err(|e| format!("{}: {e}", file.display()))?;
            cargo.arg("--").arg(format!("-Clink-arg={}", file.display()));
            how.push(format!("icon {ASSETS}/{ICON}"));
        } else {
            how.push(format!("no {ASSETS}/{ICON}: the default icon"));
        }
    }
    say(format!("building {} {} ({})", game.name, game.bin, how.join(", ")));
    let mut child = cargo.stdout(Stdio::piped()).spawn().map_err(|e| format!("cargo: {e}"))?;
    let mut exe = None;
    let mut libraries = Vec::new();
    for line in BufReader::new(child.stdout.take().ok_or("cargo: no output")?).lines() {
        let Ok(message) = serde_json::from_str::<Value>(&line.unwrap_or_default()) else { continue };
        for library in dynamic_libraries(&message, std::env::consts::OS) {
            if !libraries.contains(&library) {
                libraries.push(library);
            }
        }
        if message["reason"] == "compiler-artifact" && message["target"]["name"] == game.bin.as_str() {
            exe = message["executable"].as_str().map(PathBuf::from).or(exe);
        }
    }
    let status = child.wait().map_err(|e| format!("cargo: {e}"))?;
    if !status.success() {
        return Err(format!("the build failed ({status}): see cargo's messages above"));
    }
    let exe = exe.ok_or("cargo built no program")?;
    say(format!("built: {}", exe.display()));
    Ok(Built { exe, libraries })
}

/// The libraries a `build-script-executed` message links dynamically (`dylib=steam_api64`), as the
/// files of `os` found in the paths it links from; none for any other message.
fn dynamic_libraries(message: &Value, os: &str) -> Vec<PathBuf> {
    if message["reason"] != "build-script-executed" {
        return Vec::new();
    }
    let strings = |key: &str| -> Vec<String> { message[key].as_array().into_iter().flatten().filter_map(|v| v.as_str().map(str::to_owned)).collect() };
    let paths: Vec<PathBuf> = strings("linked_paths")
        .iter()
        .map(|p| {
            let kinds = ["native", "dependency", "crate", "framework", "all"];
            PathBuf::from(p.split_once('=').filter(|(kind, _)| kinds.contains(kind)).map_or(p.as_str(), |(_, path)| path))
        })
        .collect();
    let mut found = Vec::new();
    for lib in strings("linked_libs") {
        // `dylib=name`, or a bare name (dynamic is rustc's default); not `static=`, `framework=`.
        let name = match lib.split_once('=') {
            Some(("dylib", name)) => name,
            None => lib.as_str(),
            Some(_) => continue,
        };
        // `name:rename` links `rename`'s file.
        let name = name.rsplit(':').next().unwrap_or(name);
        let file = match os {
            "windows" => format!("{name}.dll"),
            "macos" => format!("lib{name}.dylib"),
            _ => format!("lib{name}.so"),
        };
        if let Some(path) = paths.iter().map(|dir| dir.join(&file)).find(|path| path.is_file()) {
            found.push(path);
        }
    }
    found
}

/// Adds a flag for rustc to the ones the environment already gives (a variable replaces those of
/// `.cargo/config.toml`, so they are not there to keep).
fn add_rustflag(cargo: &mut Command, flag: &str) {
    if let Some(encoded) = std::env::var_os("CARGO_ENCODED_RUSTFLAGS").filter(|f| !f.is_empty()) {
        let mut flags = encoded;
        flags.push("\u{1f}");
        flags.push(flag);
        cargo.env("CARGO_ENCODED_RUSTFLAGS", flags);
    } else {
        let mut flags = std::env::var_os("RUSTFLAGS").unwrap_or_default();
        if !flags.is_empty() {
            flags.push(" ");
        }
        flags.push(flag);
        cargo.env("RUSTFLAGS", flags);
    }
}

/// A file the Steamworks SDK reads beside the program when Steam did not start it: a developer's
/// (it names the app to be), never a player's.
const STEAM_APPID: &str = "steam_appid.txt";

/// The program, the libraries it needs beside it and `assets/` into `folder`, in place of the ones
/// there (anything else in it stays, but a `steam_appid.txt`). Says what went in.
fn place(game: &Game, exe: &Path, libraries: &[PathBuf], folder: &Path) -> Result<String, String> {
    let assets = game.dir.join(ASSETS);
    // Before anything in the folder changes: the asset folder is no link either (the links in it
    // are refused by `copy_dir`).
    refuse_link(&assets)?;
    let target = folder.join(ASSETS);
    if target.exists() {
        std::fs::remove_dir_all(&target).map_err(|e| format!("{}: {e}", target.display()))?;
    }
    std::fs::create_dir_all(folder).map_err(|e| format!("{}: {e}", folder.display()))?;
    let program = folder.join(exe.file_name().ok_or("the build named no program")?);
    std::fs::copy(exe, &program).map_err(|e| format!("{} → {}: {e}", exe.display(), program.display()))?;
    let size = std::fs::metadata(&program).map(|m| m.len()).unwrap_or(0);
    let (files, bytes) = if assets.is_dir() { copy_dir(&assets, &target)? } else { (0, 0) };
    let mut placed = format!("program {}, {ASSETS}/ {files} files, {}", megabytes(size), megabytes(bytes));
    let shipped = place_libraries(libraries, folder)?;
    if !shipped.is_empty() {
        placed.push_str(&format!(", beside it {}", shipped.join(", ")));
    }
    if remove_steam_appid(folder)? {
        say(format!("removed {STEAM_APPID} from the package: a game for players never ships it (Steam starts it with its app id)"));
    }
    Ok(placed)
}

/// Copies `libraries` into `folder`; their file names.
fn place_libraries(libraries: &[PathBuf], folder: &Path) -> Result<Vec<String>, String> {
    let mut names = Vec::new();
    for library in libraries {
        let name = library.file_name().ok_or_else(|| format!("{}: no file name", library.display()))?;
        let to = folder.join(name);
        std::fs::copy(library, &to).map_err(|e| format!("{} → {}: {e}", library.display(), to.display()))?;
        names.push(name.to_string_lossy().into_owned());
    }
    Ok(names)
}

/// Removes a `steam_appid.txt` from the package's folder: Steam starts a player's game with its
/// app id, and the file would name another. Whether there was one.
fn remove_steam_appid(folder: &Path) -> Result<bool, String> {
    remove_from_package(folder, STEAM_APPID)
}

/// Removes file `name` from the package's folder; whether it was there.
fn remove_from_package(folder: &Path, name: &str) -> Result<bool, String> {
    let file = folder.join(name);
    if !file.exists() {
        return Ok(false);
    }
    std::fs::remove_file(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(true)
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

/// A package holds files: a link (a symlink, a junction) could bring in anything it points to, or
/// nothing on the player's machine.
fn refuse_link(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => Err(format!("{} is a link: put the files themselves in the asset folder", path.display())),
        _ => Ok(()),
    }
}

/// Copies `from` into `to` (created), every file and folder; the number of files and their bytes.
fn copy_dir(from: &Path, to: &Path) -> Result<(u64, u64), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    let (mut files, mut bytes) = (0, 0);
    let entries = std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))?;
    for entry in entries {
        let entry = entry.map_err(|e| format!("{}: {e}", from.display()))?;
        let (source, target) = (entry.path(), to.join(entry.file_name()));
        let kind = entry.file_type().map_err(|e| format!("{}: {e}", source.display()))?;
        refuse_link(&source)?;
        if kind.is_dir() {
            let (f, b) = copy_dir(&source, &target)?;
            files += f;
            bytes += b;
        } else {
            bytes += std::fs::copy(&source, &target).map_err(|e| format!("{} → {}: {e}", source.display(), target.display()))?;
            files += 1;
        }
    }
    Ok((files, bytes))
}

/// The DLLs the program imports are all the system's, none of a redistributable runtime. Says
/// which they are and whether the program opens a console window.
fn check_imports(program: &Path) -> Result<bool, String> {
    let bytes = std::fs::read(program).map_err(|e| format!("{}: {e}", program.display()))?;
    let pe = read_pe(&bytes).map_err(|e| format!("{}: {e}", program.display()))?;
    say(format!(
        "subsystem: {}",
        if pe.gui { "windows (no console window)" } else { "console (a console window opens with it)" }
    ));
    let folder = program.parent().unwrap_or(Path::new("."));
    let (foreign, shipped) = foreign_imports(&pe.imports, folder, system_dll);
    if foreign.is_empty() {
        if shipped.is_empty() {
            say(format!("imports: system DLLs only ({})", pe.imports.join(", ")));
        } else {
            say(format!("imports: system DLLs and, beside the program, {} ({})", shipped.join(", "), pe.imports.join(", ")));
        }
        return Ok(true);
    }
    say(format!(
        "FAILED imports: {} {} not part of Windows: a clean machine cannot start the program (link the C runtime in with +crt-static; ship any other DLL beside the program)",
        foreign.iter().map(|d| d.as_str()).collect::<Vec<_>>().join(", "),
        if foreign.len() == 1 { "is" } else { "are" }
    ));
    Ok(false)
}

/// The imports that are no system DLL (by `system`), split into those not in `folder` (a clean
/// machine cannot start the program) and those shipped there (the Steamworks SDK's).
fn foreign_imports<'a>(imports: &'a [String], folder: &Path, system: impl Fn(&str) -> bool) -> (Vec<&'a String>, Vec<&'a str>) {
    let (shipped, foreign): (Vec<&String>, Vec<&String>) = imports.iter().filter(|dll| !system(dll)).partition(|dll| folder.join(dll).is_file());
    (foreign, shipped.into_iter().map(String::as_str).collect())
}

/// A DLL every Windows has: not a redistributable runtime, and in `System32` here (or an API set).
fn system_dll(dll: &str) -> bool {
    let lower = dll.to_ascii_lowercase();
    let redistributable = ["vcruntime", "msvcp", "msvcr", "concrt", "vccorlib", "ucrtbased", "libgcc", "libstdc++", "libwinpthread"];
    if redistributable.iter().any(|r| lower.starts_with(r)) {
        return false;
    }
    if lower.starts_with("api-ms-win-") || lower.starts_with("ext-ms-") {
        return true;
    }
    let system = std::env::var_os("SystemRoot").map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
    system.join("System32").join(dll).is_file()
}

/// What `shards package` reads of a Windows program.
#[derive(Debug, PartialEq)]
struct PeInfo {
    /// The DLLs of its import table, in order.
    imports: Vec<String>,
    /// Subsystem `windows`: no console window.
    gui: bool,
}

/// The import table and subsystem of a PE file (a Windows program).
fn read_pe(bytes: &[u8]) -> Result<PeInfo, String> {
    let bad = || "not a Windows program (a PE file)".to_owned();
    let u16_at = |at: usize| bytes.get(at..at + 2).map(|b| u16::from_le_bytes([b[0], b[1]])).ok_or_else(bad);
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]])).ok_or_else(bad);
    if bytes.get(..2) != Some(b"MZ") {
        return Err(bad());
    }
    let pe = u32_at(0x3c)? as usize;
    if bytes.get(pe..pe + 4) != Some(b"PE\0\0") {
        return Err(bad());
    }
    let coff = pe + 4;
    let sections = u16_at(coff + 2)? as usize;
    let optional = coff + 20;
    let section_table = optional + u16_at(coff + 16)? as usize;
    let directories = match u16_at(optional)? {
        0x10b => optional + 96,
        0x20b => optional + 112,
        _ => return Err(bad()),
    };
    let subsystem = u16_at(optional + 68)?;
    // The file offset of an address in the loaded image, by the section that holds it.
    let offset = |rva: u32| -> Result<usize, String> {
        for section in (0..sections).map(|i| section_table + i * 40) {
            let size = u32_at(section + 8)?.max(u32_at(section + 16)?);
            let (address, raw) = (u32_at(section + 12)?, u32_at(section + 20)?);
            if let Some(inside) = rva.checked_sub(address).filter(|d| *d < size) {
                return Ok((raw + inside) as usize);
            }
        }
        Err(bad())
    };
    let mut imports = Vec::new();
    // Data directory 1: the import table, descriptors of 20 bytes until one without a name.
    let table = u32_at(directories + 8)?;
    if table != 0 {
        let mut descriptor = offset(table)?;
        loop {
            let name = u32_at(descriptor + 12)?;
            if name == 0 {
                break;
            }
            let at = offset(name)?;
            let len = bytes.get(at..).and_then(|rest| rest.iter().position(|b| *b == 0)).ok_or_else(bad)?;
            imports.push(String::from_utf8_lossy(&bytes[at..at + len]).into_owned());
            descriptor += 20;
        }
    }
    Ok(PeInfo { imports, gui: subsystem == 2 })
}

/// The package copied to a temporary folder and started there as a player's machine would:
/// `--check-content`, then `--smoke TICKS`, with no environment but the system's; then nothing new
/// may be in its folder.
fn check_clean_run(folder: &Path, program: &Path, ticks: u64) -> Result<bool, String> {
    let copy = std::env::temp_dir().join(format!("shards-package-{}", std::process::id()));
    if copy.exists() {
        std::fs::remove_dir_all(&copy).map_err(|e| format!("{}: {e}", copy.display()))?;
    }
    copy_dir(folder, &copy)?;
    let result = run_copy(&copy, &copy.join(program.file_name().unwrap_or_default()), ticks);
    let _ = std::fs::remove_dir_all(&copy);
    result
}

fn run_copy(copy: &Path, program: &Path, ticks: u64) -> Result<bool, String> {
    let before = files_in(copy);
    let mut passed = true;
    for (what, args) in [("check-content", vec!["--check-content".to_owned()]), ("smoke", vec!["--smoke".to_owned(), ticks.to_string()])] {
        let run = run_bare(program, copy, &args)?;
        if run.ok {
            // The verdict is the last line of stdout (the log goes to stderr).
            let last = |text: &str| text.lines().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_owned());
            say(format!("{what}: {}", last(&run.stdout).or_else(|| last(&run.stderr)).unwrap_or_else(|| "(no output)".into())));
        } else {
            passed = false;
            say(format!("FAILED {what} (`{} {}` in a copy of the package):", program.display(), args.join(" ")));
            for line in run.stdout.lines().chain(run.stderr.lines()) {
                say(format!("  {line}"));
            }
        }
    }
    let written = changed(&before, &files_in(copy));
    if written.is_empty() {
        say("writes: nothing in its own folder");
    } else {
        passed = false;
        say(format!(
            "FAILED writes: the program changed its own folder, which a player may not be allowed to write to: {}",
            written.join(", ")
        ));
    }
    Ok(passed)
}

/// Paths under a folder, each with its size and when it was last written (folders: 0, none).
type Listing = BTreeMap<PathBuf, (u64, Option<SystemTime>)>;

/// What differs between two [`files_in`] listings: new and rewritten paths, then removed ones.
fn changed(before: &Listing, after: &Listing) -> Vec<String> {
    let written = after.iter().filter(|(path, file)| before.get(*path) != Some(file)).map(|(path, _)| path.display().to_string());
    let removed = before.keys().filter(|path| !after.contains_key(*path)).map(|path| format!("{} (removed)", path.display()));
    written.chain(removed).collect()
}

/// Every file and folder under `root`.
fn files_in(root: &Path) -> Listing {
    fn walk(root: &Path, dir: &Path, into: &mut Listing) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = entry.path();
            let relative = path.strip_prefix(root).unwrap_or(&path).to_path_buf();
            if entry.file_type().is_ok_and(|t| t.is_dir()) {
                into.insert(relative, (0, None));
                walk(root, &path, into);
            } else {
                let metadata = entry.metadata().ok();
                into.insert(relative, (metadata.as_ref().map_or(0, |m| m.len()), metadata.and_then(|m| m.modified().ok())));
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}

/// How a run of the package's program went.
struct Run {
    /// It exited with 0.
    ok: bool,
    stdout: String,
    stderr: String,
}

/// Runs `program` in `dir` with only the environment a player's machine has.
fn run_bare(program: &Path, dir: &Path, args: &[String]) -> Result<Run, String> {
    run_bare_for(program, dir, args, RUN_TIMEOUT)
}

/// [`run_bare`], ending the program and every process it started after `timeout`.
fn run_bare_for(program: &Path, dir: &Path, args: &[String], timeout: Duration) -> Result<Run, String> {
    let mut command = Command::new(program);
    command.args(args).current_dir(dir).env_clear().stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    if cfg!(windows) {
        for name in ["SystemRoot", "windir", "SystemDrive", "TEMP", "TMP", "USERPROFILE", "APPDATA", "LOCALAPPDATA", "ProgramData", "OS"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
        command.env("PATH", format!(r"{system}\System32;{system}"));
    } else {
        for name in ["HOME", "TMPDIR", "XDG_RUNTIME_DIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        command.env("PATH", "/usr/bin:/bin");
        // Its own process group, to be ended with everything it started.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut command, 0);
    }
    let mut child = command.spawn().map_err(|e| format!("{}: {e}", program.display()))?;
    let read = |pipe: Option<Box<dyn Read + Send>>| {
        let text = Arc::new(Mutex::new(Vec::new()));
        let sink = text.clone();
        let reader = std::thread::spawn(move || {
            let Some(mut pipe) = pipe else { return };
            let mut chunk = [0; 8192];
            while let Ok(n @ 1..) = pipe.read(&mut chunk) {
                if let Ok(mut text) = sink.lock() {
                    text.extend_from_slice(&chunk[..n]);
                }
            }
        });
        (reader, text)
    };
    let stdout = read(child.stdout.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let stderr = read(child.stderr.take().map(|p| Box::new(p) as Box<dyn Read + Send>));
    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait().map_err(|e| format!("{}: {e}", program.display()))? {
            break Some(status);
        }
        if started.elapsed() > timeout {
            end_tree(&mut child);
            let _ = child.wait();
            break None;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    // A process the program started may still hold its output open: wait a while, no longer.
    let until = Instant::now() + OUTPUT_GRACE;
    let (stdout, mut stderr) = (read_until(stdout, until), read_until(stderr, until));
    if status.is_none() {
        stderr.push_str(&format!("\n(still running after {}s: ended)", timeout.as_secs()));
    }
    Ok(Run { ok: status.is_some_and(|s| s.success()), stdout, stderr })
}

/// How long the output of a program that ended may stay open (held by a process it started).
const OUTPUT_GRACE: Duration = Duration::from_secs(5);

/// Ends `child` and every process it started.
fn end_tree(child: &mut std::process::Child) {
    #[cfg(windows)]
    {
        // `/T` follows the processes it started.
        let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
        let _ = Command::new(format!(r"{system}\System32\taskkill.exe"))
            .args(["/T", "/F", "/PID", &child.id().to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    if let Ok(group) = libc::pid_t::try_from(child.id()) {
        // SAFETY: a plain call; the group is the child's own (`process_group(0)`).
        unsafe { libc::kill(-group, libc::SIGKILL) };
    }
    let _ = child.kill();
}

/// What a reader read: all of it once the pipe closed, or what came before `until`.
fn read_until((reader, text): (std::thread::JoinHandle<()>, Arc<Mutex<Vec<u8>>>), until: Instant) -> String {
    while !reader.is_finished() && Instant::now() < until {
        std::thread::sleep(Duration::from_millis(20));
    }
    let bytes = text.lock().map(|t| t.clone()).unwrap_or_default();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// `.res` resource types.
const RT_ICON: u16 = 3;
const RT_GROUP_ICON: u16 = 14;

/// A compiled resource file (what `rc.exe` writes) with one icon: `png` its only image (Windows
/// reads PNG images in icons since Vista), in icon group 1, the one Explorer shows. The PNG is
/// square, at most 256×256.
fn icon_res(png: &[u8]) -> Result<Vec<u8>, String> {
    let (width, height) = png_size(png)?;
    if width != height || !(1..=256).contains(&width) {
        return Err(format!("the icon is {width}×{height}: it must be square, at most 256×256 (256×256 serves every size)"));
    }
    let len = u32::try_from(png.len()).map_err(|_| "the icon is too big".to_owned())?;
    // Every .res starts with an empty entry.
    let mut res = vec![0, 0, 0, 0, 32, 0, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0];
    res.extend([0; 16]);
    resource(&mut res, RT_ICON, 1, 0x1010, png);
    // The group: one image, 32 bits, its size; 256 is written as 0.
    let side = if width == 256 { 0 } else { width as u8 };
    let mut group = Vec::new();
    for value in [0u16, 1, 1] {
        group.extend(value.to_le_bytes());
    }
    group.extend([side, side, 0, 0]);
    group.extend(1u16.to_le_bytes());
    group.extend(32u16.to_le_bytes());
    group.extend(len.to_le_bytes());
    group.extend(1u16.to_le_bytes());
    resource(&mut res, RT_GROUP_ICON, 1, 0x1030, &group);
    Ok(res)
}

/// One entry of a `.res`: a 32-byte header (numeric type and name, neutral language), the
/// data, padding to four bytes.
fn resource(res: &mut Vec<u8>, kind: u16, name: u16, flags: u16, data: &[u8]) {
    res.extend((data.len() as u32).to_le_bytes());
    res.extend(32u32.to_le_bytes());
    res.extend([0xff, 0xff]);
    res.extend(kind.to_le_bytes());
    res.extend([0xff, 0xff]);
    res.extend(name.to_le_bytes());
    res.extend(0u32.to_le_bytes()); // data version
    res.extend(flags.to_le_bytes()); // memory flags: moveable, discardable (and pure, a group)
    res.extend(0u16.to_le_bytes()); // language: neutral
    res.extend(0u32.to_le_bytes()); // version
    res.extend(0u32.to_le_bytes()); // characteristics
    res.extend(data);
    while !res.len().is_multiple_of(4) {
        res.push(0);
    }
}

/// Width and height from a PNG's header.
fn png_size(png: &[u8]) -> Result<(u32, u32), String> {
    if png.get(..8) != Some(b"\x89PNG\r\n\x1a\n") || png.get(12..16) != Some(b"IHDR") {
        return Err("not a PNG image".into());
    }
    let be = |at: usize| u32::from_be_bytes([png[at], png[at + 1], png[at + 2], png[at + 3]]);
    if png.len() < 24 {
        return Err("a PNG cut short".into());
    }
    Ok((be(16), be(20)))
}

/// FNV-1a, 64 bits: a name for a file's contents.
fn fnv1a(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, b| (hash ^ u64::from(*b)).wrapping_mul(0x0100_0000_01b3))
}

/// Output of a command that must succeed.
fn output(command: &mut Command, what: &str) -> Result<Vec<u8>, String> {
    let out = command.stderr(Stdio::piped()).output().map_err(|e| format!("{what}: {e}"))?;
    if !out.status.success() {
        return Err(format!("{what}: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    Ok(out.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 1×1 PNG's header and nothing more: all `icon_res` reads of it.
    fn png(side: u32) -> Vec<u8> {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend(side.to_be_bytes());
        png.extend(side.to_be_bytes());
        png.extend([8, 6, 0, 0, 0]);
        png
    }

    #[test]
    fn an_icon_resource_is_an_empty_entry_the_image_and_its_group() {
        let image = png(256);
        let res = icon_res(&image).unwrap();
        // The empty entry.
        assert_eq!(&res[..16], &[0, 0, 0, 0, 32, 0, 0, 0, 0xff, 0xff, 0, 0, 0xff, 0xff, 0, 0]);
        // The image: its size, type RT_ICON, name 1, then the PNG itself.
        let icon = &res[32..];
        assert_eq!(u32::from_le_bytes(icon[..4].try_into().unwrap()) as usize, image.len());
        assert_eq!(&icon[8..16], &[0xff, 0xff, 3, 0, 0xff, 0xff, 1, 0]);
        assert_eq!(&icon[32..32 + image.len()], image.as_slice());
        // The group, after the image padded to four bytes: one 256-pixel image (0), 32 bits, id 1.
        let group = &res[32 + (32 + image.len()).next_multiple_of(4)..];
        assert_eq!(&group[8..16], &[0xff, 0xff, 14, 0, 0xff, 0xff, 1, 0]);
        let entry = &group[32..];
        assert_eq!(&entry[..6], &[0, 0, 1, 0, 1, 0]);
        assert_eq!(&entry[6..10], &[0, 0, 0, 0]);
        assert_eq!(u32::from_le_bytes(entry[14..18].try_into().unwrap()) as usize, image.len());
        assert_eq!(&entry[18..20], &[1, 0]);
        assert!(res.len().is_multiple_of(4));
    }

    #[test]
    fn an_icon_must_be_a_square_png_of_256_at_most() {
        assert!(icon_res(&png(48)).is_ok());
        assert!(icon_res(&png(512)).unwrap_err().contains("at most 256×256"));
        let mut wide = png(64);
        wide[19] = 128;
        assert!(icon_res(&wide).unwrap_err().contains("square"));
        assert_eq!(icon_res(b"GIF89a").unwrap_err(), "not a PNG image");
    }

    #[test]
    fn a_run_that_hangs_is_ended_with_what_it_started() {
        // A shell that waits for a program of its own, which holds the output too.
        let (program, args) = if cfg!(windows) {
            let system = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_owned());
            (PathBuf::from(format!(r"{system}\System32\cmd.exe")), ["/c", "ping -n 60 127.0.0.1"])
        } else {
            (PathBuf::from("/bin/sh"), ["-c", "sleep 60; true"])
        };
        let args: Vec<String> = args.iter().map(|a| (*a).to_owned()).collect();
        let started = Instant::now();
        let run = run_bare_for(&program, &std::env::temp_dir(), &args, Duration::from_secs(1)).unwrap();
        assert!(!run.ok);
        assert!(run.stderr.ends_with("(still running after 1s: ended)"), "{}", run.stderr);
        assert!(started.elapsed() < Duration::from_secs(4), "what it started ended with it: {:?}", started.elapsed());
    }

    #[test]
    fn a_program_names_the_dlls_it_imports() {
        assert_eq!(read_pe(b"not a program").unwrap_err(), "not a Windows program (a PE file)");
        // This test's own program: on Windows a PE file that imports kernel32.
        if cfg!(windows) {
            let me = std::fs::read(std::env::current_exe().unwrap()).unwrap();
            let pe = read_pe(&me).unwrap();
            assert!(pe.imports.iter().any(|d| d.eq_ignore_ascii_case("kernel32.dll")), "{pe:?}");
            assert!(!pe.gui);
            assert!(system_dll("KERNEL32.dll"));
        }
        assert!(!system_dll("VCRUNTIME140.dll"));
        assert!(!system_dll("libgcc_s_seh-1.dll"));
    }

    #[cfg(unix)]
    #[test]
    fn a_link_in_the_assets_is_refused() {
        let dir = std::env::temp_dir().join(format!("shards-package-link-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(dir.join("secret.txt"), "not for players").unwrap();
        std::os::unix::fs::symlink(dir.join("secret.txt"), dir.join("assets/notes.txt")).unwrap();
        let err = copy_dir(&dir.join("assets"), &dir.join("copy")).unwrap_err();
        assert!(err.ends_with("notes.txt is a link: put the files themselves in the asset folder"), "{err}");
        assert!(!dir.join("copy/notes.txt").exists());
        // The asset folder itself, too.
        std::os::unix::fs::symlink(dir.join("assets"), dir.join("linked")).unwrap();
        let err = refuse_link(&dir.join("linked")).unwrap_err();
        assert!(err.ends_with("linked is a link: put the files themselves in the asset folder"), "{err}");
        assert!(refuse_link(&dir.join("assets")).is_ok() && refuse_link(&dir.join("none")).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_dynamic_libraries_build_scripts_link_go_beside_the_program_and_steam_appid_txt_never_does() {
        let dir = std::env::temp_dir().join(format!("shards-package-libraries-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let (out, package) = (dir.join("out"), dir.join("package"));
        std::fs::create_dir_all(&out).unwrap();
        std::fs::create_dir_all(&package).unwrap();
        for file in ["steam_api64.dll", "steam_api64.lib", "libsteam_api.so", "libsteam_api.dylib"] {
            std::fs::write(out.join(file), file).unwrap();
        }
        // What steamworks-sys's build script tells cargo.
        let message = serde_json::json!({
            "reason": "build-script-executed",
            "package_id": "registry+https://github.com/rust-lang/crates.io-index#steamworks-sys@0.13.0",
            "linked_libs": ["dylib=steam_api64", "static=other", "dylib=missing"],
            "linked_paths": [format!("native={}", out.display())],
            "out_dir": out.display().to_string(),
        });
        assert_eq!(dynamic_libraries(&message, "windows"), [out.join("steam_api64.dll")], "the static one and one not found are not shipped");
        let unix = serde_json::json!({"reason": "build-script-executed", "linked_libs": ["dylib=steam_api"], "linked_paths": [out.display().to_string()]});
        assert_eq!(dynamic_libraries(&unix, "linux"), [out.join("libsteam_api.so")]);
        assert_eq!(dynamic_libraries(&unix, "macos"), [out.join("libsteam_api.dylib")]);
        let artifact = serde_json::json!({"reason": "compiler-artifact", "linked_libs": ["dylib=steam_api64"], "linked_paths": [out.display().to_string()]});
        assert!(dynamic_libraries(&artifact, "windows").is_empty());

        assert_eq!(place_libraries(&[out.join("steam_api64.dll")], &package).unwrap(), ["steam_api64.dll"]);
        assert!(package.join("steam_api64.dll").is_file());
        assert!(!remove_steam_appid(&package).unwrap(), "none to remove");
        // A file of another name stands in for a steam_appid.txt: a test never writes one, where the
        // Steamworks SDK of some run could read it.
        assert_eq!(STEAM_APPID, "steam_appid.txt");
        std::fs::write(package.join("stale.txt"), "480").unwrap();
        assert!(remove_from_package(&package, "stale.txt").unwrap());
        assert!(!package.join("stale.txt").exists());

        // The program's imports: the system's, one shipped beside it, one missing.
        let imports = ["KERNEL32.dll".to_owned(), "steam_api64.dll".to_owned(), "VCRUNTIME140.dll".to_owned()];
        let (foreign, shipped) = foreign_imports(&imports, &package, |dll| dll == "KERNEL32.dll");
        assert_eq!((foreign, shipped), (vec![&imports[2]], vec!["steam_api64.dll"]));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_run_that_removes_files_changed_its_folder() {
        let dir = std::env::temp_dir().join(format!("shards-package-changed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("assets/sounds")).unwrap();
        std::fs::write(dir.join("assets/game.ron"), "()").unwrap();
        std::fs::write(dir.join("assets/sounds/hit.ogg"), "ogg").unwrap();
        std::fs::write(dir.join("settings.ron"), "()").unwrap();
        let before = files_in(&dir);
        assert!(changed(&before, &files_in(&dir)).is_empty());
        std::fs::write(dir.join("settings.ron"), "(volume: 1.0)").unwrap();
        std::fs::write(dir.join("new.log"), "").unwrap();
        std::fs::remove_dir_all(dir.join("assets/sounds")).unwrap();
        let sounds = Path::new("assets").join("sounds");
        assert_eq!(
            changed(&before, &files_in(&dir)),
            [
                "new.log".to_owned(),
                "settings.ron".to_owned(),
                format!("{} (removed)", sounds.display()),
                format!("{} (removed)", sounds.join("hit.ogg").display()),
            ]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_dist_profile_is_used_when_the_workspace_defines_it() {
        let dir = std::env::temp_dir().join(format!("shards-package-profile-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\n\n[profile.release]\nlto = true\n").unwrap();
        assert_eq!(default_profile(&dir), "release");
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\n\n[profile.dist]\ninherits = \"release\"\n").unwrap();
        assert_eq!(default_profile(&dir), "dist");
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\n\n[ profile.\"dist\" ]  # for players\ninherits = \"release\"\n").unwrap();
        assert_eq!(default_profile(&dir), "dist");
        std::fs::write(dir.join("Cargo.toml"), "[workspace]\n# [profile.dist]\n[profile.distant]\n").unwrap();
        assert_eq!(default_profile(&dir), "release");
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
