# AGENTS.md

Инструкции для агентов-кодеров (Claude Code, Codex и другие), которые работают в этом репозитории.

## Главное правило

В движке нет LLM и не должно быть. Не добавляйте встроенных или локальных агентов, моделей,
вызовов API моделей, API-ключей, циклов «модель → инструмент» внутри игры. Игра — среда, которой
управляют снаружи: вы, через CLI `shards`. Всё, что нужно агенту, добавляется как инструмент
(`add_ai_tool` / `add_ai_query`) или как команда `shards`, а не как код, который думает за агента.

## Сборка

- Системные пакеты: `pkg-config libasound2-dev libudev-dev` (для окна под Xvfb ещё `xvfb mesa-vulkan-drivers
  libxkbcommon-x11-0`).
- `cargo build --workspace`, `cargo test --workspace`. Первая сборка Bevy долгая;
  `.cargo/config.toml` ограничивает её четырьмя потоками. `template/` — отдельный workspace:
  `cargo test --manifest-path template/Cargo.toml`.
- Крейты: `shards` (зонтик, единственная зависимость игры), `shards_core`, `shards_content`, `shards_motion`,
  `shards_action`, `shards_level` (уровни из Blender), `shards_world` (регионы, генерация), `shards_npc` (враги, навсетка),
  `shards_save` (сохранения: секции, слоты, миграции, копии), `shards_ui` (экраны и меню из контента), `shards_locale`
  (языки: тексты на Fluent, смена на лету), `shards_audio` (звук: шины, голоса, музыка в слоях), `shards_dev` (меню и
  консоль разработчика), `ai_core`, `shards_script`, `shards_cli`, `sandbox` (арена движка). Карта — в README, «Архитектура».

## Как смотреть на игру и действовать в ней

```bash
cargo sandbox --headless --paused --port 0 --port-file /tmp/g.port &   # без окна, на паузе: время идёт только по step
export SHARDS_PORT_FILE=/tmp/g.port
cargo shards wait                       # ждёт загрузку, печатает pid, seed и порт
cargo shards observe                    # срез мира текстом + журнал с прошлого раза
cargo shards tools                      # инструменты и сигнатуры (--json: схемы аргументов)
cargo shards act move_to target=e6      # действие → CHANGES и JOURNAL
cargo shards step 120 --until journal:died
cargo shards inspect e6                 # все компоненты сущности
cargo shards save-replay                # сессия с тика 0 → replays/tick<T>.json
cargo shards save-snapshot              # состояние на этом тике → snapshots/tick<T>.json
cargo shards load-snapshot snapshots/tick120.json   # вернуться к нему (регион и навсетка вернутся с ним); реплей дальше идёт с этого снапшота
cargo shards scenario FILE...           # файл сценария: команды, ожидания, инварианты (README, «Сценарии»)
cargo shards content check              # контент запущенной игры; без игры: cargo sandbox --check-content
cargo shards act regen region=wilds seed=42    # сгенерировать регион и перейти в него (экран загрузки, игрок у ворот)
cargo shards act nav_path to=e6         # маршрут по навсетке региона; npc_state id=eN — что у NPC на уме
cargo shards worldgen --region wilds --seeds 1..1000 --report   # пакетная проверка генератора; без игры: cargo sandbox --worldgen-report wilds --seeds 1..1000
cargo sandbox --bake-nav                # запечь навигацию тайлов в assets/nav/ (после правки тайла, его файла уровня или тела agent)
cargo sandbox --replay FILE --verify    # реплей без окна; добавьте --windowed (под xvfb-run -a) для окна
cargo shards assert add hp 'field:6:Health.current<300'   # правило: игра встанет, когда оно нарушено (`step` → код выхода 3)
cargo shards fuzz --seed 7 --steps 300  # случайные входы, пока не нарушится правило
cargo shards soak --minutes 30 --regen wilds   # стенд: агенты с зерном играют за всех участников, до первого рассинхрона
kill <pid>                              # pid из `wait`; на Windows (и где угодно) — cargo shards quit
cargo shards package --manifest-path template/Cargo.toml   # сборка для игроков в template/dist/ и её проверка (`--smoke`, DLL)
```

