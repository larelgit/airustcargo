//! The rules' damage factors (C3.2): the target class's incoming factor (`DamageCtx::rules`), the
//! credited body's outgoing factor after the hooks, `deal_ruled_damage` against `deal_damage`.
//! The rules content is applied to the running app with `content_apply`, as a game's scenario does:
//! three modes (`plain` changes nothing, `ward` halves what a `player` takes, `power` makes a `brute`
//! hit one and a half times as hard) and the class `brute`, which a session of two doubles.
//! Every number is measured against the same fight without rules.

use bevy::prelude::*;
use serde_json::json;
use shards_action::prelude::*;
use shards_action::{
    deal_damage, deal_ruled_damage, objects::spawn_objects, objects::SpawnDef, ActionAppExt, Armor, Blocked, DamageCtx,
    Damaged, Evaded, GuardBroken, Invulnerable, Parried,
};
use shards_core::PlayerId;
use shards_rules::{ActiveRules, RuleClass};

use crate::common::*;
use crate::reactions::bout_with;

const RULES: &str = r#"{"modes": ["plain", "ward", "power"], "start": "plain", "classes": ["brute"],
    "session": {"brute": {"outgoing_damage": [1.0, 2.0]}}}"#;

/// Puts the rules in the app and plays `mode`.
fn play(app: &mut App, mode: &str) {
    let sets = [
        ("plain", json!({"name": "Plain"})),
        ("ward", json!({"name": "Ward", "classes": {"player": {"incoming_damage": 0.5}}})),
        ("power", json!({"name": "Power", "classes": {"brute": {"outgoing_damage": 1.5}}})),
    ];
    let mut changes = vec![json!({"kind": "rules", "id": "default", "def": serde_json::from_str::<serde_json::Value>(RULES).unwrap()})];
    changes.extend(sets.iter().map(|(id, def)| json!({"kind": "rule_set", "id": id, "def": def})));
    ok(app, "content_apply", json!({"changes": changes}));
    assert_eq!(app.world().resource::<ActiveRules>().mode, "plain", "a world with rules content plays `start`");
    app.world_mut().resource_mut::<ActiveRules>().mode = mode.to_owned();
}

