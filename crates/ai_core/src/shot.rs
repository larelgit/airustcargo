//! `shot`: a screenshot of the game window with every `eN` mark drawn on it.
//!
//! Pixels are a side channel for visual bugs (a mesh in the wrong place, a telegraph
//! that does not match its hitbox, a label drawn over the wrong thing); numbers still
//! come from `observe`. The id overlay is forced on before the captured frame, so
//! the marks are in the picture, and a legend maps every mark to image pixels.
//!
//! The capture is Bevy's own read-back of the rendered frame, not a grab of the
//! desktop: a window without focus that the compositor redraws late still gives the
//! current frame. It waits until the GPU has no render pipelines left to compile
//! (right after startup, meshes and marks are skipped until theirs are ready) and
//! is asynchronous (the frame comes back a frame or two later), so the tool returns
//! the paths at once. The legend is computed in the captured frame itself (a
//! following camera may still be moving after a `step`) and written next to the
//! PNG as JSON, before the PNG appears: both are written to temp files and renamed.
//! A journal line says when the shot is saved; `shards shot` waits for it.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use bevy::camera::CameraUpdateSystems;
use bevy::prelude::*;
use bevy::render::render_resource::PipelineCache;
use bevy::render::view::screenshot::{save_to_disk, Screenshot, ScreenshotCaptured};
use bevy::render::{Render, RenderApp, RenderSystems};
use bevy::transform::TransformSystems;
use bevy::window::PrimaryWindow;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::highlight::AiOverlay;
use crate::journal::{self, JournalKind};
use crate::observe::{on_screen, AiCamera, Sighting};
use crate::session::Scope;
use crate::sim;
use crate::tools::{register_tool, Call, ToolKind, ToolResult};

/// Default folder for screenshots, relative to the game's working directory.
pub const SHOTS_DIR: &str = "shots";

/// Frames between switching the overlay on and capturing: gizmo lines reach the GPU
/// as an asset, one frame after they are drawn.
const SETTLE_FRAMES: u32 = 1;

/// Longest wait for pipelines to finish compiling; then the shot is taken anyway.
const MAX_COMPILE_WAIT_FRAMES: u32 = 600;

/// Frames to wait for the GPU before giving up on a capture (a minimized window
/// may never present a frame).
const TIMEOUT_FRAMES: u32 = 600;

/// Save a screenshot of the game window as PNG, with the `eN` id of every
/// AI-visible entity drawn over it, plus a JSON legend: where each entity is in the
/// image. For visual bugs; read numbers from `observe`. Both files appear a frame or
/// two after the call (`shards shot` waits and prints the legend).
#[derive(Deserialize, JsonSchema, Default)]
pub struct ShotArgs {
    /// Draw the id marks. Default true.
    pub marks: Option<bool>,
    /// Where to save it (must end in .png). Default: shots/tick<T>-<N>.png in the
    /// game's working directory. Existing files there are replaced.
    pub path: Option<String>,
}

/// What `shot` returns right away.
#[derive(Serialize, Debug, Clone)]
pub struct ShotTicket {
    /// Absolute path; the PNG appears there once the frame is captured.
    pub path: String,
    /// The [`ShotInfo`] JSON, written just before the PNG appears.
    pub legend_path: String,
    /// Tick of the request (the legend has the tick of the captured frame).
    pub tick: u64,
    pub marks: bool,
}

/// Written next to the PNG (`x.png` → `x.json`): the captured frame described.
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ShotInfo {
    pub path: String,
    pub width: u32,
    pub height: u32,
    /// Tick of the captured frame.
    pub tick: u64,
    pub marks: bool,
    /// AI-visible entities in the picture: `e6 Boss "Frost Warden"  px(640,300)`,
    /// in image pixels from the top-left corner (the entity at chest height).
    pub legend: Vec<String>,
}

struct Pending {
    path: PathBuf,
    marks: bool,
    /// Frames to wait before capturing, so the overlay switched on for this shot has
    /// reached the GPU.
    frames_left: u32,
    /// Frames spent waiting for pipelines to compile.
    waited: u32,
}

struct InFlight {
    screenshot: Entity,
    path: PathBuf,
    marks: bool,
    frames: u32,
}

#[derive(Resource, Default)]
pub struct AiShots {
    pending: Vec<Pending>,
    in_flight: Vec<InFlight>,
    taken: u32,
}

