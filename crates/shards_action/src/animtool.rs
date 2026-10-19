//! The `animation` tool: what the animation bridge shows (or would show) for a body, and the
//! `anim` field of `ai/status`.
//!
//! A game gives [`Animated`] only to the bodies it dresses in a window or with `--offscreen`, so
//! headless nothing is drawn. The tool still answers there: the choice is a pure function of the
//! action machine, the controller and the content ([`select_clip_in`], [`select_layer_in`],
//! [`mount_places`]), so for a body that has no `Animated` it takes the set named by `animset=` and
//! says what the bridge would pick. Drawn, it adds what the bridge has done: whether the body is
//! bound, what plays on its layers, where its props sit. Every line says what it is; the pose is
//! never read from the bones.

use std::collections::BTreeMap;

use ai_core::tools::{register_tool, ToolKind};
use ai_core::{AiId, AiAppExt, Call, DevCommand, Id, Scope, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Value};
use shards_content::ContentDb;
use shards_core::Predicted;
use shards_motion::{MotionState, MoveModifiers};

use crate::anim::{
    mode_name, mount_places, select_clip_in, select_layer_in, AnimSet, Animated, BodyStatus, MountView, Place, PropSwaps, LEFT_KEY,
    MODE_KEY,
};
use crate::def::{ActionDef, MovesetDef};
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
#[derive(Deserialize, JsonSchema, Default)]
pub struct AnimationArgs {
    /// The body (an entity id). Default: every animated body.
    pub id: Option<Id>,
    /// The animset to preview on a body that has no `Animated`, e.g. `humanoid`.
    pub animset: Option<String>,
    /// Also the places of the bones the set names (sockets), in the world.
    #[serde(default)]
    pub bones: bool,
}

fn animation(In(call): In<Call<AnimationArgs>>, world: &mut World) -> ToolResult {
    let args = call.args;
    let db = world.get_resource::<ContentDb>().ok_or("this game has no content")?;
    if let Some(named) = &args.animset
        && db.get::<AnimSet>(named).is_none()
    {
        return Err(format!("animset `{named}` is not in content; `content_list kind=animset` lists them"));
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
    let lines: Result<Vec<String>, String> = bodies.into_iter().map(|e| describe(world, e, args.animset.as_deref(), args.bones)).collect();
    Ok(lines?.join("\n"))
}

fn label(world: &World, e: Entity) -> String {
    world.get::<AiId>(e).map_or_else(|| format!("{e}"), |id| id.to_string())
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
    let action = run.zip(key).map(|(r, k)| (k, r.t.min(r.length), r.length));
    let moveset = world.get::<Fighter>(body).map(|f| f.moveset.as_str());
    let staggers = moveset
        .and_then(|m| db.get::<MovesetDef>(m))
        .and_then(|m| m.stagger.as_deref())
        .zip(run)
        .is_some_and(|(stagger, r)| r.forced && r.action == stagger);
    let status = BodyStatus {
        dead: world.get::<Dead>(body).is_some(),
        staggered: staggers,
        pace: world.get::<MoveModifiers>(body).map_or(1.0, MoveModifiers::pace),
    };
    let standing = MotionState::default();
    let motion = world.get::<MotionState>(body).unwrap_or(&standing);

    let mut out = vec![head];
    // The body's clip.
    let choice = select_clip_in(action, motion, status, set, moveset);
    let mut line = match choice.clip {
        Some(clip) => {
            let why = match (run, choice.seek_to) {
                (Some(r), Some(p)) => format!(
                    " seeked to {:.0} % (action {} at {}, phase {})",
                    p * 100.0,
                    r.action,
                    clock_of(r),
                    r.phase
                ),
                // A mantle is seeked too, by its own progress.
                (None, Some(p)) => format!(" seeked to {:.0} % (mantle)", p * 100.0),
                // A clip that plays once and holds (a dead body with no action running).
                _ if !choice.looping => format!(" played once at x{:.2}{}", choice.speed, if status.dead { " (dead)" } else { "" }),
                _ => format!(" looping at x{:.2} ({})", choice.speed, crate::anim::locomotion_state_at(motion, &set.speeds, status.pace).name()),
            };
            format!("  body: {clip}{why}")
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
    let layer = select_layer_in(action, status, set, moveset);
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
        layers.push(text);
    }
    if !layers.is_empty() {
        out.push(format!("  layers: {}", layers.join(" · ")));
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
            drop_text = drop.map(|d| format!("drop {:.2}/{:.2} m", b.drop_depth(), d.down));
        }
        parts.push(rules_text);
        if left_rules.is_some() && !parts.iter().any(|p| p.starts_with("left ")) {
            parts.push(format!("@left:{} rules wait {} s after leaving", mode_name(mode), left_rules.map_or(0.0, |r| r.wait)));
        }
        if !layers_off.is_empty() {
            parts.push(format!("layers off: {}", layers_off.join(", ")));
        }
        parts.extend(drop_text);
        out.push(format!("  mode: {}", parts.join(" · ")));
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
            Some((rules, def?, action.map_or(r.t, |(_, t, _)| t)))
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
