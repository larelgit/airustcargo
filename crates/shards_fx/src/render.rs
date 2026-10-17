//! The effects drawn (feature `render`): each effect's pool of meshes and its material, the
//! particles placed every frame from their ages, and the telegraph decals placed over the zones
//! the ground shows. Only while something is drawn (a window or `--offscreen`). Nothing here
//! touches the simulation: the meshes and the decals are the picture's, without `AiId`.

use std::collections::BTreeMap;

use ai_core::AiCamera;
use bevy::asset::{embedded_asset, RenderAssetUsages};
use bevy::core_pipeline::prepass::DepthPrepass;
use bevy::light::NotShadowCaster;
use bevy::mesh::{MeshTag, MeshVertexBufferLayoutRef, PrimitiveTopology};
use bevy::pbr::decal::{ForwardDecal, ForwardDecalMaterial, ForwardDecalMaterialExt};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, RenderPipelineDescriptor, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;
use bevy::transform::TransformSystems;
use shards_action::draw::{intensity, palette, pulse_at, shown_telegraphs};
use shards_action::render::TelegraphSettings;
use shards_content::ContentDb;

use crate::decal::{DecalShape, ZoneDecal};
use crate::def::{EffectDef, ParticleShape};
use crate::{FxSettings, FxSystems, Particles, Playing};

const PARTICLE_SHADER: &str = "embedded://shards_fx/shaders/particle.wesl";
const ZONE_SHADER: &str = "embedded://shards_fx/shaders/zone.wesl";

/// How far above or below its quad a surface still takes a decal's paint, fading (m): the
/// ground and the feet of who stands on it, not a body or a wall.
pub const DEPTH_FADE: f32 = 0.6;

/// A particle's material, one per effect: the colour comes from each particle's `MeshTag`
/// (sRGB bytes, alpha last), so an effect's particles share it and draw in batches.
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone, PartialEq)]
pub struct ParticleMaterial {
    /// 0 a soft round spot (a quad), 1 a shaded solid (a cube or a shard), 2 a solid lit by
    /// itself.
    #[uniform(0)]
    pub kind: u32,
    /// Added to what is behind (`AlphaMode::Add`), else blended over it.
    pub glow: bool,
}

impl ParticleMaterial {
    pub fn of(def: &EffectDef) -> Self {
        let kind = match (def.shape, def.glow) {
            (ParticleShape::Quad, _) => 0,
            (_, false) => 1,
            (_, true) => 2,
        };
        Self { kind, glow: def.glow }
    }
}

impl Material for ParticleMaterial {
    fn fragment_shader() -> ShaderRef {
        PARTICLE_SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        if self.glow { AlphaMode::Add } else { AlphaMode::Blend }
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }

    fn specialize(
        _: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        _: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        // A shard is thin, and a quad turned away for a frame still shows.
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

/// A telegraph's decal: the zone's shape in its quad, how far the fill has grown and the
/// colour. Drawn by Bevy's forward decal ([`ZoneDecalMaterial`]).
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone, Default, PartialEq)]
pub struct ZoneMaterial {
    /// The palette's colour (linear): warm for an enemy's attack, cool for an ally's.
    #[uniform(0)]
    pub color: LinearRgba,
    /// The shape's numbers: a disc's or a stadium's radius, a sector's range and half angle.
    #[uniform(0)]
    pub shape: Vec4,
    /// Its points in the quad's frame: a disc's or a rectangle's centre, a sector's apex, a
    /// stadium's two ends (`xy`, `zw`); a rectangle's half size (`zw`).
    #[uniform(0)]
    pub points: Vec4,
    /// The quad's half size (`xy`) and where the fill grows from (`zw`).
    #[uniform(0)]
    pub frame: Vec4,
    /// How far the fill has grown (0 to 1), the flicker, the shape's kind, the stripes' spacing.
    #[uniform(0)]
    pub state: Vec4,
    /// What the stripes are measured from: the quad's centre seen from the point the fill grows
    /// from, in world x and z (`xy`), and the quad's facing (`zw`). The stripes keep the world's
    /// 45° and close in on that point as they tighten with the hit.
    #[uniform(0)]
    pub world: Vec4,
}

