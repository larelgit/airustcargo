//! Player settings (E8.6) on the sandbox: where it keeps them, and which old file it imports.

use shards::motion::InputSettings;
use shards::serde_json::json;
use shards::settings::SettingsAppId;

use crate::common::*;

#[test]
fn the_sandbox_asks_for_its_old_controls_file_and_keeps_settings_under_its_own_name() {
    let app = sandbox::test_app(1);
    let legacy = app.world().resource::<InputSettings>();
    // `~/.config/shards/input.ron` was the sandbox's; a game that does not ask never reads it.
    assert!(legacy.import && legacy.app_name == "shards", "{legacy:?}");
    assert_eq!(app.world().resource::<SettingsAppId>().0, "shards-sandbox");
}

#[test]
fn a_test_run_keeps_no_settings_so_it_reads_no_shared_file() {
    let mut app = sandbox::test_app(1);
    let out = ok(&mut app, "settings", json!({}));
    assert!(text(&out).contains("not saved: this run keeps no settings file"), "{}", text(&out));
}
