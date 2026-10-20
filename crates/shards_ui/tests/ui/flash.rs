//! Full-screen flashes (U2): one node over the HUD and under menus, its alpha frame by frame as
//! the player's flashes setting allows, and the tool and the trigger that start one.

use std::time::Duration;

use serde_json::json;
use shards::bevy::time::TimeUpdateStrategy;
use shards::bevy::window::Window;
use shards::prelude::*;
use shards::settings::{AccessibilitySettings, FlashLevel};
use shards::ui::flash::FlashOverlay;
use shards::ui::{FlashDef, PlayFlash, ScreenFlash};

use crate::common::*;

/// One frame of real time (s).
const FRAME: f32 = 0.05;

/// A game with a window entity (drawing, without a GPU) whose real clock moves one [`FRAME`] a
/// frame, at flashes level `level`.
fn windowed(level: &str) -> App {
    let mut app = game();
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::from_secs_f32(FRAME)));
    app.world_mut().spawn(Window::default());
    app.world_mut().resource_mut::<AccessibilitySettings>().flashes = level.into();
    app.update();
    app
}

/// How many flash nodes there are, and the first one's alpha.
fn overlay(app: &mut App) -> (usize, Option<f32>) {
    let mut q = app.world_mut().query_filtered::<(&BackgroundColor, &GlobalZIndex), With<FlashOverlay>>();
    let nodes: Vec<(f32, i32)> = q.iter(app.world()).map(|(b, z)| (b.0.alpha(), z.0)).collect();
    assert!(nodes.iter().all(|(_, z)| *z == 5), "over the HUD (1), under menus (10 and up): {nodes:?}");
    (nodes.len(), nodes.first().map(|(a, _)| *a))
}

fn def(app: &App, id: &str) -> FlashDef {
    app.world().resource::<shards::content::ContentDb>().peek::<FlashDef>(id).cloned().unwrap()
}

/// What `flash_play` answers, then the alpha drawn on each frame until the node is gone (at
/// most `frames`).
fn play_flash(app: &mut App, id: &str, frames: usize) -> (String, Vec<f32>) {
    let out = dev(app, "flash_play", json!({"flash": id}));
    assert!(out.ok, "{}", out.result);
    let mut alphas = Vec::new();
    for _ in 0..frames {
        app.update();
        match overlay(app) {
            (1, Some(a)) => alphas.push(a),
            (0, None) => break,
            other => panic!("one flash node at most: {other:?}"),
        }
    }
    (text(&out), alphas)
}

#[test]
fn a_flash_covers_the_picture_frame_by_frame_as_written_then_goes() {
    let mut app = windowed("full");
    let (line, alphas) = play_flash(&mut app, "reality", 40);
    assert_eq!(line, "playing reality: #ffffff at 0.9 for 0.6 s (in 0.1, hold 0.2, out 0.3)");
    let shape = def(&app, "reality").shaped(FlashLevel::Full).unwrap();
    // The node of frame k shows the flash k frames after the call.
    for (k, got) in alphas.iter().enumerate() {
        let want = shape.alpha_at((k + 1) as f32 * FRAME);
        assert!((got - want).abs() < 1e-3, "frame {}: {got} drawn, {want} written", k + 1);
    }
    assert!((10..=12).contains(&alphas.len()), "0.6 s at 0.05 s a frame: {alphas:?}");
    assert!(alphas.iter().any(|a| (a - 0.9).abs() < 1e-3), "it holds at its strength: {alphas:?}");
    assert_eq!(app.world().resource::<ScreenFlash>().0, None, "ended");
    assert_eq!(journal(&app, "flash: reality").len(), 1, "a local journal line");
}