fn shot_tool(In(call): In<Call<ShotArgs>>, world: &mut World) -> ToolResult<ShotTicket> {
    let has_window = world
        .try_query_filtered::<(), With<PrimaryWindow>>()
        .is_some_and(|mut q| q.iter(world).next().is_some());
    if !has_window {
        return Err("no window to capture: this game runs headless. Use `observe` for the world as text, \
                    or run it with a window (`cargo sandbox`)"
            .into());
    }
    let marks = call.args.marks.unwrap_or(true);
    let tick = sim::tick(world);
    let n = {
        let mut shots = world.resource_mut::<AiShots>();
        shots.taken += 1;
        shots.taken
    };
    let path = match call.args.path {
        Some(p) => PathBuf::from(p),
        None => Path::new(SHOTS_DIR).join(format!("tick{tick}-{n}.png")),
    };
    let path = prepare_path(&path)?;

    let frames_left = if marks { SETTLE_FRAMES } else { 0 };
    world.resource_mut::<AiShots>().pending.push(Pending { path: path.clone(), marks, frames_left, waited: 0 });
    if marks {
        world.resource_mut::<AiOverlay>().shots += 1;
    }
    Ok(ShotTicket {
        path: path.display().to_string(),
        legend_path: legend_path(&path).display().to_string(),
        tick,
        marks,
    })
}

/// Absolute `.png` path with an existing parent folder and no stale PNG or legend
/// in the way (whoever waits for the file must not pick up an old one).
fn prepare_path(path: &Path) -> Result<PathBuf, String> {
    let is_png = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"));
    if !is_png {
        return Err(format!("path must end in .png, got `{}`", path.display()));
    }
    let path = if path.is_absolute() {
        path.to_owned()
    } else {
        std::env::current_dir().map_err(|e| format!("no working directory: {e}"))?.join(path)
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    }
    for stale in [path.clone(), legend_path(&path)] {
        match std::fs::remove_file(&stale) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(format!("cannot replace {}: {e}", stale.display())),
        }
    }
    Ok(path)
}

/// `x.png` → `x.json`.
pub fn legend_path(png: &Path) -> PathBuf {
    png.with_extension("json")
}

/// Written first, renamed when complete: `x.png` → `x.part.png`, `x.json` → `x.part.json`.
fn part_path(path: &Path) -> PathBuf {
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    path.with_file_name(format!("{stem}.part.{ext}"))
}

/// Where every AI-visible entity is in the image, in physical pixels.
fn legend(world: &World) -> Vec<String> {
    let scale = world
        .try_query_filtered::<&Camera, With<AiCamera>>()
        .and_then(|mut q| q.iter(world).find(|c| c.is_active).and_then(Camera::target_scaling_factor))
        .unwrap_or(1.0);
    on_screen(world)
        .into_iter()
        .filter_map(|(f, sighting)| {
            let (scr, note) = match sighting {
                Sighting::Visible { scr: Some(p) } => (p, ""),
                Sighting::Occluded { scr: Some(p) } => (p, "  occluded"),
                _ => return None,
            };
            let px = scr * scale;
            Some(format!("{} {}  px({:.0},{:.0}){note}", f.id, f.label, px.x, px.y))
        })
        .collect()
}

/// Render pipelines still compiling, as the render world saw it after its last
/// frame. Starts at 1 ("not known yet") when there is a renderer.
#[derive(Resource, Clone, Default)]
struct PipelinesCompiling(Arc<AtomicUsize>);

fn count_compiling(cache: Res<PipelineCache>, compiling: Res<PipelinesCompiling>) {
    compiling.0.store(cache.waiting_pipelines().count(), Ordering::Relaxed);
}

/// Turns ready requests into Bevy `Screenshot`s and gives up on captures that never
/// come back. Runs after the overlay is drawn and transforms and cameras are
/// updated, so the legend describes exactly the frame that is captured.
fn run_shots(world: &mut World) {
    let compiling = world.resource::<PipelinesCompiling>().0.load(Ordering::Relaxed);
    let mut ready = Vec::new();
    {
        let mut shots = world.resource_mut::<AiShots>();
        let mut still_pending = Vec::new();
        for mut p in shots.pending.drain(..) {
            if p.frames_left > 0 {
                p.frames_left -= 1;
                still_pending.push(p);
            } else if compiling > 0 && p.waited < MAX_COMPILE_WAIT_FRAMES {
                p.waited += 1;
                still_pending.push(p);
            } else {
                ready.push(p);
            }
        }
        shots.pending = still_pending;
    }

    for p in ready {
        let info = ShotInfo {
            path: p.path.display().to_string(),
            width: 0,
            height: 0,
            tick: sim::tick(world),
            marks: p.marks,
            legend: legend(world),
        };
        let screenshot = world.spawn(Screenshot::primary_window()).observe(save_when_captured(p.path.clone(), info)).id();
        world.resource_mut::<AiShots>().in_flight.push(InFlight { screenshot, path: p.path, marks: p.marks, frames: 0 });
    }

    let mut timed_out = Vec::new();
    for f in &mut world.resource_mut::<AiShots>().in_flight {
        f.frames += 1;
        if f.frames == TIMEOUT_FRAMES {
            timed_out.push((f.screenshot, f.path.clone()));
        }
    }
    for (screenshot, path) in timed_out {
        let _ = world.try_despawn(screenshot);
        finish(world, screenshot, Err(format!("screenshot {} timed out (window minimized?)", path.display())));
    }
}

