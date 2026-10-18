//! Generation: steps and checks registered by name, run over a [`VoxelVolume`] by [`generate`].
//!
//! **A step is game code** the content hash cannot see, so it is registered with a **version**
//! (`app.add_voxel_step("hills", 1, hills)`) and a region's content names the version it
//! expects (`steps: [("hills", 1)]`). A step set whose versions differ from the registry's is a
//! content error ([`VoxelSteps::validate_steps`]), found when content loads, not on the first
//! `regen`. Changing what a step does means raising its registered version (and the content's).
//!
//! **Generation is a pure function of (seed, version, attempt, the step list with versions,
//! params, palette)**: every random number comes from [`DetRng`] keyed by those and the step's
//! place in the list ([`VoxelGen::rng`], [`VoxelGen::rng_keyed`]), every height from [`Noise`] ([`VoxelGen::noise`], keyed by a salt instead of the place); never
//! `SimRng`, never a clock, never the iteration order of a hash map (params and marks are
//! ordered). The same input gives the same volume, bit for bit, on every machine of one
//! architecture, and [`Generated::hash`] says so in one number.
//!
//! **Checks** run after the steps (the arena is flat, the spawn is on ground and not under
//! water). A failed step or check fails the *attempt*; the next attempt has other rng keys. After
//! `attempts` the generation fails with every attempt's reason.

use std::collections::{BTreeMap, BTreeSet};

use ai_core::DetRng;
use bevy::prelude::*;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use serde_json::Value;

use crate::noise::Noise;
use crate::palette::VoxelPalette;
use crate::volume::{Fnv, VoxelVolume, AIR};

/// The most attempts a generation may ask for.
pub const MAX_ATTEMPTS: u32 = 64;

/// Domain tags: the first key of each kind of stream, so a step's stream, a keyed stream and a
/// noise field never meet whatever the keys after them are.
const STREAM_STEP: u64 = 0x7374_6570;
const STREAM_KEYED: u64 = 0x006b_6579_6564;
const STREAM_NOISE: u64 = 0x006e_6f69_7365;

/// A generation step: edits the volume, may fail the attempt with a reason.
pub type StepFn = fn(&mut VoxelGen) -> Result<(), String>;
/// A check of the finished volume: `Err` with a reason fails the attempt.
pub type CheckFn = fn(&VoxelGen) -> Result<(), String>;

/// The registered steps (with versions) and checks. A resource: the plugin adds it with the
/// engine's stock steps, a game adds its own at build time (`add_voxel_step`). Code registered
/// at build time, not simulation state.
#[derive(Resource, Clone, Debug)]
pub struct VoxelSteps {
    steps: BTreeMap<String, (u32, StepFn)>,
    checks: BTreeMap<String, CheckFn>,
}

impl Default for VoxelSteps {
    /// The registry with the stock steps ([`noise_hills`], [`fill_water`]).
    fn default() -> Self {
        let mut steps = Self { steps: BTreeMap::new(), checks: BTreeMap::new() };
        steps.add_step("noise_hills", NOISE_HILLS_VERSION, noise_hills).expect("stock names are free");
        steps.add_step("fill_water", FILL_WATER_VERSION, fill_water).expect("stock names are free");
        steps
    }
}

impl VoxelSteps {
    /// Registers step `name` at `version` (at least 1). A name already taken, a name that is not
    /// a lowercase word (`a-z`, `0-9`, `_`) and a version 0 are errors.
    pub fn add_step(&mut self, name: &str, version: u32, run: StepFn) -> Result<(), String> {
        check_name("step", name)?;
        if version == 0 {
            return Err(format!("voxel step `{name}`: versions start at 1"));
        }
        if self.steps.contains_key(name) {
            return Err(format!("voxel step `{name}` is registered twice"));
        }
        self.steps.insert(name.to_owned(), (version, run));
        Ok(())
    }

    /// Registers check `name`. Same rules for the name as a step's; a second check of the same
    /// name is an error.
    pub fn add_check(&mut self, name: &str, run: CheckFn) -> Result<(), String> {
        check_name("check", name)?;
        if self.checks.contains_key(name) {
            return Err(format!("voxel check `{name}` is registered twice"));
        }
        self.checks.insert(name.to_owned(), run);
        Ok(())
    }

    /// The registered version of step `name`.
    pub fn version(&self, name: &str) -> Option<u32> {
        self.steps.get(name).map(|(v, _)| *v)
    }

    /// The step names, in name order.
    pub fn step_names(&self) -> Vec<&str> {
        self.steps.keys().map(String::as_str).collect()
    }

    /// The check names, in name order.
    pub fn check_names(&self) -> Vec<&str> {
        self.checks.keys().map(String::as_str).collect()
    }

