//! Props on the humanoid's bones: sockets, carry, mounts, the equip of a weapon switch, clip
//! families and the socket lookup, on the headless engine with the real model.

use shards::action::render::{AnimationBound, PropOf, Sockets};
use shards::action::{force_action, Animated, Parried, PropSwaps};
use shards::bevy::animation::AnimationPlayer;
use shards::bevy::ecs::system::RunSystemOnce;
use shards::bevy::world_serialization::WorldAssetRoot;
use shards::motion::{Collider, Interpolated, Shape};
use shards::prelude::*;
use shards::serde_json::json;
use shards::testing::ok;

use crate::fixtures::{kit_assets, skinned_gltf, KIT, KIT_PROPS};
use crate::headless::{progress, spawn_model, update_until};

const HUMANOID: &str = "models/humanoid.glb";

/// The kit's app with a bound hero, standing on a floor, its sword drawn.
fn kit_app(name: &str) -> (App, Entity) {
    kit_app_with(name, KIT, KIT_PROPS, HUMANOID)
}

fn kit_app_with(name: &str, animsets: &str, props: &str, model: &'static str) -> (App, Entity) {
    let dir = kit_assets(name, animsets);
    std::fs::write(dir.join("content/anim/props.prop.ron"), props).unwrap();
    std::fs::write(dir.join("models/model.gltf"), skinned_gltf(false)).unwrap();
    std::fs::write(
        dir.join("models/statue.gltf"),
        r#"{"asset": {"version": "2.0"}, "scene": 0, "scenes": [{"nodes": [0]}],
            "nodes": [{"name": "root", "children": [1]}, {"name": "hand.R"}]}"#,
    )
    .unwrap();
    let mut app = shards::testing::test_app(|config| config.asset_dir = dir.to_string_lossy().into_owned(), |_| {});
    let errors: Vec<String> = app.world().resource::<ContentDb>().errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "the kit and its files are valid content: {errors:#?}");
    app.world_mut().spawn((Collider::new(Shape::cuboid(Vec3::new(30.0, 0.5, 30.0))), Transform::from_xyz(0.0, -0.5, 0.0)));
    let hero = spawn_model(&mut app, Vec3::ZERO, model);
    update_until(&mut app, "the hero bound", |app| app.world().get::<AnimationBound>(hero).is_some());
    app.update();
    (app, hero)
}

fn bound(app: &App, hero: Entity) -> &AnimationBound {
    app.world().get::<AnimationBound>(hero).unwrap()
}

/// The prop `id`, once it is drawn (waits a few frames for the bridge).
fn drawn(app: &mut App, hero: Entity, id: &str) -> Entity {
    update_until(app, &format!("prop {id} drawn"), |app| bound(app, hero).prop(id).is_some());
    app.update();
    bound(app, hero).prop(id).unwrap()
}

fn name_of(app: &App, entity: Entity) -> String {
    app.world().get::<Name>(entity).map(|n| n.to_string()).unwrap_or_default()
}

/// The name of the bone `prop` hangs from.
fn hangs_from(app: &App, prop: Entity) -> String {
    let parent = app.world().get::<ChildOf>(prop).expect("a prop has a bone").parent();
    name_of(app, parent)
}

fn hidden(app: &App, prop: Entity) -> bool {
    app.world().get::<Visibility>(prop).is_some_and(|v| *v == Visibility::Hidden)
}

fn prop_count(app: &mut App) -> usize {
    let world = app.world_mut();
    world.query::<&PropOf>().iter(world).count()
}

fn frames(app: &mut App, ticks: u32) {
    ok(app, "step", json!({"ticks": ticks}));
    app.update();
    app.update();
}

fn is_below(app: &App, mut entity: Entity, ancestor: Entity) -> bool {
    while let Some(parent) = app.world().get::<ChildOf>(entity) {
        entity = parent.parent();
        if entity == ancestor {
            return true;
        }
    }
    false
}

