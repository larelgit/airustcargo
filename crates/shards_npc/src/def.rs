//! Content: how NPCs behave, perceive, share attacks and choose them.
//!
//! Five kinds, all hot-reloadable, all numbers a game tunes without a rebuild:
//!
//! - `behavior` ([`BehaviorDef`]): a state machine. States name a **state hook** (`kind`,
//!   engine-provided like `chase` or the game's own) with entry parameters; transitions are
//!   guarded by **named conditions** with parameters. The logic behind the names is code
//!   (`add_npc_condition`, `add_npc_state`), the wiring is data.
//! - `perception` ([`PerceptionDef`]): sight range, field of view, eye height, hearing,
//!   memory.
//! - `director` ([`DirectorDef`]): attack tokens by number of players, the collision budget,
//!   which attack tags may overlap, what those without a token do.
//! - `attack_plan` ([`AttackPlanDef`]): the attacks an NPC chooses from (weights, distance
//!   windows, cooldowns, tags, costs) and the sequences it never plays.
//! - `npc_tuning` ([`NpcTuning`], id `default`): engine-wide numbers (noise radii, avoidance
//!   between bodies ([`AvoidanceDef`]), route following); a behaviour may bring its own
//!   avoidance.
//!
//! ```ron
//! // content/npc/grunt.behavior.ron
//! {
//!     "grunt": (
//!         initial: "idle",
//!         states: {
//!             "idle": (transitions: [(to: "chase", when: ["target_seen"])]),
//!             "chase": (params: {"within": 3.0}, transitions: [
//!                 (to: "home", when: [("!target_in_leash", {"margin": 2.0})]),
//!                 (to: "fight", when: [("target_within", {"meters": 4.0})]),
//!                 (to: "idle", when: ["target_lost"]),
//!             ]),
//!             "fight": (kind: "attack", transitions: [(to: "chase", when: [("!target_within", {"meters": 6.0})])]),
//!             "home": (kind: "return_home", params: {"heal": 20.0}, transitions: [(to: "idle", when: ["at_home"])]),
//!         },
//!     ),
//! }
//! ```

use std::collections::BTreeMap;
use std::fmt;

use serde::de::Deserializer;
use serde::ser::{SerializeTuple, Serializer};
use serde::{Deserialize, Serialize};
use shards_action::{ActionDef, MovesetDef};
use shards_content::{ContentDb, ContentDef, Refs};

/// One parameter value: a flag, a number or a word.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Param {
    Flag(bool),
    Num(f32),
    Text(String),
}

/// Named parameters of a state or a condition: `{"meters": 3.0, "speed": 0.5}`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(transparent)]
pub struct Params(pub BTreeMap<String, Param>);

impl Params {
    /// Number `key`, or `default` when it is missing or not a number.
    pub fn num(&self, key: &str, default: f32) -> f32 {
        match self.0.get(key) {
            Some(Param::Num(v)) => *v,
            _ => default,
        }
    }

    /// Word `key`.
    pub fn text(&self, key: &str) -> Option<&str> {
        match self.0.get(key) {
            Some(Param::Text(v)) => Some(v),
            _ => None,
        }
    }

    /// Flag `key`, or `default`.
    pub fn flag(&self, key: &str, default: bool) -> bool {
        match self.0.get(key) {
            Some(Param::Flag(v)) => *v,
            _ => default,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// `meters=3 speed=0.5`: for journal lines and tools.
    pub fn describe(&self) -> String {
        self.0
            .iter()
            .map(|(k, v)| match v {
                Param::Flag(b) => format!("{k}={b}"),
                Param::Num(n) => format!("{k}={n}"),
                Param::Text(t) => format!("{k}={t}"),
            })
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// A named condition with parameters: `"target_seen"`, `("target_within", {"meters": 3.0})`.
/// A `!` before the name negates it: `"!has_token"`.
#[derive(Clone, Debug, PartialEq)]
pub struct Cond {
    pub name: String,
    pub negate: bool,
    pub params: Params,
}

impl Cond {
    pub fn new(name: &str, params: Params) -> Self {
        match name.strip_prefix('!') {
            Some(rest) => Self { name: rest.trim().to_owned(), negate: true, params },
            None => Self { name: name.trim().to_owned(), negate: false, params },
        }
    }

    fn written_name(&self) -> String {
        if self.negate { format!("!{}", self.name) } else { self.name.clone() }
    }
}

impl fmt::Display for Cond {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.params.is_empty() {
            write!(f, "{}", self.written_name())
        } else {
            write!(f, "{}({})", self.written_name(), self.params.describe())
        }
    }
}

impl<'de> Deserialize<'de> for Cond {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Name(String),
            With(String, Params),
        }
        Ok(match Raw::deserialize(d)? {
            Raw::Name(name) => Cond::new(&name, Params::default()),
            Raw::With(name, params) => Cond::new(&name, params),
        })
    }
}

impl Serialize for Cond {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        if self.params.is_empty() {
            return s.serialize_str(&self.written_name());
        }
        let mut t = s.serialize_tuple(2)?;
        t.serialize_element(&self.written_name())?;
        t.serialize_element(&self.params)?;
        t.end()
    }
}

/// Go to state `to` when every condition in `when` holds (an empty `when` always holds).
/// A state's transitions are tried in order; the first that holds is taken, one per tick.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct TransitionDef {
    pub to: String,
    #[serde(default)]
    pub when: Vec<Cond>,
}

/// One state of a behaviour.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct StateDef {
    /// The state hook that runs it (`idle`, `patrol`, `chase`, `return_home`, `wait_token`,
    /// `attack`, `flee`, `hold`, `investigate`, or one the game added). Default: the state's
    /// own name.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Entry parameters the hook reads: `{"speed": 0.5, "within": 2.0}`.
    #[serde(skip_serializing_if = "Params::is_empty")]
    pub params: Params,
    /// On entry a timer is drawn in `(min, max)` seconds (`SimRng`); `timer_done` holds once it
    /// ran out.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<(f32, f32)>,
    /// On entry the plan's rest is clamped to at most this many seconds, once (a rage that
    /// brings the next attack sooner). A rest that is already shorter stays.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rest_clamp: Option<f32>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub transitions: Vec<TransitionDef>,
}

/// A behaviour (content kind `behavior`): a state machine whose logic the engine and the
/// game provide by name.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct BehaviorDef {
    /// The state a new NPC starts in.
    pub initial: String,
    pub states: BTreeMap<String, StateDef>,
    /// How its NPCs keep clear of other bodies, instead of the tuning's (`npc_tuning`
    /// `avoidance`); fields left out keep the engine's defaults. `(weight: 0)`: a boss that
    /// walks its way whoever stands there.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avoidance: Option<AvoidanceDef>,
    /// Decision cadence (seconds): a per-NPC timer that ticks down only while the NPC runs
    /// no action. Each time it runs out is a decision tick ([`NpcCtx::deciding`](crate::NpcCtx::deciding), the
    /// condition `deciding`), and it starts over; the first tick the NPC is free is one.
    /// Without it every tick is a decision tick. The state hooks and conditions that decide
    /// consult it; the engine's own states re-derive their output every tick and ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub think: Option<f32>,
}

