//! Fighter resources: health, stamina, poise, armor, team, charges, cooldowns.
//!
//! All are plain components a game (or a prefab) puts on a character next to
//! [`Fighter`]. A fighter without [`Stamina`] pays no stamina, one without [`Poise`]
//! never staggers from hits, one without [`Health`] cannot be hurt. Regeneration runs
//! in `SimSet::Resolve`, after hits, so a hit this tick resets its delay first.

use ai_core::journal::{self, JournalKind};
use bevy::ecs::lifecycle::HookContext;
use bevy::ecs::world::DeferredWorld;
use bevy::prelude::*;
use shards_content::ContentDb;
use shards_core::Intent;
use shards_motion::{
    CharacterController, CharacterParams, Collider, LayerMask, MotionMode, MotionState, MoveModifiers, SprintResume,
};

use crate::def::{CombatTuning, EPS};
use crate::machine::{ActionState, InputBuffer};
use crate::status::Statuses;

/// Makes an entity a fighter driven by moveset `moveset` (a content id). Its
/// [`Collider`] becomes a hurtbox (gains [`LayerMask::HURTBOX`]): hits find it there.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
#[require(ActionState, InputBuffer, Cooldowns, Statuses, Intent)]
#[component(on_add = hurtbox_on_add)]
pub struct Fighter {
    pub moveset: String,
}

impl Fighter {
    pub fn new(moveset: impl Into<String>) -> Self {
        Self { moveset: moveset.into() }
    }
}

fn hurtbox_on_add(mut world: DeferredWorld, ctx: HookContext) {
    if let Some(mut collider) = world.get_mut::<Collider>(ctx.entity) {
        make_hurtbox(&mut collider);
    }
}

/// A collider added after [`Fighter`] (prefabs insert components one by one) becomes a
/// hurtbox too.
pub(crate) fn hurtbox_on_collider(add: On<Insert<Collider>>, mut colliders: Query<&mut Collider, With<Fighter>>) {
    if let Ok(mut collider) = colliders.get_mut(add.entity) {
        make_hurtbox(&mut collider);
    }
}

fn make_hurtbox(collider: &mut Collider) {
    if !collider.layers.memberships.contains(LayerMask::HURTBOX) {
        collider.layers.memberships |= LayerMask::HURTBOX;
    }
}

#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

impl Health {
    pub fn new(max: f32) -> Self {
        Self { current: max, max }
    }
}

impl Default for Health {
    fn default() -> Self {
        Self::new(100.0)
    }
}

/// Spent by actions, blocks and sprinting; comes back `regen` per second once
/// `delay` seconds passed since it was last spent.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Stamina {
    pub current: f32,
    pub max: f32,
    pub regen: f32,
    pub delay: f32,
    /// Seconds since it was last spent (stops counting at `delay`).
    pub since_use: f32,
    /// Sprinting ran it dry: no sprint until the button is let go. Without this the
    /// first regenerated drop would be sprinted away again, over and over.
    pub exhausted: bool,
}

impl Stamina {
    pub fn new(max: f32, regen: f32, delay: f32) -> Self {
        Self { current: max, max, regen, delay, since_use: delay, exhausted: false }
    }

    /// Takes `amount` and restarts the regeneration delay.
    pub fn spend(&mut self, amount: f32) {
        self.current = (self.current - amount).max(0.0);
        self.since_use = 0.0;
    }

    /// Whether a sprint is held back right now: the bar is empty, or the lock a dry sprint set
    /// has not opened under `rule` (see [`SprintResume`]). `held`: the sprint button is down.
    /// The host's `settle_exhaustion` and a client's prediction agree through this.
    pub fn sprint_blocked(&self, rule: SprintResume, held: bool) -> bool {
        self.current <= EPS
            || match rule {
                SprintResume::OnRelease => self.exhausted && held,
                SprintResume::Fraction(f) if f > 0.0 => self.exhausted && self.current < f * self.max,
                // No lock under this rule: only an empty bar stops the sprint.
                SprintResume::Fraction(_) => false,
            }
    }

    /// Opens the lock a dry sprint set, when `rule` says it opens: the button let go
    /// (`OnRelease`), or the bar back at its fraction (`Fraction`, held or not). A `Fraction(0)`
    /// rule never consults the lock, so it leaves one set under another rule (a fast swim's) alone.
    pub fn settle_exhaustion(&mut self, rule: SprintResume, held: bool) {
        if !self.exhausted {
            return;
        }
        match rule {
            SprintResume::OnRelease => self.exhausted = held,
            SprintResume::Fraction(f) if f > 0.0 => self.exhausted = self.current < f * self.max,
            SprintResume::Fraction(_) => {}
        }
    }
}