impl Material for ZoneMaterial {
    fn fragment_shader() -> ShaderRef {
        ZONE_SHADER.into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }

    fn enable_prepass() -> bool {
        false
    }

    fn enable_shadows() -> bool {
        false
    }
}

/// The telegraph decals' material: the zone material as Bevy's forward decal.
pub type ZoneDecalMaterial = ForwardDecalMaterial<ZoneMaterial>;

/// A particle of an effect's pool: slot `slot` of effect `effect`.
#[derive(Component, Clone, Debug, PartialEq, Eq)]
pub struct FxParticle {
    pub effect: String,
    pub slot: usize,
}

/// A telegraph's decal of the pool.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct TelegraphDecal;

/// A depth prepass this module gave a camera for the decals: taken away again with them.
#[derive(Component, Clone, Copy, Debug)]
pub struct FxPrepass;

/// The meshes of an effect's pool, as made for its definition.
struct PoolView {
    made_for: EffectDef,
    entities: Vec<Entity>,
}

/// The pools' meshes and materials, by effect.
#[derive(Resource, Default)]
struct Pools {
    views: BTreeMap<String, PoolView>,
    meshes: Option<[Handle<Mesh>; 3]>,
}

/// The decals made so far (shown or hidden) and what each shows now.
#[derive(Resource, Default)]
struct Decals {
    slots: Vec<(Entity, Handle<ZoneDecalMaterial>)>,
    /// One line per decal on the ground now, for `effects`.
    lines: Vec<String>,
}

pub(crate) fn plugin(app: &mut App) {
    embedded_asset!(app, "shaders/particle.wesl");
    embedded_asset!(app, "shaders/zone.wesl");
    app.add_plugins((MaterialPlugin::<ParticleMaterial>::default(), MaterialPlugin::<ZoneDecalMaterial>::default()))
        .init_resource::<Pools>()
        .init_resource::<Decals>()
        .add_systems(
            PostUpdate,
            (sync_pools, crate::emit_due, draw_particles, draw_decals, prepass_cameras)
                .chain()
                .in_set(FxSystems::Draw)
                .run_if(ai_core::display::drawn),
        )
        // Before transforms propagate, and so before visibility and bounds: a particle placed
        // this frame is drawn where it is this frame.
        .configure_sets(PostUpdate, FxSystems::Draw.before(TransformSystems::Propagate));
}

/// A sliver of crystal a unit long along Y: a stretched, flattened octahedron with flat faces.
fn shard_mesh() -> Mesh {
    let (top, bottom) = (Vec3::new(0.0, 0.5, 0.0), Vec3::new(0.0, -0.5, 0.0));
    let ring = [Vec3::new(0.18, 0.05, 0.0), Vec3::new(0.0, 0.05, 0.11), Vec3::new(-0.18, 0.05, 0.0), Vec3::new(0.0, 0.05, -0.11)];
    let (mut positions, mut normals, mut uvs) = (Vec::new(), Vec::new(), Vec::new());
    for i in 0..4 {
        let (a, b) = (ring[i], ring[(i + 1) % 4]);
        for [p, q, r] in [outward(top, a, b), outward(bottom, a, b)] {
            let n = (q - p).cross(r - p).normalize_or(Vec3::Y);
            for v in [p, q, r] {
                positions.push(v.to_array());
                normals.push(n.to_array());
                uvs.push([0.5 + v.x, 0.5 - v.y]);
            }
        }
    }
    Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default())
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
}

/// A triangle from a tip to two points of the ring, wound to face out (away from the middle).
fn outward(tip: Vec3, a: Vec3, b: Vec3) -> [Vec3; 3] {
    let n = (a - tip).cross(b - tip);
    let middle = (tip + a + b) / 3.0;
    if n.dot(middle) >= 0.0 { [tip, a, b] } else { [tip, b, a] }
}

