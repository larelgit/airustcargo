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
