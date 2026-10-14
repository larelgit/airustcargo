//! What the `render` feature puts on screen, as plain data.
//!
//! Every drawing here is a function from the world (or from numbers) to a [`DrawList`] of
//! lines, polylines and text in world space, or to a screen position. Nothing touches a GPU,
//! a gizmo or a window, so the geometry is tested headless; the systems of the `render`
//! feature only hand the lists to Bevy's gizmos.
//!
//! - [`telegraphs`]: the warning on the ground for every attack that has a telegraph and
//!   every one aimed at the player: the zone's outline and hatch, a ring that fills as the hit
//!   nears, and the zone's **symbol** drawn as a billboard, so danger reads without colour
//!   (GDD 13.7).
//! - [`overlay`]: the combat overlay for developers: hitboxes of open windows (red),
//!   hurtboxes (green), guard arcs (blue), i-frames (outline), every zone with its time.
//! - [`edge_marker`]: where a marker for an attack off screen sits on the screen's edge.

use std::f32::consts::{PI, TAU};

use ai_core::AiId;
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::{flat, LocalPlayer};
use shards_motion::{Collider, LayerMask, PhysicsWorld, Shape};

use crate::def::{touches, ActionDef, HitShape, EPS};
use crate::fighter::{hostile, Dead, Fighter, Invulnerable, Team};
use crate::machine::ActionState;
use crate::zone::{hit_zones, HitZone, WorldShape};

// --- the drawing list ------------------------------------------------------------------

/// One thing to draw, in world space.
#[derive(Clone, Debug, PartialEq)]
pub enum Prim {
    /// Connected points: an outline, a ring, a stroke of a glyph.
    Strip { points: Vec<Vec3>, color: Color },
    Line { a: Vec3, b: Vec3, color: Color },
    /// ASCII text centered on `at`, in the plane of `rotation` (a camera's rotation makes
    /// it face the camera), `size` meters tall.
    Text { at: Vec3, rotation: Quat, text: String, size: f32, color: Color },
}

/// A frame's drawing.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct DrawList(pub Vec<Prim>);

impl DrawList {
    pub fn strip(&mut self, points: Vec<Vec3>, color: Color) {
        if points.len() >= 2 {
            self.0.push(Prim::Strip { points, color });
        }
    }

    pub fn line(&mut self, a: Vec3, b: Vec3, color: Color) {
        self.0.push(Prim::Line { a, b, color });
    }

    pub fn text(&mut self, at: Vec3, rotation: Quat, text: impl Into<String>, size: f32, color: Color) {
        self.0.push(Prim::Text { at, rotation, text: text.into(), size, color });
    }

    pub fn extend(&mut self, other: DrawList) {
        self.0.extend(other.0);
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// How many of each kind: `(strips, lines, texts)`.
    pub fn counts(&self) -> (usize, usize, usize) {
        let mut n = (0, 0, 0);
        for p in &self.0 {
            match p {
                Prim::Strip { .. } => n.0 += 1,
                Prim::Line { .. } => n.1 += 1,
                Prim::Text { .. } => n.2 += 1,
            }
        }
        n
    }
}

/// The overlay's colors (sRGB), with an alpha added when used.
pub mod palette {
    use bevy::prelude::Color;

    pub const HIT: [f32; 3] = [1.0, 0.22, 0.18];
    pub const HURT: [f32; 3] = [0.25, 0.95, 0.4];
    pub const DEAD: [f32; 3] = [0.5, 0.55, 0.5];
    pub const GUARD: [f32; 3] = [0.3, 0.55, 1.0];
    pub const PERFECT: [f32; 3] = [0.4, 0.95, 1.0];
    pub const IFRAMES: [f32; 3] = [1.0, 0.95, 0.55];
    pub const ZONE: [f32; 3] = [1.0, 0.6, 0.1];
    /// A warning for the local player: hot.
    pub const DANGER: [f32; 3] = [1.0, 0.3, 0.12];
    /// A warning of a friendly attack: cool.
    pub const ALLY: [f32; 3] = [0.3, 0.85, 0.9];

