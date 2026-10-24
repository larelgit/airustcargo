//! Use by looking (F5): what a press of `interact` does to the thing a character looks at.
//!
//! [`shards_motion::use_target`] is the ray: from the eye along the look against the `USE` and `WORLD`
//! layers, the nearest [`Usable`] in reach with no wall nearer. This module is the simulation's side
//! of it, and what agents see:
//!
//! - **`ActionSystems::Use`** (`SimSet::Act`, after the context is resolved and before the machine), in
//!   `AiId` order: a body's `interact` press that finds a target uses it. A [`Usable`] with no
//!   `hold_s` is used at once, with the event [`Used`] `{ who, what, name }` (journalled); one with a
//!   `hold_s` starts a [`UseProgress`] on the body, which grows while the button stays held and the
//!   look stays on the same thing, and ends in `Used` at `hold_s`. Looking away, getting out of
//!   reach, letting go, dying or going down cancels it (nothing is told: the component is gone).
//! - **A revive wins.** A body whose moveset binds `interact` to an action that holds now (a revive
//!   next to a downed ally: [`ContextInputs::bound`]) leaves the press to the machine, which starts
//!   it; a use never takes that press. When the binding does not hold (no ally in reach), a target
//!   in reach takes the press.
//! - What a use *does* (open a door, take the tablet, refuse a locked door) is the game's: it observes
//!   [`Used`] and answers with what it owns (`mover_go`, a pickup, a line of text).
//!
//! For agents: the `interact` tool (`interact`, `interact target=e12`, `interact hold=true`), the
//! `USE e12 "door" 1.4m` line of `observe`, and for a HUD the sources `you.use_target` (the prompt)
//! and `you.use_progress` (a hold's progress, a ratio) through [`use_view`].

use ai_core::prelude::*;
use ai_core::label;
use bevy::ecs::schedule::SystemCondition;
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_content::ContentDb;
use shards_core::input_names::INTERACT;
use shards_core::{Gaze, Intent};
use shards_motion::{eye_of, params_of, use_point, use_probe, use_target, Usable, UseTarget};

use crate::combat::Downed;
use crate::context::{resolve as resolve_context, ContextInputs};
use crate::def::{MovesetDef, EPS};
use crate::fighter::{Dead, Fighter};
use crate::look::{gaze_toward, surface, wrap};
use crate::machine::{understands, Blocker};
use crate::tools::press_held;

/// A character used a [`Usable`]: a press that found it (a plain thing), or a hold that reached its
/// `hold_s`. Triggered in the simulation, in `AiId` order of the users, on the machine that runs it
/// (the host in co-op; a client is not told). The game observes it.
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct Used {
    /// The character that used it.
    pub who: Entity,
    /// The thing used (the entity with the [`Usable`]).
    pub what: Entity,
    /// The [`Usable`]'s name (the marker's `name`).
    pub name: String,
}

/// A hold-to-use in progress, on the character that holds `interact` ([`Usable::hold_s`]): the thing
/// it holds on and the seconds it has held so far. Simulation state (snapshots, the replay hash):
/// it starts with the press, grows each tick while the button is held and the look stays on the same
/// thing in reach, and goes with the [`Used`] at `hold_s`, or when any of that stops.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct UseProgress {
    /// The [`Usable`] being used.
    pub what: AiId,
    /// Seconds held so far.
    pub elapsed: f32,
}

/// What a HUD shows of a character's use ([`use_view`]).
#[derive(Clone, Debug, PartialEq)]
pub struct UseView {
    /// The [`Usable`]'s name.
    pub name: String,
    /// The prompt: a Fluent `@id` or plain text (the name when the thing gives none).
    pub prompt: String,
    /// Seconds to hold; 0 for a press.
    pub hold_s: f32,
    /// Metres from the eye.
    pub distance: f32,
    /// While holding: `(seconds held, hold_s)`.
    pub progress: Option<(f32, f32)>,
}

/// What `viewer` would use now, for a prompt: `None` when its look meets nothing in reach.
pub fn use_view(world: &World, viewer: Entity) -> Option<UseView> {
    if !may_use(world, viewer) {
        return None;
    }
    let t = use_target(world, viewer)?;
    let held = world.get::<UseProgress>(viewer).zip(world.get::<AiId>(t.entity)).filter(|(p, id)| p.what == **id).map(|(p, _)| p.elapsed);
    Some(UseView {
        name: t.name,
        prompt: t.prompt,
        hold_s: t.hold_s,
        distance: t.distance,
        progress: held.filter(|_| t.hold_s > 0.0).map(|e| (e.min(t.hold_s), t.hold_s)),
    })
}

