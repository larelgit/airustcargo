//! A client's own body between the host's states: the inputs it predicts with, what it
//! predicted each tick, and the correction when the host's state of the body comes.
//!
//! * `NetSet::PredictInput` puts the presses this machine sent for the tick into the body's
//!   `Intent` (the capture took them out of it), and sets aside the ones not sent yet and
//!   what the body steers by (`move_dir`, `aim`, `target`, the order queue);
//!   `NetSet::PredictEnd` puts those back and records the tick: `(tick, feet, mode,
//!   applied)` in a ring of [`HISTORY`]. Orders steer the predicted body as they steer the
//!   host's, but what they wrote is undone after the tick: the host follows the orders
//!   itself, so the capture sends only what the player changed.
//! * `NetSystems::Correct` (after `ApplyState`) compares the host's feet at its tick `T`
//!   with the feet predicted for `T`, less what was corrected since ([`remaining`]). Under
//!   [`DEAD_ZONE`] nothing happens; otherwise a share (`NetDef::correction_rate`) of it
//!   moves the body, the controller told it left it there. Over `NetDef::snap_distance`, a
//!   motion mode that differs for [`MODE_TICKS`], or a death, the whole error goes at once
//!   and the host's motion state is taken (a snap, journaled).
//!
//! The own body's predicted components are never written by the host's deltas otherwise,
//! and the desync hash leaves them out.

use std::collections::VecDeque;

use ai_core::journal::{self, JournalKind};
use ai_core::state::{decode_value, ComponentValues};
use ai_core::{sim, AiId, AiIndex};
use bevy::prelude::*;
use bevy::reflect::{FromReflect, TypePath};
use shards_core::{Intent, NetPrediction, Order, PredictedMotion, Press, Predicted};

use crate::client::{ClientStage, NetClient};
use crate::def::net_def;

/// Predicted ticks remembered.
pub const HISTORY: usize = 128;
/// An error shorter than this is left alone (meters).
pub const DEAD_ZONE: f32 = 0.02;
/// Host ticks a motion mode may differ before the body snaps.
pub const MODE_TICKS: u64 = 6;
/// Ticks the error figures cover.
pub const ERROR_WINDOW: u64 = 600;

/// One predicted tick of the own body.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    pub tick: u64,
    /// The feet after the tick.
    pub feet: Vec3,
    /// The motion mode after the tick ([`PredictedMotion::mode`]).
    pub mode: Option<u8>,
    /// The correction made just before the tick (in its feet already).
    pub applied: Vec3,
}

/// What [`remaining`] says to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Verdict {
    /// Inside the dead zone.
    Keep,
    /// Move by this much.
    Blend(Vec3),
    /// Move by the whole error and take the host's motion state.
    Snap(Vec3),
}

/// What is left to correct: the host's feet at `tick`, less the feet predicted for it, less
/// every correction made after it. `None` when `tick` is not in the history.
pub fn remaining(history: &VecDeque<Sample>, unrecorded: Vec3, tick: u64, host_feet: Vec3) -> Option<Vec3> {
    let i = history.binary_search_by_key(&tick, |s| s.tick).ok()?;
    let since = history.iter().skip(i + 1).fold(unrecorded, |sum, s| sum + s.applied);
    Some(host_feet - history[i].feet - since)
}

/// The rule for an error: keep inside `dead_zone`, snap over `snap` (or when `forced`),
/// otherwise blend `rate` of it.
pub fn verdict(error: Vec3, dead_zone: f32, rate: f32, snap: f32, forced: bool) -> Verdict {
    let length = error.length();
    if forced || length > snap {
        Verdict::Snap(error)
    } else if length < dead_zone {
        Verdict::Keep
    } else {
        Verdict::Blend(error * rate)
    }
}

/// The own body's prediction on a client: history, the host's state waiting to be compared,
/// figures.
#[derive(Resource, Default, Debug)]
pub struct NetCorrection {
    history: VecDeque<Sample>,
    /// Corrections made since the newest sample (they go into the next one).
    unrecorded: Vec3,
    /// The newest host state of the body: its tick and predicted components.
    pending: Option<(u64, ComponentValues)>,
    /// The host tick since which the motion modes differ.
    mismatch_since: Option<u64>,
    /// Mode differences up to this tick are from before a snap.
    forgive_until: u64,
    /// What `PredictInput` set aside.
    stash: Option<Stash>,
    /// `(host tick, error)` of the comparisons of the last [`ERROR_WINDOW`] ticks.
    errors: VecDeque<(u64, f32)>,
    /// Comparisons, blends and snaps made in this session.
    pub compared: u64,
    pub blends: u64,
    pub snaps: u32,
    /// `(host tick, line)` of the last snap.
    pub last_snap: Option<(u64, String)>,
}

impl NetCorrection {
    /// Mean and largest error over the last [`ERROR_WINDOW`] ticks, in meters.
    pub fn error(&self) -> (f32, f32) {
        if self.errors.is_empty() {
            return (0.0, 0.0);
        }
        let sum: f32 = self.errors.iter().map(|(_, e)| e).sum();
        let max = self.errors.iter().map(|(_, e)| *e).fold(0.0, f32::max);
        (sum / self.errors.len() as f32, max)
    }

