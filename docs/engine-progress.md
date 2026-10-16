# Ход работ по ТЗ

Сверка с [engine-tz.md](engine-tz.md): что из «Что сделать» и «Готово, когда» каждого модуля E1–E11 уже есть,
где это лежит и чем проверено. Состояние на 15 октября 2026: фазы A, B, C и D собраны: в фазе D интерфейс (D1),
локализация (D2), звук (D3) и остатки E4 и E11 (D4: меню и консоль разработчика, оверлей NPC, `path_to` и `clearance` по
навсетке, проверка сокетов файла уровня; см. «Фазы»). Впереди фаза E. CI на GitHub
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
| B | мир | E5; E6; из E4 — уровни из Blender; из E11 — `clearance`/`path_to` по навсетке, снапшоты | 🟡 **собрана, проверена** локально и в CI. «Готово, когда» E5, E6, уровней из Blender и снапшотов выполнены и проверены тестами, сценариями и реплеями (таблица ниже). `path_to` и `clearance` по навсетке доделаны в D4. Не сделано до конца: стриминг геометрии тайлов для физики (стримится только картинка), навсетка под несколько тел. Тег `v0.3.0` не поставлен |
| C | кооператив (этап 3) | E7; E8; из E11 — стенд «хост + клиенты» | ✅ **собрана и проверена** локально и в CI: задел (клиенты без `SimSet`, правила репликации, кодек состояния, отложенный `step`, хуки выхода, флаги запуска), канальный слой сети `shards_net` (E7.2), сессия кооператива и стенд «хост + клиенты» (E7.1, 3, 4, 6, 7; E11.6), сохранения E8.1–5 (`shards_save`), настройки игрока E8.6 (`shards_settings`), сохранения в кооперативе (`shards::coop`: персонаж едет с игроком, мир хранит хост), предсказание своего тела, сглаживание чужих и оверлей сети (E7.4, 5, 8; C4), CLI сети и сценарии стенда (C7), сохранения и кооператив на двоих в шаблоне, долгая игра `shards soak` (C9). Приёмка E7 — полчаса хоста и двух клиентов на 150 мс и 2 % без рассинхрона (таблица ниже). Тег не поставлен |
| D | контент и интерфейс | E9; остаток E4 и E11 | ✅ **собрана**, проверена локально: интерфейс E9.1–3 (`shards_ui`, D1) — экраны и меню из контента, дерево интерфейса для агента, рисование с подписями доступности, HUD и меню в песочнице и шаблоне; локализация E9.4 (`shards_locale`, D2) — тексты на Fluent, язык на лету; звук E9.5 (`shards_audio`, D3) — шины, голоса, пространственные звуки, музыка в слоях по игре; остаток E4 и E11 (D4) — меню и консоль разработчика (`shards_dev`, E11.1; каждый пункт списка 32.8 GDD и в меню, и в `shards`), оверлей NPC (`npc_overlay`, F7), `path_to` и `clearance` по навсетке (E11.5), сверка маркеров-сокетов файла уровня с сокетами тайла (E4.4). Тег не поставлен |
| E | платформа | E10 | 🟡 начата: из E10 есть стилизованные материалы (E10.1, `shards_look`: виды мешей из контента — свет полосами, плоские грани, контуры), растительность с уровнями детализации (E10.2, `shards_scatter`), эффекты и декали (E10.3, `shards_fx`: частицы из пулов по контенту, телеграфы атак декалями на земле), бюджеты (время тика: `step`, `[perf]`, `--tick-budget-ms`; время кадра и счётчики в `status`, автотест `--perf` — E4 фазы), сборка под Windows (E1 фазы: `shards package`, `--smoke`, `quit`; проверена на Windows 11, на чистой машине — эмуляцией окружения), отчёт о падении (E2 фазы: `crashes/…json`, `crash_report`) и скриншоты без окна (E3 фазы: `--offscreen`); остальное не начато |

Что проверено для приёмки фазы A (все команды запускались из корня репозитория, без окна, если не сказано иное):

| что | как проверено |
|---|---|
| сборка и тесты движка | `cargo test --workspace` (0 падают; число тестов по крейтам — в разделе «Тесты»); `cargo clippy --workspace --all-targets --locked -- -D warnings` и то же для шаблона без замечаний; `template/Cargo.lock` не расходится с корневым |
| шаблон отдельно от движка | `cargo test --manifest-path template/Cargo.toml` (число тестов — в разделе «Тесты»); копия `template/` вне репозитория, переименованная, с зависимостью-`path` на `crates/shards`: `cargo test`, запуск без окна, `shards scenario`, перезапись реплея (отдельный репозиторий с тегом так не проверить: тега ещё нет) |
| сценарии против живой игры | `cargo sandbox --headless --paused --port 0 --port-file F`, `SHARDS_PORT_FILE=F cargo shards scenario crates/sandbox/scenarios/*.scenario` → `5 of 5 scenarios passed` |
| контент | `cargo sandbox --check-content` → `ok: 38 definitions in 9 files`; сломанная копия контента даёт ошибки с файлом, строкой и подсказкой (README, «Контент») |
| эталонные реплеи без окна | `cargo sandbox --replay tests/replays/combat_prototype.json --verify` → `identical to the recording (39 checks)`, то же для `obstacle_course.json` (17 проверок) и для реплея шаблона (5 проверок) |
| те же реплеи в окне | `xvfb-run -a cargo sandbox --replay tests/replays/obstacle_course.json --verify --windowed` → `identical to the recording (17 checks) [1010 ticks …, 21 frames]` (программный Vulkan, lavapipe); так же `combat_prototype.json` (39) и реплей шаблона |
| скриншот с метками | `xvfb-run -a cargo sandbox --paused …` и `shards shot`: PNG и легенда |

Что проверено для приёмки фазы B (команды запускались из корня репозитория без окна, если не сказано иное; `cargo test --workspace`:
все проходят, 0 падают; шаблон: все проходят):

| что | как проверено |
|---|---|
| шаблон генерирует регион 8×8 из тестовых тайлов | `template/`: долина `valley` (4 тайла: луг, колонны, ворота и руина из файла glTF), тесты `tests/game/region.rs`; `cargo run -- --worldgen-report valley --seeds 1..1000` → `1000/1000 passed (63 after a retry)` |
| 1000 зёрен проходят проверку | `cargo sandbox --worldgen-report wilds --seeds 1..1000` → `1000/1000 passed (42 after a retry), 0 fell back, 0 failed, 0 panicked [1000 seeds in 0.31s]`; тесты `a_thousand_seeds_pass_their_checks_quickly` (`shards_world`), `a_thousand_seeds_of_the_valley_pass_its_checks` (шаблон); подземелье `crypt` — те же 1000 зёрен за 0.17 с; CI-задания `headless-cli` и `template` гоняют отчёт |
| одно зерно и версия — побайтно один манифест | тесты `an_8x8_region_generates_passes_its_checks_and_is_byte_stable`, `the_same_seed_builds_the_same_world_down_to_ids`, `the_same_seed_and_version_give_the_same_manifest_byte_for_byte` (шаблон); сценарий `region` сверяет хэш `7bbfdc3b62623fbc` дебрей с зерном 42 и повторяет его после другого региона |
| переход между регионами с экраном загрузки без окна | тесты `regen_replaces_the_region_through_the_loading_screen_and_keeps_the_hero`, `a_region_changed_from_on_enter_in_game_does_not_enter_the_game_again`, `going_to_the_valley_passes_the_loading_screen_headless_and_keeps_the_hero` (шаблон); сценарий и реплей `region` (игрок тот же, тик не сдвинулся) |
| враг обходит колонны по навсетке и не выходит за зону | сценарий `npc_navmesh` (`npc_detour` — обход колоннады, `npc_beyond_leash` — ни разу за поводок), реплей `npc_navmesh.json`; тесты `an_npc_walks_around_pillars_by_navmesh_and_stays_on_its_leash`, `a_sentry_chases_to_its_leash_goes_home_healing_and_never_crosses`; рейдер шаблона: `a_raider_comes_by_the_navmesh_and_keeps_to_its_camp` |
| при одном игроке из трёх врагов атакуют не больше двух | сценарий `npc_director` (30 с боя, `three_attacking` ни разу), тест `one_player_three_enemies_never_more_than_two_attack_at_once`, реплей `npc_director.json` |
| всё совпадает в реплее | `cargo sandbox --replay tests/replays/<файл>.json --verify` для всех шести (`obstacle_course`, `combat_prototype`, `npc_director`, `npc_navmesh`, `region`, `snapshot`): `identical to the recording`; то же в окне под Xvfb (`--verify --windowed`); тест `every_reference_replay_verifies_headless`; реплеи шаблона `smoke` и `valley` |
| шаблон собирает уровень из Blender-файла с маркерами | `template/assets/tiles/ruin.gltf` + `ruin.level.ron` (обязательные маркеры); тесты `a_typo_in_the_level_is_an_error_that_names_the_fix`, колонна руины на месте, где поставил тайл. Файл сделан тестовым генератором glTF (`crates/sandbox/tests/sandbox/tiles.rs`), настоящий экспорт из Blender здесь не пробовали |
| реплей начинается со снапшота, фаза босса — загрузка снапшота | сценарий `snapshot` и реплей `snapshot.json`; тесты `go_to_boss_phase_two_is_a_snapshot_loaded_by_name`, `a_snapshot_in_the_generated_region_goes_on_the_same_in_a_fresh_arena`; `ai_core::snapshot`, `shards::run` (код 2 для снапшота, который сборка не принимает) |
| сценарии модулей идут в CI | `test` (`every_scenario_file_passes_on_a_fresh_game`, `…after_another`, `…backwards`) и `headless-cli` (вперёд, потом назад в одной живой игре) |
| `content check` читает уровни и навигацию | `cargo sandbox --check-content` → `ok: 70 definitions in 23 files`; сломанные копии: опечатка компонента в узле, пропавший маркер, устаревший файл навсетки, опечатка сокета и точки — ошибки с файлом, узлом и подсказкой (README) |
| `clearance`/`path_to` по навсетке | ✅ (D4) `path_to` спрашивает того же планировщика маршрутов (`RoutePlanner`), что `move_to path=true`: в регионе отвечает навсетка (`"provider": "navmesh"`), вне его — локальная сетка (`"local grid"`); `clearance` закрывает направление, где навсетка кончается раньше физики больше чем на метр (`E closed 4.0m (the navmesh goes no further)`). Тесты `navmesh.rs` (сшивка с запечатанным краем, `move_to_path_follows_the_navmesh_beyond_the_local_planners_reach`), `perception.rs` (`local grid` вне региона), сценарий `npc_navmesh` |