impl BehaviorDef {
    /// The hook name of state `state`.
    pub fn kind_of<'a>(&'a self, state: &'a str) -> &'a str {
        self.states.get(state).and_then(|s| s.kind.as_deref()).unwrap_or(state)
    }
}

impl ContentDef for BehaviorDef {
    const KIND: &'static str = "behavior";

    fn validate(&self, _: &str, refs: &mut Refs) {
        let names: Vec<&str> = self.states.keys().map(String::as_str).collect();
        let unknown = |refs: &mut Refs, field: &str, state: &str| {
            let hint = shards_content::db::closest(state, names.iter().copied())
                .map(|c| format!(" (did you mean `{c}`?)"))
                .unwrap_or_default();
            refs.error(format!("{field} names an unknown state `{state}`{hint}; states: {}", names.join(", ")));
        };
        if let Some(avoidance) = &self.avoidance {
            avoidance.check(refs, "avoidance");
        }
        if let Some(think) = self.think
            && !(think > 0.0 && think.is_finite())
        {
            refs.error(format!("think must be > 0 seconds (is {think}); leave it out to decide every tick"));
        }
        if self.states.is_empty() {
            refs.error("states is empty; give at least one, e.g. {\"idle\": ()}");
            return;
        }
        if !self.states.contains_key(&self.initial) {
            unknown(refs, "initial", &self.initial);
        }
        for (name, state) in &self.states {
            if let Some((lo, hi)) = state.duration
                && !(lo >= 0.0 && hi >= lo && hi.is_finite())
            {
                refs.error(format!("states.{name}.duration ({lo}, {hi}) must be 0 <= min <= max"));
            }
            if let Some(cap) = state.rest_clamp
                && !(cap >= 0.0 && cap.is_finite())
            {
                refs.error(format!("states.{name}.rest_clamp {cap} must be a number >= 0"));
            }
            for (i, t) in state.transitions.iter().enumerate() {
                if !self.states.contains_key(&t.to) {
                    unknown(refs, &format!("states.{name}.transitions[{i}].to"), &t.to);
                }
                for c in &t.when {
                    if c.name.is_empty() {
                        refs.error(format!("states.{name}.transitions[{i}].when has a condition without a name"));
                    }
                }
            }
        }
    }
}

/// How an NPC senses (content kind `perception`). Missing fields keep their defaults.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct PerceptionDef {
    /// Sees hostile fighters this far (m).
    pub sight_range: f32,
    /// Field of view, full angle (degrees; 360 sees all around).
    pub fov_deg: f32,
    /// Eyes above the feet (m): sight rays start here.
    pub eye_height: f32,
    /// Within this distance a target is noticed whatever way the NPC faces (still not through
    /// walls).
    pub near: f32,
    /// Sight needs a clear ray through the world (`WORLD` layer). `false`: walls do not
    /// block (a boss that always knows its arena).
    pub line_of_sight: bool,
    /// Hearing: a noise of radius `r` is heard within `r × hearing` (0: deaf).
    pub hearing: f32,
    /// Seconds a target stays known after it was last seen or heard.
    pub memory: f32,
    /// A target already known keeps being seen up to this far (m, at least `sight_range`)
    /// while the ray to it is clear: its place is refreshed, so the NPC chases what it sees
    /// and not a stale point. `sight_range` then governs only noticing a target anew (an
    /// aggro that is lost farther out than it is gained). `None`: a known target is seen
    /// within `sight_range` like any other.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keep_range: Option<f32>,
}

impl Default for PerceptionDef {
    fn default() -> Self {
        Self {
            sight_range: 15.0,
            fov_deg: 120.0,
            eye_height: 1.6,
            near: 2.0,
            line_of_sight: true,
            hearing: 1.0,
            memory: 5.0,
            keep_range: None,
        }
    }
}

impl ContentDef for PerceptionDef {
    const KIND: &'static str = "perception";

    fn validate(&self, _: &str, refs: &mut Refs) {
        for (name, v) in [
            ("sight_range", self.sight_range),
            ("eye_height", self.eye_height),
            ("near", self.near),
            ("hearing", self.hearing),
            ("memory", self.memory),
        ] {
            non_negative(refs, name, v);
        }
        if !(self.fov_deg > 0.0 && self.fov_deg <= 360.0) {
            refs.error(format!("fov_deg must be in (0, 360] degrees (is {})", self.fov_deg));
        }
        if let Some(keep) = self.keep_range
            && !(keep >= self.sight_range && keep.is_finite())
        {
            refs.error(format!(
                "keep_range {keep} must be at least sight_range {} (it is how far a known target stays seen); leave it out for none",
                self.sight_range
            ));
        }
    }
}

/// What an NPC without an attack token does around its target.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Role {
    /// Walk around the target at `distance`, `orbit` m/s along the circle.
    #[default]
    Circle,
    /// Side-step around the target at `distance`, changing direction every `period` seconds.
    Strafe,
    /// Keep within `distance` of the target: close in when farther, else stand.
    Hold,
}

impl Role {
    pub fn name(self) -> &'static str {
        match self {
            Role::Circle => "circle",
            Role::Strafe => "strafe",
            Role::Hold => "hold",
        }
    }
}

/// A waiting role and its numbers. Waiters get slots around their target in the order they
/// start waiting; slot `n` takes role `n % roles` of [`DirectorDef::waiting`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct WaitRole {
    pub role: Role,
    /// Meters from the target (centre to centre).
    pub distance: f32,
    /// Movement input while getting there: 0.5 walks, 1 runs.
    pub speed: f32,
    /// Circle and strafe: meters per second along the circle.
    pub orbit: f32,
    /// Strafe: seconds before it turns the other way.
    pub period: f32,
}

impl Default for WaitRole {
    fn default() -> Self {
        Self { role: Role::Circle, distance: 4.0, speed: 0.5, orbit: 1.0, period: 2.0 }
    }
}

/// How NPCs share their target (content kind `director`). An NPC fights by the director its
/// [`Npc`](crate::Npc) names (`default` when it names none, the engine's numbers when content
/// has no `default`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct DirectorDef {
    /// Attack tokens by number of living players: `{1: 2, 2: 3, 3: 4}`. Only a token holder
    /// starts an attack, and holds the token until the attack ends. More players than listed:
    /// the highest entry; fewer: the lowest.
    pub tokens: BTreeMap<u32, u32>,
    /// How much danger one target may face at once: the attacks in flight on it may cost this
    /// much together ([`AttackOption::cost`]). `None`: no limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub budget: Option<f32>,
    /// Tag pairs whose attacks may hit one target at the same time (`("light", "heavy")`;
    /// `"*"` matches any tag). Empty: any attacks may overlap.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub allowed: Vec<(String, String)>,
    /// What those without a token do, by slot.
    pub waiting: Vec<WaitRole>,
    /// A token not used for an attack within this many seconds goes back.
    pub patience: f32,
    /// Seconds between the starts of strikes of different NPCs of this director (two nearly
    /// simultaneous blows cannot both be parried). Every strike an NPC starts is noted, the
    /// planner presses a strike (and the director grants a token for one) only when no other
    /// NPC began one that recently, and a combo continuation that is not spaced is refused
    /// and ends its chain. An NPC's own series is not held back by itself. 0: no spacing.
    /// `budget` and `allowed` are different rules (what may land on a target at once).
    #[serde(skip_serializing_if = "is_zero")]
    pub min_spacing: f32,
    /// Actions that are no strike: neither wait for the spacing nor count for it (a retreat,
    /// a buff, a repositioning). Matched against the action's tags, the plan option's tags
    /// and its name.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub spacing_exempt_tags: Vec<String>,
}

