//! `NetDef`: the session's numbers, as content (`assets/content/**/*.net.ron`, id `default`).

use ai_core::Scope;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::{ContentDb, ContentDef, PrefabDef, Refs};

/// How a networked session plays: slots, interest, send rate, timeouts. One definition, id
/// `default`; without one the engine's defaults (these field defaults) apply.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct NetDef {
    /// Players in a session, the host's included.
    pub max_players: u8,
    /// The prefab a joining player's body is spawned from (default spawner).
    pub player_prefab: String,
    /// Meters around a client's body within which it is sent the world.
    pub interest_radius: f32,
    /// Meters beyond the radius an entity in view stays in view (no flicker at the edge).
    pub interest_hysteresis: f32,
    /// Ticks between state sends (2: 30 Hz at 60 ticks/s).
    pub send_every: u32,
    /// Ticks between desync checks (on send ticks).
    pub desync_every: u32,
    /// Prediction error, in meters, above which the own body snaps instead of blending (C4).
    pub snap_distance: f32,
    /// Share of the correction still to make that moves the own body each tick, until the
    /// next comparison with the host's state says what is left (C4).
    pub correction_rate: f32,
    /// Seconds of silence after which a peer counts as gone.
    pub timeout_secs: f32,
    /// Seconds a player who left keeps their slot and body for a rejoin with the same token.
    pub rejoin_grace_secs: f32,
    /// The highest scope a client's forwarded calls run with on the host.
    pub guest_scope: Scope,
    /// A client whose copy differs from the host's is sent the whole world again.
    pub resync_on_desync: bool,
    /// Unacknowledged input frames a client repeats in every packet.
    pub input_redundancy: u32,
    /// Largest state delta, in KB: beyond it the farthest entities wait for the next one.
    pub delta_cap_kb: u32,
}

impl Default for NetDef {
    fn default() -> Self {
        Self {
            max_players: 3,
            player_prefab: "player".into(),
            interest_radius: 60.0,
            interest_hysteresis: 10.0,
            send_every: 2,
            desync_every: 60,
            snap_distance: 1.5,
            correction_rate: 0.15,
            timeout_secs: 10.0,
            rejoin_grace_secs: 120.0,
            guest_scope: Scope::Player,
            resync_on_desync: true,
            input_redundancy: 32,
            delta_cap_kb: 256,
        }
    }
}

impl NetDef {
    /// What is wrong with the numbers, if anything.
    pub fn problems(&self) -> Vec<String> {
        let mut out = Vec::new();
        let mut check = |ok: bool, text: &str| {
            if !ok {
                out.push(text.to_owned());
            }
        };
        check((1..=16).contains(&self.max_players), "max_players must be 1..=16");
        check(self.interest_radius > 0.0 && self.interest_radius.is_finite(), "interest_radius must be above 0");
        check(self.interest_hysteresis >= 0.0 && self.interest_hysteresis.is_finite(), "interest_hysteresis must be 0 or more");
        check((1..=60).contains(&self.send_every), "send_every must be 1..=60 ticks");
        check(self.desync_every >= 1, "desync_every must be at least 1 tick");
        check(self.snap_distance > 0.0, "snap_distance must be above 0");
        check(self.correction_rate > 0.0 && self.correction_rate <= 1.0, "correction_rate must be in (0, 1]");
        check(self.timeout_secs >= 1.0, "timeout_secs must be at least 1");
        check(self.rejoin_grace_secs >= 0.0, "rejoin_grace_secs must be 0 or more");
        check((1..=64).contains(&self.input_redundancy), "input_redundancy must be 1..=64");
        check((16..=4096).contains(&self.delta_cap_kb), "delta_cap_kb must be 16..=4096");
        out
    }
}

impl ContentDef for NetDef {
    const KIND: &'static str = "net";

    fn validate(&self, id: &str, refs: &mut Refs) {
        if id != "default" {
            refs.error("the session reads only the definition `default`; rename this one or merge it into `default`");
        }
        for problem in self.problems() {
            refs.error(problem);
        }
        refs.require::<PrefabDef>(&self.player_prefab, "player_prefab");
    }
}

/// The numbers a game without a `net` definition uses (tests change them here instead of
/// adding content, which would change the content hash).
#[derive(Resource, Clone, Debug, Default)]
pub struct NetDefaults(pub NetDef);

/// The session's numbers: content `default`, else [`NetDefaults`].
pub fn net_def(world: &World) -> NetDef {
    world
        .get_resource::<ContentDb>()
        .and_then(|db| db.get::<NetDef>("default").cloned())
        .or_else(|| world.get_resource::<NetDefaults>().map(|d| d.0.clone()))
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_defaults_are_the_contract_and_valid() {
        let def = NetDef::default();
        assert_eq!((def.max_players, def.send_every, def.desync_every, def.input_redundancy, def.delta_cap_kb), (3, 2, 60, 32, 256));
        assert!(def.problems().is_empty());
        let bad = NetDef { send_every: 0, correction_rate: 2.0, ..NetDef::default() };
        assert_eq!(bad.problems(), ["send_every must be 1..=60 ticks", "correction_rate must be in (0, 1]"]);
    }
}