Что проверено для приёмки фазы C (Windows, отладочная сборка; `cargo test --workspace` и тесты шаблона — в разделе «Тесты»):

| что | как проверено |
|---|---|
| хост и два клиента без окна полчаса играют скриптами агентов при 150 мс и 2 % потерь без рассинхрона | `cargo sandbox --stand 2 --headless --paused --port 0 --port-file F --net-sim latency=150,loss=0.02`, `SHARDS_PORT_FILE=F cargo shards soak --seed 30 --minutes 30 --regen wilds --regen-minutes 5 --save-minutes 3` → `PASS soak seed 30: ticks 0 → 108000 (30:00 of game time), 7005 rounds, 5 region changes, 9 saves`, рассинхронов 0 у хоста и обоих клиентов (6 минут). То же клиентами-процессами по настоящему UDP (`--stand 2 --stand-udp`, зерно 31): `PASS`, 0 рассинхронов; шаги `soak` гнали хоста в 2,7 раза быстрее реального времени, поэтому ввод клиентов часто опаздывал (около 5300 входов на клиента), а предсказание часто сбрасывалось (средняя ошибка 1–1,5 м). В процессе: `half_an_hour_of_three_players_on_lossy_links_stays_in_sync` (`#[ignore]`, 168 с), каждый пуш — `two_minutes_of_three_players_on_lossy_links_stay_in_sync` и три минуты на живом стенде в CI (`headless-cli`), полчаса — `soak.yml` раз в неделю |
| подключение и отключение не теряют прогресс | `a_player_who_leaves_has_the_latest_in_their_file_and_comes_back_to_it`, `a_rejoin_within_the_grace_keeps_the_body_and_sends_the_file_what_it_missed`, `a_rejoin_after_the_grace_restores_the_character_the_host_kept`, `world_progress_a_guest_made_is_in_the_hosts_world_save_after_the_guest_left` (`net_coop.rs`); шаблон: `a_guest_brings_their_trophies_to_the_next_session` |
| сетевые сценарии на транспорте в памяти идут в CI | `every_stand_scenario_file_passes_on_a_fresh_stand`, `a_stand_scenario_run_twice_gives_identical_journals` (`scenario_files.rs`), `every_stand_scenario_passes` (шаблон); живой стенд в `headless-cli`: `shards scenario crates/sandbox/scenarios/stand/*.scenario` |
| «сохранить → загрузить» на 1000 случайных состояниях не теряет данных | `a_thousand_random_states_round_trip_through_files` (`shards_save/tests/save/roundtrip.rs`) |
| повреждённый файл восстанавливается из копии | `every_kind_of_damage_to_the_primary_is_restored_from_the_first_backup`, `the_next_backup_is_tried_when_the_first_is_bad_too` (`damage.rs`) |
| файл старой версии мигрирует и загружается | `old_files_migrate_and_load` на закоммиченных образцах версии 1 (`migrate.rs`) |
| шаблон: сохранение и кооп на двоих | `scenarios/saves.scenario`, `scenarios/stand/coop.scenario`, `tests/game/coop.rs` |

## Сводка

| модуль | фаза | состояние |
|---|---|---|
| E1. Ядро, сборка, шаблон | A | ✅ (сохранения и кооператив на двоих в шаблоне — C9) |
| E2. Движение | A | ✅ физика на parry3d, контроллер, ввод, камера, оценка пути; навсетка — E6 |
| E3. Действия, бой, анимация | A | ✅ бой на данных, анимационный мост, телеграфы |
| E4. Контент и уровни | A (часть), B, D | 🟡 база, префабы, `content check` — ✅; уровни из Blender — 🟡 (формат и пайплайн ✅, настоящий экспорт Blender не пробовали) |
| E5. Генерация мира | B | 🟡 генератор, решатель, проверки, манифест, подземелья, отчёты, смена региона, активные зоны — ✅; стримится только картинка тайлов |
| E6. Навигация и ИИ врагов | B | ✅ навсетка по тайлам, поведение, восприятие, директор, планировщик; 🟡 уклонение простое, навсетка под одно тело |
| E7. Сеть и кооператив | C | ✅ транспорт, сессия, ввод, репликация, подключение и отключение, предсказание и сглаживание, оверлей сети (`shards_net`), CLI сети и сценарии стенда, долгая игра `shards soak`; приёмка — полчаса трёх игроков на 150 мс и 2 % без рассинхрона |
| E8. Сохранения и настройки | C | ✅ сохранения (1–5, `shards_save`), настройки игрока (6, `shards_settings`) и сохранения в кооперативе (7, `shards::coop`) |
| E9. Интерфейс, звук, локализация | D | ✅ интерфейс, локализация и звук |
| E10. Рендер, производительность, платформа | E | 🟡 стилизованные материалы (`shards_look`: виды, свет полосами, плоские грани, контуры) — ✅; растительность и инстансинг (`shards_scatter`) — ✅; эффекты и декали (`shards_fx`: частицы из пулов, декали телеграфов) — ✅; бюджеты и автотест (время тика и кадра, счётчики, `--perf`) — ✅; сборка под Windows — 🟡 (всё, кроме запуска на настоящей чистой машине и проверки одинаковых реплеев Windows ↔ Linux в CI); отчёт о падении — ✅; скриншоты без окна — ✅; остальное — ⏳ |
| E11. Инструменты разработчика | A (часть), B, C, D | 🟡 сценарии, оверлеи (бой, коллайдеры, навсетка, тайлы, NPC, сеть), меню и консоль разработчика, `god`, `spawn_prefab`, `goto_tile`, снапшоты, `clearance` и `path_to` по физике и навсетке — ✅; агент-игрок и сценарии модулей фазы E — впереди |

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
| 5 | шаблон в `template/`: персонаж, враг, арена, сохранение, кооп на двоих | ✅ | `template/`: персонаж, враг, арена, свой вид контента, инструмент и условие, сценарии, реплей, 28 тестов (и один `#[ignore]`: запись эталонных реплеев). Сохранения (C9): свои разделы `trophies` (персонаж) и `fallen` (мир) в `src/progress.rs`, автосохранение очищенного двора, `scenarios/saves.scenario`. Кооп на двоих (C9): `content/session.net.ron`, `scenarios/stand/coop.scenario`, `tests/game/coop.rs` (гость на 150 мс и 2 %: трофей его на каждой копии и в его файле, следующая игра с ним, реплей хоста) |
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
`path_to` по физике — ✅, `path_to` — локальный сеточный A* (0.5 м, `src/perception/path.rs`); в загруженном регионе `move_to path=true` и `path_to` спрашивают навсетку (E6) через `RoutePlanner` (`provider` в ответе), а `clearance` закрывает направление, где она кончается (`closed`; D4).

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
| 8 | телеграфы: зоны на земле, звуковые метки, символы | ✅ | `src/zone.rs`, `src/draw.rs`, `src/render/overlay.rs`, `src/cue.rs` (`SoundCue` играет модуль звука E9.5: звук телеграфа там, куда придётся удар) |

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
| 4 | `shards content check` | ✅ | `shards content check` (живая игра) и `--check-content` (без игры, код 0/1), `check_dir`. Читает файлы уровней (`level`: узлы, компоненты, префабы, обязательные маркеры), наличие файлов уровней тайлов, запечённую навигацию. Маркеры-сокеты файла уровня сверяются с сокетами тайла в RON (D4: `add_content_check` — проверка, которую модуль добавляет к виду; ставит её `TileLevelsPlugin`; тест `socket_markers_of_a_tiles_file_stand_where_the_tile_has_their_sockets`). Узлы файла тайла читаются, только если файл назван ещё и определением `level` |
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
| 7 | стриминг; ИИ врагов только в активных зонах | 🟡 | Регион грузится целиком за экраном загрузки (`RegionLoading`), ИИ врагов — только в активных зонах (`ActiveZone`/`Dormant`, `zones.rs`; тест `npcs_far_from_every_player_go_dormant_and_wake_when_one_comes`). **Стримится только картинка тайлов** по расстоянию (`stream.rs`, `visual_lods`, только когда игра рисует: окно или `--offscreen`; тест `tile_visuals_come_and_go_with_the_players`): геометрия тайлов для физики создаётся вся сразу, подгрузки по расстоянию нет |

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
| одинаковые зерно и версия дают побайтно одинаковый манифест | ✅ | `the_same_seed_and_version_give_the_same_manifest_byte_for_byte` (шаблон), сценарий `region` (хэш `7bbfdc3b62623fbc`) |
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
тестов на него нет, но `shards shot` под Xvfb показывает его вместе с `world_overlay`. Оверлей NPC (`npc_overlay`, F7; D4) — ✅:
строка на NPC рядом с игроками (состояние, цель, жетон, поводок, что видит и слышит, маршрут) и гизмо в окне; тест
`npc_overlay_answers_a_line_per_npc_near_the_players`, сценарий `npc_navmesh`, окно проверено глазами.

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
| 1 | сессия: одиночная, хост, клиент | ✅ `NetPlugin`: `--host`, `--join`, `--stand N`; `NetState`, `net_status`; в одиночной игре модуль ничего не делает (эталонные реплеи прежние). Тесты `crates/sandbox/tests/sandbox/net.rs` (два из них — замеры под `--ignored`), `net_lifecycle.rs` |
| 2 | транспорт: UDP в LAN, транспорт в памяти | ✅ `crates/shards_net` (без Bevy): `MemoryHub` (в памяти, детерминированный: одно зерно и одни отправки — одни доставки), `UdpTransport` (LAN), имитация условий `LinkConditions`/`Conditioned` поверх любого транспорта, `Link` (ненадёжные «последнее побеждает», надёжные упорядоченные, блобы, deflate с бюджетом распаковки, подтверждения, RTT, потери, скорость, тайм-ауты); 81 юнит-тест (`src/tests.rs`, `conditioner_tests.rs`, `hub.rs`, `udp.rs`, `wire.rs`, `track.rs`). Сессия поверх него (на Bevy) — пункты 1, 3, 4, 6–8 |
| 3 | ввод с номером тика | ✅ кадры `Intent` с тиком и избытком, хост применяет их на своём тике к телу игрока (и после респауна), пределы кадра и пакета (и пакета в байтах до разбора), эпохи при мире заново; пока тики хоста идут подряд (`net_resync`, рассинхрон, смена региона), ввод в пути переживает `Rebase`, часы клиента не идут назад, а удерживаемое, что хост сбросил, уходит снова; снапшот, который двигает тик, начинает ввод заново, даже если его мир ждёт места в канале; реплей хоста пишет всех. `a_dodge_on_a_client_is_an_input_of_the_host_at_its_tick`, `a_dodge_over_a_lossy_link_lands_on_its_stamped_tick`, `net_sync.rs`, `net_guards.rs` |
| 4 | репликация | ✅ дельты по правилам репликации в зоне интереса и по ссылкам, хэши каждые `desync_every` тиков, рассинхрон — журнал, `net_desync` (у клиента и хоста) и мир заново не чаще раза в 5 с (счёт начинается заново со снапшотом, который двигает тик); вид, ссылки которого упёрлись в предел 64, не сверяется, пока это так (`every_entity_in_view_is_sent_when_its_references_reach_the_cap`); значения без обратного чтения (NaN) остаются на хосте. Чужие сущности в окне клиента показаны между двумя состояниями хоста (`interp`, буфер с юнит-тестами; новая сущность сразу на своём месте); их игровой `Transform` — новейшее состояние хоста (`others_are_where_the_hosts_newest_state_has_them`). В окне не проверено |
| 5 | предсказание | ✅ C4: своё тело клиента идёт на тике нажатия (`NetSet::PredictInput` → `Predict` → `PredictEnd`, те же системы приказов, движения и действий с `With<Predicted>`), действия начинаются сразу и подтверждаются хостом, урон — только хоста; поправка по тикам (`correction_rate`), рывок при большой ошибке, расходящемся режиме движения, смерти и первом сравнении после мира заново или прыжка часов, после рывка тело не трогают, пока взятое состояние не догонит метки хоста; пройденные точки пути остаются пройденными, законченный хостом приказ приходит с его `aim`/`target`. `crates/sandbox/tests/sandbox/net_predict.rs` (14 тестов, 150 мс и 2 % потерь: бег, столб, путь из точек, ход к цели атаки — ошибка 0 м), юнит-тесты `shards_net::predict`, `inputs`. Ревью C4 (9 находок) закрыто, новые тесты проверены на коде без правки |
| 6 | мир через манифест | ✅ мир приходит снапшотом с манифестом региона, клиент строит регион сам; смена региона и снапшот на хосте отправляют мир заново и сверяют игроков с новым миром, игрок, которого хост добавил сам, остаётся, какой бы номер ни держал до него гость: тела пиров помечены `NetSeat`, тела гостей снапшота из другого процесса уходят (`a_region_change_on_the_host_sends_every_client_the_new_world`, `a_snapshot_loaded_on_the_host_goes_to_its_clients`, `a_body_the_host_joined_itself_stays_through_a_rebuilt_world`, `a_number_a_guest_left_is_the_hosts_own_once_it_joins_a_player_there`, `guest_bodies_of_a_snapshot_loaded_into_a_new_host_leave_and_free_their_numbers`, `net_sync.rs`) |
| 7 | подключение и отключение | ✅ хук `join_policy` («не во время босса»), отказ с причиной при другой сборке и полной сессии, выход и тайм-аут — тело ждёт `rejoin_grace_secs`, возвращение по токену хоста (хранится в `<save-dir>/net/`, проверено на UDP: `a_client_joins_a_host_over_udp_loopback`); вторая игра с тем же токеном занимает слот (и держит его, пока ждёт `join_policy`: `a_slot_a_second_game_took_stays_its_own_while_it_waits`), первая получает `Bye` с причиной, персонаж уходит в то соединение, которое вышло, даже когда оно не первое у хоста (`a_second_game_with_a_players_token_takes_the_slot_and_the_first_is_told`); выход хоста и `net_end` завершают сессию один раз с персонажами клиентам, игроки уходят, ждущим — отказ (`net_end_lets_the_players_go_and_refuses_whoever_waits`); закончившийся клиент замолкает, хост не заводит пира по одним подтверждениям; `--exit-with-host`: 0 — конец от хоста, 1 — отказ, кик, потеря хоста, 2 — сессию не открыть; по UDP `pause`/`resume`/`speed` — у хоста. Перед концом сессии хост сохраняет мир и своего персонажа, персонажи клиентов уходят с `SessionEnd` один раз (E8, `shards::coop`) |
| 8 | отладка сети | ✅ строки `net:` в журнале (с ограничением частоты для того, что вызывает пир), `net_sim` (задержка и потери на ходу), `net_status`, `net_kick`, `net_resync`, `net_end`, условия `net_joined`, `net_left`, `net_disconnected`, `net_desync`; оверлей сети `net_overlay` (F6: каналы, рассинхроны, ошибка предсказания, рывки) и поле `prediction` в `net_status` (C4), у клиента ещё `desyncs` и `last_desync` (C9). CLI сети (C7): `shards --peer c1`, `wait --peer all`, `shards stand` и `stand stop`, сценарии стенда (`stand clients=2 …`, `@c1`). Долгая игра `shards soak` (C9) |

