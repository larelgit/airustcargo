//! The marks drawn (feature `render`): each laid mark is a decal, Bevy's `ForwardDecal` with a
//! `StandardMaterial` (so it is lit and shadowed as the surface under it is). The decal's quad is turned
//! so that its +Y is the surface's normal, and Bevy's shader projects it along that normal onto what the
//! depth prepass shows within the kind's `depth` of the quad, so a mark lies on a wall, a ceiling or a floor
//! alike. The telegraph decals use the same projector with an unlit zone material and project straight
//! down only; a mark needs lighting, a picture and any direction, which the standard material has.
//!
//! A kind keeps one decal entity per slot of its pool, made as the pool grows (at most its `max_live`), and
//! hidden while the slot is empty. Only the picture's: no `AiId`, nothing a replay or a snapshot sees.

use std::collections::BTreeMap;

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::pbr::decal::{ForwardDecal, ForwardDecalMaterial, ForwardDecalMaterialExt};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};

use crate::mark::{Mark, MarkDef};
use crate::{clock, Marks};

/// A mark's material: the standard one, as a forward decal.
pub type MarkMaterial = ForwardDecalMaterial<StandardMaterial>;

/// The decal of slot `slot` of the marks of kind `kind`.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct MarkDecal {
    pub kind: String,
    pub slot: usize,
}

/// A slot's decal, and what it shows now.
struct SlotView {
    entity: Entity,
    material: Handle<MarkMaterial>,
    /// The mark it shows and the opacity written into its material (0 to 255); `None` while it is hidden.
    shown: Option<(Mark, u8)>,
}

/// The decals of one kind, as made for its definition.
struct KindView {
    made_for: MarkDef,
    texture: Handle<Image>,
    slots: Vec<SlotView>,
}

/// The decals by kind.
#[derive(Resource, Default)]
pub struct MarkViews {
    kinds: BTreeMap<String, KindView>,
    /// The soft round spot a mark without a picture wears.
    spot: Option<Handle<Image>>,
}

impl MarkViews {
    /// Decal entities made so far, shown or hidden.
    pub fn made(&self) -> usize {
        self.kinds.values().map(|k| k.slots.len()).sum()
    }
}

/// A soft white round spot, opaque in the middle and clear at its rim: the picture of a mark that has none.
fn soft_spot() -> Image {
    const SIDE: u32 = 64;
    let mut data = Vec::with_capacity((SIDE * SIDE * 4) as usize);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (dx, dy) = ((x as f32 + 0.5) / SIDE as f32 * 2.0 - 1.0, (y as f32 + 0.5) / SIDE as f32 * 2.0 - 1.0);
            let r = (dx * dx + dy * dy).sqrt();
            let t = ((r - 0.35) / 0.65).clamp(0.0, 1.0);
            let alpha = 1.0 - t * t * (3.0 - 2.0 * t);
            data.extend([255, 255, 255, (alpha * 255.0).round() as u8]);
        }
    }
    let size = Extent3d { width: SIDE, height: SIDE, depth_or_array_layers: 1 };
    Image::new(size, TextureDimension::D2, data, TextureFormat::Rgba8UnormSrgb, RenderAssetUsages::default())
}

/// The material of a kind's decal: its colour (clear until a mark shows through it), its picture, its roughness.
fn material_of(def: &MarkDef, texture: Handle<Image>) -> MarkMaterial {
    let [r, g, b, _] = def.rgba();
    ForwardDecalMaterial {
        base: StandardMaterial {
            base_color: Color::srgba_u8(r, g, b, 0),
            base_color_texture: Some(texture),
            alpha_mode: AlphaMode::Blend,
            perceptual_roughness: def.roughness,
            reflectance: 0.3,
            unlit: def.unlit,
            ..default()
        },
        extension: ForwardDecalMaterialExt { depth_fade_factor: def.depth },
    }
}

/// The opacity of `mark` now, 0 to 255: the kind's colour alpha times its fades.
fn opacity(def: &MarkDef, mark: &Mark, now: f64) -> u8 {
    (f32::from(def.rgba()[3]) * def.alpha(mark.age(now))).round().clamp(0.0, 255.0) as u8
}

