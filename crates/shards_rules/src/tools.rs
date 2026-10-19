//! The rules' tools: `rules` (what the rules in play say, or what one rule set says) and `rules_set`
//! (the host changes the rule set, out of combat), and the condition `rules_changeable`. Every read of
//! content here is a `peek`: what an agent is told never counts as the simulation's read.

use std::collections::BTreeSet;

use ai_core::tools::{register_tool, ToolKind};
use ai_core::{AiReplay, Call, Scope, SimTick, ToolResult};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{json, Map, Value};
use shards_content::db::closest;
use shards_content::ContentDb;
use shards_core::{EngineState, NetRole};

use crate::def::{ClassRules, RuleSetDef, RulesDef};
use crate::guard::can_change;
use crate::read::{class_rules, peek_rules_def, rest_factor, row_factor, Column};
use crate::state::ActiveRules;
use crate::{RulesCause, RulesChanged, PLAYER_CLASS};

/// A game without `rules/default`.
pub const NO_RULES: &str =
    "this game has no rules: add `content/**/*.rules.ron` with the definition `default` and a `*.rule_set.ron` for each mode it lists";
/// `rules_set` outside a world.
pub const IN_A_WORLD: &str = "the rules belong to a world: start or continue a game first (`new_game`, `continue_game`)";
/// `rules_set` while a region loads (the world is there, between two regions).
pub const REGION_LOADING: &str = "a region is loading: the rules change in the game; call it again once `ai/status` says `engine.state` `InGame`";
/// `rules_set` on a co-op client that has no forwarder to refuse it first (the route says the same, with the host).
pub const ON_THE_HOST_ONLY: &str = "`rules_set` runs on the host only: the host chooses the rules: call it on the host";
/// `rules_set force=true` from a player's session.
pub const FORCE_IS_DEV: &str =
    "`force` skips the out-of-combat check and is for dev sessions (`session --scope dev`): a player changes the rules out of combat";

pub(crate) fn register(world: &mut World) {
    register_tool(world, "rules", Scope::Player, ToolKind::Query, rules);
    register_tool(world, "rules_set", Scope::Player, ToolKind::Action, rules_set);
}

/// The rules: the rule set in play (its name and summary, the factors it gives each class, with the session
/// table's at the session size), the modes in order, the session size, the generation, whether a change is
/// allowed now and who decides. `mode` describes that rule set instead, without session factors; `playing`
/// always names the one in play.
#[derive(Deserialize, JsonSchema, Default)]
pub struct RulesArgs {
    /// A rule set to describe (`gentle`); default: the one in play.
    pub mode: Option<String>,
}

fn rules(In(call): In<Call<RulesArgs>>, world: &World) -> ToolResult<Value> {
    describe(world, call.args.mode.as_deref())
}

/// Changes the rule set the world plays, from the next tick: `mode=harsh`, or `cycle=1` (the next in the rules'
/// `modes`, wrapping around) or `cycle=-1` (the one before). Only out of combat (`step --until
/// named:rules_changeable` waits for it); `force=true` skips that check, from a dev session. In co-op the host
/// decides: a client is refused.
#[derive(Deserialize, JsonSchema, Default)]
pub struct RulesSetArgs {
    /// The rule set to play (`harsh`).
    pub mode: Option<String>,
    /// 1: the next rule set in `modes`; -1: the one before.
    pub cycle: Option<i32>,
    /// Skip the out-of-combat check (dev sessions only).
    #[serde(default)]
    pub force: bool,
}

fn rules_set(In(call): In<Call<RulesSetArgs>>, world: &mut World) -> ToolResult {
    let changed = change(world, &call.args, call.caller.scope);
    // A refusal changed nothing: the recording forgets it.
    if changed.is_err() && world.contains_resource::<AiReplay>() {
        ai_core::replay::forget_call(world);
    }
    changed
}

/// `unknown rule set `gentel` (did you mean `gentle`?); this game's rule sets: gentle, plain, harsh`.
fn unknown(mode: &str, modes: &[String]) -> String {
    let hint = closest(mode, modes.iter().map(String::as_str)).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
    format!("unknown rule set `{mode}`{hint}; this game's rule sets: {}", modes.join(", "))
}

