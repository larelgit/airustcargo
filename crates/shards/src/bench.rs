//! The performance autotest (`--perf ID`) and what `ai/status` counts (`counts`).
//!
//! A `perf` content definition ([`PerfDef`], `content/**/*.perf.ron`) names a scene, how long to
//! warm up and to measure, and budgets. The run loads the game, goes to the scene (the game's start
//! world, or a region generated from a seed: its load is timed, and the bake of its navigation when
//! that is a budget), runs the setup lines, warms up,
//! measures, and gives a [`Verdict`] per budget. Headless it measures ticks, as fast as they go;
//! `--offscreen` the game plays in real time and draws as fast as the GPU allows, and the frames
//! ([`crate::frames`]), the GPU's time and the triangles it drew are measured too. A debug build
//! measures without judging: the budgets are for `--release`.

use std::time::{Duration, Instant};

use ai_core::perf::TickStats;
use ai_core::sim::{StepEnd, StepReport};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shards_content::{ContentAppExt, ContentDef, Refs};
use shards_core::EngineConfig;

/// The agent the setup lines and the region's `regen` run as.
const AGENT: &str = "perf";

/// Ticks a warmup or a measurement may lag behind real time before the run gives up: several times
/// their own duration, and a minute more.
fn wall_limit(ticks: u64, timestep: Duration) -> Duration {
    let lag = Duration::try_from_secs_f64(timestep.as_secs_f64() * ticks as f64 * 4.0).unwrap_or(Duration::MAX);
    lag.saturating_add(Duration::from_secs(60))
}

/// The most ticks a warmup or a measurement plays: an hour at 60 Hz.
pub const MAX_TICKS: u64 = 216_000;

/// A performance autotest (`content/**/*.perf.ron`), played by `--perf ID`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PerfDef {
    /// A region (or dungeon) to generate and go to first; its load is timed (`region_load_s`).
    /// `None`: the game's start world.
    #[serde(default)]
    pub region: Option<PerfRegion>,
    /// Lines run in order once the world is there, as `shards act` and the console take them
    /// (`god`, `goto_tile x=5 y=5`, `spawn_prefab id=skeleton x=24 z=136`). Each must succeed; a
    /// load one starts is waited out.
    #[serde(default)]
    pub setup: Vec<String>,
    /// The picture `--offscreen` draws, in pixels. Default: the game's window.
    #[serde(default)]
    pub size: Option<(u32, u32)>,
    /// Ticks played before measuring (shaders compile, caches fill). Default 120.
    #[serde(default = "default_warmup")]
    pub warmup: u64,
    /// Ticks measured. Default 600: ten seconds at 60 Hz.
    #[serde(default = "default_ticks")]
    pub ticks: u64,
    /// What the measured run must hold to.
    #[serde(default)]
    pub budgets: Budgets,
}

fn default_warmup() -> u64 {
    120
}

fn default_ticks() -> u64 {
    600
}

/// A region and its seed: `(region: "wilds", seed: 42)`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct PerfRegion {
    /// A `region` or `dungeon` content id.
    pub region: String,
    pub seed: u64,
}

/// Budgets: a missing one is not checked, except that ticks (and frames, offscreen) are always held
/// to their budget, `over_share` of them at most going over.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct Budgets {
    /// A tick slower than this is over budget. Default: the tick's own duration (16.7 ms at 60 Hz).
    #[serde(default)]
    pub tick_ms: Option<f64>,
    /// The average tick, at most.
    #[serde(default)]
    pub tick_avg_ms: Option<f64>,
    /// A frame slower than this is over budget (`--offscreen`). Default 1/60 s.
    #[serde(default)]
    pub frame_ms: Option<f64>,
    /// The average frame, at most.
    #[serde(default)]
    pub frame_avg_ms: Option<f64>,
    /// The share of ticks, and of frames, that may go over their budget. Default 0.01.
    #[serde(default)]
    pub over_share: Option<f64>,
    /// The GPU's time per frame, on average, at most (where the GPU measures it).
    #[serde(default)]
    pub gpu_avg_ms: Option<f64>,
    /// Seconds the region may take to generate and load.
    #[serde(default)]
    pub region_load_s: Option<f64>,
    /// Seconds baking the navigation of every tile the region uses may take, every body class
    /// (`--bake-nav` for that region, nothing written). Measured only when the budget is named:
    /// big tiles take a while.
    ///
    /// Left out of a definition's JSON while unset (as `draws`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nav_bake_s: Option<f64>,
    /// Entities at the end, at most.
    #[serde(default)]
    pub entities: Option<u64>,
    /// Meshes some view draws at the end (a camera, or a light into its shadow map), at most: the
    /// draws before Bevy batches them.
    #[serde(default)]
    pub visible_meshes: Option<u64>,
    /// Distinct meshes among those drawn at the end, at most: instances of one mesh (the scatter's
    /// grass) with one material are one batch on the GPU, so this bounds the batches better than
    /// `visible_meshes`.
    #[serde(default)]
    pub mesh_assets: Option<u64>,
    /// Triangles per frame, on average, at most (where the GPU counts them).
    #[serde(default)]
    pub triangles: Option<u64>,
    /// Draw commands per frame, on average, at most: what the render phases issue once Bevy has
    /// batched them, every view's (cameras, shadow maps) summed, a multi-draw once ([`crate::draws`]
    /// says what is counted). Where Bevy keeps the GPU's batches to itself the budget is not judged.
    ///
    /// Left out of a definition's JSON while unset: the content hash (which replays compare) is
    /// over that JSON, so a definition that does not name the budget keeps the hash it had before
    /// the field existed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub draws: Option<u64>,
}

