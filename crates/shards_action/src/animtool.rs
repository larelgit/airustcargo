//! The `animation` tool: what the animation bridge shows (or would show) for a body, and the
//! `anim` field of `ai/status`.
//!
//! A game gives [`Animated`] only to the bodies it dresses in a window or with `--offscreen`, so
//! headless nothing is drawn. The tool still answers there: the choice is a pure function of the
//! action machine, the controller and the content ([`select_clip_timed`], [`select_layer_timed`],
//! [`mount_places`]), so for a body that has no `Animated` it takes the set named by `animset=` and
//! says what the bridge would pick. Drawn, it adds what the bridge has done: whether the body is
//! bound, what plays on its layers (an event overlay too), where its props sit. Every line says what
//! it is; the pose is never read from the bones. `event=` starts an event overlay on a drawn body
//! (the picture only), to see one without landing a hit.

use std::collections::BTreeMap;

use ai_core::tools::{register_tool, ToolKind};
use ai_core::{AiId, AiAppExt, Call, DevCommand, Id, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use shards_content::ContentDb;
use shards_core::Predicted;
use shards_motion::{LocomotionInput, MotionState, MoveModifiers};

use crate::anim::{
    mode_name, mount_places, phase_seek, select_clip_timed, select_layer_timed, ActionTime, AnimSet, Animated, BodyStatus, Heading,
    MountView, OverlayDef, Place, PropSwaps, LEFT_KEY, MODE_KEY, OVERLAY_EVENTS,
};
use crate::def::{ActionDef, Face, MovesetDef};
use crate::fighter::{Dead, Fighter};
use crate::machine::{ActionState, Running};
use crate::predict::PredictedAction;

pub(crate) fn register(app: &mut App) {
    register_tool(app.world_mut(), "animation", Scope::Dev, ToolKind::Query, animation);
    app.add_ai_status("anim", status).add_dev_entry(
        "world",
        "animation",
        "Animation: what the bridge shows",
        DevCommand::Run("animation".into()),
    );
}

/// What the bridge shows for a body, from the action machine, the controller and the content. A
/// body with `Animated` shows its set; for any other pass `animset=` (nothing is drawn headless,
/// so this says what the bridge would choose). Without `id`, every animated body, one line each.
/// `event=` plays an event overlay on a drawn body, as if the engine event had named it.
#[derive(Deserialize, JsonSchema, Default)]
pub struct AnimationArgs {
    /// The body (an entity id). Default: every animated body.
    pub id: Option<Id>,
    /// The animset to preview on a body that has no `Animated`, e.g. `humanoid`.
    pub animset: Option<String>,
    /// Also the places of the bones the set names (sockets), in the world.
    #[serde(default)]
    pub bones: bool,
    /// Plays the overlay the body's set gives this engine event (`damaged`, `blocked`, `landed`,
    /// ...) on the drawn body from the next frame, as if the event had named it: the picture only,
    /// nothing in the simulation. Needs `id`.
    pub event: Option<String>,
}

fn animation(In(call): In<Call<AnimationArgs>>, world: &mut World) -> ToolResult {
    let args = call.args;
    let db = world.get_resource::<ContentDb>().ok_or("this game has no content")?;
    if let Some(named) = &args.animset
        && db.get::<AnimSet>(named).is_none()
    {
        return Err(format!("animset `{named}` is not in content; `content_list kind=animset` lists them"));
    }
    if args.event.is_some() && args.id.is_none() {
        return Err("event=<name> plays an overlay on one body: name it with id=<body>".into());
    }
    let bodies: Vec<Entity> = match args.id {
        Some(id) => vec![id.entity(world)?],
        None => {
            if args.animset.is_some() {
                return Err("animset=<id> previews a body: name it with id=<body>".into());
            }
            let mut q = world.query_filtered::<(Entity, &AiId), With<Animated>>();
            let mut found: Vec<(AiId, Entity)> = q.iter(world).map(|(e, id)| (*id, e)).collect();
            found.sort_by_key(|(id, _)| id.0);
            if found.is_empty() {
                return Ok("no animated body: a game gives `Animated` to the characters it dresses in a window or with \
                           --offscreen; pass id=<body> animset=<id> to see what a set would choose"
                    .into());
            }
            found.into_iter().map(|(_, e)| e).collect()
        }
    };
    let mut out = Vec::new();
    if let Some(event) = &args.event {
        // `id` is given (checked above): the one body.
        out.push(preview_event(world, bodies[0], args.animset.as_deref(), event)?);
    }
    for body in bodies {
        out.push(describe(world, body, args.animset.as_deref(), args.bones)?);
    }
    Ok(out.join("\n"))
}

/// `hit on upper, in 0.05 s, out 0.20-0.35 s, 70 %`: an overlay as content gives it.
fn overlay_text(def: &OverlayDef) -> String {
    let fade_in = if def.fade_in > 0.0 { format!(", in {:.2} s", def.fade_in) } else { String::new() };
    let (from, to) = def.fade_out;
    format!("{} on {}{fade_in}, out {from:.2}-{to:.2} s, {:.0} %", def.clip, def.layer, def.strength * 100.0)
}

/// `event=`: the overlay the body's set gives `event`, started on the drawn body (the picture only).
fn preview_event(world: &mut World, body: Entity, named: Option<&str>, event: &str) -> Result<String, String> {
    let events: Vec<&str> = OVERLAY_EVENTS.iter().map(|(e, _)| *e).collect();
    if !events.contains(&event) {
        let hint =
            shards_content::db::closest(event, events.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
        return Err(format!("event `{event}` is no engine event an overlay answers{hint}; events: {}", events.join(", ")));
    }
    let who = label(world, body);
    let animset = world
        .get::<Animated>(body)
        .map(|a| a.animset.clone())
        .or_else(|| named.map(str::to_owned))
        .ok_or_else(|| format!("{who} is not animated; pass animset=<id> to see what a set's overlay would be"))?;
    let db = world.resource::<ContentDb>();
    let set = db.get::<AnimSet>(&animset).ok_or_else(|| format!("animset `{animset}` is not in content; `content_list kind=animset` lists them"))?;
    let Some(def) = set.overlays.get(event) else {
        let have: Vec<&str> = set.overlays.keys().map(String::as_str).collect();
        let have = if have.is_empty() { "none".to_owned() } else { have.join(", ") };
        return Err(format!("animset `{animset}` has no overlay for `{event}`; its overlays: {have}"));
    };
    let what = overlay_text(def);
    // The bridge starts no overlay on a dead body.
    if world.get::<Dead>(body).is_some() {
        return Ok(format!("overlay {event} of {animset}: {what}; {who} is dead, so nothing plays"));
    }
    #[cfg(feature = "render")]
    {
        let clock = world.get_resource::<Time<Fixed>>().map_or(0.0, |t| t.elapsed_secs_f64() + t.overstep().as_secs_f64());
        if let Some(mut bound) = world.get_mut::<crate::render::AnimationBound>(body) {
            bound.note_event(event, clock);
            return Ok(format!("overlay {event} starts on {who} from the next frame: {what} (the picture only)"));
        }
    }
    Ok(format!("overlay {event} of {animset}: {what}; {who} is not drawn (a window or --offscreen draws it), so nothing plays"))
}

fn label(world: &World, e: Entity) -> String {
    world.get::<AiId>(e).map_or_else(|| format!("{e}"), |id| id.to_string())
}

/// How a seeked clip follows the action: ` by phase` for a clip timed to the action's phases (and the
/// phases it loops), ` evenly (the clip is timed to windup, active, recovery; the action is not)` for
/// one timed to others, nothing for a clip without phases.
fn seek_how(set: &AnimSet, clip: &str, action: Option<&ActionTime>) -> String {
    let (Some(def), Some(a)) = (set.clip(clip), action) else { return String::new() };
    if def.phases.is_empty() {
        return String::new();
    }
    if phase_seek(a.phases, &def.phases, &def.loops, a.t, a.length).is_some() {
        return if def.loops.is_empty() { " by phase".to_owned() } else { format!(" by phase, looping {}", def.loops.join(", ")) };
    }
    let names: Vec<&str> = def.phases.iter().map(|p| p.0.as_str()).collect();
    format!(" evenly (the clip is timed to {}; the action is not)", names.join(", "))
}

/// The model's turn now, for a drawn body: radians from the body's facing.
#[cfg(feature = "render")]
fn bound_turn(world: &World, body: Entity) -> Option<f32> {
    world.get::<crate::render::AnimationBound>(body).map(crate::render::AnimationBound::model_turn)
}

#[cfg(not(feature = "render"))]
fn bound_turn(_: &World, _: Entity) -> Option<f32> {
    None
}

/// ` · the model turned 90° to the run's heading (turn 0.08 s)` while a run turns the model (or one
/// that would), and while a drawn model is still turned; nothing otherwise.
fn turn_text(set: &AnimSet, def: Option<&ActionDef>, key: Option<&str>, now: Option<f32>) -> String {
    let secs = key.map_or(0.0, |k| set.turn_of(k));
    let turns = secs > 0.0 && def.is_some_and(|d| d.face == Face::Keep);
    match now {
        Some(yaw) if yaw != 0.0 => {
            let why = if turns { format!("to the run's heading (turn {secs} s)") } else { "on its way back to the body's facing".to_owned() };
            format!(" · the model turned {:.0}° {why}", yaw.to_degrees())
        }
        _ if turns => format!(" · the model turns to the run's heading (turn {secs} s)"),
        _ => String::new(),
    }
}

/// The seconds `0.21/0.60 s` of a run.
fn clock_of(run: &Running) -> String {
    format!("{:.2}/{:.2} s", run.t, run.length)
}

fn describe(world: &World, body: Entity, named: Option<&str>, bones: bool) -> Result<String, String> {
    let who = label(world, body);
    let own = world.get::<Animated>(body).map(|a| a.animset.as_str());
    let (animset, preview) = match (own, named) {
        (Some(own), _) => (own, false),
        (None, Some(named)) => (named, true),
        (None, None) => {
            return Err(format!(
                "{who} is not animated: a game gives `Animated` to the characters it dresses in a window or with \
                 --offscreen; pass animset=<id> to see what a set would choose"
            ));
        }
    };
    let db = world.resource::<ContentDb>();
    let set = db.get::<AnimSet>(animset).ok_or_else(|| {
        format!("animset `{animset}` is not in content; `content_list kind=animset` lists them")
    })?;

    #[cfg(feature = "render")]
    let bound = world.get::<crate::render::AnimationBound>(body);
    #[cfg(feature = "render")]
    let head = if preview {
        format!("{who} · animset {animset} (named: {who} has no Animated; nothing is drawn headless) · what the bridge would show, the pose is not applied")
    } else {
        let state = if bound.is_some() { "bound" } else { "not bound (the model or the glTF is loading)" };
        let held = if bound.is_some_and(|b| b.held()) { "held (hit stop)" } else { "not held" };
        format!("{who} · animset {animset} · {state} · {held}")
    };
    #[cfg(not(feature = "render"))]
    let head = if preview {
        format!("{who} · animset {animset} (named: {who} has no Animated; nothing is drawn headless) · what the bridge would show, the pose is not applied")
    } else {
        format!("{who} · animset {animset} · built without the `render` feature: nothing is drawn · what the bridge would show")
    };

    let head = match (own, named) {
        (Some(o), Some(n)) if o != n => format!("{head} · animset={n} ignored: this body has its own `Animated`"),
        _ => head,
    };

    // The running action, as the bridge reads it.
    let predicted = world.get::<Predicted>(body).is_some();
    let run: Option<&Running> = if predicted {
        world.get::<PredictedAction>(body).map(|p| &p.run)
    } else {
        world.get::<ActionState>(body).and_then(|s| s.current.as_ref())
    };
    let def = run.and_then(|r| db.get::<ActionDef>(&r.action));
    let key = run.map(|r| set.action_key(&r.action, r.anim.as_deref(), def.map_or(&[], |d| d.tags.as_slice())));
    // Looked ahead by the fixed clock's overstep as the bridge looks (not an action with a hold loop).
    let ahead = if def.is_some_and(|d| d.hold.is_some()) {
        0.0
    } else {
        world.get_resource::<Time<Fixed>>().map_or(0.0, |t| t.overstep().as_secs_f32())
    };
    let action =
        run.zip(key).map(|(r, k)| ActionTime::new(k, (r.t + ahead).min(r.length), r.length, def.map_or(&[], |d| d.phases.as_slice())));
    let moveset = world.get::<Fighter>(body).map(|f| f.moveset.as_str());
    let staggers = moveset
        .and_then(|m| db.get::<MovesetDef>(m))
        .and_then(|m| m.stagger.as_deref())
        .zip(run)
        .is_some_and(|(stagger, r)| r.forced && r.action == stagger);
    let standing = MotionState::default();
    let motion = world.get::<MotionState>(body).unwrap_or(&standing);
    // Facing a target: the heading the bridge kept (its slack at the quarters' edges), else as it would choose now.
    #[cfg(feature = "render")]
    let kept = bound.map_or(Heading::Forward, |b| b.heading());
    #[cfg(not(feature = "render"))]
    let kept = Heading::Forward;
    let faces = world.get::<LocomotionInput>(body).is_some_and(|l| l.face.is_some());
    let facing = world.get::<Transform>(body).filter(|_| faces).map(|tf| Heading::of(*tf.forward(), motion.velocity, kept));
    let status = BodyStatus {
        dead: world.get::<Dead>(body).is_some(),
        staggered: staggers,
        pace: world.get::<MoveModifiers>(body).map_or(1.0, MoveModifiers::pace),
        facing,
    };

    let mut out = vec![head];
    // The body's clip.
    let choice = select_clip_timed(action, motion, status, set, moveset);
    let mut line = match choice.clip {
        Some(clip) => {
            let why = match (run, choice.seek_to) {
                (Some(r), Some(p)) => format!(
                    " seeked to {:.0} %{} (action {} at {}, phase {})",
                    p * 100.0,
                    seek_how(set, clip, action.as_ref()),
                    r.action,
                    clock_of(r),
                    r.phase
                ),
                // A mantle is seeked too, by its own progress.
                (None, Some(p)) => format!(" seeked to {:.0} % (mantle)", p * 100.0),
                // A clip that plays once and holds (a dead body with no action running).
                _ if !choice.looping => format!(" played once at x{:.2}{}", choice.speed, if status.dead { " (dead)" } else { "" }),
                _ => {
                    let state = crate::anim::locomotion_state_at(motion, &set.speeds, status.pace);
                    let faced = match facing {
                        Some(h) if choice.speed < 0.0 => format!(", facing a target, moving {}: the clip backward", h.name()),
                        Some(h) => format!(", facing a target, moving {}", h.name()),
                        None => String::new(),
                    };
                    format!(" looping at x{:.2} ({}{faced})", choice.speed, state.name())
                }
            };
            format!("  body: {clip}{why}{}", turn_text(set, def, key, bound_turn(world, body)))
        }
        None => "  body: nothing for this moment (the pose stays as it is)".to_owned(),
    };
    if let Some(m) = moveset {
        let family = set.by_moveset.get(m).map_or_else(String::new, |t| {
            format!(" · family {m}: {}", t.iter().map(|(a, b)| format!("{a} -> {b}")).collect::<Vec<_>>().join(", "))
        });
        line.push_str(&family);
    }
    out.push(line);

    // The layer.
    let layer = select_layer_timed(action, status, set, moveset);
    #[cfg(feature = "render")]
    let plays = |name: &str| bound.map(|b| (b.layer_clip(name).map(str::to_owned), b.layer_share(name)));
    let mut layers: Vec<String> = Vec::new();
    for name in set.layers.keys() {
        #[allow(unused_mut, reason = "the drawn side replaces it")]
        let mut text = match layer.filter(|l| l.layer == name) {
            Some(l) => format!("{name}: {} (wanted)", l.choice.clip.unwrap_or("-")),
            None => format!("{name}: none"),
        };
        #[cfg(feature = "render")]
        if let Some((Some(clip), share)) = plays(name) {
            text = format!("{name}: {clip} {:.0} %", share * 100.0);
        }
        #[cfg(feature = "render")]
        if let Some(play) = bound.and_then(|b| b.overlay_on(name)) {
            text.push_str(&format!(" · overlay {} ({}) at {:.2} s, {:.0} %", play.event, play.clip, play.age, play.share * 100.0));
        }
        layers.push(text);
    }
    if !layers.is_empty() {
        out.push(format!("  layers: {}", layers.join(" · ")));
    }
    if !set.overlays.is_empty() {
        let list: Vec<String> = set.overlays.iter().map(|(event, def)| format!("{event}: {}", overlay_text(def))).collect();
        out.push(format!("  overlays: {}", list.join(" · ")));
    }

    // The motion mode: what hangs on it (mode rules, the draw after leaving, a layer off, a drop).
    let mode = motion.mode;
    let (mode_rules, left_rules) = (set.mode_rules(mode), set.left_rules(mode));
    let layers_off: Vec<&str> = set.layers.keys().map(String::as_str).filter(|l| set.layer_off(l, mode)).collect();
    let drop = set.drops.get(mode_name(mode));
    let has_modes = set.mounts.keys().any(|k| k.starts_with(MODE_KEY) || k.starts_with(LEFT_KEY))
        || !set.drops.is_empty()
        || set.layers.values().any(|l| !l.off_in.is_empty());
    if has_modes {
        let mut parts = vec![format!("{} ({})", mode_name(mode), if preview { "as the controller says" } else { "drawn" })];
        // The preview cannot know how long ago an action ended: a running one makes the rules step aside.
        let yields = mode_rules.is_some_and(|r| r.yield_after > 0.0 && run.is_some());
        #[allow(unused_mut, reason = "the drawn side replaces it")]
        let mut rules_text = match (mode_rules, yields) {
            (Some(r), true) => format!("@mode:{} steps aside (an action runs, yield_after {} s)", mode_name(mode), r.yield_after),
            (Some(_), false) => format!("@mode:{} applies", mode_name(mode)),
            (None, _) => "no @mode rules".to_owned(),
        };
        #[allow(unused_mut, reason = "the drawn side adds the drop")]
        let mut drop_text = drop.map(|d| format!("drop {:.2} m over {:.2} s when an action runs", d.down, d.seconds));
        #[cfg(feature = "render")]
        if let Some(b) = bound {
            let clock = world.get_resource::<Time<Fixed>>().map_or(0.0, |t| t.elapsed_secs_f64() + t.overstep().as_secs_f64());
            if let Some(rules) = mode_rules {
                rules_text = if b.modes.yielding() {
                    let ago = b.modes.acted_ago(clock).map_or(String::new(), |a| format!(", an action {a:.2} s ago"));
                    format!("@mode:{} steps aside (yield_after {} s{ago})", mode_name(mode), rules.yield_after)
                } else {
                    format!("@mode:{} applies", mode_name(mode))
                };
            }
            if let Some((left, waiting)) = b.modes.left() {
                let wait = set.left_rules(left).map_or(0.0, |r| r.wait);
                parts.push(format!("left {} {} (wait {wait} s)", mode_name(left), if waiting { "waiting" } else { "drawing" }));
            }
            let down = drop.map_or_else(|| set.drops.values().map(|d| d.down).fold(0.0, f32::max), |d| d.down);
            drop_text = (drop.is_some() || b.drop_depth() > 0.0).then(|| format!("drop {:.2}/{:.2} m", b.drop_depth(), down));
        }
        parts.push(rules_text);
        if left_rules.is_some() && !parts.iter().any(|p| p.starts_with("left ")) {
            parts.push(format!("@left:{} rules wait {} s after leaving", mode_name(mode), left_rules.map_or(0.0, |r| r.wait)));
        }
        if !layers_off.is_empty() {
            parts.push(format!("layers off: {}", layers_off.join(", ")));
        }
        out.push(format!("  mode: {}", parts.join(" · ")));
        // The model's own offsets (the drop; later slices add the facing and the lunge).
        if let Some(drop) = drop_text {
            out.push(format!("  model: {drop}"));
        }
    }

    // The props.
    let swaps = world.get::<PropSwaps>(body);
    let carried: Vec<(String, String)> = set.carried(moveset).iter().map(|c| (c.prop.clone(), c.socket.clone())).collect();
    if !carried.is_empty() || !set.sockets.is_empty() {
        let list: Vec<String> = carried
            .iter()
            .map(|(prop, socket)| {
                let shown = swaps.and_then(|s| s.0.get(prop));
                format!("{prop}{} at {socket}", shown.map_or(String::new(), |v| format!(" (as {v})")))
            })
            .collect();
        out.push(format!(
            "  carry (moveset {}): {}",
            moveset.unwrap_or("none"),
            if list.is_empty() { "nothing".to_owned() } else { list.join(", ") }
        ));
    }
    // Drawn, the bridge may still show props the moveset no longer carries (an open equip).
    #[cfg(feature = "render")]
    let placed = bound.is_some_and(|b| b.places().next().is_some());
    #[cfg(not(feature = "render"))]
    let placed = false;
    if !carried.is_empty() || placed {
        let rules = run.and_then(|r| {
            let rules = set.mount_rules(&r.action, r.anim.as_deref(), def.map_or(&[], |d| d.tags.as_slice()))?;
            Some((rules, def?, action.map_or(r.t, |a| a.t)))
        });
        #[allow(unused_mut, reason = "the drawn side adds the equip")]
        let mut equip = None;
        #[cfg(feature = "render")]
        if let Some(b) = bound {
            let clock = world.get_resource::<Time<Fixed>>().map_or(0.0, |t| t.elapsed_secs_f64() + t.overstep().as_secs_f64());
            equip = b.mounts_equip(set, clock);
        }
        #[allow(unused_mut, reason = "the drawn side replaces it")]
        let mut places = mount_places(&MountView { carried: &carried, previous: &[], run: rules, equip, mode: mode_rules.filter(|r| !(r.yield_after > 0.0 && run.is_some())).map(|r| (r, 0.0)), left: None, cut: &[] });
        // Drawn: what the bridge computed (it also knows the previous moveset's props and the cut hold).
        #[cfg(feature = "render")]
        // Its placements, hidden props and props on missing sockets included.
        if let Some(b) = bound
            && b.places().next().is_some()
        {
            places = b.places().map(|(p, place)| (p.to_owned(), place.clone())).collect();
        }
        let text: Vec<String> = places
            .iter()
            .map(|(prop, place)| match place {
                Place::Socket(socket) => format!("{prop} at {socket}"),
                Place::Hidden => format!("{prop} hidden"),
            })
            .collect();
        let why = match (run, rules) {
            (Some(r), Some(_)) => format!(" (rules of {} at {})", r.action, clock_of(r)),
            _ => String::new(),
        };
        #[cfg(feature = "render")]
        let drawn = bound.map_or(String::new(), |b| {
            let n = b.props().count();
            let equip = if b.equipping() { " · equip: open" } else { " · equip: none" };
            format!(" · {n} drawn{equip}")
        });
        #[cfg(not(feature = "render"))]
        let drawn = String::new();
        out.push(format!("  props: {}{why}{drawn}", text.join(", ")));
    }
    if bones {
        let mut places: BTreeMap<&str, String> = BTreeMap::new();
        for (socket, def) in &set.sockets {
            #[cfg(feature = "render")]
            let at = bound
                .and_then(|b| b.socket_bone(socket))
                .and_then(|bone| world.get::<GlobalTransform>(bone))
                .map(|t| t.translation());
            #[cfg(not(feature = "render"))]
            let at: Option<Vec3> = None;
            let place = at.map_or_else(|| "not found in a model".to_owned(), |p| format!("at ({:.2}, {:.2}, {:.2})", p.x, p.y, p.z));
            places.insert(socket, format!("{socket} ({}) {place}", def.bone));
        }
        if !places.is_empty() {
            out.push(format!("  bones: {}", places.into_values().collect::<Vec<_>>().join(" · ")));
        }
    }
    Ok(out.join("\n"))
}

/// `ai/status` -> `anim: {animated, bound, props, plays, overlays_started, feet_solved, ms}`:
/// the bodies the bridge dresses and how much it did last frame. All zero where nothing is drawn.
fn status(world: &World) -> Value {
    #[cfg(feature = "render")]
    if let Some(stats) = world.get_resource::<crate::render::AnimStats>() {
        return json!({
            "animated": stats.animated,
            "bound": stats.bound,
            "props": stats.props,
            "plays": stats.plays,
            "overlays_started": stats.overlays_started,
            "feet_solved": stats.feet_solved,
            "ms": stats.ms,
        });
    }
    let _ = world;
    json!({"animated": 0, "bound": 0, "props": 0, "plays": 0, "overlays_started": 0, "feet_solved": 0, "ms": 0.0})
}
