//! The engine's own save sections.
//!
//! The save module ([`shards_save`]) knows nothing of motion, combat or regions; the
//! umbrella, which knows them all, says what of theirs a save holds:
//!
//! | section | kind | holds |
//! |---|---|---|
//! | `region` | world | the region in the world: its id, seed, generator version and manifest hash. Applying it generates the region again and changes to it through the world module's region change (when it differs from the one there), and refuses, changing nothing, if it no longer comes out the same |
//! | `position` | character | the body's feet, its rotation (characters turn about Y: that is their facing) and the region it stood in. Applied onto a body in the same region only; in another one the body stays where it is and the journal says why |
//! | `vitals` | character | health, stamina and charges. Applied to the components the body already has (a fresh body has what its prefab gives it); a charge the body lacks is added. A body that died since the save is revived (`shards_action::revive`, as the dev `heal` does, and `Healed` follows); a save made at zero health leaves it dead |
//!
//! A game replaces one with a section of the same name (`position` as "restore at the last
//! camp"): the new one takes its place in the order sections are applied.

use ai_core::journal::{self, JournalKind};
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_action::{revive, Charge, Charges, Health, Healed, Stamina};
use shards_content::ContentDb;
use shards_save::{SaveAppExt, SaveKind, SaveSection};
use shards_world::{generate, region, ActiveRegion, WorldGen};

/// Adds the engine's sections to the registry. Part of [`EnginePlugins`](crate::EnginePlugins).
pub struct EngineSavePlugin;

impl Plugin for EngineSavePlugin {
    fn build(&self, app: &mut App) {
        // World first, then the character: the position is applied onto the region just built.
        app.add_save_section::<RegionSave>().add_save_section::<PositionSave>().add_save_section::<VitalsSave>();
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

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SavedRegion {
    pub id: String,
    pub seed: u64,
    /// The generator version of the region's definition.
    pub version: u32,
    /// [`RegionManifest::hash`](shards_world::RegionManifest::hash), hex: what the world was.
    pub manifest_hash: String,
}

impl SaveSection for RegionSave {
    const NAME: &'static str = "region";
    const VERSION: u32 = 1;
    const KIND: SaveKind = SaveKind::World;

    fn capture(world: &World, _: Option<Entity>) -> Result<Self, String> {
        let active = world.get_resource::<ActiveRegion>().map(|r| SavedRegion {
            id: r.manifest.region.clone(),
            seed: r.manifest.seed,
            version: r.manifest.version,
            manifest_hash: format!("{:016x}", r.hash),
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
        let want = u64::from_str_radix(&saved.manifest_hash, 16).map_err(|_| format!("manifest_hash `{}` is not hex", saved.manifest_hash))?;
        let title = format!("{} seed {} v{}", saved.id, saved.seed, saved.version);
        if world.get_resource::<ActiveRegion>().is_some_and(|r| r.hash == want) {
            return Ok(());
        }
        let worldgen = world.get_resource::<WorldGen>().ok_or("the world module is not installed")?.clone();
        // Pure: the same seed and content make the same manifest. Checked before anything changes.
        let generated = generate::generate(world.resource::<ContentDb>(), &worldgen, &saved.id, saved.seed).map_err(|e| {
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