/// The share of ticks and frames that may go over their budget unless the definition says.
pub const OVER_SHARE: f64 = 0.01;

impl Budgets {
    pub fn over_share(&self) -> f64 {
        self.over_share.unwrap_or(OVER_SHARE)
    }
}

impl ContentDef for PerfDef {
    const KIND: &'static str = "perf";
    // Only what a machine shows or plays, or what lies outside a session: never in the replay check.
    const SIMULATION: bool = false;

    fn validate(&self, _: &str, refs: &mut Refs) {
        if let Some(PerfRegion { region, .. }) = &self.region {
            let db = refs.db();
            let known = || db.ids::<shards_world::content::RegionDef>().chain(db.ids::<shards_world::content::DungeonDef>());
            if !known().any(|id| id == region) {
                let hint = shards_content::db::closest(region, known()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                refs.error(format!("region: no `region` or `dungeon` is named `{region}`{hint}"));
            }
        }
        for (i, line) in self.setup.iter().enumerate() {
            if let Err(why) = ai_core::dev::parse_line(line) {
                refs.error(format!("setup[{i}]: {why}"));
            }
        }
        if self.size.is_some_and(|(w, h)| w == 0 || h == 0) {
            refs.error("size: a picture is at least 1x1 pixels");
        }
        if self.ticks == 0 {
            refs.error("ticks: measure at least one tick (600 is ten seconds at 60 Hz)");
        }
        for (name, ticks) in [("warmup", self.warmup), ("ticks", self.ticks)] {
            if ticks > MAX_TICKS {
                refs.error(format!("{name}: at most {MAX_TICKS} ticks (an hour at 60 Hz), not {ticks}"));
            }
        }
        let b = &self.budgets;
        for (name, value) in [
            ("tick_ms", b.tick_ms),
            ("tick_avg_ms", b.tick_avg_ms),
            ("frame_ms", b.frame_ms),
            ("frame_avg_ms", b.frame_avg_ms),
            ("gpu_avg_ms", b.gpu_avg_ms),
            ("region_load_s", b.region_load_s),
            ("nav_bake_s", b.nav_bake_s),
        ] {
            if value.is_some_and(|v| !(v.is_finite() && v > 0.0)) {
                refs.error(format!("budgets.{name}: a budget is a number above 0"));
            }
        }
        // The game holds these two as durations, as `--tick-budget-ms` and `--frame-budget-ms` do.
        for (name, value) in [("tick_ms", b.tick_ms), ("frame_ms", b.frame_ms)] {
            match value.filter(|v| v.is_finite() && *v > 0.0).map(|v| Duration::try_from_secs_f64(v / 1000.0)) {
                Some(Ok(budget)) if budget.is_zero() => refs.error(format!("budgets.{name}: the budget is below 1 ns")),
                Some(Err(_)) => refs.error(format!("budgets.{name}: the budget is too large")),
                _ => {}
            }
        }
        if b.over_share.is_some_and(|s| !(0.0..=1.0).contains(&s)) {
            refs.error("budgets.over_share: a share from 0 (none may go over) to 1 (all may)");
        }
        for (name, set) in [("region_load_s", b.region_load_s.is_some()), ("nav_bake_s", b.nav_bake_s.is_some())] {
            if set && self.region.is_none() {
                refs.error(format!("budgets.{name}: no region is loaded; name one in `region`, or drop the budget"));
            }
        }
    }
}

/// The `perf` content kind and the `counts` field of `ai/status`.
pub(crate) struct PerfPlugin;

impl Plugin for PerfPlugin {
    fn build(&self, app: &mut App) {
        use ai_core::AiAppExt;
        app.add_content::<PerfDef>().add_ai_status("counts", counts).add_ai_status("frames", frames_status);
    }
}

/// `counts` in `ai/status`: the entities of the world, its meshes, the meshes some view draws (a
/// camera, or a light into its shadow map) and the meshes the active cameras draw, each camera's
/// counted (the draws of the picture before Bevy batches them). Headless nothing is drawn: 0.
pub fn counts(world: &World) -> Value {
    let meshes = mesh_counts(world);
    json!({
        "entities": world.entities().count_spawned(),
        "meshes": meshes.all,
        "visible_meshes": meshes.visible,
        "camera_meshes": meshes.in_cameras,
        "mesh_assets": meshes.assets,
    })
}

/// Meshes: all, those some view draws, those the active cameras draw, and the distinct mesh assets
/// among those drawn (instances of one mesh with one material are one batch on the GPU).
#[derive(Default)]
struct MeshCounts {
    all: u64,
    visible: u64,
    in_cameras: u64,
    assets: u64,
}

#[cfg(feature = "render")]
fn mesh_counts(world: &World) -> MeshCounts {
    use bevy::camera::visibility::VisibleEntities;
    let mut counts = MeshCounts::default();
    if let Some(mut meshes) = world.try_query::<(Option<&ViewVisibility>, &Mesh3d)>() {
        let mut assets = std::collections::HashSet::new();
        for (seen, mesh) in meshes.iter(world) {
            counts.all += 1;
            if seen.is_some_and(|v| v.get()) {
                counts.visible += 1;
                assets.insert(mesh.0.id());
            }
        }
        counts.assets = assets.len() as u64;
    }
    if let Some(mut cameras) = world.try_query::<(&Camera, &VisibleEntities)>() {
        let mesh = std::any::TypeId::of::<Mesh3d>();
        counts.in_cameras = cameras.iter(world).filter(|(camera, _)| camera.is_active).map(|(_, seen)| seen.len(mesh) as u64).sum();
    }
    counts
}

#[cfg(not(feature = "render"))]
fn mesh_counts(_: &World) -> MeshCounts {
    MeshCounts::default()
}

/// `frames` in `ai/status` ([`crate::frames`]); `null` headless.
fn frames_status(world: &World) -> Value {
    #[cfg(feature = "render")]
    return crate::frames::status(world);
    #[cfg(not(feature = "render"))]
    {
        let _ = world;
        Value::Null
    }
}

/// `--perf ID`'s definition, read from the content on disk before the game is built for it (it
/// names the picture's size): `build` makes an app headless that only lends its content kinds.
pub(crate) fn read(build: &impl Fn(&EngineConfig) -> App, config: &EngineConfig, id: &str) -> Result<PerfDef, String> {
    let config = EngineConfig { headless: true, offscreen: false, testing: true, port: None, hot_reload: false, ..config.clone() };
    let app = build(&config);
    let kinds = app.world().resource::<shards_content::ContentKinds>().clone();
    let root = shards_content::asset_root(app.world());
    let sources = shards_content::db::read_dir(&root).map_err(|e| e.to_string())?;
    let db = shards_content::ContentDb::build(&kinds, &sources);
    let problems: Vec<String> = db.errors().filter(|e| e.kind.as_deref() == Some(PerfDef::KIND)).map(ToString::to_string).collect();
    if let Some(def) = db.get::<PerfDef>(id).filter(|_| problems.is_empty()) {
        return Ok(def.clone());
    }
    if !problems.is_empty() {
        return Err(format!("the `perf` content has problems (`--check-content` lists them all):\n{}", problems.join("\n")));
    }
    let ids: Vec<&str> = db.ids::<PerfDef>().collect();
    Err(match (shards_content::db::closest(id, ids.iter().copied()), ids.is_empty()) {
        (_, true) => format!("no `perf` definition `{id}`: there is none in {} (`**/*.perf.ron`)", root.join("content").display()),
        (Some(close), false) => format!("no `perf` definition `{id}` (did you mean `{close}`?)"),
        (None, false) => format!("no `perf` definition `{id}`; there are: {}", ids.join(", ")),
    })
}

/// The config `--perf` builds the game with: no server, no files, paused until it measures; the
/// picture and the budgets the definition names.
pub(crate) fn config(def: &PerfDef, config: EngineConfig) -> EngineConfig {
    let ms = |ms: f64| Duration::from_secs_f64(ms / 1000.0);
    EngineConfig {
        headless: !config.offscreen,
        port: None,
        paused: true,
        crash_replays: false,
        hot_reload: false,
        report_perf: false,
        settings: shards_core::SettingsStore::Off,
        window: def.size.unwrap_or(config.window),
        tick_budget: def.budgets.tick_ms.map(ms).or(config.tick_budget),
        frame_budget: def.budgets.frame_ms.map(ms).or(config.frame_budget),
        ..config
    }
}

/// Frames as the run measured them.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FrameSummary {
    pub frames: u64,
    pub avg: Duration,
    pub max: Duration,
    pub over_budget: u64,
    pub budget: Duration,
    pub fps: Option<f64>,
    pub main: Duration,
    pub render: Duration,
    pub gpu_avg_ms: Option<f64>,
    pub triangles: Option<f64>,
    /// Draw commands per frame, on average ([`crate::draws`]); `None`: this GPU keeps its batches private.
    pub draws: Option<f64>,
}

