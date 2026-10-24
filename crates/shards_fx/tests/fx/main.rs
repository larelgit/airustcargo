//! Effects end to end on the whole engine: the content check, the tools, the status and the
//! developer menu headless (nothing is drawn, nothing plays); on the headless engine counted as
//! drawn (its render plugins stay registered without a GPU) the pools, the particles a play or an
//! event starts and the clock they live by, and the telegraph decals over the zones of attacks.
//! `offscreen` draws them on a GPU and reads the pixels back (`--ignored`). Every test writes its
//! own asset folder and removes it after.

mod content;
mod decals;
mod gore;
mod offscreen;
mod offscreen_gore;
mod particles;
mod tools;

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use shards::ai_core::display::Offscreen;
use shards::ai_core::CallOutcome;
use shards::fx::{FxParticle, Particles};
use shards::motion::{character, Collider, Shape};
use shards::prelude::*;
use shards::testing;

/// A slam (a 3 m circle round the attacker, telegraphed), a sweep (a cone ahead) and a light
/// swing without a telegraph.
pub const ACTIONS: &str = r#"{
    "light_1": (net_id: 1, phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)],
        hits: [(span: Phase("active"), shape: Cone(range: 2.0, angle: 100), damage: 20)]),
    "slam": (net_id: 2, phases: [("windup", 0.8), ("active", 0.1), ("recovery", 0.3)],
        hits: [(span: Phase("active"), shape: Circle(radius: 3.0), damage: 20)],
        telegraph: (symbol: "⚠")),
    "sweep": (net_id: 3, phases: [("windup", 0.6), ("active", 0.1), ("recovery", 0.3)],
        hits: [(span: Phase("active"), shape: Cone(range: 4.0, angle: 90), damage: 20)],
        telegraph: (symbol: "!")),
}"#;

pub const MOVESETS: &str = r#"{
    "hero": (inputs: {"light": "light_1"}),
    "brute": (inputs: {"light": "slam", "heavy": "sweep"}),
}"#;

/// Sparks that burst out of the struck side, dust that goes on rising for half a second, a glow.
pub const EFFECTS: &str = r##"{
    "spark": (count: 12, lifetime: (0.2, 0.4), speed: (2.0, 4.0), aim: Hit, color: "#fff2b3", color_end: "#ff6a1a00",
        glow: true, offset: (0.0, 1.0, 0.0), max_live: 32),
    "dust": (count: 6, rate: 20.0, duration: 0.5, lifetime: (0.6, 0.8), shape: Cube, gravity: -2.0, max_live: 24),
    "glow": (count: 4, lifetime: (0.5, 0.5), color: "#7dff9e", max_live: 8),
}"##;

/// The table: hits spark, deaths raise dust, heals glow, and so does a clip's `swoosh`.
pub const FX: &str = r#"{"test": (effects: {"damaged": "spark", "died": "dust", "healed": "glow"}, cues: {"swoosh": "glow"})}"#;

/// An asset folder of the tests' own, removed when it is dropped.
pub struct Folder(pub PathBuf);

impl Folder {
    /// The folder with the actions, the movesets, the effects and the table.
    pub fn new(name: &str) -> Self {
        let folder = Self::empty(name);
        folder.put("content/combat/actions.action.ron", ACTIONS);
        folder.put("content/combat/movesets.moveset.ron", MOVESETS);
        folder.put("content/fx/test.effect.ron", EFFECTS);
        folder.put("content/fx/test.fx.ron", FX);
        folder
    }

    /// A folder with an empty `content`.
    pub fn empty(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("shards_fx_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("content")).unwrap();
        Self(dir)
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

/// The engine on `assets`, headless and paused at tick 0: nothing is drawn. A floor.
pub fn game(assets: &Folder) -> App {
    let dir = assets.path().to_string_lossy().into_owned();
    let mut app = testing::test_app(move |config| config.asset_dir = dir, |_| {});
    floor(app.world_mut());
    app
}

/// [`game`], but it counts as drawing offscreen (an image no GPU draws into): the effect systems
/// run on `update`, without a GPU.
pub fn drawn(assets: &Folder) -> App {
    let dir = assets.path().to_string_lossy().into_owned();
    let mut app = testing::test_app(
        move |config| config.asset_dir = dir,
        |app| {
            app.insert_resource(Offscreen { image: Handle::default(), size: UVec2::new(320, 240) });
        },
    );
    floor(app.world_mut());
    app
}

pub fn floor(world: &mut World) -> Entity {
    world.spawn((Collider::new(Shape::cuboid(Vec3::new(30.0, 0.5, 30.0))), Transform::from_xyz(0.0, -0.5, 0.0))).id()
}

/// A fighter with `moveset` on `team` at `at`, facing `facing`: `hp` health.
pub fn fighter(world: &mut World, moveset: &str, team: &str, at: Vec3, facing: Vec3, hp: f32) -> Entity {
    let (controller, intent, transform) = character("", at);
    world
        .spawn((
            Name::new(moveset.to_owned()),
            AiKind::new(moveset.to_owned()),
            Fighter::new(moveset),
            Health::new(hp),
            Stamina::new(100.0, 30.0, 0.8),
            Team::new(team),
            controller,
            intent,
            transform.looking_to(facing, Vec3::Y),
        ))
        .id()
}

/// A key press between ticks.
pub fn press(app: &mut App, e: Entity, input: &str) {
    app.world_mut().get_mut::<Intent>(e).unwrap().press(input);
}

/// Runs `ticks` ticks.
pub fn step(app: &mut App, ticks: u64) {
    ok(app, "step", json!({ "ticks": ticks }));
}

/// The text of a call that must succeed.
pub fn ok(app: &mut App, tool: &str, args: Value) -> String {
    text(&testing::ok(app, tool, args))
}

/// The error of a call that must fail.
pub fn refused(app: &mut App, tool: &str, args: Value) -> String {
    let out = testing::call(app, tool, args);
    assert!(!out.ok, "{tool} should fail: {}", text(&out));
    text(&out)
}

pub fn text(out: &CallOutcome) -> String {
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

/// `ai/status` → `fx`.
pub fn status(app: &App) -> Value {
    shards::ai_core::status::status(app.world())["fx"].clone()
}

/// The `eN` of an entity.
pub fn id(app: &App, e: Entity) -> String {
    app.world().get::<AiId>(e).expect("an AiId").to_string()
}

/// Particles of `effect` alive now (the pool's count).
pub fn live(app: &App, effect: &str) -> usize {
    app.world().resource::<Particles>().effects.get(effect).map_or(0, |p| p.pool.live())
}

/// Meshes of `effect`'s pool, and how many of them are shown.
pub fn meshes(app: &mut App, effect: &str) -> (usize, usize) {
    let world = app.world_mut();
    let mut query = world.query::<(&FxParticle, &Visibility)>();
    let mine: Vec<bool> = query.iter(world).filter(|(p, _)| p.effect == effect).map(|(_, v)| *v != Visibility::Hidden).collect();
    (mine.len(), mine.iter().filter(|shown| **shown).count())
}
