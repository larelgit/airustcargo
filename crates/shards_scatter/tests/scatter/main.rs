//! Scatter end to end on the whole engine: a 2×2 region of test tiles (a field, a column, a
//! mound, a wall) with three scatters. Headless nothing is strewn; on the headless engine counted
//! as drawn (its render plugins stay registered without a GPU) the instances, where they stand
//! and where not, their levels of detail, looks and batches, the same every run, what counts as
//! ground and as a solid, and what `graphics.foliage`, `scatter_set`, a content change (a scatter's
//! or a tile's tags) and a region change do. `offscreen` draws
//! them on a GPU and reads the pixels back (`--ignored`). Every test writes its own asset folder
//! and removes it after.

mod offscreen;
mod strewing;

use std::path::{Path, PathBuf};

use serde_json::Value;
use shards::ai_core::display::Offscreen;
use shards::ai_core::CallOutcome;
use shards::prelude::*;
use shards::scatter::{ScatterInstance, ScatterRoot};
use shards::testing;

/// Four 16 m tiles whose ground is y = 0: an open field; a column 2 m across and 4 m tall in
/// the middle; a mound (a cone 4 m across its foot in the ground, rising 1 m above it, its side
/// 26.6° steep); a wall 12 m long, 0.6 m thick and 3 m tall across the middle (along x).
const TILES: &str = r#"{
    "field": (
        sockets: (n: "g", e: "g", s: "g", w: "g"),
        slots: [(at: (0.0, 0.0, 0.0), accepts: ["spawn"])],
        geometry: Inline([Box(min: (-8.0, -1.0, -8.0), max: (8.0, 0.0, 8.0))]),
    ),
    "column": (
        sockets: (n: "g", e: "g", s: "g", w: "g"),
        tags: ["stone"],
        geometry: Inline([
            Box(min: (-8.0, -1.0, -8.0), max: (8.0, 0.0, 8.0)),
            Solid(shape: Cuboid(half: (1.0, 2.0, 1.0)), at: (0.0, 2.0, 0.0)),
        ]),
    ),
    "mound": (
        sockets: (n: "g", e: "g", s: "g", w: "g"),
        geometry: Inline([
            Box(min: (-8.0, -1.0, -8.0), max: (8.0, 0.0, 8.0)),
            Solid(shape: Cone(half_height: 1.0, radius: 4.0), at: (0.0, 0.0, 0.0)),
        ]),
    ),
    "wall": (
        sockets: (n: "g", e: "g", s: "g", w: "g"),
        tags: ["stone"],
        geometry: Inline([
            Box(min: (-8.0, -1.0, -8.0), max: (8.0, 0.0, 8.0)),
            Solid(shape: Cuboid(half: (6.0, 1.5, 0.3)), at: (0.0, 1.5, 0.0)),
        ]),
    ),
}"#;

const REGION: &str = r#"{
    "plain": (
        size: (2, 2),
        tile_size: 16.0,
        version: 1,
        fixed: [
            (at: (0, 0), tile: "field", rot: 0),
            (at: (1, 0), tile: "column", rot: 0),
            (at: (0, 1), tile: "mound", rot: 0),
            (at: (1, 1), tile: "wall", rot: 0),
        ],
        pois: [(poi: "spawn", cells: [(0, 0)])],
    ),
}"#;

const WORLD: &[(&str, &str)] = &[
    ("content/world/test.tile.ron", TILES),
    ("content/world/test.region.ron", REGION),
    ("content/world/test.poi.ron", r#"{ "spawn": () }"#),
    ("content/world/test.socket_rules.ron", r#"{ "default": (kinds: ["g"], passable: ["g"]) }"#),
];

/// The looks: two greens and a grey, none with an outline.
pub const LOOKS: &str = r##"{
    "green": (color: Some("#2f8f3a"), shading: Matte),
    "olive": (color: Some("#7a8f2f"), shading: Matte),
    "grey": (color: Some("#808080"), flat: true),
}"##;

/// Tufts everywhere (two looks, two levels of detail); stones on the column and wall tiles (by
/// tag) and the mound, flat ground only, half a meter clear of solids, casting shadows; moss on
/// the mound only, up its side.
pub const SCATTERS: &str = r##"{
    "tufts": (
        density: 1.0,
        spacing: 0.3,
        scale: (0.8, 1.2),
        tilt: 10.0,
        looks: ["green", "olive"],
        fade: 2.0,
        lods: [
            (mesh: Blades(width: 0.08, height: 0.4, count: 5), until: 20.0),
            (mesh: Blades(width: 0.12, height: 0.4, count: 2), until: 40.0),
        ],
    ),
    "stones": (
        tiles: ["#stone", "mound"],
        density: 0.5,
        max_slope: 20.0,
        clearance: 0.5,
        looks: ["grey"],
        shadows: true,
        lods: [(mesh: Rock(radius: 0.2, detail: 1, squash: 0.6), until: 50.0)],
    ),
    "moss": (
        tiles: ["mound"],
        density: 2.0,
        max_slope: 40.0,
        height: (-0.5, 1.5),
        looks: ["green"],
        lods: [(mesh: Rock(radius: 0.1, detail: 0), until: 30.0)],
    ),
}"##;