/// The pools follow content: every effect it defines has a pool of `max_live` hidden meshes
/// (made again when the effect changes, or when something took its meshes away), and the pools
/// of effects that are gone go with their meshes.
fn sync_pools(world: &mut World) {
    let defs: BTreeMap<String, EffectDef> = match world.get_resource::<ContentDb>() {
        Some(db) => db.iter::<EffectDef>().map(|(id, def)| (id.to_owned(), def.clone())).collect(),
        None => return,
    };
    let meshes = world.resource_scope(|world, mut pools: Mut<Pools>| {
        pools
            .meshes
            .get_or_insert_with(|| {
                let mut meshes = world.resource_mut::<Assets<Mesh>>();
                [
                    meshes.add(Rectangle::from_size(Vec2::ONE).mesh().build()),
                    meshes.add(Cuboid::from_length(1.0).mesh().build()),
                    meshes.add(shard_mesh()),
                ]
            })
            .clone()
    });
    let stale: Vec<String> = {
        let pools = world.resource::<Pools>();
        let gone = pools.views.keys().filter(|id| !defs.contains_key(*id)).cloned();
        let changed = defs.iter().filter_map(|(id, def)| {
            let view = pools.views.get(id);
            let fresh = view.is_some_and(|v| v.made_for == *def && v.entities.iter().all(|e| world.get_entity(*e).is_ok()));
            (!fresh).then(|| id.clone())
        });
        gone.chain(changed).collect()
    };
    for id in stale {
        if let Some(old) = world.resource_mut::<Pools>().views.remove(&id) {
            for entity in old.entities {
                if let Ok(entity) = world.get_entity_mut(entity) {
                    entity.despawn();
                }
            }
        }
        let Some(def) = defs.get(&id) else {
            world.resource_mut::<Particles>().effects.remove(&id);
            continue;
        };
        let material = world.resource_mut::<Assets<ParticleMaterial>>().add(ParticleMaterial::of(def));
        let mesh = match def.shape {
            ParticleShape::Quad => meshes[0].clone(),
            ParticleShape::Cube => meshes[1].clone(),
            ParticleShape::Shard => meshes[2].clone(),
        };
        let entities: Vec<Entity> = (0..def.pool_size())
            .map(|slot| {
                world
                    .spawn((
                        Name::new(format!("fx {id} #{slot}")),
                        FxParticle { effect: id.clone(), slot },
                        Mesh3d(mesh.clone()),
                        MeshMaterial3d(material.clone()),
                        MeshTag::new(0),
                        Transform::from_scale(Vec3::ZERO),
                        Visibility::Hidden,
                        NotShadowCaster,
                    ))
                    .id()
            })
            .collect();
        world.resource_mut::<Pools>().views.insert(id.clone(), PoolView { made_for: def.clone(), entities });
        // Live particles of an older definition go: the pool is new.
        let mut particles = world.resource_mut::<Particles>();
        if particles.effects.get(&id).is_none_or(|p| p.def != *def) {
            particles.effects.insert(id.clone(), Playing::new(def));
        }
    }
}

/// `r | g << 8 | b << 16 | a << 24`: what the particle shader unpacks.
pub fn pack(color: [u8; 4]) -> u32 {
    u32::from_le_bytes(color)
}

/// The rotation that turns a quad to the camera: the AI's camera, else the first active one.
fn facing_camera(cameras: &Query<(&Camera, &GlobalTransform, Has<AiCamera>), With<Camera3d>>) -> Quat {
    let mut found: Vec<(bool, Quat)> = cameras.iter().filter(|(c, ..)| c.is_active).map(|(_, t, ai)| (!ai, t.rotation())).collect();
    found.sort_by_key(|(not_ai, _)| *not_ai);
    found.first().map_or(Quat::IDENTITY, |(_, r)| *r)
}

