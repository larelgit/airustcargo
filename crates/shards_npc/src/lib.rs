//! # shards_npc — enemy AI: behaviour, perception, combat director, attack planner
//!
//! Game AI for NPCs (no language model anywhere): parameters in content, logic in code, and
//! every NPC drives its body through `Intent`, exactly like a player does.
//!
//! - **Behaviour** ([`brain`]): a state machine per NPC from content ([`BehaviorDef`], kind
//!   `behavior`): states run **state hooks** with entry parameters, transitions are guarded
//!   by **named conditions** with parameters. The engine provides the generic ones (`idle`,
//!   `patrol`, `chase`, `return_home`, `wait_token`, `attack`, `flee`, `hold`, `investigate`;
//!   `target_seen`, `target_within`, `has_token`, ...), the game adds its own by name
//!   ([`NpcAppExt::add_npc_condition`], [`NpcAppExt::add_npc_state`]).
//! - **Perception** ([`perception`]): sight through physics rays against the `WORLD` layer
//!   from the eyes, field of view and range from content ([`PerceptionDef`]); hearing of
//!   [`Noise`] events (hits and sprinting make them by default); memory of the last known
//!   place.
//! - **Combat director** ([`director`]): attack tokens limited by the number of players, a
//!   collision budget per target, allowed tag combinations, and waiting roles (circle,
//!   strafe, hold) for those without a token ([`DirectorDef`]).
//! - **Attack planner** ([`planner`]), for enemies and bosses: weights, distance windows,
//!   cooldowns and forbidden sequences over the fighter's moveset ([`AttackPlanDef`]); it asks
//!   the action machine's own rule ([`shards_action::can_start`]) before it presses.
//! - **Navigation** ([`nav`]): every tile baked ahead of time into a walkable grid (the
//!   motion module's own probe and move rule), stitched over the region's tile graph when it
//!   loads ([`NavRegion`]), routes by A* over the tile graph, then over the grid of the
//!   corridor, then string pulling; `--bake-nav` and `nav_bake` write the bake files.
//! - **Nav agents** ([`nav`]): routes from a pluggable provider ([`NavPaths`]: the navmesh in
//!   a loaded region, the motion module's local grid elsewhere), path following, the leash
//!   around [`Home`], avoidance between bodies (sampled reciprocal velocity obstacles, numbers
//!   in content: [`AvoidanceDef`]). Dormant NPCs (the world module's active zones) do nothing.
//!
//! ```ron
//! // content/npc/grunt.behavior.ron
//! { "grunt": (initial: "idle", states: {
//!     "idle": (transitions: [(to: "fight", when: ["target_seen"])]),
//!     "fight": (kind: "attack", transitions: [(to: "home", when: ["!target_in_leash"]), (to: "idle", when: ["target_lost"])]),
//!     "home": (kind: "return_home", params: {"heal": 15}, transitions: [(to: "idle", when: ["at_home"])]),
//! }) }
//! // content/npc/grunt.perception.ron
//! { "grunt": (sight_range: 14, fov_deg: 140, memory: 6) }
//! // content/npc/default.director.ron
//! { "default": (tokens: {1: 2, 2: 3, 3: 4}, budget: Some(2.5), allowed: [("light", "light"), ("light", "heavy")],
//!               waiting: [(role: Circle, distance: 4.5), (role: Strafe, distance: 3.5), (role: Hold, distance: 6)]) }
//! // content/npc/grunt.attack_plan.ron
//! { "grunt": (options: [(input: "light", weight: 3, tags: ["light"]), (input: "heavy", cooldown: 4, cost: 1.5, tags: ["heavy"])],
//!             bans: [(after: ["heavy"], then: ["heavy"], within: 2)], rest: (0.4, 1.2)) }
//! ```
//!
//! An NPC is a fighter with [`Npc`] (and optionally [`Home`], [`Patrol`]):
//!
//! ```ignore
//! commands.spawn((character("grunt", at), Fighter::new("grunt"), Team::new("monsters"),
//!                 Npc::new("grunt", "grunt").with_plan("grunt"), Home::at(at, 15.0)));
//! ```
//!
//! ## Order inside a tick
//!
//! The NPC systems run at the end of `SimSet::Input`, after the agents' orders
//! (`ActionSystems::Orders`): NPC brains are an input source like a keyboard, and a game's
//! own brains in `SimSet::Think` run after them (they may read [`NpcBrain`] and add flavour).
//!
//! ```text
//! NpcSystems::Sense     who is active (NpcActivity), perception: sight, hearing, memory, focus
//! NpcSystems::Decide    each NPC: one transition at most, its state hook → NpcDrive
//! NpcSystems::Direct    tokens: started attacks remembered, finished ones back, grants; waiting roles
//! NpcSystems::Plan      token holders: choose an attack, close in, press it (budget, combinations)
//! NpcSystems::Navigate  routes (NavPaths), following, leash → the velocity the route asks for
//! NpcSystems::Drive     avoidance between bodies (leash again), then → Intent: move_dir, aim,
//!                       presses, held inputs
//! ```
//!
//! Everything iterates in `AiId` order and draws randomness from `SimRng` streams keyed by the
//! NPC; all state lives in components of the NPCs (hashed in replay checks, sounds heard but
//! not yet taken in included), none in resources: a game without NPCs hashes as before.
//!
//! Seams: [`NavPaths`] (routes; the navmesh by default), [`NpcActivity`] (who is active; by
//! default every NPC the world module has not marked [`Dormant`](shards_world::Dormant)).
//!
//! Tools: `npc_state {id}`, `nav_path {from?, to, radius?}` and `nav_info` (queries),
//! `npc_set_state {id, state}`, `nav_bake {tile?}`, `nav_overlay {on?}` and `npc_overlay
//! {on?}` ([`overlay`], F7 in a window) (dev). Stop conditions: `npc_attacking`, `npcs_calm`,
//! `npc_beyond_leash`, `npc_detour`, `npc_avoiding`. Journal: `e7 idle → chase e5`, `e7 takes an attack token on e5 (2/2)`, `e7
//! returns its attack token after swipe (1/2)`. The NPC's line: `attack e5 · token · leash
//! 6/15m`.

