//! Use by looking (F5): what a character's look meets that it can use.
//!
//! A thing a character can use carries [`Usable`] and a collider on the [`USE`](LayerMask::USE)
//! layer, a layer only this ray asks for: a level's `interactable` marker gets both from its node
//! (`shards_level`), and a game's prefab can carry them by hand. [`use_target`] casts a ray from the
//! character's eye ([`eye_of`]) along its look ([`look_dir`]) against the `USE` and `WORLD` layers
//! together:
//!
//! - a target counts only when it is nearer than every `WORLD` collider on the ray (or at the same
//!   distance: a button flush with its wall), so a wall or a pane of glass blocks a use;
//! - it counts only within its reach: the marker's own [`Usable::use_range`], else the body's
//!   `use_range` (`locomotion`, 2 m unless the content says);
//! - of the targets that count, the nearest wins, and two at the same distance go to the lower
//!   [`ColliderId`].
//!
//! Everything here reads the simulation (never a camera) and changes nothing, so the simulation's
//! use of a press (`shards_action`), the `USE` line of `observe`, the HUD's prompt and a test's
//! check all ask the same ray.

use ai_core::prelude::*;
use bevy::prelude::*;

use crate::camera::lock::body_point;
use crate::eye::{eye_of, look_dir, params_of};
use crate::physics::{Collider, ColliderId, LayerMask, PhysicsWorld, QueryFilter, RayHit};

/// How far a character reaches when neither its `locomotion` nor the thing it looks at says (m).
pub const DEFAULT_USE_RANGE: f32 = 2.0;

/// How far a use probe looks along the ray (m): far enough to name a thing that is out of reach, so the
/// refusal can say how far it is.
pub const PROBE_RANGE: f32 = 100.0;

/// How much nearer than a wall a use area may stand before it counts as in front of it (m): float
/// noise, so a button laid flush with its wall is not lost to rounding.
const FLUSH: f32 = 1e-4;

/// Something a character can use by looking at it (F5). A level's `interactable` marker makes one
/// (`hold_s`, `use_range` and `prompt` are node properties); so does a prefab with `"Usable": (name:
/// "door")` and a collider on `CollisionLayers::USE`. What a use does is the game's: it observes
/// `Used` (`shards_action`).
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
#[require(AiKind = AiKind::new("Usable"))]
pub struct Usable {
    /// What a game tells it by (`door`, `tablet`): the `name` of the `Used` event and of the `USE`
    /// line. A level marker's name.
    pub name: String,
    /// The text a HUD shows when the character looks at it (`you.use_target`): a Fluent `@id` or
    /// plain text. Empty: the name.
    pub prompt: String,
    /// Seconds the button is held for the use to complete: 0 (the default) is a press. A valve or a
    /// lever that takes time is held, and looking away or letting go cancels it.
    pub hold_s: f32,
    /// How far from the eye it can be used (m), whatever the body reaches. `None`: the body's own
    /// `use_range`.
    pub use_range: Option<f32>,
}

impl Usable {
    /// A press-to-use thing called `name`.
    pub fn new(name: impl Into<String>) -> Self {
        Self { name: name.into(), ..default() }
    }

    /// The same, held for `seconds`.
    pub fn held(mut self, seconds: f32) -> Self {
        self.hold_s = seconds;
        self
    }

    /// The same, from `metres` away whatever the body reaches.
    pub fn reaching(mut self, metres: f32) -> Self {
        self.use_range = Some(metres);
        self
    }

    /// What a HUD shows: the prompt, else the name.
    pub fn prompt_text(&self) -> &str {
        if self.prompt.is_empty() { &self.name } else { &self.prompt }
    }

    /// How far this one can be used from, for a body that reaches `body` metres.
    pub fn reach(&self, body: f32) -> f32 {
        self.use_range.unwrap_or(body)
    }
}

