//! Window-only layer: meshes, camera, HUD, keyboard → `Intent`, telegraph gizmos.
//! Gameplay never depends on anything here, so the same game runs headless.

use ai_core::highlight::AiOverlay;
use ai_core::prelude::*;
use ai_core::sim;
use bevy::prelude::*;

use crate::gameplay::combat::{Action, AttackKind, Health, Shape, Stamina, Team};
use crate::gameplay::intent::Intent;
use crate::gameplay::{Boss, Pillar, Player, Skeleton, ARENA_HALF};

pub struct VisualsPlugin;

impl Plugin for VisualsPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ClearColor(Color::srgb(0.05, 0.06, 0.09)))
            // Before `Startup`: the arena spawns there, and `dress` needs the palette.
            .add_systems(PreStartup, setup_assets)
            .add_systems(Startup, (setup_scene, setup_hud).chain())
            .add_observer(dress::<Player>)
            .add_observer(dress::<Skeleton>)
            .add_observer(dress::<Boss>)
            .add_observer(dress::<Pillar>)
            .add_systems(PreUpdate, keyboard_to_intent)
            .add_systems(Update, (dev_keys, follow_camera, update_hud, telegraphs));
    }
}

#[derive(Resource)]
struct Palette {
    capsule: Handle<Mesh>,
    boss: Handle<Mesh>,
    nose: Handle<Mesh>,
    pillar: Handle<Mesh>,
    player: Handle<StandardMaterial>,
    skeleton: Handle<StandardMaterial>,
    boss_mat: Handle<StandardMaterial>,
    stone: Handle<StandardMaterial>,
    dark: Handle<StandardMaterial>,
}

fn setup_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<StandardMaterial>>) {
    let mut mat = |c: Color| mats.add(StandardMaterial { base_color: c, perceptual_roughness: 0.8, ..default() });
    commands.insert_resource(Palette {
        capsule: meshes.add(Capsule3d::new(0.45, 0.9)),
        boss: meshes.add(Capsule3d::new(1.0, 1.6)),
        nose: meshes.add(Cuboid::new(0.25, 0.25, 0.5)),
        pillar: meshes.add(Cuboid::new(2.0, 4.0, 2.0)),
        player: mat(Color::srgb(0.25, 0.55, 1.0)),
        skeleton: mat(Color::srgb(0.85, 0.82, 0.7)),
        boss_mat: mat(Color::srgb(0.55, 0.85, 1.0)),
        stone: mat(Color::srgb(0.35, 0.36, 0.4)),
        dark: mat(Color::srgb(0.1, 0.1, 0.12)),
    });
}

