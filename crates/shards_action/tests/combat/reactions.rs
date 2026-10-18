//! Hit outcome, part one (B1): reaction tiers and their actions, armour, protection timers,
//! pushes, poise protection, `ActionEnded`. Content: `assets/content/reactions.*.ron`.
//! The attacker (`r_attacker`) stands at the origin facing north; the victim 2 m north of it.

use bevy::prelude::*;
use shards_action::prelude::*;
use shards_action::{
    end_break, grant_protection, poise_damage, react, reset_poise, ActionAppExt, ActionEnd, ActionEnded, DamageCtx,
    Damaged, Knockback, Protection, Reacted, Reaction,
};

use crate::common::*;

/// The events these tests look at, in the order they were triggered.
#[derive(Resource, Default)]
pub struct Seen {
    pub ended: Vec<(u64, String, String, ActionEnd)>,
    pub reacted: Vec<(u64, u8, String)>,
    pub damaged: Vec<Damaged>,
}

/// An observer recording `Seen`; the tick is the simulation's.
pub fn record(app: &mut App) {
    app.init_resource::<Seen>();
    app.add_observer(|e: On<ActionEnded>, mut seen: ResMut<Seen>, tick: Res<ai_core::SimTick>| {
        let e = e.event();
        seen.ended.push((tick.0, format!("{:?}", e.entity), e.action.clone(), e.end));
    });
    app.add_observer(|e: On<Reacted>, mut seen: ResMut<Seen>, tick: Res<ai_core::SimTick>| {
        seen.reacted.push((tick.0, e.event().tier, e.event().action.clone()));
    });
    app.add_observer(|e: On<Damaged>, mut seen: ResMut<Seen>| seen.damaged.push(e.event().clone()));
}

pub fn seen(app: &App) -> &Seen {
    app.world().resource::<Seen>()
}

/// An attacker and a victim of `victim` moveset, recording events.
pub fn bout(victim: &str) -> (App, Entity, Entity) {
    bout_with(victim, |_| {})
}

pub fn bout_with(victim: &str, setup: impl FnOnce(&mut App)) -> (App, Entity, Entity) {
    let mut app = app_with(|app| {
        record(app);
        setup(app);
    });
    let world = app.world_mut();
    let attacker = fighter(world, "Hero", "r_attacker", "heroes", Vec3::ZERO, N);
    let target = fighter(world, "Dummy", victim, "monsters", Vec3::new(0.0, 0.0, -2.0), S);
    (app, attacker, target)
}

fn protection(app: &App, e: Entity) -> Option<Protection> {
    app.world().get::<Protection>(e).copied()
}

fn poise(app: &App, e: Entity) -> Poise {
    *app.world().get::<Poise>(e).unwrap()
}

#[test]
fn a_hit_without_a_tier_flinches_as_before() {
    let (mut app, attacker, victim) = bout("r_victim");
    press(&mut app, attacker, "technique.7");
    step(&mut app, 8);
    assert_eq!(action(&app, victim), "r_flinch", "the moveset's `hit`, the old rule");
    assert!(lines(&app, "reacts").is_empty(), "{:#?}", journal(&app));
    assert!(protection(&app, victim).is_none() && app.world().get::<Knockback>(victim).is_none());
    assert!(seen(&app).reacted.is_empty());
}

#[test]
fn tiers_force_their_reaction_action() {
    for (input, tier, word, expected) in [
        ("light", 1, "light", "r_react_light"),
        ("heavy", 2, "heavy", "r_react_heavy"),
        ("technique.1", 3, "knockdown", "r_react_down"),
    ] {
        let (mut app, attacker, victim) = bout("r_victim");
        let (a, v) = (id(&app, attacker), id(&app, victim));
        press(&mut app, attacker, input);
        step(&mut app, 8);
        assert_eq!(action(&app, victim), expected, "tier {tier}");
        assert_eq!(lines(&app, "reacts"), [format!("6 {v} reacts {word} ({a} {expected})")]);
        assert_eq!(seen(&app).reacted, [(6, tier, expected.to_owned())]);
        // The hit's event says what the target was put into.
        let damaged = seen(&app).damaged.last().unwrap();
        assert_eq!(damaged.reaction, Some(tier));
    }
}

