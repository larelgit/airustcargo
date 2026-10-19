//! Headless-safe gameplay: no meshes, no windows, only data and `FixedUpdate` systems.
//! Rendering attaches itself to these entities from `visuals` when a window exists.
//!
//! The engine does the rest: characters move with its controller, fight with its action
//! machine, collide through its physics layer; the monsters are its NPCs (behaviours,
//! perception, the combat director and attack plans are content in `content/npc/`); the
//! generated region south of the arena is its world module's (`content/region/`), and so is the
//! voxel arena north of it, made by the sandbox's generation steps ([`terrain`]). What is the
//! sandbox's own: the world it spawns from content ([`layout`]) when the engine enters
//! `InGame` (content exists only after loading), the boss's phases and what is left of the
//! dead ([`fate`]).

pub mod fate;
pub mod layout;
pub mod terrain;

use shards::core::CoreSystems;
use shards::prelude::*;

pub use layout::{BoundsDef, LayoutDef, LayoutPiece, PieceKind, Surface};

/// Half size of the square arena (meters).
pub const ARENA_HALF: f32 = 20.0;

#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct Player;

#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct Skeleton;

/// A boss: its phase, and the moveset and attack plan it fights with in each phase.
#[derive(Component, Reflect, Default, Clone, Debug)]
#[reflect(Component, Default)]
pub struct Boss {
    /// 1, then 2 from half health on.
    pub phase: u8,
    /// Moveset content ids, first phase first.
    pub movesets: Vec<String>,
    /// `attack_plan` content ids, first phase first (its `Npc` fights by it).
    pub plans: Vec<String>,
}

#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
pub struct Pillar;

pub struct GameplayPlugin;

impl Plugin for GameplayPlugin {
    fn build(&self, app: &mut App) {
        app.register_type::<Player>()
            .register_type::<Skeleton>()
            .register_type::<Boss>()
            .register_type::<Pillar>()
            .register_type::<LayoutPiece>()
            .add_content::<LayoutDef>()
            .add_content::<BoundsDef>()
            .add_systems(OnEnter(EngineState::InGame), layout::spawn_world)
            .add_plugins((fate::plugin, terrain::plugin));
        // The engine's systems decide (NPCs), move, fight and resolve; the sandbox's react
        // (phases, after deaths) and tidy up, before the lock-on check: a lock on what they
        // take away is let go of on the same tick (`action.lock_on_valid`).
        app.add_systems(
            FixedUpdate,
            (
                // Before the encounter step, so a phase change and the adds it adopts are seen in the same tick.
                fate::boss_phases.in_set(SimSet::Resolve).after(ActionSystems::Deaths).before(shards::encounter::EncounterSystems::Step),
                (fate::remove_corpses, layout::rebuild_changed_layouts)
                    .chain()
                    .in_set(SimSet::Cleanup)
                    .after(CoreSystems::ClearPresses)
                    .before(shards::action::LockSystems::Check),
            ),
        );
    }
}
