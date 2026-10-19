//! # shards_action — actions and combat
//!
//! Attacks, blocks, parries, dodges and a healing flask are all one thing here: an
//! **action**, a timeline written in content ([`ActionDef`], kind `action`). One
//! **machine** per [`Fighter`] runs them, for players, enemies and bosses alike; a
//! **moveset** ([`MovesetDef`], kind `moveset`) says which input starts which action.
//!
//! - **Machine** ([`machine`]): presses from `Intent` wait in an [`InputBuffer`]
//!   (0.15 s by default) until their action can start; cancel windows let an input
//!   cut an action short (combos are chains of such transitions); a held input loops
//!   its action (a raised block). [`can_start`] is the one rule the machine and the
//!   agent tools share: `ok`, `ok: queued, dodge starts in 0.12s`, `not enough stamina:
//!   10/25 (ready in 1.0s)`, `busy: windup heavy 0.80s (ready in 0.80s)`.
//! - **Hits** ([`combat`]): open hit windows overlap hurtboxes through the physics layer
//!   after this tick's movement; attackers and targets in `AiId` order, each target
//!   once per action. The damage pipeline: god mode, i-frames, guard and perfect
//!   guard, guard break, armor, the game's hooks, then health, statuses and poise.
//! - **Fighter resources** ([`fighter`]): [`Health`], [`Stamina`] (regenerates after a
//!   delay; sprinting drains it), [`Poise`] (breaks into a stagger), [`Armor`],
//!   [`Team`], [`Charges`], [`Dead`].
//! - **Statuses** ([`status`]): buildup to a threshold, a timed effect the game
//!   implements as hooks ([`ActionAppExt::add_status_effect`]).
//! - **Telegraphs and zones** ([`zone`]): enemies' warnings ([`Telegraph`] events with a
//!   symbol and a sound cue), [`hit_zones`] for decals and the AI, [`threat_indicators`]
//!   for off-screen warnings.
//! - **Animation** ([`anim`]): content kind `animset` (a glTF's clips, which one shows which
//!   locomotion state or action, blends, cues) and [`select_clip`], the pure choice of the
//!   clip a character shows: gameplay leads, the animation is seeked to the action's
//!   progress. [`Animated`] characters are driven by the bridge of the `render` feature.
//! - **Displacement bake** ([`bake`]): an action's `displacement` read off the root bone of its
//!   clip (`bake: (animset, clip)`), written into its file by `--bake-motion` or `motion_bake`;
//!   content checks call a curve stale when its clip changed. [`clipfile`] reads glTF clips
//!   without a window.
//! - **Drawing** ([`draw`]): the geometry of what the `render` feature puts on screen, as
//!   plain data: ground telegraphs with symbols, the combat overlay, edge markers for
//!   attacks off screen.
//! - **Cues** ([`cue`]): [`SoundCue`] (a telegraph's) and [`ActionSound`] (an action's `Sound`
//!   effect) for the audio module, [`AnimationCue`] for effects.
//!
//! The controller stays the only thing that moves a body: an action's displacement
//! curve goes into `LocomotionInput::external`, so a wall stops a lunge.
//!
//! ## Content
//!
//! ```ron
//! // content/combat/hero.action.ron
//! {
//!     "light_1": (
//!         net_id: 1,
//!         phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)],
//!         cost: (stamina: 10),
//!         hits: [(span: Phase("active"), shape: Cone(range: 2.0, angle: 100), damage: 20, poise: 15)],
//!         cancels: [(span: Secs(0.3, 0.55), into: [Combo("light", "light_2"), Input("dodge")])],
//!         displacement: [(0.0, 0.0), (0.3, 0.5)],
//!         anim: "light_1",
//!     ),
//!     "roll": (net_id: 4, phases: [("roll", 0.4)], cost: (stamina: 25), iframes: [Secs(0.02, 0.2)],
//!              displacement: [(0.35, 4.0)], steer: Move),
//!     "block": (net_id: 5, phases: [("raise", 0.1), ("hold", 0.3), ("lower", 0.1)],
//!               hold: (input: "block", span: Phase("hold")), move_speed: 0.4,
//!               guard: (span: Phases("raise", "hold"), perfect: Phase("raise"), damage_mult: 0.1)),
//!     "drink": (net_id: 6, phases: [("drink", 1.0)], cost: (charge: "flask"), effects: [(at: 0.6, effect: Heal(40))]),
//! }
//! // content/combat/hero.moveset.ron
//! { "hero": (inputs: {"light": "light_1", "dodge": "roll", "block": "block", "use_item.flask": "drink"},
//!            stagger: "stagger") }
//! // content/combat/statuses.status.ron
//! { "bleed": (threshold: 100, decay_per_s: 10, duration: 3, effect: "bleed") }
//! ```
//!
//! ## Order inside a tick
//!
//! ```text
//! SimSet::Input    ActionSystems::Orders    Order::Attack → Intent (after core's follow_orders)
//! SimSet::Act      ActionSystems::Context   guard presses (GuardArm), context bindings (ContextInputs)
//!                  ActionSystems::Machine   presses → buffer → start/advance → LocomotionInput
//!                  ActionSystems::Objects   combat objects: delayed starts, their runs step (effects, telegraphs)
//!                  ActionSystems::Effects   pairs start (start_pairs), heal, stamina, statuses, sounds, spawns, game effects
//! SimSet::Move     (controller)             integrates LocomotionInput, external included; then projectiles fly
//! SimSet::Resolve  ActionSystems::Hits      overlaps (a projectile sweeps its flight) → damage pipeline → events;
//!                                           projectiles that met a target or a wall impact
//!                  ActionSystems::Fighters  sprint drain, regeneration, cooldowns, memories age, pairs kept or lost,
//!                                           finished actions end
//!                  ActionSystems::Statuses  status ticks and expiry
//!                  ActionSystems::Deaths    combat objects whose action is over go; fighters at 0 health die
//! ```
//!
//! Tools: `light_attack`, `heavy_attack`, `block`, `parry`, `dodge`, `use_technique`,
//! `use_item`, `lock_on`, `attack` (player); `god`, `heal`, `stagger`, `react`, `remember`, `motion_bake` (dev);
//! `combat_overlay` (dev, with the `render` feature). Queries: `combat_objects` (player).
//!
//! ## The render feature
//!
//! With `render` the plugin also draws what the machine decides, in a window (a headless run
//! with the feature builds the same systems; nothing shows):
//!
//! - **Animation bridge** (`render::bridge`): one `AnimationGraph` per animset, built from
//!   its glTF, shared by every [`Animated`] character; each frame the clip of
//!   [`select_clip`] is blended in with `AnimationTransitions`, seeked to the action's
//!   progress while an action runs.
//! - **Telegraphs** (`render::overlay`): every [`HitZone`] that has a telegraph, and every one
//!   hostile to the local player, is drawn on the ground (outline, hatch, a filling ring, its
//!   symbol as a billboard) in any case.
//! - **Combat overlay** (`combat_overlay`, F2 in a window): hitboxes, hurtboxes, guard arcs,
//!   i-frames, every zone with its time left.
//! - **Off-screen warnings** (`render::hud`): a marker on the screen edge toward an attack
//!   about to land where the camera does not look, for the `LocalPlayer`.