impl Default for DirectorDef {
    fn default() -> Self {
        Self {
            tokens: BTreeMap::from([(1, 2), (2, 3), (3, 4)]),
            budget: None,
            allowed: Vec::new(),
            waiting: vec![WaitRole::default()],
            patience: 4.0,
            min_spacing: 0.0,
            spacing_exempt_tags: Vec::new(),
        }
    }
}

impl DirectorDef {
    /// The id NPCs fight by unless they name another.
    pub const DEFAULT: &'static str = "default";

    /// Tokens for `players` living players.
    pub fn limit(&self, players: u32) -> u32 {
        self.tokens
            .range(..=players.max(1))
            .next_back()
            .or_else(|| self.tokens.iter().next())
            .map_or(0, |(_, n)| *n)
    }

    /// Director `id` (empty: `default`) from `db`, or the engine's numbers.
    pub fn of(db: &ContentDb, id: &str) -> DirectorDef {
        let id = if id.is_empty() { Self::DEFAULT } else { id };
        db.get::<DirectorDef>(id).cloned().unwrap_or_default()
    }

    /// Whether attacks tagged `a` and `b` may land on one target at the same time.
    pub fn may_overlap(&self, a: &[String], b: &[String]) -> bool {
        if self.allowed.is_empty() {
            return true;
        }
        let matches = |pattern: &str, tags: &[String]| pattern == "*" || tags.iter().any(|t| t == pattern);
        self.allowed.iter().any(|(x, y)| (matches(x, a) && matches(y, b)) || (matches(x, b) && matches(y, a)))
    }

    /// Whether an action with `tags` is a strike for the spacing: none of its tags is exempt.
    pub fn is_strike(&self, tags: &[String]) -> bool {
        !tags.iter().any(|t| self.spacing_exempt_tags.contains(t))
    }
}

impl ContentDef for DirectorDef {
    const KIND: &'static str = "director";

    fn validate(&self, _: &str, refs: &mut Refs) {
        if self.tokens.is_empty() {
            refs.error("tokens is empty; give tokens by number of players, e.g. {1: 2, 2: 3, 3: 4}");
        }
        if self.tokens.contains_key(&0) {
            refs.error("tokens: player counts start at 1");
        }
        if let Some(b) = self.budget
            && !(b > 0.0 && b.is_finite())
        {
            refs.error(format!("budget must be > 0 (is {b}); leave it out for no limit"));
        }
        non_negative(refs, "patience", self.patience);
        non_negative(refs, "min_spacing", self.min_spacing);
        if self.min_spacing == 0.0 && !self.spacing_exempt_tags.is_empty() {
            refs.error("spacing_exempt_tags without a min_spacing exempts nothing; set `min_spacing` or leave the tags out");
        }
        for (i, w) in self.waiting.iter().enumerate() {
            w.validate(&format!("waiting[{i}]"), refs);
        }
    }
}

impl WaitRole {
    /// The checks of a waiting role, for a director's `waiting[i]` and a plan's `wait`.
    fn validate(&self, at: &str, refs: &mut Refs) {
        non_negative(refs, &format!("{at}.distance"), self.distance);
        non_negative(refs, &format!("{at}.orbit"), self.orbit);
        if !(0.0..=1.0).contains(&self.speed) {
            refs.error(format!("{at}.speed is a movement input in 0..1 (is {})", self.speed));
        }
        if self.role == Role::Strafe && self.period <= 0.0 {
            refs.error(format!("{at}.period must be > 0 for a strafe (is {})", self.period));
        }
    }
}

/// One attack an NPC may choose: a moveset `input`, or an `action` (pressed through the
/// moveset input that starts it).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AttackOption {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    /// The name bans, cooldowns and tools use; default: the input or the action.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Relative chance among the options that may be chosen.
    pub weight: f32,
    /// Distance window (m, centre to centre): no closer than `min`...
    pub min: f32,
    /// ...and no farther than `max`. Default: the action's reach × the plan's `cover` plus the
    /// target's body radius.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f32>,
    /// Seconds before the planner chooses it again (on top of the action's own cooldown).
    pub cooldown: f32,
    /// Tags for bans and the director's allowed combinations; the action's own tags and the
    /// option's name count too.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub tags: Vec<String>,
    /// Its share of the target's collision budget while it is in flight.
    pub cost: f32,
    /// Conditions (the behaviour's: `["enraged", ("health_below", {"fraction": 0.5})]`, `!`
    /// negates) that must all hold for the option to be chosen; the planner and the director
    /// do not count it while one does not. Evaluated with the NPC's other decisions.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub when: Vec<Cond>,
    /// A combo continuation: when this option's action starts, the NPC rolls `chance` and, on
    /// success, presses `input` at the start of the action's recovery.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chain: Option<ChainDef>,
    /// The target's angle off the NPC's facing, degrees, `(min, max)` within 0..=180: 0 is straight
    /// ahead, 180 straight behind. `(95.0, 180.0)` answers an attack from the flank or the back.
    /// `None`: any angle. The facing is the one the body has when the planner chooses; how fast it
    /// turns is the body's turn rate.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub angle: Option<(f32, f32)>,
    /// Living players `(min, max)` for which it may be chosen (`(1, 1)`: solo only; `(2, 3)`:
    /// co-op only). The engine does not know a game's session cap, so a game's content test checks
    /// that `max` does not exceed it. `None`: any number.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub players: Option<(u32, u32)>,
    /// The most times in a row it may be chosen: not while the newest `max_repeats` attacks of the
    /// NPC's history are all this option. `None`: no limit.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_repeats: Option<u32>,
    /// Not chosen right after an attack carrying one of these tags (the newest history entry; its
    /// tags are its option's, its action's and its name).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub forbid_after: Vec<String>,
    /// Not chosen while one of these is a live tag: a combat object of the NPC's own (anywhere)
    /// or a zone near the target (within the plan's `zone_margin`) whose action carries it, or a
    /// zone another NPC's attack in flight will lay at the target's feet. `["capture"]`: one
    /// capture at a time.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub forbid_while: Vec<String>,
    /// As `forbid_while`, but only while one player is living (or none): `["long"]` keeps a solo
    /// player out of two long zones at once.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub solo_forbid_while: Vec<String>,
}

/// A combo continuation of an attack ([`AttackOption::chain`]). The input goes into the press
/// queue like anyone's, so it continues the attack only if the action has a cancel window
/// (`Combo`) that lets it through before the press runs out (`input_buffer`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct ChainDef {
    /// The moveset input pressed at the start of the recovery: the one a `Combo` of the
    /// action answers.
    pub input: String,
    /// The odds, 0 to 1, rolled once when the attack starts.
    pub chance: f32,
    /// Where the director's `min_spacing` is asked. Off (the default): when the input is pressed,
    /// at the start of the recovery. On: on the tick the action machine would take the queued
    /// input into the combo window. The design spaces attack *starts*, and the press and the
    /// start are some ticks apart; a strike another NPC began in between refuses the continuation,
    /// which is withdrawn from the queue and ends the chain.
    #[serde(skip_serializing_if = "is_false")]
    pub gate_at_start: bool,
}

