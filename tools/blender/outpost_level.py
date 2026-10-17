"""Builds the `outpost` test level the way an artist would and exports it with Blender's
own glTF exporter: the fixtures of `crates/shards_level/tests/level/blender/`.

The level has what a real level has: meshes with non-applied transforms and a modifier,
parented objects (Ctrl+P, with a parent inverse), empties as markers, a collection instanced
twice (its source collection in a scene of its own), a sun and a camera, names with spaces and non-ASCII
characters, and custom properties of every kind the property panel makes (text, numbers,
booleans, float arrays, data-block pointers, and Python values: dicts and lists of text).

Run it headless, never with a window:

    blender -b --factory-startup --python-exit-code 1 -P tools/blender/outpost_level.py -- crates/shards_level/tests/level/blender

It writes into the folder given after `--`:

- `outpost.glb`: the default export format (glTF Binary);
- `outpost_lights.gltf` + `outpost_lights.bin`: glTF Separate, the same scene with
  *Include -> Punctual Lights* ticked (the exporter marks `KHR_lights_punctual` required);
- `outpost_embedded.gltf`: glTF Embedded, one `.gltf` with its buffer inside as base64 (the
  exporter offers it once *Allow glTF Embedded format* is ticked in its preferences, which the
  script does);
- `outpost_draco.glb`: the same scene with Draco mesh compression;
- `outpost_meshopt.glb` and `outpost_meshopt_khr.glb`: the same scene with *Data -> Meshopt
  Compression*, once with each of its two extensions (`EXT_meshopt_compression`, the
  default, and `KHR_meshopt_compression`);
- `outpost_broken.glb`: the scene with mistakes an artist makes (typos, a navlink to
  nowhere, a property on the mesh data instead of the object), for the error messages.

Every export ticks *Include -> Custom Properties* and *Data -> Mesh -> Apply Modifiers*, as
the README tells artists to; every other option keeps the exporter's default.
"""

import math
import os
import sys

import bmesh
import bpy


def out_dir():
    args = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    if len(args) != 1:
        raise SystemExit("usage: blender -b --factory-startup --python-exit-code 1 -P outpost_level.py -- <output folder>")
    path = os.path.abspath(args[0])
    os.makedirs(path, exist_ok=True)
    return path


# --- building blocks ------------------------------------------------------------------


def box_mesh(name):
    """A 2 x 2 x 2 cube around its origin, like *Add -> Mesh -> Cube*."""
    mesh = bpy.data.meshes.new(name)
    bm = bmesh.new()
    bmesh.ops.create_cube(bm, size=2.0)
    bm.to_mesh(mesh)
    bm.free()
    return mesh


def cylinder_mesh(name, radius=1.0, depth=2.0):
    """A cylinder around its origin along Z, like *Add -> Mesh -> Cylinder*."""
    mesh = bpy.data.meshes.new(name)
    bm = bmesh.new()
    bmesh.ops.create_cone(bm, cap_ends=True, segments=16, radius1=radius, radius2=radius, depth=depth)
    bm.to_mesh(mesh)
    bm.free()
    return mesh


def cone_mesh(name, radius=1.0, depth=2.0):
    """A cone around its origin along Z, like *Add -> Mesh -> Cone*."""
    mesh = bpy.data.meshes.new(name)
    bm = bmesh.new()
    bmesh.ops.create_cone(bm, cap_ends=True, segments=16, radius1=radius, radius2=0.0, depth=depth)
    bm.to_mesh(mesh)
    bm.free()
    return mesh


def sphere_mesh(name, radius=1.0):
    """A UV sphere around its origin, like *Add -> Mesh -> UV Sphere*."""
    mesh = bpy.data.meshes.new(name)
    bm = bmesh.new()
    bmesh.ops.create_uvsphere(bm, u_segments=16, v_segments=8, radius=radius)
    bm.to_mesh(mesh)
    bm.free()
    return mesh


def add(collection, name, data=None, location=(0.0, 0.0, 0.0), rotation=(0.0, 0.0, 0.0), scale=(1.0, 1.0, 1.0)):
    """An object in `collection`; `data=None` makes an empty. Rotation in degrees (XYZ Euler)."""
    obj = bpy.data.objects.new(name, data)
    if data is None:
        obj.empty_display_type = "PLAIN_AXES"
    obj.location = location
    obj.rotation_euler = [math.radians(a) for a in rotation]
    obj.scale = scale
    collection.objects.link(obj)
    return obj


def parent_keep_transform(child, parent):
    """Ctrl+P -> Object: the child stays where it is, through the parent inverse matrix."""
    bpy.context.view_layer.update()
    child.parent = parent
    child.matrix_parent_inverse = parent.matrix_world.inverted()


def parent_without_inverse(child, parent):
    """Ctrl+P -> Object (Without Inverse): the child's location is in the parent's space."""
    child.parent = parent
    child.matrix_parent_inverse.identity()


