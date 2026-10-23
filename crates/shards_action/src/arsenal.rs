//! The arsenal: the weapons a body carries in numbered slots, the one in its hands, switching
//! between them, and the calls a pickup makes.
//!
//! A weapon is a moveset ([`MovesetDef`]): what its inputs start is the weapon's behaviour, its
//! `weapon` block ([`WeaponDef`](crate::WeaponDef)) says which charges it brings and which of them is the
//! magazine and the reserve. An [`Arsenal`] on a body holds up to nine of them by slot and says which is
//! in the hands; holding one is the body's [`Fighter::moveset`], changed the way any moveset change is, so
//! the animation bridge's `@moveset` equip rules (draw and holster pictures) see a switch like any other.
//!
//! Inputs: `weapon.slot.N`, `weapon.next`, `weapon.prev` in the body's `Intent` (a game binds keys and the
//! wheel to them; the standard input map binds the wheel to `lock_switch` and leaves these free). A press
//! asks for a switch; the switch waits for the running action's cancel window that names the input
//! `weapon` (or `Any`), or for the action to end, and then cuts the action, changes the moveset and drops the
//! presses that waited for the old weapon. Ammo is the body's [`Charges`]: a magazine and a reserve are two
//! of them; costs, reloads and the refusals are in the action machine (`Cost::charge_count`,
//! `Effect::Refill`, `MovesetDef::auto_reload`).
//!
//! A pickup calls [`grant_weapon`] and [`add_to_charge`]; both say what they took, so a pickup that took
//! nothing can stay on the floor. Everything here is simulation state on `AiId` bodies: reflected, in
//! snapshots and the replay hash.

use ai_core::prelude::*;
use ai_core::{label, AiAppExt, AiId};
use bevy::prelude::*;
use schemars::JsonSchema;
use serde::Deserialize;
use shards_content::{ContentAppExt, ContentDb, PrefabDef, Refs};
use shards_core::input_names::{RELOAD, WEAPON, WEAPON_NEXT, WEAPON_PREV, WEAPON_SLOT};
use shards_core::{Intent, NetRole};

use crate::combat::Downed;
use crate::context::cut_action;
use crate::def::{ActionDef, MovesetDef};
use crate::fighter::{Charge, Charges, Dead, Fighter};
use crate::machine::{ticks_until, ActionRejected, ActionState, InputBuffer, Refilled};

/// The most weapons a body carries: slots are numbered 1 to this, like the number keys.
pub const MAX_SLOTS: u8 = 9;

/// The weapons a body carries and the one in its hands. See the module.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct Arsenal {
    /// The weapons carried, one per slot, by slot number ascending.
    pub slots: Vec<Held>,
    /// The slot whose moveset the body fights with; `None`: none, the body has its `bare` moveset.
    pub current: Option<u8>,
    /// A switch asked for that waits for the running action's cancel window.
    pub pending: Option<u8>,
    /// The moveset the body had before its first weapon: back on the body when it holds none.
    pub bare: Option<String>,
}

/// One weapon in a slot.
#[derive(Reflect, Clone, Debug, PartialEq)]
pub struct Held {
    pub slot: u8,
    pub moveset: String,
}

impl Arsenal {
    /// The moveset in `slot`.
    pub fn weapon(&self, slot: u8) -> Option<&str> {
        self.slots.iter().find(|h| h.slot == slot).map(|h| h.moveset.as_str())
    }

    /// The moveset in the hands.
    pub fn in_hands(&self) -> Option<&str> {
        self.weapon(self.current?)
    }

    /// `1 sidearm, 2 blaster`.
    pub fn list(&self) -> String {
        self.slots.iter().map(|h| format!("{} {}", h.slot, h.moveset)).collect::<Vec<_>>().join(", ")
    }

    /// Whether a weapon is in `slot`.
    fn holds(&self, slot: u8) -> bool {
        self.slots.iter().any(|h| h.slot == slot)
    }
}

/// A body changed the weapon in its hands. Host only; not a net event (the [`Arsenal`] and the body's
/// moveset are replicated).
#[derive(Event, Reflect, Clone, Debug, PartialEq)]
pub struct WeaponSwitched {
    pub entity: Entity,
    pub slot: u8,
    pub moveset: String,
    /// The moveset it held before (`None`: its bare one).
    pub from: Option<String>,
}

/// What a switch request comes to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Switch {
    /// Change to this slot.
    To(u8),
    /// The body holds this slot already and no switch is asked for: nothing to do.
    Already(u8),
}

/// What the input `request` (`weapon.slot.2`, `weapon.next`, `weapon.prev`) asks of `arsenal`: the slot to
/// go to, or why not. `next` and `prev` go on from a switch already asked for, so two presses of `next`
/// go two weapons on. The one rule the machine's system, [`select_slot`] and the `weapon` tool all ask.
pub fn switch_target(arsenal: &Arsenal, request: &str) -> Result<Switch, String> {
    let base = arsenal.pending.or(arsenal.current);
    let target = if request == WEAPON_NEXT || request == WEAPON_PREV {
        let n = arsenal.slots.len();
        if n == 0 {
            return Err("you carry no weapons".into());
        }
        let at = base.and_then(|b| arsenal.slots.iter().position(|h| h.slot == b));
        let to = match (request == WEAPON_NEXT, at) {
            (true, Some(i)) => (i + 1) % n,
            (true, None) => 0,
            (false, Some(i)) => (i + n - 1) % n,
            (false, None) => n - 1,
        };
        arsenal.slots[to].slot
    } else if let Some(n) = request.strip_prefix(WEAPON_SLOT).and_then(|r| r.strip_prefix('.')) {
        let slot: u8 = n.parse().map_err(|_| format!("`{request}`: a slot is a number from 1 to {MAX_SLOTS}"))?;
        if !arsenal.holds(slot) {
            return Err(match arsenal.slots.is_empty() {
                true => format!("no weapon in slot {slot}: you carry none"),
                false => format!("no weapon in slot {slot}; you carry: {}", arsenal.list()),
            });
        }
        slot
    } else {
        return Err(format!("unknown weapon input `{request}`; they are {WEAPON_SLOT}.N, {WEAPON_NEXT} and {WEAPON_PREV}"));
    };
    Ok(if Some(target) == arsenal.current && arsenal.pending.is_none() { Switch::Already(target) } else { Switch::To(target) })
}

