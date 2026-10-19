//! Hit stop end to end on the whole engine: the content check, the blows (a real swing and the
//! combat events themselves) turned into holds by the table, and what each kind of machine does
//! with a hold: headless nothing, alone with a picture the real-time clock (zero ticks run inside
//! it), in co-op only the pictures. Every test writes its own asset folder and removes it after.

mod content;
mod coop;
mod headless;
mod solo;

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::{json, Value};
use shards::action::{Damaged, HitWeight};
use shards::ai_core::display::Offscreen;
use shards::ai_core::{AiJournal, CallOutcome, JournalKind};
use shards::motion::{character, Collider, Shape};
use shards::prelude::*;
use shards::testing;

/// A light swing that hits 2 m ahead for 20.
pub const ACTIONS: &str = r#"{
    "light_1": (net_id: 1, phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)],
        hits: [(span: Phase("active"), shape: Cone(range: 2.0, angle: 100), damage: 20)]),
}"#;

pub const MOVESETS: &str = r#"{
    "hero": (inputs: {"light": "light_1"}),
    "brute": (inputs: {}),
}"#;

/// The sandbox's ladder, shortened: finisher, break, counter, heavy reaction, hit, block, parry,
/// perfect dodge, and a stagger from a broken poise.
pub const TABLE: &str = r#"{
    "feel": (
        max_seconds: 0.12,
        rules: [
            (name: "finisher", when: (event: Damaged, weight_at_least: Finisher), seconds: 0.10),
            (name: "poise break", when: (event: Damaged, weight_at_least: Break), seconds: 0.10),
            (name: "counter", when: (event: Damaged, weight_at_least: Counter), seconds: 0.08),
            (name: "heavy hit", when: (event: Damaged, reaction_at_least: 2), seconds: 0.08),
            (name: "hit", when: (event: Damaged), seconds: 0.06),
            (name: "block", when: (event: Blocked), seconds: 0.06),
            (name: "parry", when: (event: Parried), seconds: 0.10),
            (name: "perfect dodge", when: (event: PerfectEvaded), seconds: 0.10),
        ],
    ),
}"#;

/// An asset folder of the tests' own, removed when it is dropped.
pub struct Folder(pub PathBuf);

impl Folder {
    /// The folder with the actions, the movesets and the table.
    pub fn new(name: &str) -> Self {
        let folder = Self::empty(name);
        folder.put("content/combat/actions.action.ron", ACTIONS);
        folder.put("content/combat/movesets.moveset.ron", MOVESETS);
        folder.put("content/feel/test.hitstop.ron", TABLE);
        folder
    }

    /// A folder with an empty `content`.
    pub fn empty(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("shards_hitstop_{name}_{}", std::process::id()));
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

/// [`game`], but it counts as drawing offscreen (an image no GPU draws into).
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

/// Runs `ticks` ticks (a stepped game: the clock is paused while they run).
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

/// `ai/status` → `hitstop`.
pub fn status(app: &App) -> Value {
    shards::ai_core::status::status(app.world())["hitstop"].clone()
}

/// The `eN` of an entity.
pub fn id(app: &App, e: Entity) -> String {
    app.world().get::<AiId>(e).expect("an AiId").to_string()
}

/// Journal lines of `kind` that contain `part`.
pub fn lines(app: &App, kind: JournalKind, part: &str) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.filter(|e| e.kind == kind && e.text.contains(part)).map(|e| e.text.clone()).collect()
}

/// A hit that landed with `weight` and reaction tier `reaction`, as the combat module reports it.
pub fn damaged(attacker: Entity, target: Entity, weight: HitWeight, reaction: Option<u8>) -> Damaged {
    Damaged {
        attacker,
        target,
        action: "light_1".into(),
        amount: 10.0,
        poise: 0.0,
        hp_left: 90.0,
        invulnerable: false,
        ignore_reason: None,
        weight,
        reaction,
        absorbed: 0.0,
        source: None,
    }
}

/// One frame of real time per update: the real-time clock a window runs, at 60 Hz.
pub const FRAME: Duration = Duration::from_micros(16_667);
