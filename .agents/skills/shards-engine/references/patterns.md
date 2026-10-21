# Patterns, and the files to copy them from

Each pattern names a real file in this repository: open it, copy its shape and its imports, and keep its
level of comments (doc comments say what and why; a tool's doc comments are its schema). The sketches below
are the outline, not a replacement for the file.

## A whole module

`crates/shards_rules/src/lib.rs` (`RulesPlugin::build`): types registered, content kinds, a content check on
another kind, a snapshot resource, an optional replay check, a snapshot prepare, systems in an ordered set,
`add_engine_module(name, VERSION)`, an engine rule, a condition, an `ai/status` field, a dev entry, a journal
line, tools, a co-op route. A new crate also needs the steps of `docs/agents/new-module.md` and its line in
`EnginePlugins` (`crates/shards/src/plugins.rs`).

```rust
app.register_type::<MyState>()
    .add_content::<MyDef>()
    .add_snapshot_resource::<MyState>()
    .configure_sets(FixedUpdate, MySystems::Run.in_set(SimSet::Resolve).after(ActionSystems::Hits))
    .add_systems(FixedUpdate, run.in_set(MySystems::Run))
    .add_engine_module("my_module", VERSION)
    .add_engine_invariant("my_module.state_sane", state_sane)
    .add_ai_condition("my_thing_ready", ready)
    .journal_event::<ThingHappened>(journal_line);
tools::register(app.world_mut());
```

## A tool

Follow the crate's own `src/tools.rs`. Some register from `register(app: &mut App)` with `app.add_ai_tool` /
`add_ai_query` (`crates/shards_action/src/tools.rs`, `crates/shards_npc/src/ai.rs`), others from
`register(world: &mut World)` with `register_tool(world, name, scope, ToolKind::…, f)`
(`crates/shards_rules/src/tools.rs`: `rules` a query, `rules_set` an action); `add_ai_query` is `register_tool`
with `ToolKind::Query`. `hitstop_play` in `crates/shards_hitstop/src/tools.rs` is a control tool, never recorded:
do not copy it for a tool that changes the world. A game: `template/src/ai.rs` (`taunt`).

```rust
/// What the tool does and answers, for the agent that reads `shards tools --json`. Only this doc comment, on the
/// argument struct, becomes the tool's description: one on the `fn` is never read.
#[derive(Deserialize, JsonSchema)]
pub struct ThingArgs {
    /// The body to act on: 6 or "e6" (`observe` lists them).
    pub id: Id,
    /// How far, in metres (default 1).
    #[serde(default)]
    pub far: Option<f32>,
}

fn thing(In(call): In<Call<ThingArgs>>, world: &mut World) -> ToolResult {
    let body = call.args.id.entity(world)?; // `e57 no longer exists (… despawned at tick 812); live: …`
    let Some(mut fighter) = world.get_mut::<Fighter>(body) else {
        return Err(format!("{} is not a fighter; `observe` lists the fighters", call.args.id));
    };
    // ... the error says how to fix the call, the success what happened.
    Ok(format!("ok: thing done to {}", call.args.id))
}

// in the crate's register(): app.add_ai_tool("thing", Scope::Dev, thing);
```

- A required argument is a plain field (`id: Id`): the engine refuses a missing or malformed one by itself, with
  the signature (``missing field `id`. Expected: thing(id: integer|string, far?: number)``). `Option<…>` makes the
  signature say `id?`, which `shards tools`, the fuzzer and the tool contracts then believe.
- `Scope::Player`: what a person could do; it acts through `Intent` and the game's rules and needs a body
  (`call.caller.require_subject()?`). `Scope::Dev`: cheats, inspection, developer actions.
- `ToolKind::Query` reads (no journal, no replay); `ToolKind::Action` is journaled, shows `CHANGES` and is
  recorded in replays, so it must be deterministic given its arguments; `ToolKind::Control` moves time or the
  session (pause, step, possess) and is never recorded.
- `crates/sandbox/tests/sandbox/tool_contracts.rs` calls every tool by itself, with 20 argument sets valid by its
  schema and 10 broken ones: it must not panic, a refusal must name the signature or the bad argument, and a
  session of its action calls must replay identically. Write the doc comments and the errors with that in mind.
- Resolve ids through `Id::entity` / `AiIndex::resolve`: their errors already teach.
- A JSON answer (`ToolResult<Value>`, like `npc_state`): round an `f32` in `f64` before `json!`, or `0.48` prints
  as `0.47999998927116394` (`crates/shards_npc/src/nav/tools.rs` does it). A query reads content with `peek`.
- A shortcut a developer wants in the window (a toggle, "go to boss phase") is also a dev menu entry calling the
  same tool: `add_dev_entry("combat", "thing", "Do the thing", DevCommand::Run("thing id=e5".into()))`. A query
  meant for agents needs none: `console line="…"` reaches every tool.
- In co-op (`NetRoute::default_for`): a query answers where it is asked (`Local`; it cannot be forwarded to the
  host), from what that machine holds, a client's own body as it predicts it (`shown_state`); a query only the
  host can answer is `NetRoute::HostOnly(why)` or refuses on a client in its body, as `npc_schedule` does. A dev
  action is forwarded to the host by default; one only the host may make: `app.net_route(name, NetRoute::HostOnly(why))`.

## A condition

`fn(&World) -> bool`, registered with `add_ai_condition("name", f)`: `arena_cleared` in `template/src/ai.rs`.
Query with `world.try_query_filtered::<…>()` and return `false` when the world has none of the things.

## An invariant and its control test

```rust
/// What must hold and why, and why any carve-out exists. Reads only; names offenders lowest id first.
pub fn state_sane(world: &World) -> Option<String> {
    let mut broken = Offenders::default();
    // `with_query` keeps the query state between ticks: building one walks every archetype.
    with_query::<(&AiId, &MyThing), (), _>(world, |query| {
        for (id, thing) in query.iter(world) {
            if !(0.0..=1.0).contains(&thing.share) {
                broken.note(*id, || format!("MyThing.share = {} outside 0..=1", thing.share));
            }
        }
    })?;
    broken.report()
}
```

Real ones: `state_known` in `crates/shards_npc/src/invariants.rs`, `crates/shards_encounter/src/invariants.rs`;
the game's `grunt_chance` in `template/src/ai.rs` (a game may query directly). Its name goes into `ENGINE_RULES`
in `crates/sandbox/tests/sandbox/invariants.rs`, and the rule count that file's test expects goes up by one. The
control test plants the bug and expects the stop (`plant` and `stops_on` in
`crates/sandbox/tests/sandbox/engine_rules.rs`; a module's own tests can hold its rules' controls, as
`crates/shards_encounter/tests/encounter/invariants.rs` does):

```rust
#[test]
fn a_share_outside_zero_to_one_stops_the_game() {
    let mut app = sandbox::test_app(42);
    plant(&mut app, 5, |world| { /* set a MyThing.share to 2.0 */ });
    let line = stops_on(&mut app, "my_module.state_sane", 5);
    assert!(line.contains("MyThing.share = 2 outside 0..=1"), "{line}");
}
```

## A content kind

`template/src/arena.rs` (`ArenaDef`) is the commented model: `#[serde(deny_unknown_fields)]`, a `validate` that
checks numbers and declares every reference with `refs.require::<PrefabDef>(&id, "field")` (a missing id
becomes an error naming the file, the field and the closest id), `add_content::<T>()`, `require_content` for
the ids the code relies on. Engine kinds: `crates/shards_hitstop/src/def.rs`, `crates/shards_fx/src/def.rs`
(presentation: `const SIMULATION: bool = false;`). A field added to an existing kind:

```rust
/// What it does; off by default, so content that does not use it is unchanged.
#[serde(default, skip_serializing_if = "Option::is_none")]
pub zone_margin: Option<f32>,
```

## What agents see and hear

- A component's line in `observe`/`inspect`: `app.register_ai_component::<MyThing>(|t| format!("charging {:.1}s", t.left))`;
  a fragment that needs other components: `register_ai_describer`; the world around it: `register_ai_world_describer`
  (`crates/shards_action/src/ai.rs` registers many).
- A journal line for an event: `app.journal_event::<RulesChanged>(journal_line)` (`crates/shards_rules/src/lib.rs`).
  The line is a short sentence with ids: `e19 hand broken by e5`.
- A field of `ai/status` (`shards status`): `add_ai_status("rules", status)`.

## Simulation state

- A resource: `#[derive(Resource, Reflect, Default)]`, `#[reflect(Resource, Default)]`, `add_snapshot_resource::<R>()`;
  `crates/shards_world/src/lib.rs` (`RegionBuild`), `crates/shards_rules/src/lib.rs` (`ActiveRules`).
- A component on bodies: `#[derive(Component, Reflect, Clone, Debug, PartialEq)]`, `#[reflect(Component, Default)]`,
  `register_type` (`Grunt` in `template/src/brain.rs`; `Downed` and `Grace` in `shards_action`).
- Test it with a snapshot round trip (`crates/sandbox/tests/sandbox/snapshots.rs`) and run the state audit.

## A system in the simulation

`template/src/brain.rs` (`grunt_brain`) shows the habits in twenty lines: `SimSet::Think`, targets sorted by
`AiId`, randomness from `SimRng` with a salt constant, the result written to `Intent` as an `Order`, not to the
body. Engine systems sit in their module's set (`NpcSystems::Plan`, `ActionSystems::Hits`, …) and a new set is
ordered against the existing ones in `configure_sets`.

## Tests

The crate's test binary and its helpers: `crates/shards_npc/tests/npc/main.rs`, `common.rs`, `live.rs`; a crate
that needs the whole engine: `crates/shards_rules/tests/rules/main.rs` (`shards::testing`). More in
`.agents/skills/shards-verify/references/test-kinds.md`.
