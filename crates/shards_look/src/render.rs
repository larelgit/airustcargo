//! The looks drawn (feature `render`). Each mesh with a look gets the look's material, made from
//! its own once per look and material; a flat copy of its mesh for a `flat` look; and an outline
//! hull as its child for a look with an outline. A look edited in content, a [`Look`] put on,
//! changed or taken off, and a game giving a styled mesh a material of its own restyle what they
//! concern. Nothing here touches the simulation: the meshes are the picture's.

use std::collections::{BTreeMap, HashMap};

use bevy::asset::embedded_asset;
use bevy::camera::visibility::VisibilitySystems;
use bevy::ecs::system::SystemParam;
use bevy::gltf::GltfMaterialName;
use bevy::light::NotShadowCaster;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Face, RenderPipelineDescriptor, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;
use shards_content::{parse_color, ContentDb};

use crate::def::{material_looks, LookDef, OutlineDef, Shading};
use crate::{Look, LookSettings, LookSystems};

const TOON_SHADER: &str = "embedded://shards_look/shaders/toon.wesl";
const OUTLINE_SHADER: &str = "embedded://shards_look/shaders/outline.wesl";

/// A `Toon` look's material: Bevy's StandardMaterial, lit in bands.
pub type ToonMaterial = ExtendedMaterial<StandardMaterial, Toon>;

/// What a toon material adds to the StandardMaterial: how many bands of light.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct Toon {
    #[uniform(100)]
    pub bands: u32,
}

impl MaterialExtension for Toon {
    fn fragment_shader() -> ShaderRef {
        TOON_SHADER.into()
    }
}

/// An outline's material: one colour, drawn on the back faces of the hull pushed out by `width`
/// pixels. In no prepass and no shadow map.
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct OutlineMaterial {
    #[uniform(0)]
    pub color: LinearRgba,
    #[uniform(0)]
    pub width: f32,
}

impl Material for OutlineMaterial {
    fn vertex_shader() -> ShaderRef {
        OUTLINE_SHADER.into()
    }

    fn fragment_shader() -> ShaderRef {
        OUTLINE_SHADER.into()
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
        // Inside out: the hull's front faces would cover the mesh.
        descriptor.primitive.cull_mode = Some(Face::Front);
        Ok(())
    }
}

/// A mesh drawn with a look, and what it was drawn with before: put back when the look changes.
#[derive(Component, Debug, Clone)]
pub struct Looked {
    /// The look.
    pub look: String,
    /// The material the mesh had.
    pub source: Handle<StandardMaterial>,
    /// The StandardMaterial the look draws it with (none for a toon look): a different one later
    /// is the game's, and the mesh is styled from it anew.
    drawn: Option<Handle<StandardMaterial>>,
    /// The mesh it had, when the look draws a flat copy instead.
    pub flattened: Option<Handle<Mesh>>,
    /// Its outline, a child.
    pub hull: Option<Entity>,
}

/// An outline: a styled mesh again, inside out and a few pixels wider, as its child.
#[derive(Component, Debug, Clone, Copy)]
pub struct OutlineHull;

/// What the looks made, kept so that a mesh drawn again with the same look and material shares
/// them; emptied when the looks change.
#[derive(Resource, Default)]
struct Styles {
    /// The looks the meshes were styled with.
    looks: Vec<(String, LookDef)>,
    /// glTF material name → look.
    by_material: HashMap<String, String>,
    /// (look, the mesh's own material) → what it is drawn with.
    materials: HashMap<(String, AssetId<StandardMaterial>), Drawn>,
    /// Mesh → its flat copy (`None`: it cannot be flattened).
    flat: HashMap<AssetId<Mesh>, Option<Handle<Mesh>>>,
    /// Mesh → its outline hull (`None`: it cannot be outlined).
    hulls: HashMap<AssetId<Mesh>, Option<Handle<Mesh>>>,
    /// Look → its outline's material.
    outlines: HashMap<String, Handle<OutlineMaterial>>,
    /// Meshes to look at again: under a `Look` that changed.
    revisit: Vec<Entity>,
    /// Meshes whose look waits for their material or mesh (a glTF file still loading).
    waiting: Vec<Entity>,
    /// Every mesh again: the looks changed.
    everything: bool,
}

/// A look's material for one material of the game's.
#[derive(Clone, Debug)]
enum Drawn {
    Standard(Handle<StandardMaterial>),
    Toon(Handle<ToonMaterial>),
}

