//! Weapons (F4): ammo as named charges, costs and refusals, reloads that give nothing when cut, the
//! arsenal's slots and switches, and the calls a pickup makes. Content: `assets/content/weapons.*.ron`.

use bevy::prelude::*;
use serde_json::json;
use shards_action::{
    add_to_charge, force_action, grant_weapon, select_slot, take_weapon, Arsenal, Charge, Charges, Grant, Held, Fighter, MAX_SLOTS,
};

use crate::common::*;

/// The player with `moveset` and exactly these charges (`name`, `current`, `max`; sorted by name).
fn armed(app: &mut App, moveset: &str, charges: &[(&str, u32, u32)]) -> Entity {
    let hero = hero(app.world_mut(), moveset, Vec3::ZERO, N);
    let owned = charges.iter().map(|(name, current, max)| Charge { name: (*name).to_owned(), current: *current, max: *max }).collect();
    app.world_mut().entity_mut(hero).insert(Charges(owned));
    hero
}

/// What the body holds of a charge: `(current, max)`.
fn held(app: &App, e: Entity, name: &str) -> (u32, u32) {
    let charge = app.world().get::<Charges>(e).unwrap().get(name).unwrap_or_else(|| panic!("no charge {name}"));
    (charge.current, charge.max)
}

/// The player with a three-weapon arsenal on top of the plain `hero` moveset: 1 sidearm, 2 blade, 3 scatter gun,
/// the sidearm in the hands, after the first tick has aligned it.
fn with_arsenal(app: &mut App) -> Entity {
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let slots = vec![
        Held { slot: 1, moveset: "w_sidearm".into() },
        Held { slot: 2, moveset: "w_blade".into() },
        Held { slot: 3, moveset: "w_scatter".into() },
    ];
    app.world_mut().entity_mut(hero).insert(Arsenal { slots, current: Some(1), ..default() });
    step(app, 1);
    hero
}

fn moveset(app: &App, e: Entity) -> String {
    app.world().get::<Fighter>(e).unwrap().moveset.clone()
}

fn arsenal(app: &App, e: Entity) -> Arsenal {
    app.world().get::<Arsenal>(e).unwrap().clone()
}

#[test]
fn a_dry_magazine_is_refused_with_the_way_out_and_a_reload_fills_it() {
    let mut app = app();
    let hero = armed(&mut app, "w_sidearm", &[("sidearm", 0, 6), ("sidearm.reserve", 12, 36)]);
    let h = id(&app, hero);

    // The tool says why and what to do; a press that waits out its buffer is journaled with the reason.
    assert_eq!(fails(&mut app, "light_attack", json!({})), "sidearm empty (0/6): reload");
    press(&mut app, hero, "light");
    step(&mut app, 14);
    let rejected = lines(&app, "rejected");
    assert_eq!(rejected.len(), 1, "{rejected:?}");
    assert!(rejected[0].ends_with(&format!("{h} light rejected: sidearm empty: reload")), "{rejected:?}");
    assert_eq!(held(&app, hero, "sidearm"), (0, 6), "a refused press costs nothing");

    // The reload moves the rounds at its 0.5 s mark, not before.
    assert_eq!(ok(&mut app, "reload", json!({})), "ok: w_reload");
    step(&mut app, 25);
    assert_eq!(held(&app, hero, "sidearm"), (0, 6), "the draw is not over");
    step(&mut app, 10);
    assert_eq!(held(&app, hero, "sidearm"), (6, 6));
    assert_eq!(held(&app, hero, "sidearm.reserve"), (6, 36));
    assert!(
        lines(&app, "refilled").iter().any(|l| l.ends_with(&format!("{h} refilled sidearm +6 (6/6), 6 sidearm.reserve left"))),
        "{:?}",
        lines(&app, "refilled")
    );

    // A full magazine is no reason to reload; a shot takes a round.
    step(&mut app, 10);
    assert_eq!(fails(&mut app, "reload", json!({})), "sidearm is already full (6/6)");
    assert_eq!(ok(&mut app, "light_attack", json!({})), "ok: w_shot");
    step(&mut app, 2);
    assert_eq!(held(&app, hero, "sidearm"), (5, 6));
}

