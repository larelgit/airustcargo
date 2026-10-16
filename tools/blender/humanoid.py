"""Builds the humanoid of the sandbox and the template the way an animator would, and exports
it with Blender's own glTF exporter: a low-poly body skinned to an armature of sixteen bones,
and its clips.

The body is a mannequin of tubes and boxes (a sword in the right hand, a buckler on the left
forearm), each part skinned to its bone, the tubes blended across elbows, knees and the waist so
they bend. Vertex colours shade its parts in greys: a game's look gives the colour and they
darken it (trousers, boots, the belt). The rig: `root` on the ground, `hips` under it, `spine`,
`head`, and arms (`upper_arm`, `forearm`, `hand`) and legs (`thigh`, `shin`, `foot`) with `.L`
and `.R`. It faces Blender's -Y, so glTF's +Z: the front of a glTF model, as the format says.

Clips: `idle`, `walk` and `run` loops, the three swings of the light combo (`light_1`..`light_3`),
`heavy`, `block`, the dodge `roll`, `hit` and `death`. Attacks and the roll move `root` forward,
as an animator authors a lunge: that is the root motion `--bake-motion` reads into the actions'
displacement curves. An attack's clip is timed to its action: the table below has the phases of
each game's actions, and every such clip carries them (the action's custom property `phases`,
exported as the animation's extras) so the bake can tell when the two drift apart.

Run it headless, never with a window, once per game:

    blender -b --factory-startup --python-exit-code 1 -P tools/blender/humanoid.py -- sandbox crates/sandbox/assets/models
    blender -b --factory-startup --python-exit-code 1 -P tools/blender/humanoid.py -- template template/assets/models

It writes into the folder given after the game:

- `humanoid.glb`: the skinned body, the armature and every clip;
- `humanoid_gestures.glb` (sandbox only): the armature alone, with three more clips (`bow`,
  `taunt`, `drink`). Clips of another file for the same skeleton: same node names, so they play
  on the body of `humanoid.glb`. `drink` moves only the arms, the head and the spine: the
  sandbox's animset plays it on the layer `upper` while the legs keep walking.

Every export ticks *Include -> Custom Properties* (the phases) and exports the active colour
attribute; the rest keeps the exporter's defaults (animations sampled at every frame, one glTF
animation per action). The export is byte-for-byte reproducible: running the script twice gives
the same files.
"""

import math
import os
import sys

import bmesh
import bpy
from mathutils import Euler, Quaternion, Vector

# --- what each game's actions are --------------------------------------------------------

# The phases of the actions a clip is made for, in seconds, as each game's `*.action.ron` has
# them: the swing lands in the action's active phase, the root moves where the action's
# displacement used to. `lunge` is (from, to, meters forward). Clips no action is baked from
# (loops, hit, death) are the same in both games.
SANDBOX = {
    "fps": 60,
    "light_1": {"phases": [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)], "lunge": (0.2, 0.3, 0.3)},
    "light_2": {"phases": [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)], "lunge": (0.2, 0.3, 0.3)},
    "light_3": {"phases": [("windup", 0.25), ("active", 0.1), ("recovery", 0.45)], "lunge": (0.25, 0.35, 0.4)},
    "heavy": {"phases": [("windup", 0.55), ("active", 0.15), ("recovery", 0.6)], "lunge": (0.5, 0.7, 0.6)},
    "roll": {"phases": [("roll", 0.4), ("recover", 0.1)], "lunge": (0.0, 0.35, 4.0)},
    "block": {"phases": [("raise", 0.15), ("hold", 0.3), ("lower", 0.1)]},
    "drink": {"phases": [("drink", 1.0), ("recover", 0.3)]},
    "gestures": True,
}

# The template's hero has a quicker first swing and a slower second one, and no lunge on it.
# Its swing's 0.18 s is no whole frame at 60 fps, its roll's 0.35 s none at 50: its clips are
# sampled at 100. No action of the template uses `light_3`, `heavy` or `block`; they keep
# the sandbox's phases, moved to whole frames.
TEMPLATE = dict(
    SANDBOX,
    fps=100,
    light_1={"phases": [("windup", 0.18), ("active", 0.1), ("recovery", 0.3)], "lunge": (0.18, 0.3, 0.4)},
    light_2={"phases": [("windup", 0.2), ("active", 0.1), ("recovery", 0.4)], "lunge": (0.2, 0.3, 0.0)},
    light_3={"phases": [("windup", 0.24), ("active", 0.1), ("recovery", 0.46)], "lunge": (0.24, 0.34, 0.4)},
    heavy={"phases": [("windup", 0.56), ("active", 0.14), ("recovery", 0.6)], "lunge": (0.5, 0.7, 0.6)},
    block={"phases": [("raise", 0.14), ("hold", 0.3), ("lower", 0.1)]},
    gestures=False,
)

GAMES = {"sandbox": SANDBOX, "template": TEMPLATE}


def parse_args():
    args = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    if len(args) != 2 or args[0] not in GAMES:
        raise SystemExit("usage: blender -b --factory-startup --python-exit-code 1 -P humanoid.py -- <sandbox|template> <output folder>")
    path = os.path.abspath(args[1])
    os.makedirs(path, exist_ok=True)
    return GAMES[args[0]], path


