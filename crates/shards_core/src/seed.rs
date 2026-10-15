//! The seed lock: whether the world seed is final yet.
//!
//! In single player and on a host it is from the start (`--seed`). A client joining a
//! networked game learns the seed from its host, after content has loaded: it holds the
//! lock ([`hold_seed`]) until then, and [`adopt_seed`] releases it. Loading that depends
//! on the seed (a generated start region) waits for [`seed_ready`]; the loading gate holds
//! the simulation under `seed` meanwhile.

use ai_core::{SimConfig, SimSeed};
use bevy::prelude::*;

use crate::{EngineConfig, LoadStatus};

/// Held while the world seed is not known yet, with the reason
/// (`net: waiting for the host's seed`).
#[derive(Resource, Clone, Debug, Default)]
pub struct SeedLock {
    held: Option<String>,
}

impl SeedLock {
    /// Why the seed is not final, if it is not.
    pub fn reason(&self) -> Option<&str> {
        self.held.as_deref()
    }
}

/// Whether the world seed is final: seed-dependent loading may go on.
pub fn seed_ready(world: &World) -> bool {
    world.get_resource::<SeedLock>().is_none_or(|lock| lock.held.is_none())
}

/// Holds the seed (and with it the loading gate) until [`adopt_seed`].
pub fn hold_seed(world: &mut World, reason: &str) {
    world.get_resource_or_init::<SeedLock>().held = Some(reason.to_owned());
}

/// Makes `seed` the world seed (`EngineConfig.seed`, `SimSeed`, `SimConfig.seed`) and
/// releases the lock.
pub fn adopt_seed(world: &mut World, seed: u64) {
    if let Some(mut config) = world.get_resource_mut::<EngineConfig>() {
        config.seed = seed;
    }
    if let Some(mut config) = world.get_resource_mut::<SimConfig>() {
        config.seed = seed;
    }
    world.insert_resource(SimSeed(seed));
    world.get_resource_or_init::<SeedLock>().held = None;
}

/// The loading gate's view (`seed`).
pub(crate) fn seed_status(world: &World) -> LoadStatus {
    match world.get_resource::<SeedLock>().and_then(SeedLock::reason) {
        Some(reason) => LoadStatus::Pending(reason.to_owned()),
        None => LoadStatus::Ready,
    }
}