#[test]
fn armor_stops_the_reaction_but_poise_still_breaks() {
    // Permanent armour 1: a light hit does its damage and its poise, and no more.
    let (mut app, attacker, victim) = bout("r_heavyweight");
    press(&mut app, attacker, "light");
    step(&mut app, 8);
    assert_eq!(action(&app, victim), "idle");
    assert_eq!(hp(&app, victim), 90.0);
    assert_eq!(poise(&app, victim).current, 25.0, "the poise damage still lands");
    assert!(seen(&app).reacted.is_empty());
    // A heavy hit is above that armour.
    step(&mut app, 20);
    press(&mut app, attacker, "heavy");
    step(&mut app, 8);
    assert_eq!(action(&app, victim), "r_react_heavy");

    // A window of armour on the running action, and poise breaking through it.
    let (mut app, attacker, victim) = bout("r_victim");
    press(&mut app, victim, "light");
    press(&mut app, attacker, "light");
    step(&mut app, 8);
    assert_eq!(action(&app, victim), "r_armored", "tier 1 against armour 1 (its windup)");
    assert_eq!(poise(&app, victim).current, 25.0);
    step(&mut app, 20);
    press(&mut app, attacker, "technique.2"); // 100 poise, asks tier 2
    step(&mut app, 8);
    assert_eq!(action(&app, victim), "r_stagger", "the poise broke: the stagger, not the reaction");
    assert!(seen(&app).reacted.is_empty());
    assert_eq!(seen(&app).damaged.last().unwrap().reaction, None, "no reaction when the poise breaks");
}

#[test]
fn armour_three_beats_a_knockdown() {
    let (mut app, attacker, victim) = bout("r_victim");
    press(&mut app, victim, "light");
    step(&mut app, 32); // r_armored: armour 3 from 0.5 s
    press(&mut app, attacker, "technique.1");
    step(&mut app, 8);
    assert_eq!(action(&app, victim), "r_armored", "a knockdown against armour 3");
    assert!(seen(&app).reacted.is_empty());
}

#[test]
fn hyper_armor_still_ignores_poise() {
    let (mut app, attacker, victim) = bout("r_victim");
    press(&mut app, victim, "heavy");
    press(&mut app, attacker, "technique.2");
    step(&mut app, 8);
    assert_eq!(action(&app, victim), "r_hyper");
    assert_eq!(poise(&app, victim).current, 30.0, "poise untouched");
    assert!(seen(&app).reacted.is_empty());
}

#[test]
fn immunity_does_not_stop_a_knockdown() {
    let (mut app, _, victim) = bout("r_victim");
    grant_protection(app.world_mut(), victim, 1.0, 0.0);
    assert!(!react(app.world_mut(), victim, Reaction::hit(2, None, Vec3::Z, 0.0, 0)), "tier 2 is inside the immunity");
    assert!(react(app.world_mut(), victim, Reaction::hit(3, None, Vec3::Z, 0.0, 0)), "tier 3 is not");
    assert_eq!(action(&app, victim), "r_react_down");
}