/// An asset folder of the tests' own, removed when it is dropped.
pub struct Folder(pub PathBuf);

impl Folder {
    /// The folder with the test region, [`LOOKS`] and [`SCATTERS`].
    pub fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("shards_scatter_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("content")).unwrap();
        let folder = Self(dir);
        for (file, text) in WORLD {
            folder.put(file, text);
        }
        folder.put("content/look/test.look.ron", LOOKS);
        folder.put("content/world/test.scatter.ron", SCATTERS);
        folder
    }

    /// Writes `text` to `file` under the folder.
    pub fn put(&self, file: &str, text: &str) {
        let path = self.0.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The engine on `assets` with region `plain` (seed 7), headless and paused at tick 0: nothing
/// is drawn.
pub fn game(assets: &Folder) -> App {
    build(assets, 7, false)
}

/// [`game`], but it counts as drawing offscreen (an image no GPU draws into): the scatter is
/// strewn on `update`, without a GPU.
pub fn drawn(assets: &Folder) -> App {
    build(assets, 7, true)
}

pub fn build(assets: &Folder, seed: u64, drawn: bool) -> App {
    let app = unchecked(assets, seed, drawn);
    let errors = content_errors(&app);
    assert!(errors.is_empty(), "test content has errors: {errors:#?}");
    app
}

/// [`build`], whatever errors the content has.
pub fn unchecked(assets: &Folder, seed: u64, drawn: bool) -> App {
    let dir = assets.path().to_string_lossy().into_owned();
    testing::test_app(
        move |config| (config.asset_dir, config.seed) = (dir, seed),
        |app| {
            app.start_region("plain");
            if drawn {
                app.insert_resource(Offscreen { image: Handle::default(), size: UVec2::new(320, 240) });
            }
        },
    )
}

pub fn content_errors(app: &App) -> Vec<String> {
    app.world().resource::<shards::content::ContentDb>().errors().map(ToString::to_string).collect()
}

/// The `scatter` tool's lines, without the last pass's (its time differs from run to run).
pub fn scatter_lines(app: &mut App) -> Vec<String> {
    ok(app, "scatter", serde_json::json!({})).lines().filter(|l| !l.starts_with("last pass: ")).map(str::to_owned).collect()
}

/// The text of a call that must succeed.
pub fn ok(app: &mut App, tool: &str, args: Value) -> String {
    text(&testing::ok(app, tool, args))
}

pub fn text(out: &CallOutcome) -> String {
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

/// `ai/status` → `scatter`.
pub fn status(app: &App) -> Value {
    shards::ai_core::status::status(app.world())["scatter"].clone()
}

/// One level of detail of one instance, as it stands in the world.
#[derive(Clone, Debug, PartialEq)]
pub struct Seen {
    pub scatter: String,
    pub look: String,
    /// The tile's cell.
    pub cell: (i32, i32),
    pub lod: u8,
    /// In the tile's frame (its center at the origin).
    pub local: Vec3,
    pub transform: Transform,
    pub entity: Entity,
}

/// Every mesh entity strewn, in cell, scatter, look, level and position order.
pub fn seen(app: &mut App) -> Vec<Seen> {
    let world = app.world_mut();
    let mut out: Vec<Seen> = world
        .query::<(Entity, &ScatterInstance, &Transform, &ChildOf)>()
        .iter(world)
        .map(|(entity, instance, tf, parent)| {
            let root = world.get::<ScatterRoot>(parent.parent()).expect("an instance hangs under its scatter's root");
            Seen {
                scatter: root.scatter.clone(),
                look: root.look.clone(),
                cell: (root.tile.x, root.tile.y),
                lod: instance.lod,
                local: tf.translation,
                transform: *tf,
                entity,
            }
        })
        .collect();
    out.sort_by(|a, b| {
        (a.cell, &a.scatter, &a.look, a.lod)
            .cmp(&(b.cell, &b.scatter, &b.look, b.lod))
            .then(a.local.x.total_cmp(&b.local.x))
            .then(a.local.z.total_cmp(&b.local.z))
    });
    out
}

/// The nearest level of each instance of `scatter` on `cell` (its position in the tile's frame).
pub fn instances(seen: &[Seen], scatter: &str, cell: (i32, i32)) -> Vec<Vec3> {
    seen.iter().filter(|s| s.scatter == scatter && s.cell == cell && s.lod == 0).map(|s| s.local).collect()
}
