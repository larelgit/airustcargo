//! Statuses: buildup, threshold, timed effect. No concrete effects here: burning,
//! cold or bleeding are the game's hooks.
//!
//! Hits (and actions' `Status` effects) add buildup, scaled by the target's
//! [`StatusResist`]. Buildup decays by `decay_per_s`. When it reaches the definition's
//! `threshold` the status becomes active for `duration` seconds: [`StatusApplied`], the
//! game's `on_apply` hook, `on_tick` every tick while it lasts, then [`StatusExpired`]
//! and `on_expire`. At most [`CombatTuning::max_statuses`](crate::CombatTuning) are
//! active at once on a fighter; a further one waits at full buildup (still decaying)
//! for a free slot.
//!
//! ```ron
//! // content/combat/statuses.status.ron
//! { "bleed": (threshold: 100, decay_per_s: 10, duration: 3, effect: "bleed") }
//! ```

use ai_core::AiId;
use bevy::ecs::query::QueryFilter;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::{ContentDb, ContentDef, Refs};
use shards_motion::MoveModifiers;

use crate::body::ProcGates;
use crate::def::{CombatTuning, MovesetDef, EPS};
use crate::fighter::{report_once, Dead, Fighter};

/// A status (content kind `status`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StatusDef {
    /// Buildup that triggers it.
    pub threshold: f32,
    /// Buildup lost per second.
    #[serde(default)]
    pub decay_per_s: f32,
    /// Seconds it stays active.
    pub duration: f32,
    /// The game hook it runs (`add_status_effect`); none: a marker only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub effect: Option<String>,
    /// While active, buildup keeps coming: reaching the threshold again adds a stack and
    /// restarts the duration. Without it, buildup is ignored while active.
    #[serde(default)]
    pub stacks: bool,
    /// While active, multiplies the speed of the afflicted character (0.75: a quarter slower;
    /// a swim too). Several statuses multiply. 1: no change. Read by
    /// [`statuses_to_modifiers`] into `MoveModifiers`.
    #[serde(default = "one", skip_serializing_if = "is_one")]
    pub speed_factor: f32,
    /// While active, no walk or run faster than this (m/s) on the ground; the lowest of all
    /// active statuses counts. Applied before `speed_factor`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub speed_cap: Option<f32>,
    /// While active, the afflicted character cannot walk, jump, glide or climb; gravity and
    /// pushes still move it.
    #[serde(default, skip_serializing_if = "is_false")]
    pub root: bool,
    /// After a proc, buildup that reaches the threshold again within this many seconds is
    /// swallowed (no proc). The gate is a [`ProcGates`](crate::ProcGates) entry on the fighter.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub min_proc_interval: f32,
    /// Buildup decays while the status is active (the old rule). `false`: it decays only
    /// while inactive and keeps building up while active.
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub cool_while_active: bool,
    /// Reaching the threshold while active restarts its duration (no stacks), and buildup
    /// keeps coming while it is active.
    #[serde(default, skip_serializing_if = "is_false")]
    pub refresh: bool,
    /// A proc refused (gate or cap) empties the bar. `false`: it waits at full (the old rule).
    #[serde(default, skip_serializing_if = "is_false")]
    pub reset_on_refuse: bool,
    /// Counts toward the active cap (the old rule). `false`: an instant status (an impulse).
    #[serde(default = "yes", skip_serializing_if = "is_true")]
    pub capped: bool,
}

fn one() -> f32 {
    1.0
}