/// Whether this machine runs the simulation (the arsenal's calls change nothing on a co-op client).
fn simulates(world: &World) -> bool {
    world.get_resource::<NetRole>().is_none_or(|r| r.simulates())
}

// --- the calls a game's pickups make ------------------------------------------------------

/// What [`grant_weapon`] did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Grant {
    /// A new weapon in the slot.
    Added,
    /// It took the place of another weapon (the old moveset).
    Replaced(String),
    /// The slot held this weapon already: nothing changed (a pickup gives ammo instead).
    AlreadyHeld,
}

/// Puts the weapon `moveset` into `slot` (1 to [`MAX_SLOTS`]) of `body`'s arsenal (made if it has none),
/// and gives the body the charges the weapon brings that it lacks ([`WeaponDef::charges`](crate::WeaponDef)).
/// A body that held nothing takes it into its hands at once; otherwise the game calls [`select_slot`]
/// when it wants the switch. A weapon that replaces the one in the hands replaces it in the hands.
pub fn grant_weapon(world: &mut World, body: Entity, slot: u8, moveset: &str) -> Result<Grant, String> {
    if !simulates(world) {
        return Err("this machine does not simulate: the host grants weapons".into());
    }
    if !(1..=MAX_SLOTS).contains(&slot) {
        return Err(format!("slot {slot}: slots are numbered 1 to {MAX_SLOTS}"));
    }
    if world.get::<Fighter>(body).is_none() {
        return Err("the body is not a fighter: it has no Fighter component".into());
    }
    if world.resource::<ContentDb>().peek::<MovesetDef>(moveset).is_none() {
        let known: Vec<&str> = world.resource::<ContentDb>().peek_ids::<MovesetDef>().collect();
        return Err(format!("unknown moveset `{moveset}`; the movesets are: {}", known.join(", ")));
    }
    let mut arsenal = world.get::<Arsenal>(body).cloned().unwrap_or_default();
    let grant = match arsenal.slots.iter_mut().find(|h| h.slot == slot) {
        Some(held) if held.moveset == moveset => return Ok(Grant::AlreadyHeld),
        Some(held) => Grant::Replaced(std::mem::replace(&mut held.moveset, moveset.to_owned())),
        None => {
            arsenal.slots.push(Held { slot, moveset: moveset.to_owned() });
            arsenal.slots.sort_by_key(|h| h.slot);
            Grant::Added
        }
    };
    world.entity_mut(body).insert(arsenal);
    give_stock(world, body, moveset);
    let held = world.get::<Arsenal>(body).is_some_and(|a| a.current.is_some());
    let replaced_in_hands = world.get::<Arsenal>(body).is_some_and(|a| a.current == Some(slot) && grant != Grant::Added);
    if !held || replaced_in_hands {
        switch_to(world, body, slot, false, held);
    }
    Ok(grant)
}

/// Takes the weapon in `slot` from `body`; its moveset. A weapon in the hands is replaced by the lowest
/// slot left, or by the body's bare moveset when none is (the arsenal goes then; a body that started armed
/// has a bare moveset only if its prefab names one in `bare`, else it keeps the last weapon's moveset). The charges
/// stay: the game decides what becomes of the rounds (`add_to_charge` and a pickup of its own).
pub fn take_weapon(world: &mut World, body: Entity, slot: u8) -> Option<String> {
    if !simulates(world) {
        return None;
    }
    let mut arsenal = world.get::<Arsenal>(body)?.clone();
    let at = arsenal.slots.iter().position(|h| h.slot == slot)?;
    let taken = arsenal.slots.remove(at).moveset;
    let in_hands = arsenal.current == Some(slot);
    if arsenal.pending == Some(slot) {
        arsenal.pending = None;
    }
    if in_hands {
        arsenal.current = None;
    }
    let next = arsenal.slots.first().map(|h| h.slot);
    let bare = arsenal.bare.clone();
    if arsenal.slots.is_empty() {
        world.entity_mut(body).remove::<Arsenal>();
    } else {
        world.entity_mut(body).insert(arsenal);
    }
    if in_hands {
        match next {
            // The weapon that was in the hands just went: the one that comes up replaces it (`from` says so).
            Some(next) => switch_to(world, body, next, true, true),
            None => {
                cut_action(world, body);
                if let (Some(bare), Some(mut fighter)) = (bare, world.get_mut::<Fighter>(body)) {
                    fighter.moveset = bare;
                }
            }
        }
    }
    Some(taken)
}

