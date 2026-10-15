//! Languages end to end on the whole engine: a menu whose text is messages, switched to another
//! language while it is open, the tools an agent reads text with, an edited file applied while
//! the game runs, and the content check that names mistakes.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};
use shards::ai_core::{call_tool, AiJournal, CallOutcome};
use shards::content::{check_dir, ContentKinds};
use shards::prelude::*;
use shards::testing;

/// The languages and the menu of the tests.
const ASSETS: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/locale/assets");

/// The engine on the assets in `dir`, headless and paused at tick 0.
fn game_on(dir: &str) -> App {
    let dir = dir.to_owned();
    testing::test_app(move |config| config.asset_dir = dir, |_| {})
}

fn game() -> App {
    game_on(ASSETS)
}

fn call(app: &mut App, tool: &str, args: Value) -> CallOutcome {
    call_tool(app.world_mut(), "dev", tool, args)
}

fn text(out: &CallOutcome) -> String {
    out.result.as_str().map_or_else(|| out.result.to_string(), str::to_owned)
}

/// The text of a call that must succeed.
fn ok(app: &mut App, tool: &str, args: Value) -> String {
    let out = call(app, tool, args.clone());
    assert!(out.ok, "{tool} {args} failed: {}", out.result);
    text(&out)
}

/// The error of a call that must fail.
fn refused(app: &mut App, tool: &str, args: Value) -> String {
    let out = call(app, tool, args.clone());
    assert!(!out.ok, "{tool} {args} should have been refused: {}", out.result);
    text(&out)
}

/// Journal lines that contain `needle`.
fn journal(app: &App, needle: &str) -> Vec<String> {
    app.world().resource::<AiJournal>().after(0).0.map(|e| e.text.clone()).filter(|t| t.contains(needle)).collect()
}

/// The line of `ui_read` for widget `path`, indentation trimmed.
fn line(tree: &str, path: &str) -> String {
    tree.lines().map(str::trim_start).find(|l| l.starts_with(&format!("{path}  "))).unwrap_or_else(|| panic!("no {path} in:\n{tree}")).to_owned()
}

fn speak(app: &mut App, language: &str) -> String {
    ok(app, "settings_set", json!({"group": "locale", "key": "language", "value": language}))
}

