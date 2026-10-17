//! Perception: the world as compact text, relative to a viewer.
//!
//! ```text
//! tick 1532 (25.53s) | paused | speed 1x | seed 42 | you e12
//! YOU  e12 Player  hp 64/100  stam 30/80  Dodge 0.20s  at (3.0, -4.2) facing N
//! ALERT in hit zone of e42 slam (lands in 0.60s): move 2.1m S to leave it
//! SEEN (by threat)
//!   e42 Boss "Frost Warden"  hp 2300/4000  phase 2  WindupSlam 0.60s  8.4m NE  scr(812,344)
//!   e57 Skeleton  hp 40/40  Chase  15.0m W  occluded
//! NOT IN VIEW (within 40m)
//!   e58 Skeleton  hp 40/40  6.2m S
//! SINCE seq 118
//!   1490 e42 -> e12 FrostSlash -18
//!   1512 e42 phase 1->2
//! ```
//!
//! What the subject sees depends on the subject alone ([`classify`]): a ray from its
//! eyes (`pos + EYE_HEIGHT`) to the target decides SEEN or `occluded`, within
//! [`AiPerception::radius`]; with [`AiPerception::fov_deg`] set, whatever lies outside
//! the cone around its facing is NOT IN VIEW (by default it sees all around and that
//! section never appears). The window's camera plays no part: the same world reads the
//! same with a window or without, however far a following camera lags behind, and a
//! possessed skeleton sees with its own eyes. A window only adds `scr(x,y)`, where an
//! entity is in the [`AiCamera`] frame.
//!
//! Numbers and positions always come from here, never from pixels.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use bevy::camera::Camera;
use bevy::prelude::*;

use crate::describe::{all_facts, facts, EntityFacts};
use crate::id::{AiId, AiLandmark, AiPlayer};
use crate::journal::AiJournal;
use crate::rng::SimSeed;
use crate::sim;
use crate::spatial::{self, compass, fmt_pos, relative, EYE_HEIGHT};

/// Marks the camera that `scr(x,y)` and screenshot legends refer to. What the AI sees
/// never depends on it.
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct AiCamera;

/// Perception limits.
#[derive(Resource, Clone, Debug)]
pub struct AiPerception {
    /// Entities farther than this from the viewer are left out (use `nearby`).
    pub radius: f32,
    /// Maximum entity lines per observation.
    pub max_listed: usize,
    /// Field of view in degrees, centered on the subject's facing: entities outside it
    /// are NOT IN VIEW. `None`: the subject sees all around.
    pub fov_deg: Option<f32>,
}