pub mod ai;
pub mod anim;
mod animtool;
mod coverage;
pub mod bake;
pub mod body;
pub mod clipfile;
pub mod combat;
pub mod context;
pub mod cue;
pub mod def;
pub mod draw;
pub mod fighter;
pub mod leap;
pub mod lock;
pub mod machine;
mod invariants;
pub mod objects;
mod orders;
mod predict;
#[cfg(feature = "render")]
pub mod render;
pub mod stance;
pub mod status;
mod tools;
pub mod zone;

use ai_core::{AiAppExt, SimSet};
use bevy::prelude::*;
use shards_content::ContentAppExt;
use shards_core::{CoreSystems, EngineAppExt, NetSet, Predicted};
use shards_motion::{ControllerSystems, ModifierSystems, MotionPlugin, MotionSystems};

pub use anim::{
    crossed_cues, cut_holds, equip_open, euler_degrees, locomotion_state, locomotion_state_at, mount_places, select_clip,
    select_clip_for, select_clip_in, select_layer, select_layer_in, ActionClip, AnimSet, Animated, AnimationCue, BodyStatus,
    Carry, ClipChoice, ClipDef, CueDef, GroundSpeeds, LayerChoice, LayerDef, Locomotion, MountMove, MountRules, MountView, Place,
    PropDef, PropSwaps, SocketDef, EQUIP_EVENTS, EQUIP_KEY, MAX_LAYERS,
    mode_name, parse_mode, DropDef, ModeLeft, LEFT_KEY, MODES, MODE_KEY,
};
pub use combat::{
    deal_damage, end_break, kill, kill_with, poise_damage, react, reset_poise, revive, stagger, Blocked, DamageCtx, DamageHook, DamageHooks, Damaged, Died,
    Evaded, GuardBroken, HitWeight, IgnoreReason, Outcome, Parried, PerfectEvaded, Reacted, Reaction,
    StaggerCause, Staggered,
};
pub use body::{
    clear_barrier, clear_damage_taken, extend_damage_taken, grant_barrier, set_damage_taken, Barrier, BarrierGranted, DamageTaken,
    DamageTakenExpired, ProcGates, TakenMult,
};
pub use context::{
    cut_action, forget, hold_break, pair_victim, release_pair, remember, Bound, ContextCondition, ContextConditions, ContextInputs,
    GuardArm, Memory, PairEnd, PairEnded, PairStarted, Paired, Pairing, Remembered, RunHalted,
};
pub use cue::{ActionSound, SoundCue};
pub use def::{
    ActionDef, AssistDef, BindingDef, BufferMode, CancelDef, ChargeDef, CombatTuning, Cost, Effect, Forget, GuardDef, GuardPress, HitDef, HitFlag,
    HitShape, HoldDef, LeapDef, LeapSearch, Local, MemoryDef, MotionBake, PairDef, PairSelect, PairState, PairTeam, ReactionDef, Recall, StaminaMode, When,
    Face, MovesetDef, Phase, Span, Steer, TelegraphDef, TimedEffect, Transition,
};
pub use fighter::{
    grant_protection, set_reaction_immunity, Armor, Charge, Charges, Cooldown, Cooldowns, Dead, Fighter, Health, Invulnerable, Knockback, Poise,
    Protection, Stamina, Team,
};
pub use leap::{plan_leap, LeapPlan, LeapSite};
pub use machine::{
    action_progress, apply_effect, can_start, extend_recovery, force_action, press_life, ActionEffect, ActionEffects, ActionEnd, ActionEnded,
    ActionMissed, ActionRejected,
    ActionStarted,
    intangible, off_ground, state_intangible, ActionState, Blocker, FighterView, Healed, InputBuffer, LeapRun, Queued, RunExtras, Running, Start, Telegraph,
};
pub use lock::{LockLoss, LockLost, LockReleased, LockSight, LockSwitched, LockSystems};
pub use predict::{shown_queue, shown_state, ActionPredictor, PredictedAction, CONFIRM_SLACK};
pub use stance::{actions_to_modifiers, hold_regen_during_actions};
pub use status::{
    statuses_to_modifiers, NoStatusSlow,
    add_status_buildup, StatusApplied, StatusDef, StatusEffects, StatusExpired, StatusHook, StatusHooks, StatusResist,
    Statuses,
};
pub use objects::{
    despawn_objects_of, live_tags_at, objects_of, spawn_objects, CombatObject, ObjectsSpawned, Projectile, ProjectileFlight, RepeatMark,
    Repeats, SpawnAim, SpawnDef, SpawnPattern,
};
pub use zone::{action_zones, hit_zones, threat_indicators, HitZone, Indicator, WorldShape};

