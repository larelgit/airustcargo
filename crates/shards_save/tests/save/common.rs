//! A small game to save: a hero, chests, a calendar, the engine's own sections beside them.
//! Every test builds it on the whole engine (`shards::testing::test_app`), with a save folder
//! of its own under the temp folder.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use shards::ai_core::{call_tool, AiJournal, CallOutcome, DetRng, JournalKind};
use shards::prelude::*;
use shards::save::{SaveConfig, SaveKind, SaveSection};
use shards::testing;

pub const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/save/assets");

/// A folder of its own under the temp folder, removed when the test ends.
pub struct TempDir(PathBuf);

impl TempDir {
    pub fn new(tag: &str) -> Self {
        static N: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("shards_save_{tag}_{}_{}", std::process::id(), N.fetch_add(1, Ordering::Relaxed)));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }

    /// `world/slot1.save` in the folder.
    pub fn file(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

// --- the game's state -----------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Coins {
    pub gold: u32,
    pub silver: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Item {
    pub name: String,
    pub count: u32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub enum Mode {
    #[default]
    Idle,
    Moving {
        speed: f32,
    },
    Dead(u8),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Extra {
    pub note: String,
    pub weights: Vec<f64>,
}

/// The player's character: a bit of everything a save has to carry.
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Hero {
    pub name: String,
    pub level: u32,
    pub xp: u64,
    pub delta: i64,
    pub pos: [f32; 3],
    pub precise: f64,
    pub tags: Vec<String>,
    pub stats: BTreeMap<String, Vec<f32>>,
    pub extra: Option<Extra>,
    pub flag: Option<bool>,
    pub mode: Mode,
    pub coins: Coins,
    pub items: Vec<Item>,
}

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Chest {
    pub open: bool,
    pub loot: Vec<u32>,
}

#[derive(Resource, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Calendar {
    pub day: u32,
    pub season: String,
    pub omens: BTreeMap<String, i32>,
}

/// What the game keeps in the world (not on the player's body): a tester's lever.
#[derive(Resource, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct Weather(pub String);

pub const CHESTS: usize = 6;

pub fn spawn_world(mut commands: Commands) {
    commands.spawn((AiKind::new("Hero"), AiPlayer, PlayerId(0), Transform::default(), Hero::default(), Name::new("Hero")));
    for n in 0..CHESTS {
        commands.spawn((SaveKey::new(format!("chest.{n}")), Chest::default(), Transform::default()));
    }
}

/// The save registrations of the test game.
pub fn register_saves(app: &mut App) {
    app.save_component::<Hero>(SaveKind::Character, "hero", 1)
        .save_component::<Chest>(SaveKind::World, "chests", 1)
        .save_resource::<Calendar>("calendar", 1);
}

/// The test game on the whole engine, saving into `dir`. `setup` adds what a test needs.
pub fn game_in(dir: &TempDir, setup: impl FnOnce(&mut App)) -> App {
    bare_in(dir, |app| {
        register_saves(app);
        setup(app);
    })
}

/// The test game with no save sections of its own (the engine's are there): `setup` registers
/// the ones a test is about.
pub fn bare_in(dir: &TempDir, setup: impl FnOnce(&mut App)) -> App {
    testing::test_app(
        |config| config.asset_dir = ASSETS.into(),
        |app| {
            app.init_resource::<Calendar>().add_systems(OnEnter(EngineState::InGame), spawn_world);
            app.world_mut().resource_mut::<SaveConfig>().dir = dir.path().to_owned();
            setup(app);
        },
    )
}

pub fn game(dir: &TempDir) -> App {
    game_in(dir, |_| {})
}

// --- calling ----------------------------------------------------------------------------------

pub fn act(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "dev", tool, args)
}

pub fn ok(app: &mut App, tool: &str, args: Value) -> String {
    let out = testing::ok(app, tool, args);
    match out.result {
        Value::String(s) => s,
        other => other.to_string(),
    }
}

pub fn fails(app: &mut App, tool: &str, args: Value) -> String {
    let out = act(app, tool, args.clone());
    assert!(!out.ok, "{tool} {args} should fail, answered: {}", out.result);
    out.result.as_str().unwrap_or_default().to_owned()
}

pub fn step(app: &mut App, ticks: u64) {
    testing::ok(app, "step", json!({"ticks": ticks}));
}

/// The journal lines of every kind, as `tick [kind] text`.
pub fn journal(app: &App) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.map(ToString::to_string).collect()
}

