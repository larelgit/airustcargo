//! Why an attack option may not be chosen now: the rules of the attack planner, judged in one
//! place.
//!
//! One function, `planner::refusal`, judges an option of an NPC's plan against one set of inputs
//! (`Inputs`), built by one builder (`inputs`) from the NPC's read-only view, its target, the
//! strike notes and the attacks in flight its caller collected. The director's grant (`direct`,
//! step 3: some option must pass, distance aside) and the planner (`plan_attacks`: the options it
//! may choose or keep) both ask it, so the two never judge an option differently; whether an
//! option may be chosen is `refusal(..).is_none()`. The rules, in the order they are named (the
//! first that refuses is the answer, [`Refusal`]):
//!
//! 1. `Weight`: weight 0, a chain link that is never chosen;
//! 2. `Cooldown`: on the plan's cooldown;
//! 3. `Closed`: a `when` condition does not hold;
//! 4. `Banned`: a plan ban (`SequenceBan`) after the last attacks;
//! 5. `Machine`: the action machine would start it neither now nor by waiting;
//! 6. `Budget`: it does not fit in what is left of the target's collision budget;
//! 7. `Overlap`: it may not overlap with an attack in flight on the target;
//! 8. `Spacing`: another NPC of its director began a strike less than `min_spacing` ago.
//!
//! Every rule is a pure read of the world as the system sees it, so the order changes nothing
//! but the answer's name. The action machine is asked where the rules before this module asked
//! it (after the weight, the cooldown and the bans, a closed option too), because its rule reads
//! definitions (a cancel window's action, a context binding's) that the replay's `content` check
//! records: what a session reads is what it read before. The distance window is not a rule here:
//! the planner prefers the options whose window holds and, with `in_range_only`, chooses only
//! among them.
//!
//! The players the director counts are `Players` and `living_players`.

use ai_core::AiId;
use bevy::prelude::*;
use shards_action::{Dead, FighterView};
use shards_content::ContentDb;
use shards_core::PlayerId;
use shards_motion::PhysicsWorld;

use crate::def::{AttackPlanDef, DirectorDef};
use crate::director::{npc_site, npc_view, FightingReadOnlyItem};
use crate::planner::{InFlight, NpcOptions, NpcPlan};
use crate::strike::{chain_note, note_list, pool, spacing_ticks, strike_pressed, waiting, Note};

/// Why an attack option may not be chosen now, distance aside: the first rule that refuses it,
/// in the order of this list. The boss scheduler's next slices add rules in their places in the
/// order (the angle, the players, repeats, `forbid_after`, live tags), each a new variant:
/// a game's `match` needs a `_` arm, so a new variant does not break it.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Refusal {
    /// Its weight is 0: a chain link, never chosen.
    Weight,
    /// On the plan's cooldown: seconds left.
    Cooldown { left: f32 },
    /// A `when` condition does not hold (the NPC's `NpcOptions` does not list it open): the
    /// option's conditions as written.
    Closed { when: String },
    /// A plan ban: `bans[ban]` forbids it after `after`, the newest past attack (its name) the
    /// ban looks back to.
    Banned { after: String, ban: usize },
    /// The action machine would start it neither now nor by waiting (`Blocker::reason`).
    Machine { reason: String },
    /// It costs `cost`, more than the `left` of the target's collision budget.
    Budget { left: f32, cost: f32 },
    /// It may not overlap (the director's `allowed`) with an attack in flight on the target: the
    /// NPC `by` that holds that attack, and the attack's tags.
    Overlap { by: AiId, with: Vec<String> },
    /// NPC `by` of its director began a strike less than `min_spacing` ago: `ticks` left to wait.
    Spacing { by: AiId, ticks: u64 },
}

/// What the callers of [`inputs`] hold for every NPC they judge.
pub(crate) struct FightCx<'a> {
    pub db: &'a ContentDb,
    pub physics: &'a PhysicsWorld,
    pub tick: u64,
    pub dt: f32,
}

/// Everything [`refusal`](crate::planner::refusal) reads of one NPC and its target besides the
/// option.
pub(crate) struct Inputs<'a> {
    pub plan_def: &'a AttackPlanDef,
    pub plan: &'a NpcPlan,
    /// What the action machine reads of the fighter (`can_start`), a leap's landing toward the
    /// target included.
    pub view: FighterView<'a>,
    pub dt: f32,
    /// What is left of the target's collision budget for this NPC; `None`: no budget.
    pub budget: Option<f32>,
    /// The other NPCs' attacks in flight on the target: the holder and the attack's tags.
    pub overlapping: Vec<(AiId, Vec<String>)>,
    pub director: DirectorDef,
    /// Another NPC of its director began a strike less than `min_spacing` ago: who, and the ticks
    /// left. `None`: it may strike, or its plan `ignores_spacing`.
    pub spacing: Option<(AiId, u64)>,
    /// The options its `when` conditions open (only NPCs whose plan has gated options carry it).
    pub open: Option<&'a NpcOptions>,
}

