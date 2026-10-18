//! Tool contracts: every registered tool, called with arguments made from its own schema, keeps
//! the promises the engine makes about tools.
//!
//! Each tool gets 30 argument sets from the fuzzer's generator (`shards::script::schema`): 20
//! that satisfy the schema and 10 that break exactly one argument on purpose. For every call:
//!
//! - no gameplay system fails and the tool does not panic;
//! - an error says something (it is not empty), and an error for a call the schema rejects names
//!   the tool's signature or the bad argument: an error says how to fix the call (docs/agents/watching-and-driving.md).
//!
//! At the end of a tool, the session's replay is played from tick 0 in a fresh game and must
//! match: that proves every successful action call was recorded.
//!
//! Which tools: queries and actions of every scope; of the control tools, only `step` (1 to 30
//! ticks): the others move time or the session itself. Tools that cannot run in a test app are
//! in [`SKIP`], each with its reason. Each tool plays on its own fresh game, so tools cannot
//! affect each other.
//!
//! The checker proves itself: the control tests plant each bug class in a tool of a test game
//! (a tool that panics, one that fails with an empty error, one whose error does not say how to
//! fix the call, one whose change is not recorded) and show the checker names it.

use std::time::Instant;

use sandbox::SandboxPlugin;
use shards::ai_core::replay;
use shards::ai_core::scenario::WorldDriver;
use shards::ai_core::{call_tool, AiJournal, JournalKind};
use shards::prelude::*;
use shards::script::fuzz::ToolDesc;
use shards::script::inputs::{ids_in, SplitMix};
use shards::script::fuzz::{learn_words, vocabulary};
use shards::script::schema::{invalid_options, schema_args, Hints};
use shards::serde_json::{json, Value};

/// Tools the contracts do not call, and why. One list: a tool is here or it is checked.
const SKIP: &[(&str, &str)] = &[];

/// Argument sets per tool: valid by schema, then invalid on purpose.
const VALID_SETS: usize = 20;
const INVALID_SETS: usize = 10;

/// What a contract run found out about one tool.
#[derive(Debug, Default)]
struct Verdict {
    calls: usize,
    /// Invalid calls the tool took without an error (the schema says no, serde said yes).
    invalid_taken: usize,
    /// Valid calls the tool took (the rest were refused: no such entity, not now, ...).
    valid_taken: usize,
    /// Broken promises, one line each.
    broken: Vec<String>,
}

