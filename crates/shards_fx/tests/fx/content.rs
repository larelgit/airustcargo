//! The content check: what a broken effect or table of effects is told, file and all.

use serde_json::json;

use crate::*;

fn errors(app: &mut App) -> String {
    let out = shards::testing::call(app, "content_check", json!({}));
    text(&out)
}

#[test]
fn the_test_content_is_clean() {
    let assets = Folder::new("clean");
    let mut app = game(&assets);
    let said = errors(&mut app);
    assert!(said.starts_with("ok: "), "{said}");
    let glow: serde_json::Value = serde_json::from_str(&ok(&mut app, "content_get", json!({"kind": "effect", "id": "glow"}))).unwrap();
    assert_eq!((glow["file"].as_str(), glow["def"]["count"].as_u64()), (Some("content/fx/test.effect.ron"), Some(4)));
}

#[test]
fn a_broken_effect_says_what_and_where() {
    let assets = Folder::new("broken");
    assets.put(
        "content/fx/broken.effect.ron",
        r##"{
            "upside_down": (lifetime: (0.8, 0.2)),
            "too_many": (count: 40, max_live: 16),
            "nothing": (count: 0),
            "bad_colour": (color: "#12345"),
            "too_wide": (spread: 400.0),
            "endless": (count: 16, rate: 5e9, duration: 1.0, max_live: 64),
            "flat": (size_ease: 0.0),
            "shrinking": (swell: -1.0),
            "backwards": (push: (1.0, 0.5)),
            "darker_than_black": (intensity: -1.0),
            "uncountable": (strength: (count_per: NaN)),
        }"##,
    );
    assets.put("content/fx/more.fx.ron", r#"{"more": (effects: {"damged": "spark", "blocked": "sprak"}, cues: {"": "glow"})}"#);
    let mut app = game(&assets);
    let said = errors(&mut app);
    for wanted in [
        "lifetime: (min, max) with min above 0 and at most max, not (0.8, 0.2)",
        "one play starts 40 particles and max_live keeps 16: raise max_live to 40 at least",
        "it starts no particle: give `count`, or `rate` and `duration`",
        "#12345",
        "spread: ",
        // A rate too big to count: named, not an overflow (a panic in a debug build, a pass in a
        // release one).
        "one play starts 4294967295 particles and max_live keeps 64",
        "size_ease: above 0 (1 a line), not 0",
        "swell: 0 or more, not -1",
        "push: (min, max) with min 0 or more and at most max, not (1, 0.5)",
        "intensity: 0 or more, not -1",
        "strength.count_per: a number of particles, not NaN",
        "broken.effect.ron",
        "a game has one table of effects, and this one has more",
        "no engine event `damged` (did you mean `damaged`?)",
        "sprak",
        "more.fx.ron",
    ] {
        assert!(said.contains(wanted), "no `{wanted}` in:\n{said}");
    }
}

#[test]
fn a_broken_mark_a_broken_chunk_and_a_broken_link_say_what_and_where() {
    let assets = Folder::empty("gore_broken");
    assets.put(
        "content/fx/bad.mark.ron",
        r##"{
            "hole": (size: (0.1, 0.2)),
            "upside_down": (size: (0.5, 0.1)),
            "no_cap": (max_live: 0),
            "backwards": (fade_in: -1.0),
            "never_ends": (fade_out: 2.0),
            "crowded": (lifetime: Secs(1.0), fade_in: 0.8, fade_out: 0.8),
            "short": (lifetime: Secs(0.0)),
            "thin": (depth: 0.0),
            "shiny": (roughness: 2.0),
            "bad_colour": (color: "red"),
            "climbs": (texture: "../hole.png"),
            "not_a_png": (texture: "textures/hole.jpg"),
            "missing": (texture: "textures/none.png"),
            "twice": (tags: ["gore", "gore", ""]),
        }"##,
    );
    assets.put(
        "content/fx/bad.effect.ron",
        r##"{
            "bouncy": (collide: true, restitution: 1.5),
            "sticky": (collide: true, friction: -1.0),
            "lonely": (restitution: 0.9),
            "stays_up": (rest: UntilCleared),
            "floats": (collide: true, rest: UntilCleared, gravity: 1.0),
            "meshy": (mesh: "models/none.glb", shape: Cube, image: "textures/none.png"),
            "wrong_file": (mesh: "models/chunk.obj"),
            "flat": (image: "textures/leaf.png", shape: Cube),
        }"##,
    );
    assets.put(
        "content/fx/links.fx.ron",
        r#"{"links": (
            marks: {
                "shot_imapct": [(mark: "hole")],
                "damaged": [(mark: "hol", place: Behind(0.0), count: 99, spread: 200.0)],
                "died": [(mark: "hole")],
                "shot_impact": [(mark: "hole", spread: 1.0), (mark: "hole", count: 2), (mark: "hole")],
                "shot_hit": [(mark: "hole", place: Below(150.0))],
            },
        )}"#,
    );
    let mut app = game(&assets);
    let said = errors(&mut app);
    for wanted in [
        "size: (min, max) in metres with min above 0 and at most max, not (0.5, 0.1)",
        "max_live: 1 to 4096, not 0",
        "fade_in: seconds, 0 or more, not -1",
        "fade_out: a mark that lasts until cleared has no end to fade out towards",
        "fade_in 0.8 and fade_out 0.8 do not fit in lifetime Secs(1)",
        "lifetime: Secs(n) with n above 0 and at most 86400, not 0",
        "depth: metres above 0, not 0",
        "roughness: 0 to 1, not 2",
        "color: ",
        "texture `../hole.png` leaves the assets folder",
        "texture `textures/hole.jpg` is not a .png file",
        "texture `textures/none.png` is no file in the assets folder",
        "tags[1]: `gore` twice",
        "tags[2]: a word of 1 to 64 characters without control characters, not ``",
        "bad.mark.ron",
        "restitution: 0 (it lands dead) to 1 (it keeps all its speed), not 1.5",
        "friction: 0 or more, not -1",
        "restitution and friction act on a particle that lands: add `collide: true`",
        "rest: UntilCleared is for a particle that lands: add `collide: true`",
        "rest: UntilCleared needs `gravity` below 0",
        "mesh `models/none.glb` is no file in the assets folder",
        "mesh replaces `shape`",
        "mesh and image: a particle is a mesh or a picture",
        "mesh `models/chunk.obj` is not a .glb or .gltf file",
        "image: only a `Quad` particle wears a picture",
        "bad.effect.ron",
        "no event `shot_imapct` lays marks (did you mean `shot_impact`?)",
        "marks.damaged[0].mark -> unknown mark `hol` (did you mean `hole`?)",
        "marks.damaged[0].count: 1 to 16, not 99",
        "marks.damaged[0].spread: 0 to 180 degrees across, not 200",
        "marks.damaged[0].place: a reach above 0 and at most 100 metres, not 0",
        "marks.died[0].place: `died` has no surface of its own to lay a mark on",
        "marks.shot_impact[0]: `spread` and `offset` search for a surface",
        "marks.shot_impact[1].count: `At` lays one mark on the one surface",
        "marks.shot_hit[0].place: a reach above 0 and at most 100 metres, not 150",
        "links.fx.ron",
    ] {
        assert!(said.contains(wanted), "no `{wanted}` in:\n{said}");
    }
    // The one good thing in all of it raises no complaint.
    assert!(!said.contains("mark `hole`:") && !said.contains("marks.shot_impact[2]"), "{said}");
}