/// Version of the action module.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The types a game uses most.
pub mod prelude {
    pub use crate::{
        ActionAppExt, ActionDef, ActionPlugin, ActionState, ActionSystems, AnimSet, Animated, Armor, Charges, CombatTuning,
        Dead, Fighter, Health, Invulnerable, MovesetDef, Poise, Stamina, StatusDef, StatusResist, Statuses, Team,
    };
}

/// The action module's systems, for ordering a game's systems against them.
#[derive(SystemSet, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ActionSystems {
    /// `SimSet::Input`, after `CoreSystems::FollowOrders`: `Order::Attack`.
    Orders,
    /// `SimSet::Act`, before the machine: guard presses arm, and every fighter's context
    /// bindings are resolved ([`ContextInputs`]).
    Context,
    /// `SimSet::Act`: every fighter's action machine.
    Machine,
    /// `SimSet::Act`, after the machine: the combat objects' delayed starts and runs
    /// (`objects::run_objects`; [`objects`] says when they spawn, fly and go).
    Objects,
    /// `SimSet::Act`, after the machine: effects the actions reached.
    Effects,
    /// `SimSet::Act`, after the effects: pushes (a reaction's knockback) add to the displacement
    /// the machine wrote.
    Push,
    /// `SimSet::Resolve`: hit detection and the damage pipeline.
    Hits,
    /// `SimSet::Resolve`: sprint drain, regeneration, cooldowns; actions that ran out end.
    Fighters,
    /// `SimSet::Resolve`: statuses tick and expire.
    Statuses,
    /// `SimSet::Resolve`, last: fighters at 0 health die.
    Deaths,
    /// `NetSet::Predict`, between `MotionSystems::PredictOrders` and `PredictIntent`: a
    /// client's own body walks into reach of its `Order::Attack` (the walk only).
    PredictOrders,
    /// `NetSet::Predict`, between `MotionSystems::PredictIntent` and the controller: a
    /// client starts its own body's actions ahead of the host ([`PredictedAction`]).
    Predict,
}