Задел (C1): клиент не гоняет `SimSet` (`simulates`/`predicts`, `NetSet::Predict`, `Predicted`), правила репликации
(`net_ignore`, `rules_hash`), общий со снапшотами кодек состояния (`ai_core::state`), токены приложений одного процесса,
маршруты инструментов для клиента, агент, привязанный к игроку, отложенный `step`, `SeedLock`, хуки выхода, флаги
`--host`/`--join`/`--stand`.

Сессия (C3) и правки по ревью: гость действует только своим телом (хост выполняет лишь пересылаемые инструменты, с
пределом вызовов и агентов), всё, что присылает пир, ограничено и попадает в журнал обрезанным, токен возвращения
выдаёт хост (вторую игру с тем же токеном хост пускает, а первую отпускает с причиной), игрок, которого хост добавил
сам, переживает пересборку мира, время клиента по UDP — у хоста (решает транспорт, а не флаг тестов). Правки второго
ревью (C3 fix review: часы клиента после мира заново и ввод мира, который после снапшота ждёт места в канале, удерживаемое, сброшенное хостом, счёт рассинхрона после снапшота,
метка `NetSeat`, неполный вид без сверки, размеры до разбора, текст `Bye` и ошибок разбора, ожидание места в канале по
байтам, строка в секунду о неотправленном, слот второй игры, `net_end` и тишина после конца, `speed` на стенде, коды
выхода, хуки выхода стенда, настройки хоста стенда) закрыты тестами в `crates/sandbox/tests/sandbox/net_*.rs`, юнит-тестами
`shards_net` и `shards` (`run.rs`); каждый из них проверен на коде без правки: падает или (для новых функций) не
собирается. Не сделано по этому ревью: предел частоты `Hello`, `Ready` и `Bye` подряд (раздел ниже).

