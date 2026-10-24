//! A headless game: no picture, but the lights' state is there: the `lights` query counts them, reads their flicker
//! off the simulation's clock and says who casts a shadow.

use shards::light::LightState;
use shards::settings::GraphicsSettings;

use crate::*;

/// The line of the `lights` answer about the light called `name`.
fn line<'a>(said: &'a str, name: &str) -> &'a str {
    said.lines().find(|l| l.contains(&format!("\"{name}\""))).unwrap_or_else(|| panic!("no line for {name} in\n{said}"))
}

/// Four lights around a camera 2 m up: `Lamp.A` (steady, shadow) 1 m away, `Lamp.B` (blinks) 3 m, `Lamp.D` (the spot,
/// shadow) 3 m above, `Lamp.C` (wanders) 5 m.
fn room(name: &str) -> (Folder, App) {
    let assets = Folder::new(name);
    let mut app = game(&assets);
    camera(&mut app, Vec3::new(0.0, 2.0, 0.0));
    light(&mut app, "steady", "Lamp.A", Vec3::new(1.0, 2.0, 0.0));
    light(&mut app, "blink", "Lamp.B", Vec3::new(0.0, 2.0, 3.0));
    light(&mut app, "wander", "Lamp.C", Vec3::new(5.0, 2.0, 0.0));
    light(&mut app, "beam", "Lamp.D", Vec3::new(0.0, 5.0, 0.0));
    // The transforms are propagated: the camera is where it was put.
    app.update();
    (assets, app)
}

#[test]
fn the_query_counts_the_lights_and_reads_the_flicker_off_the_clock() {
    let (_assets, mut app) = room("query");
    let said = ok(&mut app, "lights", json!({}));
    assert!(
        said.starts_with("lights: 4 (point 3, spot 1) · 2 flicker · 2 want a shadow, 2 cast one (budget 4: graphics.shadow_lights) · clock 0.000 s\n"),
        "{said}"
    );
    assert!(said.contains("viewer: the camera at (0.0, 2.0, 0.0)"), "{said}");
    // Tick 0: the first letter of the pattern, `m`, is the light as written.
    let blink = line(&said, "Lamp.B");
    assert!(blink.contains("blink \"Lamp.B\" point at (0.0, 2.0, 3.0) 3.0 m away"), "{blink}");
    assert!(blink.contains("pattern `ma` at 2/s, step 0 = m: 100 % (2000 lm)"), "{blink}");
    assert!(line(&said, "Lamp.A").contains("steady · shadow: yes"), "{said}");
    assert!(line(&said, "Lamp.B").contains("shadow: not asked"), "{said}");
    assert!(line(&said, "Lamp.D").contains("spot at (0.0, 5.0, 0.0)"), "{said}");
    assert!(said.contains("grading: the camera's own"), "no volume, no grade: {said}");

    // 45 ticks are three quarters of a second: at two steps a second the second letter, `a`, is on (mid-step, away from a boundary).
    step(&mut app, 45);
    let said = ok(&mut app, "lights", json!({ "id": "blink" }));
    assert!(said.contains("clock 0.750 s"), "{said}");
    assert!(said.contains("pattern `ma` at 2/s, step 1 = a: 0 % (0 lm)"), "{said}");
    step(&mut app, 30);
    let said = ok(&mut app, "lights", json!({ "id": "blink" }));
    assert!(said.contains("step 0 = m: 100 % (2000 lm)"), "it loops: {said}");
    // `id` narrows the list, not the counts.
    assert!(!said.contains("Lamp.A"), "{said}");
    assert!(said.contains("lights: 4 (point 3, spot 1)"), "{said}");
}

#[test]
fn the_flicker_is_a_function_of_the_clock_and_the_same_in_every_run() {
    let run = |name: &str| {
        let (_assets, mut app) = room(name);
        step(&mut app, 77);
        let said = ok(&mut app, "lights", json!({ "id": "wander" }));
        line(&said, "Lamp.C").to_owned()
    };
    let (first, second) = (run("noise_a"), run("noise_b"));
    assert_eq!(first, second, "the noise has a seed of its own, never the simulation's random numbers");
    assert!(first.contains("noise 6/s depth 0.5"), "{first}");
    // Between 1 - depth and 1 times 3000 lm.
    let lumens: f32 = first.split(" lm of ").next().unwrap().rsplit(' ').next().unwrap().parse().unwrap();
    assert!((1500.0..=3000.0).contains(&lumens), "{first}");
    // The same clock reached by one step or two is the same moment.
    let (_assets, mut app) = room("noise_c");
    step(&mut app, 40);
    step(&mut app, 37);
    assert_eq!(line(&ok(&mut app, "lights", json!({ "id": "wander" })), "Lamp.C"), first);
    // A lamp elsewhere with the same definition wanders another way.
    let (_assets, mut app) = room("noise_d");
    light(&mut app, "wander", "Lamp.C2", Vec3::new(-5.0, 2.0, 0.0));
    step(&mut app, 77);
    let said = ok(&mut app, "lights", json!({ "id": "wander" }));
    let other = line(&said, "Lamp.C2").split(" lm of ").next().unwrap().rsplit(' ').next().unwrap().to_owned();
    assert_ne!(other, lumens.to_string(), "{said}");
}

fn casting(app: &mut App) -> Vec<String> {
    app.update();
    let mut names: Vec<String> = app.world().resource::<LightState>().casting().map(|l| l.name.clone()).collect();
    names.sort();
    names
}