# --- the scene ------------------------------------------------------------------------


def build():
    bpy.ops.wm.read_factory_settings(use_empty=True)
    scene = bpy.context.scene
    scene.name = "Outpost"
    scene["theme"] = "outpost"
    root = scene.collection

    stone = bpy.data.materials.new("Stone")
    stone.diffuse_color = (0.5, 0.5, 0.45, 1.0)

    # The floor: the default cube scaled flat, the scale left on the object.
    ground = add(root, "Ground", box_mesh("Ground"), location=(0.0, 0.0, -0.25), scale=(12.0, 12.0, 0.25))
    ground.data.materials.append(stone)
    ground["collider"] = "auto"

    # A wall made of one block and an Array modifier (4 blocks along X), scaled thin.
    wall = add(root, "Wall North", box_mesh("Wall"), location=(-3.0, 10.0, 1.5), scale=(1.0, 0.25, 1.5))
    array = wall.modifiers.new("Array", "ARRAY")
    array.count = 4
    array.relative_offset_displace = (1.0, 0.0, 0.0)
    wall["collider"] = "auto"

    # A ruin: a turned, scaled empty with a pillar and a landmark parented to it.
    ruins = add(root, "Ruins", location=(6.0, -6.0, 0.0), rotation=(0.0, 0.0, 45.0), scale=(2.0, 2.0, 2.0))
    pillar = add(root, "Säule", cylinder_mesh("Pillar"), location=(8.0, -6.0, 2.0), scale=(0.5, 0.5, 2.0))
    pillar["collider"] = "cylinder"
    parent_keep_transform(pillar, ruins)
    shrine = add(root, "Shrine", location=(6.0, -8.0, 0.0), rotation=(0.0, 0.0, 30.0))
    shrine["marker"] = "landmark"
    shrine["name"] = "Old Shrine"
    parent_keep_transform(shrine, ruins)

    # Markers.
    start = add(root, "Player Start", location=(0.0, -8.0, 0.0), rotation=(0.0, 0.0, 90.0))
    start["marker"] = "spawn"
    start["name"] = "start"

    markers = add(root, "Markers", location=(-8.0, -8.0, 0.0), rotation=(0.0, 0.0, 90.0))
    camp = add(root, "Костёр", location=(2.0, 0.0, 0.0))
    parent_without_inverse(camp, markers)
    camp["marker"] = "camp"
    camp["name"] = "bandits"

    lever = add(root, "Lever", location=(4.0, 9.0, 1.0))
    lever["marker"] = "interactable"
    lever["name"] = "lever"

    for name, location, socket in [("Edge N", (0.0, 12.0, 0.0), "road"), ("Edge S", (0.0, -12.0, 0.0), "road"), ("Edge E", (12.0, 0.0, 0.0), "river")]:
        edge = add(root, name, location=location)
        edge["socket"] = socket

    top = add(root, "Ladder Top", location=(10.0, 8.0, 3.0))
    bottom = add(root, "Ladder Bottom", location=(10.0, 9.0, 0.0))
    bottom["marker"] = "navlink"
    bottom["name"] = "ladder"
    # The panel's Data-Block property type: the artist picks the object, not its name.
    bottom["to"] = top

    vendor = add(root, "Vendor", location=(-4.0, 8.0, 0.0))
    vendor["marker"] = "custom:vendor"

    # A trigger zone and two volumes.
    goal = add(root, "Goal Zone", box_mesh("Goal"), location=(0.0, 6.0, 1.0), scale=(2.0, 2.0, 1.0))
    goal.display_type = "WIRE"
    goal["marker"] = "trigger"
    goal["name"] = "goal"

    pond = add(root, "Pond", box_mesh("Pond"), location=(-6.0, 4.0, -0.5), scale=(3.0, 2.0, 0.5))
    pond["volume"] = "water"

    vent = add(root, "Vent", cylinder_mesh("Vent"), location=(-9.0, -2.0, 3.0), scale=(1.0, 1.0, 3.0))
    vent["volume"] = "updraft"
    vent["collider"] = "cylinder"
    vent["accel"] = 12
    vent["max_speed"] = 8.0

    # A crate with an explicit collider and components: JSON typed as text, a boolean, and
    # a list of names (the panel's Python type).
    crate = add(root, "Crate.001", box_mesh("Crate"), location=(3.0, -3.0, 0.5), scale=(0.5, 0.5, 0.5))
    crate["collider"] = "box"
    crate["size"] = [2.0, 2.0, 2.0]
    crate["layers"] = ["world", "game3"]
    crate["Health"] = '{"current": 5.0, "max": 5.0}'
    crate["Locked"] = True

    # A prefab with overrides: a dict (the panel's Python type) and a text.
    guard = add(root, "Guard", location=(4.0, 4.0, 0.0), rotation=(0.0, 0.0, 180.0))
    guard["prefab"] = "bandit"
    guard["Health"] = {"current": 10.0}
    guard["Team"] = "outlaws"

    # A collection instanced twice. The source collection lives in a scene of its own: one
    # that is only excluded from the level's view layer is still exported, its objects at
    # their own place (Blender 5.2.2), so they would turn up in the level a second time.
    kit = bpy.data.collections.new("Barrel Kit")
    bpy.data.scenes.new("Kit").collection.children.link(kit)
    barrel = add(kit, "Barrel", cylinder_mesh("Barrel", radius=0.4, depth=1.0), location=(0.0, 0.0, 0.5))
    barrel["collider"] = "cylinder"
    barrel["Loot"] = {"table": "barrel", "rolls": 1, "tags": ["junk", "wood"]}
    for name, location, turn in [("Barrels.A", (5.0, 0.0, 0.0), 0.0), ("Barrels.B", (5.0, 2.0, 0.0), 90.0)]:
        instance = add(root, name, location=location, rotation=(0.0, 0.0, turn))
        instance.instance_type = "COLLECTION"
        instance.instance_collection = kit

    # The other collider shapes, sized by their meshes.
    boulder = add(root, "Boulder", sphere_mesh("Boulder"), location=(-2.0, -10.0, 1.0))
    boulder["collider"] = "sphere"
    totem = add(root, "Totem Pole", cylinder_mesh("Totem", radius=0.3, depth=3.0), location=(8.0, 4.0, 1.5))
    totem["collider"] = "capsule"
    spire = add(root, "Spire", cone_mesh("Spire"), location=(-10.0, 10.0, 1.0))
    spire["collider"] = "cone"

    # A rock, visual only (the broken variant puts a property on its mesh).
    add(root, "Rock", box_mesh("Rock"), location=(-10.0, -10.0, 0.5), scale=(1.0, 1.5, 0.5))

    # Visual only.
    post = add(root, "Lamp Post", cylinder_mesh("Post", radius=0.1, depth=3.0), location=(2.0, 2.0, 1.5))
    post.data.materials.append(stone)

    # Not exported with the default options.
    sun = add(root, "Sun", bpy.data.lights.new("Sun", "SUN"), location=(0.0, 0.0, 20.0))
    sun.data.energy = 3.0
    add(root, "Camera", bpy.data.cameras.new("Camera"), location=(0.0, -20.0, 12.0), rotation=(60.0, 0.0, 0.0))

    bpy.context.view_layer.update()