Ревью CodeRabbit по PR C3: часы клиента по UDP не идут назад (далеко впереди хоста клиент идёт вдвое медленнее, далеко
позади прыгает только вперёд; юнит-тесты `client::tests`), события `net_event` старого мира не показываются после мира заново
(`events_of_the_world_a_snapshot_replaced_are_not_shown_and_do_not_hold_back_the_new_ones`), `step` и `resume` одного участника
в одном кадре идут по порядку вызовов (`a_step_and_a_resume_asked_in_one_frame_run_in_the_order_they_were_asked`),
`snapshot_load` с неточным восстановлением возвращает мир (`a_load_that_would_not_be_exact_is_refused_and_puts_the_world_back`,
`a_load_that_cannot_restore_exactly_fails_and_changes_nothing`).
Третье ревью C3 (раунд 3): скачок тика хоста замечается до приёма игроков кадра
(`a_client_admitted_in_the_frame_the_hosts_ticks_jump_keeps_its_inputs_through_a_resync`).
Вид, с которого сняли предел ссылок, отправляет клиенту мир один раз, а не ждёт рассинхрона
(`a_view_that_stops_being_capped_gives_the_client_the_references_it_read_as_nothing`).
Мир «view complete» хранит память вида и идёт не чаще раза в 5 с (`a_view_that_stops_being_capped_through_the_hysteresis_gives_the_world_once_not_every_few_ticks`,
`a_cap_that_comes_and_goes_costs_a_world_per_cooldown_and_not_one_per_flip`).
Дельты не идут игроку, пока ему не отправлен перестроенный мир, пока тот ждёт места в канале
(`a_region_a_guest_changed_while_its_link_was_full_reaches_it_whole_not_as_deltas_first`).
Ушедший пир, который подтверждает, ждёт свой `Bye` за мегабайтами в очереди
(`a_player_sent_away_is_waited_for_while_it_takes_in_what_is_queued_ahead_of_its_goodbye`; глухой уходит, как раньше).
Чужак: первое сообщение не `Hello` — отказ (то, что не разбирается, — отказ с обеими причинами), общий бюджет разбора (опрос идёт по порядку
`PeerId` и начинается с пропущенного; номера игроков, чьи `Hello` пришли в один тик, следуют порядку `PeerId`: в стенде это порядок создания, по UDP —
хэш адреса, `players_that_say_hello_in_one_tick_take_their_numbers_in_the_order_of_their_ids`), предел числа пиров, не входящих в игру
(новый не вытесняет непрочитанного, берегущихся не больше `max_players`: `a_hello_is_not_lost_to_the_strangers_that_arrive_with_it`,
`a_burst_of_strangers_in_one_tick_never_fills_more_than_the_cap_and_spares_the_first_few`,
`a_hello_that_comes_in_pieces_is_held_for_a_while_and_then_makes_room_as_any_other`,
`a_message_ahead_of_a_gap_pins_no_seat_and_a_hello_still_gets_in`,
`strangers_holding_part_of_a_hello_pin_no_more_seats_than_a_session_has_players`,
`a_hello_in_pieces_is_spared_before_messages_that_can_never_be_a_hello`; ушедшие допущенные вытесняются в последнюю очередь, по времени
ухода: `a_table_full_of_players_that_were_put_out_and_still_owe_acknowledgements_makes_room_for_a_hello`,
`a_guest_put_out_between_ticks_is_the_last_a_full_table_forgets_not_the_first`),
бюджет `JOINING_POLL_BYTES` проверен ненадёжным чужаком
(`a_stranger_whose_first_message_is_not_a_hello_is_refused_at_once`, `peers_that_do_not_play_yet_share_a_budget_and_each_takes_a_turn`,
`the_host_holds_so_many_strangers_and_the_oldest_makes_room`, `a_stranger_that_is_not_refused_is_read_for_so_many_bytes_a_poll_and_no_more`).
Не сделано: проверка `calls_queued` до разбора `Call` у допущенного гостя; меньший `max_undelivered_bytes` канала до входа (канал
раздувает сообщения до бюджета опроса).
Мир, перестроенный вызовом гостя, доходит до клиентов (`a_region_change_a_guest_forwards_reaches_every_client_as_a_world_again`).
Перестройка мира сбрасывает признак урезанного вида и срок мира `view complete`
(`a_world_replaced_in_a_capped_view_is_not_followed_by_a_world_for_the_cap_that_is_gone`), как и любой мир целиком, кроме `view complete`
(`a_world_sent_again_to_a_player_in_a_capped_view_is_not_followed_by_a_world_for_the_cap_that_is_gone`); счётчик `Link::reliable_acked` считает и куски
блоба (юнит-тест `a_blob_that_takes_long_to_cross_keeps_raising_what_the_peer_has_acknowledged`).
Работа пира за опрос ограничена, отпущенный пир не читается, вход со сроком, `net_kick peer=`
(`a_guest_makes_the_host_decode_no_more_than_a_budget_in_one_poll`, `what_a_stranger_sends_after_its_refusal_is_not_read`,
`a_peer_that_never_says_hello_is_dropped_however_it_keeps_sending`, `a_peer_that_never_acknowledges_its_refusal_is_forgotten_all_the_same`,
`net_kick_sends_away_a_peer_that_has_no_player_yet`, `a_player_the_game_keeps_waiting_is_not_dropped_for_joining_slowly_and_a_kick_reaches_it`).
Мир больше того, что берёт канал, отказывается один раз (`a_world_over_what_a_link_carries_sends_its_player_away_once_instead_of_waiting_every_tick`,
`a_world_no_link_carries_is_tried_not_waited_for`).
Отказанная загрузка снапшота не вытесняет старые строки журнала (`a_refused_load_does_not_push_the_oldest_journal_lines_out`).
Клиент, ждущий состояния хоста, просыпается от пришедшей дельты (`a_state_that_came_wakes_a_client_that_waits_for_news`,
`a_state_that_came_makes_a_stands_client_run_again`).
Кик и конец сессии, которые не принял полный канал, называются в журнале хоста
(`a_kick_a_full_link_will_not_carry_is_said_in_the_journal`, `the_end_of_a_session_a_full_link_will_not_carry_is_said_in_the_journal`).
Тесты на описанное, но не проверенное: мир ждёт канал, полный по байтам, и не снимается каждый тик
(`a_world_waits_for_a_link_that_is_full_by_bytes_without_being_captured_again`: проверка снапшота считает снятия), коды выхода
1 и 2 (`a_game_the_host_refuses_exits_with_1`, `a_game_that_loses_its_host_exits_with_1`,
`a_game_that_cannot_take_the_hosts_world_exits_with_1`, `a_host_whose_port_is_taken_exits_with_2`), кик ждущего игрока
(`a_player_the_game_keeps_waiting_is_not_dropped_for_joining_slowly_and_a_kick_reaches_it`). Каждый проверен откатом кода.
Тест `a_world_of_an_earlier_tick_that_waits_for_room_still_starts_the_inputs_over` до третьего ревью проходил и на коде без
правки хоста (он смотрел только на часы клиента) и не видел ввода: теперь у хоста есть применённый кадр, и после мира,
который ждал места, запись ввода (`applied_input_tick`, `inputs.newest`) начата заново, а новый кадр ложится один раз на своём
тике. Проверено двумя откатами: хост считает `carry` по-старому (падает на часах клиента) и хост не сбрасывает запись
(падает на записи ввода). Оговорка «каждый тест падал на коде без правки» (абзац выше) верна для остальных тестов того ревью.

## E8. Сохранения и настройки (фаза C)

| № | что сделать | статус |
|---|---|---|
| 1 | сохранения персонажа и мира, слоты | ✅ `shards_save`: два вида, слоты `[a-z0-9_-]`, `--world`/`--character` (одиночная игра и хост грузят слот до первого тика; клиент слот мира игнорирует, а персонажа приносит в `Hello`); `tests/save/launch.rs`, `tools.rs`. Секции движка `region`, `position`, `vitals` (загрузка оживляет тело), `crates/sandbox/tests/sandbox/saves.rs` |
| 2 | схема и миграции | ✅ секции с версиями и цепочкой миграций, дыра в цепочке роняет сборку; образцы файлов версии 1 в `tests/samples/`; сохранение из более новой версии не порча: отказ без копий, `.damaged` и перезаписи, запись в такой слот тоже отказывает (`tests/save/newer.rs`) |
| 3 | атомарная запись, контрольная сумма, копии | ✅ `.tmp` + `sync_all` + `rename` + fsync папки, `shards-save/1 crc32 len`, три копии, восстановление с записью в журнале, матрица порчи (`tests/save/damage.rs`), `save_damage` для QA; реплей на проигрывании на диск не пишет (`tests/save/playback.rs`) |
| 4 | «сохранить сейчас», автосохранение | ✅ `save_now`, `autosave_on`, дебаунс из `*.save.ron`, запись в `FixedLast`, хэш состояния не меняется; при выходе ожидающие запросы пишутся сразу (`flush_saves`); эталонный реплей `tests/replays/saves.json` |
| 5 | реестр сохраняемого | ✅ `SaveRegistry`, инструмент `save_registry`, поле `save` в `ai/status` |
| 6 | настройки на фреймворке Bevy | ✅ `crates/shards_settings`: группы графики, звука, управления и доступности на `bevy::settings` (`settings.toml`), `add_settings_group` для групп игры, хранилище после `build` игры (`install`, `--settings-dir`/`--no-settings`), запись через секунду и при выходе (в том числе изменения кода игры), инструменты `settings`/`settings_set`. Файл не теряется: не TOML — копия `.bad` и последняя хорошая `.bak`, неудавшаяся запись возвращает копию и повторяется; `reset` возвращает умолчания игры. Применяют: окно (`apply_graphics`), камера и ввод (`shards_motion`: поле зрения, тряска, чувствительность, `MotionBlur`, `hold_to_toggle`, `rebind` ⇄ `ControlSettings` с теми же проверками у `settings_set`, перенос `input.ron` по просьбе игры), мерцание предупреждений (`shards_action`). Тесты: `shards_settings` (18 в `src/`, 44 в `tests/settings/`), `shards_motion/tests/motion/input.rs`, `camera.rs`, `sandbox/tests/sandbox/settings.rs`, сценарий `settings.scenario`. Не проверено на настоящем дисплее, кроме снимка под Xvfb (размер окна из настроек); громкости применяет модуль звука (`shards_audio`, E9.5; шина `ui` добавлена), `render_scale` — множитель окна, не внутреннее разрешение с апскейлом |
| 7 | сохранения в кооперативе: персонаж у игрока, мир у хоста | ✅ `shards::coop`: `Hello` несёт персонажа (слот `--character`, без флага — по `--name`; стенд `c1`…`cN`), хост применяет его в записанном `player_join` (проверяет присланное; провалившийся вход убирает тело), `CharacterSave` идёт клиенту при `save_now`, автосохранении и `save_game` по телу игрока, при уходе и концу сессии; `SessionCharacters` возвращает вернувшемуся новейшее у хоста, ушедший токен возвращается владельцу; клиент мир не пишет и не грузит, мир, загруженный хостом, уходит клиентам заново; конец сессии пишет мир и персонажа хоста и называет слот клиентам. `crates/sandbox/tests/sandbox/net_coop.rs` (34), `shards_save/tests/save/routing.rs` (3). Слоты по умолчанию — `slot1` (не `world`/`character`, как в первой записи спецификации) |

## E9. Интерфейс, звук, локализация (фаза D)

| № | что сделать | статус |
|---|---|---|
| 1 | виджеты | ✅ `crates/shards_ui`: вид контента `screen`, виджеты `Panel`, `List`, `Grid`, `Tabs`, `Text`, `Hint`, `Bar`, `Cooldown`, `Compass`, `Allies`, `Compare`, `Button`, `Slider`, `Toggle`, `Choice`; значения из источников (`add_ui_source`, `you.*` и `allies` движка) и настроек игрока (`setting`), действия кнопок (`close`, `back`, `open`, `quit`, `tool`, действие игры `add_ui_action`); проверки контента. Тесты `src/` (7), `tests/ui/` (16), сценарии `sandbox/scenarios/ui.scenario`, `template/scenarios/menus.scenario`. HUD песочницы остаётся её кодом (`visuals.rs`), HUD шаблона — контент |
| 2 | стек экранов, фокус | ✅ `UiStack`, слои `Hud`/`Menu`/`Modal`, фокус и `ui_navigate` как у геймпада (строки, сетки, переход через край), пауза одиночной игры от меню (`pauses`), клавиши: `pause` открывает `pause`, контекст `menu` (`navigate` с повтором, `confirm`, `back`) и мышь зовут `ui_*` от агента `input`, реплей их повторяет (`tests/ui/controls.rs`, `screens.rs`). Навигации крестовиной геймпада нет (стик и стрелки) |
| 3 | темы и масштаб текста, подписи доступности | ✅ вид `theme` (цвета, размеры, отступы), шрифты умножаются на `accessibility.text_scale`, у каждого виджета узел AccessKit (роль, подпись, значение) и `InputFocus` за фокусом: `tests/ui/draw.rs` (окно без GPU). С настоящим экранным чтецом и на дисплее не проверено; шрифт задаёт тема (`font`; без него — Bevy по умолчанию, только латиница) |
| 4 | локализация | ✅ `crates/shards_locale`: языки в контенте (`locale`), тексты на Fluent (`assets/locale/<язык>/*.ftl`) с параметрами, формами множественного числа и фразой во фразе, запасной язык, настройка `locale.language` и смена языка на лету (открытые экраны переписываются до возврата вызова), `@id` в текстах экранов и подписи вариантов `Choice`, шрифт темы, горячая перезагрузка `.ftl`, проверка контента (ошибки разбора с позицией, `@id`, которого нет ни в одном языке), инструменты `locale` и `text`. Тесты `src/` (10), `tests/locale/` (6), сценарий `sandbox/scenarios/locale.scenario`, в шаблоне `menus.scenario` и `tests/game/screens.rs`. Языка системы при первом запуске нет (пустая настройка — язык игры); письма справа налево и сложной вёрстки (bidi, лигатуры) не проверяли |
| 5 | звук: шины, музыка слоями, пространственные звуки | ✅ `crates/shards_audio`: виды контента `sound`, `music`, `audio` (микшер, один на игру); шины `music`, `sfx`, `ui`, `voice` с громкостью игрока (`audio.<шина>` × `audio.master`, применяется к звучащему); голоса с приоритетом и лимитом (сильный забирает голос слабого, `max_instances`), пространственные звуки с затуханием `near`→`far` и панорамой от слушателя (тело HUD, поворот камеры), дальше `far` не запускаются; звуки телеграфов (`SoundCue`) там, куда придётся удар, ключей анимации и событий движка (`damaged`, `blocked`, `died`, `ui_pressed`, …); музыка: стемы одной петли, слои по условиям (`npc_near`, `npc_engaged` модуля NPC или условия игры), напряжённый слой сразу, спокойный после `hold`, плавные входы и уходы; `Music` в снапшоте, строки журнала вида `local` (хост их не шлёт); инструменты `audio`, `sound_play`, `music_set`, строка `MUSIC` в `observe`, поле `audio` в `ai/status`; проверка контента (файлы WAV/Ogg, длина стемов, слои, события, звуки телеграфов); воспроизведение через Bevy audio с feature `audio`. Решения — на часах симуляции, одинаково без окна. Тесты `src/` (8), `tests/audio/` (15), сценарий `sandbox/scenarios/audio.scenario`, в шаблоне `tests/game/audio.rs`: **музыка меняет слой при входе в бой** (`explore` → `tension` → `combat`, когда герой подходит к грунту). Звучание на настоящем устройстве вывода не проверено (CI и проверка шли без звуковой карты); файлы синтезированы примером `make_sounds` |

