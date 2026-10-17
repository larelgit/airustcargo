//! A client's own body between the host's states: the inputs it predicts with, what it
//! predicted each tick, and the correction when the host's state of the body comes.
//!
//! * `NetSet::PredictInput` puts the presses this machine sent for the tick into the body's
//!   `Intent` (the capture took them out of it), and sets aside the ones not sent yet and
//!   what the body steers by (`move_dir`, `aim`, `target`, the order queue);
//!   `NetSet::PredictEnd` puts those back and records the tick: `(tick, feet, mode,
//!   applied)` in a ring of [`HISTORY`]. Orders steer the predicted body as they steer the
//!   host's, but what they wrote is undone after the tick: the host follows the orders
//!   itself, so the capture sends only what the player changed. Orders the prediction
//!   finished stay finished for it (the next ticks start past them) until the host's queue,
//!   taken over, drops them too.
//! * `NetSystems::Correct` (after `ApplyState`) compares the host's feet at its tick `T`
//!   with the feet predicted for `T`, less what was corrected since ([`remaining`]). Under
//!   [`DEAD_ZONE`] nothing happens; otherwise the body moves by a share
//!   (`NetDef::correction_rate`) of what is left of it every tick, until the next
//!   comparison says what is left then; the controller is told it left the body there.
//!   Over `NetDef::snap_distance`, a motion mode that differs for [`MODE_TICKS`], a death,
//!   or the first comparison after the world was taken again or the clock skipped ticks,
//!   the whole error goes at once and the host's motion state of `T` is taken (a snap,
//!   journaled). That state then runs `newest − T` ticks behind the host's labels: until it
//!   caught up ([`NetCorrection::settling`]) only a death snaps the body again.
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
/// A correction being blended in that is shorter than this is done (meters).
pub const BLEND_FLOOR: f32 = 0.001;

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
    /// Move by this much, a share of it each tick.
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
/// otherwise blend it in.
pub fn verdict(error: Vec3, dead_zone: f32, snap: f32, forced: bool) -> Verdict {
    let length = error.length();
    if forced || length > snap {
        Verdict::Snap(error)
    } else if length < dead_zone {
        Verdict::Keep
    } else {
        Verdict::Blend(error)
    }
}

