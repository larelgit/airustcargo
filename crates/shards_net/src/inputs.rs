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

/// Frames one `Inputs` packet may carry (`input_redundancy` is at most this).
pub const MAX_FRAMES_PER_PACKET: usize = 64;
/// Presses one frame may carry; a client keeps the rest for its next frames.
pub const MAX_PRESSES_PER_FRAME: usize = 8;
/// Inputs one frame may hold down.
pub const MAX_HELD: usize = 16;
/// Orders one frame's queue may hold.
pub const MAX_ORDERS: usize = 16;
/// Longest input name in a frame (a press, a held input, an attack order's input), bytes.
pub const MAX_INPUT_NAME: usize = 64;
/// Presses a host tick takes from one player, however many of their frames are due at once.
pub const MAX_PRESSES_PER_TICK: usize = 32;
/// How much longer than 1 a client's `move_dir` may be before the host cuts it (rounding).
pub const MOVE_DIR_SLACK: f32 = 1e-4;
/// Largest frame within the limits above, as JSON bytes (with room to spare).
pub const MAX_FRAME_BYTES: usize = 8 * 1024;
/// Largest `Inputs` packet a host reads, in bytes: one larger is over the limits whatever it
/// holds, and is dropped undecoded.
pub const MAX_INPUTS_BYTES: usize = MAX_FRAMES_PER_PACKET * MAX_FRAME_BYTES;