#[test]
fn a_carried_sword_rides_the_right_hand_bone() {
    let (mut app, hero) = kit_app("sockets_carry");
    let sword = drawn(&mut app, hero, "sword");
    assert_eq!(hangs_from(&app, sword), "hand.R");
    assert!(!hidden(&app, sword));
    let prop = app.world().get::<PropOf>(sword).unwrap();
    assert_eq!((prop.body, prop.prop.as_str()), (hero, "sword"));
    assert!(app.world().get::<AiId>(sword).is_none(), "a picture: no AiId, so no replay, snapshot or co-op ever sees it");
    // Held as its definition says: the grip moves it from the socket.
    let local = app.world().get::<Transform>(sword).unwrap();
    assert!(local.translation.distance(Vec3::new(0.0, 0.3, 0.0)) < 1e-6, "{local:?}");
    // One sword, not the greatsword of another moveset.
    assert_eq!(bound(&app, hero).props().map(|(p, _)| p).collect::<Vec<_>>(), ["sword"]);
    assert_eq!(prop_count(&mut app), 1);
}

#[test]
fn the_drink_moves_the_sword_to_the_hip_and_back() {
    let (mut app, hero) = kit_app("sockets_drink");
    let sword = drawn(&mut app, hero, "sword");
    app.world_mut().get_mut::<Intent>(hero).unwrap().press("drink");
    frames(&mut app, 18); // 0.3 s into the drink: the sword is on the hip from 0.2 s to 0.8 s
    let p = progress(&app, hero).expect("the drink is running");
    assert!(p > 0.15 && p < 0.35, "{p}");
    assert_eq!(bound(&app, hero).prop("sword"), Some(sword), "moved, not spawned again");
    assert_eq!(hangs_from(&app, sword), "hips");
    let local = app.world().get::<Transform>(sword).unwrap();
    let want = Quat::from_rotation_y(90f32.to_radians()) * Vec3::new(0.0, 0.3, 0.0) + Vec3::new(0.25, 0.0, 0.0);
    assert!(local.translation.distance(want) < 1e-5, "the hip socket's offset and turn: {:?} vs {want:?}", local.translation);
    // Before 0.2 s it was in the hand: a run just started puts it there.
    frames(&mut app, 60);
    assert!(progress(&app, hero).is_none(), "the drink is over");
    assert_eq!(hangs_from(&app, sword), "hand.R", "back in the hand after 0.8 s");
}

#[test]
fn a_cut_drink_keeps_the_sword_at_the_hip_for_its_cut_hold() {
    let (mut app, hero) = kit_app("sockets_cut");
    let sword = drawn(&mut app, hero, "sword");
    app.world_mut().get_mut::<Intent>(hero).unwrap().press("drink");
    frames(&mut app, 18);
    assert_eq!(hangs_from(&app, sword), "hips");
    // A roll cuts the drink: the sword stays on the hip for the first 0.1 s of the roll...
    assert!(force_action(app.world_mut(), hero, "roll"));
    app.update();
    assert_eq!(hangs_from(&app, sword), "hips", "held at the cut");
    frames(&mut app, 3); // 0.05 s into the roll
    assert_eq!(hangs_from(&app, sword), "hips", "still held");
    // ...then goes where the roll's place for it is: the hand.
    frames(&mut app, 6); // 0.15 s
    assert_eq!(hangs_from(&app, sword), "hand.R", "the hold is over");
    // A drink that ran its course is no cut: nothing held.
    frames(&mut app, 30);
    app.world_mut().get_mut::<Intent>(hero).unwrap().press("drink");
    frames(&mut app, 80);
    assert!(progress(&app, hero).is_none());
    assert_eq!(hangs_from(&app, sword), "hand.R");
}

#[test]
fn a_moveset_change_shows_the_previous_props_inside_the_previous_span() {
    let (mut app, hero) = kit_app("sockets_equip");
    let sword = drawn(&mut app, hero, "sword");
    app.world_mut().get_mut::<Fighter>(hero).unwrap().moveset = "brute".into();
    app.update();
    // 0 - 0.1 s: the old weapon is still shown, in the hand; the new one is not drawn yet.
    assert!(bound(&app, hero).equipping());
    assert_eq!(bound(&app, hero).prop("sword"), Some(sword));
    assert_eq!(hangs_from(&app, sword), "hand.R");
    assert!(bound(&app, hero).prop("great").is_none());
    // The sword goes to the back from 0.05 s to 0.1 s.
    frames(&mut app, 4); // 0.067 s
    assert_eq!(hangs_from(&app, sword), "spine");
    // After the previous span: the greatsword is in the hand, and the sword is gone.
    frames(&mut app, 4); // 0.13 s
    let great = drawn(&mut app, hero, "great");
    assert_eq!(hangs_from(&app, great), "hand.R");
    assert!(!bound(&app, hero).equipping(), "the equip ran its course");
    assert!(bound(&app, hero).prop("sword").is_none());
    app.update();
    assert!(app.world().get_entity(sword).is_err(), "the old weapon's entity is despawned");
}