/// The own body's prediction on a client: history, the host's state waiting to be compared,
/// figures.
#[derive(Resource, Default, Debug)]
pub struct NetCorrection {
    history: VecDeque<Sample>,
    /// Corrections made since the newest sample (they go into the next one).
    unrecorded: Vec3,
    /// What is left of the correction being blended in.
    blend: Vec3,
    /// The newest host state of the body: its tick and predicted components.
    pending: Option<(u64, ComponentValues)>,
    /// The host tick since which the motion modes differ.
    mismatch_since: Option<u64>,
    /// After a snap: the host tick up to which the motion state taken has not caught up.
    settle_until: Option<u64>,
    /// The first comparison of a host tick from this one on snaps (why): the world was taken
    /// again, or the clock skipped ticks the body did not move in.
    realign: Option<(u64, &'static str)>,
    /// The order queue as the last predicted tick left it (the player's, whole), and how many
    /// orders at its front the prediction finished.
    queue: VecDeque<Order>,
    done: usize,
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

    /// Whether a comparison of host tick `tick` is of the time after a snap in which the host's
    /// motion state taken has not caught up with the host's labels: the body is left alone.
    pub fn settling(&self, tick: u64) -> bool {
        self.settle_until.is_some_and(|until| tick <= until)
    }

    /// A world taken from the host again: what was predicted, and the error figures, belong to
    /// the old one. The body is where the host had it at the world's tick, which this clock is
    /// ahead of: the first comparison snaps it.
    pub(crate) fn restart(&mut self) {
        self.history.clear();
        self.unrecorded = Vec3::ZERO;
        self.blend = Vec3::ZERO;
        self.pending = None;
        self.mismatch_since = None;
        self.settle_until = None;
        self.realign = Some((0, "world taken"));
        self.queue.clear();
        self.done = 0;
        self.errors.clear();
    }

    /// A world taken with this machine's orders kept (`mine`) over the host's (`host`, as its
    /// world has them): the orders the host finished by then, or cleared, are finished here
    /// too.
    pub(crate) fn carry_orders(&mut self, mine: &VecDeque<Order>, host: &VecDeque<Order>) {
        let finished = host.len() <= mine.len() && mine.iter().skip(mine.len() - host.len()).eq(host);
        self.done = if finished { mine.len() - host.len() } else { 0 };
        self.queue.clone_from(mine);
    }

    /// The host's state of the body at `tick` (its `own` components), for the next `Correct`.
    pub(crate) fn host_state(&mut self, tick: u64, values: ComponentValues) {
        self.pending = Some((tick, values));
    }

    fn record(&mut self, tick: u64, feet: Vec3, mode: Option<u8>) {
        match self.history.back().map(|s| s.tick) {
            // A tick predicted again (the clock went back): the old one is void.
            Some(newest) if newest >= tick => self.history.retain(|s| s.tick < tick),
            // Ticks skipped (the clock jumped ahead): the host's body moved in them, this one
            // did not.
            Some(newest) if tick > newest + 1 => self.realign = Some((tick, "clock skipped")),
            _ => {}
        }
        let applied = std::mem::take(&mut self.unrecorded);
        self.history.push_back(Sample { tick, feet, mode, applied });
        while self.history.len() > HISTORY {
            self.history.pop_front();
        }
    }

    /// The host's feet at `tick`, `error` from what was predicted for it ([`remaining`]), with
    /// its motion mode and whether the body died there (`died`, why): the verdict, made
    /// (counted, the correction booked), and why a snap snaps. `tick` is in the history.
    fn judge(&mut self, tick: u64, error: Vec3, host_mode: Option<u8>, died: Option<String>, snap: f32) -> (Verdict, Option<String>) {
        let here_mode = self.history.binary_search_by_key(&tick, |s| s.tick).ok().and_then(|i| self.history[i].mode);
        self.compared += 1;
        // A tick before the window's newest is a world taken at an earlier tick.
        self.errors.retain(|(t, _)| *t <= tick && *t + ERROR_WINDOW > tick);
        self.errors.push_back((tick, error.length()));
        let settling = self.settling(tick);
        // A motion mode that differs for long is a different situation, not a small drift.
        let mut why = died;
        match (here_mode, host_mode) {
            (Some(here), Some(there)) if here != there && !settling => {
                let since = *self.mismatch_since.get_or_insert(tick);
                if tick - since >= MODE_TICKS && why.is_none() {
                    why = Some("motion mode".into());
                }
            }
            _ => self.mismatch_since = None,
        }
        if let Some((from, reason)) = self.realign
            && tick >= from
        {
            self.realign = None;
            if why.is_none() && error.length() >= DEAD_ZONE {
                why = Some(reason.into());
            }
        }
        let verdict = if settling && why.is_none() { Verdict::Keep } else { verdict(error, DEAD_ZONE, snap, why.is_some()) };
        match &verdict {
            Verdict::Keep => self.blend = Vec3::ZERO,
            Verdict::Blend(by) => {
                self.blends += 1;
                self.blend = *by;
            }
            Verdict::Snap(by) => {
                self.snaps += 1;
                self.unrecorded += *by;
                self.blend = Vec3::ZERO;
                self.mismatch_since = None;
                // The host's motion state of `tick` goes on from this machine's newest tick:
                // the host's labels are reached that many ticks later.
                let newest = self.history.back().map_or(tick, |s| s.tick);
                self.settle_until = Some(newest + newest.saturating_sub(tick));
            }
        }
        (verdict, why)
    }

    /// This tick's share (`rate`) of the correction being blended in, booked as made.
    fn blend_step(&mut self, rate: f32) -> Option<Vec3> {
        if self.blend.length() < BLEND_FLOOR {
            self.blend = Vec3::ZERO;
            return None;
        }
        let by = self.blend * rate;
        self.blend -= by;
        self.unrecorded += by;
        Some(by)
    }

    /// How many orders at the front of `queue` (the body's, before its tick) the prediction
    /// finished. The queue may have changed since the last tick: orders added behind keep
    /// them; the host's queue taken over (orders it finished dropped from the front) counts
    /// them as finished there; anything else is a new queue.
    fn finished(&mut self, queue: &VecDeque<Order>) -> usize {
        let last = &self.queue;
        if queue.len() >= last.len() && queue.iter().zip(last).all(|(a, b)| a == b) {
            // The same queue, or orders added behind it.
        } else if queue.len() < last.len() && last.iter().skip(last.len() - queue.len()).eq(queue) {
            self.done = self.done.saturating_sub(last.len() - queue.len());
        } else {
            self.done = 0;
        }
        self.queue.clone_from(queue);
        self.done.min(queue.len())
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
    /// Orders at the front of `orders` the tick started past.
    skipped: usize,
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
/// ones still waiting to be sent aside. The tick starts past the orders already finished.
pub(crate) fn predict_input(world: &mut World) {
    let Some((_, body)) = own_body(world) else { return };
    if world.get::<Predicted>(body).is_none() {
        return;
    }
    let tick = sim::tick(world);
    let (stamped, presses) = world.resource::<NetClient>().capture.last_presses.clone();
    let sent = if stamped + 1 == tick { presses } else { Vec::new() };
    let Some(orders) = world.get::<Intent>(body).map(|i| i.orders.clone()) else { return };
    let skipped = world.resource_mut::<NetCorrection>().finished(&orders);
    let Some(mut intent) = world.get_mut::<Intent>(body) else { return };
    let waiting = std::mem::replace(&mut intent.pressed, sent);
    let stash = Stash { waiting, move_dir: intent.move_dir, aim: intent.aim, target: intent.target, orders, skipped };
    intent.orders.drain(..skipped);
    if skipped > 0 && intent.orders.is_empty() {
        // The last order popped there stopped the body (`follow_orders`).
        intent.move_dir = Vec3::ZERO;
    }
    world.resource_mut::<NetCorrection>().stash = Some(stash);
}

/// `NetSet::PredictEnd`: what was set aside back, the orders the tick finished remembered,
/// and the tick recorded.
pub(crate) fn predict_end(world: &mut World) {
    let stash = world.resource_mut::<NetCorrection>().stash.take();
    let Some((_, body)) = own_body(world) else { return };
    let mut done = None;
    if let Some(stash) = stash
        && let Some(mut intent) = world.get_mut::<Intent>(body)
    {
        // Orders popped from the front stay finished; anything else the tick did to the
        // queue is undone with it.
        let (all, left) = (stash.orders.len(), intent.orders.len());
        let popped = left <= all - stash.skipped && stash.orders.iter().skip(all - left).eq(intent.orders.iter());
        done = Some(if popped { all - left } else { stash.skipped });
        intent.pressed = stash.waiting;
        intent.move_dir = stash.move_dir;
        intent.aim = stash.aim;
        intent.target = stash.target;
        intent.orders = stash.orders;
    }
    if let Some(done) = done {
        world.resource_mut::<NetCorrection>().done = done;
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

/// `NetSystems::Correct`: the host's state of the own body against what was predicted, when
/// one came; then this tick's share of the correction being blended in.
pub(crate) fn correct(world: &mut World) {
    let pending = world.get_resource_mut::<NetCorrection>().and_then(|mut c| c.pending.take());
    let Some((id, body)) = own_body(world) else { return };
    if world.get::<Predicted>(body).is_none() {
        return;
    }
    let def = net_def(world);
    let motion = world.get_resource::<PredictedMotion>().copied();
    if let Some((tick, values)) = pending {
        compare(world, id, body, motion, tick, &values, def.snap_distance);
    }
    let step = world.resource_mut::<NetCorrection>().blend_step(def.correction_rate);
    if let Some(by) = step {
        move_body(world, motion, body, by);
    }
}

fn compare(world: &mut World, id: u32, body: Entity, motion: Option<PredictedMotion>, tick: u64, values: &ComponentValues, snap: f32) {
    let Some(host) = host_feet(world, values) else { return };
    let host_mode = motion.and_then(|m| (m.host_mode)(world, values));
    let Some(error) = ({
        let c = world.resource::<NetCorrection>();
        remaining(&c.history, c.unrecorded, tick, host)
    }) else {
        return;
    };
    let died = world.get_resource_mut::<NetPrediction>().and_then(|mut p| p.snap.take());
    let (verdict, why) = world.resource_mut::<NetCorrection>().judge(tick, error, host_mode, died, snap);
    let Verdict::Snap(by) = verdict else { return };
    let line = match &why {
        Some(why) => format!("net: e{id} corrected by {:.1} m (snap: {why})", by.length()),
        None => format!("net: e{id} corrected by {:.1} m (snap)", by.length()),
    };
    world.resource_mut::<NetCorrection>().last_snap = Some((tick, line.clone()));
    let feet = world.get::<Transform>(body).map_or(host, |t| t.translation) + by;
    match motion {
        Some(m) => (m.adopt)(world, body, values, feet),
        None => move_body(world, None, body, by),
    }
    journal::emit(world, JournalKind::Sim, line);
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
    use shards_core::Target;

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
        assert_eq!(verdict(Vec3::X * 0.01, 0.02, 1.5, false), Verdict::Keep);
        assert_eq!(verdict(Vec3::X * 2.0, 0.02, 1.5, false), Verdict::Snap(Vec3::X * 2.0));
        assert_eq!(verdict(Vec3::X * 0.01, 0.02, 1.5, true), Verdict::Snap(Vec3::X * 0.01), "a death snaps");
        assert_eq!(verdict(Vec3::X, 0.02, 1.5, false), Verdict::Blend(Vec3::X));
    }

    /// The host stands at 1.0; this machine predicted 0.0 and compares every `every` ticks,
    /// 10 ticks late: the feet after each tick.
    fn blend_towards_one(every: u64) -> Vec<f32> {
        let mut c = NetCorrection::default();
        let (mut feet, mut last, mut out) = (0.0f32, f32::MAX, Vec::new());
        for tick in 0..120u64 {
            if tick >= 12 && tick % every == 0 {
                let error = remaining(&c.history, c.unrecorded, tick - 10, Vec3::X).unwrap();
                assert!(error.x >= 0.0 && error.x <= last + 1e-6, "every {every}, tick {tick}: {error}");
                last = error.x;
                c.judge(tick - 10, error, None, None, 1.5);
            }
            if let Some(by) = c.blend_step(0.15) {
                feet += by.x;
            }
            c.record(tick, Vec3::X * feet, None);
            out.push(feet);
        }
        out
    }

    #[test]
    fn blends_move_a_share_every_tick_whatever_the_compares_and_never_overshoot() {
        // The rate is per tick: 15 % of what is left, every tick from the first comparison,
        // however often the host's states come (until what is left is within the dead zone).
        for every in [1, 2, 6] {
            let feet = blend_towards_one(every);
            for (n, tick) in (12..36).enumerate() {
                let want = 1.0 - ops::powf(0.85, (n + 1) as f32);
                assert!((feet[tick] - want).abs() < 1e-4, "every {every}, tick {tick}: {} vs {want}", feet[tick]);
            }
            assert!(feet.iter().all(|x| *x <= 1.0 + 1e-6), "no overshoot");
            assert!((1.0 - feet[119]) < DEAD_ZONE, "converged: {}", feet[119]);
        }
    }

    #[test]
    fn a_snap_takes_the_hosts_state_late_and_waits_for_it_to_catch_up() {
        // Ground 0, air 1. Snapped at host tick 100 while this machine was at 115: the host's
        // airborne state of 100 goes on from 115. The host lands at 108; the body here, 15
        // ticks behind, at 123.
        const GROUND: u8 = 0;
        const AIR: u8 = 1;
        let host_mode = |t: u64| if (100..108).contains(&t) || (150..170).contains(&t) { AIR } else { GROUND };
        let mut c = NetCorrection::default();
        for tick in 86..=115 {
            c.record(tick, Vec3::ZERO, Some(GROUND));
        }
        let (verdict, why) = c.judge(100, Vec3::Y * 2.0, Some(AIR), None, 1.5);
        assert_eq!((verdict, why), (Verdict::Snap(Vec3::Y * 2.0), None));
        assert_eq!(c.snaps, 1);
        let mut second = None;
        for tick in 116..=190u64 {
            let here = if tick <= 123 { AIR } else { GROUND };
            c.record(tick, Vec3::ZERO, Some(here));
            let host = tick - 15;
            if host % 2 == 0 {
                let (verdict, why) = c.judge(host, Vec3::X * 0.5, Some(host_mode(host)), None, 1.5);
                if host <= 130 {
                    assert_eq!(verdict, Verdict::Keep, "host tick {host}: the state taken catches up");
                }
                if matches!(verdict, Verdict::Snap(_)) && second.is_none() {
                    second = Some((host, why));
                }
            }
        }
        // Not before host tick 156: the host in the air from 150, this body on the ground.
        assert_eq!(second, Some((156, Some("motion mode".to_owned()))), "a later difference still snaps");
    }

    #[test]
    fn the_first_comparison_after_a_new_world_or_skipped_ticks_snaps() {
        let mut c = NetCorrection::default();
        c.restart();
        for tick in 50..60 {
            c.record(tick, Vec3::ZERO, None);
        }
        // Within the dead zone: nothing to put right, and the next comparison is a plain one.
        assert_eq!(c.judge(50, Vec3::X * 0.01, None, None, 1.5), (Verdict::Keep, None));
        assert_eq!(c.judge(52, Vec3::X * 0.5, None, None, 1.5).0, Verdict::Blend(Vec3::X * 0.5));
        // The clock jumped from 59 to 90: host ticks before it compare as they are, the first
        // after it snaps.
        for tick in 90..140 {
            c.record(tick, Vec3::ZERO, None);
        }
        assert_eq!(c.judge(58, Vec3::X * 0.5, None, None, 1.5).0, Verdict::Blend(Vec3::X * 0.5));
        assert_eq!(c.judge(90, Vec3::X * 0.5, None, None, 1.5), (Verdict::Snap(Vec3::X * 0.5), Some("clock skipped".to_owned())));
        // A world taken while the body moved: the first error past the dead zone snaps.
        let mut c = NetCorrection::default();
        c.restart();
        for tick in 50..60 {
            c.record(tick, Vec3::ZERO, None);
        }
        assert_eq!(c.judge(50, Vec3::X * 0.5, None, None, 1.5), (Verdict::Snap(Vec3::X * 0.5), Some("world taken".to_owned())));
    }

    #[test]
    fn error_figures_start_again_with_a_world_taken_at_an_earlier_tick() {
        let mut c = NetCorrection::default();
        for tick in 1000..1010 {
            c.record(tick, Vec3::ZERO, None);
        }
        c.judge(1005, Vec3::X, None, None, 1.5);
        assert_eq!(c.error(), (1.0, 1.0));
        c.restart();
        assert_eq!(c.error(), (0.0, 0.0), "the old world's figures go");
        for tick in 120..130 {
            c.record(tick, Vec3::ZERO, None);
        }
        // Even without a restart, a tick before the newest compared drops what came after it.
        c.errors.push_back((5000, 3.0));
        c.judge(125, Vec3::X * 0.5, None, None, 1.5);
        assert_eq!(c.error(), (0.5, 0.5));
    }

    fn walk(x: f32) -> Order {
        Order::MoveTo { target: Target::Point(Vec3::X * x), within: 0.45 }
    }

    #[test]
    fn orders_finished_here_stay_finished_until_the_host_finishes_them() {
        let mut c = NetCorrection::default();
        let path = VecDeque::from([walk(1.0), walk(2.0), walk(3.0)]);
        assert_eq!(c.finished(&path), 0);
        // The prediction popped the first leg.
        c.done = 1;
        assert_eq!(c.finished(&path), 1, "the next tick starts at the second leg");
        // The host's queue comes with the first leg still in (its state is older): no change.
        assert_eq!(c.finished(&path), 1);
        // The host finished the first leg: taken over, nothing finished here is left in it.
        let host = VecDeque::from([walk(2.0), walk(3.0)]);
        assert_eq!(c.finished(&host), 0);
        // Finished two here, the host one: one left to skip.
        c.done = 2;
        let host = VecDeque::from([walk(3.0)]);
        assert_eq!(c.finished(&host), 1);
        // A leg added behind keeps them; a new queue starts again.
        let mut more = host.clone();
        more.push_back(walk(4.0));
        assert_eq!(c.finished(&more), 1);
        assert_eq!(c.finished(&VecDeque::from([walk(9.0)])), 0);
    }

    #[test]
    fn orders_the_host_finished_before_its_world_are_finished_here() {
        let mut c = NetCorrection::default();
        let path = VecDeque::from([walk(1.0), walk(2.0), walk(3.0)]);
        // A world taken again: the host's body there had passed the first leg.
        c.restart();
        c.carry_orders(&path, &VecDeque::from([walk(2.0), walk(3.0)]));
        assert_eq!(c.finished(&path), 1);
        // A region change stopped the body there: nothing left to walk here either.
        c.restart();
        c.carry_orders(&path, &VecDeque::new());
        assert_eq!(c.finished(&path), 3);
        // The host has not taken this queue yet (its frame is on its way): all of it is left.
        c.restart();
        c.carry_orders(&path, &VecDeque::from([walk(7.0)]));
        assert_eq!(c.finished(&path), 0);
    }
}
