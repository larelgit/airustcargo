//! Attacks with telegraphs, dodges with i-frames, damage, deaths, boss phases.
//!
//! Attack numbers are content: [`AttackDef`]s in `assets/content/combat/attacks.attack.ron`,
//! one per [`AttackKind`] (by its snake_case id). Editing the file while the game runs
//! changes the next attack.
//!
//! One-shot inputs are `Intent` presses: `dodge` (with a direction) and an attack's id
//! (`sword`, `claw`, ...). They wait in an [`InputBuffer`] for up to [`INPUT_BUFFER`]
//! seconds until the character can act on them; one that expires is rejected with the
//! reason ([`ActionRejected`]). [`start_blocker`] is the rule for "can it start now, and
//! if not, why and when", shared by [`start_actions`] and the agent tools, so the two
//! never disagree.

use shards::core::input_names::DODGE;
use shards::core::{flat, Intent};
use shards::prelude::*;

use super::{Boss, Player};

#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Eq)]
#[reflect(Component)]
pub enum Team {
    Hero,
    Monster,
}

#[derive(Component, Reflect, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct Health {
    pub current: f32,
    pub max: f32,
}

impl Health {
    pub fn new(max: f32) -> Self {
        Self { current: max, max }
    }
}

#[derive(Component, Reflect, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct Stamina {
    pub current: f32,
    pub max: f32,
    pub regen: f32,
}

/// Collision radius and walking speed.
#[derive(Component, Reflect, Clone, Copy, Debug)]
#[reflect(Component)]
pub struct Body {
    pub radius: f32,
    pub speed: f32,
}

#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(crate = "shards::serde")]
pub enum AttackKind {
    Sword,
    Claw,
    FrostSlash,
    IceSlam,
    FrostNova,
}

impl AttackKind {
    pub const ALL: [AttackKind; 5] =
        [AttackKind::Sword, AttackKind::Claw, AttackKind::FrostSlash, AttackKind::IceSlam, AttackKind::FrostNova];

    /// Its id in content and the name of its press: `frost_slash`.
    pub fn id(self) -> &'static str {
        match self {
            AttackKind::Sword => "sword",
            AttackKind::Claw => "claw",
            AttackKind::FrostSlash => "frost_slash",
            AttackKind::IceSlam => "ice_slam",
            AttackKind::FrostNova => "frost_nova",
        }
    }

    pub fn from_id(id: &str) -> Option<AttackKind> {
        AttackKind::ALL.into_iter().find(|k| k.id() == id)
    }

    /// Its numbers. A kind missing from content (reported when the game starts) does
    /// nothing: no windup, no damage, no reach.
    pub fn def(self, db: &ContentDb) -> &AttackDef {
        db.get::<AttackDef>(self.id()).unwrap_or(&MISSING_ATTACK)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(crate = "shards::serde")]
pub enum Shape {
    /// In front of the attacker.
    Cone { range: f32, half_angle_deg: f32 },
    /// Around the attacker.
    Circle { radius: f32 },
}

/// One attack's numbers, from `content/combat/attacks.attack.ron`.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(crate = "shards::serde", deny_unknown_fields)]
pub struct AttackDef {
    /// Telegraph time before the hit lands (seconds).
    pub windup: f32,
    /// Vulnerable time after the hit (seconds).
    pub recover: f32,
    pub damage: f32,
    pub shape: Shape,
    /// Stamina it costs (characters without stamina ignore it).
    #[serde(default)]
    pub stamina: f32,
}

const MISSING_ATTACK: AttackDef =
    AttackDef { windup: 0.0, recover: 0.0, damage: 0.0, shape: Shape::Circle { radius: 0.0 }, stamina: 0.0 };

impl ContentDef for AttackDef {
    const KIND: &'static str = "attack";

