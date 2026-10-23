# Spec: what the Shards engine needs for a first-person shooter

23 Oct 2026 · written for coding agents · status: **building** (the owner's go, 23 Oct 2026; decisions under "Decisions and open questions")

This spec turns the game's GDD into engine work. The GDD is in Russian; its section headings are cited here in English:
1 Overview, 2 Core gameplay, 3 Systems, 4 Content. Each slice F1–F14 below says what is missing in the engine today, what to build, where the
boundary with the game is, what the AI sees, and when the slice is done. It follows the shape of [engine-tz.md](engine-tz.md), so an agent
that knows that file knows how to read this one. Progress goes into [engine-progress.md](engine-progress.md) as slices merge.

## Source and scope

- **Source.** The GDD on GitBook: site "Larel Docs", space "Imported content", page "GDD" (its title translates as "GDD: a first-person meat shooter"), merged
  on 23 Oct 2026. It is marked **a draft**. It ends at the heading "#3 Level 1 (tutorial and first mission)": Level 1 itself, Acts 2
  and 3, the weapons list, the enemies list and the bosses are not written yet. GDD references below use its headings ("GDD 3, Movement").
- **A different game.** [engine-tz.md](engine-tz.md) and the hand-off notes (`E:\handoff\engine.md` §4, requests R1–R4) were written for a
  third-person action game. This GDD is a **solo first-person shooter** in the pace of *Half-Life 1* and *Doom*, set indoors in a research
  complex, built on Shards ("Platform: PC, Shards Engine"). Nothing in those earlier plans carries over by default. What was built for them
  and helps here is listed under "What the engine already gives".
- **The owner's go (23 Oct 2026):** build the engine part now, as many slices at once as the machine allows. The earlier rule "the engine
  changes only when the game hits a need" does not hold this round. F8 waits for a real blockout; F14 and guards that move between cover stay
  out until the owner asks.

## What the GDD asks for

| GDD section | What it asks |
|---|---|
| 1, Basics; 2, Game modes | FPS, PC, single player, one difficulty |
| 1, Design pillars | fast, ruthless combat in the style of *Half-Life 1*; brutality and gore ("meat shooter"); story told through contrast and silence |
| 2, Controls | WASD; mouse look and aim; LMB fire; RMB alternative fire; Space jump; Ctrl crouch; E interact; R reload; 1–4 or the wheel to switch weapons; F melee; Esc pause |
| 3, Movement | fast, sharp, responsive; strafing and jumps "in the style of *Half-Life*"; jump and crouch; no slide; running is the default speed, with slower walking only in the complex scenes of Act 1 |
| 3, Combat | melee and firearms; weapons look like work tools or monster parts; the player takes parts of some killed monsters and uses them as weapons; monsters and humans; separate bosses |
| 3, Progression; Economy | new weapons appear through the levels; ammo, health, shield (armour), weapons |
| 3, AI | monsters attack; guards grow from light fighters to shields, rifles, armour and turrets (Acts 2–3); staff do not fight: they panic, run and hide |
| 3, Difficulty | the player kills fast and dies fast; enemies die from a few accurate shots, the number depends on the weapon; bosses take longer |
| 4, Locations | indoor sectors: rusty industrial (dim sodium lamps, deep shadows, humming pipes), sterile science sector (bright humming fluorescent tubes, glass partitions); a two-level workshop; airlocks; a cargo platform down to Level 2; key-card doors; a hub the player returns to between missions |
| 4, Narrative | late-90s look: concrete, fluorescent lamps, CRT monitors, freight lifts, heavy blast doors; staff speak a foreign language shown as unreadable symbols and sounds with a few words understood; HL1-style scenes where **the camera stays with the player but movement and actions can be taken away**; a silent hero whose state shows in sound and body animation |
| 4, Story (scenes #1, #2) | a tablet with a notification and a messenger; free roaming between scenes; an operator who talks through armoured glass while muffled lo-fi music from her cassette player comes through it; ambient hum of ventilation, dripping water; the player takes a cutter from a stand and enters an airlock that unlocks with a heavy click |

## How to work on a slice

- Read [AGENTS.md](../AGENTS.md) and the index topic file for every area you touch before you change it ([determinism.md](agents/determinism.md)
  for anything in the simulation, [tools-and-invariants.md](agents/tools-and-invariants.md) for tools, [content.md](agents/content.md) for content
  kinds, [ui-audio-look-fx-scatter-animation.md](agents/ui-audio-look-fx-scatter-animation.md) for the picture and sound).
- **One slice is one PR**, branch `claude/<id>-<name>` (for example `claude/f1-first-person`). Each slice brings unit tests, neutral sandbox
  content, a scenario, a reference replay when gameplay changes, and updates to `docs/manual.md`, `docs/engine-progress.md` and `CHANGELOG.md`
  (`Unreleased`).
- **The boundary.** The engine gets mechanisms, the game gets the "what". No names from this GDD in engine code, sandbox content, tests,
  the manual or other engine docs: no "liquidator", "operator", "cutter", "biomass", "complex". This spec is the one exception: it quotes the
  GDD to say where each need comes from. A weapon, an enemy, a number, a line of dialogue is game content.
- **Determinism.** Gameplay in `FixedUpdate` in a `SimSet`; randomness from `SimRng`; maths through `bevy::math::ops`; order by `AiId` or
  `ColliderId`. A new input field goes into `Intent` (already `replay_input`), with a serde default and `skip_serializing_if`, so old replays and
  content hashes stay valid. The camera and every picture-only part never feed the simulation.
- **The engine has no LLM.** Everything an agent needs is a tool (`add_ai_tool` / `add_ai_query`) or a `shards` command.
- **Checking.** The scoped local gate from [checking-changes.md](agents/checking-changes.md): the touched crates' tests, clippy with
  `-D warnings`, `--check-content`, every reference replay with `--verify`. CI runs the full suite. Never `cargo fmt`. Bevy stays at 0.20.0.

## What the engine already gives (no engine work)

| GDD need | Engine mechanism |
|---|---|
| Running by default, slower walking in some scenes | `locomotion` content (`run_speed`), a game `MoveModifiers` source (`cap_at`, `forbid_sprint`) |
| Jump with coyote time and a buffer; steps, slopes, ledges | `shards_motion` controller |
| Weapons as data: fire rate, windup, recovery, melee | `shards_action` actions and movesets (one moveset per weapon); `Effect::Sound`; cancel windows |
| Weapon draw and holster pictures | anim bridge props, sockets and `@moveset` mounts (D1.1) |
| Projectiles: bolts, grenades, slow plasma, impacts that lay zones | B4 spawned combat objects (`Projectile`, `impact`, `Spawn` patterns); the aim is new, see F4 |
| Headshots and parts that break off a monster | R1 body parts (`parts` content, part health, multipliers, `PartBroken`) on the `HURTBOX` layer |
| Picking up a monster part as a weapon | `PartBroken` / `Died` events plus a game pickup; granting the weapon is F4 |
| Armour that absorbs damage ("shield") | `Armor` (`crates/shards_action/src/fighter.rs:194`) and the game's damage hooks; a pool that absorbs a share of each hit is a game hook plus a component (`Barrier` is timed and absorbs 100 %, so it is not that) |
| Enemy reactions, stagger, death clips | B1 reactions, poise, death actions |
| Guards with frontal shields | B1 guard arcs and protection |
| Bosses | N8 attack scheduler, R1 parts, B4 objects |
| Monsters and guards that see, hear and chase | `shards_npc`: behaviours (`chase`, `attack`, `investigate`, `flee`), perception (fov, line of sight, hearing `Noise`, memory), director and tokens, navmesh per body class |
| Two-level workshop, stacked corridors | the navmesh bake keeps every floor of a column (`crates/shards_npc/src/nav/bake.rs:5`, up to 255 floors) |
| Levels from Blender, markers, trigger volumes | `shards_level` (`trigger`, `interactable`, `spawn`, `custom:` markers; `Volume` sensors with `VolumeEntered`/`VolumeExited`) |
| Pickups (ammo, health, armour) | game code on `VolumeEntered`, or on F5's use event |
| Key-card doors | game code on F5's use event and F6's movers |
| The hub and level changes with a loading screen | regions and `RegionLoading`; one big Blender level is a 1×1 region with a large `tile_size` (bakes with `--bake-nav`) |
| Saves at a level start, settings, FOV, mouse sensitivity and inversion, aim sensitivity (K1) | `shards_save`, `shards_settings` |
| Foreign speech shown as symbols with a few known words | U1 symbol-font spans (`[font=…]word[/font]`) in UI text |
| Voiced lines with subtitles; radio voice | `shards_audio` `Voice` bus, U4 subtitles |
| Tablet, messenger, HUD, menus | `shards_ui` screens and widgets from content; the tablet as a full-screen screen |
| Damage flash, screen shake, hit stop | U2 `flash`, `AddTrauma`, `shards_hitstop` |
| Taking movement away in a scene | `MoveModifiers::root` from a game source, input contexts; F12 makes it one call |
| Music that follows the fight | layered music with `npc_near` / `npc_engaged` |
| Release builds, Steam, crash reports | `shards package`, `shards_steam`, crash reports |

**Not used by this game** (keep, no work): co-op and `shards_net`, the region generator and dungeons, voxels, lock-on and lock points, dodge
and parry windows, stamina costs, rules by mode, downed and revive, the glider, swimming. The K3 shot camera is the wrong tool for this GDD's
scenes: the player keeps the camera (F12 is the route).

## What the engine needs

Overview, grouped by area; the Phase column gives the order to build in. "Size" is a rough guess: S is a day of agent work, M a few days, L a week or more.

| Id | Slice | Why (GDD) | Size | Phase |
|---|---|---|---|---|
| F0 | A first-person test room in the sandbox | every slice needs one place to test | S | folded: each slice brings its own small test room |
| F1a | Look as input: `Intent.look`, the body follows it, eye and look direction, the `look` tool, the `AIM` line (F1 items 1–3 and "For the AI") | the whole game | M | 1, first |
| F1b | The first-person camera, the hidden own body, the view model, the view from a bone (F1 items 4–8) | the whole game | L | 1, after F1a |
| F2 | Crouch | 2, Controls; 3, Movement | M | 1 |
| F3 | Shooting: hitscan and 3D aim | 2, Controls; 3, Combat | M | 1 |
| F4 | Weapons: slots, switching, ammo, reload | 2, Controls; 3, Economy | M | 1 |
| F5 | Use by looking at a thing | 2, Controls (E); scenes #1, #2 | S | 1 |
| F6 | Movers: doors, lifts, platforms | 4, Locations and Narrative | M | 1 |
| F7 | Air movement in the style of *Half-Life* | 3, Movement | S | 2 |
| F8 | Mesh colliders from Blender | indoor levels | L | waits for a real blockout (the owner) |
| F9 | Gore: decals on any surface and lasting debris | 1, Design pillars | M | 2 |
| F10 | Ambient emitters and occlusion in audio | scenes #1, #2 | M | 2 |
| F11 | NPCs: gunfire, hiding, ranged fighters, turrets | 3, AI | L | 3 (humans arrive at the end of Act 1) |
| F12 | Scenes that keep the player's camera; NPC sequences | 4, Narrative; scenes #1, #2 | M | 1 (the opening needs it) |
| F13 | Lights from content: types, flicker, colour grading | 4, Locations | M | 2 |
| F14 | Screens in the world (render to texture) | 4, Narrative (CRT monitors) | M | out until the owner asks |

Phase 1 is a playable opening and Level 1 blockout; phase 2 is feel and real levels; phase 3 is the human enemies and polish.

---

## F0. A first-person test room in the sandbox

Every slice below needs the same small level to test against, and the sandbox is third-person.

What to do:

1. A sandbox level `fps_range` (a Blender export through `tools/blender/`, like `ruin_tile.py`, or inline tile geometry): a corridor with a
   low duct (crouch), a step and a ramp, a door, a lift shaft with two floors, a range of target dummies at 5, 15 and 40 m, a dummy with a head
   part (R1), a trigger volume, and an `interactable` marker.
2. A sandbox flag or content switch that spawns the player with the first-person view (F1) in that level. The third-person arena stays the default.
3. A scenario `fps_range.scenario` that walks the corridor. Later slices extend it.

Done when: `cargo sandbox --headless` loads the level, `--check-content` passes, the scenario passes.

## F1. First-person view, look as input, view model

**Today.** There is no first-person camera: `ThirdPersonCamera` orbits at an arm (`crates/shards_motion/src/camera/mod.rs`). The camera's
yaw only turns the move axes (`crates/shards_motion/src/input/mod.rs:642`). `Intent.aim` is a point to face
(`crates/shards_core/src/intent.rs:61`): the body turns toward it on the ground plane, and nothing records the view's pitch. A body faces where
it walks or its lock-on target. `RenderLayers` is used only by `render_scale` and `shards_look`.

What to do:

1. **Look in `Intent`.** A new replay input `Intent.look: Option<Look { yaw, pitch }>` (radians; pitch clamped to content limits). The camera
   writes it between ticks, the same way it writes `Intent.target` today. The simulation reads only `Intent`. Serde default `None` and
   `skip_serializing_if`, so old replays verify unchanged. `IntentState` in `shards_net` (`crates/shards_net/src/inputs.rs:68`) mirrors
   `Intent` for the wire codec: carry the new field there (or skip it explicitly and say so in the manual), with a test, or the net tests break
   even though this game has no co-op.
2. **The body follows the look.** A `locomotion` option (for example `face: Look`) turns the body's yaw to `look.yaw` every tick, with no turn
   rate. Movement input is relative to the look yaw, not to a camera. Agents' `Intent` without `look` behave as today.
3. **The eye.** `eye_height` per stance in `locomotion` content (standing, crouched from F2). The engine gives `eye_of(world, body)` and
   `look_dir(world, body)` (3D unit vector) for every system that needs them: F3, F5, F11, audio's listener.
4. **The first-person camera.** `FirstPersonCamera` (window only, the maths testable without one): it sits at the drawn body's eye
   (interpolated, `shards_motion::interp`), turns with the mouse per frame (no smoothing by default; sensitivity, inversion and FOV from
   settings; `aim_sensitivity` from K1), and writes `Intent.look`. Content kind or `camera` fields for pitch limits, the eye's smoothing on steps
   (a short ease so stairs do not jerk), landing dip (`critically_damped_step` exists), and optional view bob and roll (off by default; the game
   tunes them). Shake (`AddTrauma`) and the U2 flash work as today.
5. **Hide the own body.** The local player's body model is not drawn to its own first-person camera (a render layer), but is still drawn to
   others and to `shards shot` labels.
6. **The view model.** A second camera on its own render layer, drawn over the world with its own FOV (content) so the weapon never clips into
   walls. A view model is a glTF (arms and weapon) attached to the camera, animated by the anim bridge from the body's `ActionState` through
   its own `animset` (fire, reload, draw, idle, walk sway). Props and `@moveset` mounts work on it as they do on bodies. Picture only: no `AiId`,
   nothing in replays.
7. **A view from a bone (optional).** While an action runs that names it, the camera follows a bone of the body's model with the player's look
   added on top (getting up from a bed in scene #1). Picture only.
8. **Tick and latency.** The tick stays 60 Hz by default. Measure the time from a click to the shot's tick and to its picture on the
   `fps_range`; report it in the PR. If it feels late, the game can raise the tick through `EngineConfig`; do not add sub-tick input in this slice.

Boundary: the FOV numbers, bob, weapon models and their animations are the game's.

For the AI: the tool `look yaw= pitch=` and `look at=eN [part=NAME]` (sets `Intent.look`, recorded); `observe` gains an `AIM` line: what the
centre of the view hits and how far (`AIM e7 Dummy head 14.2m`, or `AIM wall 3.1m`), computed from the body's eye and `Intent.look`, never from
the camera.

Done when: in `fps_range` an agent turns with `look`, walks relative to its view, and `AIM` names the dummy in front of it; the replay of that
run verifies headless and `--windowed`; old reference replays verify unchanged; an `--offscreen` shot shows the view model and not the own
body; the frame-time test shows no regression.

## F2. Crouch

**Today.** The controller has no stances: the capsule's height is fixed in `locomotion` (the only "crouch" in the code is a phase name).

What to do:

1. A crouched stance in the controller: `crouch_height`, `crouch_speed`, `crouch_time` (how fast the capsule and the eye go down and up) in
   `locomotion` content.
2. Input name `crouch` (held; `accessibility.hold_to_toggle` already turns it into a toggle). Standing up waits while the space above is blocked
   (a shape cast), so the body never stands into a ceiling.
3. Crouching in the air pulls the feet up (the capsule shrinks from the top on the ground, from the bottom in the air), so a crouch-jump clears a
   higher ledge, as in *Half-Life*.
4. The stance is simulation state in `MotionState` (reflected, in snapshots and the replay hash). Events `Crouched` / `Stood`.
5. NPC perception and hits see the crouched body: its eye and its hurtbox shrink with the capsule; R1 part offsets may be given per stance
   (`PartDef.at` by stance) so a head stays a head.

For the AI: the tool `crouch hold=true|false`; the stance in the body's describer (`ground 2.1m/s crouched`); `clearance` and `path_to` take a
`stance=` argument.

Done when: in `fps_range` the agent cannot stand up under the duct, passes it crouched, and clears the crouch-jump ledge that a plain jump
misses; unit tests for the ceiling check and the air crouch; the replay verifies.

## F3. Shooting: hitscan and 3D aim

**Today.** Every `HitShape` (`crates/shards_action/src/def.rs:103`) lives on the ground plane: a sphere, a capsule thrust, a horizontal cone,
a box, a disc. B4 projectiles fly along `SpawnAim::Facing` (flat) or toward a lock-on target (`crates/shards_action/src/objects.rs:117`). There
is no instant ray, no spread, and no way to aim up or down by the view. The engine makes a `Noise` on every hit and sprint, not on a shot.

What to do:

1. **A hitscan effect.** `Effect::Fire(FireDef)` on an action, at its moment: `pellets`, `spread_deg` (a cone, each pellet drawn from `SimRng`
   keyed by tick, `AiId` and pellet index), `range`, `falloff` (damage by distance, as points), `pierce` (how many bodies a ray passes), and the
   hit's `HitDef`-like payload (damage, poise, reaction, flags). One function gives a shot's ray, `shot_ray(world, shooter) -> (origin, dir)`:
   the shooter's eye (F1) along `look_dir`, or for an NPC along its aim at its target. Every pellet starts there (spread turns `dir`), tests
   `HURTBOX` and `WORLD` through `PhysicsWorld` in the simulation, stops at the first `WORLD` collider, and resolves hits through the existing
   pipeline (credit, R1 parts and their multipliers, armour, hooks, reactions) in `AiId` order. F11's line-of-fire check uses the same
   `shot_ray`, so the check and the shot never disagree.
2. **Events for the picture and sound.** `ShotFired { shooter, from, dir, pellets }` and `ShotImpact { point, normal, collider or body, part }`,
   so F9 lays decals, `shards_fx` plays sparks and blood, and `shards_audio` plays the impact, all picture or local.
3. **Noise.** Each shot fires a `Noise` with a content radius (`FireDef.noise`), so NPCs hear gunfire (F11).
4. **3D aim for spawned objects.** `SpawnAim::Look`: a projectile pattern starts at the eye (plus an offset) and flies along `look_dir` (pitch
   included). For NPCs, the same aim toward their target's aim point, with an `aim_error_deg` from content.
5. **Recoil (optional).** `FireDef.kick: (pitch, yaw)` adds to the shooter's look as a game-tunable kick, applied in the simulation to the next
   tick's look so a replay sees it.

Boundary: the weapons, their numbers and their sounds are content.

For the AI: the tool `fire` (presses the fire input); the journal line `e5 fires pistol_shot: e7 head -45 hp`; a query `shot_trace` that shows
where a shot from the current look would go and what it would hit, without firing.

Done when: in `fps_range` an agent kills the near dummy with body shots and the head dummy with fewer head shots; a shotgun's pellets spread the
same way on every run; a wall stops a shot; a dummy behind another is hit only with `pierce`; the replay verifies; heavy mode passes.

## F4. Weapons: slots, switching, ammo, reload

**Today.** A weapon can be a moveset, and a moveset switch has draw pictures (`@moveset`). Actions cost stamina only. Binding conditions can be
the game's (`When::Condition`, `crates/shards_action/src/def.rs:1957`), so a game could hold ammo itself, but every shooter would rewrite
the same thing. The standard input map binds the mouse wheel to `lock_switch`.

What to do:

1. **Pools.** A component `Pools` with named counters: a magazine and a reserve per ammo kind (`pistol: 9/17, reserve 51`). Content gives each
   moveset its magazine size and ammo kind; a pool's maximum is content.
2. **Costs.** `ActionDef.cost: {pool: n}` (beside the stamina cost). An action that cannot pay is refused with a clear reason
   (`magazine empty: reload`), and an empty magazine can start a moveset's `auto_reload` action instead.
3. **Reload.** An effect `Refill { pool }` moves rounds from the reserve to the magazine at its moment (so a reload cut early gives nothing, or
   per round for shell-by-shell weapons: `per_round: true`).
4. **The arsenal.** A component `Arsenal`: owned weapons in numbered slots, the current one, and API `grant_weapon`, `take_weapon`,
   `select_slot`. Inputs `weapon.slot.N`, `weapon.next`, `weapon.prev`; a game that does not use lock-on binds the wheel to these. A switch
   waits for the current action to reach a cancel window, then runs the holster and draw through the existing moveset switch.
5. **Pickups.** `add_to_pool(world, body, kind, n)` and `grant_weapon` are the calls a game's pickup makes; both return what was taken, so a full
   pickup can stay on the floor.
6. All of it is simulation state on `AiId` bodies (reflected, in snapshots and the replay hash).

For the AI: tools `reload`, `weapon slot=N|next|prev`; the body's describer shows the weapon and ammo (`pistol 9/17 (51)`); UI sources
`you.ammo`, `you.reserve`, `you.weapon` for the HUD.

Done when: the scenario fires a pistol dry, sees the refusal, reloads, switches to a second weapon and back; a cut reload gives no rounds;
`--check-content` refuses a cost on a pool the moveset does not have; the replay verifies.

## F5. Use by looking at a thing

**Today.** The `interact` input exists, but its only consumer is the revive pair binding. The level's `interactable` marker is only a marker.

What to do:

1. A use ray from the eye along `look_dir` (F1), `use_range` from content (default 2 m), against a new physics layer `USE` (or the marker's own
   sensor collider) **and** the blocking `WORLD` geometry. A target counts only when it is nearer than every `WORLD` hit on the ray (glass
   and walls block a use). Among targets, the nearest wins, ties by `ColliderId`.
2. A level `interactable` gets a collider on that layer (its shape from the node, as volumes do).
3. On an `interact` press, the event `Used { who, what, name }` (in the simulation, journalled); `hold_s` on a marker turns it into a hold-to-use
   with `UseProgress` (for a valve or a lever).
4. A UI source `you.use_target` (the name, or a Fluent `@id` from the marker) for an on-screen prompt.

Boundary: what a use does (open a door, take the tablet, refuse a locked door) is the game's.

For the AI: `interact` gains `target=eN` (looks at it first, then presses); `observe` shows `USE e12 "door" 1.4m` when something is in reach.

Done when: in `fps_range` an agent looks at the marker and uses it, a thing out of range or behind glass is not used, the journal shows the
event, the replay verifies.

## F6. Movers: doors, lifts, platforms

**Today.** The manual says: "A kinematic collider does not carry what stands on it: there is no platform motion, and a body on one that moves is
left behind." Nothing moves a collider along a path. The GDD has blast doors, airlocks, freight lifts and a cargo platform in its first hour.

What to do:

1. A content kind `mover`: keyframes (positions and rotations relative to its start), speed or times, `wait`, `loop`, and what to do when blocked:
   `Stop`, `Reverse` or `Crush { damage }`.
2. A level property `mover=<id>` on a Blender node makes its colliders a kinematic mover (the node and its children move together; the picture
   follows).
3. The mover steps in `SimSet::Move` before the controller, in `AiId` order. A body standing on it is carried (vertical and horizontal velocity,
   and yaw for a turning platform); a mover moving into a body pushes it, and if the body cannot move, the blocked rule applies.
4. API `mover_go(world, mover, key)`, `mover_state`; events `MoverStarted`, `MoverArrived`, `MoverBlocked`. Simulation state on an `AiId`
   entity, in snapshots.
5. NPCs: a closed door's collider is cut from the navmesh at its next refresh (as laid colliders already are). A lift as a navlink for NPCs is
   later work; say so in the manual.

Boundary: which button calls a lift, a key card, the sounds of a door: the game's.

For the AI: the describer `mover e14 "lift" at key 1, moving up 1.2m/s`; a tool `mover_go id=eN key=K` (dev); the condition `mover_arrived`.

Done when: in `fps_range` the agent rides the lift up and down without falling through or sliding off, a closing door pushes it out of the way,
a `Crush` mover damages it; heavy mode passes; the replay verifies.

## F7. Air movement in the style of *Half-Life*

**Today.** The controller steps the velocity toward a target velocity, with `air_control` scaling the speed-up in the air. *Half-Life*
movement accelerates along the wished direction until the speed projected on that direction reaches a cap, with ground friction and a stop
speed. That is what makes strafing and air turns feel like the GDD asks.

What to do:

1. A `locomotion` option `accel: Quake { ground_accel, air_accel, air_speed_cap, friction, stop_speed }`, off by default (every current game
   and replay unchanged).
2. Written as the closed form per tick so the result does not depend on the tick rate within tolerance (test 60 and 120 Hz).
3. Whether a strafe-jump may gain speed above the run speed is open question 3; build the formula, and let content cap it.

Done when: unit tests compare the formula with reference numbers; a strafe turn in the air keeps speed; with the option off every reference
replay verifies byte for byte.

## F8. Mesh colliders from Blender

**Today.** Collider shapes are `Cuboid`, `Sphere`, `Capsule`, `Cylinder` and `Cone` (`crates/shards_motion/src/physics/mod.rs:54`);
`collider=auto` is a box around the mesh bounds. `shards_level` reads only the glTF's JSON and loads no buffers. Indoor levels with pipes,
stairs, catwalks and ramps can be blocked out with primitives, but slowly and with gaps.

**Gate:** start this slice when the first Blender blockout of a real level shows that primitives are not enough (open question 5).

What to do:

1. `Shape::TriMesh` for static `WORLD` colliders only (`collider=mesh` on a node), built with parry3d from the mesh's triangles, with
   internal-edge fixing so a capsule does not catch on seams.
2. `shards_level` reads the mesh's position and index buffers (`.glb`, `.gltf` + `.bin`, embedded `data:` URIs) for those nodes only, still
   without a window.
3. The broad phase, ray and shape casts, the controller's step and slope rules and the nav bake work on it (the bake already raycasts any shape).
4. A content-check cap on triangles per collider and per level; a non-static mesh collider is an error.
5. Snapshots: the `static_colliders` check compares mesh colliders by a digest.

Done when: `fps_range`'s ramp and stairs as one mesh collider are walked, stood on and baked; a capsule slides along a mesh wall without snags;
the broad-phase timing test shows the cost; every replay verifies.

## F9. Gore: decals on any surface and lasting debris

**Today.** `shards_fx` decals are for telegraphs only: they project straight down within 0.6 m; "there is no general API for spots (blood,
tracks)". Particles have no textures or meshes, no collision with the ground, a life of at most 30 s, and a pool that recycles the oldest. Physics
is kinematic queries only: there are no rigid bodies, so there are no ragdolls (open question 4).

What to do:

1. **Surface decals.** A content kind for spots (texture or colour, size range, random rotation, fade, `lifetime` or `UntilCleared`), projected
   along a surface normal onto any `WORLD` surface (walls, ceilings, floors) and onto bodies (optional, later). A pool per kind with a cap,
   recycling the oldest. Spawned by the game or by content links from F3's `ShotImpact` and from `Damaged`.
2. **Lasting debris.** Mesh particles (a glTF mesh per particle) and textured sprites; a particle may collide with `WORLD` colliders (a ray per
   step against `PhysicsWorld`), bounce, slide and come to rest; `rest: UntilCleared` keeps it.
3. **Clearing.** `shards_fx::clear(world, tag)` and a clear on region change, so a level reload does not pile up.
4. All picture only: no `AiId`, not in replays, nothing on a headless machine. Budgets in the `perf` counters.

Boundary: the blood, the chunks and how much of it there is are the game's content.

For the AI: the `effects` query lists live decals and resting debris counts per kind.

Done when: shots in `fps_range` leave holes on walls and blood on the floor and walls behind a dummy; chunks from a broken part bounce and rest;
the cap holds at 10× the budget; an `--offscreen` shot shows them; replays unchanged.

## F10. Ambient emitters and occlusion in audio

**Today.** Sounds are one-shots (`SoundDef`, `crates/shards_audio/src/def.rs:83`), music stems loop, and nothing else loops. Nothing places a
sound in a level, and nothing muffles a sound behind a wall or glass.

What to do:

1. **Looping sounds.** `SoundDef.looped: bool`. A component `SoundEmitter { sound, playing }` that the mixer keeps in a voice while it is in
   earshot, with priority rules so loops are not stolen by one-shots of lower priority.
2. **Emitters in levels.** A level property `sound=<id>` on a node spawns an emitter (ventilation hum, dripping water, a cassette player). API to
   start and stop one.
3. **Occlusion.** Each spatial voice is tested with a `WORLD` ray from the listener to its source; a blocked voice is quieter and duller. A
   collider may carry a `transmission` (glass passes more than concrete). If Bevy 0.20's audio cannot filter per voice, ship the gain part and
   write the gap in the manual; do not add a third-party audio crate without [third-party.md](agents/third-party.md).
4. The listener is the first-person eye (F1).
5. Local only, as today: nothing in the simulation or replays.

For the AI: `audio` lists emitters and each voice's occlusion (`hum e20 4.1m gain 0.30 (occluded: glass)`).

Done when: a looped emitter plays only in earshot and resumes when you return; behind a wall it is quieter than in the open; behind a
`transmission` collider it is between the two; tests with the mixer, no output device needed.

## F11. NPCs: gunfire, hiding, ranged fighters, turrets

**Today.** Behaviour hooks include `flee` (run away from the target) and `investigate`. Perception notices only hostile fighters (`Team`) and
noises. There is no cover or hiding search, no line-of-fire check, and attack plans close in to melee windows.

What to do:

1. **Fear without a fight.** Perception option `notice: Hostile | Threats` so a non-combatant notices an armed body that attacks anyone, and
   gunfire `Noise` (F3), as a threat it does not fight. A condition `threat_known`.
2. **Hiding.** A query `hiding_spot(world, npc, threat, within)`: navmesh nodes reachable from the NPC, out of the threat's sight (a `WORLD`
   ray from the threat's eye), farthest from it, ties by node order; a hook `hide` that runs there and stays; `cower` when no spot is found (the
   game plays the animation).
3. **Ranged fighters.** An attack-plan option that keeps a distance band (`keep_range: (min, max)`) instead of closing in; a line-of-fire check
   before a press along the shot's own ray (F3's `shot_ray`: no ally and no wall between it and the target's aim point), refused with a
   reason in `npc_schedule`; accuracy from
   content (`aim_error_deg`, worse while moving or far); bursts as actions.
4. **Turrets.** A body with no locomotion that turns its look at a content rate (yaw and pitch limits) and fires through F3; activated and
   deactivated by the game.
5. **Optional, only if Act 2's design asks:** guards that move between cover points. The GDD names shields, rifles, armour and turrets, not cover;
   do not build this until the owner confirms.

Boundary: who panics, what guards carry, and their numbers are content.

For the AI: the NPC describer shows `hiding`, `keeping range 8–14m`, `no line of fire: e9 in the way`; `npc_state` shows the hiding spot.

Done when: in a sandbox scene a non-combatant runs out of sight of a shooter and stays there; a ranged NPC holds its band, never fires through
an ally, and hits less while moving; a turret tracks and fires within its limits; the replays verify; `npc_*` invariants hold.

## F12. Scenes that keep the player's camera; NPC sequences

**Today.** A game can root the player (`MoveModifiers::root`) and switch input contexts, and can drive NPCs with orders and `force_action`. There
is no single call for "the player may look but not move or act", and no data format for a scripted NPC sequence (walk here, turn, play this,
say that, wait).

What to do:

1. **Control lock.** `ControlLock { movement, actions, look: Free | Cone { at, deg } }` set and cleared by the game with a reason; the engine
   filters the player's `Intent` accordingly (simulation state on the player, in snapshots). A gentle look nudge toward a point is optional and
   picture only.
2. **Sequences.** A content kind `sequence`: steps `MoveTo(marker)`, `Face(marker or role)`, `Play(action)`, `Sound(id)` (with its subtitle),
   `Wait(s)`, `WaitFor(condition)`, `Signal(name)`; roles bound when the game plays it (`play_sequence(world, id, roles)`). Steps run in the
   simulation on the named NPCs; events `SequenceStep`, `SequenceEnded { skipped }`; a game may stop one.
3. The NPC's brain is dormant while a sequence holds it and resumes after.

Boundary: the scenes themselves, their lines and when they play are the game's.

For the AI: tools `sequence_play id= role.NAME=eN`, `sequence_stop`; journal lines per step; `observe` shows the lock (`CONTROL looking only`).

Done when: a sandbox scene locks the player's movement, an NPC walks to a marker, turns, plays an action and a voiced line with a subtitle,
then the lock clears; the replay verifies; a snapshot mid-sequence resumes it.

## F13. Lights from content: types, flicker, colour grading

**Today.** The engine has no light content. Lamps exported from Blender are drawn by Bevy's glTF loader as they are; the sandbox spawns its own
lights. Nothing flickers, and the camera's `picture` has tone mapping and bloom only.

What to do:

1. A content kind `light`: type (point, spot), colour (or a colour temperature), intensity, range, shadows, and `flicker` (a pattern string or
   noise with rate and depth, from a picture-side seed so it never touches `SimRng`). A level property `light=<id>` on a Blender lamp applies it.
2. A shadow budget in `graphics` settings (how many lights cast shadows), nearest first.
3. Colour grading in the camera's `picture` (lift, gamma, gain or a LUT), blended by trigger volumes so a sector or a scene can change the
   palette.
4. Picture only.

For the AI: a `lights` query (counts, shadow casters, flickering lights).

Done when: an `--offscreen` shot of `fps_range` shows a flickering lamp at two moments with different light; a grading volume changes the shot's
colours; the frame-time test stays within budget.

## F14. Screens in the world (optional)

CRT monitors that show text and graphs, and possibly the tablet as an object in the hand.

What to do: a `shards_ui` screen rendered into a texture on a mesh material (a level property `screen=<id>`), updated from UI sources; picture
only; a cap on how many render each frame.

Done when: an `--offscreen` shot shows a monitor in the level with a UI screen's text on it that changes with its source.

---

## Decisions and open questions

Decided by the owner on 23 Oct 2026:

- **Build now, in parallel:** the engine part only, as many slices at once as the machine allows (question 1 below is answered: not this round).
- **Deaths:** death clips and chunks, as in *Half-Life 1* (F9). No ragdolls and no rigid-body solver (question 4).
- **Mesh colliders (F8):** wait for a real Blender blockout (question 5).
- **Out until asked:** F14 (screens in the world) and guards that move between cover (F11 item 5).

Still open:

1. ~~**The engine rule.**~~ Answered above.
2. **Which game.** Does this GDD replace *Shards of the Frontier* (the third-person game in `E:\frontier`), or is it a second game? Where will it
   live (repository, folder)?
3. **Air movement.** Should strafe-jumps gain speed above the run speed, as in *Half-Life* (bunny hopping), or only feel responsive in the air?
4. **Ragdolls** (answered above: death clips and chunks). *Half-Life 1* used death animations and flying chunks. That fits F9 with no physics solver. Real ragdolls need rigid-body
   dynamics, which the engine does not have at all: a large separate project. Death clips and chunks?
5. **Mesh colliders** (answered above: after a real blockout). Build F8 now, or after the first Blender blockout shows the need?
6. **Screens in the world.** Is the tablet as a full-screen UI enough, with F14 dropped?
7. **The rest of the GDD.** The draft stops before Level 1. The weapon list, the enemies of Acts 2–3 and the bosses may add slices (for example
   a grenade arc, a flamethrower cone, a boss with weak points); this spec will need a pass when those sections are written.