/// Asks for a switch to the weapon in `slot`: the arsenal's system makes it when the running action
/// allows (nothing runs, or a cancel window names `weapon`). Asking for the slot in the hands lets go of a
/// switch asked for before. `Err` says why not (the slot is empty, this machine does not simulate).
pub fn select_slot(world: &mut World, body: Entity, slot: u8) -> Result<(), String> {
    if !simulates(world) {
        return Err("this machine does not simulate: the host switches weapons".into());
    }
    let mut arsenal = world.get_mut::<Arsenal>(body).ok_or("the body carries no weapons")?;
    match switch_target(&arsenal, &format!("{WEAPON_SLOT}.{slot}"))? {
        Switch::Already(_) => {}
        Switch::To(to) => arsenal.pending = (Some(to) != arsenal.current).then_some(to),
    }
    Ok(())
}

/// Adds up to `n` to the charge `name` of `body` (rounds to a reserve): as many as it has room for.
/// Returns how many it took, so a pickup that took less than it holds can keep the rest (0: full, or
/// the body has no such charge).
pub fn add_to_charge(world: &mut World, body: Entity, name: &str, n: u32) -> u32 {
    if !simulates(world) {
        return 0;
    }
    let Some(mut charges) = world.get_mut::<Charges>(body) else { return 0 };
    let Some(charge) = charges.get_mut(name) else { return 0 };
    let taken = n.min(charge.max.saturating_sub(charge.current));
    if taken > 0 {
        charge.current += taken;
    }
    taken
}

/// The charges the weapon `moveset` brings, the body lacks, and so are added (sorted by name, as
/// [`Charges::new`] keeps them).
fn give_stock(world: &mut World, body: Entity, moveset: &str) {
    let stock: Vec<(String, u32, u32)> = world
        .resource::<ContentDb>()
        .get::<MovesetDef>(moveset)
        .and_then(|m| m.weapon.as_ref())
        .map(|w| w.charges.iter().map(|(name, s)| (name.clone(), s.initial(), s.max)).collect())
        .unwrap_or_default();
    if stock.is_empty() {
        return;
    }
    let mut charges = world.get::<Charges>(body).cloned().unwrap_or_default();
    let before = charges.0.len();
    for (name, current, max) in stock {
        if charges.get(&name).is_none() {
            charges.0.push(Charge { name, current, max });
        }
    }
    if charges.0.len() != before {
        charges.0.sort_by(|a, b| a.name.cmp(&b.name));
        world.entity_mut(body).insert(charges);
    }
}

/// Puts the weapon in `slot` into the hands of `body` now: cuts the running action when `cut`, changes
/// the body's moveset (remembering the one it had as `bare` when it held no weapon before: `had_weapon`
/// is the caller's word for that, since the arsenal may already have let the old weapon go), drops the
/// presses that waited for the old weapon and says so ([`WeaponSwitched`]).
fn switch_to(world: &mut World, body: Entity, slot: u8, cut: bool, had_weapon: bool) {
    let Some(arsenal) = world.get::<Arsenal>(body) else { return };
    let Some(moveset) = arsenal.weapon(slot).map(str::to_owned) else { return };
    if cut {
        cut_action(world, body);
    }
    let Some(mut fighter) = world.get_mut::<Fighter>(body) else { return };
    let was = std::mem::replace(&mut fighter.moveset, moveset.clone());
    if let Some(mut arsenal) = world.get_mut::<Arsenal>(body) {
        if !had_weapon && arsenal.bare.is_none() {
            arsenal.bare = Some(was.clone());
        }
        arsenal.current = Some(slot);
        arsenal.pending = None;
    }
    if let Some(mut buffer) = world.get_mut::<InputBuffer>(body)
        && !buffer.queued.is_empty()
    {
        buffer.queued.clear();
    }
    give_stock(world, body, &moveset);
    if !had_weapon || was != moveset {
        world.trigger(WeaponSwitched { entity: body, slot, moveset, from: had_weapon.then_some(was) });
    }
}

// --- the system ---------------------------------------------------------------------------

/// `ActionSystems::Weapons`, first: a body whose arsenal changed (a prefab spawned it, a game wrote it, a
/// snapshot restored it) gets the charges its weapons bring and the moveset of the weapon in its hands.
/// Idempotent; it overrides nothing a game does to the moveset afterwards.
pub(crate) fn align_arsenals(
    db: Res<ContentDb>,
    mut commands: Commands,
    mut bodies: Query<(Entity, &mut Arsenal, &mut Fighter, Option<&Charges>), Changed<Arsenal>>,
) {
    for (entity, mut arsenal, mut fighter, charges) in &mut bodies {
        // A slot that names nothing, or that the current one points past, is the game's mistake: say it in the checks.
        if arsenal.current.is_some_and(|c| !arsenal.holds(c)) {
            arsenal.current = None;
        }
        if arsenal.pending.is_some_and(|p| !arsenal.holds(p)) {
            arsenal.pending = None;
        }
        if let Some(moveset) = arsenal.in_hands().map(str::to_owned)
            && fighter.moveset != moveset
        {
            if arsenal.bare.is_none() {
                arsenal.bare = Some(fighter.moveset.clone());
            }
            fighter.moveset = moveset;
        }
        let mut owned = charges.cloned().unwrap_or_default();
        let before = owned.0.len();
        for held in &arsenal.slots {
            let Some(weapon) = db.get::<MovesetDef>(&held.moveset).and_then(|m| m.weapon.as_ref()) else { continue };
            for (name, stock) in &weapon.charges {
                if owned.get(name).is_none() {
                    owned.0.push(Charge { name: name.clone(), current: stock.initial(), max: stock.max });
                }
            }
        }
        if owned.0.len() != before {
            owned.0.sort_by(|a, b| a.name.cmp(&b.name));
            commands.entity(entity).insert(owned);
        }
    }
}

