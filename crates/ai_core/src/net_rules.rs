//! Replication rules: what a host sends its clients.
//!
//! There is no separate "replicated" mark. A component is replicated when it is state, by
//! the rules the engine already has: the components a snapshot holds
//! ([`is_snapshot_type`](crate::snapshot::is_snapshot_type)), minus the ones replay checks
//! leave out ([`replay_ignore`](crate::AiAppExt::replay_ignore): cosmetics and caches), minus
//! the ones a module says stay on each machine ([`net_ignore`](crate::AiAppExt::net_ignore):
//! "this is my body", prediction bookkeeping). That is the [`state_hash`](crate::replay::state_hash)
//! rule plus the identity markers (`AiKind`, `AiVisible`, `AiLandmark`) the hash leaves out:
//! a client needs them to see the entity at all.
//!
//! Resources: every [`add_snapshot_resource`](crate::AiAppExt::add_snapshot_resource) except
//! the [`net_ignore_resource`](crate::AiAppExt::net_ignore_resource) ones (id counters, which
//! each machine keeps for itself).
//!
//! [`net_predicted`](crate::AiAppExt::net_predicted) marks the components a client predicts
//! for its own body (they travel with the input acknowledgement, not with the rest).
//!
//! Host and client must agree on all of this: [`rules_hash`] goes into the handshake.

use std::any::TypeId;
use std::collections::BTreeSet;

use bevy::platform::collections::HashMap;
use bevy::prelude::*;

use crate::replay::{Fnv, ReplayHashing};
use crate::snapshot::{is_snapshot_type, SnapshotRegistry};

/// What [`AiAppExt`](crate::AiAppExt)'s `net_*` registered, by type: the type name for
/// messages and the protocol check.
#[derive(Resource, Default, Debug, Clone)]
pub struct NetRules {
    ignored: HashMap<TypeId, &'static str>,
    ignored_resources: HashMap<TypeId, &'static str>,
    predicted: HashMap<TypeId, &'static str>,
}

impl NetRules {
    pub(crate) fn ignore(&mut self, type_id: TypeId, name: &'static str) {
        self.ignored.insert(type_id, name);
    }

    pub(crate) fn ignore_resource(&mut self, type_id: TypeId, name: &'static str) {
        self.ignored_resources.insert(type_id, name);
    }

    pub(crate) fn predict(&mut self, type_id: TypeId, name: &'static str) {
        self.predicted.insert(type_id, name);
    }

    /// Component `type_id` stays on each machine.
    pub fn is_ignored(&self, type_id: TypeId) -> bool {
        self.ignored.contains_key(&type_id)
    }

    /// Resource `type_id` stays on each machine.
    pub fn is_resource_ignored(&self, type_id: TypeId) -> bool {
        self.ignored_resources.contains_key(&type_id)
    }

    /// A client predicts component `type_id` for its own body.
    pub fn is_predicted(&self, type_id: TypeId) -> bool {
        self.predicted.contains_key(&type_id)
    }

    /// The predicted components, by type.
    pub fn predicted(&self) -> impl Iterator<Item = TypeId> + '_ {
        self.predicted.keys().copied()
    }
}

/// Whether component type `type_id` (at `type_path`) is replicated in `world`: a snapshot
/// type that neither `replay_ignore` nor `net_ignore` leaves out.
pub fn is_replicated_type(world: &World, type_id: TypeId, type_path: &str) -> bool {
    is_snapshot_type(type_id, type_path)
        && !world.get_resource::<ReplayHashing>().is_some_and(|h| h.is_ignored(type_id))
        && !world.get_resource::<NetRules>().is_some_and(|r| r.is_ignored(type_id))
}

/// The replicated components `entity` has (reflected ones), by type path, sorted.
pub fn replicated_components(world: &World, entity: Entity) -> Vec<&'static str> {
    let Ok(infos) = world.inspect_entity(entity) else {
        return Vec::new();
    };
    let registry = world.resource::<AppTypeRegistry>().read();
    let mut paths: Vec<&'static str> = infos
        .filter_map(|(_, info)| registry.get(info.type_id()?))
        .filter(|r| r.data::<ReflectComponent>().is_some())
        .map(|r| (r.type_id(), r.type_info().type_path()))
        .filter(|(id, path)| is_replicated_type(world, *id, path))
        .map(|(_, path)| path)
        .collect();
    paths.sort_unstable();
    paths
}

