# Ход работ по ТЗ

Сверка с [engine-tz.md](engine-tz.md): что из «Что сделать» и «Готово, когда» каждого модуля E1–E11 уже есть,
где это лежит и чем проверено. Состояние на 14 октября 2026: фазы A и B собраны, фаза C идёт (см. «Фазы»). CI на GitHub
(`.github/workflows/ci.yml`) идёт на каждый push и pull request; все пять заданий зелёные.

Статусы:

- ✅ сделано: код в репозитории, есть тест, сценарий или команда, которыми это проверяют;
- 🟡 частично: работает, но не всё из пункта ТЗ или не всё проверено (в колонке «где» сказано, чего нет);
- ⏳ впереди: в какой фазе (A–E) пункт делается. Кода под него нет.

Правила для этого файла: статус ставят по коду и тестам, а не по намерению; если пункт нельзя проверить здесь,
так и написано. Меняя модуль, меняйте и его раздел.

## Фазы

Схема фаз из ТЗ («Порядок по фазам») и состояние приёмки. Приёмка фазы — это «Готово, когда» её модулей,
проверенное скриптами в CI.

| фаза | этап GDD | модули | состояние |
|---|---|---|---|
| A | прототип боя (GDD 39) | E1; E2; E3; из E4 — база контента, префабы, `content check`, детерминированная загрузка; из E11 — оверлеи боя, бессмертие, спавн префаба, сценарные тесты | ✅ **собрана и проверена** локально и в CI. Все пункты «Готово, когда» E1–E3 и частей E4/E11 выполнены и проверены тестами и командами ниже. Тег `v0.2.0` ещё не поставлен (его ставит мейнтейнер, см. `CHANGELOG.md`) |
| B | мир | E5; E6; из E4 — уровни из Blender; из E11 — `clearance`/`path_to` по навсетке, снапшоты | 🟡 **собрана, проверена** локально и в CI. «Готово, когда» E5, E6, уровней из Blender и снапшотов выполнены и проверены тестами, сценариями и реплеями (таблица ниже). Не сделано до конца: стриминг геометрии тайлов для физики (стримится только картинка), `path_to` и `clearance` по навсетке (по навсетке `nav_path` и `move_to path=true`), навсетка под несколько тел. Тег `v0.3.0` не поставлен |
| C | кооператив (этап 3) | E7; E8; из E11 — стенд «хост + клиенты» | 🟡 **идёт.** Есть: задел (клиенты без `SimSet`, правила репликации, кодек состояния, отложенный `step`, хуки выхода, флаги запуска), канальный слой сети `shards_net` (E7.2), сохранения E8.1–5 (`shards_save`), настройки игрока E8.6 (`shards_settings`). Впереди: сессия и стенд «хост + клиенты», ввод с тиками по сети, репликация, предсказание, подключение и отключение, отладка сети, сохранения в кооперативе |
| D | контент и интерфейс | E9; остаток E4 и E11 | ⏳ не начата |
| E | платформа | E10 | 🟡 начата: из E10 есть время тика (`step`, `[perf]`, `--tick-budget-ms`); остальное не начато (Steam и Windows нельзя проверить на этой машине) |

Что проверено для приёмки фазы A (все команды запускались из корня репозитория, без окна, если не сказано иное):

| что | как проверено |
|---|---|
| сборка и тесты движка | `cargo test --workspace` (475 проходят, 0 падают; разбивка в разделе «Тесты»); `cargo clippy --workspace --all-targets --locked -- -D warnings` и то же для шаблона без замечаний; `template/Cargo.lock` не расходится с корневым |
| шаблон отдельно от движка | `cargo test --manifest-path template/Cargo.toml` (19 тестов); копия `template/` вне репозитория, переименованная, с зависимостью-`path` на `crates/shards`: `cargo test`, запуск без окна, `shards scenario`, перезапись реплея (отдельный репозиторий с тегом так не проверить: тега ещё нет) |
| сценарии против живой игры | `cargo sandbox --headless --paused --port 0 --port-file F`, `SHARDS_PORT_FILE=F cargo shards scenario crates/sandbox/scenarios/*.scenario` → `5 of 5 scenarios passed` |
| контент | `cargo sandbox --check-content` → `ok: 38 definitions in 9 files`; сломанная копия контента даёт ошибки с файлом, строкой и подсказкой (README, «Контент») |
| эталонные реплеи без окна | `cargo sandbox --replay tests/replays/combat_prototype.json --verify` → `identical to the recording (39 checks)`, то же для `obstacle_course.json` (17 проверок) и для реплея шаблона (5 проверок) |
| те же реплеи в окне | `xvfb-run -a cargo sandbox --replay tests/replays/obstacle_course.json --verify --windowed` → `identical to the recording (17 checks) [1010 ticks …, 21 frames]` (программный Vulkan, lavapipe); так же `combat_prototype.json` (39) и реплей шаблона |
| скриншот с метками | `xvfb-run -a cargo sandbox --paused …` и `shards shot`: PNG и легенда |

Что проверено для приёмки фазы B (команды запускались из корня репозитория без окна, если не сказано иное; `cargo test --workspace`:
752 теста проходят, 0 падают, 17 `#[ignore]`; шаблон: 23 проходят):

| что | как проверено |
|---|---|
| шаблон генерирует регион 8×8 из тестовых тайлов | `template/`: долина `valley` (4 тайла: луг, колонны, ворота и руина из файла glTF), тесты `tests/game/region.rs`; `cargo run -- --worldgen-report valley --seeds 1..1000` → `1000/1000 passed (63 after a retry)` |
| 1000 зёрен проходят проверку | `cargo sandbox --worldgen-report wilds --seeds 1..1000` → `1000/1000 passed (42 after a retry), 0 fell back, 0 failed, 0 panicked [1000 seeds in 0.31s]`; тесты `a_thousand_seeds_pass_their_checks_quickly` (`shards_world`), `a_thousand_seeds_of_the_valley_pass_its_checks` (шаблон); подземелье `crypt` — те же 1000 зёрен за 0.17 с; CI-задания `headless-cli` и `template` гоняют отчёт |
| одно зерно и версия — побайтно один манифест | тесты `an_8x8_region_generates_passes_its_checks_and_is_byte_stable`, `the_same_seed_builds_the_same_world_down_to_ids`, `the_same_seed_and_version_give_the_same_manifest_byte_for_byte` (шаблон); сценарий `region` сверяет хэш `d9d721509840d909` дебрей с зерном 42 и повторяет его после другого региона |
| переход между регионами с экраном загрузки без окна | тесты `regen_replaces_the_region_through_the_loading_screen_and_keeps_the_hero`, `a_region_changed_from_on_enter_in_game_does_not_enter_the_game_again`, `going_to_the_valley_passes_the_loading_screen_headless_and_keeps_the_hero` (шаблон); сценарий и реплей `region` (игрок тот же, тик не сдвинулся) |
| враг обходит колонны по навсетке и не выходит за зону | сценарий `npc_navmesh` (`npc_detour` — обход колоннады, `npc_beyond_leash` — ни разу за поводок), реплей `npc_navmesh.json`; тесты `an_npc_walks_around_pillars_by_navmesh_and_stays_on_its_leash`, `a_sentry_chases_to_its_leash_goes_home_healing_and_never_crosses`; рейдер шаблона: `a_raider_comes_by_the_navmesh_and_keeps_to_its_camp` |
| при одном игроке из трёх врагов атакуют не больше двух | сценарий `npc_director` (30 с боя, `three_attacking` ни разу), тест `one_player_three_enemies_never_more_than_two_attack_at_once`, реплей `npc_director.json` |
| всё совпадает в реплее | `cargo sandbox --replay tests/replays/<файл>.json --verify` для всех шести (`obstacle_course`, `combat_prototype`, `npc_director`, `npc_navmesh`, `region`, `snapshot`): `identical to the recording`; то же в окне под Xvfb (`--verify --windowed`); тест `every_reference_replay_verifies_headless`; реплеи шаблона `smoke` и `valley` |
| шаблон собирает уровень из Blender-файла с маркерами | `template/assets/tiles/ruin.gltf` + `ruin.level.ron` (обязательные маркеры); тесты `a_typo_in_the_level_is_an_error_that_names_the_fix`, колонна руины на месте, где поставил тайл. Файл сделан тестовым генератором glTF (`crates/sandbox/tests/sandbox/tiles.rs`), настоящий экспорт из Blender здесь не пробовали |
| реплей начинается со снапшота, фаза босса — загрузка снапшота | сценарий `snapshot` и реплей `snapshot.json`; тесты `go_to_boss_phase_two_is_a_snapshot_loaded_by_name`, `a_snapshot_in_the_generated_region_goes_on_the_same_in_a_fresh_arena`; `ai_core::snapshot`, `shards::run` (код 2 для снапшота, который сборка не принимает) |
| сценарии модулей идут в CI | `test` (`every_scenario_file_passes_on_a_fresh_game`, `…after_another`, `…backwards`) и `headless-cli` (вперёд, потом назад в одной живой игре) |
| `content check` читает уровни и навигацию | `cargo sandbox --check-content` → `ok: 70 definitions in 23 files`; сломанные копии: опечатка компонента в узле, пропавший маркер, устаревший файл навсетки, опечатка сокета и точки — ошибки с файлом, узлом и подсказкой (README) |
| `clearance`/`path_to` по навсетке | 🟡 по навсетке отвечают `nav_path` (формат `path_to`, плюс тайлы маршрута) и `move_to path=true`; сами `path_to` и `clearance` остались запросами к физике |

