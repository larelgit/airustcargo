//! What the AI is allowed to see about an entity.
//!
//! Every component the AI should know about is registered explicitly with a short
//! describe function. Observation lines are just these fragments joined together:
//! `e42 Boss "Frost Warden"  hp 2300/4000  phase 2  WindupSlam 0.60s  8.4m NE`.

use std::borrow::Cow;
use std::sync::Arc;

use bevy::prelude::*;

use crate::id::{AiId, AiImportance, AiVisible};

/// What kind of thing this is (`Player`, `Skeleton`, `Boss`). First word of its line.
/// Implies [`AiVisible`].
#[derive(Component, Reflect, Clone, Debug)]
#[reflect(Component)]
#[require(AiVisible)]
pub struct AiKind(pub Cow<'static, str>);

impl AiKind {
    pub fn new(kind: impl Into<Cow<'static, str>>) -> Self {
        Self(kind.into())
    }
}

pub type DescribeFn = Arc<dyn Fn(EntityRef) -> Option<String> + Send + Sync>;

/// One registered fragment source.
#[derive(Clone)]
pub struct Describer {
    /// Stable key, used for diffs ("e12 hp 64/100 → 46/100") and `inspect`.
    pub key: &'static str,
    pub describe: DescribeFn,
}

/// Registered describers, in registration order (= order inside a line).
#[derive(Resource, Default, Clone)]
pub struct AiDescribers(pub Vec<Describer>);

/// Everything the AI layer knows about one entity, independent of the viewer.
#[derive(Clone, Debug, PartialEq)]
pub struct EntityFacts {
    pub id: AiId,
    pub entity: Entity,
    /// `Boss "Frost Warden"`
    pub label: String,
    pub fragments: Vec<(&'static str, String)>,
    pub pos: Vec3,
    pub facing: Option<Vec3>,
    pub importance: f32,
}

impl EntityFacts {
    pub fn fragments_text(&self) -> String {
        self.fragments.iter().map(|(_, f)| f.as_str()).collect::<Vec<_>>().join("  ")
    }

    pub fn fragment(&self, key: &str) -> Option<&str> {
        self.fragments.iter().find(|(k, _)| *k == key).map(|(_, f)| f.as_str())
    }
}

pub fn label_of(entity: EntityRef) -> String {
    let kind = entity.get::<AiKind>().map(|k| k.0.as_ref());
    let name = entity.get::<Name>().map(Name::as_str);
    match (kind, name) {
        (Some(k), Some(n)) if k != n => format!("{k} \"{n}\""),
        (Some(k), _) => k.to_owned(),
        (None, Some(n)) => format!("\"{n}\""),
        (None, None) => "entity".to_owned(),
    }
}

/// Facts about one AI-visible entity, or `None` if it is not visible to the AI.
pub fn facts(world: &World, entity: Entity) -> Option<EntityFacts> {
    let e = world.get_entity(entity).ok()?;
    if !e.contains::<AiVisible>() {
        return None;
    }
    let id = *e.get::<AiId>()?;
    let describers = world.get_resource::<AiDescribers>();
    let fragments = describers
        .map(|d| d.0.iter().filter_map(|d| (d.describe)(e).map(|f| (d.key, f))).filter(|(_, f)| !f.is_empty()).collect())
        .unwrap_or_default();
    let tf = e.get::<Transform>();
    Some(EntityFacts {
        id,
        entity,
        label: label_of(e),
        fragments,
        pos: tf.map_or(Vec3::ZERO, |t| t.translation),
        facing: tf.map(|t| *t.forward()),
        importance: e.get::<AiImportance>().map_or(0.0, |i| i.0),
    })
}

/// Facts for every live AI-visible entity, ordered by id (deterministic).
pub fn all_facts(world: &World) -> Vec<EntityFacts> {
    let Some(index) = world.get_resource::<crate::id::AiIndex>() else {
        return Vec::new();
    };
    index.ids().into_iter().filter_map(|id| index.get(id)).filter_map(|e| facts(world, e)).collect()
}
