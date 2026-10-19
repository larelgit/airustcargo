//! Sockets, props, mounts and clip families, as data and as pure decisions.

use super::*;
use shards_content::{ContentDb, ContentKinds};

const ACTIONS: &str = r#"{
    "light_1": (net_id: 1, phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)], anim: "swing_1"),
    "drink": (net_id: 2, phases: [("raise", 0.4), ("drink", 0.5), ("lower", 0.2)], tags: ["use"]),
    "roll": (net_id: 3, phases: [("roll", 0.4)]),
}"#;
const MOVESETS: &str = r#"{
    "hero": (inputs: {"light": "light_1", "dodge": "roll", "use_item": "drink"}),
    "greatsword": (inputs: {"light": "light_1"}),
}"#;
const PROPS: &str = r#"{
    "sword": (size: (0.05, 0.9, 0.02), grip: (0.0, 0.3, 0.0)),
    "great": (size: (0.1, 1.4, 0.04)),
    "flask": (size: (0.06, 0.1, 0.06)),
}"#;
const SET: &str = r#"{ "h": (
    source: "models/h.glb",
    clips: {"idle": (), "walk": (), "swing_1": (), "g_idle": (), "g_walk": (), "g_swing": (), "drink": (), "roll": ()},
    locomotion: {"idle": "idle", "walk": "walk"},
    layers: {"upper": (root: "spine")},
    actions: {"drink": (clip: "drink", layer: "upper")},
    by_moveset: {"greatsword": {"idle": "g_idle", "walk": "g_walk", "swing_1": "g_swing"}},
    sockets: {
        "hand_r": (bone: "hand.R"),
        "hip": (bone: "hips", offset: (0.25, 0.0, 0.0), turn: (0.0, 90.0, 0.0)),
        "back": (bone: "spine"),
    },
    carry: {
        "hero": [(prop: "sword", socket: "hand_r"), (prop: "flask", socket: "hip")],
        "greatsword": [(prop: "great", socket: "hand_r")],
        "*": [],
    },
    mounts: {
        "drink": (moves: [(prop: "sword", to: Some("hip"), span: Phase("drink"))], on_cut: {"sword": 0.05}),
        "@moveset": (
            moves: [(prop: "sword", to: Some("back"), span: Secs(0.1, 0.2)), (prop: "great", to: None, span: Secs(0.0, 0.1))],
            previous: Some(Secs(0.0, 0.12)),
            ended_by: ["blocked", "parried"],
        ),
    },
) }"#;

fn kinds() -> ContentKinds {
    let mut kinds = ContentKinds::default();
    kinds.register::<ActionDef>();
    kinds.register::<AnimSet>();
    kinds.register::<PropDef>();
    kinds.register::<MovesetDef>();
    kinds.register::<crate::status::StatusDef>();
    kinds
}

fn sources(props: &str, animsets: &str) -> [shards_content::db::Source; 4] {
    [
        shards_content::db::Source { path: "content/a.action.ron".into(), text: ACTIONS.into() },
        shards_content::db::Source { path: "content/m.moveset.ron".into(), text: MOVESETS.into() },
        shards_content::db::Source { path: "content/p.prop.ron".into(), text: props.into() },
        shards_content::db::Source { path: "content/h.animset.ron".into(), text: animsets.into() },
    ]
}

fn errors_of(props: &str, animsets: &str) -> Vec<String> {
    ContentDb::build(&kinds(), &sources(props, animsets)).errors().map(ToString::to_string).collect()
}

fn kit() -> AnimSet {
    let db = ContentDb::build(&kinds(), &sources(PROPS, SET));
    assert_eq!(db.errors().count(), 0, "{:?}", db.errors().map(ToString::to_string).collect::<Vec<_>>());
    db.get::<AnimSet>("h").unwrap().clone()
}