    fn validate(&self, id: &str, refs: &mut Refs) {
        if AttackKind::from_id(id).is_none() {
            let known: Vec<&str> = AttackKind::ALL.iter().map(|k| k.id()).collect();
            refs.error(format!("no attack kind uses this id; the sandbox knows {}", known.join(", ")));
        }
        for (name, value) in [("windup", self.windup), ("recover", self.recover), ("damage", self.damage), ("stamina", self.stamina)] {
            if value.is_nan() || value < 0.0 {
                refs.error(format!("{name} must be a number >= 0, not {value}"));
            }
        }
    }
}

impl AttackDef {
    pub fn reach(&self) -> f32 {
        match self.shape {
            Shape::Cone { range, .. } => range,
            Shape::Circle { radius } => radius,
        }
    }

    /// Long windups are worth a journal line ("e42 winds up IceSlam").
    pub fn is_telegraphed(&self) -> bool {
        self.windup >= 0.5
    }
}

pub const DODGE_TIME: f32 = 0.35;
pub const DODGE_SPEED: f32 = 12.0;
pub const DODGE_STAMINA: f32 = 25.0;
const CORPSE_TIME: f32 = 1.5;

/// How long a one-shot input waits for the character to be able to act (seconds).
pub const INPUT_BUFFER: f32 = 0.2;

/// What a character is doing right now.
#[derive(Component, Reflect, Clone, Copy, Debug, PartialEq, Default)]
#[reflect(Component, Default)]
pub enum Action {
    #[default]
    Idle,
    /// Telegraph: the hit lands when `left` reaches 0. Facing is locked.
    Windup { attack: AttackKind, left: f32 },
    /// After a hit; cannot act.
    Recover { left: f32 },
    /// Moving fast, immune to hits.
    Dodge { dir: Vec3, left: f32 },
    Dead { for_s: f32 },
}

impl Action {
    /// `windup Sword 0.10s`, `recover 0.25s`, `dodge 0.20s`, `dead`; empty when idle.
    pub fn describe(&self) -> String {
        match self {
            Action::Idle => String::new(),
            Action::Windup { attack, left } => format!("windup {attack:?} {left:.2}s"),
            Action::Recover { left } => format!("recover {left:.2}s"),
            Action::Dodge { left, .. } => format!("dodge {left:.2}s"),
            Action::Dead { .. } => "dead".into(),
        }
    }
}

/// A one-shot action a character asks for.
#[derive(Reflect, Clone, Copy, Debug, PartialEq, Eq)]
pub enum Act {
    Dodge,
    Attack(AttackKind),
}

impl std::fmt::Display for Act {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Act::Dodge => "dodge",
            Act::Attack(_) => "attack",
        })
    }
}

/// One-shot inputs taken from `Intent`, waiting until the character can act on them.
/// `Intent` stays pure input; this is game state (and part of replay checks).
#[derive(Component, Reflect, Default, Clone, Copy, Debug, PartialEq)]
#[reflect(Component, Default)]
pub struct InputBuffer {
    /// Dodge direction (zero: forward).
    pub dodge: Option<Buffered<Vec3>>,
    pub attack: Option<Buffered<AttackKind>>,
}

/// An input in the [`InputBuffer`], with the seconds it may still wait.
#[derive(Reflect, Clone, Copy, Debug, PartialEq)]
pub struct Buffered<T> {
    pub input: T,
    pub left: f32,
}

/// Why an action cannot start right now.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Blocker {
    /// Not enough stamina. `ready_in`: seconds until it can start (stamina regenerated
    /// and nothing else in the way); `None` without regeneration.
    Stamina { have: f32, need: f32, ready_in: Option<f32> },
    /// In an action this one cannot cut short (a windup is a commitment; only a dodge
    /// cancels a recovery). `ready_in`: seconds until it can start.
    Busy { action: Action, ready_in: f32 },
    Dead,
}

impl Blocker {
    /// Seconds until the action could start, if nothing else happens; `None`: waiting
    /// will not do.
    pub fn ready_in(&self) -> Option<f32> {
        match self {
            Blocker::Stamina { ready_in, .. } => *ready_in,
            Blocker::Busy { ready_in, .. } => Some(*ready_in),
            Blocker::Dead => None,
        }
    }