fn is_one(v: &f32) -> bool {
    *v == 1.0
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn is_true(v: &bool) -> bool {
    *v
}

fn yes() -> bool {
    true
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

impl ContentDef for StatusDef {
    const KIND: &'static str = "status";

    fn validate(&self, _: &str, refs: &mut Refs) {
        if !(self.threshold > 0.0 && self.threshold.is_finite()) {
            refs.error(format!("threshold must be > 0 (is {})", self.threshold));
        }
        for (name, v) in [("decay_per_s", self.decay_per_s), ("duration", self.duration)] {
            if !(v >= 0.0 && v.is_finite()) {
                refs.error(format!("{name} must be a number >= 0 (is {v})"));
            }
        }
        if !(self.min_proc_interval >= 0.0 && self.min_proc_interval.is_finite()) {
            refs.error(format!("min_proc_interval must be a number >= 0 (is {})", self.min_proc_interval));
        }
        for (name, v) in [("speed_factor", Some(self.speed_factor)), ("speed_cap", self.speed_cap)] {
            if v.is_some_and(|v| !(v >= 0.0 && v.is_finite())) {
                refs.error(format!("{name} must be a number >= 0 (is {v:?})"));
            }
        }
    }
}

/// A body that takes statuses but is not slowed by them: the `speed_factor` and `speed_cap` of its
/// active statuses are not added to its `MoveModifiers` (a `root` still is). The buildup, the
/// effect and everything else a status does still happen; `StatusResist` is for a body that
/// should not take the status at all. A game can also leave `speed_factor` unset and contribute
/// the slow itself from a `ModifierSystems::Contribute` system that reads the statuses and the
/// body's kind.
#[derive(Component, Reflect, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[reflect(Component, Default)]
pub struct NoStatusSlow;

/// `ModifierSystems::Statuses`: the active statuses with a `speed_factor`, `speed_cap` or `root`
/// add them to the character's [`MoveModifiers`] (in the order the statuses were applied, so the product is the
/// same every run: that order is part of the snapshot and the replay).
pub fn statuses_to_modifiers<F: QueryFilter + 'static>(
    db: Res<ContentDb>,
    mut characters: Query<(&Statuses, &mut MoveModifiers, Has<NoStatusSlow>), F>,
) {
    for (statuses, mut modifiers, exempt) in &mut characters {
        for entry in statuses.0.iter().filter(|e| e.active > 0.0) {
            let Some(def) = db.get::<StatusDef>(&entry.id) else { continue };
            if def.speed_factor != 1.0 && !exempt {
                modifiers.slow(def.speed_factor);
            }
            if let Some(cap) = def.speed_cap
                && !exempt
            {
                modifiers.cap_at(cap);
            }
            if def.root {
                modifiers.root();
            }
        }
    }
}

/// Buildup and active statuses of one entity, by status id.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct Statuses(pub Vec<StatusEntry>);

#[derive(Reflect, Clone, Debug, PartialEq)]
pub struct StatusEntry {
    pub id: String,
    pub buildup: f32,
    /// The definition's threshold, for showing buildup as a percentage.
    pub threshold: f32,
    /// Seconds left active; 0: building up.
    pub active: f32,
    pub stacks: u32,
}

/// Buildup multipliers by status id (0: immune); a status not listed takes 1.
#[derive(Component, Reflect, Clone, Debug, Default, PartialEq)]
#[reflect(Component, Default)]
pub struct StatusResist(pub Vec<(String, f32)>);

impl StatusResist {
    pub fn of(&self, status: &str) -> f32 {
        self.0.iter().find(|(id, _)| id == status).map_or(1.0, |(_, m)| m.max(0.0))
    }
}

/// What changed in a [`Statuses`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StatusChange {
    Applied(String),
    /// Active through this tick (its `on_tick` runs).
    Ticked(String),
    Expired(String),
}

impl Statuses {
    pub fn get(&self, id: &str) -> Option<&StatusEntry> {
        self.0.iter().find(|e| e.id == id)
    }

    /// Statuses active right now.
    pub fn active(&self) -> usize {
        self.0.iter().filter(|e| e.active > 0.0).count()
    }

    fn entry(&mut self, id: &str, threshold: f32) -> &mut StatusEntry {
        let at = match self.0.binary_search_by(|e| e.id.as_str().cmp(id)) {
            Ok(at) => at,
            Err(at) => {
                self.0.insert(at, StatusEntry { id: id.to_owned(), buildup: 0.0, threshold, active: 0.0, stacks: 0 });
                at
            }
        };
        let entry = &mut self.0[at];
        entry.threshold = threshold;
        entry
    }

