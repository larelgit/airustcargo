//! The engine's own save sections.
//!
//! The save module ([`shards_save`]) knows nothing of motion, combat or regions; the
//! umbrella, which knows them all, says what of theirs a save holds:
//!
//! | section | kind | holds |
//! |---|---|---|
//! | `region` | world | the region in the world: its id, seed, generator version, manifest hash and (v2) the manifest itself. Applying it builds the region from that manifest, without the generator, and changes to it through the world module's region change (when it differs from the one there): a game update that changes the generator leaves a saved world as it was. Refused, changing nothing, when content no longer has what the manifest names; a manifest that does not match its hash is damage (the save's backups are tried). A save of v1 kept no manifest: it is migrated to v2 without one, generated again, and refused if it no longer comes out the same; its next save is a v2 one |
//! | `position` | character | the body's feet, its rotation (characters turn about Y: that is their facing) and the region it stood in. Applied onto a body in the same region only; in another one the body stays where it is and the journal says why |
//! | `vitals` | character | health, stamina and charges. Applied to the components the body already has (a fresh body has what its prefab gives it); a charge the body lacks is added. A body that died since the save is revived (`shards_action::revive`, as the dev `heal` does, and `Healed` follows); a save made at zero health leaves it dead |
//!
//! A game replaces one with a section of the same name (`position` as "restore at the last
//! camp"): the new one takes its place in the order sections are applied, and the engine's
//! migrations of the one it replaces (`region` from v1) go with it.

use ai_core::journal::{self, JournalKind};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use shards_action::{revive, Charge, Charges, Health, Healed, Stamina};
use shards_content::ContentDb;
use shards_save::{SaveAppExt, SaveKind, SaveSection};
use shards_world::manifest::{Layout, FORMAT};
use shards_world::{generate, region, ActiveRegion, DungeonDef, RegionDef, RegionManifest, WorldGen};

/// Adds the engine's sections to the registry. Part of [`EnginePlugins`](crate::EnginePlugins).
pub struct EngineSavePlugin;

impl Plugin for EngineSavePlugin {
    fn build(&self, app: &mut App) {
        // World first, then the character: the position is applied onto the region just built.
        app.add_save_section::<RegionSave>()
            .add_save_migration(RegionSave::NAME, 1, region_from_v1)
            .add_save_section::<PositionSave>()
            .add_save_section::<VitalsSave>();
    }
}

/// A region as a save names it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RegionRef {
    /// The `region` or `dungeon` content id.
    pub id: String,
    pub seed: u64,
}

impl RegionRef {
    fn current(world: &World) -> Option<Self> {
        world.get_resource::<ActiveRegion>().map(|r| RegionRef { id: r.manifest.region.clone(), seed: r.manifest.seed })
    }

    fn text(&self) -> String {
        format!("{} seed {}", self.id, self.seed)
    }
}

// --- region -----------------------------------------------------------------------------

/// The `region` section.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct RegionSave {
    /// `None`: the world had no region.
    pub active: Option<SavedRegion>,
}

/// The region a world was saved in. Read through [`SavedRegionData`], so a manifest that is not
/// the one its hash and names describe does not read: the save module counts that copy as
/// damaged and tries its backups.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(try_from = "SavedRegionData")]
pub struct SavedRegion {
    pub id: String,
    pub seed: u64,
    /// The generator version of the region's definition when it was made.
    pub version: u32,
    /// [`RegionManifest::hash`], hex: what the world was.
    pub manifest_hash: String,
    /// The region as it was built, kept for good: loading builds it from this, never from the
    /// generator. `None` only in a save migrated from version 1 of the section, which kept
    /// none: that one is generated again and must come out with `manifest_hash`.
    pub manifest: Option<RegionManifest>,
}

/// [`SavedRegion`] as a file holds it, before it is checked.
#[derive(Deserialize)]
struct SavedRegionData {
    id: String,
    seed: u64,
    version: u32,
    manifest_hash: String,
    #[serde(default)]
    manifest: Option<RegionManifest>,
}

impl TryFrom<SavedRegionData> for SavedRegion {
    type Error = String;