    /// The short reason for the journal: `stamina 10/25`, `busy (windup Sword 0.10s)`.
    pub fn reason(&self) -> String {
        match self {
            Blocker::Stamina { have, need, .. } => format!("stamina {have:.0}/{need:.0}"),
            Blocker::Busy { action, .. } => format!("busy ({})", action.describe()),
            Blocker::Dead => "dead".into(),
        }
    }
}

/// The game's rule for starting `act` now: `None` if it can, else what blocks it.
/// A dodge starts from `Idle` or `Recover` (and cancels the recovery), an attack only
/// from `Idle`; both need their stamina. When several things block, the one that lasts
/// longest is reported.
pub fn start_blocker(act: Act, action: &Action, stamina: Option<&Stamina>, db: &ContentDb) -> Option<Blocker> {
    let busy_for = match (act, *action) {
        (_, Action::Dead { .. }) => return Some(Blocker::Dead),
        (Act::Dodge, Action::Idle | Action::Recover { .. }) | (Act::Attack(_), Action::Idle) => 0.0,
        (Act::Dodge, Action::Windup { left, .. }) => left,
        (Act::Attack(_), Action::Windup { attack, left }) => left + attack.def(db).recover,
        (_, Action::Recover { left } | Action::Dodge { left, .. }) => left,
    };
    let need = match act {
        Act::Dodge => DODGE_STAMINA,
        Act::Attack(kind) => kind.def(db).stamina,
    };
    let short = stamina.filter(|s| s.current < need).map(|s| {
        // Regeneration stops while dodging.
        let dodging = if let Action::Dodge { left, .. } = action { *left } else { 0.0 };
        let ready_in = (s.regen > 0.0).then(|| dodging + (need - s.current) / s.regen);
        Blocker::Stamina { have: s.current, need, ready_in }
    });
    let busy = (busy_for > 0.0).then_some(Blocker::Busy { action: *action, ready_in: busy_for });
    match (busy, short) {
        (None, short) => short,
        (busy, None) => busy,
        // Stamina keeps regenerating while busy: whatever takes longer decides.
        (Some(busy), Some(short)) => match short.ready_in() {
            Some(t) if t <= busy_for => Some(busy),
            _ => Some(short),
        },
    }
}

// --- events (journaled by the AI layer) -------------------------------------------

#[derive(Event, Clone, Debug)]
pub struct Telegraph {
    pub attacker: Entity,
    pub attack: AttackKind,
    pub windup: f32,
}

#[derive(Event, Clone, Debug)]
pub struct Damaged {
    pub source: Entity,
    pub target: Entity,
    pub attack: AttackKind,
    pub amount: f32,
    pub hp_left: f32,
}

#[derive(Event, Clone, Debug)]
pub struct Dodged {
    pub source: Entity,
    pub target: Entity,
    pub attack: AttackKind,
}

#[derive(Event, Clone, Debug)]
pub struct Died {
    pub entity: Entity,
    pub killer: Option<Entity>,
}

/// A buffered dodge or attack that never got to start (journaled).
#[derive(Event, Clone, Debug)]
pub struct ActionRejected {
    pub entity: Entity,
    pub action: Act,
    /// `stamina 10/25`, `busy (windup Sword 0.10s)`.
    pub reason: String,
}

#[derive(Event, Clone, Debug)]
pub struct PhaseChanged {
    pub boss: Entity,
    pub from: u8,
    pub to: u8,
}

/// A hit that landed geometrically; i-frames and health are resolved later in the tick.
#[derive(Message, Clone, Debug)]
pub struct Hit {
    pub source: Entity,
    pub target: Entity,
    pub attack: AttackKind,
    pub damage: f32,
}

// --- systems ----------------------------------------------------------------------

fn windup_scale(boss: Option<&Boss>) -> f32 {
    match boss {
        Some(b) if b.phase >= 2 => 0.8,
        _ => 1.0,
    }
}

/// What a buffered input does this tick.
enum Buffer<T> {
    Start(T),
    Wait,
    /// Its time ran out: rejected, for this reason.
    Expired(Blocker),
}