- `shards` запускайте из папки игры или ниже: там лежит токен `.shards/<port>.token`, без него игра
  отвечает `unauthorized`. Из другого места передайте его в `SHARDS_TOKEN`.
- Останавливайте игру `kill` (или Ctrl+C), не `kill -9`: так она удаляет токен и `--port-file`. На Windows сигналов нет:
  `shards quit` (инструмент `quit`) завершает игру так же и ждёт, пока она перестанет отвечать и кончится её процесс.
- Собранный бинарник запускайте через `cargo sandbox` (или с `BEVY_ASSET_ROOT=$PWD/crates/sandbox`, путь абсолютный:
  относительный не принимает наблюдатель за файлами): иначе он не найдёт `assets/`, и мир окажется пустым.
- Файлы игра пишет только к себе: реплеи в `replays/`, снимки экрана в `shots/`, снапшоты состояния в `snapshots/`,
  отчёты о падениях в `crashes/` (`save-replay replays/x.json`, `shot -o shots/x.png`, `save-snapshot snapshots/x.json`).
  Абсолютные пути и `..` не принимаются.
- Числа и позиции берите из `observe` и `inspect`, не из картинок. `shards shot` (нужно окно, можно под Xvfb) —
  для визуальных багов: PNG с метками `eN` и легенда, где на картинке какой id.
