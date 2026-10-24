//! Gore on the headless engine counted as drawn (F9): marks laid on a floor, a wall and a ceiling, the cap
//! that holds at ten times the budget, the links from a shot's impacts, a blow, a death and a part that
//! broke off, chunks that bounce and come to rest on the floor and stay, clearing, and a machine that
//! draws nothing laying nothing. The GPU side (the decals' pixels) is in `offscreen`.

use serde_json::json;
use shards::action::{Damaged, Died, PartBroken, ShotFired, ShotImpact};
use shards::fx::{clear, clear_all, debris_counts, spawn_mark, GoreStats, MarkDecal, Marks, Particles, PlayMark};
use shards::motion::Shape;

use crate::*;

/// Holes, blood that fades, a stain; chunks that bounce and stay, dust that falls through the floor.
const MARKS: &str = r##"{
    "hole": (color: "#202020", size: (0.1, 0.1), max_live: 16, tags: ["gore"]),
    "blood": (color: "#6e0f12", size: (0.3, 0.5), fade_in: 0.2, lifetime: Secs(4.0), fade_out: 1.0, max_live: 8, tags: ["gore", "blood"]),
    "scorch": (color: "#000000", size: (0.4, 0.4), max_live: 4),
}"##;

const EFFECTS: &str = r##"{
    "chunk": (count: 6, collide: true, restitution: 0.4, friction: 0.7, rest: UntilCleared, shape: Cube, lifetime: (4.0, 4.0),
        speed: (2.0, 4.0), spread: 90.0, gravity: -9.8, size: (0.1, 0.1), size_end: 1.0, color: "#c8c0b0", color_end: "#c8c0b0",
        max_live: 16, tags: ["gore"]),
    "scrap": (count: 4, collide: true, shape: Cube, lifetime: (1.0, 1.0), speed: (1.0, 2.0), spread: 60.0, gravity: -9.8,
        size: (0.1, 0.1), max_live: 8),
    "ghost": (count: 4, shape: Cube, lifetime: (3.0, 3.0), speed: (0.0, 0.0), gravity: -9.8, size: (0.1, 0.1), offset: (0.0, 1.0, 0.0), max_live: 8),
    "spark": (count: 3, aim: Surface, speed: (2.0, 2.0), spread: 0.0, lifetime: (0.5, 0.5), max_live: 16),
}"##;

const FX: &str = r#"{"gore": (
    effects: {"shot_impact": "spark", "part_broken": "chunk", "died": "chunk"},
    marks: {
        "shot_impact": [(mark: "hole")],
        "shot_hit": [(mark: "blood", place: Behind(10.0)), (mark: "blood", place: Below(3.0), count: 2, spread: 20.0)],
        "damaged": [(mark: "scorch", place: Below(2.0), offset: (0.0, 1.0, 0.0))],
        "died": [(mark: "blood", place: Below(2.0), offset: (0.0, 0.5, 0.0))],
        "part_broken": [(mark: "blood", place: Behind(10.0), offset: (0.0, 0.2, 0.0))],
    },
)}"#;

/// A drawing game with the gore content, a floor at y = 0 (the base test floor), a wall at x = 5.5 facing -X and
/// a ceiling at y = 4.
fn room(name: &str) -> (App, Folder) {
    let assets = Folder::empty(name);
    assets.put("content/fx/gore.mark.ron", MARKS);
    assets.put("content/fx/gore.effect.ron", EFFECTS);
    assets.put("content/fx/gore.fx.ron", FX);
    let mut app = drawn(&assets);
    wall_and_ceiling(app.world_mut());
    step(&mut app, 2);
    app.update();
    (app, assets)
}

fn wall_and_ceiling(world: &mut World) {
    world.spawn((Collider::new(Shape::cuboid(Vec3::new(0.5, 3.0, 8.0))), Transform::from_xyz(6.0, 3.0, 0.0)));
    world.spawn((Collider::new(Shape::cuboid(Vec3::new(8.0, 0.5, 8.0))), Transform::from_xyz(0.0, 4.5, 0.0)));
}

