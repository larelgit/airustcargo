//! Crash reports: everything a bug report needs, in one file a player can send.
//!
//! A report (`shards-crash/1`, `crashes/crash-<UTC time>-tick<N>.json` in the game's working
//! directory, or the player's own folder when that one cannot be written) holds:
//! - what happened: the panic message, where, on which thread, and the call stack;
//! - the game and the machine: title, version, program, build; OS, CPU, memory, GPU;
//! - the session: `ai/status`, the journal's last entries and the log's last lines;
//! - the session's replay, to play it again: `--replay` takes the report as it is.
//!
//! Reports are written when a gameplay system fails (with its crash replay, once per
//! system), when a panic nobody handled happens on another thread (the render thread, a task),
//! on request (the `crash_report` tool: a bug report), and by `shards::run` when a panic unwinds
//! out of the game. The world goes with that last one, so its report has what the panic hook
//! kept (the session's tick, seed and game), the log and the system, but no replay, status or
//! journal.
//!
//! The panic hook ([`install_panic_hook`]; `shards::run` installs it) keeps the last panics with
//! their call stacks; it writes nothing. Code that catches a panic itself and carries on (a
//! tool's answer, an invariant's violation) calls [`handled`], so no report is written for it.
//! [`AiCorePlugin::crash_replays`](crate::AiCorePlugin) switches off the reports the game writes by itself (a
//! failed system, a panic nobody handled), not the one an agent asks for with `crash_report`: that is a bug
//! report on request, and it is written whatever the switch says. It goes to a new file in `crashes/`, or,
//! with `path`, to the file of that name in `crashes/`, replaced each time.

use std::backtrace::{Backtrace, BacktraceStatus};
use std::collections::VecDeque;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Once, PoisonError};
use std::thread::ThreadId;
use std::time::{SystemTime, UNIX_EPOCH};

use bevy::platform::collections::HashSet;
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::journal::{self, AiJournal, JournalKind};
use crate::replay::{self, Replay};
use crate::session::Scope;
use crate::tools::{register_tool, ToolKind};
use crate::{Call, ToolResult};

/// The `format` of a crash report.
pub const FORMAT: &str = "shards-crash/1";
/// Where reports go, relative to the game's working directory.
pub const CRASHES_DIR: &str = "crashes";
/// Log lines a report keeps (the newest).
pub const LOG_LINES: usize = 200;
/// Journal entries a report keeps (the newest).
pub const JOURNAL_LINES: usize = 200;
/// Panics the hook keeps (the newest).
const PANICS_KEPT: usize = 8;

/// A crash report, as written to its file.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CrashReport {
    pub format: String,
    /// When it was written, UTC: `2026-10-16T12:34:56Z`.
    pub time: String,
    pub what: What,
    pub game: Game,
    pub system: System,
    /// Tick, seed and tick rate (`None`: no game was running).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<Session>,
    /// `ai/status` when it was written (`null`: the world was gone).
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub status: Value,
    /// The journal's newest entries, oldest first.
    #[serde(default)]
    pub journal: Vec<String>,
    /// The log's newest lines (info, warnings, errors), oldest first.
    #[serde(default)]
    pub log: Vec<String>,
    /// Other panics the hook kept, oldest first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub panics: Vec<What>,
    /// The crash replay saved with the report, relative to the working directory; `None` when it
    /// could not be written (the report, which may then be in the player's folder, still holds the replay).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub replay_file: Option<String>,
    /// The session from its start: `--replay` plays it.
    pub replay: Option<Replay>,
    /// Why there is no replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub no_replay: Option<String>,
    /// The kept panics it holds (the one it is about, its other panics). They are in a report once
    /// it is written: a report that cannot be written leaves them to the next.
    #[serde(skip)]
    pub(crate) claimed: Vec<u64>,
}

/// What went wrong.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct What {
    /// `system game::combat::apply_hits panicked at tick 50`, `thread 'main' panicked`,
    /// `bug report: the boss fell through the floor`.
    pub headline: String,
    pub message: String,
    /// `src/combat.rs:120:9`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<String>,
    /// When the panic happened, UTC.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub time: Option<String>,
    /// The call stack, a line per entry (from the panic hook).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub backtrace: Vec<String>,
}

/// The game that crashed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Game {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The program's file name: `my_game.exe`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// The engine's version.
    pub engine: String,
    /// `debug` or `release` (a `dist` build is a release build).
    pub build: String,
}

/// The machine it ran on.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct System {
    /// `Windows 11 (10.0.22631)`, `Ubuntu 24.04.1 LTS (Linux 6.8.0-45-generic)`.
    pub os: String,
    pub arch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpu: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub threads: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_free_mb: Option<u64>,
    /// The adapter the game renders with: `NVIDIA GeForce RTX 4070 (Vulkan, DiscreteGpu, driver
    /// NVIDIA 560.94)`; `none` without a renderer (headless).
    pub gpu: String,
}

/// Where the session was.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Session {
    pub tick: u64,
    pub seed: u64,
    pub tick_hz: f64,
}

/// What a report says about the game: [`AiCorePlugin::game`](crate::AiCorePlugin::game).
#[derive(Resource, Clone, Debug, Default)]
pub struct GameInfo {
    pub title: String,
    /// The game's version (`env!("CARGO_PKG_VERSION")` of the game's crate).
    pub version: Option<String>,
}