/// `ActionSystems::Weapons`, after [`align_arsenals`], before the machine: the `weapon.*` presses of every
/// body with an arsenal, in `AiId` order, ask for switches (a refused one is journaled like any refused
/// input); a switch asked for is made once the running action allows it. A dead or downed body switches
/// nothing and forgets a switch it waited for.
pub(crate) fn run_arsenals(world: &mut World, bodies: &mut QueryState<(Entity, Option<&'static AiId>), With<Arsenal>>) {
    let mut bodies: Vec<(u32, Entity)> = bodies
        .iter(world)
        .map(|(e, id)| (id.map_or(u32::MAX, |i| i.0), e))
        .collect();
    if bodies.is_empty() {
        return;
    }
    bodies.sort();
    for (_, body) in bodies {
        let presses = take_presses(world, body);
        if world.get::<Dead>(body).is_some() || world.get::<Downed>(body).is_some() {
            if let Some(mut arsenal) = world.get_mut::<Arsenal>(body)
                && arsenal.pending.is_some()
            {
                arsenal.pending = None;
            }
            continue;
        }
        for input in presses {
            let Some(arsenal) = world.get::<Arsenal>(body) else { break };
            match switch_target(arsenal, &input) {
                Ok(Switch::Already(_)) => {}
                Ok(Switch::To(slot)) => {
                    if let Some(mut arsenal) = world.get_mut::<Arsenal>(body) {
                        arsenal.pending = (Some(slot) != arsenal.current).then_some(slot);
                    }
                }
                Err(reason) => world.trigger(ActionRejected { entity: body, input, reason }),
            }
        }
        let Some(slot) = world.get::<Arsenal>(body).and_then(|a| a.pending) else { continue };
        if may_switch(world, body) {
            let held = world.get::<Arsenal>(body).is_some_and(|a| a.current.is_some());
            switch_to(world, body, slot, true, held);
        }
    }
}

/// Whether `input` is one an arsenal answers: `weapon.slot.N`, `weapon.next`, `weapon.prev` (a game's other
/// `weapon.…` inputs are its own).
fn is_switch_input(input: &str) -> bool {
    input == WEAPON_NEXT || input == WEAPON_PREV || input.strip_prefix(WEAPON_SLOT).is_some_and(|rest| rest.starts_with('.'))
}

/// The `weapon.*` presses in `body`'s `Intent`, in order, taken out of it.
fn take_presses(world: &mut World, body: Entity) -> Vec<String> {
    let has = world.get::<Intent>(body).is_some_and(|i| i.pressed.iter().any(|p| is_switch_input(&p.input)));
    if !has {
        return Vec::new();
    }
    let Some(mut intent) = world.get_mut::<Intent>(body) else { return Vec::new() };
    let mut taken = Vec::new();
    intent.pressed.retain(|p| {
        let ours = is_switch_input(&p.input);
        if ours {
            taken.push(p.input.clone());
        }
        !ours
    });
    taken
}

/// Whether a switch may be made now: nothing runs, or the running action has a cancel window open (as
/// the machine reads windows for a press) that lets the input `weapon` through.
fn may_switch(world: &World, body: Entity) -> bool {
    let Some(run) = world.get::<ActionState>(body).and_then(|s| s.current.as_ref()) else { return true };
    let Some(db) = world.get_resource::<ContentDb>() else { return true };
    db.get::<ActionDef>(&run.action).is_none_or(|def| def.cancel_into(WEAPON, run.prev, run.t).is_some())
}

/// When a switch asked for now would be made.
enum Wait {
    /// At the start of the next tick.
    Now,
    /// In so many seconds, when the running action's cancel window opens or the action ends.
    In { action: String, secs: f32, window: bool },
    /// Not by waiting alone: the action holds on while an input is held.
    Held { action: String },
}

/// [`Wait`] for `body`'s running action.
fn wait_for_switch(world: &World, body: Entity) -> Wait {
    let Some(run) = world.get::<ActionState>(body).and_then(|s| s.current.clone()) else { return Wait::Now };
    let Some(db) = world.get_resource::<ContentDb>() else { return Wait::Now };
    let Some(def) = db.get::<ActionDef>(&run.action) else { return Wait::Now };
    if def.cancel_into(WEAPON, run.prev, run.t).is_some() {
        return Wait::Now;
    }
    let dt = world.resource::<Time<Fixed>>().timestep().as_secs_f32();
    let held = world.get::<Intent>(body).is_some_and(|i| def.hold.as_ref().is_some_and(|h| i.is_held(&h.input)));
    let ends = ticks_until(run.length - run.t, dt) as f32 * dt;
    let window = def.next_cancel(WEAPON, run.t).map(|(from, _)| ticks_until(from - run.t, dt) as f32 * dt).filter(|w| *w < ends);
    match (window, held) {
        (Some(secs), _) => Wait::In { action: run.action, secs, window: true },
        (None, true) => Wait::Held { action: run.action },
        (None, false) => Wait::In { action: run.action, secs: ends, window: false },
    }
}

// --- what the AI and the HUD see ----------------------------------------------------------

/// What a HUD shows of the weapon in a body's hands ([`weapon_in_hands`]).
#[derive(Clone, Debug, PartialEq)]
pub struct WeaponView {
    pub slot: u8,
    pub moveset: String,
    /// The weapon's `weapon.name` as authored (a Fluent `@id` or plain text), else the moveset's id.
    pub name: String,
    /// The magazine: `(current, max)`.
    pub magazine: Option<(u32, u32)>,
    /// The reserve: what it holds.
    pub reserve: Option<u32>,
}

/// The weapon in `body`'s hands, with its magazine and reserve. Reads content with `peek`: it is for
/// pictures and words, not for the simulation.
pub fn weapon_in_hands(world: &World, body: Entity) -> Option<WeaponView> {
    let arsenal = world.get::<Arsenal>(body)?;
    let slot = arsenal.current?;
    let moveset = arsenal.weapon(slot)?.to_owned();
    let weapon = world.get_resource::<ContentDb>()?.peek::<MovesetDef>(&moveset).and_then(|m| m.weapon.clone()).unwrap_or_default();
    let charges = world.get::<Charges>(body);
    let held = |name: &Option<String>| name.as_deref().and_then(|n| charges.and_then(|c| c.get(n)));
    Some(WeaponView {
        slot,
        name: weapon.name.clone().unwrap_or_else(|| moveset.clone()),
        magazine: held(&weapon.magazine).map(|c| (c.current, c.max)),
        reserve: held(&weapon.reserve).map(|c| c.current),
        moveset,
    })
}

/// `weapon sidearm 6/6 (24) · slots 1 sidearm, 2 blaster · switching to 2`.
fn describe_weapon(world: &World, entity: Entity) -> Option<String> {
    let arsenal = world.get::<Arsenal>(entity)?;
    let mut parts = Vec::new();
    match weapon_in_hands(world, entity) {
        Some(w) => {
            let mut hands = format!("weapon {}", w.moveset);
            match (w.magazine, w.reserve) {
                (Some((cur, max)), Some(reserve)) => hands.push_str(&format!(" {cur}/{max} ({reserve})")),
                (Some((cur, max)), None) => hands.push_str(&format!(" {cur}/{max}")),
                (None, Some(reserve)) => hands.push_str(&format!(" ({reserve})")),
                (None, None) => {}
            }
            parts.push(hands);
        }
        None => parts.push("no weapon in hands".to_owned()),
    }
    parts.push(format!("slots {}", arsenal.list()));
    if let Some(slot) = arsenal.pending {
        parts.push(format!("switching to {slot}"));
    }
    Some(parts.join(" · "))
}

// --- content check ------------------------------------------------------------------------

/// A prefab's `Arsenal`: its slots are numbered 1 to [`MAX_SLOTS`] without repeats, name movesets that exist,
/// and `current` and `pending` name one of the slots.
fn check_prefab(prefab: &PrefabDef, _id: &str, refs: &mut Refs) {
    #[derive(Deserialize)]
    struct HeldRon {
        slot: u8,
        moveset: String,
    }
    #[derive(Deserialize, Default)]
    #[serde(default)]
    struct ArsenalRon {
        slots: Vec<HeldRon>,
        current: Option<u8>,
        pending: Option<u8>,
    }
    for (name, value) in &prefab.components {
        let is = match refs.type_registry() {
            Some(registry) => shards_content::prefab::resolve_component(registry, name)
                .is_ok_and(|reg| reg.type_id() == std::any::TypeId::of::<Arsenal>()),
            None => name == "Arsenal" || name.ends_with("::Arsenal"),
        };
        if !is {
            continue;
        }
        // A value that does not read is the prefab's own error where a type registry reads it; a bare check says so here.
        let arsenal = match ron::from_str::<ArsenalRon>(value.get_ron()) {
            Ok(arsenal) => arsenal,
            Err(_) if refs.type_registry().is_some() => continue,
            Err(e) => {
                refs.error(format!("Arsenal: {e}: it is `(slots: [(slot: 1, moveset: \"sidearm\")], current: Some(1))`"));
                continue;
            }
        };
        for (i, held) in arsenal.slots.iter().enumerate() {
            let at = format!("Arsenal.slots[{i}]");
            if !(1..=MAX_SLOTS).contains(&held.slot) {
                refs.error(format!("{at}.slot {} is outside 1 to {MAX_SLOTS}: slots are numbered like the number keys", held.slot));
            } else if arsenal.slots[..i].iter().any(|other| other.slot == held.slot) {
                refs.error(format!("{at}.slot {} is in the arsenal twice; a slot holds one weapon", held.slot));
            }
            refs.require::<MovesetDef>(&held.moveset, &format!("{at}.moveset"));
        }
        for (field, slot) in [("current", arsenal.current), ("pending", arsenal.pending)] {
            if let Some(slot) = slot
                && !arsenal.slots.iter().any(|h| h.slot == slot)
            {
                refs.error(format!("Arsenal.{field} is slot {slot}, which holds no weapon; the slots are: {}", slots_text(&arsenal.slots)));
            }
        }
    }

    fn slots_text(slots: &[HeldRon]) -> String {
        if slots.is_empty() {
            return "none".to_owned();
        }
        slots.iter().map(|h| format!("{} {}", h.slot, h.moveset)).collect::<Vec<_>>().join(", ")
    }
}

// --- the engine rule ----------------------------------------------------------------------

/// The engine rule `action.arsenal_sane`: an [`Arsenal`] holds weapons by slot, ascending and without
/// repeats, in 1 to [`MAX_SLOTS`]; `current` and `pending` name one of them; and every charge of a body
/// with an arsenal holds between 0 and its maximum.
///
/// `grant_weapon`, `take_weapon`, `select_slot` and the switch keep all of it; a violation names a writer
/// that put the state there by hand (`set`, a prefab, a game system).
pub fn arsenal_sane(world: &World) -> Option<String> {
    let mut bad = ai_core::Offenders::default();
    ai_core::with_query::<(&AiId, &Arsenal, Option<&Charges>), (), _>(world, |query| {
        for (id, arsenal, charges) in query.iter(world) {
            let ascending = arsenal.slots.windows(2).all(|w| w[0].slot < w[1].slot);
            if !ascending || arsenal.slots.iter().any(|h| !(1..=MAX_SLOTS).contains(&h.slot)) {
                bad.note(*id, || format!("Arsenal slots {} are not ascending numbers from 1 to {MAX_SLOTS}", arsenal.list()));
            }
            for (field, slot) in [("current", arsenal.current), ("pending", arsenal.pending)] {
                if let Some(slot) = slot
                    && !arsenal.holds(slot)
                {
                    bad.note(*id, || format!("Arsenal.{field} is slot {slot}, which holds no weapon ({})", arsenal.list()));
                }
            }
            for charge in charges.map_or(&[][..], |c| &c.0[..]) {
                if charge.current > charge.max {
                    bad.note(*id, || format!("charge {} holds {} of at most {}", charge.name, charge.current, charge.max));
                }
            }
        }
    })?;
    bad.report()
}

// --- tools --------------------------------------------------------------------------------

/// Reload the weapon in your hands: the action your moveset binds to `reload` refills the magazine from the
/// reserve when it reaches its end. It answers like an attack does: `ok: sidearm_reload`, or why not (the
/// magazine is full, the reserve is empty, you are busy and when you are ready).
fn reload(In(call): In<Call<NoArgs>>, world: &mut World) -> ToolResult {
    crate::tools::press(world, &call.caller, RELOAD, None, "")
}

/// A weapon slot (`2`), or `next` or `prev`.
#[derive(Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Which {
    Number(u32),
    Name(String),
}

