//! `ai/status`: what a script checks before it drives a game.
//!
//! The body is built here, without the transport, so in-process drivers (tests,
//! scenario runners) read the same fields `shards status` prints. Engine modules add
//! their own fields with [`AiAppExt::add_ai_status`](crate::AiAppExt::add_ai_status)
//! (the engine version and modules, for one).

use bevy::prelude::*;
use serde_json::{json, Value};

use crate::id::AiIndex;
use crate::journal::AiJournal;
use crate::rng::SimSeed;
use crate::session::{AgentSessions, Scope};
use crate::sim::{self, SimGate};
use crate::tools::AiTools;

/// Builds one extra `ai/status` field from the world.
pub type StatusField = fn(&World) -> Value;

/// Extra `ai/status` fields, by key, in registration order. A second field with the
/// same key replaces the first.
#[derive(Resource, Default)]
pub struct AiStatusFields(Vec<(String, StatusField)>);

impl AiStatusFields {
    /// Adds (or replaces) field `key`.
    pub fn add(&mut self, key: &str, field: StatusField) {
        self.0.retain(|(k, _)| k != key);
        self.0.push((key.to_owned(), field));
    }
}

/// The `ai/status` body (without `instance`, which only the BRP transport knows).
/// `loading` lists what holds the simulation: empty once the game can tick.
pub fn status(world: &World) -> Value {
    let seed = world.get_resource::<SimSeed>().map(|s| s.0);
    let loading = world.get_resource::<SimGate>().map(SimGate::reasons).unwrap_or_default();
    let mut body = json!({
        "tick": sim::tick(world),
        "paused": sim::is_paused(world),
        "loading": loading,
        "speed": sim::speed(world),
        "tick_hz": (1.0 / world.resource::<Time<Fixed>>().timestep().as_secs_f64() * 1000.0).round() / 1000.0,
        "seed": seed,
        "entities": world.get_resource::<AiIndex>().map_or(0, AiIndex::len),
        "journal_seq": world.get_resource::<AiJournal>().map_or(0, AiJournal::last_seq),
        "agents": world.get_resource::<AgentSessions>().map(AgentSessions::names).unwrap_or_default(),
        "tools": world.get_resource::<AiTools>().map(|t| t.names(Scope::Dev)).unwrap_or_default(),
        "replay": world.get_resource::<crate::AiReplay>().map(|_| crate::replay::status(world)),
        "perf": crate::perf::status(world),
    });
    if let (Some(fields), Some(map)) = (world.get_resource::<AiStatusFields>(), body.as_object_mut()) {
        for (key, field) in &fields.0 {
            map.insert(key.clone(), field(world));
        }
    }
    body
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::AiAppExt;

    #[test]
    fn status_lists_what_holds_the_simulation_and_the_game_fields() {
        let mut app = App::new();
        app.add_plugins(bevy::time::TimePlugin)
            .add_plugins(crate::AiCorePlugin { remote_port: None, crash_replays: false, ..Default::default() })
            .add_ai_status("engine", |_| json!({"version": "1.2.3"}));
        app.finish();
        app.cleanup();
        assert_eq!(status(app.world())["loading"], json!([]));
        app.world_mut().resource_mut::<SimGate>().hold("content", "2 files left");
        let body = status(app.world());
        assert_eq!(body["loading"], json!(["content: 2 files left"]));
        assert_eq!(body["engine"], json!({"version": "1.2.3"}));
    }
}