#[test]
fn a_cut_reload_gives_nothing() {
    let mut app = app();
    let hero = armed(&mut app, "w_sidearm", &[("sidearm", 0, 6), ("sidearm.reserve", 12, 36)]);
    ok(&mut app, "reload", json!({}));
    step(&mut app, 20);
    assert_eq!(action(&app, hero), "w_reload");
    // A stagger lands before the rounds move: the run ends, and nothing was loaded.
    assert!(force_action(app.world_mut(), hero, "w_stagger"));
    step(&mut app, 40);
    assert_eq!(held(&app, hero, "sidearm"), (0, 6));
    assert_eq!(held(&app, hero, "sidearm.reserve"), (12, 36));
    assert!(lines(&app, "refilled").is_empty(), "{:?}", lines(&app, "refilled"));
    // Reloading again, to the end, does load them.
    ok(&mut app, "reload", json!({}));
    step(&mut app, 45);
    assert_eq!(held(&app, hero, "sidearm"), (6, 6));
    assert_eq!(held(&app, hero, "sidearm.reserve"), (6, 36));
}

#[test]
fn a_reload_that_would_move_nothing_says_why() {
    let mut app = app();
    let hero = armed(&mut app, "w_sidearm", &[("sidearm", 0, 6), ("sidearm.reserve", 0, 36)]);
    assert_eq!(fails(&mut app, "reload", json!({})), "no sidearm.reserve left to refill sidearm with; find more");
    assert_eq!(
        fails(&mut app, "light_attack", json!({})),
        "sidearm empty (0/6) and no sidearm.reserve left to reload with; find more"
    );
    press(&mut app, hero, "light");
    step(&mut app, 14);
    assert!(lines(&app, "rejected").iter().any(|l| l.ends_with("light rejected: sidearm empty: no sidearm.reserve left")), "{:?}", journal(&app));
    // A pickup of ammo is the way out.
    assert_eq!(add_to_charge(app.world_mut(), hero, "sidearm.reserve", 10), 10);
    assert_eq!(ok(&mut app, "reload", json!({})), "ok: w_reload");
}

#[test]
fn an_empty_magazine_reloads_by_itself_when_the_moveset_says_so() {
    let mut app = app();
    let hero = armed(&mut app, "w_auto", &[("auto", 0, 4), ("auto.reserve", 8, 8)]);
    // The press that would fire starts the reload: the same answer the machine gives.
    assert_eq!(ok(&mut app, "light_attack", json!({})), "ok: w_auto_reload");
    step(&mut app, 20);
    assert_eq!(held(&app, hero, "auto"), (0, 4), "not yet");
    step(&mut app, 8);
    assert_eq!(held(&app, hero, "auto"), (4, 4));
    assert_eq!(held(&app, hero, "auto.reserve"), (4, 8));
    step(&mut app, 10);
    assert_eq!(ok(&mut app, "light_attack", json!({})), "ok: w_auto_shot");

    // With nothing in the reserve the reload would move nothing: the press is refused as usual.
    let mut app = crate::common::app();
    armed(&mut app, "w_auto", &[("auto", 0, 4), ("auto.reserve", 0, 8)]);
    assert_eq!(
        fails(&mut app, "light_attack", json!({})),
        "auto empty (0/4) and no auto.reserve left to reload with; find more"
    );
}

#[test]
fn a_shot_can_take_several_charges_at_once() {
    let mut app = app();
    let hero = armed(&mut app, "w_scatter", &[("shells", 1, 4), ("shells.reserve", 12, 12)]);
    assert_eq!(fails(&mut app, "light_attack", json!({})), "shells short (1 of 2 needed) (1/4): reload");
    app.world_mut().get_mut::<Charges>(hero).unwrap().get_mut("shells").unwrap().current = 3;
    assert_eq!(ok(&mut app, "light_attack", json!({})), "ok: w_scatter_shot");
    step(&mut app, 3);
    assert_eq!(held(&app, hero, "shells"), (1, 4), "two went");
}

