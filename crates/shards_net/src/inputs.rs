//! Input frames: what a client's player did on one tick, stamped with that tick.
//!
//! The client diffs its own body's `Intent` against what it remembered at the end of the
//! previous tick (the replay recorder's rule) and *moves* the presses out (it runs no
//! gameplay, so nothing else would consume them). The host keeps a [`InputQueue`] per remote
//! player and, before tick `T` runs, applies every frame stamped `≤ T` in tick order: state
//! fields last-wins, presses exactly once, the order queue replaced. Frames land in `Intent`
//! before the replay recorder looks, so the host replay records them as ordinary inputs.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use ai_core::AiId;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_core::{Intent, Order, Press};

/// Frames further ahead of the host than this are dropped.
pub const MAX_AHEAD: u64 = 120;

/// The held state of an `Intent`: what lasts until it changes.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct IntentState {
    pub move_dir: Vec3,
    pub aim: Option<Vec3>,
    pub target: Option<AiId>,
    pub held: Vec<String>,
}

impl IntentState {
    pub fn of(intent: &Intent) -> Self {
        Self { move_dir: intent.move_dir, aim: intent.aim, target: intent.target, held: intent.held.clone() }
    }

    /// Writes the state into `intent`; whether anything changed.
    pub fn write(&self, intent: &mut Intent) -> bool {
        let changed = intent.move_dir != self.move_dir || intent.aim != self.aim || intent.target != self.target || intent.held != self.held;
        if changed {
            intent.move_dir = self.move_dir;
            intent.aim = self.aim;
            intent.target = self.target;
            intent.held = self.held.clone();
        }
        changed
    }
}

/// What one client tick changed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct InputFrame {
    /// The client's tick: the host applies it before its own tick `tick + 1` runs.
    pub tick: u64,
    /// The held state, when it changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<IntentState>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub presses: Vec<Press>,
    /// The whole order queue, when a tool or the keyboard changed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub orders: Option<Vec<Order>>,
}

impl InputFrame {
    pub fn is_empty(&self) -> bool {
        self.state.is_none() && self.presses.is_empty() && self.orders.is_none()
    }
}

/// Applies `frame` to `intent`: state last-wins, presses appended, orders replaced. Returns
/// whether `intent` changed.
pub fn apply_frame(intent: &mut Intent, frame: &InputFrame) -> bool {
    let mut changed = false;
    if let Some(state) = &frame.state {
        changed |= state.write(intent);
    }
    if !frame.presses.is_empty() {
        intent.pressed.extend(frame.presses.iter().cloned());
        changed = true;
    }
    if let Some(orders) = &frame.orders {
        let orders: VecDeque<Order> = orders.iter().cloned().collect();
        if intent.orders != orders {
            intent.orders = orders;
            changed = true;
        }
    }
    changed
}

/// What [`InputQueue::receive`] did with a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Received {
    /// Waits for its tick.
    Early,
    /// Its tick has run: applied at the next chance, counted late.
    Late,
    /// Seen before.
    Duplicate,
    /// More than [`MAX_AHEAD`] ticks ahead of the host: dropped.
    TooFarAhead,
}

/// A remote player's frames on the host, by tick.
#[derive(Clone, Debug, Default)]
pub struct InputQueue {
    waiting: BTreeMap<u64, InputFrame>,
    /// Ticks of frames already applied (recent ones; older ticks count as applied).
    applied: BTreeSet<u64>,
    /// Ticks below this are all applied or forgotten.
    floor: u64,
    /// The newest frame tick received (what the client stops repeating).
    pub newest: u64,
    pub late: u64,
    pub early: u64,
    pub dropped: u64,
    pub duplicates: u64,
}

impl InputQueue {
    /// Takes a frame that arrived while the host is at `host_tick` (its next tick is
    /// `host_tick + 1`).
    pub fn receive(&mut self, frame: InputFrame, host_tick: u64) -> Received {
        if frame.tick < self.floor || self.applied.contains(&frame.tick) || self.waiting.contains_key(&frame.tick) {
            self.duplicates += 1;
            return Received::Duplicate;
        }
        if frame.tick > host_tick + MAX_AHEAD {
            self.dropped += 1;
            return Received::TooFarAhead;
        }
        self.newest = self.newest.max(frame.tick);
        let late = frame.tick < host_tick;
        self.waiting.insert(frame.tick, frame);
        if late {
            self.late += 1;
            Received::Late
        } else {
            self.early += 1;
            Received::Early
        }
    }

    /// The frames to apply before the host's tick `host_tick + 1`: every waiting one stamped
    /// `≤ host_tick`, in tick order.
    pub fn take_due(&mut self, host_tick: u64) -> Vec<InputFrame> {
        let later = self.waiting.split_off(&(host_tick + 1));
        let due = std::mem::replace(&mut self.waiting, later);
        self.applied.extend(due.keys().copied());
        // Keep the applied set bounded: anything older than the redundancy window is done.
        let floor = host_tick.saturating_sub(4 * MAX_AHEAD);
        if floor > self.floor {
            self.floor = floor;
            self.applied = self.applied.split_off(&floor);
        }
        due.into_values().collect()
    }

    /// Frames waiting for their tick.
    pub fn waiting(&self) -> usize {
        self.waiting.len()
    }
}

/// The client's side: what it remembered of its own `Intent` and the frames the host has not
/// acknowledged.
#[derive(Clone, Debug, Default)]
pub struct InputCapture {
    state: IntentState,
    orders: VecDeque<Order>,
    /// Frames not acknowledged yet, oldest first.
    pub unacked: VecDeque<InputFrame>,
    /// The presses captured last, for prediction (C4): `(tick, presses)`.
    pub last_presses: (u64, Vec<Press>),
    /// Frames dropped because more than the redundancy waited.
    pub overflow: u64,
    /// Order commands sent and not acknowledged: until then the host's orders do not replace
    /// the local ones.
    pub orders_pending_until: u64,
}