impl Default for Stamina {
    fn default() -> Self {
        Self::new(100.0, 30.0, 0.8)
    }
}

/// Stagger resistance. Poise damage lowers it; at 0 it breaks: the fighter is
/// forced into its moveset's `stagger` action and the current one is interrupted.
/// While broken further poise damage is ignored; when the stagger ends it is full
/// again. It regenerates `regen` per second once `delay` seconds passed since the
/// last hit.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct Poise {
    pub current: f32,
    pub max: f32,
    pub regen: f32,
    pub delay: f32,
    /// Seconds since the last poise damage (stops counting at `delay`).
    pub since_hit: f32,
    /// Seconds left broken; 0: not broken.
    pub broken: f32,
}

impl Poise {
    pub fn new(max: f32, regen: f32, delay: f32) -> Self {
        Self { current: max, max, regen, delay, since_hit: delay, broken: 0.0 }
    }

    pub fn is_broken(&self) -> bool {
        self.broken > 0.0
    }

    /// Whole again: full poise, no break running (the `heal` tool, a revival).
    pub fn refill(&mut self) {
        (self.current, self.broken) = (self.max, 0.0);
    }
}

impl Default for Poise {
    fn default() -> Self {
        Self::new(50.0, 10.0, 2.0)
    }
}

/// Damage reduction after guards: `percent` (0..1) first, then `flat`.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct Armor {
    pub percent: f32,
    pub flat: f32,
}

impl Armor {
    /// Damage left of `damage`.
    pub fn reduce(&self, damage: f32) -> f32 {
        (damage * (1.0 - self.percent.clamp(0.0, 1.0)) - self.flat.max(0.0)).max(0.0)
    }
}

/// Hits land only between different teams; an entity without a team is hit by all.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Eq, Hash, Default)]
#[reflect(Component, Default)]
pub struct Team(pub String);

impl Team {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

/// Whether `a` may hit `b`.
pub fn hostile(a: Option<&Team>, b: Option<&Team>) -> bool {
    match (a, b) {
        (Some(a), Some(b)) => a != b,
        _ => true,
    }
}

/// Named consumables an action's cost can take (`cost: (charge: "flask")`).
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct Charges(pub Vec<Charge>);

#[derive(Reflect, Clone, Debug, PartialEq)]
pub struct Charge {
    pub name: String,
    pub current: u32,
    pub max: u32,
}

impl Charges {
    /// Full charges: `Charges::new([("flask", 3)])`.
    pub fn new<'a>(charges: impl IntoIterator<Item = (&'a str, u32)>) -> Self {
        let mut list: Vec<Charge> =
            charges.into_iter().map(|(name, max)| Charge { name: name.to_owned(), current: max, max }).collect();
        list.sort_by(|a, b| a.name.cmp(&b.name));
        Self(list)
    }

    pub fn get(&self, name: &str) -> Option<&Charge> {
        self.0.iter().find(|c| c.name == name)
    }

    pub fn get_mut(&mut self, name: &str) -> Option<&mut Charge> {
        self.0.iter_mut().find(|c| c.name == name)
    }
}

/// Actions waiting for their cooldown, with the seconds left.
#[derive(Component, Reflect, Clone, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct Cooldowns(pub Vec<Cooldown>);

#[derive(Reflect, Clone, Debug, PartialEq)]
pub struct Cooldown {
    pub action: String,
    pub left: f32,
}

impl Cooldowns {
    /// Seconds before `action` may start again (0: ready).
    pub fn left(&self, action: &str) -> f32 {
        self.0.iter().find(|c| c.action == action).map_or(0.0, |c| c.left)
    }

    pub fn start(&mut self, action: &str, secs: f32) {
        if secs <= 0.0 {
            return;
        }
        match self.0.iter_mut().find(|c| c.action == action) {
            Some(c) => c.left = secs,
            None => {
                self.0.push(Cooldown { action: action.to_owned(), left: secs });
                self.0.sort_by(|a, b| a.action.cmp(&b.action));
            }
        }
    }
}

/// Health ran out. Dead fighters start no action, take no hit and do not move.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct Dead;

/// Dev god mode: hits do nothing (`god` tool).
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub struct Invulnerable;

/// The rule that opens a sprint's stamina lock for a fighter now: the params' for a fast swim, the
/// combat tuning's for a sprint on land.
pub(crate) fn sprint_rule(
    db: &ContentDb,
    motion: Option<&MotionState>,
    body: Option<(&CharacterController, Option<&CharacterParams>)>,
) -> SprintResume {
    if motion.is_some_and(|m| m.mode == MotionMode::Water) {
        return body.map_or(SprintResume::OnRelease, |(c, own)| c.resolve(own, Some(db)).swim_fast_resume);
    }
    db.get::<CombatTuning>(CombatTuning::ID).map_or(SprintResume::OnRelease, |t| t.sprint_resume)
}

