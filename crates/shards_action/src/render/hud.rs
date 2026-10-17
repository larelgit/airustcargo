//! Off-screen warnings (TZ E2.6 consumer): a marker on the screen edge that points toward an
//! attack about to land where the camera does not look.
//!
//! [`threat_indicators`] gives, for the `LocalPlayer` and the active camera's forward, the
//! soonest hostile zone of each attacker with its direction. Zones whose center is on screen
//! need no marker (the telegraph on the ground shows them), the others get one: a pointing
//! chevron and the attack's symbol, so it reads without colour, growing and blinking faster as
//! the hit lands. [`edge_marker`] places it. Bevy UI nodes, pooled: a marker entity is reused
//! for the next threat.

use bevy::prelude::*;
use bevy::text::{Font, FontSize, FontSource};

use shards_settings::FlashLevel;

use crate::draw::{self, edge_marker, palette, EdgeMarker};
use crate::zone::{threat_indicators, Indicator};

/// How the off-screen warnings look.
#[derive(Resource, Clone, Debug)]
pub struct ThreatHud {
    pub enabled: bool,
    /// A marker shows for attacks landing within this many seconds.
    pub horizon: f32,
    /// Distance from the screen edge to the marker's middle (logical pixels).
    pub margin: f32,
    /// The marker's size (logical pixels).
    pub size: f32,
    /// A font with the symbols of the game's telegraphs (`⚠`, `↯`). The engine's default font
    /// (Fira Sans) has neither; without a font those are shown as ASCII stand-ins (`!`, `/`).
    pub font: Option<Handle<Font>>,
    /// How much the markers flicker (the player's accessibility setting).
    pub flashes: FlashLevel,
}

impl Default for ThreatHud {
    fn default() -> Self {
        Self { enabled: true, horizon: 3.0, margin: 56.0, size: 52.0, font: None, flashes: FlashLevel::Full }
    }
}

/// The warnings to show this frame.
#[derive(Resource, Default, Debug)]
pub struct Threats {
    /// Off-screen threats with their place on the screen edge, soonest first.
    pub markers: Vec<(Indicator, EdgeMarker)>,
}

#[derive(Component)]
struct ThreatRoot;

#[derive(Component)]
struct ThreatMarker {
    label: Entity,
    pointer: Entity,
}

#[derive(Component)]
struct ThreatLabel;

#[derive(Component)]
struct ThreatPointer;

#[derive(Resource, Default)]
struct ThreatPool {
    root: Option<Entity>,
    markers: Vec<Entity>,
}

pub(crate) fn plugin(app: &mut App) {
    app.init_resource::<ThreatHud>()
        .init_resource::<Threats>()
        .init_resource::<ThreatPool>()
        .add_systems(Update, (compute, update_ui).chain().run_if(ai_core::display::drawn));
}

/// Whether `p` lies inside a viewport of `size` (logical pixels).
fn on_screen(p: Vec2, size: Vec2) -> bool {
    p.x >= 0.0 && p.y >= 0.0 && p.x <= size.x && p.y <= size.y
}

/// Where the zone behind an indicator is, in the world: on the ground plane, `distance`
/// meters from the viewer at `angle_deg` from the view direction (the inverse of what
/// [`threat_indicators`] measures), at chest height.
pub fn indicator_point(here: Vec3, view_forward: Vec3, indicator: &Indicator) -> Vec3 {
    let fwd = Vec3::new(view_forward.x, 0.0, view_forward.z).try_normalize().unwrap_or(Vec3::NEG_Z);
    let right = Vec3::new(-fwd.z, 0.0, fwd.x);
    let a = indicator.angle_deg.to_radians();
    here + (right * ops::sin(a) + fwd * ops::cos(a)) * indicator.distance + Vec3::Y
}