## Сводка

| модуль | фаза | состояние |
|---|---|---|
| E1. Ядро, сборка, шаблон | A | ✅ кроме сохранения и кооператива в шаблоне (C) |
| E2. Движение | A | ✅ физика на parry3d, контроллер, ввод, камера, оценка пути; навсетка — E6 |
| E3. Действия, бой, анимация | A | ✅ бой на данных, анимационный мост, телеграфы |
| E4. Контент и уровни | A (часть), B, D | 🟡 база, префабы, `content check` — ✅; уровни из Blender — 🟡 (формат и пайплайн ✅, настоящий экспорт Blender не пробовали) |
| E5. Генерация мира | B | 🟡 генератор, решатель, проверки, манифест, подземелья, отчёты, смена региона, активные зоны — ✅; стримится только картинка тайлов |
| E6. Навигация и ИИ врагов | B | ✅ навсетка по тайлам, поведение, восприятие, директор, планировщик; 🟡 уклонение простое, навсетка под одно тело |
| E7. Сеть и кооператив | C | 🟡 транспорт (`shards_net`) и задел; сессии ещё нет |
| E8. Сохранения и настройки | C | ✅ сохранения (1–5, `shards_save`) и настройки игрока (6, `shards_settings`); сохранения в кооперативе ждут сессии (E7) |
| E9. Интерфейс, звук, локализация | D | ⏳ |
| E10. Рендер, производительность, платформа | E | 🟡 время тика (`step`, `[perf]`, `--tick-budget-ms`) — ✅; остальное — ⏳ |
| E11. Инструменты разработчика | A (часть), B, C, D | 🟡 сценарии, оверлеи (бой, коллайдеры, навсетка, тайлы), `god`, `spawn_prefab`, `goto_tile`, снапшоты, `clearance`, `path_to` — ✅; остальное по фазам |

## Общие требования ТЗ

| № | требование | статус | как соблюдается |
|---|---|---|---|
| 1 | в движке нет LLM, код на английском, документация на русском, Bevy 0.20.0 | ✅ | `AGENTS.md`; Bevy `0.20.0` в `[workspace.dependencies]`, точные версии всех зависимостей — в `Cargo.lock` |
| 2 | симуляция в `FixedUpdate` внутри `SimSet`, случайность через `SimRng`, компоненты входят в хэш реплея | ✅ | тесты `gameplay_systems_have_a_total_order` (песочница, `shards_action`) и `the_engine_with_motion_has_a_total_order_in_the_fixed_tick` (`shards_motion`); хэш — по отражённым компонентам `AiVisible`-сущностей |
| 3 | геймплей без окна такой же, как с окном | ✅ | `--verify --windowed`, тест `observe_reads_the_same_with_a_window_camera_or_without` |
| 4 | каждый модуль открыт ИИ: describe, `journal_event`, инструменты, условия | ✅ для A- и B-модулей | `register_ai_component`, `add_ai_tool`, `add_ai_query`, `add_ai_condition` у `shards_core`, `shards_content`, `shards_motion`, `shards_action`; мир: строки `TILE`/`POI` в `observe`, `ai/status` → `world`, `near_poi`; уровни: маркеры в описании сущностей; NPC: строка состояния, журнал переходов и жетонов, `npc_state`, условия `npc_*` |
| 5 | числа и определения — файлы ассетов с горячей перезагрузкой | ✅ для A и B | `content/**/*.<вид>.ron`: движение, действия, мувсеты, статусы, анимнаборы, префабы, карта ввода; тайлы, сокеты, регионы, подземелья, точки, уровни, поведение, восприятие, директор, планы атак, настройки навсетки. Запечённая навсетка — файлы `assets/nav/`, не RON-контент: её печёт `--bake-nav` |
| 6 | сторонний крейт: проверка Bevy 0.20, закрепление, спрятан за API движка | ✅ | проверка по индексу crates.io 13 и 14 октября 2026: avian3d 0.7.0, bevy_enhanced_input 0.26.0 и lightyear 0.30.1 требуют Bevy 0.19, поэтому физика на `parry3d =0.31.1` (внутри `shards_motion`, его типов в публичном API нет), карта ввода своя; `ron =0.12.2` для контента. bevy_replicon 0.45 требует Bevy 0.20 (пригодится в фазе C) |
| 7 | у модуля: юнит-тесты, сценарий в песочнице, проверка реплеем, раздел README | ✅ для A и B | см. модули ниже; сценарии `npc_director`, `npc_navmesh`, `region`, `snapshot` и их реплеи; README — по разделу на модуль (E4.2, E5, E6, снапшоты) |
| 8 | модуль — серия PR, начинать с фазы A | — | порядок работ |

## E1. Ядро, сборка и шаблон игры