/// One use area a look passes through ([`UseProbe::hits`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UseHit {
    /// The entity with the [`Usable`].
    pub entity: Entity,
    /// Metres from the eye to where the ray meets its area (0 from inside it).
    pub distance: f32,
    /// How far it can be used from, for this body.
    pub range: f32,
    /// Its collider, which breaks a tie between two at one distance.
    pub collider: ColliderId,
}

impl UseHit {
    /// Whether it is within reach.
    pub fn reached(&self) -> bool {
        self.distance <= self.range
    }
}

/// Everything a character's look passes through on the way to what it could use: the first `WORLD`
/// collider, and every use area before it, nearest first ([`use_probe`]).
#[derive(Clone, Debug, PartialEq)]
pub struct UseProbe {
    pub eye: Vec3,
    pub dir: Vec3,
    /// How far the body reaches (`use_range`): the most a use area counts at unless it says its own.
    pub reach: f32,
    /// The nearest `WORLD` hit within [`PROBE_RANGE`]: what blocks the use of anything beyond it.
    pub wall: Option<RayHit>,
    /// The use areas up to the wall, by distance, then by collider; the ones out of their own reach too (the
    /// target is the first within it).
    pub hits: Vec<UseHit>,
}

impl UseProbe {
    /// The target: the nearest use area within its reach.
    pub fn target(&self) -> Option<&UseHit> {
        self.hits.iter().find(|h| h.reached())
    }
}

/// What a character's use would meet now ([`use_target`]).
#[derive(Clone, Debug, PartialEq)]
pub struct UseTarget {
    pub entity: Entity,
    /// The [`Usable`]'s name.
    pub name: String,
    /// What a HUD shows ([`Usable::prompt_text`]).
    pub prompt: String,
    /// Seconds to hold the button; 0 for a press.
    pub hold_s: f32,
    /// Metres from the eye.
    pub distance: f32,
    /// How far this one can be used from.
    pub range: f32,
}

/// The ray of a use, with everything it passes: from `user`'s eye along its look, against the `WORLD`
/// layer for the wall and the `USE` layer for the areas. `None` for an entity that is no character
/// with a position, or a world with no physics.
pub fn use_probe(world: &World, user: Entity) -> Option<UseProbe> {
    let physics = world.get_resource::<PhysicsWorld>()?;
    let eye = eye_of(world, user)?;
    let dir = Dir3::new(look_dir(world, user)?).ok()?;
    let body = params_of(world, user)?.use_range;
    let wall = physics.raycast(eye, dir, PROBE_RANGE, &QueryFilter::new(LayerMask::WORLD).excluding(user));
    let limit = wall.map_or(PROBE_RANGE, |w| (w.distance + FLUSH).min(PROBE_RANGE));
    let mut filter = QueryFilter::new(LayerMask::USE).excluding(user);
    let mut hits = Vec::new();
    // Each turn meets one more area or ends the walk: it leaves what it met out of the filter.
    while let Some(hit) = physics.raycast(eye, dir, limit, &filter) {
        filter = filter.excluding(hit.entity);
        if let Some(usable) = world.get::<Usable>(hit.entity) {
            let collider = world.get::<ColliderId>(hit.entity).copied().unwrap_or_default();
            hits.push(UseHit { entity: hit.entity, distance: hit.distance, range: usable.reach(body), collider });
        }
    }
    hits.sort_by(|a, b| a.distance.total_cmp(&b.distance).then(a.collider.cmp(&b.collider)));
    Some(UseProbe { eye, dir: *dir, reach: body, wall, hits })
}

/// What `user` would use now: the nearest [`Usable`] its look meets within reach, with no `WORLD`
/// collider (a wall, a pane of glass) nearer on the way. A tie goes to the lower [`ColliderId`].
/// `None`: nothing in reach, or `user` is no character.
///
/// ```ignore
/// if let Some(door) = shards::motion::use_target(world, me) {
///     println!("[E] {}", door.prompt);
/// }
/// ```
pub fn use_target(world: &World, user: Entity) -> Option<UseTarget> {
    let probe = use_probe(world, user)?;
    let hit = probe.target()?;
    let usable = world.get::<Usable>(hit.entity)?;
    Some(UseTarget {
        entity: hit.entity,
        name: usable.name.clone(),
        prompt: usable.prompt_text().to_owned(),
        hold_s: usable.hold_s.max(0.0),
        distance: hit.distance,
        range: hit.range,
    })
}