/// The rule set `mode` names, if it is one of `def`'s modes.
fn known<'a>(db: &'a ContentDb, def: &RulesDef, mode: &str) -> Option<&'a RuleSetDef> {
    def.modes.iter().any(|m| m == mode).then(|| db.peek::<RuleSetDef>(mode)).flatten()
}

/// The mode `step` places from `current` in `modes`, wrapping around; from a mode not in them, the first (or
/// the last, going back).
fn cycle(modes: &[String], current: &str, step: i32) -> Option<String> {
    let n = modes.len() as i32;
    if n == 0 {
        return None;
    }
    let at = match modes.iter().position(|m| m == current) {
        Some(i) => (i as i32 + step).rem_euclid(n),
        None if step > 0 => 0,
        None => n - 1,
    };
    Some(modes[at as usize].clone())
}

/// A mode as answers write it: `none` for no rule set.
pub(crate) fn shown(mode: &str) -> &str {
    if mode.is_empty() { "none" } else { mode }
}

fn state(world: &World) -> Option<EngineState> {
    world.get_resource::<State<EngineState>>().map(|s| *s.get())
}

/// What `rules_set` does, for a caller with `scope`: the answer, or why it refused (changing nothing).
pub fn change(world: &mut World, args: &RulesSetArgs, scope: Scope) -> Result<String, String> {
    // The route refuses a client first (`NetRoute::HostOnly`) when it has a forwarder; this holds without one.
    if !crate::decides(world) {
        return Err(ON_THE_HOST_ONLY.to_owned());
    }
    match state(world) {
        Some(EngineState::InGame) => {}
        Some(EngineState::RegionLoading) => return Err(REGION_LOADING.to_owned()),
        _ => return Err(IN_A_WORLD.to_owned()),
    }
    let Some(db) = world.get_resource::<ContentDb>() else { return Err(NO_RULES.to_owned()) };
    let Some(def) = peek_rules_def(db) else { return Err(NO_RULES.to_owned()) };
    let current = world.get_resource::<ActiveRules>().map(|r| r.mode.clone()).unwrap_or_default();
    let target = match (&args.mode, args.cycle) {
        (Some(mode), None) => mode.clone(),
        (None, Some(step)) if step == 1 || step == -1 => cycle(&def.modes, &current, step).ok_or_else(|| unknown("", &def.modes))?,
        (None, Some(step)) => return Err(format!("`cycle` is 1 (the next rule set in `modes`) or -1 (the one before), not {step}")),
        _ => {
            return Err(format!(
                "say `mode` (one of {}) or `cycle` (1: the next in `modes`, -1: the one before), not both and not neither",
                def.modes.join(", ")
            ))
        }
    };
    let Some(set) = known(db, def, &target) else { return Err(unknown(&target, &def.modes)) };
    if args.force && !scope.allows(Scope::Dev) {
        return Err(FORCE_IS_DEV.to_owned());
    }
    if !args.force {
        can_change(world).map_err(|why| {
            format!("the rules change only out of combat: {why}. Wait with `step --until named:rules_changeable`, or pass `force: true` from a dev session")
        })?;
    }
    if target == current {
        return Ok(format!("rules: `{target}` already; nothing changed"));
    }
    let what = summary(set);
    let tick = world.get_resource::<SimTick>().map_or(0, |t| t.0);
    match world.get_resource_mut::<ActiveRules>() {
        Some(mut rules) => {
            rules.mode = target.clone();
            rules.generation = rules.generation.wrapping_add(1);
        }
        None => world.insert_resource(ActiveRules { mode: target.clone(), generation: 1, players: 0 }),
    }
    let answer = format!("rules: {} → {target} at tick {tick}: bodies are brought to it at the start of the next tick; {what}", shown(&current));
    world.trigger(RulesChanged { from: current, to: target, cause: RulesCause::Call });
    Ok(answer)
}

/// What `set` changes, in a few words: `player: incoming damage ×1.5, charge flask 2; brute: health ×1.5;
/// attack tokens +1 (at least 1); attack rest ×0.5`.
pub fn summary(set: &RuleSetDef) -> String {
    let mut parts = Vec::new();
    let classes = set.classes.get_key_value(PLAYER_CLASS).into_iter().chain(set.classes.iter().filter(|(c, _)| *c != PLAYER_CLASS));
    for (class, rules) in classes {
        let said = class_summary(rules);
        if !said.is_empty() {
            parts.push(format!("{class}: {}", said.join(", ")));
        }
    }
    let tokens = set.attack_tokens;
    if tokens.shift != 0 {
        parts.push(format!("attack tokens {:+} (at least {})", tokens.shift, tokens.min));
    }
    if set.attack_rest != 1.0 {
        parts.push(format!("attack rest ×{}", set.attack_rest));
    }
    if parts.is_empty() { "the engine's numbers".to_owned() } else { parts.join("; ") }
}