    /// Adds `amount` of buildup (resistance already applied) under a limit of `limit`
    /// active statuses. The old rules, with every status counting toward the limit; see
    /// [`Statuses::add_with`] for the proc rules a status definition can ask for.
    pub fn add(&mut self, id: &str, def: &StatusDef, amount: f32, limit: usize) -> Option<StatusChange> {
        if amount <= 0.0 {
            return None;
        }
        let free = self.active() < limit;
        let entry = self.entry(id, def.threshold);
        if entry.active > 0.0 && !def.stacks {
            return None;
        }
        entry.buildup += amount;
        if entry.buildup + EPS < def.threshold {
            return None;
        }
        if entry.active > 0.0 {
            entry.stacks += 1;
        } else if free {
            entry.stacks = 1;
        } else {
            entry.buildup = def.threshold;
            return None;
        }
        entry.active = def.duration.max(EPS);
        entry.buildup = 0.0;
        Some(StatusChange::Applied(id.to_owned()))
    }

    /// [`Statuses::add`] with the proc rules of `def` (B2b), in the order Godot checks them:
    ///
    /// 1. an active status that neither `stacks` nor `refresh`es ignores the buildup (the old rule);
    /// 2. below the threshold: done;
    /// 3. reached while `gated` (a `min_proc_interval` gate is running): refused;
    /// 4. else active: `stacks` adds a stack (the old rule), `refresh` restarts the duration;
    /// 5. else, if `def.capped` and `limit` capped statuses are active already: refused;
    /// 6. else it procs.
    ///
    /// A refusal empties the bar with `reset_on_refuse`, else leaves it at the threshold (the old
    /// rule). `capped` says which statuses count toward the limit, by id (`StatusDef::capped`).
    /// The caller sets the `ProcGates` entry when this returns `Applied` and `min_proc_interval`
    /// is above 0 ([`add_status_buildup`] does).
    pub fn add_with(
        &mut self,
        id: &str,
        def: &StatusDef,
        amount: f32,
        limit: usize,
        gated: bool,
        capped: impl Fn(&str) -> bool,
    ) -> Option<StatusChange> {
        if amount <= 0.0 {
            return None;
        }
        let used = self.0.iter().filter(|e| e.active > 0.0 && e.id != id && capped(&e.id)).count();
        let entry = self.entry(id, def.threshold);
        let active = entry.active > 0.0;
        if active && !def.stacks && !def.refresh {
            return None;
        }
        entry.buildup += amount;
        if entry.buildup + EPS < def.threshold {
            return None;
        }
        let refused = if gated {
            true
        } else if active {
            if def.stacks {
                entry.stacks += 1;
            }
            false
        } else if def.capped && used >= limit {
            true
        } else {
            entry.stacks = 1;
            false
        };
        if refused {
            entry.buildup = if def.reset_on_refuse { 0.0 } else { def.threshold };
            return None;
        }
        entry.active = def.duration.max(EPS);
        entry.buildup = 0.0;
        Some(StatusChange::Applied(id.to_owned()))
    }