#[test]
fn a_per_round_reload_keeps_the_rounds_it_got_to_load() {
    let mut app = app();
    let hero = armed(&mut app, "w_scatter", &[("shells", 0, 4), ("shells.reserve", 12, 12)]);
    ok(&mut app, "reload", json!({}));
    step(&mut app, 20);
    assert_eq!(held(&app, hero, "shells"), (1, 4), "one at 0.3 s");
    step(&mut app, 18);
    assert_eq!(held(&app, hero, "shells"), (2, 4), "and one at 0.6 s");
    assert!(force_action(app.world_mut(), hero, "w_stagger"));
    step(&mut app, 60);
    assert_eq!(held(&app, hero, "shells"), (2, 4), "the third never came");
    assert_eq!(held(&app, hero, "shells.reserve"), (10, 12));
}

#[test]
fn an_arsenal_brings_its_charges_and_the_moveset_in_the_hands() {
    let mut app = app();
    let hero = with_arsenal(&mut app);
    assert_eq!(moveset(&app, hero), "w_sidearm");
    assert_eq!(arsenal(&app, hero).bare.as_deref(), Some("hero"), "the moveset it had is kept for when it holds none");
    // Its own flask stays; each weapon brought what it declared, sorted by name.
    let names: Vec<String> = app.world().get::<Charges>(hero).unwrap().0.iter().map(|c| c.name.clone()).collect();
    assert_eq!(names, ["flask", "shells", "shells.reserve", "sidearm", "sidearm.reserve"]);
    assert_eq!(held(&app, hero, "sidearm"), (6, 6));
    assert_eq!(held(&app, hero, "sidearm.reserve"), (12, 36), "`start` says 12");
    assert_eq!(held(&app, hero, "shells"), (0, 4), "an empty scatter gun");
    assert_eq!(held(&app, hero, "flask"), (2, 2));
}

#[test]
fn a_switch_waits_for_the_cancel_window_of_the_running_action() {
    let mut app = app();
    let hero = with_arsenal(&mut app);
    let h = id(&app, hero);
    assert_eq!(ok(&mut app, "light_attack", json!({})), "ok: w_shot");
    step(&mut app, 2);
    // The shot's fire phase lets nothing through; the recover phase (from 0.1 s) lets a switch.
    let answer = ok(&mut app, "weapon", json!({"slot": 2}));
    assert!(answer.starts_with("ok: switches to 2 (w_blade) in ") && answer.ends_with("when a cancel window of w_shot opens"), "{answer}");
    step(&mut app, 3);
    assert_eq!(moveset(&app, hero), "w_sidearm", "still firing");
    assert_eq!(arsenal(&app, hero).pending, Some(2), "the switch waits");
    step(&mut app, 4);
    assert_eq!(moveset(&app, hero), "w_blade");
    assert_eq!(action(&app, hero), "idle", "the shot was cut where the window let the switch through");
    let a = arsenal(&app, hero);
    assert_eq!((a.current, a.pending), (Some(2), None));
    assert_eq!(held(&app, hero, "sidearm"), (5, 6), "the round was spent");
    assert!(lines(&app, "switches to").iter().any(|l| l.ends_with(&format!("{h} switches to w_blade (slot 2, from w_sidearm)"))), "{:?}", journal(&app));
}

#[test]
fn an_idle_body_switches_at_once_and_next_and_prev_go_round_the_slots() {
    let mut app = app();
    let hero = with_arsenal(&mut app);
    assert_eq!(ok(&mut app, "weapon", json!({"slot": "next"})), "ok: switches to 2 (w_blade)");
    step(&mut app, 1);
    assert_eq!(moveset(&app, hero), "w_blade");
    // Two presses in one tick go two weapons on; the last wraps round.
    press(&mut app, hero, "weapon.next");
    press(&mut app, hero, "weapon.next");
    step(&mut app, 1);
    assert_eq!((moveset(&app, hero), arsenal(&app, hero).current), ("w_sidearm".to_owned(), Some(1)));
    press(&mut app, hero, "weapon.prev");
    step(&mut app, 1);
    assert_eq!(arsenal(&app, hero).current, Some(3));
    assert_eq!(moveset(&app, hero), "w_scatter");
    assert_eq!(ok(&mut app, "weapon", json!({"slot": 3})), "ok: already holding 3 (w_scatter)");
    // The switch drops what waited for the old weapon.
    press(&mut app, hero, "weapon.slot.1");
    step(&mut app, 1);
    assert_eq!(moveset(&app, hero), "w_sidearm");
}