#[test]
fn immunity_blocks_tiers_one_and_two_but_not_three() {
    // Granted in tick N by a hit's reaction, the 0.2 s act on the checks of N+1..=N+11.
    #[derive(Resource)]
    struct Victim(Entity);
    #[derive(Resource, Default)]
    struct Tries(Vec<(u64, bool)>);
    fn probe(world: &mut World) {
        let tick = ai_core::sim::tick(world);
        let victim = world.resource::<Victim>().0;
        match tick {
            3 => assert!(react(world, victim, Reaction::hit(1, None, Vec3::Z, 0.0, 0))),
            4..=20 => {
                let got = react(world, victim, Reaction::hit(1, None, Vec3::Z, 0.0, 0));
                world.resource_mut::<Tries>().0.push((tick, got));
            }
            _ => {}
        }
    }
    let (mut app, _, victim) = bout_with("r_victim", |app| {
        app.init_resource::<Tries>();
        app.add_systems(FixedUpdate, probe.in_set(ActionSystems::Hits));
    });
    app.insert_resource(Victim(victim));
    step(&mut app, 21);
    let first = app.world().resource::<Tries>().0.iter().find(|t| t.1).map(|t| t.0);
    assert_eq!(first, Some(15), "ticks 4..=14 blocked, 15 reacts: {:?}", app.world().resource::<Tries>().0);
}

#[test]
fn protection_follows_the_end_of_a_reaction() {
    let (mut app, attacker, victim) = bout("r_victim");
    press(&mut app, attacker, "light");
    step(&mut app, 8);
    assert_eq!(action(&app, victim), "r_react_light");
    // The reaction (0.3 s) runs out; the end grants 1 s of protection, counted from that tick.
    step(&mut app, 30);
    let ended = seen(&app).ended.iter().find(|e| e.2 == "r_react_light").cloned().expect("it ended");
    assert_eq!(ended.3, ActionEnd::Finished);
    let end_tick = ended.0;
    let now = ai_core::sim::tick(app.world_mut());
    // `react` with immunity respected, between ticks: protected for 58 more ticks after the end.
    step(&mut app, (end_tick + 58).saturating_sub(now));
    assert!(!react(app.world_mut(), victim, Reaction::hit(1, None, Vec3::Z, 0.0, 0)), "still protected");
    step(&mut app, 1);
    assert!(protection(&app, victim).is_none(), "both timers are out, the component goes");
    assert!(react(app.world_mut(), victim, Reaction::hit(1, None, Vec3::Z, 0.0, 0)), "protection is over");
}

#[test]
fn break_protection_ignores_poise_for_its_time() {
    let (mut app, _, victim) = bout("r_victim");
    assert!(poise_damage(app.world_mut(), victim, 100.0), "the first break");
    assert_eq!(action(&app, victim), "r_stagger");
    // Step until the break runs out.
    let mut ticks = 0;
    while poise(&app, victim).is_broken() {
        step(&mut app, 1);
        ticks += 1;
        assert!(ticks < 100);
    }
    assert!(protection(&app, victim).is_some_and(|p| p.poise > 2.9), "{:?}", protection(&app, victim));
    // The 3 s were granted in the tick the break ended, after that tick's hits and counted down in it:
    // the checks of the next 179 ticks are protected, the 180th is not.
    step(&mut app, 178);
    assert!(!poise_damage(app.world_mut(), victim, 100.0), "protected");
    assert_eq!(poise(&app, victim).current, 30.0, "and untouched");
    step(&mut app, 1);
    assert!(poise_damage(app.world_mut(), victim, 100.0), "poise damage counts again");
}

#[test]
fn knockback_moves_the_body_and_stops() {
    let (mut app, _, victim) = bout("r_victim");
    let start = feet(&app, victim);
    // Push east at 4 m/s, slowing 12 m/s per s: pushed between ticks, it moves from the next tick.
    assert!(react(app.world_mut(), victim, Reaction::forced(2, None, E)));
    assert_eq!(app.world().get::<Knockback>(victim).unwrap().velocity, E * 4.0);
    let dt = app.world().resource::<Time<Fixed>>().timestep().as_secs_f32();
    let (mut v, mut expected) = (4.0_f32, 0.0_f32);
    while v > 1e-4 {
        v = (v - 12.0 * dt).max(0.0);
        expected += v * dt;
    }
    step(&mut app, 40);
    let moved = feet(&app, victim) - start;
    assert!((moved.x - expected).abs() < 0.01, "moved {} m, the Euler sum is {expected}", moved.x);
    assert!(moved.z.abs() < 1e-3, "pushed along the line only: {moved}");
    assert!((expected - (16.0 / 24.0 - 4.0 * dt / 2.0)).abs() < 0.03, "about v0^2 / (2 decel) - v0 dt / 2");
    assert!(app.world().get::<Knockback>(victim).is_none(), "it stopped, the component went");
    // The first tick moves by the decayed speed.
    let (mut app, _, victim) = bout("r_victim");
    let start = feet(&app, victim);
    react(app.world_mut(), victim, Reaction::forced(2, None, E));
    step(&mut app, 1);
    let first = feet(&app, victim).x - start.x;
    assert!((first - (4.0 - 12.0 * dt) * dt).abs() < 1e-3, "{first}");
}