/// Registration API for games.
pub trait ActionAppExt {
    /// A step of the damage pipeline, after the engine's modifiers (guards, armor),
    /// before damage applies; hooks run in registration order.
    fn add_damage_hook(&mut self, hook: DamageHook) -> &mut Self;

    /// The game's code for status effect `name` (a [`StatusDef::effect`]): when it
    /// applies, every tick while active, when it expires.
    fn add_status_effect(
        &mut self,
        name: &str,
        on_apply: StatusHook,
        on_tick: StatusHook,
        on_expire: StatusHook,
    ) -> &mut Self;

    /// The game's code for an action's `Custom(name)` effect.
    fn add_action_effect(&mut self, name: &str, effect: ActionEffect) -> &mut Self;

    /// The game's condition `name` for context bindings (`When::Condition(name)`): whether it
    /// holds for the fighter now. The same name replaces a condition.
    fn add_context_condition(&mut self, name: &str, condition: ContextCondition) -> &mut Self;
}

impl ActionAppExt for App {
    fn add_damage_hook(&mut self, hook: DamageHook) -> &mut Self {
        self.world_mut().get_resource_or_init::<DamageHooks>().0.push(hook);
        self
    }

    fn add_status_effect(
        &mut self,
        name: &str,
        on_apply: StatusHook,
        on_tick: StatusHook,
        on_expire: StatusHook,
    ) -> &mut Self {
        let hooks = StatusHooks { on_apply, on_tick, on_expire };
        self.world_mut().get_resource_or_init::<StatusEffects>().insert(name, hooks);
        self
    }

    fn add_action_effect(&mut self, name: &str, effect: ActionEffect) -> &mut Self {
        let mut effects = self.world_mut().get_resource_or_init::<ActionEffects>();
        effects.0.retain(|(n, _)| n != name);
        effects.0.push((name.to_owned(), effect));
        self
    }

    fn add_context_condition(&mut self, name: &str, condition: ContextCondition) -> &mut Self {
        let mut conditions = self.world_mut().get_resource_or_init::<ContextConditions>();
        conditions.0.retain(|(n, _)| n != name);
        conditions.0.push((name.to_owned(), condition));
        self
    }
}

