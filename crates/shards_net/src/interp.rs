//! Others' entities on a client's screen, between the host's states.
//!
//! A client's gameplay `Transform` of a remote entity is the host's value at the newest state
//! applied: physics, `observe` and the desync hash read it, and it moves in steps of
//! `send_every` ticks. In a window (`render` feature, never headless) the client shows those
//! entities a little in the past instead, blended between the two states around
//! `render tick = host tick now − (send_every + jitter ticks)` plus the fraction of the tick
//! the frame is into: `PostUpdate`, after transform propagation, writes their
//! `GlobalTransform` (and moves their descendants' by the same amount). Before the oldest
//! state kept and after the newest, the entity holds there; a jump longer than [`JUMP`]
//! between two states (a teleport) is not blended. An entity that starts moving moves away
//! from where it was shown; one spawned (or spawned again on coming back into view) shows
//! at its state at once.
//!
//! [`sample`] and [`track`] are the buffer math, built without a window and unit-tested.

use std::collections::VecDeque;

use bevy::prelude::*;

/// States kept per entity.
pub const KEPT: usize = 32;
/// Two states further apart than this (meters) are not blended: the earlier one holds until
/// the later one's tick.
pub const JUMP: f32 = 4.0;

/// Where `states` (host tick, transform; oldest first) have an entity at host tick `at`
/// (fractional): blended between the two states around it (rotation by slerp), the oldest
/// before them all, the newest after. `None` without states.
pub fn sample(states: &VecDeque<(u64, Transform)>, at: f64) -> Option<Transform> {
    let (first, last) = (states.front()?, states.back()?);
    if at <= first.0 as f64 {
        return Some(first.1);
    }
    if at >= last.0 as f64 {
        return Some(last.1);
    }
    // The first state after `at`: there is one before it, and it is not the oldest.
    let i = states.partition_point(|(t, _)| *t as f64 <= at);
    let ((ta, a), (tb, b)) = (states[i - 1], states[i]);
    if a.translation.distance(b.translation) > JUMP {
        return Some(a);
    }
    let s = ((at - ta as f64) / (tb - ta) as f64) as f32;
    Some(Transform {
        translation: a.translation.lerp(b.translation, s),
        rotation: a.rotation.slerp(b.rotation, s),
        scale: a.scale.lerp(b.scale, s),
    })
}

/// The host's state `tf` of host tick `newest` into an entity's track (empty: not tracked
/// yet). An entity tracked from now on starts at `shown` (where it was last shown, as the state
/// of host tick `before`), so it moves away from there; one `new` in the world (spawned, or
/// spawned again under its id: never shown at its place) starts afresh at `tf`.
pub fn track(states: &mut VecDeque<(u64, Transform)>, new: bool, before: u64, shown: Transform, newest: u64, tf: Transform) {
    if new {
        states.clear();
    } else if states.is_empty() && before > 0 && before < newest {
        push(states, before, shown);
    }
    push(states, newest, tf);
}

/// Adds `(tick, tf)` to `states`, keeping [`KEPT`]; a tick not after the newest replaces the
/// states from it on (a world taken again).
pub fn push(states: &mut VecDeque<(u64, Transform)>, tick: u64, tf: Transform) {
    while states.back().is_some_and(|(t, _)| *t >= tick) {
        states.pop_back();
    }
    states.push_back((tick, tf));
    while states.len() > KEPT {
        states.pop_front();
    }
}

#[cfg(feature = "render")]
pub(crate) use window::plugin;

#[cfg(feature = "render")]
mod window {
    use std::collections::{BTreeMap, VecDeque};

    use ai_core::{sim, AiId, AiIndex};
    use bevy::prelude::*;
    use bevy::transform::TransformSystems;
    use shards_core::{NetPrediction, Predicted};

    use super::{sample, track};
    use crate::client::{ClientStage, NetClient};
    use crate::def::net_def;

