//! Window-only layer: meshes for what the physics layer collides with, the engine's
//! third-person camera, a HUD, telegraph zones with their symbols. Keyboard, mouse and
//! gamepad reach the player through the engine's input map (into `Intent`), like an
//! agent's tools do. Gameplay never depends on anything here, so the same game runs
//! headless; nothing here adds a component a replay would hash.

use std::collections::HashSet;

use shards::action::{hit_zones, HitShape, HitZone, WorldShape};
use shards::ai_core::highlight::AiOverlay;
use shards::ai_core::sim;
use shards::motion::{CharacterController, Collider, Shape, Volume, VolumeKind};
use shards::prelude::*;

use crate::gameplay::{Boss, LayoutPiece, Look, PieceKind, Player, Skeleton};

pub struct VisualsPlugin;

impl Plugin for VisualsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(Color::srgb(0.05, 0.06, 0.09)))
            .init_resource::<Dressed>()
            .init_resource::<Zones>()
            .add_systems(PreStartup, setup_assets)
            .add_systems(Startup, (setup_scene, setup_hud).chain())
            .add_systems(
                Update,
                (dev_keys, dress, update_hud, collect_zones, (draw_zones, label_zones, draw_updrafts)).chain(),
            );
    }
}

#[derive(Resource)]
struct Palette {
    cube: Handle<Mesh>,
    post: Handle<Mesh>,
    nose: Handle<Mesh>,
    player: Handle<StandardMaterial>,
    skeleton: Handle<StandardMaterial>,
    boss: Handle<StandardMaterial>,
    stone: Handle<StandardMaterial>,
    floor: Handle<StandardMaterial>,
    wall: Handle<StandardMaterial>,
    sand: Handle<StandardMaterial>,
    dark: Handle<StandardMaterial>,
    water: Handle<StandardMaterial>,
    goal: Handle<StandardMaterial>,
    marker: Handle<StandardMaterial>,
}

fn setup_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<StandardMaterial>>) {
    let mut solid = |c: Color| mats.add(StandardMaterial { base_color: c, perceptual_roughness: 0.85, ..default() });
    let (player, skeleton, boss) =
        (solid(Color::srgb(0.25, 0.55, 1.0)), solid(Color::srgb(0.85, 0.82, 0.7)), solid(Color::srgb(0.55, 0.85, 1.0)));
    let (stone, floor, wall) = (
        solid(Color::srgb(0.38, 0.39, 0.43)),
        solid(Color::srgb(0.17, 0.19, 0.23)),
        solid(Color::srgb(0.27, 0.28, 0.33)),
    );
    let (sand, dark, marker) =
        (solid(Color::srgb(0.55, 0.5, 0.38)), solid(Color::srgb(0.1, 0.1, 0.12)), solid(Color::srgb(1.0, 0.85, 0.2)));
    let mut glass = |c: Color| mats.add(StandardMaterial { base_color: c, alpha_mode: AlphaMode::Blend, ..default() });
    let (water, goal) = (glass(Color::srgba(0.15, 0.4, 0.95, 0.45)), glass(Color::srgba(0.3, 1.0, 0.4, 0.3)));
    commands.insert_resource(Palette {
        cube: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        post: meshes.add(Cylinder::new(0.08, 1.6)),
        nose: meshes.add(Cuboid::new(0.25, 0.25, 0.5)),
        player,
        skeleton,
        boss,
        stone,
        floor,
        wall,
        sand,
        dark,
        water,
        goal,
        marker,
    });
}