/// The replicated resources (snapshot resources minus `net_ignore_resource`), by type path,
/// in registration order.
pub fn replicated_resources(world: &World) -> Vec<&'static str> {
    let rules = world.get_resource::<NetRules>();
    world
        .get_resource::<SnapshotRegistry>()
        .map(|registry| {
            registry
                .resource_types()
                .filter(|(id, _)| !rules.is_some_and(|r| r.is_resource_ignored(*id)))
                .map(|(_, name)| name)
                .collect()
        })
        .unwrap_or_default()
}

/// A hash of the rules: every reflected component type that is replicated, the replicated
/// resources and the predicted components, by type path. Host and client built by the same
/// code get the same number; a client whose number differs reads the host's state wrongly.
pub fn rules_hash(world: &World) -> u64 {
    let registry = world.resource::<AppTypeRegistry>().read();
    let components: BTreeSet<&str> = registry
        .iter()
        .filter(|r| r.data::<ReflectComponent>().is_some())
        .map(|r| (r.type_id(), r.type_info().type_path()))
        .filter(|(id, path)| is_replicated_type(world, *id, path))
        .map(|(_, path)| path)
        .collect();
    let resources: BTreeSet<&str> = replicated_resources(world).into_iter().collect();
    let predicted: BTreeSet<&str> = world
        .get_resource::<NetRules>()
        .map(|rules| {
            rules
                .predicted
                .iter()
                .map(|(id, name)| registry.get(*id).map_or(*name, |r| r.type_info().type_path()))
                .collect()
        })
        .unwrap_or_default();
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    for (section, paths) in [("components", &components), ("resources", &resources), ("predicted", &predicted)] {
        h.bytes(section.as_bytes());
        h.u64(paths.len() as u64);
        for path in paths {
            h.bytes(path.as_bytes());
        }
    }
    h.0
}

#[cfg(test)]
mod tests {
    use bevy::prelude::*;

    use crate::prelude::*;

    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    struct Hp(f32);

    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    struct Sparkle;

    #[derive(Component, Reflect, Default)]
    #[reflect(Component, Default)]
    struct Mine;

    #[derive(Resource, Reflect, Default)]
    #[reflect(Resource, Default)]
    struct Counter(u32);

    fn game(extra_ignore: bool) -> App {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin)
            .add_plugins(crate::AiCorePlugin { remote_port: None, crash_replays: false, ..Default::default() })
            .register_type::<Hp>()
            .register_type::<Sparkle>()
            .register_type::<Mine>()
            .init_resource::<Counter>()
            .add_snapshot_resource::<Counter>()
            .replay_ignore::<Sparkle>()
            .net_ignore::<Mine>()
            .net_predicted::<Transform>();
        if extra_ignore {
            app.net_ignore_resource::<Counter>();
        }
        app
    }

    #[test]
    fn replicated_is_state_minus_replay_and_net_ignores_with_the_identity_markers() {
        let mut app = game(false);
        let e = app.world_mut().spawn((AiKind::new("Hero"), AiPlayer, Hp(3.0), Sparkle, Mine, Transform::default())).id();
        let paths = super::replicated_components(app.world(), e);
        assert_eq!(
            paths,
            [
                "ai_core::describe::AiKind",
                "ai_core::id::AiVisible",
                "ai_core::net_rules::tests::Hp",
                "bevy_transform::components::transform::Transform",
            ],
            "AiPlayer is the engine's own opt-out; Sparkle is replay-ignored, Mine net-ignored"
        );
        assert_eq!(super::replicated_resources(app.world()), ["ai_core::net_rules::tests::Counter"]);
        assert_ne!(super::rules_hash(app.world()), super::rules_hash(game(true).world()), "the rules are in the hash");
        assert_eq!(super::rules_hash(app.world()), super::rules_hash(game(false).world()));
        assert!(super::replicated_resources(game(true).world()).is_empty());
    }
}
