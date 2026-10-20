//! Scene cameras (K3): a shot takes the camera from the orbit camera, moves between its keys,
//! follows its roles' bodies, blends back to the orbit camera at its end or when skipped, and is
//! refused as the module says.

use ai_core::{call_tool, AiJournal, CallOutcome};
use bevy::camera::Projection;
use bevy::input::mouse::AccumulatedMouseMotion;
use bevy::prelude::*;
use serde_json::{json, Value};
use shards_motion::camera::shot::Ease;
use shards_motion::camera::{ShotDef, ShotEnded, Shots};
use shards_motion::*;

use super::{cam, id, tap_lock, target, timed_scene, Scene};

/// - `intro`: from 10 m south of the origin and 5 m up to 10 m east of there over 2 s, looking at
///   the boss, narrowing to 40°; back to the orbit camera over 1 s.
/// - `follow`: 4 m south of the boss and 2 m up, looking at it, for 10 s; the default blend out.
/// - `locked`: a still view that cannot be skipped, with no blend out.
/// - `brief`: a still view far above and behind, 1 s long with a 3 s blend in: it ends first.
const SHOTS: &str = r#"{
    "intro": (
        keys: [
            (at: 0.0, eye: At((0.0, 5.0, 10.0)), look: On(role: "boss")),
            (at: 2.0, eye: At((10.0, 5.0, 10.0)), look: On(role: "boss"), fov_deg: 40.0),
        ],
        blend_out_s: 1.0,
    ),
    "follow": (keys: [
        (at: 0.0, eye: On(role: "boss", offset: (0.0, 2.0, 4.0)), look: On(role: "boss")),
        (at: 10.0, eye: On(role: "boss", offset: (0.0, 2.0, 4.0)), look: On(role: "boss")),
    ]),
    "locked": (
        keys: [
            (at: 0.0, eye: At((0.0, 9.0, 0.0)), look: At((0.0, 0.0, -5.0))),
            (at: 5.0, eye: At((0.0, 9.0, 5.0)), look: At((0.0, 0.0, -5.0))),
        ],
        skippable: false,
        blend_out_s: 0.0,
    ),
    "brief": (
        keys: [
            (at: 0.0, eye: At((0.0, 20.0, 30.0)), look: At((0.0, 0.0, 0.0))),
            (at: 1.0, eye: At((0.0, 20.0, 30.0)), look: At((0.0, 0.0, 0.0))),
        ],
        blend_in_s: 3.0,
        blend_out_s: 1.0,
    ),
}"#;

/// The shots that ended, in order.
#[derive(Resource, Default)]
struct Ended(Vec<ShotEnded>);

/// The camera scene with [`SHOTS`], the game clock running at 0.1 s a frame (a shot's time), the
/// orbit camera settled.
fn shots() -> Scene {
    use shards_content::db::Source;
    use shards_content::{ContentDb, ContentKinds};
    let mut s = timed_scene(false);
    let mut kinds = ContentKinds::default();
    kinds.register::<ShotDef>();
    let db = ContentDb::build(&kinds, &[Source { path: "content/scenes.shot.ron".into(), text: SHOTS.into() }]);
    let errors: Vec<String> = db.errors().map(ToString::to_string).collect();
    assert!(errors.is_empty(), "{errors:?}");
    s.app.insert_resource(db).init_resource::<Ended>();
    s.app.add_observer(|e: On<ShotEnded>, mut ended: ResMut<Ended>| ended.0.push(e.event().clone()));
    ai_core::sim::resume(s.app.world_mut());
    frames(&mut s, 10);
    s
}

fn frames(s: &mut Scene, n: usize) {
    for _ in 0..n {
        s.app.update();
    }
}

fn call(s: &mut Scene, tool: &str, args: Value) -> CallOutcome {
    call_tool(s.app.world_mut(), "dev", tool, args)
}