/// Switch weapons: `slot=2` (the weapon in that slot), `slot=next` or `slot=prev` (round the slots). The
/// switch is made when the action you are in reaches a cancel window that allows it, or ends; asking again
/// goes on from the switch asked for.
#[derive(Deserialize, JsonSchema)]
pub struct WeaponArgs {
    pub slot: Which,
}

fn weapon(In(call): In<Call<WeaponArgs>>, world: &mut World) -> ToolResult {
    let me = crate::tools::me(world, &call.caller)?;
    if world.get::<Downed>(me).is_some() {
        return Err(crate::machine::Blocker::Downed.message());
    }
    let request = match call.args.slot {
        Which::Number(n) => format!("{WEAPON_SLOT}.{n}"),
        Which::Name(s) if s == "next" => WEAPON_NEXT.to_owned(),
        Which::Name(s) if s == "prev" => WEAPON_PREV.to_owned(),
        Which::Name(s) => match s.parse::<u32>() {
            Ok(n) => format!("{WEAPON_SLOT}.{n}"),
            Err(_) => return Err(format!("slot `{s}`: give a slot number (1 to {MAX_SLOTS}), `next` or `prev`")),
        },
    };
    let arsenal = world.get::<Arsenal>(me).ok_or("you carry no weapons (your body has no arsenal)")?;
    let slot = match switch_target(arsenal, &request)? {
        Switch::Already(slot) => {
            return Ok(format!("ok: already holding {slot} ({})", arsenal.weapon(slot).unwrap_or_default()));
        }
        Switch::To(slot) => slot,
    };
    let to = format!("{slot} ({})", arsenal.weapon(slot).unwrap_or_default());
    // The wait is judged before the press goes in: the press is taken at the start of the next tick.
    let wait = wait_for_switch(world, me);
    world.get_mut::<Intent>(me).ok_or("no Intent")?.press(&request);
    Ok(match wait {
        Wait::Now => format!("ok: switches to {to}"),
        Wait::In { action, secs, window: true } => {
            format!("ok: switches to {to} in {secs:.2}s, when a cancel window of {action} opens")
        }
        Wait::In { action, secs, window: false } => format!("ok: switches to {to} in {secs:.2}s, when {action} ends"),
        Wait::Held { action } => format!("ok: switches to {to} once {action} lets go (it goes on while its input is held)"),
    })
}

