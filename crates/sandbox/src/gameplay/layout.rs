//! The world as content: `layout` definitions (`assets/content/world/*.layout.ron`) are
//! the arena and the obstacle course: static boxes and ramps the physics layer collides
//! with, sensor volumes (water, updrafts, triggers), landmarks the AI sees, and the prefabs
//! spawned when the game starts.
//!
//! Layouts spawn in id order, each in a fixed order (solids, ramps, volumes, landmarks,
//! spawns), so colliders and AI ids come out the same in every run. A hot edit rebuilds the
//! edited layout's geometry inside the next tick (recorded in replays like any content
//! change); landmarks keep their ids and move in place; spawned characters stay. A layout
//! file added while the game runs is built the same way, prefabs included; one removed
//! takes its geometry and landmarks with it, and again leaves the characters.

use shards::motion::{Collider, CollisionLayers, Shape, Volume, VolumeKind};
use shards::prelude::*;

/// What a solid looks like in a window; nothing else depends on it.
#[derive(Serialize, Deserialize, Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(crate = "shards::serde")]
pub enum Surface {
    #[default]
    Stone,
    Floor,
    Wall,
    Sand,
}

impl Surface {
    /// The look it is drawn with (`content/look/sandbox.look.ron`).
    pub fn look(self) -> &'static str {
        match self {
            Surface::Stone => "stone",
            Surface::Floor => "floor",
            Surface::Wall => "wall",
            Surface::Sand => "sand",
        }
    }
}

/// A static box between two corners.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
pub struct Solid {
    pub min: Vec3,
    pub max: Vec3,
    #[serde(default)]
    pub look: Surface,
}

/// A slope: its top surface runs from `from` to `to` (the middles of its low and high
/// edges), `width` across.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
pub struct Ramp {
    pub from: Vec3,
    pub to: Vec3,
    pub width: f32,
    #[serde(default = "default_thickness")]
    pub thickness: f32,
}

fn default_thickness() -> f32 {
    0.5
}

/// A named point the AI lists under LANDMARKS (`e9 Stairs`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
pub struct Landmark {
    pub name: String,
    pub at: Vec3,
}

/// A sensor box: water, an updraft, a named trigger. With `landmark` the AI sees it as one,
/// standing at the middle of its floor.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
pub struct VolumeBox {
    pub min: Vec3,
    pub max: Vec3,
    pub kind: VolumeKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub landmark: Option<String>,
}

/// A prefab spawned once, when the game starts.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
pub struct Spawn {
    pub prefab: String,
    pub at: Vec3,
    /// Where it looks, on the ground plane. Default: north (-Z).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facing: Option<Vec3>,
}

/// One area of the world (content kind `layout`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
pub struct LayoutDef {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub solids: Vec<Solid>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ramps: Vec<Ramp>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub landmarks: Vec<Landmark>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<VolumeBox>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spawns: Vec<Spawn>,
}

impl ContentDef for LayoutDef {
    const KIND: &'static str = "layout";

    fn validate(&self, _: &str, refs: &mut Refs) {
        let mut boxes: Vec<(String, Vec3, Vec3)> =
            self.solids.iter().enumerate().map(|(i, s)| (format!("solids[{i}]"), s.min, s.max)).collect();
        boxes.extend(self.volumes.iter().enumerate().map(|(i, v)| (format!("volumes[{i}]"), v.min, v.max)));
        for (field, min, max) in boxes {
            if !(min.is_finite() && max.is_finite() && min.cmplt(max).all()) {
                refs.error(format!("{field}: min {min} must be below max {max} on every axis"));
            }
        }
        for (i, ramp) in self.ramps.iter().enumerate() {
            let run = (ramp.to - ramp.from).with_y(0.0).length();
            if !(run > 0.01 && ramp.width > 0.0 && ramp.thickness > 0.0 && ramp.from.is_finite() && ramp.to.is_finite())
            {
                refs.error(format!("ramps[{i}]: needs `from` and `to` apart on the ground, width and thickness > 0"));
            }
        }
        for (i, volume) in self.volumes.iter().enumerate() {
            if let VolumeKind::Updraft { accel, max_speed } = volume.kind
                && !(accel > 0.0 && max_speed > 0.0)
            {
                refs.error(format!("volumes[{i}]: an updraft needs accel and max_speed > 0"));
            }
        }
        let names =
            self.landmarks.iter().map(|l| &l.name).chain(self.volumes.iter().filter_map(|v| v.landmark.as_ref()));
        if names.into_iter().any(|n| n.trim().is_empty()) {
            refs.error("a landmark needs a name");
        }
        for (i, spawn) in self.spawns.iter().enumerate() {
            refs.require::<PrefabDef>(&spawn.prefab, &format!("spawns[{i}]"));
        }
    }
}

