//! Where the scatters go, on the headless engine counted as drawn, and what changes them.

use std::collections::BTreeSet;

use serde_json::json;
use shards::ai_core::AiJournal;
use shards::bevy::camera::visibility::VisibilityRange;
use shards::bevy::light::NotShadowCaster;
use shards::look::Looked;
use shards::scatter::{ScatterRoot, ScatterTile};
use shards::world::RegionTile;

use crate::*;

const FIELD: (i32, i32) = (0, 0);
const COLUMN: (i32, i32) = (1, 0);
const MOUND: (i32, i32) = (0, 1);
const WALL: (i32, i32) = (1, 1);

#[test]
fn headless_nothing_is_strewn_and_the_tools_say_why() {
    let assets = Folder::new("headless");
    let mut app = game(&assets);
    app.update();
    assert!(seen(&mut app).is_empty());
    let said = ok(&mut app, "scatter", json!({}));
    assert!(said.starts_with("3 scatters · nothing is strewn headless: scatter is drawn in a window or with --offscreen\n"), "{said}");
    assert!(said.lines().any(|l| l == "  tufts   1/m² · spacing 0.3 m · 2 LODs to 40 m · looks green, olive"), "{said}");
    assert!(said.contains("  stones  0.5/m² · 1 LOD to 50 m · looks grey · shadows · on #stone, mound"), "{said}");
    assert_eq!(status(&app), json!({"scatters": 3, "instances": 0, "entities": 0, "tiles": 0, "on": true}));
    let off = ok(&mut app, "scatter_set", json!({"on": false}));
    assert_eq!(off, "scatter off: nothing is strewn headless: scatter is drawn in a window or with --offscreen");
    assert_eq!(status(&app)["on"], json!(false));
    let tiles = app.world_mut().query_filtered::<(), With<ScatterTile>>().iter(app.world()).count();
    assert_eq!(tiles, 0, "headless, no tile is even looked at");
}

#[test]
fn every_tile_gets_its_scatters_where_the_ground_allows_the_same_every_run() {
    let assets = Folder::new("strewn");
    let mut app = drawn(&assets);
    let all = seen(&mut app);

    // Tufts on every tile, about 256 a tile (spacing and solids take a few), two levels each.
    for cell in [FIELD, COLUMN, MOUND, WALL] {
        let n = instances(&all, "tufts", cell).len();
        assert!((200..=256).contains(&n), "tufts on {cell:?}: {n}");
    }
    let tufts = all.iter().filter(|s| s.scatter == "tufts").count();
    assert_eq!(tufts % 2, 0, "each tuft has both levels");
    // None on the column or in it; none inside the wall.
    for p in instances(&all, "tufts", COLUMN) {
        assert!(p.y.abs() < 0.01, "on the ground, not the column's top: {p}");
        assert!(p.x.abs() > 1.0 || p.z.abs() > 1.0, "not inside the column: {p}");
    }
    for p in instances(&all, "tufts", WALL) {
        assert!(p.x.abs() > 6.0 || p.z.abs() > 0.3, "not inside the wall: {p}");
    }
    // The mound's side is 26.6° steep: tufts (up to 30°) and moss (40°, up to 1.5 m) climb it,
    // stones (20°) keep to the flat ground, and only stones keep 0.5 m from the wall.
    let up = |cell, scatter| instances(&all, scatter, cell).into_iter().filter(|p: &Vec3| p.y > 0.2).count();
    assert!(up(MOUND, "tufts") > 0 && up(MOUND, "moss") > 0, "tufts and moss on the mound's side");
    assert_eq!(up(MOUND, "stones"), 0, "stones only on flat ground");
    assert!(instances(&all, "moss", MOUND).len() > 400 && instances(&all, "moss", FIELD).is_empty(), "moss on the mound only");
    assert!(instances(&all, "stones", FIELD).is_empty() && !instances(&all, "stones", COLUMN).is_empty(), "stones by tag");
    for p in instances(&all, "stones", WALL) {
        assert!(clear_of(p, 6.0, 0.3) > 0.49, "half a meter clear of the wall: {p}");
    }
    for p in instances(&all, "stones", COLUMN) {
        assert!(clear_of(p, 1.0, 1.0) > 0.49, "half a meter clear of the column: {p}");
    }

    // The same seed gives the same scatter, to the last bit.
    let mut again = drawn(&assets);
    let transforms = |seen: &[Seen]| seen.iter().map(|s| (s.cell, s.scatter.clone(), s.lod, s.transform)).collect::<Vec<_>>();
    assert_eq!(transforms(&seen(&mut again)), transforms(&all));
    let other = &mut build(&assets, 8, true);
    assert_ne!(instances(&seen(other), "tufts", FIELD), instances(&all, "tufts", FIELD), "another seed, other places");

    let said = ok(&mut app, "scatter", json!({}));
    let header = said.lines().next().unwrap();
    let total = status(&app)["instances"].as_u64().unwrap();
    assert_eq!(header, format!("3 scatters · {total} instances ({} mesh entities) on 4 tiles · foliage 1 · scatter on", all.len()));
    let stones = said.lines().find(|l| l.starts_with("  stones")).unwrap();
    assert!(stones.contains(" on 3 tiles · ") && stones.contains("found no place (") && stones.contains("too steep"), "{stones}");
    assert_eq!(status(&app)["entities"], json!(all.len()));
}

