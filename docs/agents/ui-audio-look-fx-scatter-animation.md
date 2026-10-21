# UI, audio, look, effects, foliage and animation

Read this when you touch screens and menus, sound and music, mesh looks, particle effects and decals, scatter, or animsets and baked motion. Part of the [AGENTS.md](../../AGENTS.md) index.

## Listening to the game

- Sound is resolved on the simulation clock and is the same without a window: listen to the game through `audio` and the `MUSIC` line in `observe`, not through
  the speakers (except the sound of the `AnimationCue` clip marker: it exists only if a model is drawn). Music changes go
  to the journal as lines like `[local] music: arena explore → combat` (they are not game events). The sandbox's and the template's sound files are synthesised by `cargo run -p sandbox --example make_sounds`.

## UI

- **The HUD and menus are content** (`*.screen.ron`, the theme is `*.theme.ron`), not window code: the game gives widgets their values with
  `add_ui_source`, and button actions with `add_ui_action`. Screens are outside the simulation (no `AiId`, not in the snapshot); anything that changes the world,
  a button does through a tool (`tool …`) or a game action, and the replay repeats the press itself (`ui_click`, `ui_navigate`).

## Sound

- **Sound is content** (`*.sound.ron`, `*.music.ron`, the mixer `*.audio.ron`): an attack names its sound in `telegraph.sound`, and an action
  plays the effect `Sound("whoosh")` at its own moment (the swing); the game plays its own `PlaySound`, and a music layer waits for a condition by name
  (`add_ai_condition`). Sound only reads the world and does not change it; what only this machine shows is written to the journal as
  `JournalKind::Local`, not `Event` (the host sends events to clients). Sound is resolved on the simulation clock and is the same without a window and on
  a co-op client, so a sound the game must always hear is `telegraph.sound` or the `Sound` effect, not a clip marker:
  its sound (`AnimationCue` with the sound's name) exists only where a model is drawn, and `audio` without a window will not show it.

## Looks

- **Mesh looks are content** (`*.look.ron`): the look sets the colour, the light in bands, the flat faces, the outline and the glow; a prefab picks its look with
  the component `"Look": ("hero")`, and the code gives a mesh one white material and `Look::new("stone")` on the mesh itself. Only the
  picture changes (the `render` feature, a window or `--offscreen`), never the gameplay. A `Look` in a body's prefab is an ordinary component (replay,
  snapshot, co-op clients), so a `Look` that only the picture needs is put by the code on a child mesh without `AiId`, not on the
  body; a shared style is a preset (`extends: "low_poly"`); the colour and the outline thickness do not go into game code.

## Foliage

- **Foliage is content** (`*.scatter.ron`): the definition decides what grows where, how densely and in which kinds, and
  the layout comes from the region's seed, the cell and the id through `DetRng` (never `SimRng`). Instances are for the picture only: without `AiId` and
  colliders, under the tile's root; whatever must block walking or mean something in the game is placed in the tile as a prefab or a collider.
  Foliage kinds have no outline (a kind with an outline is a content error: each instance would get its own shell). The ground and
  obstacles for the layout come only from static colliders (`Body::Static`): a platform that moves does not hold the grass.

## Effects

- **Effects are content** (`*.effect.ron`, the event table is one `*.fx.ron`): the effect sets the number of particles, their life, speed, colours and pool, and
  a combat event (`damaged`, `died`, …) or a clip marker picks it from the table, and the game plays it at its own moment with
  `PlayEffect`. Effects and decals are only pictures (the `render` feature, a window or `--offscreen`): they do not change the world, their entities have no
  `AiId`, and the particle clock is the simulation clock (pause freezes it). Effect numbers are not put in game code.
