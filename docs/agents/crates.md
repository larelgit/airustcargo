# Crates

Read this when you need to know which crate owns what. Part of the [AGENTS.md](../../AGENTS.md) index.

- Crates: `shards` (the umbrella, the game's only dependency), `shards_core`, `shards_content`, `shards_motion`,
  `shards_action`, `shards_level` (levels from Blender), `shards_world` (regions, generation), `shards_npc` (enemies, navmesh),
  `shards_encounter` (waves of enemies at a place: anchors, runs, wipes and resets), `shards_save` (saves: sections, slots, migrations, copies), `shards_ui` (screens and menus from content), `shards_locale`
  (languages: texts in Fluent, switching on the fly), `shards_audio` (sound: buses, voices, music in layers), `shards_look` (look: light
  in bands, flat faces, outlines from content), `shards_scatter` (foliage and small objects by tile), `shards_voxel` (voxel terrain: chunked block volume,
  palette, generation steps, hash, the `voxel_region` region kind), `shards_fx`
  (effects: particles from pools, telegraph decals, marks on surfaces, debris that lands and stays), `shards_hitstop` (hit stop: the table of holds, the clock hold alone and the pictures-only hold in co-op), `shards_light` (lights from content: types, flicker, the shadow budget; the colour grade of the picture is the camera's, in `shards_motion`), `shards_rules` (rule sets: the mode a world plays, by class and session size, `rules`/`rules_set`), `shards_steam` (Steam: lobbies, relay for co-op, achievements, cloud;
  the umbrella's `steam` feature, off by default), `shards_dev` (developer menu and console), `ai_core`, `shards_script`,
  `shards_cli`, `sandbox` (the engine's arena). The map is in `docs/manual.md`, "Architecture".