/// The marks of `kind`, in slot order.
fn marks_of(app: &App, kind: &str) -> Vec<shards::fx::Mark> {
    app.world().resource::<Marks>().kinds.get(kind).map(|p| p.slots.iter().flatten().copied().collect()).unwrap_or_default()
}

/// The decals of `kind` that are shown: their transforms.
fn decals_of(app: &mut App, kind: &str) -> Vec<Transform> {
    let world = app.world_mut();
    let mut query = world.query::<(&MarkDecal, &Transform, &Visibility)>();
    query.iter(world).filter(|(d, _, v)| d.kind == kind && **v != Visibility::Hidden).map(|(_, t, _)| *t).collect()
}

fn body(world: &mut World, at: Vec3) -> Entity {
    world.spawn((Name::new("body"), Health::new(100.0), Transform::from_translation(at))).id()
}

/// Triggers an event and applies the commands its observers queued, as the next sync point of a frame would.
macro_rules! fire {
    ($world:expr, $event:expr) => {{
        $world.trigger($event);
        $world.flush();
    }};
}

fn shot(world: &mut World, shooter: Entity, from: Vec3) {
    fire!(world, ShotFired { shooter, action: "gun".into(), from, dir: Vec3::X, pellets: 1, noise: 0.0 });
}

fn impact(world: &mut World, shooter: Entity, point: Vec3, normal: Vec3, target: Option<Entity>) {
    fire!(world, ShotImpact {
        shooter,
        action: "gun".into(),
        pellet: 0,
        point,
        normal,
        target,
        collider: target.is_none().then_some(Entity::PLACEHOLDER),
        part: None,
    });
}

#[test]
fn a_mark_lies_on_a_floor_a_wall_and_a_ceiling_facing_along_the_surface() {
    let (mut app, _assets) = room("surfaces");
    for (point, normal) in [([1.0, 0.0, 1.0], [0, 1, 0]), ([5.5, 1.5, -1.0], [-1, 0, 0]), ([2.0, 4.0, 0.0], [0, -1, 0])] {
        let said = ok(&mut app, "mark_spawn", json!({"mark": "hole", "point": point, "normal": normal}));
        assert!(said.starts_with("laid hole at ("), "{said}");
    }
    app.update();
    let marks = marks_of(&app, "hole");
    assert_eq!(marks.len(), 3);
    let decals = decals_of(&mut app, "hole");
    assert_eq!(decals.len(), 3, "one decal each, shown");
    for (mark, decal) in marks.iter().zip(&decals) {
        assert!((decal.rotation * Vec3::Y).abs_diff_eq(mark.normal, 1e-4), "the quad faces along {}", mark.normal);
        assert!(decal.translation.abs_diff_eq(mark.point + mark.normal * shards::fx::mark::LIFT, 1e-5));
        assert!((decal.scale.x - 0.1).abs() < 1e-6, "a hole 0.1 m across: {}", decal.scale);
    }
    // A search finds the surface for an agent that does not know its normal: the floor under a point, the wall ahead.
    let floor = ok(&mut app, "mark_spawn", json!({"mark": "hole", "from": [2.0, 2.0, 0.0], "toward": [0, -1, 0], "reach": 5}));
    assert!(floor.contains("at (2.00, 0.00, 0.00) on a surface facing (0.00, 1.00, 0.00)"), "{floor}");
    let wall = ok(&mut app, "mark_spawn", json!({"mark": "hole", "from": [0.0, 1.0, 0.0], "toward": [1, 0, 0]}));
    assert!(wall.contains("at (5.50, 1.00, 0.00) on a surface facing (-1.00, 0.00, 0.00)"), "{wall}");
    assert!(refused(&mut app, "mark_spawn", json!({"mark": "hole", "from": [0, 2, 0], "toward": [0, 1, 0], "reach": 1})).starts_with("no world surface within 1 m"));
    // The ways a call goes wrong say how to make it right.
    assert_eq!(
        refused(&mut app, "mark_spawn", json!({"mark": "hol", "point": [0, 0, 0], "normal": [0, 1, 0]})),
        "no mark `hol` (did you mean `hole`?): content `*.mark.ron` defines marks; 3 known"
    );
    assert!(refused(&mut app, "mark_spawn", json!({"mark": "hole"})).starts_with("give `point` and `normal`, or `from` and `toward`"));
    assert!(refused(&mut app, "mark_spawn", json!({"mark": "hole", "point": [0, 0, 0], "normal": [0, 0, 0]})).contains("normal that point somewhere"));
}

