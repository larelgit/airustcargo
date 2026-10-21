//! Particles on the headless engine counted as drawn: the pools made once from content, what a
//! play, an event, a cue and `PlayEffect` start, and the clock they live by (frozen by a pause,
//! moved on by `step`).

use serde_json::json;
use shards::action::AnimationCue;
use shards::bevy::mesh::MeshTag;
use shards::fx::{play_with, EffectAt, FxParticle, Particle, Particles, PlayEffect, PlayOptions};

use crate::*;

#[test]
fn every_effect_has_its_pool_of_hidden_meshes_from_the_start() {
    let assets = Folder::new("pools");
    let mut app = drawn(&assets);
    app.update();
    assert_eq!(meshes(&mut app, "spark"), (32, 0));
    assert_eq!(meshes(&mut app, "dust"), (24, 0));
    assert_eq!(meshes(&mut app, "glow"), (8, 0));
    // One mesh and one material for the whole pool: one batch.
    let world = app.world_mut();
    let mut query = world.query::<(&FxParticle, &Mesh3d, &MeshMaterial3d<shards::fx::ParticleMaterial>)>();
    let spark: Vec<_> = query.iter(world).filter(|(p, ..)| p.effect == "spark").map(|(_, m, mat)| (m.0.id(), mat.0.id())).collect();
    assert!(spark.windows(2).all(|w| w[0] == w[1]), "one mesh, one material");
    // Updates make no more.
    for _ in 0..3 {
        app.update();
    }
    assert_eq!(meshes(&mut app, "spark"), (32, 0));
    assert!(ok(&mut app, "effects", json!({})).starts_with("3 effects · 0 particles alive in pools of 64 · telegraph decals on, on the ground: 0\n"));
}

#[test]
fn an_effect_the_check_refuses_still_plays_within_the_limits() {
    let assets = Folder::new("limits");
    // Typos: a pool and a burst of four billion, five billion a second, a life of 30 years. The
    // check names them, and the first load uses the definition anyway.
    assets.put(
        "content/fx/typo.effect.ron",
        r#"{"typo": (count: 4000000000, rate: 5e9, duration: 1.0, lifetime: (0.5, 1e9), max_live: 4000000000)}"#,
    );
    let mut app = drawn(&assets);
    app.update();
    let check = text(&shards::testing::call(&mut app, "content_check", json!({})));
    assert!(check.contains("count: at most 1024") && check.contains("max_live: 1 to 4096"), "{check}");
    assert_eq!(meshes(&mut app, "typo"), (4096, 0), "a pool of MAX_LIVE meshes at most");
    assert_eq!(
        ok(&mut app, "effect_play", json!({"effect": "typo", "point": [0, 1, 0]})),
        "played typo at a point (0.0, 1.0, 0.0): 1024 particles (+4096 as it goes on), 1024/4096 alive in its pool"
    );
    // A second on, the rate has emitted all it adds, no more than MAX_LIVE.
    step(&mut app, 60);
    app.update();
    let particles = app.world().resource::<Particles>();
    assert!(particles.emitters.is_empty(), "nothing left to emit");
    let pool = &particles.effects["typo"].pool;
    assert_eq!((pool.size(), pool.started), (4096, 1024 + 4096));
    assert!(pool.slots.iter().flatten().all(|p| p.life <= 30.0), "a particle lives 30 s at most");
}