fn class_summary(rules: &ClassRules) -> Vec<String> {
    let mut said = Vec::new();
    for (what, factor) in [
        ("incoming damage", rules.incoming_damage),
        ("outgoing damage", rules.outgoing_damage),
        ("health", rules.max_health),
        ("poise", rules.max_poise),
    ] {
        if factor != 1.0 {
            said.push(format!("{what} ×{factor}"));
        }
    }
    for rule in &rules.stamina_cost {
        said.push(format!("stamina ×{} for {}", rule.factor, rule.tags.join(", ")));
    }
    if rules.perfect_bonus > 0.0 {
        said.push(format!("perfect window +{} s", rules.perfect_bonus));
    }
    for (charge, max) in &rules.charges {
        said.push(format!("charge {charge} {max}"));
    }
    said
}

/// A factor as JSON, the way it prints (`0.05`, not the `f64` an `f32` widens to).
fn num(x: f32) -> Value {
    format!("{x}").parse::<f64>().ok().and_then(serde_json::Number::from_f64).map_or(Value::Null, Value::Number)
}

/// What `set` gives `class`, as `rules` answers it: only what differs from the engine's numbers. With
/// `session` (the size in play), the factors the session table also scales show the mode's, the session's
/// and their product.
fn class_json(set: &RuleSetDef, def: &RulesDef, class: &str, session: Option<u32>) -> Value {
    let rules = class_rules(set, class);
    let mut out = Map::new();
    if rules.incoming_damage != 1.0 {
        out.insert("incoming_damage".into(), num(rules.incoming_damage));
    }
    for (column, factor) in [(Column::OutgoingDamage, rules.outgoing_damage), (Column::MaxHealth, rules.max_health), (Column::MaxPoise, rules.max_poise)] {
        let row = column.of(def, class);
        match session.filter(|_| !row.is_empty()) {
            Some(players) => {
                let s = row_factor(row, players);
                out.insert(column.name().into(), json!({"mode": num(factor), "session": num(s), "total": num(factor * s)}));
            }
            None if factor != 1.0 => {
                out.insert(column.name().into(), num(factor));
            }
            None => {}
        }
    }
    if !rules.stamina_cost.is_empty() {
        let costs: Vec<Value> = rules.stamina_cost.iter().map(|r| json!({"tags": r.tags, "factor": num(r.factor)})).collect();
        out.insert("stamina_cost".into(), Value::Array(costs));
    }
    if rules.perfect_bonus != 0.0 {
        out.insert("perfect_bonus".into(), num(rules.perfect_bonus));
    }
    if !rules.charges.is_empty() {
        out.insert("charges".into(), json!(rules.charges));
    }
    Value::Object(out)
}

/// Whether a change is allowed now, as `rules` says it.
fn change_now(world: &World) -> String {
    if world.get_resource::<NetRole>() == Some(&NetRole::Client) {
        return "the host chooses".to_owned();
    }
    match state(world) {
        Some(EngineState::InGame) => {}
        Some(EngineState::RegionLoading) => return "refused now: a region is loading (it changes once the game is back)".to_owned(),
        _ => return "refused now: the rules belong to a world (start or continue a game first)".to_owned(),
    }
    match can_change(world) {
        Ok(()) => "allowed now".to_owned(),
        Err(why) => format!("refused now: {why}"),
    }
}