    pub fn rgba(c: [f32; 3], alpha: f32) -> Color {
        Color::srgba(c[0], c[1], c[2], alpha.clamp(0.0, 1.0))
    }
}

use palette::rgba;

// --- rings and footprints ---------------------------------------------------------------

/// Points of a circle of `radius` around `center` in the ground plane at `y`, `n` segments;
/// the last point equals the first.
pub fn ring(center: Vec3, radius: f32, y: f32, n: usize) -> Vec<Vec3> {
    (0..=n)
        .map(|i| {
            let a = TAU * i as f32 / n as f32;
            Vec3::new(center.x + radius * a.cos(), y, center.z + radius * a.sin())
        })
        .collect()
}

/// The footprint of `shape` on the ground plane at height `y`: a closed ring of points (the
/// first point is not repeated), the same 2D shape [`WorldShape::exit`] answers for. A
/// sphere counts as its widest disc, a cone as a flat sector.
pub fn footprint(shape: &WorldShape, y: f32) -> Vec<Vec3> {
    let (o, f, r) = (shape.origin, shape.facing, shape.right());
    let at = |p: Vec3| Vec3::new(p.x, y, p.z);
    let arc = |from: f32, to: f32, center: Vec3, radius: f32, n: usize, out: &mut Vec<Vec3>| {
        for i in 0..=n {
            let a = from + (to - from) * i as f32 / n as f32;
            out.push(at(center + r * (radius * a.cos()) + f * (radius * a.sin())));
        }
    };
    let mut out = Vec::new();
    match shape.shape {
        HitShape::Sphere { radius } | HitShape::Circle { radius, .. } => {
            out.extend(ring(o, radius, y, 40).into_iter().take(40));
        }
        HitShape::Capsule { radius, length } => {
            // The back cap round the start (from -right to +right through -forward), then the
            // front cap round the end.
            arc(PI, TAU, o, radius, 16, &mut out);
            arc(0.0, PI, o + f * length, radius, 16, &mut out);
        }
        HitShape::Cone { range, angle, .. } => {
            let half = (angle.to_radians() * 0.5).min(PI);
            if half >= PI - 1e-3 {
                out.extend(ring(o, range, y, 48).into_iter().take(48));
            } else {
                out.push(at(o));
                let n = ((angle / 6.0).ceil() as usize).clamp(4, 60);
                for i in 0..=n {
                    let phi = -half + 2.0 * half * i as f32 / n as f32;
                    out.push(at(o + f * (range * phi.cos()) + r * (range * phi.sin())));
                }
            }
        }
        HitShape::Box { width, depth, .. } => {
            let (hx, hz) = (width * 0.5, depth * 0.5);
            out.extend([r * hx + f * hz, r * hx - f * hz, -r * hx - f * hz, -r * hx + f * hz].map(|p| at(o + p)));
        }
        HitShape::Line { length, width, .. } => {
            let (hx, c) = (width * 0.5, o + f * length * 0.5);
            let hz = length * 0.5;
            out.extend([r * hx + f * hz, r * hx - f * hz, -r * hx - f * hz, -r * hx + f * hz].map(|p| at(c + p)));
        }
    }
    out
}

/// `ring` closed (first point repeated at the end) for drawing as a strip.
fn closed(points: &[Vec3]) -> Vec<Vec3> {
    let mut v = points.to_vec();
    if let Some(first) = points.first() {
        v.push(*first);
    }
    v
}

/// The average of the ring's points: where a fill grows from.
fn centroid(ring: &[Vec3]) -> Vec3 {
    if ring.is_empty() {
        return Vec3::ZERO;
    }
    ring.iter().copied().sum::<Vec3>() / ring.len() as f32
}

/// `ring` scaled about `about` (ground plane only) by `k`.
fn scaled(ring: &[Vec3], about: Vec3, k: f32) -> Vec<Vec3> {
    ring.iter().map(|p| Vec3::new(about.x + (p.x - about.x) * k, p.y, about.z + (p.z - about.z) * k)).collect()
}

/// Parallel lines `spacing` apart, at `angle` (radians) in the ground plane, clipped to
/// the polygon `ring` (even-odd): the hatch that stands for a fill. Anchored to world
/// coordinates, so a zone that moves does not make its hatch slide. At most about 80 lines.
pub fn hatch(ring: &[Vec3], spacing: f32, angle: f32) -> Vec<(Vec3, Vec3)> {
    if ring.len() < 3 {
        return Vec::new();
    }
    let (dir, normal) = (Vec2::new(angle.cos(), angle.sin()), Vec2::new(-angle.sin(), angle.cos()));
    let pts: Vec<Vec2> = ring.iter().map(|p| Vec2::new(p.x, p.z)).collect();
    let (lo, hi) = pts.iter().fold((f32::MAX, f32::MIN), |(lo, hi), p| (lo.min(p.dot(normal)), hi.max(p.dot(normal))));
    let spacing = spacing.max((hi - lo) / 80.0).max(0.02);
    let y = ring[0].y;
    let mut out = Vec::new();
    let mut s = (lo / spacing).ceil() * spacing;
    if s <= lo + 1e-4 {
        s += spacing;
    }
    while s < hi - 1e-4 {
        let mut xs: Vec<f32> = Vec::new();
        for i in 0..pts.len() {
            let (a, b) = (pts[i], pts[(i + 1) % pts.len()]);
            let (da, db) = (a.dot(normal) - s, b.dot(normal) - s);
            if (da < 0.0) != (db < 0.0) {
                let k = da / (da - db);
                xs.push((a + (b - a) * k).dot(dir));
            }
        }
        xs.sort_by(f32::total_cmp);
        for pair in xs.as_chunks::<2>().0 {
            let at = |t: f32| {
                let p = dir * t + normal * s;
                Vec3::new(p.x, y, p.y)
            };
            out.push((at(pair[0]), at(pair[1])));
        }
        s += spacing;
    }
    out
}

// --- wireframes of volumes ---------------------------------------------------------------

fn circle_in(center: Vec3, u: Vec3, v: Vec3, radius: f32, n: usize) -> Vec<Vec3> {
    (0..=n)
        .map(|i| {
            let a = TAU * i as f32 / n as f32;
            center + u * (radius * a.cos()) + v * (radius * a.sin())
        })
        .collect()
}

fn semicircle_in(center: Vec3, u: Vec3, v: Vec3, radius: f32, n: usize) -> Vec<Vec3> {
    (0..=n)
        .map(|i| {
            let a = PI * i as f32 / n as f32;
            center + u * (radius * a.cos()) + v * (radius * a.sin())
        })
        .collect()
}

/// The wireframe of a physics shape at `position` turned by `rotation`, as polylines.
pub fn shape_wireframe(shape: &Shape, position: Vec3, rotation: Quat) -> Vec<Vec<Vec3>> {
    let place = |p: Vec3| position + rotation * p;
    let mut lines: Vec<Vec<Vec3>> = Vec::new();
    match *shape {
        Shape::Cuboid { half } => {
            let corner = |sx: f32, sy: f32, sz: f32| Vec3::new(half.x * sx, half.y * sy, half.z * sz);
            for sy in [-1.0, 1.0] {
                lines.push(vec![
                    corner(-1.0, sy, -1.0),
                    corner(1.0, sy, -1.0),
                    corner(1.0, sy, 1.0),
                    corner(-1.0, sy, 1.0),
                    corner(-1.0, sy, -1.0),
                ]);
            }
            for (sx, sz) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
                lines.push(vec![corner(sx, -1.0, sz), corner(sx, 1.0, sz)]);
            }
        }
        Shape::Sphere { radius } => {
            lines.push(circle_in(Vec3::ZERO, Vec3::X, Vec3::Y, radius, 24));
            lines.push(circle_in(Vec3::ZERO, Vec3::X, Vec3::Z, radius, 24));
            lines.push(circle_in(Vec3::ZERO, Vec3::Y, Vec3::Z, radius, 24));
        }
        Shape::Capsule { half_height, radius } => {
            let (top, bottom) = (Vec3::Y * half_height, Vec3::NEG_Y * half_height);
            lines.push(circle_in(top, Vec3::X, Vec3::Z, radius, 20));
            lines.push(circle_in(bottom, Vec3::X, Vec3::Z, radius, 20));
            for (x, z) in [(radius, 0.0), (-radius, 0.0), (0.0, radius), (0.0, -radius)] {
                lines.push(vec![bottom + Vec3::new(x, 0.0, z), top + Vec3::new(x, 0.0, z)]);
            }
            for side in [Vec3::X, Vec3::Z] {
                lines.push(semicircle_in(top, side, Vec3::Y, radius, 10));
                lines.push(semicircle_in(bottom, side, Vec3::NEG_Y, radius, 10));
            }
        }
        Shape::Cylinder { half_height, radius } => {
            let (top, bottom) = (Vec3::Y * half_height, Vec3::NEG_Y * half_height);
            lines.push(circle_in(top, Vec3::X, Vec3::Z, radius, 24));
            lines.push(circle_in(bottom, Vec3::X, Vec3::Z, radius, 24));
            for (x, z) in [(radius, 0.0), (-radius, 0.0), (0.0, radius), (0.0, -radius)] {
                lines.push(vec![bottom + Vec3::new(x, 0.0, z), top + Vec3::new(x, 0.0, z)]);
            }
        }
        Shape::Cone { half_height, radius } => {
            let (apex, base) = (Vec3::Y * half_height, Vec3::NEG_Y * half_height);
            lines.push(circle_in(base, Vec3::X, Vec3::Z, radius, 24));
            for (x, z) in [(radius, 0.0), (-radius, 0.0), (0.0, radius), (0.0, -radius)] {
                lines.push(vec![base + Vec3::new(x, 0.0, z), apex]);
            }
        }
    }
    lines.into_iter().map(|l| l.into_iter().map(place).collect()).collect()
}