# --- the rig -----------------------------------------------------------------------------

# name, head, tail, parent. Blender axes: the body faces -Y, its left is +X, Z is up.
BONES = [
    ("root", (0.0, 0.0, 0.0), (0.0, -0.3, 0.0), None),
    ("hips", (0.0, 0.0, 0.95), (0.0, 0.0, 1.1), "root"),
    ("spine", (0.0, 0.0, 1.1), (0.0, 0.0, 1.42), "hips"),
    ("head", (0.0, 0.0, 1.45), (0.0, 0.0, 1.8), "spine"),
]
for side, x in (("L", 1.0), ("R", -1.0)):
    BONES += [
        (f"upper_arm.{side}", (0.2 * x, 0.0, 1.4), (0.25 * x, 0.0, 1.12), "spine"),
        (f"forearm.{side}", (0.25 * x, 0.0, 1.12), (0.28 * x, 0.0, 0.86), f"upper_arm.{side}"),
        (f"hand.{side}", (0.28 * x, 0.0, 0.86), (0.29 * x, 0.0, 0.77), f"forearm.{side}"),
        (f"thigh.{side}", (0.1 * x, 0.0, 0.92), (0.1 * x, 0.0, 0.5), "hips"),
        (f"shin.{side}", (0.1 * x, 0.0, 0.5), (0.1 * x, 0.0, 0.09), f"thigh.{side}"),
        (f"foot.{side}", (0.1 * x, 0.0, 0.09), (0.1 * x, -0.15, 0.03), f"shin.{side}"),
    ]


def build_armature(collection):
    data = bpy.data.armatures.new("Humanoid")
    rig = bpy.data.objects.new("Humanoid", data)
    collection.objects.link(rig)
    bpy.context.view_layer.objects.active = rig
    bpy.ops.object.mode_set(mode="EDIT")
    for name, head, tail, parent in BONES:
        bone = data.edit_bones.new(name)
        bone.head, bone.tail, bone.roll = head, tail, 0.0
        if parent:
            bone.parent = data.edit_bones[parent]
            bone.use_connect = Vector(data.edit_bones[parent].tail) == Vector(head)
    bpy.ops.object.mode_set(mode="OBJECT")
    for pose_bone in rig.pose.bones:
        pose_bone.rotation_mode = "QUATERNION"
    return rig


# --- the body ----------------------------------------------------------------------------

# Greys the vertex colours shade the parts with; a look's colour is multiplied by them.
SKIN, TUNIC, BELT, TROUSERS, BOOTS, STEEL, DARK, WOOD = 1.0, 0.92, 0.38, 0.55, 0.32, 0.86, 0.22, 0.6


class Body:
    """A bmesh with a deform layer and a colour layer: parts are added with their weights."""

    def __init__(self, rig):
        self.mesh = bpy.data.meshes.new("Humanoid")
        self.obj = bpy.data.objects.new("Body", self.mesh)
        self.groups = {name: self.obj.vertex_groups.new(name=name).index for name, *_ in BONES}
        self.bm = bmesh.new()
        self.deform = self.bm.verts.layers.deform.verify()
        self.color = self.bm.loops.layers.color.new("Color")

    def vert(self, co, weights):
        v = self.bm.verts.new(co)
        for bone, w in weights.items():
            v[self.deform][self.groups[bone]] = w
        return v

    def face(self, verts, shade):
        f = self.bm.faces.new(verts)
        for loop in f.loops:
            loop[self.color] = (shade, shade, shade, 1.0)
        return f

    def box(self, center, size, bone, shade, turn=None):
        """An axis-aligned box (turned by Euler degrees `turn` about its center), all on `bone`."""
        cx, cy, cz = center
        hx, hy, hz = (s * 0.5 for s in size)
        rot = Euler([math.radians(a) for a in turn]).to_matrix() if turn else None
        corners = []
        for z in (-hz, hz):
            for x, y in ((-hx, -hy), (hx, -hy), (hx, hy), (-hx, hy)):
                offset = Vector((x, y, z))
                if rot is not None:
                    offset = rot @ offset
                corners.append(self.vert((cx + offset.x, cy + offset.y, cz + offset.z), {bone: 1.0}))
        b, t = corners[:4], corners[4:]
        self.face(list(reversed(b)), shade)
        self.face(t, shade)
        for i in range(4):
            j = (i + 1) % 4
            self.face([b[i], b[j], t[j], t[i]], shade)

    def tube(self, rings, sides, shades, cap_start=True, cap_end=True):
        """A tube through `rings`: (center, radius x, radius y, weights). Its rings are
        perpendicular to the path; `shades[i]` colours the band from ring i to ring i + 1."""
        loops = []
        centers = [Vector(r[0]) for r in rings]
        for i, (center, rx, ry, weights) in enumerate(rings):
            ahead = centers[min(i + 1, len(centers) - 1)] - centers[max(i - 1, 0)]
            axis = ahead.normalized()
            side = Vector((1.0, 0.0, 0.0)) if abs(axis.x) < 0.9 else Vector((0.0, 0.0, 1.0))
            u = (side - axis * side.dot(axis)).normalized()
            v = axis.cross(u)
            ring = []
            for k in range(sides):
                a = 2.0 * math.pi * k / sides
                ring.append(self.vert(centers[i] + u * (math.cos(a) * rx) + v * (math.sin(a) * ry), weights))
            loops.append(ring)
        for i in range(len(loops) - 1):
            for k in range(sides):
                m = (k + 1) % sides
                self.face([loops[i][k], loops[i][m], loops[i + 1][m], loops[i + 1][k]], shades[i])
        if cap_start:
            self.face(list(reversed(loops[0])), shades[0])
        if cap_end:
            self.face(loops[-1], shades[-1])

    def finish(self, rig, collection):
        self.bm.normal_update()
        bmesh.ops.recalc_face_normals(self.bm, faces=self.bm.faces)
        self.bm.to_mesh(self.mesh)
        self.bm.free()
        self.mesh.color_attributes.active_color_name = "Color"
        # The exporter takes the render colour (`export_vertex_color="ACTIVE"`).
        self.mesh.color_attributes.render_color_index = self.mesh.color_attributes.find("Color")
        material = bpy.data.materials.new("Humanoid")
        material.diffuse_color = (1.0, 1.0, 1.0, 1.0)
        self.mesh.materials.append(material)
        collection.objects.link(self.obj)
        self.obj.parent = rig
        modifier = self.obj.modifiers.new("Armature", "ARMATURE")
        modifier.object = rig
        return self.obj