fn setup_scene(mut commands: Commands) {
    commands.spawn((
        DirectionalLight { illuminance: 9000.0, shadow_maps_enabled: true, ..default() },
        Transform::from_xyz(30.0, 40.0, 20.0).looking_at(Vec3::new(40.0, 0.0, 0.0), Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight { brightness: 300.0, ..default() });
    // The engine's camera: it follows the `LocalPlayer`, mouse and right stick turn it.
    let camera = ThirdPersonCamera { distance: 8.0, pitch: 0.5, ..default() };
    let at = Transform::from_xyz(0.0, 8.0, 20.0).looking_at(Vec3::new(0.0, 0.0, 12.0), Vec3::Y);
    commands.spawn((Camera3d::default(), AiCamera, camera, at));
}

/// Entities that have their looks already (window-only bookkeeping: no marker component
/// on gameplay entities, so nothing here enters a replay's hash).
#[derive(Resource, Default)]
struct Dressed(HashSet<Entity>);

type Undressed<'a> = (
    Entity,
    &'a Collider,
    Option<&'a CharacterController>,
    Option<&'a Volume>,
    Option<&'a LayoutPiece>,
    Has<Player>,
    Has<Skeleton>,
    Has<Boss>,
);

/// Gives what collides its looks: characters a capsule with a nose, boxes and ramps their
/// stone, volumes a see-through box; landmarks without a body a post. A layout piece whose
/// collider changed (a hot edit) is dressed anew.
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it reads")]
fn dress(
    mut commands: Commands,
    palette: Res<Palette>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut dressed: ResMut<Dressed>,
    bodies: Query<Undressed>,
    changed: Query<Entity, (With<LayoutPiece>, Changed<Collider>)>,
    posts: Query<Entity, (With<AiLandmark>, Without<Collider>)>,
    children: Query<&Children>,
) {
    for entity in &changed {
        if dressed.0.remove(&entity) {
            for child in children.get(entity).into_iter().flatten() {
                commands.entity(*child).despawn();
            }
        }
    }
    for (entity, collider, character, volume, piece, player, skeleton, boss) in &bodies {
        if !dressed.0.insert(entity) {
            continue;
        }
        let at = Transform::from_translation(collider.offset);
        let mut e = commands.entity(entity);
        e.insert(Visibility::default());
        if character.is_some() {
            let Shape::Capsule { half_height, radius } = collider.shape else { continue };
            let mat = if player {
                palette.player.clone()
            } else if skeleton {
                palette.skeleton.clone()
            } else if boss {
                palette.boss.clone()
            } else {
                palette.stone.clone()
            };
            let height = 2.0 * (half_height + radius);
            let body = meshes.add(Capsule3d::new(radius, half_height * 2.0));
            e.with_children(|c| {
                c.spawn((Mesh3d(body), MeshMaterial3d(mat), at));
                let nose = Transform::from_xyz(0.0, height * 0.8, -radius - 0.15);
                c.spawn((Mesh3d(palette.nose.clone()), MeshMaterial3d(palette.dark.clone()), nose));
            });
            continue;
        }
        let Shape::Cuboid { half } = collider.shape else { continue };
        let mat = match (volume.map(|v| &v.kind), piece.map(|p| p.kind)) {
            (Some(VolumeKind::Water), _) => palette.water.clone(),
            // Drawn as a wireframe (`draw_updrafts`): a glider inside one sees through.
            (Some(VolumeKind::Updraft { .. }), _) => continue,
            (Some(VolumeKind::Trigger { .. }), _) => palette.goal.clone(),
            (None, Some(PieceKind::Solid(Look::Floor))) => palette.floor.clone(),
            (None, Some(PieceKind::Solid(Look::Wall))) => palette.wall.clone(),
            (None, Some(PieceKind::Solid(Look::Sand))) => palette.sand.clone(),
            _ => palette.stone.clone(),
        };
        let cube = Transform { scale: half * 2.0, ..at };
        e.with_child((Mesh3d(palette.cube.clone()), MeshMaterial3d(mat), cube));
    }
    for entity in &posts {
        if dressed.0.insert(entity) {
            let post = (
                Mesh3d(palette.post.clone()),
                MeshMaterial3d(palette.marker.clone()),
                Transform::from_xyz(0.0, 0.8, 0.0),
            );
            commands.entity(entity).insert(Visibility::default()).with_child(post);
        }
    }
}

/// P: pause · N / `.`: one tick while paused · F1: id overlay · F2: revive the player ·
/// F3: collider overlay · F12: marked screenshot into shots/. (The rest of the keyboard is
/// the engine's input map: WASD, Space, Shift, F, Ctrl, mouse, Q, C, Tab.)
fn dev_keys(world: &mut World) {
    let (pause, step, overlay, revive, colliders, shot) = {
        let keys = world.resource::<ButtonInput<KeyCode>>();
        (
            keys.just_pressed(KeyCode::KeyP),
            keys.just_pressed(KeyCode::KeyN) || keys.just_pressed(KeyCode::Period),
            keys.just_pressed(KeyCode::F1),
            keys.just_pressed(KeyCode::F2),
            keys.just_pressed(KeyCode::F3),
            keys.just_pressed(KeyCode::F12),
        )
    };
    let call = |world: &mut World, tool: &str, args: shards::serde_json::Value| {
        shards::ai_core::call_tool(world, "keyboard", tool, args);
    };
    if pause {
        let tool = if sim::is_paused(world) { "resume" } else { "pause" };
        call(world, tool, shards::serde_json::Value::Null);
    }
    if step && sim::is_paused(world) {
        call(world, "step", shards::serde_json::json!({"ticks": 1}));
    }
    if overlay {
        let mut o = world.resource_mut::<AiOverlay>();
        o.enabled = !o.enabled;
    }
    if revive {
        call(world, "heal", shards::serde_json::Value::Null);
    }
    if colliders {
        let on = !world.get_resource::<Colliders>().is_some_and(|c| c.0);
        world.insert_resource(Colliders(on));
        call(world, "physics_overlay", shards::serde_json::json!({"on": on}));
    }
    if shot {
        // The path lands in the journal once the file is written.
        call(world, "shot", shards::serde_json::Value::Null);
    }
}

/// Whether F3 turned the collider overlay on.
#[derive(Resource)]
struct Colliders(bool);

#[derive(Component)]
struct HudText;

#[derive(Component)]
struct BossText;

/// A HUD bar: its fill's share of the full width.
#[derive(Component, Clone, Copy)]
enum Bar {
    Health,
    Stamina,
    Poise,
}

fn setup_hud(mut commands: Commands) {
    let bar = |kind: Bar, color: Color| {
        (
            Node { width: Val::Px(240.0), height: Val::Px(9.0), margin: UiRect::bottom(Val::Px(4.0)), ..default() },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.5)),
            children![(
                Node { width: Val::Percent(100.0), height: Val::Percent(100.0), ..default() },
                BackgroundColor(color),
                kind
            )],
        )
    };
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(10.0),
            left: Val::Px(12.0),
            flex_direction: FlexDirection::Column,
            ..default()
        },
        children![
            bar(Bar::Health, Color::srgb(0.85, 0.2, 0.2)),
            bar(Bar::Stamina, Color::srgb(0.3, 0.8, 0.35)),
            bar(Bar::Poise, Color::srgb(0.9, 0.75, 0.25)),
            (Text::new(""), TextFont::from_font_size(16.0), HudText),
        ],
    ));
    commands.spawn((
        Node { position_type: PositionType::Absolute, top: Val::Px(10.0), right: Val::Px(12.0), ..default() },
        Text::new(""),
        TextFont::from_font_size(17.0),
        TextColor(Color::srgb(0.6, 0.9, 1.0)),
        BossText,
    ));
}