/// What [`update_fighters`] reads and changes on a fighter.
type Resources<'a> = (
    Option<&'a mut Stamina>,
    Option<&'a mut Poise>,
    &'a mut Cooldowns,
    Option<&'a MotionState>,
    Has<Dead>,
    Option<&'a MoveModifiers>,
    Option<(&'a CharacterController, Option<&'a CharacterParams>)>,
);

/// `SimSet::Resolve`, after hits: sprinting drains stamina (`sprint_stamina_per_s` times the
/// character's `MoveModifiers::sprint_cost`; a fast swim drains the params' `swim_fast_drain`
/// instead), then stamina and poise regenerate after their delays, broken poise recovers,
/// cooldowns run down. A free sprint (cost 0) still holds the regeneration back.
pub fn update_fighters(time: Res<Time>, db: Res<ContentDb>, mut fighters: Query<Resources, With<Fighter>>) {
    let dt = time.delta_secs();
    let drain = CombatTuning::of(&db).sprint_stamina_per_s;
    for (stamina, poise, mut cooldowns, motion, dead, modifiers, body) in &mut fighters {
        if let Some(mut s) = stamina {
            if motion.is_some_and(|m| m.sprinting) && !dead {
                let swimming = motion.is_some_and(|m| m.mode == MotionMode::Water);
                let (amount, costly) = if swimming {
                    let rate = body.map_or(0.0, |(c, own)| c.resolve(own, Some(&*db)).swim_fast_drain);
                    (rate * dt, rate > 0.0)
                } else {
                    match modifiers.map_or(1.0, |m| m.sprint_cost) {
                        1.0 => (drain * dt, true),
                        cost => (drain * cost * dt, cost > 0.0),
                    }
                };
                s.spend(amount);
                // A sprint that costs nothing cannot run the stamina dry; a rule with no lock sets none.
                s.exhausted |= costly && s.current <= EPS && sprint_rule(&db, motion, body).latches();
            } else {
                regen_stamina(&mut s, dt);
            }
        }
        if let Some(mut p) = poise {
            regen_poise(&mut p, dt);
        }
        if !cooldowns.0.is_empty() {
            for c in &mut cooldowns.0 {
                c.left -= dt;
            }
            cooldowns.0.retain(|c| c.left > EPS);
        }
    }
}

/// One tick of stamina regeneration: none until `delay` passed since it was spent.
pub fn regen_stamina(s: &mut Stamina, dt: f32) {
    if s.since_use + EPS >= s.delay && s.current < s.max {
        s.current = (s.current + s.regen * dt).min(s.max);
    }
    s.since_use = (s.since_use + dt).min(s.delay);
}

/// One tick of poise: a broken one waits out its stagger and comes back full; a
/// whole one regenerates after `delay`.
pub fn regen_poise(p: &mut Poise, dt: f32) {
    if p.broken > 0.0 {
        p.broken -= dt;
        if p.broken <= EPS {
            p.broken = 0.0;
            p.current = p.max;
        }
        return;
    }
    if p.since_hit + EPS >= p.delay && p.current < p.max {
        p.current = (p.current + p.regen * dt).min(p.max);
    }
    p.since_hit = (p.since_hit + dt).min(p.delay);
}

/// `hp 64/100`.
pub fn describe_health(h: &Health) -> String {
    format!("hp {:.0}/{:.0}", h.current, h.max)
}

/// `stam 30/80`, `stam 40/80 exhausted (release sprint)`.
pub fn describe_stamina(s: &Stamina) -> String {
    let note = if s.exhausted { " exhausted (release sprint)" } else { "" };
    format!("stam {:.0}/{:.0}{note}", s.current, s.max)
}

/// `poise 30/60`, `poise broken 0.8s`.
pub fn describe_poise(p: &Poise) -> String {
    if p.is_broken() {
        format!("poise broken {:.1}s", p.broken)
    } else {
        format!("poise {:.0}/{:.0}", p.current, p.max)
    }
}

/// `flask 2/3`.
pub fn describe_charges(c: &Charges) -> String {
    c.0.iter().map(|c| format!("{} {}/{}", c.name, c.current, c.max)).collect::<Vec<_>>().join("  ")
}

/// `cooldown heavy 1.2s`.
pub fn describe_cooldowns(c: &Cooldowns) -> String {
    c.0.iter().map(|c| format!("cooldown {} {:.1}s", c.action, c.left)).collect::<Vec<_>>().join("  ")
}