impl Default for ChainDef {
    fn default() -> Self {
        Self { input: String::new(), chance: 1.0, gate_at_start: false }
    }
}

impl Default for AttackOption {
    fn default() -> Self {
        Self {
            input: None,
            action: None,
            name: None,
            weight: 1.0,
            min: 0.0,
            max: None,
            cooldown: 0.0,
            tags: Vec::new(),
            cost: 1.0,
            when: Vec::new(),
            chain: None,
            angle: None,
            players: None,
            max_repeats: None,
            forbid_after: Vec::new(),
            forbid_while: Vec::new(),
            solo_forbid_while: Vec::new(),
        }
    }
}

impl AttackOption {
    /// Its name: `name`, else the input, else the action.
    pub fn label(&self) -> &str {
        self.name.as_deref().or(self.input.as_deref()).or(self.action.as_deref()).unwrap_or("")
    }

    /// Its rules that name tags, by field: `forbid_after`, `forbid_while`, `solo_forbid_while`.
    fn tag_rules(&self) -> [(&'static str, &[String]); 3] {
        [("forbid_after", &self.forbid_after), ("forbid_while", &self.forbid_while), ("solo_forbid_while", &self.solo_forbid_while)]
    }
}

/// Never choose an attack tagged with one of `then` when one of the last `within` attacks
/// was tagged with one of `after`: `(after: ["zone"], then: ["zone"], within: 1)` = never two
/// zone attacks in a row.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct SequenceBan {
    pub after: Vec<String>,
    pub then: Vec<String>,
    pub within: u32,
}

impl Default for SequenceBan {
    fn default() -> Self {
        Self { after: Vec::new(), then: Vec::new(), within: 1 }
    }
}

/// How an NPC chooses its next attack (content kind `attack_plan`), for enemies and bosses
/// alike.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AttackPlanDef {
    pub options: Vec<AttackOption>,
    /// Also every moveset input whose action hits and that `options` does not name, weight 1.
    pub auto: bool,
    /// Default distance windows reach this share of an action's reach (plus the target's
    /// body radius): the target is well inside the zone when the warning shows.
    pub cover: f32,
    /// Choose only among attacks whose window holds the distance now, walking closer until
    /// one does. Off: when none fits, choose among all and walk into the chosen one's window.
    pub in_range_only: bool,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub bans: Vec<SequenceBan>,
    /// Seconds of rest after an attack before it asks for the next token: `(min, max)`,
    /// counted while it runs no action (a stagger after a parried attack is no rest).
    pub rest: (f32, f32),
    /// The rest after an attack that was interrupted (a stagger cut it short) instead of
    /// `rest`: `(min, max)` seconds, drawn as it is, replacing the finished attack's pause.
    /// `None`: an interrupted attack rests like a finished one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rest_interrupted: Option<(f32, f32)>,
    /// The rest an NPC starts with when its brain first runs (its pause at spawn), `(min,
    /// max)` seconds. `None`: it starts without rest.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub first_rest: Option<(f32, f32)>,
    /// The planner asks the director for a token, makes a new choice and presses only on the NPC's
    /// decision ticks (`BehaviorDef::think`, `NpcThink.decided`); between two decisions it keeps the
    /// choice it made and walks into its window. The TDD has the brain decide at 5-10 Hz and the
    /// attack press is part of the decision, so grant, choice and press share one tick. Off (the
    /// default): any tick. A behaviour without a `think` cadence decides every tick, so there it
    /// changes nothing.
    #[serde(skip_serializing_if = "is_false")]
    pub press_on_decision: bool,
    /// Noting only (CDD 27.2: a big enemy's attacks pace the small ones around it, never the
    /// reverse). The NPC's strike starts are noted and hold the other NPCs of its director back as
    /// usual, but it never waits for the director's `min_spacing` itself: no strike of its own
    /// is held back by the spacing, in the grant, the planner, a combo continuation or
    /// `NpcCtx::strike_ready`. Off (the default): it waits like the rest. Without a `min_spacing`
    /// on the director it changes nothing.
    #[serde(skip_serializing_if = "is_false")]
    pub ignores_spacing: bool,
    /// The NPC fights without the director's tokens (a boss among its adds, CDD 27.2): it is
    /// granted the right to attack whenever it asks and some option may be chosen, whatever the
    /// director's limit. Its grant is not counted, not limited, not lowered and never times out
    /// (`patience`), and no `TokenGranted`/`TokenReturned` is triggered for it. Its own choice and
    /// press do not wait on the target's `budget` or `allowed` combinations. Its strikes are noted
    /// for the director's spacing like anyone's, and its attacks in flight count against the
    /// others' `budget` and combinations. Combine with `ignores_spacing` for "never waits, still
    /// counts". Off (the default): it fights by the tokens.
    #[serde(skip_serializing_if = "is_false")]
    pub tokenless: bool,
    /// The role this NPC waits in instead of a slot of its director's `waiting` list: while it
    /// holds no grant, and while it holds one with no option chosen (nothing in reach). It takes
    /// no slot, so the others' slots do not move. `None`: a slot, and with a grant the planner
    /// closes in or stands, as before.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub wait: Option<WaitRole>,
    /// A live zone of any owner counts for the options' `forbid_while` and `solo_forbid_while`
    /// when the target stands within this many meters of its footprint, on the ground plane (0:
    /// only inside it). The NPC's own combat objects count wherever they are.
    #[serde(skip_serializing_if = "is_zero")]
    pub zone_margin: f32,
}

impl Default for AttackPlanDef {
    fn default() -> Self {
        Self {
            options: Vec::new(),
            auto: false,
            cover: 0.8,
            in_range_only: false,
            bans: Vec::new(),
            rest: (0.5, 1.0),
            rest_interrupted: None,
            first_rest: None,
            press_on_decision: false,
            ignores_spacing: false,
            tokenless: false,
            wait: None,
            zone_margin: 0.0,
        }
    }
}

impl AttackPlanDef {
    /// Some option has a `forbid_while` or a `solo_forbid_while`: the NPC's live tags are read.
    pub fn reads_live_tags(&self) -> bool {
        self.options.iter().any(|o| !o.forbid_while.is_empty() || !o.solo_forbid_while.is_empty())
    }

    /// How many past attacks the plan looks at: the largest of the bans' `within` and the options'
    /// `max_repeats`, at least 1 (`forbid_after` reads only the newest).
    pub fn memory(&self) -> usize {
        let bans = self.bans.iter().map(|b| b.within as usize);
        let repeats = self.options.iter().filter_map(|o| o.max_repeats.map(|r| r as usize));
        bans.chain(repeats).max().unwrap_or(0).max(1)
    }
}

impl ContentDef for AttackPlanDef {
    const KIND: &'static str = "attack_plan";