/// A panic as the hook saw it.
struct Kept {
    /// Names it in the reports that hold it ([`CrashReport::claimed`]).
    id: u64,
    what: What,
    backtrace: Arc<Backtrace>,
    /// The thread it happened on.
    thread: ThreadId,
    /// The app active on that thread then ([`process::active`](crate::process::active)).
    token: u64,
    /// The app that was working when it happened, as it was then.
    seen: Option<Seen>,
    /// Caught where it happened, or reported: no report of its own is written for it.
    handled: bool,
    /// In a report already (as what it is about, or among its other panics), or in one being
    /// written.
    reported: bool,
}

/// An app's game and session, as of its last frame.
#[derive(Clone)]
struct Seen {
    token: u64,
    game: Game,
    gpu: Option<String>,
    session: Session,
}

/// What the process keeps for reports, whatever happens to the worlds.
struct Memory {
    log: VecDeque<String>,
    panics: VecDeque<Kept>,
    /// The next panic's id.
    next: u64,
    seen: Vec<Seen>,
}

static MEMORY: Mutex<Memory> = Mutex::new(Memory { log: VecDeque::new(), panics: VecDeque::new(), next: 0, seen: Vec::new() });

fn memory() -> MutexGuard<'static, Memory> {
    // A report is wanted most when something already went wrong.
    MEMORY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Keeps the last panics (message, place, thread, call stack, the session they happened in)
/// for crash reports, then does what the hook before it did (prints the message). Installed once
/// per process; `shards::run` does it.
pub fn install_panic_hook() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            keep_panic(info.payload_as_str().unwrap_or("unknown panic"), info.location().map(ToString::to_string));
            previous(info);
        }));
    });
}

fn keep_panic(message: &str, location: Option<String>) {
    let current = std::thread::current();
    let thread = current.name().map_or_else(|| format!("{:?}", current.id()), str::to_owned);
    let backtrace = Arc::new(Backtrace::force_capture());
    // A panic while this thread holds the memory must not wait for itself: after a while the
    // panic goes unkept.
    let mut memory = None;
    for _ in 0..1000 {
        match MEMORY.try_lock() {
            Ok(guard) => memory = Some(guard),
            Err(std::sync::TryLockError::Poisoned(poisoned)) => memory = Some(poisoned.into_inner()),
            Err(std::sync::TryLockError::WouldBlock) => std::thread::yield_now(),
        }
        if memory.is_some() {
            break;
        }
    }
    let Some(mut memory) = memory else { return };
    let active = crate::process::active();
    let seen = memory.seen.iter().rev().find(|s| s.token == active).or(memory.seen.last()).cloned();
    let what = What {
        headline: format!("thread '{thread}' panicked"),
        message: message.to_owned(),
        location,
        thread: Some(thread),
        time: Some(utc(SystemTime::now())),
        backtrace: Vec::new(),
    };
    if memory.panics.len() == PANICS_KEPT {
        memory.panics.pop_front();
    }
    let id = memory.next;
    memory.next += 1;
    memory.panics.push_back(Kept { id, what, backtrace, thread: current.id(), token: active, seen, handled: false, reported: false });
}

/// A log line for reports (the capture layer passes info, warnings and errors).
pub(crate) fn keep_log(line: String) {
    let mut memory = memory();
    if memory.log.len() == LOG_LINES {
        memory.log.pop_front();
    }
    memory.log.push_back(line);
}

/// An unhandled panic with this message was handled where it happened (a tool answered with it, a
/// rule was reported broken): no report of its own is written for it; the next report lists it
/// among its other panics. Returns what the hook kept of it.
///
/// Two threads or two apps of one process may panic with the same words at once. The caller
/// handles its own: the newest one of this thread first, then the newest one of the app active
/// here, then the newest one of all (a system panics on a worker thread, where the active app is
/// whichever was activated last in the process).
pub fn handled(message: &str) -> Option<What> {
    claim(message, false).map(|(_, what)| what)
}

/// [`handled`] for the panic a report is about: the report does not list it again among its other
/// panics. Returns its id too, for the report's [`CrashReport::claimed`].
pub(crate) fn reported(message: &str) -> Option<(u64, What)> {
    claim(message, true)
}

fn claim(message: &str, reported: bool) -> Option<(u64, What)> {
    let (thread, token) = (std::thread::current().id(), crate::process::active());
    let (id, kept) = {
        let mut memory = memory();
        let index = {
            let open = || memory.panics.iter().enumerate().rev().filter(|(_, p)| !p.handled && p.what.message == message);
            open().find(|(_, p)| p.thread == thread).or_else(|| open().find(|(_, p)| p.token == token)).or_else(|| open().next())?.0
        };
        let kept = &mut memory.panics[index];
        kept.handled = true;
        kept.reported |= reported;
        (kept.id, taken(kept))
    };
    Some((id, with_backtrace(kept)))
}

/// A report that could not be written leaves the panics it held to the next one.
fn release(report: &CrashReport) {
    let mut memory = memory();
    memory.panics.iter_mut().filter(|p| report.claimed.contains(&p.id)).for_each(|p| p.reported = false);
}

/// A kept panic, to be written out once the memory is let go.
fn taken(kept: &Kept) -> (What, Arc<Backtrace>) {
    (kept.what.clone(), kept.backtrace.clone())
}