/// The wireframe of a hit shape as the hit detection sees it: the physics volume, except for
/// a cone, whose true sector (the narrowing the hit pass applies) is drawn instead of the
/// cylinder that bounds it.
pub fn hitbox_wireframe(shape: &WorldShape) -> Vec<Vec<Vec3>> {
    match shape.shape {
        HitShape::Cone { height, .. } => {
            let base = footprint(shape, shape.origin.y);
            let top = footprint(shape, shape.origin.y + height);
            let mut lines = vec![closed(&base), closed(&top)];
            let n = base.len();
            // The corners: the apex, the two ends of the arc and some points between.
            let step = (n / 6).max(1);
            lines.extend((0..n).step_by(step).map(|i| vec![base[i], top[i]]));
            lines.push(vec![base[n - 1], top[n - 1]]);
            lines
        }
        _ => {
            let (volume, center, rotation) = shape.physics();
            shape_wireframe(&volume, center, rotation)
        }
    }
}

// --- symbols ------------------------------------------------------------------------------

/// Strokes of a symbol glyph in a cell `x` in -0.5..0.5, `y` in 0..1, or `None` when the
/// character is an ordinary one the stroke font draws.
fn glyph_strokes(c: char) -> Option<Vec<Vec<Vec2>>> {
    let v = Vec2::new;
    let bang = |top: f32, bottom: f32, dot: f32, half: f32| {
        vec![
            vec![v(0.0, top), v(0.0, bottom)],
            vec![v(-half, top), v(-half, bottom + half)],
            vec![v(half, top), v(half, bottom + half)],
            vec![v(-half, dot + half), v(half, dot + half), v(half, dot - half), v(-half, dot - half), v(-half, dot + half)],
        ]
    };
    Some(match c {
        '!' => bang(1.0, 0.3, 0.08, 0.05),
        // A warning triangle with a bang in it.
        '⚠' => {
            let mut s = vec![vec![v(-0.5, 0.0), v(0.5, 0.0), v(0.0, 0.92), v(-0.5, 0.0)]];
            s.extend(bang(0.68, 0.3, 0.17, 0.025));
            s
        }
        // A zig-zag of lightning ending in an arrowhead.
        '↯' | '⚡' => vec![
            vec![v(0.18, 1.0), v(-0.2, 0.52), v(0.2, 0.48), v(-0.12, 0.0)],
            vec![v(-0.4, 0.2), v(-0.12, 0.0), v(0.1, 0.24)],
        ],
        '✖' | '✕' | '×' => vec![vec![v(-0.35, 0.15), v(0.35, 0.85)], vec![v(-0.35, 0.85), v(0.35, 0.15)]],
        '▲' => vec![vec![v(-0.5, 0.0), v(0.5, 0.0), v(0.0, 0.9), v(-0.5, 0.0)]],
        '◆' | '◇' => vec![vec![v(0.0, 0.0), v(0.4, 0.5), v(0.0, 1.0), v(-0.4, 0.5), v(0.0, 0.0)]],
        '↓' => vec![vec![v(0.0, 1.0), v(0.0, 0.05)], vec![v(-0.3, 0.35), v(0.0, 0.05), v(0.3, 0.35)]],
        '↑' => vec![vec![v(0.0, 0.0), v(0.0, 0.95)], vec![v(-0.3, 0.65), v(0.0, 0.95), v(0.3, 0.65)]],
        c if c.is_ascii() => return None,
        // Any other symbol the stroke font cannot draw: a bang, the generic warning.
        _ => return glyph_strokes('!'),
    })
}

/// A symbol as drawing: vector glyphs for the symbols games use for danger (`!`, `⚠`, `↯`,
/// `✖`...), the stroke font for plain ASCII text, left to right and centered on `at` in the
/// plane of `rotation`. `height` is its size in meters.
pub fn symbol(symbol: &str, at: Vec3, rotation: Quat, height: f32, color: Color) -> DrawList {
    let mut out = DrawList::default();
    let chars: Vec<char> = symbol.chars().collect();
    if chars.is_empty() {
        return out;
    }
    if chars.iter().all(|c| c.is_ascii() && !matches!(c, '!')) {
        out.text(at, rotation, symbol, height, color);
        return out;
    }
    let advance = 0.8 * height;
    let width = advance * chars.len() as f32;
    for (i, c) in chars.iter().enumerate() {
        let x = -width * 0.5 + advance * (i as f32 + 0.5);
        let center = at + rotation * Vec3::new(x, 0.0, 0.0);
        match glyph_strokes(*c) {
            Some(strokes) => {
                for stroke in strokes {
                    let points =
                        stroke.iter().map(|p| center + rotation * Vec3::new(p.x * height, (p.y - 0.5) * height, 0.0));
                    out.strip(points.collect(), color);
                }
            }
            None => out.text(center, rotation, c.to_string(), height, color),
        }
    }
    out
}

/// The symbol as plain ASCII, for a font that has nothing else (Bevy's default font):
/// `⚠` and `‼` become `!`, `↯` `/`, `✖` `x`; ASCII stays.
pub fn ascii_symbol(symbol: &str) -> String {
    symbol
        .chars()
        .map(|c| match c {
            c if c.is_ascii() => c.to_string(),
            '⚠' | '‼' => "!".to_owned(),
            '↯' | '⚡' => "/".to_owned(),
            '✖' | '✕' | '×' => "x".to_owned(),
            '▲' => "^".to_owned(),
            '↓' => "v".to_owned(),
            _ => "!".to_owned(),
        })
        .collect()
}

// --- telegraphs ----------------------------------------------------------------------------

/// How a ground telegraph looks and grows.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TelegraphStyle {
    /// Seconds before a hit lands that the warning starts to grow; farther ahead it shows
    /// at its faintest.
    pub horizon: f32,
    /// Distance between hatch lines (m) at the faintest and at the moment of the hit.
    pub hatch_far: f32,
    pub hatch_near: f32,
    /// Height above the ground (m), to stay clear of the floor.
    pub lift: f32,
    /// Height of the symbol (m) and how far above the ground it floats.
    pub symbol_size: f32,
    pub symbol_lift: f32,
}