pub mod ai;
pub mod brain;
pub mod def;
pub mod director;
pub mod nav;
pub mod overlay;
pub mod perception;
pub mod planner;

use ai_core::{AiAppExt, SimSet};
use bevy::prelude::*;
use shards_action::{ActionPlugin, ActionSystems};
use shards_content::ContentAppExt;
use shards_core::EngineAppExt;
use shards_motion::perception::RoutePlanner;
use shards_world::{WorldAppExt, WorldSystems};

pub use brain::{
    clamp_to_leash, force_state, Home, Npc, NpcActivity, NpcBrain, NpcCondition, NpcCtx, NpcDrive, NpcHooks, NpcOutput,
    NpcState, NpcStateChanged, Patrol, TargetView,
};
pub use def::{
    AttackOption, AttackPlanDef, AvoidanceDef, BehaviorDef, Cond, DirectorDef, NpcTuning, Param, Params, PerceptionDef, Role,
    SequenceBan, StateDef, TransitionDef, WaitRole,
};
pub use director::{AttackToken, Grant, TokenGranted, TokenReturned};
pub use nav::{
    bake_files, find_path, local_grid_path, local_grid_route, navmesh_path, navmesh_route, plain_sight, straight_clear, NavAgent,
    NavError, NavPath, NavPaths, NavQuery, NavRegion, NavSettingsDef, NavTile,
};
pub use overlay::NpcOverlay;
pub use perception::{sees, sight_clear, HeardAt, Noise, Perceived, Sensed, Sound};
pub use planner::{banned, choose, pick_weighted, resolve, NpcPlan, PastAttack, Resolved};

/// Version of the NPC module.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The types a game uses most.
pub mod prelude {
    pub use crate::{
        AttackPlanDef, AttackToken, BehaviorDef, DirectorDef, Home, NavAgent, NavPaths, NavRegion, Noise, Npc, NpcAppExt,
        NpcBrain, NpcCtx, NpcOutput, NpcPlugin, NpcSystems, Params, Patrol, PerceptionDef, Perceived,
    };
}

/// The NPC module's systems, in this order, at the end of `SimSet::Input`.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum NpcSystems {
    /// Who is active; perception.
    Sense,
    /// State machines: transitions and state hooks.
    Decide,
    /// The combat director: tokens and waiting roles.
    Direct,
    /// The attack planner: choose, close in, press.
    Plan,
    /// Nav agents: routes, following, leash.
    Navigate,
    /// Avoidance between bodies, then what NPCs decided goes into their `Intent`.
    Drive,
}

/// Registration API for games.
pub trait NpcAppExt {
    /// A condition behaviours name in their transitions: `when: [("low_morale", {"below": 0.3})]`.
    /// Replaces an engine condition of the same name.
    fn add_npc_condition(&mut self, name: &str, condition: NpcCondition) -> &mut Self;

