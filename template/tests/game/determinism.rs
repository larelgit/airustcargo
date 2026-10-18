//! Same seed + same inputs = same game, bit for bit. This is what makes replays, tests
//! on thousands of seeds and (later) co-op possible; a game keeps it by following the
//! rules in AGENTS.md of the engine (docs/agents/determinism.md): gameplay in `FixedUpdate` inside a `SimSet`,
//! randomness only through `SimRng`, iteration in `AiId` order, input only through `Intent`.

use shards::prelude::*;

use crate::{fight, journal, state, step, SEED};

/// The game after the fight, as text: its whole journal and every entity.
fn played(seed: u64) -> (Vec<String>, Vec<String>) {
    let mut app = my_game::test_app(seed);
    fight(&mut app);
    (journal(&app), state(&mut app))
}

#[test]
fn the_same_seed_gives_the_same_journal_and_world() {
    let (first_journal, first_world) = played(SEED);
    let (second_journal, second_world) = played(SEED);
    assert!(first_journal.iter().any(|l| l.contains("e1 -> e2")), "the hero must land hits:\n{}", first_journal.join("\n"));
    assert_eq!(first_journal, second_journal);
    assert_eq!(first_world, second_world);
}

#[test]
fn another_seed_plays_another_fight() {
    // The grunt chooses between its swipe and its slam with `SimRng`, so the seed shows.
    let (a, _) = played(1);
    let (b, _) = played(2);
    assert_ne!(a, b, "two seeds gave the same fight; is the grunt's choice still random?");
}

#[test]
fn stepping_in_pieces_gives_the_same_game_as_stepping_at_once() {
    let mut whole = my_game::test_app(SEED);
    let mut pieces = my_game::test_app(SEED);
    crate::say(&mut whole, "move_to", shards::serde_json::json!({"target": "e2", "within": 3}));
    crate::say(&mut pieces, "move_to", shards::serde_json::json!({"target": "e2", "within": 3}));
    step(&mut whole, 300);
    for _ in 0..5 {
        step(&mut pieces, 60);
        // Frames between steps (a window draws them) change nothing.
        pieces.update();
    }
    assert_eq!(state(&mut whole), state(&mut pieces));
}
