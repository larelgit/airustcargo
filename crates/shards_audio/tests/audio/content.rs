//! The content check: what would not play, or not as meant, is an error naming the file.

use shards::content::{check_dir, ContentKinds};

use crate::*;

#[test]
fn the_content_check_names_what_would_not_play() {
    let assets = Folder::new("check");
    let app = game(&assets);
    let kinds = app.world().resource::<ContentKinds>().clone();
    assert_eq!(check_dir(assets.path(), &kinds).unwrap().to_string(), "ok: 7 definitions in 3 files");

    assets.put("sounds/notes.txt", "la la la");
    assets.put(
        "content/audio/broken.sound.ron",
        r#"{
            "hiss": (files: ["sounds/hiss.wav"]),
            "hum": (files: ["sounds/notes.txt"]),
            "song": (files: ["sounds/beep.wav"], bus: Music),
            "far": (files: ["sounds/beep.wav"], spatial: (near: 5.0, far: 2.0)),
            "never": (files: ["sounds/beep.wav"], max_instances: 0),
        }"#,
    );
    assets.put(
        "content/audio/broken.music.ron",
        r#"{
            "uneven": (
                stems: [(file: "music/calm.wav", layers: ["explore"]), (file: "sounds/beep.wav", layers: ["combt"])],
                layers: [(name: "explore", when: "fighting"), (name: "combat"), (name: "boss", when: "fighting")],
            ),
            "twice": (
                stems: [(file: "music/calm.wav", layers: ["calm", "loud"])],
                layers: [(name: "calm"), (name: "loud", when: "!!fighting")],
            ),
        }"#,
    );
    assets.put("content/audio/second.audio.ron", r#"{"other": (voices: 0, music: "thme", sounds: {"damagd": "beep"})}"#);
    assets.put("content/bite.action.ron", r#"{"bite": (net_id: 1, phases: [("windup", 0.5), ("active", 0.1)], telegraph: (at: 0.1, sound: "boon"))}"#);
    let errors: Vec<String> = check_dir(assets.path(), &kinds).unwrap_err().iter().map(ToString::to_string).collect();
    let has = |needle: &str| assert!(errors.iter().any(|e| e.contains(needle)), "no `{needle}` in:\n{}", errors.join("\n"));
    has("files: no file `sounds/hiss.wav` under the asset folder");
    has("files: `sounds/notes.txt` ");
    has("bus `Music` is for tracks (`*.music.ron`): a sound plays on `Sfx`, `Ui` or `Voice`");
    has("spatial: `near` 0 or more and `far` beyond it, not near 5 far 2");
    has("max_instances: 0 would never play it; leave it out for no limit");
    has("stems loop together, so they need one length: `music/calm.wav` plays 2.00 s, `sounds/beep.wav` 0.50 s");
    has("stems[1]: no layer `combt` (did you mean `combat`?); layers: explore, combat, boss");
    has("layer `explore` is the first, the one played when no other holds: it takes no `when` (`fighting`)");
    has("layer `combat` has no `when`: only the first layer plays without one");
    has("layer `combat` plays no stem: add it to a stem's `layers`");
    // The music reads one `!`: a second would name a condition no game registers.
    has("layer `loud`: `when` names no condition (`npc_engaged`, `!npcs_calm`; one `!` at most), not `!!fighting`");
    has("a game has one mixer, and this one has more");
    has("voices: 1 to 256, not 0");
    has("thme");
    has("sounds: no engine event `damagd` (did you mean `damaged`?)");
    has("action `bite` warns with sound `boon`, which no `*.sound.ron` defines (did you mean `boom`?)");
}