    /// The host's states of every remote entity that moved, by id.
    #[derive(Resource, Default)]
    pub(crate) struct NetInterpolation {
        tracks: BTreeMap<u32, VecDeque<(u64, Transform)>>,
        /// The newest host tick recorded, and the world it came from ([`NetPrediction::epoch`]).
        newest: u64,
        epoch: u32,
        /// Client ticks minus host ticks, smoothed: the host's tick now is this client's tick
        /// less it.
        offset: Option<f64>,
    }

    pub(crate) fn plugin(app: &mut App) {
        app.init_resource::<NetInterpolation>().add_systems(
            PostUpdate,
            (
                record.before(TransformSystems::Propagate),
                show.after(TransformSystems::Propagate),
            )
                .run_if(resource_exists::<NetClient>)
                .run_if(any_with_component::<Window>),
        );
    }

    /// Every top-level entity with an id but the own body: its gameplay place, and where it
    /// was last shown.
    type Remote<'w, 's> = Query<'w, 's, (&'static AiId, Ref<'static, Transform>, &'static GlobalTransform), (Without<Predicted>, Without<ChildOf>)>;

    /// The host's newest state of each remote entity, when a new one was applied. An entity
    /// gets a track once its `Transform` changes: its last shown place (its
    /// `GlobalTransform`, not propagated yet this frame) is the state before, unless the
    /// entity is new (its `GlobalTransform` was never propagated: the origin).
    fn record(
        client: Res<NetClient>,
        prediction: Res<NetPrediction>,
        mut interp: ResMut<NetInterpolation>,
        tick: Res<ai_core::SimTick>,
        remote: Remote,
    ) {
        if client.stage != ClientStage::Connected {
            return;
        }
        let newest = client.newest_host_tick;
        if interp.epoch != prediction.epoch {
            *interp = NetInterpolation { epoch: prediction.epoch, ..default() };
        }
        if newest == interp.newest {
            return;
        }
        let before = interp.newest;
        let measured = tick.0 as f64 - newest as f64;
        interp.offset = Some(match interp.offset {
            Some(o) if (measured - o).abs() <= 3.0 => o + (measured - o) * 0.1,
            _ => measured,
        });
        interp.newest = newest;
        let mut live = Vec::new();
        for (id, tf, shown) in &remote {
            let tracked = interp.tracks.contains_key(&id.0);
            if !tracked && !tf.is_changed() {
                continue;
            }
            let states = interp.tracks.entry(id.0).or_default();
            track(states, tf.is_added(), before, shown.compute_transform(), newest, *tf);
            live.push(id.0);
        }
        live.sort_unstable();
        // Gone, or still since every state kept: shown as they are.
        let mut tracks = std::mem::take(&mut interp.tracks);
        tracks.retain(|id, states| {
            live.binary_search(id).is_ok() && !(states.len() >= super::KEPT && states.iter().all(|(_, s)| *s == states[0].1))
        });
        interp.tracks = tracks;
    }

    /// The tracked entities where the host had them at the render tick, and their
    /// descendants moved with them.
    fn show(world: &mut World) {
        let Some(offset) = world.resource::<NetInterpolation>().offset else { return };
        let def = net_def(world);
        let jitter = world.resource::<NetClient>().clock.target.saturating_sub(2).max(0) as f64;
        let overstep = world.get_resource::<Time<Fixed>>().map_or(0.0, |t| t.overstep_fraction_f64());
        let at = sim::tick(world) as f64 + overstep - offset - (def.send_every as f64 + jitter);
        let moves: Vec<(Entity, GlobalTransform)> = {
            let interp = world.resource::<NetInterpolation>();
            let index = world.resource::<AiIndex>();
            interp
                .tracks
                .iter()
                .filter_map(|(id, states)| Some((index.get(AiId(*id))?, GlobalTransform::from(sample(states, at)?))))
                .collect()
        };
        for (entity, shown) in moves {
            let Some(mut gt) = world.get_mut::<GlobalTransform>(entity) else { continue };
            if *gt == shown {
                continue;
            }
            let by = shown.affine() * gt.affine().inverse();
            *gt = shown;
            let mut below: Vec<Entity> = world.get::<Children>(entity).map(|c| c.to_vec()).unwrap_or_default();
            while let Some(child) = below.pop() {
                if let Some(children) = world.get::<Children>(child) {
                    below.extend(children.iter());
                }
                if let Some(mut gt) = world.get_mut::<GlobalTransform>(child) {
                    *gt = GlobalTransform::from(by * gt.affine());
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(x: f32) -> Transform {
        Transform::from_xyz(x, 0.0, 0.0)
    }

    #[test]
    fn a_sample_blends_between_the_states_around_it_and_holds_at_the_ends() {
        let mut states = VecDeque::new();
        assert_eq!(sample(&states, 5.0), None);
        push(&mut states, 10, at(0.0));
        push(&mut states, 12, at(1.0));
        push(&mut states, 14, at(3.0));
        assert_eq!(sample(&states, 9.0), Some(at(0.0)), "before the oldest: held there");
        assert_eq!(sample(&states, 11.0), Some(at(0.5)));
        assert_eq!(sample(&states, 12.0), Some(at(1.0)));
        assert_eq!(sample(&states, 13.5), Some(at(2.5)));
        assert_eq!(sample(&states, 20.0), Some(at(3.0)), "after the newest: held there");
        let turned = Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2));
        let mut turning = VecDeque::from([(0, Transform::IDENTITY), (4, turned)]);
        let half = sample(&turning, 2.0).unwrap();
        assert!(half.rotation.angle_between(Quat::from_rotation_y(std::f32::consts::FRAC_PI_4)) < 1e-4);
        push(&mut turning, 8, turned);
        assert_eq!(turning.len(), 3);
    }

    #[test]
    fn a_teleport_is_not_blended_and_a_new_world_replaces_later_states() {
        let mut states = VecDeque::from([(10, at(0.0)), (12, at(20.0)), (14, at(21.0))]);
        assert_eq!(sample(&states, 11.9), Some(at(0.0)), "the jump shows at its tick");
        assert_eq!(sample(&states, 13.0), Some(at(20.5)));
        // The host's world taken again from tick 12: what came after it is void.
        push(&mut states, 12, at(5.0));
        assert_eq!(states, VecDeque::from([(10, at(0.0)), (12, at(5.0))]));
        for t in 0..100 {
            push(&mut states, 20 + t, at(0.0));
        }
        assert_eq!(states.len(), KEPT, "only the newest are kept");
        assert_eq!(states.front().unwrap().0, 20 + 100 - KEPT as u64);
    }

    #[test]
    fn a_track_starts_where_the_entity_was_shown_unless_it_is_new() {
        // Standing at 1, shown there until host tick 10; it moves at 12: from 1 to 2.
        let mut states = VecDeque::new();
        track(&mut states, false, 10, at(1.0), 12, at(2.0));
        assert_eq!(states, VecDeque::from([(10, at(1.0)), (12, at(2.0))]));
        assert_eq!(sample(&states, 11.0), Some(at(1.5)));
        // Spawned with this state: never shown, its place unknown before (the origin). It
        // shows where it is at once.
        let mut states = VecDeque::new();
        track(&mut states, true, 10, Transform::IDENTITY, 12, at(30.0));
        assert_eq!(states, VecDeque::from([(12, at(30.0))]));
        assert_eq!(sample(&states, 9.0), Some(at(30.0)));
        // Gone out of view and spawned again under its id before its track went: the old
        // states are another entity's.
        let mut states = VecDeque::from([(4, at(5.0)), (6, at(6.0))]);
        track(&mut states, true, 10, Transform::IDENTITY, 12, at(40.0));
        assert_eq!(states, VecDeque::from([(12, at(40.0))]));
        // Tracked already: the newest state goes on the end.
        track(&mut states, false, 12, at(0.0), 14, at(41.0));
        assert_eq!(states, VecDeque::from([(12, at(40.0)), (14, at(41.0))]));
    }
}