    fn validate(&self, _: &str, refs: &mut Refs) {
        if self.options.is_empty() && !self.auto {
            refs.error("options is empty and auto is off: give options, or `auto: true` to use the moveset's attacks");
        }
        let mut names: Vec<&str> = Vec::new();
        for (i, o) in self.options.iter().enumerate() {
            let at = format!("options[{i}]");
            match (&o.input, &o.action) {
                (Some(_), Some(_)) => refs.error(format!("{at} names both an input and an action; give one")),
                (None, None) => refs.error(format!("{at} names neither an input nor an action, e.g. (input: \"light\")")),
                (None, Some(action)) => {
                    refs.require::<ActionDef>(action, &format!("{at}.action"));
                }
                _ => {}
            }
            if names.contains(&o.label()) {
                refs.error(format!("{at}: another option is also named `{}`; give one a `name`", o.label()));
            }
            names.push(o.label());
            non_negative(refs, &format!("{at}.weight"), o.weight);
            non_negative(refs, &format!("{at}.min"), o.min);
            non_negative(refs, &format!("{at}.cooldown"), o.cooldown);
            non_negative(refs, &format!("{at}.cost"), o.cost);
            if let Some(max) = o.max
                && !(max >= o.min && max.is_finite())
            {
                refs.error(format!("{at}.max {max} must be at least min {}", o.min));
            }
            for (ci, c) in o.when.iter().enumerate() {
                if c.name.is_empty() {
                    refs.error(format!("{at}.when[{ci}] is a condition without a name"));
                }
            }
            if let Some((lo, hi)) = o.angle
                && !(lo >= 0.0 && hi >= lo && hi <= 180.0)
            {
                refs.error(format!(
                    "{at}.angle ({lo}, {hi}) must be 0 <= min <= max <= 180 degrees (0 ahead, 180 behind); e.g. (95.0, 180.0) for an answer behind"
                ));
            }
            if let Some((lo, hi)) = o.players
                && !(lo >= 1 && hi >= lo)
            {
                refs.error(format!("{at}.players ({lo}, {hi}) must be 1 <= min <= max; e.g. (1, 1) for solo only"));
            }
            if o.max_repeats == Some(0) {
                refs.error(format!("{at}.max_repeats must be at least 1 (1: never twice in a row); leave it out for no limit"));
            }
            for (field, tags) in o.tag_rules() {
                if tags.iter().any(String::is_empty) {
                    refs.error(format!("{at}.{field} has an empty tag"));
                }
            }
            if let Some(chain) = &o.chain {
                if chain.input.is_empty() {
                    refs.error(format!("{at}.chain names no input; give the moveset input the continuation presses, e.g. (input: \"light\", chance: 0.7)"));
                }
                if !(0.0..=1.0).contains(&chain.chance) {
                    refs.error(format!("{at}.chain.chance is a probability in 0..1 (is {})", chain.chance));
                }
            }
        }
        if !self.options.is_empty() && self.options.iter().all(|o| o.weight <= 0.0) {
            refs.error("every option has weight 0: none would ever be chosen");
        }
        if !(self.cover > 0.0 && self.cover.is_finite()) {
            refs.error(format!("cover must be > 0 (is {})", self.cover));
        }
        for (name, range) in [("rest", Some(self.rest)), ("rest_interrupted", self.rest_interrupted), ("first_rest", self.first_rest)] {
            if let Some((lo, hi)) = range
                && !(lo >= 0.0 && hi >= lo && hi.is_finite())
            {
                refs.error(format!("{name} ({lo}, {hi}) must be 0 <= min <= max"));
            }
        }
        if !(self.zone_margin >= 0.0 && self.zone_margin.is_finite()) {
            refs.error(format!("zone_margin must be >= 0 and finite (is {})", self.zone_margin));
        } else if self.zone_margin > 0.0 && !self.reads_live_tags() {
            refs.error("zone_margin without any forbid_while or solo_forbid_while reads nothing; set the forbids or leave it out");
        }
        if self.options.iter().any(|o| o.tag_rules().iter().any(|(_, tags)| tags.iter().any(|t| !t.is_empty()))) {
            // A tag nothing carries never applies: a typo check. A tag any action carries is never an
            // error (an `input` option's action is not known without a moveset, and a history entry
            // carries its action's tags); a plan option's tags and name count, every plan's (phase
            // plans share one history), and so does every moveset input name (the names of `auto`
            // options).
            let mut known: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
            known.extend(refs.iter::<ActionDef>().flat_map(|(_, a)| a.tags.iter().map(String::as_str)));
            for (_, plan) in refs.iter::<AttackPlanDef>() {
                for o in &plan.options {
                    known.extend(o.tags.iter().map(String::as_str));
                    known.insert(o.label());
                }
            }
            known.extend(refs.iter::<MovesetDef>().flat_map(|(_, m)| m.inputs.keys().map(String::as_str)));
            for (i, o) in self.options.iter().enumerate() {
                for (field, tags) in o.tag_rules() {
                    for tag in tags.iter().filter(|t| !t.is_empty() && !known.contains(t.as_str())) {
                        let hint = shards_content::db::closest(tag, known.iter().copied()).map(|c| format!(" (did you mean `{c}`?)")).unwrap_or_default();
                        refs.error(format!(
                            "options[{i}].{field} names `{tag}`, which no action, no plan option and no moveset input in the content carries: it never applies{hint}"
                        ));
                    }
                }
            }
        }
        if let Some(wait) = &self.wait {
            wait.validate("wait", refs);
        }
        for (i, b) in self.bans.iter().enumerate() {
            if b.after.is_empty() || b.then.is_empty() {
                refs.error(format!("bans[{i}] needs tags in both `after` and `then`"));
            }
            if b.within == 0 {
                refs.error(format!("bans[{i}].within must be at least 1 (the last attack)"));
            }
        }
    }
}

/// How nav agents keep clear of other bodies, NPCs and players alike (in [`NpcTuning`] for
/// every NPC, in a [`BehaviorDef`] for its own). Each tick an NPC on its way tries a fixed
/// fan of velocities around the one its route asks for and takes the cheapest: straying
/// from the route costs, and so does the first collision it would run into within
/// `horizon` seconds, the sooner the more (`weight`). Another NPC that avoids too takes half
/// of each sidestep (reciprocal velocity obstacles); players, NPCs standing or swinging and
/// NPCs under an agent's orders are walked around. Missing fields keep their defaults.
///
/// ```ron
/// "avoidance": (weight: 1.0, horizon: 1.5, range: 3.0, neighbours: 6, margin: 0.2),
/// ```
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct AvoidanceDef {
    /// How much a coming collision weighs against keeping to the route: one at half the
    /// horizon costs `weight` times a full stop. 0 turns avoidance off: the NPC walks its
    /// route as if alone, and bodies only push each other apart.
    pub weight: f32,
    /// Seconds it looks ahead for collisions (never past the moment it stops at its goal).
    pub horizon: f32,
    /// Bodies farther than this from its own (m, surface to surface) are not looked at.
    pub range: f32,
    /// At most this many bodies, the nearest first (by `AiId` among equals).
    pub neighbours: u32,
    /// Room kept between bodies, on top of their radii (m). None toward its own target,
    /// which it walks up to in order to strike.
    pub margin: f32,
}

impl Default for AvoidanceDef {
    fn default() -> Self {
        Self { weight: 1.0, horizon: 1.5, range: 3.0, neighbours: 6, margin: 0.2 }
    }
}