def break_it():
    """Mistakes an artist makes: typos in a component, a collider and a navlink end, and a
    property put on the mesh data (*Object Data Properties*) instead of the object."""
    objects = bpy.data.objects
    del objects["Crate.001"]["Health"]
    objects["Crate.001"]["Helth"] = '{"current": 5.0, "max": 5.0}'
    objects["Barrel"]["Lot"] = objects["Barrel"]["Loot"].to_dict()
    del objects["Barrel"]["Loot"]
    objects["Säule"]["collider"] = "cylindre"
    objects["Ladder Bottom"]["to"] = "Ladder Tpo"
    objects["Rock"].data["collider"] = "auto"


def allow_embedded():
    """*Edit -> Preferences -> Add-ons -> glTF 2.0 -> Allow glTF Embedded format*."""
    for module in ("io_scene_gltf2", "bl_ext.blender_org.io_scene_gltf2"):
        addon = bpy.context.preferences.addons.get(module)
        if addon is not None and hasattr(addon.preferences, "allow_embedded_format"):
            addon.preferences.allow_embedded_format = True
            return
    raise RuntimeError("the glTF exporter has no `allow_embedded_format` preference in this Blender")


def export(path, fmt, lights=False, draco=False, meshopt=None):
    result = bpy.ops.export_scene.gltf(
        filepath=path,
        export_format=fmt,
        export_extras=True,
        export_apply=True,
        export_lights=lights,
        export_draco_mesh_compression_enable=draco,
        export_meshopt_compression_enable=meshopt is not None,
        export_meshopt_extension=meshopt or "EXT_meshopt_compression",
    )
    if result != {"FINISHED"}:
        raise RuntimeError(f"export of {path} failed: {result}")
    print(f"wrote {path}")


def main():
    folder = out_dir()
    build()
    export(os.path.join(folder, "outpost.glb"), "GLB")
    export(os.path.join(folder, "outpost_lights.gltf"), "GLTF_SEPARATE", lights=True)
    allow_embedded()
    export(os.path.join(folder, "outpost_embedded.gltf"), "GLTF_EMBEDDED")
    export(os.path.join(folder, "outpost_draco.glb"), "GLB", draco=True)
    export(os.path.join(folder, "outpost_meshopt.glb"), "GLB", meshopt="EXT_meshopt_compression")
    export(os.path.join(folder, "outpost_meshopt_khr.glb"), "GLB", meshopt="KHR_meshopt_compression")
    break_it()
    export(os.path.join(folder, "outpost_broken.glb"), "GLB")


main()