/// A copy of the test assets, to edit without touching the repository.
fn assets_copy(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("shards_locale_{name}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(Path::new(ASSETS), &dir);
    dir
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

#[test]
fn an_open_menu_says_everything_again_in_the_language_switched_to() {
    let mut app = game();
    ok(&mut app, "ui_open", json!({"screen": "pause"}));
    let tree = ok(&mut app, "ui_read", json!({}));
    assert!(tree.contains("pause \"Paused\" (menu, pauses the game)"), "{tree}");
    assert_eq!(line(&tree, "pause/resume"), "pause/resume  button \"Resume\" → close ◀ focus");
    assert_eq!(line(&tree, "pause/language"), "pause/language  choice \"Language\" en \"English\" (en|ru) [locale.language]");
    assert_eq!(line(&tree, "pause/speed"), "pause/speed  choice \"Speed\" slow \"Slow\" (slow|fast)");
    assert_eq!(line(&tree, "pause/sword"), "pause/sword  compare \"Sword\" damage 12 → 15 (+3)");
    assert_eq!(line(&tree, "pause/mail"), "pause/mail  text \"@ada\"", "`@@` is a text that starts with `@`");
    assert_eq!(shards::ai_core::status::status(app.world())["locale"], json!({"language": "en", "chain": ["en"], "languages": ["en", "ru"]}));

    // The menu's own choice switches it, before the call returns; values and focus stay.
    ok(&mut app, "ui_set", json!({"id": "pause/speed", "value": "fast"}));
    let switched = ok(&mut app, "ui_set", json!({"id": "pause/language", "value": "Русский"}));
    assert_eq!(switched, "pause/language en → ru");
    assert_eq!(journal(&app, "locale: language"), ["locale: language en → ru"]);
    let tree = ok(&mut app, "ui_read", json!({}));
    assert!(tree.contains("pause \"Пауза\" (menu, pauses the game)"), "{tree}");
    assert_eq!(line(&tree, "pause/resume"), "pause/resume  button \"Продолжить\" → close");
    assert_eq!(line(&tree, "pause/language"), "pause/language  choice \"Язык\" ru \"Русский\" (en|ru) [locale.language] ◀ focus");
    assert_eq!(line(&tree, "pause/speed"), "pause/speed  choice \"Скорость\" fast \"Быстро\" (slow|fast)", "the value set stays");
    assert_eq!(line(&tree, "pause/sword"), "pause/sword  compare \"Меч\" урон 12 → 15 (+3)");
    assert_eq!(shards::ai_core::status::status(app.world())["locale"]["chain"], json!(["ru", "en"]));

    // Russian lacks the hint: English shows, and the journal says so once.
    assert_eq!(line(&tree, "pause/hint"), "pause/hint  hint \"[Esc] Back\"");
    ok(&mut app, "ui_read", json!({}));
    assert_eq!(journal(&app, "has no message"), ["locale: ru has no message `pause-hint`; it is shown in en"]);

    // A choice takes what an option says, too; reset is the game's own language.
    ok(&mut app, "ui_set", json!({"id": "pause/speed", "value": "медленно"}));
    ok(&mut app, "settings_set", json!({"group": "locale", "reset": true}));
    let tree = ok(&mut app, "ui_read", json!({}));
    assert_eq!(line(&tree, "pause/speed"), "pause/speed  choice \"Speed\" slow \"Slow\" (slow|fast) ◀ focus");
    assert_eq!(journal(&app, "locale: language"), ["locale: language en → ru", "locale: language ru → en"]);
}

#[test]
fn a_language_the_game_has_not_is_refused_with_the_ones_it_has() {
    let mut app = game();
    let err = refused(&mut app, "settings_set", json!({"group": "locale", "key": "language", "value": "rus"}));
    assert!(err.contains("`rus` is not a language of this game (did you mean `ru`?): en (English), ru (Русский)"), "{err}");
    ok(&mut app, "ui_open", json!({"screen": "pause"}));
    let err = refused(&mut app, "ui_set", json!({"id": "pause/speed", "value": "warp"}));
    assert_eq!(err, "pause/speed is one of slow (Slow), fast (Fast), not `warp`");
}

#[test]
fn phrases_take_numbers_and_other_phrases_in_each_language() {
    let mut app = game();
    let say = |app: &mut App, args: Value| ok(app, "text", args).trim_end().to_owned();
    assert_eq!(say(&mut app, json!({"id": "trophies", "args": {"count": 1}})), "1 trophy");
    assert_eq!(say(&mut app, json!({"id": "trophies", "args": {"count": 5}})), "5 trophies");
    let ru = |n: u32| json!({"id": "trophies", "args": {"count": n}, "language": "ru"});
    assert_eq!([1, 3, 5, 21].map(|n| say(&mut app, ru(n))), ["1 трофей", "3 трофея", "5 трофеев", "21 трофей"]);
    assert_eq!(say(&mut app, json!({"id": "leave", "args": {"place": "@place-yard"}, "language": "ru"})), "Покинуть двор?");

    // What could not be done is said with the text, and nothing goes to the journal.
    assert_eq!(say(&mut app, json!({"id": "leave"})), "Leave {$place}?\n# en `leave`: Unknown variable: $place");
    assert_eq!(say(&mut app, json!({"id": "pause-hint", "language": "ru"})), "[Esc] Back\n# ru has no message `pause-hint`; it is shown in en");
    assert!(journal(&app, "locale: ").is_empty(), "{:?}", journal(&app, "locale: "));

    // Mistakes say how to fix the call.
    assert_eq!(refused(&mut app, "text", json!({"id": "trophie"})), "en has no message `trophie` (did you mean `trophies`?)");
    assert_eq!(refused(&mut app, "text", json!({"id": "trophies", "language": "de"})), "no language `de`; languages: en, ru");
    assert!(refused(&mut app, "text", json!({"id": "trophies", "args": [1]})).starts_with("`args` is an object of parameters"));
}

#[test]
fn the_locale_tool_says_what_each_language_has_and_lacks() {
    let mut app = game();
    let all = ok(&mut app, "locale", json!({}));
    assert_eq!(
        all,
        "playing: en\n\
         en English: 12 messages\n\
         ru Русский → en: 11 messages; lacks 1: pause-hint\n\
         # switch: settings_set group=locale key=language value=<id>; `locale language=<id>` lists one language's text\n"
    );
    speak(&mut app, "ru");
    assert!(ok(&mut app, "locale", json!({})).starts_with("playing: ru (missing messages from en)\n"));
    let ru = ok(&mut app, "locale", json!({"language": "ru"}));
    assert!(ru.starts_with("ru (Русский): 11 messages\n"), "{ru}");
    assert!(ru.contains("leave = Покинуть {$place}? [locale/ru/menus.ftl]\n"), "{ru}");
    assert_eq!(refused(&mut app, "locale", json!({"language": "ry"})), "no language `ry` (did you mean `ru`?); languages: en, ru");
}

#[test]
fn an_edited_text_file_applies_while_the_game_runs() {
    let dir = assets_copy("edit");
    let file = dir.join("locale/ru/menus.ftl");
    let mut app = game_on(&dir.to_string_lossy());
    speak(&mut app, "ru");
    ok(&mut app, "ui_open", json!({"screen": "pause"}));
    assert_eq!(shards::locale::reload_text(app.world_mut()), Vec::<String>::new(), "nothing changed yet");

    let original = std::fs::read_to_string(&file).unwrap();
    std::fs::write(&file, original.replace("pause-resume = Продолжить", "pause-resume = Дальше") + "pause-hint = [Esc] Назад\n").unwrap();
    assert_eq!(shards::locale::reload_text(app.world_mut()), ["locale/ru/menus.ftl"]);
    assert_eq!(journal(&app, "reloaded"), ["locale: text reloaded (locale/ru/menus.ftl)"]);
    let tree = ok(&mut app, "ui_read", json!({}));
    assert_eq!(line(&tree, "pause/resume"), "pause/resume  button \"Дальше\" → close ◀ focus");
    assert_eq!(line(&tree, "pause/hint"), "pause/hint  hint \"[Esc] Назад\"", "a message added is there at once");

    // An edit with a mistake is journaled with its place, once, and refused: the file keeps the
    // version that read until it is fixed.
    std::fs::write(&file, original.replace("pause-title = Пауза", "pause-title Пауза")).unwrap();
    assert_eq!(shards::locale::reload_text(app.world_mut()), Vec::<String>::new(), "the edit was refused");
    let errors = journal(&app, "locale: locale/ru/menus.ftl:1:");
    assert_eq!(errors.len(), 1, "{:?}", journal(&app, "locale: "));
    assert!(errors[0].ends_with("; kept the previous version"), "{}", errors[0]);
    let tree = ok(&mut app, "ui_read", json!({}));
    assert!(tree.contains("pause \"Пауза\""), "{tree}");
    assert_eq!(line(&tree, "pause/resume"), "pause/resume  button \"Дальше\" → close ◀ focus");
    shards::locale::reload_text(app.world_mut());
    assert_eq!(journal(&app, "locale: locale/ru/menus.ftl:1:").len(), 1, "the same mistake is journaled once");
    std::fs::write(&file, &original).unwrap();
    assert_eq!(shards::locale::reload_text(app.world_mut()), ["locale/ru/menus.ftl"], "fixed, it applies");
    let tree = ok(&mut app, "ui_read", json!({}));
    assert_eq!(line(&tree, "pause/resume"), "pause/resume  button \"Продолжить\" → close ◀ focus");

    // A language whose files cannot be read now keeps the text it had.
    let unreadable = dir.join("locale/ru/x.ftl");
    std::fs::create_dir(&unreadable).unwrap();
    assert_eq!(shards::locale::reload_text(app.world_mut()), Vec::<String>::new());
    shards::locale::reload_text(app.world_mut());
    let errors = journal(&app, "locale: locale/ru/x.ftl: cannot read:");
    assert_eq!(errors.len(), 1, "{:?}", journal(&app, "locale: "));
    assert!(errors[0].ends_with("; ru keeps the text it had"), "{}", errors[0]);
    let tree = ok(&mut app, "ui_read", json!({}));
    assert_eq!(line(&tree, "pause/resume"), "pause/resume  button \"Продолжить\" → close ◀ focus");
    std::fs::remove_dir(&unreadable).unwrap();
    assert_eq!(shards::locale::reload_text(app.world_mut()), Vec::<String>::new());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_content_check_names_mistakes_in_text_and_in_what_names_it() {
    let app = game();
    let kinds = app.world().resource::<ContentKinds>().clone();
    assert_eq!(check_dir(Path::new(ASSETS), &kinds).unwrap().to_string(), "ok: 3 definitions in 2 files");

    let dir = assets_copy("check");
    let screen = dir.join("content/menus.screen.ron");
    let text = std::fs::read_to_string(&screen).unwrap();
    std::fs::write(&screen, text.replace("\"@pause-resume\"", "\"@pause-resum\"").replace("[\"@speed-slow\", \"@speed-fast\"]", "[\"@speed-slow\"]")).unwrap();
    std::fs::write(dir.join("locale/ru/menus.ftl"), "pause-title = Пауза\npause-resume Продолжить\n").unwrap();
    std::fs::write(dir.join("locale/en/more.ftl"), "pause-title = Paused again\n").unwrap();
    std::fs::write(
        dir.join("content/languages.locale.ron"),
        "{ \"en\": (name: \"English\", fallback: \"ru\"), \"ru\": (name: \"Русский\", fallback: \"en\"), \"de\": (name: \"Deutsch\") }",
    )
    .unwrap();
    let errors: Vec<String> = check_dir(&dir, &kinds).unwrap_err().iter().map(ToString::to_string).collect();
    let has = |needle: &str| assert!(errors.iter().any(|e| e.contains(needle)), "no `{needle}` in:\n{}", errors.join("\n"));
    has("locale `de`: no text: put its Fluent files in locale/de/ under the asset folder");
    has("fallbacks go round and never end: en → ru → en");
    has("locale/ru/menus.ftl:2:14: Expected a token starting with \"=\"");
    has("locale/en/more.ftl: message `pause-title` is in locale/en/menus.ftl too: a message is in one file");
    has("choice `speed`: 1 `labels` for 2 `options`: one for each option, in their order (or none)");
    has("widget `resume` label names message `pause-resum`, which de, en and ru have not, nor their fallbacks (did you mean `@pause-resume`?)");
    let _ = std::fs::remove_dir_all(&dir);
}
