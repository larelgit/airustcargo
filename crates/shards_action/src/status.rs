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
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use shards_content::{ContentDb, ContentDef, Refs};

use crate::def::{CombatTuning, EPS};
use crate::fighter::report_once;

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
    /// active statuses.
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

    /// One tick: active statuses tick and run out, buildup decays, a status waiting at
    /// full buildup takes a free slot. `def` finds definitions (a status whose
    /// definition is gone just runs out).
    pub fn tick<'d>(
        &mut self,
        def: impl Fn(&str) -> Option<&'d StatusDef>,
        dt: f32,
        limit: usize,
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
            let decay = def(&entry.id).map_or(entry.buildup / dt.max(EPS), |d| d.decay_per_s);
            entry.buildup = (entry.buildup - decay * dt).max(0.0);
        }
        for i in 0..self.0.len() {
            let entry = &self.0[i];
            let Some(d) = def(&entry.id) else { continue };
            if entry.active == 0.0 && entry.buildup + EPS >= d.threshold && self.active() < limit {
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
/// unknown status is reported once and ignored.
pub fn add_status_buildup(world: &mut World, entity: Entity, status: &str, amount: f32) {
    let Some(def) = world.resource::<ContentDb>().get::<StatusDef>(status).cloned() else {
        let text = format!("status `{status}` is not in content; buildup ignored (content_list kind=status)");
        report_once(world, format!("status {status}"), text);
        return;
    };
    let limit = CombatTuning::of(world.resource::<ContentDb>()).max_statuses as usize;
    let resist = world.get::<StatusResist>(entity).map_or(1.0, |r| r.of(status));
    let Some(mut statuses) = world.get_mut::<Statuses>(entity) else { return };
    if let Some(change) = statuses.add(status, &def, amount * resist, limit) {
        run_changes(world, entity, vec![change]);
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

/// `SimSet::Resolve`, after hits: every entity's statuses tick, in `AiId` order.
pub fn update_statuses(world: &mut World) {
    let dt = world.resource::<Time>().delta_secs();
    let db = world.resource::<ContentDb>();
    let limit = CombatTuning::of(db).max_statuses as usize;
    let defs: Vec<(String, StatusDef)> = db.iter::<StatusDef>().map(|(id, d)| (id.to_owned(), d.clone())).collect();
    let def = |id: &str| defs.iter().find(|(d, _)| d == id).map(|(_, d)| d);
    let mut order: Vec<(u32, Entity)> = world
        .query::<(Entity, Option<&AiId>, &Statuses)>()
        .iter(world)
        .filter(|(_, _, s)| !s.0.is_empty())
        .map(|(e, id, _)| (id.map_or(u32::MAX, |i| i.0), e))
        .collect();
    order.sort();
    for (_, entity) in order {
        let Some(mut statuses) = world.get_mut::<Statuses>(entity) else { continue };
        let changes = statuses.tick(def, dt, limit);
        run_changes(world, entity, changes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bleed() -> StatusDef {
        StatusDef { threshold: 100.0, decay_per_s: 10.0, duration: 1.0, effect: None, stacks: false }
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

    #[test]
    fn resistance_scales_buildup() {
        let resist = StatusResist(vec![("bleed".into(), 0.5), ("poison".into(), 0.0)]);
        assert_eq!((resist.of("bleed"), resist.of("poison"), resist.of("burn")), (0.5, 0.0, 1.0));
        let mut s = Statuses::default();
        assert_eq!(s.add("poison", &bleed(), 100.0 * resist.of("poison"), 3), None);
        assert!(s.get("poison").is_none(), "immune: no buildup at all");
    }
}
