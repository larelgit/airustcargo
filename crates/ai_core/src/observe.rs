//! Perception: the world as compact text, relative to a viewer.
//!
//! ```text
//! tick 1532 (25.53s) | paused | speed 1x | seed 42 | you e12
//! YOU  e12 Player  hp 64/100  stam 30/80  Dodge 0.20s  at (3.0, -4.2) facing N
//! SEEN (by threat)
//!   e42 Boss "Frost Warden"  hp 2300/4000  phase 2  WindupSlam 0.60s  8.4m NE  scr(812,344)
//!   e57 Skeleton  hp 40/40  Chase  15.0m W  occluded
//! SINCE seq 118
//!   1490 e42 -> e12 FrostSlash -18
//!   1512 e42 phase 1->2
//! ```
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

/// Marks the camera whose view defines "on screen" and `scr(x,y)` coordinates.
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
}

impl Default for AiPerception {
    fn default() -> Self {
        Self { radius: 40.0, max_listed: 24 }
    }
}

#[derive(Clone, Debug, Default)]
pub struct ObserveOptions {
    /// "YOU": the viewer. Defaults to the [`AiPlayer`] entity.
    pub subject: Option<Entity>,
    /// Journal cursor: include entries with `seq > after` in a SINCE section.
    pub journal_after: Option<u64>,
}

/// Where an entity stands relative to the viewer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Sighting {
    /// In view with clear line of sight. `scr` is set when a camera projects it.
    Visible { scr: Option<Vec2> },
    /// In view but something blocks the line of sight.
    Occluded { scr: Option<Vec2> },
    /// Within perception radius but off-screen.
    NotInView,
}

struct Viewer {
    subject: Option<EntityFacts>,
    /// Origin for distances and directions.
    origin: Vec3,
    /// Origin for line-of-sight rays.
    eye: Vec3,
    camera: Option<(Camera, GlobalTransform, Vec2)>,
}

pub fn player_entity(world: &World) -> Option<Entity> {
    let mut q = world.try_query_filtered::<Entity, With<AiPlayer>>()?;
    q.iter(world).next()
}

fn viewer(world: &World, subject: Option<Entity>) -> Viewer {
    let subject = subject.or_else(|| player_entity(world)).and_then(|e| facts(world, e));
    let camera = world.try_query_filtered::<(&Camera, &GlobalTransform), With<AiCamera>>().and_then(|mut q| {
        q.iter(world)
            .find(|(c, _)| c.is_active)
            .and_then(|(c, gt)| c.logical_viewport_size().map(|size| (c.clone(), *gt, size)))
    });
    let origin = match (&subject, &camera) {
        (Some(s), _) => s.pos,
        (None, Some((_, gt, _))) => gt.translation(),
        (None, None) => Vec3::ZERO,
    };
    let eye = match &camera {
        Some((_, gt, _)) => gt.translation(),
        None => origin + Vec3::Y * EYE_HEIGHT,
    };
    Viewer { subject, origin, eye, camera }
}

fn sight(world: &World, viewer: &Viewer, target: &EntityFacts) -> Sighting {
    let aim = target.pos + Vec3::Y * EYE_HEIGHT;
    let ignore: Vec<Entity> = viewer.subject.iter().map(|s| s.entity).chain([target.entity]).collect();
    let clear = || spatial::line_of_sight(world, viewer.eye, aim, &ignore);
    match &viewer.camera {
        Some((camera, gt, size)) => match camera.world_to_viewport(gt, aim) {
            Ok(p) if p.x >= 0.0 && p.y >= 0.0 && p.x <= size.x && p.y <= size.y => {
                if clear() { Sighting::Visible { scr: Some(p) } } else { Sighting::Occluded { scr: Some(p) } }
            }
            _ => Sighting::NotInView,
        },
        None => {
            if clear() {
                Sighting::Visible { scr: None }
            } else {
                Sighting::Occluded { scr: None }
            }
        }
    }
}

/// AI-visible entities inside the [`AiCamera`] view (landmarks and the player
/// included), with their logical viewport positions: the legend of a screenshot.
/// Empty without an active camera.
pub fn on_screen(world: &World) -> Vec<(EntityFacts, Sighting)> {
    let viewer = viewer(world, None);
    if viewer.camera.is_none() {
        return Vec::new();
    }
    all_facts(world)
        .into_iter()
        .filter_map(|f| match sight(world, &viewer, &f) {
            Sighting::NotInView => None,
            s => Some((f, s)),
        })
        .collect()
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
    match sighting {
        Some(Sighting::Visible { scr: Some(p) }) => {
            let _ = write!(line, "  scr({:.0},{:.0})", p.x, p.y);
        }
        Some(Sighting::Occluded { .. }) => line.push_str("  occluded"),
        _ => {}
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
    h
}

/// The observation text. Deterministic for a given world state.
pub fn observe(world: &World, opts: &ObserveOptions) -> String {
    let perception = world.get_resource::<AiPerception>().cloned().unwrap_or_default();
    let viewer = viewer(world, opts.subject);
    let subject_id = viewer.subject.as_ref().map(|s| s.id);

    let mut out = header(world, subject_id);
    out.push('\n');
    match &viewer.subject {
        Some(s) => {
            let _ = writeln!(out, "YOU  {}", you_line(s));
        }
        None => out.push_str("YOU  (no subject: viewpoint is the camera / world origin)\n"),
    }

    let mut seen = Vec::new();
    let mut unseen = Vec::new();
    let mut landmarks = Vec::new();
    for f in all_facts(world) {
        if Some(f.id) == subject_id {
            continue;
        }
        let dist = f.pos.distance(viewer.origin);
        if dist > perception.radius {
            continue;
        }
        if world.get::<AiLandmark>(f.entity).is_some() {
            landmarks.push((f, dist));
            continue;
        }
        match sight(world, &viewer, &f) {
            Sighting::NotInView => unseen.push((f, dist, Sighting::NotInView)),
            s => seen.push((f, dist, s)),
        }
    }
    let by_threat = |a: &(EntityFacts, f32, Sighting), b: &(EntityFacts, f32, Sighting)| {
        b.0.importance.total_cmp(&a.0.importance).then(a.1.total_cmp(&b.1)).then(a.0.id.cmp(&b.0.id))
    };
    seen.sort_by(by_threat);
    unseen.sort_by(by_threat);

    let mut budget = perception.max_listed;
    let mut section = |out: &mut String, title: &str, items: &[(EntityFacts, f32, Sighting)]| {
        if items.is_empty() {
            return;
        }
        let _ = writeln!(out, "{title}");
        let shown = items.len().min(budget);
        for (f, _, s) in &items[..shown] {
            let _ = writeln!(out, "  {}", entity_line(f, Some(viewer.origin), Some(*s)));
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
            landmarks.iter().map(|(f, _)| format!("{} {} {}", f.id, f.label, relative(viewer.origin, f.pos))).collect();
        let _ = writeln!(out, "LANDMARKS  {}", items.join(" · "));
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
