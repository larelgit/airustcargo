# Combat tools

Read this when you drive a fight through `dodge`, `light_attack`, `block`, `use_item`, `attack` or `lock_on`, or react to enemy warnings. Part of the [AGENTS.md](../../AGENTS.md) index.

- Combat tools (`dodge`, `light_attack`, `block`, `use_item`, `attack`) answer at once, under the game's rules: `ok: roll (W)`,
  `ok: queued, heavy starts in 0.10s` (the input waits in the buffer), or an error with the reason and the time (`not enough stamina: 10/20
  (ready in 0.2s)`, `busy: windup heavy 0.50s (ready in 0.90s, when a cancel window opens)`). On an error, run `step`
  for the time the error names, then try again. Inputs that bypass the tools and whose time has run out show up in the journal: `e5 dodge rejected: …`.
- What a press starts can depend on the moment (B3): `observe` and `inspect` show the bound inputs (`light -> spar_finisher on e7`) and the memories (`remembers parried 0.32s`), and the tool's answer names the action it starts (`ok: spar_counter`). A guard with `perfect_needs_press` parries only on a fresh press (0.35 s after the last one): let go and press again, do not hold or mash.
- Enemy warnings reach the journal in advance (`e6 winds up slam (0.50s) ⚠ circle 4m`), and `observe`
  under the `YOU` line says where to leave the zone: `step --until "journal:winds up"`, then `dodge`.
- Lock-on (`lock_on`): with a `lock_on` definition (the sandbox has one, `content/combat/lock.lock_on.ron`) a lock is the
  simulation's, not yours to keep: it goes when the target dies, leaves the range or stays hidden behind the world for 2 s in a row,
  and the journal says why (`e5 lock lost: e6 obstructed 2.0s`); `observe` shows `lock e6 (obstructed 0.8s)` while it is hidden.
  Lock on again after a loss if you still need the facing. `lock_on side=left|right` moves the lock to the nearest target on that
  side seen from your body; `lock_on` without `target` picks by the definition's score among what your body sees, and
  `lock_on target=eN` refuses an ally, a dead or too far target and a body with no position (the error says which). A lock that stops being something you may lock on to (an ally set as `Intent.target` by anything else, a co-op client's input included) is let go of on the next check: `e5 lock released: e6`. This holds for
  every body an agent drives through `lock_on`, a monster too. A game system that sets `Intent.target` or despawns a body that may
  be locked on to goes before `LockSystems::Check` (`SimSet::Cleanup`, after `CoreSystems::ClearPresses`), or the rule
  `action.lock_on_valid` stops the game.
- Spawned combat objects (B4): an action's `Spawn` effect lays down projectiles, ground zones and summons (`crates/shards_action/src/objects.rs`; manual, "Spawned combat objects (B4)"). Watch them with the `combat_objects` query (id, owner, kind, action, phase, seconds left), the `objects_live` and `projectile_flying` conditions and the describer lines `projectile of e8 15.0m/s -> e5 0.40s`; their hits are the owner's (`Damaged.source` is the object). An aim that needs a lock-on target and finds none spawns nothing and says so in the journal: `lock_on` first. The caps `max_objects` and `max_objects_per_owner` are content (`combat_tuning`); a capped spawn is a `[warn] e8 spawn capped: …` line. A dead owner's objects go with it.
- Down, not dead (B5): where the game's lethal hook spares a body (the sandbox: a `partner` moveset while another player
  stands), a lethal hit lays it down (`e7 is down (bleeds out in 20.0s)`, `observe`/`inspect` show `downed 14.2s`). A downed
  body starts nothing (`you are down`), hits pass over it (`-0 (hp 0, downed)`) and nobody targets it; `attack` and `lock_on`
  refuse it. To stand an ally up, stand within reach and hold interact (`interact hold=true`) for the revive's time
  (`ok: spar_revive`, then `e7 revived by e5`); letting go, moving away or being hit ends it. Wait with
  `step --until "journal:revived"` or `--until named:downed`. A dev agent's `heal` stands a body up at once and `down id= bleed=`
  lays one down.