| № | что сделать | статус | где и чем проверено |
|---|---|---|---|
| 1 | крейт `shards`: `EnginePlugins`, `EngineConfig` (окно/без окна, порт, seed, тик, модули), реэкспорт | 🟡 | `crates/shards`, `crates/shards_core`. Всё есть, кроме переключателя «включённые модули»: `EnginePlugins` — группа плагинов, и модуль выключают `.disable::<…>()`, а в конфиге поля для этого нет. Состав виден в `ai/status` (`engine.modules`) |
| 2 | режим без окна из тех же плагинов, минус окно, рендер и звук | ✅ | `crates/shards/src/plugins.rs` (`without_display`); тест `headless_is_the_same_plugins_minus_display`; `MinimalPlugins` в песочнице больше нет |
| 3 | состояния: загрузка, меню, игра, экран загрузки региона, пауза | 🟡 | `crates/shards_core/src/state.rs`, `loading.rs`; тесты `crates/shards_core/tests/core.rs`. `EngineState::{Loading, MainMenu, InGame, RegionLoading}` и `PauseState`, шлюз загрузки. `RegionLoading` теперь работает: через него идёт каждая смена региона (`regen`, `region_load`), с тестами `shards_world` (`regen_replaces_the_region_through_the_loading_screen_and_keeps_the_hero`). Картинки экрана загрузки нет (E9) |
| 4 | версии: SemVer, теги git, CHANGELOG | 🟡 | `CHANGELOG.md`, версия `0.2.0` в `[workspace.package]`. Теги ставит мейнтейнер; ни `v0.1.0`, ни `v0.2.0` ещё нет |
| 5 | шаблон в `template/`: персонаж, враг, арена, сохранение, кооп на двоих | 🟡 | `template/`: персонаж, враг, арена, свой вид контента, инструмент и условие, сценарий, реплей, 23 теста (и один `#[ignore]`: запись эталонных реплеев). Сохранения и кооп — фазы C (в README шаблона оставлены разделы-заглушки) |
| 6 | CI: тесты, clippy, сборка шаблона, `--verify` реплеев | ✅ | `.github/workflows/ci.yml` (задания `test`, `clippy`, `template`, `headless-cli`, `replays-window`), `.github/actions/setup`. Идёт на GitHub на каждый push и pull request |

Граница («игра добавляет плагины поверх `EnginePlugins` и не меняет код движка»): ✅, шаблон и песочница
собраны только на `shards`, тест `the_game_depends_on_shards_and_nothing_else`.

Для ИИ (`ai/status` показывает версию и модули): ✅ тест `status_names_the_engine_and_its_modules`.

| «Готово, когда» | статус | как проверяется |
|---|---|---|
| шаблон собирается и проходит тесты отдельно от песочницы | ✅ | `template/` — отдельный workspace с единственной зависимостью `shards`; CI-задание `template`; `cargo test` в его копии вне репозитория |
| один реплей совпадает с окном (под Xvfb) и без окна | ✅ | `--verify` и `--verify --windowed` дают один отчёт; CI-задание `replays-window` (необязательное, `continue-on-error`) |
| CI зелёный на каждый PR | ✅ | задания идут на каждый push и pull request; на ветке фаз A–C зелёные все пять (`replays-window` пока необязательное, `continue-on-error`) |

## E2. Движение: физика, персонаж, ввод, камера

| № | что сделать | статус | где и чем проверено |
|---|---|---|---|
| 1 | физика в `FixedUpdate`, детерминированная; слои, триггер-объёмы, raycast и shapecast | ✅ | `crates/shards_motion/src/physics/`; тесты `tests/motion/physics.rs`. Кандидата avian3d нет смысла брать: он для Bevy 0.19. Своя кинематическая физика на parry3d: коллайдеры с `ColliderId`, слои, объёмы (вода, поток, триггер), `raycast`/`shapecast`/`overlaps`/`volumes_at`. Широкая фаза — линейный перебор с AABB (BVH — потом) |
| 2 | контроллер: ходьба, бег, спринт, прыжок, ступени, перелезание, склоны, плавание, планер; событие падения | ✅ | `src/controller/`; `tests/motion/controller.rs` (28), `edge_cases.rs`, `course.rs`; `Landed { fall_height }` |
| 3 | параметры контроллера — ассет с горячей перезагрузкой | ✅ | вид `locomotion` (`CharacterParams`); тест `locomotion_content_sets_the_numbers_and_a_reload_changes_them_on_the_next_tick` |
| 4 | ввод: карта действий в `Intent`, клавиатура, мышь, геймпад, переназначение с сохранением, удержание/нажатие, контексты | 🟡 | `src/input/`; `tests/motion/input.rs` (30). `bevy_enhanced_input` для Bevy 0.20 нет, карта своя. Переназначение сохраняется в настройках `controls` (E8.6, `settings.toml`; старый `input.ron` переносится один раз и только если игра попросила: `InputSettings::for_game`), `accessibility.hold_to_toggle` делает удержание переключателем. Геймпад проверен тестами на сущностях `Gamepad`, настоящим устройством не пробовали |
| 5 | камера: орбита, стены, захват цели и переключение, помощь прицеливания, тряска, поле зрения | ✅ | `src/camera/` (математика без окна: `math.rs`; системы: `systems.rs`, feature `render`); `tests/motion/camera.rs`. Камера — только представление, геймплей и `observe` её не читают |
| 6 | API предупреждения об атаках за кадром | ✅ | `threat_indicators(world, viewer, forward)` в `shards_action::zone`; маркеры на краю экрана — `shards_action::render::hud` |

Для ИИ: режим движения и скорость в describe (`ground 3.6m/s`) — ✅; `jump`, `sprint`, `glide` — ✅; `clearance` и
`path_to` по физике — ✅, `path_to` — локальный сеточный A* (0.5 м, `src/perception/path.rs`); навсетка (E6) отвечает за `move_to path=true` и `nav_path` в загруженном регионе, `path_to` остаётся локальным.

| «Готово, когда» | статус | как проверяется |
|---|---|---|
| агент скриптом проходит полосу препятствий: ступеньки, уступ, вода, обрыв с планером | ✅ | `crates/sandbox/scenarios/obstacle_course.scenario` (игрок со scope `player`); тесты `course.rs` песочницы и `the_obstacle_course_is_finished_the_same_way_every_time` |
| без окна и с окном результат одинаковый, реплей полосы совпадает | ✅ | `tests/replays/obstacle_course.json`: тест `every_reference_replay_verifies_headless`, `--verify --windowed` под Xvfb, CI-задание `replays-window` |

## E3. Действия, бой и анимация

| № | что сделать | статус | где и чем проверено |
|---|---|---|---|
| 1 | действие — ассет: фазы, окна отмены, кривая смещения, стоимость, хитбоксы, флаги, сетевой id | ✅ | `crates/shards_action/src/def.rs` (виды `action`, `moveset`, `combat_tuning`); тесты в `def.rs` и `tests/combat/actions.rs`. Кривая смещения — данные (запекать её из анимации должен будущий конвейер Blender, B); `net_id` уникален (валидация) |
| 2 | машина действий: `Intent`, буфер ввода, отмены по окнам, комбо, блок и идеальный блок, уклонение | ✅ | `src/machine.rs`; `a_three_hit_combo_chains_only_inside_cancel_windows`, `a_press_outside_the_window_is_buffered_then_rejected`, `dodge_i_frames_evade_a_hit_and_a_late_dodge_is_hit` |
| 3 | хитбоксы и хёртбоксы в `FixedUpdate` через физику; одно действие попадает по цели один раз | ✅ | `src/combat.rs` (`overlaps` по слою `HURTBOX` после движения); `one_action_hits_a_target_once_however_long_it_overlaps` |
| 4 | конвейер урона, события `Damaged`, `Staggered`, `Died`, порядок по `AiId` | ✅ | `src/combat.rs`; `hits_resolve_in_ai_id_order_whatever_the_query_order`, `damage_hooks_run_after_the_engine_modifiers` |
| 5 | ресурсы бойца: здоровье, выносливость, стойкость | ✅ | `src/fighter.rs`; `stamina_waits_for_its_delay`, `broken_poise_comes_back_full` |
| 6 | каркас статусов без конкретных эффектов | ✅ | `src/status.rs`; `statuses_build_up_from_hits_and_run_the_game_hooks`, `the_limit_holds_a_status_at_full_buildup`, `resistance_scales_buildup`. Статусов в контенте песочницы нет: проверено тестовым контентом |
| 7 | анимация: glTF со скинами, граф за состоянием действия, смешивание, общий скелет | 🟡 | `src/anim.rs` (вид `animset`, `select_clip`), `src/render/bridge.rs`; `crates/shards/tests/action_render` (скин-модель и клипы из другого файла с тем же скелетом). Один `AnimationPlayer` на персонажа: слоёв (верх/низ тела) нет. Настоящих моделей в репозитории нет (конвейер уровней E4.2 моделей и клипов не трогает). События анимации — только для эффектов и звука |
| 8 | телеграфы: зоны на земле, звуковые метки, символы | ✅ | `src/zone.rs`, `src/draw.rs`, `src/render/overlay.rs`, `src/cue.rs` (`SoundCue` — для модуля звука E9, играть их пока некому) |