/// Journal lines of one kind containing `needle`.
pub fn lines(app: &App, kind: JournalKind, needle: &str) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.filter(|e| e.kind == kind && e.text.contains(needle)).map(|e| e.text.clone()).collect()
}

pub fn hero_entity(app: &mut App) -> Entity {
    let world = app.world_mut();
    let mut q = world.query_filtered::<Entity, With<Hero>>();
    q.single(world).unwrap()
}

pub fn hero(app: &mut App) -> Hero {
    let e = hero_entity(app);
    app.world().get::<Hero>(e).unwrap().clone()
}

pub fn set_hero(app: &mut App, hero: Hero) {
    let e = hero_entity(app);
    *app.world_mut().get_mut::<Hero>(e).unwrap() = hero;
}

pub fn chests(app: &mut App) -> Vec<(String, Chest)> {
    let world = app.world_mut();
    let mut q = world.query::<(&SaveKey, &Chest)>();
    let mut out: Vec<(String, Chest)> = q.iter(world).map(|(k, c)| (k.0.clone(), c.clone())).collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

pub fn set_chests(app: &mut App, values: &[Chest]) {
    let world = app.world_mut();
    let mut q = world.query::<(&SaveKey, &mut Chest)>();
    for (key, mut chest) in q.iter_mut(world) {
        let n: usize = key.0.trim_start_matches("chest.").parse().unwrap();
        *chest = values[n % values.len()].clone();
    }
}

pub fn calendar(app: &App) -> Calendar {
    app.world().resource::<Calendar>().clone()
}

// --- random states ----------------------------------------------------------------------------

fn finite_f32(rng: &mut DetRng) -> f32 {
    loop {
        let f = f32::from_bits(rng.next_u64() as u32);
        if f.is_finite() {
            return f;
        }
    }
}

fn finite_f64(rng: &mut DetRng) -> f64 {
    loop {
        let f = f64::from_bits(rng.next_u64());
        if f.is_finite() {
            return f;
        }
    }
}

/// A float that is often an edge: zero of either sign, the smallest, subnormal, largest, a
/// round number, a decimal that has no exact form, or any finite bit pattern.
pub fn edge_f32(rng: &mut DetRng) -> f32 {
    match rng.below(12) {
        0 => 0.0,
        1 => -0.0,
        2 => f32::MIN_POSITIVE,
        3 => f32::MAX,
        4 => f32::MIN,
        5 => 1.0e-45,
        6 => 16_777_216.0,
        7 => 0.1,
        8 => -1.0 / 3.0,
        9 | 10 => finite_f32(rng),
        _ => rng.range(-1000.0, 1000.0),
    }
}

pub fn edge_f64(rng: &mut DetRng) -> f64 {
    match rng.below(10) {
        0 => 0.0,
        1 => -0.0,
        2 => f64::MIN_POSITIVE,
        3 => f64::MAX,
        4 => 0.1 + 0.2,
        5 => 5e-324,
        6 | 7 => finite_f64(rng),
        _ => f64::from(rng.f32()) * 1000.0 + f64::from(rng.f32()) * 1e-9,
    }
}

pub fn edge_u64(rng: &mut DetRng) -> u64 {
    match rng.below(6) {
        0 => 0,
        1 => u64::MAX,
        2 => 1 << 53,
        3 => (1 << 53) + 1,
        _ => rng.next_u64(),
    }
}

pub fn edge_i64(rng: &mut DetRng) -> i64 {
    match rng.below(5) {
        0 => i64::MIN,
        1 => i64::MAX,
        2 => -1,
        _ => rng.next_u64() as i64,
    }
}

const PALETTE: [char; 22] =
    ['a', 'Z', '0', ' ', 'é', 'ß', '日', '本', '😀', '\n', '\t', '"', '\\', '/', '\u{0}', '\u{1f}', '\u{7f}', '\u{2028}', '\u{fffd}', '𝄞', '[', '{'];

pub fn random_string(rng: &mut DetRng) -> String {
    (0..rng.below(12)).map(|_| PALETTE[rng.below(PALETTE.len() as u32) as usize]).collect()
}

pub fn random_hero(rng: &mut DetRng) -> Hero {
    let stats = (0..rng.below(4)).map(|_| (random_string(rng), (0..rng.below(5)).map(|_| edge_f32(rng)).collect())).collect();
    Hero {
        name: random_string(rng),
        level: rng.next_u64() as u32,
        xp: edge_u64(rng),
        delta: edge_i64(rng),
        pos: [edge_f32(rng), edge_f32(rng), edge_f32(rng)],
        precise: edge_f64(rng),
        tags: (0..rng.below(6)).map(|_| random_string(rng)).collect(),
        stats,
        extra: rng.chance(0.5).then(|| Extra { note: random_string(rng), weights: (0..rng.below(4)).map(|_| edge_f64(rng)).collect() }),
        flag: match rng.below(3) {
            0 => None,
            1 => Some(true),
            _ => Some(false),
        },
        mode: match rng.below(3) {
            0 => Mode::Idle,
            1 => Mode::Moving { speed: edge_f32(rng) },
            _ => Mode::Dead(rng.below(256) as u8),
        },
        coins: Coins { gold: rng.next_u64() as u32, silver: rng.below(100) },
        items: (0..rng.below(5)).map(|_| Item { name: random_string(rng), count: rng.below(1000) }).collect(),
    }
}

pub fn random_chests(rng: &mut DetRng) -> Vec<Chest> {
    (0..CHESTS).map(|_| Chest { open: rng.chance(0.5), loot: (0..rng.below(5)).map(|_| rng.next_u64() as u32).collect() }).collect()
}

pub fn random_calendar(rng: &mut DetRng) -> Calendar {
    Calendar {
        day: rng.next_u64() as u32,
        season: random_string(rng),
        omens: (0..rng.below(5)).map(|_| (random_string(rng), rng.next_u64() as i32)).collect(),
    }
}

// --- the evolving schema of the `inventory` section (migration tests) -----------------------------

/// Version 3 of the inventory, the one this game reads now: coins and counted items.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct InventoryV3 {
    pub coins: Coins,
    pub items: Vec<Item>,
}