def build_body(rig, collection):
    body = Body(rig)
    # Pelvis, belt and torso: the torso blends from the hips into the spine.
    body.box((0.0, 0.0, 0.92), (0.32, 0.2, 0.16), "hips", TROUSERS)
    body.tube(
        [
            ((0.0, 0.0, 0.98), 0.16, 0.11, {"hips": 1.0}),
            ((0.0, 0.0, 1.06), 0.15, 0.1, {"hips": 1.0}),
            ((0.0, 0.0, 1.16), 0.16, 0.11, {"hips": 0.5, "spine": 0.5}),
            ((0.0, 0.0, 1.32), 0.2, 0.12, {"spine": 1.0}),
            ((0.0, 0.0, 1.43), 0.17, 0.1, {"spine": 1.0}),
        ],
        8,
        [BELT, TUNIC, TUNIC, TUNIC],
    )
    # Neck, head and a visor that shows where it looks.
    body.tube([((0.0, 0.0, 1.42), 0.05, 0.05, {"head": 1.0}), ((0.0, 0.0, 1.53), 0.045, 0.045, {"head": 1.0})], 6, [SKIN])
    body.tube(
        [
            ((0.0, 0.0, 1.5), 0.04, 0.04, {"head": 1.0}),
            ((0.0, 0.0, 1.55), 0.11, 0.12, {"head": 1.0}),
            ((0.0, 0.0, 1.66), 0.125, 0.135, {"head": 1.0}),
            ((0.0, 0.0, 1.76), 0.1, 0.11, {"head": 1.0}),
            ((0.0, 0.0, 1.8), 0.03, 0.03, {"head": 1.0}),
        ],
        8,
        [SKIN, SKIN, SKIN, SKIN],
    )
    body.box((0.0, -0.125, 1.66), (0.2, 0.05, 0.05), "head", DARK)
    for side, x in (("L", 1.0), ("R", -1.0)):
        upper, fore, hand = f"upper_arm.{side}", f"forearm.{side}", f"hand.{side}"
        body.tube(
            [
                ((0.19 * x, 0.0, 1.43), 0.06, 0.06, {upper: 1.0}),
                ((0.23 * x, 0.0, 1.26), 0.055, 0.055, {upper: 1.0}),
                ((0.25 * x, 0.0, 1.12), 0.045, 0.045, {upper: 0.5, fore: 0.5}),
                ((0.27 * x, 0.0, 0.98), 0.042, 0.042, {fore: 1.0}),
                ((0.28 * x, 0.0, 0.88), 0.035, 0.035, {fore: 1.0}),
            ],
            6,
            [TUNIC, TUNIC, SKIN, SKIN],
        )
        body.box((0.285 * x, -0.005, 0.81), (0.06, 0.08, 0.1), hand, SKIN)
        thigh, shin, foot = f"thigh.{side}", f"shin.{side}", f"foot.{side}"
        body.tube(
            [
                ((0.1 * x, 0.0, 0.94), 0.08, 0.08, {thigh: 1.0}),
                ((0.1 * x, 0.0, 0.72), 0.07, 0.07, {thigh: 1.0}),
                ((0.1 * x, 0.0, 0.5), 0.055, 0.055, {thigh: 0.5, shin: 0.5}),
                ((0.1 * x, 0.0, 0.3), 0.05, 0.05, {shin: 1.0}),
                ((0.1 * x, 0.0, 0.1), 0.042, 0.042, {shin: 1.0}),
            ],
            6,
            [TROUSERS, TROUSERS, BOOTS, BOOTS],
        )
        body.box((0.1 * x, -0.05, 0.045), (0.1, 0.24, 0.09), foot, BOOTS)
    # The sword, in the right fist: forward and down at rest, so a hanging arm holds it low.
    grip = Vector((-0.29, -0.01, 0.8))
    blade = Vector((0.0, -0.7071, -0.7071))
    body.box(tuple(grip), (0.03, 0.03, 0.12), "hand.R", DARK, turn=(-45.0, 0.0, 0.0))
    body.box(tuple(grip + blade * 0.07), (0.18, 0.04, 0.03), "hand.R", DARK, turn=(-45.0, 0.0, 0.0))
    body.box(tuple(grip + blade * 0.47), (0.05, 0.012, 0.76), "hand.R", STEEL, turn=(-45.0, 0.0, 0.0))
    # A buckler on the outside of the left forearm, facing out.
    body.tube(
        [((0.315, 0.0, 1.0), 0.17, 0.17, {"forearm.L": 1.0}), ((0.345, 0.0, 1.0), 0.16, 0.16, {"forearm.L": 1.0})],
        10,
        [WOOD],
    )
    body.box((0.355, 0.0, 1.0), (0.03, 0.08, 0.08), "forearm.L", STEEL)
    return body.finish(rig, collection)