#[test]
fn a_play_shows_particles_that_live_by_the_simulations_clock() {
    let assets = Folder::new("clock");
    let mut app = drawn(&assets);
    let hero = fighter(app.world_mut(), "hero", "heroes", Vec3::new(2.0, 0.0, 0.0), Vec3::NEG_Z, 100.0);
    app.update();
    let e = id(&app, hero);
    assert_eq!(
        ok(&mut app, "effect_play", json!({"effect": "spark", "at": e, "toward": [1, 0, 0]})),
        format!("played spark at {e} (2.0, 0.0, 0.0): 12 particles, 12/32 alive in its pool")
    );
    app.update();
    assert_eq!(meshes(&mut app, "spark"), (32, 12));
    // Sparks fly the way given (`toward`), from a metre up, coloured from their colour on.
    let world = app.world_mut();
    let mut query = world.query::<(&FxParticle, &Transform, &Visibility, &MeshTag)>();
    let shown: Vec<(Vec3, u32)> = query
        .iter(world)
        .filter(|(p, _, v, _)| p.effect == "spark" && **v != Visibility::Hidden)
        .map(|(_, t, _, tag)| (t.translation, tag.value))
        .collect();
    for (at, tag) in &shown {
        assert!(at.distance(Vec3::new(2.0, 1.0, 0.0)) < 0.05, "just born, where it started: {at}");
        assert_eq!(tag & 0xff, 0xff, "red of #fff2b3: {tag:#x}");
    }
    assert_eq!(status(&app)["live"], 12);

    // Paused, the clock stands: frames go by and the sparks stay where they are.
    let before = shown_positions(&mut app);
    for _ in 0..5 {
        app.update();
    }
    assert_eq!(shown_positions(&mut app), before, "a pause freezes them");

    // A step moves them on (away from the hero, along +x) ...
    step(&mut app, 6);
    app.update();
    let moved = shown_positions(&mut app);
    assert_eq!(moved.len(), 12);
    let mean = moved.iter().copied().sum::<Vec3>() / moved.len() as f32;
    assert!(mean.x > 2.2 && mean.z.abs() < 0.3, "flown along the hit: {mean}");
    // ... and past their life they are gone, their meshes hidden again.
    step(&mut app, 24);
    app.update();
    assert_eq!((live(&app, "spark"), meshes(&mut app, "spark")), (0, (32, 0)));
}

fn shown_positions(app: &mut App) -> Vec<Vec3> {
    let world = app.world_mut();
    let mut query = world.query::<(&FxParticle, &Transform, &Visibility)>();
    let mut out: Vec<(usize, Vec3)> =
        query.iter(world).filter(|(_, _, v)| **v != Visibility::Hidden).map(|(p, t, _)| (p.slot, t.translation)).collect();
    out.sort_by_key(|(slot, _)| *slot);
    out.into_iter().map(|(_, at)| at).collect()
}

#[test]
fn a_pool_never_grows_and_the_oldest_particles_make_room() {
    let assets = Folder::new("full");
    let mut app = drawn(&assets);
    app.update();
    for i in 0..4 {
        let said = ok(&mut app, "effect_play", json!({"effect": "spark", "point": [i as f32, 0.0, 0.0]}));
        assert!(said.ends_with(&format!("{}/32 alive in its pool", (12 * (i + 1)).min(32))), "{said}");
    }
    app.update();
    assert_eq!(meshes(&mut app, "spark"), (32, 32));
    let pool = &app.world().resource::<Particles>().effects["spark"].pool;
    assert_eq!((pool.live(), pool.size()), (32, 32));
    assert_eq!(pool.recycled, 16, "48 started, 16 took the place of older ones");
}

#[test]
fn an_effect_with_a_rate_goes_on_emitting_as_the_clock_runs() {
    let assets = Folder::new("rate");
    let mut app = drawn(&assets);
    app.update();
    assert_eq!(
        ok(&mut app, "effect_play", json!({"effect": "dust", "point": [0, 0, 0]})),
        "played dust at a point (0.0, 0.0, 0.0): 6 particles (+10 as it goes on), 6/24 alive in its pool"
    );
    app.update();
    assert_eq!(live(&app, "dust"), 6);
    // 0.27 s later, 20 a second: five more.
    step(&mut app, 16);
    app.update();
    assert_eq!(live(&app, "dust"), 11);
    // After its half second it has emitted its ten and stops.
    step(&mut app, 16);
    app.update();
    let particles = app.world().resource::<Particles>();
    assert!(particles.emitters.is_empty(), "done emitting");
    assert_eq!(particles.effects["dust"].pool.started, 16);
}

/// Takes the live particles of `effect` out of its pool.
fn take(app: &mut App, effect: &str) -> Vec<Particle> {
    let mut particles = app.world_mut().resource_mut::<Particles>();
    let pool = &mut particles.effects.get_mut(effect).expect("played").pool;
    pool.slots.iter_mut().filter_map(Option::take).collect()
}