impl LayoutDef {
    /// What each of its solids, ramps and volumes covers on the ground plane (x, z), one
    /// rectangle per piece: empty for a layout of spawns only. Not their union: the voids
    /// between the pieces (beside the arena's corridor, beside the course's lane) are not in
    /// the world.
    pub fn footprint(&self) -> Vec<Rect> {
        let solids = self.solids.iter().map(|s| (s.min, s.max));
        let volumes = self.volumes.iter().map(|v| (v.min, v.max));
        let ramps = self.ramps.iter().map(|r| {
            let half = r.width * 0.5;
            (r.from.min(r.to) - Vec3::splat(half), r.from.max(r.to) + Vec3::splat(half))
        });
        solids.chain(volumes).chain(ramps).map(|(min, max)| Rect::from_corners(min.xz(), max.xz())).collect()
    }
}

/// Where bodies may be (content kind `bounds`, id `world`): over what the built layouts and
/// the loaded region cover on the ground, up to `margin` meters beyond, and no lower than
/// `fall_below`. The `bodies_in_world` invariant ([`crate::ai`]) holds every character to it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
pub struct BoundsDef {
    /// Meters beyond the geometry that still count as inside.
    pub margin: f32,
    /// A body below this height has fallen out of the world.
    pub fall_below: f32,
}

impl ContentDef for BoundsDef {
    const KIND: &'static str = "bounds";

    fn validate(&self, _: &str, refs: &mut Refs) {
        if !(self.margin.is_finite() && self.margin >= 0.0) {
            refs.error(format!("margin {} must be 0 or more", self.margin));
        }
        if !self.fall_below.is_finite() {
            refs.error("fall_below must be a number of meters");
        }
    }
}

/// Part of a layout's geometry: rebuilt when the layout changes.
#[derive(Component, Reflect, Clone, Debug, PartialEq)]
#[reflect(Component)]
pub struct LayoutPiece {
    pub layout: String,
    pub kind: PieceKind,
}

/// What a [`LayoutPiece`] is, for rebuilding and for the window's looks.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub enum PieceKind {
    Solid(Surface),
    Ramp,
    /// The n-th landmark (`landmarks`, then volumes with a `landmark`): keeps its id.
    Landmark(usize),
    Volume,
}

/// The layouts as last built, by id.
#[derive(Resource, Default, Clone, Debug)]
pub struct BuiltLayouts(pub Vec<(String, LayoutDef)>);

/// `OnEnter(InGame)`: every layout in id order, its prefabs included.
pub fn spawn_world(world: &mut World) {
    let layouts: Vec<(String, LayoutDef)> =
        world.resource::<ContentDb>().iter::<LayoutDef>().map(|(id, def)| (id.to_owned(), def.clone())).collect();
    for (id, def) in &layouts {
        build_with_spawns(world, id, def);
    }
    world.insert_resource(BuiltLayouts(layouts));
}

/// A layout as built when the game starts: its geometry and landmarks, then its prefabs.
fn build_with_spawns(world: &mut World, id: &str, def: &LayoutDef) {
    build(world, id, def);
    for spawn in &def.spawns {
        let facing = spawn.facing.and_then(|f| f.with_y(0.0).try_normalize()).unwrap_or(Vec3::NEG_Z);
        let at = Transform::from_translation(spawn.at).looking_to(facing, Vec3::Y);
        if let Err(why) = shards::content::spawn_prefab(world, &spawn.prefab, at, None) {
            error!("layout `{id}`: cannot spawn `{}`: {why}", spawn.prefab);
        }
    }
}

/// The geometry of one layout: everything but the spawns.
fn build(world: &mut World, id: &str, def: &LayoutDef) {
    build_geometry(world, id, def);
    for (n, landmark) in landmarks(def).into_iter().enumerate() {
        let mut e = world.spawn((
            LayoutPiece { layout: id.to_owned(), kind: PieceKind::Landmark(n) },
            Name::new(landmark.name.clone()),
        ));
        place_landmark(&mut e, &landmark);
    }
}