impl Default for AiPerception {
    fn default() -> Self {
        Self { radius: 40.0, max_listed: 24, fov_deg: None }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ObserveOptions {
    /// "YOU": the viewer. Defaults to the [`AiPlayer`] entity.
    pub subject: Option<Entity>,
    /// Journal cursor: include entries with `seq > after` in a SINCE section.
    pub journal_after: Option<u64>,
}

/// Where an entity stands relative to the subject.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sighting {
    /// Clear line of sight from the subject's eyes.
    Visible,
    /// Something blocks the line of sight.
    Occluded,
    /// Outside the subject's field of view ([`AiPerception::fov_deg`]).
    NotInView,
}

/// Where the subject looks from.
#[derive(Clone, Copy, Debug)]
pub struct Viewpoint {
    /// The subject itself: its own occluder never blocks its view.
    pub entity: Option<Entity>,
    /// Eyes: `pos + EYE_HEIGHT`.
    pub eye: Vec3,
    pub facing: Option<Vec3>,
    /// See [`AiPerception::fov_deg`].
    pub fov_deg: Option<f32>,
}

impl Viewpoint {
    /// The eyes of `subject` (the world origin without one).
    pub fn of(subject: Option<&EntityFacts>, perception: &AiPerception) -> Self {
        Self {
            entity: subject.map(|s| s.entity),
            eye: subject.map_or(Vec3::ZERO, |s| s.pos) + Vec3::Y * EYE_HEIGHT,
            facing: subject.and_then(|s| s.facing),
            fov_deg: perception.fov_deg,
        }
    }
}

/// SEEN, `occluded` or NOT IN VIEW, from the subject alone: its field of view and a
/// ray from its eyes to the target's (via `line_of_sight`, which also gets the
/// entities whose occluders must not count). Takes no camera, on purpose; the caller
/// applies the perception radius.
pub fn classify(from: &Viewpoint, target: &EntityFacts, line_of_sight: impl Fn(Vec3, Vec3, &[Entity]) -> bool) -> Sighting {
    let aim = target.pos + Vec3::Y * EYE_HEIGHT;
    if let (Some(fov), Some(facing)) = (from.fov_deg, from.facing) {
        let (ahead, to) = (Vec3::new(facing.x, 0.0, facing.z), Vec3::new(aim.x - from.eye.x, 0.0, aim.z - from.eye.z));
        if ahead.length() > 1e-3 && to.length() > 1e-3 && ahead.angle_between(to).to_degrees() > fov / 2.0 {
            return Sighting::NotInView;
        }
    }
    let ignore: Vec<Entity> = from.entity.into_iter().chain([target.entity]).collect();
    if line_of_sight(from.eye, aim, &ignore) { Sighting::Visible } else { Sighting::Occluded }
}

pub fn player_entity(world: &World) -> Option<Entity> {
    let mut q = world.try_query_filtered::<Entity, With<AiPlayer>>()?;
    q.iter(world).next()
}

/// The active [`AiCamera`] and its logical viewport size, when a window renders one.
fn screen_camera(world: &World) -> Option<(Camera, GlobalTransform, Vec2)> {
    world.try_query_filtered::<(&Camera, &GlobalTransform), With<AiCamera>>().and_then(|mut q| {
        q.iter(world)
            .find(|(c, _)| c.is_active)
            .and_then(|(c, gt)| c.logical_viewport_size().map(|size| (c.clone(), *gt, size)))
    })
}

/// Where `point` is in the camera frame (logical pixels from the top left), if in it.
fn project(camera: &(Camera, GlobalTransform, Vec2), point: Vec3) -> Option<Vec2> {
    let (camera, gt, size) = camera;
    camera.world_to_viewport(gt, point).ok().filter(|p| p.x >= 0.0 && p.y >= 0.0 && p.x <= size.x && p.y <= size.y)
}

/// An AI-visible entity in the [`AiCamera`] frame.
#[derive(Clone, Debug)]
pub struct OnScreen {
    pub facts: EntityFacts,
    /// Logical viewport position of its chest.
    pub scr: Vec2,
    /// Hidden from the camera by an occluder.
    pub occluded: bool,
}

/// AI-visible entities inside the [`AiCamera`] frame (landmarks and the player
/// included), with their logical viewport positions: the legend of a screenshot, so
/// it describes the picture, as the camera sees it. Empty without an active camera.
pub fn on_screen(world: &World) -> Vec<OnScreen> {
    let Some(camera) = screen_camera(world) else {
        return Vec::new();
    };
    let lens = camera.1.translation();
    all_facts(world)
        .into_iter()
        .filter_map(|facts| {
            let aim = facts.pos + Vec3::Y * EYE_HEIGHT;
            let scr = project(&camera, aim)?;
            let occluded = !spatial::line_of_sight(world, lens, aim, &[facts.entity]);
            Some(OnScreen { facts, scr, occluded })
        })
        .collect()
}

/// An entity in a SEEN or NOT IN VIEW list.
struct Listed {
    f: EntityFacts,
    dist: f32,
    sighting: Sighting,
}

/// One entity line as it appears in SEEN / NOT IN VIEW / nearby lists.
pub fn entity_line(f: &EntityFacts, from: Option<Vec3>, sighting: Option<Sighting>) -> String {
    let mut line = format!("{} {}", f.id, f.label);
    let frags = f.fragments_text();
    if !frags.is_empty() {
        line.push_str("  ");
        line.push_str(&frags);
    }
    match from {
        Some(origin) => {
            line.push_str("  ");
            line.push_str(&relative(origin, f.pos));
        }
        None => {
            line.push_str("  at ");
            line.push_str(&fmt_pos(f.pos));
        }
    }
    if sighting == Some(Sighting::Occluded) {
        line.push_str("  occluded");
    }
    line
}

fn you_line(s: &EntityFacts) -> String {
    let mut line = format!("{} {}", s.id, s.label);
    let frags = s.fragments_text();
    if !frags.is_empty() {
        line.push_str("  ");
        line.push_str(&frags);
    }
    let _ = write!(line, "  at {}", fmt_pos(s.pos));
    if let Some(f) = s.facing.filter(|f| Vec3::new(f.x, 0.0, f.z).length() > 0.1) {
        let _ = write!(line, " facing {}", compass(f));
    }
    line
}

pub fn header(world: &World, subject: Option<AiId>) -> String {
    let tick = sim::tick(world);
    let secs = tick as f32 * sim::tick_seconds(world);
    let state = if sim::is_paused(world) { "paused" } else { "running" };
    let mut h = format!("tick {tick} ({secs:.2}s) | {state} | speed {}x", sim::speed(world));
    if let Some(seed) = world.get_resource::<SimSeed>() {
        let _ = write!(h, " | seed {}", seed.0);
    }
    if let Some(id) = subject {
        let _ = write!(h, " | you {id}");
    }
    if let Some(gate) = world.get_resource::<sim::SimGate>().filter(|g| g.is_held()) {
        let _ = write!(h, " | loading: {}", gate.reasons().join(", "));
    }
    h
}

/// The observation text. Deterministic for a given world state.
pub fn observe(world: &World, opts: &ObserveOptions) -> String {
    let perception = world.get_resource::<AiPerception>().cloned().unwrap_or_default();
    let subject = opts.subject.or_else(|| player_entity(world)).and_then(|e| facts(world, e));
    let subject_id = subject.as_ref().map(|s| s.id);
    let origin = subject.as_ref().map_or(Vec3::ZERO, |s| s.pos);
    let viewpoint = Viewpoint::of(subject.as_ref(), &perception);
    let camera = screen_camera(world);

    let mut out = header(world, subject_id);
    out.push('\n');
    match &subject {
        Some(s) => {
            let _ = writeln!(out, "YOU  {}", you_line(s));
            for line in crate::describe::advice(world, s.entity) {
                let _ = writeln!(out, "ALERT {line}");
            }
            for line in crate::describe::observe_lines(world, s.entity) {
                let _ = writeln!(out, "{line}");
            }
        }
        None => out.push_str("YOU  (no subject: looking from the world origin)\n"),
    }

    let mut seen = Vec::new();
    let mut unseen = Vec::new();
    let mut landmarks = Vec::new();
    for f in all_facts(world) {
        if Some(f.id) == subject_id {
            continue;
        }
        let dist = f.pos.distance(origin);
        if dist > perception.radius {
            continue;
        }
        if world.get::<AiLandmark>(f.entity).is_some() {
            landmarks.push((f, dist));
            continue;
        }
        let sighting = classify(&viewpoint, &f, |a, b, ignore| spatial::line_of_sight(world, a, b, ignore));
        let item = Listed { dist, sighting, f };
        match sighting {
            Sighting::NotInView => unseen.push(item),
            _ => seen.push(item),
        }
    }
    let by_threat = |a: &Listed, b: &Listed| {
        b.f.importance.total_cmp(&a.f.importance).then(a.dist.total_cmp(&b.dist)).then(a.f.id.cmp(&b.f.id))
    };
    seen.sort_by(by_threat);
    unseen.sort_by(by_threat);

    let mut budget = perception.max_listed;
    let mut section = |out: &mut String, title: &str, items: &[Listed]| {
        if items.is_empty() {
            return;
        }
        let _ = writeln!(out, "{title}");
        let shown = items.len().min(budget);
        for item in &items[..shown] {
            let mut line = entity_line(&item.f, Some(origin), Some(item.sighting));
            // Where it is in the window, if there is one: an annotation, not a verdict.
            if let Some(p) = camera.as_ref().and_then(|c| project(c, item.f.pos + Vec3::Y * EYE_HEIGHT)) {
                let _ = write!(line, "  scr({:.0},{:.0})", p.x, p.y);
            }
            let _ = writeln!(out, "  {line}");
        }
        if shown < items.len() {
            let _ = writeln!(out, "  … {} more (use `nearby`)", items.len() - shown);
        }
        budget -= shown;
    };
    section(&mut out, "SEEN (by threat)", &seen);
    section(&mut out, &format!("NOT IN VIEW (within {:.0}m)", perception.radius), &unseen);
    if seen.is_empty() && unseen.is_empty() {
        let _ = writeln!(out, "SEEN  nothing within {:.0}m", perception.radius);
    }
    if !landmarks.is_empty() {
        landmarks.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.id.cmp(&b.0.id)));
        let items: Vec<String> =
            landmarks.iter().map(|(f, _)| format!("{} {} {}", f.id, f.label, relative(origin, f.pos))).collect();
        let _ = writeln!(out, "LANDMARKS  {}", items.join(" · "));
    }
    for line in crate::describe::observe_sections(world) {
        let _ = writeln!(out, "{line}");
    }

    if let (Some(after), Some(journal)) = (opts.journal_after, world.get_resource::<AiJournal>()) {
        let (entries, missed) = journal.after(after);
        let entries: Vec<_> = entries.collect();
        if !entries.is_empty() || missed > 0 {
            let _ = writeln!(out, "SINCE seq {after}");
            if missed > 0 {
                let _ = writeln!(out, "  ({missed} older entries dropped)");
            }
            for e in entries {
                let _ = writeln!(out, "  {e}");
            }
        }
    }
    out
}