    /// Everything wrong with a step list `(name, version)` against the registry, one line each:
    /// an unknown step (with the closest name), a version other than the registered one, a step
    /// listed twice is fine (it runs twice, with other rng keys), an empty list is an error.
    pub fn validate_steps(&self, steps: &[(String, u32)]) -> Vec<String> {
        let mut out = Vec::new();
        if steps.is_empty() {
            out.push("steps: a region lists at least one step".to_owned());
        }
        for (i, (name, version)) in steps.iter().enumerate() {
            match self.steps.get(name) {
                None => {
                    let mut message = format!("steps[{i}]: unknown voxel step `{name}`");
                    if let Some(close) = shards_content::db::closest(name, self.steps.keys().map(String::as_str)) {
                        message.push_str(&format!(" (did you mean `{close}`?)"));
                    }
                    if self.steps.is_empty() {
                        message.push_str(" (no step is registered: `add_voxel_step`)");
                    }
                    out.push(message);
                }
                Some((registered, _)) if registered != version => {
                    out.push(format!("steps[{i}]: voxel step `{name}` is at version {registered} in this build, the content expects {version}: set the version in the content to {registered}, or register {version} in the game"));
                }
                Some(_) => {}
            }
        }
        out
    }

    /// Like [`validate_steps`](Self::validate_steps) for the checks a region names.
    pub fn validate_checks(&self, checks: &[String]) -> Vec<String> {
        let mut out = Vec::new();
        for (i, name) in checks.iter().enumerate() {
            if !self.checks.contains_key(name) {
                let mut message = format!("checks[{i}]: unknown voxel check `{name}`");
                if let Some(close) = shards_content::db::closest(name, self.checks.keys().map(String::as_str)) {
                    message.push_str(&format!(" (did you mean `{close}`?)"));
                }
                out.push(message);
            }
        }
        out
    }
}

fn check_name(what: &str, name: &str) -> Result<(), String> {
    let ok = !name.is_empty() && name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_');
    if ok { Ok(()) } else { Err(format!("voxel {what} name `{name}`: lowercase letters, digits and `_` only")) }
}

/// A hash of a step list with versions, in list order: it goes into [`Generated::hash`], so the
/// same cells made by another step set are another world.
pub fn step_set_hash(steps: &[(String, u32)]) -> u64 {
    let mut h = Fnv::default();
    for (name, version) in steps {
        h.bytes(name.as_bytes());
        h.bytes(&[0]);
        h.bytes(&version.to_le_bytes());
    }
    h.0
}

/// Folds the palette ([`VoxelPalette::fingerprint`]) and a step list into a volume hash: the hash
/// of a generated world.
pub fn world_hash(volume_hash: u64, palette: &VoxelPalette, steps: &[(String, u32)]) -> u64 {
    let mut h = Fnv::default();
    h.bytes(&volume_hash.to_le_bytes());
    h.bytes(&palette.fingerprint().to_le_bytes());
    h.bytes(&step_set_hash(steps).to_le_bytes());
    h.0
}

/// A named point a step leaves in the world (a spawn, a point of interest), in world metres.
#[derive(Clone, Debug, PartialEq)]
pub struct Mark {
    pub name: String,
    pub pos: Vec3,
}

/// What to generate. Every field is an input of the result.
#[derive(Clone, Debug)]
pub struct GenSpec {
    /// Cells along each axis (a whole number of `chunk`s).
    pub size: UVec3,
    /// The edge of a cell, m.
    pub block: f32,
    /// World position of the least corner of cell (0, 0, 0).
    pub origin: Vec3,
    /// Cells per chunk.
    pub chunk: UVec3,
    pub seed: u64,
    /// The region's version (the generator's output is a function of it).
    pub version: u32,
    /// How many attempts before giving up (1 to [`MAX_ATTEMPTS`]).
    pub attempts: u32,
    /// The steps, in order, with the version the content expects of each.
    pub steps: Vec<(String, u32)>,
    /// Each step's parameters by step name (read with [`VoxelGen::params`]).
    pub params: BTreeMap<String, Value>,
    /// The checks to run after the steps, in order.
    pub checks: Vec<String>,
}

impl GenSpec {
    /// A spec with one attempt, no params and no checks.
    pub fn new(size: UVec3, chunk: UVec3, seed: u64, steps: &[(&str, u32)]) -> Self {
        Self {
            size,
            block: 1.0,
            origin: Vec3::ZERO,
            chunk,
            seed,
            version: 1,
            attempts: 1,
            steps: steps.iter().map(|(n, v)| ((*n).to_owned(), *v)).collect(),
            params: BTreeMap::new(),
            checks: Vec::new(),
        }
    }