/// The one builder of [`Inputs`], from the read-only view of a fighting NPC, its target as the
/// NPC perceives it (`(id, where, body radius)`), the strike notes and the attacks in flight its
/// caller collected. `None`: the NPC has no attack plan in content.
pub(crate) fn inputs<'a>(
    f: &'a FightingReadOnlyItem<'_, '_>,
    cx: &FightCx<'a>,
    target: (AiId, Vec3, f32),
    notes: &[Note],
    inflight: &InFlight,
) -> Option<Inputs<'a>> {
    let plan_def = f.npc.plan.as_deref().and_then(|p| cx.db.get::<AttackPlanDef>(p))?;
    let director = DirectorDef::of(cx.db, &f.npc.director);
    let (id, there, radius) = target;
    let site = npc_site(cx.physics, cx.db, f.entity, f.transform, f.intent, f.body, Some((there, radius)));
    let view = npc_view(cx.db, f.fighter, f.state, &f.drive.held, f.dead, (f.stamina, f.cooldowns, f.charges), f.motion, site);
    let (budget, overlapping) = inflight.on(id, *f.id, &director);
    // Another NPC of its director began a strike less than `min_spacing` ago: its strikes wait.
    let spacing = if plan_def.ignores_spacing {
        None
    } else {
        waiting(notes, *f.id, &pool(&f.npc.director), cx.tick, spacing_ticks(director.min_spacing, cx.dt))
    };
    Some(Inputs { plan_def, plan: f.plan, view, dt: cx.dt, budget, overlapping, director, spacing, open: f.options })
}

/// The strike notes the planner judges by: every NPC's noted strike and pending press
/// ([`note_list`]), then the presses already in an NPC's drive this tick that start a strike now
/// (a state hook's, a combo continuation's), as starts of this tick.
pub(crate) fn plan_notes<'w, 's>(
    db: &ContentDb,
    npcs: impl IntoIterator<Item = FightingReadOnlyItem<'w, 's>>,
    tick: u64,
    dt: f32,
) -> Vec<Note> {
    let npcs: Vec<_> = npcs.into_iter().collect();
    let mut notes = note_list(
        npcs.iter().map(|f| (*f.id, f.npc.director.as_str(), f.strike, f.token.held.as_ref(), chain_note(f.chain, Some(f.buffer), tick))),
    );
    // Presses already made this tick (a hook's, a continuation's) are starts of this tick.
    notes.extend(npcs.iter().filter(|f| strike_pressed(db, f, dt)).map(|f| (*f.id, pool(&f.npc.director), tick)));
    notes
}

/// The players the director counts: alive.
pub(crate) type Players<'w, 's> = Query<'w, 's, (), (With<PlayerId>, Without<Dead>)>;

/// How many players the director counts ([`Players`]).
pub(crate) fn living_players(players: &Players) -> u32 {
    players.iter().count() as u32
}

#[cfg(test)]
mod tests {
    use ai_core::AiIndex;
    use bevy::ecs::system::RunSystemOnce;
    use shards_action::{ActionDef, Fighter, MovesetDef};

    use super::*;
    use crate::brain::Npc;
    use crate::director::{FightingReadOnly, Grant};
    use crate::strike::NpcStrike;

    const DT: f32 = 1.0 / 60.0;
    const TICK: u64 = 100;
    const TARGET: AiId = AiId(999);