#[test]
fn the_cap_holds_at_ten_times_the_budget_and_the_oldest_make_room() {
    let (mut app, _assets) = room("cap");
    for i in 0..160 {
        spawn_mark(app.world_mut(), "hole", Vec3::new(i as f32 * 0.01, 0.0, 0.0), Vec3::Y).unwrap();
        if i % 40 == 39 {
            step(&mut app, 1);
            app.update();
        }
    }
    app.update();
    let pool = &app.world().resource::<Marks>().kinds["hole"];
    assert_eq!((pool.live(), pool.slots.len(), pool.started, pool.recycled), (16, 16, 160, 144));
    let newest = pool.slots.iter().flatten().map(|m| m.point.x).fold(f32::MAX, f32::min);
    assert!(newest >= 1.44 - 1e-4, "the oldest 144 were recycled, the newest 16 are left: {newest}");
    assert_eq!(decals_of(&mut app, "hole").len(), 16, "sixteen decals shown, no more made");
    let made = app.world_mut().query::<&MarkDecal>().iter(app.world()).filter(|d| d.kind == "hole").count();
    assert_eq!(made, 16, "entities are made as the pool grows, up to its cap");
    assert_eq!(status(&app)["marks"], 16);
    // The same for the others: the cap is per kind.
    for _ in 0..30 {
        spawn_mark(app.world_mut(), "scorch", Vec3::ZERO, Vec3::Y).unwrap();
    }
    assert_eq!(marks_of(&app, "scorch").len(), 4);
    assert_eq!(marks_of(&app, "hole").len(), 16);
}

#[test]
fn a_mark_fades_in_and_out_by_the_simulations_clock_and_is_gone_at_its_end() {
    let (mut app, _assets) = room("fade");
    spawn_mark(app.world_mut(), "blood", Vec3::new(1.0, 0.0, 0.0), Vec3::Y).unwrap();
    app.update();
    let alpha = |app: &mut App| {
        let world = app.world_mut();
        let mut query = world.query::<(&MarkDecal, &MeshMaterial3d<shards::fx::marks_render::MarkMaterial>)>();
        let handle = query.iter(world).next().map(|(_, m)| m.0.clone()).expect("a decal");
        world.resource::<Assets<shards::fx::marks_render::MarkMaterial>>().get(&handle).unwrap().base.base_color.to_srgba().alpha
    };
    assert!(alpha(&mut app) < 0.05, "it has just been laid: {}", alpha(&mut app));
    // A paused clock stands still: frames go by and it does not show.
    for _ in 0..5 {
        app.update();
    }
    assert!(alpha(&mut app) < 0.05);
    step(&mut app, 12);
    app.update();
    assert!((alpha(&mut app) - 1.0).abs() < 0.01, "in full after 0.2 s: {}", alpha(&mut app));
    // Lifetime Secs(4), fading over the last 1 s: half gone at 3.5 s.
    step(&mut app, 198);
    app.update();
    let a = alpha(&mut app);
    assert!((0.4..0.6).contains(&a), "{a}");
    step(&mut app, 40);
    app.update();
    assert_eq!((marks_of(&app, "blood").len(), decals_of(&mut app, "blood").len()), (0, 0));
}

