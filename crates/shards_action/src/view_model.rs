//! The view model (F1b): the arms and the weapon a first-person camera draws over the world.
//!
//! A **view model** is a glTF rig (arms, a weapon, hands) shown in front of a first-person camera on
//! a render layer of its own, by a second camera with a field of view of its own, so it never clips
//! into a wall. It is animated by the animation bridge from the looked-through body's actions
//! through an [`AnimSet`] of its own: the bridge reads the body's `ActionState`, `MotionState`,
//! `Fighter` and the rest through [`ShowsBody`] and drives the view model's skeleton, props and
//! `@moveset` mounts exactly as it drives a body's. Picture only: nothing here has an `AiId`, and
//! nothing of it is in a snapshot or a replay.
//!
//! - **Content** (kind `view_model`, `*.view_model.ron`, [`ViewModelDef`]): the model file, the animset
//!   that moves it, the field of view, where it sits in front of the camera, and a look.
//! - **Which one** is the weapon's: [`MovesetDef::view_model`](crate::MovesetDef) names a definition,
//!   so the moveset of a weapon in hand (an `Arsenal` switch changes it) changes what is drawn;
//!   `camera` content's `first_person.view_model` is the one for a body whose moveset names none.
//! - The systems (`render` feature) are in [`crate::render::view_model`].

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::{ContentDef, Refs};

use crate::anim::AnimSet;

fn default_fov() -> f32 {
    60.0
}

fn default_near() -> f32 {
    0.02
}

fn default_scale() -> f32 {
    1.0
}

fn is_default_fov(v: &f32) -> bool {
    *v == default_fov()
}

fn is_default_near(v: &f32) -> bool {
    *v == default_near()
}

fn is_default_scale(v: &f32) -> bool {
    *v == default_scale()
}

fn is_zero_vec(v: &Vec3) -> bool {
    *v == Vec3::ZERO
}

/// A view model: what a first-person camera draws in front of the world for a weapon.
///
/// ```ron
/// {
///     "sidearm": (
///         model: "models/arms.glb",        // scene 0 is drawn; its clips are the animset's `source`
///         animset: "arms",                 // the set that picks the clip for each action and state
///         look: Some("hero"),              // optional: a `look` to dress the meshes in
///         fov_deg: 60.0,                   // the view model camera's own vertical field of view
///         offset: (0.0, -0.2, -0.3),       // right, up, forward (negative z) of the camera
///     ),
/// }
/// ```
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ViewModelDef {
    /// The glTF file (`.gltf` or `.glb`), a path inside the assets folder. Its first scene is drawn.
    pub model: String,
    /// The [`AnimSet`] that animates it. Its `source` is usually the same file.
    pub animset: String,
    /// A `look` (kind `look`) the meshes are drawn with. `None`: the materials of the file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub look: Option<String>,
    /// The vertical field of view of the camera that draws the view model, in degrees. It is the
    /// model's, not the player's: the view model is made for one field of view, and the world's changes
    /// with the player's setting.
    #[serde(default = "default_fov", skip_serializing_if = "is_default_fov")]
    pub fov_deg: f32,
    /// The camera's near plane (m): nearer than this nothing of the view model is drawn.
    #[serde(default = "default_near", skip_serializing_if = "is_default_near")]
    pub near: f32,
    /// Where the model sits in front of the camera (m): right, up, and forward (the camera looks along -z).
    #[serde(default, skip_serializing_if = "is_zero_vec")]
    pub offset: Vec3,
    /// The model's size.
    #[serde(default = "default_scale", skip_serializing_if = "is_default_scale")]
    pub scale: f32,
}

/// Whether `path` is not a relative path inside the assets folder on every platform: it climbs (`..`), is rooted or
/// has a drive (`/arms.glb`, `\arms.glb`, `C:\arms.glb`, `C:arms.glb`), however this machine's `Path` reads it (a
/// content file is checked on one machine and played on another).
pub fn leaves_the_assets(path: &str) -> bool {
    let bytes = path.as_bytes();
    let drive = bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    drive
        || path.starts_with(['/', '\\'])
        || path.split(['/', '\\']).any(|part| part == "..")
        || std::path::Path::new(path).components().any(|c| !matches!(c, std::path::Component::Normal(_) | std::path::Component::CurDir))
}