impl Default for TelegraphStyle {
    fn default() -> Self {
        Self { horizon: 1.2, hatch_far: 1.1, hatch_near: 0.28, lift: 0.04, symbol_size: 0.9, symbol_lift: 2.4 }
    }
}

/// How urgent a hit `lands_in` seconds away is: 0 at the `horizon` and beyond, 1 when it
/// lands (and while it is active).
pub fn intensity(lands_in: f32, horizon: f32) -> f32 {
    if horizon <= EPS {
        return 1.0;
    }
    (1.0 - lands_in / horizon).clamp(0.0, 1.0)
}

/// A flicker that speeds up as the hit nears: a factor around 0.75..1.
pub fn pulse(intensity: f32, time: f32) -> f32 {
    0.875 + 0.125 * (time * (4.0 + 14.0 * intensity) * TAU * 0.5).sin()
}

/// The ground telegraph of one zone: outline, hatch, the filling ring and the symbol.
/// `ground_y` is the floor under the attacker; `hostile` picks the warm palette.
pub fn telegraph(
    zone: &HitZone,
    ground_y: f32,
    hostile: bool,
    style: &TelegraphStyle,
    time: f32,
    billboard: Quat,
) -> DrawList {
    let mut out = DrawList::default();
    let k = intensity(zone.lands_in, style.horizon);
    let base = if hostile { palette::DANGER } else { palette::ALLY };
    let flicker = pulse(k, time);
    let y = ground_y + style.lift;
    let outline = footprint(&zone.shape, y);
    let middle = centroid(&outline);

    out.strip(closed(&outline), rgba(base, (0.4 + 0.6 * k) * flicker));
    let spacing = style.hatch_far + (style.hatch_near - style.hatch_far) * k;
    for (a, b) in hatch(&outline, spacing, std::f32::consts::FRAC_PI_4) {
        out.line(a, b, rgba(base, (0.12 + 0.4 * k) * flicker));
    }
    // The fill: a copy of the outline that grows from the middle and reaches it as the hit
    // lands. Read as a timer even in grey.
    if k < 1.0 - 1e-3 {
        out.strip(closed(&scaled(&outline, middle, k.max(0.04))), rgba(base, 0.55 + 0.45 * k));
    } else {
        out.strip(closed(&scaled(&outline, middle, 0.98)), rgba(base, flicker));
    }
    // The symbol: the telegraph's, or a bang for an attack that gives no warning of its own
    // (what `threat_indicators` and the agent's ALERT say too).
    let sym = zone.symbol.as_deref().unwrap_or("!");
    let size = style.symbol_size * (0.8 + 0.4 * k);
    let at = Vec3::new(middle.x, ground_y + style.symbol_lift, middle.z);
    out.extend(symbol(sym, at, billboard, size, rgba(base, 0.6 + 0.4 * k)));
    // A stem from the zone to its symbol.
    out.line(Vec3::new(middle.x, y, middle.z), at - Vec3::Y * (size * 0.6), rgba(base, 0.3 + 0.3 * k));
    out
}

/// The floor under an attacker: its feet.
fn ground_of(world: &World, attacker: Entity, fallback: f32) -> f32 {
    world.get::<Transform>(attacker).map_or(fallback, |t| t.translation.y)
}

/// The local player and its team, if there is one.
fn local_player(world: &World) -> Option<(Entity, Option<Team>)> {
    let mut q = world.try_query_filtered::<(Entity, Option<&Team>), With<LocalPlayer>>()?;
    q.iter(world).next().map(|(e, t)| (e, t.cloned()))
}

/// The ground telegraphs, for the local player's eyes: every zone of an attack that has a
/// telegraph, and every zone hostile to the player even without one (drawn with a bang).
/// Enemies' zones are warm, the others cool; the player's own swings are no warning and stay
/// off the ground (the combat overlay shows them). With no local player every zone is hostile.
pub fn telegraphs(world: &World, style: &TelegraphStyle, time: f32, billboard: Quat) -> DrawList {
    let me = local_player(world);
    let mut out = DrawList::default();
    for zone in hit_zones(world) {
        let mine = me.as_ref().is_some_and(|(e, _)| *e == zone.attacker);
        let hostile = !mine && me.as_ref().is_none_or(|(_, team)| hostile(zone.team.as_ref(), team.as_ref()));
        if zone.symbol.is_none() && !hostile {
            continue;
        }
        let ground = ground_of(world, zone.attacker, zone.shape.origin.y);
        out.extend(telegraph(&zone, ground, hostile, style, time, billboard));
    }
    out
}

// --- the combat overlay ------------------------------------------------------------------------

/// What the combat overlay shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct OverlayParts {
    pub hitboxes: bool,
    pub hurtboxes: bool,
    pub guards: bool,
    pub iframes: bool,
    pub zones: bool,
}

impl Default for OverlayParts {
    fn default() -> Self {
        Self { hitboxes: true, hurtboxes: true, guards: true, iframes: true, zones: true }
    }
}

/// What [`overlay`] found, for the tool's answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OverlayCounts {
    pub hitboxes: usize,
    pub hurtboxes: usize,
    pub guards: usize,
    pub iframes: usize,
    pub zones: usize,
}

impl OverlayCounts {
    /// `2 hitboxes, 4 hurtboxes, 1 guard, 0 i-frames, 3 zones`.
    pub fn describe(&self) -> String {
        let n = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        format!(
            "{}, {}, {}, {}, {}",
            n(self.hitboxes, "hitbox", "hitboxes"),
            n(self.hurtboxes, "hurtbox", "hurtboxes"),
            n(self.guards, "guard", "guards"),
            n(self.iframes, "i-frame window", "i-frame windows"),
            n(self.zones, "zone", "zones")
        )
    }
}

/// An arc of `degrees` around `facing` (ground plane), radius `radius`, at height `y` above
/// `feet`, with its two ends joined to the center.
fn guard_arc(feet: Vec3, facing: Vec3, degrees: f32, radius: f32, y: f32) -> Vec<Vec3> {
    let f = flat(facing).try_normalize().unwrap_or(Vec3::NEG_Z);
    let r = Vec3::new(-f.z, 0.0, f.x);
    let centre = feet + Vec3::Y * y;
    let half = (degrees.to_radians() * 0.5).min(PI);
    let n = ((degrees / 6.0).ceil() as usize).clamp(4, 60);
    let mut points = vec![centre];
    points.extend((0..=n).map(|i| {
        let phi = -half + 2.0 * half * i as f32 / n as f32;
        centre + f * (radius * phi.cos()) + r * (radius * phi.sin())
    }));
    if half < PI - 1e-3 {
        points.push(centre);
    }
    points
}