/// What `rules` answers: the rules in play, or (with `mode`) that rule set without session factors.
pub fn describe(world: &World, mode: Option<&str>) -> Result<Value, String> {
    let Some(db) = world.get_resource::<ContentDb>() else { return Err(NO_RULES.to_owned()) };
    let Some(def) = peek_rules_def(db) else { return Err(NO_RULES.to_owned()) };
    let active = world.get_resource::<ActiveRules>();
    let players = active.map_or(0, |r| r.players);
    let (shown_mode, set, session) = match mode {
        Some(mode) => {
            let set = known(db, def, mode).ok_or_else(|| unknown(mode, &def.modes))?;
            (Some(mode.to_owned()), Some(set), None)
        }
        None => {
            let playing = active.filter(|r| !r.is_neutral());
            (playing.map(|r| r.mode.clone()), playing.and_then(|r| known(db, def, &r.mode)), Some(players))
        }
    };
    let mut classes = Map::new();
    if let Some(set) = set {
        let mut names: BTreeSet<&str> = set.classes.keys().map(String::as_str).collect();
        if session.is_some() {
            names.extend(def.session.keys().map(String::as_str));
        }
        for class in names {
            classes.insert(class.to_owned(), class_json(set, def, class, session));
        }
    }
    let tokens = set.map(|s| s.attack_tokens).unwrap_or_default();
    let tokens = if tokens.shift == 0 { json!({"shift": 0}) } else { json!({"shift": tokens.shift, "min": tokens.min}) };
    let role = world.get_resource::<NetRole>().copied().unwrap_or_default();
    Ok(json!({
        "mode": shown_mode,
        // What the world plays, whatever `mode` asked about (`session` and `generation` are its).
        "playing": active.filter(|r| !r.is_neutral()).map(|r| r.mode.clone()),
        "name": set.map(|s| shards_locale::resolve(world, &s.name)),
        "summary": set.filter(|s| !s.summary.is_empty()).map(|s| shards_locale::resolve(world, &s.summary)),
        "modes": def.modes,
        "start": def.start,
        "session": players,
        "generation": active.map_or(0, |r| r.generation),
        "classes": classes,
        "attack_tokens": tokens,
        "attack_rest": num(rest_factor(set)),
        "change": change_now(world),
        // By role (the core knows no peer count): a co-op host, peers or none yet, and its clients say `host`.
        "decides": if role == NetRole::Single { "this machine" } else { "host" },
    }))
}

/// The condition `rules_changeable`: rules are in play and no guard holds.
pub fn changeable(world: &World) -> bool {
    world.get_resource::<ActiveRules>().is_some_and(|r| !r.is_neutral()) && can_change(world).is_ok()
}

#[cfg(test)]
mod tests {
    use shards_content::db::Source;
    use shards_content::ContentKinds;

    use super::*;
    use crate::guard::{RulesGuard, RulesGuards};

