//! A headless game: the blows become holds in the table's eyes (the event, the journal line), and
//! nothing is held. The rules, the settings and the tools.

use shards::action::{Blocked, Parried, PerfectEvaded, StaggerCause, Staggered};
use shards::hitstop::{EventKind, HitStopped, HitStops, Outcome};

use crate::*;

/// The `HitStopped` events a game triggers, in order.
#[derive(Resource, Default)]
struct Seen(Vec<HitStopped>);

fn watch(app: &mut App) {
    app.init_resource::<Seen>();
    app.add_observer(|e: On<HitStopped>, mut seen: ResMut<Seen>| seen.0.push(e.event().clone()));
}

fn seen(app: &App) -> Vec<(String, f32)> {
    app.world().resource::<Seen>().0.iter().map(|s| (s.name.clone(), s.seconds)).collect()
}

fn last_outcome(app: &App) -> Outcome {
    app.world().resource::<HitStops>().log.back().unwrap().outcome.clone()
}

#[test]
fn a_swing_that_lands_is_a_hold_on_both_bodies_and_nothing_is_held() {
    let assets = Folder::new("swing");
    let mut app = game(&assets);
    watch(&mut app);
    let hero = fighter(app.world_mut(), "hero", "heroes", Vec3::ZERO, Vec3::NEG_Z, 100.0);
    let brute = fighter(app.world_mut(), "brute", "monsters", Vec3::new(0.0, 0.0, -1.5), Vec3::Z, 100.0);
    app.update();
    press(&mut app, hero, "light");
    step(&mut app, 30);
    assert_eq!(seen(&app), [("hit".to_owned(), 0.06)]);
    let stop = &app.world().resource::<Seen>().0[0];
    assert_eq!((stop.event, stop.rule), (EventKind::Damaged, 4));
    let (a, b) = (id(&app, hero), id(&app, brute));
    assert_eq!(stop.ids, [app.world().get::<AiId>(hero).copied().unwrap(), app.world().get::<AiId>(brute).copied().unwrap()]);
    let line = lines(&app, JournalKind::Local, "hitstop:");
    assert_eq!(line, [format!("hitstop: hit 0.06 s (damaged) on {a}, {b}")]);
    assert!(lines(&app, JournalKind::Event, "hitstop").is_empty(), "a line of this machine, not a gameplay event");
    // Nothing is held, and the tool says why.
    let said = ok(&mut app, "hitstop", json!({}));
    assert!(said.starts_with("hit stop: headless: never holds\n"), "{said}");
    assert!(said.contains("not held: nothing is drawn (headless)"), "{said}");
    assert!(said.contains("total: 1 blows · 0 clock holds · 0 picture holds · 1 not held"), "{said}");
    let state = status(&app);
    assert_eq!((state["mode"].as_str(), state["events"].as_u64(), state["clock_holds"].as_u64()), (Some("headless: never holds"), Some(1), Some(0)));
    assert_eq!(state["ticks_inside"], 0);
    assert!(!app.world().resource::<shards::ai_core::ClockHold>().is_pending());
    assert!(shards::ai_core::sim::is_paused(app.world()), "a stepped game stays paused");
}

#[test]
fn the_first_rule_that_matches_wins_for_every_event() {
    let assets = Folder::new("ladder");
    let mut app = game(&assets);
    watch(&mut app);
    let a = fighter(app.world_mut(), "hero", "heroes", Vec3::ZERO, Vec3::NEG_Z, 100.0);
    let b = fighter(app.world_mut(), "brute", "monsters", Vec3::new(0.0, 0.0, -1.5), Vec3::Z, 100.0);
    app.update();
    let world = app.world_mut();
    world.trigger(damaged(a, b, HitWeight::Finisher, Some(3)));
    world.trigger(damaged(a, b, HitWeight::Break, Some(3)));
    world.trigger(damaged(a, b, HitWeight::Counter, None));
    world.trigger(damaged(a, b, HitWeight::Heavy, Some(2)));
    world.trigger(damaged(a, b, HitWeight::Light, Some(1)));
    world.trigger(damaged(a, b, HitWeight::Light, None));
    // A hit that did not land (a roll's i-frames) gives none.
    world.trigger(Damaged { invulnerable: true, ..damaged(a, b, HitWeight::Heavy, Some(2)) });
    world.trigger(Blocked {
        attacker: a,
        defender: b,
        action: "light_1".into(),
        amount: 0.0,
        stamina: 0.0,
        hp_left: 100.0,
        weight: HitWeight::Light,
        absorbed: 0.0,
        source: None,
    });
    world.trigger(Parried { attacker: a, defender: b, action: "light_1".into(), stamina: 0.0, source: None });
    world.trigger(PerfectEvaded { attacker: a, defender: b, action: "light_1".into(), source: None });
    // A stagger matches no rule of this table.
    world.trigger(Staggered { entity: b, cause: StaggerCause::PoiseBroken });
    world.flush();
    assert_eq!(
        seen(&app),
        [
            ("finisher".to_owned(), 0.10),
            ("poise break".to_owned(), 0.10),
            ("counter".to_owned(), 0.08),
            ("heavy hit".to_owned(), 0.08),
            ("hit".to_owned(), 0.06),
            ("hit".to_owned(), 0.06),
            ("block".to_owned(), 0.06),
            ("parry".to_owned(), 0.10),
            ("perfect dodge".to_owned(), 0.10),
        ]
    );
    assert_eq!(lines(&app, JournalKind::Local, "hitstop:").len(), 9);
}