/// Why `inputs` is over the limits a client keeps to, if it is.
pub fn check(inputs: &crate::msg::Inputs) -> Result<(), String> {
    if inputs.frames.len() > MAX_FRAMES_PER_PACKET {
        return Err(format!("{} frames, at most {MAX_FRAMES_PER_PACKET}", inputs.frames.len()));
    }
    let long = |name: &str| name.len() > MAX_INPUT_NAME;
    for frame in &inputs.frames {
        let tick = frame.tick;
        if frame.presses.len() > MAX_PRESSES_PER_FRAME || frame.presses.iter().any(|p| long(&p.input)) {
            return Err(format!("frame {tick}: {} presses, at most {MAX_PRESSES_PER_FRAME} of at most {MAX_INPUT_NAME} bytes", frame.presses.len()));
        }
        if let Some(state) = &frame.state
            && (state.held.len() > MAX_HELD || state.held.iter().any(|h| long(h)))
        {
            return Err(format!("frame {tick}: {} held inputs, at most {MAX_HELD}", state.held.len()));
        }
        if let Some(orders) = &frame.orders {
            let long_order = orders.iter().any(|o| matches!(o, Order::Attack { input, .. } if long(input)));
            if orders.len() > MAX_ORDERS || long_order {
                return Err(format!("frame {tick}: {} orders, at most {MAX_ORDERS}", orders.len()));
            }
        }
    }
    Ok(())
}

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
    ///
    /// The host does not trust a client's numbers: a `move_dir` that is not finite becomes
    /// zero, one longer than 1 (beyond rounding) is cut to length 1, and an `aim` that is not
    /// finite is dropped. An honest client already sends `move_dir` clamped, so its frames
    /// pass unchanged; a bad frame cannot put garbage in `Intent` (the engine rule
    /// `core.intent_sane` would stop the host on it).
    pub fn write(&self, intent: &mut Intent) -> bool {
        let move_dir = if !self.move_dir.is_finite() {
            Vec3::ZERO
        } else if self.move_dir.length() > 1.0 + MOVE_DIR_SLACK {
            // Scaled down first: the length of a huge finite vector overflows to infinity.
            let big = self.move_dir.abs().max_element();
            (self.move_dir / big).clamp_length_max(1.0)
        } else {
            self.move_dir
        };
        let aim = self.aim.filter(|a| a.is_finite());
        let changed = intent.move_dir != move_dir || intent.aim != aim || intent.target != self.target || intent.held != self.held;
        if changed {
            intent.move_dir = move_dir;
            intent.aim = aim;
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
    /// The tick of the newest frame with a held state.
    pub state_sent_at: u64,
}

impl InputCapture {
    /// A capture that starts from `intent` (after a join: what the host gave the body).
    pub fn starting_from(intent: &Intent) -> Self {
        Self { state: IntentState::of(intent), orders: intent.orders.clone(), ..Self::default() }
    }

    /// Captures tick `tick` from the own `intent`: presses are taken out of it (at most
    /// [`MAX_PRESSES_PER_FRAME`]; the rest wait for the next tick). Returns the frame when
    /// something changed (it is queued for sending as well). A frame stays within the limits
    /// the host checks ([`check`]).
    pub fn capture(&mut self, tick: u64, intent: &mut Intent, redundancy: usize) -> Option<InputFrame> {
        let mut state = IntentState::of(intent);
        state.held.truncate(MAX_HELD);
        let orders_changed = intent.orders != self.orders;
        let mut frame = InputFrame { tick, ..default() };
        // A new order queue goes with the held state as this machine holds it: on the host,
        // orders steer the body (`move_dir`, `aim`, `target`), which a client never sees; a
        // `stop` must stop it there too.
        if state != self.state || orders_changed {
            frame.state = Some(state.clone());
            self.state = state;
            self.state_sent_at = tick;
        }
        let take = intent.pressed.len().min(MAX_PRESSES_PER_FRAME);
        frame.presses = intent.pressed.drain(..take).collect();
        if orders_changed {
            frame.orders = Some(intent.orders.iter().take(MAX_ORDERS).cloned().collect());
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

    /// Puts what this machine holds (the held state, the order queue) into `intent`: the
    /// body as the host's world gave it again, while frames after that world's tick still
    /// come to the host. The held state is compared from then on with what the host's body
    /// holds: where the host changed it itself (a region change stops every player, a new
    /// body stands still), the next capture sends it again. The orders are not: the host
    /// cleared them on purpose, and [`InputCapture::adopt_orders`] takes its queue over.
    pub fn hold(&mut self, intent: &mut Intent) {
        let mine = std::mem::replace(&mut self.state, IntentState::of(intent));
        mine.write(intent);
        intent.orders = self.orders.clone();
    }

    /// The host has every frame up to `tick`.
    pub fn ack(&mut self, tick: u64) {
        while self.unacked.front().is_some_and(|f| f.tick <= tick) {
            self.unacked.pop_front();
        }
    }

    /// The host's orders for the body (its `Intent`): taken over (here and in what is
    /// remembered) unless an order command of ours is not applied there yet, or not even
    /// captured (a tool changed the queue this tick; the state is applied before the capture).
    /// `applied` is the newest frame the host applied to the body
    /// (`OwnState::applied_input_tick`), not the newest it received: a received frame waits
    /// for its tick, and the orders sent until then are from before it.
    ///
    /// An order that ended there (the queue is shorter) leaves the body's `aim` and `target`
    /// as it steered them (an attack's lock-on, the aim of its swing): taken over as well,
    /// when the host has every held state of ours and this machine did not change it since.
    pub fn adopt_orders(&mut self, applied: u64, intent: &mut Intent, host: &Intent) {
        if self.orders_pending_until > applied || intent.orders != self.orders || intent.orders == host.orders {
            return;
        }
        let ended = host.orders.len() < intent.orders.len();
        let mut now = IntentState::of(intent);
        now.held.truncate(MAX_HELD);
        if ended && self.state_sent_at <= applied && now == self.state {
            (intent.aim, intent.target) = (host.aim, host.target);
            (self.state.aim, self.state.target) = (host.aim, host.target);
        }
        intent.orders = host.orders.clone();
        self.orders = host.orders.clone();
    }

    /// The host's lock-on target for the body, taken over when the host changed it itself (the
    /// lock-on check let go of a target that died, went out of range or stayed hidden; a
    /// `lock_switch` press or an attack order moved it): when the host has every held state of
    /// ours and this machine did not change its target since it sent it. Otherwise the next
    /// held state this machine sends (any change carries the target along) would lock the
    /// host's body on the old target again, and the prediction would face a target the host let
    /// go of. A client never checks a lock itself: its prediction keeps whatever the host keeps.
    pub fn adopt_target(&mut self, applied: u64, intent: &mut Intent, host: &Intent) {
        if host.target == intent.target || self.state_sent_at > applied || intent.target != self.state.target {
            return;
        }
        intent.target = host.target;
        self.state.target = host.target;
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
    fn a_bad_frame_cannot_put_garbage_in_the_hosts_intent() {
        let mut intent = Intent::default();
        let state = |move_dir: Vec3, aim: Option<Vec3>| IntentState { move_dir, aim, ..default() };
        // An honest, clamped frame passes bit for bit.
        let honest = Vec3::new(0.6, 0.0, 0.8);
        assert!(state(honest, Some(Vec3::X)).write(&mut intent));
        assert_eq!((intent.move_dir, intent.aim), (honest, Some(Vec3::X)));
        // Too long: cut to 1. Not finite: zero, and the aim is dropped.
        state(Vec3::new(1e30, 0.0, 0.0), None).write(&mut intent);
        assert!((intent.move_dir.length() - 1.0).abs() < 1e-4, "{:?}", intent.move_dir);
        state(Vec3::new(f32::NAN, 0.0, 1.0), Some(Vec3::splat(f32::INFINITY))).write(&mut intent);
        assert_eq!((intent.move_dir, intent.aim), (Vec3::ZERO, None));
        // The same bad frame again changes nothing.
        assert!(!state(Vec3::new(f32::NAN, 0.0, 1.0), Some(Vec3::splat(f32::INFINITY))).write(&mut intent));
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

    #[test]
    fn a_new_order_queue_carries_the_held_state() {
        let mut intent = Intent::default();
        let mut capture = InputCapture::starting_from(&intent);
        intent.orders.push_back(Order::MoveTo { target: shards_core::Target::Point(Vec3::X), within: 1.0 });
        let walk = capture.capture(1, &mut intent, 32).unwrap();
        assert!(walk.orders.is_some());
        // `stop`: no orders, nothing held. The client's held state did not change (the host's
        // order steered the body there), yet the host must hear it with the empty queue.
        intent.orders.clear();
        let stop = capture.capture(2, &mut intent, 32).unwrap();
        assert_eq!(stop.orders, Some(vec![]));
        assert_eq!(stop.state.as_ref().map(|s| s.move_dir), Some(Vec3::ZERO), "{stop:?}");
        let mut host = Intent { move_dir: Vec3::X, aim: Some(Vec3::X), ..default() };
        apply_frame(&mut host, &walk);
        host.move_dir = Vec3::X; // steered by the order
        apply_frame(&mut host, &stop);
        assert_eq!((host.move_dir, host.aim), (Vec3::ZERO, None), "stopped on the host");
    }

    #[test]
    fn a_held_state_the_host_reset_is_sent_again_after_its_world() {
        let mut intent = Intent::default();
        let mut capture = InputCapture::starting_from(&intent);
        intent.move_dir = Vec3::X;
        intent.set_held("block", true);
        assert!(capture.capture(1, &mut intent, 32).is_some());
        // The host's world again, its body stopped there (a region change).
        let mut restored = Intent::default();
        capture.hold(&mut restored);
        assert_eq!((restored.move_dir, restored.held.clone()), (Vec3::X, vec!["block".to_owned()]), "still held here");
        let again = capture.capture(2, &mut restored, 32).expect("the host hears it again");
        assert_eq!(again.state.map(|s| (s.move_dir, s.held)), Some((Vec3::X, vec!["block".to_owned()])));
        // A world the host did not change (a resync): nothing to send.
        let mut same = restored.clone();
        capture.hold(&mut same);
        assert_eq!(capture.capture(3, &mut same, 32), None);
    }

    #[test]
    fn frames_stay_within_the_limits_the_host_checks() {
        let mut intent = Intent::default();
        let mut capture = InputCapture::starting_from(&intent);
        for _ in 0..20 {
            intent.press("light");
        }
        let first = capture.capture(1, &mut intent, 32).unwrap();
        assert_eq!(first.presses.len(), MAX_PRESSES_PER_FRAME);
        assert_eq!(intent.pressed.len(), 20 - MAX_PRESSES_PER_FRAME, "the rest wait for the next ticks");
        let inputs = crate::msg::Inputs { tick: 1, frames: vec![first], ..default() };
        assert_eq!(check(&inputs), Ok(()));

        let flood = InputFrame { tick: 2, presses: vec![press("light"); 40_000], ..default() };
        let err = check(&crate::msg::Inputs { tick: 2, frames: vec![flood], ..default() }).unwrap_err();
        assert!(err.contains("40000 presses"), "{err}");
        let many = crate::msg::Inputs { tick: 3, frames: (0..100).map(frame).collect(), ..default() };
        assert!(check(&many).unwrap_err().contains("100 frames"));
        let held = IntentState { held: vec!["block".into(); 17], ..default() };
        let err = check(&crate::msg::Inputs { frames: vec![InputFrame { tick: 4, state: Some(held), ..default() }], ..default() }).unwrap_err();
        assert!(err.contains("17 held"), "{err}");
        let long = press(&"x".repeat(MAX_INPUT_NAME + 1));
        assert!(check(&crate::msg::Inputs { frames: vec![InputFrame { tick: 5, presses: vec![long], ..default() }], ..default() }).is_err());
    }

    /// The host's `Intent` of the body with `orders`.
    fn host(orders: &[Order]) -> Intent {
        Intent { orders: orders.iter().cloned().collect(), ..default() }
    }

    #[test]
    fn the_largest_packet_within_the_limits_is_under_the_bytes_a_host_reads() {
        // Every name at its longest, every character one JSON escapes.
        let name = "\"".repeat(MAX_INPUT_NAME);
        let far = Vec3::splat(-1.234_567_9e-38);
        let frame = InputFrame {
            tick: u64::MAX,
            state: Some(IntentState { move_dir: far, aim: Some(far), target: Some(AiId(u32::MAX)), held: vec![name.clone(); MAX_HELD] }),
            presses: vec![Press { input: name.clone(), dir: Some(far) }; MAX_PRESSES_PER_FRAME],
            orders: Some(vec![Order::Attack { target: AiId(u32::MAX), input: name, repeat: false }; MAX_ORDERS]),
        };
        assert!(crate::msg::encode(&frame).len() <= MAX_FRAME_BYTES, "{} bytes", crate::msg::encode(&frame).len());
        let packet = crate::msg::Inputs { tick: u64::MAX, frames: vec![frame; MAX_FRAMES_PER_PACKET], state_ack: u64::MAX, epoch: u32::MAX };
        assert_eq!(check(&packet), Ok(()));
        assert!(crate::msg::encode(&packet).len() <= MAX_INPUTS_BYTES);
    }

    #[test]
    fn orders_are_taken_from_the_host_once_it_applied_ours() {
        let mut intent = Intent::default();
        let mut capture = InputCapture::starting_from(&intent);
        let order = Order::MoveTo { target: shards_core::Target::Point(Vec3::X), within: 1.0 };
        intent.orders.push_back(order.clone());
        capture.capture(10, &mut intent, 32).unwrap();
        // A state from before tick 10's frame ran on the host: its queue is the old one.
        capture.adopt_orders(9, &mut intent, &host(&[]));
        assert_eq!(intent.orders.len(), 1, "ours is not applied there yet: kept");
        capture.adopt_orders(10, &mut intent, &host(std::slice::from_ref(&order)));
        assert_eq!(intent.orders.len(), 1);
        // The order finished on the host.
        capture.adopt_orders(12, &mut intent, &host(&[]));
        assert!(intent.orders.is_empty());
        assert_eq!(capture.capture(13, &mut intent, 32), None, "taken over: nothing to send");
        // A new order, given before this tick's capture: a state of the host's arriving
        // first does not take it back.
        intent.orders.push_back(order);
        capture.adopt_orders(13, &mut intent, &host(&[]));
        assert_eq!(intent.orders.len(), 1, "not captured yet: kept");
        assert!(capture.capture(14, &mut intent, 32).is_some_and(|f| f.orders.is_some_and(|o| o.len() == 1)));
    }

    #[test]
    fn an_order_that_ended_on_the_host_leaves_its_aim_and_lock_on_here_too() {
        // Locked on e6, then a one-shot attack on e7.
        let mut intent = Intent { target: Some(AiId(6)), ..default() };
        let mut capture = InputCapture::starting_from(&intent);
        let swing = Order::Attack { target: AiId(7), input: "light".into(), repeat: false };
        intent.orders.push_back(swing.clone());
        capture.capture(10, &mut intent, 32).unwrap();
        // The swing ended on the host: the lock-on moved to e7, the aim stayed on it.
        let ended = Intent { aim: Some(Vec3::Z), target: Some(AiId(7)), ..default() };
        capture.adopt_orders(12, &mut intent, &ended);
        assert_eq!((intent.orders.len(), intent.aim, intent.target), (0, Some(Vec3::Z), Some(AiId(7))));
        assert_eq!(capture.capture(13, &mut intent, 32), None, "the host holds it already: nothing to send");

        // Locked on e8 since the last state the host applied: the player's lock-on stays.
        intent.orders.push_back(swing.clone());
        capture.capture(20, &mut intent, 32).unwrap();
        intent.target = Some(AiId(8));
        capture.capture(21, &mut intent, 32).unwrap();
        capture.adopt_orders(20, &mut intent, &ended);
        assert_eq!((intent.orders.len(), intent.target), (0, Some(AiId(8))), "e8 is on its way");
        // Changed and not captured yet: kept as well.
        intent.orders.push_back(swing);
        capture.capture(30, &mut intent, 32).unwrap();
        intent.target = Some(AiId(9));
        capture.adopt_orders(30, &mut intent, &ended);
        assert_eq!((intent.orders.len(), intent.target), (0, Some(AiId(9))));
    }

    #[test]
    fn a_lock_the_host_let_go_of_is_not_sent_back() {
        let mut intent = Intent { target: Some(AiId(6)), ..default() };
        let mut capture = InputCapture::starting_from(&Intent::default());
        assert_eq!(capture.capture(10, &mut intent, 32).unwrap().state.unwrap().target, Some(AiId(6)));
        let let_go = Intent::default();
        // The host has not applied the lock yet: its `None` is from before it, kept.
        capture.adopt_target(9, &mut intent, &let_go);
        assert_eq!(intent.target, Some(AiId(6)));
        // Applied, and the host let go of it (its target died): taken over, and walking on does
        // not send the old target back.
        capture.adopt_target(10, &mut intent, &let_go);
        assert_eq!(intent.target, None);
        intent.move_dir = Vec3::X;
        assert_eq!(capture.capture(11, &mut intent, 32).unwrap().state.unwrap().target, None);
        // A switch the host made (a `lock_switch` press) is taken over the same way.
        intent.target = Some(AiId(6));
        capture.capture(12, &mut intent, 32).unwrap();
        capture.adopt_target(12, &mut intent, &Intent { target: Some(AiId(7)), ..default() });
        assert_eq!(intent.target, Some(AiId(7)));
        assert_eq!(capture.capture(13, &mut intent, 32), None, "nothing of ours changed");
        // A target this machine set since it sent its state is its own: kept, and sent.
        intent.target = Some(AiId(8));
        capture.adopt_target(13, &mut intent, &let_go);
        assert_eq!(intent.target, Some(AiId(8)));
        assert_eq!(capture.capture(14, &mut intent, 32).unwrap().state.unwrap().target, Some(AiId(8)));
    }
}
