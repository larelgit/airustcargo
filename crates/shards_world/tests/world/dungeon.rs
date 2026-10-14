//! Dungeons: the entrance always reaches the goal, the hand-made room is on the way, rooms
//! never overlap, doors join neighbors, and the same seed lays the same crypt.

use std::collections::BTreeSet;

use serde_json::json;
use shards::world::checks::RegionGraph;
use shards::world::generate::generate;
use shards::world::manifest::Layout;

use crate::common::*;

#[test]
fn crypts_lead_from_the_entry_through_the_vault_to_the_throne() {
    let app = app(1, None);
    let (db, worldgen) = (db(&app), worldgen(&app));
    let mut sizes = BTreeSet::new();
    for seed in 1..=60 {
        let made = generate(db, &worldgen, "crypt", seed).unwrap_or_else(|e| panic!("seed {seed}: {e}"));
        let m = &made.manifest;
        assert_eq!(m.layout, Layout::Rooms);
        let count = |room: &str| m.tiles.iter().filter(|t| t.tile == room).count();
        assert_eq!((count("entry"), count("throne"), count("vault")), (1, 1, 1), "seed {seed}");
        // Main path 3..=6 rooms between entry and throne, then 1..=3 branches.
        assert!((6..=11).contains(&m.tiles.len()), "seed {seed}: {} rooms", m.tiles.len());
        sizes.insert(m.tiles.len());
        // No two rooms share a cell; all inside the size the manifest gives.
        let mut cells = BTreeSet::new();
        for t in &m.tiles {
            for dy in 0..t.size[1] as i32 {
                for dx in 0..t.size[0] as i32 {
                    let c = [t.at[0] + dx, t.at[1] + dy];
                    assert!(m.contains(c), "seed {seed}: {c:?} outside {:?}", m.size);
                    assert!(cells.insert(c), "seed {seed}: two rooms in {c:?}");
                }
            }
        }
        assert!(m.size[0] <= 14 && m.size[1] <= 14, "seed {seed}: max_size holds");
        // Every door joins two neighboring cells of two different rooms.
        for link in &m.links {
            assert_eq!(link.kind, "door");
            let [a, b] = [link.cells[0], link.cells[1]];
            assert_eq!((a[0] - b[0]).abs() + (a[1] - b[1]).abs(), 1, "seed {seed}: {link:?}");
            assert_ne!(link.a, link.b);
        }
        assert_eq!(m.links.len(), m.tiles.len() - 1, "rooms form a tree");
        let graph = RegionGraph::build(db, m).unwrap();
        let spawn = m.pois.iter().find(|p| p.poi == "spawn").expect("the entry's spawn");
        let boss = m.pois.iter().find(|p| p.poi == "boss").expect("the throne's boss");
        assert!(graph.connected(spawn, boss), "seed {seed}: the entry reaches the throne");
        assert!(spawn.position().distance(boss.position()) >= 16.0, "the distance check held");
        let again = generate(db, &worldgen, "crypt", seed).unwrap().manifest;
        assert_eq!(again.to_canonical(), m.to_canonical(), "seed {seed}: the same crypt again");
    }
    assert!(sizes.len() > 2, "crypts of several sizes: {sizes:?}");
}

#[test]
fn a_crypt_loads_like_any_region() {
    let mut app = app(4, Some("crypt"));
    let r = region(&app);
    assert_eq!(r.manifest.layout, Layout::Rooms);
    let spawn = r.spawn_point(0);
    let hero = hero(&mut app);
    assert!(pos(&app, hero).distance(spawn) < 0.01, "the hero starts in the entry");
    ok(&mut app, "step", json!({"ticks": 30}));
    let seen = ok(&mut app, "observe", json!({}));
    assert!(seen.contains("TILE (") && seen.contains(" entry rot 0 · crypt seed 4 v1"), "{seed}", seed = seen);
    assert!(pos(&app, hero).y > -0.5, "standing on the entry's floor: {}", pos(&app, hero));
}