- Ошибки инструментов объясняют, как исправить вызов. Читайте их, а не повторяйте вызов вслепую.
- Инструменты движка (`shards tools`): **движение** — `move_to` (`path=true` ведёт по `path_to`), `move`, `stop`, `jump`,
  `sprint`, `glide`, запросы `clearance` и `path_to` (в регионе по навсетке: `path_to` называет `provider`, `clearance` —
  `closed`, где навсетка кончается раньше физики), `input_bindings`, `rebind`; **бой** — `light_attack`, `heavy_attack`,
  `block`, `parry`, `dodge`, `use_item`, `use_technique`, `lock_on`, `attack`, `god`, `heal`, `stagger`; **контент** —
  `content_list`, `content_get`, `content_check`, `content_reload`, `spawn_prefab`; **мир** — `regen`, `region_load`,
  `goto_tile`, `region_info`, `worldgen_report`, `spawn_level`, `level_info`, `level_reload`; **NPC и навигация** —
  `npc_state`, `npc_set_state`, `nav_path`, `nav_info`, `nav_bake`, оверлеи `world_overlay` и `nav_overlay` (окно),
  `npc_overlay` (строка на NPC рядом с игроками: состояние, цель, жетон, поводок, что видит и слышит; в окне — F7); **правила** —
  `assert_add`, `assert_list`, `assert_remove`, `assert_reset` (`shards assert`); **интерфейс** — `ui_read` (дерево экранов: id
  виджетов, значения, фокус), `ui_open`, `ui_close`, `ui_navigate`, `ui_click`, `ui_set`; **язык** — `locale` (языки, на каком
  играют, чего каждому не хватает), `text` (сообщение с параметрами на любом языке); язык меняет `settings_set group=locale
  key=language value=ru`; **звук** — `audio` (что слышит эта машина: трек и слой музыки, стемы, шины, голоса, неуслышанные
  звуки с причиной), `sound_play` (сыграть звук по правилам микшера: `at=eN`, `point=[x,y,z]`, `times=25`), `music_set`
  (`track=…`/`none`, `layer=combat`/`auto`); громкость — `settings_set group=audio key=sfx value=0.5`; **меню и консоль
  разработчика** — `dev_menu` (пункты по разделам со строкой каждого; `run=<id>` выполняет пункт: `boss_phase_2`, `god`,
  `region_check`), `console line="speed x=0.25"` (строка как у `shards act`); в окне это F10 и `` ` ``, `open=true` показывает
  панель для `shot`. Свои пункты игра кладёт в контент (`*.devmenu.ron`), модуль — `add_dev_entry`. **Выход** — `quit`
  (`shards quit`: хуки выхода, токен и файл порта убраны). Условия для
  `step --until named:…`: `near_poi`, `npc_attacking`, `npcs_calm`, `npc_beyond_leash`, `npc_detour`, `npc_engaged`,
  `npc_near`; свои у игры (в песочнице `boss_phase_2`, `boss_dead`, `player_dead`, `three_attacking`, в шаблоне
  `arena_cleared`, `foe_near`, `in_fight`).
- Звук решается на часах симуляции и одинаков без окна: слушайте игру через `audio` и строку `MUSIC` в `observe`, а не через
  динамики. Смены музыки идут в журнал строками `[local] music: arena explore → combat` (не игровые события). Файлы звуков
  песочницы и шаблона синтезирует `cargo run -p sandbox --example make_sounds`.
- Инструменты боя (`dodge`, `light_attack`, `block`, `use_item`, `attack`) отвечают по правилам игры сразу: `ok: roll (W)`,
  `ok: queued, heavy starts in 0.10s` (вход ждёт в буфере) или ошибка с причиной и временем (`not enough stamina: 10/20
  (ready in 0.2s)`, `busy: windup heavy 0.50s (ready in 0.90s, when a cancel window opens)`). На ошибку сделайте `step`
  на названное время и повторите. Входы мимо инструментов, у которых вышло время, видны в журнале: `e5 dodge rejected: …`.
- Предупреждения врагов приходят в журнал заранее (`e6 winds up slam (0.50s) ⚠ circle 4m`), а `observe`
  под строкой `YOU` пишет, куда выйти из зоны: `step --until "journal:winds up"`, потом `dodge`.
- Коды выхода `shards`: 0 — готово, 1 — инструмент отказал (ошибка говорит почему), 2 — сбой самого
  `shards` (нет игры, нет токена, неверная команда), 3 — игра остановилась: `step` (или `fuzz`, `soak`) встал на упавшей
  системе или нарушенном инварианте.
- Ищете баг — опишите его правилом: `shards assert add NAME COND`, где `COND` в синтаксисе `--until`
  (плюс `warning` и `error`). После каждого тика игра проверяет правила игры и ваши; на нарушении
  `step` отвечает `stopped: invariant NAME violated at tick N (replay: …)`, и реплей уже сохранён.
  Сработавшее правило молчит до `shards assert reset`; `shards assert list` показывает все правила.
  `shards fuzz --seed S --steps N` подаёт случайные входы игрока по зерну, пока правило не нарушится.
- `warn!`/`error!` игры попадают в журнал с тиком, на котором записаны, даже внутри длинного `step`:
  `step 600 --until journal:TEXT` ловит и их. За тик — не больше 64 строк, об остальных говорит
  строка `N log lines dropped`.
- Под первой строкой ответа `step` — время его тиков: `ticks: avg 0.42 ms, max 2.10 ms (tick 142),
  budget 16.7 ms`. Тики дольше бюджета (по умолчанию — длительность тика, у игры флаг
  `--tick-budget-ms`) журнал называет записью `perf`, не чаще раза в игровую секунду:
  `150 [perf] 12 ticks over budget, worst 31.4 ms at tick 150`. Время последних 600 тиков —
  в `shards status`, поле `perf`.
- Несколько игр параллельно: `--port` у каждого инстанса, `SHARDS_PORT` у CLI. Проще так:
  `cargo sandbox --headless --paused --port 0 --port-file F` и `SHARDS_PORT_FILE=F cargo shards wait`.
  На занятом порту игра не стартует (код выхода 3). `shards wait` печатает pid, seed и порт ответившей
  игры: проверьте, что это ваша.
- Кооператив без окон одной командой: `cargo sandbox --stand 2 --headless --paused --port 0 --port-file F --net-sim
  latency=150,loss=0.02 &` — хост и два клиента, у каждого свой порт и токен (`F`, `F.c1`, `F.c2`). Дальше `export
  SHARDS_PORT_FILE=F`, и: `cargo shards wait --peer all` ждёт всех (и сразу падает с причиной, если игра не стенд, хост
  кончил стенд или токен не тот; так же ведёт себя обычный `shards wait`), `cargo shards --peer c1 observe` смотрит глазами клиента (токен клиента —
  `SHARDS_TOKEN_C1` или его `.shards/<порт>.token`; `SHARDS_TOKEN` — токен хоста), `cargo shards stand` показывает
  участников, `cargo shards stand stop` останавливает стенд. Сценарии стенда (`stand clients=2 …`, команды с `@c1`) лежат в
  `crates/sandbox/scenarios/stand/`: `shards scenario crates/sandbox/scenarios/stand/*.scenario`. `--stand 2 --stand-udp` —
  то же по настоящему UDP, клиентами-процессами, только в реальном времени; клиент, не вошедший в сессию или упавший,
  останавливает стенд (ушедший сам или тот, с кем связь закрыл хост, `net_kick`, — нет), а `--stand N` не больше
  `max_players - 1`.
- Проверить, как игра играется честно: `shards session --scope player`. Тогда доступны только
  инструменты игрока, и они действуют через `Intent`, как человек.

## Правила кода

- Код и комментарии на английском, документация (README, этот файл, `docs/`) на русском.
- Bevy закреплён на 0.20.0; не обновляйте его попутно с другой задачей.
- **Граница.** Игра (шаблон, песочница) зависит только от крейта `shards` и не меняет код движка. В движке нет имён
  из конкретной игры: если механизм пригодится второй игре с другим сюжетом, он в движке, если это конкретное
  «что» (оружие, враг, число), оно в игре, то есть в контенте.
- **Контент.** Числа и определения — файлы `assets/content/**/*.<вид>.ron` (горячая перезагрузка), а не константы в коде.
  Текст, который читает игрок, — сообщения Fluent в `assets/locale/<язык>/*.ftl`: контент называет их `@id`, код
  форматирует `shards::locale::text(world, id, &Args)` целыми фразами с параметрами, не склеивая куски.
  Песочница тоже: её мир, префабы, атаки, мувсеты и движение лежат в `crates/sandbox/assets/content/`. Новый вид контента —
  структура с `ContentDef` и `add_content`; ссылки на другие определения проверяются в `validate` (`refs.require`).
- **Мир создавайте в `OnEnter(EngineState::InGame)`**, не в `Startup`: контент есть только после загрузки. Порядок
  спавна одинаков в каждом запуске, иначе `AiId` и `ColliderId` разойдутся.
- Весь геймплей — в `FixedUpdate`, в одном из `SimSet`; случайность только через `SimRng`.
  Тест `gameplay_systems_have_a_total_order` ловит системы без явного порядка. Свои системы ставьте «до» и «после»
  наборов модулей (`CoreSystems`, `MotionSystems`, `ControllerSystems`, `ActionSystems`, `PhysicsSystems`).
- Наблюдатель события (`On<Died>` и т. п.), который меняет геймплей, сразу возвращается на машине, которая не симулирует
  (`shards::core::simulates`): клиенту кооператива хост пересылает события боя для эффектов, и иначе счёт пойдёт дважды
  (рассинхрон это поймает: `net: desync … here …, host …`).
- **Физика читает `Transform`, а не `GlobalTransform`** (внутри фиксированного тика он не пересчитывается): коллайдеры
  кладите на сущности верхнего уровня, размер задавайте в `Shape`, не в `Transform::scale`.
- **Порядок — по `AiId` или `ColliderId`, никогда по `Entity`** и не по порядку запроса: окно добавляет сущности рендера и
  сдвигает индексы `Entity`, а реплей должен совпасть с окном и без него.
- Персонажами управляют только через `Intent`: клавиатура, инструменты агента, мозги врагов и реплей пишут в него.
  Новый компонент-вход регистрируйте через `replay_input::<C>()`, иначе реплеи разойдутся.
- Новое состояние симуляции вне сущностей с `AiId` (ресурс, счётчик) снапшот не увидит: зарегистрируйте его
  `add_snapshot_resource::<R>()` (нужен `Reflect`), кэш из него — `add_snapshot_rebuild`, то, вокруг чего строится
  мир (файлы, геометрия), — `add_snapshot_check`. Иначе `snapshot_load` скажет `the restored state differs`,
  а реплей со снапшота разойдётся. Ссылки на сущности без `AiId` — `add_snapshot_entity_key`.
- Проверки реплея (`shards-replay/3`) сверяют игровые компоненты сущностей с `AiVisible`, а не текст
  `observe`. Косметику и кэши исключайте `replay_ignore::<C>()`, состояние вне таких сущностей
  (ресурсы, снаряды без id) добавляйте `add_replay_check(name, |w| …)`. Всё, что только для окна, не должно менять
  игровые компоненты.
- Время тика не входит ни в хэш, ни в реплей и разное от прогона к прогону. Тест, который сравнивает
  ответы или журнал целиком, выключает его вывод: `report_perf: false` в `EngineConfig`
  (`shards::testing::config()` уже так делает).
- То, что в игре должно держаться всегда, оформляйте инвариантом:
  `add_ai_invariant(name, |w: &World| -> Option<String>)`. Правило только читает мир и возвращает
  текст нарушения с id сущности (`e6 Health.current = 700 outside 0..=600`). Правила и их срабатывания
  не входят ни в хэш реплея, ни в снапшот; сессионные (`assert_add`) реплей записывает событиями.
- Новый инструмент — структура аргументов с doc-комментариями (они становятся описанием схемы)
  и Bevy-система `In<Call<Args>> -> ToolResult`. Вид: `query` читает, `action` меняет мир
  (журнал, дифф, реплей), `control` управляет временем и сессией. Ошибка инструмента говорит, как исправить вызов.
- ИИ видит только сущности с `AiVisible` (его даёт `AiKind`). Что о них сказано, решают
  describe-функции (`register_ai_component`, `register_ai_describer`).
- `AiId` назначает движок. Не пишите его на месте (`get_mut`, `set`, BRP `world.mutate_components`):
  индекс «id → сущность» ведут хуки `on_insert` и `on_discard`. Вставка нового `AiId` перенумеровывает
  сущность, а старый id получает надгробие `renumbered to eN`.
- **HUD и меню — контент** (`*.screen.ron`, тема — `*.theme.ron`), а не код окна: значения виджетам игра даёт
  `add_ui_source`, действия кнопок — `add_ui_action`. Экраны вне симуляции (без `AiId`, не в снапшоте); всё, что меняет мир,
  кнопка делает инструментом (`tool …`) или действием игры, а реплей повторяет само нажатие (`ui_click`, `ui_navigate`).
- **Звук — контент** (`*.sound.ron`, `*.music.ron`, микшер `*.audio.ron`): атака называет звук в `telegraph.sound`, игра
  играет свой `PlaySound`, слой музыки ждёт условие по имени (`add_ai_condition`). Звук только читает мир и не меняет его;
  то, что показывает лишь эта машина, журнал пишет видом `JournalKind::Local`, а не `Event` (хост шлёт клиентам события).
- **Новый модуль движка** — крейт с плагином, который называет себя `add_engine_module(name, version)` (так он виден в
  `ai/status`), с наборами систем для упорядочивания и одной строкой в `EnginePlugins`. Состояние он описывает для ИИ
  (describe, `journal_event`), действия отдаёт инструментами (то, что разработчику нужно под рукой в окне, — ещё и пунктом
  меню разработчика `add_dev_entry`, который зовёт тот же инструмент), числа берёт из контента; у него юнит-тесты, сценарий в
  песочнице, реплей и раздел README.
- **Мир, регионы и уровни (фаза B).** Всё, что геймплей берёт из тайлов, комнат и уровней (коллайдеры, маркеры, префабы,
  точки интереса), — сущности **верхнего уровня с мировым `Transform`**: так их читает физика и так их создают
  `spawn_region` и `spawn_level`. Регион меняют инструментами (`regen`, `region_load`) или `change_region`, через
  `EngineState::RegionLoading`, внутри одного вызова; руками его не спавнят и не удаляют. Стартовый регион
  (`start_region`) строится до `OnEnter(InGame)`, его `AiId` идут первыми.
- **Генерация — чистая функция (seed, версия генератора, контент).** Случайность шагов — `GenContext::rng()` (`DetRng`),
  никогда не `SimRng`, не часы и не порядок `HashMap`/`HashSet` (порядок — по индексам и `BTreeMap`). Изменили результат
  генератора (шаги, веса, правила решателя) — поднимите `version` региона: старые зёрна дадут новые миры, сохранённые манифесты
  останутся. Пересобрать эталоны: `--worldgen-report` на 1000 зёрен, `record_reference_replays`, хэш манифеста в
  `region.scenario`.
- **Навсетка.** Запечённые тайлы лежат в `assets/nav/` (`cargo sandbox --bake-nav`, tool `nav_bake`) и коммитятся; с
  определением `nav` их проверяет `--check-content` (устаревший файл — ошибка с командой). Поменяли то, чем запекается тайл
  (зонд земли и правило шага в `shards_motion`, сэмплирование в `shards_npc::nav`), — поднимите `BAKE_VERSION` и запеките всё.
  Правка тайла, его файла уровня или префаба геометрии — запечь заново.
- **NPC действуют только через `Intent`** (как игрок и агент): мозг NPC движка пишет в него в `NpcSystems::Drive`, а не двигает
  тело. Поведение, восприятие, директор и планы атак — контент (`*.behavior.ron`, `*.perception.ron`, `*.director.ron`,
  `*.attack_plan.ron`); свою логику добавляют как `add_npc_condition` / `add_npc_state` по имени, а не копией мозга. Состояние NPC
  живёт в его компонентах (они входят в реплей и снапшот), не в ресурсах.
- **Состояние мира в снапшоте.** Новое состояние региона, уровня или навигации вне сущностей с `AiId` — `add_snapshot_resource`;
  то, что строится из сохранённого (регион из манифеста, навсетка), — `add_snapshot_prepare` / `add_snapshot_rebuild`; то,
  вокруг чего построен мир, — `add_snapshot_check`. Покройте его тестом по образцу `crates/sandbox/tests/sandbox/snapshots.rs`
  и `crates/shards_world/tests/world/regions.rs` (снапшот в регионе, загруженный в игру без него).
- **Сторонний крейт** только после проверки, что он поддерживает Bevy 0.20 и жив; версия закрепляется точно (`=x.y.z` в
  `[workspace.dependencies]`), а сам крейт прячется за API движка: его типов нет в публичном API, игра его не импортирует.
  Так физика (сейчас `parry3d`) меняется, а Bevy обновляется без правок игры.

## Если игра упала

Паника в системе геймплея не убивает игру. `step` останавливается на тике сбоя и отвечает
`stopped: gameplay system <name> panicked at tick N: <msg>`, в реальном времени игра встаёт на паузу,
в журнале — `system <name> panicked at tick N`. Сессия уже сохранена в `replays/crash-tick<N>.json`:

```bash
cargo sandbox --replay replays/crash-tick<N>.json --headless --paused &   # тот же мир с тика 0
cargo shards wait && cargo shards step <N>     # сбой повторится на тике N: смотрите inspect/journal
cargo sandbox --replay replays/crash-tick<N>.json --verify   # код 1, пока система падает
```

После исправления `--verify` больше не пишет `gameplay system … panicked`. Код 1 с `DIVERGED` тогда
значит лишь, что исправленная система меняет мир иначе, чем упавшая.

Нарушенный инвариант останавливает игру так же: `stopped: invariant <name> violated at tick N`, в
журнале — текст нарушения, реплей — `replays/invariant-<name>-tick<N>.json`. Правила `assert_*`
записаны в нём, поэтому проигрывание нарушит правило на том же тике, а `--verify` перечислит
нарушения и выйдет с кодом 1, пока ошибка не исправлена.

Вместе с краш-реплеем пишется отчёт о падении `crashes/crash-<время>-tick<N>.json` (`[sim] crash report saved: …`; если в рабочую
папку писать нельзя, он в папке игрока, например `%LOCALAPPDATA%\<программа>\crashes`, и путь называет та же строка): место и
стек паники, игра и машина (ОС, процессор, память, видеокарта), `ai/status`, последние строки журнала и лога и тот же реплей.
Его пишут и паника в другом потоке (поток рендера, задача), и `shards::run`, когда паника уносит игру (код выхода 101,
`error: the game crashed: …; crash report: …`; в таком отчёте реплея нет), и инструмент `crash_report note="…"` по запросу.
Отчёт с реплеем, который прислал игрок, проигрывается как реплей: `--replay crashes/crash-….json` (на отчёт без реплея —
`… is a crash report without a replay (<почему>)`). Ловите панику сами и идёте дальше —
зовите `ai_core::crash::handled(&message)`, иначе о ней будет отчёт (README, «Отчёт о падении»).

## Проверка изменений

- `cargo test --workspace`: юнит-тесты и сценарии в `crates/sandbox/tests/`; `cargo clippy --workspace --all-targets
  -- -D warnings` должен молчать. Менялось что-то, что видит шаблон, — `cargo test --manifest-path template/Cargo.toml`.
- Поведение, которое можно описать командами `shards`, оформляйте файлом сценария в `<crate>/scenarios/`
  (`*.scenario`, формат в README, «Сценарии»). `tests/scenario_files.rs` песочницы гоняет каждый файл
  в процессе (вперёд, потом назад в одной игре), `shards scenario` — против живой игры; нужен файл, который идёт и на свежей игре, и после
  других: начинайте его с `heal`/`teleport`/`set`/`npc_set_state … calm`/`regen`, возвращающих мир к исходному.
- Контент: `cargo sandbox --check-content` (или `cargo run -- --check-content` в шаблоне) находит битые ссылки и
  называет файл; он читает и файлы уровней (узлы, компоненты, маркеры), и запечённую навигацию. Мир: `cargo sandbox
  --worldgen-report wilds --seeds 1..1000` (в шаблоне `valley`) должен вернуть 0; CI гоняет оба.
- Новый компонент или ресурс состояния проверяйте снапшотом: `crates/sandbox/tests/sandbox/snapshots.rs` берёт
  снапшот в тик N, догоняет до M и сравнивает с игрой, загрузившей снапшот (одно состояние, один журнал).
- Изменение геймплея проверяйте реплеем: `cargo sandbox --replay FILE --verify` называет первую
  разошедшуюся проверку, сущности и компоненты, а ниже — их значения в этом прогоне (код выхода 1);
  `--verify --windowed` (под `xvfb-run -a`) — то же в окне:
  `DIVERGED at tick 60: e6 Boss: Health differs`, `  e6 now: Health { current: 390.0, max: 600.0 }`.
  Если расхождение ожидаемое, запишите реплей заново.
- По умолчанию проверки идут раз в 60 тиков, и тик в отчёте — первая проверка после расхождения. Для поиска
  бага пишите реплей с `--check-every 1` (`cargo sandbox --headless --paused --check-every 1`): тогда
  отчёт называет точный тик. Значение хранится в файле, при проигрывании флаг не нужен.
- Скорость тика проверяйте релизной сборкой: отладочная компилирует свой код с `opt-level = 1`.
  `cargo run -p sandbox --release -- --headless --paused --tick-budget-ms 5`, затем `shards step 600`:
  строка `ticks:` назовёт среднее, худший тик и число тиков за бюджетом. Профайлер Tracy включается фичей
  `--features trace_tracy` (см. README, «Время тика»).
- Эталонные реплеи `tests/replays/*.json` (полоса препятствий, бой-прототип, директор жетонов, страж на навсетке, смена
  регионов и реплей со снапшота песочницы) проверяет `cargo test`. После намеренной правки геймплея или контента перезапишите их:
  `cargo test -p sandbox --test sandbox -- --ignored record_reference_replays` (у шаблона —
  `cargo test --test game record_reference_replays -- --ignored` в `template/`).
- Изменили поведение модуля — обновите его раздел в README, `docs/engine-progress.md` (статусы) и `CHANGELOG.md`
  (`Unreleased`).
