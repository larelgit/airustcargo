//! The engine rules `npc.state_known` and `npc.tokens_within_limit`.

use ai_core::{with_query, AiId, Offenders};
use bevy::prelude::*;
use shards_action::Dead;
use shards_content::ContentDb;
use shards_core::PlayerId;

use crate::brain::{Npc, NpcBrain};
use crate::def::{BehaviorDef, DirectorDef};
use crate::director::{FightingReadOnly, Grant};

/// The engine rule `npc.state_known`: an NPC's `NpcBrain.state` is a state of its behaviour
/// definition (a key of `BehaviorDef.states`; a game's own state hook appears there under the
/// `kind` that names it, so hook states are covered).
///
/// Three carve-outs, each a state the engine itself leaves behind:
/// - an **empty** state: it is empty until the NPC's first decided tick, and an NPC spawned
///   after the Decide step of a tick is still empty at the end of it;
/// - an **inactive** NPC (`brain.active == false`: dormant or dead): the brain passes it
///   through unchanged, so a state a content hot reload removed stays on it until it wakes (an
///   active NPC is repaired the same tick);
/// - an NPC whose **behaviour is not in content** (reported once as a content problem, and the
///   NPC stands still), or a world without a `ContentDb`.
pub fn state_known(world: &World) -> Option<String> {
    let db = world.get_resource::<ContentDb>()?;
    let mut bad = Offenders::default();
    with_query::<(&AiId, &Npc, &NpcBrain), (), _>(world, |query| {
        for (id, npc, brain) in query.iter(world) {
            if brain.state.is_empty() || !brain.active {
                continue;
            }
            let Some(behavior) = db.get::<BehaviorDef>(&npc.behavior) else { continue };
            if !behavior.states.contains_key(&brain.state) {
                bad.note(*id, || format!("NpcBrain.state = `{}` is not a state of behavior `{}`", brain.state, npc.behavior));
            }
        }
    })?;
    bad.report()
}

/// What [`audit_tokens`] found on the last tick: the text of a broken token rule, or `None`.
///
/// Not simulation state: it is recomputed from the components every tick, right after the
/// director ran, so it is not part of a snapshot, a replay or a replay check. It exists only so
/// that [`tokens_within_limit`], which runs at the end of the tick, judges the director's
/// books at the moment the director closed them.
#[derive(Resource, Default, Debug, PartialEq)]
pub struct TokenAudit(pub Option<String>);

/// Right after [`direct`](crate::director::direct) (the engine adds it only with
/// `engine_invariants`): checks the books the director just closed, with the same inputs it
/// used (the living players, each grant's director, `DirectorDef::limit`).
#[allow(clippy::needless_pass_by_value, reason = "a Bevy system")]
pub(crate) fn audit_tokens(
    mut audit: ResMut<TokenAudit>,
    db: Res<ContentDb>,
    players: Query<(), (With<PlayerId>, Without<Dead>)>,
    npcs: Query<FightingReadOnly>,
) {
    let players = players.iter().count() as u32;
    // Tokens held per director (named by the grant, as `direct` counts them).
    let mut held: Vec<(&str, u32)> = Vec::new();
    let mut bad = Offenders::default();
    for f in npcs.iter() {
        let Some(grant) = &f.token.held else { continue };
        if f.dead {
            bad.note(*f.id, || format!("AttackToken is held by a dead body (director `{}`)", grant.director));
        }
        match held.iter_mut().find(|(d, _)| *d == grant.director) {
            Some((_, n)) => *n += 1,
            None => held.push((&grant.director, 1)),
        }
    }
    for &(director, count) in &held {
        let limit = DirectorDef::of(&db, director).limit(players);
        if count <= limit {
            continue;
        }
        // Over the limit is allowed only while every holder is in flight: an attack under way
        // keeps its token to its end.
        for f in npcs.iter() {
            if let Some(grant) = f.token.held.as_ref().filter(|g: &&Grant| g.director == director && !g.in_flight()) {
                bad.note(*f.id, || {
                    format!(
                        "AttackToken: director `{}` holds {count} tokens, over its limit {limit} for {players} players, \
                         and this one is not in flight",
                        grant.director
                    )
                });
            }
        }
    }
    let text = bad.report();
    if audit.0 != text {
        audit.0 = text;
    }
}

/// The engine rule `npc.tokens_within_limit`: after the director ran, no token is held by a dead
/// body, and every director holds no more tokens than its limit for the living players, unless
/// each holder beyond it has its attack in flight.
///
/// It reports what [`audit_tokens`] found right after `direct`, not the state at the end of the
/// tick, because the end of the tick is not where the books are closed. A holder that dies in
/// `Resolve` still holds its token until the next tick's `direct` takes it back, and a player
/// that dies there lowers the limit that `direct` applies only next tick; judged at the end of
/// the tick, every ordinary kill would fire the rule. And `direct` deliberately lets an attack
/// in flight keep its token above a lowered limit until the attack ends. Without the audit
/// (the world has no `TokenAudit`: the engine rules were off when the plugin was built) the
/// rule passes.
pub fn tokens_within_limit(world: &World) -> Option<String> {
    world.get_resource::<TokenAudit>()?.0.clone()
}

#[cfg(test)]
mod tests {
    use ai_core::AiIndex;
    use bevy::ecs::system::RunSystemOnce;
    use shards_action::Fighter;

    use super::*;

    fn world() -> World {
        let mut w = World::new();
        w.init_resource::<AiIndex>();
        w.init_resource::<ContentDb>();
        w.init_resource::<TokenAudit>();
        w
    }

    fn holder(w: &mut World, in_flight: bool) -> Entity {
        let mut grant = Grant::for_tests(AiId(999));
        grant.director = "default".into();
        grant.pressed = in_flight.then_some(1);
        let e = w.spawn((Npc::default(), Fighter::default(), Transform::default())).id();
        w.get_mut::<crate::director::AttackToken>(e).unwrap().held = Some(grant);
        e
    }

    fn audit(w: &mut World) -> Option<String> {
        w.run_system_once(audit_tokens).unwrap();
        w.resource::<TokenAudit>().0.clone()
    }

    #[test]
    fn tokens_within_the_limit_pass() {
        let mut w = world();
        holder(&mut w, false);
        holder(&mut w, false);
        assert_eq!(audit(&mut w), None, "two tokens, limit 2 for the engine's default director");
    }

    #[test]
    fn a_token_beyond_the_limit_is_found_unless_every_holder_beyond_it_is_in_flight() {
        let mut w = world();
        holder(&mut w, true);
        holder(&mut w, true);
        holder(&mut w, true);
        assert_eq!(audit(&mut w), None, "a player died mid-attack: the attacks keep their tokens");
        let idle = holder(&mut w, false);
        let text = audit(&mut w).expect("a fourth token that is not in flight");
        let id = w.get::<AiId>(idle).unwrap();
        assert!(text.starts_with(&format!("{id} AttackToken: director `default` holds 4 tokens, over its limit 2")), "{text}");
    }

    #[test]
    fn a_dead_holder_is_found_after_the_director_ran() {
        let mut w = world();
        let e = holder(&mut w, false);
        assert_eq!(audit(&mut w), None);
        w.entity_mut(e).insert(Dead);
        let text = audit(&mut w).expect("the director takes a dead holder's token back");
        assert!(text.contains("held by a dead body"), "{text}");
        // The rule reports what the audit found.
        assert_eq!(tokens_within_limit(&w), Some(text));
    }
}
