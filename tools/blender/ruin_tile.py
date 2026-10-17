"""Builds the `ruin` tile of the sandbox's and the template's regions the way an artist would and
exports it with Blender's own glTF exporter as *glTF Embedded* (one `.gltf` with its buffer
inside as base64): `crates/sandbox/assets/tiles/ruin.gltf`, which the template keeps a copy of.

The tile is a 16 x 16 m scene centered on the origin. Blender is Z-up and -Y is south: the
exporter turns that into the engine's Y-up with -Z north, so an object at Blender (x, y, z) stands
at (x, z, -y) in the game. Every object that matters has *Custom Properties*: `collider = auto` on
the meshes, `marker`/`name` on the camp and the landmark, `socket = grass` on the empties at the
edges; the scene has `tile` and `size`.

Run it headless, never with a window:

    blender -b --factory-startup --python-exit-code 1 -P tools/blender/ruin_tile.py -- crates/sandbox/assets/tiles

Then copy the file to `template/assets/tiles/` and bake the tile's navigation in both games
(`cargo sandbox --bake-nav ruin`; in `template/`, `cargo run -- --bake-nav ruin`). The sandbox's
test `the_committed_ruin_is_the_tile_the_spec_describes` checks the export against the tile's
spec, and that the two copies are the same file.

*glTF Embedded* sits behind the exporter's preference *Allow glTF Embedded format*; the script
turns it on. The export ticks *Include -> Custom Properties* and *Data -> Mesh -> Apply
Modifiers*, as the README tells artists to; every other option keeps the exporter's default.
"""

import math
import os
import sys

import bmesh
import bpy


def out_dir():
    args = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    if len(args) != 1:
        raise SystemExit("usage: blender -b --factory-startup --python-exit-code 1 -P ruin_tile.py -- <output folder>")
    path = os.path.abspath(args[0])
    os.makedirs(path, exist_ok=True)
    return path


def unit_cube(name):
    """A 1 x 1 x 1 cube around its origin: *Add -> Mesh -> Cube* with *Size* 1."""
    mesh = bpy.data.meshes.new(name)
    bm = bmesh.new()
    bmesh.ops.create_cube(bm, size=1.0)
    bm.to_mesh(mesh)
    bm.free()
    return mesh


def add(name, data=None, parent=None, location=(0.0, 0.0, 0.0), turn=0.0, scale=(1.0, 1.0, 1.0), props=None):
    """An object in the scene; `data=None` makes an empty. `turn` is degrees about Z. A child's
    location is in its parent's space (*Ctrl+P -> Object (Without Inverse)*). `props` are its *Custom
    Properties*."""
    obj = bpy.data.objects.new(name, data)
    if data is None:
        obj.empty_display_type = "PLAIN_AXES"
    if parent is not None:
        obj.parent = parent
        obj.matrix_parent_inverse.identity()
    obj.location = location
    obj.rotation_euler = (0.0, 0.0, math.radians(turn))
    obj.scale = scale
    for key, value in (props or {}).items():
        obj[key] = value
    bpy.context.scene.collection.objects.link(obj)
    return obj


def build():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.name = "Ruin"
    scene["tile"] = "ruin"
    scene["size"] = 16
    # One cube for every block, as linked duplicates (Alt+D) share it.
    cube = unit_cube("Cube")

    add("Ground", cube, location=(0.0, 0.0, -0.5), scale=(16.0, 16.0, 1.0), props={"collider": "auto"})
    # Three levels of hierarchy, as an artist groups things: Ruin > Walls > Wall.North.
    ruin = add("Ruin")
    walls = add("Walls", parent=ruin, location=(-1.0, 0.0, 0.0))
    add("Wall.North", cube, parent=walls, location=(-2.0, 5.0, 1.5), scale=(6.0, 0.6, 3.0), props={"collider": "auto"})
    add("Wall.West", cube, parent=walls, location=(-4.0, 1.0, 1.5), scale=(0.6, 6.0, 3.0), props={"collider": "auto"})
    add("Rubble", cube, parent=ruin, location=(3.0, 4.0, 0.5), turn=30.0, scale=(2.0, 1.2, 1.0), props={"collider": "auto"})
    add("Column", cube, parent=ruin, location=(4.0, -4.0, 2.0), scale=(1.0, 1.0, 4.0), props={"collider": "auto"})
    add("Camp", location=(-2.0, -2.0, 0.0), props={"marker": "camp", "name": "ruin_camp"})
    add("Arch", location=(1.0, -1.0, 0.0), props={"marker": "landmark", "name": "Ruin"})
    for name, at in [("Edge.N", (0.0, 8.0, 0.0)), ("Edge.E", (8.0, 0.0, 0.0)), ("Edge.S", (0.0, -8.0, 0.0)), ("Edge.W", (-8.0, 0.0, 0.0))]:
        add(name, location=at, props={"socket": "grass"})
    bpy.context.view_layer.update()


def allow_embedded():
    """*Edit -> Preferences -> Add-ons -> glTF 2.0 -> Allow glTF Embedded format*."""
    for module in ("io_scene_gltf2", "bl_ext.blender_org.io_scene_gltf2"):
        addon = bpy.context.preferences.addons.get(module)
        if addon is not None and hasattr(addon.preferences, "allow_embedded_format"):
            addon.preferences.allow_embedded_format = True
            return
    raise RuntimeError("the glTF exporter has no `allow_embedded_format` preference in this Blender")


def export(path):
    result = bpy.ops.export_scene.gltf(filepath=path, export_format="GLTF_EMBEDDED", export_extras=True, export_apply=True)
    if result != {"FINISHED"}:
        raise RuntimeError(f"export of {path} failed: {result}")
    print(f"wrote {path}")


def main():
    folder = out_dir()
    build()
    allow_embedded()
    export(os.path.join(folder, "ruin.gltf"))


main()
