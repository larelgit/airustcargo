//! A mover's way through its keys, as a pure function of its definition, its state and the
//! time: the host's tick runs it, and a co-op client runs the same steps ahead of the host's state
//! (`step::predict_movers`).

use bevy::prelude::*;

use super::def::{MoverDef, MoverLoop};
use super::MoverState;

/// What happened to a mover in a step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Note {
    /// It left key `from`, where it rested, for key `to`.
    Started { from: u32, to: u32 },
    /// It came to rest at `key`.
    Arrived { key: u32 },
}

/// The key a mover resting at `s.from` heads for next, if any: toward its goal, else by its loop.
/// In a `Cycle`, a mover resting at the last key (sent there) is at key 0 again.
fn next_key(def: &MoverDef, s: &mut MoverState) -> Option<u32> {
    let n = def.len();
    if let Some(goal) = s.goal {
        if goal == s.from || goal >= n {
            s.goal = None;
        } else {
            return Some(if goal > s.from { s.from + 1 } else { s.from - 1 });
        }
    }
    match def.looping {
        MoverLoop::Off => None,
        MoverLoop::PingPong => {
            let up = if s.from + 1 >= n {
                false
            } else if s.from == 0 {
                true
            } else {
                s.up
            };
            Some(if up { s.from + 1 } else { s.from - 1 })
        }
        MoverLoop::Cycle => {
            if s.from + 1 >= n {
                s.from = 0;
                s.to = 0;
            }
            Some(s.from + 1)
        }
    }
}

/// Runs the mover `dt` seconds on: rests out its wait, leaves for the next key, passes keys where
/// it does not stop and comes to rest where it does (its goal, a key with a wait, the end of an
/// `Off` mover's way). The notes say when it started and arrived.
pub(crate) fn advance(def: &MoverDef, s: &mut MoverState, dt: f32, notes: &mut Vec<Note>) {
    s.arrived = false;
    let n = def.len();
    if n < 2 || s.from >= n || s.to >= n {
        return;
    }
    let mut left = dt;
    // Each pass rests out a wait, starts a way or ends one: bounded by the keys.
    for _ in 0..(4 * n + 8) {
        if s.from == s.to {
            if s.wait > 0.0 {
                if left < s.wait {
                    s.wait -= left;
                    return;
                }
                left -= s.wait;
                s.wait = 0.0;
            }
            let Some(next) = next_key(def, s) else { return };
            // After `next_key`: a cycle resting at its last key leaves from key 0 (the same pose).
            let from = s.from;
            s.to = next;
            s.t = 0.0;
            s.up = next > s.from;
            notes.push(Note::Started { from, to: next });
            continue;
        }
        let duration = def.duration(s.from, s.to);
        let need = if duration > 0.0 { (1.0 - s.t) * duration } else { 0.0 };
        if left < need {
            // Rounding must not reach the key without arriving there.
            s.t = (s.t + left / duration).min(1.0 - f32::EPSILON);
            return;
        }
        left -= need;
        let key = s.to;
        s.from = key;
        s.t = 0.0;
        let reached = s.goal == Some(key);
        if reached {
            s.goal = None;
        }
        // On its way to a goal it passes the keys between without their waits (a lift sent up
        // two floors does not stop at the first).
        let wait = def.wait_at(key);
        let stops = reached || (s.goal.is_none() && wait > 0.0) || next_key(def, &mut s.clone()).is_none();
        if stops {
            // The next pass rests out the wait, then goes on if anything sends it on.
            s.wait = wait;
            s.arrived = true;
            notes.push(Note::Arrived { key });
            continue;
        }
        // It passes through, on to the next key without a stop.
        if let Some(next) = next_key(def, s) {
            s.to = next;
            s.up = next > s.from;
        }
    }
}

/// Sends the mover to `key`: it goes there through the keys between and rests. A mover on its way
/// turns round when `key` is behind it; one at rest leaves at once (a call does not wait).
pub(crate) fn send(def: &MoverDef, s: &mut MoverState, key: u32) {
    if key >= def.len() {
        return;
    }
    if s.from == s.to {
        s.goal = (key != s.from).then_some(key);
        if s.goal.is_some() {
            s.wait = 0.0;
        }
        return;
    }
    s.goal = Some(key);
    let (a, b) = (s.from, s.to);
    let ahead = if b > a { key >= b } else { key <= b };
    if !ahead {
        turn_round(s);
    }
}

/// On its way from `from` to `to`: the same pose, the other way.
fn turn_round(s: &mut MoverState) {
    std::mem::swap(&mut s.from, &mut s.to);
    s.t = (1.0 - s.t).clamp(0.0, 1.0 - f32::EPSILON);
    s.up = s.to > s.from;
}

/// `Blocked::Reverse`: back to the key it came from, where it rests; held as it was leaving a key,
/// it stays there (a loop tries the other way after its wait).
pub(crate) fn reverse(def: &MoverDef, s: &mut MoverState) {
    if s.from == s.to {
        s.goal = None;
        s.up = !s.up;
        s.wait = def.wait_at(s.from);
        return;
    }
    turn_round(s);
    s.goal = Some(s.to);
}

/// Where the mover stands: its origin moved by the pose between its keys.
pub(crate) fn pose(def: &MoverDef, s: &MoverState) -> Transform {
    let (at, rotation) = def.local(s.from, s.to, s.t);
    Transform { translation: s.origin + s.origin_rotation * at, rotation: (s.origin_rotation * rotation).normalize(), scale: Vec3::ONE }
}

