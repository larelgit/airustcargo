//! The behaviour framework: state machines from content, logic from hooks by name.

use bevy::prelude::*;
use shards_npc::prelude::*;

use crate::common::*;

/// A game's own fact about an NPC.
#[derive(Component, Reflect, Default, Clone, Copy, Debug)]
#[reflect(Component, Default)]
struct Flag(bool);

/// The game's condition: its flag is up.
fn flag_on(c: &NpcCtx, _: &Params) -> bool {
    c.get::<Flag>().is_some_and(|f| f.0)
}

/// The game's state: walk to `{x, z}`.
fn goto(_: &NpcCtx, p: &Params, out: &mut NpcOutput) {
    out.goal = Some(Vec3::new(p.num("x", 0.0), 0.0, p.num("z", 0.0)));
    out.within = 0.3;
    out.speed = 0.5;
}

pub fn hooked() -> App {
    app_with(|app| {
        app.register_type::<Flag>().add_npc_condition("flag_on", flag_on).add_npc_state("goto", goto);
    })
}

#[test]
fn transitions_follow_the_games_conditions_and_states() {
    let mut app = hooked();
    let e = npc(app.world_mut(), "grunt", Npc::new("flagged", "eyes"), Vec3::ZERO, N);
    app.world_mut().entity_mut(e).insert(Flag(false));
    step(&mut app, 10);
    assert_eq!(state(&app, e), "idle", "it starts in its initial state");
    app.world_mut().get_mut::<Flag>(e).unwrap().0 = true;
    step(&mut app, 1);
    assert_eq!(state(&app, e), "dance", "the game's condition took it there");
    step(&mut app, 150);
    let at = feet(&app, e);
    assert!(flat_distance(at, Vec3::new(3.0, 0.0, 0.0)) < 0.5, "the game's state walked it to (3, 0): {at}");

    app.world_mut().get_mut::<Flag>(e).unwrap().0 = false;
    step(&mut app, 1);
    assert_eq!(state(&app, e), "pause");
    // `duration: (0.5, 0.5)`: the timer runs out after 30 ticks.
    step(&mut app, 29);
    assert_eq!(state(&app, e), "pause");
    step(&mut app, 1);
    assert_eq!(state(&app, e), "idle");

    let me = id(&app, e);
    let story: Vec<String> = journal(&app).into_iter().filter(|l| l.contains(&me) && l.contains('→')).collect();
    let changes: Vec<&str> = story.iter().map(|l| l.split_once(' ').unwrap().1).collect();
    assert_eq!(changes, [format!("{me} idle → dance"), format!("{me} dance → pause"), format!("{me} pause → idle")]);
}

#[test]
fn one_transition_per_tick_and_the_first_that_holds() {
    let mut app = hooked();
    let e = npc(app.world_mut(), "grunt", Npc::new("flagged", "eyes"), Vec3::ZERO, N);
    app.world_mut().entity_mut(e).insert(Flag(true));
    // Tick 1 enters `idle` (its first state), tick 2 takes idle → dance; it never skips ahead.
    step(&mut app, 1);
    assert_eq!(state(&app, e), "idle");
    step(&mut app, 1);
    assert_eq!(state(&app, e), "dance");
}

#[test]
fn unknown_names_are_reported_once_with_the_closest_name() {
    let mut app = hooked();
    npc(app.world_mut(), "grunt", Npc::new("typo", "eyes"), Vec3::ZERO, N);
    npc(app.world_mut(), "grunt", Npc::new("nosuch", "eyes"), Vec3::new(3.0, 0.0, 0.0), N);
    step(&mut app, 30);
    let errors: Vec<String> = all_journal(&app).into_iter().filter(|l| l.contains("[error]")).collect();
    let typo: Vec<&String> = errors.iter().filter(|l| l.contains("unknown condition `flg_on`")).collect();
    assert_eq!(typo.len(), 1, "once, not every tick: {errors:#?}");
    assert!(typo[0].contains("(did you mean `flag_on`?)"), "{}", typo[0]);
    assert!(errors.iter().any(|l| l.contains("behavior `nosuch` is not in content; it stands still")), "{errors:#?}");
}

#[test]
fn ids_content_lacks_are_reported_with_what_they_mean() {
    let mut app = hooked();
    let npc_def = Npc::new("grunt", "eyez").with_plan("grnt").with_director("bos");
    npc(app.world_mut(), "grunt", npc_def, Vec3::ZERO, N);
    step(&mut app, 5);
    let errors: Vec<String> = all_journal(&app).into_iter().filter(|l| l.contains("[error]")).collect();
    for needle in [
        "attack_plan `grnt` is not in content (did you mean `grunt`?); it never attacks",
        "perception `eyez` is not in content (did you mean `eyes`?); it senses by the engine's defaults",
        "director `bos` is not in content (did you mean `boss`?); it fights by the engine's numbers",
    ] {
        assert_eq!(errors.iter().filter(|l| l.contains(needle)).count(), 1, "`{needle}` once in {errors:#?}");
    }
}

/// `engaged_npc` (the condition `npc_engaged` and the rules guard): the first fighting NPC in `AiId` order, and
/// only by the kind of the state, through the behaviour's content.
#[test]
fn engaged_npc_picks_the_lowest_id_and_reads_the_kind_through_the_content() {
    use ai_core::AiId;
    use shards_npc::ai::engaged_npc;
    let mut app = app();
    let pos = Vec3::new(0.0, 0.0, -6.0);
    let a = grunt(app.world_mut(), "sentry", "grunt", "default", pos);
    let b = grunt(app.world_mut(), "sentry", "grunt", "default", pos);
    let c = grunt(app.world_mut(), "sentry", "grunt", "default", pos);
    let (bid, cid) = (aid(&app, b), aid(&app, c));
    // Despawning the first swaps the last into its table row: the query now meets `c` before `b`.
    app.world_mut().despawn(a);
    let target = Some(AiId(99));
    let set = |app: &mut App, e: Entity, state: &str, active: bool, target: Option<AiId>| {
        let mut brain = app.world_mut().get_mut::<NpcBrain>(e).unwrap();
        brain.state = state.to_owned();
        brain.active = active;
        brain.target = target;
    };
    assert!(engaged_npc(app.world()).is_none());

    // `chase` of `sentry` is a fighting kind; the lower id wins whatever the query order.
    set(&mut app, b, "chase", true, target);
    set(&mut app, c, "chase", true, target);
    let order: Vec<AiId> = app.world_mut().query::<(&AiId, &NpcBrain)>().iter(app.world()).map(|(i, _)| *i).collect();
    assert_eq!(order, [cid, bid], "the query meets the higher id first, so the test can tell");
    let (id, behavior, state, t) = engaged_npc(app.world()).unwrap();
    assert_eq!((id, behavior.as_str(), state.as_str(), t), (bid, "sentry", "chase", target));

    // A state of another kind (`home` is `return_home`) with a target is not a fight; nor is an inactive NPC,
    // nor one without a target.
    set(&mut app, b, "home", true, target);
    set(&mut app, c, "chase", false, target);
    assert!(engaged_npc(app.world()).is_none(), "return_home and a dormant NPC do not hold");
    set(&mut app, c, "chase", true, None);
    assert!(engaged_npc(app.world()).is_none(), "no target");
    set(&mut app, c, "chase", true, target);
    assert_eq!(engaged_npc(app.world()).map(|e| e.0), Some(cid), "only the fighting one is named");
}