#[test]
fn the_kit_validates_and_round_trips() {
    let set = kit();
    let json = serde_json::to_value(&set).unwrap();
    let back: AnimSet = serde_json::from_value(json.clone()).unwrap();
    assert_eq!(back, set);
    assert_eq!(json["sockets"]["hip"]["offset"], serde_json::json!([0.25, 0.0, 0.0]));
    assert_eq!(json["sockets"]["hand_r"], serde_json::json!({"bone": "hand.R"}), "a zero offset and turn are left out");
    assert_eq!(json["carry"]["*"], serde_json::json!([]));
    assert!(json["mounts"]["drink"].get("previous").is_none() && json["mounts"]["drink"].get("ended_by").is_none());
    let props = ContentDb::build(&kinds(), &sources(PROPS, SET));
    let sword = props.get::<PropDef>("sword").unwrap();
    let json = serde_json::to_value(sword).unwrap();
    let mut keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["grip", "size"], "no turn, no scale, no look, no model");
}

#[test]
fn a_set_that_uses_none_of_it_keeps_its_json() {
    let text = r#"{ "h": (source: "models/h.glb", clips: {"idle": (), "drink": ()}, locomotion: {"idle": "idle"},
        layers: {"upper": (root: "spine")}, actions: {"drink": (clip: "drink", layer: "upper")}, tags: {"fire": "idle"}) }"#;
    let sets: BTreeMap<String, AnimSet> = ron::Options::default().with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME).from_str(text).unwrap();
    let json = serde_json::to_value(&sets["h"]).unwrap();
    let mut keys: Vec<&str> = json.as_object().unwrap().keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["action_blend", "actions", "blend", "clips", "layers", "locomotion", "source", "speeds", "tags"]);
    assert_eq!(json["actions"]["drink"], serde_json::json!({"clip": "drink", "layer": "upper"}));
    assert_eq!(json["tags"]["fire"], "idle", "a bare mapping is still a bare string");
}

