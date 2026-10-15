//! The point of the template: a game depends on the engine through `shards` alone. If this
//! fails, something else crept into Cargo.toml (Bevy, serde, a physics crate...), and the
//! game now breaks when the engine changes what is behind `shards`.

use shards::serde_json::Value;

/// `cargo metadata` of this package, without resolving anything from the network.
fn metadata() -> Value {
    let manifest = concat!(env!("CARGO_MANIFEST_DIR"), "/Cargo.toml");
    let output = std::process::Command::new(env!("CARGO"))
        .args(["metadata", "--no-deps", "--format-version", "1", "--offline", "--manifest-path", manifest])
        .output()
        .expect("cargo runs");
    assert!(output.status.success(), "cargo metadata failed: {}", String::from_utf8_lossy(&output.stderr));
    shards::serde_json::from_slice(&output.stdout).expect("cargo metadata prints JSON")
}

#[test]
fn the_game_depends_on_shards_and_nothing_else() {
    let metadata = metadata();
    let packages = metadata["packages"].as_array().expect("packages");
    let me = packages.iter().find(|p| p["name"] == "my_game").expect("this package");
    let mut dependencies: Vec<String> = me["dependencies"]
        .as_array()
        .expect("dependencies")
        .iter()
        .map(|d| format!("{} ({})", d["name"].as_str().unwrap(), d["kind"].as_str().unwrap_or("normal")))
        .collect();
    dependencies.sort();
    assert_eq!(dependencies, ["shards (normal)"], "the game's only dependency is the engine");
    let workspace_members = metadata["workspace_members"].as_array().expect("workspace_members");
    assert_eq!(workspace_members.len(), 1, "the template is a workspace of its own: {workspace_members:?}");
}

#[test]
fn the_settings_folder_is_named_after_the_package_the_way_a_rename_finds_it() {
    // A game made from the template is renamed with a search for `my_game` (template/README.md, README.md). A folder
    // name spelled another way (`my-game`) is not found, and every game made that way then keeps its settings in one
    // folder and overwrites the others'. The slug turns `_` into `-`, so the name in the code is the package's own.
    let source = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/lib.rs")).unwrap();
    let named = format!("SettingsAppId::new(\"{}\")", env!("CARGO_PKG_NAME"));
    assert!(source.contains(&named), "src/lib.rs must contain `{named}`: rename it together with the package");
    let app = my_game::test_app(1);
    let id = app.world().resource::<shards::settings::SettingsAppId>();
    assert_eq!(*id, shards::settings::SettingsAppId::new(env!("CARGO_PKG_NAME")));
}

#[test]
fn the_game_does_not_point_cargo_at_another_target_dir() {
    let config = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/.cargo/config.toml")).unwrap();
    assert!(!config.lines().any(|l| l.trim_start().starts_with("target-dir")), "a game's target/ lives next to its Cargo.toml");
}
