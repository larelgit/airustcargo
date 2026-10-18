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
            let at = center + Vec3::new(ops::cos(angle), 0.0, ops::sin(angle)) * 6.0;
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

/// Bolts the casters' `spawn_bolt` effect threw (a game's projectile, here only counted).
static BOLTS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// An attack without hits of its own (a cast whose projectile is an effect) holds its token
/// to its end like any other: one token, one caster at a time.
#[test]
fn a_cast_without_hits_holds_its_token_to_the_end() {
    use shards_action::ActionAppExt;
    let mut app = app_with(|app| {
        app.add_action_effect("spawn_bolt", |_, _| {
            BOLTS.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        });
    });
    let hero = player(app.world_mut(), 0, Vec3::ZERO);
    let casters: Vec<Entity> = (0..3)
        .map(|i| {
            let angle = i as f32 / 3.0 * std::f32::consts::TAU + 0.3;
            let at = Vec3::new(ops::cos(angle), 0.0, ops::sin(angle)) * 6.0;
            let npc = Npc::new("grunt", "grunt").with_plan("caster").with_director("single");
            crate::common::npc(app.world_mut(), "caster", npc, at, (-at).normalize())
        })
        .collect();
    let before = BOLTS.load(std::sync::atomic::Ordering::SeqCst);
    let mut most = 0;
    for t in 0..1200 {
        step(&mut app, 1);
        let casting = casters.iter().filter(|e| action(&app, **e).as_deref() == Some("cast")).count();
        assert!(casting <= 1, "tick {t}: {casting} casting with one token");
        most = most.max(casting);
    }
    assert_eq!(most, 1);
    assert!(BOLTS.load(std::sync::atomic::Ordering::SeqCst) - before >= 5, "the casts threw their bolts");
    let story = journal(&app);
    assert!(story.iter().any(|l| l.contains("returns its attack token after light (0/1)")), "{story:#?}");
    assert!(!story.iter().any(|l| l.contains("attack never started")), "{story:#?}");
    let _ = hero;
}

/// A token on a target the NPC no longer perceives comes back ("target gone"): the NPC never
/// steers or strikes at where that target really is.
#[test]
fn a_token_on_a_forgotten_target_comes_back() {
    use shards_npc::Grant;
    let mut app = app();
    // It looks north (fov 120°): one player in front of it, one behind it, out of sight.
    let grunt = npc(app.world_mut(), "grunt", Npc::new("grunt", "eyes").with_plan("jabber"), Vec3::ZERO, N);
    let behind = player(app.world_mut(), 0, Vec3::new(0.0, 0.0, 5.0));
    let ahead = player(app.world_mut(), 1, Vec3::new(0.0, 0.0, -6.0));
    step(&mut app, 10);
    let (behind_id, ahead_id) = (aid(&app, behind), aid(&app, ahead));
    let perceived = app.world().get::<Perceived>(grunt).unwrap().clone();
    assert!(perceived.get(behind_id).is_none(), "the one behind is not perceived: {perceived:?}");
    assert_eq!(perceived.focus, Some(ahead_id));
    // A token held on the one behind (as after a regrant on a target forgotten since).
    let grant = Grant {
        target: behind_id,
        director: "default".into(),
        since: 0,
        age: 0.0,
        attack: None,
        input: None,
        cost: 0.0,
        tags: Vec::new(),
        cooldown: 0.0,
        pressed: None,
        instance: 0,
        started: false,
    };
    app.world_mut().get_mut::<AttackToken>(grunt).unwrap().held = Some(grant);
    step(&mut app, 1);
    let held = app.world().get::<AttackToken>(grunt).unwrap().held.clone();
    assert_ne!(held.map(|g| g.target), Some(behind_id), "the token on the forgotten target came back");
    let me = id(&app, grunt);
    assert!(lines(&app, &format!("{me} returns its attack token unused: target gone")).len() == 1, "{:#?}", journal(&app));
    let goal = app.world().get::<shards_npc::NpcDrive>(grunt).unwrap().goal;
    assert!(goal.is_none_or(|g| g.z < 0.0), "it does not walk to where the forgotten one stands: {goal:?}");
}

/// One of two players leaves: the limit falls from three tokens to two, and a token beyond
/// it comes back at once unless its attack is under way.
#[test]
fn tokens_beyond_a_lowered_limit_come_back() {
    let mut app = app();
    record_starts(&mut app);
    let hero = player(app.world_mut(), 0, Vec3::ZERO);
    // Far away, unseen: it only counts as a living player (three tokens).
    let other = player(app.world_mut(), 1, Vec3::new(50.0, 0.0, 50.0));
    let grunts = ring(&mut app, 5, Vec3::ZERO, "grunt", "jabber", "default");
    let grants = |app: &App| -> Vec<shards_npc::Grant> {
        grunts.iter().filter_map(|e| app.world().get::<AttackToken>(*e).unwrap().held.clone()).collect()
    };
    let mut ready = false;
    for _ in 0..2400 {
        step(&mut app, 1);
        let held = grants(&app);
        if held.len() == 3 && held.iter().any(|g| !g.in_flight()) {
            ready = true;
            break;
        }
    }
    assert!(ready, "three tokens out, one of them not pressed yet");
    app.world_mut().despawn(other);
    step(&mut app, 1);
    let held = grants(&app);
    assert!(held.len() <= 2 || held.iter().all(|g| g.in_flight()), "the limit is two now: {held:#?}");
    assert!(!lines(&app, "returns its attack token unused: limit lowered").is_empty(), "{:#?}", journal(&app));
    for t in 0..600 {
        step(&mut app, 1);
        let held = grants(&app).len();
        if t > 60 {
            assert!(held <= 2, "tick {t} after: {held} tokens with one player");
        }
        let attacking = grunts.iter().filter(|e| attacking(&app, **e)).count();
        assert!(attacking <= 2, "tick {t} after: {attacking} attacking one player");
    }
    let _ = hero;
}