/// Starts the input in `slot` if nothing blocks it; otherwise it waits (losing `dt`)
/// until its time runs out.
fn advance<T: Copy>(slot: &mut Option<Buffered<T>>, blocker: Option<Blocker>, dt: f32) -> Buffer<T> {
    let Some(buffered) = slot.as_mut() else {
        return Buffer::Wait;
    };
    let Some(why) = blocker else {
        return slot.take().map_or(Buffer::Wait, |b| Buffer::Start(b.input));
    };
    if buffered.left > 1e-4 {
        buffered.left -= dt;
        return Buffer::Wait;
    }
    *slot = None;
    Buffer::Expired(why)
}

/// Moves this tick's presses from `Intent` into the `InputBuffer` (the last of each
/// wins) and starts what can start: at once, or as soon as it becomes possible within
/// [`INPUT_BUFFER`]. An input whose time runs out is rejected with the reason
/// ([`ActionRejected`]). Presses that are neither a dodge nor an attack are left to the
/// engine, which drops them at the end of the tick.
pub fn start_actions(
    time: Res<Time>,
    db: Res<ContentDb>,
    mut commands: Commands,
    mut q: Query<(Entity, &mut Intent, &mut InputBuffer, &mut Action, &mut Transform, Option<&mut Stamina>, Option<&Boss>)>,
) {
    let dt = time.delta_secs();
    for (entity, mut intent, mut buffer, mut action, mut tf, mut stamina, boss) in &mut q {
        if !intent.pressed.is_empty() {
            while let Some(press) = intent.take_press(DODGE) {
                buffer.dodge = Some(Buffered { input: press.dir.unwrap_or(Vec3::ZERO), left: INPUT_BUFFER });
            }
            for kind in AttackKind::ALL {
                while intent.take_press(kind.id()).is_some() {
                    buffer.attack = Some(Buffered { input: kind, left: INPUT_BUFFER });
                }
            }
        }
        let rejected = |action: Act, why: Blocker| ActionRejected { entity, action, reason: why.reason() };

        // The dodge first: it may cancel a recovery the attack would wait for.
        match advance(&mut buffer.dodge, start_blocker(Act::Dodge, &action, stamina.as_deref(), &db), dt) {
            Buffer::Start(dir) => {
                if let Some(s) = stamina.as_mut() {
                    s.current -= DODGE_STAMINA;
                }
                let dir = flat(dir).try_normalize().unwrap_or(*tf.forward());
                *action = Action::Dodge { dir, left: DODGE_TIME };
            }
            Buffer::Expired(why) => commands.trigger(rejected(Act::Dodge, why)),
            Buffer::Wait => {}
        }

        let Some(kind) = buffer.attack.map(|b| b.input) else { continue };
        match advance(&mut buffer.attack, start_blocker(Act::Attack(kind), &action, stamina.as_deref(), &db), dt) {
            Buffer::Start(kind) => {
                let spec = kind.def(&db);
                if let Some(s) = stamina.as_mut() {
                    s.current -= spec.stamina;
                }
                if let Some(aim) = intent.aim {
                    let to = aim - tf.translation;
                    face(&mut tf, to);
                }
                let windup = spec.windup * windup_scale(boss);
                *action = Action::Windup { attack: kind, left: windup };
                if spec.is_telegraphed() {
                    commands.trigger(Telegraph { attacker: entity, attack: kind, windup });
                }
            }
            Buffer::Expired(why) => commands.trigger(rejected(Act::Attack(kind), why)),
            Buffer::Wait => {}
        }
    }
}