/// Every mark where it was laid, as faint as its age makes it; an empty slot's decal hidden; the decals of
/// kinds that are gone (or changed) taken down and made again.
pub(crate) fn draw_marks(world: &mut World) {
    let now = clock(world);
    world.resource_scope(|world, mut marks: Mut<Marks>| {
        world.resource_scope(|world, mut views: Mut<MarkViews>| {
            let spot = views.spot.get_or_insert_with(|| world.resource_mut::<Assets<Image>>().add(soft_spot())).clone();
            let stale: Vec<String> = views
                .kinds
                .iter()
                .filter(|(id, view)| marks.kinds.get(*id).is_none_or(|pool| pool.def != view.made_for))
                .map(|(id, _)| id.clone())
                .collect();
            for id in stale {
                for slot in views.kinds.remove(&id).map(|view| view.slots).unwrap_or_default() {
                    if let Ok(entity) = world.get_entity_mut(slot.entity) {
                        entity.despawn();
                    }
                }
            }
            for (id, pool) in &mut marks.kinds {
                pool.expire(now);
                let def = &pool.def;
                let view = views.kinds.entry(id.clone()).or_insert_with(|| KindView {
                    made_for: def.clone(),
                    texture: match &def.texture {
                        Some(path) => world.resource::<AssetServer>().load(path.clone()),
                        None => spot.clone(),
                    },
                    slots: Vec::new(),
                });
                while view.slots.len() < pool.slots.len() {
                    let slot = view.slots.len();
                    let material = world.resource_mut::<Assets<MarkMaterial>>().add(material_of(def, view.texture.clone()));
                    let entity = world
                        .spawn((
                            Name::new(format!("mark {id} #{slot}")),
                            MarkDecal { kind: id.clone(), slot },
                            ForwardDecal,
                            MeshMaterial3d(material.clone()),
                            Transform::default(),
                            Visibility::Hidden,
                            NotShadowCaster,
                        ))
                        .id();
                    view.slots.push(SlotView { entity, material, shown: None });
                }
                for (slot, view) in pool.slots.iter().zip(&mut view.slots) {
                    let Some(mark) = slot else {
                        if view.shown.take().is_some()
                            && let Some(mut visibility) = world.get_mut::<Visibility>(view.entity)
                        {
                            visibility.set_if_neq(Visibility::Hidden);
                        }
                        continue;
                    };
                    let alpha = opacity(def, mark, now);
                    if view.shown == Some((*mark, alpha)) {
                        continue;
                    }
                    if view.shown.map(|(shown, _)| shown) != Some(*mark) {
                        if let Some(mut transform) = world.get_mut::<Transform>(view.entity) {
                            *transform = mark.transform();
                        }
                        if let Some(mut visibility) = world.get_mut::<Visibility>(view.entity) {
                            visibility.set_if_neq(Visibility::Inherited);
                        }
                    }
                    let [r, g, b, _] = def.rgba();
                    if let Some(mut material) = world.resource_mut::<Assets<MarkMaterial>>().get_mut(&view.material) {
                        material.base.base_color = Color::srgba_u8(r, g, b, alpha);
                    }
                    view.shown = Some((*mark, alpha));
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_spot_is_clear_at_its_rim_and_solid_in_the_middle() {
        let spot = soft_spot();
        let alpha = |x: u32, y: u32| spot.get_color_at(x, y).unwrap().alpha();
        assert!(alpha(32, 32) > 0.98 && alpha(0, 0) < 0.01 && alpha(63, 32) < 0.05, "{} {} {}", alpha(32, 32), alpha(0, 0), alpha(63, 32));
    }

    #[test]
    fn a_marks_opacity_is_its_colours_alpha_times_its_fades() {
        let def = MarkDef { color: "#6e0f12cc".into(), fade_in: 1.0, ..MarkDef::default() };
        let mark = Mark { born: 10.0, point: Vec3::ZERO, normal: Vec3::Y, size: 0.3, spin: 0.0, seq: 0 };
        assert_eq!([opacity(&def, &mark, 10.0), opacity(&def, &mark, 10.5), opacity(&def, &mark, 11.0), opacity(&def, &mark, 99.0)], [0, 102, 204, 204]);
    }
}
