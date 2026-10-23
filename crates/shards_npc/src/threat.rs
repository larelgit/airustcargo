//! Threats (F11): what an NPC whose perception notices threats (`notice: Threats`) holds for one.
//!
//! A **threat** is a body, of any team, that the NPC saw attack while armed (running an action that
//! fires a shot, or one with hit windows while it holds a weapon, `Arsenal::in_hands`) or heard fire a
//! shot (a [`ShotFired`] within the shot's `noise` times the NPC's `hearing`). It stays one for the
//! NPC's `memory` seconds after the last time it saw or heard that, in its [`NpcThreats`]; meanwhile
//! the NPC notices it the way another NPC notices a hostile fighter (sight, sounds, memory, focus),
//! and it notices nothing else: a hostile team alone is no threat, and a guard of its own side who
//! opens fire is one. Only NPCs whose perception notices threats carry an [`NpcThreats`]: the others
//! notice by the team rule alone, as before.
//!
//! - **Seen**: in the perception pass ([`perceive`](crate::perception)), a body the NPC sees (its
//!   sight range, field of view and clear ray) that runs such an action now is marked.
//! - **Heard**: [`shot_threat`] marks the shooter when the shot is fired, where the simulation runs.
//! - **Order**: marks are kept by `AiId`, so the order NPCs or shots come in changes nothing.

use ai_core::AiId;
use bevy::prelude::*;
use shards_action::{fire_effects, ActionDef, ActionState, Arsenal, ShotFired};
use shards_content::ContentDb;
use shards_core::NetRole;

use crate::brain::{Npc, NpcBrain, NpcCtx};
use crate::def::PerceptionDef;

/// The bodies an NPC that notices threats holds for one, by id (F11). Gameplay state: kept in replays
/// and snapshots. Only NPCs whose perception has `notice: Threats` carry it.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct NpcThreats {
    /// By id.
    pub known: Vec<Threat>,
}

/// One body an NPC holds for a threat.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub struct Threat {
    pub id: AiId,
    /// Seconds since the NPC last saw it attack or heard it fire.
    pub age: f32,
}

impl NpcThreats {
    /// Whether `id` is held for a threat.
    pub fn holds(&self, id: AiId) -> bool {
        self.known.binary_search_by_key(&id, |t| t.id).is_ok()
    }

    /// Marks `id` a threat now.
    pub(crate) fn mark(&mut self, id: AiId) {
        match self.known.binary_search_by_key(&id, |t| t.id) {
            Ok(i) => self.known[i].age = 0.0,
            Err(i) => self.known.insert(i, Threat { id, age: 0.0 }),
        }
    }

    /// The ids held, ascending.
    pub fn ids(&self) -> Vec<AiId> {
        self.known.iter().map(|t| t.id).collect()
    }
}

/// Whether a body running `state` (holding `arsenal`) attacks while armed, as an NPC that notices
/// threats sees it: an action of its own (not a forced reaction) that fires a shot, or that has hit
/// windows while a weapon is in its hands.
pub(crate) fn attacks_armed(db: &ContentDb, state: Option<&ActionState>, arsenal: Option<&Arsenal>) -> bool {
    let Some(run) = state.and_then(|s| s.current.as_ref()).filter(|r| !r.forced) else { return false };
    let Some(def) = db.get::<ActionDef>(&run.action) else { return false };
    fire_effects(def).next().is_some() || (!def.hits.is_empty() && arsenal.is_some_and(|a| a.in_hands().is_some()))
}

/// A shot heard is a threat: every active NPC that notices threats and is within the shot's `noise`
/// times its `hearing` marks the shooter (never itself). Only where the simulation runs (a co-op client
/// sees the host's shot; its NPCs think on the host).
pub(crate) fn shot_threat(
    shot: On<ShotFired>,
    role: Option<Res<NetRole>>,
    db: Res<ContentDb>,
    ids: Query<&AiId>,
    mut npcs: Query<(Entity, &Npc, &NpcBrain, &Transform, &mut NpcThreats)>,
) {
    let shot = shot.event();
    if !shards_core::simulates(role) || !(shot.noise > 0.0 && shot.from.is_finite()) {
        return;
    }
    let Ok(&shooter) = ids.get(shot.shooter) else { return };
    for (entity, npc, brain, tf, mut threats) in &mut npcs {
        if !brain.active || entity == shot.shooter {
            continue;
        }
        let hearing = db.get::<PerceptionDef>(&npc.perception).map_or(PerceptionDef::default().hearing, |d| d.hearing);
        if (shot.from - tf.translation).length() <= shot.noise * hearing {
            threats.mark(shooter);
        }
    }
}

/// The threats an NPC knows of now: those it holds ([`NpcThreats`]) whose place it knows
/// ([`Perceived`](crate::Perceived)), nearest first (ties by id), with where it knows them.
pub fn known_threats(c: &NpcCtx) -> Vec<(AiId, Vec3)> {
    let Some(threats) = c.get::<NpcThreats>() else { return Vec::new() };
    let mut out: Vec<(AiId, Vec3)> = threats.known.iter().filter_map(|t| c.perceived.get(t.id).map(|k| (t.id, k.pos))).collect();
    let here = c.pos;
    out.sort_by(|a, b| shards_core::flat(a.1 - here).length().total_cmp(&shards_core::flat(b.1 - here).length()).then(a.0.cmp(&b.0)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn marks_stay_sorted_and_a_new_mark_starts_the_clock_again() {
        let mut t = NpcThreats::default();
        t.mark(AiId(9));
        t.mark(AiId(3));
        t.known[1].age = 2.0;
        t.mark(AiId(9));
        assert_eq!(t.ids(), [AiId(3), AiId(9)]);
        assert_eq!(t.known[1].age, 0.0);
        assert!(t.holds(AiId(3)) && !t.holds(AiId(4)));
    }
}
