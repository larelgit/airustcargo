//! The sandbox warden's brain as data: attacks chosen by what reaches, with `SimRng`; never
//! two zone attacks in a row; a rest after each; home when the target leaves its leash; and, as a
//! game does it, its plan forgotten once it is home again (N8c).

use bevy::prelude::*;
use shards_action::ActionDef;
use shards_content::ContentDb;
use shards_core::NetRole;
use shards_npc::prelude::*;
use shards_npc::{reset_plan, set_plan_cooldown, BehaviorDef, CooldownSet, NpcPlan, NpcStateChanged};

use crate::common::*;

#[test]
fn the_warden_as_data() {
    let mut app = app();
    record_starts(&mut app);
    let post = Vec3::new(0.0, 0.0, -3.0);
    let world = app.world_mut();
    let warden = npc(world, "warden", Npc::new("warden", "boss").with_plan("warden").with_director("boss"), post, S);
    home(world, warden, post, 12.0);
    let hero = player(world, 0, Vec3::ZERO);
    step(&mut app, 5400);

    let starts: Vec<(u64, String)> = app
        .world()
        .resource::<Started>()
        .0
        .iter()
        .filter(|(_, who, _)| *who == warden)
        .map(|(t, _, a)| (*t, a.clone()))
        .collect();
    assert!(starts.len() >= 25, "a 90 s fight: {} attacks", starts.len());
    let zone = |a: &str| tags(&app, a).iter().any(|t| t == "zone");
    let db = app.world().resource::<ContentDb>();
    for pair in starts.windows(2) {
        let ((t0, a), (t1, b)) = (&pair[0], &pair[1]);
        assert!(!(zone(a) && zone(b)), "two zone attacks in a row: {a} at {t0}, {b} at {t1}");
        // The plan rests 1.0-1.5 s after an attack ends.
        let length = db.get::<ActionDef>(a).unwrap().length();
        let gap = (t1 - t0) as f32 * DT;
        assert!(gap >= length + 1.0 - 0.02, "{b} {gap:.2}s after {a} ({length:.2}s long): no rest");
    }
    for attack in ["slash", "slam", "sweep", "shockwave"] {
        assert!(starts.iter().any(|(_, a)| a == attack), "{attack} never played: {starts:?}");
    }

    // The target walks out of the warden's leash: it goes back to its post.
    teleport(&mut app, hero, Vec3::new(0.0, 0.0, 20.0));
    step(&mut app, 600);
    assert_eq!(state(&app, warden), "guard");
    assert!(flat_distance(feet(&app, warden), post) < 0.7, "at its post: {}", feet(&app, warden));
}

/// What a game does when a boss is home again (N8c, the game's `reset_boss`): an observer of
/// `NpcStateChanged` that sees an NPC leave a state of kind `return_home` resets its plan. Like every
/// observer that changes gameplay, it returns at once on a machine that does not simulate.
fn reset_when_home(app: &mut App) {
    app.add_observer(
        |e: On<NpcStateChanged>, npcs: Query<&Npc>, db: Res<ContentDb>, role: Option<Res<NetRole>>, mut commands: Commands| {
            if !shards_core::simulates(role) {
                return;
            }
            let Ok(npc) = npcs.get(e.entity) else { return };
            if db.get::<BehaviorDef>(&npc.behavior).is_some_and(|b| b.kind_of(&e.from) == "return_home") {
                let entity = e.entity;
                commands.queue(move |w: &mut World| {
                    reset_plan(w, entity, "home").unwrap();
                });
            }
        },
    );
}

#[test]
fn a_game_resets_the_wardens_plan_when_it_is_home_again() {
    let mut app = app_with(|app| {
        record_starts(app);
        reset_when_home(app);
    });
    let post = Vec3::new(0.0, 0.0, -3.0);
    let world = app.world_mut();
    let warden = npc(world, "warden", Npc::new("warden", "boss").with_plan("warden").with_director("boss"), post, S);
    home(world, warden, post, 12.0);
    let hero = player(world, 0, Vec3::ZERO);
    let plan = |app: &App| app.world().get::<NpcPlan>(warden).unwrap().clone();
    step(&mut app, 900);
    let fought = plan(&app);
    assert!(!fought.history.is_empty() && fought.attacks >= 3, "it fought: {fought:?}");
    // A cooldown the game set, to see it go too.
    set_plan_cooldown(app.world_mut(), warden, "light", 30.0, CooldownSet::Exactly).unwrap();
    assert!(lines(&app, "plan reset").is_empty(), "nothing resets it while it fights");

    teleport(&mut app, hero, Vec3::new(0.0, 0.0, 20.0));
    step(&mut app, 600);
    assert_eq!(state(&app, warden), "guard");
    let home_again = plan(&app);
    assert!(home_again.history.is_empty() && home_again.cooldowns.is_empty(), "{home_again:?}");
    assert!(home_again.attacks >= fought.attacks, "the count stays");
    let id = id(&app, warden);
    let said = lines(&app, "plan reset");
    assert!(said.len() == 1 && said[0].contains(&format!("{id} plan reset (home): ")), "once, on arrival: {said:?}");
    assert!(said[0].ends_with(", 1 cooldown cleared"), "{said:?}");
}
