//! Content: every row of the validation table gives its error, with the file, the encounter and
//! the field in the text; a valid definition loads.

use shards::content::ContentDb;
use shards::encounter::{EncounterDef, EncounterRulesDef, Face, HomeAt, OnReset, SpawnAt, StartDef};

use crate::common::*;

/// The errors about encounter `id`.
fn errors(db: &ContentDb, id: &str) -> Vec<String> {
    let needle = format!("encounter `{id}`");
    errors_of(db).into_iter().filter(|e| e.contains(&needle)).collect()
}

#[test]
fn the_test_content_loads_and_the_defaults_are_neutral() {
    let app = app(1);
    let db = app.world().resource::<ContentDb>();
    let ring = db.get::<EncounterDef>("ring_waves").expect("ring_waves");
    assert_eq!(ring.waves.len(), 3);
    assert!(matches!(ring.start, StartDef::Trigger { all_players_in: true, .. }));
    assert_eq!(ring.waves[1].members[0].count, 2);
    assert!(ring.once && ring.net_always && ring.blocks_join);
    assert_eq!(ring.lock.as_deref(), Some("barrier"));
    // The least a definition says: everything else is 0, none or off (no number of any particular game is a default).
    let single = db.get::<EncounterDef>("single").expect("single");
    assert_eq!((single.resume_after, single.reset_after, single.budget, single.rules.clone()), (0.0, None, None, None));
    assert_eq!((single.once, single.net_always, single.blocks_join, single.lock.clone()), (false, false, false, None));
    assert_eq!(single.on_reset, OnReset::Restart);
    let group = &single.waves[0].members[0];
    assert_eq!((group.count, group.up_to, group.face, group.home, group.leash), (1, None, Face::Center, HomeAt::Spawn, None));
    assert!(matches!(group.at, SpawnAt::Offsets(_)));
    // The pull-back heals unless told not to.
    assert!(single.pull_back.heal && single.pull_back.state.is_none() && single.pull_back.deadline == 0.0);
    assert!(db.get::<EncounterRulesDef>("default").is_some());
}

#[test]
fn every_row_of_the_validation_table_gives_its_error() {
    let app = app(1);
    let db = db_with_dir(&app, BAD);
    let want: &[(&str, &str)] = &[
        ("negative_reset", "reset_after: -1 is not a number of seconds >= 0"),
        ("zero_radius", "radius: 0 is not a distance > 0 meters"),
        ("no_waves", "waves: an encounter needs at least one wave"),
        ("empty_wave", "waves[0].members: a wave needs at least one group"),
        ("late_placed", "waves[1].placed: only the first wave can be placed (it stands before the start)"),
        ("typo_prefab", "waves[0].members[0].prefab -> unknown prefab `gruntt` (did you mean `grunt`?)"),
        ("up_to_low", "waves[0].members[0].up_to: 1 is below count 2"),
        ("few_offsets", "waves[0].members[0].at: 1 offset for up to 2 members: add offsets or lower the count"),
        (
            "behind_offsets",
            "waves[0].members[0].avoid_behind: offsets are placed by hand and never filtered: remove it, or spawn at markers",
        ),
        ("home_override", "waves[0].members[0].overrides.Home: set the home with `home` and `leash`"),
        ("typo_rules", "rules -> unknown encounter_rules `defualt` (did you mean `default`?)"),
        (
            "no_role",
            "waves[0].members[0]: prefab `plain` has no EncounterRole, which budget, allowed, forbidden and rules read: add \"EncounterRole\": (cost: 1, tags: [\"weak\"]) to it",
        ),
        (
            "over_budget",
            "waves[0]: costs up to 5 (grunt x2 at 1, brute x1 at 3), over the budget of 3: lower a count, or raise budget",
        ),
        (
            "not_allowed",
            "waves[0].members[0]: prefab `archer` has tags [ranged], none of them in allowed [weak, melee]",
        ),
        ("forbidden", "waves[0].members[0]: prefab `brute` has the forbidden tag `heavy`"),
        (
            "too_many_ranged",
            "waves[0]: up to 3 members tagged `ranged`, rule `default` allows 2: lower a count",
        ),
        (
            "two_controllers",
            "waves[0]: up to 2 members tagged `controller`, rule `default` allows 1 without the encounter tag `special_arena`: lower a count, or tag the encounter `special_arena` if it is one",
        ),
        (
            "lonely_support",
            "waves[0]: a member tagged `support` needs at least 2 others, the wave can have 1: raise a count",
        ),
        (
            "state_no_time",
            "pull_back.deadline: 0 leaves no time to walk to the state `return_home`: give the deadline in seconds, or drop the state to place survivors home at once",
        ),
        (
            "pull_to_fight",
            "pull_back.state: state `fight` of behaviour `walker` (prefab `walker`) is kind `attack`, not `return_home`: name a state with `kind: \"return_home\"`, or give that state the kind",
        ),
        ("huge_count", "waves[0].members[0].count: a group spawns at most 64 members"),
        ("big_up_to", "waves[0].members[0].count: a group spawns at most 64 members"),
        ("zero_count", "waves[0].members[0].count: a group needs at least 1 member"),
        ("bad_ring", "waves[0].members[0].at: a ring from 5 to 2 m is not 0 <= min <= max"),
        ("bad_avoid", "waves[0].members[0].avoid_behind: within must be > 0 m and angle 0..=360 degrees"),
        ("bad_leash", "waves[0].members[0].leash: 0 is not a distance > 0 meters"),
        ("bad_within", "start.within: 0 is not a distance > 0 meters"),
        ("bad_cue_sound", "waves[0].warn.sound -> unknown sound `whosh`"),
        ("bad_cue_message", "waves[0].warn.message names message `no-such-message`"),
    ];
    for (id, text) in want {
        let found = errors(&db, id);
        assert!(
            found.iter().any(|e| e.contains(text)),
            "encounter `{id}` should say `{text}`; its errors: {found:#?}"
        );
        // Every error starts with the file, the kind and the id.
        assert!(found.iter().all(|e| e.starts_with("content/bad.encounter.ron")), "{found:#?}");
    }
    // Valid ones stay valid: the special arena allows two controllers, an override of a field
    // the prefab has is fine.
    for id in ["special_controllers", "good_override", "once_fight", "pull_to_home"] {
        let own: Vec<String> = errors(&db, id).into_iter().filter(|e| e.starts_with("content/bad.encounter.ron")).collect();
        assert_eq!(own, Vec::<String>::new(), "{id}");
    }
}