/// The panic with its call stack written out (symbols are looked up now: slow the first time).
fn with_backtrace((mut what, backtrace): (What, Arc<Backtrace>)) -> What {
    if backtrace.status() == BacktraceStatus::Captured {
        what.backtrace = backtrace.to_string().lines().map(|l| l.trim_end().to_owned()).collect();
    }
    what
}

/// UTC as `2026-10-16T12:34:56Z`.
pub fn utc(time: SystemTime) -> String {
    let secs = time.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let (days, rest) = (secs / 86_400, secs % 86_400);
    let (y, m, d) = civil(days as i64);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rest / 3600, rest / 60 % 60, rest % 60)
}

/// Year, month, day of a day counted from 1970-01-01 (Howard Hinnant's `civil_from_days`).
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (yoe + era * 400 + i64::from(m <= 2), m, d)
}

fn game(info: &GameInfo) -> Game {
    Game {
        title: info.title.clone(),
        version: info.version.clone(),
        program: std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())),
        engine: env!("CARGO_PKG_VERSION").to_owned(),
        build: if cfg!(debug_assertions) { "debug" } else { "release" }.to_owned(),
    }
}

/// The adapter this world renders with, if it has one.
pub(crate) fn gpu(world: &World) -> Option<String> {
    #[cfg(feature = "render")]
    if let Some(info) = world.get_resource::<bevy::render::renderer::RenderAdapterInfo>() {
        let driver = [info.driver.as_str(), info.driver_info.as_str()].join(" ");
        let driver = driver.trim();
        let driver = if driver.is_empty() { String::new() } else { format!(", driver {driver}") };
        return Some(format!("{} ({:?}, {:?}{driver})", info.name, info.backend, info.device_type));
    }
    let _ = world;
    None
}

fn session(world: &World) -> Session {
    Session {
        tick: crate::sim::tick(world),
        seed: world.get_resource::<crate::rng::SimSeed>().map_or(0, |s| s.0),
        // Rounded as `ai/status` has it: 60, not 59.9999988.
        tick_hz: world.get_resource::<Time<Fixed>>().map_or(0.0, |t| (1.0 / t.timestep().as_secs_f64() * 1000.0).round() / 1000.0),
    }
}

/// The machine: OS, CPU, memory, and `gpu` (none: no renderer).
pub fn system(gpu: Option<String>) -> System {
    let (memory_mb, memory_free_mb) = memory_mb();
    System {
        os: os(),
        arch: std::env::consts::ARCH.to_owned(),
        cpu: cpu(),
        threads: std::thread::available_parallelism().ok().map(usize::from),
        memory_mb,
        memory_free_mb,
        gpu: gpu.unwrap_or_else(|| "none (no renderer: headless)".to_owned()),
    }
}

#[cfg(windows)]
fn os() -> String {
    #[repr(C)]
    struct OsVersionInfo {
        size: u32,
        major: u32,
        minor: u32,
        build: u32,
        platform: u32,
        service_pack: [u16; 128],
    }
    #[link(name = "ntdll")]
    unsafe extern "system" {
        fn RtlGetVersion(info: *mut OsVersionInfo) -> i32;
    }
    let mut info = OsVersionInfo { size: size_of::<OsVersionInfo>() as u32, major: 0, minor: 0, build: 0, platform: 0, service_pack: [0; 128] };
    // SAFETY: `info` is an OSVERSIONINFOW whose size field says so. Unlike `GetVersionEx`, the
    // answer does not depend on the program's manifest.
    if unsafe { RtlGetVersion(&mut info) } != 0 {
        return "Windows".to_owned();
    }
    let name = match (info.major, info.build) {
        (10, build) if build >= 22_000 => "Windows 11",
        (10, _) => "Windows 10",
        _ => "Windows",
    };
    format!("{name} ({}.{}.{})", info.major, info.minor, info.build)
}

#[cfg(target_os = "linux")]
fn os() -> String {
    let release = std::fs::read_to_string("/etc/os-release").ok();
    let name = release.as_deref().and_then(|r| r.lines().find_map(|l| l.strip_prefix("PRETTY_NAME="))).map(|n| n.trim().trim_matches('"'));
    let kernel = std::fs::read_to_string("/proc/sys/kernel/osrelease").ok();
    match (name, kernel.as_deref().map(str::trim)) {
        (Some(name), Some(kernel)) => format!("{name} (Linux {kernel})"),
        (Some(name), None) => name.to_owned(),
        (None, Some(kernel)) => format!("Linux {kernel}"),
        (None, None) => "Linux".to_owned(),
    }
}

#[cfg(not(any(windows, target_os = "linux")))]
fn os() -> String {
    std::env::consts::OS.to_owned()
}

/// Physical memory and how much of it is free, in MiB.
#[cfg(windows)]
fn memory_mb() -> (Option<u64>, Option<u64>) {
    #[repr(C)]
    #[derive(Default)]
    struct MemoryStatus {
        length: u32,
        load: u32,
        total_phys: u64,
        avail_phys: u64,
        total_page_file: u64,
        avail_page_file: u64,
        total_virtual: u64,
        avail_virtual: u64,
        avail_extended_virtual: u64,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(status: *mut MemoryStatus) -> i32;
    }
    let mut status = MemoryStatus { length: size_of::<MemoryStatus>() as u32, ..Default::default() };
    // SAFETY: `status` is a MEMORYSTATUSEX whose length field says so.
    if unsafe { GlobalMemoryStatusEx(&mut status) } == 0 {
        return (None, None);
    }
    (Some(status.total_phys >> 20), Some(status.avail_phys >> 20))
}