#[test]
fn an_attack_just_after_a_switch_shows_the_new_weapon_in_hand() {
    let (mut app, hero) = kit_app("sockets_attack");
    drawn(&mut app, hero, "sword");
    app.world_mut().get_mut::<Fighter>(hero).unwrap().moveset = "brute".into();
    app.world_mut().get_mut::<Intent>(hero).unwrap().press("light");
    frames(&mut app, 2); // 0.03 s: the equip would still be open
    assert!(progress(&app, hero).is_some(), "the greatsword's swing is running");
    assert!(!bound(&app, hero).equipping(), "a whole-body action ends the equip");
    let great = drawn(&mut app, hero, "great");
    assert_eq!(hangs_from(&app, great), "hand.R");
    assert!(bound(&app, hero).prop("sword").is_none());
}

#[test]
fn a_parried_hit_ends_the_equip() {
    let (mut app, hero) = kit_app("sockets_parried");
    drawn(&mut app, hero, "sword");
    app.world_mut().get_mut::<Fighter>(hero).unwrap().moveset = "brute".into();
    app.update();
    assert!(bound(&app, hero).equipping());
    app.world_mut().trigger(Parried { attacker: hero, defender: hero, action: "light_1".into(), stamina: 0.0, source: None });
    app.update();
    assert!(!bound(&app, hero).equipping(), "the equip ended at once, at the switch's first moment");
    let great = drawn(&mut app, hero, "great");
    assert_eq!(hangs_from(&app, great), "hand.R");
    // Another switch opens the equip again, and the listed event ends that one too.
    app.world_mut().get_mut::<Fighter>(hero).unwrap().moveset = "hero".into();
    app.update();
    assert!(bound(&app, hero).equipping());
    app.world_mut().trigger(Parried { attacker: hero, defender: hero, action: "light_1".into(), stamina: 0.0, source: None });
    app.update();
    assert!(!bound(&app, hero).equipping());
}

#[test]
fn a_moveset_family_renames_every_pick() {
    let (mut app, hero) = kit_app("sockets_family");
    update_until(&mut app, "idle", |app| bound(app, hero).clip() == Some("idle"));
    app.world_mut().get_mut::<Fighter>(hero).unwrap().moveset = "brute".into();
    app.update();
    app.update();
    assert_eq!(bound(&app, hero).clip(), Some("walk"), "the brute's standing clip is the walk");
    app.world_mut().get_mut::<Fighter>(hero).unwrap().moveset = "hero".into();
    app.update();
    app.update();
    assert_eq!(bound(&app, hero).clip(), Some("idle"), "a moveset the table does not list shows the clips as named");
}

#[test]
fn a_prop_swap_shows_the_variant() {
    let (mut app, hero) = kit_app("sockets_swap");
    let sword = drawn(&mut app, hero, "sword");
    assert_eq!(app.world().get::<PropOf>(sword).unwrap().prop, "sword");
    app.world_mut().entity_mut(hero).insert(PropSwaps([("sword".to_owned(), "gilded_sword".to_owned())].into()));
    app.update();
    app.update();
    let gilded = bound(&app, hero).prop("sword").unwrap();
    assert_eq!(app.world().get::<PropOf>(gilded).unwrap().prop, "gilded_sword");
    assert!(app.world().get_entity(sword).is_err(), "the plain sword's entity is gone, one weapon drawn");
    assert_eq!(prop_count(&mut app), 1);
    app.world_mut().entity_mut(hero).remove::<PropSwaps>();
    app.update();
    app.update();
    let plain = bound(&app, hero).prop("sword").unwrap();
    assert_eq!(app.world().get::<PropOf>(plain).unwrap().prop, "sword");
}