    /// Everything wrong with the spec against `steps` and `palette`, one line each: the volume's
    /// shape, the attempts, the step list, the checks, a `params` key that names no listed step.
    pub fn problems(&self, steps: &VoxelSteps) -> Vec<String> {
        let mut out = Vec::new();
        if let Err(e) = VoxelVolume::check_shape(self.size, self.block, self.origin, self.chunk) {
            out.push(e);
        }
        if !(1..=MAX_ATTEMPTS).contains(&self.attempts) {
            out.push(format!("attempts: 1 to {MAX_ATTEMPTS}, not {}", self.attempts));
        }
        out.extend(steps.validate_steps(&self.steps));
        out.extend(steps.validate_checks(&self.checks));
        let listed: BTreeSet<&str> = self.steps.iter().map(|(n, _)| n.as_str()).collect();
        for key in self.params.keys() {
            if !listed.contains(key.as_str()) {
                out.push(format!("params: `{key}` is not a listed step"));
            }
        }
        out
    }
}

/// The world a step edits, and what it may ask: the volume, the palette, its params, its rng and
/// noise, the marks so far.
pub struct VoxelGen<'a> {
    volume: VoxelVolume,
    palette: &'a VoxelPalette,
    spec: &'a GenSpec,
    attempt: u32,
    /// The running step: its place in the list and its name (empty while checks run).
    step_index: usize,
    step: &'a str,
    marks: Vec<Mark>,
    remarks: Vec<String>,
}

impl<'a> VoxelGen<'a> {
    /// The volume being made.
    pub fn volume(&self) -> &VoxelVolume {
        &self.volume
    }

    /// The volume, to edit. Steps only: checks get `&VoxelGen`.
    pub fn volume_mut(&mut self) -> &mut VoxelVolume {
        &mut self.volume
    }

    pub fn palette(&self) -> &VoxelPalette {
        self.palette
    }

    pub fn seed(&self) -> u64 {
        self.spec.seed
    }

    /// The region's version.
    pub fn version(&self) -> u32 {
        self.spec.version
    }

    /// Which attempt this is, from 0.
    pub fn attempt(&self) -> u32 {
        self.attempt
    }

    /// The name of the running step (empty in a check).
    pub fn step_name(&self) -> &str {
        self.step
    }

    /// The running step's `params` entry read as `T` (numbers stay content). A step without an
    /// entry reads `T` from an empty map, so a `#[serde(default)]` struct works and a struct with
    /// required fields says which one is missing.
    pub fn params<T: DeserializeOwned>(&self) -> Result<T, String> {
        let empty = Value::Object(Default::default());
        let value = self.spec.params.get(self.step).unwrap_or(&empty);
        T::deserialize(value).map_err(|e| format!("params of step `{}`: {e}", self.step))
    }

    /// The step's random stream: `DetRng::from_keys(&[domain, seed, version, attempt, step_index])`.
    /// Every call starts the same stream over: take it once per step (`let mut rng = g.rng();`),
    /// or [`rng_keyed`](Self::rng_keyed) for more.
    pub fn rng(&self) -> DetRng {
        DetRng::from_keys(&[STREAM_STEP, self.spec.seed, self.spec.version as u64, self.attempt as u64, self.step_index as u64])
    }

    /// A stream of its own beside [`rng`](Self::rng), by `key` (a cell, a salt).
    pub fn rng_keyed(&self, key: u64) -> DetRng {
        DetRng::from_keys(&[STREAM_KEYED, self.spec.seed, self.spec.version as u64, self.attempt as u64, self.step_index as u64, key])
    }

    /// Value noise for this attempt, told apart by `salt`, `frequency` lattice cells per cell
    /// (coordinates in cells). The same salt gives the same field in every step of an attempt, so
    /// two steps may sample one terrain; another attempt gives another field.
    pub fn noise(&self, salt: u64, frequency: f32) -> Noise {
        Noise::new(DetRng::from_keys(&[STREAM_NOISE, self.spec.seed, self.spec.version as u64, self.attempt as u64, salt]).next_u64(), frequency)
    }

    /// Leaves a named point at world position `pos` (the spawn, a point of interest). A name used
    /// again replaces the earlier point.
    pub fn mark(&mut self, name: &str, pos: Vec3) {
        match self.marks.iter_mut().find(|m| m.name == name) {
            Some(m) => m.pos = pos,
            None => self.marks.push(Mark { name: name.to_owned(), pos }),
        }
    }

    /// The marks so far, in the order first made.
    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }

    pub fn find_mark(&self, name: &str) -> Option<Vec3> {
        self.marks.iter().find(|m| m.name == name).map(|m| m.pos)
    }

    /// A line for the manifest and the journal ("the lake took 40 columns").
    pub fn remark(&mut self, text: impl Into<String>) {
        self.remarks.push(text.into());
    }
}