/// Every particle where its age puts it, the size and colour of that age; an empty slot's mesh
/// hidden.
fn draw_particles(
    mut particles: ResMut<Particles>,
    pools: Res<Pools>,
    fixed: Res<Time<Fixed>>,
    cameras: Query<(&Camera, &GlobalTransform, Has<AiCamera>), With<Camera3d>>,
    mut shown: Query<(&mut Transform, &mut Visibility, &mut MeshTag), With<FxParticle>>,
) {
    let now = fixed.elapsed_secs_f64() + fixed.overstep().as_secs_f64();
    let billboard = facing_camera(&cameras);
    for (id, playing) in &mut particles.effects {
        playing.pool.expire(now);
        let Some(view) = pools.views.get(id).filter(|v| v.made_for == playing.def) else { continue };
        for (slot, entity) in playing.pool.slots.iter().zip(&view.entities) {
            let Ok((mut transform, mut visibility, mut tag)) = shown.get_mut(*entity) else { continue };
            match slot {
                Some(particle) => {
                    let look = particle.shown(now, &playing.def, playing.colors);
                    let rotation = if playing.def.shape == ParticleShape::Quad { billboard } else { look.rotation };
                    *transform = Transform { translation: look.position, rotation, scale: Vec3::splat(look.size.max(0.0)) };
                    visibility.set_if_neq(Visibility::Inherited);
                    let packed = pack(look.color);
                    if tag.value != packed {
                        tag.value = packed;
                    }
                }
                None => {
                    visibility.set_if_neq(Visibility::Hidden);
                }
            }
        }
    }
}

/// The ground telegraphs' decals: one per zone the ground shows, over its footprint, the fill
/// grown as far as the hit is near; the rest hidden. Off (`telegraph_decals on=false`) or with
/// the telegraphs off, none; the gizmos draw the fill as they did.
fn draw_decals(world: &mut World) {
    let on = world.resource::<FxSettings>().decals;
    let telegraphs = world.get_resource::<TelegraphSettings>().map(|s| (s.enabled, s.style));
    if let Some(mut settings) = world.get_resource_mut::<TelegraphSettings>()
        && settings.style.decals != on
    {
        settings.style.decals = on;
    }
    let Some((enabled, style)) = telegraphs else { return };
    let shown = if on && enabled { shown_telegraphs(world) } else { Vec::new() };
    let time = world.get_resource::<Time<Real>>().map_or(0.0, |t| t.elapsed_secs());
    // Enough decals for every zone: made once, kept hidden when there are fewer.
    while world.resource::<Decals>().slots.len() < shown.len() {
        let material = world.resource_mut::<Assets<ZoneDecalMaterial>>().add(ZoneDecalMaterial {
            base: ZoneMaterial::default(),
            extension: ForwardDecalMaterialExt { depth_fade_factor: DEPTH_FADE },
        });
        let entity = world
            .spawn((
                Name::new("telegraph decal"),
                TelegraphDecal,
                ForwardDecal,
                MeshMaterial3d(material.clone()),
                Transform::default(),
                Visibility::Hidden,
                NotShadowCaster,
            ))
            .id();
        world.resource_mut::<Decals>().slots.push((entity, material));
    }
    let slots = world.resource::<Decals>().slots.clone();
    let mut lines = Vec::new();
    for (i, (entity, material)) in slots.iter().enumerate() {
        let Some(telegraph) = shown.get(i) else {
            if let Some(mut visibility) = world.get_mut::<Visibility>(*entity) {
                visibility.set_if_neq(Visibility::Hidden);
            }
            continue;
        };
        let zone = &telegraph.zone;
        let decal = ZoneDecal::new(&zone.shape, telegraph.ground_y);
        let k = intensity(zone.lands_in, style.horizon);
        let flicker = pulse_at(k, time, style.flashes);
        let base = if telegraph.hostile { palette::DANGER } else { palette::ALLY };
        let spacing = style.hatch_far + (style.hatch_near - style.hatch_far) * k;
        let wanted = zone_material(&decal, Color::srgb(base[0], base[1], base[2]).to_linear(), k, flicker, spacing);
        if let Some(mut materials) = world.get_resource_mut::<Assets<ZoneDecalMaterial>>()
            && materials.get(material).is_some_and(|m| m.base != wanted)
            && let Some(mut made) = materials.get_mut(material)
        {
            made.base = wanted;
        }
        if let Ok(mut placed) = world.get_entity_mut(*entity) {
            let transform =
                Transform { translation: decal.center, rotation: decal.rotation(), scale: Vec3::new(decal.size.x, 1.0, decal.size.y) };
            if let Some(mut t) = placed.get_mut::<Transform>() {
                t.set_if_neq(transform);
            }
            if let Some(mut visibility) = placed.get_mut::<Visibility>() {
                visibility.set_if_neq(Visibility::Inherited);
            }
        }
        let who = zone.attacker_id.map(|id| format!("{id} ")).unwrap_or_default();
        let whose = if telegraph.hostile { "hostile" } else { "ally" };
        lines.push(format!(
            "decal {who}{}: {} {:.1}×{:.1} m, fill {k:.2}, {whose}",
            zone.action,
            decal.shape.name(),
            decal.size.x,
            decal.size.y
        ));
    }
    world.resource_mut::<Decals>().lines = lines;
}