Для ИИ: describe фазы действия — ✅; подсказка «ты в зоне удара, выйди на N м» (`ALERT …`) — ✅; инструменты
`light_attack`, `heavy_attack`, `block`, `parry`, `use_technique`, `lock_on` — ✅; отказы в журнале — ✅
(`an_input_that_runs_out_is_journaled_as_rejected`).

| «Готово, когда» | статус | как проверяется |
|---|---|---|
| песочница на данных воспроизводит бой GDD 39: комбо из трёх ударов, тяжёлая атака, блок, уклонение, выносливость, враг с четырьмя атаками, стойкость, лечение | ✅ | контент `crates/sandbox/assets/content/combat/`; тесты `crates/sandbox/tests/sandbox/actions.rs` (10) |
| агент проходит бой скриптом, без окна и с окном одинаково, реплей совпадает | ✅ | `combat_prototype.scenario` (страж побеждён за 11 раундов; тест `the_combat_prototype_is_won_whatever_the_warden_chooses` прогоняет его после шести разных пауз), реплей `tests/replays/combat_prototype.json`, `--verify --windowed` |

## E4. Контент и уровни

| № | что сделать | статус | где и чем проверено |
|---|---|---|---|
| 1 | база контента: типизированные определения, стабильные id, горячая перезагрузка, проверка схемы и ссылок | ✅ | `crates/shards_content` (`db.rs`, `load.rs`); `tests/content.rs` (17). Формат RON; BSN — когда у Bevy появится загрузчик `.bsn` |
| 2 | конвейер уровней из Blender | 🟡 | `crates/shards_level` (E4.2): glTF 2.0 с custom properties → коллайдеры, датчики, маркеры (`spawn`, `socket`, `navlink`, `trigger`, `interactable`, `camp`, `landmark`, `custom:`), префабы, компоненты по имени; вид `level`; инструменты `level_info`, `spawn_level`, `level_reload`; горячая перезагрузка; тесты `tests/level/` (77). Читает только JSON файла, крейт `gltf =1.4.1` (Skein не берём). **Не проверено на настоящем экспорте Blender:** файлы в репозитории сделаны тестовым генератором glTF |
| 3 | префабы: сущность из ассета плюс переопределения | ✅ | `src/prefab.rs`; `prefabs_spawn_with_extends_and_overrides`, `prefab_problems_are_found_when_content_loads`. Сейчас это RON с компонентами по именам и `extends`; BSN-сцен нет |
| 4 | `shards content check` | ✅ | `shards content check` (живая игра) и `--check-content` (без игры, код 0/1), `check_dir`. Читает файлы уровней (`level`: узлы, компоненты, префабы, обязательные маркеры), наличие файлов уровней тайлов, запечённую навигацию. Нет: проверки, что маркеры-сокеты файла совпадают с сокетами тайла в RON (в `shards_content` нет связи между видами); узлы файла тайла читаются, только если файл назван ещё и определением `level` |
| 5 | детерминированная загрузка: симуляция стартует после загрузки, загрузка не сдвигает тики | ✅ | `LoadingGate` и `SimGate`; `nothing_ticks_until_loaded_and_loading_never_shifts_a_tick`; файлы уровней тайлов тоже за шлюзом (`the_start_region_waits_for_its_level_files_and_stands_before_the_hero`) |

Для ИИ: `content_list`, `content_get`, `spawn_prefab`, перезагрузка по имени (`content_reload`) — ✅, ошибки контента в
журнале — ✅. Правка контента на лету идёт в реплей через `content_apply`
(`a_reload_is_recorded_and_its_replay_plays_the_same_game`). Уровни: маркеры в описании сущностей
(`the_ai_sees_the_markers_that_matter`), ошибки файла с узлом и свойством
(`a_bad_component_name_is_found_with_the_file_the_level_and_the_node_path`); изменённый файл заменяет экземпляры через
записываемое действие (`an_edited_file_replaces_the_spawned_level_through_a_recorded_action`), реплей хэширует поставленные
уровни и дайджесты их файлов (`a_recording_against_another_version_of_the_file_diverges_at_its_first_check`).

| «Готово, когда» | статус | как проверяется |
|---|---|---|
| шаблон собирает уровень из Blender-файла с маркерами | ✅ локально, с оговоркой | шаблон: `assets/tiles/ruin.gltf` как тайл долины и `ruin.level.ron` с обязательными маркерами; тесты `region.rs` и `content.rs` шаблона; файл — вывод тестового генератора, не экспорт Blender |
| правка числа в ассете действия видна в запущенной игре без перезапуска | ✅ | `an_edited_file_is_reloaded_while_the_game_runs`, `an_edited_number_changes_the_next_hit_and_replays_the_same`; вручную: правка `damage` в запущенной игре дала `content reloaded: combat/player.action.ron (1 definition changed)` |
| `content check` находит битую ссылку и называет файл | ✅ | `a_missing_reference_names_the_file_the_id_and_the_field`; вручную (README, «Контент», «Мир и генерация», «Уровни из Blender») |

## E5. Генерация мира и стриминг (фаза B)

Крейт `shards_world`. Тесты: `crates/shards_world/tests/world/` (54) и юнит-тесты (17).