    fn try_from(data: SavedRegionData) -> Result<Self, String> {
        let SavedRegionData { id, seed, version, manifest_hash, manifest } = data;
        let saved = SavedRegion { id, seed, version, manifest_hash, manifest };
        saved.check()?;
        Ok(saved)
    }
}

impl SavedRegion {
    /// `wilds seed 42 v1`.
    fn title(&self) -> String {
        format!("{} seed {} v{}", self.id, self.seed, self.version)
    }

    /// The saved hash, after checking that the manifest (when there is one) is the region the
    /// save names and hashes to it: what the region was is in the file whole, or the file is
    /// damaged.
    fn check(&self) -> Result<u64, String> {
        let want = u64::from_str_radix(&self.manifest_hash, 16).map_err(|_| format!("manifest_hash `{}` is not hex", self.manifest_hash))?;
        let Some(manifest) = &self.manifest else {
            return Ok(want);
        };
        let title = self.title();
        if manifest.format != FORMAT {
            return Err(format!("the manifest of {title} is of format `{}`; this build reads `{FORMAT}`", manifest.format));
        }
        if (manifest.region.as_str(), manifest.seed, manifest.version) != (self.id.as_str(), self.seed, self.version) {
            return Err(format!("the manifest of {title} is damaged: it is the manifest of {}", manifest.title()));
        }
        let now = manifest.hash();
        if now != want {
            return Err(format!("the manifest of {title} is damaged: it hashes to {now:016x}, the save says {want:016x}"));
        }
        Ok(want)
    }
}

/// Version 1 of the section kept no manifest: such a region is generated again when it is
/// loaded ([`SavedRegion::manifest`] `None`).
fn region_from_v1(mut old: Value) -> Result<Value, String> {
    if let Some(active) = old.get_mut("active").and_then(Value::as_object_mut) {
        active.insert("manifest".to_owned(), Value::Null);
    }
    Ok(old)
}

impl SaveSection for RegionSave {
    const NAME: &'static str = "region";
    const VERSION: u32 = 2;
    const KIND: SaveKind = SaveKind::World;

    fn capture(world: &World, _: Option<Entity>) -> Result<Self, String> {
        let active = world.get_resource::<ActiveRegion>().map(|r| SavedRegion {
            id: r.manifest.region.clone(),
            seed: r.manifest.seed,
            version: r.manifest.version,
            manifest_hash: format!("{:016x}", r.hash),
            manifest: Some(r.manifest.clone()),
        });
        Ok(RegionSave { active })
    }

    fn apply(self, world: &mut World, _: Option<Entity>) -> Result<(), String> {
        // Saved outside any region: nothing to build, and nothing is unloaded either. A save moves
        // the world the way play does, through `change_region`, which always goes to a region; only
        // a snapshot, which replaces the whole world, unloads one. So a world saved in the arena and
        // loaded after a `regen` keeps the region that is loaded now.
        let Some(saved) = self.active else {
            return Ok(());
        };
        let want = saved.check()?;
        if world.get_resource::<ActiveRegion>().is_some_and(|r| r.hash == want) {
            return Ok(());
        }
        match saved.manifest {
            Some(manifest) => build_saved(world, manifest),
            None => generate_again(world, &saved.id, saved.seed, &saved.title(), want),
        }
    }
}

/// The generator version content has now for the region of `manifest`, `None` when content no
/// longer defines it.
fn generator_version(db: &ContentDb, manifest: &RegionManifest) -> Option<u32> {
    match manifest.layout {
        Layout::Grid => db.get::<RegionDef>(&manifest.region).map(|d| d.version),
        Layout::Rooms => db.get::<DungeonDef>(&manifest.region).map(|d| d.version),
    }
}

/// A region from the save's own manifest: the world it was, whatever the generator makes of
/// that seed now. Content must still have everything the manifest names; checked before
/// anything changes.
fn build_saved(world: &mut World, manifest: RegionManifest) -> Result<(), String> {
    let title = manifest.title();
    region::check_manifest(world, &manifest).map_err(|e| {
        format!("the save's world cannot be built: {e}; keep that content in the game, or write a migration for `region` that replaces it in the saved manifest")
    })?;
    let now = match generator_version(world.resource::<ContentDb>(), &manifest) {
        Some(version) if version == manifest.version => String::new(),
        Some(version) => format!(" (the generator is v{version} now)"),
        None => format!(" (`{}` is no longer in the content)", manifest.region),
    };
    journal::emit(world, JournalKind::Event, format!("region {title}: built from the save's manifest{now}"));
    region::change_region(world, manifest).map_err(|e| format!("cannot change to {title}: {e}"))?;
    Ok(())
}