    /// One tick: active statuses tick and run out, buildup decays, a status waiting at
    /// full buildup takes a free slot. `def` finds definitions (a status whose
    /// definition is gone just runs out).
    pub fn tick<'d>(
        &mut self,
        def: impl Fn(&str) -> Option<&'d StatusDef>,
        dt: f32,
        limit: usize,
    ) -> Vec<StatusChange> {
        self.tick_with(def, dt, limit, |_| false)
    }

    /// [`Statuses::tick`] with the rules of B2b: a status with `cool_while_active: false` does not
    /// decay while active, a `capped: false` one needs no free slot and does not use one, and one
    /// for which `gated` says its proc is gated does not take its slot from the waiting.
    pub fn tick_with<'d>(
        &mut self,
        def: impl Fn(&str) -> Option<&'d StatusDef>,
        dt: f32,
        limit: usize,
        gated: impl Fn(&str) -> bool,
    ) -> Vec<StatusChange> {
        let mut changes = Vec::new();
        for entry in &mut self.0 {
            if entry.active > 0.0 {
                changes.push(StatusChange::Ticked(entry.id.clone()));
                entry.active -= dt;
                if entry.active <= EPS {
                    entry.active = 0.0;
                    entry.stacks = 0;
                    changes.push(StatusChange::Expired(entry.id.clone()));
                }
            }
            let d = def(&entry.id);
            if entry.active > 0.0 && d.is_some_and(|d| !d.cool_while_active) {
                continue;
            }
            let decay = d.map_or(entry.buildup / dt.max(EPS), |d| d.decay_per_s);
            entry.buildup = (entry.buildup - decay * dt).max(0.0);
        }
        for i in 0..self.0.len() {
            let entry = &self.0[i];
            let Some(d) = def(&entry.id) else { continue };
            if entry.active == 0.0
                && entry.buildup + EPS >= d.threshold
                && !gated(&entry.id)
                && (!d.capped || self.0.iter().filter(|e| e.active > 0.0 && def(&e.id).is_none_or(|d| d.capped)).count() < limit)
            {
                let entry = &mut self.0[i];
                entry.active = d.duration.max(EPS);
                entry.stacks = 1;
                entry.buildup = 0.0;
                changes.push(StatusChange::Applied(entry.id.clone()));
            }
        }
        self.0.retain(|e| e.active > 0.0 || e.buildup > 0.0);
        changes
    }
}