#[test]
fn hook_raised_armor_stops_the_reaction() {
    fn brace(ctx: &mut DamageCtx, _: &World) {
        ctx.armor = ctx.armor.max(2);
    }
    let (mut app, attacker, victim) = bout_with("r_victim", |app| {
        app.add_damage_hook(brace);
    });
    press(&mut app, attacker, "heavy");
    step(&mut app, 8);
    assert_eq!(action(&app, victim), "idle", "tier 2 against a hook's armour 2");
    assert_eq!(hp(&app, victim), 60.0);
    step(&mut app, 20);
    press(&mut app, attacker, "technique.1");
    step(&mut app, 8);
    assert_eq!(action(&app, victim), "r_react_down", "a knockdown is above it");
}

#[test]
fn forced_reaction_without_immunity_leaves_protection() {
    let (mut app, _, victim) = bout("r_victim");
    grant_protection(app.world_mut(), victim, 1.0, 0.0);
    assert!(!react(app.world_mut(), victim, Reaction::hit(1, None, Vec3::Z, 0.0, 0)), "inside the immunity");
    assert!(react(app.world_mut(), victim, Reaction::recoil(1, None, Vec3::Z)), "a recoil ignores it");
    assert_eq!(protection(&app, victim), Some(Protection { reactions: 1.0, poise: 0.0 }), "and starts none");
    // A fresh victim: a forced reaction grants nothing.
    let (mut app, _, victim) = bout("r_victim");
    assert!(react(app.world_mut(), victim, Reaction::forced(1, None, Vec3::Z)));
    assert!(protection(&app, victim).is_none());
}

#[test]
fn a_poise_break_removes_the_push() {
    let (mut app, _, victim) = bout("r_victim");
    assert!(react(app.world_mut(), victim, Reaction::forced(2, None, E)));
    assert!(app.world().get::<Knockback>(victim).is_some());
    assert!(poise_damage(app.world_mut(), victim, 100.0));
    assert!(app.world().get::<Knockback>(victim).is_none());
    assert_eq!(action(&app, victim), "r_stagger");
}

#[test]
fn reset_poise_clears_break_and_protection() {
    let (mut app, _, victim) = bout("r_victim");
    assert!(poise_damage(app.world_mut(), victim, 100.0));
    grant_protection(app.world_mut(), victim, 0.0, 3.0);
    reset_poise(app.world_mut(), victim);
    let p = poise(&app, victim);
    assert!(!p.is_broken() && p.current == p.max);
    assert!(protection(&app, victim).is_none(), "no poise protection left");
    // The reaction timer stays when there is one.
    grant_protection(app.world_mut(), victim, 0.5, 3.0);
    reset_poise(app.world_mut(), victim);
    assert_eq!(protection(&app, victim), Some(Protection { reactions: 0.5, poise: 0.0 }));
    // end_break ends a break and grants the moveset's protection; a standing poise is left alone.
    let (mut app, _, victim) = bout("r_victim");
    end_break(app.world_mut(), victim);
    assert!(protection(&app, victim).is_none(), "no break, nothing granted");
    assert!(poise_damage(app.world_mut(), victim, 100.0));
    end_break(app.world_mut(), victim);
    assert!(!poise(&app, victim).is_broken());
    assert_eq!(protection(&app, victim), Some(Protection { reactions: 0.0, poise: 3.0 }));
}