/// Give a body a weapon (dev; what a pickup does): `give_weapon moveset=blaster slot=2`. Without `id`, your
/// body; without `slot`, the first free one. The body gets the charges the weapon brings; one that held
/// nothing takes it into its hands.
#[derive(Deserialize, JsonSchema)]
pub struct GiveWeaponArgs {
    pub id: Option<Id>,
    /// The weapon's moveset (`content_list kind=moveset`).
    pub moveset: String,
    /// The slot, 1 to 9; default the one that holds this weapon already, else the first one free.
    pub slot: Option<u8>,
}

fn give_weapon(In(call): In<Call<GiveWeaponArgs>>, world: &mut World) -> ToolResult {
    let body = target_of(world, &call.caller, call.args.id)?;
    // Without a slot: the one that holds this weapon already, else the first free one.
    let slot = match call.args.slot {
        Some(slot) => slot,
        None => {
            let arsenal = world.get::<Arsenal>(body);
            let held = arsenal.and_then(|a| a.slots.iter().find(|h| h.moveset == call.args.moveset)).map(|h| h.slot);
            let free = arsenal.map_or(Some(1), |a| (1..=MAX_SLOTS).find(|s| !a.holds(*s)));
            held.or(free).ok_or_else(|| format!("{} has all {MAX_SLOTS} slots full; name the slot to replace", label(world, body)))?
        }
    };
    let who = label(world, body);
    Ok(match grant_weapon(world, body, slot, &call.args.moveset)? {
        Grant::Added => format!("{who} carries {} in slot {slot}", call.args.moveset),
        Grant::Replaced(old) => format!("{who} carries {} in slot {slot} (instead of {old})", call.args.moveset),
        Grant::AlreadyHeld => format!("{who} carries {} in slot {slot} already; nothing changed", call.args.moveset),
    })
}