#[test]
fn a_cue_at_a_bone_plays_at_that_point_and_its_strength_scales_the_burst() {
    let assets = Folder::new("cue_strength");
    let mut app = drawn(&assets);
    let hero = fighter(app.world_mut(), "hero", "heroes", Vec3::new(2.0, 0.0, 0.0), Vec3::NEG_Z, 100.0);
    app.update();
    // `swoosh` is the table's `glow`: 4 particles of 0.08 to 0.12 m, at the point or the feet.
    let foot = Vec3::new(1.0, 0.25, 3.0);
    let cue = |at: Option<Vec3>, strength: f32| AnimationCue {
        bone: at.map(|_| "foot.L".to_owned()),
        at,
        strength,
        ..AnimationCue::new(hero, "swoosh", "walk")
    };
    let play = |app: &mut App, event: AnimationCue| {
        app.world_mut().trigger(event);
        app.world_mut().flush();
        take(app, "glow")
    };
    let sizes = |burst: &[Particle]| burst.iter().fold((f32::MAX, 0.0f32), |(lo, hi), p| (lo.min(p.size), hi.max(p.size)));

    // A cue at a bone: where the bone is, not the body's feet.
    let full = play(&mut app, cue(Some(foot), 1.0));
    assert_eq!(full.len(), 4);
    assert!(full.iter().all(|p| p.origin == foot), "{:?}", full.iter().map(|p| p.origin).collect::<Vec<_>>());
    let (lo, hi) = sizes(&full);
    assert!(lo >= 0.08 && hi <= 0.12, "as the effect says: {lo} to {hi}");

    // Half the strength: half the particles, half the size.
    let half = play(&mut app, cue(Some(foot), 0.5));
    assert_eq!(half.len(), 2);
    assert!(half.iter().all(|p| p.origin == foot));
    let (lo, hi) = sizes(&half);
    assert!(lo >= 0.04 - 1e-6 && hi <= 0.06 + 1e-6, "{lo} to {hi}");
    // A faint one still shows a particle; a strong one is a bigger burst.
    assert_eq!(play(&mut app, cue(Some(foot), 0.1)).len(), 1);
    let strong = play(&mut app, cue(Some(foot), 2.0));
    assert_eq!(strong.len(), 8);
    assert!(sizes(&strong).0 >= 0.16 - 1e-6);

    // Strength 0 plays nothing, and a strength that is no number neither.
    let started = |app: &App| (app.world().resource::<Particles>().effects["glow"].pool.started, app.world().resource::<Particles>().played);
    let before = started(&app);
    for strength in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        assert!(play(&mut app, cue(Some(foot), strength)).is_empty(), "strength {strength}");
    }
    assert_eq!(started(&app), before, "nothing started and nothing counted");

    // Without a bone the cue is the body's: its feet, at the strength it has.
    let feet = play(&mut app, cue(None, 0.5));
    assert_eq!(feet.len(), 2);
    assert!(feet.iter().all(|p| p.origin == Vec3::new(2.0, 0.0, 0.0)), "{:?}", feet.iter().map(|p| p.origin).collect::<Vec<_>>());

    // An effect with a rate scales what it emits later too: 6 + 10 at full strength, 3 + 5 at half,
    // each as big as half, over the same half second (10 a second, not 20).
    let played = shards::fx::play_scaled(app.world_mut(), "dust", EffectAt::Point(foot), None, 0.5).unwrap();
    assert_eq!((played.particles, played.later), (3, 5));
    step(&mut app, 16);
    app.update();
    let particles = app.world().resource::<Particles>();
    assert_eq!((particles.emitters.len(), particles.effects["dust"].pool.started), (1, 5), "0.27 s in: two more, still going");
    step(&mut app, 16);
    app.update();
    let particles = app.world().resource::<Particles>();
    assert!(particles.emitters.is_empty(), "done emitting");
    assert_eq!(particles.effects["dust"].pool.started, 8);
    let dust = take(&mut app, "dust");
    assert_eq!(dust.len(), 8);
    assert!(dust.iter().all(|p| p.size <= 0.06 + 1e-6), "{:?}", dust.iter().map(|p| p.size).collect::<Vec<_>>());
}