/// Whether `who` may use things: the dead and the downed use nothing, and a HUD shows them no prompt.
fn may_use(world: &World, who: Entity) -> bool {
    world.get::<Dead>(who).is_none() && world.get::<Downed>(who).is_none()
}

/// `ActionSystems::Use`: the `interact` presses and the holds of every body, in `AiId` order.
pub(crate) fn use_by_looking(world: &mut World, bodies: &mut QueryState<(Entity, &'static AiId), With<Intent>>) {
    let dt = world.resource::<Time>().delta_secs();
    if dt <= 0.0 {
        return;
    }
    let mut order: Vec<(AiId, Entity)> = bodies.iter(world).map(|(e, id)| (*id, e)).collect();
    order.sort_unstable();
    for (_, body) in order {
        step_body(world, body, dt);
    }
}

/// One body's tick of using: an ongoing hold runs on or ends, then a press that finds a target uses it.
fn step_body(world: &mut World, body: Entity, dt: f32) {
    let pressed = world.get::<Intent>(body).is_some_and(|i| i.pressed.iter().any(|p| p.input == INTERACT));
    let progress = world.get::<UseProgress>(body).cloned();
    if !pressed && progress.is_none() {
        return;
    }
    // The dead and the downed use nothing, and what they held is let go.
    if !may_use(world, body) {
        if progress.is_some() {
            world.entity_mut(body).remove::<UseProgress>();
        }
        return;
    }
    if let Some(progress) = progress {
        let held = world.get::<Intent>(body).is_some_and(|i| i.is_held(INTERACT));
        let target = if held { use_target(world, body) } else { None };
        let on_it = target.as_ref().is_some_and(|t| world.get::<AiId>(t.entity).is_some_and(|id| *id == progress.what));
        match target.filter(|_| on_it) {
            Some(t) => {
                // A press while holding is the same use, not a second one.
                if !revive_wins(world, body) {
                    take_press(world, body);
                }
                let elapsed = progress.elapsed + dt;
                if elapsed + EPS >= t.hold_s {
                    world.entity_mut(body).remove::<UseProgress>();
                    world.trigger(Used { who: body, what: t.entity, name: t.name });
                } else if let Some(mut p) = world.get_mut::<UseProgress>(body) {
                    p.elapsed = elapsed;
                }
                return;
            }
            // Let go, looked away or out of reach: it is over, and a press may start another.
            None => {
                world.entity_mut(body).remove::<UseProgress>();
            }
        }
    }
    if !pressed || revive_wins(world, body) {
        return;
    }
    let Some(target) = use_target(world, body) else { return };
    take_press(world, body);
    if target.hold_s > 0.0 {
        if let Some(id) = world.get::<AiId>(target.entity).copied() {
            world.entity_mut(body).insert(UseProgress { what: id, elapsed: 0.0 });
        }
    } else {
        world.trigger(Used { who: body, what: target.entity, name: target.name });
    }
}

/// Takes the oldest `interact` press out of `body`'s intent, so the machine never buffers it.
fn take_press(world: &mut World, body: Entity) {
    if let Some(mut intent) = world.get_mut::<Intent>(body) {
        intent.take_press(INTERACT);
    }
}

/// Whether the machine will start an action from `interact` this tick (a revive next to a downed ally):
/// the context binds the input. That press is the action's, not a use's.
fn revive_wins(world: &World, body: Entity) -> bool {
    world.get::<ContextInputs>(body).is_some_and(|c| c.bound(INTERACT).is_some())
}

// --- agents ----------------------------------------------------------------------------------

/// Use what you look at: press the interact button. Where you look is your `Intent` (the `look` tool
/// turns it): a thing marked usable (`observe` shows it as `USE e12 "door" 1.4m` when it is in reach,
/// and as `Interactable` anywhere) is used when you look at it from within its reach with no wall or
/// glass in between. `target=e12` turns your look to it first, then presses. A thing that takes time
/// to use (`hold` in its `USE` line) must be held: `interact hold=true`, step the time, then
/// `interact hold=false` (letting go early or looking away cancels it). What a use does is the game's;
/// `step --until journal:uses` waits for it. Where nothing is in reach, the press goes to the actions
/// your moveset binds to `interact` (a revive next to a downed ally), as it always has.
#[derive(Deserialize, JsonSchema, Default)]
pub struct InteractArgs {
    /// The thing to use (an id like `e12`): your look turns to it, then the button is pressed. Left
    /// out: use what you look at.
    pub target: Option<Id>,
    /// true presses and keeps the button held (a hold-to-use thing, or an action that goes on while
    /// held); false lets go of it (no press). Default: a plain press.
    pub hold: Option<bool>,
}

/// The `interact` tool: [`InteractArgs`].
pub(crate) fn interact(world: &mut World, caller: &Caller, args: InteractArgs) -> ToolResult {
    let InteractArgs { target, hold } = args;
    if hold == Some(false) {
        if target.is_some() {
            return Err("hold=false lets go of the button and takes no target; `interact target=eN` presses it".into());
        }
        return press_held(world, caller, INTERACT, None, "", hold);
    }
    let me = crate::tools::me(world, caller)?;
    if world.get::<Downed>(me).is_some() {
        return Err(Blocker::Downed.message());
    }
    if let Some(id) = target {
        if revive_would_win(world, me) {
            return Err("a revive is in reach and takes the button (a downed ally next to you): `interact` without a target starts it".into());
        }
        return use_named(world, me, id, hold);
    }
    match use_target(world, me) {
        Some(t) if !revive_would_win(world, me) => press_use(world, me, &t, hold),
        _ => press_for_moveset(world, caller, me, hold),
    }
}

/// Whether the next tick's context binds `interact`, as the machine will resolve it (the tools answer
/// by the rule the tick uses).
fn revive_would_win(world: &World, me: Entity) -> bool {
    world.get_resource::<shards_core::NetRole>().is_none_or(|r| r.simulates()) && resolve_context(world, me).is_some_and(|c| c.bound(INTERACT).is_some())
}

/// The old press: the actions the moveset binds to `interact`. A moveset that binds nothing there gets
/// the reason nothing was used instead of a complaint about the moveset.
fn press_for_moveset(world: &mut World, caller: &Caller, me: Entity, hold: Option<bool>) -> ToolResult {
    if binds_interact(world, me) {
        return press_held(world, caller, INTERACT, None, "", hold);
    }
    Err(format!("nothing to use: {}", why_nothing(world, me)))
}

/// Whether `me`'s moveset has an action for `interact` (an input of its own or a context binding).
fn binds_interact(world: &World, me: Entity) -> bool {
    let moveset = world.get::<Fighter>(me).and_then(|f| world.get_resource::<ContentDb>()?.get::<MovesetDef>(&f.moveset));
    understands(INTERACT, moveset, None)
}

/// Why a press finds nothing now, in words: the usable thing that is too far, or what the look meets.
fn why_nothing(world: &World, me: Entity) -> String {
    let Some(probe) = use_probe(world, me) else { return "your body has no eye to look from".into() };
    if let Some(hit) = probe.hits.first() {
        return format!(
            "{} is {:.1}m away and can be used from {:.1}m: walk closer, and look at it",
            thing(world, hit.entity),
            hit.distance,
            hit.range
        );
    }
    match probe.wall {
        Some(wall) => format!(
            "you look at {} {:.1}m ahead and nothing usable is in front of it (reach {:.1}m); usable things show as `Interactable` in observe",
            what_blocks(world, &wall),
            wall.distance,
            probe.reach
        ),
        None => format!("you look at nothing usable within {:.1}m; usable things show as `Interactable` in observe", probe.reach),
    }
}

/// `e12 "door"`.
fn thing(world: &World, entity: Entity) -> String {
    match world.get::<Usable>(entity) {
        Some(u) => format!("{} \"{}\"", label(world, entity), u.name),
        None => label(world, entity),
    }
}

/// What a `WORLD` hit is called: its id and kind when the AI sees it, else `floor`, `ceiling` or `wall`.
fn what_blocks(world: &World, hit: &shards_motion::RayHit) -> String {
    match world.get_entity(hit.entity).ok().filter(|e| e.contains::<AiVisible>()) {
        Some(_) => label(world, hit.entity),
        None => surface(hit.normal).to_owned(),
    }
}

/// `interact target=eN`: turn the look to the thing, check the look meets it in reach, press. An error
/// changes nothing: the look stays where it was.
fn use_named(world: &mut World, me: Entity, id: Id, hold: Option<bool>) -> ToolResult {
    let entity = id.entity(world)?;
    if entity == me {
        return Err("that is you; name a thing to use".into());
    }
    let Some(usable) = world.get::<Usable>(entity).cloned() else {
        return Err(format!("{} is nothing you can use; usable things show as `Interactable` in observe", label(world, entity)));
    };
    let name = format!("{} \"{}\"", label(world, entity), usable.name);
    let point = use_point(world, entity)
        .ok_or_else(|| format!("{name} has no use area, so no look can meet it: give its node a mesh or a `size` (a bare marker is only a point)"))?;
    let eye = eye_of(world, me).ok_or("your body has no eye to look from: it needs a character controller (CharacterController)")?;
    let gaze = gaze_toward(world, me, eye, point, &name)?;
    let gaze = params_of(world, me).map_or(gaze, |p| p.clamp_look(gaze));
    let gaze = Gaze { yaw: wrap(gaze.yaw), pitch: gaze.pitch };
    let before = world.get::<Intent>(me).ok_or("your body cannot be controlled (it has no Intent)")?.look;
    world.get_mut::<Intent>(me).expect("checked above").look = Some(gaze);
    let outcome = check_reach(world, me, entity, &name, eye.distance(point)).and_then(|t| press_use(world, me, &t, hold));
    if outcome.is_err() {
        world.get_mut::<Intent>(me).expect("checked above").look = before;
    }
    outcome
}

/// The target of the look `me` has now, when it is `entity`; else why not.
fn check_reach(world: &World, me: Entity, entity: Entity, name: &str, to_middle: f32) -> Result<UseTarget, String> {
    let probe = use_probe(world, me).ok_or("your body has no eye to look from")?;
    let Some(hit) = probe.hits.iter().find(|h| h.entity == entity) else {
        return Err(match &probe.wall {
            Some(wall) if wall.distance < to_middle => format!(
                "{name} is behind {} {:.1}m ahead: a wall or glass blocks a use; go round it",
                what_blocks(world, wall),
                wall.distance
            ),
            _ => format!("{name} is {to_middle:.1}m away and no look from here meets it: walk closer (move_to), then interact again"),
        });
    };
    if !hit.reached() {
        return Err(format!(
            "{name} is {:.1}m away and can be used from {:.1}m: walk closer (move_to), then interact again",
            hit.distance, hit.range
        ));
    }
    match use_target(world, me) {
        Some(t) if t.entity == entity => Ok(t),
        Some(other) if (other.distance - hit.distance).abs() <= 1e-3 => Err(format!(
            "{} is just as near on the same line ({:.1}m) and comes first: of two things at one distance the lower collider id is used",
            thing(world, other.entity),
            other.distance
        )),
        Some(other) => Err(format!("{} is nearer on the same line ({:.1}m): the nearest thing in reach is used", thing(world, other.entity), other.distance)),
        None => Err(format!("{name} is out of reach")),
    }
}

/// Presses the button at `t`, which the look now meets in reach. A thing that takes time must be held.
fn press_use(world: &mut World, me: Entity, t: &UseTarget, hold: Option<bool>) -> ToolResult {
    let name = format!("{} \"{}\"", label(world, t.entity), t.name);
    if t.hold_s > 0.0 && hold != Some(true) {
        return Err(format!(
            "{name} takes {:.1}s to use: hold the button, `interact hold=true`, step the time, then `interact hold=false`",
            t.hold_s
        ));
    }
    let mut intent = world.get_mut::<Intent>(me).ok_or("your body cannot be controlled (it has no Intent)")?;
    intent.press(INTERACT);
    if hold == Some(true) {
        intent.set_held(INTERACT, true);
    }
    Ok(if t.hold_s > 0.0 {
        format!("ok: holding {name} {:.1}m for {:.1}s (step the time, then `interact hold=false`)", t.distance, t.hold_s)
    } else {
        format!("ok: using {name} {:.1}m", t.distance)
    })
}

/// The `USE` line under `YOU` while the subject's look meets something in reach: `USE e12 "door"
/// 1.4m`, with `hold 1.0s` for a thing that takes time and `using 0.4/1.0s` while it is held.
fn use_lines(world: &World, viewer: Entity) -> Vec<String> {
    if !may_use(world, viewer) {
        return Vec::new();
    }
    let Some(t) = use_target(world, viewer) else { return Vec::new() };
    let mut line = format!("USE {} \"{}\" {:.1}m", label(world, t.entity), t.name, t.distance);
    if t.hold_s > 0.0 {
        line.push_str(&format!(" hold {:.1}s", t.hold_s));
        if let Some((held, of)) = use_view(world, viewer).and_then(|v| v.progress) {
            line.push_str(&format!(", using {held:.1}/{of:.1}s"));
        }
    }
    vec![line]
}

/// The `uses` journal line: `e5 uses e12 "door"`.
fn used_line(e: &Used, world: &World) -> Option<String> {
    Some(format!("{} uses {} \"{}\"", label(world, e.who), label(world, e.what), e.name))
}

pub(crate) fn plugin(app: &mut App) {
    use crate::ActionSystems;
    app.register_type::<Used>()
        .register_type::<UseProgress>()
        .journal_event::<Used>(used_line)
        .register_ai_observe_lines(use_lines)
        .add_systems(
            FixedUpdate,
            use_by_looking
                .in_set(ActionSystems::Use)
                .run_if(any_with_component::<Usable>.or_else(any_with_component::<UseProgress>)),
        );
    // Known to the world from the start, so `&World` queries (the `USE` line, the HUD) work before a use.
    app.world_mut().register_component::<UseProgress>();
}
