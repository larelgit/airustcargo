//! The looks drawn (feature `render`). Each mesh with a look gets the look's material, made from
//! its own once per look and material; a flat copy of its mesh for a `flat` look; and an outline
//! hull as its child for a look with an outline. A look edited in content, a [`Look`] put on,
//! changed or taken off, a game giving a styled mesh a material, a mesh or render layers of its
//! own, and a material or mesh changed in place restyle what they concern; what was made from a
//! mesh or a material that is gone goes with it. Only while something is drawn (a window or
//! `--offscreen`). Nothing here touches the simulation: the meshes are the picture's.

use std::collections::{BTreeMap, HashMap};

use bevy::asset::{embedded_asset, AssetEventSystems};
use bevy::camera::visibility::{NoFrustumCulling, RenderLayers, VisibilityRange};
use bevy::ecs::entity::EntityHashMap;
use bevy::ecs::system::SystemParam;
use bevy::gltf::GltfMaterialName;
use bevy::light::NotShadowCaster;
use bevy::material::OpaqueRendererMethod;
use bevy::mesh::skinning::SkinnedMesh;
use bevy::mesh::MeshVertexBufferLayoutRef;
use bevy::pbr::{ExtendedMaterial, MaterialExtension, MaterialPipeline, MaterialPipelineKey, MaterialPlugin};
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, Face, RenderPipelineDescriptor, SpecializedMeshPipelineError};
use bevy::shader::ShaderRef;
use bevy::transform::TransformSystems;
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
    /// The mesh the look draws it with (its own or the flat copy): a different one later is the
    /// game's, and the mesh is styled from it anew.
    mesh: Handle<Mesh>,
    /// The mesh it had, when the look draws a flat copy instead.
    pub flattened: Option<Handle<Mesh>>,
    /// Its outline, a child.
    pub hull: Option<Entity>,
}

impl Looked {
    /// The game's mesh: the one the flat copy was made from, else the one drawn.
    fn own_mesh(&self) -> &Handle<Mesh> {
        self.flattened.as_ref().unwrap_or(&self.mesh)
    }
}

/// An outline: a styled mesh again, inside out and a few pixels wider, as its child.
#[derive(Component, Debug, Clone, Copy)]
pub struct OutlineHull;

/// What the looks made, kept so that a mesh drawn again with the same look and material shares
/// them. A change of the looks drops the materials; the flat copies and hulls depend on the mesh
/// alone and stay (a mesh whose data went to the GPU could not make them again). What was made
/// from a mesh or a material is dropped when that one changes or is gone ([`forget`]).
#[derive(Resource, Default)]
struct Styles {
    /// The looks the meshes were styled with, as drawn (`extends` resolved).
    looks: BTreeMap<String, LookDef>,
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
    /// Meshes to look at again: under a `Look` that changed, or put back.
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
        .add_systems(
            PostUpdate,
            (forget, restyle, apply, show_outlines).chain().in_set(LookSystems::Apply).run_if(ai_core::display::drawn),
        )
        // Before transforms propagate, and so before visibility and bounds: a new hull is placed,
        // shown and bounded in the frame it appears. Before the asset events go out: the render
        // world gets the looks' new meshes and materials in that frame too.
        .configure_sets(PostUpdate, LookSystems::Apply.before(TransformSystems::Propagate).before(AssetEventSystems));
}

/// Meshes drawn with a StandardMaterial and no look yet.
type Unstyled = (With<Mesh3d>, With<MeshMaterial3d<StandardMaterial>>, Without<Looked>, Without<OutlineHull>);

/// Unstyled meshes new this frame or with a material new to them.
type Fresh = (Or<(Added<Mesh3d>, Changed<MeshMaterial3d<StandardMaterial>>)>, Unstyled);

