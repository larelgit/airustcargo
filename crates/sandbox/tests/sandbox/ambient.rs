//! Placed sounds in the sandbox (F10): the room of `content/audio/audio_room.level.ron`, which is a level file
//! with `SoundEmitter` and `SoundTransmission` as custom properties on its nodes. `ambient_audio.scenario`
//! builds the same room from prefabs and listens to it; a level has no tool to take it away, so a test
//! of its own spawns the file and reads what it made.

use shards::audio::{SoundEmitter, SoundTransmission};
use shards::level::spawn_level_by_id;
use shards::prelude::*;
use shards::serde_json::json;

use crate::common::*;

#[test]
fn the_audio_rooms_level_file_places_its_emitters_and_its_pane() {
    let mut app = sandbox::test_app(42);
    let spawned = spawn_level_by_id(app.world_mut(), "audio_room", None).unwrap();
    assert_eq!(spawned.entities.len(), 6, "a wall, a pane and four emitters");

    let world = app.world_mut();
    let mut found = world.query::<(&Name, Option<&SoundEmitter>, Option<&SoundTransmission>, &Transform)>();
    let mut emitters: Vec<(String, String, bool, Vec3)> = Vec::new();
    let mut panes: Vec<(String, f32)> = Vec::new();
    for (name, emitter, transmission, at) in found.iter(world) {
        if let Some(e) = emitter {
            emitters.push((name.to_string(), e.sound.clone(), e.playing, at.translation));
        }
        if let Some(t) = transmission {
            panes.push((name.to_string(), t.0));
        }
    }
    emitters.sort_by(|a, b| a.0.cmp(&b.0));
    // The level stands at (400, 0, 400) by default and its nodes keep their places from there: a node at
    // z = -6 is 6 m north of it.
    let names: Vec<(&str, &str, bool, Vec3)> = emitters.iter().map(|(n, s, p, at)| (n.as_str(), s.as_str(), *p, *at)).collect();
    assert_eq!(
        names,
        [
            ("Hum.Glass", "room_hum", true, Vec3::new(406.0, 1.0, 400.0)),
            ("Hum.Off", "room_hum", false, Vec3::new(400.0, 1.0, 406.0)),
            ("Hum.Open", "room_hum", true, Vec3::new(394.0, 1.0, 400.0)),
            ("Hum.Wall", "room_hum", true, Vec3::new(400.0, 1.0, 394.0)),
        ]
    );
    assert_eq!(panes, [("Glass".to_owned(), 0.6)]);

    // The hero is nowhere near (the room is 560 m away): every playing hum is out of earshot, and the one
    // that is off says so.
    step(&mut app, 2);
    let said = ok(&mut app, "audio", json!({}));
    let said = text(&said);
    assert!(said.contains("emitters 4 (0 heard)"), "{said}");
    assert!(said.contains("Hum.Off room_hum"), "{said}");
    assert!(said.contains("stopped"), "{said}");
    assert_eq!(said.matches("out of earshot, heard to 18 m").count(), 3, "{said}");
    assert!(app.world().resource::<shards::audio::Mixer>().voices().is_empty());
}

#[test]
fn the_sandboxs_mixer_has_no_occlusion_so_its_sounds_are_as_they_were() {
    let app = sandbox::test_app(42);
    let mixer = shards::audio::mixer::audio_def(app.world()).expect("the sandbox has a mixer");
    assert_eq!(mixer.occlusion, None);
    // And the one looped sound it has is only for emitters.
    let hum = app.world().resource::<shards::content::ContentDb>().get::<shards::audio::SoundDef>("room_hum").cloned().unwrap();
    assert!(hum.looped && hum.spatial.is_some());
}
