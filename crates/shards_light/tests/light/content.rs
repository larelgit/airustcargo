//! The content check: what a broken light is told, file and field.

use shards::content::ContentDef;
use shards::light::LightDef;

use crate::*;

fn errors(app: &mut App) -> String {
    text(&testing::call(app, "content_check", json!({})))
}

#[test]
fn the_test_lights_are_clean_and_not_simulation_content() {
    let assets = Folder::new("clean");
    let mut app = game(&assets);
    let said = errors(&mut app);
    assert!(said.starts_with("ok: "), "{said}");
    let beam: Value = serde_json::from_str(&ok(&mut app, "content_get", json!({"kind": "light", "id": "beam"}))).unwrap();
    assert_eq!(beam["file"].as_str(), Some("content/light/test.light.ron"));
    assert_eq!(beam["def"]["kind"]["Spot"]["outer_deg"], json!(60.0));
    // Only a picture reads a light: no replay's content hash changes with it.
    const { assert!(!<LightDef as ContentDef>::SIMULATION) };
}

#[test]
fn a_broken_light_says_what_and_where() {
    let assets = Folder::new("broken");
    assets.put(
        "content/light/test.light.ron",
        r##"{
            "dark": (intensity: -5.0, range: 0.0),
            "wide": (kind: Spot(inner_deg: 0.0, outer_deg: 200.0)),
            "both": (color: "#ffffff", kelvin: 3000.0),
            "pattern": (flicker: Pattern(pattern: "ma7", rate: 10.0)),
            "empty": (flicker: Pattern(pattern: "", rate: 10.0)),
            "fast": (flicker: Noise(rate: 0.0, depth: 2.0)),
            "hex": (color: "orange"),
        }"##,
    );
    let mut app = game(&assets);
    let said = errors(&mut app);
    for wanted in [
        "light `dark`: intensity: 0 or more lumens, not -5",
        "light `dark`: range: above 0 and at most 1000 metres, not 0",
        "light `wide`: kind: a Spot needs 0 < inner_deg <= outer_deg < 180",
        "light `both`: color and kelvin: a light has one colour",
        "light `pattern`: flicker.pattern: only the letters a to z, found `7` at 2",
        "light `empty`: flicker.pattern: at least one letter from a to z",
        "light `fast`: flicker.rate: changes a second, above 0 and at most 240, not 0",
        "light `fast`: flicker.depth: from 0 (steady) to 1 (down to dark), not 2",
        "light `hex`: color: `orange` is not a colour",
    ] {
        assert!(said.contains(wanted), "`{wanted}` in\n{said}");
    }
    assert!(said.contains("test.light.ron"), "the file is named: {said}");
}

#[test]
fn a_typo_in_a_field_is_a_parse_error_with_the_file() {
    let assets = Folder::new("typo");
    assets.put("content/light/test.light.ron", r##"{"lamp": (colour: "#ffffff")}"##);
    let mut app = game(&assets);
    let said = errors(&mut app);
    assert!(said.contains("test.light.ron") && said.contains("colour"), "{said}");
}

#[test]
fn a_broken_grading_is_named_too() {
    let assets = Folder::new("grading");
    assets.put(
        "content/light/rust.grading.ron",
        r#"{"rust": (blend_s: -1.0, grading: (temperature: 3.0, gamma: 0.0)), "ok": (blend_s: 2.0, grading: (temperature: 0.5))}"#,
    );
    let mut app = game(&assets);
    let said = errors(&mut app);
    for wanted in [
        "grading `rust`: blend_s: from 0 to 60 seconds, not -1",
        "grading `rust`: grading.temperature: from -1 to 1, not 3",
        "grading `rust`: grading.gamma: from 0.05 to 4, not 0",
    ] {
        assert!(said.contains(wanted), "`{wanted}` in\n{said}");
    }
    assert!(!said.contains("grading `ok`"), "{said}");
}