fn save_when_captured(path: PathBuf, info: ShotInfo) -> impl FnMut(On<ScreenshotCaptured>, Commands) {
    let part = part_path(&path);
    let mut save = save_to_disk(part.clone());
    move |captured: On<ScreenshotCaptured>, mut commands: Commands| {
        let screenshot = captured.entity;
        let info = ShotInfo { width: captured.image.width(), height: captured.image.height(), ..info.clone() };
        // Encoding errors are logged by `save_to_disk` (and land in the journal);
        // the rename below then fails and reports the shot as lost.
        save(captured);
        let saved = write_legend(&path, &info)
            .and_then(|()| std::fs::rename(&part, &path).map_err(|e| e.to_string()))
            .map(|()| format!("screenshot saved: {} ({}x{})", path.display(), info.width, info.height))
            .map_err(|e| format!("screenshot {} not saved: {e}", path.display()));
        commands.queue(move |world: &mut World| finish(world, screenshot, saved));
    }
}

fn write_legend(png: &Path, info: &ShotInfo) -> Result<(), String> {
    let (json, part) = (legend_path(png), part_path(&legend_path(png)));
    let text = serde_json::to_string_pretty(info).map_err(|e| e.to_string())?;
    std::fs::write(&part, text).and_then(|()| std::fs::rename(&part, &json)).map_err(|e| e.to_string())
}

/// Closes a capture: releases the overlay it held and journals the outcome.
fn finish(world: &mut World, screenshot: Entity, outcome: Result<String, String>) {
    let mut shots = world.resource_mut::<AiShots>();
    let Some(i) = shots.in_flight.iter().position(|f| f.screenshot == screenshot) else {
        return;
    };
    let shot = shots.in_flight.swap_remove(i);
    if shot.marks {
        let mut overlay = world.resource_mut::<AiOverlay>();
        overlay.shots = overlay.shots.saturating_sub(1);
    }
    match outcome {
        Ok(text) => journal::log(world, JournalKind::Sim, text),
        Err(text) => journal::log(world, JournalKind::Error, text),
    };
}

pub(crate) fn plugin(app: &mut App) {
    let compiling = PipelinesCompiling::default();
    // Needs the renderer to exist already: add AiCorePlugin after DefaultPlugins.
    if let Some(render_app) = app.get_sub_app_mut(RenderApp) {
        compiling.0.store(1, Ordering::Relaxed);
        render_app
            .insert_resource(compiling.clone())
            .add_systems(Render, count_compiling.in_set(RenderSystems::Cleanup));
    }
    app.init_resource::<AiShots>().insert_resource(compiling).add_systems(
        PostUpdate,
        run_shots
            .after(crate::highlight::draw_highlights)
            .after(TransformSystems::Propagate)
            .after(CameraUpdateSystems),
    );
    register_tool(app.world_mut(), "shot", Scope::Dev, ToolKind::Query, shot_tool);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_png_absolute_and_written_atomically() {
        assert!(prepare_path(Path::new("shot.jpg")).unwrap_err().contains("must end in .png"));
        let dir = std::env::temp_dir().join(format!("ai_core_shot_test_{}", std::process::id()));
        let target = dir.join("nested/a.png");
        std::fs::create_dir_all(target.parent().unwrap()).unwrap();
        std::fs::write(&target, b"stale").unwrap();
        std::fs::write(dir.join("nested/a.json"), b"stale").unwrap();
        let prepared = prepare_path(&target).unwrap();
        assert!(prepared.is_absolute());
        assert!(!prepared.exists(), "a stale file must not look like a finished shot");
        assert!(!legend_path(&prepared).exists(), "nor a stale legend");
        assert_eq!(part_path(&prepared), dir.join("nested/a.part.png"));
        assert_eq!(part_path(&legend_path(&prepared)), dir.join("nested/a.part.json"));
        let _ = std::fs::remove_dir_all(dir);
    }
}
