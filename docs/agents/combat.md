# Combat tools

Read this when you drive a fight through `dodge`, `light_attack`, `block`, `use_item`, `attack` or `lock_on`, or react to enemy warnings. Part of the [AGENTS.md](../../AGENTS.md) index.

- Combat tools (`dodge`, `light_attack`, `block`, `use_item`, `attack`) answer at once, under the game's rules: `ok: roll (W)`,
  `ok: queued, heavy starts in 0.10s` (the input waits in the buffer), or an error with the reason and the time (`not enough stamina: 10/20
  (ready in 0.2s)`, `busy: windup heavy 0.50s (ready in 0.90s, when a cancel window opens)`). On an error, run `step`
  for the time the error names, then try again. Inputs that bypass the tools and whose time has run out show up in the journal: `e5 dodge rejected: …`.
- Enemy warnings reach the journal in advance (`e6 winds up slam (0.50s) ⚠ circle 4m`), and `observe`
  under the `YOU` line says where to leave the zone: `step --until "journal:winds up"`, then `dodge`.
- Lock-on (`lock_on`): with a `lock_on` definition (the sandbox has one, `content/combat/lock.lock_on.ron`) a lock is the
  simulation's, not yours to keep: it goes when the target dies, leaves the range or stays hidden behind the world for 2 s in a row,
  and the journal says why (`e5 lock lost: e6 obstructed 2.0s`); `observe` shows `lock e6 (obstructed 0.8s)` while it is hidden.
  Lock on again after a loss if you still need the facing. `lock_on side=left|right` moves the lock to the nearest target on that
  side seen from your body; `lock_on` without `target` picks by the definition's score among what your body sees, and
  `lock_on target=eN` refuses an ally, a dead or too far target and a body with no position (the error says which). This holds for
  every body an agent drives through `lock_on`, a monster too. A game system that sets `Intent.target` or despawns a body that may
  be locked on to goes before `LockSystems::Check` (`SimSet::Cleanup`, after `CoreSystems::ClearPresses`), or the rule
  `action.lock_on_valid` stops the game.