#[cfg(target_os = "linux")]
fn memory_mb() -> (Option<u64>, Option<u64>) {
    let info = std::fs::read_to_string("/proc/meminfo").unwrap_or_default();
    let field = |name: &str| {
        info.lines()
            .find_map(|l| l.strip_prefix(name))
            .and_then(|v| v.trim().trim_end_matches("kB").trim().parse::<u64>().ok())
            .map(|kib| kib >> 10)
    };
    (field("MemTotal:"), field("MemAvailable:"))
}

#[cfg(not(any(windows, target_os = "linux")))]
fn memory_mb() -> (Option<u64>, Option<u64>) {
    (None, None)
}

/// The processor's name: `AMD Ryzen 7 7800X3D 8-Core Processor`.
fn cpu() -> Option<String> {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::__cpuid;
        if __cpuid(0x8000_0000).eax >= 0x8000_0004 {
            let mut name = Vec::with_capacity(48);
            for leaf in 0x8000_0002..=0x8000_0004 {
                let r = __cpuid(leaf);
                for part in [r.eax, r.ebx, r.ecx, r.edx] {
                    name.extend_from_slice(&part.to_le_bytes());
                }
            }
            let name = String::from_utf8_lossy(&name).trim_matches(char::from(0)).trim().to_owned();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    #[cfg(target_os = "linux")]
    if let Ok(info) = std::fs::read_to_string("/proc/cpuinfo") {
        let named = |key: &str| {
            info.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.trim() == key).map(|(_, v)| v.trim().to_owned()))
        };
        return named("model name").or_else(|| named("Model")).or_else(|| named("Hardware"));
    }
    None
}

/// A report of what happened in this world: its game, session, status, journal and replay (the
/// session's, or `replay` when the caller already made it), the log and the machine. Panics the
/// hook kept that no report has yet go with it (caught ones too), and are reported now.
pub fn report(world: &World, what: What, replay: Option<Replay>) -> CrashReport {
    let (replay, no_replay) = match replay {
        Some(replay) => (Some(replay), None),
        None => session_replay(world),
    };
    let status = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| crate::status::status(world)))
        .unwrap_or_else(|p| serde_json::json!({"error": format!("ai/status panicked: {}", crate::tools::panic_message(&p))}));
    let journal = world.get_resource::<AiJournal>().map_or_else(Vec::new, |j| {
        let lines: Vec<String> = j.after(0).0.map(ToString::to_string).collect();
        lines[lines.len().saturating_sub(JOURNAL_LINES)..].to_vec()
    });
    let info = world.get_resource::<GameInfo>().cloned().unwrap_or_default();
    let (claimed, kept, log): (Vec<_>, Vec<_>, _) = {
        let mut memory = memory();
        let (claimed, kept) = memory
            .panics
            .iter_mut()
            .filter(|p| !p.reported)
            .map(|p| {
                p.handled = true;
                p.reported = true;
                (p.id, taken(p))
            })
            .unzip();
        (claimed, kept, memory.log.iter().cloned().collect())
    };
    let panics = kept.into_iter().map(with_backtrace).filter(|p| *p != what).collect();
    CrashReport {
        format: FORMAT.to_owned(),
        time: utc(SystemTime::now()),
        what,
        game: game(&info),
        system: system(gpu(world)),
        session: Some(session(world)),
        status,
        journal,
        log,
        panics,
        replay_file: None,
        replay,
        no_replay,
        claimed,
    }
}

/// The session's replay, or why there is none.
fn session_replay(world: &World) -> (Option<Replay>, Option<String>) {
    if !world.get_resource::<replay::AiReplay>().is_some_and(replay::AiReplay::is_recording) {
        return (None, Some("this game does not record its session".to_owned()));
    }
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| replay::save(world))) {
        Ok(replay) => (Some(replay), None),
        Err(p) => (None, Some(format!("making the replay panicked: {}", crate::tools::panic_message(&p)))),
    }
}

/// A panic's message: its text, or `unknown panic`.
pub fn panic_text(payload: &(dyn std::any::Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_owned())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_owned())
}

/// A report of a panic that unwound out of the game (`payload`): the world went with it, so it
/// holds what the hook kept (the panic, its call stack, the game and session it happened in), the
/// log and the machine. Without the hook, only the message.
pub fn fatal(payload: &(dyn std::any::Any + Send)) -> CrashReport {
    let message = panic_text(payload);
    let current = std::thread::current();
    let thread = current.name().map_or_else(|| format!("{:?}", current.id()), str::to_owned);
    let (this, others, claimed, seen, log) = {
        let mut memory = memory();
        let index = memory
            .panics
            .iter()
            .rposition(|p| !p.handled && p.what.message == message && p.thread == current.id())
            .or_else(|| memory.panics.iter().rposition(|p| !p.handled && p.what.message == message));
        let this = index.map(|i| (taken(&memory.panics[i]), memory.panics[i].seen.clone()));
        let others: Vec<_> = memory
            .panics
            .iter()
            .enumerate()
            .filter(|(i, p)| Some(*i) != index && !p.reported)
            .map(|(_, p)| taken(p))
            .collect();
        let claimed = memory.panics.iter().enumerate().filter(|(i, p)| Some(*i) == index || !p.reported).map(|(_, p)| p.id).collect();
        memory.panics.iter_mut().for_each(|p| {
            p.handled = true;
            p.reported = true;
        });
        let last = memory.seen.last().cloned();
        (this, others, claimed, last, memory.log.iter().cloned().collect())
    };
    let (what, seen) = match this {
        Some((kept, then)) => (with_backtrace(kept), then.or(seen)),
        None => (What { headline: format!("thread '{thread}' panicked"), message, thread: Some(thread), ..default() }, seen),
    };
    CrashReport {
        format: FORMAT.to_owned(),
        time: utc(SystemTime::now()),
        what,
        game: seen.as_ref().map_or_else(|| game(&GameInfo::default()), |s| s.game.clone()),
        system: system(seen.as_ref().and_then(|s| s.gpu.clone())),
        session: seen.map(|s| s.session),
        status: Value::Null,
        journal: Vec::new(),
        log,
        panics: others.into_iter().map(with_backtrace).collect(),
        replay_file: None,
        replay: None,
        no_replay: Some("the panic unwound out of the game and took the world with it".to_owned()),
        claimed,
    }
}