/// A region of a version 1 save, which kept no manifest: generated again from its seed, and
/// refused, changing nothing, unless it comes out as it was (`want`, its manifest hash).
fn generate_again(world: &mut World, id: &str, seed: u64, title: &str, want: u64) -> Result<(), String> {
    let worldgen = world.get_resource::<WorldGen>().ok_or("the world module is not installed")?.clone();
    // Pure: the same seed and content make the same manifest. Checked before anything changes.
    let generated = generate::generate(world.resource::<ContentDb>(), &worldgen, id, seed).map_err(|e| {
        let attempts: Vec<String> = e.failures.iter().map(|f| format!("  {f}")).collect();
        if attempts.is_empty() { format!("cannot generate {title}: {}", e.message) } else { format!("cannot generate {title}: {}\n{}", e.message, attempts.join("\n")) }
    })?;
    let now = generated.manifest.hash();
    if now != want {
        return Err(format!(
            "{title} comes out differently now (manifest {want:016x} when saved, {now:016x} now, generator v{}): its definition or its tiles changed since the save, \
             so the world it describes cannot be built again; write a migration for `region` or keep the old content",
            generated.manifest.version
        ));
    }
    region::journal_generated(world, &generated);
    region::change_region(world, generated.manifest).map_err(|e| format!("cannot change to {title}: {e}"))?;
    Ok(())
}

// --- position ---------------------------------------------------------------------------

/// The `position` section.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PositionSave {
    /// Where the feet are.
    pub feet: [f32; 3],
    /// The rotation as a quaternion `[x, y, z, w]`: characters turn about Y, so this is the
    /// facing.
    pub rotation: [f32; 4],
    /// The region the body stood in, `None` outside any.
    pub region: Option<RegionRef>,
}

impl SaveSection for PositionSave {
    const NAME: &'static str = "position";
    const VERSION: u32 = 1;
    const KIND: SaveKind = SaveKind::Character;

    fn capture(world: &World, body: Option<Entity>) -> Result<Self, String> {
        let body = body.ok_or("there is no body")?;
        let tf = world.get::<Transform>(body).ok_or("the body has no Transform")?;
        Ok(PositionSave { feet: tf.translation.to_array(), rotation: tf.rotation.to_array(), region: RegionRef::current(world) })
    }

    fn apply(self, world: &mut World, body: Option<Entity>) -> Result<(), String> {
        let body = body.ok_or("there is no body")?;
        if let Some(saved) = &self.region {
            let now = RegionRef::current(world);
            if now.as_ref() != Some(saved) {
                let there = now.map_or_else(|| "no region".to_owned(), |r| r.text());
                journal::emit(
                    world,
                    JournalKind::Warn,
                    format!("save: the position was saved in region {} but the world has {there}; the body stays where it is", saved.text()),
                );
                return Ok(());
            }
        }
        let rotation = Quat::from_array(self.rotation);
        if !rotation.is_finite() || rotation.length_squared() < 1e-6 {
            return Err(format!("rotation {:?} is not a rotation", self.rotation));
        }
        // A character may come from another machine (co-op): a body at infinity is not a position.
        if self.feet.iter().any(|v| !v.is_finite()) {
            return Err(format!("feet {:?} are not a position", self.feet));
        }
        let mut tf = world.get_mut::<Transform>(body).ok_or("the body has no Transform")?;
        tf.translation = Vec3::from_array(self.feet);
        tf.rotation = rotation;
        Ok(())
    }
}

// --- vitals -----------------------------------------------------------------------------

/// The `vitals` section.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct VitalsSave {
    pub health: Option<Pool>,
    pub stamina: Option<Pool>,
    pub charges: Vec<ChargeSave>,
}

/// A value and its ceiling.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Pool {
    pub current: f32,
    pub max: f32,
}