    /// Director `arena` spaces strikes 0.7 s apart (42 ticks at 60 Hz) and gives a target a
    /// budget of 2; plan `p` attacks with `a` (`act_a`), plan `solo` too but ignores the spacing.
    fn world() -> World {
        let mut kinds = shards_content::ContentKinds::default();
        kinds.register::<ActionDef>();
        kinds.register::<MovesetDef>();
        kinds.register::<DirectorDef>();
        kinds.register::<AttackPlanDef>();
        let source = |path: &str, text: &str| shards_content::db::Source { path: path.into(), text: text.into() };
        let db = ContentDb::build(
            &kinds,
            &[
                source("content/t.action.ron", r#"{ "act_a": (net_id: 1, phases: [("swing", 0.5)]) }"#),
                source("content/t.moveset.ron", r#"{ "m": (inputs: {"a": "act_a"}) }"#),
                source("content/t.director.ron", r#"{ "arena": (min_spacing: 0.7, budget: Some(2.0)) }"#),
                source(
                    "content/t.attack_plan.ron",
                    r#"{ "p": (options: [(input: Some("a"))]), "solo": (options: [(input: Some("a"))], ignores_spacing: true) }"#,
                ),
            ],
        );
        let mut w = World::new();
        w.init_resource::<AiIndex>();
        w.insert_resource(db);
        w.init_resource::<PhysicsWorld>();
        w
    }

    fn npc(w: &mut World, plan: &str) -> (Entity, AiId) {
        let e = w.spawn((Npc::default().with_director("arena").with_plan(plan), Fighter::new("m"), Transform::default())).id();
        (e, *w.get::<AiId>(e).unwrap())
    }

    /// What `inputs` built for each NPC, in `AiId` order, from the planner's notes
    /// (`plan_notes`) and the attacks in flight: `None` without a plan in content, else the
    /// budget left, the attacks in flight on the target and the spacing.
    type Built = Vec<(AiId, Option<(Option<f32>, Vec<(AiId, Vec<String>)>, Option<(AiId, u64)>)>)>;

    fn built(w: &mut World) -> Built {
        w.run_system_once(|db: Res<ContentDb>, physics: Res<PhysicsWorld>, npcs: Query<FightingReadOnly>| -> Built {
            let cx = FightCx { db: &db, physics: &physics, tick: TICK, dt: DT };
            let inflight = InFlight::collect(npcs.iter().map(|f| (*f.id, f.token.held.as_ref())));
            let notes = plan_notes(&db, npcs.iter(), TICK, DT);
            let mut out: Built = npcs
                .iter()
                .map(|f| {
                    let built = inputs(&f, &cx, (TARGET, Vec3::ZERO, 0.4), &notes, &inflight);
                    (*f.id, built.map(|i| (i.budget, i.overlapping.clone(), i.spacing)))
                })
                .collect();
            out.sort_by_key(|(id, _)| *id);
            out
        })
        .unwrap()
    }

    /// The one builder reads the plan, the budget and the overlaps from the attacks in flight on
    /// the target (never the NPC's own), the spacing from the strike notes (a press made this
    /// tick among them, through `plan_notes`), and none for a plan that ignores the spacing.
    #[test]
    fn inputs_reads_the_plan_the_attacks_in_flight_and_the_strike_notes() {
        let mut w = world();
        // `striker` began a heavy strike 10 ticks ago and still swings at the target.
        let (striker, striker_id) = npc(&mut w, "p");
        w.entity_mut(striker).insert(NpcStrike { last_start: TICK - 10, gated: true, pool: "arena".into() });
        let grant = Grant { director: "arena".into(), cost: 1.5, tags: vec!["heavy".into()], pressed: Some(TICK - 12), started: true, ..Grant::for_tests(TARGET) };
        w.get_mut::<crate::director::AttackToken>(striker).unwrap().held = Some(grant);
        let (_, waiter) = npc(&mut w, "p");
        let (_, solo) = npc(&mut w, "solo");
        let (_, gone) = npc(&mut w, "gone");
        let (presser, presser_id) = npc(&mut w, "p");
        let heavy = vec![(striker_id, vec!["heavy".to_owned()])];
        let found = built(&mut w);
        let of = |found: &Built, id: AiId| found.iter().find(|(i, _)| *i == id).unwrap().1.clone();
        assert_eq!(of(&found, striker_id), Some((Some(2.0), Vec::new(), None)), "its own attack is not in its way");
        assert_eq!(of(&found, waiter), Some((Some(0.5), heavy.clone(), Some((striker_id, 32)))), "42 ticks, 10 gone");
        assert_eq!(of(&found, solo), Some((Some(0.5), heavy.clone(), None)), "a plan that ignores the spacing");
        assert_eq!(of(&found, gone), None, "no plan in content");
        // A press already in a drive this tick is a start of this tick for the others.
        w.get_mut::<crate::brain::NpcDrive>(presser).unwrap().press.push("a".into());
        let found = built(&mut w);
        assert_eq!(of(&found, waiter), Some((Some(0.5), heavy.clone(), Some((presser_id, 42)))));
        assert_eq!(of(&found, presser_id).unwrap().2, Some((striker_id, 32)), "its own press does not hold it back");
    }
}
