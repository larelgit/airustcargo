# Combat tools

Read this when you drive a fight through `dodge`, `light_attack`, `block`, `use_item` or `attack`, or react to enemy warnings. Part of the [AGENTS.md](../../AGENTS.md) index.

- Combat tools (`dodge`, `light_attack`, `block`, `use_item`, `attack`) answer at once, under the game's rules: `ok: roll (W)`,
  `ok: queued, heavy starts in 0.10s` (the input waits in the buffer), or an error with the reason and the time (`not enough stamina: 10/20
  (ready in 0.2s)`, `busy: windup heavy 0.50s (ready in 0.90s, when a cancel window opens)`). On an error, run `step`
  for the time the error names, then try again. Inputs that bypass the tools and whose time has run out show up in the journal: `e5 dodge rejected: …`.
- What a press starts can depend on the moment (B3): `observe` and `inspect` show the bound inputs (`light -> spar_finisher on e7`) and the memories (`remembers parried 0.32s`), and the tool's answer names the action it starts (`ok: spar_counter`). A guard with `perfect_needs_press` parries only on a fresh press (0.35 s after the last one): let go and press again, do not hold or mash.
- Enemy warnings reach the journal in advance (`e6 winds up slam (0.50s) ⚠ circle 4m`), and `observe`
  under the `YOU` line says where to leave the zone: `step --until "journal:winds up"`, then `dodge`.
