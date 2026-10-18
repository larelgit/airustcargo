//! What the session needs of the engine's other modules: which NPC internals stay on the host,
//! where joining players appear, what a client restarts after taking the host's world.

use ai_core::AiAppExt;
use bevy::prelude::*;
use serde_json::json;
use shards_motion::{ColliderId, ColliderIds};
use shards_action::combat::{Blocked, Damaged, Died, Evaded, GuardBroken, Parried, PerfectEvaded, Reacted, Staggered};
use shards_action::machine::{ActionRejected, ActionStarted, Healed, Telegraph};
use shards_action::ActionSound;
use shards_net::{NetEventAppExt, NetHooks};
use shards_npc::{AttackToken, NavAgent, NpcDrive, Perceived};
use shards_world::ActiveRegion;

/// Joins `shards_net` to the rest of the engine.
pub struct EngineNetPlugin;

impl Plugin for EngineNetPlugin {
    fn build(&self, app: &mut App) {
        // An NPC's senses, path and steering are the host's working state: a client shows the
        // NPC (its motion, action, health, brain state), it does not think for it.
        app.net_ignore::<Perceived>().net_ignore::<NavAgent>().net_ignore::<NpcDrive>().net_ignore::<AttackToken>();
        // What a client shows of the host's fight: hits, guards, deaths, warnings, sounds.
        app.net_event::<Damaged>()
            .net_event::<Blocked>()
            .net_event::<Parried>()
            .net_event::<Evaded>()
            .net_event::<PerfectEvaded>()
            .net_event::<Reacted>()
            .net_event::<GuardBroken>()
            .net_event::<Staggered>()
            .net_event::<Died>()
            .net_event::<Healed>()
            .net_event::<ActionStarted>()
            .net_event::<ActionRejected>()
            .net_event::<Telegraph>()
            // An action's `Sound` effect runs on the host only (a client's prediction starts no effects):
            // the client plays the host's sound, like the host's hit sparks.
            .net_event::<ActionSound>();
        let mut hooks = app.world_mut().get_resource_or_init::<NetHooks>();
        // Colliders a client makes itself are numbered apart from the host's.
        hooks.after_restore = |world| {
            if let Some(mut ids) = world.get_resource_mut::<ColliderIds>() {
                ids.restart_at(ColliderId(shards_net::replicate::LOCAL_BASE));
            }
        };
        hooks.spawn_point = |world, n| world.get_resource::<ActiveRegion>().map(|r| r.spawn_point(n as usize));
        hooks.describe_world = |world| {
            world.get_resource::<ActiveRegion>().map(|r| {
                json!({ "region": r.manifest.region, "version": r.manifest.version, "seed": r.manifest.seed, "manifest": format!("{:016x}", r.hash) })
            })
        };
    }
}