#[test]
fn the_tables_events_and_cues_play_their_effects() {
    let assets = Folder::new("events");
    let mut app = drawn(&assets);
    let hero = fighter(app.world_mut(), "hero", "heroes", Vec3::ZERO, Vec3::NEG_Z, 100.0);
    let brute = fighter(app.world_mut(), "brute", "monsters", Vec3::new(0.0, 0.0, -1.5), Vec3::Z, 20.0);
    app.update();
    // The hero's swing kills the brute: a spark where it was hit, dust where it fell.
    press(&mut app, hero, "light");
    step(&mut app, 30);
    assert!(app.world().get::<Health>(brute).is_none_or(|h| h.current <= 0.0));
    assert_eq!((live(&app, "spark"), live(&app, "dust") >= 6), (12, true));
    // The sparks burst from the brute's side that faced the hero, back out at the hero (+z).
    let sparks: Vec<shards::fx::Particle> =
        app.world().resource::<Particles>().effects["spark"].pool.slots.iter().flatten().cloned().collect();
    let n = sparks.len() as f32;
    let (origin, velocity) = sparks.iter().fold((Vec3::ZERO, Vec3::ZERO), |(o, v), p| (o + p.origin / n, v + p.velocity / n));
    assert!((-1.45..-0.9).contains(&origin.z) && (origin.y - 1.0).abs() < 0.05, "on the brute's near side, a metre up: {origin}");
    assert!(velocity.z > 1.0, "back at the hero: {velocity}");
    // The heal tool's `Healed` glows; so does a clip's `swoosh`.
    let e = id(&app, hero);
    ok(&mut app, "heal", json!({"id": e}));
    assert_eq!(live(&app, "glow"), 4);
    app.world_mut().trigger(AnimationCue::new(hero, "swoosh", "slash"));
    app.world_mut().flush();
    assert_eq!(live(&app, "glow"), 8);
    // A game's own play; one of an effect no content defines is journaled once.
    app.world_mut().trigger(PlayEffect::new("glow", EffectAt::Point(Vec3::ONE)));
    app.world_mut().flush();
    assert_eq!(app.world().resource::<Particles>().effects["glow"].pool.started, 12);
    for _ in 0..2 {
        app.world_mut().trigger(PlayEffect::new("glwo", EffectAt::Entity(hero)));
        app.world_mut().flush();
    }
    let journal = ok(&mut app, "journal", json!({}));
    let warned: Vec<&str> = journal.lines().filter(|l| l.contains("fx: PlayEffect: no effect `glwo` (did you mean `glow`?)")).collect();
    assert_eq!(warned.len(), 1, "{journal}");
}

/// A fan of chips that answers a blow's strength, a wake that a run pushes, a shockwave.
const RESPONSES: &str = r##"{
    "fan": (count: 12, speed: (2.0, 2.0), size: (0.1, 0.1), shape: Cube, unlit: true, max_live: 64,
        strength: (count_per: 6.0, speed: true, size: true)),
    "wake": (count: 4, rate: 20.0, duration: 0.5, speed: (0.0, 0.0), push: (0.6, 1.0), swell: 1.0, max_live: 32),
    "ring": (count: 1, shape: Disc, speed: (0.0, 0.0), lifetime: (0.3, 0.3), size: (0.7, 0.7), size_end: 4.0,
        size_ease: 2.5, color: "#ffffffb3", unlit: true, offset: (0.0, 0.05, 0.0), strength: (count_per: 0.0), max_live: 8),
}"##;

#[test]
fn a_strength_response_sets_the_number_the_speed_and_the_size_of_a_play() {
    let assets = Folder::new("response");
    assets.put("content/fx/responses.effect.ron", RESPONSES);
    let mut app = drawn(&assets);
    app.update();
    let at = EffectAt::Point(Vec3::ZERO);
    let strong = |strength: f32| PlayOptions { strength, ..PlayOptions::default() };
    // 6 more for each 1 of strength above 1, each faster and bigger with it.
    assert_eq!(play_with(app.world_mut(), "fan", at, strong(1.5)).unwrap().particles, 15);
    let fan = take(&mut app, "fan");
    assert_eq!(fan.len(), 15);
    assert!(fan.iter().all(|p| (p.velocity.length() - 3.0).abs() < 1e-4 && (p.size - 0.15).abs() < 1e-6), "{fan:?}");
    // 6 fewer for each 1 below, slower and smaller.
    assert_eq!(play_with(app.world_mut(), "fan", at, strong(0.6)).unwrap().particles, 9);
    assert!(take(&mut app, "fan").iter().all(|p| (p.velocity.length() - 1.2).abs() < 1e-4 && (p.size - 0.06).abs() < 1e-6));
    // `PlayEffect` takes the strength too.
    app.world_mut().trigger(PlayEffect::new("fan", at).strength(1.8));
    app.world_mut().flush();
    assert_eq!(take(&mut app, "fan").len(), 16, "int(6 + 6 × 1.8)");

    // A shockwave: one disc at any strength, as much bigger; level (no tumble), the size it says.
    assert_eq!(
        ok(&mut app, "effect_play", json!({"effect": "ring", "point": [1, 0, 2], "strength": 1.8})),
        "played ring at a point (1.0, 0.0, 2.0): 1 particles, 1/8 alive in its pool"
    );
    app.update();
    let world = app.world_mut();
    let mut query = world.query::<(&FxParticle, &Transform, &Visibility)>();
    let rings: Vec<Transform> = query.iter(world).filter(|(p, _, v)| p.effect == "ring" && **v != Visibility::Hidden).map(|(_, t, _)| *t).collect();
    assert_eq!(rings.len(), 1);
    assert!(rings[0].translation.abs_diff_eq(Vec3::new(1.0, 0.05, 2.0), 1e-5) && rings[0].rotation == Quat::IDENTITY, "{:?}", rings[0]);
    assert!(rings[0].scale.abs_diff_eq(Vec3::splat(0.7 * 1.8), 1e-5), "{:?}", rings[0].scale);
    assert!(refused(&mut app, "effect_play", json!({"effect": "ring", "point": [0, 0, 0], "strength": -1})).starts_with("strength: "));
}