# --- posing ------------------------------------------------------------------------------


class Clip:
    """One action of the armature: poses keyed at times in seconds, the root's motion, and the
    phases it was timed to."""

    def __init__(self, rig, name, fps, phases=None):
        self.rig, self.fps = rig, fps
        self.action = bpy.data.actions.new(name)
        self.action.use_fake_user = True
        if phases:
            self.action["phases"] = " ".join(f"{phase} {secs:g}" for phase, secs in phases)
            for end in ends(phases).values():
                if abs(end * fps - round(end * fps)) > 1e-6:
                    raise ValueError(f"{name}: its phase ending at {end} s is not a whole frame at {fps} fps")
        rig.animation_data_create()
        rig.animation_data.action = self.action
        # What the last clip left in the pose is not this clip's.
        for pose_bone in rig.pose.bones:
            pose_bone.location = (0.0, 0.0, 0.0)
            pose_bone.rotation_quaternion = (1.0, 0.0, 0.0, 0.0)

    def frame(self, t):
        """The frame nearest `t` (the phases' ends are whole frames, checked above)."""
        return int(round(t * self.fps))

    def pose(self, t, bones=None, hips=(0.0, 0.0, 0.0)):
        """Every bone keyed at `t`: `bones` turns some (Euler degrees about the armature's
        axes, relative to the parent), the others stand as at rest; `hips` moves the hips."""
        bones = bones or {}
        for pose_bone in self.rig.pose.bones:
            rest = pose_bone.bone.matrix_local.to_quaternion()
            turn = Euler([math.radians(a) for a in bones.get(pose_bone.name, (0.0, 0.0, 0.0))]).to_quaternion()
            pose_bone.rotation_quaternion = rest.inverted() @ turn @ rest
            pose_bone.keyframe_insert("rotation_quaternion", frame=self.frame(t))
        hip_bone = self.rig.pose.bones["hips"]
        hip_bone.location = hip_bone.bone.matrix_local.to_quaternion().inverted() @ Vector(hips)
        hip_bone.keyframe_insert("location", frame=self.frame(t))

    def root(self, t, forward, interpolation=None):
        """The root `forward` meters ahead (Blender's -Y) at `t`, a whole frame: the action's
        displacement is baked from it, and a key off the frame would move it in time."""
        if abs(t * self.fps - round(t * self.fps)) > 1e-6:
            raise ValueError(f"{self.action.name}: the root moves at {t} s, no whole frame at {self.fps} fps")
        bone = self.rig.pose.bones["root"]
        bone.location = bone.bone.matrix_local.to_quaternion().inverted() @ Vector((0.0, -forward, 0.0))
        bone.keyframe_insert("location", frame=self.frame(t))
        if interpolation:
            for curve in self.curves("root", "location"):
                for key in curve.keyframe_points:
                    if key.co.x == self.frame(t):
                        key.interpolation, key.easing = interpolation

    def curves(self, bone, prop):
        path = f'pose.bones["{bone}"].{prop}'
        for layer in self.action.layers:
            for strip in layer.strips:
                for bag in strip.channelbags:
                    yield from (c for c in bag.fcurves if c.data_path == path)

    def lunge(self, lunge):
        if not lunge:
            return
        start, end, meters = lunge
        if start > 0.0:
            self.root(0.0, 0.0)
        self.root(start, 0.0)
        self.root(end, meters)

    def finish(self):
        """Pins the root where the clip has no root motion, and puts the action on a track of
        its own (*Push Down*), so the exporter finds it."""
        if not any(self.curves("root", "location")):
            self.root(0.0, 0.0)
        track = self.rig.animation_data.nla_tracks.new()
        track.name = self.action.name
        track.strips.new(self.action.name, 0, self.action)
        self.rig.animation_data.action = None