| № | что сделать | статус | где и чем проверено |
|---|---|---|---|
| 1 | каркас генератора по шагам | ✅ | `src/generate.rs`: стадии `Points`, `Roads`, `Cells`, `Fill`, `Secondary`, `Decorate`, `Checks`, шаги движка (`engine.*`) работают на данных, игра добавляет шаг (`add_worldgen_step`) или заменяет стадию (`replace_worldgen_stage`); случайность — `GenContext::rng()` на `DetRng` (seed, версия, попытка, шаг). Тесты `game_steps_join_the_pipeline_and_their_panics_are_reported_with_the_seed`, `an_8x8_region_generates_passes_its_checks_and_is_byte_stable` |
| 2 | решатель сокетов | ✅ | `src/solver.rs`, `src/sockets.rs`: сокеты с высотой и частями, повороты, фиксированные клетки, границы, лимиты чисел, распространение, наименьшая энтропия, ограниченный откат; журнал изменений вместо копий (сетка 64 × 64 укладывается в 8 МиБ). Тесты `fixed_cells_borders_and_limits_hold`, юнит-тест памяти в `solver.rs` |
| 3 | проверки: достижимость, уклоны, расстояния | 🟡 | `src/checks.rs`: `spawn`, `reachable`, `slope`, `distance`, `isolated_roads`, `custom`; правила — данные региона, у каждой отказ называет правило и клетки. Тесты `checks.rs` (5). Достижимость идёт по проходимым сокетам и `links` тайлов; навсвязи (`navlink`) из файлов уровней она не читает (генерация — чистая функция контента, файлов не открывает) |
| 4 | манифест региона | ✅ | `src/manifest.rs`: `shards-region/1`, каноническая запись (ключи по алфавиту, три знака, один тайл в строке), `hash` FNV-1a, `origin` не пишется при (0, 0), загрузка не запускает генератор. Тесты `the_version_and_the_content_are_part_of_the_manifest`, `a_saved_manifest_loads_back_as_it_was`. Хранение в сохранении — E8 |
| 5 | подземелья из комнат | ✅ | `src/dungeon.rs`: виды `room` и `dungeon`, главный путь поиском в глубину, ветки, ручные комнаты как фиксированные узлы (минимум пути поднимается до их числа, комната с одной дверью — ошибка контента). Тесты `dungeon.rs` (4), `a_crypt_loads_like_any_region` |
| 6 | `shards worldgen --seeds 1..1000 --report` | ✅ | `src/report.rs`; `shards worldgen --region R --seeds S --report`, `<игра> --worldgen-report R --seeds S`, инструмент `worldgen_report`; отчёт: прошло/повтор/запасная/не прошло/паники с зерном, причины с примерами зёрен, `errors` (когда виновата не попытка), расстояния. Тесты `a_thousand_seeds_pass_their_checks_quickly`, `a_seed_that_fails_with_no_attempt_to_blame_says_why_in_the_report`, `worldgen_report_runs_in_a_live_game`, юнит-тесты в `shards`; вручную: `1000/1000 passed … in 0.31s` |
| 7 | стриминг; ИИ врагов только в активных зонах | 🟡 | Регион грузится целиком за экраном загрузки (`RegionLoading`), ИИ врагов — только в активных зонах (`ActiveZone`/`Dormant`, `zones.rs`; тест `npcs_far_from_every_player_go_dormant_and_wake_when_one_comes`). **Стримится только картинка тайлов** по расстоянию (`stream.rs`, `visual_lods`, только с окном; тест `tile_visuals_come_and_go_with_the_players`): геометрия тайлов для физики создаётся вся сразу, подгрузки по расстоянию нет |

Загрузка региона: `regen`, `region_load`, `goto_tile`, `region_info`, `world_overlay`, стартовый регион (`start_region`), `on_region_loaded` для модулей
(навсетка), `origin` региона. Тесты: `regen_replaces_the_region_through_the_loading_screen_and_keeps_the_hero`,
`a_session_that_changes_regions_replays_identically`, `a_manifest_with_a_prefab_content_lacks_is_refused_before_the_old_region_goes`,
`players_and_goto_tile_stand_on_a_floor_above_the_tile_plane_not_inside_it`. Тайлы из файлов уровней (`TileLevelsPlugin`):
`level_tiles_stand_where_their_nodes_say_for_every_quarter_turn`, `a_session_with_level_tiles_changing_regions_replays_identically`,
`a_level_with_problems_refuses_the_change_before_anything_is_touched`.
Оверлей `world_overlay` (границы тайлов, сокеты, точки, дороги) тестами не покрыт, но проверен глазами: `shards shot` под
Xvfb показывает его вместе с оверлеем навсетки.

Для ИИ: в `observe` строки `TILE` и `POI` (`TILE (1, 1) gate rot 0 · wilds seed 42 v1`, `POI e23 spawn.0 0.0m here · …`) — ✅;
инструменты `regen`, `goto_tile`, `worldgen_report`, `region_load`, `region_info` — ✅; `ai/status` → `world`; условие `near_poi`.

| «Готово, когда» | статус | как проверяется |
|---|---|---|
| шаблон генерирует регион 8×8 из тестовых тайлов | ✅ | шаблон: долина `valley` (4 тайла, три лагеря), песочница: `wilds` (9 тайлов) |
| 1000 зёрен проходят проверку | ✅ | `--worldgen-report wilds --seeds 1..1000`, то же для `valley`; тесты; CI-задания `headless-cli`, `template` |
| одинаковые зерно и версия дают побайтно одинаковый манифест | ✅ | `the_same_seed_and_version_give_the_same_manifest_byte_for_byte` (шаблон), сценарий `region` (хэш `d9d721509840d909`) |
| переход между регионами с экраном загрузки работает без окна | ✅ | `regen_replaces_the_region_through_the_loading_screen_and_keeps_the_hero`, реплей `region.json`, `--verify --windowed` |

## E6. Навигация и ИИ противников (фаза B)

Крейт `shards_npc`. Тесты: `crates/shards_npc/tests/npc/` (60) и юнит-тесты (19); `crates/shards_world/tests/world/nav.rs`.

| № | что сделать | статус | где и чем проверено |
|---|---|---|---|
| 1 | навсетка по тайлам, глобальный маршрут | ✅ | `src/nav/`: тайлы и комнаты запекаются заранее (`--bake-nav`, `nav_bake`, файлы `assets/nav/*.navtile.ron`, хэш против определения, префабов и файла уровня, `BAKE_VERSION`), при загрузке региона сшиваются по открытым сокетам и `navlink`, вырезаются твёрдыми телами, которых запекание не видело; маршрут: A* по графу тайлов → A* по сетке коридора → натягивание нити. Запросы без окна. Правило движения — то же `step_onto`, что у `path_to` и контроллера. Своя навсетка, не vleue_navigator/oxidized_navigation/landmass. Тесты `navmesh.rs`: `a_tile_bakes_to_the_same_bytes_every_time`, `bakes_are_checked_against_their_tiles_and_baked_again_when_stale`, `stitching_turns_tiles_with_them_and_follows_open_sockets`, `carving_is_the_solids_standing_now_not_their_history`, `routes_go_around_a_pillar_inside_a_tile_and_across_three_tiles`, `a_hundred_routes_on_an_8x8_region_stay_within_budget`; `generated_regions_get_a_navmesh_that_routes_between_their_points` (`shards_world`) |
| 2 | агенты: следование пути, уклонение, ограничение зоны погони | 🟡 | следование и поводок `Home` — ✅ (`a_sentry_chases_to_its_leash_goes_home_healing_and_never_crosses`); уклонение между NPC простое (упреждающий поворот, расталкивание; `two_npcs_walking_into_each_other_pass_without_touching`); цель на виду — прямая только там, где навсетка подтверждает шаги по земле, нет маршрута — NPC стоит (`an_npc_does_not_walk_off_a_cliff_toward_a_target_in_plain_sight`, `an_unreachable_goal_is_a_clear_error`) |
| 3 | каркас поведения NPC, восприятие | ✅ | `behavior` (автомат состояний в контенте; utility-оценки нет), хуки `add_npc_condition` и `add_npc_state`; зрение лучами физики, слух (`Noise`), память; случайность через `SimRng`. Тесты `fsm.rs` (4), `perception.rs` (5), `warden::the_warden_as_data` |
| 4 | режиссёр сражений: жетоны атаки | ✅ | `director`: жетоны по числу игроков (в данных), бюджет, разрешённые сочетания, роли ждущих; жетон возвращается по окончании атаки, терпению, смерти, сну, потере цели, приказу агента, снижению предела. Тесты `director.rs` (10), сценарий `npc_director` |
| 5 | планировщик атак | ✅ | `attack_plan`: веса, окна дистанции, откаты, запреты последовательностей, бюджет; общий для скелетов и стража песочницы (страж полностью на нём). Тесты `planner.rs` (8 юнит), `the_written_out_states_wait_then_strike`, сценарии `combat_prototype` и `boss_fight` на NPC |

