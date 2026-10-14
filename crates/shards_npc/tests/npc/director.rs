//! The combat director over long fights: tokens by player count, the collision budget,
//! allowed combinations, waiting roles. Counted every tick: NPCs whose action hits.

use bevy::prelude::*;
use shards_npc::prelude::*;

use crate::common::*;

/// `n` NPCs on a circle of 6 m around `center`.
fn ring(app: &mut App, n: usize, center: Vec3, behavior: &str, plan: &str, director: &str) -> Vec<Entity> {
    (0..n)
        .map(|i| {
            let angle = i as f32 / n as f32 * std::f32::consts::TAU + 0.3;
            let at = center + Vec3::new(angle.cos(), 0.0, angle.sin()) * 6.0;
            grunt(app.world_mut(), behavior, plan, director, at)
        })
        .collect()
}

/// What a long fight came to.
#[derive(Default, Debug)]
struct Tally {
    /// Most NPCs attacking at once, and at which tick it was first that many.
    most_attacking: usize,
    /// Most tokens held at once.
    most_tokens: usize,
    /// Attack starts per NPC.
    starts: Vec<usize>,
}

/// Runs `ticks` ticks; every tick counts attackers and tokens and calls `check` with them.
/// Every 10 s the first player takes a few steps elsewhere (teleported), so the NPCs chase.
fn fight(app: &mut App, players: &[Entity], npcs: &[Entity], ticks: u64, mut check: impl FnMut(&App, u64, usize, usize)) -> Tally {
    let spots = [Vec3::ZERO, Vec3::new(3.0, 0.0, -2.0), Vec3::new(-2.0, 0.0, 3.0), Vec3::new(1.0, 0.0, 2.0)];
    let mut tally = Tally::default();
    for t in 1..=ticks {
        if t % 600 == 0 {
            let spot = spots[(t / 600) as usize % spots.len()];
            teleport(app, players[0], spot);
        }
        step(app, 1);
        let attacking = npcs.iter().filter(|e| attacking(app, **e)).count();
        let tokens = npcs.iter().filter(|e| has_token(app, **e)).count();
        check(app, t, attacking, tokens);
        tally.most_attacking = tally.most_attacking.max(attacking);
        tally.most_tokens = tally.most_tokens.max(tokens);
    }
    let started = &app.world().resource::<Started>().0;
    tally.starts = npcs.iter().map(|e| started.iter().filter(|(_, who, a)| who == e && a != "grunt_stagger").count()).collect();
    tally
}

/// TZ E6 acceptance: one player, three enemies, a long fight: never more than two attack at
/// the same time (and two do). A minute each, over several seeds.
#[test]
fn one_player_three_enemies_never_more_than_two_attack_at_once() {
    for seed in [7, 11, 23] {
        let mut app = app_seeded(seed, |_| {});
        record_starts(&mut app);
        let hero = player(app.world_mut(), 0, Vec3::ZERO);
        let grunts = ring(&mut app, 3, Vec3::ZERO, "grunt", "grunt", "default");
        let tally = fight(&mut app, &[hero], &grunts, 3600, |_, t, attacking, tokens| {
            assert!(attacking <= 2, "seed {seed}, tick {t}: {attacking} attacking at once");
            assert!(tokens <= 2, "seed {seed}, tick {t}: {tokens} tokens held");
        });
        assert_eq!(tally.most_attacking, 2, "seed {seed}: {tally:?}");
        // Everyone gets its turns: the longest-waiting is served first.
        assert!(tally.starts.iter().all(|n| *n >= 10), "seed {seed}: {tally:?}");
        let story = journal(&app);
        let hero_id = id(&app, hero);
        assert!(story.iter().any(|l| l.contains(&format!("takes an attack token on {hero_id} (2/2)"))));
        assert!(story.iter().any(|l| l.contains("returns its attack token after light")));
        assert!(story.iter().any(|l| l.contains("returns its attack token after heavy")));
        // The hits land.
        assert!(story.iter().any(|l| l.contains(&format!("-> {hero_id} jab"))), "seed {seed}");
    }
}

#[test]
fn two_players_raise_the_limit_per_data() {
    let mut app = app();
    record_starts(&mut app);
    let a = player(app.world_mut(), 0, Vec3::new(-1.5, 0.0, 0.0));
    let b = player(app.world_mut(), 1, Vec3::new(1.5, 0.0, 0.0));
    let grunts = ring(&mut app, 5, Vec3::ZERO, "grunt", "jabber", "default");
    let tally = fight(&mut app, &[a, b], &grunts, 2400, |_, t, attacking, tokens| {
        assert!(attacking <= 3 && tokens <= 3, "tick {t}: {attacking} attacking, {tokens} tokens");
    });
    assert_eq!(tally.most_attacking, 3, "two players: three tokens ({tally:?})");
}