/// Solids, ramps and the volumes that are no landmarks.
fn build_geometry(world: &mut World, id: &str, def: &LayoutDef) {
    let piece = |kind| LayoutPiece { layout: id.to_owned(), kind };
    for solid in &def.solids {
        world.spawn((piece(PieceKind::Solid(solid.look)), solid_box(solid.min, solid.max)));
    }
    for ramp in &def.ramps {
        world.spawn((piece(PieceKind::Ramp), ramp_box(ramp)));
    }
    for volume in def.volumes.iter().filter(|v| v.landmark.is_none()) {
        world.spawn((piece(PieceKind::Volume), volume_box(volume)));
    }
}

/// A landmark as built: a point, or a volume the AI sees.
struct Placed {
    name: String,
    at: Vec3,
    volume: Option<(Collider, Volume)>,
}

/// The landmarks of a layout in id order: points first, then landmark volumes.
fn landmarks(def: &LayoutDef) -> Vec<Placed> {
    let points = def.landmarks.iter().map(|l| Placed { name: l.name.clone(), at: l.at, volume: None });
    let volumes = def.volumes.iter().filter_map(|v| {
        let (collider, volume, at) = standing_volume(v);
        Some(Placed { name: v.landmark.clone()?, at, volume: Some((collider, volume)) })
    });
    points.chain(volumes).collect()
}

fn place_landmark(e: &mut EntityWorldMut, landmark: &Placed) {
    e.insert((AiKind::new(landmark.name.clone()), AiLandmark, Transform::from_translation(landmark.at)));
    match &landmark.volume {
        Some((collider, volume)) => {
            e.insert((*collider, volume.clone()));
        }
        None => {
            e.remove::<(Collider, Volume)>();
        }
    }
}

fn solid_box(min: Vec3, max: Vec3) -> (Collider, Transform) {
    let half = (max - min) * 0.5;
    (Collider::new(Shape::cuboid(half)), Transform::from_translation(min + half))
}

/// A slab whose top runs from `from` to `to`: local Z along the slope, X level across it.
fn ramp_box(ramp: &Ramp) -> (Collider, Transform) {
    let along = ramp.to - ramp.from;
    let z = along.normalize();
    let x = Vec3::Y.cross(z).normalize();
    let y = z.cross(x);
    let rotation = Quat::from_mat3(&Mat3::from_cols(x, y, z));
    let middle = (ramp.from + ramp.to) * 0.5 - y * ramp.thickness * 0.5;
    let half = Vec3::new(ramp.width * 0.5, ramp.thickness * 0.5, along.length() * 0.5);
    (Collider::new(Shape::cuboid(half)), Transform::from_translation(middle).with_rotation(rotation))
}

/// A volume standing on the middle of its floor: the entity's origin is where the AI sees it.
fn standing_volume(v: &VolumeBox) -> (Collider, Volume, Vec3) {
    let half = (v.max - v.min) * 0.5;
    let floor = Vec3::new(v.min.x + half.x, v.min.y, v.min.z + half.z);
    let collider =
        Collider::new(Shape::cuboid(half)).with_offset(Vec3::Y * half.y).with_layers(CollisionLayers::VOLUME);
    (collider, Volume { kind: v.kind.clone() }, floor)
}

fn volume_box(v: &VolumeBox) -> (Collider, Volume, Transform) {
    let (collider, volume, floor) = standing_volume(v);
    (collider, volume, Transform::from_translation(floor))
}