impl Pool {
    /// Whether this can be applied: a character comes from a file, and over the network from
    /// another machine, so its numbers are checked before anything changes (`f32::clamp`
    /// panics when the ceiling is below zero).
    fn check(&self, what: &str) -> Result<(), String> {
        if !self.current.is_finite() || !self.max.is_finite() || self.max < 0.0 {
            return Err(format!("{what} {} of {} is not a pool (finite numbers, a ceiling of 0 or more)", self.current, self.max));
        }
        Ok(())
    }
}

/// Charges a saved character may name, and the longest name: a character is a file, and a file
/// that comes over the network is not trusted with more.
const MAX_CHARGES: usize = 64;
const MAX_CHARGE_NAME: usize = 64;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChargeSave {
    pub name: String,
    pub current: u32,
    pub max: u32,
}

impl SaveSection for VitalsSave {
    const NAME: &'static str = "vitals";
    const VERSION: u32 = 1;
    const KIND: SaveKind = SaveKind::Character;

    fn capture(world: &World, body: Option<Entity>) -> Result<Self, String> {
        let body = body.ok_or("there is no body")?;
        Ok(VitalsSave {
            health: world.get::<Health>(body).map(|h| Pool { current: h.current, max: h.max }),
            stamina: world.get::<Stamina>(body).map(|s| Pool { current: s.current, max: s.max }),
            charges: world
                .get::<Charges>(body)
                .map(|c| c.0.iter().map(|c| ChargeSave { name: c.name.clone(), current: c.current, max: c.max }).collect())
                .unwrap_or_default(),
        })
    }