impl SaveSection for InventoryV3 {
    const NAME: &'static str = "inventory";
    const VERSION: u32 = 3;
    const KIND: SaveKind = SaveKind::Character;

    fn capture(world: &World, body: Option<Entity>) -> Result<Self, String> {
        let hero = world.get::<Hero>(body.ok_or("no body")?).ok_or("no hero")?;
        Ok(InventoryV3 { coins: hero.coins.clone(), items: hero.items.clone() })
    }

    fn apply(self, world: &mut World, body: Option<Entity>) -> Result<(), String> {
        let mut hero = world.get_mut::<Hero>(body.ok_or("no body")?).ok_or("no hero")?;
        hero.coins = self.coins;
        hero.items = self.items;
        Ok(())
    }
}

/// A world section that is newer than the old samples: they lack it, so `missing` runs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct WeatherSave(pub String);

impl SaveSection for WeatherSave {
    const NAME: &'static str = "weather";
    const VERSION: u32 = 1;
    const KIND: SaveKind = SaveKind::World;

    fn capture(world: &World, _: Option<Entity>) -> Result<Self, String> {
        Ok(WeatherSave(world.resource::<Weather>().0.clone()))
    }

    fn apply(self, world: &mut World, _: Option<Entity>) -> Result<(), String> {
        world.insert_resource(Weather(self.0));
        Ok(())
    }

    fn missing(world: &mut World, _: Option<Entity>) -> Result<(), String> {
        world.insert_resource(Weather("clear".to_owned()));
        Ok(())
    }
}

/// Version 1 → 2: `items` were names, now counted.
pub fn inventory_1_to_2(old: Value) -> Result<Value, String> {
    let mut gold = old.get("gold").cloned().ok_or("version 1 has no gold")?;
    if gold.is_null() {
        gold = json!(0);
    }
    let mut counted: BTreeMap<String, u32> = BTreeMap::new();
    for name in old.get("items").and_then(Value::as_array).ok_or("version 1 has no items")? {
        *counted.entry(name.as_str().ok_or("an item is not a name")?.to_owned()).or_default() += 1;
    }
    let items: Vec<Value> = counted.into_iter().map(|(name, count)| json!({"name": name, "count": count})).collect();
    Ok(json!({"gold": gold, "items": items}))
}

/// Version 2 → 3: gold became coins.
pub fn inventory_2_to_3(old: Value) -> Result<Value, String> {
    Ok(json!({"coins": {"gold": old["gold"], "silver": 0}, "items": old["items"]}))
}

/// The game as of version 3 of the inventory: its migrations registered.
pub fn register_inventory(app: &mut App) {
    app.add_save_section::<InventoryV3>()
        .add_save_migration("inventory", 1, inventory_1_to_2)
        .add_save_migration("inventory", 2, inventory_2_to_3)
        .insert_resource(Weather("rain".into()))
        .add_save_section::<WeatherSave>();
}