impl CrashReport {
    /// `crash-20261016-123456-tick50.json`.
    pub fn file_name(&self) -> String {
        let stamp: String = self.time.chars().filter(char::is_ascii_digit).collect();
        let (date, time) = stamp.split_at(stamp.len().min(8));
        let tick = self.session.as_ref().map(|s| format!("-tick{}", s.tick)).unwrap_or_default();
        format!("crash-{date}-{time}{tick}.json")
    }
}

/// Writes the report into `crashes/` in the working directory, or, when that cannot be written
/// (a game installed where its player may not write), into the player's own folder for the game
/// (see [`user_dir`]). Returns where.
pub fn write(report: &CrashReport) -> Result<PathBuf, String> {
    let written = write_here_or_home(report);
    if written.is_err() {
        release(report);
    }
    written
}

fn write_here_or_home(report: &CrashReport) -> Result<PathBuf, String> {
    let here = std::env::current_dir().map_err(|e| format!("no working directory: {e}")).and_then(|dir| write_into(&dir, report));
    let Err(here) = here else { return here };
    let Some(dir) = user_dir() else { return Err(here) };
    write_file(&dir, report).map_err(|e| format!("{here}; {e}"))
}

/// Writes the report into `<base>/crashes/`. Returns where.
pub fn write_in(base: &Path, report: &CrashReport) -> Result<PathBuf, String> {
    let written = write_into(base, report);
    if written.is_err() {
        release(report);
    }
    written
}

fn write_into(base: &Path, report: &CrashReport) -> Result<PathBuf, String> {
    let example = format!("{CRASHES_DIR}/crash.json");
    let probe = crate::paths::inside_of(base, CRASHES_DIR, &format!("{CRASHES_DIR}/{}", report.file_name()), &example)?;
    write_file(probe.parent().unwrap_or(base), report)
}

/// Writes the report into `dir` under a name no other report has.
fn write_file(dir: &Path, report: &CrashReport) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let text = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    let name = report.file_name();
    let stem = name.trim_end_matches(".json");
    for n in 1..100 {
        let path = if n == 1 { dir.join(&name) } else { dir.join(format!("{stem}-{n}.json")) };
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                use std::io::Write;
                let written = file.write_all(text.as_bytes()).and_then(|()| file.flush());
                drop(file);
                return match written {
                    Ok(()) => Ok(path),
                    Err(e) => {
                        // Half a report is no report: the caller tries another folder.
                        let _ = std::fs::remove_file(&path);
                        Err(format!("cannot write {}: {e}", path.display()))
                    }
                };
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("cannot write {}: {e}", path.display())),
        }
    }
    Err(format!("cannot write {}: 99 reports of this second are there already", dir.join(&name).display()))
}

/// The player's own folder for this program's reports: `%LOCALAPPDATA%\<program>\crashes` on
/// Windows, `~/Library/Logs/<program>` on macOS, `$XDG_STATE_HOME/<program>/crashes` (or
/// `~/.local/state/<program>/crashes`) elsewhere.
pub fn user_dir() -> Option<PathBuf> {
    let program = std::env::current_exe().ok()?.file_stem()?.to_string_lossy().into_owned();
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        return Some(var("LOCALAPPDATA")?.join(program).join(CRASHES_DIR));
    }
    if cfg!(target_os = "macos") {
        return Some(var("HOME")?.join("Library/Logs").join(program));
    }
    let state = var("XDG_STATE_HOME").or_else(|| var("HOME").map(|h| h.join(".local/state")))?;
    Some(state.join(program).join(CRASHES_DIR))
}

/// `path` as the journal shows it: `crashes/<name>` when it is in the working directory's
/// `crashes/`, else as it is.
pub fn shown(path: &Path) -> String {
    let here = std::env::current_dir().ok().and_then(|d| crate::paths::canonical(&d.join(CRASHES_DIR)).ok());
    match (here, path.parent(), path.file_name()) {
        (Some(here), Some(parent), Some(name)) if parent == here => format!("{CRASHES_DIR}/{}", name.to_string_lossy()),
        _ => path.display().to_string(),
    }
}