#[test]
fn instances_are_drawn_together_near_and_alone_far() {
    let assets = Folder::new("batches");
    let mut app = drawn(&assets);
    let all = seen(&mut app);
    let world = app.world();
    let range = |s: &Seen| world.get::<VisibilityRange>(s.entity).unwrap().clone();
    let tuft = all.iter().find(|s| s.scatter == "tufts" && s.lod == 0).unwrap();
    assert_eq!((range(tuft).start_margin, range(tuft).end_margin), (0.0..0.0, 18.0..20.0));
    let far = all.iter().find(|s| s.scatter == "tufts" && s.lod == 1).unwrap();
    assert_eq!((range(far).start_margin, range(far).end_margin), (18.0..20.0, 38.0..40.0), "fades in as the near one fades out");
    let stone = all.iter().find(|s| s.scatter == "stones").unwrap();
    assert_eq!((range(stone).start_margin, range(stone).end_margin), (0.0..0.0, 50.0..50.0), "no fade: at once");
    assert!(world.get::<NotShadowCaster>(tuft.entity).is_some() && world.get::<NotShadowCaster>(stone.entity).is_none(), "stones cast shadows");

    // One mesh per level, one material per look: every tuft of a look and level is one batch.
    let batch = |s: &Seen| {
        let looked = world.get::<Looked>(s.entity).unwrap_or_else(|| panic!("{} is drawn with its look", s.entity));
        let mesh = world.get::<Mesh3d>(s.entity).unwrap().0.id();
        let material = world
            .get::<MeshMaterial3d<StandardMaterial>>(s.entity)
            .map(|m| m.0.id().untyped())
            .or_else(|| world.get::<MeshMaterial3d<shards::look::ToonMaterial>>(s.entity).map(|m| m.0.id().untyped()))
            .unwrap();
        (s.scatter.clone(), looked.look.clone(), s.lod, mesh, material)
    };
    let batches: BTreeSet<String> = all.iter().map(|s| format!("{:?}", batch(s))).collect();
    let kinds: BTreeSet<(String, String, u8)> = all.iter().map(|s| (s.scatter.clone(), s.look.clone(), s.lod)).collect();
    assert_eq!(batches.len(), kinds.len(), "one (mesh, material) per scatter, look and level: {batches:#?}");
    assert_eq!(kinds.len(), 6, "tufts: 2 looks × 2 levels; stones; moss");
    let roots = app.world_mut().query::<&ScatterRoot>().iter(app.world()).count();
    assert_eq!(roots, 4 * 2 + 3 + 1, "a root per tile, scatter and look");
}

#[test]
fn foliage_keeps_the_start_of_the_same_scatter_and_zero_keeps_none() {
    let assets = Folder::new("foliage");
    let mut app = drawn(&assets);
    let full = instances(&seen(&mut app), "tufts", FIELD);
    ok(&mut app, "settings_set", json!({"group": "graphics", "key": "foliage", "value": 0.5}));
    app.update();
    let half = instances(&seen(&mut app), "tufts", FIELD);
    assert!(half.len() * 2 >= full.len() - 2 && half.len() * 2 <= full.len() + 2, "about half: {} of {}", half.len(), full.len());
    assert!(half.iter().all(|p| full.contains(p)), "the same tufts, fewer");
    assert!(ok(&mut app, "scatter", json!({})).lines().next().unwrap().contains("· foliage 0.5 ·"));
    ok(&mut app, "settings_set", json!({"group": "graphics", "key": "foliage", "value": 0}));
    app.update();
    assert!(seen(&mut app).is_empty());
    assert_eq!(status(&app)["instances"], json!(0));
    let refused = testing::call(&mut app, "settings_set", json!({"group": "graphics", "key": "foliage", "value": 2}));
    assert!(!refused.ok && text(&refused).contains("graphics.foliage 2 is out of range"), "{}", text(&refused));
}