/// Actions, combat, fighter resources and statuses (adds [`MotionPlugin`] if it is
/// missing). Needs `CorePlugin` and `ContentPlugin` (`shards::EnginePlugins` adds all).
pub struct ActionPlugin;

impl Plugin for ActionPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<MotionPlugin>() {
            app.add_plugins(MotionPlugin);
        }
        // Explicitly, for apps built without `reflect_auto_register`: replay checks,
        // prefabs and `inspect` read components through the type registry.
        app.register_type::<Fighter>()
            .register_type::<ActionState>()
            .register_type::<InputBuffer>()
            .register_type::<Health>()
            .register_type::<Stamina>()
            .register_type::<Poise>()
            .register_type::<Armor>()
            .register_type::<Team>()
            .register_type::<Charges>()
            .register_type::<Cooldowns>()
            .register_type::<Dead>()
            .register_type::<Invulnerable>()
            .register_type::<Statuses>()
            .register_type::<StatusResist>()
            .register_type::<NoStatusSlow>()
            .register_type::<Protection>()
            .register_type::<Barrier>()
            .register_type::<DamageTaken>()
            .register_type::<ProcGates>()
            .register_type::<BarrierGranted>()
            .register_type::<DamageTakenExpired>()
            .register_type::<Knockback>()
            .register_type::<RunExtras>()
            .register_type::<Memory>()
            .register_type::<ContextInputs>()
            .register_type::<GuardArm>()
            .register_type::<Pairing>()
            .register_type::<Paired>()
            .register_type::<RunHalted>()
            .register_type::<PairStarted>()
            .register_type::<PairEnded>()
            .register_type::<LeapRun>()
            .register_type::<Animated>()
            .register_type::<PropSwaps>()
            .register_type::<ActionMissed>()
            // Typed for the network layer, which forwards them to clients.
            .register_type::<ActionStarted>()
            .register_type::<ActionRejected>()
            .register_type::<Telegraph>()
            .register_type::<ActionSound>()
            .register_type::<Healed>()
            .register_type::<Damaged>()
            .register_type::<Blocked>()
            .register_type::<Parried>()
            .register_type::<Evaded>()
            .register_type::<PerfectEvaded>()
            .register_type::<Reacted>()
            .register_type::<GuardBroken>()
            .register_type::<Staggered>()
            .register_type::<Died>();
        // Known to the world from the start, so `&World` queries (tools, zones) work
        // before the first tick or the first death.
        let world = app.world_mut();
        world.register_component::<Fighter>();
        world.register_component::<ActionState>();
        world.register_component::<Team>();
        world.register_component::<Dead>();
        world.register_component::<Invulnerable>();
        app.add_content::<ActionDef>()
            .add_content::<MovesetDef>()
            .add_content::<StatusDef>()
            .add_content::<CombatTuning>()
            .add_content::<AnimSet>()
            .add_content::<PropDef>()
            .init_resource::<DamageHooks>()
            .init_resource::<StatusEffects>()
            .init_resource::<ActionEffects>()
            .init_resource::<machine::PendingEffects>()
            .init_resource::<context::PendingPairs>()
            .init_resource::<ContextConditions>()
            .add_engine_module("action", VERSION)
            .add_engine_invariant("action.resources_in_range", invariants::resources_in_range)
            .add_engine_invariant("action.running_sane", invariants::running_sane)
            .add_engine_invariant("action.protection_sane", invariants::protection_sane)
            .add_engine_invariant("action.barrier_sane", invariants::barrier_sane)
            .add_engine_invariant("action.damage_taken_sane", invariants::damage_taken_sane)
            .add_engine_invariant("action.knockback_sane", invariants::knockback_sane)
            .add_engine_invariant("action.extras_follow_run", invariants::extras_follow_run)
            .add_engine_invariant("action.pairs_match", invariants::pairs_match)
            .add_engine_invariant("action.memory_sane", invariants::memory_sane)
            .add_engine_invariant("action.leap_follows_run", invariants::leap_follows_run)
            .add_engine_invariant("action.ghost_follows_run", invariants::ghost_follows_run)
            .add_engine_invariant("action.objects_sane", invariants::objects_sane)
            .add_engine_invariant("action.objects_capped", invariants::objects_capped);
        coverage::plugin(app);
        app
            // The machine runs after motion's `Intent → LocomotionInput` step: it
            // overrides what that step wrote (external, locked, face, sprint).
            .configure_sets(
                FixedUpdate,
                (
                    ActionSystems::Orders.in_set(SimSet::Input).after(CoreSystems::FollowOrders).after(MotionSystems::StallOrders),
                    ModifierSystems::Reset.after(ActionSystems::Orders),
                    (ActionSystems::Context, ActionSystems::Machine, ActionSystems::Objects, ActionSystems::Effects, ActionSystems::Push)
                        .chain()
                        .in_set(SimSet::Act)
                        .after(MotionSystems::Intent),
                    (ActionSystems::Hits, ActionSystems::Fighters, ActionSystems::Statuses, ActionSystems::Deaths)
                        .chain()
                        .in_set(SimSet::Resolve),
                    ActionSystems::PredictOrders
                        .in_set(NetSet::Predict)
                        .after(MotionSystems::PredictOrders)
                        .before(MotionSystems::PredictIntent),
                    ActionSystems::Predict
                        .in_set(NetSet::Predict)
                        .after(MotionSystems::PredictIntent)
                        .before(ControllerSystems::PredictMove),
                ),
            )
            .add_systems(
                FixedUpdate,
                (
                    orders::follow_attack_orders.in_set(ActionSystems::Orders),
                    machine::run_machines.in_set(ActionSystems::Machine),
                    machine::apply_effects.in_set(ActionSystems::Effects),
                    machine::check_misses.in_set(ActionSystems::Hits).after(combat::resolve_hits),
                    combat::resolve_hits.in_set(ActionSystems::Hits),
                    fighter::push_bodies.in_set(ActionSystems::Push),
                    stance::hold_regen_during_actions.in_set(ActionSystems::Fighters).before(fighter::update_fighters),
                    fighter::update_fighters.in_set(ActionSystems::Fighters),
                    machine::finish_actions.in_set(ActionSystems::Fighters).after(fighter::update_fighters),
                    fighter::tick_protection.in_set(ActionSystems::Fighters).after(machine::finish_actions),
                    body::tick_body_state.in_set(ActionSystems::Fighters).after(fighter::tick_protection),
                    status::update_statuses.in_set(ActionSystems::Statuses),
                    status::statuses_to_modifiers::<()>.in_set(ModifierSystems::Statuses),
                    status::statuses_to_modifiers::<With<Predicted>>.in_set(ModifierSystems::PredictStatuses),
                    stance::actions_to_modifiers::<()>.in_set(ModifierSystems::Contribute).after(ModifierSystems::Statuses),
                    stance::actions_to_modifiers::<With<Predicted>>
                        .in_set(ModifierSystems::PredictContribute)
                        .after(ModifierSystems::PredictStatuses),
                    combat::check_deaths.in_set(ActionSystems::Deaths),
                    orders::steer_attack_orders.in_set(ActionSystems::PredictOrders),
                    predict::predict_actions.in_set(ActionSystems::Predict),
                ),
            )
            .add_observer(fighter::hurtbox_on_collider)
            .add_observer(fighter::protect_after_reaction)
            // Cosmetic: a game may add it in a window only, and replays must not care.
            .replay_ignore::<Animated>()
            .replay_ignore::<PropSwaps>()
            .net_ignore::<PropSwaps>();
        context::plugin(app);
        predict::plugin(app);
        lock::plugin(app);
        ai::register(app);
        tools::register(app);
        animtool::register(app);
        bake::register(app);
        cue::plugin(app);
        objects::plugin(app);
        #[cfg(feature = "render")]
        render::plugin(app);
    }
}
