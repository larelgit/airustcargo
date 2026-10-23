# Combat tools

Read this when you drive a fight through `dodge`, `light_attack`, `block`, `use_item`, `attack`, `lock_on`, `reload` or `weapon`, or react to enemy warnings. Part of the [AGENTS.md](../../AGENTS.md) index.

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
- Body parts (R1): a big body (the sandbox's `colossus`) is hit through its parts (`crates/shards_action/src/parts.rs`;
  manual, "Body parts (R1)"). `observe` lists them (`hand of e19 40/60 hp`, `core of e19 off`, `hand of e19 broken`); a hit
  names the part it went through (`e5 -> e19 light_1 at hand -10 (hp 1190)`, the body's loss) and a break is
  `e19 hand broken by e5`. A swing meets the part nearest the middle of its cone, so stand where the part you want is the
  nearest one; a body whose definition says `body_hurtbox: false` takes nothing through its own capsule. A dev agent opens or
  closes a part with `part name=core op=on|off` (without `id`: the body you are locked on, or the body of the point you are
  locked on) and breaks one with `op=break`.
- Lock points (R3): a big body's parts can be lock-on targets of their own, and the colossus is locked on to only through them
  (manual, "Lock points (R3)"). `lock_on` picks a point, `lock_on point=hand` names one (of `target`, else of the body you are
  locked on), and the answer, `observe` and the journal name a point with its body: `ok: locked on e21 (hand of e19)`,
  `lock e21 (hand of e19)`. A swing that overlaps the point you are locked on goes through it, so lock on the part you want
  to hit; `side=left|right` moves between points too. A broken point lets the lock go (`e5 lock released: e21 (hand of
  e19)`; lock on another), and the body's death loses it (`lock lost: … dead`). `attack target=` on the body keeps your lock on
  one of its points.
- Weapons (F4): a weapon is a moveset, its ammo the body's `Charges` (a magazine and a reserve are two: `observe` shows `weapon pistol 6/6 (24) · slots 1 pistol, 2 blaster`; `inspect` the charges). `light_attack` on an empty magazine answers `pistol empty (0/6): reload` (with `pistol empty: no pistol.reserve left` when the reserve is empty too); `reload` starts the reload (`ok: pistol_reload`) and answers `pistol is already full (6/6)` when it would move nothing. The rounds move at one moment of the reload, so a reload cut before it (a stagger, a death, a weapon switch) loads nothing: `step --until "journal:refilled"` waits for the rounds (`e5 refilled pistol +6 (6/6), 6 pistol.reserve left`). A moveset with `auto_reload` reloads on the press that would fire it empty (`ok: blaster_reload`). `weapon slot=2` (or `next`, `prev`) switches: `ok: switches to 2 (blaster)` when nothing runs, else `ok: switches to 2 (blaster) in 0.20s, when a cancel window of pistol_shot opens` (or `when pistol_shot ends`): the running action's cancel window must name the input `weapon`; watch for `e5 switches to blaster (slot 2, from pistol)`. A slot nobody holds is refused with what the body carries. A dev agent gives and takes what a pickup would: `give_weapon moveset=ID [slot=N] [id=eN]`, `take_weapon slot=N`, `give_ammo charge=NAME n=N`. Manual: "Weapons: slots, switching, ammo, reload (F4)".
- Shooting (F3): a gun's shot is a `Fire` effect, instant rays from your eye along your look (pitch included), so aim with `look` (`look at=e7 part=head`, or angles) and press `fire` (`light`; `alt=true` is `heavy`; it answers like an attack, and a weapon that fires nothing is refused with its action's name). Before you fire, `observe` shows `SHOT rifle_shot -> e7 Dummy head 14.8m (x0.88)` (what the centre of the shot meets, and the share of the damage its distance keeps) and `shot_trace` the whole ray (bodies a piercing shot passes, the wall that stops it); both read the world as the next tick's shot will. A recoil `kick` lifts your look after each shot: look again before the next. Pellets spread at random within `spread_deg`, drawn from the tick, so read the journal, not the trace, for what a scatter shot hit: one line per shot, `e5 fires rifle_shot: e7 head -48 hp (hp 52)`, `e5 fires scatter_shot (8 pellets): e7 torso, head -54 hp ×8 (hp 6), wall ×2`, `... e31 Pillar 3.0m` (a wall stopped it), `nothing within 100m`; `step --until "journal:fires"`. Allies and the dead are shot through, walls never. Manual: "Shooting: hitscan and 3D aim (F3)".
- NPC gunfire (F11): an NPC's shot misses by its `aim_error_deg`, widened while it moves (`aim_error_moving`) and with distance (`aim_error_far`), so a strafing gunner hits less; a turret's shot flies along its look with no aim error. A gunner whose plan has `keep_range` holds a distance band and does not fire while an ally or a wall stands between it and you (`npc_schedule` says `no line of fire: e9 in the way`): stepping behind its ally or a pillar stops its fire, and stepping out starts it on the next tick. A turret turns at its content rate within its yaw and pitch limits: past its limit it holds fire (`turret tracking e5 (yaw 60°/60°, at its limit)`). Manual: "Gunfire, hiding, ranged fighters, turrets (F11)".