/// What a run measured.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Measured {
    /// The time to load the game, then to generate and load the region (if one was named).
    pub load: Duration,
    pub region_load: Option<Duration>,
    /// How many bakes (tiles times body classes) the region's navigation took, and how long
    /// (when `nav_bake_s` is a budget).
    pub nav_bake: Option<(usize, Duration)>,
    pub ticks: TickStats,
    pub tick_budget: Duration,
    /// The real time the measured ticks took.
    pub wall: Duration,
    /// `None`: nothing was drawn (headless).
    pub frames: Option<FrameSummary>,
    pub entities: u64,
    pub meshes: u64,
    pub visible_meshes: u64,
    pub camera_meshes: u64,
    pub mesh_assets: u64,
}

impl Measured {
    /// `600 ticks in 10.02s: avg 0.84 ms, max 3.10 ms (tick 712); 1431 frames, 142.8 fps: avg 6.90
    /// ms, max 14.20 ms (main 2.10, render 6.90, GPU 5.50 ms), 123456 triangles; 4211 entities, 1022
    /// meshes, 312 drawn (41 distinct), 140 in cameras`.
    pub fn line(&self) -> String {
        let t = &self.ticks;
        let mut line = format!(
            "{} ticks in {:.2}s: avg {:.2} ms, max {:.2} ms (tick {})",
            t.ticks,
            self.wall.as_secs_f64(),
            ms(t.avg()),
            ms(t.max),
            t.worst_tick
        );
        if let Some(f) = &self.frames {
            let fps = f.fps.map(|fps| format!(", {fps:.1} fps")).unwrap_or_default();
            let gpu = f.gpu_avg_ms.map(|g| format!(", GPU {g:.2}")).unwrap_or_default();
            line.push_str(&format!(
                "; {} frames{fps}: avg {:.2} ms, max {:.2} ms (main {:.2}, render {:.2}{gpu} ms)",
                f.frames,
                ms(f.avg),
                ms(f.max),
                ms(f.main),
                ms(f.render)
            ));
            if let Some(triangles) = f.triangles {
                line.push_str(&format!(", {triangles:.0} triangles"));
            }
            if let Some(draws) = f.draws {
                line.push_str(&format!(", {draws:.0} draws"));
            }
        }
        line.push_str(&format!(
            "; {} entities, {} meshes, {} drawn ({} distinct), {} in cameras",
            self.entities, self.meshes, self.visible_meshes, self.mesh_assets, self.camera_meshes
        ));
        line
    }
}