Для ИИ: строка состояния NPC в `observe` (`fight e5 · token · leash 9/10m`, `idle (dormant)`) — ✅; переходы, выдача и возврат жетонов
в журнале — ✅; инструменты `nav_path`, `npc_state`, `npc_set_state`, `nav_info`, `nav_bake`; условия `npc_attacking`, `npcs_calm`,
`npc_beyond_leash`, `npc_detour`. Оверлей навсетки (`nav_overlay`: клетки, связи через края, navlink, маршруты NPC; E11) — ✅:
тестов на него нет, но `shards shot` под Xvfb показывает его вместе с `world_overlay`.

| «Готово, когда» | статус | как проверяется |
|---|---|---|
| враг в песочнице обходит колонны по навсетке и не выходит за свою зону | ✅ | сценарий `npc_navmesh`, тест `an_npc_walks_around_pillars_by_navmesh_and_stays_on_its_leash`, у рейдера шаблона `a_raider_comes_by_the_navmesh_and_keeps_to_its_camp` |
| при одном игроке из трёх врагов атакуют не больше двух | ✅ | сценарий `npc_director` (`three_attacking` не выполняется ни на одном тике), `one_player_three_enemies_never_more_than_two_attack_at_once` |
| всё совпадает в реплее | ✅ | `npc_director.json`, `npc_navmesh.json`; тесты `a_recorded_npc_fight_plays_back_identically`, `a_recorded_npc_walk_on_the_navmesh_plays_back_identically`; снапшот: `an_npc_fight_loaded_at_any_tick_ends_the_same`, `a_generated_region_with_npcs_on_its_navmesh_loads_and_goes_on_identically` |

Ограничения навсетки: запекается под одно тело (`agent`), ходок, которому его движения не под силу, планирует по локальной
сетке (`a_walker_that_climbs_less_than_the_baked_body_plans_on_the_local_grid`); прыжка у тел нет, `navlink` годится для спуска,
подъёма в пределах перелезания и шага (`navlinks_the_body_cannot_make_are_not_used`); вырезание умеет только выключать клетки;
навсетку нужно запекать заново после правки тайла (`BAKE_VERSION` — после правки самого запекания).

## E7. Сеть и кооператив (фаза C)

| № | что сделать | статус |
|---|---|---|
| 1 | сессия: одиночная, хост, клиент | ⏳ C (есть `NetRole`, всегда `Single`) |
| 2 | транспорт: UDP в LAN, транспорт в памяти | ✅ `crates/shards_net` (без Bevy): `MemoryHub` (в памяти, детерминированный: одно зерно и одни отправки — одни доставки), `UdpTransport` (LAN), имитация условий `LinkConditions`/`Conditioned` поверх любого транспорта, `Link` (ненадёжные «последнее побеждает», надёжные упорядоченные, блобы, deflate с бюджетом распаковки, подтверждения, RTT, потери, скорость, тайм-ауты); 80 юнит-тестов (`src/tests.rs`, `conditioner_tests.rs`, `hub.rs`, `udp.rs`, `wire.rs`, `track.rs`). Сессия поверх него — следующий пакет |
| 3 | ввод с номером тика | ⏳ C (задел: `Intent` — единственный вход, реплей хранит его с тиками) |
| 4 | репликация | ⏳ C |
| 5 | предсказание | ⏳ C |
| 6 | мир через манифест | ⏳ C |
| 7 | подключение и отключение | ⏳ C |
| 8 | отладка сети | ⏳ C |

Задел (C1): клиент не гоняет `SimSet` (`simulates`/`predicts`, `NetSet::Predict`, `Predicted`), правила репликации
(`net_ignore`, `rules_hash`), общий со снапшотами кодек состояния (`ai_core::state`), токены приложений одного процесса,
маршруты инструментов для клиента, агент, привязанный к игроку, отложенный `step`, `SeedLock`, хуки выхода, флаги
`--host`/`--join`/`--stand` (пока выходят с кодом 2).

## E8. Сохранения и настройки (фаза C)

| № | что сделать | статус |
|---|---|---|
| 1 | сохранения персонажа и мира, слоты | ✅ `shards_save`: два вида, слоты `[a-z0-9_-]`, `--world`/`--character` (одиночная игра грузит слот до первого тика; хост и клиент — `load_launch_slot`, C8); `tests/save/launch.rs`, `tools.rs`. Секции движка `region`, `position`, `vitals` (загрузка оживляет тело), `crates/sandbox/tests/sandbox/saves.rs` |
| 2 | схема и миграции | ✅ секции с версиями и цепочкой миграций, дыра в цепочке роняет сборку; образцы файлов версии 1 в `tests/samples/`; сохранение из более новой версии не порча: отказ без копий, `.damaged` и перезаписи, запись в такой слот тоже отказывает (`tests/save/newer.rs`) |
| 3 | атомарная запись, контрольная сумма, копии | ✅ `.tmp` + `sync_all` + `rename` + fsync папки, `shards-save/1 crc32 len`, три копии, восстановление с записью в журнале, матрица порчи (`tests/save/damage.rs`), `save_damage` для QA; реплей на проигрывании на диск не пишет (`tests/save/playback.rs`) |
| 4 | «сохранить сейчас», автосохранение | ✅ `save_now`, `autosave_on`, дебаунс из `*.save.ron`, запись в `FixedLast`, хэш состояния не меняется; при выходе ожидающие запросы пишутся сразу (`flush_saves`); эталонный реплей `tests/replays/saves.json` |
| 5 | реестр сохраняемого | ✅ `SaveRegistry`, инструмент `save_registry`, поле `save` в `ai/status` |
| 6 | настройки на фреймворке Bevy | ✅ `crates/shards_settings`: группы графики, звука, управления и доступности на `bevy::settings` (`settings.toml`), `add_settings_group` для групп игры, хранилище после `build` игры (`install`, `--settings-dir`/`--no-settings`), запись через секунду и при выходе (в том числе изменения кода игры), инструменты `settings`/`settings_set`. Файл не теряется: не TOML — копия `.bad` и последняя хорошая `.bak`, неудавшаяся запись возвращает копию и повторяется; `reset` возвращает умолчания игры. Применяют: окно (`apply_graphics`), камера и ввод (`shards_motion`: поле зрения, тряска, чувствительность, `MotionBlur`, `hold_to_toggle`, `rebind` ⇄ `ControlSettings` с теми же проверками у `settings_set`, перенос `input.ron` по просьбе игры), мерцание предупреждений (`shards_action`). Тесты: `shards_settings` (14 в `src/`, 31 в `tests/settings/`), `shards_motion/tests/motion/input.rs`, `camera.rs`, `sandbox/tests/sandbox/settings.rs`, сценарий `settings.scenario`. Не проверено на настоящем дисплее, кроме снимка под Xvfb (размер окна из настроек); громкости ждут звукового модуля (E9), `render_scale` — множитель окна, не внутреннее разрешение с апскейлом |

## E9. Интерфейс, звук, локализация (фаза D)

| № | что сделать | статус |
|---|---|---|
| 1 | виджеты | ⏳ D (в песочнице простой HUD на Bevy UI — это код песочницы, не движка) |
| 2 | стек экранов, фокус | ⏳ D (контексты ввода `gameplay`/`menu` и `PauseState` из E1/E2 есть) |
| 3 | темы и масштаб текста, подписи доступности | ⏳ D |
| 4 | локализация | ⏳ D |
| 5 | звук: шины, музыка слоями, пространственные звуки | ⏳ D (события `SoundCue` и `AnimationCue` из E3 уже выходят) |

## E10. Рендер, производительность, платформа (фаза E)

