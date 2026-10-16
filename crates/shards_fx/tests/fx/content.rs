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
        "broken.effect.ron",
        "a game has one table of effects, and this one has more",
        "no engine event `damged` (did you mean `damaged`?)",
        "sprak",
        "more.fx.ron",
    ] {
        assert!(said.contains(wanted), "no `{wanted}` in:\n{said}");
    }
}