/// How a run went against one budget.
#[derive(Clone, Debug, PartialEq)]
pub struct Verdict {
    /// `Some(true)` met, `Some(false)` exceeded, `None` not measured here or not judged.
    pub met: Option<bool>,
    pub text: String,
}

impl Verdict {
    /// `ok    tick avg 0.84 ms (at most 4.00 ms)`, `OVER  …`, `-     …`.
    pub fn line(&self) -> String {
        let mark = match self.met {
            Some(true) => "ok  ",
            Some(false) => "OVER",
            None => "-   ",
        };
        format!("{mark}  {}", self.text)
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

/// A per-frame average as a budget line shows it: whole when it is, else to a tenth, so an average
/// just over a whole budget (900.4 against 900) does not read as the budget itself.
fn per_frame(n: f64) -> String {
    if n.fract() == 0.0 { format!("{n:.0}") } else { format!("{n:.1}") }
}

/// Every budget of `budgets` against what the run `measured`. `judged` off (a debug build): the
/// same lines, none met or exceeded.
pub fn judge(budgets: &Budgets, measured: &Measured, judged: bool) -> Vec<Verdict> {
    let mut verdicts = Vec::new();
    let mut check = |met: Option<bool>, text: String| verdicts.push(Verdict { met: met.filter(|_| judged), text });
    let share = budgets.over_share();
    let over = |what: &str, over: u64, of: u64, budget: Duration| {
        let part = if of == 0 { 0.0 } else { over as f64 / of as f64 };
        let text = format!(
            "{what} over {:.1} ms: {over} of {of}, {:.1}% (at most {:.1}%)",
            ms(budget),
            part * 100.0,
            share * 100.0
        );
        (part <= share, text)
    };
    if let Some(budget) = budgets.region_load_s {
        let text = match measured.region_load {
            Some(load) => format!("region load {:.2} s (at most {budget:.1} s)", load.as_secs_f64()),
            None => "region load: no region was loaded".to_owned(),
        };
        check(measured.region_load.map(|load| load.as_secs_f64() <= budget), text);
    }
    if let Some(budget) = budgets.nav_bake_s {
        let text = match measured.nav_bake {
            Some((bakes, took)) => format!("nav bake {:.2} s, {bakes} tiles x body classes (at most {budget:.1} s)", took.as_secs_f64()),
            None => "nav bake: no region was baked".to_owned(),
        };
        check(measured.nav_bake.map(|(_, took)| took.as_secs_f64() <= budget), text);
    }
    let ticks = &measured.ticks;
    if let Some(budget) = budgets.tick_avg_ms {
        check(Some(ms(ticks.avg()) <= budget), format!("tick avg {:.2} ms (at most {budget:.2} ms)", ms(ticks.avg())));
    }
    let (met, text) = over("ticks", ticks.over_budget, ticks.ticks, measured.tick_budget);
    check(Some(met), text);
    let frames = measured.frames;
    let not_drawn = |what: &str| format!("{what}: nothing is drawn headless (`--offscreen` draws)");
    if let Some(budget) = budgets.frame_avg_ms {
        match &frames {
            Some(f) => check(Some(ms(f.avg) <= budget), format!("frame avg {:.2} ms (at most {budget:.2} ms)", ms(f.avg))),
            None => check(None, not_drawn("frame avg")),
        }
    }
    match &frames {
        Some(f) => {
            let (met, text) = over("frames", f.over_budget, f.frames, f.budget);
            check(Some(met), text);
        }
        None if budgets.frame_ms.is_some() => check(None, not_drawn("frames over budget")),
        None => {}
    }
    if let Some(budget) = budgets.gpu_avg_ms {
        match frames.map(|f| f.gpu_avg_ms) {
            Some(Some(gpu)) => check(Some(gpu <= budget), format!("GPU {gpu:.2} ms a frame (at most {budget:.2} ms)")),
            Some(None) => check(None, "GPU time: this GPU (or build) measures none".to_owned()),
            None => check(None, not_drawn("GPU time")),
        }
    }
    if let Some(budget) = budgets.triangles {
        match frames.map(|f| f.triangles) {
            Some(Some(n)) => check(Some(n <= budget as f64), format!("triangles {} a frame (at most {budget})", per_frame(n))),
            Some(None) => check(None, "triangles: this GPU (or build) counts none".to_owned()),
            None => check(None, not_drawn("triangles")),
        }
    }
    if let Some(budget) = budgets.draws {
        match frames.map(|f| f.draws) {
            Some(Some(n)) => check(Some(n <= budget as f64), format!("draws {} a frame (at most {budget})", per_frame(n))),
            Some(None) => check(None, "draws: this GPU keeps its batches private, none are counted".to_owned()),
            None => check(None, not_drawn("draws")),
        }
    }
    if let Some(budget) = budgets.entities {
        check(Some(measured.entities <= budget), format!("entities {} (at most {budget})", measured.entities));
    }
    if let Some(budget) = budgets.visible_meshes {
        match &frames {
            Some(_) => check(
                Some(measured.visible_meshes <= budget),
                format!("visible meshes {} (at most {budget})", measured.visible_meshes),
            ),
            None => check(None, not_drawn("visible meshes")),
        }
    }
    if let Some(budget) = budgets.mesh_assets {
        match &frames {
            Some(_) => check(
                Some(measured.mesh_assets <= budget),
                format!("distinct meshes drawn {} (at most {budget})", measured.mesh_assets),
            ),
            None => check(None, not_drawn("distinct meshes drawn")),
        }
    }
    verdicts
}

/// Plays `def` on `app` (finished; paused, as `--perf` builds it, or a test app): waits for loading,
/// goes to the scene, warms up and measures. Headless the ticks run as fast as they go; offscreen
/// the game plays in real time for them, and its frames are measured too. `Err`: what stopped it
/// (the game did not load, a setup line failed, a gameplay system panicked, an invariant broke,
/// the game fell behind real time).
pub fn play(app: &mut App, def: &PerfDef) -> Result<Measured, String> {
    let started = Instant::now();
    crate::testing::wait_until_loaded(app, crate::testing::LOAD_TIMEOUT)?;
    let region_load = match &def.region {
        Some(PerfRegion { region, seed }) => {
            let started = Instant::now();
            let out = ai_core::call_tool(app.world_mut(), AGENT, "regen", json!({"region": region, "seed": seed}));
            if !out.ok {
                return Err(format!("regen region={region} seed={seed}: {}", out.result.as_str().unwrap_or(&out.result.to_string())));
            }
            crate::testing::wait_until_loaded(app, crate::testing::LOAD_TIMEOUT)?;
            Some(started.elapsed())
        }
        None => None,
    };
    let load = started.elapsed();
    let nav_bake = match (&def.region, def.budgets.nav_bake_s) {
        (Some(_), Some(_)) => Some(shards_npc::bake_region(app.world()).map_err(|why| format!("nav bake: {why}"))?),
        _ => None,
    };
    let halts = |world: &World| world.get_resource::<ai_core::sim::Halts>().map_or(0, |h| h.count);
    for line in &def.setup {
        let before = halts(app.world());
        shards_dev::run_console_line(app.world_mut(), AGENT, line).map_err(|why| format!("setup `{line}`: {why}"))?;
        if halts(app.world()) > before {
            return Err(format!("setup `{line}`: {}", stop_reason(app.world())));
        }
        crate::testing::wait_until_loaded(app, crate::testing::LOAD_TIMEOUT)?;
    }
    let drawn = app.world().get_resource::<ai_core::display::Offscreen>().is_some();
    let timestep = app.world().resource::<Time<Fixed>>().timestep();
    let (ticks, wall, frames) = if drawn { real_time(app, def, timestep)? } else { as_fast_as_they_go(app, def)? };
    let world = app.world();
    let budget = world.resource::<ai_core::perf::AiPerf>().budget_for(timestep);
    let counts = counts(world);
    let count = |key: &str| counts[key].as_u64().unwrap_or(0);
    Ok(Measured {
        load,
        region_load,
        nav_bake,
        ticks,
        tick_budget: budget,
        wall,
        frames,
        entities: count("entities"),
        meshes: count("meshes"),
        visible_meshes: count("visible_meshes"),
        camera_meshes: count("camera_meshes"),
        mesh_assets: count("mesh_assets"),
    })
}

/// Why the game halted: the last halt's reason.
fn stop_reason(world: &World) -> String {
    world.get_resource::<ai_core::sim::Halts>().and_then(|h| h.last.as_ref()).map_or_else(|| "the game halted".into(), |h| h.reason.clone())
}

/// Headless: the warmup, then the measured ticks, each in one `step`.
fn as_fast_as_they_go(app: &mut App, def: &PerfDef) -> Result<(TickStats, Duration, Option<FrameSummary>), String> {
    let world = app.world_mut();
    let timestep = world.resource::<Time<Fixed>>().timestep();
    let mut run = |ticks: u64, what: &str| -> Result<StepReport, String> {
        let report = ai_core::sim::step(world, ticks, wall_limit(ticks, timestep), |_| None)?;
        match &report.end {
            StepEnd::Completed => Ok(report),
            StepEnd::SystemFailed(failures) => {
                let f = &failures[0];
                Err(format!("gameplay {}: {}", f.headline(), f.message))
            }
            StepEnd::InvariantViolated(violated) => Err(violated.iter().map(|v| format!("{}: {}", v.headline(), v.text)).collect::<Vec<_>>().join("; ")),
            StepEnd::HeavyFailed(failed) => Err(failed.iter().map(|h| format!("{}: {}", h.headline(), h.first())).collect::<Vec<_>>().join("; ")),
            StepEnd::WallClock => Err(format!("the {what} ran {} of {ticks} ticks in {:.0}s", report.ticks(), report.wall.as_secs_f64())),
            StepEnd::Condition(why) => Err(format!("the {what} stopped at tick {}: {why}", report.to_tick)),
        }
    };
    if def.warmup > 0 {
        run(def.warmup, "warmup")?;
    }
    let report = run(def.ticks, "measurement")?;
    Ok((report.timing, report.wall, None))
}

/// Offscreen: the game plays in real time, a frame after another as fast as the GPU draws them,
/// through the warmup, then the measured ticks.
#[cfg(feature = "render")]
fn real_time(app: &mut App, def: &PerfDef, timestep: Duration) -> Result<(TickStats, Duration, Option<FrameSummary>), String> {
    let halts = |world: &World| world.get_resource::<ai_core::sim::Halts>().map_or(0, |h| h.count);
    let before = halts(app.world());
    let play = |app: &mut App, ticks: u64, what: &str| -> Result<Duration, String> {
        let (from, started) = (ai_core::sim::tick(app.world()), Instant::now());
        let limit = wall_limit(ticks, timestep);
        ai_core::sim::resume(app.world_mut());
        while ai_core::sim::tick(app.world()) < from + ticks {
            app.update();
            if halts(app.world()) > before {
                return Err(stop_reason(app.world()));
            }
            if let Some(exit) = app.should_exit() {
                return Err(format!("the game ended ({exit:?})"));
            }
            if started.elapsed() > limit {
                let ran = ai_core::sim::tick(app.world()) - from;
                return Err(format!("the {what} ran {ran} of {ticks} ticks in {:.0}s: the game cannot keep real time", limit.as_secs_f64()));
            }
        }
        ai_core::sim::pause(app.world_mut());
        Ok(started.elapsed())
    };
    play(app, def.warmup, "warmup")?;
    ai_core::perf::begin_span(app.world_mut());
    crate::frames::begin_span(app.world_mut());
    let wall = play(app, def.ticks, "measurement")?;
    // The last measured frames are still being drawn: wait for them on the paused game.
    crate::frames::end_span(app);
    let world = app.world();
    let ticks = ai_core::perf::span(world);
    let frames = crate::frames::span(world).map(|f| FrameSummary {
        frames: f.frames,
        avg: f.avg(),
        max: f.max,
        over_budget: f.over_budget,
        budget: world.resource::<crate::frames::FrameClock>().budget,
        fps: f.fps(),
        main: f.avg_main(),
        render: f.avg_render(),
        gpu_avg_ms: f.gpu_avg_ms(),
        triangles: f.triangles_avg(),
        draws: f.draws_avg(),
    });
    Ok((ticks, wall, frames))
}

#[cfg(not(feature = "render"))]
fn real_time(_: &mut App, _: &PerfDef, _: Duration) -> Result<(TickStats, Duration, Option<FrameSummary>), String> {
    Err("--offscreen draws on the GPU: build with the `render` feature".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ticks(ticks: u64, avg_ms: u64, over: u64) -> TickStats {
        let avg = Duration::from_millis(avg_ms);
        TickStats { ticks, total: avg * ticks as u32, max: avg * 2, worst_tick: 7, over_budget: over }
    }

    fn marks(verdicts: &[Verdict]) -> Vec<Option<bool>> {
        verdicts.iter().map(|v| v.met).collect()
    }

    #[test]
    fn every_budget_gets_a_verdict_and_ticks_always_do() {
        let budget = Duration::from_millis(16);
        let headless = Measured { ticks: ticks(600, 2, 3), tick_budget: budget, entities: 900, ..Default::default() };
        let none = judge(&Budgets::default(), &headless, true);
        assert_eq!(none.len(), 1, "ticks are held to their budget unasked: {none:?}");
        assert_eq!(none[0].line(), "ok    ticks over 16.0 ms: 3 of 600, 0.5% (at most 1.0%)");

        let budgets = Budgets {
            tick_avg_ms: Some(1.5),
            frame_avg_ms: Some(16.7),
            entities: Some(1000),
            over_share: Some(0.001),
            ..Default::default()
        };
        let verdicts = judge(&budgets, &headless, true);
        let lines: Vec<String> = verdicts.iter().map(Verdict::line).collect();
        assert_eq!(
            lines,
            [
                "OVER  tick avg 2.00 ms (at most 1.50 ms)",
                "OVER  ticks over 16.0 ms: 3 of 600, 0.5% (at most 0.1%)",
                "-     frame avg: nothing is drawn headless (`--offscreen` draws)",
                "ok    entities 900 (at most 1000)",
            ]
        );
        assert_eq!(marks(&judge(&budgets, &headless, false)), [None; 4], "a debug build judges nothing");
    }

    #[test]
    fn frames_the_gpu_and_the_region_are_judged_when_measured() {
        let budget = Duration::from_millis(16);
        let frames = FrameSummary {
            frames: 1000,
            avg: Duration::from_millis(7),
            max: Duration::from_millis(30),
            over_budget: 20,
            budget,
            gpu_avg_ms: None,
            triangles: Some(120_000.0),
            draws: Some(840.0),
            ..Default::default()
        };
        let measured = Measured {
            region_load: Some(Duration::from_millis(3200)),
            nav_bake: Some((8, Duration::from_millis(41_500))),
            ticks: ticks(600, 1, 0),
            tick_budget: budget,
            frames: Some(frames),
            visible_meshes: 300,
            mesh_assets: 12,
            ..Default::default()
        };
        let budgets = Budgets {
            region_load_s: Some(15.0),
            nav_bake_s: Some(40.0),
            frame_avg_ms: Some(16.7),
            gpu_avg_ms: Some(10.0),
            triangles: Some(100_000),
            draws: Some(800),
            visible_meshes: Some(500),
            mesh_assets: Some(10),
            ..Default::default()
        };
        let lines: Vec<String> = judge(&budgets, &measured, true).iter().map(Verdict::line).collect();
        assert_eq!(
            lines,
            [
                "ok    region load 3.20 s (at most 15.0 s)",
                "OVER  nav bake 41.50 s, 8 tiles x body classes (at most 40.0 s)",
                "ok    ticks over 16.0 ms: 0 of 600, 0.0% (at most 1.0%)",
                "ok    frame avg 7.00 ms (at most 16.70 ms)",
                "OVER  frames over 16.0 ms: 20 of 1000, 2.0% (at most 1.0%)",
                "-     GPU time: this GPU (or build) measures none",
                "OVER  triangles 120000 a frame (at most 100000)",
                "OVER  draws 840 a frame (at most 800)",
                "ok    visible meshes 300 (at most 500)",
                "OVER  distinct meshes drawn 12 (at most 10)",
            ]
        );
        let unbaked = Measured { nav_bake: None, ..measured };
        let line = judge(&budgets, &unbaked, true).into_iter().find(|v| v.text.starts_with("nav bake")).unwrap();
        assert_eq!(line.line(), "-     nav bake: no region was baked");
    }

    #[test]
    fn draws_are_judged_where_counted_and_reported_in_the_line() {
        let budget = Duration::from_millis(16);
        let measured = |draws: Option<f64>| Measured {
            ticks: ticks(600, 1, 0),
            tick_budget: budget,
            frames: Some(FrameSummary { frames: 100, budget, draws, ..Default::default() }),
            ..Default::default()
        };
        let budgets = Budgets { draws: Some(900), ..Default::default() };
        let line = |draws| judge(&budgets, &measured(draws), true).iter().find(|v| v.text.starts_with("draws")).map(Verdict::line);
        assert_eq!(line(Some(812.4)).as_deref(), Some("ok    draws 812.4 a frame (at most 900)"));
        assert_eq!(line(Some(900.4)).as_deref(), Some("OVER  draws 900.4 a frame (at most 900)"), "the line shows the value judged");
        assert_eq!(line(Some(901.0)).as_deref(), Some("OVER  draws 901 a frame (at most 900)"));
        assert_eq!(line(None).as_deref(), Some("-     draws: this GPU keeps its batches private, none are counted"));
        let headless = Measured { ticks: ticks(600, 1, 0), tick_budget: budget, ..Default::default() };
        let verdicts = judge(&budgets, &headless, true);
        assert!(verdicts.iter().any(|v| v.line() == "-     draws: nothing is drawn headless (`--offscreen` draws)"), "{verdicts:?}");
        assert!(measured(Some(812.4)).line().contains(", 812 draws;"), "{}", measured(Some(812.4)).line());
        assert!(!measured(None).line().contains("draws;"), "no number is made up");
    }

    #[test]
    fn a_budget_nobody_set_leaves_a_definitions_json_as_it_was() {
        // The content hash is over this JSON, and replays compare it: a field added to the budgets
        // must not move the hash of a definition that does not use it.
        let json = |budgets: Budgets| serde_json::to_value(PerfDef { budgets, ..serde_json::from_str("{}").unwrap() }).unwrap();
        for field in ["draws", "nav_bake_s"] {
            assert!(!json(Budgets::default())["budgets"].as_object().unwrap().contains_key(field), "{field}");
        }
        assert_eq!(json(Budgets { draws: Some(150), ..Default::default() })["budgets"]["draws"], 150);
        assert_eq!(json(Budgets { nav_bake_s: Some(60.0), ..Default::default() })["budgets"]["nav_bake_s"], 60.0);
    }

    fn errors(perf: &str) -> Vec<String> {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<PerfDef>();
        kinds.register::<shards_world::content::RegionDef>();
        kinds.register::<shards_world::content::DungeonDef>();
        let sources = [shards_content::db::Source { path: "content/b.perf.ron".into(), text: perf.into() }];
        shards_content::ContentDb::build(&kinds, &sources).errors().map(ToString::to_string).collect()
    }

    #[test]
    fn a_definition_names_what_it_gets_wrong() {
        assert_eq!(errors(r#"{ "bench": () }"#), Vec::<String>::new(), "every field has a default");
        let errors = errors(
            r#"{ "bench": (region: Some((region: "wild", seed: 1)), setup: ["god", ""], ticks: 0,
                budgets: (tick_avg_ms: Some(0.0), over_share: Some(2.0))) }"#,
        );
        let expected = [
            "region: no `region` or `dungeon` is named `wild`",
            "setup[1]: an empty line",
            "ticks: measure at least one tick",
            "budgets.tick_avg_ms: a budget is a number above 0",
            "budgets.over_share: a share from 0",
        ];
        assert_eq!(errors.len(), expected.len(), "{errors:#?}");
        for (error, expected) in errors.iter().zip(expected) {
            assert!(error.contains(expected), "{error} should say {expected}");
        }
        let no_region = self::errors(r#"{ "bench": (budgets: (region_load_s: Some(10.0), nav_bake_s: Some(-1.0))) }"#);
        assert_eq!(no_region.len(), 3, "{no_region:#?}");
        assert!(no_region[0].contains("budgets.nav_bake_s: a budget is a number above 0"), "{no_region:#?}");
        assert!(no_region[1].contains("budgets.region_load_s: no region is loaded"), "{no_region:#?}");
        assert!(no_region[2].contains("budgets.nav_bake_s: no region is loaded"), "{no_region:#?}");
        let huge = self::errors(
            r#"{ "bench": (warmup: 300000, ticks: 1000000000, budgets: (frame_ms: Some(1e30), tick_ms: Some(1e-9))) }"#,
        );
        let expected = [
            "warmup: at most 216000 ticks",
            "ticks: at most 216000 ticks",
            "budgets.tick_ms: the budget is below 1 ns",
            "budgets.frame_ms: the budget is too large",
        ];
        assert_eq!(huge.len(), expected.len(), "{huge:#?}");
        for (error, expected) in huge.iter().zip(expected) {
            assert!(error.contains(expected), "{error} should say {expected}");
        }
        assert_eq!(wall_limit(u64::MAX, Duration::from_secs(1)), Duration::MAX, "no limit overflows");
    }
}