/// A finished generation.
#[derive(Clone, Debug)]
pub struct Generated {
    pub volume: VoxelVolume,
    pub marks: Vec<Mark>,
    pub remarks: Vec<String>,
    /// Which attempt succeeded, from 0.
    pub attempt: u32,
    /// The step list with versions it was made by.
    pub steps: Vec<(String, u32)>,
    /// [`world_hash`]: the volume's hash with the palette fingerprint and the step set folded in. What `voxel.volume` will
    /// compare.
    pub hash: u64,
}

impl Generated {
    /// The position of mark `name`.
    pub fn find_mark(&self, name: &str) -> Option<Vec3> {
        self.marks.iter().find(|m| m.name == name).map(|m| m.pos)
    }
}

/// Generates the world `spec` describes. Fails at once with every problem of the spec
/// ([`GenSpec::problems`]), or after the last attempt with each attempt's reason.
pub fn generate(registry: &VoxelSteps, palette: &VoxelPalette, spec: &GenSpec) -> Result<Generated, String> {
    let problems = spec.problems(registry);
    if !problems.is_empty() {
        return Err(problems.join("; "));
    }
    let mut failures = Vec::new();
    for attempt in 0..spec.attempts {
        match attempt_once(registry, palette, spec, attempt) {
            Ok(done) => return Ok(done),
            Err(why) => failures.push(format!("attempt {attempt}: {why}")),
        }
    }
    Err(format!("generation failed after {} attempt(s): {}", spec.attempts, failures.join("; ")))
}

fn attempt_once(registry: &VoxelSteps, palette: &VoxelPalette, spec: &GenSpec, attempt: u32) -> Result<Generated, String> {
    let volume = VoxelVolume::new(spec.size, spec.block, spec.origin, spec.chunk)?;
    let mut g = VoxelGen { volume, palette, spec, attempt, step_index: 0, step: "", marks: Vec::new(), remarks: Vec::new() };
    for (index, (name, _)) in spec.steps.iter().enumerate() {
        let (_, run) = registry.steps[name.as_str()];
        g.step_index = index;
        g.step = name;
        run(&mut g).map_err(|e| format!("step `{name}`: {e}"))?;
    }
    g.step_index = spec.steps.len();
    g.step = "";
    g.volume.compact();
    for name in &spec.checks {
        registry.checks[name.as_str()](&g).map_err(|e| format!("check `{name}`: {e}"))?;
    }
    let hash = world_hash(g.volume.hash(), palette, &spec.steps);
    Ok(Generated { volume: g.volume, marks: g.marks, remarks: g.remarks, attempt, steps: spec.steps.clone(), hash })
}

// --- stock steps ------------------------------------------------------------------------------

/// The registered version of `noise_hills`.
pub const NOISE_HILLS_VERSION: u32 = 1;
/// The registered version of `fill_water`.
pub const FILL_WATER_VERSION: u32 = 1;

fn four() -> f32 {
    4.0
}

fn base_layers() -> f32 {
    2.0
}

fn frequency() -> f32 {
    0.03
}

fn octaves() -> u32 {
    4
}

/// Params of the stock step `noise_hills`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NoiseHillsParams {
    /// The block of the top layer of every column (a palette name).
    pub top: String,
    /// The block under it.
    pub fill: String,
    /// Layers every column has at least.
    #[serde(default = "base_layers")]
    pub base: f32,
    /// Layers the noise adds on top of `base`: the tallest hill is `base + amplitude`.
    #[serde(default = "four")]
    pub amplitude: f32,
    /// Noise lattice cells per cell.
    #[serde(default = "frequency")]
    pub frequency: f32,
    #[serde(default = "octaves")]
    pub octaves: u32,
    /// Noise salt: two steps with another salt make other hills from one seed.
    #[serde(default)]
    pub salt: u64,
}

/// The stock step `noise_hills`: every column gets a height `base + amplitude * noise` (noise in
/// `0..=1`), rounded, at least 1 and at most the volume's height, laid with
/// [`raise_column`](VoxelVolume::raise_column) (`fill` under a `top` layer). Columns that already
/// stand higher stay. Neutral: the numbers are the content's.
pub fn noise_hills(g: &mut VoxelGen) -> Result<(), String> {
    let p: NoiseHillsParams = g.params()?;
    if !(p.base.is_finite() && p.amplitude.is_finite() && p.frequency.is_finite() && p.base >= 0.0 && p.amplitude >= 0.0 && p.frequency > 0.0) {
        return Err(format!("base and amplitude are 0 or more and frequency above 0, not {} / {} / {}", p.base, p.amplitude, p.frequency));
    }
    if p.octaves == 0 || p.octaves > 8 {
        return Err(format!("octaves: 1 to 8, not {}", p.octaves));
    }
    let top = block_named(g, &p.top, "top")?;
    let fill = block_named(g, &p.fill, "fill")?;
    let mut noise = g.noise(p.salt, p.frequency);
    noise.octaves = p.octaves;
    let size = g.volume().size();
    for x in 0..size.x as i32 {
        for z in 0..size.z as i32 {
            let h = (p.base + p.amplitude * noise.sample01(x as f32, z as f32)).round().max(1.0) as u32;
            g.volume_mut().raise_column(x, z, h, fill, top);
        }
    }
    Ok(())
}