/// What the zone shader draws for `decal`: `color` (linear), the fill grown to `fill`.
pub fn zone_material(decal: &ZoneDecal, color: LinearRgba, fill: f32, flicker: f32, spacing: f32) -> ZoneMaterial {
    let (shape, points) = match decal.shape {
        DecalShape::Disc { center, radius } => (Vec4::new(radius, 0.0, 0.0, 0.0), Vec4::new(center.x, center.y, 0.0, 0.0)),
        DecalShape::Sector { apex, range, half_angle } => (Vec4::new(range, half_angle, 0.0, 0.0), Vec4::new(apex.x, apex.y, 0.0, 0.0)),
        DecalShape::Rect { center, half } => (Vec4::ZERO, Vec4::new(center.x, center.y, half.x, half.y)),
        DecalShape::Stadium { a, b, radius } => (Vec4::new(radius, 0.0, 0.0, 0.0), Vec4::new(a.x, a.y, b.x, b.y)),
    };
    let half = decal.size * 0.5;
    // The stripes are measured from where the fill grows from, not from the world's origin: their
    // spacing tightens as the hit nears, and from the origin a zone 100 m out would race hundreds
    // of stripes past in a second; from inside the zone no more go by than fit across it.
    let right = Vec3::new(-decal.facing.z, 0.0, decal.facing.x);
    let anchor = right * decal.grow_from.x + decal.facing * decal.grow_from.y;
    ZoneMaterial {
        color,
        shape,
        points,
        frame: Vec4::new(half.x, half.y, decal.grow_from.x, decal.grow_from.y),
        state: Vec4::new(fill, flicker, decal.shape.kind() as f32, spacing.max(0.05)),
        world: Vec4::new(-anchor.x, -anchor.z, decal.facing.x, decal.facing.z),
    }
}

/// The AI's 3D cameras: whether each has a depth prepass, and whether this module gave it.
type AiCameras<'w, 's> = Query<'w, 's, (Entity, Has<DepthPrepass>, Has<FxPrepass>), (With<Camera3d>, With<AiCamera>)>;

/// The cameras the decals are seen through (every camera with `AiCamera`) get a depth prepass
/// while decals are on, and lose the one this module gave them when they go off.
fn prepass_cameras(mut commands: Commands, settings: Res<FxSettings>, cameras: AiCameras) {
    for (camera, has_prepass, ours) in &cameras {
        if settings.decals && !has_prepass {
            commands.entity(camera).insert((DepthPrepass, FxPrepass));
        } else if !settings.decals && ours {
            commands.entity(camera).remove::<(DepthPrepass, FxPrepass)>();
        }
    }
}

