//! Active zones: NPC AI runs only near players.
//!
//! Every tick, in `SimSet::Input` (before orders are followed), each character that is not a
//! player (it has `Intent`, no `PlayerId`, no `AiPlayer`) gets [`ActiveZone`] within the
//! active radius of some player and [`Dormant`] beyond it, with a margin before an active
//! one goes dormant, so a character at the edge does not flicker. NPC brains and nav agents
//! skip `Dormant` characters. Zones are kept while a region is loaded (its `active_radius`,
//! else [`WorldSettings::active_radius`], else [`DEFAULT_ACTIVE_RADIUS`]) or when the game
//! sets a radius; otherwise nothing is marked and everything runs.

use ai_core::{AiId, AiPlayer};
use bevy::prelude::*;
use shards_core::{Intent, PlayerId};

use crate::region::ActiveRegion;
use crate::WorldSettings;

/// The radius (m) of a region that names none.
pub const DEFAULT_ACTIVE_RADIUS: f32 = 96.0;

/// Near a player: NPC brains and nav agents run.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct ActiveZone;

/// Far from every player: NPC brains and nav agents skip it.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct Dormant;

/// The active radius now: the region's, the settings', the default; `None` when neither a
/// region nor the settings ask for zones.
pub fn active_radius(settings: &WorldSettings, region: Option<&ActiveRegion>) -> Option<f32> {
    match region {
        Some(region) => Some(region.active_radius.or(settings.active_radius).unwrap_or(DEFAULT_ACTIVE_RADIUS)),
        None => settings.active_radius,
    }
}

type Players<'w, 's> = Query<'w, 's, &'static Transform, Or<(With<PlayerId>, With<AiPlayer>)>>;

type Characters<'w, 's> =
    Query<'w, 's, (Entity, &'static AiId, &'static Transform, Has<ActiveZone>, Has<Dormant>), (With<Intent>, Without<PlayerId>, Without<AiPlayer>)>;

/// `SimSet::Input`: marks every non-player character active or dormant.
pub fn update_zones(
    mut commands: Commands,
    settings: Res<WorldSettings>,
    region: Option<Res<ActiveRegion>>,
    players: Players,
    characters: Characters,
) {
    let mut order: Vec<(AiId, Entity, Vec3, bool, bool)> =
        characters.iter().map(|(e, id, tf, active, dormant)| (*id, e, tf.translation, active, dormant)).collect();
    order.sort_unstable_by_key(|(id, ..)| *id);
    let Some(radius) = active_radius(&settings, region.as_deref()) else {
        for (_, entity, _, active, dormant) in order {
            if active || dormant {
                commands.entity(entity).remove::<(ActiveZone, Dormant)>();
            }
        }
        return;
    };
    let players: Vec<Vec3> = players.iter().map(|tf| tf.translation).collect();
    for (_, entity, at, active, dormant) in order {
        let reach = if active { radius + settings.active_margin } else { radius };
        let near = players.is_empty() || players.iter().any(|p| Vec2::new(p.x - at.x, p.z - at.z).length() <= reach);
        match (near, active, dormant) {
            (true, true, false) | (false, false, true) => {}
            (true, ..) => {
                commands.entity(entity).remove::<Dormant>().insert(ActiveZone);
            }
            (false, ..) => {
                commands.entity(entity).remove::<ActiveZone>().insert(Dormant);
            }
        }
    }
}