## E10. Рендер, производительность, платформа (фаза E)

| № | что сделать | статус |
|---|---|---|
| 1 | стилизованные материалы | ✅ `crates/shards_look` (E10.1): вид контента `look` (`*.look.ron`) — `color` (без него меш сохраняет свой цвет), `shading` (`Toon(bands)` от 2 до 8 полос, `Matte`, `Unlit`, `Lit`), `flat` (плоские грани), `outline` (ширина в пикселях, больше 0 и не больше 16), `glow`, `materials` (материалы glTF). Компонент `Look` на сущности или предке (префаб: `"Look": ("stone")`), иначе вид по имени материала glTF; меш без вида остаётся с материалом игры. Свет: солнце с тенями полосами, остальной свет ровно, ничего не блестит, экспозиция не трогает `glow` (`ExtendedMaterial<StandardMaterial, Toon>`, шейдер WESL). Контур — оболочка наизнанку: ширина в пикселях экрана, нормали усреднены по точкам меша, у меша со скином тот же `SkinnedMesh`, без тени. Горячая перезагрузка перекрашивает меши в том же кадре. Только картинка: feature `render`, окно или `--offscreen`, без картинки ничего не стилизуется; `Look` в префабе — обычный компонент (реплей, снапшот, клиенты кооператива), картинке нужный `Look` игры ставят на меши-потомки. Пресеты low-poly — `extends` (вид начинается с другого: `low_poly` — плоские грани и контур). Для ИИ: `looks`, `look_outlines`, поле `looks` в `ai/status`, `Look` в `inspect`, два пункта меню разработчика (`world`). Проверка контента: цвет, полосы, `Lit`, ширина контура, материал одного вида, префаб с `Look` на вид, которого нет. Песочница и шаблон рисуются одним белым материалом и видами (`look/sandbox.look.ron`, `looks.look.ron`); живьём на GPU с `--offscreen` — полосы, плоские грани и контуры на картинке, `looks`: `11 looks · 49 meshes styled (36 outlined) · 2 without a look`. Тесты: `shards_look` — 12 в `src/`, 23 в `tests/look/` и 1 под `#[ignore]` (картинка на GPU: контур 3 px со всех сторон, у сферы в полосах три самых частых оттенка — 97 % пикселей против 9 % у сферы в свете Bevy); `sandbox` — тело с видом у клиента кооператива, сценарий `looks.scenario`. Не сделано: только `StandardMaterial`; контур только по силуэту, ширина одна на вид (от расстояния не зависит); `Look::new` в коде с неизвестным id проверка не видит; в окне на дисплее глазами не проверялось (только `--offscreen`) |
| 2 | растительность и инстансинг | ✅ `crates/shards_scatter` (E10.2): вид контента `scatter` (`*.scatter.ron`) — тайлы по id и `#тегу`, регионы, `density`, `spacing`, `scale`, `tilt`, `max_slope`, полоса `height`, `clearance`, виды, тени, от 1 до 4 уровней детализации с `fade`; меши-примитивы (`Blades` — пучок травы, `Cone`, `Rock`, `Box`) или glTF. Раскладка — чистая функция (зерно региона, клетка, id) через `DetRng` и статичного мира: земля — лучом по статичным коллайдерам вокруг тайла (`PhysicsWorld::subset`; движущиеся не в счёт; внутри стен и на колоннах не растёт), `clearance` — шар на земле по её нормали (склон места не отнимает); `graphics.foliage` оставляет начало того же списка; не больше 10 000 сущностей-мешей определения на тайл (ошибка контента и предупреждение в журнале). Вид с контуром, `spacing` меньше 0.01 м и слишком густое определение — ошибки контента; правка тегов тайла или комнаты рассыпает заново. Экземпляр и уровень — сущность с общим мешем, материалом вида и `VisibilityRange` под корнем тайла: Bevy собирает пакеты сам (в эталонной сцене 1364 рисуемых меша из 48 разных; с эффектами E10.3 — 1382 из 50, п. 4), дальше последнего уровня ничего не рисует, вне камеры отсекает; `Toon` растворяет уровни. Инструменты `scatter` и `scatter_set`, поле `scatter` в `ai/status`, `mesh_assets` в `counts` и бюджетах `perf`, два пункта меню разработчика. Песочница — `region/wilds.scatter.ron` (7182 экземпляра, 13864 сущности на 64 тайлах), шаблон — `region/valley.scatter.ron`. Тесты: юнит (14; `PhysicsWorld::subset` — в `shards_motion`), `crates/shards_scatter/tests/scatter/` (11: склон и стена, движущийся коллайдер, тег тайла, смена региона, слишком густое определение и другие; и один `#[ignore]` на GPU: трава зелёная в пикселях, выключенная и дальше последнего уровня не рисуется), сценарий `scatter.scenario`. Релизная `--perf reference --offscreen` с растительностью на RTX 5080: 379 кадров в секунду в 1080p, худший кадр 4.2 мс, все 8 бюджетов. Не проверено: «средний ПК» из ТЗ, меш glTF на настоящем файле, окно на дисплее |
| 3 | эффекты и декали | ✅ `crates/shards_fx` (E10.3): вид контента `effect` (`*.effect.ron`) — `count`, `rate` и `duration`, `lifetime`, `speed`, `spread`, `aim` (`Up`, `Hit`) и `lift`, `gravity`, `drag`, `size` и `size_end`, `color` и `color_end`, `shape` (`Quad`, `Cube`, `Shard`), `glow`, `offset`, `radius`, `spin`, `max_live`; вид `fx` (`*.fx.ron`, один на игру) — эффект на событие движка (`damaged`, `blocked`, `parried`, `evaded`, `guard_broken`, `staggered`, `died`, `healed`, `telegraph`) и на метку клипа; `PlayEffect` и `shards::fx::play` для своих. Частицы свои (у `bevy_hanabi` нет версии под Bevy 0.20), на процессоре, с пулами: `max_live` мешей на эффект делаются при загрузке контента и прячутся, переполнение отдаёт слот старейшей; один меш и один материал на эффект (цвет в `MeshTag`) — рисуются пачками; полёт решён формулой на часах симуляции (пауза замораживает, `step` двигает). Удар играет эффект на стороне тела, обращённой к нападающему. Декали телеграфов: `ForwardDecal` с материалом зоны на WESL (круг, сектор, прямоугольник, «стадион»), заливка растёт к удару, цвет врага и союзника как у гизмо, гизмо оставляют себе контур и символ (`TelegraphStyle::decals`), `DepthPrepass` камерам `AiCamera`, пока декали включены; `telegraph_decals on=false` возвращает гизмо. Только картинка: без окна и `--offscreen` ничего не играет, сущности без `AiId`. Для ИИ: `effects`, `effect_play`, `telegraph_decals`, поле `fx` в `ai/status`, пункты меню `effects` (`world`) и `telegraph_decals` (`combat`). Проверка контента: диапазоны, цвета, пул меньше запуска, событие с опечаткой, вторая таблица, ссылки на эффекты. Песочница: `fx/sandbox.effect.ron` (искры удара и блока, вспышка парирования, пыль смерти, свечение лечения) и таблица, сценарий `effects.scenario`; шаблон: три эффекта и таблица. Живьём на GPU с `--offscreen` (RTX 5080): декали под атаками стража и скелетов, искры, пыль и свечение на снимках; автотест `--perf reference --offscreen` с растительностью и эффектами — 327 кадров в секунду, все 8 бюджетов (п. 4). Тесты: `shards_fx` — 13 в `src/`, 15 в `tests/fx/` (в том числе эффект с ошибкой играет в пределах проверки, полосы декали отсчитываются от зоны) и 1 под `#[ignore]` (картинка на GPU: декаль красит пол только в круге зоны, без декалей тёплых пикселей нет, частицы светятся в точке запуска); `shards_action` — гизмо без штриховки и заливки при декалях. Не сделано: частицы на процессоре (не десятки тысяч), без текстур, следов, столкновений и света; декали только у телеграфов; эталонные реплеи песочницы и шаблона нужно записать заново (хэш контента): здесь они прежние, их запишут на Linux и Windows на вершине этой цепочки веток; в окне на дисплее глазами не проверялось (только `--offscreen`) |
| 4 | бюджеты и автотест производительности | ✅ время тика (задача 11): строка `ticks:` у `step`, запись `[perf]` в журнале, `perf` в `ai/status`, `--tick-budget-ms` (`ai_core::perf`, `crates/sandbox/tests/perf.rs`), профайлер Tracy (фича `trace_tracy` у `shards`, `sandbox` и шаблона). Время кадра (`shards::frames`, E4 фазы): кадр — основной мир (`First` … `Last`) и мир рендера для него, стоит более медленный из двух, без ожидания дисплея, кадры с `step` на паузе не в счёт; `frames` в `ai/status` за 600 кадров (среднее, худший, сверх бюджета `--frame-budget-ms`, `fps`, основной мир и рендер, время GPU и треугольники из диагностик рендера Bevy). Счётчики `counts`: сущности, меши, меши, которые рисует хоть один вид, меши камер. Автотест `--perf ID` по контенту `perf` (`shards::bench`): регион с зерном, строки подготовки, прогрев, замер (без окна — тики как можно быстрее, с `--offscreen` — реальное время и кадры), строка на бюджет, коды 0/1/2, отладочная сборка не судит; эталонная сцена песочницы `reference` (дикие земли 8×8 с зерном 42, их растительностью и эффектами, 15 врагов, 1080p): релиз на RTX 5080 — 327.2 кадра в секунду, 2.84 мс на кадр в среднем, худший 4.47 мс, все 8 бюджетов; 15 836 сущностей и 14 641 меш — это и растительность (около 13 900 сущностей-мешей), и 400 спрятанных мешей пулов эффектов, которые не рисуются; рисуется 1382 меша из 50 разных (повтор при 1–2 процессах `rustc` других веток — 313.6 кадра в секунду, все 8 бюджетов; без эффектов — 378.9 кадра в секунду, 2.44 мс; до растительности — 389 кадров в секунду, 2.33 мс, 7 бюджетов); WARP — 0.9 кадра в секунду, код 1. Тесты: `frames` (4), `bench` (3), `run` (`perf_plays_a_definition_and_names_what_stops_it`), `tests/engine.rs` (`the_status_counts_what_there_is_and_headless_has_no_frames`), `sandbox/tests/perf.rs` (эталонная сцена без окна; с `offscreen` — `#[ignore]`, нужен GPU). Не сделано: «целевой ПК» не выбран — проверено на RTX 5080 и WARP; отрисовки считаются мешами до пакетов Bevy, а не вызовами draw на GPU; задания CI с автотестом нет |
| 5 | Steam | ⏳ E (нельзя проверить здесь) |
| 6 | сборка под Windows | 🟡 `shards package` (`shards_cli::package`): профиль `dist` (в шаблоне `lto = "thin"`), фича игры `dist` (без `remote` и `hot_reload`; `shards/dist` глушит клавиши разработчика), C runtime вкомпилирован (`+crt-static`), иконка программы из `assets/icon.png` (ресурс `.res` пишет сам `shards`), папка `dist/<имя>-<версия>-<ОС>-<арх>/`; проверки — импорты PE только системные, `--check-content` и `--smoke` копии во временной папке с голым окружением, в папке игры ничего не записано и не удалено. Иконка окна (`shards::window_icon`), `windows_subsystem = "windows"` у шаблона с `dist` и вывод в терминал запуска, `quit` и `shards quit` вместо сигналов, закрытие консоли как SIGHUP, клиенты `--stand-udp` в своей группе процессов (`CTRL_BREAK`), пути без префикса `\\?\` (`ai_core::paths`). Тесты: `shards_cli` (`package`: 7, ссылка в `assets/` — только на unix; `a_game_is_asked_to_quit`), `shards_core` (`quit_answers_then_ends_the_game_through_its_exit_hooks`), `shards` (`window_icon`), `ai_core::paths` (имя с непарным суррогатом — только на Windows); живьём — `shards package` шаблона, `shards quit`, `stand stop` стенда в процессе и по UDP. Детерминизм Windows ↔ Linux на x86_64 (и Steam Deck) сделан в коде: `sin`, `cos`, `atan2`, `exp`, `powf` и остальные идут через `libm` в Bevy, glam, simba и коде движка (в `[workspace.dependencies]` feature `libm` у Bevy, `indexmap` у `parry3d` и прямая зависимость `glamx =0.3.1` с `libm` ради `simba/libm_force` — всё из `enhanced-determinism`, кроме `scalar-math`; вызовы — `bevy::math::ops`), SIMD у glam остаётся (SSE2 — одни и те же инструкции на любой ОС x86_64), `clippy.toml` (корень и `template/`, `disallowed-methods`) не даёт вернуть методы `f32` из std, тест `ai_core::determinism` держит результаты `ops` побитово. Между архитектурами (x86_64 ↔ ARM, Apple Silicon) не обещан: `enhanced-determinism` целиком дал бы его через glam без SIMD (`scalar-math`), но с ним `Vec4` и `Mat4` теряют выравнивание в 16 байт, `MeshUniform` Bevy 0.20 становится 168 байт против 176 у шейдера, и рендер встаёт (GPU-тест `shards_fx` упал на этом); `shards` такую сборку не собирает (`const` с `align_of` в `crates/shards/src/lib.rs`, проверено: с `enhanced-determinism` движок и шаблон не компилируются). Не сделано: запуск на настоящей чистой Windows (VM); прогон между ОС — реплей, записанный на Windows, пройти `--verify` на Linux (CI) и обратно; до него одинаковые биты на разных ОС не проверены; эталонные реплеи ещё не записаны заново (до этого они расходятся везде, не только на Windows) |
| 7 | обработчик падений в один файл для отчёта | ✅ `ai_core::crash` (E2 фазы): отчёт `shards-crash/1` в `crashes/crash-<время>-tick<N>.json` (или в папке игрока, если в рабочую писать нельзя) — паника (сообщение, место, поток, стек), игра (`EngineConfig::version`), машина (ОС, процессор, память, видеокарта), `ai/status`, журнал, лог и реплей сессии; `--replay` принимает отчёт. Пишется при сбое системы геймплея (вместе с краш-реплеем), при панике в другом потоке, которую никто не поймал (пойманную кодом, `crash::handled`, показывает следующий отчёт), при панике, унёсшей игру (`shards::run`, код 101, без реплея), и инструментом `crash_report` (пункт меню разработчика). Пишет игра с `crash_replays` (по умолчанию); тесты, `--verify`, `--smoke`, `--check-content`, `--worldgen-report` и `--bake-nav` его выключают. Обработчик паник ставит `shards::run`. Тесты: `ai_core::crash` (5), `system_failures.rs` (отчёт сбоя геймплея и его реплей, паника в другом потоке и ответ инструмента, `crash_report`), `shards::run` (`a_panic_that_takes_the_game_with_it_leaves_a_crash_report`); живьём — `crash_report` у песочницы без окна. Ловит только паники: нарушение доступа, ошибка драйвера и `abort` проходят мимо; в сборке `dist` стек без имён функций (нет `.pdb`); окна с сообщением для игрока нет |
| 8 | скриншоты без окна | ✅ `--offscreen` (`EngineConfig::offscreen`, E3 фазы): плагины окна без окна, камеры, нацеленные на главное окно, рисуют в картинку размера `window` (`ai_core::display::Offscreen`, `shards::offscreen`), UI — в последнюю камеру, HUD и оверлеи — по условию `ai_core::display::drawn`; `shot` снимает картинку; каждый кадр ждёт GPU (без этого программный рендер исчерпывал память), не дольше 30 с: зависший GPU кончает игру с кодом 1; `display` в `ai/status`; без GPU — код 2 с советом. Проверено на Windows 11: RTX 5080 (Vulkan) и WARP (`WGPU_FORCE_FALLBACK_ADAPTER=1`) дают одну картинку. Тесты: `offscreen_is_the_window_plugins_without_a_window`, `a_headless_game_has_no_picture_and_says_how_to_get_one` (`shards`), `offscreen_is_a_way_to_play_not_to_check` (`run`), `a_stuck_gpu_ends_the_game`, `every_camera_aimed_at_the_window_draws_into_the_image` (`offscreen`), `an_offscreen_game_takes_shots_without_a_window` (песочница, `#[ignore]`: нужен GPU). Не сделано: Mesa lavapipe на Linux не проверялся, задания CI со снимками нет |

