//! What the AI sees of this game and what it can do in it.

use ai_core::prelude::*;
use ai_core::spatial::{compass_vector, fmt_pos, relative};
use ai_core::AiIndex;
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;

use crate::gameplay::combat::{
    self, Act, Action, ActionRejected, Blocker, Damaged, Died, Dodged, Health, InputBuffer, PhaseChanged, Stamina,
    Telegraph, INPUT_BUFFER,
};
use crate::gameplay::intent::{flat, Intent, Order, Target};
use crate::gameplay::{self, Boss};

pub struct GameAiPlugin;

impl Plugin for GameAiPlugin {
    fn build(&self, app: &mut App) {
        app.register_ai_component::<Health>(|h| format!("hp {:.0}/{:.0}", h.current, h.max))
            .register_ai_component::<Stamina>(|s| format!("stam {:.0}/{:.0}", s.current, s.max))
            .register_ai_component::<Boss>(|b| format!("phase {}", b.phase))
            .register_ai_component::<Action>(Action::describe)
            .register_ai_component::<InputBuffer>(describe_buffer)
            .register_ai_describer("orders", |e| {
                let intent = e.get::<Intent>()?;
                let first = intent.orders.front()?;
                let more = intent.orders.len() - 1;
                Some(if more > 0 { format!("order {} (+{more})", first.describe()) } else { format!("order {}", first.describe()) })
            })
            .journal_event::<Telegraph>(|e, w| {
                Some(format!("{} winds up {:?} ({:.2}s)", label(w, e.attacker), e.attack, e.windup))
            })
            .journal_event::<Damaged>(|e, w| {
                Some(format!(
                    "{} -> {} {:?} -{:.0} (hp {:.0})",
                    label(w, e.source),
                    label(w, e.target),
                    e.attack,
                    e.amount,
                    e.hp_left
                ))
            })
            .journal_event::<Dodged>(|e, w| {
                Some(format!("{} dodged {:?} from {}", label(w, e.target), e.attack, label(w, e.source)))
            })
            .journal_event::<Died>(|e, w| {
                let name = w.get::<Name>(e.entity).map(|n| n.as_str().to_owned()).unwrap_or_default();
                Some(match e.killer {
                    Some(k) => format!("{} {name} died (killed by {})", label(w, e.entity), label(w, k)),
                    None => format!("{} {name} died", label(w, e.entity)),
                })
            })
            .journal_event::<PhaseChanged>(|e, w| Some(format!("{} phase {}->{}", label(w, e.boss), e.from, e.to)))
            .journal_event::<ActionRejected>(|e, w| Some(format!("{} {} rejected: {}", label(w, e.entity), e.action, e.reason)))
            .add_ai_condition("boss_phase_2", |w| {
                w.try_query::<&Boss>().is_some_and(|mut q| q.iter(w).any(|b| b.phase >= 2))
            })
            .add_ai_condition("player_dead", |w| {
                w.try_query_filtered::<&Action, With<gameplay::Player>>()
                    .is_some_and(|mut q| q.iter(w).any(|a| matches!(a, Action::Dead { .. })))
            })
            .replay_input::<Intent>()
            .add_ai_tool("move_to", Scope::Player, move_to)
            .add_ai_tool("move", Scope::Player, move_dir)
            .add_ai_tool("attack", Scope::Player, attack)
            .add_ai_tool("dodge", Scope::Player, dodge)
            .add_ai_tool("stop", Scope::Player, stop)
            .add_ai_tool("spawn", Scope::Dev, spawn)
            .add_ai_tool("heal", Scope::Dev, heal);
    }
}

/// `queued dodge 0.15s`: a one-shot input waiting for the character to be able to act.
fn describe_buffer(b: &InputBuffer) -> String {
    let dodge = b.dodge.map(|d| format!("queued dodge {:.2}s", d.left));
    let attack = b.attack.map(|a| format!("queued attack {:?} {:.2}s", a.input, a.left));
    [dodge, attack].into_iter().flatten().collect::<Vec<_>>().join("  ")
}

/// Whether `act` can start now, checked by the game's own rule ([`combat::start_blocker`]):
/// `Ok(None)` now, `Ok(Some(s))` in `s` seconds (within the input buffer, so it will),
/// or why it will not.
fn when_can_start(act: Act, action: &Action, stamina: Option<&Stamina>) -> Result<Option<f32>, String> {
    let Some(blocker) = combat::start_blocker(act, action, stamina) else {
        return Ok(None);
    };
    match (blocker, blocker.ready_in()) {
        (_, Some(t)) if t <= INPUT_BUFFER + 1e-4 => Ok(Some(t)),
        (Blocker::Stamina { have, need, ready_in }, _) => Err(match ready_in {
            Some(t) => format!("not enough stamina: {have:.0}/{need:.0} (ready in {t:.1}s)"),
            None => format!("not enough stamina: {have:.0}/{need:.0}"),
        }),
        (Blocker::Busy { action, ready_in }, _) => {
            let mut msg = format!("busy: {} (ready in {ready_in:.2}s)", action.describe());
            if matches!(action, Action::Windup { .. }) {
                msg.push_str("; a windup cannot be cancelled");
            }
            Err(msg)
        }
        (Blocker::Dead, _) => Err("you are dead; a dev agent can `heal` you".into()),
    }
}