#[test]
fn a_shot_at_a_wall_leaves_a_hole_and_sparks_and_a_shot_through_a_body_blood_behind_it_and_below() {
    let (mut app, _assets) = room("shots");
    let me = body(app.world_mut(), Vec3::new(-1.0, 0.0, 0.0));
    let dummy = body(app.world_mut(), Vec3::new(3.0, 0.0, 0.0));
    // A pellet that stops at the wall: a hole on it, and sparks out along its normal.
    shot(app.world_mut(), me, Vec3::new(-1.0, 1.5, 0.0));
    impact(app.world_mut(), me, Vec3::new(5.5, 1.5, 0.3), Vec3::NEG_X, None);
    let holes = marks_of(&app, "hole");
    assert_eq!(holes.len(), 1, "{}", ok(&mut app, "journal", json!({})));
    assert_eq!((holes[0].point, holes[0].normal), (Vec3::new(5.5, 1.5, 0.3), Vec3::NEG_X));
    let sparks = &app.world().resource::<Particles>().effects["spark"].pool;
    assert_eq!(sparks.live(), 3, "sparks at the point");
    assert!(sparks.slots.iter().flatten().all(|p| p.velocity.normalize().abs_diff_eq(Vec3::NEG_X, 1e-4)), "out along the normal");
    // A pellet through the dummy: blood on the wall behind it, along the shot (the point it hit and the eye it was fired from
    // give the way), and two on the floor under it.
    shot(app.world_mut(), me, Vec3::new(-1.0, 1.5, 0.0));
    impact(app.world_mut(), me, Vec3::new(2.7, 1.1, 0.0), Vec3::NEG_X, Some(dummy));
    let blood = marks_of(&app, "blood");
    assert_eq!(blood.len(), 3);
    let wall = blood.iter().filter(|m| m.normal == Vec3::NEG_X).collect::<Vec<_>>();
    let floor = blood.iter().filter(|m| m.normal == Vec3::Y).collect::<Vec<_>>();
    assert_eq!((wall.len(), floor.len()), (1, 2), "one behind, two below");
    assert!((wall[0].point.x - 5.5).abs() < 1e-3, "on the wall: {}", wall[0].point);
    // The shot went down 0.4 m over 3.7 m: it reaches the wall 0.3 m below where it hit the body.
    assert!(wall[0].point.y < 1.1 && wall[0].point.y > 0.6, "{}", wall[0].point);
    for m in floor {
        assert!(m.point.y.abs() < 1e-3 && (m.point.x - 2.7).abs() < 1.0, "on the floor under the hit: {}", m.point);
    }
    // The searches are counted for the frame that follows.
    app.update();
    let stats = *app.world().resource::<GoreStats>();
    assert_eq!(stats.rays, 3, "three searches for a surface");
    assert_eq!(status(&app)["rays"]["last"], 3);
    // A pellet that hit the dummy with nobody's `ShotFired` before it: the way is back along the normal; nothing breaks.
    let stranger = body(app.world_mut(), Vec3::new(0.0, 0.0, 2.0));
    impact(app.world_mut(), stranger, Vec3::new(2.7, 1.1, 0.0), Vec3::NEG_X, Some(dummy));
    assert_eq!(marks_of(&app, "blood").len(), 6);
}