/// Take the weapon in a slot from a body (dev). The rounds stay in its charges.
#[derive(Deserialize, JsonSchema)]
pub struct TakeWeaponArgs {
    pub id: Option<Id>,
    pub slot: u8,
}

fn take_weapon_tool(In(call): In<Call<TakeWeaponArgs>>, world: &mut World) -> ToolResult {
    let body = target_of(world, &call.caller, call.args.id)?;
    let who = label(world, body);
    let slot = call.args.slot;
    take_weapon(world, body, slot).map(|m| format!("{who} no longer carries {m} (slot {slot})")).ok_or_else(|| {
        match world.get::<Arsenal>(body) {
            Some(a) => format!("{who} has no weapon in slot {slot}; it carries: {}", a.list()),
            None => format!("{who} carries no weapons"),
        }
    })
}

/// Add rounds to a charge of a body (dev; what an ammo pickup does): up to its maximum.
#[derive(Deserialize, JsonSchema)]
pub struct GiveAmmoArgs {
    pub id: Option<Id>,
    /// The charge, e.g. `sidearm.reserve`.
    pub charge: String,
    pub n: u32,
}

fn give_ammo(In(call): In<Call<GiveAmmoArgs>>, world: &mut World) -> ToolResult {
    let body = target_of(world, &call.caller, call.args.id)?;
    let who = label(world, body);
    let name = call.args.charge;
    if world.get::<Charges>(body).and_then(|c| c.get(&name)).is_none() {
        let have = world.get::<Charges>(body).map_or_else(String::new, crate::fighter::describe_charges);
        return Err(format!("{who} has no charge `{name}`; it has: {}", if have.is_empty() { "none".to_owned() } else { have }));
    }
    let taken = add_to_charge(world, body, &name, call.args.n);
    let (current, max) = world.get::<Charges>(body).and_then(|c| c.get(&name)).map_or((0, 0), |c| (c.current, c.max));
    let rest = match taken {
        0 => ", it was full".to_owned(),
        t if t < call.args.n => format!(", {} did not fit", call.args.n - t),
        _ => String::new(),
    };
    Ok(format!("{who} {name} +{taken} ({current}/{max}){rest}"))
}

/// The body a dev tool acts on: `id`, else your own.
fn target_of(world: &World, caller: &Caller, id: Option<Id>) -> Result<Entity, String> {
    match id {
        Some(id) => id.entity(world),
        None => caller.require_subject(),
    }
}

pub(crate) fn plugin(app: &mut App) {
    use crate::ActionSystems;
    app.register_type::<Arsenal>()
        .register_type::<Held>()
        .register_type::<WeaponSwitched>()
        .register_type::<Refilled>()
        .register_ai_world_describer("weapon", describe_weapon)
        .add_content_check::<PrefabDef>("arsenal", check_prefab)
        .add_ai_tool("reload", Scope::Player, reload)
        .add_ai_tool("weapon", Scope::Player, weapon)
        .add_ai_tool("give_weapon", Scope::Dev, give_weapon)
        .add_ai_tool("take_weapon", Scope::Dev, take_weapon_tool)
        .add_ai_tool("give_ammo", Scope::Dev, give_ammo)
        .journal_event::<WeaponSwitched>(|e, w| {
            Some(match &e.from {
                Some(from) => format!("{} switches to {} (slot {}, from {from})", label(w, e.entity), e.moveset, e.slot),
                None => format!("{} switches to {} (slot {})", label(w, e.entity), e.moveset, e.slot),
            })
        })
        .journal_event::<Refilled>(|e, w| {
            Some(format!("{} refilled {} +{} ({}/{}), {} {} left", label(w, e.entity), e.charge, e.taken, e.current, e.max, e.left, e.from))
        });
    // Registered up front: the engine rule queries it through `with_query`, which sees nothing while a
    // component in its query is unknown to the world.
    app.world_mut().register_component::<Arsenal>();
    app.add_systems(
        FixedUpdate,
        (align_arsenals, run_arsenals).chain().in_set(ActionSystems::Weapons),
    );
}

#[cfg(test)]
mod tests {
    use shards_content::db::Source;
    use shards_content::{ContentDb, ContentKinds};

    use super::*;

    fn arsenal(slots: &[(u8, &str)], current: Option<u8>, pending: Option<u8>) -> Arsenal {
        Arsenal {
            slots: slots.iter().map(|(slot, moveset)| Held { slot: *slot, moveset: (*moveset).to_owned() }).collect(),
            current,
            pending,
            bare: None,
        }
    }