/// The keys, for the HUD: the engine's input map (`InputMap::standard`) and the dev keys.
const CONTROLS: &str = "WASD move | Space jump | Shift sprint | F glide | Ctrl dodge\n\
                        LMB light | RMB heavy | Q block | C flask | Tab lock-on\n\
                        P pause | N step | F1 ids | F2 revive | F3 colliders | F12 shot";

type PlayerHud<'a> = (
    &'a AiId,
    &'a Health,
    Option<&'a Stamina>,
    Option<&'a Poise>,
    Option<&'a Charges>,
    &'a ActionState,
    Option<&'a MotionState>,
    Has<Dead>,
);

fn update_hud(
    tick: Res<SimTick>,
    time: Res<Time<Virtual>>,
    player: Query<PlayerHud, With<Player>>,
    boss: Query<(&AiId, &Health, &Boss, &Name, Option<&Poise>, Has<Dead>)>,
    mut bars: Query<(&Bar, &mut Node)>,
    mut hud: Query<&mut Text, (With<HudText>, Without<BossText>)>,
    mut boss_hud: Query<&mut Text, (With<BossText>, Without<HudText>)>,
) {
    if let Ok((id, hp, st, poise, charges, action, motion, dead)) = player.single() {
        let share = |now: f32, max: f32| if max > 0.0 { (now / max).clamp(0.0, 1.0) * 100.0 } else { 0.0 };
        for (bar, mut node) in &mut bars {
            let pct = match bar {
                Bar::Health => share(hp.current, hp.max),
                Bar::Stamina => st.map_or(0.0, |s| share(s.current, s.max)),
                Bar::Poise => poise.map_or(0.0, |p| share(p.current, p.max)),
            };
            if node.width != Val::Percent(pct) {
                node.width = Val::Percent(pct);
            }
        }
        if let Ok(mut text) = hud.single_mut() {
            let state = if time.is_paused() { "PAUSED" } else { "running" };
            let flasks = charges.and_then(|c| c.get("flask")).map_or(0, |f| f.current);
            let doing = action.current.as_ref().map_or("free".to_owned(), |r| format!("{} {}", r.phase, r.action));
            // The built-in font is ASCII only.
            let moving = motion
                .map(shards::motion::describe_motion)
                .unwrap_or_default()
                .replace('↑', "up ")
                .replace('↓', "down ");
            let dead = if dead { "   DEAD (F2 revives)" } else { "" };
            text.0 = format!(
                "{id}  HP {:.0}/{:.0}  STAM {:.0}  flasks {flasks}{dead}\n{doing}  {moving}\ntick {}  {state}\n{CONTROLS}",
                hp.current,
                hp.max,
                st.map_or(0.0, |s| s.current),
                tick.0
            );
        }
    }
    if let Ok(mut text) = boss_hud.single_mut() {
        text.0 = boss
            .iter()
            .map(|(id, hp, b, name, poise, dead)| {
                let poise = poise.map(shards::action::fighter::describe_poise).unwrap_or_default();
                let state = if dead { "  fallen".to_owned() } else { format!("  {poise}") };
                format!("{id} {name}  phase {}  {:.0}/{:.0}{state}", b.phase, hp.current, hp.max)
            })
            .collect::<Vec<_>>()
            .join("\n");
    }
}