/// Writes `report`, journals where (or why not), and returns where.
pub fn save(world: &mut World, report: &CrashReport) -> Option<String> {
    match write(report) {
        Ok(path) => {
            let shown = shown(&path);
            journal::log(world, JournalKind::Sim, format!("crash report saved: {shown} (send it with a bug report; --replay plays it)"));
            Some(shown)
        }
        Err(e) => {
            journal::log(world, JournalKind::Error, format!("crash report not saved: {e}"));
            None
        }
    }
}

/// Places that panicked off the main loop and already have a report: one report per place.
#[derive(Resource, Default)]
struct Reported(HashSet<String>);

/// Takes this app out of the process's memory when its world is dropped.
#[derive(Resource)]
struct Watched(u64);

impl Drop for Watched {
    fn drop(&mut self) {
        memory().seen.retain(|s| s.token != self.0);
    }
}

/// `Last`, after the failures of the frame are collected: remembers this app's game and session
/// (a panic that takes the world with it is reported with them), and reports panics nobody
/// handled (another thread: the render thread, a task), once per place.
fn watch(world: &mut World) {
    let token = crate::process::ensure_token(world);
    if !world.contains_resource::<Watched>() {
        world.insert_resource(Watched(token));
    }
    let session = session(world);
    {
        let mut memory = memory();
        match memory.seen.iter_mut().find(|s| s.token == token) {
            Some(seen) => {
                seen.session = session;
                // The adapter does not change: it is named once.
                if seen.gpu.is_none() {
                    seen.gpu = gpu(world);
                }
            }
            None => {
                let game = game(world.get_resource::<GameInfo>().unwrap_or(&GameInfo::default()));
                memory.seen.push(Seen { token, game, gpu: gpu(world), session });
            }
        }
    }
    if !world.get_resource::<crate::AiFailures>().is_some_and(|f| f.crash_replays) {
        return;
    }
    let unhandled = {
        let mut memory = memory();
        let found = memory.panics.iter_mut().find(|p| !p.handled && p.seen.as_ref().is_none_or(|s| s.token == token));
        // Reported now, or already at the same place.
        found.map(|p| {
            p.handled = true;
            p.reported = true;
            (p.id, taken(p))
        })
    };
    let Some((id, kept)) = unhandled else { return };
    let place = kept.0.location.clone().unwrap_or_else(|| kept.0.message.clone());
    if world.get_resource_or_init::<Reported>().0.contains(&place) {
        return;
    }
    let what = with_backtrace(kept);
    let headline = format!("{}: {}", what.headline, what.message);
    journal::log(world, JournalKind::Error, headline.clone());
    let mut report = report(world, what, None);
    report.claimed.push(id);
    // A place whose report could not be written gets one the next time it panics.
    if let Some(path) = save(world, &report) {
        world.get_resource_or_init::<Reported>().0.insert(place);
        eprintln!("error: {headline}; crash report: {path}");
    }
}

/// Write a crash report now, as a bug report: the log, the journal, `ai/status`, the machine (OS,
/// CPU, memory, GPU) and the session's replay in one file in crashes/ (`--replay` plays it).
#[derive(Deserialize, JsonSchema, Default)]
pub struct CrashReportArgs {
    /// What is wrong, in a sentence; it heads the report: "the boss fell through the floor".
    pub note: Option<String>,
    /// Where to write it, inside crashes/ in the game's working directory, e.g. "crashes/bug.json";
    /// a file of that name is replaced. Default: a new crashes/crash-<time>-tick<T>.json (or, when
    /// that folder cannot be written, one in the player's own folder).
    pub path: Option<String>,
}

/// A good path for the errors that refuse a bad one.
const CRASH_PATH_EXAMPLE: &str = "crashes/bug.json";

/// `requested` (a path inside `crashes/`, e.g. `crashes/bug.json`) as an absolute path inside
/// `<base>/crashes/`, with the folder created. Refuses an absolute path, `..` and a symlink
/// that leads out, with an example of a good one ([`crate::paths::inside_of`]).
pub fn path_in(base: &Path, requested: &str) -> Result<PathBuf, String> {
    crate::paths::inside_of(base, CRASHES_DIR, requested, CRASH_PATH_EXAMPLE)
}

/// Writes `report` to `path` (see [`path_in`]), replacing a file that is there: written beside it
/// and renamed, so a report that cannot be written leaves the old file as it was.
pub fn write_at(path: &Path, report: &CrashReport) -> Result<(), String> {
    use std::io::Write;
    let text = serde_json::to_string_pretty(report).map_err(|e| e.to_string())?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Each call makes a temporary file of its own: the first free `<name>.<pid>.<n>.part`, taken with
    // `create_new`. A name already there (another call writing the same report, a file left over, a
    // planted symlink) is skipped, never written through or removed; only the call's own file is.
    let mut taken = None;
    for n in 0..64 {
        let part = path.with_file_name(format!("{name}.{}.{n}.part", std::process::id()));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&part) {
            Ok(file) => {
                taken = Some((part, file));
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(format!("cannot write {}: {e}", path.display())),
        }
    }
    let Some((part, mut file)) = taken else {
        return Err(format!("cannot write {}: 64 temporary files beside it are taken", path.display()));
    };
    // The bytes reach the disk before the rename, so a power cut after it cannot leave an empty
    // report where the old one was.
    file.write_all(text.as_bytes())
        .and_then(|()| file.sync_all())
        .and_then(|()| {
            drop(file);
            std::fs::rename(&part, path)
        })
        .map_err(|e| {
            let _ = std::fs::remove_file(&part);
            format!("cannot write {}: {e}", path.display())
        })?;
    // On unix the new name is kept by syncing its folder too; a failure here loses nothing written.
    #[cfg(unix)]
    if let Some(dir) = path.parent() {
        let _ = std::fs::File::open(dir).and_then(|d| d.sync_all());
    }
    Ok(())
}