- **A play carries its strength and its push** (`PlayOptions`, `PlayEffect::strength` and `::push`, a cue's `strength`): how an effect answers them (more, faster or bigger particles; the share of the push each takes) is its own content (`strength`, `push`), and its look over a life (`size_ease`, `swell`, `Disc`, `unlit`) too. A play without a push draws no number, so existing effects start the particles they always did; keep new fields off by default.

## Animation

- **Animation is content** (`*.animset.ron`): an animset sets which clip from which glTF file shows a state or an action, the blending and the markers
  (`cues` → sound and effect; a cue may name a `bone` and a `strength`: it plays at the bone's world place, scaled, and is presentation only, D1.7); a Blender script builds the models and clips (`tools/blender/humanoid.py`, only `-b
  --factory-startup`, without a window), and its output is committed. `Animated` and the model are only a picture: the game puts them on the body when
  `config.draws()`; the replay does not check `Animated`, and the gameplay does not depend on the clip. An action's offset (`displacement`) with `bake:
  (animset, clip)` is baked from the clip's root bone: **if you change the clip, bake it again** (`cargo sandbox --bake-motion`, tool
  `motion_bake`); otherwise `--check-content` names the stale curve. Do not edit such a curve by hand. A new bake changes
  the gameplay: the reference replays then diverge and are re-recorded.
- **Sockets, props and clip families are presentation too** (`animset`: `sockets`, `carry`, `mounts`, `by_moveset`; the kind `prop`, `*.prop.ron`; `shards_action::render::props`). The engine spawns the props under the socket's bone, without `AiId`, so they never reach a replay, a snapshot or a client; where a prop sits is read every frame from the action's time and the seconds since a weapon switch (`mount_places`, pure), never from a clip's cue. Do not give a prop an `AiId`, do not read a bone or a prop in `FixedUpdate`, and do not add a presentation component to a body that the replay or the snapshot would carry: follow `Animated` and `PropSwaps` (reflected, `replay_ignore`, `net_ignore`, inserted by the game only where something is drawn). Every new `animset` field serialises only when set (`skip_serializing_if`), because the content hash feeds the co-op handshake: a test per file (`existing_animsets_keep_their_json`) pins the JSON of the sets a change leaves alone. A walk of the skeleton by bone name goes through `render::props::skeleton`, which skips the subtrees below a `PropOf`; a prop's glTF gets no `AnimationPlayer` (`retarget_on_ready` returns below a `PropOf`). A change to `sockets`, `carry` or `mounts` rebuilds nothing (only `clips`, `layers`, layered mappings and `by_moveset` shape the graph); the props go over to a new binding. A game's own attachment (the core crystal on the chest) reads `Sockets`, it does not spawn a prop of its own. The tool `animation id=eN [animset=humanoid]` says what the bridge shows or would show (headless too); the counts are in `ai/status` -> `anim`.
- **Motion modes and the model's own writes (D1.2).** `mounts["@mode:<mode>"]` and `mounts["@left:<mode>"]` are two more clocks of the same pure `mount_places` (the order: cut hold, running action, mode, left, equip, carry socket); their per-body state is `render::modes::ModeState` (advanced once a frame in `drive` from the drawn mode, whether an action runs and the simulation clock, so a pause freezes it and a hold shifts it; the wait, the linger and the drop are pure and tested without Bevy). The mode a body is drawn in is `MotionState::mode` and nothing else; a game that wants another reason to step aside does it in content. `ModeLeft` is a presentation event: observe it for effects, never for gameplay. Anything the bridge writes on the model itself goes on a **bone** in `PostUpdate`, in the `AnimSystems` stages (`Root`, then `Model`: after `AnimationSystems` and `InterpSystems::Blend`, before `Propagate`), never on the model child that carries `Interpolated` (its blend takes any foreign write as the new rest and compounds it); `apply_model` converts metres into the bone's parent space and takes last frame's write off first when the clips did not rewrite the bone. Lengths in the world (drops, later lunges) are metres, lengths on a bone are the rig's units. A layer's `off_in` is read in place (it is not part of the graph's shape).
- **Event overlays (D1.6) are presentation.** The observers in `render::bridge` (`overlay_events`) only note an event on the drawn body's `AnimationBound`; `drive` starts the overlay on the next frame and ages it on the simulation clock (`Time<Fixed>` elapsed plus overstep), so a pause freezes it and a hit stop shifts it. Never make gameplay wait on an overlay or read one in `FixedUpdate`. A new engine event an overlay should answer goes in both `OVERLAY_EVENTS` (`anim.rs`, the names the check and the tool know) and `overlay_events` (its observer, on the body the event names). An overlay's clip plays on its own masked node (`Built::overlay_node`), so it never fights an action's layered node for the same clip; a change to `overlays` reshapes the graph like a layered mapping does.
- **Render interpolation is presentation** (`shards_motion::interp`): a drawn child of a simulated body carries `Interpolated` and is moved in `PostUpdate`
  (`InterpSystems::Blend`) between the body's last two tick poses. Never write a body's `Transform` for the picture's sake (physics reads it), never put
  interpolation state on an entity with an `AiId`, and put a game system that reads where a body is *drawn* (a camera, a marker) after `InterpSystems::Blend`
  and use `shown_translation`. A visual child that must stay at the tick's pose carries `NotInterpolated`. The `graphics.interpolation` setting off must
  give exactly the old picture.
