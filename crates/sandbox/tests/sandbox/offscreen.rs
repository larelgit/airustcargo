//! Pictures without a window (`--offscreen`): the GPU draws the sandbox into an image, the HUD and
//! the `eN` marks included, and `shot` saves it.

use std::time::{Duration, Instant};

use shards::ai_core::AiJournal;
use shards::serde_json::json;

use crate::common::{ok, step};

#[test]
#[ignore = "needs a GPU, or a software one (WGPU_FORCE_FALLBACK_ADAPTER=1; Mesa's lavapipe on Linux): cargo test -p sandbox --test sandbox -- --ignored offscreen"]
fn an_offscreen_game_takes_shots_without_a_window() {
    let mut app = shards::testing::test_app(
        |config| (config.headless, config.offscreen) = (false, true),
        |app| {
            app.add_plugins((sandbox::SandboxPlugin, sandbox::visuals::VisualsPlugin));
        },
    );
    let display = shards::ai_core::status::status(app.world())["display"].clone();
    assert_eq!((display["mode"].as_str(), &display["size"]), (Some("offscreen"), &json!([1280, 720])), "{display}");
    assert!(display["gpu"].is_string(), "the GPU that draws is named: {display}");

    step(&mut app, 30);
    let ticket = ok(&mut app, "shot", json!({"path": format!("shots/offscreen-test-{}.png", std::process::id())})).result;
    let (png, legend) = (ticket["path"].as_str().unwrap().to_owned(), ticket["legend_path"].as_str().unwrap().to_owned());
    // The frame comes back a few frames later, once the GPU has compiled its pipelines.
    let started = Instant::now();
    let said = loop {
        app.update();
        let journal = app.world().resource::<AiJournal>();
        if let Some(line) = journal.after(0).0.map(|e| e.text.clone()).find(|t| t.starts_with("screenshot ")) {
            break line;
        }
        assert!(started.elapsed() < Duration::from_secs(120), "no screenshot within two minutes");
    };
    assert_eq!(said, format!("screenshot saved: {png} (1280x720)"));

    let bytes = std::fs::read(&png).unwrap();
    let size = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    assert!(bytes.starts_with(b"\x89PNG") && (size(16), size(20)) == (1280, 720), "a 1280x720 PNG");
    assert!(bytes.len() > 20_000, "a picture, not an empty image: {} bytes", bytes.len());
    let legend: shards::serde_json::Value = shards::serde_json::from_str(&std::fs::read_to_string(&legend).unwrap()).unwrap();
    let marks: Vec<&str> = legend["legend"].as_array().unwrap().iter().filter_map(|l| l.as_str()).collect();
    assert!(marks.iter().any(|l| l.starts_with("e5 Player  px(")), "{marks:?}");
    let _ = std::fs::remove_file(&png);
    let _ = std::fs::remove_file(ticket["legend_path"].as_str().unwrap());
}