#[test]
fn a_push_gives_each_particle_its_share_the_burst_and_what_the_rate_adds() {
    let assets = Folder::new("push");
    assets.put("content/fx/responses.effect.ron", RESPONSES);
    let mut app = drawn(&assets);
    app.update();
    let push = Vec3::new(3.0, 0.0, -1.0);
    // `wake` starts still: what moves it is its share (0.6 to 1.0) of the push.
    let shares = |burst: &[Particle]| -> Vec<f32> {
        burst
            .iter()
            .map(|p| {
                assert!(p.velocity.cross(push).length() < 1e-4, "along the push: {}", p.velocity);
                p.velocity.length() / push.length()
            })
            .collect()
    };
    let played = play_with(app.world_mut(), "wake", EffectAt::Point(Vec3::ZERO), PlayOptions { push, ..PlayOptions::default() }).unwrap();
    assert_eq!((played.particles, played.later), (4, 10));
    let burst = shares(&take(&mut app, "wake"));
    assert!(burst.iter().all(|s| (0.6..=1.0).contains(s)), "{burst:?}");
    assert!(burst.windows(2).any(|w| w[0] != w[1]), "scattered: {burst:?}");
    step(&mut app, 32);
    app.update();
    let later = shares(&take(&mut app, "wake"));
    assert_eq!(later.len(), 10, "what the rate adds is pushed too");
    assert!(later.iter().all(|s| (0.6..=1.0).contains(s)), "{later:?}");
    // From `PlayEffect`; a push that is no number pushes nothing.
    app.world_mut().trigger(PlayEffect::new("wake", EffectAt::Point(Vec3::ZERO)).push(Vec3::new(f32::NAN, 0.0, 0.0)));
    app.world_mut().flush();
    assert!(take(&mut app, "wake").iter().all(|p| p.velocity == Vec3::ZERO));
    let said = ok(&mut app, "effect_play", json!({"effect": "wake", "point": [0, 0, 0], "push": [0, 0, 2]}));
    assert!(said.starts_with("played wake at a point (0.0, 0.0, 0.0): 4 particles (+10 as it goes on)"), "{said}");
    assert!(take(&mut app, "wake").iter().all(|p| (1.2..=2.0).contains(&p.velocity.z)));
}

#[test]
fn a_play_without_the_new_fields_starts_the_particles_it_always_did() {
    let assets = Folder::new("legacy");
    let mut app = drawn(&assets);
    app.update();
    let at = Vec3::new(1.0, 0.0, -2.0);
    let played = shards::fx::play_scaled(app.world_mut(), "spark", EffectAt::Point(at), Some(Vec3::X), 0.5).unwrap();
    let burst = take(&mut app, "spark");
    // What a scaled play always did: `round(count × strength)` particles from the pool's stream, each its size times
    // the strength, nothing else drawn.
    let db = app.world().resource::<shards::content::ContentDb>();
    let def = db.get::<shards::fx::EffectDef>("spark").unwrap().clone();
    let mut rng = shards::fx::sim::Rng::new(0x5EED_F00D);
    let aim = shards::fx::sim::aim_of(&def, Some(Vec3::X));
    let expected: Vec<Particle> = (0..6)
        .map(|_| {
            let mut p = shards::fx::sim::spawn(&def, at, aim, burst[0].born, &mut rng);
            p.size *= 0.5;
            p
        })
        .collect();
    assert_eq!((played.particles, burst), (6, expected));
}
