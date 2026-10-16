//! Achievements as content and who unlocks them: the content check, a single game's unlocks and
//! their store on Steam, an unlock Steam had from an earlier run, none during a replay, and co-op,
//! where the host decides and each machine unlocks for its own player.

use std::time::Duration;

use serde_json::json;
use shards::ai_core::journal::JournalKind;
use shards::ai_core::replay;
use shards::content::{check_dir, ContentKinds};
use shards::net::MemoryHub;
use shards::prelude::*;
use shards_steam::achievements::{CLIENT, STORE_AFTER};
use shards_steam::fake::FIRST_USER;
use shards_steam::{flush_achievements, unlock_achievement, Achievements, FakeSteam, UnlockAchievement};

use crate::*;

fn achievements(world: &World) -> &Achievements {
    world.resource::<Achievements>()
}

#[test]
fn the_content_check_names_what_is_wrong_with_an_achievement() {
    let (app, _) = single();
    let kinds = app.world().resource::<ContentKinds>().clone();
    let dir = std::env::temp_dir().join(format!("shards_steam_check_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let put = |file: &str, text: &str| {
        let path = dir.join(file);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    };
    put("content/languages.locale.ron", r#"{"en": (name: "English")}"#);
    put("locale/en/a.ftl", "ach-ok = Fine\nach-ok-desc = Fine indeed\n");
    put(
        "content/bad.achievement.ron",
        r#"{
            "ok": (steam: "ACH_OK", name: "@ach-ok", description: "@ach-ok-desc"),
            "twin": (steam: "ACH_OK", name: "@ach-ok", description: "@ach-ok-desc"),
            "nameless": (name: "@ach-ok", description: "@ach-ok-desc"),
            "spaced": (steam: "ACH ONE", name: "@ach-ok", description: "@ach-ok-desc"),
            "plain": (steam: "ACH_PLAIN", name: "Plain text", description: "@ach-ok-desc"),
            "missing": (steam: "ACH_MISSING", name: "@ach-okk", description: "@ach-ok-desc"),
        }"#,
    );
    let errors = check_dir(&dir, &kinds).expect_err("the file has mistakes");
    let _ = std::fs::remove_dir_all(&dir);
    let said: Vec<String> = errors.iter().map(|e| format!("{}: {}", e.id.clone().unwrap_or_default(), e.message)).collect();
    let has = |id: &str, part: &str| assert!(said.iter().any(|s| s.starts_with(&format!("{id}: ")) && s.contains(part)), "{id} `{part}` not in {said:#?}");
    has("twin", "steam: `ACH_OK` is achievement `ok`'s already");
    has("nameless", "steam: the achievement's API name on Steam is missing");
    has("spaced", "steam: `ACH ONE` is no Steam API name");
    has("plain", "name: the player reads it in their language: name a message, `@ach-plain`");
    has("missing", "name names message `ach-okk`, which en has not, nor its fallbacks (did you mean `@ach-ok`?)");
    assert!(!said.iter().any(|s| s.starts_with("ok: ")), "the first of two is fine: {said:#?}");
    assert_eq!(said.len(), 5, "{said:#?}");
}

#[test]
fn a_single_game_unlocks_once_and_steam_stores_it_a_moment_later() {
    let (mut app, steam) = single();
    let out = ok(&mut app, "achievement_unlock", json!({"id": "first_blood"}));
    assert_eq!(out, "unlocked first_blood (First blood) on Steam as ACH_FIRST_BLOOD");
    assert_eq!(steam.stats(FIRST_USER).set, ["ACH_FIRST_BLOOD"]);
    assert!(steam.stats(FIRST_USER).stored.is_empty(), "the store waits a moment");
    assert!(achievements(app.world()).store_pending());
    assert!(journal(app.world(), Some(JournalKind::Local)).contains(&"achievement unlocked: First blood (first_blood)".to_owned()));

    // A second unlock in the same moment makes no second store.
    assert_eq!(unlock_achievement(app.world_mut(), "warden_down").unwrap(), "unlocked warden_down (The warden falls) on Steam as ACH_WARDEN");
    assert_eq!(ok(&mut app, "achievement_unlock", json!({"id": "first_blood"})), "first_blood (First blood) was unlocked already");
    app.update();
    assert_eq!(steam.stats(FIRST_USER).stores, 0, "not before {STORE_AFTER:?}");
    std::thread::sleep(STORE_AFTER + Duration::from_millis(50));
    app.update();
    let stats = steam.stats(FIRST_USER);
    assert_eq!((stats.stored, stats.stores), (vec!["ACH_FIRST_BLOOD".to_owned(), "ACH_WARDEN".to_owned()], 1));
    let a = achievements(app.world());
    assert_eq!((a.unlocked.len(), a.stores, a.store_pending()), (2, 1, false));
    assert!(a.unlocked.iter().all(|u| u.on_steam.is_ok() && u.tick == 0));

    // A mistake names the ones there are.
    let wrong = err(&mut app, "achievement_unlock", json!({"id": "first_blod"}));
    assert_eq!(wrong, "no achievement `first_blod` (did you mean `first_blood`?); there are: first_blood, warden_down");
}

#[test]
fn steam_keeps_an_unlock_from_an_earlier_run_and_one_it_does_not_know_stays_in_this_run() {
    let steam = FakeSteam::new(MemoryHub::new(1));
    steam.define_achievements(&["ACH_FIRST_BLOOD"]);
    let mut first = game(config(|_| {}), steam.user(FIRST_USER, "player"));
    ok(&mut first, "achievement_unlock", json!({"id": "first_blood"}));
    // The exit stores what waits.
    flush_achievements(first.world_mut());
    assert_eq!(steam.stats(FIRST_USER).stores, 1);
    drop(first);

    let mut again = game(config(|_| {}), steam.user(FIRST_USER, "player"));
    assert_eq!(ok(&mut again, "achievement_unlock", json!({"id": "first_blood"})), "unlocked first_blood (First blood); Steam had it already");
    assert!(!achievements(again.world()).store_pending(), "nothing to store");
    let only_here = ok(&mut again, "achievement_unlock", json!({"id": "warden_down"}));
    assert!(only_here.starts_with("unlocked warden_down (The warden falls) in this run only: Steam has no achievement `ACH_WARDEN`"), "{only_here}");
    assert!(achievements(again.world()).unlocked[1].on_steam.is_err());
}

#[test]
fn nothing_is_unlocked_while_a_replay_plays() {
    let (mut recorded, _) = single();
    ok(&mut recorded, "step", json!({"ticks": 30}));
    let recording = replay::save(recorded.world());
    drop(recorded);

    let steam = FakeSteam::new(MemoryHub::new(1));
    let mut app = build(config(|_| {}), steam.user(FIRST_USER, "player"), save_folder());
    replay::play(app.world_mut(), recording).unwrap();
    shards::testing::wait_until_loaded(&mut app, shards::testing::LOAD_TIMEOUT).unwrap();
    let refused = err(&mut app, "achievement_unlock", json!({"id": "first_blood"}));
    assert_eq!(refused, "`first_blood` is not unlocked while a replay plays: its unlocks were the recording's");
    app.world_mut().trigger(UnlockAchievement::everyone("first_blood"));
    app.update();
    assert!(achievements(app.world()).unlocked.is_empty());
    assert!(steam.stats(FIRST_USER).set.is_empty());
    assert!(journal(app.world(), Some(JournalKind::Warn)).iter().all(|l| !l.starts_with("achievement:")), "a replay's unlocks are not mistakes");
}

#[test]
fn in_co_op_the_host_decides_and_each_machine_unlocks_for_its_own_player() {
    let steam = FakeSteam::new(MemoryHub::new(21));
    let host = build(config(lobby_host), steam.user(FIRST_USER, "host"), save_folder());
    let mut stand = stand_with_host(&steam, host);
    let lobby = session(stand.world(0)).lobby_id().unwrap();
    let app = build(config(|c| client(c, "c1", Some(lobby))), steam.user(SECOND_USER, "c1"), save_folder());
    let c1 = add_client(&mut stand, "c1", app);

    // Everyone: the host now, the client when the event arrives.
    let out = stand.call(0, "achievement_unlock", json!({"id": "first_blood"}));
    assert!(out.ok, "{}", out.result);
    assert_eq!(text(&out), "unlocked first_blood (First blood) on Steam as ACH_FIRST_BLOOD; sent to the session's 1 clients");
    // Player 1 only: not the host's.
    let out = stand.call(0, "achievement_unlock", json!({"id": "warden_down", "player": 1}));
    assert_eq!(text(&out), "warden_down (The warden falls) is player 1's: not this machine's; sent to the session's 1 clients");
    stand.run(20);
    assert_eq!(steam.stats(FIRST_USER).set, ["ACH_FIRST_BLOOD"]);
    assert_eq!(steam.stats(SECOND_USER).set, ["ACH_FIRST_BLOOD", "ACH_WARDEN"]);
    let unlocked: Vec<&str> = achievements(stand.world(c1)).unlocked.iter().map(|u| u.id.as_str()).collect();
    assert_eq!(unlocked, ["first_blood", "warden_down"]);

    // A client never unlocks by itself, by the tool or by the function.
    let out = stand.call(c1, "achievement_unlock", json!({"id": "first_blood"}));
    assert!(!out.ok);
    assert!(text(&out).contains(CLIENT), "{}", text(&out));
    assert_eq!(unlock_achievement(stand.world_mut(c1), "warden_down"), Err(CLIENT.to_owned()));
}