/// What a mesh is drawn from: its mesh, its material, its glTF material's name.
type Drawable = (&'static Mesh3d, &'static MeshMaterial3d<StandardMaterial>, Option<&'static GltfMaterialName>);

/// What an outline takes from its mesh: the skin, the render layers, no frustum culling, a
/// visibility range.
type Carried = (Option<&'static SkinnedMesh>, Option<&'static RenderLayers>, Has<NoFrustumCulling>, Option<&'static VisibilityRange>);

/// A styled mesh and its StandardMaterial: the look's own, or one the game gave it.
type StyledMaterial = (Entity, &'static Looked, &'static MeshMaterial3d<StandardMaterial>);

/// What [`restyle`] reads: the styled meshes, what the game gave them since, the looks that
/// changed, the tree under them.
#[derive(SystemParam)]
struct Restyling<'w, 's> {
    styled: Query<'w, 's, (Entity, &'static Looked)>,
    regiven: Query<'w, 's, StyledMaterial, Changed<MeshMaterial3d<StandardMaterial>>>,
    reshaped: Query<'w, 's, (Entity, &'static Looked, &'static Mesh3d), Changed<Mesh3d>>,
    layered: Query<'w, 's, (Entity, &'static Looked), Changed<RenderLayers>>,
    unlayered: RemovedComponents<'w, 's, RenderLayers>,
    changed: Query<'w, 's, Entity, Changed<Look>>,
    removed: RemovedComponents<'w, 's, Look>,
    children: Query<'w, 's, &'static Children>,
}

/// Puts back what the looks changed where they no longer hold: every mesh when the looks in
/// content changed, the meshes under a `Look` that changed or went, a styled mesh the game gave a
/// material or a mesh of its own (the look starts from that one, and it stays when the look goes),
/// an outlined mesh whose render layers changed (its outline takes them). [`apply`] styles them
/// again in the same frame.
fn restyle(mut commands: Commands, db: Res<ContentDb>, mut styles: ResMut<Styles>, mut find: Restyling) {
    let mut undone: EntityHashMap<Looked> = EntityHashMap::default();
    for (entity, looked, material) in &find.regiven {
        if looked.drawn.as_ref() != Some(&material.0) {
            undone.entry(entity).or_insert_with(|| looked.clone()).source = material.0.clone();
        }
    }
    for (entity, looked, mesh) in &find.reshaped {
        if looked.mesh != mesh.0 {
            undone.entry(entity).or_insert_with(|| looked.clone()).flattened = None;
        }
    }
    let unlayered: Vec<Entity> = find.unlayered.read().collect();
    for (entity, looked) in find.layered.iter().chain(find.styled.iter_many(unlayered).flatten()) {
        if looked.hull.is_some() {
            undone.entry(entity).or_insert_with(|| looked.clone());
        }
    }
    let roots: Vec<Entity> = find.changed.iter().chain(find.removed.read()).collect();
    let looks: Option<BTreeMap<String, LookDef>> = db
        .is_changed()
        .then(|| db.iter::<LookDef>().map(|(id, def)| (id.to_owned(), def.resolve(|base| db.get::<LookDef>(base)))).collect())
        .filter(|looks| *looks != styles.looks);
    if let Some(looks) = looks {
        for (entity, looked) in &find.styled {
            undone.entry(entity).or_insert_with(|| looked.clone());
        }
        styles.by_material = material_looks(looks.iter().map(|(id, def)| (id.as_str(), def)));
        styles.looks = looks;
        styles.materials.clear();
        styles.outlines.clear();
        styles.everything = true;
    } else {
        for root in roots {
            for entity in std::iter::once(root).chain(find.children.iter_descendants(root)) {
                if let Ok((_, looked)) = find.styled.get(entity) {
                    undone.entry(entity).or_insert_with(|| looked.clone());
                }
                styles.revisit.push(entity);
            }
        }
    }
    for (entity, looked) in &undone {
        undo(&mut commands, *entity, looked);
        styles.revisit.push(*entity);
    }
}

/// Forgets what the looks made from a mesh or a material that is gone, or changed in place
/// (`Assets::get_mut`, a glTF file reloaded), and puts back the meshes drawn from a changed one:
/// [`apply`] styles them from it anew. Asset events go out after the look systems, so a change
/// shows the frame after. The looks' own meshes and materials are new assets, and drawing them
/// changes none (the render world takes them untracked): a restyle does not come round again.
fn forget(
    mut commands: Commands,
    mut styles: ResMut<Styles>,
    mut meshes: MessageReader<AssetEvent<Mesh>>,
    mut materials: MessageReader<AssetEvent<StandardMaterial>>,
    styled: Query<(Entity, &Looked)>,
) {
    let mut reshaped = Vec::new();
    for event in meshes.read() {
        if let AssetEvent::Unused { id } | AssetEvent::Removed { id } | AssetEvent::Modified { id } = event {
            styles.flat.remove(id);
            styles.hulls.remove(id);
        }
        if let AssetEvent::Modified { id } = event {
            reshaped.push(*id);
        }
    }
    let mut gone = Vec::new();
    let mut recolored = Vec::new();
    for event in materials.read() {
        if let AssetEvent::Unused { id } | AssetEvent::Removed { id } | AssetEvent::Modified { id } = event {
            gone.push(*id);
        }
        if let AssetEvent::Modified { id } = event {
            recolored.push(*id);
        }
    }
    if !gone.is_empty() {
        styles.materials.retain(|(_, source), _| !gone.contains(source));
    }
    if reshaped.is_empty() && recolored.is_empty() {
        return;
    }
    for (entity, looked) in &styled {
        if reshaped.contains(&looked.own_mesh().id()) || recolored.contains(&looked.source.id()) {
            undo(&mut commands, entity, looked);
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
    fresh: Query<'w, 's, Entity, Fresh>,
    all: Query<'w, 's, Entity, Unstyled>,
    meshes: Query<'w, 's, Drawable, Unstyled>,
    carried: Query<'w, 's, Carried>,
    looks: Query<'w, 's, &'static Look>,
    parents: Query<'w, 's, &'static ChildOf>,
}

/// Styles the meshes that have a look and none yet: new ones, ones whose material changed, ones
/// [`restyle`] or [`forget`] put back, ones that waited for their files.
fn apply(mut commands: Commands, settings: Res<LookSettings>, mut styles: ResMut<Styles>, mut assets: LookAssets, found: Meshes) {
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
        let Ok((mesh, material, gltf)) = found.meshes.get(entity) else { continue };
        let Some(id) = look_of(entity, gltf, &found, &styles.by_material) else { continue };
        // A look no content defines: the content check names the prefab that wants it.
        let Some(def) = styles.looks.get(&id).cloned() else { continue };
        let flat = def.flat == Some(true);
        let needs_mesh = flat || def.outline.is_some();
        let (Some(source), true) = (assets.standard.get(&material.0).cloned(), !needs_mesh || assets.meshes.contains(&mesh.0)) else {
            styles.waiting.push(entity);
            continue;
        };
        // A see-through surface shows the hull behind it, and a cut-out one is outlined whole.
        let opaque = source.alpha_mode == AlphaMode::Opaque;
        let key = (id.clone(), material.0.id());
        let drawn = match styles.materials.get(&key) {
            Some(drawn) => drawn.clone(),
            None => {
                let drawn = material_for(&def, source, &mut assets);
                styles.materials.insert(key, drawn.clone());
                drawn
            }
        };
        let flat = if flat { flat_copy(&mut styles, &mut assets.meshes, &mesh.0) } else { None };
        let hull = def.outline.as_ref().filter(|_| opaque).and_then(|outline| {
            let hull = hull_of(&mut styles, &mut assets.meshes, &mesh.0)?;
            let material = outline_of(&mut styles, &mut assets.outlines, &id, outline);
            let shown = if settings.outlines { Visibility::Inherited } else { Visibility::Hidden };
            let mut child = commands.spawn((OutlineHull, Mesh3d(hull), MeshMaterial3d(material), Transform::IDENTITY, shown, NotShadowCaster, ChildOf(entity)));
            if let Ok((skin, layers, unculled, range)) = found.carried.get(entity) {
                if let Some(skin) = skin {
                    child.insert(skin.clone());
                }
                if let Some(layers) = layers {
                    child.insert(layers.clone());
                }
                if unculled {
                    child.insert(NoFrustumCulling);
                }
                if let Some(range) = range {
                    child.insert(range.clone());
                }
            }
            Some(child.id())
        });
        let mut styled = commands.entity(entity);
        let (drawn_mesh, flattened) = match flat {
            Some(flat) => {
                styled.insert(Mesh3d(flat.clone()));
                (flat, Some(mesh.0.clone()))
            }
            None => (mesh.0.clone(), None),
        };
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
        styled.insert(Looked { look: id, source: material.0.clone(), drawn: drawn_standard, mesh: drawn_mesh, flattened, hull });
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

/// The look's material (`def` resolved), from the mesh's own: its colour (unless the look has
/// one), alpha (unless the look's colour has one), texture, vertex colours and alpha mode stay.
fn material_for(def: &LookDef, mut material: StandardMaterial, assets: &mut LookAssets) -> Drawn {
    if let Some(color) = &def.color {
        material.base_color = recolor(material.base_color, color);
    }
    let shading = def.shading.unwrap_or_default();
    if let Some([r, g, b, _]) = def.glow.as_deref().and_then(|c| parse_color(c).ok()) {
        let glow = Color::srgb_u8(r, g, b).to_linear();
        if shading == Shading::Unlit {
            // Bevy draws an unlit surface without its emissive light: the glow is in its colour.
            let base = material.base_color.to_linear();
            material.base_color = LinearRgba { alpha: base.alpha, ..base + glow }.into();
        } else {
            material.emissive = glow;
            // Shown as it is, whatever the camera's exposure.
            material.emissive_exposure_weight = 0.0;
        }
    }
    let no_shine = |m: &mut StandardMaterial| {
        m.perceptual_roughness = 1.0;
        m.metallic = 0.0;
        m.reflectance = 0.0;
    };
    // A glTF file's material may be unlit (KHR_materials_unlit): the look says.
    material.unlit = shading == Shading::Unlit;
    match shading {
        Shading::Toon { bands } => {
            no_shine(&mut material);
            // Deferred light is Bevy's own: the bands are drawn in the forward pass.
            material.opaque_render_method = OpaqueRendererMethod::Forward;
            Drawn::Toon(assets.toon.add(ExtendedMaterial { base: material, extension: Toon { bands } }))
        }
        Shading::Matte => {
            no_shine(&mut material);
            Drawn::Standard(assets.standard.add(material))
        }
        Shading::Unlit => Drawn::Standard(assets.standard.add(material)),
        Shading::Lit { roughness, metallic } => {
            Drawn::Standard(assets.standard.add(StandardMaterial { perceptual_roughness: roughness, metallic, ..material }))
        }
    }
}

/// A look's colour over the mesh's own `color`: `#rrggbb` keeps its alpha, `#rrggbbaa` gives one.
fn recolor(own: Color, text: &str) -> Color {
    let Ok([r, g, b, a]) = parse_color(text) else { return own };
    // `parse_color` gives 255 for a colour without alpha: only the eight digits say.
    let alpha = if text.len() == 9 { f32::from(a) / 255.0 } else { own.alpha() };
    Color::srgb_u8(r, g, b).with_alpha(alpha)
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

    /// `material_for` on its own, in a bare app.
    fn with_assets(test: impl Fn(&mut LookAssets) + Send + Sync + 'static) {
        let mut app = App::new();
        app.add_plugins(MinimalPlugins).add_plugins(AssetPlugin::default());
        app.init_asset::<StandardMaterial>().init_asset::<ToonMaterial>().init_asset::<Mesh>().init_asset::<OutlineMaterial>();
        app.add_systems(Update, move |mut assets: LookAssets| test(&mut assets));
        app.update();
    }

    fn look(text: &str) -> LookDef {
        let options = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME);
        options.from_str::<LookDef>(text).unwrap().resolve(|_| None)
    }

    #[test]
    fn a_look_keeps_the_mesh_s_colour_unless_it_has_one() {
        with_assets(|assets| {
            let red = StandardMaterial { base_color: Color::srgb(1.0, 0.0, 0.0), unlit: true, ..default() };
            let Drawn::Standard(kept) = material_for(&look("(shading: Matte)"), red.clone(), assets) else { panic!("matte is a StandardMaterial") };
            let kept = assets.standard.get(&kept).unwrap();
            assert_eq!((kept.base_color, kept.perceptual_roughness, kept.reflectance), (Color::srgb(1.0, 0.0, 0.0), 1.0, 0.0));
            assert!(!kept.unlit, "lit, though the mesh's own material was unlit (a glTF file's KHR_materials_unlit)");
            let blue = look(r##"(color: Some("#0000ff"), glow: Some("#ffffff"))"##);
            let Drawn::Toon(toon) = material_for(&blue, red, assets) else { panic!("toon is the default") };
            let toon = assets.toon.get(&toon).unwrap();
            assert_eq!(toon.base.base_color, Color::srgba_u8(0, 0, 255, 255));
            assert_eq!((toon.extension.bands, toon.base.emissive, toon.base.emissive_exposure_weight), (3, LinearRgba::WHITE, 0.0));
            assert!(!toon.base.unlit);
            assert_eq!(toon.base.opaque_render_method, OpaqueRendererMethod::Forward, "bands need the forward pass");
        });
    }

    #[test]
    fn a_look_s_colour_keeps_the_mesh_s_alpha_unless_it_gives_one() {
        with_assets(|assets| {
            let glass = StandardMaterial { base_color: Color::srgba(1.0, 1.0, 1.0, 0.25), alpha_mode: AlphaMode::Blend, ..default() };
            let mut alpha = |text: &str| {
                let Drawn::Standard(drawn) = material_for(&look(text), glass.clone(), assets) else { panic!("matte") };
                let drawn = assets.standard.get(&drawn).unwrap();
                assert_eq!(drawn.alpha_mode, AlphaMode::Blend);
                (drawn.base_color.to_srgba().to_u8_array_no_alpha(), drawn.base_color.alpha())
            };
            assert_eq!(alpha(r##"(color: Some("#ff8000"), shading: Matte)"##), ([255, 128, 0], 0.25));
            assert_eq!(alpha(r##"(color: Some("#f80"), shading: Matte)"##), ([255, 136, 0], 0.25));
            assert_eq!(alpha(r##"(color: Some("#ff800080"), shading: Matte)"##), ([255, 128, 0], 128.0 / 255.0));
            assert_eq!(alpha(r##"(color: Some("#ff8000ff"), shading: Matte)"##), ([255, 128, 0], 1.0));
        });
    }

    #[test]
    fn an_unlit_look_s_glow_is_in_its_colour() {
        with_assets(|assets| {
            let dim = StandardMaterial { base_color: Color::srgba(0.2, 0.0, 0.0, 0.5), ..default() };
            let Drawn::Standard(drawn) = material_for(&look(r##"(shading: Unlit, glow: Some("#7fe0ff"))"##), dim, assets) else { panic!("unlit") };
            let drawn = assets.standard.get(&drawn).unwrap();
            let glow = Color::srgb_u8(0x7f, 0xe0, 0xff).to_linear();
            let base = Color::srgb(0.2, 0.0, 0.0).to_linear();
            let colour = drawn.base_color.to_linear();
            assert!(drawn.unlit);
            assert_eq!((colour.red, colour.green, colour.blue, colour.alpha), (base.red + glow.red, glow.green, glow.blue, 0.5));
            assert_eq!(drawn.emissive, LinearRgba::BLACK, "Bevy would not show it");
        });
    }
}