/// A seed per tool: the name, so the tools do not draw each other's arguments.
fn seed_of(name: &str) -> u64 {
    name.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// Whether an error teaches: it names the tool's signature, or the argument that was broken.
fn teaches(error: &str, signature: &str, argument: Option<&str>) -> bool {
    error.contains(signature) || argument.is_some_and(|a| error.contains(a))
}

/// Plays the contracts of `tool` on games `fresh` makes.
fn check(fresh: &dyn Fn() -> App, tool: &ToolDesc) -> Verdict {
    let mut verdict = Verdict::default();
    let mut app = fresh();
    let mut rng = SplitMix(seed_of(&tool.name));
    let observed = call_tool(app.world_mut(), "dev", "observe", json!({}));
    let ids = ids_in(observed.result.as_str().unwrap_or_default());
    let mut words = vocabulary(&mut WorldDriver { world: app.world_mut() }, "dev").unwrap_or_default();
    let mut seen_journal = app.world().resource::<AiJournal>().last_seq();
    let is_step = tool.name == "step";
    let rotation = rng.below(1000) as usize;
    for n in 0..VALID_SETS + INVALID_SETS {
        let on_purpose = n >= VALID_SETS;
        let hints = Hints { ids: &ids, words: &words };
        let (mut args, broke, twin) = if on_purpose {
            // Each way to break a call in turn (from a start of the seed's choosing), so ten calls
            // meet ten of them rather than the same one ten times.
            let mut options = invalid_options(&tool.schema, &mut rng, hints);
            let bad = options.swap_remove((rotation + n - VALID_SETS) % options.len());
            (bad.args, Some(bad.argument), Some(bad.valid))
        } else {
            (schema_args(&tool.schema, &mut rng, hints), None, None)
        };
        // `step` is the one control tool, with 1 to 30 ticks (a count from the schema could run a minute).
        if is_step && let Some(object) = args.as_object_mut() {
            if object.contains_key("ticks") && !on_purpose {
                object.insert("ticks".into(), json!(1 + rng.below(30)));
            }
            if !on_purpose {
                object.remove("until");
            }
        }
        let out = call_tool(app.world_mut(), "dev", &tool.name, args.clone());
        verdict.calls += 1;
        let at = format!("{} {args}", tool.name);
        let text = out.result.as_str().map(str::to_owned).unwrap_or_else(|| out.result.to_string());
        // The tool did not panic: its error would say so, and the journal keeps it.
        let journal = app.world().resource::<AiJournal>();
        let news: Vec<String> = journal.after(seen_journal).0.filter(|e| e.kind == JournalKind::Error).map(ToString::to_string).collect();
        seen_journal = journal.last_seq();
        if let Some(line) = news.iter().find(|l| l.contains("panicked")) {
            verdict.broken.push(format!("{at}: a panic: {line}"));
        }
        if !out.ok && text.contains(&format!("{} panicked", tool.name)) {
            verdict.broken.push(format!("{at}: the tool panicked: {text}"));
        }
        // No gameplay system failed. (A violated invariant is a rule of the game, which a dev
        // tool may break on purpose: `set` a health out of range.)
        if let Some(halt) = &out.halted && !halt.reason.starts_with("invariant") {
            verdict.broken.push(format!("{at}: the game halted: {}", halt.reason));
        }
        if !out.ok {
            // What a refusal lists is what the next call may name.
            learn_words(&mut words, &text);
            if text.trim().is_empty() {
                verdict.broken.push(format!("{at}: failed with an empty error"));
            } else if let Some(argument) = &broke
                && !teaches(&text, &tool.signature, argument.as_deref())
                && !same_error_unbroken(&mut app, &tool.name, twin, &text)
            {
                verdict.broken.push(format!(
                    "{at}: the error does not name the signature `{}` or the bad argument ({}): {text}",
                    tool.signature,
                    argument.as_deref().unwrap_or("the arguments as a whole")
                ));
            }
        } else if on_purpose {
            verdict.invalid_taken += 1;
        } else {
            verdict.valid_taken += 1;
        }
    }
    // Every successful action call was recorded: the session plays back to the same game.
    let recording = replay::save(app.world());
    match shards::testing::verify(&mut fresh(), recording) {
        Ok(report) if report.identical() => {}
        // The recording ends in a rule the session broke on purpose; the playback breaks it too.
        Err(e) if e.starts_with("invariant ") && app.world().resource::<shards::ai_core::sim::Halts>().count > 0 => {}
        Ok(report) => verdict.broken.push(format!("the session's replay does not play back: {}", report.headline())),
        Err(e) => verdict.broken.push(format!("the session's replay cannot be played: {e}")),
    }
    verdict
}

/// Whether the call fails the same way without the break: the error is about the rest of the
/// arguments (random text that names nothing), not about what was broken on purpose. Judged by
/// calling the unbroken twin.
fn same_error_unbroken(app: &mut App, tool: &str, twin: Option<Value>, error: &str) -> bool {
    let Some(twin) = twin else { return false };
    let out = call_tool(app.world_mut(), "dev", tool, twin);
    !out.ok && out.result.as_str() == Some(error)
}

/// The tools the contracts cover: queries, actions, and `step` (names sorted).
fn covered(tools: &[ToolDesc]) -> Vec<&ToolDesc> {
    let mut out: Vec<&ToolDesc> = tools
        .iter()
        .filter(|t| t.kind != "control" || t.name == "step")
        .filter(|t| !SKIP.iter().any(|(name, _)| *name == t.name))
        .collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

fn sandbox_game() -> App {
    sandbox::test_app(42)
}

#[test]
fn every_tool_keeps_its_contract() {
    // Tools write their files (snapshots/, replays/) in the working folder.
    let _turn = crate::scenario_files::snapshot_file_turn();
    let started = Instant::now();
    let app = sandbox_game();
    let tools = shards::ai_core::scenario::tool_list(app.world());
    drop(app);
    assert!(tools.len() > 50, "the sandbox lists its tools: {} found", tools.len());
    for (name, _) in SKIP {
        assert!(tools.iter().any(|t| t.name == *name), "SKIP names `{name}`, which is not a tool: drop it from the list");
    }
    let mut broken = Vec::new();
    let (mut calls, mut taken) = (0, 0);
    for tool in covered(&tools) {
        let begun = Instant::now();
        let verdict = check(&sandbox_game, tool);
        eprintln!(
            "{:<22} {:>3} calls, {:>2} of {VALID_SETS} valid taken, {} invalid taken, {:?}",
            tool.name,
            verdict.calls,
            verdict.valid_taken,
            verdict.invalid_taken,
            begun.elapsed()
        );
        calls += verdict.calls;
        taken += verdict.invalid_taken;
        broken.extend(verdict.broken);
    }
    eprintln!("{calls} calls on {} tools in {:?}; {taken} invalid ones taken", covered(&tools).len(), started.elapsed());
    assert!(broken.is_empty(), "{} broken promises:\n{}", broken.len(), broken.join("\n"));
}

// --- the checker proves itself -------------------------------------------------------------

/// Arguments of the planted tools. The schema comes from the derive, like a real tool's.
#[derive(Deserialize, JsonSchema, Default)]
#[schemars(crate = "shards::schemars")]
#[serde(crate = "shards::serde")]
#[allow(dead_code, reason = "only its schema matters")]
struct Planted {
    /// A number.
    #[serde(default)]
    n: u32,
}

/// Arguments with a limit that serde does not enforce: the tool does, and says too little.
#[derive(Deserialize, JsonSchema)]
#[schemars(crate = "shards::schemars")]
#[serde(crate = "shards::serde")]
struct Limited {
    /// At least 1.
    #[schemars(range(min = 1))]
    n: i64,
}

fn panics(_: In<Call<Planted>>) -> ToolResult {
    panic!("planted: this tool has a bug")
}

fn fails_with_nothing_to_say(_: In<Call<Planted>>) -> ToolResult {
    Err(String::new())
}

fn refuses_without_saying_how_to_fix_it(In(call): In<Call<Limited>>) -> ToolResult {
    if call.args.n < 1 { Err("too small".into()) } else { Ok("fine".into()) }
}

/// Changes the player and takes the call back out of the recording, so playback never makes
/// the change.
fn changes_without_a_record(_: In<Call<Planted>>, world: &mut World) -> ToolResult {
    let mut players = world.query_filtered::<&mut Health, With<sandbox::gameplay::Player>>();
    players.single_mut(world).map_err(|e| e.to_string())?.current -= 1.0;
    shards::ai_core::replay::forget_call(world);
    Ok("changed".into())
}

fn cracks_after_tick_3(tick: Res<SimTick>) {
    if tick.0 >= 3 {
        panic!("planted: a gameplay system gives way");
    }
}

/// A sandbox with one planted tool of each kind of bug.
fn planted_game() -> App {
    shards::testing::test_app(
        |config| config.seed = 42,
        |app| {
            app.add_plugins(SandboxPlugin);
            app.add_ai_tool("plant_panic", Scope::Player, panics);
            app.add_ai_tool("plant_empty_error", Scope::Player, fails_with_nothing_to_say);
            app.add_ai_tool("plant_vague_error", Scope::Player, refuses_without_saying_how_to_fix_it);
            app.add_ai_tool("plant_unrecorded", Scope::Player, changes_without_a_record);
            app.add_ai_tool("plant_fine", Scope::Player, |_: In<Call<Planted>>| -> ToolResult { Ok("fine".into()) });
        },
    )
}

fn planted(name: &str) -> Verdict {
    let _turn = crate::scenario_files::snapshot_file_turn();
    let app = planted_game();
    let tools = shards::ai_core::scenario::tool_list(app.world());
    let tool = tools.iter().find(|t| t.name == name).unwrap_or_else(|| panic!("no tool {name}"));
    check(&planted_game, tool)
}

fn names(verdict: &Verdict, needle: &str) -> bool {
    verdict.broken.iter().any(|b| b.contains(needle))
}

#[test]
fn a_tool_that_does_what_it_should_passes() {
    let verdict = planted("plant_fine");
    assert!(verdict.broken.is_empty(), "{:#?}", verdict.broken);
    assert_eq!(verdict.calls, VALID_SETS + INVALID_SETS);
}

#[test]
fn a_panicking_tool_is_caught() {
    let verdict = planted("plant_panic");
    assert!(names(&verdict, "the tool panicked"), "{:#?}", verdict.broken);
    assert!(names(&verdict, "a panic: "), "the journal keeps it: {:#?}", verdict.broken);
}

#[test]
fn an_empty_error_is_caught() {
    let verdict = planted("plant_empty_error");
    assert!(names(&verdict, "failed with an empty error"), "{:#?}", verdict.broken);
}

#[test]
fn an_error_that_does_not_say_how_to_fix_the_call_is_caught() {
    let verdict = planted("plant_vague_error");
    // Only the call under its limit gets the vague error; the rest (wrong types) are taught by the engine.
    assert!(names(&verdict, "does not name the signature"), "{:#?}", verdict.broken);
    assert!(verdict.broken.iter().any(|b| b.contains("(n)") && b.contains("too small")), "{:#?}", verdict.broken);
    assert!(verdict.broken.len() < INVALID_SETS, "{:#?}", verdict.broken);
}

#[test]
fn an_action_that_is_not_recorded_is_caught() {
    let verdict = planted("plant_unrecorded");
    assert!(names(&verdict, "the session's replay does not play back"), "{:#?}", verdict.broken);
}

#[test]
fn a_gameplay_system_that_panics_is_caught() {
    let _turn = crate::scenario_files::snapshot_file_turn();
    let cracking = || {
        shards::testing::test_app(
            |config| config.seed = 42,
            |app| {
                app.add_plugins(SandboxPlugin);
                app.add_systems(FixedUpdate, cracks_after_tick_3.in_set(SimSet::Resolve));
            },
        )
    };
    let app = cracking();
    let tools = shards::ai_core::scenario::tool_list(app.world());
    let step = tools.iter().find(|t| t.name == "step").unwrap();
    let verdict = check(&cracking, step);
    assert!(names(&verdict, "the game halted: system "), "{:#?}", verdict.broken);
}

#[test]
fn the_skip_list_names_real_tools_and_gives_reasons() {
    let app = sandbox_game();
    let tools = shards::ai_core::scenario::tool_list(app.world());
    for (name, reason) in SKIP {
        assert!(tools.iter().any(|t| t.name == *name), "`{name}` is not a tool");
        assert!(reason.len() > 10, "`{name}` needs a reason");
    }
    // Control tools other than `step` are left out by kind, not by the list.
    let covered: Vec<&str> = covered(&tools).iter().map(|t| t.name.as_str()).collect();
    assert!(covered.contains(&"step") && !covered.contains(&"pause") && !covered.contains(&"snapshot_load"), "{covered:?}");
}