pub(crate) fn plugin(app: &mut App) {
    embedded_asset!(app, "shaders/toon.wesl");
    embedded_asset!(app, "shaders/outline.wesl");
    app.add_plugins((MaterialPlugin::<ToonMaterial>::default(), MaterialPlugin::<OutlineMaterial>::default()))
        .init_resource::<Styles>()
        .add_systems(PostUpdate, (restyle, apply, show_outlines).chain().in_set(LookSystems::Apply))
        // New hulls get their bounds and visibility in the frame they appear.
        .configure_sets(PostUpdate, LookSystems::Apply.before(VisibilitySystems::CalculateBounds));
}

/// Meshes drawn with a StandardMaterial and no look yet.
type Unstyled = (With<Mesh3d>, With<MeshMaterial3d<StandardMaterial>>, Without<Looked>, Without<OutlineHull>);

/// What [`restyle`] reads: the styled meshes, the looks that changed, the tree under them.
#[derive(SystemParam)]
struct Restyling<'w, 's> {
    styled: Query<'w, 's, (Entity, &'static Looked)>,
    regiven: Query<'w, 's, (Entity, &'static Looked, &'static MeshMaterial3d<StandardMaterial>), Changed<MeshMaterial3d<StandardMaterial>>>,
    changed: Query<'w, 's, Entity, Changed<Look>>,
    removed: RemovedComponents<'w, 's, Look>,
    children: Query<'w, 's, &'static Children>,
}

/// Puts back what the looks changed where they no longer hold: every mesh when the looks in
/// content changed, the meshes under a `Look` that changed or went, a styled mesh the game gave a
/// material of its own. [`apply`] styles them again in the same frame.
fn restyle(mut commands: Commands, db: Res<ContentDb>, mut styles: ResMut<Styles>, mut find: Restyling) {
    if db.is_changed() {
        let looks: Vec<(String, LookDef)> = db.iter::<LookDef>().map(|(id, def)| (id.to_owned(), def.clone())).collect();
        if looks != styles.looks {
            for (entity, looked) in &find.styled {
                undo(&mut commands, entity, looked);
            }
            let by_material = material_looks(looks.iter().map(|(id, def)| (id.as_str(), def)));
            *styles = Styles { looks, by_material, everything: true, ..Styles::default() };
            return;
        }
    }
    for (entity, looked, material) in &find.regiven {
        if looked.drawn.as_ref() != Some(&material.0) {
            // The game's own material now: the look starts from it.
            let looked = Looked { source: material.0.clone(), ..looked.clone() };
            undo(&mut commands, entity, &looked);
            styles.revisit.push(entity);
        }
    }
    let roots: Vec<Entity> = find.changed.iter().chain(find.removed.read()).collect();
    for root in roots {
        for entity in std::iter::once(root).chain(find.children.iter_descendants(root)) {
            if let Ok((_, looked)) = find.styled.get(entity) {
                undo(&mut commands, entity, looked);
            }
            styles.revisit.push(entity);
        }
    }
}

/// The mesh as it was before its look.
fn undo(commands: &mut Commands, entity: Entity, looked: &Looked) {
    if let Some(hull) = looked.hull
        && let Ok(mut hull) = commands.get_entity(hull)
    {
        hull.try_despawn();
    }
    let Ok(mut mesh) = commands.get_entity(entity) else { return };
    mesh.remove::<(Looked, MeshMaterial3d<ToonMaterial>)>().insert(MeshMaterial3d(looked.source.clone()));
    if let Some(flattened) = &looked.flattened {
        mesh.insert(Mesh3d(flattened.clone()));
    }
}

/// The asset collections a look makes things in.
#[derive(SystemParam)]
struct LookAssets<'w> {
    meshes: ResMut<'w, Assets<Mesh>>,
    standard: ResMut<'w, Assets<StandardMaterial>>,
    toon: ResMut<'w, Assets<ToonMaterial>>,
    outlines: ResMut<'w, Assets<OutlineMaterial>>,
}

/// What [`apply`] reads of the meshes and the tree above them.
#[derive(SystemParam)]
struct Meshes<'w, 's> {
    fresh: Query<'w, 's, Entity, (Or<(Added<Mesh3d>, Changed<MeshMaterial3d<StandardMaterial>>)>, Unstyled)>,
    all: Query<'w, 's, Entity, Unstyled>,
    meshes: Query<
        'w,
        's,
        (&'static Mesh3d, &'static MeshMaterial3d<StandardMaterial>, Option<&'static GltfMaterialName>, Option<&'static SkinnedMesh>),
        Unstyled,
    >,
    looks: Query<'w, 's, &'static Look>,
    parents: Query<'w, 's, &'static ChildOf>,
}

