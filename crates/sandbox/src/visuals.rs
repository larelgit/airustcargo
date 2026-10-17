//! Picture-only layer (a window or `--offscreen`): meshes for what the physics layer collides with, the engine's
//! third-person camera, a HUD, the updrafts' wireframes. Keyboard, mouse and gamepad reach
//! the player through the engine's input map (into `Intent`), like an agent's tools do.
//! Gameplay never depends on anything here, so the same game runs headless; nothing here
//! adds a component a replay would hash.
//!
//! The combat picture is the engine's (`shards_action`'s `render` feature, part of
//! `EnginePlugins`): ground telegraphs with their symbols, the F2 combat overlay and the
//! markers for attacks off screen, drawn for the `LocalPlayer` (the player prefab has it)
//! through the active camera (the `AiCamera` below). Nothing of that is drawn here.

use std::collections::HashSet;

use shards::ai_core::highlight::AiOverlay;
use shards::ai_core::sim;
use shards::motion::{CharacterController, Collider, Shape, Volume, VolumeKind};
use shards::prelude::*;

use shards::look::Look;

use crate::gameplay::{Boss, LayoutPiece, PieceKind, Player};

pub struct VisualsPlugin;

impl Plugin for VisualsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(Color::srgb(0.05, 0.06, 0.09)))
            .init_resource::<Dressed>()
            .add_systems(PreStartup, setup_assets)
            .add_systems(Startup, (setup_scene, setup_hud).chain())
            .add_systems(Update, (dev_keys, dress, update_hud, draw_updrafts).chain());
    }
}

#[derive(Resource)]
struct Palette {
    cube: Handle<Mesh>,
    post: Handle<Mesh>,
    nose: Handle<Mesh>,
    /// Under every look (`content/look/sandbox.look.ron`): the look gives the colour and the light.
    base: Handle<StandardMaterial>,
    water: Handle<StandardMaterial>,
    goal: Handle<StandardMaterial>,
}

fn setup_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<StandardMaterial>>) {
    let base = mats.add(StandardMaterial { base_color: Color::WHITE, perceptual_roughness: 0.85, ..default() });
    let mut glass = |c: Color| mats.add(StandardMaterial { base_color: c, alpha_mode: AlphaMode::Blend, ..default() });
    let (water, goal) = (glass(Color::srgba(0.15, 0.4, 0.95, 0.45)), glass(Color::srgba(0.3, 1.0, 0.4, 0.3)));
    commands.insert_resource(Palette {
        cube: meshes.add(Cuboid::new(1.0, 1.0, 1.0)),
        post: meshes.add(Cylinder::new(0.08, 1.6)),
        nose: meshes.add(Cuboid::new(0.25, 0.25, 0.5)),
        base,
        water,
        goal,
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
    Has<Look>,
);

/// Gives what collides its meshes and looks (`content/look/sandbox.look.ron`): characters a
/// capsule with a nose (a character's prefab names its look, the rest are stone), boxes and
/// ramps their surface's look, volumes a see-through box; landmarks without a body a post. A
/// layout piece whose collider changed (a hot edit) is dressed anew.
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
    let base = || MeshMaterial3d(palette.base.clone());
    for (entity, collider, character, volume, piece, looked) in &bodies {
        if !dressed.0.insert(entity) {
            continue;
        }
        let at = Transform::from_translation(collider.offset);
        let mut e = commands.entity(entity);
        e.insert(Visibility::default());
        if character.is_some() {
            let Shape::Capsule { half_height, radius } = collider.shape else { continue };
            let height = 2.0 * (half_height + radius);
            let body = meshes.add(Capsule3d::new(radius, half_height * 2.0));
            e.with_children(|c| {
                let mut body = c.spawn((Mesh3d(body), base(), at));
                if !looked {
                    body.insert(Look::new("stone"));
                }
                let nose = Transform::from_xyz(0.0, height * 0.8, -radius - 0.15);
                c.spawn((Mesh3d(palette.nose.clone()), base(), nose, Look::new("dark")));
            });
            continue;
        }
        let Shape::Cuboid { half } = collider.shape else { continue };
        let cube = (Mesh3d(palette.cube.clone()), Transform { scale: half * 2.0, ..at });
        match (volume.map(|v| &v.kind), piece.map(|p| p.kind)) {
            (Some(VolumeKind::Water), _) => e.with_child((cube, MeshMaterial3d(palette.water.clone()))),
            // Drawn as a wireframe (`draw_updrafts`): a glider inside one sees through.
            (Some(VolumeKind::Updraft { .. }), _) => continue,
            (Some(VolumeKind::Trigger { .. }), _) => e.with_child((cube, MeshMaterial3d(palette.goal.clone()))),
            (None, Some(PieceKind::Solid(surface))) => e.with_child((cube, base(), Look::new(surface.look()))),
            _ if looked => e.with_child((cube, base())),
            _ => e.with_child((cube, base(), Look::new("stone"))),
        };
    }
    for entity in &posts {
        if dressed.0.insert(entity) {
            let post = (Mesh3d(palette.post.clone()), base(), Look::new("marker"), Transform::from_xyz(0.0, 0.8, 0.0));
            commands.entity(entity).insert(Visibility::default()).with_child(post);
        }
    }
}

/// P: pause · N / `.`: one tick while paused · F1: id overlay · F3: collider overlay ·
/// F4: revive the player · F12: marked screenshot into shots/. (F2, the combat overlay, F6,
/// the net overlay, F7, the NPC overlay, the console key and F10, the developer menu, are the
/// engine's, and so is the rest of the keyboard, its input map: WASD, Space, Shift, F, Ctrl,
/// mouse, Q, C, Tab.) They wait while the console or the menu has the keyboard.
fn dev_keys(world: &mut World) {
    if shards::dev::has_keyboard(world) {
        return;
    }
    let (pause, step, overlay, revive, colliders, shot) = {
        let keys = world.resource::<ButtonInput<KeyCode>>();
        (
            keys.just_pressed(KeyCode::KeyP),
            keys.just_pressed(KeyCode::KeyN) || keys.just_pressed(KeyCode::Period),
            keys.just_pressed(KeyCode::F1),
            keys.just_pressed(KeyCode::F4),
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
                        LMB light | RMB heavy | Q block | C flask | Tab lock-on | Esc menu\n\
                        P pause | N step | F1 ids | F2 combat | F3 colliders | F4 revive | F12 shot\n\
                        F6 net | F7 npcs | F10 dev menu | ` console";

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
            let dead = if dead { "   DEAD (F4 revives)" } else { "" };
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