/// What the observers saw: `(kind, health damage)` of every outcome that damages.
#[derive(Resource, Default)]
struct Dealt(Vec<(&'static str, f32)>);

fn watch(app: &mut App) {
    app.init_resource::<Dealt>();
    app.add_observer(|e: On<Damaged>, mut d: ResMut<Dealt>| {
        if !e.event().invulnerable {
            d.0.push(("hit", e.event().amount));
        }
    });
    app.add_observer(|e: On<Blocked>, mut d: ResMut<Dealt>| d.0.push(("block", e.event().amount)));
    app.add_observer(|e: On<GuardBroken>, mut d: ResMut<Dealt>| d.0.push(("break", e.event().amount)));
    app.add_observer(|_: On<Parried>, mut d: ResMut<Dealt>| d.0.push(("parry", 0.0)));
    app.add_observer(|_: On<Evaded>, mut d: ResMut<Dealt>| d.0.push(("evade", 0.0)));
}

/// What one swing did.
#[derive(Debug)]
struct Swing {
    /// `(outcome, health damage)` in order.
    dealt: Vec<(&'static str, f32)>,
    hp: f32,
    poise: f32,
    stamina: f32,
}

/// The attacker (class `attacker`) swings `input` at a victim (class `victim`) that holds its guard
/// when `guarding`; `mode` is the rules in play (`None`: no rules content at all). `setup` runs on the
/// two bodies before the swing.
fn swing(mode: Option<&str>, attacker: Option<&str>, victim: Option<&str>, guarding: bool, input: &str) -> Swing {
    swing_with(mode, attacker, victim, guarding, input, |_, _, _| {})
}

fn swing_with(
    mode: Option<&str>,
    attacker: Option<&str>,
    victim: Option<&str>,
    guarding: bool,
    input: &str,
    setup: impl FnOnce(&mut App, Entity, Entity),
) -> Swing {
    let (mut app, a, v) = bout_with(if guarding { "b_victim" } else { "r_victim" }, watch);
    if let Some(mode) = mode {
        play(&mut app, mode);
    }
    for (e, class) in [(a, attacker), (v, victim)] {
        if let Some(class) = class {
            app.world_mut().entity_mut(e).insert(RuleClass(class.to_owned()));
        }
    }
    setup(&mut app, a, v);
    if guarding {
        hold(&mut app, v, "block", true);
        step(&mut app, 30);
    }
    press(&mut app, a, input);
    step(&mut app, 12);
    let poise = app.world().get::<Poise>(v).unwrap();
    Swing {
        dealt: app.world().resource::<Dealt>().0.clone(),
        hp: hp(&app, v),
        poise: poise.max - poise.current,
        stamina: stamina(&app, v),
    }
}

fn near(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-3
}

#[track_caller]
fn assert_dealt(swing: &Swing, kind: &str, amount: f32) {
    assert_eq!(swing.dealt.len(), 1, "{swing:?}");
    assert_eq!(swing.dealt[0].0, kind, "{swing:?}");
    assert!(near(swing.dealt[0].1, amount), "{kind}: wanted {amount}, {swing:?}");
}

// --- incoming ------------------------------------------------------------------------------------

#[test]
fn incoming_halves_a_hit_and_not_its_poise() {
    let base = swing(None, None, None, false, "light");
    assert_dealt(&base, "hit", 10.0);
    assert!(base.poise > 0.0, "the light attack deals poise: {base:?}");
    // A mode without a factor, and a body of another class, change nothing.
    for other in [swing(Some("plain"), None, Some("player"), false, "light"), swing(Some("ward"), None, Some("brute"), false, "light")] {
        assert_dealt(&other, "hit", 10.0);
        assert!(near(other.poise, base.poise), "{other:?}");
    }
    // A body with no class is not touched either, in a mode that rules `player`.
    assert_dealt(&swing(Some("ward"), None, None, false, "light"), "hit", 10.0);
    let ward = swing(Some("ward"), None, Some("player"), false, "light");
    assert_dealt(&ward, "hit", 5.0);
    assert!(near(ward.hp, 95.0), "{ward:?}");
    assert!(near(ward.poise, base.poise), "the incoming factor leaves poise alone: {ward:?} {base:?}");
}

#[test]
fn incoming_halves_a_blocks_chip_and_a_guard_breaks_damage_not_the_guards_cost() {
    let base = swing(None, None, None, true, "light");
    assert_eq!(base.dealt.len(), 1, "{base:?}");
    assert_eq!(base.dealt[0].0, "block");
    let chip = base.dealt[0].1;
    assert!(chip > 0.0, "{base:?}");
    let ward = swing(Some("ward"), None, Some("player"), true, "light");
    assert_dealt(&ward, "block", chip * 0.5);
    assert!(near(ward.stamina, base.stamina), "the block's stamina cost is not scaled: {ward:?} {base:?}");
    assert!(near(ward.poise, base.poise), "{ward:?} {base:?}");

    let base = swing(None, None, None, true, "technique.6");
    assert_eq!(base.dealt[0].0, "break", "{base:?}");
    let ward = swing(Some("ward"), None, Some("player"), true, "technique.6");
    assert_dealt(&ward, "break", base.dealt[0].1 * 0.5);
}

#[test]
fn incoming_does_not_reach_a_parry_an_evade_or_god_mode() {
    // A parry and an evade return before the factor: nothing is dealt either way.
    let parried = swing(Some("ward"), None, Some("player"), false, "heavy");
    assert!(near(parried.dealt[0].1, 20.0), "an unguarded heavy hit: 40 halved: {parried:?}");
    let (mut app, a, v) = bout_with("r_victim", watch);
    play(&mut app, "ward");
    app.world_mut().entity_mut(v).insert(RuleClass("player".into()));
    hold(&mut app, v, "block", true);
    press(&mut app, a, "heavy");
    step(&mut app, 8);
    assert_eq!(app.world().resource::<Dealt>().0, [("parry", 0.0)]);
    assert_eq!(hp(&app, v), 100.0);

    let (mut app, a, v) = bout_with("r_victim", watch);
    play(&mut app, "ward");
    app.world_mut().entity_mut(v).insert(RuleClass("player".into()));
    press(&mut app, v, "dodge");
    step(&mut app, 2);
    press(&mut app, a, "technique.7");
    step(&mut app, 12);
    assert_eq!(app.world().resource::<Dealt>().0, [("evade", 0.0)]);
    assert_eq!(hp(&app, v), 100.0);

    let (mut app, a, v) = bout_with("r_victim", watch);
    play(&mut app, "ward");
    app.world_mut().entity_mut(v).insert((RuleClass("player".into()), Invulnerable));
    press(&mut app, a, "light");
    step(&mut app, 12);
    assert_eq!(hp(&app, v), 100.0);
}

#[test]
fn incoming_comes_before_the_flat_part_of_armor() {
    let base = swing_with(None, None, None, false, "technique.7", |app, _, v| {
        app.world_mut().entity_mut(v).insert(Armor { percent: 0.0, flat: 2.0 });
    });
    assert_dealt(&base, "hit", 8.0);
    // 10 × 0.5 − 2 = 3, not (10 − 2) × 0.5 = 4.
    let ward = swing_with(Some("ward"), None, Some("player"), false, "technique.7", |app, _, v| {
        app.world_mut().entity_mut(v).insert(Armor { percent: 0.0, flat: 2.0 });
    });
    assert_dealt(&ward, "hit", 3.0);
}

/// A hook that rebuilds damage from `base`, folding in what a game folds in: the incoming factor, once.
fn rebuild(ctx: &mut DamageCtx, _: &World) {
    ctx.damage = ctx.base * 3.0 * ctx.rules;
}

#[test]
fn a_hook_that_rebuilds_from_base_folds_the_incoming_factor_once() {
    let hooked = |mode: Option<&str>, class: Option<&str>| {
        let (mut app, a, v) = bout_with("r_victim", |app| {
            watch(app);
            app.add_damage_hook(rebuild);
        });
        if let Some(mode) = mode {
            play(&mut app, mode);
        }
        if let Some(class) = class {
            app.world_mut().entity_mut(v).insert(RuleClass(class.to_owned()));
        }
        press(&mut app, a, "technique.7");
        step(&mut app, 12);
        app.world().resource::<Dealt>().0.clone()
    };
    assert_eq!(hooked(None, None), [("hit", 30.0)]);
    assert_eq!(hooked(Some("ward"), Some("player")), [("hit", 15.0)]);
    assert_eq!(hooked(Some("ward"), Some("brute")), [("hit", 30.0)]);
}

// --- outgoing ------------------------------------------------------------------------------------

#[test]
fn outgoing_scales_health_and_poise_of_a_hit_a_block_and_a_guard_break() {
    for (guarding, input, kind) in [(false, "light", "hit"), (true, "light", "block"), (true, "technique.6", "break")] {
        let base = swing(None, None, None, guarding, input);
        assert_eq!(base.dealt[0].0, kind, "{base:?}");
        assert!(base.dealt[0].1 > 0.0, "{base:?}");
        let power = swing(Some("power"), Some("brute"), None, guarding, input);
        assert_dealt(&power, kind, base.dealt[0].1 * 1.5);
        assert!(near(power.poise, base.poise * 1.5), "{kind} poise: {power:?} {base:?}");
        assert!(near(power.stamina, base.stamina), "{kind}: the guard's cost is the hit's, unscaled: {power:?} {base:?}");
        // The class that is not ruled, and the mode that does not rule it, deal what they dealt.
        let other = swing(Some("power"), Some("player"), None, guarding, input);
        assert_dealt(&other, kind, base.dealt[0].1);
        let plain = swing(Some("plain"), Some("brute"), None, guarding, input);
        assert_dealt(&plain, kind, base.dealt[0].1);
        assert!(near(plain.poise, base.poise), "{plain:?}");
    }
}

#[test]
fn outgoing_adds_nothing_to_a_parry_an_evade_or_an_ignored_hit() {
    // A parry: the stamina it costs is the guard's.
    let run = |mode: Option<&str>, class: Option<&str>, setup: fn(&mut App, Entity, Entity), tick: u64, wait: u64| {
        let (mut app, a, v) = bout_with("r_victim", watch);
        if let Some(mode) = mode {
            play(&mut app, mode);
        }
        if let Some(class) = class {
            app.world_mut().entity_mut(a).insert(RuleClass(class.to_owned()));
        }
        setup(&mut app, a, v);
        step(&mut app, wait);
        press(&mut app, a, "heavy");
        step(&mut app, tick);
        (app.world().resource::<Dealt>().0.clone(), hp(&app, v), stamina(&app, v))
    };
    let guard = |app: &mut App, _: Entity, v: Entity| hold(app, v, "block", true);
    let base = run(None, None, guard, 8, 0);
    assert_eq!(base.0, [("parry", 0.0)]);
    assert_eq!(run(Some("power"), Some("brute"), guard, 8, 0), base);
    let god = |app: &mut App, _: Entity, v: Entity| {
        app.world_mut().entity_mut(v).insert(Invulnerable);
    };
    let ignored = run(Some("power"), Some("brute"), god, 8, 0);
    assert_eq!((ignored.0.len(), ignored.1), (0, 100.0), "{ignored:?}");
}

#[test]
fn outgoing_is_times_the_session_row() {
    let hit = |players: u8| {
        let (mut app, a, v) = bout_with("r_victim", watch);
        play(&mut app, "power");
        app.world_mut().entity_mut(a).insert(RuleClass("brute".into()));
        for n in 0..players {
            app.world_mut().spawn(PlayerId(n));
        }
        step(&mut app, 1); // the rules count the session
        press(&mut app, a, "light");
        step(&mut app, 12);
        let _ = v;
        app.world().resource::<Dealt>().0.clone()
    };
    assert_eq!(hit(1), [("hit", 15.0)], "a session of one: 1.5 × 1");
    assert_eq!(hit(2), [("hit", 30.0)], "a session of two: 1.5 × 2");
}

/// Names the third body as the credit (as a game's hook does for an object's owner).
#[derive(Resource)]
struct Third(Entity);

fn credit_third(ctx: &mut DamageCtx, world: &World) {
    ctx.credit = world.resource::<Third>().0;
}

/// Rebuilds the damage from `base`, as a game's formula does: no hook folds the outgoing factor.
fn rebuild_plain(ctx: &mut DamageCtx, _: &World) {
    ctx.damage = ctx.base;
}

#[test]
fn outgoing_follows_the_credit_a_hook_names_and_no_hook_folds_it() {
    let (mut app, a, v) = bout_with("r_victim", |app| {
        watch(app);
        app.add_damage_hook(credit_third);
        app.add_damage_hook(rebuild_plain);
    });
    play(&mut app, "power");
    // The attacker has no class; the body the hook credits is a brute.
    let third = fighter(app.world_mut(), "Third", "dummy", "heroes", Vec3::new(8.0, 0.0, 0.0), N);
    app.world_mut().entity_mut(third).insert(RuleClass("brute".into()));
    app.world_mut().insert_resource(Third(third));
    press(&mut app, a, "technique.7");
    step(&mut app, 12);
    assert_eq!(app.world().resource::<Dealt>().0, [("hit", 15.0)], "the credit's factor, once: 10 × 1.5");
    assert_eq!(hp(&app, v), 85.0);
    // And the attacker's own class does not count when the credit is someone else's.
    let (mut app, a, v) = bout_with("r_victim", |app| {
        watch(app);
        app.add_damage_hook(credit_third);
    });
    play(&mut app, "power");
    let third = fighter(app.world_mut(), "Third", "dummy", "heroes", Vec3::new(8.0, 0.0, 0.0), N);
    app.world_mut().insert_resource(Third(third));
    app.world_mut().entity_mut(a).insert(RuleClass("brute".into()));
    press(&mut app, a, "technique.7");
    step(&mut app, 12);
    assert_eq!(app.world().resource::<Dealt>().0, [("hit", 10.0)]);
    assert_eq!(hp(&app, v), 90.0);
}

// --- combat objects ------------------------------------------------------------------------------

#[test]
fn a_combat_objects_damage_is_ruled_by_its_owners_class() {
    let bolt = |mode: Option<&str>, owner: Option<&str>| {
        let mut app = app_with(watch);
        if let Some(mode) = mode {
            play(&mut app, mode);
        }
        let hero = hero(app.world_mut(), "hero", Vec3::ZERO, N);
        let target = brute(app.world_mut(), Vec3::new(0.0, 0.0, -3.0), S);
        if let Some(owner) = owner {
            app.world_mut().entity_mut(hero).insert(RuleClass(owner.to_owned()));
        }
        let def: SpawnDef = ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(r#"(prefab: ["t_bolt"], action: ["t_fly_plain"], at: (forward: 0.5, up: 1))"#)
            .unwrap();
        spawn_objects(app.world_mut(), hero, "test", &def, None);
        step(&mut app, 20);
        (app.world().resource::<Dealt>().0.clone(), hp(&app, target))
    };
    let (base, hp_base) = bolt(None, None);
    assert_eq!(base.len(), 1, "{base:?}");
    assert!(base[0].1 > 0.0 && near(hp_base, 100.0 - base[0].1), "{base:?} {hp_base}");
    let (ruled, hp_ruled) = bolt(Some("power"), Some("brute"));
    assert_eq!(ruled.len(), 1, "{ruled:?}");
    assert!(near(ruled[0].1, base[0].1 * 1.5), "the owner's class rules the bolt's damage: {ruled:?} {base:?}");
    assert!(near(hp_ruled, 100.0 - base[0].1 * 1.5), "{hp_ruled}");
    // The bolt has no class of its own; an owner of another class, or of none, deals the base.
    assert!(near(bolt(Some("power"), Some("player")).0[0].1, base[0].1));
    assert!(near(bolt(Some("power"), None).0[0].1, base[0].1));
}

// --- damage that is not a hit --------------------------------------------------------------------

#[test]
fn deal_damage_is_never_scaled_and_deal_ruled_damage_takes_the_incoming_factor() {
    let (mut app, _, v) = bout_with("r_victim", |_| {});
    play(&mut app, "ward");
    app.world_mut().entity_mut(v).insert(RuleClass("player".into()));
    let left = deal_damage(app.world_mut(), v, 10.0, None);
    assert_eq!(left, 90.0, "a pit deals what it says");
    let left = deal_ruled_damage(app.world_mut(), v, 10.0, None);
    assert_eq!(left, 85.0, "a burn tick: 10 × 0.5");
    // A body of no class, and a mode without a factor, take it as it is.
    app.world_mut().entity_mut(v).remove::<RuleClass>();
    assert_eq!(deal_ruled_damage(app.world_mut(), v, 10.0, None), 75.0);
    app.world_mut().entity_mut(v).insert(RuleClass("player".into()));
    app.world_mut().resource_mut::<ActiveRules>().mode = "plain".into();
    assert_eq!(deal_ruled_damage(app.world_mut(), v, 10.0, None), 65.0);
    // The outgoing factor is not applied by it: a burn's power is captured where it starts.
    app.world_mut().resource_mut::<ActiveRules>().mode = "power".into();
    app.world_mut().entity_mut(v).insert(RuleClass("brute".into()));
    assert_eq!(deal_ruled_damage(app.world_mut(), v, 10.0, None), 55.0);
    // A kill volume must kill in every mode.
    app.world_mut().resource_mut::<ActiveRules>().mode = "ward".into();
    app.world_mut().entity_mut(v).insert(RuleClass("player".into()));
    deal_damage(app.world_mut(), v, 55.0, None);
    assert!(app.world().get::<Dead>(v).is_some());
}

#[test]
fn without_rules_content_every_factor_is_one() {
    let (mut app, _, v) = bout_with("r_victim", |_| {});
    app.world_mut().entity_mut(v).insert(RuleClass("player".into()));
    assert!(app.world().get_resource::<ActiveRules>().is_none());
    assert_eq!(deal_ruled_damage(app.world_mut(), v, 10.0, None), 90.0);
    assert_eq!(shards_rules::incoming(app.world(), v), 1.0);
    assert_eq!(shards_rules::outgoing(app.world(), v), 1.0);
}
