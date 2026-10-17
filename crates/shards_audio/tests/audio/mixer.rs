//! The mixer: voices and their limit, distance, the player's volumes, the engine's events and
//! cues, and what the tools say about it.

use serde_json::json;
use shards::action::SoundCue;
use shards::ai_core::JournalKind;
use shards::audio::{play, refresh, Bus, Played, SoundAt};
use shards::prelude::*;
use shards::ui::UiScreenChanged;

use crate::*;

#[test]
fn four_voices_then_a_stronger_sound_takes_the_weakest_and_an_equal_one_is_not_heard() {
    let assets = Folder::new("voices");
    let mut app = game(&assets);
    for id in 1..=4 {
        assert_eq!(play(app.world_mut(), "beep", SoundAt::Everywhere), Ok(Played::Started { id, stole: None }));
    }
    let full = "all 4 voices play sounds as strong or stronger";
    assert_eq!(play(app.world_mut(), "beep", SoundAt::Everywhere), Ok(Played::Dropped(full.into())));
    assert_eq!(play(app.world_mut(), "boom", SoundAt::Everywhere), Ok(Played::Started { id: 5, stole: Some("beep #1 (voice limit 4)".into()) }));
    assert_eq!(playing(&app), ["beep #2", "beep #3", "beep #4", "boom #5"]);

    assert_eq!(ok(&mut app, "sound_play", json!({"sound": "beep", "times": 3})), format!("beep ×3: 0 playing, 3 not heard: {full}"));
    let said = ok(&mut app, "audio", json!({}));
    assert!(said.contains("voices 4/4 (started 5, dropped 4)"), "{said}");
    assert!(said.contains(&format!("  dropped beep at 0.0s: {full}")), "{said}");
}

#[test]
fn a_sound_with_max_instances_takes_its_own_oldest_voice_and_its_files_take_turns() {
    let assets = Folder::new("instances");
    let mut app = game(&assets);
    play(app.world_mut(), "step", SoundAt::Everywhere).unwrap();
    play(app.world_mut(), "step", SoundAt::Everywhere).unwrap();
    assert_eq!(play(app.world_mut(), "step", SoundAt::Everywhere), Ok(Played::Started { id: 3, stole: Some("its own #1 (max 2)".into()) }));
    let files: Vec<String> = voices(&app).iter().map(|v| format!("#{} {}", v.id, v.file)).collect();
    assert_eq!(files, ["#2 sounds/boom.wav", "#3 sounds/beep.wav"]);
}

#[test]
fn a_spatial_sound_fades_with_distance_follows_its_source_and_is_not_started_out_of_earshot() {
    let assets = Folder::new("distance");
    let mut app = game(&assets);
    let at = |x, z| SoundAt::Point { position: Vec3::new(x, 0.0, z), source: None };
    play(app.world_mut(), "beep", at(0.0, -6.0)).unwrap();
    play(app.world_mut(), "beep", at(8.0, 0.0)).unwrap();
    let heard = voices(&app);
    // `(1 - t)²`, `t` from `near` 2 m to `far` 10 m: 6 m is half way, 8 m three quarters.
    near(heard[0].gain, 0.25);
    near(heard[1].gain, 0.0625);
    assert_eq!(heard[0].direction, Some(Vec3::NEG_Z), "ahead");
    assert_eq!(heard[1].direction, Some(Vec3::X), "to the right");
    assert_eq!(play(app.world_mut(), "beep", at(0.0, 12.0)), Ok(Played::Dropped("out of earshot: 12.0 m, heard to 10 m".into())));

    let foe = entity(&app, FOE);
    play(app.world_mut(), "beep", SoundAt::Entity(foe)).unwrap();
    near(voices(&app)[2].gain, ops::powf(1.0 - 3.0 / 8.0, 2.0));
    app.world_mut().get_mut::<Transform>(foe).unwrap().translation = Vec3::new(0.0, 0.0, -1.0);
    refresh(app.world_mut());
    let follows = &voices(&app)[2];
    assert_eq!((follows.distance, follows.gain), (Some(1.0), 1.0), "it follows the foe");
    assert_eq!(follows.source_id.map(|id| id.to_string()).as_deref(), Some(FOE));
}

#[test]
fn voices_end_when_they_have_played_on_the_simulations_clock() {
    let assets = Folder::new("clock");
    let mut app = game(&assets);
    play(app.world_mut(), "beep", SoundAt::Everywhere).unwrap();
    play(app.world_mut(), "boom", SoundAt::Everywhere).unwrap();
    app.update();
    assert_eq!(playing(&app), ["beep #1", "boom #2"], "paused, nothing plays out");
    let half = ticks(&app, 0.5);
    step(&mut app, half + 1);
    assert_eq!(playing(&app), ["boom #2"], "the beep plays 0.5 s");
    step(&mut app, half);
    assert!(playing(&app).is_empty(), "{:?}", playing(&app));
}

#[test]
fn the_players_volumes_apply_to_what_plays_now() {
    let assets = Folder::new("volumes");
    let mut app = game(&assets);
    play(app.world_mut(), "beep", SoundAt::Everywhere).unwrap();
    play(app.world_mut(), "click", SoundAt::Everywhere).unwrap();
    let gains = |app: &App| voices(app).iter().map(|v| v.gain).collect::<Vec<_>>();
    assert_eq!(gains(&app), [1.0, 1.0]);
    ok(&mut app, "settings_set", json!({"group": "audio", "key": "sfx", "value": 0.5}));
    ok(&mut app, "settings_set", json!({"group": "audio", "key": "master", "value": 0.5}));
    app.update();
    assert_eq!(gains(&app), [0.25, 0.5], "sfx 0.5 × master 0.5; the menu's bus at 1 × master");
    near(shards::audio::bus_volume(app.world(), Bus::Music), 0.4);
    let said = ok(&mut app, "audio", json!({}));
    assert!(said.contains("buses (with master): music 0.40 · sfx 0.25 · ui 0.50 · voice 0.50"), "{said}");
}