    #[test]
    fn requests_resolve_against_the_slots() {
        let a = arsenal(&[(1, "pistol"), (3, "shotgun"), (4, "knife")], Some(3), None);
        assert_eq!(switch_target(&a, "weapon.slot.4"), Ok(Switch::To(4)));
        assert_eq!(switch_target(&a, "weapon.slot.3"), Ok(Switch::Already(3)));
        assert_eq!(switch_target(&a, "weapon.next"), Ok(Switch::To(4)));
        assert_eq!(switch_target(&a, "weapon.prev"), Ok(Switch::To(1)));
        assert_eq!(
            switch_target(&a, "weapon.slot.2"),
            Err("no weapon in slot 2; you carry: 1 pistol, 3 shotgun, 4 knife".to_owned()),
            "a gap in the slots is refused, not skipped"
        );
        assert!(switch_target(&a, "weapon.slot.x").unwrap_err().contains("a slot is a number from 1 to 9"));
        assert!(switch_target(&a, "weapon.up").unwrap_err().starts_with("unknown weapon input `weapon.up`; they are weapon.slot.N, weapon.next and weapon.prev"));
        // Round the ends.
        assert_eq!(switch_target(&arsenal(&[(1, "a"), (2, "b")], Some(2), None), "weapon.next"), Ok(Switch::To(1)));
        assert_eq!(switch_target(&arsenal(&[(1, "a"), (2, "b")], Some(1), None), "weapon.prev"), Ok(Switch::To(2)));
        // From nothing: the first, or the last.
        assert_eq!(switch_target(&arsenal(&[(2, "a"), (5, "b")], None, None), "weapon.next"), Ok(Switch::To(2)));
        assert_eq!(switch_target(&arsenal(&[(2, "a"), (5, "b")], None, None), "weapon.prev"), Ok(Switch::To(5)));
        // One weapon: next is that weapon again, nothing to do.
        assert_eq!(switch_target(&arsenal(&[(2, "a")], Some(2), None), "weapon.next"), Ok(Switch::Already(2)));
        assert_eq!(switch_target(&Arsenal::default(), "weapon.next"), Err("you carry no weapons".to_owned()));
        assert_eq!(switch_target(&Arsenal::default(), "weapon.slot.1"), Err("no weapon in slot 1: you carry none".to_owned()));
    }

    #[test]
    fn a_switch_asked_for_is_where_the_next_press_goes_on_from() {
        let a = arsenal(&[(1, "a"), (2, "b"), (3, "c")], Some(1), Some(2));
        assert_eq!(switch_target(&a, "weapon.next"), Ok(Switch::To(3)));
        assert_eq!(switch_target(&a, "weapon.prev"), Ok(Switch::To(1)), "back to the one in the hands: a switch to it, which lets the other go");
        assert_eq!(switch_target(&a, "weapon.slot.1"), Ok(Switch::To(1)));
    }

    fn kinds() -> ContentKinds {
        let mut kinds = ContentKinds::default();
        kinds.register::<crate::ActionDef>();
        kinds.register::<MovesetDef>();
        kinds.register::<PrefabDef>();
        kinds.add_check::<PrefabDef>("arsenal", check_prefab);
        kinds
    }

    fn errors(prefabs: &str) -> Vec<String> {
        let sources = [
            Source { path: "content/a.action.ron".into(), text: r#"{"jab": (net_id: 1, phases: [("a", 0.3)])}"#.into() },
            Source { path: "content/m.moveset.ron".into(), text: r#"{"pistol": (inputs: {"light": "jab"}), "knife": (inputs: {"light": "jab"})}"#.into() },
            Source { path: "content/p.prefab.ron".into(), text: prefabs.into() },
        ];
        ContentDb::build(&kinds(), &sources).errors().map(ToString::to_string).collect()
    }

    #[test]
    fn a_prefab_arsenal_names_movesets_that_exist_in_slots_that_hold_them() {
        let clean = r#"{"p": (components: {"Arsenal": (slots: [(slot: 1, moveset: "pistol"), (slot: 2, moveset: "knife")], current: Some(1))})}"#;
        assert_eq!(errors(clean), Vec::<String>::new());
        let broken = r#"{"p": (components: {"Arsenal": (
            slots: [(slot: 1, moveset: "pistl"), (slot: 1, moveset: "knife"), (slot: 0, moveset: "knife"), (slot: 10, moveset: "pistol")],
            current: Some(2),
            pending: Some(3),
        )})}"#;
        let expected = [
            "content/p.prefab.ron: prefab `p`: Arsenal.slots[0].moveset -> unknown moveset `pistl` (did you mean `pistol`?)",
            "content/p.prefab.ron: prefab `p`: Arsenal.slots[1].slot 1 is in the arsenal twice; a slot holds one weapon",
            "content/p.prefab.ron: prefab `p`: Arsenal.slots[2].slot 0 is outside 1 to 9: slots are numbered like the number keys",
            "content/p.prefab.ron: prefab `p`: Arsenal.slots[3].slot 10 is outside 1 to 9: slots are numbered like the number keys",
            "content/p.prefab.ron: prefab `p`: Arsenal.current is slot 2, which holds no weapon; the slots are: 1 pistl, 1 knife, 0 knife, 10 pistol",
            "content/p.prefab.ron: prefab `p`: Arsenal.pending is slot 3, which holds no weapon; the slots are: 1 pistl, 1 knife, 0 knife, 10 pistol",
        ];
        let errors = errors(broken);
        for line in expected {
            assert!(errors.iter().any(|e| e == line), "missing `{line}` in {errors:#?}");
        }
        assert_eq!(errors.len(), expected.len(), "{errors:#?}");
    }
}