/// A shape a little bigger, for the i-frame outline around a body.
fn inflated(shape: Shape, by: f32) -> Shape {
    match shape {
        Shape::Cuboid { half } => Shape::Cuboid { half: half + Vec3::splat(by) },
        Shape::Sphere { radius } => Shape::Sphere { radius: radius + by },
        Shape::Capsule { half_height, radius } => Shape::Capsule { half_height, radius: radius + by },
        Shape::Cylinder { half_height, radius } => Shape::Cylinder { half_height: half_height + by, radius: radius + by },
        Shape::Cone { half_height, radius } => Shape::Cone { half_height: half_height + by, radius: radius + by },
    }
}

/// `light_1 0.20s` before a hit, `light_1 now 0.10s` while it is open.
fn zone_label(zone: &HitZone) -> String {
    if zone.lands_in > EPS {
        format!("{} {:.2}s", zone.action, zone.lands_in)
    } else {
        format!("{} now {:.2}s", zone.action, zone.active_for.max(0.0))
    }
}

/// The combat overlay (TZ E11.2) of the world as it is now: hitboxes of the windows open in
/// the last tick (red), hurtboxes (green, grey when dead), guard arcs (blue; the perfect
/// window brighter), i-frames (an outline around the body), and every [`HitZone`] still to
/// come or open on the ground with the time left. Fighters in `AiId` order; read-only.
pub fn overlay(world: &World, parts: OverlayParts, billboard: Quat) -> (DrawList, OverlayCounts) {
    let mut out = DrawList::default();
    let mut counts = OverlayCounts::default();
    let Some(db) = world.get_resource::<ContentDb>() else { return (out, counts) };

    if parts.hurtboxes
        && let Some(physics) = world.get_resource::<PhysicsWorld>()
    {
        for c in physics.colliders().filter(|c| c.layers.memberships.contains(LayerMask::HURTBOX)) {
            let dead = world.get::<Dead>(c.entity).is_some();
            let color = rgba(if dead { palette::DEAD } else { palette::HURT }, if dead { 0.5 } else { 0.9 });
            for line in shape_wireframe(&c.shape, c.position, c.rotation) {
                out.strip(line, color);
            }
            counts.hurtboxes += 1;
        }
    }

    if let Some(mut q) = world.try_query_filtered::<(Entity, Option<&AiId>, &ActionState, &Transform), With<Fighter>>() {
        let mut fighters: Vec<_> = q.iter(world).filter(|(_, _, s, _)| s.current.is_some()).collect();
        fighters.sort_by_key(|(e, id, ..)| (id.map_or(u32::MAX, |i| i.0), *e));
        for (entity, _, state, tf) in fighters {
            let Some(run) = &state.current else { continue };
            let Some(def) = db.get::<ActionDef>(&run.action) else { continue };
            let feet = tf.translation;
            if parts.hitboxes {
                for hit in &def.hits {
                    if !def.span(&hit.span).is_some_and(|w| touches(w, run.prev, run.t)) {
                        continue;
                    }
                    let shape = WorldShape::new(hit.shape, hit.at, feet, run.facing);
                    for line in hitbox_wireframe(&shape) {
                        out.strip(line, rgba(palette::HIT, 1.0));
                    }
                    counts.hitboxes += 1;
                }
            }
            if parts.guards
                && let Some(guard) = def.guard.as_ref().filter(|_| run.windows.guard || run.windows.perfect)
            {
                let (color, radius) =
                    if run.windows.perfect { (rgba(palette::PERFECT, 1.0), 1.5) } else { (rgba(palette::GUARD, 0.9), 1.3) };
                out.strip(guard_arc(feet, run.facing, guard.arc, radius, 0.9), color);
                if run.windows.perfect {
                    out.strip(guard_arc(feet, run.facing, guard.arc, radius - 0.15, 0.9), color);
                }
                counts.guards += 1;
            }
            if parts.iframes && run.windows.iframes {
                let body = world
                    .get::<Collider>(entity)
                    .map(|c| (inflated(c.shape, 0.12), feet + c.offset))
                    .unwrap_or((Shape::Capsule { half_height: 0.5, radius: 0.5 }, feet + Vec3::Y));
                for line in shape_wireframe(&body.0, body.1, tf.rotation) {
                    out.strip(line, rgba(palette::IFRAMES, 0.9));
                }
                counts.iframes += 1;
            }
        }
    }
    if parts.iframes
        && let Some(mut q) = world.try_query_filtered::<(&Transform, &Collider), With<Invulnerable>>()
    {
        // God mode: the same outline as i-frames, dimmer, for as long as it lasts.
        for (tf, c) in q.iter(world) {
            for line in shape_wireframe(&inflated(c.shape, 0.12), tf.translation + c.offset, tf.rotation) {
                out.strip(line, rgba(palette::IFRAMES, 0.45));
            }
        }
    }

    if parts.zones {
        for zone in hit_zones(world) {
            let ground = ground_of(world, zone.attacker, zone.shape.origin.y) + 0.06;
            let ring = footprint(&zone.shape, ground);
            let k = intensity(zone.lands_in, 1.2);
            out.strip(closed(&ring), rgba(palette::ZONE, 0.5 + 0.5 * k));
            let middle = centroid(&ring);
            out.text(middle + Vec3::Y * 0.5, billboard, zone_label(&zone), 0.3, rgba(palette::ZONE, 1.0));
            counts.zones += 1;
        }
    }
    (out, counts)
}

// --- the screen edge -----------------------------------------------------------------------------

/// Where an off-screen warning sits on the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EdgeMarker {
    /// Pixels from the top left corner of the viewport.
    pub position: Vec2,
    /// Unit vector on screen (x right, y down) from the center toward the marker: the way
    /// the warning points.
    pub direction: Vec2,
}

impl EdgeMarker {
    /// The direction as degrees clockwise from straight up.
    pub fn angle_deg(&self) -> f32 {
        self.direction.x.atan2(-self.direction.y).to_degrees()
    }
}