/// This frame's hit zones (`shards::action::hit_zones` reads the whole world).
#[derive(Resource, Default)]
struct Zones(Vec<HitZone>);

fn collect_zones(world: &mut World) {
    let zones = hit_zones(world);
    world.resource_mut::<Zones>().0 = zones;
}

/// The ground outline of every attack still to land: red for enemies, blue for the
/// player's; a second outline grows toward the edge as it is about to land.
fn draw_zones(mut gizmos: Gizmos, zones: Res<Zones>, teams: Query<&Team>) {
    for zone in &zones.0 {
        let hostile = teams.get(zone.attacker).is_ok_and(|t| t.0 != "heroes");
        let color = if hostile { Color::srgb(1.0, 0.25, 0.2) } else { Color::srgb(0.4, 0.8, 1.0) };
        let soon = (1.0 - zone.lands_in / 0.5).clamp(0.05, 1.0);
        outline(&mut gizmos, &zone.shape, 1.0, color);
        outline(&mut gizmos, &zone.shape, soon, color.with_alpha(0.6));
    }
}

/// `shape`'s footprint on the ground, scaled by `scale` about its origin.
fn outline(gizmos: &mut Gizmos, s: &WorldShape, scale: f32, color: Color) {
    let ground = |p: Vec3| Vec3::new(p.x, s.origin.y.min(p.y) + 0.05, p.z);
    let (o, f, r) = (s.origin, s.facing, s.right());
    let flat_circle = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    match s.shape {
        HitShape::Sphere { radius } | HitShape::Circle { radius, .. } => {
            gizmos.circle(Isometry3d::new(ground(o), flat_circle), radius * scale, color);
        }
        HitShape::Cone { range, angle, .. } => {
            let half = angle.to_radians() * 0.5;
            let steps = 12;
            let edge = |i: i32| {
                let a = -half + 2.0 * half * i as f32 / steps as f32;
                ground(o + (f * a.cos() + r * a.sin()) * range * scale)
            };
            gizmos.line(ground(o), edge(0), color);
            gizmos.line(ground(o), edge(steps), color);
            for i in 0..steps {
                gizmos.line(edge(i), edge(i + 1), color);
            }
        }
        HitShape::Capsule { radius, length } => {
            rect(gizmos, ground(o + f * length * 0.5), f, r, radius * 2.0, length * scale, color)
        }
        HitShape::Box { width, depth, .. } => rect(gizmos, ground(o), f, r, width, depth * scale, color),
        HitShape::Line { length, width, .. } => {
            rect(gizmos, ground(o + f * length * scale * 0.5), f, r, width, length * scale, color)
        }
    }
}

