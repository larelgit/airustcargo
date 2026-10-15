//! Acceptance: save → load loses nothing, on a thousand random states.

use serde_json::json;
use shards::ai_core::{DetRng, JournalKind};
use shards::save::{capture_sections, load_slot, save_slot, SaveKind, SaveWhat};

use shards::prelude::*;

use crate::common::*;

/// The canonical text of what a save would hold now, by kind: equal text, equal bytes in a file.
fn held(app: &App) -> (String, String) {
    let world = app.world();
    let body = shards::save::default_body(world);
    (
        capture_sections(world, SaveKind::World, None).unwrap().canonical(),
        capture_sections(world, SaveKind::Character, body).unwrap().canonical(),
    )
}

fn install(app: &mut App, rng: &mut DetRng) {
    set_hero(app, random_hero(rng));
    set_chests(app, &random_chests(rng));
    *app.world_mut().resource_mut::<Calendar>() = random_calendar(rng);
}

#[test]
fn a_thousand_random_states_round_trip_through_files() {
    let dir = TempDir::new("thousand");
    let mut app = game(&dir);
    // Fifteen hundred files and folder flushes would be the whole run: the flush has its own tests.
    app.world_mut().resource_mut::<shards::save::SaveConfig>().sync = false;
    for i in 0..1000_u64 {
        install(&mut app, &mut DetRng::from_keys(&[i, 0x5eed]));
        let (hero_then, chests_then, calendar_then) = (hero(&mut app), chests(&mut app), calendar(&app));
        let saved = held(&app);

        save_slot(app.world_mut(), SaveWhat::All, "slot1", "", "test").unwrap();
        // A different state in the world, so that the load has something to undo.
        install(&mut app, &mut DetRng::from_keys(&[i, 0xa5a5]));
        let reports = load_slot(app.world_mut(), SaveWhat::All, "slot1").unwrap();
        assert_eq!(reports.len(), 2);

        assert_eq!(held(&app), saved, "state {i}: what a save holds");
        assert_eq!(hero(&mut app), hero_then, "state {i}: the hero");
        assert_eq!(chests(&mut app), chests_then, "state {i}: the chests");
        assert_eq!(calendar(&app), calendar_then, "state {i}: the calendar");
    }
    assert!(lines(&app, JournalKind::Warn, "damaged").is_empty(), "no recovery was needed");
}

#[test]
fn edge_values_survive_bit_for_bit() {
    let dir = TempDir::new("edges");
    let mut app = game(&dir);
    let mut stats = std::collections::BTreeMap::new();
    stats.insert("edges".to_owned(), vec![f32::MIN_POSITIVE, f32::MAX, f32::MIN, 1.0e-45, -0.0, 0.1]);
    let wanted = Hero {
        name: "ünï \u{0} 😀 \"quoted\" \\ \n".into(),
        xp: u64::MAX,
        delta: i64::MIN,
        pos: [0.1, -0.0, f32::MAX],
        precise: 0.1 + 0.2,
        stats,
        extra: Some(Extra { note: String::new(), weights: vec![f64::MAX, f64::MIN_POSITIVE, 5e-324, 1.0 / 3.0, -0.0] }),
        flag: Some(false),
        mode: Mode::Moving { speed: 3.0e-39 },
        ..Hero::default()
    };
    set_hero(&mut app, wanted.clone());
    ok(&mut app, "save_game", json!({"what": "character"}));
    set_hero(&mut app, Hero::default());
    ok(&mut app, "load_game", json!({"what": "character"}));
    let got = hero(&mut app);
    assert_eq!(got, wanted);
    // `==` cannot tell zeros apart: the bits can.
    assert_eq!(got.pos[1].to_bits(), (-0.0_f32).to_bits());
    assert_eq!(got.precise.to_bits(), (0.1_f64 + 0.2).to_bits());
    assert_eq!(got.stats["edges"].iter().map(|f| f.to_bits()).collect::<Vec<_>>(), wanted.stats["edges"].iter().map(|f| f.to_bits()).collect::<Vec<_>>());
    assert_eq!(got.extra.unwrap().weights[4].to_bits(), (-0.0_f64).to_bits());
}

#[test]
fn a_state_that_cannot_be_read_back_is_refused_and_nothing_is_written() {
    let dir = TempDir::new("nan");
    let mut app = game(&dir);
    set_hero(&mut app, Hero { pos: [f32::NAN, 0.0, 0.0], ..Hero::default() });
    let err = fails(&mut app, "save_game", json!({"what": "character"}));
    assert!(err.contains("section hero") && err.contains("does not read back") && err.contains("NaN"), "{err}");
    assert!(!dir.file("character/slot1.save").exists(), "no file");
    assert_eq!(lines(&app, JournalKind::Error, "character/slot1 not saved").len(), 1);
    // The same hero, repaired, saves.
    set_hero(&mut app, Hero::default());
    ok(&mut app, "save_game", json!({"what": "character"}));
}

#[test]
fn saving_changes_nothing_in_the_world() {
    let dir = TempDir::new("pure");
    let mut app = game(&dir);
    install(&mut app, &mut DetRng::from_keys(&[7, 7]));
    step(&mut app, 3);
    let before = shards::ai_core::replay::state_hash(app.world());
    let snapshot = shards::ai_core::snapshot::capture(app.world()).unwrap();
    ok(&mut app, "save_game", json!({"what": "all", "label": "x"}));
    shards::save::save_now(app.world_mut(), SaveWhat::All, "again").unwrap();
    assert_eq!(shards::ai_core::replay::state_hash(app.world()), before);
    assert_eq!(shards::ai_core::snapshot::capture(app.world()).unwrap().entities, snapshot.entities);
}

#[test]
fn a_character_captured_in_one_place_is_written_and_applied_in_another() {
    // Co-op: the host captures a remote player's body, the player's machine writes the file and
    // sends the sections back on a rejoin, the host applies them to a new body.
    let dir = TempDir::new("elsewhere");
    let mut app = game(&dir);
    let wanted = random_hero(&mut DetRng::from_keys(&[3, 1]));
    set_hero(&mut app, wanted.clone());
    let body = hero_entity(&mut app);
    let captured = capture_sections(app.world(), SaveKind::Character, Some(body)).unwrap();

    // Over a wire, as text.
    let received = shards::save::SectionSet::from_canonical(&captured.canonical()).unwrap();
    assert_eq!(received, captured, "the text reads back exactly");
    let report = shards::save::write_sections(app.world_mut(), SaveKind::Character, "remote", "left the game", "left", 77, received).unwrap();
    assert_eq!((report.tick, report.sections), (77, captured.len()));

    let file = shards::save::read_file(app.world_mut(), SaveKind::Character, "remote").unwrap();
    assert_eq!((file.tick, file.label.as_str()), (77, "left the game"));
    assert_eq!(file.sections, captured);

    set_hero(&mut app, Hero::default());
    let applied = shards::save::apply_sections(app.world_mut(), SaveKind::Character, Some(body), &file.sections).unwrap();
    assert_eq!(applied.applied, ["position", "vitals", "hero"], "the engine registered its sections first");
    assert_eq!(hero(&mut app), wanted);
    // Not into a world with nobody to put it in.
    let err = shards::save::apply_sections(app.world_mut(), SaveKind::Character, None, &file.sections).unwrap_err();
    assert!(err.message.contains("no player body") && err.applied.is_empty(), "{err:?}");
}