/// The spot on the edge of a `viewport`-sized screen (inset by `margin` pixels) for a
/// warning `angle_deg` from where the camera looks on the ground plane (0 ahead, +90 right,
/// ±180 behind, as [`Indicator::angle_deg`](crate::Indicator)): ahead is the top edge, right
/// the right edge, behind the bottom.
pub fn edge_marker(angle_deg: f32, viewport: Vec2, margin: f32) -> EdgeMarker {
    let a = angle_deg.to_radians();
    let direction = Vec2::new(a.sin(), -a.cos());
    let half = (viewport * 0.5 - Vec2::splat(margin)).max(Vec2::splat(1.0));
    let reach = |d: f32, h: f32| if d.abs() > 1e-5 { h / d.abs() } else { f32::INFINITY };
    let t = reach(direction.x, half.x).min(reach(direction.y, half.y));
    EdgeMarker { position: viewport * 0.5 + direction * t, direction }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::def::Local;

    const N: Vec3 = Vec3::NEG_Z;

    fn at(shape: HitShape) -> WorldShape {
        WorldShape::new(shape, Local::default(), Vec3::ZERO, N)
    }

    fn zone(shape: HitShape, lands_in: f32, symbol: Option<&str>) -> HitZone {
        HitZone {
            attacker: Entity::PLACEHOLDER,
            attacker_id: None,
            team: None,
            action: "slam".into(),
            hit: 0,
            shape: at(shape),
            lands_in,
            active_for: 0.3,
            symbol: symbol.map(str::to_owned),
            flags: Vec::new(),
        }
    }

    fn inside_ring(ring: &[Vec3], p: Vec3) -> bool {
        // Even-odd in the ground plane.
        let mut inside = false;
        for i in 0..ring.len() {
            let (a, b) = (ring[i], ring[(i + 1) % ring.len()]);
            if (a.z > p.z) != (b.z > p.z) && p.x < a.x + (p.z - a.z) / (b.z - a.z) * (b.x - a.x) {
                inside = !inside;
            }
        }
        inside
    }

    #[test]
    fn footprints_agree_with_the_hit_test() {
        let shapes = [
            HitShape::Circle { radius: 4.5, height: 2.0 },
            HitShape::Sphere { radius: 1.5 },
            HitShape::Capsule { radius: 0.6, length: 3.0 },
            HitShape::Box { width: 2.0, height: 2.0, depth: 4.0 },
            HitShape::Line { length: 8.0, width: 1.0, height: 0.5 },
            HitShape::Cone { range: 3.0, angle: 90.0, height: 2.0 },
            HitShape::Cone { range: 3.0, angle: 360.0, height: 2.0 },
        ];
        for shape in shapes {
            let ws = WorldShape::new(shape, Local { forward: 0.5, ..default() }, Vec3::new(2.0, 0.0, 3.0), Vec3::new(1.0, 0.0, -1.0));
            let ring = footprint(&ws, 0.0);
            assert!(ring.len() >= 4, "{shape:?}: {}", ring.len());
            let middle = ws.center();
            for p in &ring {
                // A hair inside each outline point is in the zone, a hair outside is not.
                let inward = *p + (middle - *p) * 0.03;
                let outward = *p + (*p - middle) * 0.05;
                assert!(ws.contains(flat(inward), 0.0) || (p.distance(middle) < 0.3), "{shape:?}: {inward} inside");
                if !matches!(shape, HitShape::Cone { angle, .. } if angle > 180.0) {
                    assert!(!ws.contains(flat(outward), 0.0), "{shape:?}: {outward} is outside");
                }
            }
            assert!(inside_ring(&ring, middle), "{shape:?}: the center is inside its own footprint");
        }
    }

    #[test]
    fn the_cone_footprint_is_an_apex_and_an_arc() {
        let ring = footprint(&at(HitShape::Cone { range: 3.0, angle: 90.0, height: 2.0 }), 0.5);
        assert_eq!(ring[0], Vec3::new(0.0, 0.5, 0.0), "the apex");
        assert!(ring.iter().skip(1).all(|p| ((p.x * p.x + p.z * p.z).sqrt() - 3.0).abs() < 1e-3 && p.y == 0.5));
        // The arc spans +-45 degrees about -Z.
        let (first, last) = (ring[1], *ring.last().unwrap());
        assert!((first.x.abs() - 3.0 * 45f32.to_radians().sin()).abs() < 1e-3 && first.z < 0.0, "{first}");
        assert!((first.x + last.x).abs() < 1e-3, "symmetric about the facing");
    }

    #[test]
    fn hatch_stays_inside_the_outline() {
        let shapes = [
            HitShape::Circle { radius: 4.5, height: 2.0 },
            HitShape::Box { width: 2.0, height: 2.0, depth: 4.0 },
            HitShape::Cone { range: 3.0, angle: 270.0, height: 2.0 },
            HitShape::Capsule { radius: 0.6, length: 3.0 },
        ];
        for shape in shapes {
            let ring = footprint(&at(shape), 0.0);
            let lines = hatch(&ring, 0.5, 0.7);
            assert!(lines.len() > 3, "{shape:?}: {}", lines.len());
            for (a, b) in lines {
                let mid = (a + b) * 0.5;
                assert!(inside_ring(&ring, mid), "{shape:?}: {mid}");
                // Both ends sit on the border: just beyond them is outside.
                let along = (b - a).normalize_or_zero();
                assert!(!inside_ring(&ring, a - along * 0.02) && !inside_ring(&ring, b + along * 0.02), "{shape:?}");
            }
        }
    }

    #[test]
    fn hatch_density_follows_the_spacing_and_stays_bounded() {
        let ring = footprint(&at(HitShape::Circle { radius: 4.0, height: 2.0 }), 0.0);
        let sparse = hatch(&ring, 1.0, 0.0).len();
        let dense = hatch(&ring, 0.25, 0.0).len();
        assert!(dense > sparse * 3, "{sparse} -> {dense}");
        assert!(hatch(&ring, 0.0001, 0.0).len() <= 90, "a degenerate spacing cannot flood the gizmos");
        assert!(hatch(&ring[..2], 0.5, 0.0).is_empty());
        // Anchored to the world: shifting the ring does not move the lines it shares.
        let moved: Vec<Vec3> = ring.iter().map(|p| *p + Vec3::new(0.0, 0.0, 0.5)).collect();
        let a = hatch(&ring, 0.5, 0.0);
        let b = hatch(&moved, 0.5, 0.0);
        assert!(a.iter().any(|(p, _)| b.iter().any(|(q, _)| (p.z - q.z).abs() < 1e-4)), "same lattice");
    }

    #[test]
    fn urgency_grows_as_the_hit_nears() {
        assert_eq!(intensity(5.0, 1.2), 0.0);
        assert_eq!(intensity(0.0, 1.2), 1.0);
        assert!((intensity(0.6, 1.2) - 0.5).abs() < 1e-6);
        assert!(intensity(0.9, 1.2) < intensity(0.3, 1.2));
        assert_eq!(intensity(1.0, 0.0), 1.0);
        for t in [0.0, 0.1, 0.77, 3.0] {
            for k in [0.0, 0.5, 1.0] {
                let p = pulse(k, t);
                assert!((0.74..=1.01).contains(&p), "{p}");
            }
        }
    }

    #[test]
    fn a_telegraph_has_outline_hatch_fill_and_symbol() {
        let style = TelegraphStyle::default();
        let z = zone(HitShape::Circle { radius: 3.0, height: 2.0 }, 0.6, Some("!"));
        let list = telegraph(&z, 0.0, true, &style, 0.0, Quat::IDENTITY);
        let (strips, lines, texts) = list.counts();
        assert!(strips >= 2 + 4, "outline, fill ring and the bang's strokes: {strips}");
        assert!(lines > 5, "the hatch and the stem: {lines}");
        assert_eq!(texts, 0, "a bang is drawn from strokes");
        // Everything sits on the ground, the symbol above it.
        for p in &list.0 {
            if let Prim::Strip { points, .. } = p {
                for q in points {
                    assert!(q.y >= style.lift - 1e-4, "{q}");
                }
            }
        }
        let highest = list
            .0
            .iter()
            .flat_map(|p| match p {
                Prim::Strip { points, .. } => points.clone(),
                Prim::Line { a, b, .. } => vec![*a, *b],
                Prim::Text { at, .. } => vec![*at],
            })
            .map(|p| p.y)
            .fold(0.0, f32::max);
        assert!(highest > style.symbol_lift, "the symbol floats above the zone: {highest}");
        // The fill ring is half way to the outline at half the horizon... (lands_in 0.6 of 1.2).
        let rings: Vec<&Vec<Vec3>> = list
            .0
            .iter()
            .filter_map(|p| match p {
                Prim::Strip { points, .. } if points.len() > 30 => Some(points),
                _ => None,
            })
            .collect();
        assert_eq!(rings.len(), 2);
        let radius = |r: &Vec<Vec3>| r.iter().map(|p| (p.x * p.x + p.z * p.z).sqrt()).fold(0.0, f32::max);
        assert!((radius(rings[0]) - 3.0).abs() < 1e-3);
        assert!((radius(rings[1]) - 1.5).abs() < 1e-3, "the fill grows with the urgency: {}", radius(rings[1]));
    }

    #[test]
    fn the_hatch_thickens_and_the_fill_closes_as_the_hit_lands() {
        let style = TelegraphStyle::default();
        let far = telegraph(&zone(HitShape::Circle { radius: 3.0, height: 2.0 }, 1.1, Some("!")), 0.0, true, &style, 0.0, Quat::IDENTITY);
        let near = telegraph(&zone(HitShape::Circle { radius: 3.0, height: 2.0 }, 0.1, Some("!")), 0.0, true, &style, 0.0, Quat::IDENTITY);
        let active = telegraph(&zone(HitShape::Circle { radius: 3.0, height: 2.0 }, 0.0, Some("!")), 0.0, true, &style, 0.0, Quat::IDENTITY);
        assert!(near.counts().1 > far.counts().1 * 2, "{} -> {}", far.counts().1, near.counts().1);
        assert!(active.counts().1 >= near.counts().1);
        let alpha = |l: &DrawList| match &l.0[0] {
            Prim::Strip { color, .. } => color.to_srgba().alpha,
            _ => unreachable!(),
        };
        assert!(alpha(&near) > alpha(&far), "the outline brightens: {} -> {}", alpha(&far), alpha(&near));
    }

    #[test]
    fn warnings_of_allies_are_cool_and_of_enemies_warm() {
        let style = TelegraphStyle::default();
        let z = zone(HitShape::Sphere { radius: 1.0 }, 0.5, Some("!"));
        let colour = |hostile| match &telegraph(&z, 0.0, hostile, &style, 0.0, Quat::IDENTITY).0[0] {
            Prim::Strip { color, .. } => color.to_srgba(),
            _ => unreachable!(),
        };
        let (hot, cool) = (colour(true), colour(false));
        assert!(hot.red > hot.blue && cool.blue > cool.red, "{hot:?} {cool:?}");
    }

    #[test]
    fn symbols_draw_as_strokes_or_text() {
        let q = Quat::IDENTITY;
        let c = Color::WHITE;
        let bang = symbol("!", Vec3::ZERO, q, 1.0, c);
        assert_eq!(bang.counts(), (4, 0, 0), "{bang:?}");
        let warn = symbol("⚠", Vec3::ZERO, q, 1.0, c);
        assert_eq!(warn.counts().0, 5, "a triangle and a small bang");
        let bolt = symbol("↯", Vec3::ZERO, q, 1.0, c);
        assert_eq!(bolt.counts(), (2, 0, 0));
        let word = symbol("RUN", Vec3::ZERO, q, 1.0, c);
        assert_eq!(word.counts(), (0, 0, 1), "plain ASCII goes to the stroke font in one piece");
        let mixed = symbol("⚠x", Vec3::ZERO, q, 1.0, c);
        assert_eq!(mixed.counts().2, 1, "an ASCII letter next to a glyph is text");
        let unknown = symbol("☃", Vec3::ZERO, q, 1.0, c);
        assert_eq!(unknown.counts(), (4, 0, 0), "an unknown symbol is a bang");
        assert!(symbol("", Vec3::ZERO, q, 1.0, c).is_empty());
    }

    #[test]
    fn glyphs_face_the_camera_and_center_on_the_anchor() {
        let at = Vec3::new(3.0, 2.0, -4.0);
        // A camera looking along +X (rotated a quarter turn): the glyph's plane is the YZ plane.
        let turned = Quat::from_rotation_y(-std::f32::consts::FRAC_PI_2);
        let list = symbol("!", at, turned, 1.0, Color::WHITE);
        for p in &list.0 {
            let Prim::Strip { points, .. } = p else { panic!() };
            for q in points {
                assert!((q.x - at.x).abs() < 1e-4, "flat in the view plane: {q}");
                assert!((q.y - at.y).abs() <= 0.5 + 1e-4, "centered vertically: {q}");
            }
        }
        let wide = symbol("!!", at, Quat::IDENTITY, 1.0, Color::WHITE);
        let xs: Vec<f32> = wide
            .0
            .iter()
            .flat_map(|p| match p {
                Prim::Strip { points, .. } => points.iter().map(|q| q.x).collect::<Vec<_>>(),
                _ => vec![],
            })
            .collect();
        let (lo, hi) = xs.iter().fold((f32::MAX, f32::MIN), |(l, h), x| (l.min(*x), h.max(*x)));
        assert!(((lo + hi) * 0.5 - at.x).abs() < 0.05, "a pair is centered: {lo}..{hi}");
        assert!(hi - lo > 0.5, "and spread out");
    }

    #[test]
    fn ascii_symbols_for_plain_fonts() {
        assert_eq!(ascii_symbol("⚠"), "!");
        assert_eq!(ascii_symbol("↯"), "/");
        assert_eq!(ascii_symbol("!!"), "!!");
        assert_eq!(ascii_symbol("☃"), "!");
        assert_eq!(ascii_symbol("RUN ✖"), "RUN x");
    }

    #[test]
    fn wireframes_have_the_volume_of_their_shape() {
        let bounds = |lines: &[Vec<Vec3>]| {
            lines.iter().flatten().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(*p), hi.max(*p)))
        };
        let (lo, hi) = bounds(&shape_wireframe(&Shape::cuboid(Vec3::new(1.0, 2.0, 3.0)), Vec3::new(10.0, 0.0, 0.0), Quat::IDENTITY));
        assert_eq!((lo, hi), (Vec3::new(9.0, -2.0, -3.0), Vec3::new(11.0, 2.0, 3.0)));
        assert_eq!(shape_wireframe(&Shape::cuboid(Vec3::ONE), Vec3::ZERO, Quat::IDENTITY).len(), 6);
        let (lo, hi) = bounds(&shape_wireframe(&Shape::Sphere { radius: 2.0 }, Vec3::ZERO, Quat::IDENTITY));
        assert!((hi - Vec3::splat(2.0)).length() < 1e-3 && (lo + Vec3::splat(2.0)).length() < 1e-3);
        let (lo, hi) = bounds(&shape_wireframe(&Shape::Capsule { half_height: 0.5, radius: 0.4 }, Vec3::ZERO, Quat::IDENTITY));
        assert!((hi.y - 0.9).abs() < 1e-3 && (lo.y + 0.9).abs() < 1e-3 && (hi.x - 0.4).abs() < 1e-3, "{lo} {hi}");
        let (lo, hi) = bounds(&shape_wireframe(&Shape::Cylinder { half_height: 1.0, radius: 0.5 }, Vec3::ZERO, Quat::IDENTITY));
        assert!((hi.y - 1.0).abs() < 1e-3 && (lo.y + 1.0).abs() < 1e-3 && (hi.z - 0.5).abs() < 1e-3);
        // Turned a quarter about Z, a tall cylinder lies along X.
        let (lo, hi) = bounds(&shape_wireframe(
            &Shape::Cylinder { half_height: 1.0, radius: 0.5 },
            Vec3::ZERO,
            Quat::from_rotation_z(std::f32::consts::FRAC_PI_2),
        ));
        assert!((hi.x - 1.0).abs() < 1e-3 && (lo.x + 1.0).abs() < 1e-3 && (hi.y - 0.5).abs() < 1e-3, "{lo} {hi}");
    }

    #[test]
    fn hitbox_wireframes_cover_what_the_hit_pass_overlaps() {
        // A box hit shape: same as its physics cuboid.
        let ws = WorldShape::new(HitShape::Box { width: 2.0, height: 1.0, depth: 4.0 }, Local::default(), Vec3::new(0.0, 1.0, 0.0), N);
        let lines = hitbox_wireframe(&ws);
        let (lo, hi) = lines.iter().flatten().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        assert!((lo - Vec3::new(-1.0, 0.5, -2.0)).length() < 1e-3 && (hi - Vec3::new(1.0, 1.5, 2.0)).length() < 1e-3, "{lo} {hi}");
        // A cone is its sector, standing `height` tall: nothing behind the apex.
        let cone = at(HitShape::Cone { range: 3.0, angle: 90.0, height: 2.0 });
        let lines = hitbox_wireframe(&cone);
        let (lo, hi) = lines.iter().flatten().fold((Vec3::MAX, Vec3::MIN), |(lo, hi), p| (lo.min(*p), hi.max(*p)));
        assert!(hi.z <= 1e-4 && (lo.z + 3.0).abs() < 0.01, "forward is -Z: {lo} {hi}");
        assert!((hi.y - 2.0).abs() < 1e-3 && lo.y.abs() < 1e-4);
        assert!((hi.x - 3.0 * 45f32.to_radians().sin()).abs() < 1e-3);
    }

    #[test]
    fn guard_arcs_span_their_angle() {
        let arc = guard_arc(Vec3::ZERO, N, 120.0, 1.3, 0.9);
        assert_eq!(arc.first(), arc.last(), "joined to the center at both ends");
        let tip = arc[1];
        let angle = (tip - Vec3::Y * 0.9).angle_between(Vec3::NEG_Z).to_degrees();
        assert!((angle - 60.0).abs() < 1e-2, "{angle}");
        let full = guard_arc(Vec3::ZERO, N, 360.0, 1.3, 0.9);
        assert!(full.len() > 40 && full.first() != full.last());
    }

    #[test]
    fn edge_markers_sit_on_the_screen_edge_toward_the_threat() {
        let screen = Vec2::new(1600.0, 900.0);
        let m = edge_marker(0.0, screen, 40.0);
        assert!((m.position - Vec2::new(800.0, 40.0)).length() < 1e-3, "ahead is the top: {:?}", m.position);
        let m = edge_marker(90.0, screen, 40.0);
        assert!((m.position - Vec2::new(1560.0, 450.0)).length() < 1e-2, "right: {:?}", m.position);
        let m = edge_marker(-90.0, screen, 40.0);
        assert!((m.position - Vec2::new(40.0, 450.0)).length() < 1e-2, "left: {:?}", m.position);
        let m = edge_marker(180.0, screen, 40.0);
        assert!((m.position - Vec2::new(800.0, 860.0)).length() < 1e-2, "behind is the bottom: {:?}", m.position);
        let m = edge_marker(-180.0, screen, 40.0);
        assert!((m.position - Vec2::new(800.0, 860.0)).length() < 1e-2);
        // A diagonal lands on the nearer edge: a wide screen is hit by its top first at 45 degrees.
        let m = edge_marker(45.0, screen, 40.0);
        assert!((m.position.y - 40.0).abs() < 1e-2 && m.position.x > 800.0 && m.position.x < 1560.0, "{:?}", m.position);
        assert!((m.angle_deg() - 45.0).abs() < 1e-3);
        for a in (-180..=180).step_by(15) {
            let m = edge_marker(a as f32, screen, 40.0);
            assert!(m.position.x >= 39.9 && m.position.x <= 1560.1 && m.position.y >= 39.9 && m.position.y <= 860.1, "{a}: {:?}", m.position);
        }
        // Tiny viewports do not invert.
        let m = edge_marker(30.0, Vec2::new(50.0, 50.0), 40.0);
        assert!(m.position.is_finite());
    }

    #[test]
    fn counts_read_like_a_report() {
        let c = OverlayCounts { hitboxes: 1, hurtboxes: 4, guards: 0, iframes: 2, zones: 1 };
        assert_eq!(c.describe(), "1 hitbox, 4 hurtboxes, 0 guards, 2 i-frame windows, 1 zone");
    }
}
