//! The tools, the status, the developer menu and `inspect`: what an agent reads of the looks
//! headless, where nothing is drawn.

use serde_json::json;
use shards::look::{Look, Looked};

use crate::*;

#[test]
fn looks_lists_every_look_and_headless_nothing_is_drawn_or_styled() {
    let assets = Folder::new("list");
    let mut app = game(&assets);
    let said = ok(&mut app, "looks", json!({}));
    assert_eq!(
        said,
        "5 looks · nothing is drawn headless: looks style meshes in a window or with --offscreen\n\
         \x20 Moss   matte\n\
         \x20 glass  unlit · glow #7fe0ff\n\
         \x20 hero   #408cff · toon 3 bands · flat · outline 2px #000000\n\
         \x20 metal  lit roughness 0.4 metallic 1\n\
         \x20 stone  #6b6f78 · toon 3 bands · outline 1.5px #16171c · materials Rock, Rock.001"
    );
    assert_eq!(status(&app), json!({"looks": 5, "styled": 0, "outlined": 0, "outlines": true}));

    // A mesh with a look stays as the game made it: the look systems wait for a picture.
    let world = app.world_mut();
    let red = world.resource_mut::<Assets<StandardMaterial>>().add(Color::srgb(1.0, 0.0, 0.0));
    let sphere = world.resource_mut::<Assets<Mesh>>().add(Sphere::new(1.0));
    let body = world.spawn((Mesh3d(sphere.clone()), MeshMaterial3d(red.clone()), Look::new("hero"))).id();
    for _ in 0..3 {
        app.update();
    }
    let world = app.world();
    assert!(world.get::<Looked>(body).is_none() && world.get::<Children>(body).is_none(), "no look, no outline");
    assert_eq!(world.get::<MeshMaterial3d<StandardMaterial>>(body).map(|m| &m.0), Some(&red));
    assert_eq!(world.get::<Mesh3d>(body).map(|m| &m.0), Some(&sphere));
    assert_eq!(status(&app), json!({"looks": 5, "styled": 0, "outlined": 0, "outlines": true}));
    assert!(ok(&mut app, "looks", json!({})).starts_with("5 looks · nothing is drawn headless"));

    let none = Folder::empty("none");
    let mut bare = game(&none);
    assert_eq!(
        ok(&mut bare, "looks", json!({})),
        "0 looks · nothing is drawn headless: looks style meshes in a window or with --offscreen\n\
         no look defined: add one in content/**/*.look.ron"
    );
}

#[test]
fn look_outlines_switches_the_outlines_and_the_status_says_so() {
    let assets = Folder::new("switch");
    let mut app = game(&assets);
    assert_eq!(ok(&mut app, "look_outlines", json!({"on": false})), "outlines off: 0 drawn with an outline");
    assert!(!app.world().resource::<shards::look::LookSettings>().outlines);
    assert_eq!(status(&app)["outlines"], false);
    assert_eq!(ok(&mut app, "look_outlines", json!({})), "outlines on: 0 drawn with an outline", "on unless it says");
    assert_eq!(status(&app)["outlines"], true);
    let modules = &shards::ai_core::status::status(app.world())["engine"]["modules"];
    assert!(modules.as_array().unwrap().iter().any(|m| m["name"] == "look"), "{modules}");
}

#[test]
fn the_developer_menu_has_both_under_world() {
    let assets = Folder::new("menu");
    let mut app = game(&assets);
    let list = ok(&mut app, "dev_menu", json!({}));
    let lines: Vec<&str> = list.lines().collect();
    let world = lines.iter().position(|l| *l == "world").unwrap_or_else(|| panic!("no section `world`:\n{list}"));
    let section: Vec<&str> = lines[world + 1..].iter().take_while(|l| l.starts_with("  ")).map(|l| l.trim_start()).collect();
    let entry = |id: &str| {
        section
            .iter()
            .find(|l| l.split_whitespace().next() == Some(id))
            .unwrap_or_else(|| panic!("no entry `{id}` under world:\n{list}"))
            .to_string()
    };
    let outlines = entry("look_outlines");
    assert!(outlines.contains("Outlines of the looks") && outlines.ends_with("look_outlines on=false"), "{outlines}");
    let looks = entry("looks");
    assert!(looks.contains("Looks and what they draw") && looks.ends_with("looks"), "{looks}");
    // The switch runs the line of its other state, and shows the next one.
    assert_eq!(ok(&mut app, "dev_menu", json!({"run": "look_outlines"})), "outlines off: 0 drawn with an outline");
    let list = ok(&mut app, "dev_menu", json!({}));
    assert!(list.lines().any(|l| l.trim_start().starts_with("look_outlines") && l.ends_with("look_outlines on=true")), "{list}");
    assert!(ok(&mut app, "dev_menu", json!({"run": "looks"})).starts_with("5 looks · "));
}

#[test]
fn inspect_shows_the_look_a_prefab_gives() {
    let assets = Folder::new("inspect");
    let mut app = game(&assets);
    assert_eq!(ok(&mut app, "spawn_prefab", json!({"id": "pillar", "x": 2, "z": 0})), "spawned e1 pillar at (2.0, 0.0)");
    let seen = ok(&mut app, "inspect", json!({"id": "e1"}));
    assert!(seen.contains("  Look: stone\n"), "{seen}");
}
