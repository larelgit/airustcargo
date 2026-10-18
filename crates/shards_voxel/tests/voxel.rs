//! The voxel module on the whole engine: the `voxel_palette` kind loaded from files and from
//! `content_apply`, its errors naming the file, the module in `ai/status`, the registry through
//! the app, and generation from content. Every test writes its own asset folder (nothing is
//! added under the sandbox's content: a new content file changes every replay's content hash).

use std::path::{Path, PathBuf};

use serde_json::json;
use shards::content::ContentDb;
use shards::core::EngineModules;
use shards::prelude::*;
use shards::testing;
use shards::voxel::{generate, GenSpec, VoxelAppExt, VoxelGen, VoxelPalette, VoxelPaletteDef, VoxelSteps};

const LOOKS: &str = r##"{ "voxel_terrain": (color: Some("#ffffff")) }"##;

const PALETTE: &str = r##"{
    "island": (
        blocks: [
            (name: "grass", top: "#5b8f3a", side: Some("#6f5a3c"), bottom: Some("#5c4730"), tags: ["ground"]),
            (name: "dirt", top: "#6f5a3c"),
            (name: "leaves", top: "#3f7f2f", solid: false, foliage: true),
            (name: "water", top: "#2a6f9f", solid: false, liquid: true),
        ],
        look: Some("voxel_terrain"),
    ),
}"##;

struct Folder(PathBuf);

