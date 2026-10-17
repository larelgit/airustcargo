//! The level, as content: a content kind of the game's own (`arena`, files
//! `content/**/*.arena.ron`) and the system that builds the world from it.
//!
//! This is the pattern for anything with numbers: a struct with serde derives, a
//! `ContentDef` impl that checks it (and its references to other content, so a typo is an
//! error with a file and a line instead of a crash), `add_content` to register it.
//! Gameplay reads it from the `ContentDb`; the engine hot-reloads and replays it.

use shards::content::spawn_prefab;
use shards::motion::Shape;
use shards::prelude::*;

/// The arena the game builds.
pub const ARENA: &str = "yard";

/// A level: a floor, walls, and who stands where at the start.
///
/// The serde path points at the engine's re-export, so the game needs no dependency of
/// its own on serde. `deny_unknown_fields` turns a typo in a file into an error with its line.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
pub struct ArenaDef {
    /// The floor is a square this far (m) from the center in every direction, fenced all round.
    pub half: f32,
    /// Obstacles standing on the floor.
    #[serde(default)]
    pub walls: Vec<WallDef>,
    /// Characters, spawned in this order (so their ids are the same on every run).
    #[serde(default)]
    pub spawns: Vec<SpawnDef>,
}

/// A box standing on the floor.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
pub struct WallDef {
    /// Center of its footprint: (x east, z south), meters.
    pub at: (f32, f32),
    /// Half extents: (x, height, z), meters.
    pub half: (f32, f32, f32),
}

/// A character placed at the start.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
pub struct SpawnDef {
    /// What a world save knows this character by (`SaveKey` `yard.<key>`, see `progress::Fallen`).
    /// It stays with the character when spawns are added, removed or reordered; never give an
    /// old key to a new character, or a save made before would put it down.
    pub key: String,
    /// The prefab to spawn (`characters.prefab.ron`).
    pub prefab: String,
    /// Where its feet stand: (x east, z south), meters.
    pub at: (f32, f32),
    /// Which way it faces, degrees counterclockwise from north (-Z). Default 0.
    #[serde(default)]
    pub yaw_deg: f32,
}

impl ContentDef for ArenaDef {
    const KIND: &'static str = "arena";

    fn validate(&self, _id: &str, refs: &mut Refs) {
        if !(self.half > 0.0 && self.half.is_finite()) {
            refs.error(format!("half must be > 0 (is {})", self.half));
        }
        for (i, wall) in self.walls.iter().enumerate() {
            let (x, y, z) = wall.half;
            if ![x, y, z].iter().all(|v| *v > 0.0 && v.is_finite()) {
                refs.error(format!("walls[{i}].half must be positive in all three (is {x}, {y}, {z})"));
            }
        }
        for (i, spawn) in self.spawns.iter().enumerate() {
            // A prefab that does not exist is an error naming this field, with the closest id.
            refs.require::<PrefabDef>(&spawn.prefab, &format!("spawns[{i}].prefab"));
            if spawn.key.trim().is_empty() {
                refs.error(format!("spawns[{i}].key is empty: name the character for saves (`key: \"gate_grunt\"`)"));
            } else if let Some(first) = self.spawns[..i].iter().position(|s| s.key == spawn.key) {
                refs.error(format!("spawns[{i}].key `{}` is the key of spawns[{first}] too: a save names one character by it", spawn.key));
            }
        }
    }
}

/// Registers the `arena` content kind and builds the arena once the world may exist.
pub struct ArenaPlugin;

impl Plugin for ArenaPlugin {
    fn build(&self, app: &mut App) {
        app.add_content::<ArenaDef>()
            // The code names `yard`: content without it has an error saying who needs it.
            .require_content::<ArenaDef>(&[ARENA], "my_game")
            // Content exists only after `EngineState::Loading`, so the world is spawned here,
            // never in `Startup`.
            .add_systems(OnEnter(EngineState::InGame), spawn_arena);
    }
}

/// Builds the floor, the characters and the walls. Order matters for ids: characters come
/// first (they are `e1`, `e2`, ...), then the walls.
fn spawn_arena(world: &mut World) {
    let Some(arena) = world.resource::<ContentDb>().get::<ArenaDef>(ARENA).cloned() else {
        error!("the arena `{ARENA}` is not in content/**/*.arena.ron; there is no world to play in");
        return;
    };

    // Static geometry: the top of the floor is y = 0, and characters stand on it.
    let half = arena.half;
    world.spawn((Name::new("Floor"), Collider::new(Shape::cuboid(Vec3::new(half, 0.5, half))), Transform::from_xyz(0.0, -0.5, 0.0)));
    // The fence: invisible to the AI (no id), only keeps characters on the floor.
    let height = 2.0;
    for (x, z, hx, hz) in [(0.0, -half, half, 0.5), (0.0, half, half, 0.5), (-half, 0.0, 0.5, half), (half, 0.0, 0.5, half)] {
        world.spawn((Collider::standing_box(Vec3::new(hx, height, hz)), Transform::from_xyz(x, 0.0, z)));
    }

    for spawn in &arena.spawns {
        let at = Vec3::new(spawn.at.0, 0.0, spawn.at.1);
        let transform = Transform::from_translation(at).with_rotation(Quat::from_rotation_y(spawn.yaw_deg.to_radians()));
        match spawn_prefab(world, &spawn.prefab, transform, None) {
            // What a world save knows it by (`progress::Fallen`): its key in content, which stays
            // with it whatever else the content changes.
            Ok(character) => {
                world.entity_mut(character).insert(SaveKey::new(format!("{ARENA}.{}", spawn.key)));
            }
            Err(why) => error!("arena `{ARENA}`: cannot spawn {}: {why}", spawn.prefab),
        }
    }

    for wall in &arena.walls {
        let (hx, hy, hz) = wall.half;
        world.spawn((
            // Landmarks are listed on one line of `observe` (`LANDMARKS e3 Wall 7.4m W · …`).
            AiKind::new("Wall"),
            AiLandmark,
            Collider::standing_box(Vec3::new(hx, hy, hz)),
            Transform::from_xyz(wall.at.0, 0.0, wall.at.1),
        ));
    }
}