#[test]
fn a_slot_nobody_carries_is_refused_like_any_input() {
    let mut app = app();
    let hero = with_arsenal(&mut app);
    let err = fails(&mut app, "weapon", json!({"slot": 7}));
    assert_eq!(err, "no weapon in slot 7; you carry: 1 w_sidearm, 2 w_blade, 3 w_scatter");
    assert!(fails(&mut app, "weapon", json!({"slot": "up"})).starts_with("slot `up`: give a slot number"));
    // A keyboard binding does not go through the tool: the journal says it.
    press(&mut app, hero, "weapon.slot.7");
    step(&mut app, 1);
    assert!(
        lines(&app, "rejected").iter().any(|l| l.ends_with("weapon.slot.7 rejected: no weapon in slot 7; you carry: 1 w_sidearm, 2 w_blade, 3 w_scatter")),
        "{:?}",
        journal(&app)
    );
    assert_eq!(moveset(&app, hero), "w_sidearm");
}

#[test]
fn switching_away_in_a_reload_cuts_it_and_gives_nothing() {
    let mut app = app();
    let hero = with_arsenal(&mut app);
    app.world_mut().get_mut::<Charges>(hero).unwrap().get_mut("sidearm").unwrap().current = 0;
    ok(&mut app, "reload", json!({}));
    step(&mut app, 15);
    assert_eq!(action(&app, hero), "w_reload");
    // The draw lets a switch through.
    assert!(ok(&mut app, "weapon", json!({"slot": 2})).starts_with("ok: switches to 2 (w_blade)"));
    step(&mut app, 2);
    assert_eq!(moveset(&app, hero), "w_blade");
    step(&mut app, 30);
    assert_eq!(held(&app, hero, "sidearm"), (0, 6));
    assert_eq!(held(&app, hero, "sidearm.reserve"), (12, 36));
    assert!(lines(&app, "refilled").is_empty());
    // Back to the sidearm and reload it through: the rounds come.
    ok(&mut app, "weapon", json!({"slot": 1}));
    step(&mut app, 1);
    ok(&mut app, "reload", json!({}));
    step(&mut app, 45);
    assert_eq!(held(&app, hero, "sidearm"), (6, 6));
}

#[test]
fn an_action_without_a_window_makes_the_switch_wait_for_its_end() {
    let mut app = app();
    let hero = with_arsenal(&mut app);
    // Slot 3's scatter gun: its shot (0.2 s) has no window for `weapon`.
    ok(&mut app, "weapon", json!({"slot": 3}));
    step(&mut app, 1);
    app.world_mut().get_mut::<Charges>(hero).unwrap().get_mut("shells").unwrap().current = 4;
    assert_eq!(ok(&mut app, "light_attack", json!({})), "ok: w_scatter_shot");
    step(&mut app, 2);
    let answer = ok(&mut app, "weapon", json!({"slot": 1}));
    assert!(answer.contains("when w_scatter_shot ends"), "{answer}");
    step(&mut app, 5);
    assert_eq!(moveset(&app, hero), "w_scatter", "mid-shot");
    step(&mut app, 12);
    assert_eq!(moveset(&app, hero), "w_sidearm", "the shot is over");
}