fn crash_report_tool(In(call): In<Call<CrashReportArgs>>, world: &mut World) -> ToolResult {
    let CrashReportArgs { note, path: requested } = call.args;
    // The path is checked before the report is made: a report holds the panics nobody has reported
    // yet, and a call that is refused must leave them to the next.
    let target = requested
        .map(|requested| {
            let base = std::env::current_dir().map_err(|e| format!("no working directory: {e}"))?;
            path_in(&base, &requested)
        })
        .transpose()?;
    let note = note.unwrap_or_default();
    let headline = if note.is_empty() { "bug report".to_owned() } else { format!("bug report: {note}") };
    let report = report(world, What { headline, message: note, ..default() }, None);
    let path = match target {
        Some(path) => match write_at(&path, &report) {
            Ok(()) => path,
            Err(e) => {
                release(&report);
                return Err(e);
            }
        },
        None => write(&report)?,
    };
    let replay = match (&report.replay, &report.no_replay) {
        (Some(r), _) => format!("a replay of {} ticks and {} events", r.end.tick, r.events.len()),
        (None, why) => format!("no replay: {}", why.as_deref().unwrap_or("none")),
    };
    Ok(format!(
        "saved {}: tick {}, {} log lines, {} journal entries, {replay}",
        shown(&path),
        report.session.as_ref().map_or(0, |s| s.tick),
        report.log.len(),
        report.journal.len()
    ))
}