/// Journals `text` as an engine error once per `key` (content mistakes found at run
/// time, like an effect hook nobody registered).
pub(crate) fn report_once(world: &mut World, key: String, text: String) {
    let mut reported = world.get_resource_or_init::<Reported>();
    if reported.0.contains(&key) {
        return;
    }
    reported.0.push(key);
    journal::emit(world, JournalKind::Error, text);
}

#[derive(Resource, Default)]
pub(crate) struct Reported(Vec<String>);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stamina_waits_for_its_delay() {
        let dt = 0.1;
        let mut s = Stamina::new(100.0, 10.0, 0.3);
        s.spend(50.0);
        let mut seen = Vec::new();
        for _ in 0..6 {
            regen_stamina(&mut s, dt);
            seen.push(s.current);
        }
        // Spent at 0: no regeneration until 0.3 s have passed.
        assert_eq!(&seen[..3], &[50.0, 50.0, 50.0]);
        assert!((seen[3] - 51.0).abs() < 1e-4 && (seen[5] - 53.0).abs() < 1e-4, "{seen:?}");
    }

    #[test]
    fn broken_poise_comes_back_full() {
        let mut p = Poise::new(40.0, 10.0, 1.0);
        p.current = 0.0;
        p.broken = 0.25;
        regen_poise(&mut p, 0.1);
        regen_poise(&mut p, 0.1);
        assert!(p.is_broken() && p.current == 0.0);
        regen_poise(&mut p, 0.1);
        assert!(!p.is_broken() && p.current == 40.0);
    }

    /// A sprint dry at 100 max stamina: what each resume rule says, tick by tick.
    #[test]
    fn a_dry_sprint_opens_by_its_rule() {
        let dry = |exhausted| Stamina { current: 0.0, exhausted, ..Stamina::new(100.0, 30.0, 0.8) };
        // Today's rule: only the button's release opens it; any stamina is then enough.
        let mut s = dry(true);
        s.current = 60.0;
        assert!(s.sprint_blocked(SprintResume::OnRelease, true), "held: still locked, however full");
        s.settle_exhaustion(SprintResume::OnRelease, true);
        assert!(s.exhausted);
        s.settle_exhaustion(SprintResume::OnRelease, false);
        assert!(!s.exhausted && !s.sprint_blocked(SprintResume::OnRelease, true));
        assert!(dry(false).sprint_blocked(SprintResume::OnRelease, false), "an empty bar always blocks");

        // A quarter lock (a fast swim): 24.9 is blocked held or not, 25 opens, letting go does nothing.
        let quarter = SprintResume::Fraction(0.25);
        let mut s = dry(true);
        for current in [0.0, 12.0, 24.9] {
            s.current = current;
            s.settle_exhaustion(quarter, false);
            assert!(s.exhausted && s.sprint_blocked(quarter, true) && s.sprint_blocked(quarter, false), "{current}");
        }
        s.current = 25.0;
        assert!(!s.sprint_blocked(quarter, true), "the prediction opens at the same point");
        s.settle_exhaustion(quarter, true);
        assert!(!s.exhausted);

        // No lock (a sprint on land): an empty bar stops it, the first drop of stamina resumes it,
        // and a lock left by another rule is neither consulted nor cleared.
        let none = SprintResume::Fraction(0.0);
        assert!(!none.latches() && quarter.latches() && SprintResume::OnRelease.latches());
        let mut s = dry(true);
        assert!(s.sprint_blocked(none, true));
        s.current = 0.5;
        assert!(!s.sprint_blocked(none, true));
        s.settle_exhaustion(none, true);
        assert!(s.exhausted, "a fast swim's lock survives a walk on land");
    }

    #[test]
    fn armor_and_teams() {
        assert_eq!(Armor { percent: 0.5, flat: 2.0 }.reduce(20.0), 8.0);
        assert_eq!(Armor { percent: 0.0, flat: 30.0 }.reduce(20.0), 0.0);
        let (a, b) = (Team::new("heroes"), Team::new("monsters"));
        assert!(hostile(Some(&a), Some(&b)) && !hostile(Some(&a), Some(&a)) && hostile(None, Some(&a)));
    }

    #[test]
    fn cooldowns_and_charges_by_name() {
        let mut c = Cooldowns::default();
        c.start("heavy", 1.5);
        c.start("bash", 0.5);
        c.start("none", 0.0);
        assert_eq!(c.0.iter().map(|c| c.action.as_str()).collect::<Vec<_>>(), ["bash", "heavy"]);
        assert_eq!(c.left("heavy"), 1.5);
        assert_eq!(describe_cooldowns(&c), "cooldown bash 0.5s  cooldown heavy 1.5s");
        let charges = Charges::new([("flask", 3), ("bomb", 1)]);
        assert_eq!(describe_charges(&charges), "bomb 1/1  flask 3/3");
    }
}