fn ok(s: &mut Scene, tool: &str, args: Value) -> String {
    let out = call(s, tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

fn fails(s: &mut Scene, tool: &str, args: Value) -> String {
    let out = call(s, tool, args.clone());
    assert!(!out.ok, "{tool} {args} should fail, answered: {}", out.result);
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

/// Where the camera is, where it looks, its field of view in degrees.
fn pose(s: &Scene) -> (Vec3, Vec3, f32) {
    let tf = s.app.world().get::<Transform>(s.camera).unwrap();
    let fov = match s.app.world().get::<Projection>(s.camera).unwrap() {
        Projection::Perspective(p) => p.fov.to_degrees(),
        other => panic!("{other:?}"),
    };
    (tf.translation, *tf.forward(), fov)
}

fn near(a: Vec3, b: Vec3) -> bool {
    (a - b).length() < 1e-3
}

fn ended(s: &Scene) -> Vec<(String, bool)> {
    s.app.world().resource::<Ended>().0.iter().map(|e| (e.id.clone(), e.skipped)).collect()
}

fn journal(s: &Scene, prefix: &str) -> Vec<String> {
    s.app.world().resource::<AiJournal>().after(0).0.map(|e| e.text.clone()).filter(|t| t.starts_with(prefix)).collect()
}

fn playing(s: &Scene) -> Option<String> {
    s.app.world().resource::<Shots>().playing().map(str::to_owned)
}

#[test]
fn a_shot_takes_the_camera_moves_between_its_keys_and_gives_it_back_to_the_orbit() {
    let mut s = shots();
    let (orbit_at, orbit_look, orbit_fov) = pose(&s);
    let player_fov = s.app.world().resource::<CameraSettings>().fov_deg;
    let (yaw, pitch) = (cam(&s).yaw, cam(&s).pitch);
    let boss = id(&s.app, s.near).to_string();
    let boss_at = s.app.world().get::<Transform>(s.near).unwrap().translation;

    assert_eq!(ok(&mut s, "play_shot", json!({"id": "intro", "role.boss": boss})), "shot intro playing: 2.0s, blend in 0.0s, out 1.0s");
    frames(&mut s, 10);
    let (at, look, fov) = pose(&s);
    assert!(near(at, Vec3::new(5.0, 5.0, 10.0)), "a second in: half way between the keys, {at}");
    assert!(near(look, (boss_at - at).normalize()), "looking at the boss: {look}");
    assert!((fov - (player_fov + 40.0) / 2.0).abs() < 0.01, "from the player's field of view half way to 40°: {fov}");
    let seen = ok(&mut s, "observe", json!({}));
    assert!(seen.contains(&format!("SHOT intro 1.0/2.0s skippable, roles boss={boss}")), "{seen}");

    // The orbit camera stands still meanwhile: a look turns it not at all.
    s.app.insert_resource(AccumulatedMouseMotion { delta: Vec2::new(200.0, 50.0) });
    s.app.update();
    s.app.insert_resource(AccumulatedMouseMotion::default());
    assert_eq!((cam(&s).yaw, cam(&s).pitch), (yaw, pitch));

    // At the last key the shot ends and blends back to the orbit camera over 1 s.
    frames(&mut s, 9);
    let last = Vec3::new(10.0, 5.0, 10.0);
    assert!(near(pose(&s).0, last), "{:?}", pose(&s));
    frames(&mut s, 5);
    let (at, _, fov) = pose(&s);
    assert!(near(at, last.lerp(orbit_at, Ease::InOut.apply(0.5))), "half way back: {at}");
    assert!((fov - (40.0 + orbit_fov) / 2.0).abs() < 0.01, "{fov}");
    let seen = ok(&mut s, "observe", json!({}));
    assert!(seen.contains("SHOT intro done, back to the orbit camera in 0.5s"), "{seen}");
    assert_eq!(playing(&s).as_deref(), Some("intro"), "the blend back is still the shot's");
    frames(&mut s, 6);
    assert_eq!(playing(&s), None);
    let (at, look, fov) = pose(&s);
    assert!(near(at, orbit_at) && near(look, orbit_look) && (fov - orbit_fov).abs() < 1e-3, "the orbit camera's own view again");
    assert_eq!(ended(&s), [("intro".to_owned(), false)]);
    assert_eq!(journal(&s, "shot intro"), [format!("shot intro starts: 2.0s, boss={boss}"), "shot intro ends (done)".to_owned()]);
    assert!(!ok(&mut s, "observe", json!({})).contains("SHOT"));
}

#[test]
fn a_skip_blends_back_from_where_the_shot_was_and_a_body_that_goes_holds_its_place() {
    let mut s = shots();
    let orbit_at = pose(&s).0;
    let boss = id(&s.app, s.near).to_string();
    let boss_at = s.app.world().get::<Transform>(s.near).unwrap().translation;
    ok(&mut s, "play_shot", json!({"id": "follow", "role.boss": boss}));
    frames(&mut s, 1);
    assert!(near(pose(&s).0, boss_at + Vec3::new(0.0, 2.0, 4.0)));

    // The boss walks: the camera goes with it.
    s.app.world_mut().get_mut::<Transform>(s.near).unwrap().translation.x += 3.0;
    frames(&mut s, 1);
    let walked = boss_at + Vec3::new(3.0, 2.0, 4.0);
    assert!(near(pose(&s).0, walked), "{:?}", pose(&s));

    // Lock-on picks nobody while a shot plays.
    tap_lock(&mut s);
    assert_eq!(target(&s), None);

    // The boss goes: the camera holds where it last saw it.
    s.app.world_mut().despawn(s.near);
    frames(&mut s, 3);
    assert!(near(pose(&s).0, walked), "{:?}", pose(&s));

    // The game's skip: back to the orbit camera from where the shot was, over the default 0.5 s.
    assert_eq!(ok(&mut s, "stop_shot", json!({})), "shot follow stopped: back to the orbit camera over 0.5s");
    frames(&mut s, 2);
    assert!(near(pose(&s).0, walked.lerp(orbit_at, Ease::InOut.apply(0.4))), "{:?}", pose(&s));
    assert!(ended(&s).is_empty(), "it ends when the blend back is done");
    frames(&mut s, 4);
    assert_eq!(ended(&s), [("follow".to_owned(), true)]);
    assert!(near(pose(&s).0, orbit_at));
    assert_eq!(journal(&s, "shot follow"), ["shot follow starts: 10.0s, boss=".to_owned() + &boss, "shot follow ends (skipped)".to_owned()]);
    assert_eq!(fails(&mut s, "stop_shot", json!({})), "no shot is playing");
}

#[test]
fn a_shot_is_refused_what_it_cannot_follow_and_one_that_cannot_be_skipped_is_cancelled() {
    let mut s = shots();
    let boss = id(&s.app, s.near).to_string();
    let hero = id(&s.app, s.player).to_string();
    let e = fails(&mut s, "play_shot", json!({"id": "outro"}));
    assert!(e.contains("no shot `outro`; shots: ") && e.contains("follow") && e.contains("intro"), "{e}");
    let e = fails(&mut s, "play_shot", json!({"id": "intro"}));
    assert!(e.ends_with("shot `intro` follows role `boss`: give a body for each of boss (`role.boss=eN`)"), "{e}");
    let e = fails(&mut s, "play_shot", json!({"id": "intro", "role.boss": boss, "role.hero": hero}));
    assert!(e.ends_with("shot `intro` has no role `hero`; its roles: boss"), "{e}");
    let e = fails(&mut s, "play_shot", json!({"id": "intro", "boss": boss}));
    assert!(e.ends_with("unknown argument `boss`: give `id` and a body per role, `role.NAME=eN`"), "{e}");
    let e = fails(&mut s, "play_shot", json!({"id": "intro", "role.boss": "boss"}));
    assert!(e.contains("role.boss: `\"boss\"` is not an entity id (`e5`)"), "{e}");
    assert_eq!(playing(&s), None);

    // A shot that cannot be skipped: the skip is refused, a cancel ends it at once (no blend out).
    let out = ok(&mut s, "play_shot", json!({"id": "locked"}));
    assert_eq!(out, "shot locked playing: 5.0s, blend in 0.0s, out 0.0s, not skippable");
    frames(&mut s, 1);
    assert!(near(pose(&s).0, Vec3::new(0.0, 9.0, 0.1)), "{:?}", pose(&s));
    let e = fails(&mut s, "stop_shot", json!({}));
    assert_eq!(e, "shot `locked` is not skippable (`skippable: false`); `cancel` ends it anyway");
    assert_eq!(ok(&mut s, "stop_shot", json!({"force": true})), "shot locked cancelled");
    assert_eq!((playing(&s), ended(&s)), (None, vec![("locked".to_owned(), true)]));

    // A second shot in place of the first: the first ends as skipped, the second blends in from it.
    ok(&mut s, "play_shot", json!({"id": "intro", "role.boss": boss}));
    frames(&mut s, 2);
    ok(&mut s, "play_shot", json!({"id": "follow", "role.boss": boss}));
    assert_eq!(playing(&s).as_deref(), Some("follow"));
    assert_eq!(ended(&s).last(), Some(&("intro".to_owned(), true)));
}

#[test]
fn a_shot_shorter_than_its_blend_in_goes_back_from_where_the_blend_got_to() {
    let mut s = shots();
    let orbit_at = pose(&s).0;
    let far = Vec3::new(0.0, 20.0, 30.0);
    ok(&mut s, "play_shot", json!({"id": "brief"}));
    // The camera never jumps: a little toward the shot on each frame, then back.
    let (mut at, mut closest) = (orbit_at, f32::MAX);
    for frame in 1..=25 {
        frames(&mut s, 1);
        let now = pose(&s).0;
        assert!(now.distance(at) < 3.0, "frame {frame}: a jump from {at} to {now}");
        closest = closest.min(now.distance(far));
        at = now;
    }
    // The blend in was a third of the way when the shot ended: back from there, the shot's own
    // place never reached.
    let way = orbit_at.distance(far);
    assert!(closest > way * 0.5, "{closest} of {way}");
    assert_eq!(ended(&s), [("brief".to_owned(), false)]);
    assert!(near(pose(&s).0, orbit_at));
}