#[test]
fn a_blow_and_a_death_lay_marks_and_an_ignored_blow_none() {
    let (mut app, _assets) = room("blows");
    let attacker = body(app.world_mut(), Vec3::new(0.0, 0.0, 0.0));
    let victim = body(app.world_mut(), Vec3::new(3.0, 0.0, 0.0));
    let hit = |ignored: bool| Damaged {
        attacker,
        target: victim,
        action: "swing".into(),
        amount: 10.0,
        poise: 0.0,
        hp_left: 90.0,
        invulnerable: ignored,
        ignore_reason: None,
        weight: Default::default(),
        reaction: None,
        absorbed: 0.0,
        source: None,
        part: None,
    };
    fire!(app.world_mut(), hit(true));
    assert_eq!(marks_of(&app, "scorch").len(), 0, "god mode hurts nobody and bleeds nothing");
    fire!(app.world_mut(), hit(false));
    let scorch = marks_of(&app, "scorch");
    assert_eq!((scorch.len(), scorch[0].normal), (1, Vec3::Y));
    assert!(scorch[0].point.abs_diff_eq(Vec3::new(3.0, 0.0, 0.0), 1e-3), "on the floor under the one hit: {}", scorch[0].point);
    // A death: a stain under the body and chunks from it.
    fire!(app.world_mut(), Died { entity: victim, killer: Some(attacker), action: None, source: None });
    assert_eq!(marks_of(&app, "blood").len(), 1);
    assert_eq!(app.world().resource::<Particles>().effects["chunk"].pool.live(), 6);
}

#[test]
fn chunks_of_a_part_that_broke_bounce_and_come_to_rest_on_the_floor_and_stay() {
    let (mut app, _assets) = room("chunks");
    let me = body(app.world_mut(), Vec3::new(-1.0, 0.0, 0.0));
    let target = body(app.world_mut(), Vec3::new(2.0, 0.0, 0.0));
    let part = app.world_mut().spawn(Transform::from_xyz(2.0, 1.5, 0.0)).id();
    fire!(app.world_mut(), PartBroken { body: target, part, id: "head".into(), by: Some(me) });
    // Six chunks and a stain on the wall behind (away from the shooter).
    assert_eq!(app.world().resource::<Particles>().effects["chunk"].pool.live(), 6);
    assert_eq!(marks_of(&app, "blood").len(), 1);
    app.update();
    // They fly: not at rest at once, not yet below their start.
    assert_eq!(debris_counts(app.world()), (0, 6));
    step(&mut app, 30);
    app.update();
    let (resting, flying) = debris_counts(app.world());
    assert!(resting <= 6 && resting + flying == 6);
    // Four seconds of flight at most; then they lie on the floor: a cube 0.1 m across, its middle 0.05 m up, and they stay.
    step(&mut app, 240);
    app.update();
    assert_eq!(debris_counts(app.world()), (6, 0), "all at rest");
    let lying = |app: &App| -> Vec<Vec3> {
        let pool = &app.world().resource::<Particles>().effects["chunk"].pool;
        pool.slots.iter().flatten().map(|p| p.body.unwrap().to).collect()
    };
    let at_rest = lying(&app);
    assert!(at_rest.iter().all(|p| (p.y - 0.05).abs() < 0.01), "on the floor: {at_rest:?}");
    assert!(at_rest.iter().any(|p| p.distance(Vec3::new(2.0, 0.05, 0.0)) > 0.3), "they scattered");
    // `UntilCleared`: far past their 4 s life, and past the 30 s cap, they are still there, in the same places.
    step(&mut app, 2400);
    app.update();
    assert_eq!(lying(&app), at_rest);
    assert_eq!(app.world().resource::<Particles>().effects["chunk"].pool.live(), 6);
    // Their meshes are shown where they lie.
    assert_eq!(meshes(&mut app, "chunk"), (16, 6));
    assert!(ok(&mut app, "effects", json!({})).contains("(6 at rest)"));
    assert_eq!(status(&app)["debris"], json!({"resting": 6, "flying": 0}));
    assert!(status(&app)["rays"]["total"].as_u64().unwrap() > 20, "{}", status(&app));
}