impl AvoidanceDef {
    /// Content errors in the numbers, the fields named under `at`.
    pub fn check(&self, refs: &mut Refs, at: &str) {
        for (name, v) in [("weight", self.weight), ("range", self.range), ("margin", self.margin)] {
            non_negative(refs, &format!("{at}.{name}"), v);
        }
        if !(self.horizon > 0.0 && self.horizon <= 10.0) {
            refs.error(format!("{at}.horizon must be in (0, 10] seconds (is {})", self.horizon));
        }
        if !(1..=32).contains(&self.neighbours) {
            refs.error(format!("{at}.neighbours must be 1 to 32 (is {}); set weight 0 to turn avoidance off", self.neighbours));
        }
    }

    /// Whether it steers at all.
    pub fn on(&self) -> bool {
        self.weight > 0.0
    }
}

/// Engine-wide NPC numbers (content kind `npc_tuning`, id `default`). Missing fields keep
/// their defaults.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct NpcTuning {
    /// Radius of the noise a hit makes (m), heard around the one hit; 0: hits are silent.
    pub hit_noise: f32,
    /// Radius of the noise a sprinting character makes (m); 0: silent.
    pub sprint_noise: f32,
    /// How nav agents keep clear of other bodies (a behaviour may bring its own).
    pub avoidance: AvoidanceDef,
    /// Seconds between two route plans of one agent whose way is blocked.
    pub repath: f32,
    /// A waypoint counts as reached this close (m).
    pub waypoint_reach: f32,
}

impl Default for NpcTuning {
    fn default() -> Self {
        Self { hit_noise: 10.0, sprint_noise: 8.0, avoidance: AvoidanceDef::default(), repath: 1.0, waypoint_reach: 0.4 }
    }
}

impl NpcTuning {
    /// The id the engine reads.
    pub const ID: &'static str = "default";

    /// The tuning in `db`, or the defaults.
    pub fn of(db: &ContentDb) -> NpcTuning {
        db.get::<NpcTuning>(Self::ID).cloned().unwrap_or_default()
    }
}

impl ContentDef for NpcTuning {
    const KIND: &'static str = "npc_tuning";

    fn validate(&self, id: &str, refs: &mut Refs) {
        if id != Self::ID {
            refs.error(format!("the engine reads only the tuning `{}`; rename this one", Self::ID));
        }
        for (name, v) in [
            ("hit_noise", self.hit_noise),
            ("sprint_noise", self.sprint_noise),
            ("repath", self.repath),
            ("waypoint_reach", self.waypoint_reach),
        ] {
            non_negative(refs, name, v);
        }
        self.avoidance.check(refs, "avoidance");
    }
}

fn is_zero(v: &f32) -> bool {
    *v == 0.0
}

fn is_false(v: &bool) -> bool {
    !*v
}