/// Entities within `radius` of `center`, nearest first, optionally filtered by a
/// case-insensitive substring of their line ("skeleton", "boss", "hp").
pub fn nearby(world: &World, center: Vec3, radius: f32, filter: Option<&str>, exclude: Option<AiId>) -> Vec<String> {
    let filter = filter.map(str::to_lowercase);
    let mut found: Vec<(EntityFacts, f32)> = all_facts(world)
        .into_iter()
        .filter(|f| Some(f.id) != exclude)
        .map(|f| {
            let d = f.pos.distance(center);
            (f, d)
        })
        .filter(|(_, d)| *d <= radius)
        .filter(|(f, _)| {
            filter.as_ref().is_none_or(|needle| entity_line(f, None, None).to_lowercase().contains(needle))
        })
        .collect();
    found.sort_by(|a, b| a.1.total_cmp(&b.1).then(a.0.id.cmp(&b.0.id)));
    found.iter().map(|(f, _)| entity_line(f, Some(center), None)).collect()
}

/// Full detail on one entity: fragments plus every reflectable gameplay component
/// ([`is_gameplay_type`](crate::describe::is_gameplay_type); engine ones are only named).
pub fn inspect(world: &World, entity: Entity, viewer_subject: Option<Entity>) -> Result<String, String> {
    let f = facts(world, entity).ok_or_else(|| format!("{entity} is not visible to the AI"))?;
    let mut out = format!("{} {}  [{}]\n", f.id, f.label, entity);
    let _ = write!(out, "at {}", fmt_pos(f.pos));
    if let Some(viewer) = viewer_subject.or_else(|| player_entity(world)).and_then(|e| facts(world, e))
        && viewer.id != f.id
    {
        let _ = write!(out, "  {} of you ({})", relative(viewer.pos, f.pos), viewer.id);
    }
    if let Some(facing) = f.facing.filter(|v| Vec3::new(v.x, 0.0, v.z).length() > 0.1) {
        let _ = write!(out, "  facing {}", compass(facing));
    }
    out.push('\n');
    for (key, frag) in &f.fragments {
        let _ = writeln!(out, "  {key}: {frag}");
    }

    let registry = world.resource::<AppTypeRegistry>().read();
    let e = world.entity(entity);
    let mut hidden = Vec::new();
    let _ = writeln!(out, "components:");
    let mut lines = Vec::new();
    for (_, info) in world.inspect_entity(entity).map_err(|e| e.to_string())? {
        let name = info.name().shortname().to_string();
        let reflected = info
            .type_id()
            .and_then(|t| registry.get(t))
            .filter(|reg| crate::describe::is_gameplay_type(reg.type_info().type_path()))
            .and_then(|reg| reg.data::<ReflectComponent>())
            .and_then(|rc| rc.reflect(e));
        match reflected {
            Some(value) => lines.push(format!("  {:?}", value.as_partial_reflect())),
            None => hidden.push(name),
        }
    }
    lines.sort();
    for l in lines {
        out.push_str(&l);
        out.push('\n');
    }
    hidden.sort();
    if !hidden.is_empty() {
        let _ = writeln!(out, "  (+{} engine components: {})", hidden.len(), hidden.join(", "));
    }
    Ok(out)
}