pub(crate) fn plugin(app: &mut App, game: GameInfo) {
    app.insert_resource(game).init_resource::<Reported>().add_systems(Last, watch.after(crate::failures::collect_between_ticks));
    register_tool(app.world_mut(), "crash_report", Scope::Dev, ToolKind::Query, crash_report_tool);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_counts_days_like_a_calendar() {
        let at = |secs: u64| utc(UNIX_EPOCH + std::time::Duration::from_secs(secs));
        assert_eq!(at(0), "1970-01-01T00:00:00Z");
        assert_eq!(at(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(at(1_700_000_000), "2023-11-14T22:13:20Z");
        assert_eq!(at(1_791_929_599), "2026-10-13T22:13:19Z");
    }

    fn sample() -> CrashReport {
        CrashReport {
            format: FORMAT.to_owned(),
            time: "2026-10-16T12:34:56Z".to_owned(),
            what: What { headline: "bug report".to_owned(), ..default() },
            game: game(&GameInfo { title: "Test".to_owned(), version: Some("1.2.3".to_owned()) }),
            system: system(None),
            session: Some(Session { tick: 50, seed: 7, tick_hz: 60.0 }),
            status: Value::Null,
            journal: Vec::new(),
            log: vec!["50 WARN game: careful".to_owned()],
            panics: Vec::new(),
            replay_file: None,
            replay: None,
            no_replay: Some("this game does not record its session".to_owned()),
            claimed: Vec::new(),
        }
    }

    #[test]
    fn reports_are_named_by_their_time_and_tick_and_never_overwrite_each_other() {
        let report = sample();
        assert_eq!(report.file_name(), "crash-20261016-123456-tick50.json");
        assert_eq!(CrashReport { session: None, ..sample() }.file_name(), "crash-20261016-123456.json");

        let base = std::env::temp_dir().join(format!("ai_core_crash_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let first = write_in(&base, &report).unwrap();
        let second = write_in(&base, &report).unwrap();
        assert_eq!(first.file_name().unwrap(), "crash-20261016-123456-tick50.json");
        assert_eq!(second.file_name().unwrap(), "crash-20261016-123456-tick50-2.json");
        assert_eq!(first.parent(), Some(crate::paths::canonical(&base.join(CRASHES_DIR)).unwrap().as_path()));
        let read: CrashReport = serde_json::from_str(&std::fs::read_to_string(&first).unwrap()).unwrap();
        assert_eq!(read, report);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn a_report_asked_to_a_path_goes_there_and_replaces_the_one_before() {
        let base = std::env::temp_dir().join(format!("ai_core_crash_path_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let folder = || std::fs::read_dir(base.join(CRASHES_DIR)).unwrap().map(|e| e.unwrap().file_name().to_string_lossy().into_owned()).collect::<Vec<_>>();
        let read = |path: &Path| -> CrashReport { serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap() };

        let first = CrashReport { session: Some(Session { tick: 50, seed: 7, tick_hz: 60.0 }), ..sample() };
        let path = path_in(&base, "crashes/bug.json").unwrap();
        assert_eq!(path, crate::paths::canonical(&base.join(CRASHES_DIR)).unwrap().join("bug.json"));
        write_at(&path, &first).unwrap();
        assert_eq!(read(&path), first);

        // The same path again is the same file, replaced: nothing else is left in the folder.
        let second = CrashReport { session: Some(Session { tick: 90, seed: 7, tick_hz: 60.0 }), ..sample() };
        assert_eq!(path_in(&base, "crashes/./bug.json").unwrap(), path);
        write_at(&path, &second).unwrap();
        assert_eq!(read(&path), second);
        assert_eq!(folder(), ["bug.json"]);

        // A symlink waiting under the first temporary name is skipped: not written through, not removed.
        #[cfg(unix)]
        {
            let outside = base.join("outside.txt");
            std::fs::write(&outside, "keep").unwrap();
            let planted = path.with_file_name(format!("bug.json.{}.0.part", std::process::id()));
            std::os::unix::fs::symlink(&outside, &planted).unwrap();
            write_at(&path, &first).unwrap();
            assert_eq!(std::fs::read_to_string(&outside).unwrap(), "keep");
            assert_eq!(read(&path), first);
            let mut names = folder();
            names.sort();
            assert_eq!(names, ["bug.json".to_string(), format!("bug.json.{}.0.part", std::process::id())]);
            std::fs::remove_file(&planted).unwrap();
        }

        // Calls writing the same report at once each use a temporary file of their own: all of them
        // succeed, the report is whole, and no temporary file is left.
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| {
                    for _ in 0..10 {
                        write_at(&path, &second).unwrap();
                    }
                });
            }
        });
        assert_eq!(read(&path), second);
        assert_eq!(folder(), ["bug.json"]);

        // A folder below crashes/ is made.
        let deeper = path_in(&base, "crashes/mine/bug.json").unwrap();
        write_at(&deeper, &first).unwrap();
        assert_eq!(read(&deeper), first);

        // Anything outside crashes/ is refused with a path that works, and nothing is written.
        let example = "path must be inside crashes/, e.g. crashes/bug.json";
        for (bad, why) in [
            ("bug.json", "is outside crashes/"),
            ("replays/bug.json", "is outside crashes/"),
            ("crashes/../bug.json", "goes up with `..`"),
            ("../crashes/bug.json", "goes up with `..`"),
            ("crashes", "names no file"),
            (if cfg!(windows) { "C:\\bug.json" } else { "/tmp/bug.json" }, "is absolute"),
        ] {
            let refused = path_in(&base, bad).unwrap_err();
            assert!(refused.starts_with(example) && refused.contains(why), "`{bad}`: {refused}");
        }
        assert!(!base.join("bug.json").exists() && !base.join("replays").exists(), "nothing outside crashes/");
        let _ = std::fs::remove_dir_all(base);
    }

    /// The kept panics are the process's: a report built by one test claims the panics another is
    /// about to handle. Tests that read them take turns.
    fn alone() -> MutexGuard<'static, ()> {
        static TURN: Mutex<()> = Mutex::new(());
        TURN.lock().unwrap_or_else(PoisonError::into_inner)
    }

    #[test]
    fn each_thread_handles_its_own_panic_of_the_same_words() {
        let _alone = alone();
        install_panic_hook();
        let message = "crash test: the same words on two threads";
        let here = std::thread::current().name().map(str::to_owned);
        assert!(std::panic::catch_unwind(|| panic!("{message}")).is_err());
        // Another thread panics with the same words after this one, and does not handle it yet.
        std::thread::Builder::new()
            .name("crash test: other".to_owned())
            .spawn(move || assert!(std::panic::catch_unwind(|| panic!("{message}")).is_err()))
            .unwrap()
            .join()
            .unwrap();
        assert_eq!(handled(message).and_then(|what| what.thread), here, "this thread's own, though the other is newer");
        assert_eq!(handled(message).and_then(|what| what.thread).as_deref(), Some("crash test: other"));
        assert!(handled(message).is_none(), "both are handled");
    }

    #[test]
    fn a_report_that_cannot_be_written_leaves_its_panics_to_the_next() {
        let _alone = alone();
        install_panic_hook();
        let message = "crash test: a panic in a report that is never written";
        assert!(std::panic::catch_unwind(|| panic!("{message}")).is_err());
        // Caught here, as a tool's answer is: no report of its own, a line in the next one.
        assert!(handled(message).is_some());
        let mut world = World::new();
        world.init_resource::<crate::sim::SimTick>();
        let bug = || report(&world, What { headline: "bug report".to_owned(), ..default() }, None);
        let lists = |report: &CrashReport| report.panics.iter().any(|p| p.message == message);
        let first = bug();
        assert!(lists(&first));
        // A file where the folder would be: nothing can be written under it.
        let blocked = std::env::temp_dir().join(format!("ai_core_crash_blocked_{}", std::process::id()));
        std::fs::write(&blocked, "").unwrap();
        assert!(write_in(&blocked, &first).is_err());
        let second = bug();
        assert!(lists(&second), "the report that was not written left it to the next");
        let base = std::env::temp_dir().join(format!("ai_core_crash_written_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        write_in(&base, &second).unwrap();
        assert!(!lists(&bug()), "the written one holds it");
        let _ = std::fs::remove_file(blocked);
        let _ = std::fs::remove_dir_all(base);
    }

    #[test]
    fn the_machine_is_described() {
        let system = system(Some("GPU".to_owned()));
        assert!(!system.os.is_empty() && system.arch == std::env::consts::ARCH, "{system:?}");
        assert_eq!(system.gpu, "GPU");
        if cfg!(any(windows, target_os = "linux")) {
            assert!(system.memory_mb.is_some_and(|m| m > 0), "{system:?}");
        }
        if cfg!(windows) {
            assert!(system.os.starts_with("Windows") && system.os.contains("(10."), "{system:?}");
        }
        assert_eq!(super::system(None).gpu, "none (no renderer: headless)");
    }
}