impl InputCapture {
    /// A capture that starts from `intent` (after a join: what the host gave the body).
    pub fn starting_from(intent: &Intent) -> Self {
        Self { state: IntentState::of(intent), orders: intent.orders.clone(), ..Self::default() }
    }

    /// Captures tick `tick` from the own `intent`: presses are taken out of it. Returns the
    /// frame when something changed (it is queued for sending as well).
    pub fn capture(&mut self, tick: u64, intent: &mut Intent, redundancy: usize) -> Option<InputFrame> {
        let state = IntentState::of(intent);
        let mut frame = InputFrame { tick, ..default() };
        if state != self.state {
            frame.state = Some(state.clone());
            self.state = state;
        }
        frame.presses = std::mem::take(&mut intent.pressed);
        if intent.orders != self.orders {
            frame.orders = Some(intent.orders.iter().cloned().collect());
            self.orders = intent.orders.clone();
            self.orders_pending_until = tick;
        }
        if !frame.presses.is_empty() {
            self.last_presses = (tick, frame.presses.clone());
        }
        if frame.is_empty() {
            return None;
        }
        self.unacked.push_back(frame.clone());
        while self.unacked.len() > redundancy.max(1) {
            self.unacked.pop_front();
            self.overflow += 1;
        }
        Some(frame)
    }

    /// The host has every frame up to `tick`.
    pub fn ack(&mut self, tick: u64) {
        while self.unacked.front().is_some_and(|f| f.tick <= tick) {
            self.unacked.pop_front();
        }
    }

    /// The host's orders for the body: taken over (here and in what is remembered) unless an
    /// order command of ours is still on its way.
    pub fn adopt_orders(&mut self, acked: u64, intent: &mut Intent, host: &VecDeque<Order>) {
        if self.orders_pending_until > acked || intent.orders == *host {
            return;
        }
        intent.orders = host.clone();
        self.orders = host.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(input: &str) -> Press {
        Press { input: input.into(), dir: None }
    }

    fn frame(tick: u64) -> InputFrame {
        InputFrame { tick, presses: vec![press("dodge")], ..default() }
    }

    #[test]
    fn presses_apply_exactly_once_and_state_is_last_wins() {
        let mut queue = InputQueue::default();
        let mut intent = Intent::default();
        let walk = |x: f32| IntentState { move_dir: Vec3::new(x, 0.0, 0.0), ..default() };
        assert_eq!(queue.receive(InputFrame { tick: 5, state: Some(walk(1.0)), presses: vec![press("dodge")], orders: None }, 3), Received::Early);
        assert_eq!(queue.receive(InputFrame { tick: 6, state: Some(walk(0.5)), ..default() }, 3), Received::Early);
        // Repeated by redundancy: dropped.
        assert_eq!(queue.receive(frame(5), 3), Received::Duplicate);
        assert!(queue.take_due(4).is_empty(), "early frames wait");
        for f in queue.take_due(6) {
            apply_frame(&mut intent, &f);
        }
        assert_eq!(intent.move_dir.x, 0.5, "the newest state wins");
        assert_eq!(intent.pressed, [press("dodge")]);
        // Applied frames come again (an ack was lost): nothing happens twice.
        assert_eq!(queue.receive(frame(5), 7), Received::Duplicate);
        assert!(queue.take_due(8).is_empty());
        assert_eq!(queue.newest, 6);
    }

    #[test]
    fn late_frames_apply_at_once_and_far_ones_are_dropped() {
        let mut queue = InputQueue::default();
        assert_eq!(queue.receive(frame(10), 40), Received::Late);
        assert_eq!(queue.take_due(40).len(), 1);
        assert_eq!(queue.late, 1);
        assert_eq!(queue.receive(frame(40 + MAX_AHEAD + 1), 40), Received::TooFarAhead);
        assert_eq!(queue.dropped, 1);
        assert_eq!(queue.waiting(), 0);
    }

    #[test]
    fn order_commands_replace_the_queue() {
        let mut intent = Intent::default();
        intent.orders.push_back(Order::MoveTo { target: shards_core::Target::Point(Vec3::X), within: 1.0 });
        let replace = InputFrame { tick: 1, orders: Some(vec![]), ..default() };
        assert!(apply_frame(&mut intent, &replace));
        assert!(intent.orders.is_empty());
        assert!(!apply_frame(&mut intent, &replace), "the same queue again changes nothing");
    }

    #[test]
    fn capture_diffs_against_the_last_tick_and_moves_presses_out() {
        let mut intent = Intent::default();
        let mut capture = InputCapture::starting_from(&intent);
        assert_eq!(capture.capture(1, &mut intent, 32), None, "nothing changed");
        intent.press("dodge");
        intent.move_dir = Vec3::X;
        let first = capture.capture(2, &mut intent, 32).unwrap();
        assert!(intent.pressed.is_empty(), "presses leave the local intent");
        assert_eq!((first.presses.len(), first.state.as_ref().map(|s| s.move_dir)), (1, Some(Vec3::X)));
        intent.set_held("block", true);
        let second = capture.capture(3, &mut intent, 32).unwrap();
        assert_eq!(second.state.unwrap().held, ["block"]);
        assert_eq!(capture.unacked.len(), 2);
        capture.ack(2);
        assert_eq!(capture.unacked.iter().map(|f| f.tick).collect::<Vec<_>>(), [3]);
        for tick in 4..50 {
            intent.press("light");
            capture.capture(tick, &mut intent, 32);
        }
        assert_eq!(capture.unacked.len(), 32, "at most the redundancy waits");
    }
}