/// An NPC an agent took over (orders in its `Intent`) is out of the fight: no tokens, no
/// presses, no held inputs, until its orders are done.
#[test]
fn an_npc_under_orders_takes_no_tokens() {
    use shards_core::{Intent, Order, Target};
    let mut app = app();
    let hero = player(app.world_mut(), 0, Vec3::ZERO);
    let grunt = grunt(app.world_mut(), "grunt", "jabber", "default", Vec3::new(0.0, 0.0, -2.0));
    step(&mut app, 30);
    let far = Vec3::new(0.0, 0.0, -40.0);
    app.world_mut().get_mut::<Intent>(grunt).unwrap().orders.push_back(Order::MoveTo { target: Target::Point(far), within: 0.5 });
    let me = id(&app, grunt);
    let given = shards_core::flat(feet(&app, grunt)).length();
    let mut t = 0;
    while t < 900 {
        step(&mut app, 1);
        t += 1;
        if app.world().get::<Intent>(grunt).unwrap().orders.is_empty() {
            break;
        }
        if t > 60 {
            assert!(!has_token(&app, grunt), "tick {t} under orders: it holds a token");
        }
    }
    assert!(t > 120, "the orders lasted: {t} ticks");
    let taken: Vec<String> = lines(&app, &format!("{me} takes an attack token")).into_iter().filter(|l| (30 + 60..30 + t).contains(&tick_of(l))).collect();
    assert!(taken.is_empty(), "{taken:#?}");
    assert!(shards_core::flat(feet(&app, grunt)).length() > given + 10.0, "it walked its orders");
    assert!(app.world().get::<Intent>(grunt).unwrap().held.is_empty());
    let _ = hero;
}

/// The tick of a journal line (`tick text`).
fn tick_of(line: &str) -> u64 {
    line.split_whitespace().next().and_then(|t| t.trim_start_matches('[').trim_end_matches(']').parse().ok()).unwrap_or(0)
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
                    let angle = ops::atan2(at.z, at.x);
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

/// An enemy's chain rides the press queue (B2a): the next link, pressed in the first link's
/// windup, waits for the combo window and starts on the tick after it opens, and the attack token
/// stays with the NPC through both links (the continuation is the same attack). Without a queue
/// span the early press runs out before the window opens, and the chain stops at its first link.
#[test]
fn an_early_chain_press_waits_for_the_combo_window() {
    use shards_core::Intent;
    let chain = |moveset: &str| {
        let mut app = app();
        record_starts(&mut app);
        player(app.world_mut(), 0, Vec3::ZERO);
        let plan = Npc::new("grunt", "grunt").with_plan("linker").with_director("single");
        let e = npc(app.world_mut(), moveset, plan, Vec3::new(0.0, 0.0, -1.5), Vec3::Z);
        let mut waited = 0;
        while action(&app, e).is_none_or(|a| !a.starts_with("link")) {
            step(&mut app, 1);
            waited += 1;
            assert!(waited < 600, "it never attacked: {:#?}", journal(&app));
        }
        // The brain presses the next link at once, as a chain decided at its start does.
        app.world_mut().get_mut::<Intent>(e).unwrap().press("light");
        let mut kept = true;
        for _ in 0..80 {
            step(&mut app, 1);
            if action(&app, e).is_some() {
                kept &= has_token(&app, e);
            }
        }
        let starts: Vec<(u64, String)> =
            app.world().resource::<Started>().0.iter().filter(|s| s.1 == e).map(|s| (s.0, s.2.clone())).collect();
        (starts, kept, lines(&app, "light rejected"))
    };
    let (starts, kept, rejected) = chain("linker");
    assert_eq!(starts.len(), 2, "{starts:?}");
    let (first, second) = (&starts[0], &starts[1]);
    assert_eq!((first.1.as_str(), second.1.as_str()), ("link_1", "link_2"), "{starts:?}");
    assert_eq!(second.0, first.0 + 30, "the window opens at 0.5 s (30 ticks): the link starts on the next tick");
    assert!(kept, "the token stays through the chain");
    assert!(rejected.is_empty(), "{rejected:?}");

    let (starts, _, rejected) = chain("linker_plain");
    assert_eq!(starts.iter().map(|s| s.1.as_str()).collect::<Vec<_>>(), ["link_plain"], "{starts:?}");
    assert_eq!(rejected.len(), 1, "the early press ran out: {rejected:?}");
}