/// The middle of the use area of `entity`: where a look at it aims. `None` for an entity with no
/// position or no collider (a bare marker no look can meet).
pub fn use_point(world: &World, entity: Entity) -> Option<Vec3> {
    let tf = world.get::<Transform>(entity)?;
    let collider = world.get::<Collider>(entity)?;
    Some(body_point(tf, Some(collider), 0.5))
}

/// What `observe` says of a usable thing beyond its kind and name, when it is not the plain case: `usable (hold 1.0s,
/// reach 4.0m)`, and `usable (no use area)` for a marker with no collider, which no look can meet.
fn describe(world: &World, entity: Entity) -> Option<String> {
    let usable = world.get::<Usable>(entity)?;
    let mut notes = Vec::new();
    if usable.hold_s > 0.0 {
        notes.push(format!("hold {:.1}s", usable.hold_s));
    }
    if let Some(range) = usable.use_range {
        notes.push(format!("reach {range:.1}m"));
    }
    if world.get::<ColliderId>(entity).is_none() {
        notes.push("no use area".to_owned());
    }
    (!notes.is_empty()).then(|| format!("usable ({})", notes.join(", ")))
}

pub(crate) fn plugin(app: &mut App) {
    app.register_type::<Usable>().register_ai_world_describer("usable", describe);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::controller::{CharacterController, CharacterParams};
    use crate::physics::{CollisionLayers, PhysicsPlugin, Shape};
    use shards_core::{Gaze, Intent};

    /// A world with physics and a character at the origin looking north (-Z) from 1.6 m up.
    fn room() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((MinimalPlugins, PhysicsPlugin));
        let user = app
            .world_mut()
            .spawn((
                Transform::default(),
                CharacterController::default(),
                CharacterParams { eye_height: Some(1.6), ..default() },
                Intent { look: Some(Gaze::new(0.0, 0.0)), ..default() },
            ))
            .id();
        (app, user)
    }

    /// A use area `size` wide at `at`, called `name`.
    fn area(app: &mut App, name: &str, at: Vec3, half: Vec3, usable: Usable) -> Entity {
        let usable = Usable { name: name.into(), ..usable };
        app.world_mut().spawn((usable, Collider::new(Shape::cuboid(half)).with_layers(CollisionLayers::USE), Transform::from_translation(at))).id()
    }

    fn wall(app: &mut App, at: Vec3, half: Vec3) -> Entity {
        app.world_mut().spawn((Collider::new(Shape::cuboid(half)), Transform::from_translation(at))).id()
    }

    /// Syncs the physics world, as a tick's first system does.
    fn sync(app: &mut App) {
        app.update();
    }

    fn target(app: &mut App, user: Entity) -> Option<UseTarget> {
        sync(app);
        use_target(app.world(), user)
    }

    #[test]
    fn a_thing_in_reach_in_front_is_the_target() {
        let (mut app, user) = room();
        let panel = area(&mut app, "panel", Vec3::new(0.0, 1.6, -1.5), Vec3::splat(0.2), Usable::default());
        let t = target(&mut app, user).expect("a panel 1.3 m ahead");
        assert_eq!((t.entity, t.name.as_str(), t.hold_s), (panel, "panel", 0.0));
        assert!((t.distance - 1.3).abs() < 1e-4 && t.range == DEFAULT_USE_RANGE, "{t:?}");
    }

    #[test]
    fn out_of_range_or_out_of_line_is_not() {
        let (mut app, user) = room();
        area(&mut app, "far", Vec3::new(0.0, 1.6, -4.0), Vec3::splat(0.2), Usable::default());
        assert_eq!(target(&mut app, user), None, "3.8 m is past the 2 m reach");
        // The probe still names it, for a refusal that says how far it is.
        let probe = use_probe(app.world(), user).unwrap();
        assert!(probe.hits.len() == 1 && !probe.hits[0].reached() && (probe.hits[0].distance - 3.8).abs() < 1e-3, "{probe:?}");
        assert_eq!((probe.reach, probe.wall), (DEFAULT_USE_RANGE, None));
        area(&mut app, "aside", Vec3::new(1.0, 1.6, -1.0), Vec3::splat(0.2), Usable::default());
        assert_eq!(target(&mut app, user), None, "not on the line of the look");
        app.world_mut().get_mut::<Intent>(user).unwrap().look = Some(Gaze::from_degrees(-45.0, 0.0));
        assert_eq!(target(&mut app, user).map(|t| t.name), Some("aside".into()), "looked at");
    }

    #[test]
    fn a_marker_can_reach_further_or_less_than_the_body() {
        let (mut app, user) = room();
        let far = area(&mut app, "console", Vec3::new(0.0, 1.6, -4.0), Vec3::splat(0.2), Usable::default().reaching(5.0));
        assert_eq!(target(&mut app, user).map(|t| (t.entity, t.range)), Some((far, 5.0)));
        app.world_mut().despawn(far);
        area(&mut app, "tiny", Vec3::new(0.0, 1.6, -1.5), Vec3::splat(0.2), Usable::default().reaching(1.0));
        assert_eq!(target(&mut app, user), None, "1.3 m is past its own 1 m");
        // The body's own reach is content.
        app.world_mut().entity_mut(user).insert(CharacterParams { eye_height: Some(1.6), use_range: 0.5, ..default() });
        area(&mut app, "near", Vec3::new(0.0, 1.6, -0.5), Vec3::splat(0.05), Usable::default());
        assert_eq!(target(&mut app, user).map(|t| t.name), Some("near".into()));
        app.world_mut().entity_mut(user).insert(CharacterParams { eye_height: Some(1.6), use_range: 0.2, ..default() });
        assert_eq!(target(&mut app, user), None, "a body that reaches 0.2 m does not reach the panel 0.5 m off");
    }

    #[test]
    fn a_wall_or_a_pane_between_blocks_the_use() {
        let (mut app, user) = room();
        area(&mut app, "behind", Vec3::new(0.0, 1.6, -1.6), Vec3::splat(0.2), Usable::default());
        assert!(target(&mut app, user).is_some());
        let glass = wall(&mut app, Vec3::new(0.0, 1.6, -0.9), Vec3::new(1.0, 1.0, 0.02));
        assert_eq!(target(&mut app, user), None, "a pane at 0.88 m is a WORLD collider: it blocks");
        let probe = use_probe(app.world(), user).unwrap();
        assert_eq!(probe.wall.map(|w| w.entity), Some(glass));
        assert!(probe.hits.is_empty(), "the area behind the pane is not met: {probe:?}");
        // A pane beside the line does not.
        app.world_mut().despawn(glass);
        wall(&mut app, Vec3::new(2.0, 1.6, -0.9), Vec3::new(0.5, 1.0, 0.02));
        assert!(target(&mut app, user).is_some());
    }

    #[test]
    fn a_solid_that_is_also_usable_counts_at_its_own_distance() {
        let (mut app, user) = room();
        let door = app
            .world_mut()
            .spawn((
                Usable::new("door"),
                Collider::new(Shape::cuboid(Vec3::new(0.5, 1.0, 0.1)))
                    .with_layers(CollisionLayers::new(LayerMask::WORLD | LayerMask::USE, LayerMask::ALL)),
                Transform::from_xyz(0.0, 1.0, -1.5),
            ))
            .id();
        assert_eq!(target(&mut app, user).map(|t| t.entity), Some(door), "a door is a wall and a target");
        // Something to use behind a closed door is out of reach for the door.
        area(&mut app, "lever", Vec3::new(0.0, 1.6, -1.9), Vec3::splat(0.05), Usable::default());
        assert_eq!(target(&mut app, user).map(|t| t.entity), Some(door), "and the door hides the lever");
    }

    #[test]
    fn the_nearest_wins_and_a_tie_goes_to_the_lower_collider() {
        let (mut app, user) = room();
        let farther = area(&mut app, "farther", Vec3::new(0.0, 1.6, -1.8), Vec3::splat(0.1), Usable::default());
        let nearer = area(&mut app, "nearer", Vec3::new(0.0, 1.6, -1.0), Vec3::splat(0.1), Usable::default());
        assert_eq!(target(&mut app, user).map(|t| t.entity), Some(nearer), "the nearer, though spawned later");
        let probe = use_probe(app.world(), user).unwrap();
        assert_eq!(probe.hits.iter().map(|h| h.entity).collect::<Vec<_>>(), [nearer, farther]);
        // Two at one distance: the lower collider id, whichever the world lists first.
        let (mut app, user) = room();
        let first = area(&mut app, "first", Vec3::new(0.0, 1.6, -1.5), Vec3::splat(0.1), Usable::default());
        let second = area(&mut app, "second", Vec3::new(0.0, 1.6, -1.5), Vec3::splat(0.1), Usable::default());
        sync(&mut app);
        let ids = |a: Entity, b: Entity| (app.world().get::<ColliderId>(a).copied(), app.world().get::<ColliderId>(b).copied());
        let (a, b) = ids(first, second);
        assert!(a < b, "{a:?} {b:?}");
        assert_eq!(use_target(app.world(), user).map(|t| t.entity), Some(first));
        // And the other way round: the winner is the lower id, not the first spawned.
        app.world_mut().despawn(first);
        let again = area(&mut app, "again", Vec3::new(0.0, 1.6, -1.5), Vec3::splat(0.1), Usable::default());
        sync(&mut app);
        assert!(app.world().get::<ColliderId>(second) < app.world().get::<ColliderId>(again));
        assert_eq!(use_target(app.world(), user).map(|t| t.entity), Some(second));
    }

    #[test]
    fn a_use_area_collides_with_nothing() {
        let layers = CollisionLayers::USE;
        for other in [CollisionLayers::WORLD, CollisionLayers::CHARACTER, CollisionLayers::VOLUME] {
            assert!(!layers.interacts_with(&other) && !other.interacts_with(&layers));
        }
        assert!(!LayerMask::USE.intersects(LayerMask::WORLD | LayerMask::CHARACTER | LayerMask::HURTBOX | LayerMask::HITBOX | LayerMask::VOLUME | LayerMask::PROJECTILE));
    }

    #[test]
    fn a_look_that_points_up_or_down_finds_what_is_there() {
        let (mut app, user) = room();
        let lamp = area(&mut app, "lamp", Vec3::new(0.0, 3.0, 0.0), Vec3::splat(0.2), Usable::default().reaching(3.0));
        assert_eq!(target(&mut app, user), None, "level look: nothing overhead");
        app.world_mut().get_mut::<Intent>(user).unwrap().look = Some(Gaze::from_degrees(0.0, 89.0));
        assert_eq!(target(&mut app, user).map(|t| t.entity), Some(lamp), "looking up at it");
    }

    #[test]
    fn the_point_of_a_use_area_is_its_middle() {
        let (mut app, _) = room();
        let panel = area(&mut app, "panel", Vec3::new(1.0, 2.0, -3.0), Vec3::new(0.2, 0.3, 0.1), Usable::default());
        let at = use_point(app.world(), panel).unwrap();
        assert!(at.distance(Vec3::new(1.0, 2.0, -3.0)) < 1e-5, "{at}");
        // A bare marker has no use area to aim at.
        let bare = app.world_mut().spawn((Usable::new("lever"), Transform::default())).id();
        assert_eq!(use_point(app.world(), bare), None);
    }
}