#[cfg(test)]
mod tests {
    use super::super::def::{MoverDef, MoverKey, MoverLoop};
    use super::*;

    fn key(y: f32) -> MoverKey {
        MoverKey { at: Vec3::Y * y, ..default() }
    }

    fn lift(looping: MoverLoop, wait: f32) -> MoverDef {
        MoverDef { keys: vec![key(0.0), key(2.0), key(4.0)], speed: Some(2.0), turn_speed: None, wait, looping, blocked: default() }
    }

    fn run(def: &MoverDef, s: &mut MoverState, ticks: u32) -> Vec<Note> {
        let mut notes = Vec::new();
        for _ in 0..ticks {
            advance(def, s, 0.1, &mut notes);
        }
        notes
    }

    #[test]
    fn an_off_mover_rests_until_sent_and_passes_the_keys_between() {
        // Key 1's wait is for a stop there; on the way to key 2 it does not stop.
        let def = lift(MoverLoop::Off, 1.0);
        let mut s = MoverState::default();
        assert!(run(&def, &mut s, 10).is_empty(), "nothing sends it");
        send(&def, &mut s, 2);
        // 4 m at 2 m/s: 2 s, through key 1 without its wait.
        let notes = run(&def, &mut s, 19);
        assert_eq!(notes, [Note::Started { from: 0, to: 1 }]);
        assert!(s.to == 2 && s.from == 1, "{s:?}");
        let notes = run(&def, &mut s, 2);
        assert_eq!(notes, [Note::Arrived { key: 2 }]);
        assert!(s.from == 2 && s.to == 2 && s.goal.is_none(), "{s:?}");
        assert!((pose(&def, &s).translation.y - 4.0).abs() < 1e-6);
    }

    #[test]
    fn a_call_behind_the_mover_turns_it_round() {
        let def = lift(MoverLoop::Off, 0.0);
        let mut s = MoverState::default();
        send(&def, &mut s, 2);
        run(&def, &mut s, 5);
        let before = pose(&def, &s).translation;
        send(&def, &mut s, 0);
        assert!((pose(&def, &s).translation - before).length() < 1e-5, "the same place, the other way");
        let notes = run(&def, &mut s, 6);
        assert_eq!(notes, [Note::Arrived { key: 0 }]);
        assert_eq!(pose(&def, &s).translation, Vec3::ZERO);
    }

    #[test]
    fn a_ping_pong_mover_goes_up_and_back_and_rests_at_each_key() {
        let def = lift(MoverLoop::PingPong, 0.5);
        let mut s = MoverState::default();
        let notes = run(&def, &mut s, 60);
        let keys: Vec<u32> = notes.iter().filter_map(|n| if let Note::Arrived { key } = n { Some(*key) } else { None }).collect();
        // Each way takes 1 s and each key 0.5 s: in 6 s it reaches 1, 2, 1, 0.
        assert_eq!(keys, [1, 2, 1, 0]);
    }

    #[test]
    fn a_cycle_comes_back_to_key_0_and_goes_on() {
        let spin = |y: f32| MoverKey { turn: Vec3::Y * y, ..default() };
        let def = MoverDef {
            keys: vec![spin(0.0), spin(180.0), spin(360.0)],
            speed: None,
            turn_speed: Some(90.0),
            wait: 0.0,
            looping: MoverLoop::Cycle,
            blocked: default(),
        };
        let mut s = MoverState::default();
        let notes = run(&def, &mut s, 50);
        assert_eq!(notes, [Note::Started { from: 0, to: 1 }], "it never rests");
        // 5 s at 90°/s: 450°, a whole turn and a quarter.
        assert!((s.from, s.to) == (0, 1), "{s:?}");
        let turned = pose(&def, &s).rotation;
        assert!(turned.angle_between(Quat::from_rotation_y(std::f32::consts::FRAC_PI_2)) < 1e-3, "{turned}");
    }

    #[test]
    fn a_cycle_that_rests_at_its_last_key_leaves_from_key_0() {
        let spin = |y: f32| MoverKey { turn: Vec3::Y * y, ..default() };
        let def = MoverDef {
            keys: vec![spin(0.0), spin(180.0), spin(360.0)],
            speed: None,
            turn_speed: Some(90.0),
            wait: 0.5,
            looping: MoverLoop::Cycle,
            blocked: default(),
        };
        let mut s = MoverState::default();
        // 2 s a half turn, half a second's rest at each key: at the last key by 5 s, gone by 5.5 s.
        let notes = run(&def, &mut s, 60);
        let starts: Vec<Note> = notes.into_iter().filter(|n| matches!(n, Note::Started { .. })).collect();
        assert_eq!(starts, [Note::Started { from: 0, to: 1 }, Note::Started { from: 1, to: 2 }, Note::Started { from: 0, to: 1 }]);
    }

    #[test]
    fn reverse_takes_it_back_to_the_key_it_left() {
        let def = lift(MoverLoop::Off, 0.0);
        let mut s = MoverState::default();
        send(&def, &mut s, 1);
        run(&def, &mut s, 3);
        reverse(&def, &mut s);
        assert_eq!((s.from, s.to, s.goal), (1, 0, Some(0)));
        let notes = run(&def, &mut s, 4);
        assert_eq!(notes, [Note::Arrived { key: 0 }]);
    }
}