# Poses: bone -> (x, y, z) degrees about the armature's axes, relative to the parent. About X a
# hanging limb swings back (forward is negative) and the spine bends forward; about Z the body
# turns to its left; about Y a hanging right arm lifts out to the side (a left arm with negative
# angles).
STAND = {
    "upper_arm.L": (-8.0, 0.0, 0.0),
    "forearm.L": (-20.0, 0.0, 0.0),
    "upper_arm.R": (-8.0, 0.0, 0.0),
    "forearm.R": (-25.0, 0.0, 0.0),
}

GUARD = {
    "spine": (6.0, 0.0, 8.0),
    "head": (-4.0, 0.0, -6.0),
    "upper_arm.L": (-30.0, -10.0, 0.0),
    "forearm.L": (-60.0, 0.0, 20.0),
    "upper_arm.R": (-25.0, 12.0, 0.0),
    "forearm.R": (-50.0, 0.0, 0.0),
    "thigh.L": (-14.0, 0.0, 0.0),
    "shin.L": (20.0, 0.0, 0.0),
    "foot.L": (-6.0, 0.0, 0.0),
    "thigh.R": (6.0, 0.0, 0.0),
    "shin.R": (14.0, 0.0, 0.0),
    "foot.R": (-20.0, 0.0, 0.0),
}
GUARD_HIPS = (0.0, 0.0, -0.04)


def merge(base, **changes):
    out = dict(base)
    for key, value in changes.items():
        out[key.replace("_L", ".L").replace("_R", ".R")] = value
    return out


def ends(phases):
    """The time each phase ends."""
    out, t = {}, 0.0
    for name, secs in phases:
        t = round(t + secs, 6)
        out[name] = t
    return out


def idle(rig, fps):
    clip = Clip(rig, "idle", fps)
    breathe = merge(STAND, spine=(3.0, 0.0, 0.0), head=(-2.0, 0.0, 0.0))
    clip.pose(0.0, STAND)
    clip.pose(1.0, breathe, hips=(0.0, 0.0, -0.01))
    clip.pose(2.0, STAND)
    clip.finish()


def stride(rig, name, fps, period, legs, knee, arms, lean, bob):
    """A walk or run loop: the left leg forward at the start, the right one half a period on."""
    clip = Clip(rig, name, fps)
    half, quarter = period / 2.0, period / 4.0

    def step(lead, t):
        a, b = ("L", "R") if lead == "L" else ("R", "L")
        pose = {
            "spine": (lean, 0.0, 0.0),
            f"thigh.{a}": (-legs, 0.0, 0.0),
            f"shin.{a}": (knee * 0.3, 0.0, 0.0),
            f"foot.{a}": (-8.0, 0.0, 0.0),
            f"thigh.{b}": (legs * 0.8, 0.0, 0.0),
            f"shin.{b}": (knee * 0.6, 0.0, 0.0),
            f"foot.{b}": (12.0, 0.0, 0.0),
            f"upper_arm.{a}": (arms, 0.0, 0.0),
            f"upper_arm.{b}": (-arms, 0.0, 0.0),
            f"forearm.{a}": (-20.0 - arms * 0.5, 0.0, 0.0),
            f"forearm.{b}": (-30.0 - arms, 0.0, 0.0),
        }
        clip.pose(t, pose, hips=(0.0, 0.0, -bob))

    def passing(lead, t):
        a, b = ("L", "R") if lead == "L" else ("R", "L")
        pose = {
            "spine": (lean, 0.0, 0.0),
            f"thigh.{a}": (legs * 0.2, 0.0, 0.0),
            f"shin.{a}": (knee * 0.2, 0.0, 0.0),
            f"thigh.{b}": (-legs * 0.6, 0.0, 0.0),
            f"shin.{b}": (knee, 0.0, 0.0),
            f"foot.{b}": (-10.0, 0.0, 0.0),
            "forearm.L": (-25.0, 0.0, 0.0),
            "forearm.R": (-30.0, 0.0, 0.0),
        }
        clip.pose(t, pose, hips=(0.0, 0.0, bob * 0.5))

    step("L", 0.0)
    passing("L", quarter)
    step("R", half)
    passing("R", half + quarter)
    step("L", period)
    clip.finish()


def swing(rig, game, name, windup, strike, settle):
    """An attack: from the guard to `windup` by the end of the windup phase, to `strike` by the
    end of the active one, `settle` part way through the recovery, back to the guard at the
    end. Its root lunges as the game's table says."""
    timing = game[name]
    clip = Clip(rig, name, game["fps"], timing["phases"])
    at = ends(timing["phases"])
    phases = [p for p, _ in timing["phases"]]
    windup_end, active_end, end = at[phases[0]], at[phases[1]], at[phases[2]]
    clip.pose(0.0, GUARD, GUARD_HIPS)
    clip.pose(windup_end, *windup)
    clip.pose(active_end, *strike)
    clip.pose(active_end + (end - active_end) * 0.4, *settle)
    clip.pose(end, GUARD, GUARD_HIPS)
    clip.lunge(timing.get("lunge"))
    if timing.get("lunge"):
        clip.root(end, timing["lunge"][2])
    clip.finish()