/// Params of the stock step `fill_water`.
#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FillWaterParams {
    /// Columns whose natural surface is below this layer get liquid up to it.
    pub level: u32,
    /// The liquid block (a palette name); unset: the first liquid block of the palette.
    #[serde(default)]
    pub block: Option<String>,
}

/// The stock step `fill_water`: in every column whose natural surface is under `level`, the air
/// cells from the surface up to `level` become the liquid block. Cells already holding a block
/// stay; the surfaces do not move (the natural ground is what the water lies on).
pub fn fill_water(g: &mut VoxelGen) -> Result<(), String> {
    let p: FillWaterParams = g.params()?;
    let liquid = match &p.block {
        Some(name) => {
            let id = block_named(g, name, "block")?;
            if !g.palette().liquid(id) {
                return Err(format!("block `{name}` is not a liquid in the palette"));
            }
            id
        }
        None => (1..=g.palette().len() as u8).find(|&id| g.palette().liquid(id)).ok_or("the palette has no liquid block")?,
    };
    let size = g.volume().size();
    let level = p.level.min(size.y);
    for x in 0..size.x as i32 {
        for z in 0..size.z as i32 {
            for y in g.volume().surface(x, z)..level {
                let at = IVec3::new(x, y as i32, z);
                if g.volume().get(at) == AIR {
                    g.volume_mut().set(at, liquid);
                }
            }
        }
    }
    Ok(())
}