#[test]
fn the_socket_lookup_gives_the_bone_and_offset() {
    let (mut app, hero) = kit_app("sockets_lookup");
    drawn(&mut app, hero, "sword");
    let (bone, local) = app.world_mut().run_system_once(move |s: Sockets| s.bone(hero, "hip")).unwrap().expect("the hip socket");
    assert_eq!(name_of(&app, bone), "hips");
    assert!(local.translation.distance(Vec3::new(0.25, 0.0, 0.0)) < 1e-6);
    assert!(local.rotation.angle_between(Quat::from_rotation_y(90f32.to_radians())) < 1e-3);
    let (hand, _) = app.world_mut().run_system_once(move |s: Sockets| s.bone(hero, "hand_r")).unwrap().unwrap();
    assert_eq!(name_of(&app, hand), "hand.R");
    assert!(app.world_mut().run_system_once(move |s: Sockets| s.bone(hero, "nowhere")).unwrap().is_none());
    let stranger = app.world_mut().spawn_empty().id();
    assert!(app.world_mut().run_system_once(move |s: Sockets| s.bone(stranger, "hip")).unwrap().is_none());
    // In the world: the bone's place with the socket's offset on it.
    app.update();
    let world = app.world_mut().run_system_once(move |s: Sockets| s.world(hero, "hip")).unwrap();
    assert!(world.is_some(), "propagated at least once");
}

#[test]
fn a_prop_comes_back_after_the_model_is_spawned_again() {
    let (mut app, hero) = kit_app("sockets_respawn");
    let sword = drawn(&mut app, hero, "sword");
    // The model is spawned again (a hot reload of the glTF): its bones, and the sword below one, go.
    let model: Vec<Entity> = {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, With<WorldAssetRoot>>();
        q.iter(world).filter(|e| world.get::<ChildOf>(*e).is_some_and(|c| c.parent() == hero)).collect()
    };
    assert_eq!(model.len(), 1);
    let scene = app.world().get::<WorldAssetRoot>(model[0]).unwrap().0.clone();
    app.world_mut().entity_mut(model[0]).despawn();
    app.world_mut().entity_mut(hero).with_child(WorldAssetRoot(scene));
    update_until(&mut app, "the sword drawn again", |app| bound(app, hero).prop("sword").is_some_and(|e| e != sword));
    app.update();
    let again = bound(&app, hero).prop("sword").unwrap();
    assert_eq!(hangs_from(&app, again), "hand.R");
    assert_eq!(prop_count(&mut app), 1, "one sword, not two");
}

#[test]
fn editing_a_socket_offset_moves_the_prop_without_a_second_copy() {
    let (mut app, hero) = kit_app("sockets_edit");
    let sword = drawn(&mut app, hero, "sword");
    let (player, graph) = {
        let b = bound(&app, hero);
        (b.player, app.world().get::<shards::bevy::animation::graph::AnimationGraphHandle>(b.player).unwrap().id())
    };
    let before = ok(&mut app, "content_get", json!({"kind": "animset", "id": "humanoid"})).result["def"].clone();
    let mut moved = before.clone();
    moved["sockets"]["hand_r"]["offset"] = json!([0.0, 0.5, 0.0]);
    ok(&mut app, "content_apply", json!({"changes": [{"kind": "animset", "id": "humanoid", "def": moved, "before": before}]}));
    app.update();
    app.update();
    assert_eq!(bound(&app, hero).player, player, "no rebind: only the sockets changed");
    assert_eq!(app.world().get::<shards::bevy::animation::graph::AnimationGraphHandle>(player).unwrap().id(), graph);
    assert_eq!(bound(&app, hero).prop("sword"), Some(sword), "the same entity");
    assert_eq!(prop_count(&mut app), 1, "no second copy");
    let local = app.world().get::<Transform>(sword).unwrap();
    assert!(local.translation.distance(Vec3::new(0.0, 0.8, 0.0)) < 1e-5, "the new offset plus the grip: {:?}", local.translation);
    // An edit of the clips does rebuild the graph, and the props go over to the new binding.
    let before = ok(&mut app, "content_get", json!({"kind": "animset", "id": "humanoid"})).result["def"].clone();
    let mut louder = before.clone();
    louder["clips"]["walk"]["cues"] = json!([{"at": 0.1, "cue": "step"}]);
    ok(&mut app, "content_apply", json!({"changes": [{"kind": "animset", "id": "humanoid", "def": louder, "before": before}]}));
    app.update();
    app.update();
    app.update();
    assert_eq!(bound(&app, hero).prop("sword"), Some(sword), "a rebind keeps the props it had");
    assert_eq!(prop_count(&mut app), 1);
}