def attacks(rig, game):
    # Light 1: a flat slash from the right to the left.
    swing(
        rig, game, "light_1",
        (merge(GUARD, spine=(4.0, 0.0, -35.0), head=(0.0, 0.0, 25.0), upper_arm_R=(-40.0, 75.0, -40.0),
               forearm_R=(-70.0, 0.0, 0.0), upper_arm_L=(-40.0, -20.0, 0.0)), GUARD_HIPS),
        (merge(GUARD, spine=(10.0, 0.0, 40.0), head=(0.0, 0.0, -25.0), upper_arm_R=(-80.0, 30.0, 60.0),
               forearm_R=(-20.0, 0.0, 0.0), thigh_L=(-30.0, 0.0, 0.0), shin_L=(30.0, 0.0, 0.0)), (0.0, 0.0, -0.08)),
        (merge(GUARD, spine=(10.0, 0.0, 30.0), upper_arm_R=(-60.0, 20.0, 50.0), forearm_R=(-40.0, 0.0, 0.0)), GUARD_HIPS),
    )
    # Light 2: the backhand, from the left back to the right.
    swing(
        rig, game, "light_2",
        (merge(GUARD, spine=(4.0, 0.0, 40.0), head=(0.0, 0.0, -25.0), upper_arm_R=(-70.0, 10.0, 70.0),
               forearm_R=(-80.0, 0.0, 0.0)), GUARD_HIPS),
        (merge(GUARD, spine=(10.0, 0.0, -40.0), head=(0.0, 0.0, 25.0), upper_arm_R=(-75.0, 70.0, -50.0),
               forearm_R=(-15.0, 0.0, 0.0), thigh_R=(-25.0, 0.0, 0.0), shin_R=(25.0, 0.0, 0.0), thigh_L=(10.0, 0.0, 0.0)),
         (0.0, 0.0, -0.08)),
        (merge(GUARD, spine=(8.0, 0.0, -25.0), upper_arm_R=(-55.0, 45.0, -30.0), forearm_R=(-40.0, 0.0, 0.0)), GUARD_HIPS),
    )
    # Light 3: an overhead chop with a step in.
    swing(
        rig, game, "light_3",
        (merge(GUARD, spine=(-12.0, 0.0, 5.0), head=(-8.0, 0.0, 0.0), upper_arm_R=(-165.0, 15.0, 0.0),
               forearm_R=(-40.0, 0.0, 0.0), upper_arm_L=(-50.0, -10.0, 0.0)), (0.0, 0.0, 0.0)),
        (merge(GUARD, spine=(30.0, 0.0, 0.0), head=(-15.0, 0.0, 0.0), upper_arm_R=(-60.0, 10.0, 0.0),
               forearm_R=(-10.0, 0.0, 0.0), thigh_L=(-45.0, 0.0, 0.0), shin_L=(45.0, 0.0, 0.0),
               thigh_R=(20.0, 0.0, 0.0), shin_R=(25.0, 0.0, 0.0)), (0.0, 0.0, -0.14)),
        (merge(GUARD, spine=(22.0, 0.0, 0.0), upper_arm_R=(-55.0, 10.0, 0.0), forearm_R=(-25.0, 0.0, 0.0),
               thigh_L=(-35.0, 0.0, 0.0), shin_L=(35.0, 0.0, 0.0)), (0.0, 0.0, -0.1)),
    )
    # Heavy: both arms high and far back, then down with the whole body.
    swing(
        rig, game, "heavy",
        (merge(GUARD, spine=(-20.0, 0.0, 15.0), head=(-10.0, 0.0, -10.0), upper_arm_R=(-170.0, 0.0, 0.0),
               forearm_R=(-60.0, 0.0, 0.0), upper_arm_L=(-160.0, -10.0, 0.0), forearm_L=(-60.0, 0.0, 0.0),
               thigh_L=(-25.0, 0.0, 0.0), shin_L=(35.0, 0.0, 0.0), thigh_R=(15.0, 0.0, 0.0), shin_R=(30.0, 0.0, 0.0)),
         (0.0, 0.0, -0.1)),
        (merge(GUARD, spine=(45.0, 0.0, -5.0), head=(-25.0, 0.0, 0.0), upper_arm_R=(-55.0, 5.0, 0.0),
               forearm_R=(-5.0, 0.0, 0.0), upper_arm_L=(-40.0, -10.0, 0.0), forearm_L=(-30.0, 0.0, 0.0),
               thigh_L=(-60.0, 0.0, 0.0), shin_L=(70.0, 0.0, 0.0), thigh_R=(30.0, 0.0, 0.0), shin_R=(45.0, 0.0, 0.0)),
         (0.0, 0.0, -0.3)),
        (merge(GUARD, spine=(35.0, 0.0, -5.0), head=(-20.0, 0.0, 0.0), upper_arm_R=(-50.0, 5.0, 0.0),
               forearm_R=(-15.0, 0.0, 0.0), thigh_L=(-50.0, 0.0, 0.0), shin_L=(60.0, 0.0, 0.0),
               thigh_R=(25.0, 0.0, 0.0), shin_R=(40.0, 0.0, 0.0)), (0.0, 0.0, -0.24)),
    )