// --- diffs ---------------------------------------------------------------------

/// Viewer-independent state of all AI-visible entities, for before/after diffs.
#[derive(Clone, Debug, Default)]
pub struct Digest(pub BTreeMap<AiId, EntityFacts>);

pub fn digest(world: &World) -> Digest {
    Digest(all_facts(world).into_iter().map(|f| (f.id, f)).collect())
}

/// Human/LLM-readable changes between two digests.
pub fn diff(before: &Digest, after: &Digest, origin: Option<Vec3>) -> Vec<String> {
    let mut out = Vec::new();
    for (id, a) in &after.0 {
        let Some(b) = before.0.get(id) else {
            out.push(format!("+ {}", entity_line(a, origin, None)));
            continue;
        };
        let mut changes = Vec::new();
        for (key, new) in &a.fragments {
            match b.fragment(key) {
                Some(old) if old == new => {}
                Some(old) => changes.push(format!("{old} → {new}")),
                None => changes.push(format!("+{new}")),
            }
        }
        for (key, old) in &b.fragments {
            if a.fragment(key).is_none() {
                changes.push(format!("-{old}"));
            }
        }
        let moved = a.pos - b.pos;
        if moved.length() >= 0.3 {
            changes.push(format!("moved {}", relative(Vec3::ZERO, moved)));
        }
        if !changes.is_empty() {
            out.push(format!("~ {} {}: {}", id, a.label, changes.join("; ")));
        }
    }
    for (id, b) in &before.0 {
        if !after.0.contains_key(id) {
            out.push(format!("- {} {}", id, b.label));
        }
    }
    out
}