#[test]
fn debris_that_does_not_stay_fades_as_any_particle_and_one_that_never_lands_flies_through() {
    let (mut app, _assets) = room("fade_debris");
    ok(&mut app, "effect_play", json!({"effect": "scrap", "point": [1, 1, 0]}));
    ok(&mut app, "effect_play", json!({"effect": "ghost", "point": [-2, 1, 0]}));
    step(&mut app, 90);
    app.update();
    // The scrap landed and lay a while; the ghost fell through the floor (no `collide`).
    let (ghosts, now) = (&app.world().resource::<Particles>().effects["ghost"].pool, shards::fx::clock(app.world()));
    assert!(ghosts.live() == 4 && ghosts.slots.iter().flatten().all(|p| p.body.is_none() && p.position((now - p.born) as f32, -9.8, 0.0).y < -1.0));
    step(&mut app, 120);
    app.update();
    assert_eq!(live(&app, "scrap"), 0, "after its 1 s of life it is gone, at rest or not");
    assert_eq!(live(&app, "ghost"), 0);
}

#[test]
fn ten_times_the_pool_of_chunks_leaves_the_pool_full_and_the_oldest_recycled() {
    let (mut app, _assets) = room("flood");
    for i in 0..27 {
        ok(&mut app, "effect_play", json!({"effect": "chunk", "point": [i as f32 * 0.1 - 1.0, 1.0, 0.0]}));
        step(&mut app, 2);
        app.update();
    }
    // 27 plays of 6 = 162 chunks into a pool of 16.
    let pool = &app.world().resource::<Particles>().effects["chunk"].pool;
    assert_eq!((pool.live(), pool.size(), pool.started, pool.recycled), (16, 16, 162, 146));
    step(&mut app, 400);
    app.update();
    let (resting, flying) = debris_counts(app.world());
    assert_eq!((resting, flying), (16, 0), "the 16 that were left lie on the floor");
    assert_eq!(meshes(&mut app, "chunk"), (16, 16));
}

#[test]
fn clearing_takes_marks_and_debris_away_by_tag_and_a_new_world_takes_everything() {
    let (mut app, _assets) = room("clearing");
    let fill = |app: &mut App| {
        for kind in ["hole", "blood", "scorch"] {
            spawn_mark(app.world_mut(), kind, Vec3::ZERO, Vec3::Y).unwrap();
        }
        ok(app, "effect_play", json!({"effect": "chunk", "point": [0, 1, 0]}));
        ok(app, "effect_play", json!({"effect": "scrap", "point": [0, 1, 0]}));
    };
    fill(&mut app);
    // `gore` is on holes, blood and the chunks; `blood` on the blood alone; the scorch and the scrap carry none.
    let cleared = clear(app.world_mut(), "blood");
    assert_eq!((cleared.marks, cleared.particles), (1, 0));
    let cleared = clear(app.world_mut(), "gore");
    assert_eq!((cleared.marks, cleared.particles), (1, 6));
    assert_eq!(marks_of(&app, "scorch").len(), 1);
    assert_eq!(live(&app, "scrap"), 4);
    assert_eq!(ok(&mut app, "fx_clear", json!({"tag": "gore"})), "cleared 0 marks and 0 particles tagged `gore`");
    app.update();
    assert_eq!((decals_of(&mut app, "hole").len(), meshes(&mut app, "chunk")), (0, (16, 0)), "what is cleared is hidden");
    assert_eq!(ok(&mut app, "fx_clear", json!({"all": true})), "cleared 1 marks and 4 particles of every kind");
    assert!(refused(&mut app, "fx_clear", json!({})).starts_with("give a `tag`"));
    assert!(refused(&mut app, "fx_clear", json!({"tag": "gore", "all": true})).starts_with("give a `tag` or `all=true`, not both"));
    // A world built again (a region change, a snapshot) clears everything.
    fill(&mut app);
    shards::ai_core::world_rebuilt(app.world_mut(), "region wilds");
    app.world_mut().flush();
    assert_eq!((app.world().resource::<Marks>().live(), app.world().resource::<Particles>().live()), (0, 0));
    // And clearing everything counts what it took.
    fill(&mut app);
    assert_eq!(clear_all(app.world_mut()).total(), 3 + 6 + 4);
}