#[test]
fn a_mappings_ends_equip_is_written_only_when_it_differs_from_its_default() {
    let parse = |text: &str| -> ActionClip {
        ron::Options::default()
            .with_default_extension(ron::extensions::Extensions::IMPLICIT_SOME)
            .from_str(text)
            .unwrap_or_else(|e| panic!("{text}: {e}"))
    };
    assert!(parse(r#""roll""#).ends_equip, "a whole-body mapping ends the equip");
    assert!(!parse(r#"(clip: "drink", layer: "upper")"#).ends_equip, "a layered one does not");
    let flipped = parse(r#"(clip: "roll", ends_equip: false)"#);
    assert!(!flipped.ends_equip && flipped.layer.is_none());
    assert_eq!(serde_json::to_value(&flipped).unwrap(), serde_json::json!({"clip": "roll", "ends_equip": false}));
    let layered = parse(r#"(ends_equip: true, layer: "upper", clip: "drink")"#);
    assert!(layered.ends_equip);
    assert_eq!(
        serde_json::to_value(&layered).unwrap(),
        serde_json::json!({"clip": "drink", "layer": "upper", "ends_equip": true})
    );
    assert_eq!(serde_json::to_value(parse(r#"(clip: "roll", ends_equip: true)"#)).unwrap(), serde_json::json!("roll"));
    assert_eq!(
        serde_json::to_value(parse(r#"(clip: "drink", layer: "upper", ends_equip: false)"#)).unwrap(),
        serde_json::json!({"clip": "drink", "layer": "upper"})
    );
    for clip in [flipped, layered, ActionClip::body("a"), ActionClip::on("a", "b")] {
        let back: ActionClip = serde_json::from_value(serde_json::to_value(&clip).unwrap()).unwrap();
        assert_eq!(back, clip);
    }
    let set = kit();
    assert!(!set.ends_equip("drink"), "the drink is layered");
    assert!(set.ends_equip("light_1") && set.ends_equip("roll"), "a whole-body action, mapped or not");
}

#[test]
fn a_misspelt_field_of_the_new_types_is_a_parse_error() {
    for (bad, field) in [
        (SET.replace(r#"(bone: "hand.R")"#, r#"(bone: "hand.R", ofset: (1.0, 0.0, 0.0))"#), "ofset"),
        (SET.replace(r#"ended_by: ["blocked", "parried"]"#, r#"ended_by: [], yield_after: 1.2"#), "yield_after"),
        (SET.replace(r#"(prop: "great", socket: "hand_r")"#, r#"(prop: "great", socket: "hand_r", slot: 2)"#), "slot"),
    ] {
        let errors = errors_of(PROPS, &bad);
        assert!(errors.len() == 1 && errors[0].contains(field), "{field}: {errors:#?}");
    }
    let errors = errors_of(r#"{ "x": (size: (1.0, 1.0, 1.0), colour: 3) }"#, SET);
    assert!(errors.iter().any(|e| e.contains("colour")), "{errors:#?}");
}

#[test]
fn by_moveset_sockets_and_carry_are_checked() {
    let bad = SET
        .replace(r#""greatsword": {"idle""#, r#""greatsord": {"idle""#)
        .replace(r#""walk": "g_walk""#, r#""walk": "g_walkk""#)
        .replace(r#""hip": (bone: "hips""#, r#""hip": (bone: """#)
        .replace(r#"(prop: "sword", socket: "hand_r"), (prop: "flask", socket: "hip")"#, r#"(prop: "gret", socket: "hand_l"), (prop: "sword", socket: "back"), (prop: "flask", socket: "hip")"#);
    assert_eq!(
        errors_of(PROPS, &bad),
        [
            "content/h.animset.ron: animset `h`: by_moveset -> unknown moveset `greatsord` (did you mean `greatsword`?)",
            "content/h.animset.ron: animset `h`: by_moveset.greatsord.walk -> unknown clip `g_walkk` (did you mean `g_walk`?); clips: drink, g_idle, g_swing, g_walk, idle, roll, swing_1, walk",
            "content/h.animset.ron: animset `h`: sockets.hip.bone is empty; name the bone the socket rides, e.g. `sockets: {\"hip\": (bone: \"hand.R\")}`",
            "content/h.animset.ron: animset `h`: carry.hero[0].prop -> unknown prop `gret` (did you mean `great`?)",
            "content/h.animset.ron: animset `h`: carry.hero[0].socket -> unknown socket `hand_l` (did you mean `hand_r`?); sockets: back, hand_r, hip",
        ]
    );
    // The same prop twice under one moveset, a moveset the content lacks, and no sockets at all.
    let twice = SET.replace(
        r#""*": [],"#,
        r#""*": [(prop: "sword", socket: "back"), (prop: "sword", socket: "hip")], "armed": [(prop: "sword", socket: "back")],"#,
    );
    assert_eq!(
        errors_of(PROPS, &twice),
        [
            "content/h.animset.ron: animset `h`: carry.*[1].prop `sword` is carried twice by `*`; a prop sits in one socket",
            "content/h.animset.ron: animset `h`: carry -> unknown moveset `armed`",
        ]
    );
    let bare = r#"{ "h": (source: "models/h.glb", clips: {"idle": (), "swing_1": ()}, carry: {"hero": [(prop: "sword", socket: "hand_r")]}) }"#;
    assert_eq!(
        errors_of(PROPS, bare),
        [
            "content/h.animset.ron: animset `h`: carry.hero[0].socket -> unknown socket `hand_r`; the set has no sockets: \
             name one in `sockets`, e.g. `sockets: {\"hand_r\": (bone: \"hand.R\")}`"
        ]
    );
}

#[test]
fn mounts_are_checked() {
    let bad = SET
        .replace(r#"to: Some("back")"#, r#"to: Some("bak")"#)
        .replace(r#"span: Phase("drink")"#, r#"span: Phases("raise", "sip")"#)
        .replace(r#"ended_by: ["blocked", "parried"]"#, r#"ended_by: ["blocked", "bonked"]"#)
        .replace(r#"on_cut: {"sword": 0.05}"#, r#"on_cut: {"sword": -1.0, "ghost": 0.1}, ended_by: ["blocked"]"#)
        .replace(r#"(prop: "great", to: None, span: Secs(0.0, 0.1))"#, r#"(prop: "great", to: None, span: Phase("x")), (prop: "flask", to: None, span: Secs(0.3, 0.1))"#);
    assert_eq!(
        errors_of(PROPS, &bad),
        [
            "content/h.animset.ron: animset `h`: mounts.@moveset.moves[0].to -> unknown socket `bak` (did you mean `back`?); sockets: back, hand_r, hip",
            "content/h.animset.ron: animset `h`: mounts.@moveset.moves[1].span: under `@moveset` a span is `Secs(from, to)`, counted from the weapon switch (it has no phases)",
            "content/h.animset.ron: animset `h`: mounts.@moveset.moves[2].span `Secs(0.3, 0.1)` must satisfy 0 <= from <= to",
            "content/h.animset.ron: animset `h`: mounts.@moveset.ended_by has an unknown event `bonked`; events: blocked, parried, damaged, reacted, guard_broken, staggered, evaded, perfect_evaded, pair_started, pair_ended, landed",
            "content/h.animset.ron: animset `h`: mounts.drink.moves[0].span names phase `sip`, which no action looked up by `drink` has",
            "content/h.animset.ron: animset `h`: mounts.drink.ended_by belongs under `@moveset`: only a weapon switch's equip ends by an event",
            "content/h.animset.ron: animset `h`: mounts.drink.on_cut.ghost names a prop that is never carried (no `carry` entry has it)",
            "content/h.animset.ron: animset `h`: mounts.drink.on_cut.sword must be >= 0 (is -1)",
        ]
    );
    let more = SET
        .replace(r#""drink": (moves"#, r#""@mode:water": (), "typo": (moves: [(prop: "sword", to: None, span: Secs(0.0, 1.0))]), "drink": (moves"#)
        .replace(r#"moves: [(prop: "sword", to: Some("back")"#, r#"on_cut: {"sword": 0.1}, moves: [(prop: "boat", to: None, span: Secs(0.0, 1.0)), (prop: "sword", to: Some("back")"#);
    assert_eq!(
        errors_of(PROPS, &more),
        [
            "content/h.animset.ron: animset `h`: mounts.@mode:water: the only rules key that starts with `@` is `@moveset` \
             (a motion mode's keys come with a later slice); name an action, an `anim` or a tag",
            "content/h.animset.ron: animset `h`: mounts.@moveset.moves[0].prop `boat` is never carried (no `carry` entry has it); the move does nothing",
            "content/h.animset.ron: animset `h`: mounts.@moveset.on_cut belongs under an action's key: it is what a cut run holds",
            "content/h.animset.ron: animset `h`: mounts.typo names no action, `anim` or tag; the rules are looked up by an action's id, its `anim` or one of its tags",
        ]
    );
    // An action is found by its tag, and a phase by any of the actions the key finds.
    let by_tag = SET.replace(r#""drink": (moves"#, r#""use": (moves"#);
    assert_eq!(errors_of(PROPS, &by_tag), Vec::<String>::new());
}

#[test]
fn props_are_checked() {
    let one = |def: &str| errors_of(&format!(r#"{{ "sword": ({def}), "great": (size: (1.0, 1.0, 1.0)), "flask": (size: (1.0, 1.0, 1.0)) }}"#), SET);
    assert_eq!(one(""), ["content/p.prop.ron: prop `sword`: a prop needs a `model` (a glTF) or a `size` (a box), e.g. `size: (0.05, 0.9, 0.02)`"]);
    assert_eq!(
        one(r#"model: "a.glb", size: (1.0, 1.0, 1.0)"#),
        ["content/p.prop.ron: prop `sword`: a prop has a `model` or a `size`, not both; leave one out"]
    );
    assert_eq!(one(r#"model: "a.png""#), ["content/p.prop.ron: prop `sword`: model `a.png` is not a glTF file (.gltf or .glb)"]);
    assert_eq!(one(r#"model: "a.glb""#), Vec::<String>::new(), "without an asset folder the file is not looked for");
    assert_eq!(
        one(r#"size: (1.0, 0.0, 1.0), scale: -1.0"#),
        [
            "content/p.prop.ron: prop `sword`: size must be positive on every axis (is [1, 0, 1])",
            "content/p.prop.ron: prop `sword`: scale must be > 0 (is -1)",
        ]
    );
}

#[test]
fn a_prop_sits_in_its_socket_with_its_grip_and_turn() {
    let set = kit();
    let db = ContentDb::build(&kinds(), &sources(PROPS, SET));
    let sword = db.get::<PropDef>("sword").unwrap();
    let in_hand = sword.in_socket(&set.sockets["hand_r"]);
    assert_eq!(in_hand.translation, Vec3::new(0.0, 0.3, 0.0), "the grip moves it from the socket");
    assert_eq!(in_hand.scale, Vec3::ONE);
    let at_hip = sword.in_socket(&set.sockets["hip"]);
    let want = Quat::from_rotation_y(90f32.to_radians()) * Vec3::new(0.0, 0.3, 0.0) + Vec3::new(0.25, 0.0, 0.0);
    assert!(at_hip.translation.distance(want) < 1e-5, "{:?} vs {want:?}", at_hip.translation);
    assert!(at_hip.rotation.angle_between(Quat::from_rotation_y(90f32.to_radians())) < 1e-3);
    let turned = PropDef { turn: Vec3::new(90.0, 0.0, 0.0), scale: 2.0, size: Some(Vec3::ONE), ..default() };
    let t = turned.in_socket(&set.sockets["back"]);
    assert!(t.rotation.angle_between(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)) < 1e-3 && t.scale == Vec3::splat(2.0));
    // The Euler order is Bevy's XYZ.
    let q = euler_degrees(Vec3::new(10.0, 20.0, 30.0));
    assert!(q.angle_between(Quat::from_euler(EulerRot::XYZ, 10f32.to_radians(), 20f32.to_radians(), 30f32.to_radians())) < 1e-3);
}

#[test]
fn a_moveset_family_renames_every_pick() {
    let set = kit();
    let moving = |v: f32| MotionState { velocity: Vec3::new(0.0, 0.0, -v), grounded: true, ..default() };
    assert_eq!(set.family(Some("greatsword"), "idle"), "g_idle");
    assert_eq!(set.family(Some("hero"), "idle"), "idle", "a moveset the table does not list shows the clips as named");
    assert_eq!(set.family(None, "idle"), "idle");
    let mut broken = set.clone();
    broken.by_moveset.get_mut("greatsword").unwrap().insert("drink".into(), "g_nothing".into());
    assert_eq!(broken.family(Some("greatsword"), "drink"), "drink", "a rename to a clip the set lacks is ignored");
    assert_eq!(set.family(Some("greatsword"), "roll"), "roll", "a clip the table does not name");
    // Locomotion, an action and its blend go through the table; without a moveset nothing changes.
    let idle = select_clip_in(None, &moving(0.0), BodyStatus::default(), &set, Some("greatsword"));
    assert_eq!(idle.clip, Some("g_idle"));
    assert_eq!(select_clip_in(None, &moving(2.0), BodyStatus::default(), &set, Some("greatsword")).clip, Some("g_walk"));
    let swing = select_clip_in(Some(("swing_1", 0.3, 0.6)), &moving(0.0), BodyStatus::default(), &set, Some("greatsword"));
    assert_eq!((swing.clip, swing.seek_to), (Some("g_swing"), Some(0.5)));
    assert_eq!(select_clip_for(Some(("swing_1", 0.3, 0.6)), &moving(0.0), BodyStatus::default(), &set).clip, Some("swing_1"));
    assert_eq!(select_clip_in(None, &moving(0.0), BodyStatus::default(), &set, Some("hero")), select_clip_for(None, &moving(0.0), BodyStatus::default(), &set));
    // A clip on a layer too.
    let mut layered = set.clone();
    layered.by_moveset.get_mut("greatsword").unwrap().insert("drink".into(), "g_swing".into());
    let drink = select_layer_in(Some(("drink", 0.2, 1.0)), BodyStatus::default(), &layered, Some("greatsword")).unwrap();
    assert_eq!((drink.layer, drink.choice.clip), ("upper", Some("g_swing")));
    assert_eq!(select_layer(Some(("drink", 0.2, 1.0)), BodyStatus::default(), &layered).unwrap().choice.clip, Some("drink"));
}

#[test]
fn rules_are_found_by_id_then_anim_then_tag() {
    let set = kit();
    assert!(set.mount_rules("drink", None, &[]).is_some_and(|r| r.on_cut.contains_key("sword")));
    let mut more = set.clone();
    more.mounts.insert("swing_1".into(), MountRules { ended_by: vec!["x".into()], ..default() });
    more.mounts.insert("use".into(), MountRules { ended_by: vec!["y".into()], ..default() });
    let tags = ["fire".to_owned(), "use".to_owned()];
    assert_eq!(more.mount_rules("light_1", Some("swing_1"), &[]).unwrap().ended_by, ["x"], "by `anim`");
    assert_eq!(more.mount_rules("heavy", None, &tags).unwrap().ended_by, ["y"], "by a tag the set does not map to a clip");
    assert!(more.mount_rules("heavy", Some("nothing"), &["fire".to_owned()]).is_none());
    assert!(more.mount_rules("drink", Some("swing_1"), &tags).unwrap().on_cut.contains_key("sword"), "the id first");
    assert_eq!(set.carried(Some("greatsword")).len(), 1);
    assert_eq!(set.carried(Some("hero")).len(), 2);
    assert!(set.carried(Some("stranger")).is_empty(), "the `*` entry, empty here");
    assert!(set.carried(None).is_empty());
}

fn carried(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|(p, s)| ((*p).to_owned(), (*s).to_owned())).collect()
}

fn places(set: &AnimSet, view: &MountView) -> Vec<(String, Place)> {
    let _ = set;
    mount_places(view)
}

fn at(socket: &str) -> Place {
    Place::Socket(socket.to_owned())
}

#[test]
fn a_carried_prop_sits_in_its_carry_socket_until_a_rule_moves_it() {
    let set = kit();
    let db = ContentDb::build(&kinds(), &sources(PROPS, SET));
    let drink = db.get::<ActionDef>("drink").unwrap();
    let own = carried(&[("sword", "hand_r"), ("flask", "hip")]);
    let view = |t: f32| MountView {
        carried: &own,
        previous: &[],
        run: Some((set.mounts.get("drink").unwrap(), drink, t)),
        equip: None,
        cut: &[],
    };
    // The drink phase (0.4 - 0.9 s) moves the sword to the hip; before and after it is in the hand.
    let rest = MountView { run: None, ..view(0.0) };
    assert_eq!(places(&set, &rest), [("flask".into(), at("hip")), ("sword".into(), at("hand_r"))]);
    for (t, want) in [(0.0, "hand_r"), (0.39, "hand_r"), (0.4, "hip"), (0.89, "hip"), (0.9, "hand_r"), (1.1, "hand_r")] {
        let sword = places(&set, &view(t)).into_iter().find(|(p, _)| p == "sword").unwrap().1;
        assert_eq!(sword, at(want), "sword at {t} s");
    }
    // A move with no `to` hides the prop; the last open move wins.
    let mut rules = MountRules::default();
    rules.moves.push(MountMove { prop: "sword".into(), to: None, span: Span::Secs(0.0, 1.0) });
    rules.moves.push(MountMove { prop: "sword".into(), to: Some("back".into()), span: Span::Secs(0.5, 0.7) });
    let run = |t| MountView { run: Some((&rules, drink, t)), ..rest };
    assert_eq!(places(&set, &run(0.2))[1], ("sword".into(), Place::Hidden));
    assert_eq!(places(&set, &run(0.6))[1], ("sword".into(), at("back")));
    assert_eq!(places(&set, &run(0.8))[1], ("sword".into(), Place::Hidden));
}

#[test]
fn the_cut_hold_beats_the_running_action_and_the_action_beats_the_equip() {
    let set = kit();
    let db = ContentDb::build(&kinds(), &sources(PROPS, SET));
    let roll = db.get::<ActionDef>("roll").unwrap();
    let own = carried(&[("sword", "hand_r")]);
    let equip = set.equip_rules().unwrap();
    let mut rules = MountRules::default();
    rules.moves.push(MountMove { prop: "sword".into(), to: Some("hip".into()), span: Span::Secs(0.0, 1.0) });
    let held = [("sword".to_owned(), at("back"))];
    let view = MountView {
        carried: &own,
        previous: &[],
        run: Some((&rules, roll, 0.01)),
        equip: Some((equip, 0.15)),
        cut: &held,
    };
    assert_eq!(mount_places(&view), [("sword".into(), at("back"))], "the cut hold first");
    assert_eq!(mount_places(&MountView { cut: &[], ..view }), [("sword".into(), at("hip"))], "then the running action");
    assert_eq!(mount_places(&MountView { run: None, cut: &[], ..view }), [("sword".into(), at("back"))], "then the equip (0.1 - 0.2 s)");
    assert_eq!(mount_places(&MountView { run: None, cut: &[], equip: Some((equip, 0.25)), ..view }), [("sword".into(), at("hand_r"))]);
    assert_eq!(mount_places(&MountView { run: None, cut: &[], equip: None, ..view }), [("sword".into(), at("hand_r"))]);
}

#[test]
fn the_previous_props_show_inside_the_previous_span_and_the_new_ones_after() {
    let set = kit();
    let equip = set.equip_rules().unwrap();
    let new = carried(&[("great", "hand_r")]);
    let old = carried(&[("sword", "hand_r"), ("flask", "hip")]);
    let view = |since: f32| MountView { carried: &new, previous: &old, run: None, equip: Some((equip, since)), cut: &[] };
    // 0 - 0.12 s: the old kit shows (the sword is on the back from 0.1 s); the new weapon is hidden.
    assert_eq!(
        mount_places(&view(0.05)),
        [("flask".into(), at("hip")), ("great".into(), Place::Hidden), ("sword".into(), at("hand_r"))]
    );
    assert_eq!(
        mount_places(&view(0.11)),
        [("flask".into(), at("hip")), ("great".into(), Place::Hidden), ("sword".into(), at("back"))]
    );
    // After the previous span: the new kit; the old props are hidden.
    assert_eq!(
        mount_places(&view(0.13)),
        [("flask".into(), Place::Hidden), ("great".into(), at("hand_r")), ("sword".into(), Place::Hidden)]
    );
    // The `great` move hides it for 0 - 0.1 s even when shown: the equip's moves apply to the shown props.
    let same = MountView { previous: &[], ..view(0.05) };
    assert_eq!(mount_places(&same), [("great".into(), Place::Hidden)]);
    assert_eq!(mount_places(&MountView { previous: &[], ..view(0.5) }), [("great".into(), at("hand_r"))]);
}

#[test]
fn the_equip_is_open_until_its_last_span_ends() {
    let set = kit();
    let equip = set.equip_rules().unwrap();
    assert!(equip_open(equip, 0.0) && equip_open(equip, 0.19));
    assert!(!equip_open(equip, 0.2) && !equip_open(equip, 5.0));
    assert!(!equip_open(&MountRules::default(), 0.0), "no rules: nothing to show");
}

#[test]
fn a_cut_holds_the_props_that_sat_away_from_home() {
    let set = kit();
    let rules = set.mounts.get("drink").unwrap();
    let own = carried(&[("sword", "hand_r"), ("flask", "hip")]);
    let mut shown: BTreeMap<String, Place> = BTreeMap::new();
    shown.insert("sword".into(), at("hip"));
    shown.insert("flask".into(), at("hip"));
    assert_eq!(cut_holds(rules, &own, &shown), [("sword".to_owned(), at("hip"), 0.05)]);
    shown.insert("sword".into(), at("hand_r"));
    assert!(cut_holds(rules, &own, &shown).is_empty(), "it is home: nothing to hold");
    shown.insert("sword".into(), Place::Hidden);
    assert_eq!(cut_holds(rules, &own, &shown), [("sword".to_owned(), Place::Hidden, 0.05)], "hidden is away too");
    assert!(cut_holds(rules, &carried(&[("flask", "hip")]), &shown).is_empty(), "a prop that is not carried");
}
