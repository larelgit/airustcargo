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

## Animation

- **Animation is content** (`*.animset.ron`): an animset sets which clip from which glTF file shows a state or an action, the blending and the markers
  (`cues` → sound and effect); a Blender script builds the models and clips (`tools/blender/humanoid.py`, only `-b
  --factory-startup`, without a window), and its output is committed. `Animated` and the model are only a picture: the game puts them on the body when
  `config.draws()`; the replay does not check `Animated`, and the gameplay does not depend on the clip. An action's offset (`displacement`) with `bake:
  (animset, clip)` is baked from the clip's root bone: **if you change the clip, bake it again** (`cargo sandbox --bake-motion`, tool
  `motion_bake`); otherwise `--check-content` names the stale curve. Do not edit such a curve by hand. A new bake changes
  the gameplay: the reference replays then diverge and are re-recorded.