#[test]
fn a_prop_model_gets_no_player_and_never_shadows_a_bone() {
    let props = r#"{ "sword": (size: (0.05, 0.9, 0.02)), "great": (size: (0.1, 1.4, 0.04)), "gilded_sword": (size: (0.1, 1.0, 0.04)),
        "statue": (model: "models/statue.gltf") }"#;
    // The statue's scene has nodes named `root` and `hand.R`, as the rig does.
    let animsets = KIT.replace(r#"carry: {"hero": [(prop: "sword", socket: "hand_r")]"#, r#"carry: {"hero": [(prop: "statue", socket: "back"), (prop: "sword", socket: "hand_r")]"#);
    let (mut app, hero) = kit_app_with("sockets_shadow", &animsets, props, HUMANOID);
    let statue = drawn(&mut app, hero, "statue");
    update_until(&mut app, "the statue's scene spawned", |app| {
        let world = app.world();
        let mut names = world.try_query::<(Entity, &Name)>().unwrap();
        names.iter(world).any(|(e, n)| n.as_str() == "hand.R" && is_below(app, e, statue))
    });
    for _ in 0..3 {
        app.update();
    }
    // No player or animation target below the prop: its model is not a rig.
    let below: Vec<Entity> = {
        let world = app.world_mut();
        let mut q = world.query_filtered::<Entity, With<AnimationPlayer>>();
        q.iter(world).collect()
    };
    assert_eq!(below.len(), 1, "one player in the whole scene: the humanoid's, {below:?}");
    assert!(!is_below(&app, below[0], statue));
    // The sword's socket is the humanoid's right hand, not the statue's node of that name.
    let hand = bound(&app, hero).socket_bone("hand_r").expect("the hand bone");
    assert!(!is_below(&app, hand, statue), "the right hand is a bone of the rig");
    let sword = drawn(&mut app, hero, "sword");
    assert_eq!(app.world().get::<ChildOf>(sword).unwrap().parent(), hand);
}

#[test]
fn a_prop_is_never_adopted_for_interpolation() {
    let (mut app, hero) = kit_app("sockets_interp");
    let sword = drawn(&mut app, hero, "sword");
    assert!(app.world().get::<Interpolated>(sword).is_none());
    assert_ne!(app.world().get::<ChildOf>(sword).unwrap().parent(), hero, "never a direct child of the body, the only ones adopted");
    assert!(is_below(&app, sword, hero), "but below it: it rides the model child's blended pose with the bones");
}

#[test]
fn a_missing_socket_bone_is_warned_once_and_skipped() {
    // The model has no `hand.R` (it is the one-triangle skinned model), though the set's file does.
    let (mut app, hero) = kit_app_with("sockets_missing", KIT, KIT_PROPS, "models/model.gltf");
    for _ in 0..5 {
        app.update();
    }
    assert!(bound(&app, hero).socket_bone("hand_r").is_none());
    assert!(bound(&app, hero).prop("sword").is_none(), "no bone, no sword: skipped");
    assert_eq!(prop_count(&mut app), 0);
    assert!(app.world().get::<Animated>(hero).is_some(), "the body goes on animating");
}

/// The drink cut by something other than a roll: the sword stays on the hip for the cut hold
/// whenever the cutter is a whole-body run of its own.
fn drink_cut_by(name: &str, cut: impl FnOnce(&mut App, Entity)) {
    let (mut app, hero) = kit_app(name);
    let sword = drawn(&mut app, hero, "sword");
    app.world_mut().get_mut::<Intent>(hero).unwrap().press("drink");
    frames(&mut app, 18);
    assert_eq!(hangs_from(&app, sword), "hips", "{name}: the drink put it on the hip");
    cut(&mut app, hero);
    app.update();
    assert!(progress(&app, hero).is_some(), "{name}: the cutter runs");
    assert_eq!(hangs_from(&app, sword), "hips", "{name}: held at the cut");
    frames(&mut app, 9); // 0.15 s later: the hold (0.1 s) is over
    assert_ne!(hangs_from(&app, sword), "hips", "{name}: the hold is over");
}