/// The id of the block `name` of the palette, or the error a step returns.
pub fn block_named(g: &VoxelGen, name: &str, field: &str) -> Result<u8, String> {
    g.palette().id(name).ok_or_else(|| format!("{field}: the palette has no block `{name}`"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::palette::{BlockDef, VoxelPaletteDef};
    use serde_json::json;

    fn palette() -> VoxelPalette {
        let mut water = BlockDef::solid("water", "#2a6f9f");
        (water.solid, water.liquid) = (false, true);
        VoxelPalette::from_def(&VoxelPaletteDef::new(vec![BlockDef::solid("grass", "#5b8f3a"), BlockDef::solid("dirt", "#6f5a3c"), water])).unwrap()
    }

    fn hills_spec(seed: u64) -> GenSpec {
        let mut spec = GenSpec::new(UVec3::new(32, 16, 32), UVec3::new(16, 16, 16), seed, &[("noise_hills", 1), ("fill_water", 1)]);
        spec.params.insert("noise_hills".into(), json!({"top": "grass", "fill": "dirt", "base": 3.0, "amplitude": 8.0, "frequency": 0.1}));
        spec.params.insert("fill_water".into(), json!({"level": 6}));
        spec
    }

    #[test]
    fn the_registry_validates_names_versions_and_lists() {
        fn nothing(_: &mut VoxelGen) -> Result<(), String> {
            Ok(())
        }
        fn ok_check(_: &VoxelGen) -> Result<(), String> {
            Ok(())
        }
        let mut steps = VoxelSteps::default();
        assert_eq!(steps.step_names(), vec!["fill_water", "noise_hills"], "the stock steps");
        steps.add_step("arena", 3, nothing).unwrap();
        assert_eq!(steps.version("arena"), Some(3));
        assert!(steps.add_step("arena", 4, nothing).unwrap_err().contains("registered twice"));
        assert!(steps.add_step("noise_hills", 2, nothing).unwrap_err().contains("registered twice"), "a game cannot shadow a stock step");
        assert!(steps.add_step("Arena", 1, nothing).unwrap_err().contains("lowercase"));
        assert!(steps.add_step("", 1, nothing).is_err());
        assert!(steps.add_step("zero", 0, nothing).unwrap_err().contains("start at 1"));
        steps.add_check("flat", ok_check).unwrap();
        assert!(steps.add_check("flat", ok_check).unwrap_err().contains("registered twice"));
        assert_eq!(steps.check_names(), vec!["flat"]);

        let list = |items: &[(&str, u32)]| items.iter().map(|(n, v)| ((*n).to_owned(), *v)).collect::<Vec<_>>();
        assert!(steps.validate_steps(&list(&[("noise_hills", 1), ("arena", 3), ("arena", 3)])).is_empty());
        let bad = steps.validate_steps(&list(&[("noise_hils", 1), ("arena", 2), ("nowhere", 1)]));
        assert_eq!(bad[0], "steps[0]: unknown voxel step `noise_hils` (did you mean `noise_hills`?)");
        assert_eq!(bad[1], "steps[1]: voxel step `arena` is at version 3 in this build, the content expects 2: set the version in the content to 3, or register 2 in the game");
        assert!(bad[2].starts_with("steps[2]: unknown voxel step `nowhere`"), "{bad:?}");
        assert_eq!(steps.validate_steps(&[]), vec!["steps: a region lists at least one step".to_owned()]);
        assert!(VoxelSteps { steps: BTreeMap::new(), checks: BTreeMap::new() }.validate_steps(&list(&[("a", 1)]))[0].contains("no step is registered"));
        assert_eq!(steps.validate_checks(&["flat".into(), "flatt".into()]), vec!["checks[1]: unknown voxel check `flatt` (did you mean `flat`?)".to_owned()]);
    }

    #[test]
    fn a_spec_names_every_problem() {
        let steps = VoxelSteps::default();
        let mut spec = hills_spec(1);
        assert!(spec.problems(&steps).is_empty(), "{:?}", spec.problems(&steps));
        spec.size = UVec3::new(30, 16, 32);
        spec.attempts = 0;
        spec.steps.push(("missing".into(), 1));
        spec.checks.push("nope".into());
        spec.params.insert("stray".into(), json!({}));
        let problems = spec.problems(&steps);
        assert_eq!(problems.len(), 5, "{problems:#?}");
        assert!(problems[0].contains("not a whole number of"));
        assert!(problems[1].starts_with("attempts: 1 to 64, not 0"));
        assert!(problems[4].contains("`stray` is not a listed step"));
        let err = generate(&steps, &palette(), &spec).unwrap_err();
        assert!(err.contains("attempts") && err.contains("unknown voxel step `missing`"), "{err}");
    }

    #[test]
    fn the_same_spec_makes_the_same_world_and_another_seed_another() {
        let (steps, pal) = (VoxelSteps::default(), palette());
        let a = generate(&steps, &pal, &hills_spec(1)).unwrap();
        // A second registry, a second palette: nothing is shared between runs.
        let b = generate(&VoxelSteps::default(), &palette(), &hills_spec(1)).unwrap();
        assert_eq!((a.hash, a.volume.hash()), (b.hash, b.volume.hash()));
        assert_eq!(a.volume, b.volume);
        let c = generate(&steps, &pal, &hills_spec(2)).unwrap();
        assert_ne!(a.hash, c.hash);
        // The world has hills and a lake in its low ground.
        let tops: Vec<u32> = (0..32).flat_map(|x| (0..32).map(move |z| (x, z))).map(|(x, z)| a.volume.surface(x, z)).collect();
        let (lo, hi) = (*tops.iter().min().unwrap(), *tops.iter().max().unwrap());
        assert!(lo >= 3 && hi <= 11 && hi - lo >= 3, "{lo}..{hi}");
        let water = pal.id("water").unwrap();
        let lake = a.volume.count(|id| id == water);
        assert!(lake > 0, "some column is under level 6");
        for x in 0..32 {
            for z in 0..32 {
                let s = a.volume.surface(x, z);
                assert_ne!(a.volume.get(IVec3::new(x, s as i32 - 1, z)), water, "water lies on the ground, not in it");
                if s < 6 {
                    assert_eq!(a.volume.get(IVec3::new(x, 5, z)), water, "({x}, {z}) is under level 6");
                } else {
                    assert_ne!(a.volume.get(IVec3::new(x, 6, z)), water);
                }
            }
        }
        assert_eq!(a.volume.dense_chunks().1, 4);
        assert_eq!(a.steps, hills_spec(1).steps);
    }

    #[test]
    fn the_step_set_is_part_of_the_hash() {
        let a = generate(&VoxelSteps::default(), &palette(), &hills_spec(1)).unwrap();
        let mut spec = hills_spec(1);
        spec.steps[1].1 = 1;
        assert_eq!(world_hash(a.volume.hash(), &palette(), &spec.steps), a.hash);
        let other = [("noise_hills".to_owned(), 1), ("fill_water".to_owned(), 2)];
        assert_ne!(world_hash(a.volume.hash(), &palette(), &other), a.hash, "the same cells under another step version are another world");
        let swapped = [("fill_water".to_owned(), 1), ("noise_hills".to_owned(), 1)];
        assert_ne!(step_set_hash(&swapped), step_set_hash(&spec.steps), "the order of steps counts");
        assert_ne!(step_set_hash(&[("ab".into(), 1)]), step_set_hash(&[("a".into(), 1), ("b".into(), 1)]));
    }

    #[test]
    fn a_golden_world_pins_noise_rng_and_hash() {
        let a = generate(&VoxelSteps::default(), &palette(), &hills_spec(1)).unwrap();
        assert_eq!((a.volume.hash(), a.hash), GOLDEN_WORLD, "{:#018x} {:#018x}", a.volume.hash(), a.hash);
    }

    const GOLDEN_WORLD: (u64, u64) = (0x4214394880047bc2, 0x1c55608147bab276);

    #[test]
    fn rng_and_noise_are_keyed_by_seed_version_attempt_and_step() {
        fn probe(g: &mut VoxelGen) -> Result<(), String> {
            let (mut r, mut again) = (g.rng(), g.rng());
            assert_eq!(r.next_u64(), again.next_u64(), "rng() starts the same stream every call");
            let k = g.rng_keyed(5).next_u64();
            assert_ne!(k, g.rng().next_u64());
            let word = r.next_u64() ^ k ^ g.noise(1, 0.1).sample(3.3, 4.4).to_bits() as u64;
            let n = g.palette().id("dirt").unwrap();
            let at = g.step_name().len() as i32;
            g.volume_mut().set(IVec3::new(at, 0, 0), n);
            g.remark(format!("{word:x}"));
            Ok(())
        }
        let mut steps = VoxelSteps::default();
        steps.add_step("probe", 1, probe).unwrap();
        let run = |seed: u64, version: u32, list: &[(&str, u32)]| {
            let mut spec = GenSpec::new(UVec3::new(8, 8, 8), UVec3::splat(8), seed, list);
            spec.version = version;
            generate(&steps, &palette(), &spec).unwrap().remarks
        };
        let base = run(1, 1, &[("probe", 1)]);
        assert_eq!(base, run(1, 1, &[("probe", 1)]));
        assert_ne!(base, run(2, 1, &[("probe", 1)]), "seed");
        assert_ne!(base, run(1, 2, &[("probe", 1)]), "region version");
        let twice = run(1, 1, &[("probe", 1), ("probe", 1)]);
        assert_ne!(twice[0], twice[1], "the same step listed twice has another place in the list, so another stream");
    }

    #[test]
    fn a_failed_check_retries_with_other_keys_and_collects_every_reason() {
        fn flat(g: &VoxelGen) -> Result<(), String> {
            let tops: Vec<u32> = (0..16).map(|x| g.volume().surface(x, 8)).collect();
            if tops.windows(2).all(|w| w[0] == w[1]) { Ok(()) } else { Err(format!("not flat: {tops:?}")) }
        }
        fn never(_: &VoxelGen) -> Result<(), String> {
            Err("can never pass".into())
        }
        fn spawn_on_ground(g: &mut VoxelGen) -> Result<(), String> {
            let h = g.volume().surface(8, 8) as f32;
            g.mark("spawn", Vec3::new(8.5, h, 8.5));
            g.mark("spawn", Vec3::new(8.5, h + 1.0, 8.5));
            g.remark("spawned");
            Ok(())
        }
        let mut steps = VoxelSteps::default();
        steps.add_step("spawn", 1, spawn_on_ground).unwrap();
        steps.add_check("flat", flat).unwrap();
        steps.add_check("never", never).unwrap();
        let mut spec = GenSpec::new(UVec3::new(16, 8, 16), UVec3::new(8, 8, 8), 1, &[("noise_hills", 1), ("spawn", 1)]);
        spec.params.insert("noise_hills".into(), json!({"top": "grass", "fill": "dirt", "base": 2.0, "amplitude": 2.0, "frequency": 0.9}));
        spec.attempts = 3;
        spec.checks = vec!["never".into()];
        let err = generate(&steps, &palette(), &spec).unwrap_err();
        assert!(err.starts_with("generation failed after 3 attempt(s): attempt 0: check `never`: can never pass; attempt 1:"), "{err}");
        assert!(err.contains("attempt 2: check `never`"), "{err}");
        // A check that passes on some attempt ends the loop at it, and the marks are that attempt's.
        spec.checks = vec!["flat".into()];
        spec.attempts = 40;
        let mut retried = false;
        for seed in 1..=8 {
            spec.seed = seed;
            match generate(&steps, &palette(), &spec) {
                Ok(g) => {
                    retried |= g.attempt > 0;
                    assert_eq!(g.marks.len(), 1, "a name used again replaces the point");
                    assert_eq!(g.find_mark("spawn").unwrap().y, g.volume.surface(8, 8) as f32 + 1.0);
                    assert_eq!(g.remarks, vec!["spawned".to_owned()]);
                }
                Err(e) => assert!(e.contains("not flat"), "{e}"),
            }
        }
        assert!(retried, "some seed is bumpy at attempt 0 at this frequency");
        // Reproducible: the same failure text twice.
        assert_eq!(generate(&steps, &palette(), &spec).map(|g| g.hash), generate(&steps, &palette(), &spec).map(|g| g.hash));
    }

    #[test]
    fn a_failing_step_fails_the_attempt_naming_it() {
        fn boom(_: &mut VoxelGen) -> Result<(), String> {
            Err("no room".into())
        }
        let mut steps = VoxelSteps::default();
        steps.add_step("boom", 1, boom).unwrap();
        let spec = GenSpec::new(UVec3::new(8, 8, 8), UVec3::splat(8), 1, &[("boom", 1)]);
        assert_eq!(generate(&steps, &palette(), &spec).unwrap_err(), "generation failed after 1 attempt(s): attempt 0: step `boom`: no room");
    }

    #[test]
    fn stock_steps_report_bad_params() {
        let (steps, pal) = (VoxelSteps::default(), palette());
        let run = |name: &str, params: Value| {
            let mut spec = GenSpec::new(UVec3::new(8, 8, 8), UVec3::splat(8), 1, &[(name, 1)]);
            spec.params.insert(name.into(), params);
            generate(&steps, &pal, &spec).unwrap_err()
        };
        assert!(run("noise_hills", json!({"top": "grass"})).contains("missing field `fill`"));
        assert!(run("noise_hills", json!({"top": "grass", "fill": "rock"})).contains("fill: the palette has no block `rock`"));
        assert!(run("noise_hills", json!({"top": "grass", "fill": "dirt", "extra": 1})).contains("unknown field `extra`"));
        assert!(run("noise_hills", json!({"top": "grass", "fill": "dirt", "frequency": 0.0})).contains("frequency above 0"));
        assert!(run("noise_hills", json!({"top": "grass", "fill": "dirt", "octaves": 0})).contains("octaves: 1 to 8"));
        assert!(run("fill_water", json!({})).contains("missing field `level`"));
        assert!(run("fill_water", json!({"level": 3, "block": "dirt"})).contains("is not a liquid"));
        let no_liquid = VoxelPalette::from_def(&VoxelPaletteDef::new(vec![BlockDef::solid("a", "#fff")])).unwrap();
        let mut spec = GenSpec::new(UVec3::new(8, 8, 8), UVec3::splat(8), 1, &[("fill_water", 1)]);
        spec.params.insert("fill_water".into(), json!({"level": 3}));
        assert!(generate(&steps, &no_liquid, &spec).unwrap_err().contains("the palette has no liquid block"));
    }

    #[test]
    fn params_of_a_step_without_an_entry_read_as_an_empty_map() {
        #[derive(Deserialize)]
        struct P {
            #[serde(default)]
            n: u32,
        }
        fn read(g: &mut VoxelGen) -> Result<(), String> {
            let p: P = g.params()?;
            g.remark(format!("n={}", p.n));
            Ok(())
        }
        let mut steps = VoxelSteps::default();
        steps.add_step("read", 1, read).unwrap();
        let mut spec = GenSpec::new(UVec3::new(8, 8, 8), UVec3::splat(8), 1, &[("read", 1)]);
        assert_eq!(generate(&steps, &palette(), &spec).unwrap().remarks, vec!["n=0".to_owned()]);
        spec.params.insert("read".into(), json!({"n": 7}));
        assert_eq!(generate(&steps, &palette(), &spec).unwrap().remarks, vec!["n=7".to_owned()]);
    }

    #[test]
    fn ron_params_read_as_json_values() {
        // The shape a region's `params: { "hills": (amplitude: 7.0) }` takes in a RON file.
        let params: BTreeMap<String, Value> = ron::from_str(r#"{ "noise_hills": (top: "grass", fill: "dirt", amplitude: 7.0, octaves: 3), "fill_water": (level: 4) }"#).unwrap();
        assert_eq!(params["noise_hills"]["amplitude"], json!(7.0));
        assert_eq!(params["fill_water"], json!({"level": 4}));
    }

    #[test]
    fn the_palette_and_the_stream_kind_are_part_of_a_world() {
        let spec = hills_spec(1);
        let a = generate(&VoxelSteps::default(), &palette(), &spec).unwrap();
        // Same cells, the blocks renamed: another world.
        let mut def = palette().def().clone();
        def.blocks[0].name = "renamed".to_owned();
        let other = VoxelPalette::from_def(&def).unwrap();
        assert_ne!(world_hash(a.volume.hash(), &other, &spec.steps), a.hash);
        assert_ne!(other.fingerprint(), palette().fingerprint());
    }
}
