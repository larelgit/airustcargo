// A particle (shards_fx): its colour is its `MeshTag` (sRGB bytes, alpha last), so every particle
// of an effect shares one material and they draw in batches. A quad is a soft round spot; a cube
// or a shard is shaded by a light from above, or lit by itself when it glows. A glowing effect is
// added to what is behind it (`AlphaMode::Add`, drawn premultiplied with alpha 0).

import bevy_pbr::render::{
    forward_io::VertexOutput,
    mesh_functions::get_tag,
};

struct Particle {
    kind: u32,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0)
var<uniform> particle: Particle;

fn srgb_to_linear(c: vec3<f32>) -> vec3<f32> {
    let low = c / 12.92;
    let high = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(high, low, c <= vec3<f32>(0.04045));
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let srgba = unpack4x8unorm(get_tag(in.instance_index));
    var rgb = srgb_to_linear(srgba.rgb);
    var alpha = srgba.a;
    if particle.kind == 0u {
        // Whole in the middle, nothing at the quad's edge: a round spot, not a square.
        let r = length(in.uv - vec2<f32>(0.5)) * 2.0;
        alpha = alpha * (1.0 - smoothstep(0.25, 1.0, r));
    } else if particle.kind == 1u {
        let n = normalize(in.world_normal);
        let light = 0.45 + 0.55 * max(dot(n, normalize(vec3<f32>(0.35, 1.0, 0.25))), 0.0);
        rgb = rgb * light;
    }
    if alpha <= 0.003 {
        discard;
    }
    var out = vec4<f32>(rgb, alpha);
@if(BLEND_PREMULTIPLIED_ALPHA) {
    out = vec4<f32>(rgb * alpha, 0.0);
}
    return out;
}