#[test]
fn a_reaction_without_reactions_flinches() {
    let (mut app, attacker, victim) = bout("r_plain");
    press(&mut app, attacker, "heavy"); // asks tier 2, knockback 3
    step(&mut app, 8);
    assert_eq!(action(&app, victim), "r_flinch", "the old flinch, whatever the tier");
    assert!(app.world().get::<Knockback>(victim).is_none() && protection(&app, victim).is_none());
    assert!(seen(&app).reacted.is_empty(), "no Reacted for the old flinch");
    // An armoured one-hit-wonder: the rule still applies to the flinch (a hook's armour).
    let (mut app, _, victim) = bout("r_plain");
    assert!(!react(app.world_mut(), victim, Reaction::hit(1, None, Vec3::Z, 0.0, 1)));
    assert_eq!(action(&app, victim), "idle", "tier 1 against armour 1");
}

#[test]
fn action_ended_order_is_pinned() {
    // One tick, two ends: a hit forces B out of its action (triggered at once, in `Hits`), and C's
    // action runs out (triggered through `Commands`, applied after `finish_actions`).
    let (mut app, attacker, b) = bout("r_victim");
    let c = fighter(app.world_mut(), "Dummy", "r_victim", "monsters", Vec3::new(20.0, 0.0, 0.0), S);
    press(&mut app, b, "light"); // r_armored
    press(&mut app, c, "technique.1"); // r_blink: ends in tick 6
    press(&mut app, attacker, "heavy"); // hits B in tick 6
    step(&mut app, 8);
    let order: Vec<(u64, String, ActionEnd)> =
        seen(&app).ended.iter().filter(|e| e.0 == 6).map(|e| (e.0, e.2.clone(), e.3)).collect();
    assert_eq!(
        order,
        [(6, "r_armored".into(), ActionEnd::Interrupted), (6, "r_blink".into(), ActionEnd::Finished)],
        "the forced end first, the finished one at the next sync point: {:?}",
        seen(&app).ended
    );
}

#[test]
fn the_react_tool_and_condition_and_the_describers() {
    let (mut app, _, victim) = bout("r_victim");
    let v = id(&app, victim);
    assert_eq!(ok(&mut app, "react", serde_json::json!({"id": v, "tier": 2})), format!("ok: {v} reacts heavy"));
    assert_eq!(action(&app, victim), "r_react_heavy");
    assert_eq!(lines(&app, "reacts"), [format!("0 {v} reacts heavy")]);
    // Backwards from where it faces (south: it looks at the attacker, so the push goes north).
    assert!(app.world().get::<Knockback>(victim).is_some_and(|k| k.velocity.z < 0.0));
    let observed = ok(&mut app, "inspect", serde_json::json!({"id": v}));
    assert!(observed.contains("pushed 4.0m/s"), "{observed}");
    let bad = fails(&mut app, "react", serde_json::json!({"id": v, "tier": 4}));
    assert!(bad.contains("use 1 (light), 2 (heavy) or 3 (knockdown)"), "{bad}");
    // A reaction to a protected body: the dev tool forces it through.
    grant_protection(app.world_mut(), victim, 1.0, 2.0);
    let observed = ok(&mut app, "inspect", serde_json::json!({"id": v}));
    assert!(observed.contains("protected 1.00s") && observed.contains("poise protected 2.00s"), "{observed}");
    // `step --until named:reacting`.
    let (mut app, attacker, _) = bout("r_victim");
    press(&mut app, attacker, "light");
    let out = ok(&mut app, "step", serde_json::json!({"ticks": 30, "until": [{"named": "reacting"}]}));
    assert!(out.contains("reacting"), "{out}");
}