#[test]
fn the_engines_events_cues_and_menus_play_the_sounds_content_gives_them() {
    let assets = Folder::new("events");
    let mut app = game(&assets);
    // `healed` → beep, from the one healed.
    ok(&mut app, "set", json!({"id": HERO, "component": "Health", "path": "current", "value": 50.0}));
    ok(&mut app, "heal", json!({"id": HERO}));
    let healed = voices(&app);
    assert_eq!(healed.iter().map(|v| (v.sound.as_str(), v.source_id.map(|i| i.to_string()))).collect::<Vec<_>>(), [("beep", Some(HERO.to_owned()))]);

    // An attack's telegraph: its sound where the attack lands, the attacker's.
    let foe = entity(&app, FOE);
    app.world_mut().trigger(SoundCue { entity: foe, cue: "boom".into(), position: Vec3::new(0.0, 0.0, -3.0) });
    app.world_mut().flush();
    let cue = voices(&app).into_iter().find(|v| v.sound == "boom").expect("the cue plays");
    assert_eq!((cue.position, cue.source, cue.source_id.map(|i| i.to_string())), (Some(Vec3::new(0.0, 0.0, -3.0)), None, Some(FOE.to_owned())));

    // A cue no sound has is said once, not every time.
    for _ in 0..2 {
        app.world_mut().trigger(SoundCue { entity: foe, cue: "bom".into(), position: Vec3::ZERO });
        app.world_mut().flush();
    }
    let warned = journal(&app, JournalKind::Warn, "no sound `bom` (did you mean `boom`?)");
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert!(warned[0].starts_with("audio: ") && warned[0].contains("warns"), "{warned:?}");

    // A menu opening sounds on the menus' bus, the same everywhere; the mixer has no `ui_closed`.
    app.world_mut().trigger(UiScreenChanged { screen: "pause".into(), open: true });
    app.world_mut().trigger(UiScreenChanged { screen: "pause".into(), open: false });
    app.world_mut().flush();
    let menu: Vec<(String, Bus, Option<Vec3>)> = voices(&app).into_iter().filter(|v| v.bus == Bus::Ui).map(|v| (v.sound, v.bus, v.position)).collect();
    assert_eq!(menu, [("click".to_owned(), Bus::Ui, None)]);
}

#[test]
fn audio_and_observe_say_what_plays_and_sound_play_explains_a_wrong_call() {
    let assets = Folder::new("tools");
    let mut app = game(&assets);
    wait(&mut app, 1.0);
    assert_eq!(ok(&mut app, "sound_play", json!({"sound": "beep", "at": FOE})), "playing beep #1: sounds/beep.wav at 5.0 m, gain 0.39, 0.50s");
    let said = ok(&mut app, "audio", json!({}));
    for line in [
        "music theme: explore for 1.0s\n",
        "  music/calm.wav [theme] 1.00\n",
        "  music/drums.wav [theme] 0.00\n",
        "buses (with master): music 0.80 · sfx 1.00 · ui 1.00 · voice 1.00\n",
        "voices 1/4 (started 1, dropped 0)\n",
        "  #1 beep sfx e2 5.0m gain 0.39 prio 50 (0.50s)\n",
        "listener e1 at (0.0, 0.0, 0.0)\n",
    ] {
        assert!(said.contains(line), "no {line:?} in:\n{said}");
    }
    let seen = ok(&mut app, "observe", json!({}));
    assert!(seen.contains("MUSIC  theme: explore for 1.0s\n"), "{seen}");

    assert_eq!(refused(&mut app, "sound_play", json!({"sound": "bep"})), "no sound `bep` (did you mean `beep`?): content `*.sound.ron` defines sounds; 4 known");
    assert_eq!(refused(&mut app, "sound_play", json!({"sound": "beep", "at": FOE, "point": [0.0, 0.0, 0.0]})), "give `at` (an entity) or `point` (x, y, z), not both");
    assert_eq!(refused(&mut app, "sound_play", json!({"sound": "beep", "times": 65})), "times: 1 to 64, not 65");
}

#[test]
fn an_actions_sound_effect_plays_from_its_actor_without_a_model_and_an_unknown_one_is_said_once() {
    use shards::action::{apply_effect, Effect};
    let assets = Folder::new("effect");
    let mut app = game(&assets);
    let hero = entity(&app, HERO);
    // What an action's `Sound` effect does on the frame it reaches (the machine calls this): the
    // sound is the actor's own, from where it stands, with no clip, model or window involved.
    apply_effect(app.world_mut(), hero, "swing", &Effect::Sound("beep".into()));
    app.world_mut().flush();
    let heard = voices(&app);
    assert_eq!(heard.iter().map(|v| (v.sound.as_str(), v.bus, v.source_id.map(|i| i.to_string()))).collect::<Vec<_>>(), [("beep", Bus::Sfx, Some(HERO.to_owned()))]);
    assert_eq!(heard[0].position, Some(Vec3::ZERO), "spatial: from where the actor is");

    for _ in 0..2 {
        apply_effect(app.world_mut(), hero, "swing", &Effect::Sound("bep".into()));
        app.world_mut().flush();
    }
    let warned = journal(&app, JournalKind::Warn, "no sound `bep` (did you mean `beep`?)");
    assert_eq!(warned.len(), 1, "{warned:?}");
    assert!(warned[0].starts_with("audio: action `swing` plays it"), "{warned:?}");
}