/// Telegraph decals on the ground now.
pub fn decals_shown(world: &World) -> usize {
    world.get_resource::<Decals>().map_or(0, |d| d.lines.len())
}

/// One line per telegraph decal on the ground now: whose, which action, its shape and size, how
/// far the fill has grown.
pub fn decal_lines(world: &World) -> Vec<String> {
    world.get_resource::<Decals>().map(|d| d.lines.clone()).unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_colour_packs_as_the_shader_unpacks_it() {
        // unpack4x8unorm: the lowest byte is x (red), the highest w (alpha).
        assert_eq!(pack([0x11, 0x22, 0x33, 0x44]), 0x4433_2211);
    }

    #[test]
    fn a_shard_faces_out_on_every_side() {
        let mesh = shard_mesh();
        let positions = mesh.attribute(Mesh::ATTRIBUTE_POSITION).unwrap().as_float3().unwrap();
        let normals = match mesh.attribute(Mesh::ATTRIBUTE_NORMAL).unwrap() {
            bevy::mesh::VertexAttributeValues::Float32x3(n) => n.clone(),
            other => panic!("{other:?}"),
        };
        assert_eq!(positions.len(), 24, "eight faces of three corners");
        // Convex round the middle: every corner of a face is on the outer side of it.
        for (p, n) in positions.iter().zip(&normals) {
            assert!(Vec3::from(*p).dot(Vec3::from(*n)) > 0.0, "{p:?} {n:?}");
        }
        let material = |shape, glow| ParticleMaterial::of(&EffectDef { shape, glow, ..EffectDef::default() });
        assert_eq!(material(ParticleShape::Quad, true).kind, 0);
        assert_eq!(material(ParticleShape::Shard, false).kind, 1);
        assert_eq!(material(ParticleShape::Cube, true).alpha_mode(), AlphaMode::Add);
        assert_eq!(material(ParticleShape::Cube, false).alpha_mode(), AlphaMode::Blend);
    }

    #[test]
    fn the_stripes_are_measured_from_the_zone_not_from_the_worlds_origin() {
        use shards_action::{HitShape, Local, WorldShape};
        let cone = HitShape::Cone { range: 4.0, angle: 90.0, height: 2.0 };
        let made = |at: Vec3| {
            let decal = ZoneDecal::new(&WorldShape::new(cone, Local::default(), at, Vec3::new(0.6, 0.0, 0.8)), at.y);
            (decal, zone_material(&decal, LinearRgba::RED, 0.5, 1.0, 0.5))
        };
        // The shader's stripe coordinate: a point of the quad (its frame), in world axes from the
        // stripes' anchor, along the 45° diagonal.
        let stripe = |m: &ZoneMaterial, p: Vec2| {
            let facing = Vec2::new(m.world.z, m.world.w);
            let w = Vec2::new(m.world.x, m.world.y) + Vec2::new(-facing.y, facing.x) * p.x + facing * p.y;
            w.dot(Vec2::splat(std::f32::consts::FRAC_1_SQRT_2))
        };
        let (_, by_origin) = made(Vec3::new(0.0, 0.0, -2.0));
        let (far, out_there) = made(Vec3::new(24.0, 0.0, 136.0));
        assert!(by_origin.world.abs_diff_eq(out_there.world, 1e-4), "the same zone anywhere, the same stripes on it");
        assert!(stripe(&out_there, far.grow_from).abs() < 1e-4, "measured from where the fill grows from");
        // From the hit's horizon to the hit (1.1 m to 0.28 m apart) a point of the quad passes a few
        // stripes; measured from the world's origin, 136 m out, it would pass hundreds.
        let half = far.size * 0.5;
        for corner in [Vec2::new(-half.x, -half.y), Vec2::new(half.x, -half.y), Vec2::new(-half.x, half.y), half] {
            let s = stripe(&out_there, corner);
            let passed = (s / 0.28 - s / 1.1).abs();
            assert!(passed < 15.0, "{corner}: {passed} stripes go by");
        }
    }
}