def block(rig, game):
    timing = game["block"]
    clip = Clip(rig, "block", game["fps"], timing["phases"])
    at = ends(timing["phases"])
    raised = merge(GUARD, spine=(10.0, 0.0, -10.0), head=(6.0, 0.0, 0.0), upper_arm_L=(-75.0, 0.0, -45.0),
                   forearm_L=(-80.0, 0.0, -30.0), upper_arm_R=(-10.0, 20.0, 0.0), forearm_R=(-70.0, 0.0, 0.0),
                   thigh_L=(-20.0, 0.0, 0.0), shin_L=(30.0, 0.0, 0.0), thigh_R=(15.0, 0.0, 0.0), shin_R=(25.0, 0.0, 0.0))
    clip.pose(0.0, GUARD, GUARD_HIPS)
    clip.pose(at["raise"], raised, (0.0, 0.0, -0.08))
    clip.pose(at["hold"], raised, (0.0, 0.0, -0.08))
    clip.pose(at["lower"], GUARD, GUARD_HIPS)
    clip.finish()


def roll(rig, game):
    """A forward roll: the hips turn a full circle about the body's left axis while the body
    curls up, then it stands. The root goes forward fast at first, slower at the end."""
    timing = game["roll"]
    clip = Clip(rig, "roll", game["fps"], timing["phases"])
    at = ends(timing["phases"])
    t_roll = at["roll"]
    tuck = {
        "spine": (35.0, 0.0, 0.0),
        "head": (30.0, 0.0, 0.0),
        "upper_arm.L": (-60.0, 0.0, 0.0),
        "forearm.L": (-100.0, 0.0, 0.0),
        "upper_arm.R": (-60.0, 0.0, 0.0),
        "forearm.R": (-90.0, 0.0, 0.0),
        "thigh.L": (-120.0, 0.0, 0.0),
        "shin.L": (130.0, 0.0, 0.0),
        "thigh.R": (-115.0, 0.0, 0.0),
        "shin.R": (125.0, 0.0, 0.0),
    }
    clip.pose(0.0, merge(GUARD, spine=(25.0, 0.0, 0.0)), (0.0, 0.0, -0.1))
    for share, turn, low in ((0.25, 90.0, 0.5), (0.5, 180.0, 0.55), (0.75, 270.0, 0.45)):
        clip.pose(t_roll * share, merge(tuck, hips=(turn, 0.0, 0.0)), (0.0, 0.0, -low))
    clip.pose(t_roll, merge(GUARD, spine=(20.0, 0.0, 0.0), hips=(360.0, 0.0, 0.0), thigh_L=(-40.0, 0.0, 0.0),
                              shin_L=(60.0, 0.0, 0.0), thigh_R=(-20.0, 0.0, 0.0), shin_R=(50.0, 0.0, 0.0)), (0.0, 0.0, -0.18))
    clip.pose(at["recover"], merge(GUARD, hips=(360.0, 0.0, 0.0)), GUARD_HIPS)
    start, end, meters = timing["lunge"]
    clip.root(start, 0.0, ("QUAD", "EASE_OUT"))
    clip.root(end, meters)
    clip.root(at["recover"], meters)
    clip.finish()


def hit(rig, fps):
    clip = Clip(rig, "hit", fps)
    reel = merge(GUARD, spine=(-25.0, 0.0, 10.0), head=(-25.0, 0.0, 0.0), upper_arm_L=(-50.0, -40.0, 0.0),
                 upper_arm_R=(-40.0, 50.0, 0.0), forearm_R=(-30.0, 0.0, 0.0), thigh_L=(10.0, 0.0, 0.0),
                 shin_L=(25.0, 0.0, 0.0), thigh_R=(-25.0, 0.0, 0.0), shin_R=(30.0, 0.0, 0.0))
    clip.pose(0.0, GUARD, GUARD_HIPS)
    clip.pose(0.15, reel, (0.0, 0.12, -0.08))
    clip.pose(0.45, merge(reel, spine=(-10.0, 0.0, 5.0), head=(-8.0, 0.0, 0.0)), (0.0, 0.08, -0.06))
    clip.pose(0.8, GUARD, GUARD_HIPS)
    clip.finish()