## E11. Инструменты разработчика и слой ИИ

| № | что сделать | статус | где и чем проверено |
|---|---|---|---|
| 1 | меню разработчика и консоль через `call_tool` | ✅ | `crates/shards_dev` (D4): меню (F10; `dev_menu`) — пункты модулей (`add_dev_entry`: время, мир, бой, ИИ, сеть, сохранения) и игры (вид контента `devmenu`), переключатели показывают строку другого состояния, пункты с пропуском дописываются в консоли, пункт без дела говорит почему; консоль (`` ` ``; `console`) — строка `tool key=value …` через `call_tool` от агента `console` (scope Dev), история, Tab по схемам инструментов, контекст ввода `dev` на время ввода. Тесты `crates/shards_dev/tests/dev/` (6, среди них клавиши оверлеев при открытой панели) и юнит-тесты (2), `ai_core::dev` (2), сценарий `dev_menu`; окно проверено глазами (`console open=true`, `dev_menu open=true`, `shards shot`; клавиши F10, `` ` ``, стрелки, Enter, Tab, Escape — настоящими нажатиями) |
| 2 | оверлеи: хитбоксы, хёртбоксы, зоны атак; навсетка; состояние NPC; сеть; границы тайлов | ✅ | `combat_overlay`, `physics_overlay`, `overlay` (id) — ✅ (`shards_action::render`, `shards_motion::overlay`); навсетка и маршруты NPC — `nav_overlay` (`shards_npc::nav::overlay`), границы тайлов, сокеты, точки и дороги — `world_overlay` (`shards_world::overlay`): ✅ глазами (`shards shot` под Xvfb), тестов на них нет. Состояние NPC — `npc_overlay` (`shards_npc::overlay`, F7; D4): строка на NPC рядом с игроками и гизмо в окне (строка над NPC, линия к цели, конус зрения, где потерял цель, шум, поводок), тест `npc_overlay_answers_a_line_per_npc_near_the_players`, сценарий `npc_navmesh`; сеть — `net_overlay` (C) |
| 3 | инструменты: бессмертие, телепорт к тайлу или точке, фаза босса, спавн префаба, замедление | ✅ | `god`, `teleport` (в точку), `goto_tile` (к тайлу; тест `goto_tile_lands_on_the_ground_of_a_tile`), `spawn_prefab`, `spawn_level`, `speed`; переход к фазе босса через снапшот (`snapshot_load`, тест `go_to_boss_phase_two_is_a_snapshot_loaded_by_name`, сценарий `snapshot`) |
| 4 | снапшоты в любой тик | ✅ | `ai_core::snapshot`: `snapshot_save`/`snapshot_load`, `shards save-snapshot`/`load-snapshot`, реплей `shards-replay/3` со снапшота; регион, его уровни-тайлы и навсетка возвращаются со снапшотом (`add_snapshot_prepare`, `RegionBuild`, `add_snapshot_rebuild("navmesh")`), загрузка, которой отказали после подготовки, возвращает мир, каким он был, номера уровней (`LevelCounter`) — состояние снапшота; `tests/sandbox/snapshots.rs` (28), `ai_core::snapshot` (20), `shards::run` (2), `shards_npc/tests/npc/snapshot.rs` (4), тесты регионов и уровней `shards_world`. Не входит: правка контента и `spawn_level` до снапшота в реплей со снапшота (свежая игра должна быть уже такой); в реплее `content_apply`, который добавляет файл уровня тайла, и `regen` в тот же тик файл не успевают загрузить |
| 5 | `clearance`, `path_to`, видимость лучами физики | ✅ | по физике — `src/perception/`, `physics_line_of_sight`; `tests/motion/perception.rs`. По навсетке (D4) — `path_to` через `RoutePlanner` (поле `provider`: `navmesh` в регионе, `local grid` вне его), `clearance` с `closed`, где навсетка кончается раньше физики (`reach`), `nav_path` (плюс тайлы маршрута) и `move_to path=true`; тесты `navmesh.rs` (`stitching_turns_tiles_with_them_and_follows_open_sockets` — `E closed 4.0m`, `a_diagonal_reach_stops_where_a_side_column_is_switched_off` — диагональ мимо вырезанной колонки, `move_to_path_follows_the_navmesh_beyond_the_local_planners_reach` — `provider` и `max`), сценарий `npc_navmesh` |
| 6 | связать слой ИИ с модулями | 🟡 | модули фазы A и B — ✅ (генератор: `regen`, `region_load`, `goto_tile`, `worldgen_report`; уровни и NPC — инструменты и строки в `observe`). Инструменты сохранений (C) — ✅ (`save_game`, `load_game`, `save_check`, `save_list`, `save_registry`, `save_damage`: `crates/shards_save/src/tools.rs`, тесты `tests/save/tools.rs`). Стенд «хост + клиенты» (C) — ✅: `--stand N` (хост и клиенты в одном процессе, свой BRP-порт и файл порта у каждого, общее время: `step` у любого гонит всех, `pause`/`resume`/`speed` видны в `ai/status`, стенд кончается с хостом и при выходе доставляет конец сессии; настройки игрока — у хоста, `--settings-dir`/`--no-settings`), агент, привязанный к игроку, инструменты `net_*`; тесты `net_lifecycle.rs`, `shards::run`. Для агента: `--stand N --stand-udp` (хост по UDP, клиенты — дочерние процессы; тест `stand_udp.rs` с настоящими процессами), `shards --peer c1` (токен клиента — `SHARDS_TOKEN_C1` или его файл, на любом пути), `wait --peer all` (останавливается сразу на том, что ожиданием не лечится), `shards stand` / `stand stop`, опрос отложенного `step` через `ai/step_poll`, клиент, не вошедший в сессию или упавший, останавливает стенд по UDP (ушедший сам или тот, с кем связь закрыл хост, `net_kick`, — нет; прощание клиента, не принявшего мир, не в счёт), `--stand N` проверяется по `max_players` до запуска процессов, сценарии стенда (заголовок `stand`, `@c1`; `crates/sandbox/scenarios/stand/`, тесты `scenario_files.rs`; шаг CI «Stand scenarios against a live stand»). Дерево интерфейса (D1) — ✅: `ui_read`, `ui_open`, `ui_close`, `ui_navigate`, `ui_click`, `ui_set`, раздел `UI` в `observe`, поле `ui` в `ai/status`. Агент-игрок — впереди |
| 7 | сценарные тесты: файл сценария в CI без окна, свой набор у модуля | ✅ для A–D | `shards_script`, `ai_core::scenario`, `shards scenario`; сценарии песочницы: `obstacle_course` (E2), `combat_prototype`, `boss_fight`, `dodge`, `errors` (E3), `npc_director`, `npc_navmesh` (E6), `region` (E5), `snapshot`, `dev_menu` (E11), `saves`, `settings` (E8), сценарии стенда `stand/*.scenario` (E7), `ui`, `locale`, `audio` (E9), сценарии шаблона `smoke`, `valley`, `menus`; CI-задания `test` (в процессе, вперёд и назад) и `headless-cli` (против живой игры, вперёд и назад). Сценарии остальных модулей — по мере появления |
| задача 9 | инварианты: правила, которые останавливают игру | ✅ | `ai_core::invariants`: `add_ai_invariant`, `assert_add`/`assert_list`/`assert_remove`/`assert_reset`, `shards assert`, `shards fuzz`, код выхода 3, реплей нарушения; правила песочницы `health_in_range` и `bodies_in_world`; `tests/sandbox/invariants.rs`, тесты `ai_core::invariants`, `shards_cli` (`fuzz_inputs_come_from_the_seed`), `shards_script` (оборванный шаг) |
| задача 10 | подробности расхождения реплея | ✅ | `Check.parts`, `--check-every`, `--check-detail`; `--verify` называет сущности и компоненты и печатает их значения; `tests/sandbox/replays.rs`, тесты `ai_core::replay` |
| задача 11 | время тика | ✅ | `ai_core::perf` (см. E10, п. 4); `crates/sandbox/tests/perf.rs`. Tracy: фича `trace_tracy` у `shards`, `sandbox` и шаблона (README, «Время тика») |