    const RULES: &str = r#"{"default": (modes: ["gentle", "plain", "harsh"], start: "plain", classes: ["brute"],
        session: {"brute": (max_health: [1.0, 1.5])})}"#;
    const SETS: &str = r#"{
        "gentle": (name: "Gentle", summary: "Half the damage", classes: {
            "player": (incoming_damage: 0.5, perfect_bonus: 0.05, stamina_cost: [(tags: ["roll"], factor: 0.5)], charges: {"flask": 3}),
            "brute": (max_health: 0.5),
        }),
        "plain": (name: "Plain", classes: {"player": (charges: {"flask": 3})}),
        "harsh": (name: "Harsh", classes: {"player": (incoming_damage: 1.5, charges: {"flask": 2}), "brute": (max_health: 1.5)},
            attack_tokens: (shift: 1), attack_rest: 0.5),
    }"#;

    fn world() -> World {
        let mut kinds = ContentKinds::default();
        kinds.register::<RuleSetDef>();
        kinds.register::<RulesDef>();
        let sources = [Source { path: "content/r.rules.ron".into(), text: RULES.into() }, Source { path: "content/s.rule_set.ron".into(), text: SETS.into() }];
        let db = ContentDb::build(&kinds, &sources);
        assert_eq!(db.errors().count(), 0);
        let mut world = World::new();
        world.insert_resource(db);
        world.insert_resource(State::new(EngineState::InGame));
        world.insert_resource(ActiveRules { mode: "plain".into(), generation: 1, players: 2 });
        world
    }

    fn set(mode: Option<&str>, cycle: Option<i32>, force: bool) -> RulesSetArgs {
        RulesSetArgs { mode: mode.map(str::to_owned), cycle, force }
    }

    fn fighting(_: &World) -> Option<String> {
        Some("e6 `brute` is fighting (`chase`, target e5)".to_owned())
    }

    #[test]
    fn rules_set_refuses_in_this_order_with_these_words() {
        let mut world = world();
        let refused = |world: &mut World, args: RulesSetArgs, scope| change(world, &args, scope).unwrap_err();
        let modes = "gentle, plain, harsh";
        assert_eq!(
            refused(&mut world, set(Some("harsh"), Some(1), false), Scope::Player),
            format!("say `mode` (one of {modes}) or `cycle` (1: the next in `modes`, -1: the one before), not both and not neither")
        );
        assert_eq!(
            refused(&mut world, set(None, None, false), Scope::Player),
            format!("say `mode` (one of {modes}) or `cycle` (1: the next in `modes`, -1: the one before), not both and not neither")
        );
        assert_eq!(refused(&mut world, set(None, Some(2), false), Scope::Player), "`cycle` is 1 (the next rule set in `modes`) or -1 (the one before), not 2");
        assert_eq!(refused(&mut world, set(None, Some(0), false), Scope::Player), "`cycle` is 1 (the next rule set in `modes`) or -1 (the one before), not 0");
        assert_eq!(
            refused(&mut world, set(Some("gentel"), None, false), Scope::Player),
            format!("unknown rule set `gentel` (did you mean `gentle`?); this game's rule sets: {modes}")
        );
        assert_eq!(refused(&mut world, set(Some("harsh"), None, true), Scope::Player), FORCE_IS_DEV);
        let guards: Vec<(&'static str, RulesGuard)> = vec![("npc_engaged", fighting)];
        world.insert_resource(RulesGuards(guards));
        assert_eq!(
            refused(&mut world, set(Some("harsh"), None, false), Scope::Player),
            "the rules change only out of combat: e6 `brute` is fighting (`chase`, target e5) [npc_engaged]. Wait with \
             `step --until named:rules_changeable`, or pass `force: true` from a dev session"
        );
        assert!(!changeable(&world), "a guard holds");
        assert_eq!(world.resource::<ActiveRules>().mode, "plain", "nothing changed");
        assert!(change(&mut world, &set(Some("harsh"), None, true), Scope::Dev).is_ok(), "a dev session forces it");
        assert_eq!(world.resource::<ActiveRules>().mode, "harsh");

        let mut world = self::world();
        world.remove_resource::<ContentDb>();
        assert_eq!(refused(&mut world, set(Some("harsh"), None, false), Scope::Player), NO_RULES);
        world.insert_resource(State::new(EngineState::RegionLoading));
        assert_eq!(refused(&mut world, set(Some("harsh"), None, false), Scope::Player), REGION_LOADING, "the world is there: wait for it");
        world.insert_resource(State::new(EngineState::MainMenu));
        assert_eq!(refused(&mut world, set(Some("harsh"), None, false), Scope::Player), IN_A_WORLD, "the game state comes first");
        // A client without a forwarder (the route refuses it before the tool runs when there is one).
        world.insert_resource(NetRole::Client);
        assert_eq!(refused(&mut world, set(Some("harsh"), None, true), Scope::Dev), ON_THE_HOST_ONLY, "before everything else");
        assert_eq!(ON_THE_HOST_ONLY, format!("`rules_set` runs on the host only: {}", crate::HOST_DECIDES));
        let mut world = self::world();
        world.insert_resource(NetRole::Client);
        assert_eq!(refused(&mut world, set(Some("harsh"), None, true), Scope::Dev), ON_THE_HOST_ONLY);
        assert_eq!(world.resource::<ActiveRules>().mode, "plain", "a client never writes the host's rules");
        assert_eq!(
            REGION_LOADING,
            "a region is loading: the rules change in the game; call it again once `ai/status` says `engine.state` `InGame`"
        );
        assert_eq!(
            IN_A_WORLD,
            "the rules belong to a world: start or continue a game first (`new_game`, `continue_game`)"
        );
        assert_eq!(
            NO_RULES,
            "this game has no rules: add `content/**/*.rules.ron` with the definition `default` and a `*.rule_set.ron` for each mode it lists"
        );
        assert_eq!(
            FORCE_IS_DEV,
            "`force` skips the out-of-combat check and is for dev sessions (`session --scope dev`): a player changes the rules out of combat"
        );
    }

    #[test]
    fn rules_set_changes_the_mode_and_its_generation_once() {
        let mut world = world();
        world.insert_resource(SimTick(1834));
        let answer = change(&mut world, &set(Some("harsh"), None, false), Scope::Player).unwrap();
        assert_eq!(
            answer,
            "rules: plain → harsh at tick 1834: bodies are brought to it at the start of the next tick; player: incoming damage ×1.5, \
             charge flask 2; brute: health ×1.5; attack tokens +1 (at least 1); attack rest ×0.5"
        );
        assert_eq!(*world.resource::<ActiveRules>(), ActiveRules { mode: "harsh".into(), generation: 2, players: 2 });
        assert_eq!(change(&mut world, &set(Some("harsh"), None, false), Scope::Player).unwrap(), "rules: `harsh` already; nothing changed");
        assert_eq!(world.resource::<ActiveRules>().generation, 2, "the same mode changes nothing");
        // `cycle` wraps both ways.
        assert!(change(&mut world, &set(None, Some(1), false), Scope::Player).unwrap().starts_with("rules: harsh → gentle"));
        assert!(change(&mut world, &set(None, Some(-1), false), Scope::Player).unwrap().starts_with("rules: gentle → harsh"));
        assert!(change(&mut world, &set(None, Some(-1), false), Scope::Player).unwrap().starts_with("rules: harsh → plain"));
        assert_eq!(world.resource::<ActiveRules>().generation, 5);
        assert!(changeable(&world));
        world.resource_mut::<ActiveRules>().mode.clear();
        assert!(!changeable(&world), "no rules in play");
    }

    #[test]
    fn rules_describes_the_set_in_play_with_its_session_factors() {
        let mut world = world();
        let rules = describe(&world, None).unwrap();
        assert_eq!(rules["mode"], "plain");
        assert_eq!(rules["classes"], json!({"brute": {"max_health": {"mode": 1.0, "session": 1.5, "total": 1.5}}, "player": {"charges": {"flask": 3}}}));
        assert_eq!((&rules["change"], &rules["decides"], &rules["session"]), (&json!("allowed now"), &json!("this machine"), &json!(2)));
        world.resource_mut::<ActiveRules>().mode = "gentle".into();
        let gentle = describe(&world, None).unwrap();
        assert_eq!(
            gentle["classes"],
            json!({
                "brute": {"max_health": {"mode": 0.5, "session": 1.5, "total": 0.75}},
                "player": {"incoming_damage": 0.5, "perfect_bonus": 0.05, "stamina_cost": [{"tags": ["roll"], "factor": 0.5}], "charges": {"flask": 3}},
            })
        );
        assert_eq!((&gentle["name"], &gentle["summary"], &gentle["attack_rest"]), (&json!("Gentle"), &json!("Half the damage"), &json!(1.0)));
        assert_eq!(gentle["playing"], "gentle");
        let harsh = describe(&world, Some("harsh")).unwrap();
        assert_eq!((&harsh["mode"], &harsh["playing"]), (&json!("harsh"), &json!("gentle")), "`playing` says what the world plays");
        assert_eq!(harsh["classes"]["brute"], json!({"max_health": 1.5}), "a named set: no session factors");
        assert_eq!((&harsh["attack_tokens"], &harsh["attack_rest"]), (&json!({"shift": 1, "min": 1}), &json!(0.5)));
        assert_eq!(describe(&world, Some("gentel")).unwrap_err(), "unknown rule set `gentel` (did you mean `gentle`?); this game's rule sets: gentle, plain, harsh");
        world.insert_resource(NetRole::Host);
        assert_eq!(describe(&world, None).unwrap()["decides"], "host", "by role: a host with no peer yet says `host` too");
        world.insert_resource(State::new(EngineState::RegionLoading));
        assert_eq!(describe(&world, None).unwrap()["change"], "refused now: a region is loading (it changes once the game is back)");
        world.insert_resource(State::new(EngineState::InGame));
        world.insert_resource(NetRole::Client);
        let client = describe(&world, None).unwrap();
        assert_eq!((&client["change"], &client["decides"]), (&json!("the host chooses"), &json!("host")));
        world.resource_mut::<ActiveRules>().mode.clear();
        assert_eq!(describe(&world, None).unwrap()["playing"], Value::Null, "neutral: nothing in play");
        world.remove_resource::<ContentDb>();
        assert_eq!(describe(&world, None).unwrap_err(), NO_RULES);
    }
}
