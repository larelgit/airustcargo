//! The tools, the status and the developer menu: what an agent reads of the effects headless,
//! where nothing is drawn and nothing plays.

use serde_json::json;
use shards::fx::{FxSettings, Particles};

use crate::*;

#[test]
fn effects_lists_every_effect_and_the_table_and_headless_nothing_plays() {
    let assets = Folder::new("list");
    let mut app = game(&assets);
    let said = ok(&mut app, "effects", json!({}));
    assert_eq!(
        said,
        "3 effects · nothing is drawn headless: effects play in a window or with --offscreen · telegraph decals on\n\
         \x20 dust   6 cube + 20/s for 0.5s #ffffff → transparent · 0.6-0.8 s · gravity -2 · pool 24\n\
         \x20 glow   4 quad #7dff9e → transparent · 0.5 s · pool 8\n\
         \x20 spark  12 quad glow #fff2b3 → #ff6a1a00 · 0.2-0.4 s · aim hit · pool 32\n\
         events: damaged → spark · died → dust · healed → glow\n\
         cues: swoosh → glow"
    );
    assert_eq!(
        status(&app),
        json!({"effects": 3, "live": 0, "decals": 0, "telegraph_decals": true, "marks": 0, "debris": {"resting": 0, "flying": 0},
               "rays": {"last": 0, "max": 0, "total": 0, "budget": 2048}})
    );
    let modules = &shards::ai_core::status::status(app.world())["engine"]["modules"];
    assert!(modules.as_array().unwrap().iter().any(|m| m["name"] == "fx"), "{modules}");

    // Headless an effect is known but plays nowhere; an event plays nothing.
    let hero = fighter(app.world_mut(), "hero", "heroes", Vec3::ZERO, Vec3::NEG_Z, 100.0);
    let brute = fighter(app.world_mut(), "brute", "monsters", Vec3::new(0.0, 0.0, -1.5), Vec3::Z, 20.0);
    let e = id(&app, hero);
    assert_eq!(
        refused(&mut app, "effect_play", json!({"effect": "spark", "at": e})),
        "nothing is drawn headless: effects play in a window or with --offscreen (effect `spark` is known)"
    );
    press(&mut app, hero, "light");
    step(&mut app, 30);
    assert!(app.world().get::<Health>(brute).is_none_or(|h| h.current <= 0.0), "the swing hit and killed the brute");
    let particles = app.world().resource::<Particles>();
    assert_eq!((particles.played, particles.live()), (0, 0), "nothing played headless");
    assert!(!ok(&mut app, "journal", json!({})).contains("fx:"), "and nothing to warn about");

    let none = Folder::empty("none");
    let mut bare = game(&none);
    assert_eq!(
        ok(&mut bare, "effects", json!({})),
        "0 effects · nothing is drawn headless: effects play in a window or with --offscreen · telegraph decals on\n\
         no effect defined: add one in content/**/*.effect.ron\n\
         no table of effects (`*.fx.ron`): engine events play none"
    );
}

#[test]
fn effect_play_says_how_to_call_it() {
    let assets = Folder::new("play");
    let mut app = game(&assets);
    assert_eq!(
        refused(&mut app, "effect_play", json!({"effect": "sprak", "point": [0, 0, 0]})),
        "no effect `sprak` (did you mean `spark`?): content `*.effect.ron` defines effects; 3 known"
    );
    assert_eq!(
        refused(&mut app, "effect_play", json!({"effect": "spark"})),
        "give `at` (an entity: `at=e6`) or `point` (`point=[4, 0, -2]`)"
    );
    assert_eq!(
        refused(&mut app, "effect_play", json!({"effect": "spark", "at": "e1", "point": [0, 0, 0]})),
        "give `at` (an entity) or `point` (x, y, z), not both"
    );
    assert!(refused(&mut app, "effect_play", json!({"effect": "spark", "at": "e99"})).contains("e99"));
    assert_eq!(
        refused(&mut app, "effect_play", json!({"effect": "spark", "point": [0, 0, 0], "strength": -0.5})),
        "strength: a number, 0 or more (1 plays the effect as it is), not -0.5"
    );
}

#[test]
fn telegraph_decals_switches_the_decals_and_the_status_says_so() {
    let assets = Folder::new("switch");
    let mut app = game(&assets);
    assert_eq!(ok(&mut app, "telegraph_decals", json!({"on": false})), "telegraph decals off (nothing is drawn headless)");
    assert!(!app.world().resource::<FxSettings>().decals);
    assert_eq!(status(&app)["telegraph_decals"], false);
    assert!(ok(&mut app, "effects", json!({})).lines().next().unwrap().ends_with("· telegraph decals off"));
    assert_eq!(ok(&mut app, "telegraph_decals", json!({})), "telegraph decals on (nothing is drawn headless)", "on unless it says");
    assert_eq!(status(&app)["telegraph_decals"], true);
}

#[test]
fn the_developer_menu_lists_the_effects_and_switches_the_decals() {
    let assets = Folder::new("menu");
    let mut app = game(&assets);
    let list = ok(&mut app, "dev_menu", json!({}));
    let entry = |id: &str| {
        list.lines()
            .map(str::trim_start)
            .find(|l| l.split_whitespace().next() == Some(id))
            .unwrap_or_else(|| panic!("no entry `{id}`:\n{list}"))
            .to_owned()
    };
    let effects = entry("effects");
    assert!(effects.contains("Effects, their pools and decals") && effects.ends_with("effects"), "{effects}");
    let decals = entry("telegraph_decals");
    assert!(decals.contains("Telegraphs as decals on the ground") && decals.ends_with("telegraph_decals on=false"), "{decals}");
    assert_eq!(ok(&mut app, "dev_menu", json!({"run": "telegraph_decals"})), "telegraph decals off (nothing is drawn headless)");
    let list = ok(&mut app, "dev_menu", json!({}));
    assert!(list.lines().any(|l| l.trim_start().starts_with("telegraph_decals") && l.ends_with("telegraph_decals on=true")), "{list}");
    assert!(ok(&mut app, "dev_menu", json!({"run": "effects"})).starts_with("3 effects · "));
}
