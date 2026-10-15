//! What an AI agent can do in this game that the engine does not know about, and what it
//! can wait for. The engine already exposes movement, combat, content and time as tools
//! (`shards tools` lists them); a game adds only its own rules, the same way.
//!
//! - an **action** tool changes the world: it is journaled, answers with the changes it
//!   caused, and is recorded in replays (`add_ai_tool`);
//! - a **query** tool only reads (`add_ai_query`);
//! - a **condition** is a name `step --until named:NAME` stops on (`add_ai_condition`).
//!
//! Tool errors teach the fix: say what is wrong and what to do instead.

use shards::ai_core::{AiConditions, AiIndex};
use shards::core::flat;
use shards::core::input_names::LIGHT;
use shards::prelude::*;

use crate::brain::Grunt;

pub struct GameAiPlugin;

impl Plugin for GameAiPlugin {
    fn build(&self, app: &mut App) {
        app.add_ai_tool("taunt", Scope::Player, taunt)
            .add_ai_condition("arena_cleared", arena_cleared)
            // What the music listens to (`content/audio/yard.music.ron`): a foe near, a fight on.
            .add_ai_condition("foe_near", foe_near)
            .add_ai_condition("in_fight", in_fight);
    }
}

/// Draw a grunt's attention: it drops what it is doing and comes for you, even from beyond
/// its own aggro range. Pull them one at a time to fight a crowd.
///
/// The doc comments of this struct and its fields become the tool's description in
/// `shards tools --json`, so write them for the agent that reads them.
#[derive(Deserialize, JsonSchema)]
#[serde(crate = "shards::serde")]
#[schemars(crate = "shards::schemars")]
pub struct TauntArgs {
    /// The grunt to taunt: an entity id such as e2 (`observe` lists them).
    pub target: Id,
}

/// `Scope::Player`: a player could do this, so it works through the game's own rules (the
/// grunt's `Intent`) and needs a body (`call.caller.require_subject()`).
fn taunt(
    In(call): In<Call<TauntArgs>>,
    index: Res<AiIndex>,
    ids: Query<&AiId>,
    mut grunts: Query<(&mut Intent, Has<Dead>), With<Grunt>>,
) -> ToolResult {
    let me = call.caller.require_subject()?;
    let my_id = *ids.get(me).map_err(|_| "your body has no id; possess a character first".to_owned())?;
    let target = call.args.target;
    // `resolve` answers with the engine's own errors: `e57 no longer exists (Grunt despawned at tick 812)`.
    let grunt = index.resolve(target.0)?;
    let (mut intent, dead) =
        grunts.get_mut(grunt).map_err(|_| format!("{target} is not a grunt; taunt works on grunts, see `observe` for their ids"))?;
    if dead {
        return Err(format!("{target} is already dead"));
    }
    intent.orders.clear();
    intent.orders.push_back(Order::Attack { target: my_id, input: LIGHT.to_owned(), repeat: true });
    Ok(format!("ok: {target} comes for {my_id}"))
}

/// True once the arena has grunts and every one of them is dead:
/// `shards step 3000 --until named:arena_cleared`.
fn arena_cleared(world: &World) -> bool {
    let Some(mut grunts) = world.try_query_filtered::<Has<Dead>, With<Grunt>>() else { return false };
    let dead: Vec<bool> = grunts.iter(world).collect();
    !dead.is_empty() && dead.into_iter().all(|dead| dead)
}

/// How far past its aggro a grunt counts as near for `foe_near` (× the aggro).
const NEAR_AGGRO: f32 = 1.5;

/// A hero could be noticed any moment: within half again a living grunt's aggro, or near an awake
/// raider (the engine's `npc_near`). The yard's music plays `tension` then
/// (`content/audio/yard.music.ron`).
fn foe_near(world: &World) -> bool {
    let heroes: Vec<Vec3> = world
        .try_query_filtered::<&Transform, (With<PlayerId>, Without<Dead>)>()
        .map(|mut q| q.iter(world).map(|t| t.translation).collect())
        .unwrap_or_default();
    let near_a_grunt = world.try_query_filtered::<(&Grunt, &Transform), Without<Dead>>().is_some_and(|mut grunts| {
        grunts.iter(world).any(|(grunt, at)| heroes.iter().any(|hero| flat(*hero - at.translation).length() <= grunt.aggro * NEAR_AGGRO))
    });
    near_a_grunt || engine_condition(world, "npc_near")
}

/// A grunt fights: a hero is within its aggro, or it has an attack order (a taunt brings it from
/// farther); or a raider fights (the engine's `npc_engaged`). The yard's music plays `combat`.
fn in_fight(world: &World) -> bool {
    let heroes: Vec<Vec3> = world
        .try_query_filtered::<&Transform, (With<PlayerId>, Without<Dead>)>()
        .map(|mut q| q.iter(world).map(|t| t.translation).collect())
        .unwrap_or_default();
    let a_grunt_fights = world.try_query_filtered::<(&Grunt, &Transform, &Intent), Without<Dead>>().is_some_and(|mut grunts| {
        grunts.iter(world).any(|(grunt, at, intent)| {
            intent.orders.iter().any(|order| matches!(order, Order::Attack { .. }))
                || heroes.iter().any(|hero| flat(*hero - at.translation).length() <= grunt.aggro)
        })
    });
    a_grunt_fights || engine_condition(world, "npc_engaged")
}

/// A condition the engine registered, by name (false without one of that name).
fn engine_condition(world: &World, name: &str) -> bool {
    world.get_resource::<AiConditions>().and_then(|conditions| conditions.0.get(name)).is_some_and(|condition| condition(world))
}