    fn apply(self, world: &mut World, body: Option<Entity>) -> Result<(), String> {
        let body = body.ok_or("there is no body")?;
        // Everything is checked first, so a section that is refused changes nothing.
        for (what, pool) in [("health", &self.health), ("stamina", &self.stamina)] {
            if let Some(pool) = pool {
                pool.check(what)?;
            }
        }
        if self.charges.len() > MAX_CHARGES || self.charges.iter().any(|c| c.name.len() > MAX_CHARGE_NAME || c.name.chars().any(char::is_control)) {
            return Err(format!("at most {MAX_CHARGES} charges, named in {MAX_CHARGE_NAME} characters without control characters"));
        }
        let mut entity = world.get_entity_mut(body).map_err(|e| e.to_string())?;
        let mut hp = None;
        if let (Some(saved), Some(mut health)) = (self.health, entity.get_mut::<Health>()) {
            health.max = saved.max;
            health.current = saved.current.clamp(0.0, saved.max);
            hp = Some(health.current);
        }
        if let (Some(saved), Some(mut stamina)) = (self.stamina, entity.get_mut::<Stamina>()) {
            stamina.max = saved.max;
            stamina.current = saved.current.clamp(0.0, saved.max);
            stamina.exhausted = false;
        }
        if !self.charges.is_empty() {
            let mut charges = match entity.get_mut::<Charges>() {
                Some(charges) => charges,
                None => {
                    entity.insert(Charges::default());
                    entity.get_mut::<Charges>().ok_or("cannot give the body charges")?
                }
            };
            for saved in self.charges {
                match charges.get_mut(&saved.name) {
                    Some(charge) => {
                        charge.max = saved.max;
                        charge.current = saved.current.min(saved.max);
                    }
                    None => charges.0.push(Charge { name: saved.name, current: saved.current.min(saved.max), max: saved.max }),
                }
            }
            charges.0.sort_by(|a, b| a.name.cmp(&b.name));
        }
        // A body that died since the save lives again, the way the dev `heal` brings it back.
        // A save made at zero health stays dead: `check_deaths` kills what has no health left.
        if let Some(hp) = hp.filter(|hp| *hp > 0.0)
            && revive(world, body)
        {
            world.trigger(Healed { entity: body, amount: hp, hp });
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use shards_world::manifest::{ManifestPoi, PlacedTile};

    fn manifest() -> RegionManifest {
        RegionManifest {
            format: FORMAT.into(),
            region: "meadow".into(),
            layout: Layout::Grid,
            seed: 42,
            version: 3,
            attempt: 1,
            fallback: None,
            size: [2, 1],
            tile_size: 16.0,
            origin: [-64.0, 48.0],
            tiles: vec![
                PlacedTile { at: [0, 0], size: [1, 1], tile: "gate".into(), rot: 90, decor: None },
                PlacedTile { at: [1, 0], size: [1, 1], tile: "grass".into(), rot: 0, decor: Some("rocks".into()) },
            ],
            pois: vec![ManifestPoi { id: "spawn.0".into(), poi: "spawn".into(), cell: [0, 0], pos: [-56.1, 0.25, 56.0], area: 0 }],
            links: vec![],
            spawn: Some("spawn.0".into()),
            content_hash: "00ff".into(),
        }
    }

    fn saved(manifest: RegionManifest) -> Value {
        let hash = format!("{:016x}", manifest.hash());
        let active = SavedRegion { id: manifest.region.clone(), seed: manifest.seed, version: manifest.version, manifest_hash: hash, manifest: Some(manifest) };
        serde_json::to_value(RegionSave { active: Some(active) }).unwrap()
    }

    fn read(value: Value) -> Result<RegionSave, String> {
        serde_json::from_value(value).map_err(|e| e.to_string())
    }

    #[test]
    fn a_saved_region_reads_only_with_the_manifest_its_hash_and_names_describe() {
        let good = saved(manifest());
        let back = read(good.clone()).unwrap();
        assert_eq!(back.active.as_ref().and_then(|a| a.manifest.clone()), Some(manifest()));
        assert_eq!(serde_json::to_value(&back).unwrap(), good, "reads back to the same data");

        let mut turned = good.clone();
        turned["active"]["manifest"]["tiles"][0]["rot"] = json!(180);
        let err = read(turned).unwrap_err();
        assert!(err.contains("the manifest of meadow seed 42 v3 is damaged: it hashes to ") && err.contains(&format!("the save says {:016x}", manifest().hash())), "{err}");

        let mut other = manifest();
        other.seed = 43;
        let mut swapped = saved(other);
        swapped["active"]["seed"] = json!(42);
        let err = read(swapped).unwrap_err();
        assert!(err.contains("is damaged: it is the manifest of meadow seed 43 v3"), "{err}");

        let mut foreign = manifest();
        foreign.format = "shards-region/0".into();
        let err = read(saved(foreign)).unwrap_err();
        assert!(err.contains("is of format `shards-region/0`; this build reads `shards-region/1`"), "{err}");

        let mut garbled = good;
        garbled["active"]["manifest_hash"] = json!("not hex");
        assert!(read(garbled).unwrap_err().contains("manifest_hash `not hex` is not hex"));
    }

    #[test]
    fn a_version_1_region_reads_as_one_without_a_manifest() {
        let v1 = json!({"active": {"id": "wilds", "seed": 42, "version": 1, "manifest_hash": "7bbfdc3b62623fbc"}});
        let migrated = region_from_v1(v1).unwrap();
        assert_eq!(migrated["active"]["manifest"], Value::Null);
        let active = read(migrated).unwrap().active.unwrap();
        assert_eq!(active.check(), Ok(0x7bbf_dc3b_6262_3fbc));
        assert_eq!((active.title(), active.manifest), ("wilds seed 42 v1".to_owned(), None));
        // Saved outside any region: nothing to add.
        assert_eq!(region_from_v1(json!({"active": null})).unwrap(), json!({"active": null}));
    }

    #[test]
    fn a_pool_of_another_machine_is_checked_before_it_is_applied() {
        assert!(Pool { current: 40.0, max: 100.0 }.check("health").is_ok());
        assert!(Pool { current: 0.0, max: 0.0 }.check("health").is_ok(), "a body with nothing left is a pool");
        assert!(Pool { current: 500.0, max: 100.0 }.check("health").is_ok(), "a current above the ceiling is clamped, not refused");
        for bad in [Pool { current: 1.0, max: -1.0 }, Pool { current: f32::NAN, max: 10.0 }, Pool { current: 1.0, max: f32::INFINITY }] {
            let why = bad.check("stamina").unwrap_err();
            assert!(why.starts_with("stamina ") && why.contains("is not a pool"), "{why}");
        }
    }
}
