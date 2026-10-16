// A look's outline (shards_look): the mesh again, its back faces only, every vertex pushed out
// along its normal by the outline's width in pixels, so the hull shows as a line of that width
// around the silhouette however far the mesh is. Skinned meshes bend it with them.

import bevy_pbr::render::{
    mesh_functions,
    skinning,
    forward_io::{Vertex, decompress_vertex},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::view,
};

struct Outline {
    color: vec4<f32>,
    width: f32,
}

@group(constants::MATERIAL_BIND_GROUP) @binding(0)
var<uniform> outline: Outline;

struct OutlineVertex {
    @builtin(position) position: vec4<f32>,
}

@vertex
fn vertex(vertex_in: Vertex) -> OutlineVertex {
    let vertex = decompress_vertex(vertex_in, vertex_in.instance_index);
@if(SKINNED)
    let world_from_local = skinning::skin_model(vertex.joint_indices, vertex.joint_weights, vertex_in.instance_index);
@else
    let world_from_local = mesh_functions::get_world_from_local(vertex_in.instance_index);
@if(SKINNED)
    let normal = skinning::skin_normals(world_from_local, vertex.normal);
@else
    let normal = mesh_functions::mesh_normal_local_to_world(vertex.normal, vertex_in.instance_index);

    let world = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
    var clip = position_world_to_clip(world.xyz);
    // The normal on screen, in pixels; pushed that way by `width` pixels (clip space is two
    // units across the viewport, scaled by w).
    let size = view.viewport.zw;
    let along = (view.clip_from_world * vec4<f32>(normal, 0.0)).xy * size;
    let length_px = length(along);
    if length_px > 1e-6 {
        clip = vec4<f32>(clip.xy + along / length_px * (2.0 * outline.width / size) * clip.w, clip.zw);
    }
    var out: OutlineVertex;
    out.position = clip;
    return out;
}

@fragment
fn fragment() -> @location(0) vec4<f32> {
    return outline.color;
}