#[test]
fn the_nearest_lights_that_ask_for_a_shadow_get_one() {
    let (_assets, mut app) = room("budget");
    assert_eq!(casting(&mut app), ["Lamp.A", "Lamp.D"], "both ask and four may have one");
    app.world_mut().resource_mut::<GraphicsSettings>().shadow_lights = 1;
    assert_eq!(casting(&mut app), ["Lamp.A"], "the nearer of the two: 1 m against 3 m");
    // The camera moves next to the spot: it is the nearer now.
    let viewer = app.world_mut().query_filtered::<Entity, With<shards::ai_core::AiCamera>>().single(app.world()).unwrap();
    app.world_mut().get_mut::<Transform>(viewer).unwrap().translation = Vec3::new(0.0, 4.5, 0.0);
    app.update();
    app.update();
    assert_eq!(casting(&mut app), ["Lamp.D"]);
    let said = ok(&mut app, "lights", json!({}));
    assert!(said.contains("2 want a shadow, 1 cast one (budget 1: graphics.shadow_lights)"), "{said}");
    assert!(line(&said, "Lamp.A").contains("shadow: no (over the budget)"), "{said}");
    // None at all: a budget of 0, and shadows off in the settings.
    app.world_mut().resource_mut::<GraphicsSettings>().shadow_lights = 0;
    assert!(casting(&mut app).is_empty());
    assert!(ok(&mut app, "lights", json!({})).contains("shadow: no (budget 0)"));
    *app.world_mut().resource_mut::<GraphicsSettings>() = GraphicsSettings { shadows: "off".into(), ..default() };
    assert!(casting(&mut app).is_empty(), "`graphics.shadows = off` is no shadow lights, whatever the count says");
    assert_eq!(status(&app)["budget"], json!(0));
    // Lights equally near go by their key, not by the order they were spawned in.
    let (_assets, mut twins) = {
        let assets = Folder::new("budget_twins");
        let mut app = game(&assets);
        camera(&mut app, Vec3::ZERO);
        light(&mut app, "steady", "Lamp.Z", Vec3::new(2.0, 0.0, 0.0));
        light(&mut app, "steady", "Lamp.Y", Vec3::new(-2.0, 0.0, 0.0));
        app.update();
        (assets, app)
    };
    twins.world_mut().resource_mut::<GraphicsSettings>().shadow_lights = 1;
    assert_eq!(casting(&mut twins), ["Lamp.Y"], "the key decides a tie");
}

#[test]
fn a_light_without_a_definition_is_named_and_draws_nothing() {
    let (_assets, mut app) = room("ghost");
    light(&mut app, "ghost", "Lamp.X", Vec3::new(9.0, 2.0, 0.0));
    let said = ok(&mut app, "lights", json!({}));
    assert!(said.starts_with("lights: 5 (point 3, spot 1, without a definition 1)"), "{said}");
    assert!(line(&said, "Lamp.X").contains("no `light` definition `ghost`"), "{said}");
    let wrong = refused(&mut app, "lights", json!({ "id": "stead" }));
    assert!(wrong.contains("no light of definition `stead` (did you mean `steady`?)"), "{wrong}");
}

#[test]
fn the_status_counts_the_lights_and_nothing_is_drawn_headless() {
    let (_assets, mut app) = room("status");
    app.update();
    let state = status(&app);
    assert_eq!((state["lights"].as_u64(), state["casting"].as_u64(), state["flickering"].as_u64(), state["budget"].as_u64()), (Some(4), Some(2), Some(2), Some(4)), "{state}");
    // The module is listed with the others.
    let modules = shards::ai_core::status::status(app.world())["engine"]["modules"].to_string();
    assert!(modules.contains("\"light\""), "{modules}");
    // No picture: no Bevy light anywhere.
    let lights = app.world_mut().query::<&shards::bevy::light::PointLight>().iter(app.world()).count();
    assert_eq!(lights, 0, "a headless game draws no light");
}

#[test]
fn a_game_without_lights_costs_nothing_and_says_so() {
    let assets = Folder::new("none");
    let mut app = game(&assets);
    let said = ok(&mut app, "lights", json!({}));
    assert!(said.starts_with("lights: 0 (point 0, spot 0)"), "{said}");
    assert!(said.contains("viewer: none"), "{said}");
}

#[test]
fn the_players_flash_setting_calms_a_flicker() {
    use shards::settings::AccessibilitySettings;
    let (_assets, mut app) = room("flashes");
    // Three quarters of a second: at two steps a second the blinking light is on its dark letter.
    step(&mut app, 45);
    let said = ok(&mut app, "lights", json!({ "id": "blink" }));
    assert!(said.contains("step 1 = a: 0 % (0 lm)") && !said.contains("accessibility.flashes"), "{said}");
    // Reduced: the same step, but the dark letter is half light, and the answer says why.
    app.world_mut().resource_mut::<AccessibilitySettings>().flashes = "reduced".into();
    let said = ok(&mut app, "lights", json!({ "id": "blink" }));
    assert!(said.contains("accessibility.flashes = reduced: a flicker steps or changes at most 2 times a second"), "{said}");
    assert!(line(&said, "Lamp.B").contains("step 1 = a: 50 % (1000 lm)"), "{said}");
    // Off: nothing flickers, and the light stays as written; the query still counts what is authored to flicker.
    app.world_mut().resource_mut::<AccessibilitySettings>().flashes = "off".into();
    let said = ok(&mut app, "lights", json!({}));
    assert!(said.contains("accessibility.flashes = off: no light flickers") && said.contains("· 2 flicker ·"), "{said}");
    assert!(line(&said, "Lamp.B").contains("2000 lm of 2000 · steady (accessibility.flashes = off)"), "{said}");
    assert!(line(&said, "Lamp.C").contains("3000 lm of 3000 · steady (accessibility.flashes = off)"), "{said}");
    // The default is as designed.
    app.world_mut().resource_mut::<AccessibilitySettings>().flashes = "full".into();
    assert!(!ok(&mut app, "lights", json!({})).contains("accessibility.flashes"));
}