/// Styles the meshes that have a look and none yet: new ones, ones whose material changed, ones
/// [`restyle`] put back, ones that waited for their files.
fn apply(
    mut commands: Commands,
    db: Res<ContentDb>,
    settings: Res<LookSettings>,
    mut styles: ResMut<Styles>,
    mut assets: LookAssets,
    found: Meshes,
) {
    let mut candidates: Vec<Entity> = if std::mem::take(&mut styles.everything) {
        found.all.iter().collect()
    } else {
        found.fresh.iter().collect()
    };
    candidates.append(&mut styles.revisit);
    candidates.append(&mut styles.waiting);
    candidates.sort_unstable();
    candidates.dedup();
    for entity in candidates {
        let Ok((mesh, material, gltf, skin)) = found.meshes.get(entity) else { continue };
        let Some(id) = look_of(entity, gltf, &found, &styles.by_material) else { continue };
        // A look no content defines: the content check names the prefab that wants it.
        let Some(def) = db.get::<LookDef>(&id) else { continue };
        let needs_mesh = def.flat || def.outline.is_some();
        let (Some(source), true) = (assets.standard.get(&material.0).cloned(), !needs_mesh || assets.meshes.contains(&mesh.0)) else {
            styles.waiting.push(entity);
            continue;
        };
        let key = (id.clone(), material.0.id());
        let drawn = match styles.materials.get(&key) {
            Some(drawn) => drawn.clone(),
            None => {
                let drawn = material_for(def, source, &mut assets);
                styles.materials.insert(key, drawn.clone());
                drawn
            }
        };
        let flat = if def.flat { flat_copy(&mut styles, &mut assets.meshes, &mesh.0) } else { None };
        let hull = def.outline.as_ref().and_then(|outline| {
            let hull = hull_of(&mut styles, &mut assets.meshes, &mesh.0)?;
            let material = outline_of(&mut styles, &mut assets.outlines, &id, outline);
            let shown = if settings.outlines { Visibility::Inherited } else { Visibility::Hidden };
            let mut child = commands.spawn((OutlineHull, Mesh3d(hull), MeshMaterial3d(material), Transform::IDENTITY, shown, NotShadowCaster, ChildOf(entity)));
            if let Some(skin) = skin {
                child.insert(skin.clone());
            }
            Some(child.id())
        });
        let mut styled = commands.entity(entity);
        let flattened = flat.map(|flat| {
            styled.insert(Mesh3d(flat));
            mesh.0.clone()
        });
        let drawn_standard = match drawn {
            Drawn::Standard(handle) => {
                styled.insert(MeshMaterial3d(handle.clone()));
                Some(handle)
            }
            Drawn::Toon(handle) => {
                styled.remove::<MeshMaterial3d<StandardMaterial>>().insert(MeshMaterial3d(handle));
                None
            }
        };
        styled.insert(Looked { look: id, source: material.0.clone(), drawn: drawn_standard, flattened, hull });
    }
}

/// The look of a mesh: its own `Look`, else the nearest above it, else its glTF material's.
fn look_of(entity: Entity, gltf: Option<&GltfMaterialName>, found: &Meshes, by_material: &HashMap<String, String>) -> Option<String> {
    let mut at = Some(entity);
    while let Some(here) = at {
        if let Ok(look) = found.looks.get(here) {
            return Some(look.0.clone());
        }
        at = found.parents.get(here).ok().map(ChildOf::parent);
    }
    gltf.and_then(|name| by_material.get(&name.0)).cloned()
}

/// The look's material, from the mesh's own: its colour (unless the look has one), texture,
/// vertex colours and transparency stay.
fn material_for(def: &LookDef, mut material: StandardMaterial, assets: &mut LookAssets) -> Drawn {
    if let Some([r, g, b, a]) = def.color.as_deref().and_then(|c| parse_color(c).ok()) {
        material.base_color = Color::srgba_u8(r, g, b, a);
    }
    if let Some([r, g, b, _]) = def.glow.as_deref().and_then(|c| parse_color(c).ok()) {
        // Alpha 0: shown as it is, whatever the camera's exposure.
        material.emissive = LinearRgba { alpha: 0.0, ..Color::srgb_u8(r, g, b).to_linear() };
    }
    let no_shine = |m: &mut StandardMaterial| {
        m.perceptual_roughness = 1.0;
        m.metallic = 0.0;
        m.reflectance = 0.0;
    };
    match def.shading {
        Shading::Toon { bands } => {
            no_shine(&mut material);
            Drawn::Toon(assets.toon.add(ExtendedMaterial { base: material, extension: Toon { bands } }))
        }
        Shading::Matte => {
            no_shine(&mut material);
            Drawn::Standard(assets.standard.add(material))
        }
        Shading::Unlit => Drawn::Standard(assets.standard.add(StandardMaterial { unlit: true, ..material })),
        Shading::Lit { roughness, metallic } => {
            Drawn::Standard(assets.standard.add(StandardMaterial { perceptual_roughness: roughness, metallic, ..material }))
        }
    }
}