#[test]
fn the_calls_a_pickup_makes_say_what_they_took() {
    let mut app = app();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let world = app.world_mut();
    assert_eq!(grant_weapon(world, hero, 0, "w_sidearm").unwrap_err(), format!("slot 0: slots are numbered 1 to {MAX_SLOTS}"));
    assert!(grant_weapon(world, hero, 1, "w_nope").unwrap_err().starts_with("unknown moveset `w_nope`; the movesets are: "));

    // A body with no weapon takes the first into its hands, with the charges it brings.
    assert_eq!(grant_weapon(world, hero, 2, "w_sidearm"), Ok(Grant::Added));
    let a = world.get::<Arsenal>(hero).unwrap();
    assert_eq!((a.current, a.bare.as_deref()), (Some(2), Some("hero")));
    assert_eq!(world.get::<Fighter>(hero).unwrap().moveset, "w_sidearm");
    assert_eq!(world.get::<Charges>(hero).unwrap().get("sidearm.reserve").unwrap().current, 12);
    // The same weapon again changes nothing: a pickup gives ammo instead. Another weapon goes beside it.
    assert_eq!(grant_weapon(world, hero, 2, "w_sidearm"), Ok(Grant::AlreadyHeld));
    assert_eq!(grant_weapon(world, hero, 1, "w_blade"), Ok(Grant::Added));
    assert_eq!(world.get::<Arsenal>(hero).unwrap().list(), "1 w_blade, 2 w_sidearm", "by slot");
    assert_eq!(world.get::<Fighter>(hero).unwrap().moveset, "w_sidearm", "the sidearm stays in the hands");

    // Ammo: as much as the charge has room for.
    assert_eq!(add_to_charge(world, hero, "sidearm.reserve", 100), 24);
    assert_eq!(add_to_charge(world, hero, "sidearm.reserve", 1), 0, "full");
    assert_eq!(add_to_charge(world, hero, "no_such_charge", 5), 0);

    // A weapon in the hands taken away: the lowest slot left comes up; the last one takes the arsenal.
    select_slot(world, hero, 1).unwrap();
    assert!(select_slot(world, hero, 5).unwrap_err().starts_with("no weapon in slot 5"));
    assert_eq!(take_weapon(world, hero, 2).as_deref(), Some("w_sidearm"));
    assert_eq!(take_weapon(world, hero, 2), None);
    assert_eq!(world.get::<Fighter>(hero).unwrap().moveset, "w_blade");
    assert_eq!(world.get::<Arsenal>(hero).unwrap().current, Some(1));
    assert_eq!(world.get::<Charges>(hero).unwrap().get("sidearm.reserve").unwrap().current, 36, "the rounds stay");
    assert_eq!(take_weapon(world, hero, 1).as_deref(), Some("w_blade"));
    assert!(world.get::<Arsenal>(hero).is_none());
    assert_eq!(world.get::<Fighter>(hero).unwrap().moveset, "hero", "bare hands again");

    // A weapon that replaces the one in the hands replaces it in the hands.
    assert_eq!(grant_weapon(world, hero, 1, "w_blade"), Ok(Grant::Added));
    assert_eq!(grant_weapon(world, hero, 1, "w_auto"), Ok(Grant::Replaced("w_blade".into())));
    assert_eq!(world.get::<Fighter>(hero).unwrap().moveset, "w_auto");
}

#[test]
fn the_dev_tools_do_what_a_pickup_does() {
    let mut app = app();
    let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
    let h = id(&app, hero);
    assert_eq!(ok(&mut app, "give_weapon", json!({"moveset": "w_sidearm"})), format!("{h} carries w_sidearm in slot 1"));
    assert_eq!(ok(&mut app, "give_weapon", json!({"moveset": "w_blade"})), format!("{h} carries w_blade in slot 2"));
    assert_eq!(ok(&mut app, "give_weapon", json!({"moveset": "w_blade", "slot": 2})), format!("{h} carries w_blade in slot 2 already; nothing changed"));
    assert!(fails(&mut app, "give_weapon", json!({"moveset": "w_blade", "slot": 12})).contains("slots are numbered 1 to 9"));
    assert_eq!(ok(&mut app, "give_ammo", json!({"charge": "sidearm.reserve", "n": 30})), format!("{h} sidearm.reserve +24 (36/36), 6 did not fit"));
    assert_eq!(ok(&mut app, "give_ammo", json!({"charge": "sidearm.reserve", "n": 5})), format!("{h} sidearm.reserve +0 (36/36), it was full"));
    assert!(fails(&mut app, "give_ammo", json!({"charge": "rockets", "n": 1})).starts_with(&format!("{h} has no charge `rockets`; it has: ")));
    assert_eq!(ok(&mut app, "take_weapon", json!({"slot": 1})), format!("{h} no longer carries w_sidearm (slot 1)"));
    assert_eq!(fails(&mut app, "take_weapon", json!({"slot": 1})), format!("{h} has no weapon in slot 1; it carries: 2 w_blade"));
}

#[test]
fn a_dead_body_switches_nothing() {
    let mut app = app();
    let hero = with_arsenal(&mut app);
    shards_action::kill(app.world_mut(), hero, None, None);
    press(&mut app, hero, "weapon.slot.2");
    step(&mut app, 3);
    assert_eq!(moveset(&app, hero), "w_sidearm");
    assert_eq!(fails(&mut app, "weapon", json!({"slot": 2})), "you are dead; a dev agent can `heal` you");
}