def death(rig, fps):
    """Knees give, it falls on its back and lies still."""
    clip = Clip(rig, "death", fps)
    clip.pose(0.0, GUARD, GUARD_HIPS)
    clip.pose(0.3, merge(GUARD, spine=(-20.0, 0.0, 10.0), head=(-30.0, 0.0, 0.0), thigh_L=(-40.0, 0.0, 0.0),
                         shin_L=(80.0, 0.0, 0.0), thigh_R=(-30.0, 0.0, 0.0), shin_R=(70.0, 0.0, 0.0),
                         upper_arm_L=(-30.0, -30.0, 0.0), upper_arm_R=(-20.0, 40.0, 0.0)), (0.0, 0.05, -0.35))
    lying = {
        "hips": (-90.0, 0.0, 0.0),
        "spine": (-5.0, 0.0, 5.0),
        "head": (-10.0, 0.0, 15.0),
        "upper_arm.L": (-20.0, -70.0, 0.0),
        "forearm.L": (-20.0, 0.0, 0.0),
        "upper_arm.R": (-30.0, 80.0, 0.0),
        "forearm.R": (-30.0, 0.0, 0.0),
        "thigh.L": (-10.0, -10.0, 0.0),
        "shin.L": (15.0, 0.0, 0.0),
        "thigh.R": (-25.0, 8.0, 0.0),
        "shin.R": (40.0, 0.0, 0.0),
    }
    clip.pose(0.8, lying, (0.0, 0.55, -0.82))
    clip.pose(1.2, lying, (0.0, 0.55, -0.84))
    clip.finish()


def bow(rig, fps):
    clip = Clip(rig, "bow", fps)
    low = merge(STAND, hips=(25.0, 0.0, 0.0), spine=(30.0, 0.0, 0.0), head=(15.0, 0.0, 0.0),
                upper_arm_R=(-55.0, -20.0, -40.0), forearm_R=(-90.0, 0.0, 0.0), upper_arm_L=(15.0, 0.0, 0.0),
                thigh_L=(-25.0, 0.0, 0.0), thigh_R=(-25.0, 0.0, 0.0))
    clip.pose(0.0, STAND)
    clip.pose(0.5, low, (0.0, 0.05, 0.0))
    clip.pose(1.1, low, (0.0, 0.05, 0.0))
    clip.pose(1.6, STAND)
    clip.finish()


def taunt(rig, fps):
    clip = Clip(rig, "taunt", fps)
    up = merge(STAND, spine=(-10.0, 0.0, 0.0), head=(-15.0, 0.0, 0.0), upper_arm_R=(-170.0, 10.0, 0.0),
               forearm_R=(-20.0, 0.0, 0.0), upper_arm_L=(-20.0, -40.0, 0.0), forearm_L=(-60.0, 0.0, 0.0))
    shake = merge(up, upper_arm_R=(-160.0, 25.0, 0.0), forearm_R=(-45.0, 0.0, 0.0))
    clip.pose(0.0, STAND)
    clip.pose(0.3, up)
    clip.pose(0.5, shake)
    clip.pose(0.7, up)
    clip.pose(0.9, shake)
    clip.pose(1.4, STAND)
    clip.finish()


def drink(rig, game):
    """A swig from a flask: the right hand comes up to the mouth, the head tips back, the hand
    goes down. Only the spine, the head and the arms move; the legs stand as at rest, so a game
    plays it on a layer (the animset's `layers`) over whatever the legs do, a walk or a run. Timed
    to the sandbox's `drink` action: the swig is at 0.7 s, where its heal lands."""
    timing = game["drink"]
    clip = Clip(rig, "drink", game["fps"], timing["phases"])
    at = ends(timing["phases"])
    raised = merge(STAND, spine=(4.0, 0.0, 0.0), upper_arm_R=(-50.0, 45.0, 45.0), forearm_R=(-125.0, 0.0, 0.0))
    clip.pose(0.0, STAND)
    clip.pose(0.4, raised)
    clip.pose(0.7, merge(raised, spine=(-2.0, 0.0, 0.0), head=(-14.0, 0.0, 0.0)))
    clip.pose(at["drink"], raised)
    clip.pose(at["recover"], STAND)
    clip.finish()


# --- export ------------------------------------------------------------------------------


def scene(fps):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.context.scene.name = "Humanoid"
    bpy.context.scene.render.fps = fps
    return bpy.context.scene.collection


def export(path):
    result = bpy.ops.export_scene.gltf(
        filepath=path,
        export_format="GLB",
        export_extras=True,
        # Only the shades, as COLOR_0: with all colours on, the exporter adds a white COLOR_0
        # (no material reads ours) and moves the shades to COLOR_1, which Bevy does not read.
        export_vertex_color="ACTIVE",
        export_all_vertex_colors=False,
    )
    if result != {"FINISHED"}:
        raise RuntimeError(f"export of {path} failed: {result}")
    print(f"wrote {path} ({os.path.getsize(path)} bytes)")


def main():
    game, folder = parse_args()
    fps = game["fps"]
    root = scene(fps)
    rig = build_armature(root)
    build_body(rig, root)
    idle(rig, fps)
    stride(rig, "walk", fps, period=0.8, legs=28.0, knee=35.0, arms=18.0, lean=4.0, bob=0.03)
    stride(rig, "run", fps, period=0.6, legs=45.0, knee=95.0, arms=35.0, lean=14.0, bob=0.06)
    attacks(rig, game)
    block(rig, game)
    roll(rig, game)
    hit(rig, fps)
    death(rig, fps)
    export(os.path.join(folder, "humanoid.glb"))
    if game["gestures"]:
        root = scene(fps)
        rig = build_armature(root)
        bow(rig, fps)
        taunt(rig, fps)
        drink(rig, game)
        export(os.path.join(folder, "humanoid_gestures.glb"))


if __name__ == "__main__":
    main()