| «Готово, когда» | статус |
|---|---|
| каждый пункт списка 32.8 GDD доступен и из меню разработчика, и из `shards` | ✅ зерно региона (`region_info`), сотни регионов (`worldgen_200`), достижимость (`region_check`), фаза босса (`boss_phase_2` песочницы), бессмертие и замедление (`god`, `slow`), журнал сети (`net_overlay`), проверка сохранений (`save_check`), хитбоксы (`combat_overlay`), состояние ИИ (`npc_overlay`), лаг (`net_lag`): тест `every_item_of_the_gdd_list_has_an_entry_whose_tool_exists`, сценарий `dev_menu`; из `shards` — те же инструменты и `dev_menu run=<id>` |
| сценарии всех модулей идут в CI | 🟡 сценарии модулей фаз A–D идут (`test`, `headless-cli`); модулей фазы E ещё нет |

## Тесты

Тесты по крейтам, `cargo test -p <крейт>`: все строки, кроме шаблона, взяты из одного полного прогона `cargo test --workspace`
15 октября 2026 (после слияния C7, `3b2d359`, и правок шестого круга ревью; сборка этого дерева, 0 падают). Строки `shards_script`,
`shards_cli`, `shards_world` и `sandbox` после C9, `shards_ui` и `shards` после D1, `shards_locale` после D2, `shards_audio` после D3, `ai_core`, `shards_content`, `shards_world`, `shards_npc` и `shards_dev` после D4, `ai_core`, `shards`, `shards_cli`, `shards_core` и `sandbox` после E1–E3 фазы E, `ai_core` после E10.6 пересчитаны по разнице: тесты добавлены и убраны в этих крейтах, а прогон C9 шёл на
Windows, где тестов меньше (часть только для Unix) и тогда падали девять (теперь — только эталонные реплеи, см. «Известные ограничения»). У шаблона — счёт `#[test]` и
`#[ignore]` в `template/tests/game`, его прогон отдельный. Общего числа здесь и в README нет: оно расходилось с таблицами при
каждой правке, а проверить его можно только полным прогоном. Помечены `#[ignore]`: запись эталонных реплеев, проверка окна под Xvfb,
замеры сети, получасовая долгая игра, примеры в документации; у шаблона 36 (и один `#[ignore]`: запись его эталонов). Запускать нужно из обычной оболочки:
под `nohup` игнорируется SIGHUP, и тест `kill_and_hangup_remove_the_token_and_the_port_file` падает на `signal 1`.