#[test]
fn a_mark_the_game_plays_is_laid_and_one_no_content_defines_is_journaled_once() {
    let (mut app, _assets) = room("play");
    fire!(app.world_mut(), PlayMark::new("hole", Vec3::new(1.0, 0.0, 1.0), Vec3::Y));
    assert_eq!(marks_of(&app, "hole").len(), 1);
    for _ in 0..3 {
        fire!(app.world_mut(), PlayMark::new("holle", Vec3::ZERO, Vec3::Y));
    }
    let journal = ok(&mut app, "journal", json!({}));
    assert_eq!(journal.matches("fx: PlayMark: no mark `holle` (did you mean `hole`?)").count(), 1, "{journal}");
}

#[test]
fn a_machine_that_draws_nothing_lays_nothing_and_says_so() {
    let assets = Folder::empty("headless");
    assets.put("content/fx/gore.mark.ron", MARKS);
    assets.put("content/fx/gore.effect.ron", EFFECTS);
    assets.put("content/fx/gore.fx.ron", FX);
    let mut app = game(&assets);
    wall_and_ceiling(app.world_mut());
    step(&mut app, 2);
    let me = body(app.world_mut(), Vec3::ZERO);
    shot(app.world_mut(), me, Vec3::new(0.0, 1.5, 0.0));
    impact(app.world_mut(), me, Vec3::new(5.5, 1.5, 0.0), Vec3::NEG_X, None);
    fire!(app.world_mut(), PlayMark::new("hole", Vec3::ZERO, Vec3::Y));
    assert_eq!((app.world().resource::<Marks>().live(), app.world().resource::<Marks>().laid), (0, 0));
    assert_eq!(app.world().resource::<Particles>().played, 0);
    assert_eq!(
        refused(&mut app, "mark_spawn", json!({"mark": "hole", "point": [0, 0, 0], "normal": [0, 1, 0]})),
        "nothing is drawn headless: marks are laid in a window or with --offscreen (mark `hole` is known)"
    );
    let said = ok(&mut app, "effects", json!({}));
    assert!(said.contains("3 marks · nothing is drawn headless: marks are laid in a window or with --offscreen"), "{said}");
    assert!(said.contains("  hole    spot #202020 · 0.1 m · until cleared · tags gore · up to 16"), "{said}");
    assert!(!said.contains("debris:"), "no debris line without a picture");
    assert_eq!(app.world_mut().query::<&MarkDecal>().iter(app.world()).count(), 0);
    assert_eq!(ok(&mut app, "fx_clear", json!({"all": true})), "cleared 0 marks and 0 particles of every kind");
}

#[test]
fn a_count_that_no_check_passed_lays_no_more_than_the_limit() {
    // A table with an error still loads (the first load plays what it has): a count of four billion must not run four
    // billion searches when a shot lands.
    let assets = Folder::empty("count_limit");
    assets.put("content/fx/gore.mark.ron", MARKS);
    assets.put("content/fx/gore.fx.ron", r#"{"big": (marks: {"shot_impact": [(mark: "hole", place: Below(5.0), count: 4000000000)]})}"#);
    let mut app = drawn(&assets);
    step(&mut app, 2);
    app.update();
    let me = body(app.world_mut(), Vec3::new(-1.0, 0.0, 0.0));
    let said = errors_of(&mut app);
    assert!(said.contains("count: 1 to 16, not 4000000000"), "{said}");
    impact(app.world_mut(), me, Vec3::new(2.0, 1.0, 0.0), Vec3::NEG_X, None);
    let marks = app.world().resource::<Marks>();
    assert_eq!((marks.laid, marks.live()), (16, 16), "sixteen at most, each below the point");
    assert_eq!(app.world().resource::<GoreStats>().searches, 16);
}

fn errors_of(app: &mut App) -> String {
    text(&shards::testing::call(app, "content_check", json!({})))
}
