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
  `shards_action`, `ai_core`, `shards_script`, `shards_cli`, `sandbox` (арена движка). Карта — в README, «Архитектура».

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
cargo shards load-snapshot snapshots/tick120.json   # вернуться к нему; реплей дальше идёт с этого снапшота
cargo shards scenario FILE...           # файл сценария: команды, ожидания, инварианты (README, «Сценарии»)
cargo shards content check              # контент запущенной игры; без игры: cargo sandbox --check-content
cargo sandbox --replay FILE --verify    # реплей без окна; добавьте --windowed (под xvfb-run -a) для окна
kill <pid>                              # pid из `wait`
```

- `shards` запускайте из папки игры или ниже: там лежит токен `.shards/<port>.token`, без него игра
  отвечает `unauthorized`. Из другого места передайте его в `SHARDS_TOKEN`.
- Останавливайте игру `kill` (или Ctrl+C), не `kill -9`: так она удаляет токен и `--port-file`.
- Собранный бинарник запускайте через `cargo sandbox` (или с `BEVY_ASSET_ROOT=crates/sandbox`): иначе он не найдёт
  `assets/`, и мир окажется пустым.
- Файлы игра пишет только к себе: реплеи в `replays/`, снимки экрана в `shots/`, снапшоты состояния в `snapshots/`
  (`save-replay replays/x.json`, `shot -o shots/x.png`, `save-snapshot snapshots/x.json`). Абсолютные пути и `..`
  не принимаются.
- Числа и позиции берите из `observe` и `inspect`, не из картинок. `shards shot` (нужно окно, можно под Xvfb) —
  для визуальных багов: PNG с метками `eN` и легенда, где на картинке какой id.
- Ошибки инструментов объясняют, как исправить вызов. Читайте их, а не повторяйте вызов вслепую.
- Инструменты движка (`shards tools`): **движение** — `move_to` (`path=true` ведёт по `path_to`), `move`, `stop`, `jump`,
  `sprint`, `glide`, запросы `clearance` и `path_to`, `input_bindings`, `rebind`; **бой** — `light_attack`, `heavy_attack`,
  `block`, `parry`, `dodge`, `use_item`, `use_technique`, `lock_on`, `attack`, `god`, `heal`, `stagger`; **контент** —
  `content_list`, `content_get`, `content_check`, `content_reload`, `spawn_prefab`. Свои у игры (в песочнице условия
  `boss_phase_2`, `boss_dead`, `player_dead`).
- Инструменты боя (`dodge`, `light_attack`, `block`, `use_item`, `attack`) отвечают по правилам игры сразу: `ok: roll (W)`,
  `ok: queued, heavy starts in 0.10s` (вход ждёт в буфере) или ошибка с причиной и временем (`not enough stamina: 10/20
  (ready in 0.2s)`, `busy: windup heavy 0.50s (ready in 0.90s, when a cancel window opens)`). На ошибку сделайте `step`
  на названное время и повторите. Входы мимо инструментов, у которых вышло время, видны в журнале: `e5 dodge rejected: …`.
- Предупреждения врагов приходят в журнал заранее (`e6 winds up slam (0.50s) ⚠ circle 4m`), а `observe`
  под строкой `YOU` пишет, куда выйти из зоны: `step --until "journal:winds up"`, потом `dodge`.
- `warn!`/`error!` игры попадают в журнал с тиком, на котором записаны, даже внутри длинного `step`:
  `step 600 --until journal:TEXT` ловит и их. За тик — не больше 64 строк, об остальных говорит
  строка `N log lines dropped`.
- Несколько игр параллельно: `--port` у каждого инстанса, `SHARDS_PORT` у CLI. Проще так:
  `cargo sandbox --headless --paused --port 0 --port-file F` и `SHARDS_PORT_FILE=F cargo shards wait`.
  На занятом порту игра не стартует (код выхода 3). `shards wait` печатает pid, seed и порт ответившей
  игры: проверьте, что это ваша.
- Проверить, как игра играется честно: `shards session --scope player`. Тогда доступны только
  инструменты игрока, и они действуют через `Intent`, как человек.

## Правила кода

- Код и комментарии на английском, документация (README, этот файл, `docs/`) на русском.
- Bevy закреплён на 0.20.0; не обновляйте его попутно с другой задачей.
- **Граница.** Игра (шаблон, песочница) зависит только от крейта `shards` и не меняет код движка. В движке нет имён
  из конкретной игры: если механизм пригодится второй игре с другим сюжетом, он в движке, если это конкретное
  «что» (оружие, враг, число), оно в игре, то есть в контенте.
- **Контент.** Числа и определения — файлы `assets/content/**/*.<вид>.ron` (горячая перезагрузка), а не константы в коде.
  Песочница тоже: её мир, префабы, атаки, мувсеты и движение лежат в `crates/sandbox/assets/content/`. Новый вид контента —
  структура с `ContentDef` и `add_content`; ссылки на другие определения проверяются в `validate` (`refs.require`).
- **Мир создавайте в `OnEnter(EngineState::InGame)`**, не в `Startup`: контент есть только после загрузки. Порядок
  спавна одинаков в каждом запуске, иначе `AiId` и `ColliderId` разойдутся.
- Весь геймплей — в `FixedUpdate`, в одном из `SimSet`; случайность только через `SimRng`.
  Тест `gameplay_systems_have_a_total_order` ловит системы без явного порядка. Свои системы ставьте «до» и «после»
  наборов модулей (`CoreSystems`, `MotionSystems`, `ControllerSystems`, `ActionSystems`, `PhysicsSystems`).
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
- Новый инструмент — структура аргументов с doc-комментариями (они становятся описанием схемы)
  и Bevy-система `In<Call<Args>> -> ToolResult`. Вид: `query` читает, `action` меняет мир
  (журнал, дифф, реплей), `control` управляет временем и сессией. Ошибка инструмента говорит, как исправить вызов.
- ИИ видит только сущности с `AiVisible` (его даёт `AiKind`). Что о них сказано, решают
  describe-функции (`register_ai_component`, `register_ai_describer`).
- `AiId` назначает движок. Не пишите его на месте (`get_mut`, `set`, BRP `world.mutate_components`):
  индекс «id → сущность» ведут хуки `on_insert` и `on_discard`. Вставка нового `AiId` перенумеровывает
  сущность, а старый id получает надгробие `renumbered to eN`.
- **Новый модуль движка** — крейт с плагином, который называет себя `add_engine_module(name, version)` (так он виден в
  `ai/status`), с наборами систем для упорядочивания и одной строкой в `EnginePlugins`. Состояние он описывает для ИИ
  (describe, `journal_event`), действия отдаёт инструментами, числа берёт из контента; у него юнит-тесты, сценарий в
  песочнице, реплей и раздел README.
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

## Проверка изменений

- `cargo test --workspace`: юнит-тесты и сценарии в `crates/sandbox/tests/`; `cargo clippy --workspace --all-targets
  -- -D warnings` должен молчать. Менялось что-то, что видит шаблон, — `cargo test --manifest-path template/Cargo.toml`.
- Поведение, которое можно описать командами `shards`, оформляйте файлом сценария в `<crate>/scenarios/`
  (`*.scenario`, формат в README, «Сценарии»). `tests/scenario_files.rs` песочницы гоняет каждый файл
  в процессе, `shards scenario` — против живой игры; нужен файл, который идёт и на свежей игре, и после
  других: начинайте его с `heal`/`teleport`/`set`, возвращающих мир к исходному.
- Контент: `cargo sandbox --check-content` (или `cargo run -- --check-content` в шаблоне) находит битые ссылки и
  называет файл.
- Новый компонент или ресурс состояния проверяйте снапшотом: `crates/sandbox/tests/sandbox/snapshots.rs` берёт
  снапшот в тик N, догоняет до M и сравнивает с игрой, загрузившей снапшот (одно состояние, один журнал).
- Изменение геймплея проверяйте реплеем: `cargo sandbox --replay FILE --verify` называет первый тик,
  где игра разошлась с записью (код выхода 1); `--verify --windowed` (под `xvfb-run -a`) — то же в окне.
  Если расхождение ожидаемое, запишите реплей заново.
- Эталонные реплеи `tests/replays/*.json` (полоса препятствий и бой-прототип песочницы) проверяет
  `cargo test`. После намеренной правки геймплея или контента перезапишите их:
  `cargo test -p sandbox --test sandbox -- --ignored record_reference_replays` (у шаблона —
  `cargo test --test game record_reference_replays -- --ignored` в `template/`).
- Изменили поведение модуля — обновите его раздел в README, `docs/engine-progress.md` (статусы) и `CHANGELOG.md`
  (`Unreleased`).
