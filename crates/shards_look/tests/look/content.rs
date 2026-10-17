//! The content check: a look that would not draw as written, and a prefab that names a look no
//! file defines, are errors naming the file.

use shards::content::{check_dir, ContentDb, ContentKinds};

use crate::*;

#[test]
fn the_content_check_names_what_would_not_draw() {
    let assets = Folder::new("check");
    let app = game(&assets);
    let kinds = app.world().resource::<ContentKinds>().clone();
    assert_eq!(check_dir(assets.path(), &kinds).unwrap().to_string(), "ok: 6 definitions in 2 files");

    assets.put(
        "content/look/broken.look.ron",
        r##"{
            "blue": (color: Some("blue")),
            "short": (color: Some("#12345")),
            "dim": (glow: Some("#ffff")),
            "flat": (shading: Toon(bands: 1)),
            "smooth": (shading: Toon(bands: 9)),
            "rough": (shading: Lit(roughness: 1.5, metallic: -0.1)),
            "hairline": (outline: Some((width: 0.0))),
            "thick": (outline: Some((width: 17.0, color: "black"))),
            "rubble": (materials: ["Rock"]),
            "ruin": (materials: ["stone", ""]),
        }"##,
    );
    let errors: Vec<String> = check_dir(assets.path(), &kinds).unwrap_err().iter().map(ToString::to_string).collect();
    let has = |needle: &str| assert!(errors.iter().any(|e| e.contains(needle)), "no `{needle}` in:\n{}", errors.join("\n"));
    has("broken.look.ron: look `blue`: color: `blue` is not a colour: `#rrggbb` or `#rrggbbaa`");
    has("look `short`: color: `#12345` is not a colour");
    // `#rgb` reads, `#rgba` does not.
    has("look `dim`: glow: `#ffff` is not a colour");
    has("look `flat`: shading: Toon has 2 to 8 bands, not 1");
    has("look `smooth`: shading: Toon has 2 to 8 bands, not 9");
    has("look `rough`: shading: Lit roughness is 0 to 1, not 1.5");
    has("look `rough`: shading: Lit metallic is 0 to 1, not -0.1");
    has("look `hairline`: outline: width is above 0 and at most 16 pixels, not 0");
    has("look `thick`: outline: width is above 0 and at most 16 pixels, not 17");
    has("look `thick`: outline: color: `black` is not a colour");
    has("look `stone`: materials: `Rock` is drawn with look `rubble` already; a material has one look");
    has("look `ruin`: materials: `stone` is the name of look `stone`, which draws that material already");
    has("look `ruin`: materials: an empty name; give a glTF material's name, as Blender shows it");
    assert_eq!(errors.len(), 13, "one error each:\n{}", errors.join("\n"));
}

#[test]
fn a_look_extends_one_some_file_defines_and_never_itself() {
    let assets = Folder::empty("extends");
    let app = game(&assets);
    let kinds = app.world().resource::<ContentKinds>().clone();
    assets.put(
        "content/look/presets.look.ron",
        r##"{
            "low_poly": (flat: true),
            "hero": (extends: "low_pol"),
            "a": (extends: "b"),
            "b": (extends: "a"),
            "neon": (color: Some("pink")),
            "sign": (extends: "neon", glow: Some("#ffff00")),
        }"##,
    );
    let errors: Vec<String> = check_dir(assets.path(), &kinds).unwrap_err().iter().map(ToString::to_string).collect();
    let has = |needle: &str| assert!(errors.iter().any(|e| e.contains(needle)), "no `{needle}` in:\n{}", errors.join("\n"));
    has("presets.look.ron: look `hero`: extends -> unknown look `low_pol` (did you mean `low_poly`?)");
    has("look `a`: extends goes round in a circle: a -> b -> a");
    has("look `b`: extends goes round in a circle: b -> a -> b");
    has("look `neon`: color: `pink` is not a colour");
    // A look that extends a broken one is not broken too: the error is where the value is.
    assert_eq!(errors.len(), 4, "{}", errors.join("\n"));
}

#[test]
fn a_prefab_that_names_a_look_no_file_defines_is_an_error_naming_its_file() {
    let assets = Folder::new("prefab");
    assets.put("content/crates.prefab.ron", r#"{"crate": (components: {"AiKind": ("Crate"), "Look": ("stne")})}"#);
    let app = game(&assets);
    let wanted = "crates.prefab.ron: prefab `crate`: Look -> unknown look `stne` (did you mean `stone`?)";
    // Loaded in a game, where the registry names the component, and checked from the files alone.
    let loaded: Vec<String> = app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect();
    assert!(loaded.iter().any(|e| e.ends_with(wanted)), "{loaded:?}");
    let kinds = app.world().resource::<ContentKinds>().clone();
    let checked: Vec<String> = check_dir(assets.path(), &kinds).unwrap_err().iter().map(ToString::to_string).collect();
    assert!(checked.iter().any(|e| e.ends_with(wanted)), "{checked:?}");
    assert_eq!(checked.len(), 1, "{checked:?}");
}