    /// A state hook behaviours name as a state's `kind`: what the NPC wants each tick in it.
    /// Replaces an engine state of the same name.
    fn add_npc_state(&mut self, name: &str, state: NpcState) -> &mut Self;
}

impl NpcAppExt for App {
    fn add_npc_condition(&mut self, name: &str, condition: NpcCondition) -> &mut Self {
        self.world_mut().get_resource_or_init::<NpcHooks>().conditions.insert(name.to_owned(), condition);
        self
    }

    fn add_npc_state(&mut self, name: &str, state: NpcState) -> &mut Self {
        self.world_mut().get_resource_or_init::<NpcHooks>().states.insert(name.to_owned(), state);
        self
    }
}

/// NPC behaviour, perception, the combat director, the attack planner, navigation and nav
/// agents (adds [`ActionPlugin`] if it is missing). Needs `CorePlugin` and `ContentPlugin`;
/// regions (and so the navmesh) come with the world module's `WorldPlugin`
/// (`shards::EnginePlugins` adds all).
pub struct NpcPlugin;

impl Plugin for NpcPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<ActionPlugin>() {
            app.add_plugins(ActionPlugin);
        }
        // Explicitly, for apps built without `reflect_auto_register`: replay checks,
        // prefabs and `inspect` read components through the type registry.
        app.register_type::<Npc>()
            .register_type::<NpcBrain>()
            .register_type::<NpcDrive>()
            .register_type::<Home>()
            .register_type::<Patrol>()
            .register_type::<Perceived>()
            .register_type::<NavAgent>()
            .register_type::<AttackToken>()
            .register_type::<NpcPlan>();
        // Known to the world from the start, so `&World` queries (tools) work before the
        // first NPC.
        let world = app.world_mut();
        world.register_component::<Npc>();
        world.register_component::<NpcBrain>();
        world.register_component::<Home>();
        {
            // The engine's hooks; a game's of the same name, added before, stay.
            let mut hooks = world.get_resource_or_init::<NpcHooks>();
            for (name, condition) in brain::engine_conditions() {
                hooks.conditions.entry(name.to_owned()).or_insert(condition);
            }
            for (name, state) in brain::engine_states() {
                hooks.states.entry(name.to_owned()).or_insert(state);
            }
        }
        app.add_content::<BehaviorDef>()
            .add_content::<PerceptionDef>()
            .add_content::<DirectorDef>()
            .add_content::<AttackPlanDef>()
            .add_content::<NpcTuning>()
            .add_content::<NavSettingsDef>()
            .init_resource::<NavPaths>()
            .init_resource::<NpcActivity>()
            .init_resource::<nav::NavBakes>()
            // The navmesh of each region, stitched as it loads; after a snapshot load it is
            // made again for the region and the solids the snapshot put back.
            .on_region_loaded(nav::region::region_loaded)
            .add_snapshot_rebuild("navmesh", nav::rebuild)
            .add_engine_module("npc", VERSION)
            .configure_sets(
                FixedUpdate,
                (
                    NpcSystems::Sense,
                    NpcSystems::Decide,
                    NpcSystems::Direct,
                    NpcSystems::Plan,
                    NpcSystems::Navigate,
                    NpcSystems::Drive,
                )
                    .chain()
                    .in_set(SimSet::Input)
                    .after(ActionSystems::Orders)
                    .after(WorldSystems::Zones),
            )
            .add_systems(
                FixedUpdate,
                (
                    (brain::update_activity, perception::perceive).chain().in_set(NpcSystems::Sense),
                    brain::decide.in_set(NpcSystems::Decide),
                    director::direct.in_set(NpcSystems::Direct),
                    planner::plan_attacks.in_set(NpcSystems::Plan),
                    (nav::nav_paths, nav::nav_steer).chain().in_set(NpcSystems::Navigate),
                    (nav::nav_avoid, nav::drive_intents).chain().in_set(NpcSystems::Drive),
                ),
            )
            .add_observer(perception::hear_noise)
            .add_observer(perception::hit_noise);
        // `move_to path=true` on the navmesh (a game's own planner, added before, stays).
        if !app.world().contains_resource::<RoutePlanner>() {
            app.insert_resource(nav::ROUTE_PLANNER);
        }
        ai::register(app);
        nav::tools::register(app.world_mut());
        overlay::plugin(app);
        #[cfg(feature = "render")]
        nav::overlay::plugin(app);
    }
}