/// `bleed 40% · burning 2.1s`, `burning×2 2.1s`.
pub fn describe_statuses(s: &Statuses) -> String {
    s.0.iter()
        .map(|e| match (e.active > 0.0, e.stacks) {
            (true, n) if n > 1 => format!("{}×{n} {:.1}s", e.id, e.active),
            (true, _) => format!("{} {:.1}s", e.id, e.active),
            (false, _) => format!("{} {:.0}%", e.id, 100.0 * e.buildup / e.threshold.max(EPS)),
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// A status effect hook: the game's code for one moment of a status.
pub type StatusHook = fn(&mut World, Entity);

/// The game's code for one status effect.
#[derive(Clone, Copy)]
pub struct StatusHooks {
    pub on_apply: StatusHook,
    pub on_tick: StatusHook,
    pub on_expire: StatusHook,
}

/// Status effect hooks by effect name ([`ActionAppExt::add_status_effect`](crate::ActionAppExt)).
#[derive(Resource, Default, Clone)]
pub struct StatusEffects(pub(crate) Vec<(String, StatusHooks)>);

impl StatusEffects {
    pub fn get(&self, name: &str) -> Option<StatusHooks> {
        self.0.iter().find(|(n, _)| n == name).map(|(_, h)| *h)
    }

    pub(crate) fn insert(&mut self, name: &str, hooks: StatusHooks) {
        self.0.retain(|(n, _)| n != name);
        self.0.push((name.to_owned(), hooks));
    }
}

/// A status became active on `entity` (`stacks` > 1: it stacked).
#[derive(Event, Clone, Debug, PartialEq)]
pub struct StatusApplied {
    pub entity: Entity,
    pub status: String,
    pub stacks: u32,
}

#[derive(Event, Clone, Debug, PartialEq)]
pub struct StatusExpired {
    pub entity: Entity,
    pub status: String,
}

/// Adds `amount` of status `status` buildup to `entity`, scaled by its
/// [`StatusResist`]; applies it (event, `on_apply`) if it reaches the threshold. An
/// unknown status is reported once and ignored. The proc rules of the definition apply
/// ([`Statuses::add_with`]): the fighter's [`ProcGates`] gate it, its moveset's `max_statuses`
/// (else the tuning's) caps it, and a proc of a status with `min_proc_interval` closes its gate.
pub fn add_status_buildup(world: &mut World, entity: Entity, status: &str, amount: f32) {
    let Some(def) = world.resource::<ContentDb>().get::<StatusDef>(status).cloned() else {
        let text = format!("status `{status}` is not in content; buildup ignored (content_list kind=status)");
        report_once(world, format!("status {status}"), text);
        return;
    };
    let limit = status_limit(world, entity);
    let gated = world.get::<ProcGates>(entity).is_some_and(|g| g.gated(status));
    let resist = world.get::<StatusResist>(entity).map_or(1.0, |r| r.of(status));
    // Only the statuses the fighter holds can count toward the limit: look those up, not the database.
    let uncapped: Vec<String> = {
        let db = world.resource::<ContentDb>();
        world
            .get::<Statuses>(entity)
            .map(|s| s.0.iter().filter(|e| db.get::<StatusDef>(&e.id).is_some_and(|d| !d.capped)).map(|e| e.id.clone()).collect())
            .unwrap_or_default()
    };
    let capped = |id: &str| !uncapped.iter().any(|u| u == id);
    let Some(mut statuses) = world.get_mut::<Statuses>(entity) else { return };
    if let Some(change) = statuses.add_with(status, &def, amount * resist, limit, gated, capped) {
        let changes = vec![change];
        close_gates(world, entity, &changes);
        run_changes(world, entity, changes);
    }
}

/// How many statuses `entity` may have active at once: its moveset's `max_statuses`, else the tuning's.
fn status_limit(world: &World, entity: Entity) -> usize {
    let db = world.resource::<ContentDb>();
    world
        .get::<Fighter>(entity)
        .and_then(|f| db.get::<MovesetDef>(&f.moveset))
        .and_then(|m| m.max_statuses)
        .unwrap_or_else(|| CombatTuning::of(db).max_statuses) as usize
}

/// A proc of a status with a `min_proc_interval` closes its gate for that long.
fn close_gates(world: &mut World, entity: Entity, changes: &[StatusChange]) {
    for change in changes {
        let StatusChange::Applied(id) = change else { continue };
        let Some(secs) = world.resource::<ContentDb>().get::<StatusDef>(id).map(|d| d.min_proc_interval).filter(|s| *s > 0.0)
        else {
            continue;
        };
        match world.get_mut::<ProcGates>(entity) {
            Some(mut g) => g.set(id, secs),
            None => {
                let mut g = ProcGates::default();
                g.set(id, secs);
                world.entity_mut(entity).insert(g);
            }
        }
    }
}

/// Events and game hooks for `changes`, in order.
fn run_changes(world: &mut World, entity: Entity, changes: Vec<StatusChange>) {
    for change in changes {
        let id = match &change {
            StatusChange::Applied(id) | StatusChange::Ticked(id) | StatusChange::Expired(id) => id.clone(),
        };
        let effect = world.resource::<ContentDb>().get::<StatusDef>(&id).and_then(|d| d.effect.clone());
        let hooks = match &effect {
            Some(name) => {
                let hooks = world.resource::<StatusEffects>().get(name);
                if hooks.is_none() {
                    let text =
                        format!("status `{id}`: no effect hook `{name}`; register it with app.add_status_effect");
                    report_once(world, format!("status effect {name}"), text);
                }
                hooks
            }
            None => None,
        };
        match change {
            StatusChange::Applied(status) => {
                let stacks = world.get::<Statuses>(entity).and_then(|s| s.get(&status)).map_or(1, |e| e.stacks);
                world.trigger(StatusApplied { entity, status, stacks });
                if let Some(h) = hooks {
                    (h.on_apply)(world, entity);
                }
            }
            StatusChange::Ticked(_) => {
                if let Some(h) = hooks {
                    (h.on_tick)(world, entity);
                }
            }
            StatusChange::Expired(status) => {
                world.trigger(StatusExpired { entity, status });
                if let Some(h) = hooks {
                    (h.on_expire)(world, entity);
                }
            }
        }
        if world.get_entity(entity).is_err() {
            return;
        }
    }
}

/// `SimSet::Resolve`, after hits: every living entity's statuses tick, in `AiId` order.
/// The dead's are frozen (no hooks run on a corpse); a revive resumes them.
pub fn update_statuses(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    let mut order: Vec<(u32, Entity)> = world
        .query_filtered::<(Entity, Option<&AiId>, &Statuses), Without<Dead>>()
        .iter(world)
        .filter(|(_, _, s)| !s.0.is_empty())
        .map(|(e, id, _)| (id.map_or(u32::MAX, |i| i.0), e))
        .collect();
    order.sort();
    for (_, entity) in order {
        let limit = status_limit(world, entity);
        let gates = world.get::<ProcGates>(entity).cloned().unwrap_or_default();
        // The definitions of the statuses this entity carries now, each by id: a status nobody
        // has is not read (one added to content leaves a session that never meets it as it
        // was). Read at its turn, not before the loop: a hook of an entity before it this tick
        // may have given it a status.
        let Some(statuses) = world.get::<Statuses>(entity) else { continue };
        let db = world.resource::<ContentDb>();
        let defs: Vec<(String, StatusDef)> =
            statuses.0.iter().filter_map(|entry| Some((entry.id.clone(), db.get::<StatusDef>(&entry.id)?.clone()))).collect();
        let def = |id: &str| defs.iter().find(|(d, _)| d == id).map(|(_, d)| d);
        let Some(mut statuses) = world.get_mut::<Statuses>(entity) else { continue };
        let changes = statuses.tick_with(def, dt, limit, |id| gates.gated(id));
        close_gates(world, entity, &changes);
        run_changes(world, entity, changes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bleed() -> StatusDef {
        StatusDef {
            threshold: 100.0,
            decay_per_s: 10.0,
            duration: 1.0,
            effect: None,
            stacks: false,
            speed_factor: 1.0,
            speed_cap: None,
            root: false,
            min_proc_interval: 0.0,
            cool_while_active: true,
            refresh: false,
            reset_on_refuse: false,
            capped: true,
        }
    }

    #[test]
    fn buildup_reaches_the_threshold_and_decays() {
        let mut s = Statuses::default();
        let def = bleed();
        assert_eq!(s.add("bleed", &def, 60.0, 3), None);
        assert_eq!(describe_statuses(&s), "bleed 60%");
        s.tick(|_| Some(&def), 1.0, 3);
        assert_eq!(s.get("bleed").unwrap().buildup, 50.0, "decays 10/s");
        assert_eq!(s.add("bleed", &def, 50.0, 3), Some(StatusChange::Applied("bleed".into())));
        assert_eq!(describe_statuses(&s), "bleed 1.0s");
        assert_eq!(s.add("bleed", &def, 500.0, 3), None, "ignored while active without stacks");
        let changes = s.tick(|_| Some(&def), 0.5, 3);
        assert_eq!(changes, [StatusChange::Ticked("bleed".into())]);
        let changes = s.tick(|_| Some(&def), 0.5, 3);
        assert_eq!(changes, [StatusChange::Ticked("bleed".into()), StatusChange::Expired("bleed".into())]);
        assert!(s.0.is_empty(), "nothing left to track");
    }

    #[test]
    fn stacking_statuses_restart_and_count() {
        let mut s = Statuses::default();
        let def = StatusDef { stacks: true, ..bleed() };
        s.add("burn", &def, 100.0, 3);
        s.tick(|_| Some(&def), 0.5, 3);
        assert_eq!(s.add("burn", &def, 100.0, 3), Some(StatusChange::Applied("burn".into())));
        let e = s.get("burn").unwrap();
        assert_eq!((e.stacks, e.active), (2, 1.0));
        assert_eq!(describe_statuses(&s), "burn×2 1.0s");
    }

    #[test]
    fn the_limit_holds_a_status_at_full_buildup() {
        let mut s = Statuses::default();
        let def = bleed();
        assert!(s.add("a", &def, 100.0, 1).is_some());
        assert_eq!(s.add("b", &def, 150.0, 1), None, "no free slot");
        assert_eq!(s.get("b").unwrap().buildup, 100.0, "capped at the threshold");
        // `a` runs out; `b` (decayed below the threshold meanwhile) waits for more.
        let changes = s.tick(|_| Some(&def), 1.0, 1);
        assert!(changes.contains(&StatusChange::Expired("a".into())));
        assert!(!changes.contains(&StatusChange::Applied("b".into())), "{changes:?}");
        assert_eq!(s.add("b", &def, 10.0, 1), Some(StatusChange::Applied("b".into())));
    }

    fn chill() -> StatusDef {
        StatusDef { min_proc_interval: 2.0, cool_while_active: false, refresh: true, reset_on_refuse: true, duration: 4.0, ..bleed() }
    }

    fn all_capped(_: &str) -> bool {
        true
    }

    #[test]
    fn add_with_the_old_rules_is_add() {
        // The same calls on a definition that asks for nothing new answer alike.
        let def = StatusDef { stacks: true, ..bleed() };
        let (mut old, mut new) = (Statuses::default(), Statuses::default());
        for (id, amount, limit) in [("a", 60.0, 1), ("a", 60.0, 1), ("b", 150.0, 1), ("a", 100.0, 1), ("b", 20.0, 2)] {
            assert_eq!(old.add(id, &def, amount, limit), new.add_with(id, &def, amount, limit, false, all_capped));
        }
        assert_eq!(old, new);
    }

    #[test]
    fn a_gate_refuses_and_a_reset_empties_the_bar() {
        let (def, mut s) = (chill(), Statuses::default());
        assert!(s.add_with("chill", &def, 100.0, 3, false, all_capped).is_some());
        assert_eq!(s.add_with("chill", &def, 100.0, 3, true, all_capped), None, "gated");
        assert_eq!(s.get("chill").unwrap().buildup, 0.0, "reset_on_refuse");
        assert_eq!(s.get("chill").unwrap().active, 4.0, "the running one is untouched");
        // Without reset_on_refuse the refused proc waits at full (the old rule).
        let plain = StatusDef { reset_on_refuse: false, ..chill() };
        let mut s = Statuses::default();
        s.add_with("chill", &plain, 100.0, 3, false, all_capped);
        s.add_with("chill", &plain, 150.0, 3, true, all_capped);
        assert_eq!(s.get("chill").unwrap().buildup, 100.0);
    }

    #[test]
    fn refresh_restarts_and_buildup_comes_while_active() {
        let (def, mut s) = (chill(), Statuses::default());
        s.add_with("chill", &def, 100.0, 3, false, all_capped);
        s.tick_with(|_| Some(&def), 1.0, 3, |_| false);
        assert_eq!(s.add_with("chill", &def, 40.0, 3, false, all_capped), None, "below the threshold: it adds up while active");
        assert_eq!(s.get("chill").unwrap().buildup, 40.0, "and did not cool in the tick before");
        assert_eq!(s.add_with("chill", &def, 60.0, 3, false, all_capped), Some(StatusChange::Applied("chill".into())));
        let e = s.get("chill").unwrap();
        assert_eq!((e.active, e.stacks, e.buildup), (4.0, 1, 0.0));
    }

    #[test]
    fn uncapped_statuses_neither_need_nor_use_a_slot() {
        let jolt = StatusDef { capped: false, duration: 0.0, ..bleed() };
        let (a, mut s) = (bleed(), Statuses::default());
        s.add_with("a", &a, 100.0, 1, false, |_| true);
        assert!(s.add_with("jolt", &jolt, 100.0, 1, false, |id| id != "jolt").is_some(), "the cap is full of `a`");
        // `b` still finds no slot: the jolt is active but does not count.
        let b = bleed();
        assert_eq!(s.add_with("b", &b, 100.0, 1, false, |id| id != "jolt"), None);
        assert_eq!(s.get("b").unwrap().buildup, 100.0);
    }

    #[test]
    fn a_gated_status_waiting_at_full_does_not_take_its_slot() {
        let def = StatusDef { decay_per_s: 0.0, ..bleed() };
        let mut s = Statuses::default();
        // Waiting at full: the cap (0) refused it, the old rule.
        assert_eq!(s.add_with("b", &def, 100.0, 0, false, all_capped), None);
        assert!(s.tick_with(|_| Some(&def), 0.1, 0, |_| false).is_empty(), "still no slot");
        assert!(s.tick_with(|_| Some(&def), 0.1, 1, |_| true).is_empty(), "a slot, but the gate is closed");
        let changes = s.tick_with(|_| Some(&def), 0.1, 1, |_| false);
        assert_eq!(changes, [StatusChange::Applied("b".into())]);
    }

    #[test]
    fn cool_while_active_off_stops_decay_only_while_active() {
        let def = StatusDef { cool_while_active: false, stacks: true, ..bleed() };
        let mut s = Statuses::default();
        s.add_with("b", &def, 100.0, 3, false, all_capped);
        s.add_with("b", &def, 60.0, 3, false, all_capped);
        s.tick_with(|_| Some(&def), 0.5, 3, |_| false);
        assert_eq!(s.get("b").unwrap().buildup, 60.0, "active: it does not cool");
        s.tick_with(|_| Some(&def), 0.5, 3, |_| false); // runs out; cooling resumes in the same tick
        assert_eq!(s.get("b").unwrap().buildup, 55.0);
    }

    /// The entity that gets a status from a hook.
    #[derive(Component)]
    struct Target;

    fn spread(world: &mut World, _: Entity) {
        let target = world.query_filtered::<Entity, With<Target>>().single(world).unwrap();
        add_status_buildup(world, target, "b", 50.0);
    }

    fn quiet(_: &mut World, _: Entity) {}

    #[test]
    fn a_status_a_hook_gives_an_entity_later_in_the_tick_decays_by_its_definition() {
        use shards_content::db::Source;
        use shards_content::{ContentDb, ContentKinds};
        let mut kinds = ContentKinds::default();
        kinds.register::<StatusDef>();
        let text = r#"{
            "a": (threshold: 100, decay_per_s: 0, duration: 1, effect: "spread"),
            "b": (threshold: 100, decay_per_s: 10, duration: 1),
            "c": (threshold: 100, decay_per_s: 10, duration: 1),
        }"#;
        let db = ContentDb::build(&kinds, &[Source { path: "content/s.status.ron".into(), text: text.into() }]);
        assert_eq!(db.errors().count(), 0);
        let mut world = World::new();
        world.insert_resource(db);
        let mut effects = StatusEffects::default();
        effects.insert("spread", StatusHooks { on_apply: spread, on_tick: quiet, on_expire: quiet });
        world.insert_resource(effects);
        let mut time = Time::<()>::default();
        time.advance_by(std::time::Duration::from_millis(100));
        world.insert_resource(time);
        // e1's `a` applies this tick, and its hook gives e2 `b`: e2, after e1 in `AiId` order,
        // carries only `c` when the tick starts.
        let entry = |id: &str, buildup: f32| StatusEntry { id: id.into(), buildup, threshold: 100.0, active: 0.0, stacks: 0 };
        let second = world.spawn((AiId(2), Statuses(vec![entry("c", 50.0)]), Target)).id();
        world.spawn((AiId(1), Statuses(vec![entry("a", 100.0)])));
        update_statuses(&mut world);
        let statuses = world.get::<Statuses>(second).unwrap();
        assert_eq!(statuses.get("b").map(|b| b.buildup), Some(49.0), "decayed 10/s for 0.1 s: {statuses:?}");
        assert_eq!(statuses.get("c").map(|c| c.buildup), Some(49.0));
    }

    #[test]
    fn resistance_scales_buildup() {
        let resist = StatusResist(vec![("bleed".into(), 0.5), ("poison".into(), 0.0)]);
        assert_eq!((resist.of("bleed"), resist.of("poison"), resist.of("burn")), (0.5, 0.0, 1.0));
        let mut s = Statuses::default();
        assert_eq!(s.add("poison", &bleed(), 100.0 * resist.of("poison"), 3), None);
        assert!(s.get("poison").is_none(), "immune: no buildup at all");
    }
}