impl ContentDef for ViewModelDef {
    const KIND: &'static str = "view_model";
    // A picture: a headless game never reads it, so it is outside the replay's content check.
    const SIMULATION: bool = false;

    fn validate(&self, _id: &str, refs: &mut Refs) {
        if self.model.is_empty() {
            refs.error("model is empty; name the glTF, e.g. `model: \"models/arms.glb\"`");
        } else if !(self.model.ends_with(".glb") || self.model.ends_with(".gltf")) {
            refs.error(format!("model `{}` is not a glTF file (.gltf or .glb)", self.model));
        } else if leaves_the_assets(&self.model) {
            refs.error(format!("model `{}` leaves the assets folder: give a path inside it, without `..`, a drive or a leading slash", self.model));
        } else if let Some(root) = refs.asset_root()
            && !root.join(&self.model).is_file()
        {
            refs.error(format!("model `{}` is no file in the assets folder", self.model));
        }
        refs.require::<AnimSet>(&self.animset, "animset");
        if let Some(look) = &self.look {
            if look.is_empty() {
                refs.error("look is empty; name a `look` or leave it out");
            } else {
                refs.require::<shards_look::LookDef>(look, "look");
            }
        }
        if !(self.fov_deg.is_finite() && (10.0..=120.0).contains(&self.fov_deg)) {
            refs.error(format!("fov_deg is {}, it must be between 10 and 120 degrees", self.fov_deg));
        }
        if !(self.near.is_finite() && self.near > 0.0 && self.near < 1.0) {
            refs.error(format!("near is {}, it must be above 0 and below 1 metre", self.near));
        }
        if !self.offset.is_finite() || self.offset.length() > 5.0 {
            refs.error(format!("offset is {}, it must be a point within 5 m of the camera", self.offset));
        }
        if !(self.scale.is_finite() && self.scale > 0.0) {
            refs.error(format!("scale is {}, it must be above 0", self.scale));
        }
    }
}

/// The content check of the `camera` kind: the view model its first-person tuning names exists.
pub(crate) fn check_camera(def: &shards_motion::camera::CameraTuning, _id: &str, refs: &mut Refs) {
    if let Some(id) = &def.first_person.view_model {
        refs.require::<ViewModelDef>(id, "first_person.view_model");
    }
}

/// On a picture-only rig that shows another entity's actions: the animation bridge drives the rig's
/// own skeleton (its [`Animated`](crate::Animated) set, its props) from the **body's** `ActionState`,
/// `MotionState`, `Fighter` and the rest, which the rig does not carry. It is how a view model is
/// animated without a second fighter: copying `ActionState` onto it would make the action machine tick it.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Eq)]
#[reflect(Component)]
pub struct ShowsBody(pub Entity);