fn rect(gizmos: &mut Gizmos, center: Vec3, f: Vec3, r: Vec3, width: f32, depth: f32, color: Color) {
    let (hf, hr) = (f * depth * 0.5, r * width * 0.5);
    let corners = [center + hf + hr, center + hf - hr, center - hf - hr, center - hf + hr];
    for i in 0..4 {
        gizmos.line(corners[i], corners[(i + 1) % 4], color);
    }
}

/// Updrafts: their box as a wireframe and streaks rising through it.
fn draw_updrafts(mut gizmos: Gizmos, time: Res<Time<Real>>, q: Query<(&Volume, &Collider, &Transform)>) {
    let color = Color::srgba(0.5, 0.95, 1.0, 0.6);
    for (volume, collider, tf) in &q {
        let (VolumeKind::Updraft { .. }, Shape::Cuboid { half }) = (&volume.kind, collider.shape) else { continue };
        let center = tf.translation + collider.offset;
        gizmos.cube(Transform { translation: center, rotation: tf.rotation, scale: half * 2.0 }, color);
        let rise = (time.elapsed_secs() * 4.0).rem_euclid(half.y * 2.0);
        for i in 0..9 {
            let (fx, fz) = ((i % 3) as f32 - 1.0, (i / 3) as f32 - 1.0);
            let base = center + Vec3::new(fx * half.x * 0.6, -half.y, fz * half.z * 0.6);
            let y = (rise + i as f32 * 1.7).rem_euclid(half.y * 2.0);
            gizmos.line(base + Vec3::Y * y, base + Vec3::Y * (y + 1.5).min(half.y * 2.0), color);
        }
    }
}

/// One screen label per zone: its symbol and when it lands (`⚠ 0.32s`), so danger reads
/// without colour.
#[derive(Component)]
struct ZoneLabel;

fn label_zones(
    mut commands: Commands,
    zones: Res<Zones>,
    camera: Query<(&Camera, &GlobalTransform), With<AiCamera>>,
    mut labels: Query<(Entity, &mut Text, &mut Node, &mut Visibility), With<ZoneLabel>>,
) {
    let Ok((camera, eye)) = camera.single() else { return };
    let mut free: Vec<_> = labels.iter_mut().collect();
    for zone in &zones.0 {
        let Some(symbol) = &zone.symbol else { continue };
        let at = zone.shape.center() + Vec3::Y * 0.3;
        let Ok(screen) = camera.world_to_viewport(eye, at) else { continue };
        let symbol = readable(symbol);
        let text = if zone.lands_in > 0.0 { format!("{symbol} {:.2}s", zone.lands_in) } else { symbol.to_owned() };
        match free.pop() {
            Some((_, mut label, mut node, mut visible)) => {
                label.0 = text;
                (node.left, node.top) = (Val::Px(screen.x - 14.0), Val::Px(screen.y - 12.0));
                *visible = Visibility::Inherited;
            }
            None => {
                commands.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(screen.x - 14.0),
                        top: Val::Px(screen.y - 12.0),
                        ..default()
                    },
                    Text::new(text),
                    TextFont::from_font_size(22.0),
                    TextColor(Color::srgb(1.0, 0.9, 0.3)),
                    ZoneLabel,
                ));
            }
        }
    }
    for (_, _, _, mut visible) in free {
        *visible = Visibility::Hidden;
    }
}

/// The symbol as the window's built-in font can draw it (ASCII only): the content's
/// symbols for the warden's attacks get ASCII stand-ins; a game ships a font with its own.
fn readable(symbol: &str) -> &str {
    match symbol {
        "⚠" => "/!\\",
        "⟲" => "(@)",
        "≈" => "~~~",
        "↯" => ">>",
        s if s.is_ascii() => s,
        _ => "!",
    }
}
