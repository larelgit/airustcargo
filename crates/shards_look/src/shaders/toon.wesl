// A look's `Toon` shading (shards_look): Bevy's StandardMaterial for everything but the light.
// The sun (every directional light, with its shadows) lights the surface in flat bands; ambient
// light, point lights and spot lights light it evenly; nothing shines. Only the forward pass
// draws with it: the look sets its material's render method to `Forward` (deferred light is
// Bevy's own), and the prepass and the deferred pass take the StandardMaterial's shaders.

import bevy_pbr::render::{
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::alpha_discard,
    forward_io::{VertexOutput, FragmentOutput},
    pbr_functions::main_pass_post_lighting_processing,
    pbr_types::PbrInput,
    mesh_view_bindings as view_bindings,
    mesh_view_types,
    mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT,
    pbr_lighting as lighting,
    pbr_lighting::LAYER_BASE,
    pbr_ambient::ambient_light,
    clustered_forward as clustering,
    shadows,
};

@if(VISIBILITY_RANGE_DITHER)
import bevy_pbr::render::pbr_functions::visibility_range_dither;

struct Toon {
    bands: u32,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(100)
var<uniform> toon: Toon;

const FRAC_1_PI: f32 = 0.3183098862;

/// `t` (0 unlit, 1 fully lit) in `n` flat steps: 0, 1/(n-1), ..., 1. Any light at all is the
/// first step up, so light and shadow part where the light ends. Each step's edge is smoothed
/// over what `t` changes in one pixel, so it is not jagged.
fn band(t: f32, n: u32) -> f32 {
    let steps = f32(max(n, 2u) - 1u);
    let x = clamp(t, 0.0, 1.0) * steps;
    let w = max(fwidth(x), 1e-4);
    return (floor(x) + smoothstep(0.0, w, fract(x))) / steps;
}

fn toon_light(in: PbrInput) -> vec4<f32> {
    let base = in.material.base_color;
    let N = in.N;
    let V = in.V;
    let NdotV = max(dot(N, V), 0.0001);
    let view_z = dot(vec4<f32>(
        view_bindings::view.view_from_world[0].z,
        view_bindings::view.view_from_world[1].z,
        view_bindings::view.view_from_world[2].z,
        view_bindings::view.view_from_world[3].z
    ), in.world_position);
    let receives = (in.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u;

    // The sun, in bands: as bright as a matte surface where it is fully lit.
    var sun = vec3<f32>(0.0);
    let n_directional = view_bindings::lights.n_directional_lights;
    for (var i: u32 = 0u; i < n_directional; i = i + 1u) {
        let light = &view_bindings::lights.directional_lights[i];
        var shadow = 1.0;
        if receives && ((*light).flags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u {
            shadow = shadows::fetch_directional_shadow(i, in.world_position, in.world_normal, view_z, in.frag_coord.xy)
                * in.directional_shadow_factor;
        }
        let t = max(dot(N, (*light).direction_to_light), 0.0) * shadow;
        sun += (*light).color.rgb * band(t, toon.bands);
    }

    // Lamps and spots, evenly: Bevy's diffuse light of each, without shine.
    var lighting_input: lighting::LightingInput;
    lighting_input.layers[LAYER_BASE].NdotV = NdotV;
    lighting_input.layers[LAYER_BASE].N = N;
    lighting_input.layers[LAYER_BASE].R = reflect(-V, N);
    lighting_input.layers[LAYER_BASE].perceptual_roughness = 1.0;
    lighting_input.layers[LAYER_BASE].roughness = 1.0;
    lighting_input.P = in.world_position.xyz;
    lighting_input.V = V;
    lighting_input.diffuse_color = vec3<f32>(1.0);
    lighting_input.metallic = 0.0;
    lighting_input.F0_dielectric = vec3<f32>(0.0);
    lighting_input.F0_metallic = vec3<f32>(0.0);
    lighting_input.F_ab = lighting::F_AB(1.0, NdotV);
    var lamps = vec3<f32>(0.0);
    let cluster_index = clustering::view_fragment_cluster_index(in.frag_coord.xy, view_z, in.is_orthographic);
    let ranges = clustering::unpack_clusterable_object_index_ranges(cluster_index);
    for (var i: u32 = ranges.first_point_light_index_offset; i < ranges.first_spot_light_index_offset; i = i + 1u) {
        let light_id = clustering::get_clusterable_object_id(i);
        var shadow = 1.0;
        if receives && (view_bindings::clustered_lights.data[light_id].flags & mesh_view_types::POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u {
            shadow = shadows::fetch_point_shadow(light_id, in.world_position, in.world_normal, in.frag_coord.xy);
        }
        lamps += lighting::point_light(light_id, &lighting_input, true, true) * shadow;
    }
    for (var i: u32 = ranges.first_spot_light_index_offset; i < ranges.first_rect_light_index_offset; i = i + 1u) {
        let light_id = clustering::get_clusterable_object_id(i);
        var shadow = 1.0;
        if receives && (view_bindings::clustered_lights.data[light_id].flags & mesh_view_types::POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u {
            shadow = shadows::fetch_spot_shadow(
                light_id,
                in.world_position,
                in.world_normal,
                view_bindings::clustered_lights.data[light_id].shadow_map_near_z,
                in.frag_coord.xy,
            );
        }
        lamps += lighting::spot_light(light_id, &lighting_input, true) * shadow;
    }

    // The light all around, evenly.
    let ambient = ambient_light(in.world_position, N, V, NdotV, vec3<f32>(1.0), vec3<f32>(0.0), 1.0, in.diffuse_occlusion);

    // Its `a` is the material's `emissive_exposure_weight` (0 for a look's glow): how much the
    // camera's exposure scales it.
    let emissive = in.material.emissive;
    let glow = emissive.rgb * base.a * mix(1.0, view_bindings::view.exposure, emissive.a);
    let lit = view_bindings::view.exposure * (sun * FRAC_1_PI + lamps + ambient);
    return vec4<f32>(base.rgb * lit + glow, base.a);
}

@fragment
fn fragment(
    in: VertexOutput,
    @builtin(front_facing) is_front: bool,
) -> FragmentOutput {
    // Between two levels of detail (a `VisibilityRange` with margins), dissolved like Bevy's own.
@if(VISIBILITY_RANGE_DITHER)
    visibility_range_dither(in.position, in.visibility_range_dither);

    var pbr_input = pbr_input_from_standard_material(in, is_front);
    pbr_input.material.base_color = alpha_discard(
        pbr_input.material.flags,
        pbr_input.material.alpha_cutoff,
        pbr_input.material.base_color
    );

    var out: FragmentOutput;
    out.color = main_pass_post_lighting_processing(pbr_input, toon_light(pbr_input));
    return out;
}