#[test]
fn an_override_that_names_a_missing_component_or_field_is_checked_like_a_prefab_spawn() {
    let app = app(1);
    let db = db_with_dir(&app, BAD);
    let unknown = errors(&db, "unknown_override").join("\n");
    assert!(unknown.contains("waves[0].members[0].overrides") && unknown.contains("Helth"), "{unknown}");
    let field = errors(&db, "bad_override_field").join("\n");
    assert!(field.contains("waves[0].members[0].overrides") && field.contains("curent"), "{field}");
}

#[test]
fn a_prefab_check_reads_the_anchor_it_carries() {
    let app = app(1);
    let db = db_with_dir(&app, BAD);
    let all = errors_of(&db);
    let once = all.iter().find(|e| e.contains("prefab `once_anchor`")).expect("once_anchor is refused");
    assert!(
        once.contains("EncounterAnchor: the encounter `once_fight` is `once`, so its anchor needs a key (a won encounter is remembered by it)"),
        "{once}"
    );
    let lost = all.iter().find(|e| e.contains("prefab `lost_anchor`")).expect("lost_anchor is refused");
    assert!(lost.contains("unknown encounter `once_figth` (did you mean `once_fight`?)"), "{lost}");
    assert!(!all.iter().any(|e| e.contains("prefab `good_anchor`")), "{all:#?}");
}

#[test]
fn a_misspelt_field_of_a_definition_is_an_error() {
    let app = app(1);
    let db = db_with_dir(&app, BAD);
    let text = errors(&db, "typo_field").join("\n");
    // deny_unknown_fields: the definition does not load at all, and the file says why.
    assert!(db.get::<EncounterDef>("typo_field").is_none() || !text.is_empty());
    let all = errors_of(&db).join("\n");
    assert!(all.contains("wavez"), "{all}");
}

#[test]
fn overrides_read_as_a_merge_patch_in_ron_struct_or_map_form() {
    // `(current: 10.0)` and `{"current": 10.0}` both land in a JSON object.
    let a: serde_json::Value = ron::from_str("(current: 10.0)").expect("struct form");
    let b: serde_json::Value = ron::from_str("{\"current\": 10.0}").expect("map form");
    assert_eq!(a, serde_json::json!({"current": 10.0}));
    assert_eq!(b, serde_json::json!({"current": 10.0}));
}

#[test]
fn defaults_of_the_group_are_count_one_center_and_spawn() {
    let def: EncounterDef = ron::from_str("(radius: 5.0, start: Manual, waves: [(members: [(prefab: \"x\", at: Marker(\"m\"))])])").expect("defaults");
    let group = &def.waves[0].members[0];
    assert_eq!((group.count, group.face, group.home), (1, Face::Center, HomeAt::Spawn));
    assert!(def.pull_back.heal);
}