    /// The predicted ticks remembered.
    pub fn history(&self) -> &VecDeque<Sample> {
        &self.history
    }

    /// A world taken from the host again: what was predicted belongs to the old one.
    pub(crate) fn restart(&mut self) {
        self.history.clear();
        self.unrecorded = Vec3::ZERO;
        self.pending = None;
        self.mismatch_since = None;
        self.forgive_until = 0;
    }

    /// The host's state of the body at `tick` (its `own` components), for the next `Correct`.
    pub(crate) fn host_state(&mut self, tick: u64, values: ComponentValues) {
        self.pending = Some((tick, values));
    }

    fn record(&mut self, tick: u64, feet: Vec3, mode: Option<u8>) {
        if self.history.back().is_some_and(|s| s.tick >= tick) {
            // A tick predicted again (the clock went back): the old one is void.
            self.history.retain(|s| s.tick < tick);
        }
        let applied = std::mem::take(&mut self.unrecorded);
        self.history.push_back(Sample { tick, feet, mode, applied });
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
    }
}

/// The own body's `Intent` before its tick was predicted: the presses not sent yet, and the
/// steering the orders write while predicting.
#[derive(Debug)]
struct Stash {
    waiting: Vec<Press>,
    move_dir: Vec3,
    aim: Option<Vec3>,
    target: Option<AiId>,
    orders: VecDeque<Order>,
}

/// The own body on this client, if it is in the world.
fn own_body(world: &World) -> Option<(u32, Entity)> {
    let client = world.get_resource::<NetClient>()?;
    if client.stage != ClientStage::Connected {
        return None;
    }
    let id = client.body?;
    Some((id, world.resource::<AiIndex>().get(AiId(id))?))
}

/// `NetSet::PredictInput`: the presses sent for this tick into the body's `Intent`; the
/// ones still waiting to be sent aside.
pub(crate) fn predict_input(world: &mut World) {
    let Some((_, body)) = own_body(world) else { return };
    if world.get::<Predicted>(body).is_none() {
        return;
    }
    let tick = sim::tick(world);
    let (stamped, presses) = world.resource::<NetClient>().capture.last_presses.clone();
    let sent = if stamped + 1 == tick { presses } else { Vec::new() };
    let Some(mut intent) = world.get_mut::<Intent>(body) else { return };
    let waiting = std::mem::replace(&mut intent.pressed, sent);
    let stash = Stash { waiting, move_dir: intent.move_dir, aim: intent.aim, target: intent.target, orders: intent.orders.clone() };
    world.resource_mut::<NetCorrection>().stash = Some(stash);
}

/// `NetSet::PredictEnd`: what was set aside back, and the tick recorded.
pub(crate) fn predict_end(world: &mut World) {
    let stash = world.resource_mut::<NetCorrection>().stash.take();
    let Some((_, body)) = own_body(world) else { return };
    if let Some(stash) = stash
        && let Some(mut intent) = world.get_mut::<Intent>(body)
    {
        intent.pressed = stash.waiting;
        intent.move_dir = stash.move_dir;
        intent.aim = stash.aim;
        intent.target = stash.target;
        intent.orders = stash.orders;
    }
    let Some(feet) = world.get::<Transform>(body).map(|t| t.translation) else { return };
    let mode = world.get_resource::<PredictedMotion>().and_then(|m| (m.mode)(world, body));
    let tick = sim::tick(world);
    world.resource_mut::<NetCorrection>().record(tick, feet, mode);
}

fn host_feet(world: &World, values: &ComponentValues) -> Option<Vec3> {
    let value = values.get(Transform::type_path())?;
    let reflected = decode_value(world, Transform::type_path(), value).ok()?;
    Transform::from_reflect(reflected.as_ref()).map(|t| t.translation)
}

