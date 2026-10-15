//! Seeded random player inputs: what `shards fuzz` plays a game with, and what the agents of a
//! soak (`shards soak`, [`crate::soak`]) play every member of a stand with. The same seed and
//! the same observations make the same calls.

use serde_json::{json, Value};

const DIRS: [&str; 8] = ["N", "NE", "E", "SE", "S", "SW", "W", "NW"];

/// SplitMix64: tiny and seedable, plenty to pick inputs with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SplitMix(pub u64);

impl SplitMix {
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// `0..n` (`0` when `n` is 0).
    pub fn below(&mut self, n: u64) -> u64 {
        self.next_u64() % n.max(1)
    }
}

/// Ids of the entities an observation lists (not your own, not journal lines).
pub fn ids_in(observation: &str) -> Vec<String> {
    observation
        .lines()
        .skip(1) // header: tick, pause, seed, you
        .take_while(|l| !l.starts_with("SINCE"))
        .filter(|l| !l.starts_with("YOU"))
        .flat_map(|l| l.split([' ', '·']))
        .filter(|t| is_id(t))
        .map(str::to_owned)
        .collect()
}

/// The body an observation is seen from: `e5` of `… | you e5` on its first line.
pub fn own_body(observation: &str) -> Option<String> {
    let header = observation.lines().next()?;
    let (_, you) = header.rsplit_once("| you ")?;
    let id = you.split_whitespace().next()?;
    is_id(id).then(|| id.to_owned())
}

/// Whether the observer's own body is dead: its `YOU` line says so.
pub fn own_body_dead(observation: &str) -> bool {
    observation.lines().find(|l| l.starts_with("YOU")).is_some_and(|l| l.split([' ', '·', ',']).any(|w| w == "dead"))
}

fn is_id(t: &str) -> bool {
    t.len() > 1 && t.starts_with('e') && t[1..].bytes().all(|b| b.is_ascii_digit())
}

/// One random player input: a tool and its arguments. Targets come from `ids` (what the
/// player sees); with none, the inputs that need one fall back to inputs that do not.
pub fn random_input(rng: &mut SplitMix, ids: &[String]) -> (&'static str, Value) {
    let dir = |rng: &mut SplitMix| DIRS[rng.below(DIRS.len() as u64) as usize];
    let target = |rng: &mut SplitMix| ids.get(rng.below(ids.len() as u64) as usize).cloned();
    match rng.below(3) {
        0 => ("move", json!({"dir": dir(rng), "meters": 1 + rng.below(8)})),
        1 => match target(rng) {
            Some(id) if rng.below(2) == 0 => ("attack", json!({"target": id, "repeat": rng.below(2) == 0})),
            // `attack` needs a target; a plain swing in the facing direction is `light_attack`.
            _ => ("light_attack", json!({})),
        },
        _ => match target(rng) {
            Some(id) if rng.below(2) == 0 => ("dodge", json!({"away_from": id})),
            _ => ("dodge", json!({"dir": dir(rng)})),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OBSERVATION: &str = "tick 0 (0.00s) | paused | speed 1x | seed 42 | you e5\n\
                               YOU  e5 Player  hp 100/100\n\
                               SEEN (by threat)\n  e6 Boss \"Frost Warden\"  hp 4000/4000  18.0m N\n  e7 Skeleton  12.6m W\n\
                               LANDMARKS  e3 Pillar 8.6m W · e4 Pillar 8.6m E\n\
                               SINCE seq 0\n  0 e5 -> e9 Sword";

    #[test]
    fn inputs_come_from_the_seed() {
        assert_eq!(ids_in(OBSERVATION), ["e6", "e7", "e3", "e4"]);
        let run = |seed| {
            let mut rng = SplitMix(seed);
            (0..20).map(|_| random_input(&mut rng, &ids_in(OBSERVATION))).collect::<Vec<_>>()
        };
        assert_eq!(run(7), run(7));
        assert_ne!(run(7), run(8));
        let tools: Vec<&str> = run(7).iter().map(|(t, _)| *t).collect();
        assert!(["move", "attack", "light_attack", "dodge"].iter().all(|t| tools.contains(t)), "{tools:?}");
        // `attack` takes a target (a call without one is refused), with entities seen or not.
        for ids in [ids_in(OBSERVATION), Vec::new()] {
            let mut rng = SplitMix(3);
            let inputs: Vec<_> = (0..200).map(|_| random_input(&mut rng, &ids)).collect();
            assert!(inputs.iter().filter(|(t, _)| *t == "attack").all(|(_, args)| args["target"].is_string()), "{inputs:?}");
            assert!(inputs.iter().any(|(t, _)| *t == "light_attack"), "a plain swing is still sent");
        }
    }

    #[test]
    fn the_observer_and_its_death_are_read_from_the_observation() {
        assert_eq!(own_body(OBSERVATION).as_deref(), Some("e5"));
        assert!(!own_body_dead(OBSERVATION));
        assert_eq!(own_body("tick 0 (0.00s) | paused | speed 1x | seed 42\nYOU nobody"), None);
        let dead = "tick 9 (0.15s) | paused | speed 1x | seed 42 | you e15\nYOU  e15 Player  hp 0/100 · dead\n";
        assert_eq!(own_body(dead).as_deref(), Some("e15"));
        assert!(own_body_dead(dead));
        assert!(!own_body_dead("tick 9 | you e15\nYOU  e15 Player  hp 3/100\n  e6 Boss · dead"), "only the YOU line counts");
    }
}