| № | что сделать | статус |
|---|---|---|
| 1 | стилизованные материалы | ⏳ E |
| 2 | растительность и инстансинг | ⏳ E |
| 3 | эффекты и декали (телеграфы на земле из E3 — гизмо, не декали) | ⏳ E |
| 4 | бюджеты и автотест производительности | 🟡 время тика (задача 11) — ✅: строка `ticks:` у `step`, запись `[perf]` в журнале, `perf` в `ai/status`, `--tick-budget-ms` (`ai_core::perf`, `crates/sandbox/tests/perf.rs`). Время кадра, счётчики сущностей и отрисовок, автотест на эталонной сцене, профайлер Tracy (у `shards` нет фичи `trace_tracy`) — E |
| 5 | Steam | ⏳ E (нельзя проверить здесь) |
| 6 | сборка под Windows | ⏳ E (нельзя проверить здесь) |
| 7 | обработчик падений в один файл для отчёта | 🟡 краш-реплей `replays/crash-tick<N>.json` есть (E1/ai_core); сведений о системе и общего файла — ⏳ E |
| 8 | скриншоты без окна | ⏳ E (`shards shot` требует окна; под Xvfb с программным Vulkan работает) |

## E11. Инструменты разработчика и слой ИИ

| № | что сделать | статус | где и чем проверено |
|---|---|---|---|
| 1 | меню разработчика и консоль через `call_tool` | 🟡 | единая точка входа `ai_core::call_tool` есть, ею пользуются тесты, реплеи и клавиши разработчика в окне песочницы (P, N, F1–F4, F12). Самого внутриигрового меню и консоли нет — D |
| 2 | оверлеи: хитбоксы, хёртбоксы, зоны атак; навсетка; состояние NPC; сеть; границы тайлов | 🟡 | `combat_overlay`, `physics_overlay`, `overlay` (id) — ✅ (`shards_action::render`, `shards_motion::overlay`); навсетка и маршруты NPC — `nav_overlay` (`shards_npc::nav::overlay`), границы тайлов, сокеты, точки и дороги — `world_overlay` (`shards_world::overlay`): ✅ глазами (`shards shot` под Xvfb), тестов на них нет. Состояние NPC — строка в `observe` и `npc_state`, отдельного оверлея нет; сеть — C |
| 3 | инструменты: бессмертие, телепорт к тайлу или точке, фаза босса, спавн префаба, замедление | ✅ | `god`, `teleport` (в точку), `goto_tile` (к тайлу; тест `goto_tile_lands_on_the_ground_of_a_tile`), `spawn_prefab`, `spawn_level`, `speed`; переход к фазе босса через снапшот (`snapshot_load`, тест `go_to_boss_phase_two_is_a_snapshot_loaded_by_name`, сценарий `snapshot`) |
| 4 | снапшоты в любой тик | ✅ | `ai_core::snapshot`: `snapshot_save`/`snapshot_load`, `shards save-snapshot`/`load-snapshot`, реплей `shards-replay/3` со снапшота; регион, его уровни-тайлы и навсетка возвращаются со снапшотом (`add_snapshot_prepare`, `RegionBuild`, `add_snapshot_rebuild("navmesh")`), загрузка, которой отказали после подготовки, возвращает мир, каким он был, номера уровней (`LevelCounter`) — состояние снапшота; `tests/sandbox/snapshots.rs` (28), `ai_core::snapshot` (20), `shards::run` (2), `shards_npc/tests/npc/snapshot.rs` (4), тесты регионов и уровней `shards_world`. Не входит: правка контента и `spawn_level` до снапшота в реплей со снапшота (свежая игра должна быть уже такой); в реплее `content_apply`, который добавляет файл уровня тайла, и `regen` в тот же тик файл не успевают загрузить |
| 5 | `clearance`, `path_to`, видимость лучами физики | 🟡 | по физике — ✅ (`src/perception/`, `physics_line_of_sight`; `tests/motion/perception.rs` (29)). По навсетке — `nav_path` (формат `path_to`, плюс тайлы маршрута; `tools::nav_path_answers_with_the_route_around_a_wall`) и `move_to path=true` (`move_to_path_follows_the_navmesh_beyond_the_local_planners_reach`); сами `path_to` и `clearance` остались запросами к физике |
| 6 | связать слой ИИ с модулями | 🟡 | модули фазы A и B — ✅ (генератор: `regen`, `region_load`, `goto_tile`, `worldgen_report`; уровни и NPC — инструменты и строки в `observe`). Инструменты сохранений (C) — ✅ (`save_game`, `load_game`, `save_check`, `save_list`, `save_registry`, `save_damage`: `crates/shards_save/src/tools.rs`, тесты `tests/save/tools.rs`). Дерево интерфейса (D), агент-игрок и стенд «хост + клиенты» (C) — впереди |
| 7 | сценарные тесты: файл сценария в CI без окна, свой набор у модуля | ✅ для A и B | `shards_script`, `ai_core::scenario`, `shards scenario`; сценарии песочницы: `obstacle_course` (E2), `combat_prototype`, `boss_fight`, `dodge`, `errors` (E3), `npc_director`, `npc_navmesh` (E6), `region` (E5), `snapshot` (E11), сценарии шаблона `smoke`, `valley`; CI-задания `test` (в процессе, вперёд и назад) и `headless-cli` (против живой игры, вперёд и назад). Сценарии остальных модулей — по мере появления |
| задача 9 | инварианты: правила, которые останавливают игру | ✅ | `ai_core::invariants`: `add_ai_invariant`, `assert_add`/`assert_list`/`assert_remove`/`assert_reset`, `shards assert`, `shards fuzz`, код выхода 3, реплей нарушения; правила песочницы `health_in_range` и `bodies_in_world`; `tests/sandbox/invariants.rs`, тесты `ai_core::invariants`, `shards_cli` (`fuzz_inputs_come_from_the_seed`), `shards_script` (оборванный шаг) |
| задача 10 | подробности расхождения реплея | ✅ | `Check.parts`, `--check-every`, `--check-detail`; `--verify` называет сущности и компоненты и печатает их значения; `tests/sandbox/replays.rs`, тесты `ai_core::replay` |
| задача 11 | время тика | ✅ | `ai_core::perf` (см. E10, п. 4); `crates/sandbox/tests/perf.rs`. Tracy игре на `shards` не включить: фичи `trace_tracy` у `shards` нет (в #4 песочница зависела от `bevy` напрямую, и работало `--features bevy/trace_tracy`); проброс — E10 |

| «Готово, когда» | статус |
|---|---|
| каждый пункт списка 32.8 GDD доступен и из меню разработчика, и из `shards` | ⏳ D |
| сценарии всех модулей идут в CI | 🟡 сценарии модулей фазы A и B идут (`test`, `headless-cli`); модулей C–E ещё нет |

## Тесты

`cargo test --workspace` на 14 октября 2026:

Всего 1109 тестов проходят, 0 падают, 23 помечены `#[ignore]` (запись эталонных реплеев, проверка окна под Xvfb, примеры в
документации); у шаблона отдельно 24 проходят (и один `#[ignore]`: запись его эталонов). Запускать нужно из обычной оболочки:
под `nohup` игнорируется SIGHUP, и тест `kill_and_hangup_remove_the_token_and_the_port_file` падает на `signal 1`.

| крейт | тестов | где |
|---|---|---|
| `ai_core` | 86 | `crates/ai_core/src/*` (снапшоты 20) |
| `shards_script` | 35 | `crates/shards_script/src/*` |
| `shards_cli` | 12 | `crates/shards_cli/src/*` |
| `shards_core` | 14 | `src/` и `tests/core.rs` |
| `shards_content` | 22 | `src/` и `tests/content.rs` |
| `shards_motion` | 175 | `src/` и `tests/motion/` (контроллер 28, перцепция 29, ввод 30, физика 11, …) |
| `shards_settings` | 57 | `src/` (16) и `tests/settings/` (41: хранилище, файл и его порча, правка файла руками в работающей игре, повтор записи и запись при выходе, группы, инструменты, изоляция от снапшота, хэша и репликации) |
| `shards_action` | 145 | `src/` и `tests/combat/` |
| `shards_level` | 80 | `src/` (3) и `tests/level/` (77: файл и узлы, свойства, коллайдеры, маркеры, спавн, `level` в контенте, реплей, картинка) |
| `shards_world` | 72 | `src/` (17) и `tests/world/` (55: генерация, проверки, подземелья, регионы, смена региона, уровни-тайлы, зоны, навсетка региона) |
| `shards_npc` | 82 | `src/` (19) и `tests/npc/` (63: поведение, восприятие, директор, навсетка, реплей, снапшот, инструменты, страж данными) |
| `shards_save` | 87 | `src/` (32) и `tests/save/` (55) |
| `shards_net` | 80 | `src/` |
| `shards` | 21 | `src/`, `tests/engine.rs`, `tests/failures.rs`, `tests/level_render.rs`, `tests/run_to.rs` (3), `tests/action_render/` |
| `sandbox` | 141 | `tests/sandbox/` (120, из них `snapshots.rs` 28, `replays.rs` 23, `invariants.rs` 12; ещё `#[ignore]` запись эталонных реплеев), `tests/system_failures.rs` (8), `tests/perf.rs` (6), `tests/log_capture.rs` (5), `src/` (2) |
| шаблон | 24 | `template/tests/game/` (содержание, детерминизм, реплеи, сценарии, регион, манифест) |

## Известные ограничения и отложенное

Что работает не так, как можно ждать, и что отложено осознанно:

- **`path_to` и `clearance` не по навсетке.** `path_to` — локальный сеточный A* (клетка 0.5 м, поиск в ограниченной области,
  не дальше 80 м по умолчанию), `clearance` — лучи физики. В загруженном регионе по навсетке идут `move_to path=true`, `nav_path`
  и NPC; вне региона они планируют по локальной сетке.
- **Навсетка рассчитана на одно тело.** Тайлы запекаются под `agent` из `nav` (по умолчанию тело скелета песочницы); ходок,
  которому движения этого тела не под силу (шире, выше, ступенька ниже, хуже перелезает или плавает), планирует по локальной
  сетке (страж песочницы так и ходит). У тел нет прыжка и лестниц: `navlink` годится для спуска, подъёма в пределах
  перелезания и шага. Вырезание (`solids` позже запекания) умеет только выключать клетки, а не добавлять ступеньки.
  Уклонение между NPC простое (упреждающий поворот, расталкивание). Навсетку нужно запекать после правки тайла
  (`--bake-nav`), иначе контент называет устаревший файл, а регион запекает тайл при загрузке.
- **Физика — линейный перебор.** Широкая фаза — проход по всем коллайдерам с предфильтром по AABB, O(n) на запрос.
  BVH встанет за тем же API, когда миры вырастут: регион 8 × 8 в песочнице (около 150 коллайдеров) играется без заметных
  задержек (реплей в 1900 тиков проходит за пару секунд), но саму широкую фазу отдельно не измеряли.
- **Уровни из Blender проверены не на настоящем экспорте.** Конвейер читает glTF 2.0, но файлы в репозитории
  (`ruin.gltf` песочницы и шаблона) сделаны тестовым генератором; Blender здесь нет. Проверки, что маркеры-сокеты файла
  совпадают с сокетами тайла в RON, нет (в `shards_content` нет связи между видами). Узлы файла тайла `content check` читает,
  только если файл назван ещё и определением `level`; иначе сломанный файл виден в журнале, когда загружается.
- **Стримится только картинка тайлов.** Геометрия региона для физики создаётся вся при загрузке региона, по расстоянию
  загружаются и выгружаются только визуальные сцены тайлов (`stream.rs`, только с окном). Кривая смещения действия
  записывается вручную, запекать её из анимации пока нечем.
- **Регион в сохранении.** Секция `region` хранит id, зерно, версию генератора и хэш манифеста; регион строится заново из зерна
  и отказывает до изменений, если контент или тайлы с тех пор изменились (нужна миграция секции или старый контент).
- **Анимация.** Один `AnimationPlayer` на персонажа, слоёв нет. Клипы общего скелета подхватывает мост, но настоящих
  моделей в репозитории нет, только сгенерированная для тестов.
- **Ввод.** Переназначение хранится в настройках игрока (E8.6); `input.ron` только переносится. Геймпад не проверялся на
  настоящем устройстве. Символы телеграфов шире ASCII (`⚠`) рисуются штрихами или нужным шрифтом
  (`ThreatHud::font`); без шрифта подставляется ASCII.
- **Зоны атак — плоские.** `hit_zones` и подсказка «выйди на N м» считают след зоны на земле (шар — как круг, конус —
  как сектор). Для «выйти из зоны» этого хватает; прыжок над низкой линией учитывает только сам удар.
- **Ошибка в файле контента роняет все его определения.** Файл, который не разобрался, не даёт в базу ни одного
  определения, и следом идут ошибки «unknown action …» из файлов, которые на них ссылаются. Причина — первая строка
  списка. Прежняя версия остаётся в силе.
- **Реплеи привязаны к сборке и контенту.** Правка числа в контенте или геймплея расходит реплей на ближайшей проверке
  (`DIVERGED at tick 60`); эталоны записывают заново (`record_reference_replays`). Реплей может начинаться со снапшота, но то, что до снапшота
  изменило «то, вокруг чего построен мир» (правка контента, `spawn_level`), в него не входит; регион в снапшоте входит.
  Известная щель: при проигрывании реплея `content_apply`, добавляющий файл уровня тайла, и `regen` в тот же тик не успевают
  загрузить файл. Снапшот и реплей другой минорной версии движка не принимаются (`force` пропускает проверку): после поднятия
  версии `0.3` эталон `tests/replays/snapshot.json` записывают заново.
- **Скриншоты требуют окна** (под Xvfb хватает программного Vulkan). Рендер в картинку без окна — E10.
- **Задание CI `replays-window` необязательное** (`continue-on-error`) до тех пор, пока не докажет стабильность на
  раннерах. Остальные четыре задания обязательные.
- **Теги не поставлены.** `v0.1.0` (коммит `e90193d`), `v0.2.0` и `v0.3.0` ставит мейнтейнер; версия в `Cargo.toml` остаётся
  `0.2.0`, пока он не выпустит `0.3.0` (записи фазы B уже лежат в `CHANGELOG.md` под `0.3.0`). Пока тегов нет, игра закрепляет
  движок коммитом (`rev = "…"`).
- **Шаблон ещё без сохранения и кооператива** (C), без HUD и меню (D). Долина, рейдеры на навсетке и уровень из файла в нём
  есть.
- **Относительный `BEVY_ASSET_ROOT` не принимает наблюдатель за файлами.** `BEVY_ASSET_ROOT=crates/sandbox target/debug/sandbox
  --headless` падает в `AssetPlugin` с `Failed to create file watcher from path "crates/sandbox/assets"`; абсолютный путь
  (`$PWD/crates/sandbox`) работает, как и `cargo sandbox`.
