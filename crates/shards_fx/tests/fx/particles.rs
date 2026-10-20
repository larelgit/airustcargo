//! Particles on the headless engine counted as drawn: the pools made once from content, what a
//! play, an event, a cue and `PlayEffect` start, and the clock they live by (frozen by a pause,
//! moved on by `step`).

use serde_json::json;
use shards::action::AnimationCue;
use shards::bevy::mesh::MeshTag;
use shards::fx::{EffectAt, FxParticle, Particle, Particles, PlayEffect};

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
    // each as big as half.
    let played = shards::fx::play_scaled(app.world_mut(), "dust", EffectAt::Point(foot), None, 0.5).unwrap();
    assert_eq!((played.particles, played.later), (3, 5));
    step(&mut app, 32);
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