fn non_negative(refs: &mut Refs, field: &str, v: f32) {
    if !(v >= 0.0 && v.is_finite()) {
        refs.error(format!("{field} must be a number >= 0 (is {v})"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ron<T: serde::de::DeserializeOwned>(text: &str) -> T {
        ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(text)
            .unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn conditions_read_as_names_or_with_params_and_round_trip_through_json() {
        let t: TransitionDef =
            ron(r#"(to: "fight", when: ["target_seen", "!has_token", ("target_within", {"meters": 3, "loud": true})])"#);
        assert_eq!(t.when[0], Cond::new("target_seen", Params::default()));
        assert!(t.when[1].negate && t.when[1].name == "has_token");
        assert_eq!(t.when[2].params.num("meters", 0.0), 3.0, "integers read as numbers");
        assert!(t.when[2].params.flag("loud", false));
        assert_eq!(t.when[2].to_string(), "target_within(loud=true meters=3)");
        let json = serde_json::to_value(&t).unwrap();
        assert_eq!(serde_json::from_value::<TransitionDef>(json).unwrap(), t);
    }

    #[test]
    fn token_limits_follow_the_player_count() {
        let d = DirectorDef::default();
        assert_eq!((d.limit(0), d.limit(1), d.limit(2), d.limit(3), d.limit(7)), (2, 2, 3, 4, 4));
        let d: DirectorDef = ron("(tokens: {2: 3, 4: 5})");
        assert_eq!((d.limit(1), d.limit(3), d.limit(4)), (3, 3, 5));
        let json = serde_json::to_value(&d).unwrap();
        assert_eq!(serde_json::from_value::<DirectorDef>(json).unwrap(), d, "player counts survive JSON keys");
    }

    #[test]
    fn allowed_combinations_match_tags_either_way_round() {
        let d: DirectorDef = ron(r#"(allowed: [("light", "heavy"), ("light", "light")])"#);
        let (light, heavy) = (vec!["light".to_owned()], vec!["heavy".to_owned()]);
        assert!(d.may_overlap(&light, &heavy) && d.may_overlap(&heavy, &light) && d.may_overlap(&light, &light));
        assert!(!d.may_overlap(&heavy, &heavy));
        assert!(DirectorDef::default().may_overlap(&heavy, &heavy), "no list: anything overlaps");
        let any: DirectorDef = ron(r#"(allowed: [("*", "light")])"#);
        assert!(any.may_overlap(&heavy, &light) && !any.may_overlap(&heavy, &heavy));
    }

    #[test]
    fn validation_names_the_field_and_the_fix() {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<BehaviorDef>();
        kinds.register::<AttackPlanDef>();
        kinds.register::<ActionDef>();
        let behaviors = r#"{
            "b": (initial: "idel", states: {
                "idle": (transitions: [(to: "chace", when: ["target_seen"])]),
                "chase": (duration: (2.0, 1.0)),
            }),
        }"#;
        let plans = r#"{
            "p": (options: [(input: "light", action: "x"), (action: "swip", weight: -1), (input: "light")], rest: (1, 0)),
        }"#;
        let sources = [
            shards_content::db::Source { path: "content/b.behavior.ron".into(), text: behaviors.into() },
            shards_content::db::Source { path: "content/p.attack_plan.ron".into(), text: plans.into() },
        ];
        let db = ContentDb::build(&kinds, &sources);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        // Kinds validate in name order: `attack_plan` before `behavior`.
        let expected = [
            "content/p.attack_plan.ron: attack_plan `p`: options[0] names both an input and an action; give one",
            "content/p.attack_plan.ron: attack_plan `p`: options[1].action -> unknown action `swip`",
            "content/p.attack_plan.ron: attack_plan `p`: options[1].weight must be a number >= 0 (is -1)",
            "content/p.attack_plan.ron: attack_plan `p`: options[2]: another option is also named `light`; give one a `name`",
            "content/p.attack_plan.ron: attack_plan `p`: rest (1, 0) must be 0 <= min <= max",
            "content/b.behavior.ron: behavior `b`: initial names an unknown state `idel` (did you mean `idle`?); states: chase, idle",
            "content/b.behavior.ron: behavior `b`: states.chase.duration (2, 1) must be 0 <= min <= max",
            "content/b.behavior.ron: behavior `b`: states.idle.transitions[0].to names an unknown state `chace` (did you mean `chase`?); states: chase, idle",
        ];
        assert_eq!(errors, expected);
    }

    #[test]
    fn rest_semantics_are_checked_and_neutral_by_default() {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<AttackPlanDef>();
        kinds.register::<ActionDef>();
        let text = r#"{ "p": (auto: true, rest: (0.5, 1), rest_interrupted: Some((1, 0.5)), first_rest: Some((-1, 2))) }"#;
        let db = ContentDb::build(&kinds, &[shards_content::db::Source { path: "content/p.attack_plan.ron".into(), text: text.into() }]);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        assert_eq!(
            errors,
            [
                "content/p.attack_plan.ron: attack_plan `p`: rest_interrupted (1, 0.5) must be 0 <= min <= max",
                "content/p.attack_plan.ron: attack_plan `p`: first_rest (-1, 2) must be 0 <= min <= max",
            ]
        );
        let neutral = serde_json::to_string(&AttackPlanDef::default()).unwrap();
        for field in ["rest_interrupted", "first_rest", "press_on_decision", "ignores_spacing", "tokenless", "wait"] {
            assert!(!neutral.contains(field), "{field} stays out of a plan that does not use it: {neutral}");
        }
    }

    #[test]
    fn a_chain_is_neutral_by_default() {
        let neutral = serde_json::to_string(&ChainDef { input: "light".into(), ..Default::default() }).unwrap();
        assert!(!neutral.contains("gate_at_start"), "{neutral}");
    }

    /// The gates of an option leave an option and a plan that do not use them serializing as they
    /// did (replays read the JSON), and each one serializes and reads back.
    #[test]
    fn the_scheduler_gates_are_neutral_by_default_and_round_trip() {
        let neutral = serde_json::to_value(AttackOption::default()).unwrap();
        for field in ["angle", "players", "max_repeats", "forbid_after"] {
            assert!(neutral.get(field).is_none(), "{field} stays out of an option that does not use it: {neutral}");
        }
        assert_eq!(
            neutral,
            serde_json::json!({"weight": 1.0, "min": 0.0, "cooldown": 0.0, "cost": 1.0}),
            "an option serializes as it did before the gates"
        );
        let set = AttackOption {
            input: Some("light".into()),
            angle: Some((95.0, 180.0)),
            players: Some((2, 3)),
            max_repeats: Some(2),
            forbid_after: vec!["zone".into()],
            ..Default::default()
        };
        let json = serde_json::to_value(&set).unwrap();
        assert_eq!(json["angle"], serde_json::json!([95.0, 180.0]));
        assert_eq!(json["players"], serde_json::json!([2, 3]));
        assert_eq!(json["max_repeats"], 2);
        assert_eq!(json["forbid_after"], serde_json::json!(["zone"]));
        assert_eq!(serde_json::from_value::<AttackOption>(json).unwrap(), set);
        let written: AttackOption = ron(r#"(input: "light", angle: (95.0, 180.0), players: (1, 1), max_repeats: 1, forbid_after: ["zone"])"#);
        assert_eq!((written.angle, written.players, written.max_repeats), (Some((95.0, 180.0)), Some((1, 1)), Some(1)));
    }

    #[test]
    fn a_plans_memory_is_the_longest_of_its_bans_and_its_repeat_limits() {
        let option = |max_repeats| AttackOption { input: Some("a".into()), max_repeats, ..Default::default() };
        let ban = |within| SequenceBan { after: vec!["x".into()], then: vec!["y".into()], within };
        let plan = |options, bans| AttackPlanDef { options, bans, ..Default::default() };
        assert_eq!(AttackPlanDef::default().memory(), 1, "at least the newest");
        assert_eq!(plan(vec![option(None)], vec![]).memory(), 1);
        assert_eq!(plan(vec![option(None)], vec![ban(3)]).memory(), 3, "unchanged without max_repeats");
        assert_eq!(plan(vec![option(Some(2))], vec![]).memory(), 2);
        assert_eq!(plan(vec![option(Some(2)), option(Some(5))], vec![ban(3)]).memory(), 5);
        assert_eq!(plan(vec![option(Some(2))], vec![ban(4)]).memory(), 4);
    }

    /// `tokenless` and `wait` read, serialize and round-trip; a plan's waiting role is checked as a
    /// director's is, under `wait.`.
    #[test]
    fn tokenless_and_a_waiting_role_round_trip_and_the_role_validates() {
        let plan: AttackPlanDef =
            ron(r#"(options: [(input: "light")], tokenless: true, wait: (role: Strafe, distance: 2.4, speed: 1.0, orbit: 2.0, period: 1.875))"#);
        assert!(plan.tokenless);
        assert_eq!(plan.wait.as_ref().map(|w| (w.role, w.distance, w.period)), Some((Role::Strafe, 2.4, 1.875)));
        let json = serde_json::to_value(&plan).unwrap();
        assert_eq!(json["tokenless"], true);
        assert_eq!(json["wait"]["role"], "Strafe");
        assert_eq!(serde_json::from_value::<AttackPlanDef>(json).unwrap(), plan);
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<AttackPlanDef>();
        let text = r#"{
            "ok": (options: [(input: "light")], tokenless: true, wait: (role: Hold, distance: 2.0)),
            "bad": (options: [(input: "light")], wait: (role: Strafe, distance: -1.0, speed: 1.5, orbit: -2.0, period: 0.0)),
        }"#;
        let db = ContentDb::build(&kinds, &[shards_content::db::Source { path: "content/p.attack_plan.ron".into(), text: text.into() }]);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        let at = |what: &str| format!("content/p.attack_plan.ron: attack_plan `bad`: {what}");
        assert_eq!(
            errors,
            [
                at("wait.distance must be a number >= 0 (is -1)"),
                at("wait.orbit must be a number >= 0 (is -2)"),
                at("wait.speed is a movement input in 0..1 (is 1.5)"),
                at("wait.period must be > 0 for a strafe (is 0)"),
            ]
        );
    }

    #[test]
    fn the_gates_validate_with_the_field_and_the_fix() {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<AttackPlanDef>();
        kinds.register::<ActionDef>();
        kinds.register::<MovesetDef>();
        let source = |path: &str, text: &str| shards_content::db::Source { path: path.into(), text: text.into() };
        let plans = r#"{
            "p": (options: [
                (input: "light", angle: Some((180.0, 95.0))),
                (input: "heavy", angle: Some((-1.0, 90.0)), players: Some((0, 3))),
                (input: "zap", angle: Some((0.0, 181.0)), players: Some((3, 2)), max_repeats: Some(0)),
                (input: "slam", tags: ["zone_center"], forbid_after: ["zone_centre", "", "melee", "slam"]),
                (input: "hit", forbid_after: ["jab"]),
            ]),
        }"#;
        let actions = r#"{ "a": (net_id: 1, phases: [("swing", 0.5)], tags: ["melee"]) }"#;
        let movesets = r#"{ "m": (inputs: {"jab": "a"}) }"#;
        let db = ContentDb::build(
            &kinds,
            &[source("content/p.attack_plan.ron", plans), source("content/a.action.ron", actions), source("content/m.moveset.ron", movesets)],
        );
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        let at = "content/p.attack_plan.ron: attack_plan `p`: ";
        let expected = [
            format!("{at}options[0].angle (180, 95) must be 0 <= min <= max <= 180 degrees (0 ahead, 180 behind); e.g. (95.0, 180.0) for an answer behind"),
            format!("{at}options[1].angle (-1, 90) must be 0 <= min <= max <= 180 degrees (0 ahead, 180 behind); e.g. (95.0, 180.0) for an answer behind"),
            format!("{at}options[1].players (0, 3) must be 1 <= min <= max; e.g. (1, 1) for solo only"),
            format!("{at}options[2].angle (0, 181) must be 0 <= min <= max <= 180 degrees (0 ahead, 180 behind); e.g. (95.0, 180.0) for an answer behind"),
            format!("{at}options[2].players (3, 2) must be 1 <= min <= max; e.g. (1, 1) for solo only"),
            format!("{at}options[2].max_repeats must be at least 1 (1: never twice in a row); leave it out for no limit"),
            format!("{at}options[3].forbid_after has an empty tag"),
            format!(
                "{at}options[3].forbid_after names `zone_centre`, which no action, no plan option and no moveset input in the content carries: it never applies (did you mean `zone_center`?)"
            ),
        ];
        assert_eq!(errors, expected);
        // `melee` (an action's tag), `slam` (an option's name) and `jab` (a moveset input) are known.
        assert!(!errors.iter().any(|e| e.contains("`melee`") || e.contains("`slam`") || e.contains("`jab`")), "{errors:?}");
    }

    /// The live-tag forbids and `zone_margin` stay out of an option and a plan that do not use
    /// them, round-trip, and validate: an empty tag, a typo (with the nearest known tag), a bad
    /// margin and a margin with nothing to read.
    #[test]
    fn the_live_tag_forbids_round_trip_and_validate() {
        let neutral = serde_json::to_value(AttackPlanDef { options: vec![AttackOption::default()], ..Default::default() }).unwrap();
        for field in ["forbid_while", "solo_forbid_while"] {
            assert!(neutral["options"][0].get(field).is_none(), "{field} stays out: {neutral}");
        }
        assert!(neutral.get("zone_margin").is_none(), "{neutral}");
        let plan: AttackPlanDef =
            ron(r#"(options: [(input: "trap", forbid_while: ["capture"], solo_forbid_while: ["long"])], zone_margin: 1.0)"#);
        assert!(plan.reads_live_tags());
        assert!(!AttackPlanDef::default().reads_live_tags());
        let json = serde_json::to_value(&plan).unwrap();
        assert_eq!(json["options"][0]["forbid_while"], serde_json::json!(["capture"]));
        assert_eq!(json["options"][0]["solo_forbid_while"], serde_json::json!(["long"]));
        assert_eq!(json["zone_margin"], 1.0);
        assert_eq!(serde_json::from_value::<AttackPlanDef>(json).unwrap(), plan);

        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<AttackPlanDef>();
        kinds.register::<ActionDef>();
        let source = |path: &str, text: &str| shards_content::db::Source { path: path.into(), text: text.into() };
        let plans = r#"{
            "p": (options: [
                (input: "trap", forbid_while: ["capture", ""], solo_forbid_while: ["lnog"]),
                (input: "vines", solo_forbid_while: ["long", ""]),
            ], zone_margin: -1.0),
            "q": (options: [(input: "light")], zone_margin: 1.5),
            "r": (options: [(input: "trap", forbid_while: ["capture"])], zone_margin: 1.0),
        }"#;
        let actions = r#"{ "puddle": (net_id: 1, phases: [("active", 1.0)], tags: ["capture", "long"]) }"#;
        let db = ContentDb::build(&kinds, &[source("content/p.attack_plan.ron", plans), source("content/a.action.ron", actions)]);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        let at = |plan: &str, what: &str| format!("content/p.attack_plan.ron: attack_plan `{plan}`: {what}");
        assert_eq!(
            errors,
            [
                at("p", "options[0].forbid_while has an empty tag"),
                at("p", "options[1].solo_forbid_while has an empty tag"),
                at("p", "zone_margin must be >= 0 and finite (is -1)"),
                at(
                    "p",
                    "options[0].solo_forbid_while names `lnog`, which no action, no plan option and no moveset input in the content carries: it never applies (did you mean `long`?)"
                ),
                at("q", "zone_margin without any forbid_while or solo_forbid_while reads nothing; set the forbids or leave it out"),
            ]
        );
    }

    #[test]
    fn keep_range_must_reach_at_least_as_far_as_sight() {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<PerceptionDef>();
        let text = r#"{ "ok": (sight_range: 10, keep_range: Some(16)), "short": (sight_range: 10, keep_range: Some(8)) }"#;
        let db = ContentDb::build(&kinds, &[shards_content::db::Source { path: "content/p.perception.ron".into(), text: text.into() }]);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        assert_eq!(errors.len(), 1, "{errors:?}");
        assert!(errors[0].contains("perception `short`: keep_range 8 must be at least sight_range 10"), "{errors:?}");
        assert_eq!(PerceptionDef::default().keep_range, None);
        assert!(!serde_json::to_string(&PerceptionDef::default()).unwrap().contains("keep_range"), "a default stays out of the file");
    }

    #[test]
    fn avoidance_numbers_are_checked_in_the_tuning_and_in_a_behaviour() {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<NpcTuning>();
        kinds.register::<BehaviorDef>();
        let source = |path: &str, text: &str| shards_content::db::Source { path: path.into(), text: text.into() };
        let tuning = r#"{ "default": (avoidance: (weight: -1, horizon: 0, neighbours: 0)) }"#;
        let behaviors = r#"{ "b": (initial: "idle", avoidance: (range: -2, margin: -0.5, horizon: 12, neighbours: 33), states: {"idle": ()}) }"#;
        let db = ContentDb::build(&kinds, &[source("content/t.npc_tuning.ron", tuning), source("content/b.behavior.ron", behaviors)]);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        let expected = [
            "content/b.behavior.ron: behavior `b`: avoidance.range must be a number >= 0 (is -2)",
            "content/b.behavior.ron: behavior `b`: avoidance.margin must be a number >= 0 (is -0.5)",
            "content/b.behavior.ron: behavior `b`: avoidance.horizon must be in (0, 10] seconds (is 12)",
            "content/b.behavior.ron: behavior `b`: avoidance.neighbours must be 1 to 32 (is 33); set weight 0 to turn avoidance off",
            "content/t.npc_tuning.ron: npc_tuning `default`: avoidance.weight must be a number >= 0 (is -1)",
            "content/t.npc_tuning.ron: npc_tuning `default`: avoidance.horizon must be in (0, 10] seconds (is 0)",
            "content/t.npc_tuning.ron: npc_tuning `default`: avoidance.neighbours must be 1 to 32 (is 0); set weight 0 to turn avoidance off",
        ];
        assert_eq!(errors, expected);
        // The fields `avoidance` took the place of are gone: content that still has them is
        // told which fields there are.
        let old = r#"{ "default": (avoid_time: 0.5, avoid_margin: 0.3) }"#;
        let db = ContentDb::build(&kinds, &[source("content/t.npc_tuning.ron", old)]);
        let errors: Vec<String> = db.errors().map(|e| e.to_string()).collect();
        assert_eq!(
            errors,
            [
                "content/t.npc_tuning.ron:1:15: npc_tuning `default`: Unexpected field named `avoid_time` in `NpcTuning`, expected one \
                 of `hit_noise`, `sprint_noise`, `avoidance`, `repath`, or `waypoint_reach` instead"
            ]
        );
    }
}
