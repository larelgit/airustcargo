//! The yard sounds like the fight: its music turns from calm to tension to combat as the hero
//! comes to the grunt, the grunt's attacks warn with their sounds, and the menus click.

use shards::ai_core::JournalKind;
use shards::audio::Music;
use shards::prelude::*;
use shards::serde_json::json;

use crate::{say, step, SEED};

fn layer(app: &App) -> Option<String> {
    app.world().resource::<Music>().layer.clone()
}

/// Music lines: this machine's own, not gameplay events.
fn music_lines(app: &App) -> Vec<String> {
    let journal = app.world().resource::<shards::ai_core::AiJournal>();
    journal.after(0).0.filter(|e| e.kind == JournalKind::Local).map(|e| e.text.clone()).collect()
}

#[test]
fn the_music_turns_to_combat_when_the_hero_comes_to_the_grunt() {
    let mut app = my_game::test_app(SEED);
    step(&mut app, 60);
    assert_eq!(layer(&app).as_deref(), Some("explore"), "22 m from the grunt: calm");

    say(&mut app, "move_to", json!({"target": "e2", "within": 3}));
    let went = say(&mut app, "step", json!({"ticks": 600, "until": [{"named": "in_fight"}]}));
    assert!(went.contains("in_fight"), "{went}");
    step(&mut app, 1);
    assert_eq!(layer(&app).as_deref(), Some("combat"));
    assert_eq!(music_lines(&app), ["music: yard plays explore", "music: yard explore → tension", "music: yard tension → combat"]);

    // The drums come in over a second; `observe` says the layer.
    step(&mut app, 60);
    let heard = say(&mut app, "audio", json!({}));
    assert!(heard.contains("  music/yard_drums.wav [yard] 1.00\n"), "{heard}");
    assert!(say(&mut app, "observe", json!({})).contains("MUSIC  yard: combat for 1.0s"));
}

#[test]
fn the_grunts_attacks_warn_with_their_sounds_and_the_menus_click() {
    let mut app = my_game::test_app(SEED);
    say(&mut app, "god", json!({}));
    say(&mut app, "move_to", json!({"target": "e2", "within": 2}));
    let warned = say(&mut app, "step", json!({"ticks": 900, "until": [{"journal": "winds up"}]}));
    assert!(warned.contains("winds up"), "{warned}");
    let heard = say(&mut app, "audio", json!({}));
    assert!(heard.contains(" grunt_swipe sfx e2 ") || heard.contains(" grunt_slam sfx e2 "), "the warning plays where the attack lands:\n{heard}");

    say(&mut app, "ui_open", json!({"screen": "pause"}));
    let heard = say(&mut app, "audio", json!({}));
    assert!(heard.contains(" ui_open ui "), "{heard}");
}

#[test]
fn the_heros_swing_whooshes_on_the_simulation_clock_with_no_model_drawn() {
    // The whoosh is the action's own `Sound` effect (hero.action.ron), not the clip's cue: a cue
    // sounds only where a model plays the clip, and this game runs headless.
    let mut app = my_game::test_app(SEED);
    assert!(!say(&mut app, "audio", json!({})).contains(" whoosh "), "nothing before the swing");
    assert!(say(&mut app, "light_attack", json!({})).starts_with("ok: light_1"));
    step(&mut app, 8);
    assert!(!say(&mut app, "audio", json!({})).contains(" whoosh "), "the swing is still winding up (0.18 s)");
    step(&mut app, 6);
    let heard = say(&mut app, "audio", json!({}));
    assert!(heard.contains(" whoosh sfx e1 "), "the swing's whoosh plays from the hero's body:\n{heard}");
}