/// `Update`: the threats the player cannot see.
fn compute(world: &mut World) {
    let hud = world.resource::<ThreatHud>().clone();
    let mut markers = Vec::new();
    if hud.enabled
        && let Some(viewer) = super::local_player(world)
        && let Some(cam) = super::active_camera(world)
        && let (Some(camera), Some(gt), Some(here)) = (
            world.get::<Camera>(cam).cloned(),
            world.get::<GlobalTransform>(cam).copied(),
            world.get::<Transform>(viewer).map(|t| t.translation),
        )
        && let Some(viewport) = camera.logical_viewport_size()
    {
        let forward = *gt.forward();
        for indicator in threat_indicators(world, viewer, forward) {
            if indicator.lands_in > hud.horizon {
                continue;
            }
            let seen = camera
                .world_to_viewport(&gt, indicator_point(here, forward, &indicator))
                .is_ok_and(|p| on_screen(p, viewport));
            if !seen {
                let place = edge_marker(indicator.angle_deg, viewport, hud.margin);
                markers.push((indicator, place));
            }
        }
    }
    world.resource_mut::<Threats>().markers = markers;
}

/// The marker's text: the attack's symbol, as the font allows.
fn label_of(symbol: &str, hud: &ThreatHud) -> String {
    if hud.font.is_some() { symbol.to_owned() } else { draw::ascii_symbol(symbol) }
}