/// Where to go: an entity id (42 or "e42") or a ground point {"x": 3, "z": -4}.
#[derive(Deserialize, JsonSchema, Clone, Copy, Debug)]
#[serde(untagged)]
pub enum TargetArg {
    Entity(Id),
    Point { x: f32, z: f32 },
}

fn resolve_target(index: &AiIndex, t: TargetArg) -> Result<Target, String> {
    Ok(match t {
        TargetArg::Entity(id) => {
            index.resolve(id.0)?;
            Target::Entity(id.0)
        }
        TargetArg::Point { x, z } => Target::Point(Vec3::new(x, 0.0, z)),
    })
}

fn my_intent<'a>(caller: &Caller, q: &'a mut Query<(&Transform, &mut Intent, &Action)>) -> Result<(Vec3, Mut<'a, Intent>), String> {
    let me = caller.require_subject()?;
    let (tf, intent, action) = q.get_mut(me).map_err(|_| "your body cannot be controlled (no Intent)".to_string())?;
    if matches!(action, Action::Dead { .. }) {
        return Err("you are dead; a dev agent can `heal` you".into());
    }
    Ok((tf.translation, intent))
}

/// Walk to an entity or a point. Replaces your current orders unless `queue` is set.
/// The game steers every tick; check progress with `observe` or `step`.
#[derive(Deserialize, JsonSchema)]
pub struct MoveToArgs {
    pub target: TargetArg,
    /// Stop this close to the target (meters). Default 1.5.
    pub within: Option<f32>,
    /// Append to the order queue instead of replacing it.
    #[serde(default)]
    pub queue: bool,
}

fn move_to(
    In(call): In<Call<MoveToArgs>>,
    index: Res<AiIndex>,
    mut q: Query<(&Transform, &mut Intent, &Action)>,
    positions: Query<&Transform>,
) -> ToolResult {
    let target = resolve_target(&index, call.args.target)?;
    let (here, mut intent) = my_intent(&call.caller, &mut q)?;
    if !call.args.queue {
        intent.orders.clear();
    }
    let order = Order::MoveTo { target, within: call.args.within.unwrap_or(1.5).max(0.2) };
    let desc = order.describe();
    intent.orders.push_back(order);
    let goal = match target {
        Target::Point(p) => Some(p),
        Target::Entity(id) => index.get(id).and_then(|e| positions.get(e).ok()).map(|t| t.translation),
    };
    Ok(match goal {
        Some(g) => format!("ok: {desc}, {} away", relative(here, g)),
        None => format!("ok: {desc}"),
    })
}

/// Walk a distance in a compass direction (N = -Z, E = +X).
#[derive(Deserialize, JsonSchema)]
pub struct MoveArgs {
    /// N, NE, E, SE, S, SW, W or NW.
    pub dir: String,
    pub meters: f32,
}

fn move_dir(In(call): In<Call<MoveArgs>>, mut q: Query<(&Transform, &mut Intent, &Action)>) -> ToolResult {
    let dir = compass_vector(&call.args.dir).ok_or_else(|| format!("unknown direction `{}`; use N, NE, E, SE, S, SW, W, NW", call.args.dir))?;
    let (here, mut intent) = my_intent(&call.caller, &mut q)?;
    let goal = here + dir * call.args.meters.clamp(0.0, 60.0);
    intent.orders.clear();
    intent.orders.push_back(Order::MoveTo { target: Target::Point(goal), within: 0.3 });
    Ok(format!("ok: moving to {}", fmt_pos(goal)))
}

/// Attack. With `target`: walk into sword range and swing (`repeat`: until it dies).
/// Without: swing once right now in the direction you face (10 stamina; from idle only:
/// a busy character queues the swing for up to 0.2 s or is refused with the reason).
#[derive(Deserialize, JsonSchema, Default)]
pub struct AttackArgs {
    pub target: Option<Id>,
    #[serde(default)]
    pub repeat: bool,
}