impl FromWorld for ShowsBody {
    fn from_world(_: &mut World) -> Self {
        Self(Entity::PLACEHOLDER)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_definition_with_only_its_two_names_is_complete() {
        let def: ViewModelDef = ron::from_str(r#"(model: "models/arms.glb", animset: "arms")"#).unwrap();
        assert_eq!((def.fov_deg, def.near, def.scale, def.offset, def.look.as_deref()), (60.0, 0.02, 1.0, Vec3::ZERO, None));
        assert_eq!(ron::to_string(&def).unwrap(), r#"(model:"models/arms.glb",animset:"arms")"#, "what is left at its default is not written");
        let full: ViewModelDef =
            ron::from_str(r#"(model: "m.gltf", animset: "a", look: Some("hero"), fov_deg: 50.0, near: 0.05, offset: (0.1, -0.2, -0.3), scale: 0.8)"#).unwrap();
        assert_eq!(ron::from_str::<ViewModelDef>(&ron::to_string(&full).unwrap()).unwrap(), full);
        assert!(ron::from_str::<ViewModelDef>(r#"(model: "m.glb", animset: "a", fov: 50.0)"#).is_err(), "a typo is an error");
    }

    #[test]
    fn what_a_view_model_names_must_exist_wherever_it_is_named() {
        use shards_content::db::Source;
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<ViewModelDef>();
        kinds.register::<crate::def::MovesetDef>();
        kinds.register::<crate::def::ActionDef>();
        kinds.register::<AnimSet>();
        kinds.register::<shards_look::LookDef>();
        kinds.register::<shards_motion::camera::CameraTuning>();
        kinds.add_check::<shards_motion::camera::CameraTuning>("first-person view model", check_camera);
        let source = |path: &str, text: &str| Source { path: path.into(), text: text.into() };
        let sources = [
            source("content/set.animset.ron", r#"{ "arms": (source: "models/arms.glb", clips: {"idle": ()}) }"#),
            source("content/looks.look.ron", r#"{ "stone": () }"#),
            source(
                "content/v.view_model.ron",
                r#"{
                    "arms": (model: "models/arms.glb", animset: "arms", look: Some("stone")),
                    "wrong": (model: "models/arms.png", animset: "legs", look: Some("moss"), fov_deg: 3.0, near: 2.0, scale: 0.0, offset: (9.0, 0.0, 0.0)),
                    "up": (model: "../arms.glb", animset: "arms"),
                    "root": (model: "/arms.glb", animset: "arms"),
                    "back": (model: "\\arms.glb", animset: "arms"),
                    "drive": (model: "C:/arms.glb", animset: "arms"),
                    "win": (model: "C:\\arms.glb", animset: "arms"),
                    "inside": (model: "models/./arms.glb", animset: "arms"),
                }"#,
            ),
            source("content/m.moveset.ron", r#"{ "good": (inputs: {}, view_model: Some("arms")), "bad": (inputs: {}, view_model: Some("legs")) }"#),
            source("content/c.camera.ron", r#"{ "default": (first_person: (view_model: Some("hands"))), "other": (first_person: (view_model: Some("arms"))) }"#),
        ];
        let db = shards_content::ContentDb::build(&kinds, &sources);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        // In the order of the files' paths.
        let expected = [
            "content/c.camera.ron: camera `default`: first_person.view_model -> unknown view_model `hands`",
            "content/m.moveset.ron: moveset `bad`: view_model -> unknown view_model `legs`",
            "content/v.view_model.ron: view_model `back`: model `\\arms.glb` leaves the assets folder: give a path inside it, without `..`, a drive or a leading slash",
            "content/v.view_model.ron: view_model `drive`: model `C:/arms.glb` leaves the assets folder: give a path inside it, without `..`, a drive or a leading slash",
            "content/v.view_model.ron: view_model `root`: model `/arms.glb` leaves the assets folder: give a path inside it, without `..`, a drive or a leading slash",
            "content/v.view_model.ron: view_model `up`: model `../arms.glb` leaves the assets folder: give a path inside it, without `..`, a drive or a leading slash",
            "content/v.view_model.ron: view_model `win`: model `C:\\arms.glb` leaves the assets folder: give a path inside it, without `..`, a drive or a leading slash",
            "content/v.view_model.ron: view_model `wrong`: model `models/arms.png` is not a glTF file (.gltf or .glb)",
            "content/v.view_model.ron: view_model `wrong`: animset -> unknown animset `legs`",
            "content/v.view_model.ron: view_model `wrong`: look -> unknown look `moss`",
            "content/v.view_model.ron: view_model `wrong`: fov_deg is 3, it must be between 10 and 120 degrees",
            "content/v.view_model.ron: view_model `wrong`: near is 2, it must be above 0 and below 1 metre",
            "content/v.view_model.ron: view_model `wrong`: offset is [9, 0, 0], it must be a point within 5 m of the camera",
            "content/v.view_model.ron: view_model `wrong`: scale is 0, it must be above 0",
        ];
        assert_eq!(errors, expected);
    }
}