fn spawn_marker(commands: &mut Commands, root: Entity, hud: &ThreatHud) -> Entity {
    let font = hud.font.clone().map_or(FontSource::default(), FontSource::Handle);
    let label = commands
        .spawn((
            ThreatLabel,
            Text::new("!"),
            TextFont { font, font_size: FontSize::Px(hud.size * 0.5), ..default() },
            TextColor(Color::WHITE),
        ))
        .id();
    let chevron = Node {
        position_type: PositionType::Absolute,
        width: Val::Px(14.0),
        height: Val::Px(14.0),
        border: UiRect { top: Val::Px(4.0), right: Val::Px(4.0), ..default() },
        ..default()
    };
    let pointer = commands.spawn((ThreatPointer, chevron, BorderColor::all(Color::WHITE), UiTransform::default())).id();
    let marker = commands
        .spawn((
            ThreatMarker { label, pointer },
            Node {
                position_type: PositionType::Absolute,
                width: Val::Px(hud.size),
                height: Val::Px(hud.size),
                border: UiRect::all(Val::Px(3.0)),
                border_radius: BorderRadius::all(Val::Px(hud.size * 0.5)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                display: Display::None,
                ..default()
            },
            BackgroundColor(Color::NONE),
            BorderColor::all(Color::WHITE),
        ))
        .add_children(&[label, pointer])
        .id();
    commands.entity(root).add_child(marker);
    marker
}

/// Shows `Threats`: spawns the root and markers the first time they are needed, moves and
/// colours the pooled ones, hides the rest.
#[allow(clippy::too_many_arguments, reason = "a Bevy system: one parameter per thing it touches")]
fn update_ui(
    mut commands: Commands,
    time: Res<Time<Real>>,
    hud: Res<ThreatHud>,
    threats: Res<Threats>,
    mut pool: ResMut<ThreatPool>,
    roots: Query<(), With<ThreatRoot>>,
    mut markers: Query<(&ThreatMarker, &mut Node, &mut BackgroundColor, &mut BorderColor), Without<ThreatPointer>>,
    mut pointers: Query<(&mut Node, &mut BorderColor, &mut UiTransform), With<ThreatPointer>>,
    mut labels: Query<(&mut Text, &mut TextColor), With<ThreatLabel>>,
) {
    let wanted = threats.markers.len();
    if wanted == 0 && pool.markers.is_empty() {
        return;
    }
    let root = match pool.root.filter(|r| roots.contains(*r)) {
        Some(root) => root,
        None => {
            // The root went (UI rebuilt): start the pool again.
            pool.markers.clear();
            let root = commands
                .spawn((
                    ThreatRoot,
                    Name::new("threat markers"),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(0.0),
                        top: Val::Px(0.0),
                        width: Val::Percent(100.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    GlobalZIndex(1000),
                ))
                .id();
            pool.root = Some(root);
            root
        }
    };
    while pool.markers.len() < wanted {
        let marker = spawn_marker(&mut commands, root, &hud);
        pool.markers.push(marker);
    }
    let now = time.elapsed_secs();
    for (slot, &entity) in pool.markers.iter().enumerate() {
        let Ok((marker, mut node, mut background, mut border)) = markers.get_mut(entity) else { continue };
        let Some((indicator, place)) = threats.markers.get(slot) else {
            if node.display != Display::None {
                node.display = Display::None;
            }
            continue;
        };
        let urgency = draw::intensity(indicator.lands_in, hud.horizon);
        let flicker = draw::pulse_at(urgency, now, hud.flashes);
        let size = hud.size * (0.85 + 0.35 * urgency);
        let color = palette::rgba(palette::DANGER, (0.55 + 0.45 * urgency) * flicker);
        node.display = Display::Flex;
        node.width = Val::Px(size);
        node.height = Val::Px(size);
        node.left = Val::Px(place.position.x - size * 0.5);
        node.top = Val::Px(place.position.y - size * 0.5);
        node.border_radius = BorderRadius::all(Val::Px(size * 0.5));
        *background = BackgroundColor(Color::srgba(0.05, 0.02, 0.02, 0.55 + 0.3 * urgency));
        *border = BorderColor::all(color);
        if let Ok((mut text, mut text_color)) = labels.get_mut(marker.label) {
            let symbol = label_of(&indicator.symbol, &hud);
            if text.0 != symbol {
                text.0 = symbol;
            }
            text_color.0 = Color::srgba(1.0, 0.95, 0.9, 0.7 + 0.3 * urgency);
        }
        if let Ok((mut chevron, mut chevron_color, mut transform)) = pointers.get_mut(marker.pointer) {
            // The chevron sits on the outer side of the ring and points away from the center.
            let out = place.direction;
            let reach = size * 0.5 + 3.0;
            chevron.left = Val::Px(size * 0.5 - 7.0 + out.x * reach - 3.0);
            chevron.top = Val::Px(size * 0.5 - 7.0 + out.y * reach - 3.0);
            *chevron_color = BorderColor::all(color);
            // The drawn corner points up and to the right (45 degrees clockwise from up).
            transform.rotation = Rot2::degrees(place.angle_deg() - 45.0);
        }
    }
}

#[cfg(test)]
mod tests {
    use ai_core::AiId;

    use super::*;

    fn indicator(angle_deg: f32, distance: f32) -> Indicator {
        Indicator {
            source: Entity::PLACEHOLDER,
            source_id: Some(AiId(5)),
            action: "slam".into(),
            angle_deg,
            lands_in: 0.5,
            symbol: "⚠".into(),
            distance,
            inside: false,
        }
    }

    #[test]
    fn the_point_of_an_indicator_is_where_threat_indicators_measured_it() {
        let here = Vec3::new(1.0, 0.0, 2.0);
        for forward in [Vec3::NEG_Z, Vec3::X, Vec3::new(1.0, 0.3, -1.0)] {
            let flat_fwd = Vec3::new(forward.x, 0.0, forward.z).normalize();
            let right = Vec3::new(-flat_fwd.z, 0.0, flat_fwd.x);
            for angle in [-170.0f32, -90.0, -20.0, 0.0, 45.0, 90.0, 179.0] {
                let p = indicator_point(here, forward, &indicator(angle, 7.0));
                let to = Vec3::new(p.x - here.x, 0.0, p.z - here.z);
                assert!((to.length() - 7.0).abs() < 1e-4);
                let back = ops::atan2(to.dot(right), to.dot(flat_fwd)).to_degrees();
                assert!((back - angle).abs() < 1e-2, "{forward}: {angle} -> {back}");
                assert!((p.y - 1.0).abs() < 1e-6, "chest height");
            }
        }
    }

    #[test]
    fn labels_fall_back_to_ascii_without_a_font() {
        let mut hud = ThreatHud::default();
        assert_eq!(label_of("⚠", &hud), "!");
        assert_eq!(label_of("↯", &hud), "/");
        hud.font = Some(Handle::default());
        assert_eq!(label_of("⚠", &hud), "⚠");
    }

    #[test]
    fn viewport_bounds() {
        let size = Vec2::new(1280.0, 720.0);
        assert!(on_screen(Vec2::new(640.0, 360.0), size));
        assert!(on_screen(Vec2::ZERO, size) && on_screen(size, size));
        assert!(!on_screen(Vec2::new(-1.0, 10.0), size) && !on_screen(Vec2::new(10.0, 721.0), size));
    }
}