fn flat_copy(styles: &mut Styles, meshes: &mut Assets<Mesh>, mesh: &Handle<Mesh>) -> Option<Handle<Mesh>> {
    if let Some(made) = styles.flat.get(&mesh.id()) {
        return made.clone();
    }
    let made = meshes.get(mesh).and_then(crate::mesh::flat).map(|flat| meshes.add(flat));
    styles.flat.insert(mesh.id(), made.clone());
    made
}

fn hull_of(styles: &mut Styles, meshes: &mut Assets<Mesh>, mesh: &Handle<Mesh>) -> Option<Handle<Mesh>> {
    if let Some(made) = styles.hulls.get(&mesh.id()) {
        return made.clone();
    }
    let made = meshes.get(mesh).and_then(crate::mesh::hull).map(|hull| meshes.add(hull));
    styles.hulls.insert(mesh.id(), made.clone());
    made
}

fn outline_of(styles: &mut Styles, materials: &mut Assets<OutlineMaterial>, look: &str, outline: &OutlineDef) -> Handle<OutlineMaterial> {
    styles
        .outlines
        .entry(look.to_owned())
        .or_insert_with(|| {
            let [r, g, b, a] = parse_color(&outline.color).unwrap_or([0, 0, 0, 255]);
            materials.add(OutlineMaterial { color: Color::srgba_u8(r, g, b, a).to_linear(), width: outline.width })
        })
        .clone()
}

/// Outlines shown or hidden as `look_outlines` says.
fn show_outlines(settings: Res<LookSettings>, mut hulls: Query<&mut Visibility, With<OutlineHull>>) {
    if !settings.is_changed() {
        return;
    }
    let shown = if settings.outlines { Visibility::Inherited } else { Visibility::Hidden };
    for mut visibility in &mut hulls {
        visibility.set_if_neq(shown);
    }
}

/// What the looks style now: meshes per look, outlines, meshes without a look, meshes waiting
/// for their files.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Usage {
    pub styled: BTreeMap<String, usize>,
    pub outlined: usize,
    pub unstyled: usize,
    pub waiting: usize,
}

impl Usage {
    pub fn total(&self) -> usize {
        self.styled.values().sum()
    }
}

/// What the looks style in `world` now.
pub fn usage(world: &World) -> Usage {
    let mut usage = Usage::default();
    if let Some(mut styled) = world.try_query::<&Looked>() {
        for looked in styled.iter(world) {
            *usage.styled.entry(looked.look.clone()).or_default() += 1;
            usage.outlined += usize::from(looked.hull.is_some());
        }
    }
    if let Some(mut unstyled) = world.try_query_filtered::<(), Unstyled>() {
        usage.unstyled = unstyled.iter(world).count();
    }
    usage.waiting = world.get_resource::<Styles>().map_or(0, |s| s.waiting.len());
    usage
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_look_keeps_the_mesh_s_colour_unless_it_has_one() {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(AssetPlugin::default());
        app.init_asset::<StandardMaterial>().init_asset::<ToonMaterial>().init_asset::<Mesh>().init_asset::<OutlineMaterial>();
        app.add_systems(Update, |mut assets: LookAssets| {
            let red = StandardMaterial { base_color: Color::srgb(1.0, 0.0, 0.0), ..default() };
            let own = LookDef { shading: Shading::Matte, ..LookDef::default() };
            let Drawn::Standard(kept) = material_for(&own, red.clone(), &mut assets) else { panic!("matte is a StandardMaterial") };
            let kept = assets.standard.get(&kept).unwrap();
            assert_eq!((kept.base_color, kept.perceptual_roughness, kept.reflectance), (Color::srgb(1.0, 0.0, 0.0), 1.0, 0.0));
            let blue = LookDef { color: Some("#0000ff".into()), glow: Some("#ffffff".into()), ..LookDef::default() };
            let Drawn::Toon(toon) = material_for(&blue, red, &mut assets) else { panic!("toon is the default") };
            let toon = assets.toon.get(&toon).unwrap();
            assert_eq!(toon.base.base_color, Color::srgba_u8(0, 0, 255, 255));
            assert_eq!((toon.extension.bands, toon.base.emissive.alpha), (3, 0.0));
        });
        app.update();
    }
}