| крейт | тестов | где |
|---|---|---|
| `ai_core` | 102 | `crates/ai_core/src/*` (снапшоты 22, строки и пункты меню разработчика `dev` 2, отчёт о падении `crash` 5, побитовые результаты `ops` и glam на `libm` `determinism` 3; ещё 1 только на Windows) |
| `shards_script` | 59 | `crates/shards_script/src/*` (разбор, условия, запуск сценариев 30, долгая игра `soak` 7, входы 2) |
| `shards_cli` | 62 | `crates/shards_cli/src/*` (клиент игры-пира `peer` 26, сценарии 12, команды 17, сборка `package` 7; один только на unix) |
| `shards_core` | 15 | `src/` и `tests/core.rs` |
| `shards_content` | 24 | `src/` и `tests/content.rs` (проверки, которые другой модуль добавляет к виду) |
| `shards_motion` | 176 | `src/` и `tests/motion/` (контроллер 28, перцепция 29, ввод 30, физика 11, `PhysicsWorld::subset` для растительности 1, …) |
| `shards_settings` | 62 | `src/` (18) и `tests/settings/` (44: хранилище, файл и его порча, правка файла руками в работающей игре, повтор записи и запись при выходе, группы, инструменты, изоляция от снапшота, хэша и репликации) |
| `shards_ui` | 23 | `src/` (7: разбор действий и цветов, проверки) и `tests/ui/` (16: экраны, меню, клавиатура, рисование и подписи доступности, реплей, правка и отказ контента) |
| `shards_locale` | 16 | `src/` (10: формы множественного числа, фраза во фразе, запасной язык, разбор файлов) и `tests/locale/` (6: смена языка в игре, меню, проверка контента, перезагрузка файла) |
| `shards_audio` | 25 | `src/` (10: длина WAV и Ogg по страницам, битая страница и контрольная сумма Ogg, затухание, шины, голоса) и `tests/audio/` (15: лимит голосов и кража, `max_instances`, расстояние и источник, часы, громкость игрока, события, телеграфы и меню, инструменты, слои и `hold` у каждого слоя свой, `music_set`, строки `local`, трек, убранный из контента, снапшот, проверка контента) |
| `shards_look` | 35 | `src/` (12: описание вида и пресеты, материал из исходного, прозрачность и свечение, плоская копия и оболочка контура, морф-таргеты и данные на GPU) и `tests/look/` (23: проверки контента и `extends`, инструменты и меню разработчика, какой меш какой вид берёт, материалы, контуры и их `RenderLayers`, горячая перезагрузка и пресеты, ассеты, изменённые на месте, клиенту кооператива — тело с видом в `sandbox`); 1 под `#[ignore]`: картинка на GPU — полосы и контур в пикселях |
| `shards_scatter` | 25 | `src/` (14: определение и каждая его ошибка, `spacing` не меньше сантиметра, контур через `extends`, предел сущностей на тайл; меши; раскладка — зерно, `spacing`, `graphics.foliage`, земля и полоса высот, свободное место над склоном и у стен, только статичные коллайдеры рядом с тайлом, предел для крошечного `spacing`) и `tests/scatter/` (11: без картинки, тайлы и повтор, стены и склон, только статичные коллайдеры, пакеты и `VisibilityRange`, `foliage`, `scatter_set`, правка контента и тегов тайла, слишком густое определение, смена региона; ещё один `#[ignore]` — картинка на GPU); прогон 16 октября 2026 на Windows |
| `shards_fx` | 28 | `src/` (13: описание эффекта, опечатка в числах без переполнения и в пределах, полёт формулой против шагов, конус и шар, ось удара, пул и вытеснение старейшей, формы декалей и рост заливки, полосы от зоны, а не от начала мира, упаковка цвета, меш осколка) и `tests/fx/` (15: проверки контента, эффект с ошибкой играет в пределах проверки, инструменты, меню и статус без картинки, пулы из контента, запуск, часы симуляции, `rate`, эффекты событий на стороне удара, меток и `PlayEffect`, декали круга и сектора, цвет союзника, выключение декалей и `DepthPrepass`); 1 под `#[ignore]`: картинка на GPU — декаль в зоне и частицы в пикселях; прогон 16 октября 2026 на Windows, вместе с растительностью |
| `shards_action` | 146 | `src/` и `tests/combat/` |
| `shards_level` | 80 | `src/` (3) и `tests/level/` (77: файл и узлы, свойства, коллайдеры, маркеры, спавн, `level` в контенте, реплей, картинка) |
| `shards_world` | 75 | `src/` (17) и `tests/world/` (58: генерация, проверки, подземелья, регионы, смена региона, стена по краю, уровни-тайлы и их маркеры-сокеты, зоны, навсетка региона) |
| `shards_npc` | 83 | `src/` (19) и `tests/npc/` (64: поведение, восприятие, директор, навсетка и `path_to`/`clearance` по ней, реплей, снапшот, инструменты и `npc_overlay`, страж данными) |
| `shards_dev` | 8 | `src/` (2: пункт игры, общее начало слов) и `tests/dev/` (6: список GDD 32.8 и порядок разделов, пункты и строки через инструменты, консоль и история, клавиши оверлеев ждут открытую панель, Tab, битый пункт в контенте) |
| `shards_save` | 91 | `src/` (33) и `tests/save/` (58) |
| `shards_net` | 127 | `src/` (82 — канальный слой без Bevy, 45 — сессия) |
| `shards` | 60 | `src/` (49: `bench` 3, `coop` 3, `frames` 4, `offscreen` 2, `run` 17, `stand_udp` 14, `ui_wiring` 1, `window_icon` 1, …), `tests/engine.rs` (5, из них 4 с `render`), `tests/failures.rs`, `tests/level_render.rs`, `tests/run_to.rs` (3), `tests/action_render/` |
| `sandbox` | 304 | `tests/sandbox/` (280, из них кооператив `net*.rs` 150: `net.rs` 17, `net_coop.rs` 34, `net_sync.rs` 31, `net_guards.rs` 30, `net_lifecycle.rs` 20, `net_predict.rs` 14, `net_seams.rs` 4; `stand_udp.rs` 7, `snapshots.rs` 29, `replays.rs` 23, `invariants.rs` 12, долгая игра `soak.rs` 1; ещё 5 `#[ignore]`: запись эталонных реплеев, два замера сети, получасовая долгая игра и снимок без окна, `offscreen.rs`), `tests/system_failures.rs` (10), `tests/perf.rs` (7: время тика и автотест на эталонной сцене; ещё 1 `#[ignore]`: автотест с `offscreen`), `tests/log_capture.rs` (5), `src/` (2) |
| шаблон | 36 | `template/tests/game/` (содержание 8, кооператив 4, детерминизм 3, реплеи 3 и запись под `#[ignore]`, сценарии и сценарии стенда 4, регион 4, экраны 5, звук 2, манифест 3) |

## Известные ограничения и отложенное

Что работает не так, как можно ждать, и что отложено осознанно:

- **Вне региона `path_to` локальный.** Вне загруженного региона (и для тела, которое навсетка не обслуживает) `path_to` и
  `move_to path=true` — локальный сеточный A* (клетка 0.5 м, поиск в ограниченной области, не дальше 80 м по умолчанию), а
  `clearance` — только лучи физики.
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
  (`ruin.gltf` песочницы и шаблона) сделаны тестовым генератором; Blender здесь нет. Узлы файла тайла `content check` читает,
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
- **Скриншоты без окна — только с GPU** (`--offscreen`) или с программным рендером: WARP на Windows проверен, Mesa lavapipe на Linux —
  нет. В CI снимков нет: задание не добавлено (Actions сейчас не запускаются).
- **Задание CI `replays-window` необязательное** (`continue-on-error`) до тех пор, пока не докажет стабильность на
  раннерах. Остальные четыре задания обязательные.
- **Теги не поставлены.** `v0.1.0` (коммит `e90193d`), `v0.2.0` и `v0.3.0` ставит мейнтейнер; версия в `Cargo.toml` остаётся
  `0.2.0`, пока он не выпустит `0.3.0` (записи фазы B уже лежат в `CHANGELOG.md` под `0.3.0`). Пока тегов нет, игра закрепляет
  движок коммитом (`rev = "…"`).
- **Вход в сессию без предела частоты.** `Hello`, `Ready` и `Bye` подряд не ограничены по частоте; пир, который шлёт их
  без остановки, получает отказ на первом `Hello` и не читается дальше (бюджет разбора за опрос), а срок входа
  (`6 × timeout_secs`) и срок забвения ушедшего (10 с без подтверждений, потолок 5 минут) не дают ему занять запись надолго, а
  таблица пиров, которые не играют, ограничена (`max_strangers`) и при нехватке места отдаёт запись, стараясь не терять
  непрочитанный `Hello` (не более `max_players` берегущихся от создания, 5 с). Сессия по LAN без проверки личности, предел по
  адресу по UDP обходится сменой порта; пир, который шлёт по одной датаграмме с сообщением чаще, чем его отпускают, заводится
  заново. **Известный предел:** кто шлёт со многих настоящих адресов без остановки, может задержать, а то и сорвать вход игрока,
  чей `Hello` идёт по частям (уже подтверждённые куски канал не пришлёт снова), или пришёл одной датаграммой в тот же тик,
  что и пачка датаграмм с новых адресов; без рукопожатия, не держащего состояния у хоста (cookie), это не закрыть.
- **Контент во время сессии не синхронизируется.** Хэш контента сверяется при входе. Если файл контента правят во время
  сессии (горячая перезагрузка), мир заново, который хост шлёт после этого (смена региона, рассинхрон), клиент с другим
  контентом не берёт (`net: cannot take the host's world: …`) и выходит из сессии. Так кончился первый получасовой
  прогон `shards soak`, когда файл региона на диске поменяли посреди игры. Контент правят между сессиями.
- **Сохранения в кооперативе: что не сделано.** Клиент не просит у хоста собственное сохранение сейчас (его `save_game
  what=character` отвечает, что персонажа сохраняет хост: у клиента нет полного персонажа, и то, что он снял бы сам,
  затёрло бы хороший файл). Персонаж больше 64 КБ ни один хост не примет. Слоты по умолчанию — `slot1`, у клиента по
  имени игрока. Ветки с проигрываемым реплеем (`replay_playing`) в маршрутизации хоста тестом не покрыты.
- **Интерфейс: чего нет.** Шрифт задаёт тема (`font`; без него — шрифт Bevy, только латиница), в меню нет крестовины
  геймпада (стик и стрелки), нет экрана переназначения клавиш (страница `controls` меняет чувствительность и переключатели,
  клавиши — `rebind` и `settings_set`), картинки экрана загрузки. Рисование проверено тестом без GPU (узлы, тема, подписи
  AccessKit), а не глазами на дисплее и не настоящим экранным чтецом.
- **Эталонные реплеи расходятся на любой ОС, в том числе в CI на Linux, пока их не запишут заново**
  (`record_reference_replays`; `record-replays.yml` в это дерево не попал; их запишут на Linux и Windows на вершине этой
  цепочки веток): падают `every_reference_replay_verifies_headless` движка и `reference_replays_still_match` шаблона. Причин
  две. Первая наблюдалась: реплеи записаны в `fb78b42` (14 октября), а контент песочницы и шаблона с тех пор менялся много раз
  (фазы C и D, виды E10.1, растительность E10.2, эффекты E10.3). В прогоне на Windows 16 октября, после E10.6, оба теста
  расходятся на первой проверке, на тике 60: `Look differs` у тел (у шаблона ещё `SaveKey` и `Trophies`) и `replay check
  content differs`, а реплей со снапшота отказан с `content differs`. Хэш контента от ОС не зависит, поэтому на Linux тесты
  падают так же (сам прогон на Linux не делали). Вторая причина ожидается, но за первой её не видно: симуляция перешла на
  `libm` (E10.6), и числа `f32` могут отличаться от записанных с математикой glibc в последних битах. После новой записи тесты
  должны пройти на любой ОС x86_64, в том числе на Windows, но это ещё не проверено. Других падений на Windows в том прогоне нет:
  пути `\\?\` у `snapshot_save` (три теста `scenario_files`), разделители путей
  (`a_relative_root_makes_the_asset_folder_absolute`), сигналы Unix у `shards stand stop` (два теста `peer`) и запись
  настроек (два теста `persist`), из-за которых раньше падали ещё восемь тестов движка, исправлены сборкой под Windows
  (E10.6). Игра и стенд на Windows работают: получасовые прогоны `shards soak` из таблицы приёмки фазы C шли там.
- **Относительный `BEVY_ASSET_ROOT` не принимает наблюдатель за файлами.** `BEVY_ASSET_ROOT=crates/sandbox target/debug/sandbox
  --headless` падает в `AssetPlugin` с `Failed to create file watcher from path "crates/sandbox/assets"`; абсолютный путь
  (`$PWD/crates/sandbox`) работает, как и `cargo sandbox`.
