// A telegraph's decal (shards_fx): Bevy's forward decal projects the quad onto what lies under it
// (the ground, the feet of who stands there), and this fills the zone's footprint in the quad: a
// disc, a cone's sector, a box's or a line's rectangle, a capsule's stadium. The zone is faint,
// with stripes that stay put in the world and a glow inside its edge; a copy of it grows from its
// middle as the hit nears (`fill`, 0 to 1) with a bright rim, and covers it, flickering, while the
// hit lands. The colour is the gizmo telegraph's: warm for an enemy, cool for an ally.

import bevy_pbr::render::forward_io::VertexOutput;
import bevy_pbr::decal::forward::get_forward_decal_info;

struct Zone {
    color: vec4<f32>,
    // A disc's or a stadium's radius (x); a sector's range (x) and half angle (y).
    shape: vec4<f32>,
    // A disc's or a rectangle's centre, a sector's apex (xy); a stadium's other end, a
    // rectangle's half size (zw).
    points: vec4<f32>,
    // The quad's half size (xy), where the fill grows from (zw).
    frame: vec4<f32>,
    // The fill (x), the flicker (y), the shape's kind (z), the stripes' spacing (w).
    state: vec4<f32>,
    // The quad's centre on the ground, world x and z (xy); its facing (zw).
    world: vec4<f32>,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0)
var<uniform> zone: Zone;

const PI: f32 = 3.14159265;

/// From an apex along +y, `range` far, `half_angle` each side; every way round from PI.
fn sector(p: vec2<f32>, apex: vec2<f32>, range: f32, half_angle: f32) -> f32 {
    let q0 = p - apex;
    let beyond = length(q0) - range;
    if half_angle >= PI - 0.001 {
        return beyond;
    }
    let q = vec2<f32>(abs(q0.x), q0.y);
    let edge = vec2<f32>(sin(half_angle), cos(half_angle));
    let along = clamp(dot(q, edge), 0.0, range);
    let to_edge = length(q - edge * along);
    let within = q.x * edge.y - q.y * edge.x <= 0.0;
    return select(max(to_edge, beyond), max(beyond, -to_edge), within);
}

fn rect(p: vec2<f32>, center: vec2<f32>, half_size: vec2<f32>) -> f32 {
    let d = abs(p - center) - half_size;
    return length(max(d, vec2<f32>(0.0))) + min(max(d.x, d.y), 0.0);
}

fn stadium(p: vec2<f32>, a: vec2<f32>, b: vec2<f32>, radius: f32) -> f32 {
    let ab = b - a;
    let l = dot(ab, ab);
    var t = 0.0;
    if l > 1e-8 {
        t = clamp(dot(p - a, ab) / l, 0.0, 1.0);
    }
    return length(p - (a + ab * t)) - radius;
}

/// How far `p` (metres in the quad's frame) is outside the zone: below 0 inside.
fn zone_distance(p: vec2<f32>) -> f32 {
    let kind = u32(zone.state.z + 0.5);
    if kind == 0u {
        return length(p - zone.points.xy) - zone.shape.x;
    }
    if kind == 1u {
        return sector(p, zone.points.xy, zone.shape.x, zone.shape.y);
    }
    if kind == 2u {
        return rect(p, zone.points.xy, zone.points.zw);
    }
    return stadium(p, zone.points.xy, zone.points.zw, zone.shape.x);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let decal = get_forward_decal_info(in);
    let size = zone.frame.xy * 2.0;
    // The quad's frame: x to the right of the zone's facing, y along it (the quad's v runs back).
    let p = vec2<f32>((decal.uv.x - 0.5) * size.x, (0.5 - decal.uv.y) * size.y);
    let fill = zone.state.x;
    let flicker = zone.state.y;
    let spacing = zone.state.w;

    let d = zone_distance(p);
    let aa = max(fwidth(d), 0.005);
    // The fill: the zone scaled about where it grows from, reaching its edge as the hit lands.
    let k = max(fill, 0.04);
    let origin = zone.frame.zw;
    let df = zone_distance(origin + (p - origin) / k) * k;
    let aaf = max(fwidth(df), 0.005);
    // Stripes at 45°, anchored to the world: a zone that moves does not drag them along.
    let facing = zone.world.zw;
    let right = vec2<f32>(-facing.y, facing.x);
    let w = zone.world.xy + right * p.x + facing * p.y;
    let s = dot(w, vec2<f32>(0.70710678, 0.70710678)) / spacing;
    let line = abs(fract(s + 0.5) - 0.5) * spacing;
    let aas = max(fwidth(s) * spacing, 0.005);
    let stripe = 1.0 - smoothstep(0.035, 0.035 + aas, line);

    let inside = 1.0 - smoothstep(-aa, aa, d);
    let filled = 1.0 - smoothstep(-aaf, aaf, df);
    let rim = (1.0 - smoothstep(0.04, 0.04 + aaf, abs(df))) * step(fill, 0.999);
    let edge = 1.0 - smoothstep(0.0, 0.25, -d);

    var alpha = (0.10 + 0.15 * fill) * flicker;
    alpha = max(alpha, stripe * (0.16 + 0.30 * fill) * flicker);
    alpha = max(alpha, edge * (0.30 + 0.35 * fill) * flicker);
    var fill_alpha = 0.30 + 0.30 * fill;
    if fill >= 0.999 {
        fill_alpha = 0.62 * flicker;
    }
    alpha = max(alpha, filled * fill_alpha);
    alpha = max(alpha, rim * (0.65 + 0.35 * fill));
    alpha = alpha * inside * decal.alpha;
    if alpha <= 0.002 {
        discard;
    }
    return vec4<f32>(zone.color.rgb, alpha);
}