#[test]
fn scatter_set_takes_everything_away_and_strews_it_again() {
    let assets = Folder::new("toggle");
    let mut app = drawn(&assets);
    let before = seen(&mut app);
    let total = status(&app)["instances"].as_u64().unwrap();
    assert_eq!(ok(&mut app, "scatter_set", json!({"on": false})), "scatter off: 0 instances on 0 tiles");
    assert!(seen(&mut app).is_empty());
    assert_eq!(status(&app)["on"], json!(false));
    app.update();
    assert!(seen(&mut app).is_empty(), "off stays off on new frames");
    assert_eq!(ok(&mut app, "scatter_set", json!({})), format!("scatter on: {total} instances on 4 tiles"));
    let after = seen(&mut app);
    let transforms = |seen: &[Seen]| seen.iter().map(|s| (s.cell, s.scatter.clone(), s.lod, s.transform)).collect::<Vec<_>>();
    assert_eq!(transforms(&after), transforms(&before));
    let menu = ok(&mut app, "dev_menu", json!({}));
    assert!(menu.contains("scatter_set on=false"), "the menu's toggle offers the other state: {menu}");
}

#[test]
fn a_content_change_strews_again_and_other_edits_do_not() {
    let assets = Folder::new("reload");
    let mut app = drawn(&assets);
    let first: Vec<Entity> = seen(&mut app).iter().map(|s| s.entity).collect();
    assets.put("content/look/test.look.ron", &LOOKS.replace("#808080", "#909090"));
    ok(&mut app, "content_reload", json!({}));
    app.update();
    let kept: Vec<Entity> = seen(&mut app).iter().map(|s| s.entity).collect();
    assert_eq!(kept, first, "a look's colour changes the material, not the scatter");

    assets.put("content/world/test.scatter.ron", &SCATTERS.replace("density: 2.0", "density: 1.0"));
    ok(&mut app, "content_reload", json!({}));
    app.update();
    let moss = instances(&seen(&mut app), "moss", MOUND).len();
    assert!((200..=256).contains(&moss), "half the moss: {moss}");
    assert!(!seen(&mut app).iter().any(|s| first.contains(&s.entity)), "strewn anew");
}

#[test]
fn a_region_change_takes_the_old_tiles_scatter_with_them() {
    let assets = Folder::new("regions");
    let mut app = drawn(&assets);
    let before = instances(&seen(&mut app), "tufts", FIELD);
    ok(&mut app, "regen", json!({"region": "plain", "seed": 9}));
    app.update();
    let all = seen(&mut app);
    assert_eq!(status(&app)["tiles"], json!(4), "the new region's four tiles, not eight");
    assert_ne!(instances(&all, "tufts", FIELD), before, "another seed, another scatter");
    let tiles: Vec<Entity> = app.world_mut().query_filtered::<Entity, With<RegionTile>>().iter(app.world()).collect();
    for s in &all {
        let root = app.world().get::<ChildOf>(s.entity).unwrap().parent();
        let tile = app.world().get::<ChildOf>(root).unwrap().parent();
        assert!(tiles.contains(&tile), "under a tile of the region");
    }
    let journal = app.world().resource::<AiJournal>();
    let warned: Vec<String> = journal.after(0).0.map(|e| e.text.clone()).filter(|t| t.contains("B0004")).collect();
    assert!(warned.is_empty(), "nothing to warn about: {warned:#?}");
}

/// How far `p` is, across the ground, from a solid `half_x` by `half_z` around the tile's middle.
fn clear_of(p: Vec3, half_x: f32, half_z: f32) -> f32 {
    Vec2::new((p.x.abs() - half_x).max(0.0), (p.z.abs() - half_z).max(0.0)).length()
}