impl Folder {
    fn new(name: &str, files: &[(&str, &str)]) -> Self {
        let dir = std::env::temp_dir().join(format!("shards_voxel_{name}_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("content")).unwrap();
        for (file, text) in files {
            let path = dir.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn game(folder: &Folder, setup: impl FnOnce(&mut App)) -> App {
    let dir = folder.path().to_string_lossy().into_owned();
    testing::test_app(move |config| config.asset_dir = dir, setup)
}

fn errors(app: &App) -> Vec<String> {
    app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect()
}

#[test]
fn a_palette_file_loads_and_becomes_a_runtime_palette() {
    let folder = Folder::new("load", &[("content/a.look.ron", LOOKS), ("content/island.voxel_palette.ron", PALETTE)]);
    let app = game(&folder, |_| {});
    assert_eq!(errors(&app), Vec::<String>::new());
    let db = app.world().resource::<ContentDb>();
    let palette = VoxelPalette::from_db(db, "island").unwrap();
    assert_eq!((palette.len(), palette.id("water"), palette.tagged("ground")), (4, Some(4), vec![1]));
    assert!(palette.solid(1) && !palette.solid(3) && palette.foliage(3) && palette.liquid(4));
    assert_eq!(db.get::<VoxelPaletteDef>("island").unwrap().look.as_deref(), Some("voxel_terrain"));
    assert!(VoxelPalette::from_db(db, "nothing").unwrap_err().contains("no voxel palette `nothing`"));
    assert_eq!(db.summary().kinds["voxel_palette"], 1);
}

#[test]
fn a_bad_palette_file_is_a_content_error_that_names_the_file() {
    let bad = r##"{
        "broken": (
            blocks: [
                (name: "grass", top: "#12"),
                (name: "grass", top: "#fff"),
                (name: "water", top: "#fff", liquid: true),
            ],
            shade: (up: 2.0),
            look: Some("voxel_terain"),
        ),
    }"##;
    let folder = Folder::new("bad", &[("content/a.look.ron", LOOKS), ("content/world/broken.voxel_palette.ron", bad)]);
    let app = testing::test_app(
        {
            let dir = folder.path().to_string_lossy().into_owned();
            move |config| config.asset_dir = dir
        },
        |_| {},
    );
    let all = errors(&app);
    let file = "content/world/broken.voxel_palette.ron";
    let has = |needle: &str| assert!(all.iter().any(|e| e.contains(file) && e.contains("voxel_palette `broken`") && e.contains(needle)), "{needle}: {all:#?}");
    has("blocks[0] `grass`: top: `#12` is not a colour");
    has("blocks[1] `grass`: the name is used by an earlier block");
    has("blocks[2] `water`: a liquid is never solid");
    has("shade.up: 0 to 1, not 2");
    has("look -> unknown look `voxel_terain` (did you mean `voxel_terrain`?)");
}

#[test]
fn a_palette_syntax_error_names_the_file_and_the_place() {
    let folder = Folder::new("syntax", &[("content/oops.voxel_palette.ron", r##"{ "p": (blocks: [(name: "a", top: "#fff", colour: "x")]) }"##)]);
    let app = game(&folder, |_| {});
    let all = errors(&app);
    assert!(all.iter().any(|e| e.contains("content/oops.voxel_palette.ron") && e.contains("colour")), "{all:#?}");
}

#[test]
fn content_apply_adds_and_refuses_palettes() {
    let folder = Folder::new("apply", &[("content/a.look.ron", LOOKS)]);
    let mut app = game(&folder, |_| {});
    let good = json!({"blocks": [{"name": "stone", "top": "#888888"}, {"name": "sea", "top": "#2244aa", "solid": false, "liquid": true}]});
    testing::ok(&mut app, "content_apply", json!({"changes": [{"kind": "voxel_palette", "id": "rock", "def": good}]}));
    let palette = VoxelPalette::from_db(app.world().resource::<ContentDb>(), "rock").unwrap();
    assert_eq!((palette.id("stone"), palette.liquid(2)), (Some(1), true));
    let got = testing::ok(&mut app, "content_get", json!({"kind": "voxel_palette", "id": "rock"})).result;
    assert_eq!(got["def"]["blocks"][1]["name"], "sea");

    // A palette that does not validate is refused whole, and the content stays as it was.
    let bad = json!({"blocks": [{"name": "stone", "top": "#888888"}, {"name": "stone", "top": "nope"}]});
    let hash = app.world().resource::<ContentDb>().hash();
    let out = testing::call(&mut app, "content_apply", json!({"changes": [{"kind": "voxel_palette", "id": "bad", "def": bad}]}));
    assert!(!out.ok, "{}", out.result);
    let text = out.result.to_string();
    assert!(text.contains("voxel_palette `bad`") && text.contains("is not a colour"), "{text}");
    assert_eq!(app.world().resource::<ContentDb>().hash(), hash);
    assert!(app.world().resource::<ContentDb>().get::<VoxelPaletteDef>("bad").is_none());
    let list = testing::ok(&mut app, "content_list", json!({})).result;
    assert!(list.to_string().contains("voxel_palette"), "{list}");
}

#[test]
fn the_kind_alone_changes_no_content_hash() {
    // A registered kind with no definitions feeds nothing to the hash that replays compare.
    let folder = Folder::new("hash", &[]);
    let app = game(&folder, |_| {});
    assert_eq!(app.world().resource::<ContentDb>().hash(), 0xcbf2_9ce4_8422_2325, "an empty database hashes to the FNV basis");
}

#[test]
fn the_module_is_listed_and_the_registry_takes_steps_from_the_game() {
    fn flatten(g: &mut VoxelGen) -> Result<(), String> {
        let (grass, dirt) = (g.palette().id("grass").ok_or("no grass")?, g.palette().id("dirt").ok_or("no dirt")?);
        let size = g.volume().size();
        for x in 0..size.x as i32 {
            for z in 0..size.z as i32 {
                g.volume_mut().raise_column(x, z, 4, dirt, grass);
            }
        }
        g.mark("centre", Vec3::new(size.x as f32 * 0.5, 4.0, size.z as f32 * 0.5));
        Ok(())
    }
    fn centre_is_marked(g: &VoxelGen) -> Result<(), String> {
        g.find_mark("centre").map(|_| ()).ok_or_else(|| "no centre mark".to_owned())
    }
    let no_look = PALETTE.replace("look: Some(\"voxel_terrain\"),", "");
    let folder = Folder::new("steps", &[("content/island.voxel_palette.ron", no_look.as_str())]);
    let app = game(&folder, |app| {
        app.add_voxel_step("flatten", 2, flatten).add_voxel_check("centre_marked", centre_is_marked);
    });
    assert_eq!(errors(&app), Vec::<String>::new());
    let modules: Vec<String> = app.world().resource::<EngineModules>().iter().map(|m| m.name.clone()).collect();
    assert!(modules.contains(&"voxel".to_owned()), "{modules:?}");
    let steps = app.world().resource::<VoxelSteps>();
    assert_eq!((steps.version("flatten"), steps.check_names()), (Some(2), vec!["centre_marked"]));
    assert_eq!(steps.step_names(), vec!["fill_water", "flatten", "noise_hills"]);
}

#[test]
fn generation_runs_from_a_content_palette_and_the_games_registry() {
    fn hill(g: &mut VoxelGen) -> Result<(), String> {
        let (grass, dirt) = (g.palette().id("grass").unwrap(), g.palette().id("dirt").unwrap());
        g.volume_mut().raise_column(5, 5, 6, dirt, grass);
        g.mark("hill", Vec3::new(5.5, 6.0, 5.5));
        Ok(())
    }
    fn has_hill(g: &VoxelGen) -> Result<(), String> {
        (g.volume().surface(5, 5) == 6).then_some(()).ok_or_else(|| "no hill".to_owned())
    }
    let folder = Folder::new("gen", &[("content/a.look.ron", LOOKS), ("content/island.voxel_palette.ron", PALETTE)]);
    let app = game(&folder, |app| {
        app.add_voxel_step("hill", 3, hill).add_voxel_check("has_hill", has_hill);
    });
    assert_eq!(errors(&app), Vec::<String>::new());
    let palette = VoxelPalette::from_db(app.world().resource::<ContentDb>(), "island").unwrap();
    let steps = app.world().resource::<VoxelSteps>();
    assert_eq!(steps.version("hill"), Some(3));
    let mut spec = GenSpec::new(UVec3::new(16, 8, 16), UVec3::new(8, 8, 8), 1, &[("noise_hills", 1), ("hill", 3), ("fill_water", 1)]);
    spec.params.insert("noise_hills".into(), json!({"top": "grass", "fill": "dirt", "base": 1.0, "amplitude": 2.0}));
    spec.params.insert("fill_water".into(), json!({"level": 2}));
    spec.checks = vec!["has_hill".into()];
    let world = generate(steps, &palette, &spec).unwrap();
    assert_eq!(world.volume.surface(5, 5), 6);
    assert_eq!(world.find_mark("hill"), Some(Vec3::new(5.5, 6.0, 5.5)));
    // A stale version in content is an error naming the step, before anything runs.
    spec.steps[1].1 = 2;
    let err = generate(steps, &palette, &spec).unwrap_err();
    assert!(err.contains("voxel step `hill` is at version 3 in this build, the content expects 2"), "{err}");
}

#[test]
fn registering_a_step_twice_is_a_build_mistake() {
    fn nothing(_: &mut VoxelGen) -> Result<(), String> {
        Ok(())
    }
    let result = std::panic::catch_unwind(|| {
        let mut app = App::new();
        app.add_voxel_step("same", 1, nothing).add_voxel_step("same", 2, nothing);
    });
    let panic = result.unwrap_err();
    let message = panic.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(message.contains("`same` is registered twice"), "{message}");
}