fn attack(
    In(call): In<Call<AttackArgs>>,
    index: Res<AiIndex>,
    stamina: Query<&Stamina>,
    mut q: Query<(&Transform, &mut Intent, &Action)>,
) -> ToolResult {
    if let Some(t) = call.args.target {
        index.resolve(t.0)?;
    }
    let me = call.caller.require_subject()?;
    let action = q.get(me).map(|(_, _, a)| *a).unwrap_or_default();
    let (_, mut intent) = my_intent(&call.caller, &mut q)?;
    match call.args.target {
        Some(t) => {
            intent.orders.clear();
            intent.orders.push_back(Order::Attack { target: t.0, repeat: call.args.repeat });
            Ok(format!("ok: attacking {t}{}", if call.args.repeat { " until it dies" } else { "" }))
        }
        None => {
            let sword = combat::AttackKind::Sword;
            let when = when_can_start(Act::Attack(sword), &action, stamina.get(me).ok())?;
            intent.attack = Some(sword);
            Ok(match when {
                None => "ok: swinging".into(),
                Some(t) => format!("ok: queued, swings in {t:.2}s"),
            })
        }
    }
}

/// Dodge roll: 0.35 s of invulnerability, 4 m of distance, 25 stamina.
/// Best used when an enemy windup is about to end. Works from idle or during a
/// recovery (cancelling it); otherwise it waits up to 0.2 s ("ok: queued") or is
/// refused with the reason and when it would work.
#[derive(Deserialize, JsonSchema, Default)]
pub struct DodgeArgs {
    /// Compass direction (N, NE, ...). Default: away from the nearest enemy windup, else forward.
    pub dir: Option<String>,
    /// Or: dodge directly away from this entity.
    pub away_from: Option<Id>,
}

fn dodge(
    In(call): In<Call<DodgeArgs>>,
    index: Res<AiIndex>,
    positions: Query<&Transform>,
    stamina: Query<&Stamina>,
    mut q: Query<(&Transform, &mut Intent, &Action)>,
) -> ToolResult {
    let away = match call.args.away_from {
        Some(id) => Some(positions.get(index.resolve(id.0)?).map_err(|_| "no position")?.translation),
        None => None,
    };
    let me = call.caller.require_subject()?;
    let action = q.get(me).map(|(_, _, a)| *a).unwrap_or_default();
    let (here, mut intent) = my_intent(&call.caller, &mut q)?;
    let dir = match (&call.args.dir, away) {
        (Some(d), _) => compass_vector(d).ok_or_else(|| format!("unknown direction `{d}`"))?,
        (None, Some(p)) => flat(here - p).normalize_or_zero(),
        (None, None) => Vec3::ZERO, // forward
    };
    // Refused calls leave the intent (and the orders) alone.
    let when = when_can_start(Act::Dodge, &action, stamina.get(me).ok())?;
    intent.dodge = Some(dir);
    intent.orders.clear();
    let towards = if dir == Vec3::ZERO { "forward".into() } else { ai_core::spatial::compass(dir).to_string() };
    Ok(match when {
        None => format!("ok: dodging {towards}"),
        Some(t) => format!("ok: queued, dodges in {t:.2}s ({towards})"),
    })
}

/// Cancel all orders and stand still.
fn stop(In(call): In<Call<NoArgs>>, mut q: Query<(&Transform, &mut Intent, &Action)>) -> ToolResult {
    let (_, mut intent) = my_intent(&call.caller, &mut q)?;
    intent.orders.clear();
    intent.move_dir = Vec3::ZERO;
    Ok("ok: stopped".into())
}

/// Spawn a monster at a ground point.
#[derive(Deserialize, JsonSchema)]
pub struct SpawnArgs {
    /// "skeleton" or "boss".
    pub kind: String,
    pub x: f32,
    pub z: f32,
}

fn spawn(In(call): In<Call<SpawnArgs>>, world: &mut World) -> ToolResult {
    let at = Vec3::new(call.args.x, 0.0, call.args.z);
    let mut queue = bevy::ecs::world::CommandQueue::default();
    let entity = {
        let mut commands = Commands::new(&mut queue, world);
        match call.args.kind.to_lowercase().as_str() {
            "skeleton" => gameplay::spawn_skeleton(&mut commands, at),
            "boss" => gameplay::spawn_boss(&mut commands, at),
            other => return Err(format!("unknown kind `{other}`; use skeleton or boss")),
        }
    };
    queue.apply(world);
    let id = world.get::<AiId>(entity).copied().ok_or("spawn failed")?;
    Ok(format!("spawned {id} {} at {}", call.args.kind, fmt_pos(at)))
}

/// Restore an entity to full health (revives the dead player). Default: you.
#[derive(Deserialize, JsonSchema, Default)]
pub struct HealArgs {
    pub id: Option<Id>,
}

fn heal(In(call): In<Call<HealArgs>>, index: Res<AiIndex>, mut q: Query<(&mut Health, &mut Action)>) -> ToolResult {
    let e = match call.args.id {
        Some(id) => index.resolve(id.0)?,
        None => call.caller.require_subject()?,
    };
    let (mut health, mut action) = q.get_mut(e).map_err(|_| "entity has no Health")?;
    health.current = health.max;
    if matches!(*action, Action::Dead { .. }) {
        *action = Action::Idle;
    }
    Ok(format!("healed to {:.0}", health.max))
}