#[test]
fn the_tool_lists_the_table_and_makes_a_hold_through_it() {
    let assets = Folder::new("tool");
    let mut app = game(&assets);
    let a = fighter(app.world_mut(), "hero", "heroes", Vec3::ZERO, Vec3::NEG_Z, 100.0);
    app.update();
    let said = ok(&mut app, "hitstop", json!({}));
    assert!(said.contains("table: 8 rules · ceiling 0.12 s · cooldown 0.00 s"), "{said}");
    assert!(said.contains("   2 poise break    damaged · weight ≥ Break → 0.10 s"), "{said}");
    assert!(said.contains("   4 heavy hit      damaged · reaction ≥ 2 → 0.08 s"), "{said}");
    assert!(said.contains("settings: scale 1 (accessibility.hit_stop; 0 is off) · only the local player's blows: no"), "{said}");
    let at = id(&app, a);
    assert_eq!(
        ok(&mut app, "hitstop_play", json!({"event": "damaged", "weight": "break", "attacker": at})),
        "poise break 0.10 s (damaged): not held: nothing is drawn (headless) (0.00 s)"
    );
    assert_eq!(ok(&mut app, "hitstop_play", json!({"event": "staggered"})), "no hold: no rule of the table gives a hold for staggered");
    assert_eq!(ok(&mut app, "hitstop_play", json!({"seconds": 9.0})), "tool 0.50 s (damaged): not held: nothing is drawn (headless) (0.00 s)");
    assert!(refused(&mut app, "hitstop_play", json!({"event": "damge"})).contains("did you mean `damaged`?"));
    assert!(refused(&mut app, "hitstop_play", json!({"weight": "huge"})).contains("weights: Light, Heavy"));
    assert!(refused(&mut app, "hitstop_play", json!({"seconds": -1.0})).contains("a duration above 0"));
    let said = ok(&mut app, "hitstop", json!({"last": 1}));
    assert!(said.contains("total: 2 blows"), "{said}");
    assert_eq!(said.lines().filter(|l| l.starts_with("  tick ")).count(), 1, "{said}");
}

#[test]
fn a_game_without_a_table_holds_nothing_and_says_so() {
    let assets = Folder::empty("none");
    let mut app = game(&assets);
    let a = fighter(app.world_mut(), "hero", "heroes", Vec3::ZERO, Vec3::NEG_Z, 100.0);
    app.update();
    app.world_mut().trigger(damaged(a, a, HitWeight::Light, None));
    app.world_mut().flush();
    assert_eq!(status(&app)["events"], 0);
    assert!(ok(&mut app, "hitstop", json!({})).contains("no table (`*.hitstop.ron`): no blow holds anything"));
    assert_eq!(ok(&mut app, "hitstop_play", json!({})), "no hold: the game has no hit stop table (`*.hitstop.ron`)");
}

#[test]
fn the_players_settings_scale_and_filter_the_holds() {
    use shards::settings::groups::AccessibilitySettings;
    let assets = Folder::new("settings");
    let mut app = drawn(&assets);
    let hero = fighter(app.world_mut(), "hero", "heroes", Vec3::ZERO, Vec3::NEG_Z, 100.0);
    let brute = fighter(app.world_mut(), "brute", "monsters", Vec3::new(0.0, 0.0, -1.5), Vec3::Z, 100.0);
    app.world_mut().entity_mut(hero).insert(LocalPlayer);
    app.update();
    // Stepped, a drawn game does not hold either: the outcome says it, whatever the setting.
    app.world_mut().trigger(damaged(hero, brute, HitWeight::Light, None));
    app.world_mut().flush();
    assert_eq!(last_outcome(&app), Outcome::Skipped("the clock is paused or stepped".into()));

    ok(&mut app, "settings_set", json!({"group": "accessibility", "key": "hit_stop", "value": "0"}));
    app.world_mut().trigger(damaged(hero, brute, HitWeight::Light, None));
    app.world_mut().flush();
    assert_eq!(last_outcome(&app), Outcome::Skipped("hit stop is off in the settings".into()));
    assert!(refused(&mut app, "settings_set", json!({"group": "accessibility", "key": "hit_stop", "value": "3"})).contains("0 to 1"));

    ok(&mut app, "settings_set", json!({"group": "accessibility", "key": "hit_stop", "value": "0.5"}));
    ok(&mut app, "settings_set", json!({"group": "accessibility", "key": "hit_stop_local_only", "value": "true"}));
    let other = fighter(app.world_mut(), "brute", "monsters", Vec3::new(5.0, 0.0, 0.0), Vec3::Z, 100.0);
    app.world_mut().trigger(damaged(brute, other, HitWeight::Light, None));
    app.world_mut().flush();
    assert!(matches!(&last_outcome(&app), Outcome::Skipped(why) if why.contains("local player is not in this blow")), "{:?}", last_outcome(&app));
    assert_eq!(app.world().resource::<AccessibilitySettings>().hit_stop_scale(), 0.5);
}