#[test]
fn a_drink_cut_by_a_reaction_a_stagger_or_death_holds_the_sword_too() {
    use shards::action::{kill, react, stagger, Reaction, StaggerCause};
    drink_cut_by("sockets_cut_react", |app, hero| {
        let r = Reaction { tier: 1, from: None, dir: Vec3::X, knockback: 0.0, ignore_immunity: true, ignore_armor: true, armor: None, sets_immunity: false };
        assert!(react(app.world_mut(), hero, r), "the flinch");
    });
    drink_cut_by("sockets_cut_stagger", |app, hero| {
        assert!(stagger(app.world_mut(), hero, StaggerCause::Dev));
    });
    drink_cut_by("sockets_cut_death", |app, hero| kill(app.world_mut(), hero, None, None));
}

#[test]
fn an_event_the_rules_do_not_list_leaves_the_equip_open() {
    use shards::action::{Blocked, HitWeight};
    let (mut app, hero) = kit_app("sockets_unlisted");
    drawn(&mut app, hero, "sword");
    app.world_mut().get_mut::<Fighter>(hero).unwrap().moveset = "brute".into();
    app.update();
    assert!(bound(&app, hero).equipping());
    // The kit lists only `parried`: a block of another fighter's hit is not one of them.
    let stranger = app.world_mut().spawn_empty().id();
    app.world_mut().trigger(Blocked {
        attacker: stranger,
        defender: hero,
        action: "light_1".into(),
        amount: 0.0,
        stamina: 0.0,
        hp_left: 100.0,
        weight: HitWeight::Light,
        absorbed: 0.0,
        source: None,
    });
    app.update();
    assert!(bound(&app, hero).equipping(), "an unlisted event leaves the equip open");
    // The same event aimed at someone else changes nothing either.
    app.world_mut().trigger(Parried { attacker: hero, defender: stranger, action: "light_1".into(), stamina: 0.0, source: None });
    app.update();
    assert!(bound(&app, hero).equipping(), "a parry of another body is no event of this one");
}

#[test]
fn a_moveset_that_shows_up_late_is_the_baseline_not_a_switch() {
    let (mut app, hero) = kit_app("sockets_late");
    drawn(&mut app, hero, "sword");
    // A body whose fighter comes a frame after `Animated` shows no equip.
    let late = spawn_model(&mut app, Vec3::new(3.0, 0.0, 0.0), HUMANOID);
    app.world_mut().entity_mut(late).remove::<Fighter>();
    update_until(&mut app, "bound", |app| app.world().get::<AnimationBound>(late).is_some());
    app.update();
    assert!(!bound(&app, late).equipping());
    let fighter = app.world().get::<Fighter>(hero).unwrap().clone();
    app.world_mut().entity_mut(late).insert(fighter);
    app.update();
    app.update();
    assert!(!bound(&app, late).equipping(), "the first moveset is the baseline");
    drawn(&mut app, late, "sword");
}

#[test]
fn the_tool_shows_what_the_bridge_drew_through_a_cut_hold_and_an_equip() {
    let (mut app, hero) = kit_app("sockets_tool");
    drawn(&mut app, hero, "sword");
    let said = |app: &mut App| ok(app, "animation", json!({"id": format!("e{}", app.world().get::<AiId>(hero).unwrap().0)})).result.to_string();
    app.world_mut().get_mut::<Intent>(hero).unwrap().press("drink");
    frames(&mut app, 18);
    assert!(force_action(app.world_mut(), hero, "roll"));
    app.update();
    // The roll's rules say the hand, the cut hold says the hip: the tool says what is drawn.
    assert_eq!(hangs_from(&app, bound(&app, hero).prop("sword").unwrap()), "hips");
    let text = said(&mut app);
    assert!(text.contains("sword at hip"), "{text}");
    frames(&mut app, 12);
    let text = said(&mut app);
    assert!(text.contains("sword at hand_r"), "{text}");
    // An equip: the old weapon shows during `previous`.
    app.world_mut().get_mut::<Fighter>(hero).unwrap().moveset = "brute".into();
    app.update();
    let text = said(&mut app);
    assert!(text.contains("sword at hand_r") && text.contains("equip: open"), "{text}");
}