fn setup_scene(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut mats: ResMut<Assets<StandardMaterial>>) {
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(ARENA_HALF * 2.0, ARENA_HALF * 2.0))),
        MeshMaterial3d(mats.add(StandardMaterial { base_color: Color::srgb(0.16, 0.18, 0.22), ..default() })),
    ));
    commands.spawn((
        DirectionalLight { illuminance: 9000.0, shadow_maps_enabled: true, ..default() },
        Transform::from_xyz(8.0, 18.0, 6.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.insert_resource(GlobalAmbientLight { brightness: 250.0, ..default() });
    let start = Vec3::new(0.0, 0.0, 12.0);
    let camera = Transform::from_translation(start + CAMERA_OFFSET).looking_at(start + CAMERA_LOOK_AHEAD, Vec3::Y);
    commands.spawn((Camera3d::default(), AiCamera, camera));
}

/// Gives a freshly spawned gameplay entity its looks (works for `spawn` from tools too).
fn dress<T: Component>(add: On<Add<T>>, mut commands: Commands, palette: Res<Palette>, q: Query<(Has<Player>, Has<Skeleton>, Has<Boss>)>) {
    let entity = add.event().entity;
    let Ok((is_player, is_skeleton, is_boss)) = q.get(entity) else { return };
    let (mesh, mat, lift) = if is_player {
        (palette.capsule.clone(), palette.player.clone(), 0.9)
    } else if is_skeleton {
        (palette.capsule.clone(), palette.skeleton.clone(), 0.9)
    } else if is_boss {
        (palette.boss.clone(), palette.boss_mat.clone(), 1.8)
    } else {
        // Pillar: a 4 m box standing on the entity's origin.
        commands.entity(entity).insert(Visibility::default()).with_child((
            Mesh3d(palette.pillar.clone()),
            MeshMaterial3d(palette.stone.clone()),
            Transform::from_xyz(0.0, 2.0, 0.0),
        ));
        return;
    };
    commands.entity(entity).insert(Visibility::default()).with_children(|c| {
        c.spawn((Mesh3d(mesh), MeshMaterial3d(mat), Transform::from_xyz(0.0, lift, 0.0)));
        c.spawn((Mesh3d(palette.nose.clone()), MeshMaterial3d(palette.dark.clone()), Transform::from_xyz(0.0, lift + 0.3, -0.55)));
    });
}

/// Human input becomes `Intent`, exactly like an agent's tool calls do.
fn keyboard_to_intent(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut q: Query<(&mut Intent, &Transform), With<Player>>,
) {
    let Ok((mut intent, tf)) = q.single_mut() else { return };
    let mut dir = Vec3::ZERO;
    for (key, d) in [
        (KeyCode::KeyW, Vec3::NEG_Z),
        (KeyCode::ArrowUp, Vec3::NEG_Z),
        (KeyCode::KeyS, Vec3::Z),
        (KeyCode::ArrowDown, Vec3::Z),
        (KeyCode::KeyA, Vec3::NEG_X),
        (KeyCode::ArrowLeft, Vec3::NEG_X),
        (KeyCode::KeyD, Vec3::X),
        (KeyCode::ArrowRight, Vec3::X),
    ] {
        if keys.pressed(key) {
            dir += d;
        }
    }
    let human_moving = dir != Vec3::ZERO;
    if human_moving {
        // The human takes over: drop whatever an agent ordered.
        intent.orders.clear();
        intent.move_dir = dir.normalize();
    } else if intent.orders.is_empty() {
        intent.move_dir = Vec3::ZERO;
    }
    if keys.just_pressed(KeyCode::Space) {
        intent.dodge = Some(if human_moving { dir } else { *tf.forward() });
    }
    if keys.just_pressed(KeyCode::KeyJ) || mouse.just_pressed(MouseButton::Left) {
        intent.attack = Some(AttackKind::Sword);
    }
}

/// P: pause · N / `.`: one tick while paused · F1: id overlay · R: revive ·
/// F12: marked screenshot into shots/.
fn dev_keys(world: &mut World) {
    let (pause, step, overlay, revive, shot) = {
        let keys = world.resource::<ButtonInput<KeyCode>>();
        (
            keys.just_pressed(KeyCode::KeyP),
            keys.just_pressed(KeyCode::KeyN) || keys.just_pressed(KeyCode::Period),
            keys.just_pressed(KeyCode::F1),
            keys.just_pressed(KeyCode::KeyR),
            keys.just_pressed(KeyCode::F12),
        )
    };
    if pause {
        let tool = if sim::is_paused(world) { "resume" } else { "pause" };
        ai_core::call_tool(world, "keyboard", tool, serde_json::Value::Null);
    }
    if step && sim::is_paused(world) {
        ai_core::call_tool(world, "keyboard", "step", serde_json::json!({"ticks": 1}));
    }
    if overlay {
        let mut o = world.resource_mut::<AiOverlay>();
        o.enabled = !o.enabled;
    }
    if revive {
        ai_core::call_tool(world, "keyboard", "heal", serde_json::Value::Null);
    }
    if shot {
        // The path lands in the journal once the file is written.
        ai_core::call_tool(world, "keyboard", "shot", serde_json::Value::Null);
    }
}

/// Behind and above the player, looking a bit ahead (north) so the arena is in frame.
const CAMERA_OFFSET: Vec3 = Vec3::new(0.0, 20.0, 12.0);
const CAMERA_LOOK_AHEAD: Vec3 = Vec3::new(0.0, 0.0, -5.0);

fn follow_camera(
    time: Res<Time<Real>>,
    player: Query<&Transform, (With<Player>, Without<Camera3d>)>,
    mut cam: Query<&mut Transform, With<Camera3d>>,
) {
    let (Ok(p), Ok(mut c)) = (player.single(), cam.single_mut()) else { return };
    let t = 1.0 - (-6.0 * time.delta_secs()).exp();
    c.translation = c.translation.lerp(p.translation + CAMERA_OFFSET, t);
    let focus = c.translation - CAMERA_OFFSET + CAMERA_LOOK_AHEAD;
    c.look_at(focus, Vec3::Y);
}

#[derive(Component)]
struct HudText;

#[derive(Component)]
struct BossText;

fn setup_hud(mut commands: Commands) {
    commands.spawn((
        Node { position_type: PositionType::Absolute, top: Val::Px(10.0), left: Val::Px(12.0), ..default() },
        Text::new(""),
        TextFont::from_font_size(17.0),
        HudText,
    ));
    commands.spawn((
        Node { position_type: PositionType::Absolute, top: Val::Px(10.0), right: Val::Px(12.0), ..default() },
        Text::new(""),
        TextFont::from_font_size(17.0),
        TextColor(Color::srgb(0.6, 0.9, 1.0)),
        BossText,
    ));
}

fn update_hud(
    tick: Res<SimTick>,
    time: Res<Time<Virtual>>,
    player: Query<(&AiId, &Health, &Stamina, &Action), With<Player>>,
    boss: Query<(&AiId, &Health, &Boss, &Name)>,
    mut hud: Query<&mut Text, (With<HudText>, Without<BossText>)>,
    mut boss_hud: Query<&mut Text, (With<BossText>, Without<HudText>)>,
) {
    if let (Ok(mut text), Ok((id, hp, st, action))) = (hud.single_mut(), player.single()) {
        let state = if time.is_paused() { "PAUSED" } else { "running" };
        let dead = if matches!(action, Action::Dead { .. }) { "   DEAD — R to revive" } else { "" };
        text.0 = format!(
            "{id}  HP {:.0}/{:.0}  STAM {:.0}/{:.0}{dead}\ntick {}  {state}\nWASD move | Space dodge | J/LMB attack\nP pause | N step | F1 ids | F12 shot",
            hp.current, hp.max, st.current, st.max, tick.0
        );
    }
    if let Ok(mut text) = boss_hud.single_mut() {
        text.0 = boss
            .iter()
            .map(|(id, hp, b, name)| format!("{id} {name}  phase {}  {:.0}/{:.0}", b.phase, hp.current, hp.max))
            .collect::<Vec<_>>()
            .join("\n");
    }
}

/// Windup shapes on the ground, health bars over heads.
fn telegraphs(mut gizmos: Gizmos, q: Query<(&Transform, &Action, &Team, Option<&Health>)>) {
    let ground = Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    for (tf, action, team, health) in &q {
        let p = tf.translation;
        if let Action::Windup { attack, left } = action {
            let spec = attack.spec();
            let progress = (1.0 - left / spec.windup.max(0.01)).clamp(0.0, 1.0);
            let color = if *team == Team::Monster { Color::srgb(1.0, 0.25, 0.2) } else { Color::srgb(0.4, 0.8, 1.0) };
            match spec.shape {
                Shape::Circle { radius } => {
                    gizmos.circle(Isometry3d::new(p + Vec3::Y * 0.05, ground), radius, color);
                    gizmos.circle(Isometry3d::new(p + Vec3::Y * 0.05, ground), radius * progress, color);
                }
                Shape::Cone { range, half_angle_deg } => {
                    let fwd = *tf.forward();
                    let steps = 8;
                    let mut prev = None;
                    for i in 0..=steps {
                        let a = (-half_angle_deg + 2.0 * half_angle_deg * i as f32 / steps as f32).to_radians();
                        let edge = p + Quat::from_rotation_y(a) * fwd * range + Vec3::Y * 0.05;
                        if i == 0 || i == steps {
                            gizmos.line(p + Vec3::Y * 0.05, edge, color);
                        }
                        if let Some(prev) = prev {
                            gizmos.line(prev, edge, color);
                        }
                        prev = Some(edge);
                    }
                    gizmos.line(p + Vec3::Y * 0.05, p + fwd * range * progress + Vec3::Y * 0.05, color);
                }
            }
        }
        if let Some(h) = health
            && h.current < h.max
            && !matches!(action, Action::Dead { .. })
        {
            let top = p + Vec3::Y * if h.max > 1000.0 { 4.0 } else { 2.3 };
            let w = if h.max > 1000.0 { 2.4 } else { 1.0 };
            let frac = (h.current / h.max).clamp(0.0, 1.0);
            gizmos.line(top - Vec3::X * w, top + Vec3::X * w, Color::srgb(0.25, 0.05, 0.05));
            gizmos.line(top - Vec3::X * w, top - Vec3::X * w + Vec3::X * 2.0 * w * frac, Color::srgb(0.2, 0.9, 0.3));
        }
    }
}