/// Advances timers; a windup that reaches zero produces `Hit`s.
pub fn tick_actions(
    time: Res<Time>,
    db: Res<ContentDb>,
    mut hits: MessageWriter<Hit>,
    mut actors: Query<(Entity, &mut Action, &Transform, &Team, &Body)>,
) {
    let dt = time.delta_secs();
    // Targets as they were at the start of this phase, so hit order never depends on
    // which attacker the query happens to visit first.
    let targets: Vec<(Entity, Vec3, Team, f32)> = actors
        .iter()
        .filter(|(_, a, ..)| !matches!(a, Action::Dead { .. }))
        .map(|(e, _, tf, team, body)| (e, tf.translation, *team, body.radius))
        .collect();
    for (entity, mut action, tf, team, _) in &mut actors {
        match *action {
            Action::Windup { attack, left } if left - dt <= 0.0 => {
                let spec = attack.def(&db);
                for &(target, pos, tteam, radius) in &targets {
                    if tteam != *team && in_shape(tf, spec.shape, pos, radius) {
                        hits.write(Hit { source: entity, target, attack, damage: spec.damage });
                    }
                }
                *action = Action::Recover { left: spec.recover };
            }
            Action::Windup { attack, left } => *action = Action::Windup { attack, left: left - dt },
            Action::Recover { left } => {
                *action = if left - dt <= 0.0 { Action::Idle } else { Action::Recover { left: left - dt } }
            }
            Action::Dodge { dir, left } => {
                *action = if left - dt <= 0.0 { Action::Idle } else { Action::Dodge { dir, left: left - dt } }
            }
            Action::Dead { for_s } => *action = Action::Dead { for_s: for_s + dt },
            Action::Idle => {}
        }
    }
}

fn in_shape(attacker: &Transform, shape: Shape, point: Vec3, radius: f32) -> bool {
    let to = flat(point - attacker.translation);
    match shape {
        Shape::Circle { radius: r } => to.length() <= r + radius,
        Shape::Cone { range, half_angle_deg } => {
            if to.length() > range + radius {
                return false;
            }
            if to.length() < radius {
                return true;
            }
            let fwd = flat(*attacker.forward()).normalize_or_zero();
            fwd.angle_between(to.normalize_or_zero()).to_degrees() <= half_angle_deg
        }
    }
}

pub fn face(tf: &mut Transform, dir: Vec3) {
    let dir = flat(dir);
    if dir.length_squared() > 1e-6 {
        let target = tf.translation + dir;
        tf.look_at(target, Vec3::Y);
    }
}

/// Applies hits: dodging characters are immune; otherwise damage, maybe death.
pub fn apply_hits(
    mut commands: Commands,
    mut hits: MessageReader<Hit>,
    mut q: Query<(&mut Health, &mut Action)>,
) {
    for hit in hits.read() {
        let Ok((mut health, mut action)) = q.get_mut(hit.target) else { continue };
        match *action {
            Action::Dead { .. } => continue,
            Action::Dodge { .. } => {
                commands.trigger(Dodged { source: hit.source, target: hit.target, attack: hit.attack });
                continue;
            }
            _ => {}
        }
        health.current = (health.current - hit.damage).max(0.0);
        commands.trigger(Damaged {
            source: hit.source,
            target: hit.target,
            attack: hit.attack,
            amount: hit.damage,
            hp_left: health.current,
        });
        if health.current <= 0.0 {
            *action = Action::Dead { for_s: 0.0 };
            commands.trigger(Died { entity: hit.target, killer: Some(hit.source) });
        }
    }
}

/// Phase 2 at half health, however the health got there (hits or a dev `set`).
pub fn boss_phases(mut commands: Commands, mut q: Query<(Entity, &mut Boss, &Health)>) {
    for (entity, mut boss, health) in &mut q {
        if boss.phase == 1 && health.current <= health.max * 0.5 {
            boss.phase = 2;
            commands.trigger(PhaseChanged { boss: entity, from: 1, to: 2 });
        }
    }
}

pub fn regen_stamina(time: Res<Time>, mut q: Query<(&mut Stamina, &Action)>) {
    for (mut s, action) in &mut q {
        if !matches!(action, Action::Dodge { .. } | Action::Dead { .. }) {
            s.current = (s.current + s.regen * time.delta_secs()).min(s.max);
        }
    }
}

/// Monster corpses disappear; the player stays down until revived.
pub fn remove_corpses(mut commands: Commands, q: Query<(Entity, &Action), Without<Player>>) {
    for (entity, action) in &q {
        if let Action::Dead { for_s } = action
            && *for_s >= CORPSE_TIME
        {
            commands.entity(entity).despawn();
        }
    }
}