/// `NetSystems::Correct`: the host's state of the own body against what was predicted.
pub(crate) fn correct(world: &mut World) {
    let Some((tick, values)) = world.get_resource_mut::<NetCorrection>().and_then(|mut c| c.pending.take()) else {
        return;
    };
    let Some((id, body)) = own_body(world) else { return };
    if world.get::<Predicted>(body).is_none() {
        return;
    }
    let Some(host) = host_feet(world, &values) else { return };
    let def = net_def(world);
    let motion = world.get_resource::<PredictedMotion>().copied();
    let host_mode = motion.and_then(|m| (m.host_mode)(world, &values));
    let Some(error) = ({
        let c = world.resource::<NetCorrection>();
        remaining(&c.history, c.unrecorded, tick, host)
    }) else {
        return;
    };
    let died = world.get_resource_mut::<NetPrediction>().and_then(|mut p| p.snap.take());

    let mut c = world.resource_mut::<NetCorrection>();
    let i = c.history.binary_search_by_key(&tick, |s| s.tick).expect("remaining found it");
    let here_mode = c.history[i].mode;
    c.compared += 1;
    c.errors.push_back((tick, error.length()));
    while c.errors.front().is_some_and(|(t, _)| *t + ERROR_WINDOW <= tick) {
        c.errors.pop_front();
    }
    // A motion mode that differs for long is a different situation, not a small drift.
    let mut why = died;
    match (here_mode, host_mode) {
        (Some(here), Some(there)) if here != there && tick > c.forgive_until => {
            let since = *c.mismatch_since.get_or_insert(tick);
            if tick - since >= MODE_TICKS && why.is_none() {
                why = Some("motion mode".into());
            }
        }
        _ => c.mismatch_since = None,
    }
    let verdict = verdict(error, DEAD_ZONE, def.correction_rate, def.snap_distance, why.is_some());
    match &verdict {
        Verdict::Keep => {}
        Verdict::Blend(by) => {
            c.blends += 1;
            c.unrecorded += *by;
        }
        Verdict::Snap(by) => {
            c.snaps += 1;
            c.unrecorded += *by;
            c.mismatch_since = None;
            c.forgive_until = c.history.back().map_or(tick, |s| s.tick);
        }
    }
    match verdict {
        Verdict::Keep => {}
        Verdict::Blend(by) => move_body(world, motion, body, by),
        Verdict::Snap(by) => {
            let line = match &why {
                Some(why) => format!("net: e{id} corrected by {:.1} m (snap: {why})", by.length()),
                None => format!("net: e{id} corrected by {:.1} m (snap)", by.length()),
            };
            world.resource_mut::<NetCorrection>().last_snap = Some((tick, line.clone()));
            let feet = world.get::<Transform>(body).map_or(host, |t| t.translation) + by;
            match motion {
                Some(m) => (m.adopt)(world, body, &values, feet),
                None => move_body(world, None, body, by),
            }
            journal::emit(world, JournalKind::Sim, line);
        }
    }
}

fn move_body(world: &mut World, motion: Option<PredictedMotion>, body: Entity, by: Vec3) {
    match motion {
        Some(m) => (m.shift)(world, body, by),
        None => {
            if let Some(mut tf) = world.get_mut::<Transform>(body) {
                tf.translation += by;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn history(feet: &[(u64, f32, f32)]) -> VecDeque<Sample> {
        feet.iter().map(|&(tick, x, applied)| Sample { tick, feet: Vec3::X * x, mode: None, applied: Vec3::X * applied }).collect()
    }

    #[test]
    fn remaining_leaves_out_what_was_corrected_since() {
        // Predicted 1.0 at tick 10, the host had 1.5: half a meter off. 0.2 was moved since
        // (0.1 before tick 11, 0.1 not recorded yet): 0.3 is left.
        let h = history(&[(9, 0.9, 0.0), (10, 1.0, 0.0), (11, 1.2, 0.1)]);
        let left = remaining(&h, Vec3::X * 0.1, 10, Vec3::X * 1.5).unwrap();
        assert!((left.x - 0.3).abs() < 1e-6, "{left}");
        // A correction before tick 10 is in its feet already.
        let h = history(&[(10, 1.1, 0.1), (11, 1.3, 0.0)]);
        assert!((remaining(&h, Vec3::ZERO, 10, Vec3::X * 1.5).unwrap().x - 0.4).abs() < 1e-6);
        assert_eq!(remaining(&h, Vec3::ZERO, 12, Vec3::ZERO), None, "a tick not predicted");
    }

    #[test]
    fn small_errors_are_kept_large_ones_snap_the_rest_blend() {
        assert_eq!(verdict(Vec3::X * 0.01, 0.02, 0.15, 1.5, false), Verdict::Keep);
        assert_eq!(verdict(Vec3::X * 2.0, 0.02, 0.15, 1.5, false), Verdict::Snap(Vec3::X * 2.0));
        assert_eq!(verdict(Vec3::X * 0.01, 0.02, 0.15, 1.5, true), Verdict::Snap(Vec3::X * 0.01), "a death snaps");
        let Verdict::Blend(by) = verdict(Vec3::X, 0.02, 0.15, 1.5, false) else { panic!("blends") };
        assert!((by.x - 0.15).abs() < 1e-6);
    }

    #[test]
    fn repeated_blends_converge_without_overshoot() {
        // The host stands at 1.0; this machine predicted 0.0 for every tick and corrects
        // every other tick: the error left shrinks by the rate each time, never changes sign.
        let mut c = NetCorrection::default();
        let mut feet = 0.0f32;
        let mut last = f32::MAX;
        for tick in 0..200u64 {
            if tick >= 10 && tick % 2 == 0 {
                let error = remaining(&c.history, c.unrecorded, tick - 10, Vec3::X).unwrap();
                assert!(error.x >= 0.0 && error.x <= last + 1e-6, "tick {tick}: {error}");
                last = error.x;
                if let Verdict::Blend(by) = verdict(error, DEAD_ZONE, 0.15, 1.5, false) {
                    c.unrecorded += by;
                    feet += by.x;
                }
            }
            c.record(tick, Vec3::X * feet, None);
        }
        assert!(last < DEAD_ZONE, "converged: {last}");
        assert!((feet - 1.0).abs() < 0.05, "{feet}");
    }
}