#[test]
fn the_written_out_states_wait_then_strike() {
    let mut app = app();
    record_starts(&mut app);
    let hero = player(app.world_mut(), 0, Vec3::ZERO);
    let grunts = ring(&mut app, 3, Vec3::ZERO, "patient", "jabber", "default");
    let tally = fight(&mut app, &[hero], &grunts, 1800, |_, t, attacking, _| {
        assert!(attacking <= 2, "tick {t}: {attacking} attacking at once");
    });
    assert_eq!(tally.most_attacking, 2);
    assert!(tally.starts.iter().all(|n| *n >= 5), "{tally:?}");
    let story = journal(&app);
    for change in ["wait → strike", "strike → wait"] {
        assert!(story.iter().any(|l| l.contains(change)), "no `{change}`");
    }
}

#[test]
fn the_collision_budget_blocks_a_third_overlapping_attack() {
    let mut app = app();
    record_starts(&mut app);
    let hero = player(app.world_mut(), 0, Vec3::ZERO);
    // Three tokens, a budget of 2.0, attacks costing 1.0 each.
    let grunts = ring(&mut app, 3, Vec3::ZERO, "grunt", "jabber", "budget");
    let mut blocked = 0;
    let tally = fight(&mut app, &[hero], &grunts, 2400, |_, t, attacking, tokens| {
        assert!(attacking <= 2, "tick {t}: {attacking} attacks on one target with a budget for two");
        if tokens == 3 && attacking == 2 {
            blocked += 1;
        }
    });
    assert_eq!(tally.most_tokens, 3, "three tokens were out: {tally:?}");
    assert_eq!(tally.most_attacking, 2);
    assert!(blocked > 0, "a third token holder waited for the budget");
}

#[test]
fn only_allowed_combinations_overlap() {
    let mut app = app();
    record_starts(&mut app);
    let hero = player(app.world_mut(), 0, Vec3::ZERO);
    // Three tokens; light + light and light + heavy may overlap, heavy + heavy may not.
    let grunts = ring(&mut app, 3, Vec3::ZERO, "grunt", "mixed", "combos");
    let (mut light_heavy, mut light_light) = (0, 0);
    fight(&mut app, &[hero], &grunts, 3600, |app, t, _, _| {
        let running: Vec<String> = grunts.iter().filter(|e| attacking(app, **e)).filter_map(|e| action(app, *e)).collect();
        let heavy = running.iter().filter(|a| *a == "smash").count();
        let light = running.iter().filter(|a| *a == "jab").count();
        assert!(heavy <= 1, "tick {t}: two heavy attacks at once ({running:?})");
        light_heavy += usize::from(heavy == 1 && light >= 1);
        light_light += usize::from(light >= 2);
    });
    assert!(light_heavy > 0 && light_light > 0, "allowed overlaps happen: {light_heavy} light+heavy, {light_light} light+light");
}

#[test]
fn those_without_a_token_wait_in_their_roles() {
    let mut app = app();
    record_starts(&mut app);
    let hero = player(app.world_mut(), 0, Vec3::ZERO);
    // One token; slot 0 circles at 4 m, slot 1 holds within 6 m.
    let grunts = ring(&mut app, 3, Vec3::ZERO, "grunt", "jabber", "single");
    step(&mut app, 240);
    let mut circling_moved = false;
    let mut seen_roles: Vec<String> = Vec::new();
    let mut last_angle: Vec<Option<f32>> = vec![None; grunts.len()];
    for _ in 0..240 {
        step(&mut app, 1);
        for (i, e) in grunts.iter().enumerate() {
            let brain = app.world().get::<NpcBrain>(*e).unwrap().clone();
            if has_token(&app, *e) {
                assert_eq!(brain.role, None, "a token holder has no waiting role");
                last_angle[i] = None;
                continue;
            }
            let Some(role) = brain.role else { continue };
            let at = feet(&app, *e) - feet(&app, hero);
            let d = Vec2::new(at.x, at.z).length();
            match role.as_str() {
                "circle" => {
                    assert!(d < 4.0 + 1.0, "a circler keeps about 4 m: {d:.2}");
                    let angle = at.z.atan2(at.x);
                    if let Some(prev) = last_angle[i]
                        && (angle - prev).abs() > 1e-3
                    {
                        circling_moved = true;
                    }
                    last_angle[i] = Some(angle);
                }
                "hold" => assert!(d < 6.0 + 0.8, "a holder stays within 6 m: {d:.2}"),
                other => panic!("unexpected role {other}"),
            }
            if !seen_roles.contains(&role) {
                seen_roles.push(role);
            }
        }
    }
    seen_roles.sort();
    assert_eq!(seen_roles, ["circle", "hold"]);
    assert!(circling_moved, "circlers go round");
}