#[test]
fn reduced_is_softer_and_slower_and_off_draws_nothing() {
    let mut app = windowed("reduced");
    let (line, alphas) = play_flash(&mut app, "crack", 80);
    assert!(line.ends_with(" · flashes reduced: at 0.35 for 2.85 s, pulses at 2/s"), "{line}");
    assert!(alphas.iter().all(|a| *a <= 0.35 + 1e-4), "{alphas:?}");
    // Half as deep: a dip keeps half the strength, it never goes dark in the middle of the glitch.
    let held = &alphas[4..40];
    assert!(held.iter().all(|a| *a >= 0.35 * 0.5 - 1e-3), "{held:?}");

    let mut app = windowed("off");
    let (line, alphas) = play_flash(&mut app, "crack", 10);
    assert!(line.ends_with(" · flashes off: not drawn"), "{line}");
    assert!(alphas.is_empty(), "no node: {alphas:?}");
    assert_eq!(app.world().resource::<ScreenFlash>().0, None);
    assert_eq!(journal(&app, "flash: crack").len(), 1, "started and journaled all the same");
}

#[test]
fn a_newer_flash_replaces_the_one_playing() {
    let mut app = windowed("full");
    assert!(dev(&mut app, "flash_play", json!({"flash": "crack"})).ok);
    app.update();
    app.update();
    // The trigger a game's observer uses starts it as the tool does.
    app.world_mut().trigger(PlayFlash("reality".into()));
    app.update();
    let flash = app.world().resource::<ScreenFlash>().0.clone().expect("playing");
    assert_eq!(flash.id, "reality");
    let (count, alpha) = overlay(&mut app);
    assert_eq!(count, 1);
    let age = flash.age(app.world().resource::<Time<Real>>().elapsed());
    assert!(age <= FRAME + 1e-4, "from its own start: {age} s old");
    let first = def(&app, "reality").shaped(FlashLevel::Full).unwrap().alpha_at(age);
    assert!((alpha.unwrap() - first).abs() < 1e-3, "{alpha:?} against {first}");

    // A trigger with an unknown id warns in the journal and leaves the flash as it is.
    app.world_mut().trigger(PlayFlash("realty".into()));
    app.update();
    assert_eq!(journal(&app, "flash: no flash `realty` (did you mean `reality`?)").len(), 1);
    assert_eq!(app.world().resource::<ScreenFlash>().0.as_ref().map(|f| f.id.as_str()), Some("reality"));
}

#[test]
fn the_tool_names_what_is_wrong_and_headless_draws_nothing() {
    let mut app = game();
    let out = dev(&mut app, "flash_play", json!({"flash": "realty"}));
    assert!(!out.ok);
    assert_eq!(text(&out), "no flash `realty` (did you mean `reality`?): content `*.flash.ron` defines flashes; 2 known");
    assert!(!play(&mut app, "flash_play", json!({"flash": "reality"})).ok, "a dev tool: the player's scope cannot");
    let out = dev(&mut app, "flash_play", json!({"flash": "reality"}));
    assert!(text(&out).ends_with(" · nothing is drawn headless: flashes show in a window or with --offscreen"), "{}", text(&out));
    assert_eq!(app.world().resource::<ScreenFlash>().0.as_ref().map(|f| f.id.as_str()), Some("reality"), "timed all the same");
}

#[test]
fn the_content_check_refuses_a_flash_that_cannot_play() {
    use shards::content::db::Source;
    use shards::content::{ContentDb, ContentKinds};
    let mut kinds = ContentKinds::default();
    kinds.register::<FlashDef>();
    let text = r##"{
        "a": (color: "#ffffff80", alpha: 1.5),
        "b": (fade_in_s: 0.0, hold_s: 0.0, fade_out_s: 0.0),
        "c": (pulses: 3, pulse_rate: 0.0, pulse_depth: 2.0),
        "d": (hold_s: 40.0),
    }"##;
    let db = ContentDb::build(&kinds, &[Source { path: "content/f.flash.ron".into(), text: text.into() }]);
    let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
    let has = |needle: &str| assert!(errors.iter().any(|e| e.contains(needle)), "no `{needle}` in {errors:#?}");
    has("color `#ffffff80`: give it as `#rrggbb`; how strong the flash is, is `alpha`");
    has("alpha: 0 to 1, not 1.5");
    has("fade_in_s + hold_s + fade_out_s is 0 s");
    has("pulse_rate: pulses a second, above 0, not 0");
    has("pulse_depth: 0 to 1, not 2");
    has("is 40.55 s");
    assert_eq!(errors.len(), 6, "{errors:#?}");
}