/// `SimSet::Cleanup`: the layouts built are made to match the definitions for the next tick
/// (a hot edit, a replayed `content_apply`), in id order within each step:
///
/// 1. One whose definition is gone (its file was removed) loses its boxes, ramps, volumes
///    and landmarks.
/// 2. One whose definition changed is rebuilt: its boxes, ramps and plain volumes anew, its
///    landmarks moved in place (they keep their ids; extra ones are added, missing ones
///    removed).
/// 3. One that was added (its file is new) is built as at start, prefabs included.
///
/// Characters a layout spawned stay where they are.
pub fn rebuild_changed_layouts(world: &mut World) {
    let (removed, changed, added): (Vec<String>, Vec<(String, LayoutDef)>, Vec<(String, LayoutDef)>) = {
        let Some(built) = world.get_resource::<BuiltLayouts>().map(|b| &b.0[..]) else { return };
        let db = world.resource::<ContentDb>();
        let mut removed: Vec<String> =
            built.iter().filter(|(id, _)| db.get::<LayoutDef>(id).is_none()).map(|(id, _)| id.clone()).collect();
        removed.sort();
        let owned = |(id, def): (&str, &LayoutDef)| (id.to_owned(), def.clone());
        let changed = db
            .iter::<LayoutDef>()
            .filter(|(id, def)| built.iter().any(|(b, old)| b == id && old != *def))
            .map(owned)
            .collect();
        let added = db.iter::<LayoutDef>().filter(|(id, _)| built.iter().all(|(b, _)| b != id)).map(owned).collect();
        (removed, changed, added)
    };
    for id in removed {
        let mut pieces: Vec<(Option<AiId>, Entity)> = world
            .query::<(Entity, &LayoutPiece, Option<&AiId>)>()
            .iter(world)
            .filter(|(_, p, _)| p.layout == id)
            .map(|(e, _, ai)| (ai.copied(), e))
            .collect();
        // By id, not by entity: the tombstones of the landmarks come out the same in every run.
        pieces.sort_by_key(|(ai, _)| *ai);
        for (_, entity) in pieces {
            world.despawn(entity);
        }
        world.resource_mut::<BuiltLayouts>().0.retain(|(b, _)| *b != id);
        info!("layout `{id}` removed");
    }
    for (id, def) in changed {
        let pieces: Vec<(Entity, PieceKind, Option<AiId>)> = world
            .query::<(Entity, &LayoutPiece, Option<&AiId>)>()
            .iter(world)
            .filter(|(_, p, _)| p.layout == id)
            .map(|(e, p, ai)| (e, p.kind, ai.copied()))
            .collect();
        let mut kept: Vec<(usize, AiId, Entity)> = Vec::new();
        for (entity, kind, ai) in pieces {
            match (kind, ai) {
                (PieceKind::Landmark(n), Some(ai)) => kept.push((n, ai, entity)),
                _ => {
                    world.despawn(entity);
                }
            }
        }
        // Landmarks by number; the rest of the geometry anew.
        kept.sort_by_key(|(n, ai, _)| (*n, *ai));
        let wanted = landmarks(&def);
        for (n, landmark) in wanted.iter().enumerate() {
            match kept.iter().find(|(k, ..)| *k == n) {
                Some(&(_, _, entity)) => {
                    let mut e = world.entity_mut(entity);
                    e.insert(Name::new(landmark.name.clone()));
                    place_landmark(&mut e, landmark);
                }
                None => {
                    let piece = LayoutPiece { layout: id.clone(), kind: PieceKind::Landmark(n) };
                    let mut e = world.spawn((piece, Name::new(landmark.name.clone())));
                    place_landmark(&mut e, landmark);
                }
            }
        }
        for &(n, _, entity) in &kept {
            if n >= wanted.len() {
                world.despawn(entity);
            }
        }
        build_geometry(world, &id, &def);
        info!("layout `{id}` rebuilt");
        let mut built = world.get_resource_or_init::<BuiltLayouts>();
        match built.0.iter_mut().find(|(b, _)| *b == id) {
            Some(slot) => slot.1 = def,
            None => built.0.push((id, def)),
        }
    }
    for (id, def) in added {
        build_with_spawns(world, &id, &def);
        info!("layout `{id}` added");
        world.resource_mut::<BuiltLayouts>().0.push((id, def));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_ramp_box_has_its_top_from_one_edge_to_the_other() {
        let ramp = Ramp { from: Vec3::new(70.0, 3.3, 0.0), to: Vec3::new(82.0, 8.0, 0.0), width: 8.0, thickness: 0.5 };
        let (collider, tf) = ramp_box(&ramp);
        let Shape::Cuboid { half } = collider.shape else { panic!() };
        // The top face's low and high edges.
        let top = |z: f32| tf.translation + tf.rotation * Vec3::new(0.0, half.y, z);
        let (a, b) = (top(-half.z), top(half.z));
        let (low, high) = if a.y < b.y { (a, b) } else { (b, a) };
        assert!((low - ramp.from).length() < 1e-3 && (high - ramp.to).length() < 1e-3, "{low} {high}");
        // Across the ramp stays level.
        let across = tf.rotation * Vec3::X;
        assert!(across.y.abs() < 1e-5 && (across.z.abs() - 1.0).abs() < 1e-5, "{across}");
    }

    #[test]
    fn a_landmark_volume_stands_on_its_floor() {
        let v = VolumeBox {
            min: Vec3::new(121.0, 6.0, -3.0),
            max: Vec3::new(129.0, 9.0, 3.0),
            kind: VolumeKind::Trigger { name: "goal".into() },
            landmark: Some("Goal".into()),
        };
        let (collider, _, at) = standing_volume(&v);
        assert_eq!(at, Vec3::new(125.0, 6.0, 0.0));
        assert_eq!(collider.offset, Vec3::new(0.0, 1.5, 0.0));
    }
}
