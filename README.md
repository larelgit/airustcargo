# Shards: игровой движок с ИИ-интеграцией (Rust + Bevy 0.20)

Shards — набор крейтов, на котором отдельный репозиторий собирает игру, не трогая код движка. Игра зависит от
одного крейта, `shards`, и получает детерминированный тик, физику и контроллер персонажа, ввод и камеру, бой на
данных, базу контента, уровни из Blender, сгенерированные регионы, навигацию и врагов-NPC и слой ИИ. Механизмы лежат в движке, содержание (оружие, враги, числа, сюжет) — в игре; в
коде движка нет имён из конкретной игры. Цель — движок под «Осколки Предела»: ТЗ в [docs/engine-tz.md](docs/engine-tz.md),
сверка с ним (что сделано, что впереди) в [docs/engine-progress.md](docs/engine-progress.md).

Движок сделан так, чтобы игру можно было делать вместе с внешними агентами-кодерами: Claude Code, Codex и им
подобными. ИИ не разглядывает пиксели. Движок и так знает всё, что есть в кадре, поэтому он **описывает мир
структурированным текстом** и даёт **типизированный API действий**. Одинаковый seed и одинаковые входы дают
одинаковую игру, поэтому любой сеанс записывается реплеем и проверяет новую сборку.

**Внутри движка LLM нет**: ни встроенного агента, ни локальной модели, ни API-ключей, ни вызовов к моделям. Игра —
среда. Агент запускает её, смотрит и действует из терминала через CLI `shards` (JSON-RPC методы `ai/*` поверх Bevy
Remote Protocol на 127.0.0.1), без MCP-прослоек. Тем же CLI пользуются человек, скрипты и CI. Инструкции для
агентов — в [AGENTS.md](AGENTS.md), своя игра по шагам — ниже и в [template/README.md](template/README.md),
версии и подключение по тегу — в [CHANGELOG.md](CHANGELOG.md).

![sandbox: страж замахивается, на земле красная зона удара с символом](docs/sandbox.png)

## Содержание

1. [Быстрый старт](#быстрый-старт)
2. [Как сделать игру на Shards: по шагам](#как-сделать-игру-на-shards-по-шагам)
3. [Архитектура](#архитектура): [крейты](#крейты) · [порядок тика](#порядок-тика) · [граница движка и игры](#граница-движка-и-игры)
4. [Ядро (E1)](#ядро-e1)
5. [Контент (E4)](#контент-e4)
6. [Движение (E2)](#движение-e2)
7. [Действия и бой (E3)](#действия-и-бой-e3)
8. [Уровни из Blender (E4.2)](#уровни-из-blender-e42)
9. [Мир и генерация (E5)](#мир-и-генерация-e5)
10. [Навигация и NPC (E6)](#навигация-и-npc-e6)
11. [Слой ИИ](#слой-ии): [что видит ИИ](#что-видит-ии) · [инструменты](#инструменты) · [время и детерминизм](#время-и-детерминизм) · [задел под сеть](#задел-под-сеть-фаза-c) · [транспорты и CLI](#транспорты-и-cli) · [скриншоты](#скриншоты) · [агенты](#агенты)
12. [Сценарии (E11)](#сценарии-e11)
13. [Инварианты (E11)](#инварианты-e11)
14. [Время тика (E11)](#время-тика-e11)
15. [Реплеи](#реплеи)
16. [Снапшоты (E11.4)](#снапшоты-e114)
17. [Сохранения (E8.1–5)](#сохранения-e815)
18. [Песочница](#песочница)
19. [Тесты и CI](#тесты-и-ci)
20. [Что дальше](#что-дальше)
21. [Соглашения и сборка](#соглашения-и-сборка)

## Быстрый старт

```bash
# 1. Системные пакеты (один раз; нужен sudo):
sudo apt install pkg-config libasound2-dev libudev-dev
# (+ libwayland-dev libxkbcommon-dev, если нужен нативный Wayland: --features wayland)

# 2. Игра без окна, на паузе: время идёт только по `step`. Свободный порт и файл с его номером:
cargo sandbox --headless --paused --port 0 --port-file /tmp/shards.port &
export SHARDS_PORT_FILE=/tmp/shards.port

# 3. Из того же или другого терминала:
cargo shards wait                          # ready: tick 0 · pid 4242 · seed 42 · port 40817
cargo shards observe                       # срез мира текстом + журнал с прошлого раза
cargo shards tools                         # что можно сделать (--json: со схемами аргументов)
cargo shards act attack target=e6 repeat=true
cargo shards step --until journal:winds    # в ответе CHANGES и JOURNAL
cargo shards content check                 # ok: 70 definitions in 23 files
cargo shards act regen region=wilds seed=42   # сгенерированный регион к югу от арены; игрок у его ворот
cargo shards act nav_path to='{"x": 24, "z": 140}'   # маршрут к логову по навсетке: path 94.5m, 4 turns: 33.9m SE → …
cargo shards worldgen --region wilds --seeds 1..1000 --report   # проверка генератора на тысяче зёрен
cargo shards scenario crates/sandbox/scenarios/*.scenario
cargo shards save-replay                   # сессия с тика 0 → replays/tick<T>.json
cargo shards save-snapshot                 # состояние на этом тике → snapshots/tick<T>.json (load-snapshot — назад)
kill 4242                                  # pid из `wait`; не kill -9: игра удалит токен и файл порта

# 4. Окно с игрой и снимок с метками eN (нужен дисплей и GPU; без дисплея — через xvfb-run, см. «Скриншоты»):
cargo sandbox
cargo shards shot

# 5. Реплей: seed и частота тиков берутся из файла.
cargo sandbox --replay replays/tick332.json
cargo sandbox --replay tests/replays/combat_prototype.json --verify
# replay finished at tick 2307: identical to the recording (39 checks) [2307 ticks in 3.46s]
xvfb-run -a cargo sandbox --replay tests/replays/combat_prototype.json --verify --windowed   # то же в окне

# 6. Контент без игры: читает и проверяет все файлы (в том числе файлы уровней тайлов и запечённую навигацию), код 0 или 1.
cargo sandbox --check-content
# ok: 70 definitions in 23 files

# 7. Мир без игры: генератор на тысяче зёрен (код 0, если прошли все) и запекание навигации тайлов.
cargo sandbox --worldgen-report wilds --seeds 1..1000
# ok: worldgen wilds v1, seeds 1..1000: 1000/1000 passed (42 after a retry), 0 fell back, 0 failed, 0 panicked [1000 seeds in 0.31s, 0.3 ms/seed]
cargo sandbox --bake-nav               # baked 9 tiles into nav/ (после правки тайла; без правок файлы не меняются)
```

`cargo sandbox` — это `cargo run -p sandbox --`, `cargo shards` — `cargo run -q -p shards_cli --` (алиасы в
`.cargo/config.toml`); все флаги после `--` уходят в `shards::run` или в CLI. Если запустить собранный бинарник
напрямую (`target/debug/sandbox`), игра ищет `assets/` рядом с исполняемым файлом: укажите `BEVY_ASSET_ROOT=$PWD/crates/sandbox`
(абсолютный путь: относительный не принимает наблюдатель за файлами).

**Rust** ставится через rustup, он добавляет `~/.cargo/bin` в PATH; версия берётся из `rust-toolchain.toml`. В уже
открытых терминалах выполните `source ~/.cargo/env` или откройте новый.

**Пока нет sudo**, сборка работает через локальный обходной путь. В `~/.local/share/rustengine-sysdeps/` распакованы
`pkgconf`, `libasound2-dev` и `libudev-dev`, а также лежит shim `pkg-config`. Перед любой командой `cargo
build/test/run` выполните `source ~/.local/share/rustengine-sysdeps/env.sh`. Нужен он потому, что `bevy_remote` тянет
цепочку `bevy_dev_tools` → `bevy_audio` → `alsa-sys`, а `alsa-sys` ищет `alsa.pc` через pkg-config. После `apt install`
обходной путь не нужен.

Управление в окне — карта ввода движка (`InputMap::standard`, переназначается инструментом `rebind`): WASD или
стрелки — ходить, мышь — камера, Space — прыжок, Shift — спринт, F — планер (держать), Ctrl — перекат, ЛКМ — лёгкий
удар (комбо), ПКМ — тяжёлый, Q — блок (держать), C — фляга, Tab — захват цели, E — «взаимодействие»; геймпад тоже.
Клавиши разработчика в песочнице: P — пауза, N или `.` — один тик на паузе, F1 — оверлей с id, F2 — оверлей боя
(движка), F3 — коллайдеры, F4 — вылечить и воскресить, F12 — снимок с метками в `shots/`. Клавиатура человека и
инструменты агента пишут в один и тот же `Intent`, и нажатия идут через тот же буфер ввода: удар или перекат, который
не удалось начать за время буфера, виден в журнале как `rejected`.

## Как сделать игру на Shards: по шагам

Игра — отдельный репозиторий с одной зависимостью, `shards`. Стартовая точка — [`template/`](template/README.md):
минимальная игра (герой, враг, арена, числа в контенте, свой инструмент, сценарий, реплей, тесты). Ниже шаги по
порядку; каждый проверен на копии шаблона (переименование, `cargo test`, запуск без окна, `shards scenario`, перезапись
реплея). Подробности каждого шага, таблицы файлов и заглушки на будущие модули — в [template/README.md](template/README.md).

1. **Скопируйте шаблон и переименуйте пакет.**
   ```bash
   cp -r template ~/games/pit && cd ~/games/pit && git init
   grep -rIl my_game . | grep -v Cargo.lock | xargs sed -i 's/my_game/pit/g'   # пакет `pit`; Cargo.lock правится сам
   ```
2. **Подключите движок по тегу.** В `Cargo.toml` вместо пути (`../crates/shards`) — тег релиза; версии и тег описаны в
   [CHANGELOG.md](CHANGELOG.md). Сборка не меняется, пока вы сами не поменяете тег.
   ```toml
   [dependencies]
   shards = { git = "https://github.com/larelgit/airustcargo", tag = "v0.2.0", default-features = false }
   ```
   Пока тег не поставлен мейнтейнером, закрепите коммит: `rev = "<хэш>"`. `default-features = false` нужен, чтобы
   features (`render`, `remote`, `hot_reload`, `audio`, `gamepad`, …) включала игра в своём `[features]`. Других
   зависимостей нет: Bevy, serde и schemars приходят через `shards` (`shards::prelude::*`, `shards::serde`,
   `shards::schemars`). CLI ставится той же версии: `cargo install shards_cli --git … --tag v0.2.0`.
3. **`main` — одна строка.** `shards::run` читает командную строку (`--headless --seed --port --port-file --no-remote
   --paused --hz --replay --verify --windowed --check-content --worldgen-report --seeds --bake-nav`), ловит `kill` и закрытый
   терминал:
   ```rust
   fn main() -> std::process::ExitCode {
       shards::run(pit::build_app)
   }
   ```
4. **`build_app` собирает движок и игру.** `EnginePlugins::new(config)` — весь движок из одного `EngineConfig`
   (`config` уже заполнен командной строкой), поверх него плагины игры. Всё, что только для окна (меши, свет,
   камера), — отдельный плагин под feature `render`.
   ```rust
   pub fn build_app(config: &EngineConfig) -> App {
       let mut app = App::new();
       app.add_plugins(EnginePlugins::new(EngineConfig { title: "My game".into(), ..config.clone() }))
           .add_plugins(GamePlugin);
       if !config.headless && !config.testing {
           app.add_plugins(visuals::VisualsPlugin);
       }
       app
   }
   ```
   Для тестов — `shards::testing::test_app(|config| config.seed = 7, |app| { app.add_plugins(GamePlugin); })`: без
   окна, загруженная, на паузе на тике 0.
5. **Мир создавайте в `OnEnter(EngineState::InGame)`, не в `Startup`.** Пока контент не загружен, `LoadingGate` не
   пускает ни одного тика, поэтому загрузка не сдвигает тики; сущности спавнятся в одном и том же порядке, и `AiId`
   с `ColliderId` одинаковы в каждом запуске.
   ```rust
   app.add_systems(OnEnter(EngineState::InGame), spawn_arena);
   ```
6. **Числа и определения — файлы контента** `assets/content/**/*.<вид>.ron`, они меняются на лету. Своя структура с
   `ContentDef` и `app.add_content::<T>()` — свой вид (уровень, оружие, враг); `require_content` называет id, на которые
   опирается код. Проверка без игры: `cargo run -- --check-content`. Подробнее — «Контент (E4)».
7. **Персонаж — префаб.** Компоненты по именам: `CharacterController` (числа движения — id в `*.locomotion.ron`),
   `Fighter` (мувсет), `Health`, `Stamina`, `Poise`, `Team`; `PlayerId` и `LocalPlayer` — тот, кем управляет клавиатура и
   камера; `AiPlayer` — тело агента по умолчанию. Врага получают расширением (`extends: "grunt"`).
8. **Движение, ввод и камера** уже в движке. Клавиши — `InputMap::standard` или свой `*.input_map.ron` (id `default`),
   игрок переназначает их инструментом `rebind`; камера — `commands.spawn((Camera3d::default(), AiCamera,
   ThirdPersonCamera::default()))` в плагине для окна. Имя папки настроек — `InputSettings.app_name`.
9. **Боец с мувсетом — файлы.** `*.action.ron` (фазы, окна ударов, неуязвимость, отмены, смещение, стоимость),
   `*.moveset.ron` (какой ввод какое действие запускает, `stagger` и `death`), `*.combat_tuning.ron` (id `default`).
   Мозг врага — своя система в `SimSet::Think`, которая пишет приказ в `Intent` (`Order::Attack`): идти, смотреть и
   бить будет движок по тем же правилам, что у игрока (выносливость, замах, откат).
10. **Свой инструмент и своё условие для агента** (`src/ai.rs`): структура аргументов с doc-комментариями и система
    `In<Call<Args>> -> ToolResult`.
    ```rust
    app.add_ai_tool("taunt", Scope::Player, taunt)           // действие: журнал, CHANGES, реплей
       .add_ai_condition("arena_cleared", arena_cleared);    // `step --until named:arena_cleared`
    ```
11. **Сценарии и эталонные реплеи.** `scenarios/*.scenario` — команды `shards`, ожидания и инварианты; тот же файл идёт
    против живой игры (`shards scenario`) и в `cargo test`. Удачный сеанс сохраняют как эталон:
    ```bash
    shards save-replay replays/smoke.json            # из запущенной игры (или из теста записи)
    cargo test --test game record_reference_replays -- --ignored   # перезапись эталона шаблона: tests/replays/smoke.json
    cargo run -- --replay tests/replays/smoke.json --verify
    # replay finished at tick 258: identical to the recording (5 checks) [258 ticks in 0.05s]
    ```
12. **Тесты.** Один бинарник `tests/game`: сценарии на свежей игре, детерминизм (тот же seed — тот же журнал, `step
    300` равен 5 × `step 60`), реплеи, контент, регион, манифест (единственная зависимость — `shards`). Шаблон проходит 23
    теста за секунды.
13. **CI.** `cargo test --locked`, `cargo run --locked -- --check-content` (читает и файлы уровней тайлов, и запечённую
    навигацию), `--worldgen-report REGION --seeds 1..1000`, `--verify` каждого файла из `tests/replays/` (рабочий `ci.yml` —
    в «CI игры» шаблона и в `.github/workflows/ci.yml` движка).
14. **Новая версия движка.** Поменяйте `tag`, выполните `cargo update -p shards`, прочитайте раздел «Изменено» в
    `CHANGELOG.md`, прогоните тесты и реплеи; реплеи, записанные на старой версии, после изменения геймплея могут
    разойтись — тогда их записывают заново. Снапшоты другой минорной версии движка не грузятся (`force` пропускает проверку).
15. **Регион.** Тайлы с сокетами на краях (`*.tile.ron`), правила сокетов (`*.socket_rules.ron`), точки интереса
    (`*.poi.ron`) и сам регион (`*.region.ron`: размер, фиксированные клетки, точки, проверки, запасная карта `tile_map`,
    `origin`, `active_radius`): формат и все поля — в «Мир и генерация (E5)». Кода не нужно. Проверка без игры:
    ```bash
    cargo run -- --check-content                          # битый сокет, тайл или точка называют файл и поле
    cargo run -- --worldgen-report valley --seeds 1..1000   # код 0, если прошли все зёрна
    ```
    В игре регион появляется по `shards act regen region=valley seed=42` (экран загрузки, игрок у ворот), а
    `app.start_region("valley")` в плагине игры строит его, пока игра грузится. Меняете результат генератора — поднимите
    `version` региона.
16. **Враг-NPC.** Префаб бойца с компонентами `Npc` (поведение, восприятие, план атак) и `Home` (поводок), а поведение, зрение,
    план атак и директор жетонов — файлы `*.behavior.ron`, `*.perception.ron`, `*.attack_plan.ron`, `*.director.ron` в
    `assets/content/npc/`; точка интереса с `prefab` ставит его в лагерь региона. Свои условия и состояния поведения —
    `app.add_npc_condition(..)` и `app.add_npc_state(..)`. Пока нет навсетки, NPC ходят по локальной сетке; чтобы они ходили по
    навсетке региона, запеките тайлы (`cargo run -- --bake-nav`) и положите `assets/nav/` в репозиторий, а определение `nav`
    (`*.nav.ron`) заставит `--check-content` называть устаревший файл. Подробно — «Навигация и NPC (E6)».
17. **Уровень из Blender.** Объекты с custom properties, экспорт glTF с Custom Properties, файл в `assets/`, определение `level`
    (`*.level.ron`: файл и обязательные маркеры) или тайл с `geometry: Level(path: "tiles/ruin.gltf")`: «Уровни из Blender
    (E4.2)».

Правила, которые держат игру детерминированной (геймплей в `FixedUpdate` внутри `SimSet`, случайность только через
`SimRng`, порядок по `AiId`, управление только через `Intent`, `replay_input`/`replay_ignore`/`add_replay_check`), — в
разделе «Детерминизм: правила игры» [template/README.md](template/README.md). Мир добавляет своё: генерация — чистая функция
(seed, версия, контент) на `DetRng`, а не `SimRng`; регион меняют инструментами и `RegionLoading`, не руками; новое состояние
мира входит в снапшот (`add_snapshot_resource`, `add_snapshot_rebuild`, `add_snapshot_check`).

## Архитектура

```text
            игра (отдельный репозиторий)  ·  песочница (crates/sandbox) — тоже игра на `shards`
                               │ зависит только от
                               ▼
                     shards (зонтик: EnginePlugins, EngineConfig, run, testing, prelude, TileLevelsPlugin)
                               │
          ┌────────────────────┼─────────────────────┐
     shards_npc           shards_world           shards_level
     враги, навсетка      регионы, генератор,    уровни из Blender
                          активные зоны
          └────────────────────┼─────────────────────┘
        ┌────────────┬─────────┴──────┬──────────────┐
   shards_action  shards_motion  shards_content  shards_core
   бой на данных  физика, ввод,  база контента,   конфиг, состояния,
                  камера         префабы          Intent, шлюз загрузки
        └────────────┴─────────┬──────┴──────────────┘
                               │ подключён ко всем модулям
                            ai_core  ──►  CLI `shards`  ◄── агенты · CI · человек
                               │
           parry3d, gltf, ron, … — только внутри модулей, наружу не торчат
```

`shards_world` и `shards_level` друг друга не знают (их соединяет `shards::TileLevelsPlugin`); `shards_npc` стоит над обоими:
берёт у мира граф тайлов, активные зоны и событие «регион загружен», у уровней — файлы тайлов для запекания.

### Крейты

| крейт | что это |
|---|---|
| `crates/shards` | **единственная зависимость игры.** `EnginePlugins`, `EngineConfig`, `run` (стандартный `main`: ещё `--worldgen-report` и `--bake-nav`), `testing` (тестовые приложения), `TileLevelsPlugin` (уровни как тайлы), `prelude`, реэкспорт `bevy`, `serde`, `schemars`, `serde_json` и модулей (`shards::world`, `shards::npc`, …) |
| `crates/shards_core` | ядро (E1): `EngineConfig`, состояния, шлюз загрузки, `Intent`, `PlayerId`/`LocalPlayer`/`NetRole`, реестр модулей |
| `crates/shards_content` | контент (E4): `ContentDef`, база, загрузка и горячая перезагрузка, префабы, инструменты контента, `check_dir` |
| `crates/shards_motion` | движение (E2): физика на parry3d, контроллер, карта ввода, камера, инструменты и запросы движения |
| `crates/shards_action` | бой (E3): действия, мувсеты, машина, попадания, ресурсы, статусы, телеграфы, анимационный мост, оверлеи |
| `crates/shards_level` | уровни из Blender (E4.2): glTF с custom properties → коллайдеры, датчики, маркеры, префабы, компоненты; вид `level`, инструменты |
| `crates/shards_world` | мир (E5): тайлы, регионы и подземелья в контенте, генератор с решателем сокетов и проверками, манифест, пакетные отчёты, загрузка и смена региона, активные зоны, стриминг картинки |
| `crates/shards_save` | сохранения (E8.1–5): секции и реестр, слоты, конверт `shards-save/1`, миграции, атомарная запись с тремя копиями и восстановлением, `save_now` и автосохранение, инструменты `save_*` и `load_game`; секции движка (`region`, `position`, `vitals`) регистрирует зонтик |
| `crates/shards_npc` | враги (E6): автомат поведения, восприятие, режиссёр боя (жетоны), планировщик атак, навсетка по тайлам (`nav`: запекание, сшивка, маршруты), агенты и поводок |
| `crates/ai_core` | слой ИИ, не зависит от конкретной игры: id, срез мира, журнал, инструменты, время, реплеи, транспорт |
| `crates/shards_script` | без Bevy: формат сценариев, их запуск, разбор условий `--until`; общий для `shards` и тестов |
| `crates/shards_cli` | бинарник `shards`, тонкий клиент к `ai/*` |
| `crates/sandbox` | тестовая арена движка, собранная только на `shards`, как игра (см. «Песочница») |
| `template/` | минимальная игра для копирования: отдельный workspace с единственной зависимостью `shards` |

Модули `ai_core`:

| модуль | ответственность |
|---|---|
| `id` | `AiId` (`e42`): стабильный монотонный id, никогда не переиспользуется; надгробия для удалённых. Id назначает только движок: `set` и BRP `world.mutate_components` его не меняют, а новая вставка перенумеровывает сущность (надгробие `renumbered to e99`) |
| `describe` | только явно зарегистрированные компоненты видны ИИ, у каждого короткий describe; подсказки под строкой `YOU` (`register_ai_advice`) |
| `observe` | текстовый срез: YOU / ALERT / SEEN (по угрозе) / NOT IN VIEW / LANDMARKS / SINCE, видимость от глаз субъекта; `inspect`, `nearby`, диффы |
| `spatial` | позиции относительно игрока («8.4m NE»), line of sight (подменяемо: движение ставит лучи по физике, `AiOccluder` тоже работает) |
| `journal` | кольцевой буфер игровых событий (observers) плюс перехват `warn!`/`error!` из tracing: строка получает тик, на котором записана, и попадает в журнал после каждого тика; не больше 64 строк на тик |
| `tools` | реестр: args-struct + Bevy-система, JSON Schema (schemars), scope, `catch_unwind` |
| `builtin` | встроенные инструменты: observe, inspect, step, set/get через reflection, … |
| `sim` | тик, пауза, синхронный `step` по FixedMain, стоп-условия, остановка игры (`Halt`) на упавшей системе и нарушенном инварианте, `SimGate` (удержание тиков на время загрузки) |
| `invariants` | правила, которые проверяются после каждого тика: игровые (`add_ai_invariant`) и сессионные (`assert_add`); нарушение останавливает игру, как упавшая система |
| `replay` | запись входов и action-вызовов с тика 0, проигрывание, хэши состояния, отчёт о расхождении |
| `perf` | реальное время каждого тика против бюджета: строка `ticks:` в ответе `step`, записи `perf` в журнале, поле `perf` в `ai/status`; ни в хэше, ни в реплее |
| `rng` | детерминированный RNG: счётчик по (seed, tick, AiId, salt) |
| `scenario` | запуск файлов сценариев в процессе через `call_tool` (`run_file`, `run_str`) |
| `session` | агенты: имя, scope (Player/Dev), «тело», курсор журнала |
| `status` | поля `ai/status`; модули добавляют свои через `add_ai_status` |
| `failures` | паника системы геймплея останавливает игру, а не убивает её: очередь сбоев, краш-реплеи |
| `snapshot` | снапшоты: состояние симуляции в любой тик в файл и обратно, реплей со снапшота (см. «Снапшоты») |
| `paths` | файлы пишутся только в `replays/`, `shots/` и `snapshots/` игры |
| `remote` | методы `ai/*` для BRP (feature `remote`) |
| `highlight` | гизмо-подсветка и оверлей id поверх сцены (feature `render`) |
| `shot` | снимок окна с метками `eN` и легендой в пикселях картинки (feature `render`) |

### Порядок тика

Весь геймплей идёт в `FixedUpdate`, в одном из `SimSet`; системы модулей лежат в своих наборах, так что система игры
встаёт «до» или «после» них (`.before(ActionSystems::Hits)`) и порядок остаётся полным:

```text
FixedFirst       ai_core: тик +1, применить события реплея и вызовы между тиками
FixedUpdate
  PhysicsSystems::Sync            коллайдеры, сдвинутые между тиками (`teleport`, `set`)
  SimSet::Input    WorldSystems::Zones           ActiveZone / Dormant вокруг игроков (до приказов)
                   CoreSystems::FollowOrders     Order::MoveTo → Intent
                   ActionSystems::Orders         Order::Attack → Intent (после FollowOrders)
                   NpcSystems::Sense             кто активен; зрение, слух, память (после Orders и Zones)
                   NpcSystems::Decide            автомат поведения: переход, хук состояния
                   NpcSystems::Direct            жетоны атаки, роли ждущих
                   NpcSystems::Plan              планировщик: выбрать атаку, подойти, нажать
                   NpcSystems::Navigate          маршруты по навсетке, следование, уклонение, поводок
                   NpcSystems::Drive             что решил NPC → Intent
  SimSet::Think    мозги игры: решают, что делать, и пишут в Intent (видят NpcBrain и могут её дополнить)
  SimSet::Act      MotionSystems::Intent         Intent → LocomotionInput
                   ActionSystems::Machine        буфер ввода, запуск и ход действий (смещение, i-frames, блок)
                   ActionSystems::Effects        лечение, выносливость, статусы, эффекты игры
  SimSet::Move     ControllerSystems::Move       контроллер — единственный, кто двигает тела
  PhysicsSystems::SyncAfterMove                  новые позиции тел для попаданий
  SimSet::Resolve  ActionSystems::Hits           окна ударов × хёртбоксы → конвейер урона → события
                   ActionSystems::Fighters       спринт, восстановление, перезарядки, конец действий
                   ActionSystems::Statuses       накопление и срок статусов
                   ActionSystems::Deaths         боец с 0 здоровья умирает
  SimSet::Cleanup  CoreSystems::ClearPresses     нажатия, которые никто не взял, сбрасываются
FixedLast        ai_core: проверки реплея (хэш мира), записать входы, журнал, очередь сбоев
```

Между тиками (`PreUpdate`) клавиатура, мышь и геймпад пишут в `Intent`; камера работает в `PostUpdate` и ничего в
симуляции не читает. Смена региона и навсетка — не часть тика: `regen` и `region_load` — инструменты-действия, они идут между
тиками и внутри одного вызова делают весь переход `InGame → RegionLoading → InGame` (старый регион уходит, новый строится,
навсетка сшивается), так что ни один тик не видит полурегион и тик не сдвигается. Картинка тайлов (`stream`) идёт в `Update`.

Тест `gameplay_systems_have_a_total_order` ловит системы без явного порядка у песочницы, движка с движением и боевого модуля.

### Граница движка и игры

- Движок даёт механизмы и инструменты: детерминированный тик, физику, контроллер, систему действий, базу контента,
  слой ИИ. Игра даёт содержание и числа: оружие, атаки, врагов, боссов, тайлы, сюжет, баланс, внешний вид.
- Проверка: если механизм пригодится второй игре с другим сюжетом — он в движке; если это конкретное «что» — в игре.
- Игра зависит только от `shards` и не меняет код движка: свои плагины она добавляет поверх `EnginePlugins`. Это
  проверяет тест шаблона (`manifest.rs`).
- Модуль движка подключается одной строкой в `EnginePlugins` и называет себя в `ai/status`
  (`add_engine_module(name, version)`).
- Сторонний крейт выбирают только после проверки, поддерживает ли он Bevy 0.20 и жив ли проект; версию закрепляют, а сам
  крейт прячут за API движка. Так он заменяется, а Bevy обновляется без правок игры.

## Ядро (E1)

Крейты `shards` и `shards_core`. Всё, что нужно запустить игру: конфиг, состояния, шлюз загрузки, `Intent`.

**`EngineConfig`.** Одно значение собирает движок: `EnginePlugins::new(config)`. `shards::run` заполняет его командной
строкой, тесты берут `shards::testing::config()`.

| поле | что |
|---|---|
| `title`, `window` | заголовок и размер окна (1280×720) |
| `headless` | без окна, GPU и звука: CI, серверы, параллельные прогоны |
| `seed` | seed мира (42 по умолчанию): тот же seed и те же входы дают ту же игру |
| `port` | порт BRP для `shards` (`None` — без сервера) |
| `paused` | старт на паузе; тики идут по `shards step` |
| `tick_hz` | тиков в секунду (60) |
| `crash_replays` | сбой геймплея сохраняет `replays/crash-tick<N>.json` |
| `after_loading` | куда идёт `Loading` после загрузки (`InGame`; `MainMenu` для игры с меню) |
| `hot_reload` | применять файлы контента по мере правки (нужна cargo feature `hot_reload`; в тестах и `--verify` выключено) |
| `asset_dir` | папка ассетов; контент в `<asset_dir>/content/`. Относительно папки игры (`CARGO_MANIFEST_DIR` под cargo, иначе рядом с исполняемым файлом); `BEVY_ASSET_ROOT` перекрывает |
| `testing` | тестовое приложение: без окна, логгера, Ctrl+C и цикла; его ведут через `app.update()` и инструменты |
| `embedded` | игра рядом с другой в том же процессе (клиенты стенда): как `testing`, но со своим портом BRP, а её логи идут в её журнал (`testing::embedded_config(port)`) |
| `net` | `NetLaunch`: `Off`, `Host { port }` (`--host`, 7777 = `DEFAULT_NET_PORT`), `Join { addr }`, `Stand { clients, udp }` |
| `player_name`, `net_sim` | имя игрока для других (`--name`); условия сети для своих линков (`--net-sim latency=150,loss=0.02`) |
| `save_dir`, `character_slot`, `world_slot` | папка сохранений (`saves`); слоты персонажа и мира (`--character`, `--world`) |
| `settings` | `SettingsStore`: `Default` (папка настроек платформы), `Dir(путь)` (`--settings-dir`), `Off` (`--no-settings`, тесты) |
| `exit_with_host` | клиент выходит, когда хост завершил сессию, а не возвращается в меню |

**Окно и без окна — те же плагины.** Режим без окна — `EnginePlugins` без окна, GPU и звука: `WindowPlugin` без
окна, нет winit, `RenderPlugin` без бэкенда (плагины рендера остаются зарегистрированными), цикл на 60 Гц вместо цикла
событий. Работают трансформы, ассеты, сцены, состояния и физика, поэтому геймплей одинаковый. Это проверяет `--verify
--windowed`: реплей в окне даёт тот же отчёт, что без окна. Cargo features у `shards`, `sandbox` и шаблона: `render`
(окно, 3D, UI, клавиатура и мышь; без неё игра собирается только для `--headless`), `remote` (методы `ai/*`),
`hot_reload` (по умолчанию все три), `audio`, `gamepad`, `wayland`, `dynamic`.

**`shards::run`** — стандартный `main`. Флаги: `--headless --seed N --port N|0 --port-file F --no-remote --paused --hz N
--replay FILE --verify [--windowed] --check-content --worldgen-report REGION [--seeds S] --bake-nav [TILES]`; для фазы C:
`--host [PORT] | --join ADDR | --stand N [--stand-udp]` (взаимоисключающие, `--stand` только с `--headless`), `--name`,
`--net-sim SPEC`, `--save-dir DIR`, `--settings-dir DIR | --no-settings`, `--character SLOT`, `--world SLOT` (слот —
1–32 символа `a-z0-9_-`), `--exit-with-host`. Пока сети нет, `--host`, `--join` и `--stand` выходят с кодом 2
(`networking arrives with shards_net`). Занятый порт —
выход с кодом 3 до того, как собрано хоть что-то. `--verify` выходит с кодом 0 (совпало), 1 (разошлось или упала система
геймплея) или 2 (реплей или его снапшот не читаются); `--check-content`, `--worldgen-report` (все зёрна прошли) и `--bake-nav`
(все тайлы запеклись) — 0 или 1. Эти три режима ничего не запускают (`--bake-nav` грузит игру без окна, но не делает ни тика).
`kill` и закрытый терминал завершают игру через `AppExit`: токен и файл порта удаляются.

**Состояния** (`EngineState`, Bevy States): `Loading` → `InGame` (или `MainMenu`, если так задал `after_loading`),
`RegionLoading` — экран загрузки региона: через него идёт `regen` (см. «Мир и генерация»). Внутри `InGame` — `PauseState::{Running, Paused}`: игровая пауза,
в одиночной игре она останавливает и симуляцию; `shards pause` и `--paused` — инструмент разработчика и `PauseState` не
трогают. Игра вешает свои системы на `OnEnter`/`OnExit` этих состояний; переходы пишутся в журнал (`state Loading ->
InGame`).

**Шлюз загрузки.** Модули регистрируют, чего ждать (`app.require_loaded("content", check)`; контент регистрирует свои
файлы). Пока что-то не готово, `ai_core::SimGate` не пускает ни одного тика, ни в реальном времени, ни через `step`
(он отвечает `the game is still loading (content: 2 files left); wait with \`shards wait\``), поэтому загрузка не
сдвигает тики: тик 1 всегда идёт на одном и том же мире. `shards wait` ждёт конца загрузки.

**`Intent`** — единственный вход персонажа: клавиатура, геймпад, инструменты агента, мозги врагов, (позже) сеть и
реплей пишут в один компонент между тиками, а геймплей его читает.

| поле | что |
|---|---|
| `move_dir` | куда идти по земле, длина ≤ 1 (до 0.5 — шаг, 1 — бег) |
| `aim`, `target` | куда смотреть; цель захвата |
| `held` | кнопки, зажатые сейчас: `sprint`, `block`, `glide`, … |
| `pressed` | нажатия с прошлого тика (`Press { input, dir }`); их берёт геймплей, остальные сбрасываются в конце тика |
| `orders` | приказы на много тиков: `Order::MoveTo { target, within }`, `Order::Attack { target, input, repeat }` |

Имена ввода движка — в `shards::core::input_names` (`jump`, `sprint`, `glide`, `dodge`, `light`, `heavy`, `block`,
`parry`, `technique.N`, `use_item`, `interact`, `lock_on`); игра добавляет свои. `Intent` — вход реплея
(`replay_input::<Intent>()` движок делает сам): записываются изменения между тиками. `PlayerId` и `LocalPlayer`
отмечают игроков, `NetRole` (`Single` по умолчанию) — задел под сеть.

**Задел под сеть (фаза C).** Всё это в одиночной игре ничего не меняет.

- Клиент не симулирует: каждый `SimSet` идёт с `run_if(simulates)` (`NetRole::simulates()` — всё, кроме `Client`).
  На клиенте из `FixedUpdate` идут только `NetSet::Predict` (`run_if(predicts)`, своё тело с маркером `Predicted`) и
  системы вне `SimSet`. Их список — тест песочницы `net_seams`: сейчас это `sync_physics` дважды.
- `app.add_exit_hook(name, fn(&mut World))`: хуки выхода идут один раз, по порядку регистрации, когда записан
  `AppExit` (система в `Last`) или пришёл сигнал (`shards::run`).
- `SeedLock`: `hold_seed(world, reason)` держит шлюз загрузки под `seed`, `adopt_seed(world, seed)` пишет seed в
  `EngineConfig`, `SimSeed` и `SimConfig` и отпускает. Стартовый регион ждёт `seed_ready` и строится по `SimSeed`.
- Смена региона заканчивается событием `WorldRebuilt { reason: "region <id>" }` (как и загрузка снапшота).
- `player_body(world, n)` — тело с `PlayerId(n)`; через него агент, привязанный к игроку, находит своё тело.

**`ai/status`** показывает версию и модули: `engine: {version: "0.2.0", modules: [core, content, motion, action, level,
world, npc], state: "InGame", pause: "Running"}` и `loading: []`, пока ничего не грузится; с регионом в мире ещё поле `world`.

## Контент (E4)

Числа и определения игры лежат в файлах, не в коде: атаки, префабы, числа движения, уровень. Файлы правятся на лету.

**Виды и файлы.** Вид — Rust-структура с `ContentDef` (`KIND = "attack"`), зарегистрированная в `app.add_content::<T>()`.
Определения лежат в `<assets>/content/**/*.<вид>.ron`, в каждом файле одна карта «стабильный id → определение»:

```ron
// assets/content/combat/attacks.attack.ron
{
    "sword": (windup: 0.15, recover: 0.25, damage: 25.0),
    "claw": (windup: 0.45, recover: 0.6, damage: 8.0),
}
```

- Id — непустая строка без пробелов и управляющих символов; уникален внутри вида по всем файлам.
- Движок регистрирует свои виды: `prefab`, `locomotion`, `action`, `moveset`, `status`, `combat_tuning`, `animset`,
  `input_map`, уровни `level`, мир `tile`, `socket_rules`, `poi`, `region`, `tile_map`, `room`, `dungeon`, враги `behavior`,
  `perception`, `director`, `attack_plan`, `npc_tuning` и `nav`. Игра добавляет свои: у песочницы `layout`, у шаблона `arena`.
  `content_list` (инструмент) показывает виды и число определений.
- `#[serde(deny_unknown_fields)]` на структуре превращает опечатку в ошибку со строкой; без него опечатка в необязательном
  поле молча оставит значение по умолчанию.
- `require_content::<T>(&["yard"], "pit")` называет id, которые использует код: без него в контенте будет ошибка,
  а правка, которая его удаляет, откажет.

**Загрузка и проверка.** Папка `content` грузится через asset server, пока шлюз загрузки держит симуляцию; затем каждый
файл разбирается, и вся база проверяется целиком: схема, повторные id, ссылки между определениями (`refs.require::<ActionDef>(id,
"поле")`). Ошибки называют файл, строку, определение и поле, а при опечатке и ближайший id. Определения, которые
разобрались, работают всё равно. Настоящие сообщения (три опечатки в копии контента песочницы):

```text
$ BEVY_ASSET_ROOT=/tmp/broken target/debug/sandbox --check-content
content/characters/bodies.locomotion.ron:12:9: locomotion `player`: Unexpected field named `walk_spead` in `CharacterParams`, expected one of `radius`, `height`, `walk_speed`, … or `turn_speed_deg` instead
content/combat/fighters.moveset.ron: moveset `player`: inputs.light -> unknown action `light_one` (did you mean `light_1`?)
content/characters/characters.prefab.ron: prefab `skeleton`: unknown component `Helth` (did you mean `Health`?)
3 content errors in /tmp/broken/assets
```

Повторный id называет оба файла (`content/b.attack.ron: attack `sword`: already defined in content/a.attack.ron`), файл с
неверным видом — ближайший зарегистрированный (`no content kind `atack` (did you mean `attack`?); registered: …`). Файл,
который не разобрался, не даёт в базу ни одного определения, поэтому следом идут ошибки ссылок из других файлов:
причина — первая строка.

**Читать контент** в системах — `Content<T>` (`attacks.get("sword")`), вне систем — `ContentDb::get::<T>(id)`. Геймплей
контент не пишет.

**Горячая перезагрузка** (`hot_reload`, включена по умолчанию): правка, создание и удаление файла проверяются вместе
со всей базой. Чистая правка применяется инструментом `content_apply`, поэтому она записывается в реплей на своём тике и
проигрывание повторяет её; в журнале — `content reloaded: combat/player.action.ron (1 definition changed)`. Сломанная
правка остаётся в журнале ошибкой с файлом и строкой, прежняя версия действует:

```text
#7 2 [error] content: content/combat/player.action.ron:15:77: action `light_1`: Unexpected field named `dmage` in `HitDef`, expected one of `span`, `shape`, `at`, `damage`, … ; kept the previous version
```

Хэш базы входит в проверки реплея: запись, сделанная с другими числами, расходится на ближайшей проверке (так и надо,
эталоны записывают заново). Чем применяется на лету: числа движения и боя действуют со следующего тика; уровень и префабы
читаются при создании сущностей (новые сущности, например после `spawn_prefab`, видят правку, а уже созданные — нет).

**Префабы** — сущности в данных. Компоненты по коротким именам или полным путям, значения читаются через
reflection, как в сценах Bevy; каждый компонент должен иметь `#[derive(Reflect)]` и `#[reflect(Component)]`. Поля, которые
покрывает `Default` (с `#[reflect(Default)]`), можно не писать. `extends` начинает с другого префаба и заменяет компоненты целиком;
циклы — ошибка проверки.

```ron
// content/characters/monsters.prefab.ron
{
    "skeleton": (components: {
        "Name": "Skeleton", "AiKind": ("Skeleton"),
        "CharacterController": (params: "skeleton"), "Fighter": (moveset: "skeleton"),
        "Health": (current: 40.0, max: 40.0),
    }),
    "skeleton_captain": (extends: "skeleton", components: {"Health": (current: 90.0, max: 90.0)}),
}
```

В коде — `spawn_prefab(world, "skeleton", transform, None)`, у агента — инструмент `spawn_prefab`.

**Инструменты.** Игрок: `content_list`, `content_get`; разработчик: `content_check` (перечитывает все файлы с диска и
проверяет, ничего не применяя), `content_reload` (перечитать и применить, как горячая перезагрузка), `content_apply`
(применить определения из JSON; их записывает и проигрывает реплей), `spawn_prefab`:

```text
$ shards act content_list
content: 70 definitions in 23 files
  action  23
  animset  0
  attack_plan  3
  behavior  2
  combat_tuning  1
  director  2
  dungeon  0
  input_map  0
  layout  3
  level  1
  locomotion  3
  moveset  4
  nav  1
  npc_tuning  0
  perception  3
  poi  4
  prefab  7
  region  1
  room  0
  socket_rules  1
  status  0
  tile  9
  tile_map  1
$ shards act spawn_prefab id=skeleton x=5 z=5
spawned e15 skeleton at (5.0, 5.0)
CHANGES
  + e15 Skeleton  hp 40/40  poise 20/20  8.6m NE
```

**`--check-content` и `shards content check`.** Первый читает файлы без игры (для CI: код 0 или 1; отсутствующая папка
`content` — тоже ошибка, чтобы проверка не прошла впустую), второй спрашивает запущенную игру: `ok: 70 definitions in 23
files`. Модули добавляют к проверке своё: файлы уровней, названные определениями `level` (каждый узел, компонент и
обязательный маркер), файлы уровней, из которых построены тайлы (должны лежать в папке ассетов), и запечённая навигация
тайлов (с определением `nav` устаревший или отсутствующий файл — ошибка с командой `--bake-nav`).

Чего в контенте пока нет: проверки, что маркеры-сокеты файла уровня совпадают с сокетами тайла в его RON (у `shards_content`
нет связи между видами), и сцен BSN вместо RON (когда у Bevy появится загрузчик `.bsn`). Уровни из Blender — отдельный
раздел ниже.

## Движение (E2)

Крейт `shards_motion`: физика, контроллер персонажа, карта ввода, камера и инструменты движения. Состояние в describe
(`ground 3.6m/s`, `air ↑3.1m/s`, `water`, `glide ↓2.0m/s`, `mantle 40%`), события в журнале (`e5 climbs a 1.3m
ledge`, `e5 air → water`, `e5 reached goal`).

**Физика.** Кинематическая: ничего не симулирует силами, а отвечает на вопросы «что пересекает этот луч, где
остановится эта капсула, что лежит в этом объёме».

- `Collider` (форма: коробка, шар, капсула, цилиндр, конус; смещение; слои), `Body` (`Static` | `Kinematic`),
  `Volume` (датчик: вода, восходящий поток, именованный триггер) и события `VolumeEntered`/`VolumeExited`.
- Слои — битовая маска: `WORLD`, `CHARACTER`, `HURTBOX`, `HITBOX`, `VOLUME`, `PROJECTILE`; биты 16..32 — слои игры
  (`LayerMask::game(n)`).
- Запросы — `PhysicsWorld::{raycast, shapecast, overlaps, volumes_at}` и системный параметр `SpatialQuery`; работают из
  систем и из `&World` (инструменты ИИ). Попадание при старте внутри формы даёт расстояние 0.
- Детерминизм: каждый коллайдер получает `ColliderId` по порядку создания и никогда не переиспользует его; весь перебор и
  результаты идут по нему, а не по `Entity` (окно добавляет сущности рендера и сдвигает индексы `Entity`); при равенстве
  расстояний побеждает меньший id. Позиции читаются из `Transform`, а не из `GlobalTransform` (внутри фиксированного
  тика он не пересчитывается): коллайдеры кладите на сущности верхнего уровня. `Transform::scale` игнорируется: размер в
  `Shape`.
- Широкая фаза — линейный перебор с предфильтром по AABB (O(n) на запрос); BVH встанет за тем же API, когда миры вырастут.

**Почему parry3d, а не avian3d.** ТЗ называло кандидатом avian3d и требовало проверить поддержку Bevy 0.20, прежде чем брать
сторонний крейт. Проверка по индексу crates.io (13–14 октября 2026): avian3d 0.7.0 требует Bevy 0.19, а Bevy закреплён
на 0.20.0 (то же у bevy_enhanced_input 0.26.0 и lightyear 0.30.1). Для кинематического контроллера нужны запросы
столкновений, а не динамика, поэтому взят `parry3d =0.31.1` (геометрия dimforge, не зависит от Bevy; на ней стоит и avian).
Он закреплён, живёт внутри `shards_motion` и в публичный API не торчит: API говорит на glam (`Vec3`, `Quat`) и на своём
`Shape`. Вернуться к avian3d можно, когда он выйдет под Bevy 0.20, не трогая игру. По той же проверке карта ввода своя, а
не `bevy_enhanced_input`.

**Контроллер.** `CharacterController { params: "id" }` — капсула, которую двигает только он (действия вроде переката
отдают смещение ему же, и стена останавливает выпад). Режимы: земля (ходьба, бег, спринт, склоны до `max_slope_deg`,
ступеньки до `step_height`, прилипание вниз по лестнице), воздух (гравитация, управление в воздухе, прыжок с «койот-
временем» и буфером), планер (медленное падение, восходящие потоки), вода (плавание по поверхности, выход через
мелководье или прыжок), перелезание на уступ до `mantle_height` по заранее проверенному пути. Персонажи не мешают друг
другу при проходе: после движения всех, по порядку id, капсулы раздвигаются. Падение с высоты — событие `Landed {
fall_height }`, плюс `Jumped`, `Mantled`, `ModeChanged`. Позиция `Transform` — это ступни; запись в неё снаружи
(`teleport`, респаун) ставит персонажа заново, без скорости и падения.

**Параметры — контент.** Вид `locomotion`, карта id → числа; пропущенные поля берут значения по умолчанию, опечатка —
ошибка со строкой. Читаются каждый тик, поэтому правка файла меняет следующий шаг.

```ron
// assets/content/characters.locomotion.ron
{ "hero": (radius: 0.4, height: 1.8, walk_speed: 2.0, run_speed: 5.5, sprint_speed: 8.0, jump_height: 1.3) }
```

Остальные поля (`acceleration`, `deceleration`, `snap_distance`, `gravity`, `coyote_time`, `jump_buffer`, `air_control`,
`terminal_speed`, `mantle_height`, `mantle_time`, `swim_depth`, `float_depth`, `swim_speed`, `glide_fall_speed`,
`glide_speed`, `glide_accel`, `turn_speed_deg`) описаны в `shards_motion::CharacterParams`.

**Ввод.** Карта действий превращает клавиатуру, мышь и геймпад в `Intent` для сущности с `LocalPlayer`. Читается между
тиками (`PreUpdate`); `Intent` трогается, только когда что-то изменилось (реплей записывает каждое изменение).

- `InputMap { contexts }`: контекст (`gameplay`, `menu`, свои) → действия → привязки. Привязка: `Key`, `Mouse`,
  `GamepadButton`, `GamepadAxis` (мёртвая зона, масштаб), `KeyAxis`, `Keys2D` (WASD как 2D-ось), `Stick`, `MouseMotion`.
  Режим действия: `Press` (одно нажатие), `Hold` (пока зажато), `Toggle` (нажатие включает до следующего: «удержание или
  нажатие» для доступности). `directional: true` — нажатие несёт направление, куда идёт игрок (перекат туда, куда
  направлен стик). Слот 0 — основная привязка, слот 1 и дальше — запасные.
- Стек контекстов `InputContexts`: стреляет только верхний; `menu` блокирует `gameplay`, пока его не снимут.
- Умолчания — из контента (`*.input_map.ron`, id `default`, горячая перезагрузка) или встроенный
  `InputMap::standard`:
  ```ron
  // assets/content/controls.input_map.ron
  { "default": (contexts: [
      (name: "gameplay", actions: [
          (action: "move", bindings: [Keys2D(up: KeyW, down: KeyS, left: KeyA, right: KeyD), Stick(stick: Left)]),
          (action: "jump", bindings: [Key(Space), GamepadButton(South)]),
          (action: "sprint", bindings: [Key(ShiftLeft)], mode: Hold),
          (action: "dodge", bindings: [Key(ControlLeft)], directional: true),
      ]),
  ]) }
  ```
- Переназначение: `rebind` (разработчик) кладёт привязку в слот действия, меняет режим (`mode`) или снимает слот (`unbind`).
  Привязка, которую уже использует другое действие, отклоняется с именем этого действия, если не дать `force`:
  ```text
  $ shards act rebind action=jump binding='Key(KeyE)'
  error: Key(KeyE) is already bound to `interact` in `gameplay`; rebind `interact` first, or force it to take the input over
  $ shards act rebind action=interact binding='Key(KeyG)'
  ok: gameplay/interact: slot 0 = Key(KeyG)
  $ shards act rebind action=jump binding='Key(KeyE)'
  ok: gameplay/jump: slot 0 = Key(KeyE)
  $ shards act rebind action=sprint mode=Toggle
  ok: gameplay/sprint: mode toggle
  ```
- Сохранение: изменения игрока лежат отдельным файлом-переопределением `$XDG_CONFIG_HOME/<игра>/input.ron`
  (`~/.config/<игра>/input.ron`, `%APPDATA%` на Windows), поэтому новые умолчания доходят и до старых файлов. Имя игры —
  `InputSettings.app_name`. Окно читает и пишет файл, без окна и в тестах — нет (чужие файлы не трогаются; явно заданный
  `InputSettings.file` читается всегда). Файл, который не удалось прочитать, `rebind` не затирает. Сохранение на
  собственном файле, а не на настройках Bevy (`bevy_settings`) — до модуля сохранений и настроек (E8).
- `input_bindings` показывает, что чем управляется:
  ```text
  $ shards act input_bindings
  gameplay (active)
    move         Keys2D(up:KeyW,down:KeyS,left:KeyA,right:KeyD) · Keys2D(up:ArrowUp,…) · Stick(stick:Left,deadzone:0.15,scale:1.0)
    jump         Key(Space) · GamepadButton(South)
    sprint       [hold] Key(ShiftLeft) · GamepadButton(LeftThumb)
    …
  other contexts: menu (input_bindings context=NAME)
  ```
- Настоящий геймпад — feature `gamepad` (gilrs, нужен libudev). Без неё сущности `Gamepad` работают, если их создаёт игра
  или тест.

**Камера** (`ThirdPersonCamera`, только с окном; математика без окна). Орбита мышью и правым стиком; столкновение со
стенами (луч от головы к желаемой позиции подтягивает камеру); захват цели: клавиша `lock_on` выбирает цель ближе всего к
центру кадра в конусе и радиусе `CameraSettings::{lock_cone_deg, lock_range}`, сдвиг взгляда в сторону переключает на
соседа, цель выходит из радиуса, умирает или теряет `LockOnTarget` — захват снимается; помощь прицеливания, тряска
(`AddTrauma`; отключается настройкой), поле зрения. Выбранная цель пишется в `Intent.target` между тиками (это вход, его
записывает реплей), но чужую цель камера никогда не снимает. **Камера — только представление:** геймплей и `observe`
её не читают (`observe` видит глазами субъекта), поэтому окно с камерой играет ту же игру, что и без окна.

**Предупреждения об атаках за кадром** (E2.6): `threat_indicators(world, viewer, forward)` из модуля боя даёт по зонам атак
направление для индикатора; маркеры на краю экрана рисует `shards_action::render::hud` (см. «Действия и бой»).

**Инструменты движения** (игрок, только через `Intent`):

| инструмент | что делает |
|---|---|
| `move_to` | идти к сущности или точке `{x, z}`; `within`, `queue`; `path=true` ведёт по маршруту `path_to` |
| `move`, `stop` | пройти N метров по компасу (N = −Z, E = +X); остановиться |
| `jump`, `sprint`, `glide` | прыжок (из воды — на берег); спринт и планер как включённое состояние (`on=false` снимает) |
| `clearance` | запрос: как далеко в восемь сторон и что остановит |
| `path_to` | запрос: маршрут по тому, что можно пройти, проплыть и перелезть |
| `input_bindings` (игрок), `rebind` (разработчик) | управление |
| `physics_overlay` (разработчик, окно) | коллайдеры и объёмы |

Ошибки учат (`jump` в воде у берега: `ok: jumping out of the water onto the ledge 0.4m ahead (+1.3m)`; далеко от берега:
`the ledge is 2.1m ahead (+1.3m), out of reach from the water: swim within … and jump, or keep swimming into it`).

```text
$ shards act clearance                       # игрок в начале полосы препятствий (x = 40)
N wall 3.6m · NE wall 5.1m · E ledge 11.6m (+1.3m) · SE wall 5.1m · S wall 3.6m · SW wall 5.1m · W free 15.0m · NW wall 5.1m
$ shards act path_to target=e12              # e12 — ориентир «Cliff»
{
  "distance": 44.5,
  "path": "path 44.5m, 6 turns: 11.5m E → 0.5m E (mantle 1.3m) → 4.0m E → 9.5m E (swim) → 0.5m E (mantle 1.3m) → 18.5m E",
  "turns": 6,
  "waypoints": [
    {"mode": "walk", "x": 51.5, "y": 2.0, "z": 0.0},
    {"mode": "mantle", "x": 52.0, "y": 3.3, "z": 0.0},
    {"mode": "walk", "x": 56.0, "y": 3.3, "z": 0.0},
    {"mode": "swim", "x": 65.5, "y": 2.0, "z": 0.0},
    {"mode": "mantle", "x": 66.0, "y": 3.3, "z": 0.0},
    {"mode": "walk", "x": 84.5, "y": 8.0, "z": 0.0}
  ]
}
```

- `clearance` в каждом из восьми направлений называет, что остановит: `free` (в пределах 15 м), `wall`, `cliff` (с глубиной
  обрыва), `water` (достаточно глубокая, чтобы плыть), `ledge` (уступ, который можно перелезть, и на сколько он выше).
  `around=ID` меряет от другой сущности. Идёт тем же способом, что контроллер, и работает без окна.
- `path_to` — локальный сеточный A*: клетка 0.5 м, поиск в ограниченной области вокруг начала и цели (`max` — предел в
  метрах, по умолчанию 80; больше — ошибка с подсказкой `raise max, or go in stages`). Ходит по тому, что умеет
  контроллер: идти, плыть (дороже вдвое), перелезать (дороже), спускаться до безопасной высоты, прыгать в воду сверху.
  Результат один и тот же на любой машине (целые стоимости, порядок по индексу). Это локальный планировщик, не навсетка.
- `shards act move_to target=e12 path=true` проходит именно этот маршрут; одна и та же функция `plan_path` отвечает и
  инструменту, и тестам. В загруженном регионе `move_to path=true` идёт по навсетке, а тот же маршрут в формате `path_to`
  показывает `nav_path` (см. «Навигация и NPC»); `path_to` и `clearance` остаются запросами к физике.

## Действия и бой (E3)

Крейт `shards_action`. Атака, блок, парирование, перекат и фляга — одно и то же: **действие**, временная шкала из контента
(`*.action.ron`). Одна машина действий на бойца (`Fighter`) ведёт игрока, врагов и боссов; **мувсет** (`*.moveset.ron`)
говорит, какой ввод какое действие запускает, плюс слоты `stagger`, `hit` и `death`.

**Форматы.** Все числа боя — файлы, подхватываются на лету. Один пример на все виды (он проходит `--check-content`):

```ron
// content/combat/hero.action.ron — действия: id → шкала
{
    "light_1": (
        net_id: 1,                                         // уникальный id действия «на проводе» (для сети)
        phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)],
        cost: (stamina: 10),
        hits: [(span: Phase("active"), shape: Cone(range: 2.0, angle: 100), damage: 20, poise: 15, status: [("bleed", 30)])],
        cancels: [(span: Secs(0.3, 0.55), into: [Combo("light", "light_2"), Input("dodge")])],
        displacement: [(0.0, 0.0), (0.3, 0.5)],            // (время, метры вперёд) — выпад
        anim: "light_1",
    ),
    "roll": (net_id: 3, phases: [("roll", 0.4)], cost: (stamina: 25), iframes: [Secs(0.0, 0.18)],
             displacement: [(0.35, 4.0)], steer: Move),
    "block": (net_id: 4, phases: [("raise", 0.1), ("hold", 0.3), ("lower", 0.1)],
              hold: (input: "block", span: Phase("hold")), move_speed: 0.4,
              guard: (span: Phases("raise", "hold"), perfect: Phase("raise"), damage_mult: 0.1)),
    "drink": (net_id: 5, phases: [("drink", 1.0)], cost: (charge: "flask"), effects: [(at: 0.6, effect: Heal(40))]),
    "slam": (net_id: 6, phases: [("windup", 0.9), ("active", 0.1), ("recovery", 0.7)], cost: (cooldown: 5.0),
             hits: [(span: Phase("active"), shape: Circle(radius: 3.0), damage: 22, poise: 40)],
             hyper_armor: [Phases("windup", "active")], telegraph: (symbol: "⚠", sound: "slam")),
}
// content/combat/hero.moveset.ron — какой ввод какое действие запускает
{ "hero": (inputs: {"light": "light_1", "dodge": "roll", "block": "block", "use_item.flask": "drink"},
           stagger: "stagger", death: "death") }
// content/combat/statuses.status.ron — статусы: порог, спад, срок, имя хука игры
{ "bleed": (threshold: 100, decay_per_s: 10, duration: 3, effect: "bleed") }
// content/combat/tuning.combat_tuning.ron — числа всей боевой системы (id `default`)
{ "default": (input_buffer: 0.15, max_statuses: 3, sprint_stamina_per_s: 10, lock_on_range: 20) }
```

- Окна — это `Span`: фаза (`Phase("active")`), от фазы до фазы (`Phases("raise", "hold")`) или секунды (`Secs(0.3, 0.45)`).
  На шкале бывают: окна удара (форма: `Sphere`, `Capsule`, `Cone`, `Box`, `Circle`, `Line` — линия по земле; урон, урон
  стойкости, накопление статуса, флаги `Unblockable`/`GuardBreak`/`NoParry`, теги для хуков игры), неуязвимость
  (`iframes`), блок и идеальный блок (`guard`), гиперброня (`hyper_armor`), отмены (комбо — цепочка переходов `Combo`,
  `Input`, `Any`), удержание (`hold`: блок держится, пока зажат ввод), эффекты по времени (`Heal`, `RestoreStamina`,
  `Status`, `Custom` — хук игры), телеграф (`telegraph`).
- Тик, в котором действие кончается, ещё его часть: окна удара и защиты его видят. Окно защиты, которое начинается до
  промежутка удержания и кончается внутри него, валидация отвергает: оно открывалось бы на каждом круге.
- Смещение (`displacement`) действие отдаёт контроллеру через `LocomotionInput::external`: двигает тело только
  контроллер, поэтому выпад останавливается о стену.
- Анимация и звук — только имена: `anim: "light_1"` (клип) и `telegraph.sound` (метка для модуля звука).

**Машина.** Нажатие из `Intent` ждёт в буфере ввода `CombatTuning::input_buffer` (0.15 с по умолчанию), пока его действие
не станет возможным; окна отмены дают перебить действие другим (комбо); удержанное действие зацикливается (поднятый
блок). Одно правило `can_start` отвечает и машине, и инструментам: `ok: light_1`, `ok: queued, light starts in 0.10s`,
`not enough stamina: 20/40 (ready in 1.5s)`, `busy: windup light_1 0.17s (ready in 0.27s, when a cancel window opens)`.
Нажатие, чьё время вышло, попадает в журнал: `e5 light rejected: busy (active light_1 0.10s)`.

**Попадания** ищутся в `SimSet::Resolve` после движения: формы ударов пересекаются с хёртбоксами (коллайдеры со слоем
`HURTBOX`; его получает коллайдер `Fighter`), атакующие и цели по порядку `AiId`, одна цель — один раз за действие (и за
группу `group`). Конвейер урона: бессмертие (`god`) → неуязвимость (`Evaded`) → парирование (`Parried`, атакующий
шатается) / блок (`Blocked`, стоит выносливости) / пробитие защиты (`GuardBroken`) → броня → хуки игры
(`add_damage_hook`, по порядку регистрации) → здоровье, статусы, стойкость (пробитие — `stagger`) → смерть. События:
`Damaged`, `Staggered`, `Died`, `Healed`, `ActionStarted`, `ActionRejected`.

**Ресурсы бойца** — компоненты: `Health`; `Stamina` (восстанавливается после задержки; спринт её тратит, а выжатый
досуха не включается, пока кнопку не отпустят); `Poise` (восстанавливается после задержки; 0 — пробитие, вынужденное
`stagger`-действие); `Armor`; `Team` (бьют только чужих); `Charges` (расходуемое по имени: фляга); `Cooldowns`; `Dead`.

**Статусы** — каркас без конкретных эффектов: накопление от ударов (`status: [("bleed", 30)]`) с затуханием и
множителями сопротивления (`StatusResist`), при пороге статус включается на `duration`, число одновременных ограничено
(`max_statuses`). Горение, холод или кровотечение — хуки игры: `add_status_effect(name, on_apply, on_tick, on_expire)`.
Эффекты действий `Custom(name)` — `add_action_effect`.

**Телеграфы и зоны.** Враг предупреждает заранее: событие `Telegraph` (символ и звуковая метка), `hit_zones(world)` для
декалей и ИИ (каждая зона ещё не отыгранного или идущего окна удара, там, где атакующий будет к моменту удара),
`threat_indicators(world, viewer, forward)` — направление для предупреждений за кадром. «Выйти из зоны» считается по следу
зоны на земле (шар — как круг, конус — как сектор): путь по одному из 8 румбов, округлённый вверх, чтобы, сделав ровно
так, из зоны выйти.

**Сторона окна** (feature `render`; без окна те же системы собираются и ничего не рисуют):

- **Анимация** (`anim`, `render::bridge`). Вид `animset` — библиотека клипов из glTF: какой клип показывает какое состояние
  движения и какое действие, смешивание, метки по времени (`cues` → `AnimationCue`, только для эффектов и звука, геймплей
  от них не зависит). **Геймплей ведёт, анимация догоняет:** клип действия подматывается на его прогресс (время геймплея в
  масштабе длины клипа), клипы движения зацикливаются со скоростью по скорости тела. Один `AnimationGraph` на набор,
  общий для всех `Animated` персонажей; персонажи с тем же скелетом (одинаковые имена узлов) делят клипы. Модель — дочерняя
  сущность персонажа (`WorldAssetRoot`). Один `AnimationPlayer` на персонажа: слоёв (верх/низ тела) нет.
  ```ron
  // content/anim/humanoid.animset.ron
  { "humanoid": (
      source: "models/humanoid.glb",
      clips: {
          "idle": (), "walk": (), "run": (speed: 1.0), "fall": (name: "Armature|Falling"),
          "swing_1": (name: "Armature|Slash1", cues: [(at: 0.28, cue: "swoosh")]),
          "roll": (blend: 0.04), "hit": (), "die": (),
      },
      locomotion: {"idle": "idle", "walk": "walk", "run": "run", "air": "fall", "stagger": "hit", "dead": "die"},
      actions: {"light_1": "swing_1", "roll": "roll"},
      blend: 0.2, action_blend: 0.06,
      speeds: (walk: 2.0, run: 5.0, sprint: 8.0),
  ) }
  ```
- **Телеграфы на земле** (`render::overlay`) рисуются всегда (не только в оверлее): контур зоны, штриховка, кольцо,
  которое заполняется по мере удара, и **символ** из контента как табличка на камеру, чтобы опасность читалась без цвета.
  Предупреждения врагов — тёплого цвета, союзников — холодного; рисуется всё, что направлено на локального игрока. Символ шире
  ASCII (`⚠`) требует шрифта (`ThreatHud::font`), без него подставляется ASCII.
- **Оверлей боя** (`combat_overlay`, F2 в окне): хитбоксы открытых окон (красный), хёртбоксы (зелёный), дуги защиты
  (синий, идеальная ярче), i-frames контуром, каждая зона с оставшимся временем.
- **Маркеры за кадром** (`render::hud`): стрелка и символ на краю экрана в сторону атаки, которая вот-вот ударит там, где
  камера не смотрит; растёт и мигает чаще по мере удара. Для `LocalPlayer`.

**Что видит ИИ.** В describe: `windup slam 0.78s (zone circle 4.5m) hyper-armor`, `stam 75/100`, `poise broken 0.8s`,
`bleed 60%`, `queued light 0.13s`, `roll roll 0.33s i-frames`; под строкой `YOU` — подсказка
`ALERT in hit zone of e6 shockwave (lands in 0.50s): move 1.3m E to leave it`; в журнале `e2 winds up slam (0.80s) ⚠
circle 4.5m`, `e1 -> e2 heavy -45 (hp 55)`, `e1 blocked e2 swipe: -2 hp, -15 stamina (hp 98)`, `e2 staggered (parried)`.

**Инструменты** (игрок, только через `Intent`): `light_attack`, `heavy_attack`, `block on=…`, `parry`, `dodge dir=…|away_from=…`,
`use_technique slot=1`, `use_item item=flask`, `lock_on target=…|off=true`, `attack target=… input=… repeat=true`;
разработчика: `god`, `heal`, `stagger`, `combat_overlay`. Однократные инструменты отвечают по `can_start`:

```text
$ shards act dodge
ok: roll (W)
$ shards act heavy_attack
ok: queued, heavy starts in 0.10s
$ shards act parry
error: your moveset `player` has no `parry`; it has: block, dodge, heavy, light, use_item, use_item.flask
```

Отказы по ресурсам и времени называют причину и срок: `not enough stamina: 10/20 (ready in 0.2s)`, `busy: windup heavy
0.50s (ready in 0.90s, when a cancel window opens)`; на ошибку делают `step` на названное время и повторяют.

## Уровни из Blender (E4.2)

Крейт `shards_level`. Художник собирает уровень в Blender, вешает на объекты *Custom Properties* и экспортирует glTF;
движок превращает файл в сущности: коллайдеры и датчики для физики, маркеры (точки появления, сокеты, связи навигации,
триггеры, лагеря, ориентиры), префабы из базы контента и любые компоненты игры по имени. С `render` тот же файл ещё и
рисуется. Движок читает только JSON файла (дерево узлов, преобразования, имена, `extras`, границы мешей): ни картинок, ни
буферов он не грузит, поэтому уровень собирается и проверяется без окна.

```text
Blender (.blend) ── экспорт glTF ──► tiles/ruin.gltf ──► LevelAsset ──► spawn_level ──► сущности
  объекты + custom properties        узлы + extras        (без GPU)       верхнего уровня, в мировых
                                                                           координатах: Collider, Volume,
                                                                           LevelMarker, префабы, компоненты
```

**Как сделать уровень в Blender.**

1. Моделируйте в метрах. Экспортёр Blender сам переводит Z-up в +Y-up. Имена объектов любые: ошибки называют узел путём по
   иерархии (`Ruin/Column`).
2. Выберите объект, *Object Properties → Custom Properties*, кнопка `+`: имя и значение. Текст, числа и списки чисел
   экспортируются. Свойства читаются с **объектов**, не с мешей.
3. *File → Export → glTF 2.0*. В разделе *Include* отметьте **Custom Properties**: без этого все свойства теряются.
   *Transform → +Y Up* оставьте. Подходят `.gltf`, `.glb` и вариант со встроенными данными.
4. Положите файл в папку ассетов игры (`assets/tiles/ruin.gltf`), назовите его в контенте (вид `level`, ниже) и выполните
   `shards content check`.

**Свойства узла** бывают двух видов: свойства уровня (слова в нижнем регистре, их читает движок) и компоненты (имена
компонентов игры в `CamelCase`).

| свойство | значение | что делает |
|---|---|---|
| `marker` | `spawn`, `socket`, `navlink`, `trigger`, `interactable`, `camp`, `landmark`, `custom:<имя>` | узел — маркер этого вида (`LevelMarker`) |
| `name` | текст | имя маркера (`start`); без него — имя узла |
| `socket` | текст | сокращение для `marker = socket`: значение — тип сокета (`road`) |
| `to` | текст | у `navlink`: имя узла, где связь кончается |
| `collider` | `box`, `sphere`, `capsule`, `cylinder`, `cone`, `auto` | твёрдый `Collider`; `auto` — коробка по мешу |
| `size`, `radius`, `height`, `offset` | числа и списки | размеры и центр формы; без них берутся границы меша |
| `layers`, `collides_with` | `world`, `character`, `volume`, `game0`…`game15` (имя или список) | слои коллайдера |
| `volume` | `water`, `updraft`, `trigger` | датчик `Volume` формы `collider` (по умолчанию `auto`); `accel` и `max_speed` — у потока |
| `prefab` | текст | заспавнить в узле префаб из контента; свойства-компоненты правят его поля |

Любой другой ключ — **компонент**: короткое имя (`Health`) или полный путь типа, значение — JSON в том виде, как его
показывают `inspect` и `get`:

```text
Health  = {"current": 40.0, "max": 40.0}
Faction = "bandits"
Locked  = true                           (компонент без полей)
```

Панель Blender делает только текст, числа и списки, поэтому объект JSON вводят текстом: текст, в котором лежит объект или
список JSON, читается как он. Компонент должен быть зарегистрирован для reflection (`#[derive(Reflect)]`,
`#[reflect(Component)]`), как для префабов. У узла с `prefab` свойства-компоненты сливаются с префабом по полям
(`Health = {"current": 10}` оставляет `max` префаба). Узел без этих свойств (группа, меш без `collider`) ничего не порождает,
меш остаётся только картинкой. Ключи с `_` игнорируются.

**Маркеры движка** (`marker`). Игра находит их обычным запросом `Query<(&LevelMarker, &Transform)>`.

| `marker` | сущность | что видит ИИ |
|---|---|---|
| `spawn` | `LevelMarker` | `SpawnPoint "start"` |
| `socket` | `LevelMarker` с именем-типом сокета | нет |
| `navlink` | `LevelMarker`, у него `to` — мировая позиция конца | нет |
| `trigger` | `LevelMarker`, датчик-`Volume` и его коллайдер | `Trigger "goal"` |
| `interactable` | `LevelMarker` | `Interactable "door"` |
| `camp` | `LevelMarker` | `Camp "bandits"` |
| `landmark` | `LevelMarker`, `AiLandmark`, `AiKind` по имени маркера | в строке LANDMARKS |
| `custom:<имя>` | `LevelMarker` | нет (нужен `AiKind` — добавьте компонентом) |

Узлы файла `crates/sandbox/assets/tiles/ruin.gltf` (руина 16 × 16 м; в репозитории его пишет тест
`crates/sandbox/tests/sandbox/tiles.rs` вместо экспортёра Blender, настоящий экспорт мы не пробовали): пол `Ground` и стены `Wall.North`, `Wall.West`, `Rubble`, `Column` с `collider = auto`; `Camp`
(`marker = camp`, `name = ruin_camp`), `Arch` (`marker = landmark`, `name = Ruin`) и четыре `Edge.N/E/S/W` с `socket = grass`.

**Файл в контенте.** Вид `level` (`LevelDef`): путь файла, обязательные маркеры, место по умолчанию.

```ron
// content/region/ruin.level.ron
{
    "ruin": (
        file: "tiles/ruin.gltf",
        required_markers: [(kind: "camp", name: "ruin_camp"), (kind: "socket", name: "grass", min: 4), (kind: "landmark")],
        at: (0.0, 0.0, 0.0),
    ),
}
```

`shards content check`, `--check-content` и каждая перезагрузка контента читают сам файл: нет файла или он не читается,
узел называет несуществующий компонент или такое значение, которое компонент не принимает, неизвестный префаб, `navlink` без
конца, нет обязательного маркера. Каждая ошибка называет файл контента, уровень, узел и свойство. Настоящий вывод (в копии
контента песочницы опечатка в `Helth` и переименован маркер `ruin_camp`):

```text
$ BEVY_ASSET_ROOT=/tmp/broken target/debug/sandbox --check-content
content/region/ruin.level.ron: level `ruin`: node `Ruin/Column`: unknown component `Helth` (did you mean `Health`?)
content/region/ruin.level.ron: level `ruin`: required marker missing: `camp` named `ruin_camp` in `tiles/ruin.gltf` (found 0, need 1); add the custom property marker = "camp" and name = "ruin_camp" to an object in Blender
…
```

Шлюз загрузки держит симуляцию, пока файлы не загружены (`LevelLibrary`).

**Инструменты.** Игрок: `level_info`; разработчик: `spawn_level`, `level_reload`.

```text
$ shards act level_info id=ruin
level ruin: tiles/ruin.gltf, at (0.0, 0.0)
13 nodes, 6 markers
  camp "ruin_camp" at (-2.0, 2.0)  Camp
  landmark "Ruin" at (1.0, 1.0)  Arch
  socket "grass" at (0.0, -8.0)  Edge.N
  …
checks: ok
$ shards act spawn_level id=ruin x=60 z=-40
spawned level `ruin` as instance 1 at (60.0, -40.0): 11 entities (5 solids, 0 volumes, 6 markers)
  camp "ruin_camp" at (58.0, -38.0)
  …
```

**Что получается при спавне.** Сущности по узлам в порядке файла, все **верхнего уровня, с мировым `Transform`** (физика читает
`Transform`, а внутри тика он не пересчитывается). Масштаб узла уходит в размеры коллайдеров, у сущности масштаб 1. У каждой
сущности есть `FromLevel` (уровень, номер экземпляра, путь узла). Если у какого-то узла есть проблема, не спавнится ничего.
Один файл можно поставить несколько раз (каждый раз новый экземпляр). `despawn_level` и `reload_level` работают с
экземпляром.

**Горячая перезагрузка.** Изменённый на диске файл asset server перечитывает сам, а экземпляры заменяются через
`level_reload` (он в журнале и в реплее, как любое действие). Файл с проблемами оставляет экземпляры, как они были.
Сущности, которые игра успела изменить, возвращаются такими, как их описывает файл.

**Уровни как тайлы регионов.** Тайл с `geometry: Level(path: "tiles/ruin.gltf")` строится из файла (см. «Мир и генерация»).
Крейты `shards_level` и `shards_world` друг друга не знают, их соединяет зонтик: `shards::TileLevelsPlugin` (входит в
`EnginePlugins`) просит все файлы тайлов, пока игра грузится, и спавнит их на тайл.

- Файл лежит в системе координат тайла: начало — центр тайла, −Z на север, +Y вверх, метр есть метр (тайлу 16 м — пол
  16 × 16 м). Поворот тайла поворачивает файл на четверти оборота по часовой.
- Узлы становятся сущностями как в любом уровне, но с метками региона (`TileId`, `RegionPart`): они уходят вместе с регионом
  и получают одни и те же `AiId` и `ColliderId` в каждом запуске. Узлы-маркеры (`marker`, `socket`) ещё и `Marker` тайла,
  как маркеры из inline-геометрии.
- **Решатель читает сокеты из RON тайла, а не из файла**: генерация — чистая функция seed, версии и контента, она работает
  без ассетов (`--worldgen-report`). Узлы `socket` и слоты в файле ставятся в мире для игры (стыки, декор на краях), назад
  решатель их не читает. Проверки, что маркеры-сокеты файла совпадают с сокетами тайла в RON, нет (в `shards_content` нет
  связи между видами).
- Сломанный изнутри файл тайла (узлы, компоненты, префабы) попадает в журнал ошибкой с именами тайлов, как только
  загрузится, а смена региона, которая его использует, отказывает, пока ничего не тронуто. `content check` не читает узлы
  файлов тайлов, если файл не назван ещё и определением `level` (как `ruin.level.ron` песочницы), но отсутствующий файл или
  путь за пределами папки ассетов проваливает проверку, называя тайл.
- Перезагруженный файл тайла получает метки региона внутри перезагрузки: реплей идёт без кадров, и метка на кадр позже
  разошлась бы с записью.

**Детерминизм.** Спавн — чистая функция файла и места, в фиксированном порядке, поэтому `AiId` и `ColliderId` одинаковы в
каждом запуске. `spawn_level` и `level_reload` — действия, реплей записывает их. Проверка реплея `level` хэширует, где поставлен
каждый уровень, и дайджест его файла: реплей, записанный на другой версии файла, расходится на первой проверке (после
правки уровня эталоны записывают заново). Пока не стоит ни один уровень, проверка ничего не добавляет в хэш
(`add_optional_replay_check`): у игры без уровней хэши прежние, и хэш не помнит уровней, которые были и ушли (снапшот, снятый
до первого уровня, грузится точно и после региона с тайлами-уровнями). Снапшот проверяет те же поставленные уровни
(`levels`) и хранит счётчик номеров экземпляров (`LevelCounter`).

## Мир и генерация (E5)

Крейт `shards_world`. Регион — сетка макротайлов с сокетами на краях (или подземелье из комнат, стыкующихся дверь в дверь).
Генератор собирает его из зерна, проверяет, при неудаче повторяет и в конце берёт запасную карту. Результат —
**манифест**: данные, из которых мир строится заново без генератора. Регион меняется через экран загрузки, а ИИ врагов
работает только в активных зонах вокруг игроков.

Набор тайлов, правила сокетов и пороги проверок — контент игры; порядок шагов, решатель, повторы и отчёт — движок.

```text
seed ─► Points ─► Roads ─► Cells ─► Fill (решатель сокетов) ─► Secondary ─► Decorate ─► Checks ─► манифест
  точки    дороги   клетки    тайлы с поворотами     ещё точки       декор      правила
              любой шаг или проверка не прошли ─► следующая попытка ─► … ─► запасная карта (tile_map)
```

### Форматы контента

Семь видов: `tile`, `socket_rules`, `poi`, `region`, `tile_map`, `room`, `dungeon`. Все проверяются вместе с базой, ошибки
называют файл, поле и ближайший id. Настоящие файлы песочницы — `crates/sandbox/assets/content/region/`.

**Тайл** (`*.tile.ron`): края с сокетами, повороты, вес, слоты для точек интереса, геометрия.

```ron
// content/region/wilds.tile.ron — тайл 16 м; система координат тайла: центр в начале, север −Z, до поворота
{
    "road_bend": (
        sockets: (n: "grass road grass", e: "grass road grass", s: "grass", w: "grass"),
        rotations: [0, 90, 180, 270],            // на сколько градусов по часовой генератор может его повернуть
        weight: 2.0,                             // относительная частота; повороты делят её
        slots: [(at: (-3.0, 0.0, 3.0), accepts: ["camp", "shrine"])],      // куда может встать точка интереса
        geometry: Inline([Box(min: (-8.0, -1.0, -8.0), max: (8.0, 0.0, 8.0))]),
    ),
    "ruin": (                                    // геометрия из файла Blender (см. «Уровни из Blender»)
        sockets: (n: "grass", e: "grass", s: "grass", w: "grass"),
        rotations: [0, 90, 180, 270], max_count: 4,
        slots: [(at: (-2.0, 0.0, 2.0), accepts: ["camp"])],
        geometry: Level(path: "tiles/ruin.gltf"),
    ),
}
```

Остальные поля `TileDef`: `min_count` и `max_count` (сколько таких тайлов в регионе), `tags` (для наборов `#forest` и
лимитов), `pois` (точки, которые у тайла есть всегда: `(poi: "spawn", at: (0, 0, 0))`), `links` (какие стороны соединены
внутри тайла: `[[N, S], [E, W]]` — речной брод без моста; по умолчанию любая со всеми), `slope` (класс уклона, 0 — плоско),
`decor` (префабы для оформления, по одному на тайл), `visual` (id для картинки в окне). Геометрия (`TileGeometry`): `None`,
`Inline([...])` или `Level(path: …)`. Куски inline-геометрии — `Box`, `Solid` (форма любого вида), `Volume` (вода, поток,
триггер), `Marker` (именованная точка для игры и модулей), `Prefab`; они спавнятся в порядке списка.

**Сокеты.** Край тайла — текст из сокетов по часовой вокруг тайла: на северном краю с запада на восток, на восточном с севера
на юг и так далее. Порядок по часовой переживает поворот, а соседи читают общий край в разные стороны, поэтому край
совпадает с развёрнутым краем соседа. Край из одного слова — весь край; `"grass road grass"` — дорога в средней трети;
`road@1` — сокет на уровне высоты 1. Что с чем стыкуется — вид `socket_rules`:

```ron
// content/region/wilds.socket_rules.ron
{
    "default": (
        kinds: ["grass", "road"],        // каждый вид сокета, который встречается в тайлах (опечатка в тайле — ошибка)
        passable: ["grass", "road"],     // через них можно пройти (проверка достижимости, навсетка)
        roads: ["road"],                 // эти — дороги (план дорог, проверка «дорога никуда»)
    ),
}
```

Также `pairs` (какие разные виды стыкуются, симметрично), `forbid` (не стыкуются ни с чем: запечатанные стороны, граница
региона), `any_height` (виды, которым высота безразлична: склон обрыва) и `height_tolerance` (на сколько уровней высоты
могут различаться стыкуемые сокеты).

**Точка интереса** (`*.poi.ron`): как её называет ИИ и что в ней стоит.

```ron
{
    "spawn": (label: Some("Spawn")),
    "camp":  (label: Some("Camp"), prefab: Some("camp_skeleton")),    // лагерь спавнит скелета при сборке региона
}
```

**Регион** (`*.region.ron`).

```ron
// content/region/wilds.region.ron
{
    "wilds": (
        size: (8, 8),                      // клеток: восток × юг
        tile_size: 16.0,                   // метров
        version: 1,                        // версия генератора, см. ниже
        origin: (-64.0, 48.0),             // где северо-западный угол в мире (x, z); по умолчанию (0, 0)
        fixed: [(at: (1, 1), tile: "gate", rot: 0), (at: (5, 5), tile: "hall", rot: 0)],
        pois: [(poi: "spawn", cells: [(1, 1)]), (poi: "den", cells: [(5, 5)]), (poi: "shrine"), (poi: "camp", count: 2)],
        checks: (isolated_roads: false, distances: [(a: "spawn", b: "camp", min: 40.0), (a: "den", b: "camp", min: 40.0)]),
        fallback: "wilds_safe",            // tile_map на случай, когда ни одна попытка не прошла
        active_radius: 40.0,               // NPC дальше этого от всех игроков спят
    ),
}
```

Остальное: `rules` (id `socket_rules`, по умолчанию `default`), `tiles` (id и `#теги`; пусто — все тайлы), `border` (сокет, который
обязан стоять на каждом краю границы), `limits` (счётчик тайлов по id или `#тегу`: `(of: "#forest", min: 4, max: 20)`),
`roads: true` (связать обязательные точки дорогами, остовным деревом), `secondary` (точки, которые ставятся в свободные
слоты после тайлов, сколько поместится), `attempts` (8 попыток по умолчанию), `backtracks` (2000 откатов решателя на
попытку), `spawn` (точка, где появляются игроки; по умолчанию `spawn`). `checks` — правила, которым регион обязан
соответствовать:

| правило | что проверяет |
|---|---|
| `reachable` (по умолчанию да) | из точки появления можно дойти до каждой точки интереса по проходимым сокетам и `links` тайлов |
| `max_slope` | ни один тайл не круче |
| `distances` | каждая `a` не ближе `min` к каждой `b`; у каждой `a` есть `b` не дальше `max` |
| `isolated_roads` (по умолчанию да) | каждая дорога ведёт к точке интереса |
| `custom` | имена проверок игры (`add_worldgen_check`) |

Всегда проверяется и то, что точка появления есть. Каждая неудача называет правило и клетки (`check reachable`, `check
distance`).

**Запасная карта** (`*.tile_map.ron`): регион руками, ряды клеток с севера: `tile` или `tile:90` в клетке.

```ron
{ "wilds_safe": (rows: ["grass grass grass …", "grass gate grass …", "…"],
                 pois: [(poi: "spawn", at: (1, 1)), (poi: "den", at: (5, 5)), (poi: "camp", at: (6, 2))]) }
```

Проверки читают манифест и контент, поэтому судят запасную карту и сохранённый манифест так же. Запасная карта, не
прошедшая проверки, всё равно берётся, замечание уходит в `remarks`.

**Что ловит `content check`.** Неизвестный сокет, тайл, префаб и точка, регион, чьи обязательные точки некуда поставить,
точка появления, которой нигде нет, размер запасной карты не по региону. Настоящий вывод (в копии контента песочницы
опечатки в сокете, точке и запасной карте):

```text
$ BEVY_ASSET_ROOT=/tmp/broken target/debug/sandbox --check-content
content/region/wilds.region.ron: region `wilds`: tile `road_t` uses socket kind `gras`, which rules `default` does not list (did you mean `grass`?)
content/region/wilds.region.ron: region `wilds`: pois[4].poi -> unknown poi `tower`
content/region/wilds.region.ron: region `wilds`: fallback -> unknown tile_map `wilds_safe2` (did you mean `wilds_safe`?)
3 content errors in /tmp/broken/assets
```

### Генератор, решатель, проверки

Генератор — конвейер именованных шагов по стадиям `Points → Roads → Cells → Fill → Secondary → Decorate → Checks`. Движок
ставит по шагу на стадию (`engine.points`, `engine.roads`, …), так что игра, у которой есть только данные, работает без кода.
Игра добавляет свой шаг после шагов движка или заменяет стадию целиком, и свои проверки по имени:

```rust
app.add_plugins(EnginePlugins::new(config))
    .start_region("wilds")                                  // генерируется с seed мира, пока игра грузится
    .add_worldgen_step(Stage::Decorate, "flowers", flowers)  // после шагов движка на этой стадии
    .replace_worldgen_stage(Stage::Roads, "my_roads", my_roads)
    .add_worldgen_check("river_crossings", crossings);       // имя — в checks.custom региона

fn flowers(ctx: &mut GenContext) -> Result<(), String> {
    let mut rng = ctx.rng();                // поток шага: ключ (seed, версия, попытка, шаг), не SimRng
    // ... ctx.pois, ctx.roads, ctx.decor, ctx.filled ...
    Err("flowers: no meadow tile".into())   // «причина: подробности»; причина группирует отказы в отчёте
}
```

**Решатель сокетов** заполняет сетку повёрнутыми тайлами, края которых стыкуются (в духе WFC). Каждая клетка начинает с
вариантами (тайл × разрешённый поворот), которые оставили предыдущие шаги. Распространение ограничений убирает у соседей то,
что не стыкуется с оставшимся (AC-3 по сигнатурам краёв), лимиты чисел убирают тайл отовсюду, когда счёт достиг максимума.
Следующей решается клетка с наименьшим числом вариантов (при равенстве меньший индекс по строкам), вариант берётся по весу
из `DetRng`; противоречие откатывает последнее решение и запрещает его, не больше `backtracks` раз за попытку. Решают только
целые числа (веса в тысячных, порядок по числу вариантов), поэтому одно зерно даёт одни и те же тайлы на любой платформе. Все
изменения доменов пишутся в журнал отката, а не копируются: решение стоит памяти по тому, что изменило, и сетка 64 × 64 с
откатами укладывается в 8 МиБ.

**Детерминизм.** Генерация — чистая функция зерна, версии генератора региона, номера попытки и контента. Случайные потоки шагов
берутся из `GenContext::rng()` (`DetRng`, ключ из этих четырёх чисел и имени шага), не из `SimRng` и никогда не зависят от
порядка `HashMap`. Поле `version` региона входит в каждый ключ: новая версия даёт старым зёрнам новые миры, а уже сохранённые
манифесты остаются как были. **Меняете результат генератора — поднимите `version`.**

**Попытки и запасная карта.** Попытка не прошла (шаг вернул `Err`, решатель не нашёл тайл, не прошла проверка) — следующая, с
новым ключом, до `attempts`. Если ни одна не прошла — берётся `fallback`; без него смена региона отказывает и называет
причины всех попыток. Первая строка журнала говорит, как вышло: `region generated: wilds seed 42 v1 (attempt 1, 64 tiles,
5 POIs)`; у запасной карты в манифесте `attempt: 0`.

### Манифест

`RegionManifest` — регион как данные, они хранятся вечно (в сохранении их будет держать E8): зерно, версия генератора,
сработавшая попытка (0 — запасная карта), каждый тайл с поворотом, точки интереса с позициями, дороги или двери между ними и
хэш контента, из которого регион сделан. Загрузка манифеста **не запускает генератор**, поэтому обновление игры, меняющее
генератор, не перестраивает старые миры.

Каноническая запись (`to_canonical`, формат `shards-region/1`) побайтно стабильна: ключи по алфавиту, числа не больше чем с
тремя знаками, один тайл в строке. Одно зерно, версия и контент дают одни и те же байты, `RegionManifest::hash` — FNV-1a
по ним. Позиции поворачиваются точными перестановками, а не синусами. Настоящий манифест `wilds` с зерном 42 (88 строк):

```json
{
  "attempt": 1,
  "content_hash": "37778bd5b35f5e91",
  "format": "shards-region/1",
  "layout": "grid",
  "links": [],
  "origin": [-64, 48],
  "pois": [
    {"cell": [1, 1], "id": "spawn.0", "poi": "spawn", "pos": [-40, 0, 72]},
    {"cell": [5, 5], "id": "den.0", "poi": "den", "pos": [24, 0, 140]},
    …
  ],
  "region": "wilds",
  "seed": 42,
  "size": [8, 8],
  "spawn": "spawn.0",
  "tile_size": 16,
  "tiles": [
    {"at": [0, 0], "rot": 0, "tile": "grass"},
    {"at": [3, 0], "rot": 0, "tile": "pillars"},
    {"at": [4, 0], "rot": 90, "tile": "ruin"},
    …
  ],
  "version": 1
}
```

(`origin` не пишется, когда он `(0, 0)`: манифесты регионов в начале мира остались такими же по байтам и хэшам.) Хэш виден в
`region_info` (`"manifest_hash": "d9d721509840d909"` для этого региона) и в `ai/status` → `world`; сценарий `region` сравнивает
его с эталоном.

### Подземелья

Подземелье — комнаты, стыкующиеся дверь в дверь. Вид `room`: размер в клетках, двери на границе его футпринта, повороты, вес,
точки и слоты, геометрия. Вид `dungeon`: вход, цель, наборы комнат, ручные комнаты и длина пути.

```ron
// crates/shards_world/tests/world/assets/content/dungeon/crypt.dungeon.ron
{ "crypt": (version: 1, cell_size: 8.0, entrance: "entry", goal: "throne", rooms: ["#crypt"],
            fixed: ["vault"],            // ручная комната: фиксированный узел главного пути
            path: (3, 6),                // комнат между входом и целью, ручные в их числе
            branches: (1, 3),            // лишних комнат в стороне от пути
            max_size: (14, 14), secondary: [(poi: "chest", count: 2)],
            checks: (distances: [(a: "spawn", b: "boss", min: 16.0)])) }
// …crypt.room.ron
{ "vault": (size: (2, 1), doors: [(at: (0, 0), side: W), (at: (1, 0), side: E)],
            pois: [(poi: "vault", at: (0.0, 0.0, 0.0))], slots: [(at: (4.0, 0.0, 0.0), accepts: ["chest"])],
            geometry: Inline([Box(min: (-8.0, -1.0, -4.0), max: (8.0, 0.0, 4.0))])), … }
```

Главный путь кладётся первым: вход, `path` комнат (ручные среди них, в случайных местах), цель; каждая следующая комната входит
в дверь, обращённую к свободной двери предыдущей. Путь ищется поиском в глубину с ограниченным откатом, поэтому подземелье,
которое получилось, всегда проходимо от входа к цели. Потом на свободные двери вешаются ветки. Все розыгрыши — из `DetRng` с
ключом из зерна, версии, попытки и стадии, как у сеточных регионов. Подземелье строится так же, как регион (`regen
region=crypt`), его манифест имеет `layout: rooms`, а связи — `door`. Ручная комната, которую нельзя положить посреди пути
(у неё меньше двух дверей), — ошибка контента; нижняя граница `path` поднимается до числа ручных комнат.

### Пакетная проверка: `shards worldgen`, `--worldgen-report`

Регион генерируется для многих зёрен без окна, ничего не строя. Первая строка начинается с `ok:` (все прошли) или `failed:`,
дальше причины неудачных попыток с примерами зёрен, расстояния между точками (мин / медиана / макс) и паники генератора с
номером зерна.

```text
$ cargo sandbox --worldgen-report wilds --seeds 1..1000
ok: worldgen wilds v1, seeds 1..1000: 1000/1000 passed (42 after a retry), 0 fell back, 0 failed, 0 panicked [1000 seeds in 0.31s, 0.3 ms/seed]
failed attempts (43):
  43× check distance (e.g. seeds 19, 25, 71)
distances between points (m, min / median / max):
  camp–camp  9.5 / 66.0 / 155.7
  camp–den  40.0 / 65.2 / 93.8
  camp–shrine  9.5 / 68.7 / 154.8
  camp–spawn  41.0 / 78.1 / 140.0
  den–shrine  9.5 / 55.7 / 118.9
  den–spawn  93.4 / 93.4 / 93.4
  shrine–spawn  12.0 / 70.3 / 140.0
```

Код выхода: 0, если прошли все зёрна, иначе 1 (нет такого региона — тоже 1, с подсказкой `unknown region `wild` (did you mean
`wilds`?)`). Зёрна: `1..1000` (оба конца включены), `7`, `1,5,10..20`; не больше миллиона. Тот же отчёт даёт запущенная игра:
`shards worldgen --region wilds --seeds 1..1000 --report` (инструмент `worldgen_report`). Подземелье проверяется так же
(`--worldgen-report crypt`): тысяча зёрен проходит за 0.17 с. CI гоняет этот отчёт для песочницы и шаблона.

### Загрузка региона

Инструменты (разработчик; действия, реплей записывает и повторяет их в тот же тик):

- `regen region=ID seed=N` — сгенерировать регион (или подземелье) и перейти в него; без аргументов — следующее зерно текущего
  региона;
- `region_load manifest=…` — построить регион из манифеста, как он был (из `region_info manifest=true`), ничего не генерируя;
- `goto_tile x y [id]` — телепорт в центр тайла на землю (пол выше плоскости тайла тоже, не внутрь него);
- запросы: `region_info` (игрок: что за регион, точки, хэш манифеста; `manifest=true` — весь манифест) и
  `worldgen_report`; с окном ещё `world_overlay` (границы тайлов, сокеты, точки, дороги).

```text
$ shards act regen region=wilds seed=42
ok: wilds seed 42 v1 (attempt 1, 64 tiles, 5 POIs, 199 entities), spawn at (-40.0, 72.0)
CHANGES
  ~ e5 Player: moved 72.1m SW
  + e23 Spawn "spawn.0"  72.1m SW
  + e24 Den "den.0"  130.2m S
  + e25 Skeleton "Den Guard"  hp 40/40  poise 20/20  130.2m S
  …
JOURNAL
  0 region generated: wilds seed 42 v1 (attempt 1, 64 tiles, 5 POIs)
  0 region loaded: wilds seed 42 v1 (64 tiles, 199 entities, players at (-40.0, 72.0))
  0 navmesh: wilds seed 42 v1 (64 tiles, 65012 nodes, 7168 links across edges, 0 navlinks)
```

Манифест из `region_info manifest=true` возвращает регион таким, каким он был, ничего не генерируя (хэш тот же):

```text
$ shards act regen region=wilds seed=7                 # другой регион…
$ shards act region_load manifest='{"attempt": 1, "content_hash": "37778bd5b35f5e91", "format": "shards-region/1", …}'
ok: wilds seed 42 v1 loaded from its manifest (64 tiles, 199 entities)
$ shards act region_info | grep manifest_hash
  "manifest_hash": "d9d721509840d909",
```

Что происходит внутри вызова (`change_region`): все тайлы, префабы и файлы уровней проверяются **до** того, как что-то
тронуто (опечатка в префабе оставляет старый регион, а не мир без региона); состояние идёт `InGame → RegionLoading → InGame`
(экран загрузки: тики не идут, `OnExit(InGame)` и `OnEnter(InGame)` для этой поездки не выполняются, игроки остаются);
сущности старого региона уходят (`region unloaded: …`); новый строится тайл за тайлом по строкам (комнаты — по северо-западной
клетке), куски геометрии каждого тайла в порядке файла, потом точки интереса (сущность-ориентир с `AiKind` по метке точки и
префаб точки: лагерь спавнит скелета); игроки переносятся на точку появления; модули, подписавшиеся на `on_region_loaded`
(навсетка), достраивают свои данные. Все сущности — верхнего уровня, с мировым `Transform`, с `TileId` и `RegionPart`, так что
тот же манифест даёт те же `AiId` и `ColliderId` в каждом запуске, с окном и без него. Если какой-то другой модуль ещё
что-то грузит (файл уровня), шлюз загрузки держит тики, пока всё не готово: смена региона не сдвигает ни одного тика.

**Стартовый регион.** `start_region("wilds")` в плагине игры генерирует регион с seed мира, пока игра грузится, и строит его
до `OnEnter(InGame)`: его `AiId` идут первыми. Игра без этого начинает без региона. Песочница так не делает: её «дебри» вызываются
`regen`.

**Регион рядом с другими местами.** Поле `origin` ставит северо-западный угол региона где угодно в мире (центры клеток, точки,
навсетка считаются от него): песочница держит дебри к югу от арены и полосы препятствий, а не вместо них. В `region_info`
у такого региона есть `north_west_corner`, в `observe` — `TILE outside …`, когда игрок вне его.

### Активные зоны и стриминг

Каждый тик в `SimSet::Input` (до выполнения приказов) каждый персонаж, который не игрок (у него есть `Intent`, нет
`PlayerId` и `AiPlayer`), получает `ActiveZone` в пределах радиуса от какого-нибудь игрока и `Dormant` дальше. Чтобы персонаж
на границе не мигал, активный засыпает только на `active_margin` (8 м) дальше. Радиус: `active_radius` региона, иначе
`WorldSettings::active_radius`, иначе 96 м; без региона и без настройки зон нет, и всё работает. Мозги NPC и навигация
пропускают `Dormant`; он виден в `observe` (`dormant`). Когда игрок в дебрях, монстры арены спят:

```text
  ~ e6 Boss "Frost Warden": +dormant
  ~ e7 Skeleton: +dormant
```

**Стриминг.** Геометрия региона для игры создаётся вся при загрузке региона. Окно (`render`) ещё показывает тайлы: картинку
тайла игра делает сама (`set_tile_visual_spawner`, по LOD), а движок создаёт и убирает её по расстоянию до ближайшего игрока
(`WorldSettings::visual_lods`, по умолчанию 96 и 256 м; дальше последней границы убрано). Это только картинка: без окна
ничего не стримится, на тики это не влияет. Подгрузки геометрии тайлов для физики по расстоянию нет (весь регион в памяти).

### Регион глазами ИИ

```text
$ shards observe
YOU  e5 Player  hp 100/100  stam 100/100  poise 60/60  flask 3/3  god mode  at (-40.0, 72.0) facing N
TILE (1, 1) gate rot 0 · wilds seed 42 v1
POI  e23 spawn.0 0.0m here · e26 shrine.0 70.3m SE · e29 camp.1 84.0m S
```

`TILE` — тайл под субъектом (`TILE outside wilds seed 42 v1 (8×8 tiles of 16m from (-64, 48))`, если он вне региона),
`POI` — три ближайшие точки с направлением и расстоянием. `ai/status` получает поле `world` (регион, зерно, версия, число
тайлов, хэши манифеста и контента). Условие `step --until named:near_poi` останавливает шаг, когда игрок в пределах 3 м от
точки интереса (регистрируется с первым регионом).

## Навигация и NPC (E6)

Крейт `shards_npc`: враги, которых игра описывает данными, и навигация, по которой они ходят. Языковой модели тут нет:
поведение — таблицы в контенте и обычный код. NPC управляет телом через `Intent`, так же, как игрок и как агент
(`Order::Attack`), поэтому бьёт и ходит по правилам игрока (выносливость, замах, откат).

- **Поведение**: автомат состояний из контента (`behavior`); логика состояний и условий — именованные функции движка или игры.
- **Восприятие**: зрение лучами через физику, слух, память о последнем месте цели.
- **Режиссёр боя** (`director`): жетоны атаки по числу игроков, бюджет столкновения, разрешённые сочетания, роли ждущих.
- **Планировщик атак** (`attack_plan`): веса, окна дистанции, откаты, запреты последовательностей; для врагов и боссов.
- **Навсетка**: тайлы запекаются заранее, при загрузке региона сшиваются, маршруты по графу тайлов.
- **Агенты навигации**: следование по маршруту, уклонение, поводок вокруг дома.

NPC — боец (`Fighter`) с компонентом `Npc` (и необязательными `Home`, `Patrol`); остальное приходит с ним через `#[require]`.
В префабе это одна строка, кода игры не нужно (`crates/sandbox/assets/content/region/camps.prefab.ron`):

```ron
"Npc": (behavior: "skeleton", perception: "guard", plan: Some("skeleton"), director: ""),
"Home": (pos: None, leash: 12.0),          // pos: None — там, где NPC стоит на первом тике
```

### Поведение

Каждый тик активный NPC (в порядке `AiId`) сначала пробует переходы своего состояния по порядку: берётся первый, у которого
все условия выполнены (не больше одного за тик), потом хук состояния говорит, чего NPC хочет сейчас: куда идти, куда смотреть,
нужен ли жетон, бить ли, что нажать. Условия и состояния только читают мир, поэтому порядок обхода NPC ничего не меняет.

```ron
// content/npc/monsters.behavior.ron
{
    "skeleton": (
        initial: "idle",
        states: {
            "idle": (transitions: [(to: "fight", when: ["target_known", ("target_in_leash", {"margin": 2.0})])]),
            "fight": (kind: "attack", transitions: [
                (to: "home", when: [("!target_in_leash", {"margin": 2.0})]),    // «!» — отрицание
                (to: "home", when: ["target_lost"]),
            ]),
            "home": (kind: "return_home", params: {"heal": 10.0}, transitions: [
                (to: "fight", when: ["target_known", "target_in_leash"]),
                (to: "idle", when: ["at_home"]),
            ]),
        },
    ),
}
```

Состояние: `kind` (хук; по умолчанию имя самого состояния), `params` (параметры входа), `duration: (мин, макс)` (таймер на
входе из `SimRng`; условие `timer_done`) и `transitions`. Условие — `"имя"` или `("имя", {"параметр": число})`. Опечатка в
состоянии перехода — ошибка контента с подсказкой; неизвестное условие или хук называется в журнале один раз и никогда не
выполняется.

| хук (`kind`) | что делает | параметры |
|---|---|---|
| `idle` | стоит | `face_target` |
| `hold` | стоит лицом к цели в поводке | |
| `patrol` | обходит точки `Patrol` по кругу | `speed`, `within`, `wait` |
| `chase` | бежит к цели, не выходя из поводка | `speed`, `within` |
| `return_home` | идёт домой и лечится | `speed`, `within`, `heal` (здоровья в секунду) |
| `wait_token` | ждёт жетон в роли режиссёра | `speed` |
| `attack` | с жетоном планировщик выбирает атаку, подходит и нажимает; без жетона просит его и ждёт в роли | `speed` |
| `flee` | убегает от цели | `speed`, `distance` |
| `investigate` | идёт на звук или запомненную точку | `speed`, `within` |

Условия движка: `target_known`, `target_seen`, `target_heard`, `target_lost`, `target_within {meters}`, `target_in_leash
{margin}`, `beyond_leash {margin}`, `at_home {within}`, `arrived`, `has_token`, `attack_done`, `resting`, `busy`, `timer_done`,
`in_state {secs}`, `health_below {fraction}`, `heard_noise`, `chance {p}` (из `SimRng`).

**Своё в игре**: функции с именем, на которое ссылается контент; игровая функция с именем движка заменяет его.

```rust
app.add_npc_condition("low_morale", |c, p| c.health_fraction() < p.num("below", 0.3))     // fn(&NpcCtx, &Params) -> bool
   .add_npc_state("rally", |c, p, out| {                                                   // fn(&NpcCtx, &Params, &mut NpcOutput)
       out.goal = c.home;                      // идти домой; маршрут, уклонение и поводок сделает навигация
       out.within = 2.0;
       out.speed = p.num("speed", 1.0);
   });
```

`NpcCtx` даёт NPC, его цель (`target`: расстояние, видна ли), поводок, жетон, план, мир на чтение и `rng(salt)` из `SimRng`.

### Восприятие

Параметры — вид `perception`: `sight_range`, `fov_deg`, `eye_height`, `near`, `line_of_sight`, `hearing`, `memory`.

```ron
// content/npc/monsters.perception.ron
{
    "skeleton": (sight_range: 18.0, fov_deg: 360.0, near: 3.0, hearing: 1.0, memory: 6.0),
    "guard":    (sight_range: 14.0, fov_deg: 360.0, line_of_sight: false, memory: 3.0),    // знает свой зал: колонны не прячут
}
```

Зрение: враждебный боец (`Team`) в пределах `sight_range`, в поле зрения (или ближе `near` при любом повороте) и с чистым лучом
от глаз до тела через слой `WORLD`. Слух: `Noise { position, radius, source, origin }` (игра запускает их сама; движок делает шум на
каждое попадание и на каждый спринт) доходит до NPC в пределах `radius × hearing` от `position` и ждёт в его `Perceived`.
Кто не видит источник, узнаёт его место из `origin` (где источник стоит; `None` — в `position`): шум попадания звучит там,
где попали, а нападающего отмечает там, где он стоит. Память:
замеченное остаётся известным `memory` секунд на последнем месте. Цель (фокус) держится за уже известным, пока другой не
окажется намного ближе. Что NPC знает, лежит в его компонентах (`Perceived`) и входит в хэш реплея и снапшот целиком.

### Режиссёр боя и жетоны

```ron
// content/npc/monsters.director.ron
{
    "default": (tokens: {1: 2, 2: 3, 3: 4},                  // жетонов атаки по числу живых игроков
                waiting: [(role: Circle, distance: 3.5, orbit: 1.2), (role: Strafe, distance: 3.0, period: 1.5), (role: Hold, distance: 5.0)],
                patience: 3.0),
    "boss": (tokens: {1: 1}, waiting: [(role: Hold, distance: 2.6)]),           // босс дерётся один
}
```

Атаку начинает только NPC с жетоном, и держит его, пока атака не кончится. Жетонов у режиссёра столько, сколько сказано для
числа живых игроков (больше игроков, чем в таблице, — последняя запись; меньше — первая). Просящих обслуживают по порядку
ожидания, потом по `AiId`. Жетон возвращается, когда атака кончилась, когда его не использовали `patience` секунд (`unused`), когда
держатель умер, уснул (`dormant`), забыл цель (`target gone`), вышел из боя (`left the fight`) или оказался под приказом агента
(`under orders`). Когда живых игроков стало меньше, жетоны сверх нового предела возвращаются сразу, сначала самые новые
(`limit lowered`), кроме тех, чья атака уже идёт: она доходит до конца с жетоном. Атака — действие, которое начал нажатый
вход, а не принудительное (оглушение, вздрагивание, смерть); есть ли у него удары, значения не имеет: каст со снарядом из
эффекта тоже атака и держит жетон до конца, а планировщик помнит её (откат, запреты последовательностей, счётчик).
Цель, которую NPC забыл (она вышла из его восприятия), для режиссёра и планировщика исчезла: планировщик нажимает только на
цель, которую NPC чувствует сейчас. NPC под приказом агента (`move_to`, `attack` инструментом) выпадает из боя: жетонов и мест
ожидания не берёт, ничего не нажимает и отпускает то, что держал.

Без жетона NPC ждёт в **роли** вокруг цели: ждущие получают места по очереди, место `n` берёт роль `n % число ролей` из
`waiting` (`Circle` — ходит по кругу, `Strafe` — шагает вбок, меняя сторону каждые `period` секунд, `Hold` — держится на
расстоянии). Ещё: `budget` — сколько опасности (`cost` атак) может быть в полёте на одну цель; `allowed` — какие пары тегов
могут бить цель одновременно (`("light", "heavy")`, `"*"` — любой тег); их проверяет планировщик при нажатии. У режиссёра нет
своего состояния: всё в компонентах NPC (`AttackToken`, `NpcPlan`), реплей и снапшот несут его целиком.

```text
$ shards scenario crates/sandbox/scenarios/npc_director.scenario   # три скелета на одного игрока
  123 e7 idle → fight e5
  123 e8 idle → fight e5
  123 e14 idle → fight e5
  123 e7 takes an attack token on e5 (1/2)
  123 e8 takes an attack token on e5 (2/2)
  209 e7 -> e5 claw -0 (hp 100, god mode)
  209 e8 -> e5 claw -0 (hp 100, god mode)
  252 e7 returns its attack token after light (0/2)
  252 e8 returns its attack token after light (0/2)
  252 e14 takes an attack token on e5 (1/2)
```

### План атак

Вид `attack_plan` — из чего NPC выбирает следующую атаку. Тот же планировщик у скелета и у босса.

```ron
// content/npc/monsters.attack_plan.ron
{
    "skeleton": (options: [(input: "light", tags: ["light"])], rest: (1.4, 1.9)),
    "warden":   (auto: true, in_range_only: true,                       // любая атака мувсета, что достаёт до цели сейчас
                 bans: [(after: ["zone"], then: ["zone"], within: 1)],  // никогда две «зоны» подряд
                 rest: (1.2, 1.8)),
}
```

Вариант (`AttackOption`): `input` мувсета или `action`, `name`, `weight`, окно дистанции `min`/`max` (по умолчанию достаёт
действие, умноженное на `cover`), `cooldown`, `tags`, `cost` (доля бюджета цели). Планировщик оставляет варианты, которые можно
выбрать (не в откате, не запрещены предыдущими атаками, `can_start` машины действий говорит «сейчас или по ожиданию», влезают
в бюджет и разрешённые сочетания), берёт по весам тот, окно которого держит дистанцию (иначе из всех, если не `in_range_only`)
потоком `SimRng`, подходит в его окно и нажимает вход. Пауза `rest` отсчитывается, пока NPC не занят никаким действием (оглушение
после парирования отдыхом не считается; откаты идут всегда).

### Поводок, дом и зоны

`Home { pos, leash }`: где NPC дома и как далеко он может уйти (метры). Цели NPC держатся внутри поводка (с запасом 0.5 м на
тело); условия `target_in_leash`, `beyond_leash`, `at_home` дают поведению повернуть домой. Скелеты песочницы на арене — 30 м,
страж — 26 м, лагерные скелеты дебрей — 12 м, страж логова — 10 м. NPC в `Dormant` (дальше активного радиуса от всех игроков,
см. «Мир и генерация») не думает и не ходит и просыпается в тот тик, когда игрок подошёл. Условия `step --until`:
`named:npc_attacking` (чья-то атака идёт), `named:npcs_calm` (ни у кого нет цели), `named:npc_beyond_leash` (активный NPC дальше
поводка на метр: шаг, который должен держать NPC в зоне, на нём не должен останавливаться), `named:npc_detour` (NPC идёт
по маршруту из нескольких участков, потому что цель не на виду).

### Навсетка

Каждый тайл (и комната подземелья) **запекается** заранее в сетку проходимых клеток. Геометрия тайла спавнится одна в
пустом мире тем же спавнером, что и регион, и опрашивается теми же зондом земли и правилом шага движения (`step_onto`), что
использует `path_to` и контроллер. Поэтому навсетка, локальный планировщик и контроллер согласны, что можно пройти, перелезть,
спрыгнуть и проплыть. Узел — клетка на одном уровне пола (этажи — разные узлы), связи — ходьба, плавание, перелезание, спуск к
восьми соседям и прыжки через две клетки у подножия уступа.

```text
$ cargo sandbox --bake-nav               # все тайлы, которые используют регионы (или --bake-nav grass,ruin; в игре: tool nav_bake)
baked 9 tiles into nav/
  gate: gate.navtile.ron (1000 nodes, 3 areas, 4 portals)
  grass: grass.navtile.ron (1024 nodes, 1 area, 4 portals)
  hall: hall.navtile.ron (972 nodes, 2 areas, 4 portals)
  pillars: pillars.navtile.ron (944 nodes, 5 areas, 4 portals)
  ruin: ruin.navtile.ron (992 nodes, 4 areas, 4 portals)
  …
```

Запеканка загружает игру без окна и без тиков, пишет файлы `assets/nav/<тайл>.navtile.ron` (комнаты — `.navroom.ron`) и
завершается кодом 0 (все запеклись) или 1 (нельзя запечь или нет такого тайла: `no tile or room `grss` to bake (did you
mean `grass`?)`). Файл — заголовок RON с числами, которые читает человек, и хэш того, из чего он запечён, вокруг
упакованной сетки в base64. Тот же тайл — те же байты: повторный `--bake-nav` ничего не меняет в git.

Настройки — вид `nav` (id `default`):

```ron
// content/region/wilds.nav.ron
{ "default": (cell: 0.5, agent: Some("skeleton"), safe_drop: 2.0) }
```

`cell` — ребро клетки в метрах (ребро тайла должно делиться на неё), `agent` — id `locomotion` тела, под которое пекутся
тайлы (радиус, рост, высота ступеньки, склон, перелезание), `safe_drop` — глубже этого спуск считается обрывом, `dir` — папка
файлов (`nav`), `check_bakes` — проверять ли файлы контентом. **С определением `nav` отсутствующий или устаревший файл — ошибка
контента**, и `shards content check` / `--check-content` называют его:

```text
content/region/wilds.nav.ron: nav `default`: the bake of tile `grass` (nav/grass.navtile.ron) is stale: the tile, its prefabs or level file, or the nav settings changed since it was baked; bake it again with `--bake-nav grass`
```

Хэш файла берёт формат и версию запеканки, определение тайла, префабы его геометрии с теми, что они расширяют (в том числе
префабы узлов файла уровня), байты файла уровня, размер тайла и настройки с телом. Тайл без свежего файла при загрузке региона
всё равно запекается на месте, а в журнале предупреждение с именами:

```text
  0 [warn] navmesh: 5 tiles baked while loading (stale: grass, road_bend, road_end, road_straight, road_t); bake them with `--bake-nav all` (or the nav_bake tool)
```

**Версия запеканки.** `BAKE_VERSION` в `shards_npc::nav::tile` входит в хэш каждого файла. Если вы меняете то, чем запекается
тайл (сэмплирование, зонд земли или правило шага в `shards_motion`), поднимите её: все файлы станут устаревшими, и контент
назовёт каждый. Файлы лежат в репозитории игры.

**Регион собирает навсетку при загрузке** (`NavRegion`, строится внутри вызова, который его грузит, так что ни один тик не видит
регион без неё):

1. по тайлу манифеста берётся его файл (если свежий) или запекается на месте;
2. каждый поставленный тайл — экземпляр запеканки, повёрнутый вместе с тайлом;
3. **вырезание**: твёрдые тела, которых запеканка не видела (декор, реквизит, коллайдеры, появившиеся потом), выключают узлы, где
   тело больше не помещается. Узел выключен, если сейчас стоит такое тело, и это чистая функция тел, стоящих сейчас: навсетка не
   зависит от порядка, в котором они появлялись и уходили; изменения замечаются каждый тик, и связи у краёв решаются заново;
4. **сшивка** по каждому краю, который сокеты оставили открытым (граф региона из `shards_world`): граничный узел получает
   то же движение, которое сделал бы локальный планировщик, в узел соседнего тайла, в том числе прыжки через две клетки (подъём
   или спуск), когда уступ лежит прямо на краю тайла; край с непроходимым сокетом закрыт, какой бы ни была геометрия;
5. **navlink** — маркеры уровня `navlink` с `to` становятся односторонними связями (спуск любой глубины, подъём в пределах
   высоты перелезания тела, шаг в соседнюю клетку); те, что не по силам телу, не используются, а `nav_info` и журнал говорят
   почему (прыжка у тел нет);
6. **граф тайлов**: порталы (на сторону и область каждого тайла), стоимости между ними внутри тайла, связи между тайлами.

Маршрут (`NavRegion::path`): A* по графу тайлов, потом A* по сетке тайлов коридора, потом «натягивание нити» с проверкой по
сетке и телесным ощупыванием физики. Стоимости целые, ничья — меньший id: тот же регион даёт тот же маршрут, и NPC в реплее
идут одинаково. Маршруты не требуют окна. Навсетка рассчитана на одно тело (`agent`): ходок, которому движения этого тела не
под силу (шире или выше, ступенька ниже, перелезает или плавает хуже; `NavRegion::serves`), планирует по локальной сетке,
поэтому `move_to path=true` не ведёт его через перелезание, которое он не сделает (`nav_path from=<он>` планирует так же, по его
собственному телу, а не по телу по умолчанию). Страж песочницы шире скелета и ходит так.
Навсетка, которую не удалось построить, не пробуется заново каждый тик, а только когда сменился регион или контент.

**Запросы и инструменты.**

| инструмент | что делает |
|---|---|
| `nav_path` (игрок, запрос) | маршрут, который выбрал бы NPC, в формате `path_to` плюс тайлы, через которые он идёт (`"provider": "navmesh"`) |
| `nav_info` (разработчик, запрос) | навсетка региона: тайлы, узлы, связи, вырезанное, navlink и какие тайлы запеклись при загрузке |
| `nav_bake` (разработчик, `control`) | то же, что `--bake-nav`: один тайл, несколько или все |
| `nav_overlay` (разработчик, окно) | рисует клетки (зелёные; вода синяя, вырезанное красное), связи через края, navlink, маршруты NPC и границы тайлов |
| `npc_state` (игрок, запрос) | всё об уме NPC: состояние, цель, жетон, план, маршрут, поводок |
| `npc_set_state` (разработчик) | принудительно перевести NPC в состояние его поведения (в журнале `(npc_set_state)`) |
| `move_to path=true` | идёт по маршруту навсетки в регионе, а вне региона по локальной сетке |

```text
$ shards act nav_path from='{"x": 24, "z": 140}' to=e5          # от логова стража к игроку за колоннами
{
  "distance": 16.8,
  "path": "path 16.8m, 3 turns: 6.2m E → 3.5m N → 7.2m NW",
  "provider": "navmesh",
  "tiles": ["(5, 5)"],
  …
}
$ shards act nav_info
{ "agent": {"height": 1.8, "radius": 0.4}, "baked_while_loading": [], "from_files": ["gate", "grass", "hall", …],
  "region": "wilds seed 42 v1", "summary": "64 tiles, 65012 nodes, 7168 links across edges, 0 navlinks", … }
```

`path_to` и `clearance` остаются запросами к физике (локальная сетка в 0.5 м), в регионе маршруты по навсетке дают `nav_path` и
`move_to path=true`. Снапшот навсетку не сохраняет: после загрузки снапшота она строится заново для региона и тел, которые
снапшот вернул.

### Агенты навигации

Откуда NPC берёт маршрут — `NavPaths` (подмена: `fn(&World, from, to, radius) -> waypoints`): по умолчанию навсетка внутри
загруженного региона и локальная сетка в остальных местах. Цель на виду (`plain_sight`: на навсетке прямая линия шагов по
земле, никогда не через обрыв, уступ или вырезанное; вне региона широкий бросок по слою `WORLD` над землёй без дыр и глубоких
спусков) маршрута не требует. Для другой маршрут строится заново, когда цель сдвинулась или каждые `repath` секунд, поэтому
запросов мало, как бы много NPC ни бежало. Нет маршрута — NPC стоит на месте до следующего плана и никогда не идёт напрямик
(`npc_state` → `nav.no_route`). Уклонение: каждый смотрит на `avoid_time` секунд вперёд, куда идут другие, и сворачивает от
точки наибольшего сближения (вправо, если встреча лоб в лоб), плюс расталкивание вплотную. Числа (`hit_noise`, `sprint_noise`,
`avoid_time`, `avoid_margin`, `repath`, `waypoint_reach`) — вид `npc_tuning`, id `default`.

### NPC глазами ИИ

В `observe` у NPC строка состояния (`chase e5 · token · leash 12/20m`, `attack e5 · circle`, `idle (dormant)`), в журнале —
переходы и жетоны:

```text
  2 e25 idle → fight e5
  2 e25 takes an attack token on e5 (1/2)
  183 e25 returns its attack token unused: unused (0/2)
  332 e25 -> e5 claw -0 (hp 100, god mode)
```

`npc_state` целиком (фрагмент): `"state": "fight"`, `"target": "e5"`, `"token": {"attack": "light", "started": true, …}`, `"plan":
{"history": ["light"], "rest": 0.0, …}`, `"nav": {"arrived": true, "direct": true, "waypoints": 0, "no_route": false}`, `"home":
{"at": "(24.0, 140.0)", "distance": 8.51, "leash": 10.0}`.

### Ограничения

- Навсетка запекается под одно тело; другим планирует локальная сетка (это дольше и с ограничением по области поиска).
- У тел нет прыжка и лестниц: `navlink` годится для спуска, подъёма в пределах перелезания и шага.
- Геометрия, которая появляется позже запеканки, вырезается, а не запекается: вырезание умеет только выключать клетки.
  Ступеньку, которой не было в запеканке, навсетка не узнает, пока тайл не запечён заново.
- Правка тайла, файла уровня, префаба его геометрии, тела `agent` или настроек `nav` требует `--bake-nav`; без него контент
  называет устаревший файл, а игра запекает его при загрузке региона.
- Уклонение между NPC простое: упреждающий поворот и расталкивание, без согласованного планирования.

## Слой ИИ

Крейт `ai_core`: всё, чем агент смотрит на игру и действует в ней. Он не зависит от конкретной игры.

### Что видит ИИ

```text
tick 207 (3.45s) | paused | speed 1x | seed 42 | you e5
YOU  e5 Player  hp 100/100  stam 100/100  poise 60/60  flask 3/3  at (0.0, 12.0) facing N
ALERT in hit zone of e6 shockwave (lands in 0.50s): move 1.3m E to leave it
SEEN (by threat)
  e6 Boss "Frost Warden"  hp 600/600  poise 140/140  windup shockwave 0.50s (zone line 9m×1.6m)  phase 1  7.4m N
  e7 Skeleton  ground 3.6m/s  hp 40/40  poise 20/20  4.5m NW
  e8 Skeleton  hp 40/40  poise 20/20  18.4m NE
LANDMARKS  e3 Pillar 8.6m NW · e4 Pillar 8.6m NE · e1 Pillar 20.2m N · e2 Pillar 20.2m N
SINCE seq 2
  0 [agent] dev: step {"until":[{"journal":"winds"}]}
  207 e6 winds up shockwave (0.50s) ≈ line 9m×1.6m
```

- Видимость считается только от субъекта («YOU»): луч от его глаз (`pos + EYE_HEIGHT`) к цели в радиусе
  `AiPerception::radius` (40 м). Луч идёт по физике (слой `WORLD`, коллайдеры) и по `AiOccluder`; если сущность за
  колонной, к ней приписывается `occluded`. Камера окна на это не влияет: с окном и без вывод одинаков, даже пока
  следящая камера догоняет игрока после большого `step`, а после `possess` агент смотрит глазами своего тела.
- NOT IN VIEW — необязательный конус обзора: `AiPerception { fov_deg: Some(120.0), .. }`. Всё, что вне конуса вокруг
  `facing` субъекта, попадает туда. По умолчанию (`None`) обзор круговой и этого раздела нет.
- `scr(x,y)` — отдельная пометка: если в окне есть `AiCamera` и сущность в её кадре, к строке дописывается место на экране,
  например `e8 Skeleton  hp 40/40  6.3m NE  occluded  scr(905,342)`. На SEEN, `occluded` и NOT IN VIEW она не влияет.
  Легенда `shot` по-прежнему считается от камеры: она описывает картинку.
- SEEN отсортирован по угрозе: `AiImportance` по убыванию, потом расстояние.
- ALERT — подсказки модулей для субъекта (`register_ai_advice`): сейчас «ты в зоне удара, выйди на N м».
- SINCE показывает только новые записи журнала. У каждого агента свой курсор.
- Компас: N = −Z, E = +X.

Как игра описывает себя для ИИ:

```rust
app.register_ai_component::<Health>(|h| format!("hp {:.0}/{:.0}", h.current, h.max))
   .register_ai_describer("orders", |e| /* фрагмент из нескольких компонентов */)
   .journal_event::<Damaged>(|e, w| Some(format!("{} -> {} -{:.0}", label(w, e.source), label(w, e.target), e.amount)))
   .add_ai_tool("move_to", Scope::Player, move_to)   // действие: журнал + список изменений
   .add_ai_query("threats", Scope::Player, threats)  // чтение: без журнала
   .add_ai_condition("boss_phase_2", |w| /* ... */)
   .add_ai_status("my_field", |w| /* serde_json::Value в ai/status */)
   .add_ai_invariant("health_in_range", |w| /* Some(текст), если нарушено */)  // см. «Инварианты»
   .replay_input::<Intent>()                        // вход для реплеев (см. «Реплеи»)
   .replay_ignore::<Sparkle>()                      // косметика: не входит в проверки реплея
   .add_replay_check("weather", |w| /* u64 */);     // данные вне AiVisible-сущностей
```

Маркеры сущностей: `AiKind("Boss")`, `AiImportance(10.0)`, `AiLandmark`, `AiOccluder`, `AiPlayer`.

### Инструменты

Инструмент — это структура аргументов (serde + schemars) и обычная Bevy-система `In<Call<Args>> -> ToolResult`. Схема
и сигнатура генерируются автоматически (`shards tools`, `--json` — со схемами; описание берётся из doc-комментариев).

| scope | инструменты |
|---|---|
| **Player** (играет честно, только через `Intent`) | observe, inspect, nearby, line_of_sight, journal, tools, highlight, overlay, content_list, content_get; движение: move_to (`path=true`), move, stop, jump, sprint, glide, clearance, path_to, input_bindings; бой: light_attack, heavy_attack, block, parry, dodge, use_item, use_technique, lock_on, attack; мир: region_info, level_info, nav_path, npc_state |
| **Dev** (всё, включая время) | + pause, resume, speed, step, possess, teleport, despawn, get, set (reflection), spawn_prefab, heal, god, stagger, content_check, content_reload, content_apply, rebind, physics_overlay, combat_overlay, shot, replay_save, snapshot_save, snapshot_load, assert_add, assert_list, assert_remove, assert_reset; мир: regen, region_load, goto_tile, worldgen_report, world_overlay, spawn_level, level_reload, npc_set_state, nav_bake, nav_info, nav_overlay |

Игра добавляет свои (в шаблоне `taunt` и условие `arena_cleared`, в песочнице условия `boss_phase_2`, `boss_dead`,
`player_dead`, `three_attacking`). Модули добавляют условия для `step --until named:…`: `near_poi` (мир, появляется с первым
регионом), `npc_attacking`, `npcs_calm`, `npc_beyond_leash`, `npc_detour` (NPC). У каждого инструмента есть вид (`kind` в `shards tools`):

- `query` только читает мир: без записи в журнал и без диффа;
- `action` меняет мир: пишет в журнал, возвращает `CHANGES` и попадает в реплей;
- `control` управляет временем или сессией (pause, resume, speed, step, possess, content_reload, rebind, snapshot_save,
  snapshot_load, nav_bake): журнал и дифф
  как у action, но в реплей не попадает. Он меняет, *когда* что-то происходит, а не *что*.
  Исключение — правила (`assert_add`, `assert_remove`, `assert_reset`): мир они не меняют, но
  решают, где игра остановится, поэтому реплей записывает их отдельными событиями.

- **Действие возвращает изменения.** Каждый action-инструмент сравнивает срез мира до и после вызова и возвращает
  `CHANGES` и `JOURNAL`. Агенту не нужен второй `observe`, чтобы узнать, что произошло.
- **Ошибки подсказывают, как исправить вызов:**
  ``bad arguments for teleport: missing field `x`. Expected: teleport(id: integer|string, x: number, y?: number, z: number)``,
  `e57 no longer exists (Skeleton despawned at tick 812); live: …`,
  `e7 no longer exists (Skeleton renumbered to e99 at tick 40)`,
  ``unknown component `Mana`; this entity has: ActionState, Body, CharacterController, …``,
  `AiId is managed by the engine; despawn and spawn instead`,
  ``unknown tool `fly`; available: attack, block, clearance, …``.
- **Инструменты боя объясняют отказ.** Нажатие ждёт в буфере ввода, пока действие не станет возможным, а `Intent` остаётся
  чистым входом. Инструмент сразу проверяет то же правило, что и машина действий (`can_start`): можно сейчас — `ok: roll
  (W)`; станет можно в пределах буфера — `ok: queued, heavy starts in 0.10s`; иначе ошибка с причиной и временем, а
  `Intent` не меняется: `not enough stamina: 10/20 (ready in 0.2s)`, `busy: windup heavy 0.50s (ready in 0.90s, when a
  cancel window opens)`. Что чем отменяется, решает контент. Вход, записанный мимо инструментов (клавиатура), у которого
  вышло время, попадает в журнал: `e5 dodge rejected: stamina 7/20`.
- **Паника в инструменте не роняет игру.** Агент получает ошибку, запись уходит в журнал, инструмент перерегистрируется
  при следующем вызове. Расписания тиков при этом на месте; если паника всё-таки унесла их (игра отключила перехват,
  `catch_system_panics: false`), ответ так и говорит: `the game lost its FixedMain, FixedUpdate schedules`. Команда,
  которую поставил инструмент и которая упала, записывается как ошибка инструмента (`tool orphan: command … failed`): это
  не сбой геймплея, игра не встаёт на паузу.

### Время и детерминизм

- Весь геймплей идёт в `FixedUpdate`, по умолчанию 60 Гц, в жёстком порядке `SimSet::Input → Think → Act → Move →
  Resolve → Cleanup` (внутри — наборы модулей, см. «Порядок тика»).
- `SimRng` — счётчиковый RNG по ключу (seed, tick, AiId, salt). Результат не зависит от порядка систем и от того, сколько
  раз кто-то «крутил» генератор.
- `step` синхронно прогоняет `FixedMain` ровно N тиков, игра при этом стоит на паузе. Есть стоп-условия: `journal:TEXT`,
  `gone:ID`, `shows:ID:TEXT`, `near:A:B:M`, `field:ID:Comp.path<N`, `named:NAME`, `warning`, `error` (первая запись журнала
  этого уровня, в том числе `warn!`/`error!` игры). Лимиты: до 216 000 тиков и 20 с реального времени за вызов.
- **Логи внутри `step`.** `warn!`/`error!` попадают в журнал с тиком, на котором записаны, сразу после этого тика, а не в
  конце шага. Поэтому `step --until journal:TEXT` останавливается на тике предупреждения. За тик в журнал идёт не больше
  64 строк, остальные сводятся в одну: `70 [warn] 36 log lines dropped (limit 64 per tick)`. Предупреждение, которое
  вызвал action, приходит в его же `JOURNAL`.
- **Сбой геймплея останавливает игру, а не убивает её.** В Bevy 0.20 паника системы уходит в `FallbackErrorHandler`, а
  тот по умолчанию паникует снова. Эта паника уносит из мира `FixedMain` и `FixedUpdate`: каждый следующий `step` падал
  бы с `cannot run a tick here`, а в реальном времени тики молча переставали бы идти. `AiCorePlugin` ставит свой
  обработчик (`catch_system_panics`, по умолчанию `true`). Он пишет ошибку в лог и в очередь сбоев, а очередь разбирается
  после каждого тика:
  - `step` останавливается на тике сбоя и отвечает `stopped: gameplay system <name> panicked at tick 50: <msg>; crash
    replay: replays/crash-tick50.json`, в журнале — `system <name> panicked at tick 50: <msg>`;
  - в реальном времени игра встаёт на паузу (`[sim] paused: system <name> panicked at tick 50`), следующие тики этого
    кадра не идут;
  - сессия сохраняется в `replays/crash-tick<N>.json` (по одному файлу на упавшую систему). Воспроизвести сбой:
    `cargo sandbox --replay replays/crash-tick50.json` (или `--headless --paused` и `shards step`), игра упадёт на том же
    тике. `--verify` выходит с кодом 1, пока система падает;
  - следующий `step` идёт дальше: расписания на месте.
- Так же останавливает игру нарушенный инвариант: `stopped: invariant <name> violated at tick N` (см. «Инварианты»).
- `AiCorePlugin` включает ambiguity detection для `FixedUpdate`: две системы, которые трогают одни данные без явного
  порядка, дают warning, и он попадает в журнал. Тест `gameplay_systems_have_a_total_order` превращает это в ошибку, а
  контрольный тест проверяет, что сама проверка ловит такую систему.
- Тесты подтверждают: один seed и одни вызовы дают побитово одинаковый мир и журнал; `step 300` даёт то же, что 5 × `step
  60` с `observe` и кадрами рендера между ними.
- Реплей одинаков с окном и без: `--verify --windowed` проигрывает его в окне (рендер, камера, карта ввода, визуал игры
  работают) и даёт тот же отчёт, что и `--verify` без окна. Рендер-сущности не сдвигают ни `AiId`, ни `ColliderId`, ни
  порядок геймплея.

### Задел под сеть (фаза C)

Точки опоры для `shards_net` и сохранений; в одиночной игре они ничего не меняют, реплеи и их хэши прежние.

- **Несколько игр в одном процессе** (`ai_core::process`). У каждого приложения свой `AppToken`; `process::activate`
  делает его активным (это делают `First`, `call_tool`, `sim::step`, `sim::run_tick`, `failures::collect`). Строки лога
  идут в журнал активной игры (`journal::register_capture` для игры без своего `LogPlugin`), паника системы — в её
  очередь сбоев. Игры одного процесса обновляются по очереди, не параллельно.
- **Правила репликации** (`ai_core::net_rules`): реплицируется то, что хэширует реплей (`state_hash`), минус
  `net_ignore::<C>()`; ресурсы — снапшотные минус `net_ignore_resource::<R>()`; `net_predicted::<C>()` — что клиент
  предсказывает для своего тела. `replicated_components(world, e)` и `rules_hash(world)` (для проверки протокола).
  Движок исключает `AiPlayer`, `LocalPlayer`, `Predicted` и `ColliderIds`.
- **Кодек состояния** (`ai_core::state`), общий со снапшотами: `encode_entity`, `encode_replicated`,
  `encode_resources`, `apply_changes(world, &StateChanges)` (недостающие id создаются по порядку, значения пишутся
  дважды, удаления, despawn с пометкой надгробия: `e7 is out of view since tick 180`).
- **Снапшоты и id:** `sim::set_tick`, `snapshot::restore_releasing(world, &snap, gate)` (грузит под удержанным
  `SimGate`; отказ возвращает мир, как и у `restore`), `AiIndex::set_local_base(base)` (свои id клиента — со второго счётчика, id хоста их не сдвигают).
- **Реплей и журнал:** `replay::set_recording(world, false)` (клиент не пишет реплей; `replay_save` говорит почему),
  `replay::restart_from_current`, `journal::set_local_events(world, false)` (события пишет хост),
  `journal::push_remote(world, host_tick, kind, text)` — строка хоста, которую видят `journal`, `observe` и
  `--until journal:`.
- **Маршруты инструментов:** `ToolSpec.net` — `Local`, `Host` (по умолчанию у Dev-действий) или `HostOnly`
  (`snapshot_save`, `snapshot_load`, `replay_save`). С `AiTools::set_forwarder` вызовы `Host` уходят хосту, `HostOnly`
  получают отказ с причиной; без форвардера всё как раньше. `app.net_route(name, route)` меняет маршрут.
- **Агент, привязанный к игроку:** `ai/session {player: N}` и `possess player=N`; тело ищется заново на каждом вызове,
  так что переживает респаун.
- **Отложенный шаг:** с `StepRoute::Deferred` инструменты `step`, `pause`, `resume`, `speed` ставят запрос в очередь и
  отвечают `{"pending": id}`; драйвер стенда выполняет его между кадрами, а `step_poll {id}` (BRP `ai/step_poll`)
  возвращает отчёт ровно как у обычного `step`. Тики драйвер гоняет через `sim::step`, поэтому отложенный шаг встаёт на
  упавшей системе и нарушенном инварианте так же, как обычный, и его отчёт называет остановку (`halted`).
- **Правила на клиенте:** инварианты (`InvariantSystems`) идут только там, где идёт симуляция (`shards_core::simulates`):
  мир клиента следует за хостом, и правила игры проверяет хост. `assert_add`, `assert_list`, `assert_remove` и
  `assert_reset` у клиента с форвардером уходят хосту (`NetRoute::Host`).

### Транспорты и CLI

**BRP `ai/*`.** JSON-RPC 2.0, `POST http://127.0.0.1:15702`:

| метод | params |
|---|---|
| `ai/act` | `{agent?, tool, args?}` |
| `ai/observe` | `{agent?, since_seq?}` |
| `ai/step` | `{agent?, ticks?, until?}` |
| `ai/journal` | `{agent?, since_seq?, last?}` |
| `ai/shot` | `{agent?, marks?, path?}` |
| `ai/tools` | `{agent?}` |
| `ai/session` | `{agent, scope?, subject?, clear_subject?}` |
| `ai/status` | — (в ответе `instance: {pid, seed, port, started_at}`: какая игра ответила; `engine`: версия, модули, состояние; `loading`: что ещё грузится; `perf`: время последних 600 тиков) |

Стандартные методы BRP (`world.query`, `world.get_components`, …) тоже доступны: `shards raw …`. BRP даёт полный доступ к
миру, поэтому сервер слушает только 127.0.0.1, включается feature `remote` и пускает только с токеном:

- при старте игра пишет случайный токен в `.shards/<port>.token` своей рабочей папки (права 0600) и удаляет файл при
  выходе: по Ctrl+C, `kill`, закрытию окна или терминала (если игра запущена не через `nohup`). После `kill -9` файл
  остаётся, следующая игра на этом порту его перепишет;
- каждый метод — `ai/*`, стандартные `world.*`, `registry.*`, `rpc.discover` и `+watch` — требует `params.token`. Без него
  или с чужим токеном ответ один: `unauthorized: run shards from the game folder or set SHARDS_TOKEN`;
- `shards` берёт токен из `SHARDS_TOKEN` (`--token`) или из `.shards/<port>.token`, поднимаясь от текущей папки вверх, и
  сам добавляет его к параметрам. Своему клиенту передайте его явно:

```bash
TOKEN=$(cat .shards/15702.token)
curl -d '{"jsonrpc":"2.0","id":1,"method":"ai/status","params":{"token":"'$TOKEN'"}}' http://127.0.0.1:15702
```

Релизная сборка без BRP (и без наблюдателя за файлами):

```bash
cargo build --release -p sandbox --no-default-features --features render
cargo build --release -p sandbox --no-default-features --features remote   # только для --headless (сервер, генерация)
```

**CLI `shards`:** observe, step, act, scenario, shot, save-replay, save-snapshot, load-snapshot, assert, fuzz, tools, inspect, nearby,
journal, pause, resume, speed, status, session, content, worldgen, raw, wait. Аргументы `act` передаются JSON-объектом или парами `key=value`. Флаги
`--port`/`SHARDS_PORT`, `--port-file`/`SHARDS_PORT_FILE` (порт из файла игры; `--port` важнее), `--token`/`SHARDS_TOKEN`,
`--agent`/`SHARDS_AGENT`, `--json` (у `tools` — со схемами аргументов). `shards wait` ждёт, пока игра ответит и
загрузится (контент; пока идёт загрузка, `step` отвечает ошибкой), и печатает, кто ответил: `ready: tick 0 · pid 4242 ·
seed 1 · port 15711` (ту же строку игра печатает при старте, её же первой строкой выводит `shards status`). `shards
content check` перечитывает файлы контента запущенной игры и выходит с кодом 1 при ошибках; без запущенной игры —
`--check-content`. `shards worldgen --region R --seeds 1..1000 --report` печатает пакетный отчёт генератора запущенной игры
(код 1, пока прошли не все зёрна); без игры — `--worldgen-report R --seeds …` самой игры.
Коды выхода — ниже.

**Параллельные headless-инстансы.** У каждого свой порт:

```bash
cargo sandbox --headless --paused --seed 1 --port 15711 &
cargo sandbox --headless --paused --seed 2 --port 15712 &
SHARDS_PORT=15711 cargo shards step 600 --until named:player_dead
```

- Занятый порт — ошибка: игра не запускается, пишет `port 15711 is busy (another game?). Pick another with --port` и
  выходит с кодом 3. Без этой проверки вторая игра работала бы без сервера, а `shards` молча управлял бы первой. Проверка
  доступна любой игре: `ai_core::remote::ensure_port_free(port)`.
- Чтобы не выбирать порты вручную, `--port 0` берёт свободный порт, а `--port-file F` записывает его номер в файл (при
  выходе по Ctrl+C, `kill` или закрытию окна файл удаляется). `shards` читает его через `--port-file F` или
  `SHARDS_PORT_FILE=F`:

```bash
cargo sandbox --headless --paused --seed 1 --port 0 --port-file /tmp/shards-a.port &
cargo sandbox --headless --paused --seed 2 --port 0 --port-file /tmp/shards-b.port &
SHARDS_PORT_FILE=/tmp/shards-a.port cargo shards wait   # ready: tick 0 · pid 4242 · seed 1 · port 40817
SHARDS_PORT_FILE=/tmp/shards-a.port cargo shards step 600 --until named:player_dead
```

**Внутри игры** та же точка входа, что за BRP, — `ai_core::call_tool(world, agent, tool, args)`. Её вызывают тесты,
реплеи и клавиши разработчика в окне.

**Коды выхода.** `shards`:

| код | значит |
|---|---|
| 0 | готово |
| 1 | инструмент отказал; ошибка говорит, почему и как исправить вызов (`content check` и `worldgen` — ещё и когда нашли ошибки) |
| 2 | сбой самого `shards`: игры нет на порту, нет токена, неверная командная строка |
| 3 | игра остановилась: `step` (или `fuzz`) встал на упавшей системе геймплея или нарушенном инварианте; ответ называет причину и реплей |

У `shards scenario` свои: 0 — все прошли, 1 — сценарий не прошёл, 2 — файл не читается или игра недоступна (см. «Сценарии»).

`cargo sandbox` (и любая игра на `shards::run`):

| код | значит |
|---|---|
| 0 | игра закрыта; с `--verify` — реплей совпал с записью, ничего не сломалось; `--check-content`, `--worldgen-report` — ошибок нет |
| 1 | `--verify`: игра разошлась с записью, упала система геймплея или нарушен инвариант; `--check-content`, `--worldgen-report` — нашли ошибки |
| 2 | реплей не читается или не подходит к игре; `--port-file` не записать |
| 3 | порт занят (`port 15711 is busy (another game?)`) |
| 128 + N | остановлена сигналом N: 130 — Ctrl+C, 143 — `kill`, 129 — закрытый терминал (SIGHUP) |

**Пример сеанса (реальный вывод).** Под первой строкой ответа `step` — время его тиков (см. «Время тика»).

```text
$ shards step --until journal:winds
ran 208 ticks (0 → 208), stopped: journal: 208 e6 winds up shockwave (0.50s) ≈ line 9m×1.6m
ticks: avg 1.49 ms, max 3.89 ms (tick 120), budget 16.7 ms
CHANGES
  ~ e6 Boss "Frost Warden": +windup shockwave 0.50s (zone line 9m×1.6m); +fight e5 · token · leash 11/26m; moved 10.6m S
  …
JOURNAL
  2 e6 guard → fight e5
  2 e6 takes an attack token on e5 (1/1)
  208 e6 winds up shockwave (0.50s) ≈ line 9m×1.6m
$ shards act dodge
ok: roll (W)
$ shards step 40
ran 40 ticks (208 → 248)
ticks: avg 1.43 ms, max 3.20 ms (tick 242), budget 16.7 ms
CHANGES
  ~ e5 Player: stam 100/100 → stam 88/100; moved 4.0m W
  ~ e6 Boss "Frost Warden": windup shockwave 0.50s (zone line 9m×1.6m) → active shockwave 0.08s (zone line 9m×1.6m)
$ shards act attack target=e6 repeat=true
ok: attacking e6 with light until it dies
CHANGES
  ~ e5 Player: +order attack e6 (repeat)
$ shards step --until journal:light_2
ran 101 ticks (248 → 349), stopped: journal: 349 e5 -> e6 light_2 -22 (hp 558)
ticks: avg 1.28 ms, max 3.55 ms (tick 338), budget 16.7 ms
CHANGES
  ~ e5 Player: stam 88/100 → stam 84/100; +active light_2 0.10s (zone cone 2.2m 110°); moved 6.2m NE
  ~ e6 Boss "Frost Warden": hp 600/600 → hp 558/600; poise 140/140 → poise 114/140; fight e5 · token · leash 11/26m → fight e5 · hold · leash 11/26m; -active shockwave 0.08s (zone line 9m×1.6m)
  …
JOURNAL
  296 e8 idle → fight e5
  …
  331 e5 -> e6 light_1 -20 (hp 580)
  349 e5 -> e6 light_2 -22 (hp 558)
$ shards act heavy_attack
ok: queued, heavy starts in 0.10s
```

### Скриншоты

`shot` (scope Dev) снимает окно игры средствами самого Bevy, это не захват рабочего стола. Поэтому окно без фокуса, которое
композитор перерисовывает с задержкой, всё равно даёт текущий кадр.

```text
$ shards shot
saved /…/shots/tick90-1.png (1280x720, tick 90, marks on)
LEGEND (image pixels)
  e1 Pillar  px(496,182)
  e5 Player  px(640,387)
  e6 Boss "Frost Warden"  px(640,207)
  e7 Skeleton  px(349,260)  occluded
  …
```

- Оверлей с id включается на время снимка (кадр до съёмки, чтобы гизмо-линии успели дойти до GPU). Сам снимок ждёт, пока GPU
  докомпилирует пайплайны: сразу после старта меши и метки иначе пропускаются.
- Легенда считается в том кадре, который снимается. После `step` следящая камера ещё догоняет игрока, и координаты на момент
  запроса были бы неверны. Легенда пишется рядом с PNG как `<имя>.png.legend.json` (`path`, `width`, `height`, `tick`,
  `marks`, `legend`): такое имя не столкнётся с чужим `<имя>.json`.
- Снимки пишутся только внутрь `shots/` рабочей папки игры: `-o shots/boss.png` можно, абсолютный путь, `..` и выход через
  симлинк — нет (`path must be inside shots/, e.g. shots/boss.png`).
- Оба файла пишутся во временные и переименовываются, так что появившийся PNG уже целиком на диске. Старый снимок и его
  легенда с тем же именем удаляются при запросе, и только внутри `shots/`. В журнале появляется `screenshot saved: …`.
- Инструмент сразу возвращает пути, `shards shot` ждёт файл (`--timeout`, по умолчанию 10 с). Флаги: `--no-marks`, `-o PATH`.
  Без окна (`--headless`) инструмент отвечает ошибкой с подсказкой использовать `observe`.
- Снимки требуют окна и GPU, поэтому в `cargo test` их нет. Без дисплея их можно сделать через Xvfb и программный Vulkan
  (`apt install xvfb mesa-vulkan-drivers libxkbcommon-x11-0`):

```bash
xvfb-run -a cargo sandbox --paused --port 0 --port-file /tmp/shards.port &
SHARDS_PORT_FILE=/tmp/shards.port cargo shards wait && cargo shards shot
# видимость не зависит от камеры: после шага с движением два observe подряд дают одинаковые
# SEEN и NOT IN VIEW, хотя камера ещё догоняет игрока (меняются только scr(x,y))
cargo shards act move dir=W meters=9 && cargo shards step 300
cargo shards observe; sleep 1; cargo shards observe
```

### Агенты

Агент — внешний агент-кодер (Claude Code, Codex, …) с терминалом. Ему не нужны ни SDK, ни определения инструментов: он
вызывает `shards` через bash и читает вывод как текст.

```bash
cargo sandbox --headless --paused &      # игра стоит, пока агент думает
cargo shards wait
cargo shards observe                     # что видно
cargo shards tools                       # что можно сделать (--json: со схемами аргументов)
cargo shards act attack target=e6 repeat=true
cargo shards step --until journal:winds  # в ответе CHANGES и JOURNAL
cargo shards shot                        # визуальный баг: PNG с метками и легенда (нужно окно)
```

- Время стоит, пока агент думает: на паузе тики идут только по `step`.
- По умолчанию у агента scope `Dev`. Чтобы проверить, как игра играется честно, переключите его в `Player` (`shards session
  --scope player`): тогда он ходит только через `Intent`, как человек.
- У каждого агента своё имя (`--agent`/`SHARDS_AGENT`), а с ним свой курсор журнала, scope и «тело». Агенты могут работать
  с одной игрой или каждый со своим headless-инстансом на своём порту.
- Найденный баг сохраняется реплеем (`shards save-replay`), его воспроизводит человек или CI.
- Гипотезу о баге проверяет правило: `shards assert add NAME COND`, затем `step` или
  `shards fuzz`. Игра остановится на тике нарушения и сама сохранит реплей (см. «Инварианты»).
- Движок не запускает модели и не ходит к их API. Всё, что нужно агенту, добавляется как инструмент
  или команда `shards`, а не как код, который думает за агента.

## Сценарии (E11)

Сценарий — текстовый файл: команды `shards`, ожидания и инварианты. Один и тот же файл идёт в `cargo test` без окна и против
запущенной игры (`shards scenario FILE...`). Смысл у него одинаковый: формат и все правила (что делает команда, как
проверяется ожидание, откуда считает `mark`) лежат в `shards_script`, а у двух запускателей только транспорт:
`ai_core::call_tool` и BRP `ai/*`.

```text
# Комментарий. Пустые строки пропускаются. Слова режутся как в shell: "…", '…' (JSON берите в одинарные).
seed 42                                    # необязательно, первой строкой: игра должна идти с этим seed
act attack target=e6 repeat=true           # вызвать инструмент (как `shards act`); ошибка вызова проваливает сценарий
act? teleport id=e5                        # то же, но ошибка допустима; её текст читает `expect result`
step 120                                   # N тиков (по умолчанию 1, а с --until 600)
step 900 --until near:e5:e6:3 -u journal:died   # условия те же, что у `shards step`
observe                                    # срез мира; его текст становится «последним результатом»
mark                                       # журнал для expect считается отсюда (иначе с начала сценария)
session --agent bot --scope player         # кто выполняет следующие команды; ещё --subject ID, --clear-subject

expect result contains "ok: attacking"     # текст результата последнего act/act?/step/observe; регистр важен
expect result !contains "error"
expect stopped "boss_phase_2"              # последний step остановило условие, в описании которого есть текст
expect journal "e6 phase 1->2"             # запись журнала с начала сценария или с `mark`
expect !journal "died"
expect field e5 Health.current > 0         # число через `get`; операции < <= > >= == !=
expect shows e6 "phase 2"                  # то, что `inspect` пишет о сущности над списком компонентов

invariant field e5 Health.current > 0      # любое ожидание; проверяется после каждого следующего act/act?/step
```

- **Кто что делает.** Команды идут от агента `scenario` со scope `dev` (`session --agent` переключает его; флаг `--agent` у
  `shards` на сценарии не действует). Ожидания и инварианты читают мир другим агентом, `scenario-check` со scope `dev`:
  сценарий может играть за агента со scope `player` и всё равно видеть всё, а проверки не двигают курсор журнала
  проверяемого агента. Записи журнала с вызовами агентов (`[agent] …`) в `expect journal` не попадают: `step --until
  journal:died` не найдёт сам себя.
- **Регистр.** `expect result` различает регистр, чтобы можно было проверить точный текст ошибки (`Expected: teleport(`).
  `journal`, `stopped` и `shows` не различают, как `--until journal:`/`shows:`.
- **Инварианты** проверяются после каждой следующей команды `act`, `act?` и `step`, а не между тиками одного `step`.
  Первое нарушение проваливает сценарий и называет команду, которая его вызвала: `invariant `field e5 Health.current > 0`
  (line 12) broke: e5 Health.current is 0, which is not > 0`. Длинный шаг режьте на части или сторожите условием: `step 600
  --until field:e5:Health.current<1`.
- **Оборванный шаг** проваливает сценарий, хотя вызов `step` успешен: упала система геймплея (`stopped: gameplay system …
  panicked`), нарушен инвариант (`stopped: invariant … violated`, см. «Инварианты») или тики не уложились в 20 с
  реального времени. Падение или нарушение, которого сценарий ждёт, разрешает `act? step …`.
- **`expect !journal`** (и такой же инвариант) проваливается, если часть записей с `mark` вытеснена из буфера журнала и не
  прочитана: среди них могла быть искомая. Ставьте `mark` чаще.
- **Ошибки разбора** называют строку: ``obstacle.scenario:12: unknown command `stpe`; expected act, act?, step, …``.
  Показываются сразу все плохие строки, и пока они есть, не запускается ничего.
- **Мир берётся как есть.** Сценарий не перезапускает игру. Чтобы файл шёл и на свежей игре, и после других, он сам
  приводит мир в порядок первыми командами (`heal`, `teleport`, `set`, `npc_set_state … calm`, `regen`): так сделаны все
  `crates/sandbox/scenarios/*.scenario`, они идут на свежей игре и после других, вперёд и назад (CI проверяет оба порядка).

В тесте:

```rust
let mut app = sandbox::test_app(42);
let report = ai_core::scenario::run_file(app.world_mut(), "scenarios/boss_fight.scenario");
assert!(report.passed, "{report}");   // отчёт — построчный журнал и вердикт
```

Против игры (`--quiet` печатает только упавшие команды и вердикты, `--json` — отчёты в JSON):

```text
$ shards scenario --quiet crates/sandbox/scenarios/*.scenario
PASS crates/sandbox/scenarios/boss_fight.scenario (50 commands, 422 ticks)
PASS crates/sandbox/scenarios/combat_prototype.scenario (215 commands, 2214 ticks)
PASS crates/sandbox/scenarios/dodge.scenario (74 commands, 591 ticks)
PASS crates/sandbox/scenarios/errors.scenario (38 commands, 2 ticks)
PASS crates/sandbox/scenarios/npc_director.scenario (38 commands, 1923 ticks)
PASS crates/sandbox/scenarios/npc_navmesh.scenario (31 commands, 1271 ticks)
PASS crates/sandbox/scenarios/obstacle_course.scenario (86 commands, 1010 ticks)
PASS crates/sandbox/scenarios/region.scenario (40 commands, 353 ticks)
PASS crates/sandbox/scenarios/saves.scenario (76 commands, 157 ticks)
PASS crates/sandbox/scenarios/snapshot.scenario (51 commands, 365 ticks)
10 of 10 scenarios passed
```

Без `--quiet` каждая команда печатается строкой `ok  39 step 900 --until near:e5:e6:3.2 → ran 129 ticks (150 → 279),
stopped: e5 within 3.2m of e6`. Код выхода: 0 — все прошли, 1 — сценарий не прошёл (`FAIL file:line: сообщение`), 2 —
файл не читается или не разбирается, игра недоступна. Если игра пропала посреди файла, остальные файлы не запускаются.

## Инварианты (E11)

Инвариант — именованное правило, которое проверяется после каждого тика: и в `step`, и в реальном времени. Нарушенное
правило останавливает игру так же, как упавшая система:

- в журнале ошибка `invariant <name> violated at tick N: <текст>`;
- в реальном времени игра встаёт на паузу (`[sim] paused: invariant <name> violated at tick N`), следующие тики этого кадра
  не идут;
- сессия сохраняется в `replays/invariant-<name>-tick<N>.json`;
- `step` останавливается на этом тике и отвечает `stopped: invariant <name> violated at tick N (replay:
  replays/invariant-<name>-tick<N>.json)`, в ответе есть поле `halted`, а `shards` выходит с кодом 3. Отложенный шаг
  (`StepRoute::Deferred`) встаёт так же.

Правила игры задаёт код. Функция получает `&World` и возвращает текст нарушения или `None`:

```rust
app.add_ai_invariant("health_in_range", |w| /* Some("e6 Health.current = 700 outside 0..=600") */);
```

В песочнице их два: `health_in_range` (здоровье в `0..=max`) и `bodies_in_world` (каждый персонаж над миром: над ареной с
коридором, полосой препятствий или загруженным регионом, с запасом `margin`, и не ниже `fall_below`; числа — в
`world/world.bounds.ron`, площади — из построенных layout и манифеста региона): `e7 outside the world: x = 200.0, z = 0.0`,
`e7 fell out of the world: y = -80.0`. Ни один сценарий, тест и эталонный реплей песочницы их не нарушает. Паника внутри
правила тоже нарушение: `the rule itself panicked: …`.

Агент добавляет свои правила на время сессии (scope Dev), условием в синтаксисе `--until`. Реальный вывод:

```text
$ shards assert add hp 'field:6:Health.current<500'      # остановиться, когда у стража меньше 500 hp
added hp: the game halts when field:e6:Health.current<500
$ shards assert add calm error                            # … на первой записи [error] в журнале
added calm: the game halts when error
$ shards assert list
4 rules, checked after every tick
  health_in_range  game rule  armed
  bodies_in_world  game rule  armed
  hp  when field:e6:Health.current<500  armed
  calm  when error  armed
$ shards act attack target=e6 repeat=true
ok: attacking e6 with light until it dies
$ shards step 600                                          # код выхода 3
ran 230 ticks (0 → 230), stopped: invariant hp violated at tick 230 (replay: replays/invariant-hp-tick230.json)
ticks: avg 0.76 ms, max 3.55 ms (tick 108), budget 16.7 ms
…
JOURNAL
  …
  230 e5 -> e6 light_2 -22 (hp 486)
  230 [error] invariant hp violated at tick 230: e6 Health.current = 486
  230 [sim] invariant replay saved: replays/invariant-hp-tick230.json (play it with --replay to reproduce)
$ shards assert reset                                      # снова включить сработавшие правила
re-armed hp
$ shards assert remove bodies_in_world                     # код выхода 1
error: `bodies_in_world` is one of the game's invariants and stays; after it fires, `assert_reset` re-arms it
$ cargo sandbox --replay replays/invariant-hp-tick230.json --verify    # код выхода 1
invariant hp violated at tick 230: e6 Health.current = 486
replay finished at tick 230: identical to the recording (4 checks); 1 invariant violated [230 ticks in 0.46s]
```

- Сработавшее правило выключено до `assert_reset`, иначе оно срабатывало бы на каждом тике, пока мир не исправлен.
  `assert_reset` включает все сработавшие правила (игровые тоже), и проверка продолжается со следующего тика.
- `warning` и `error` срабатывают на запись журнала этого уровня, в том числе на `warn!`/`error!` игры. Запись о нарушении
  другого правила их не будит.
- Имя правила — до 64 букв, цифр, `_` и `-`: из него получается имя файла реплея. Правило игры нельзя ни удалить, ни
  заменить своим.
- `assert add/remove/reset` записываются в реплей (события `assert`) и повторяются на том же тике, поэтому реплей,
  сохранённый при нарушении, нарушает правило на том же тике. Сами правила не входят ни в хэш, ни в снапшот: загрузка
  снапшота (и проигрывание с него) оставляет правила сессии, включает сработавшие заново, а запись с этого тика начинается
  с событий `assert`, которые добавляют правила сессии, — так реплей со снапшота проверяет те же правила.
- `--verify` перечисляет нарушения, случившиеся при проигрывании, идёт до конца записи и выходит с кодом 1, если они были.
- Реплей пишется по одному на нарушение и только пока идёт запись (клиент сети его не пишет). Тестам он не нужен:
  `crash_replays: false` у `EngineConfig` выключает и его (или `AiInvariants.replays = false`).
- Правила проверяет тот, кто считает мир: на клиенте сети `InvariantSystems` не идёт (см. «Задел под сеть»).

**Фаззинг.** `shards fuzz --seed S --steps N` подаёт N случайных входов игрока (ходьба, удар, уворот; цели берутся из
`observe`), после каждого — `step` на 1–30 тиков (`--max-ticks`). Тот же seed против той же свежей игры даёт те же вызовы.
Если правило нарушилось, `fuzz` печатает ответ `step` и путь к реплею нарушения и выходит с кодом 3; если нет — сохраняет
сессию в `replays/fuzz-seed<S>.json`. Реальный вывод:

```text
$ shards fuzz --seed 1 --steps 50
fuzz seed 1: 50 inputs, nothing broke (ticks 0 → 857)
replay: /…/replays/fuzz-seed1.json
$ shards assert add low 'field:5:Health.current<50'       # в свежей игре
$ shards fuzz --seed 3 --steps 300                        # код выхода 3
fuzz seed 3: the game halted after 177 inputs (ticks 0 → 2752)
ran 23 ticks (2729 → 2752), stopped: invariant low violated at tick 2752 (replay: replays/invariant-low-tick2752.json)
…
replay: replays/invariant-low-tick2752.json
$ cargo sandbox --replay replays/invariant-low-tick2752.json --verify    # код выхода 1
invariant low violated at tick 2752: e5 Health.current = 42
replay finished at tick 2752: identical to the recording (46 checks); 1 invariant violated [2752 ticks in 2.45s]
```

## Время тика (E11)

Каждый прогон `FixedMain` (один тик) замеряется по реальным часам, и в `step`, и в обычной игре. Замер оборачивает
`run_fixed_main` своего приложения, поэтому в него входят все системы `FixedFirst` … `FixedLast`, игровые тоже, а несколько
игр в одном процессе меряют каждая своё. Последние 600 тиков (10 с при 60 Гц) хранит ресурс `AiPerf`. Ответ `step` получает
строку о своих тиках:

```text
$ shards step 600
ran 600 ticks (0 → 600)
ticks: avg 0.52 ms, max 2.60 ms (tick 540), budget 16.7 ms
```

- Бюджет по умолчанию — длительность тика: 16.7 мс при 60 Гц. Свой задаёт `--tick-budget-ms 5`, в своей игре —
  `EngineConfig { tick_budget: Some(Duration::from_millis(5)), .. }`. Тики дольше бюджета `step` считает в той же строке:
  `…, budget 5.0 ms, 3 over budget`.
- Тики над бюджетом попадают в журнал отдельным видом записей `perf`, не чаще раза в игровую секунду. Первая запись появляется
  сразу, следующие тики копятся и уходят одной записью, когда с прошлой пройдёт 60 тиков: `150 [perf] 12 ticks over budget,
  worst 31.4 ms at tick 150`. Так же и в реальном времени, где `step` нет.
- `ai/status` и `shards status` показывают поле `perf` за последние 600 тиков (`ticks` — сколько их набралось):
  `{"avg_ms": 0.522, "budget_ms": 16.667, "max_ms": 2.598, "over_budget": 0, "ticks": 600, "worst_tick": 540}`.
- Первый тик дольше остальных: на нём Bevy строит расписания.
- Замеры не входят ни в хэш состояния, ни в реплей, ни в снапшот: время у каждого прогона своё, а проигрывание сверяет игру.
- Тестовые приложения (`shards::testing::config`) не выводят строку `ticks:` и не пишут записи `perf` (`report_perf: false`
  у `EngineConfig`): сценарии сравнивают ответы и журналы целиком. Замер при этом идёт, и `status` его показывает. Тест,
  которому время нужно, включает вывод сам (`AiPerf::report`), как `crates/sandbox/tests/perf.rs` (отдельный бинарник: остальные тесты не сбивают ему часы).
- Отладочная сборка медленнее релизной. Точный бюджет проверяйте релизной: `cargo run -p sandbox --release -- --headless
  --paused --tick-budget-ms 5`, затем `shards step 600`.
- Где именно уходит время внутри тика, покажет профайлер [Tracy](https://github.com/wolfpld/tracy) через фичу Bevy
  `trace_tracy`: каждая система — отдельная зона. Игре на `shards` её пока не пробросить одним флагом (у неё нет своей
  зависимости от `bevy`, а фича `shards` для этого ещё не заведена).

## Реплеи

Запись идёт всегда, с тика 0, и почти ничего не стоит. В неё попадает:

- **входы**: компоненты, зарегистрированные через `replay_input` (`Intent` движок регистрирует сам, игра добавляет свои).
  Сохраняется каждое изменение, сделанное *между* тиками (клавиатура, сетевой клиент, инструмент агента), с тиком, к
  которому оно относится. Изменения, которые делают системы геймплея внутри тика, детерминированы и не хранятся.
  Компонент клонируется, только когда он изменился;
- **action-вызовы** (`teleport`, `set`, `spawn_prefab`, `attack`, `content_apply`, `regen`, `region_load`, `goto_tile`,
  `spawn_level`, `level_reload`, `npc_set_state`, …) вместе со scope и «телом» агента. Вызовы,
  сделанные изнутри другого инструмента, не пишутся: их повторит внешний вызов. `control`-инструменты (pause, step,
  possess, content_reload, rebind) не пишутся. Сырые правки через BRP (`world.insert_components`) идут мимо инструментов
  и тоже не пишутся. Правка контента на лету записывается как `content_apply` на своём тике;
- **правила**: `assert_add`, `assert_remove`, `assert_reset` с тиком, на котором их вызвали (см. «Инварианты»);
- **проверки**: хэш игровых данных каждые `check_every` тиков (60 по умолчанию) и в конце записи: для каждой сущности с
  `AiVisible` по порядку `AiId` — её id и все отражаемые игровые компоненты (сериализация через reflection, компоненты в
  порядке имени типа); плюс значения `add_replay_check` (хэш базы контента среди них). Рядом с хэшем лежит его разбивка,
  посчитанная в том же проходе: хэш каждого компонента каждой сущности и каждое значение `add_replay_check` под своим
  именем. По ней отчёт называет, что именно разошлось.

```bash
shards save-replay                          # или ai/act {"tool": "replay_save", "args": {"path": …}}
shards save-replay replays/boss-fight.json  # своё имя, но только внутри replays/
cargo sandbox --replay replays/tick332.json # окно, реальное время; seed и частота из файла
cargo sandbox --replay replays/tick332.json --headless --paused   # дальше шагами через shards step
cargo sandbox --replay tests/replays/obstacle_course.json --verify
# replay finished at tick 1010: identical to the recording (17 checks) [1010 ticks in 1.37s]
xvfb-run -a cargo sandbox --replay tests/replays/obstacle_course.json --verify --windowed  # в окне, по 60 тиков на кадр
# replay finished at tick 1010: identical to the recording (17 checks) [1010 ticks in 3.00s, 22 frames]
```

**Эталонные реплеи** лежат в `tests/replays/` (папка в git, в отличие от `replays/`): полоса препятствий, бой-прототип,
директор жетонов (`npc_director`), страж логова на навсетке (`npc_navmesh`), смена регионов (`region`) и реплей со снапшота
(`snapshot`), сыгранные их сценариями на свежей игре. Тест песочницы проверяет каждый файл самой игрой (`sandbox --replay FILE
--verify`), CI — ещё и в окне под Xvfb. Шаблон хранит свои в `template/tests/replays/` (`smoke`, `valley`). Если геймплей или контент
поменялся намеренно, запишите их заново: `cargo test -p sandbox --test sandbox -- --ignored record_reference_replays`
(для шаблона — `cargo test --test game record_reference_replays -- --ignored`).

- Игровые данные определяет то же правило, что и в `inspect`: всё, кроме типов `bevy_*` и внутренних маркеров `ai_core`,
  плюс `Transform` и `Name`. Поэтому компоненты, которые добавляет только окно (`Visibility`, `GlobalTransform`, меши), хэш не
  меняют, а `Entity` внутри компонента хэшируется как `AiId`. Подписи и describe-фрагменты — это текст для ИИ, в хэш они не
  входят: новая формулировка или новый describer не ломают старые реплеи.
- Состав хэша задаёт игра: `replay_ignore::<C>()` исключает компонент (косметика, кэши), `add_replay_check(name, fn(&World)
  -> u64)` добавляет данные вне `AiVisible`-сущностей (снаряды без id, ресурсы). Для состояния, которого может не быть, —
  `add_optional_replay_check(name, fn(&World) -> Option<u64>)`: пока функция возвращает `None`, хэш такой же, как без
  проверки (так модуль уровней хэширует `level`: игра без уровней хэширует как раньше, и хэш не зависит от того, были ли
  уровни прежде). Компонент, который не сериализуется, один раз попадает в журнал (`replay checks leave out …`) и дальше
  пропускается.
- `replay_save` пишет только внутрь `replays/` рабочей папки игры (вложенные папки можно). Абсолютный путь, `..` и симлинк,
  который ведёт наружу, — ошибка с подсказкой: `path must be inside replays/, e.g. replays/boss-fight.json`.
- Проигрывание требует свежего мира: тик 0, тот же seed и та же частота тиков. События применяются между тиками, ровно там,
  где они случились. Входы, которых нет в записи (человек жмёт клавиши во время просмотра), откатываются. Реплей, начатый
  до загрузки контента, создаёт мир из контента, с которым был записан.
- Каждая проверка сравнивается с записью. Отчёт называет первую проверку, на которой игра перестала совпадать, и что в ней
  разошлось: `DIVERGED at tick 60: e6 Boss: Health differs` (подробнее — в «Что разошлось»). После изменения кода это
  регрессия. `--verify` годится для CI: код выхода 0 — совпало и ничего не нарушено, 1 — разошлось, упала система геймплея
  или нарушен инвариант (нарушения перечислены выше вердикта), 2 — реплей не читается или не подходит к игре.
- В конце записи игра становится живой: реплей работает и как точка сохранения для ветвления. Любой action-вызов во время
  проигрывания перехватывает управление и завершает его (`replay interrupted at tick 130 of 332: dev took over
  (teleport)`).
- Во время проигрывания запись продолжается, поэтому ветка сохраняется одним реплеем.
- Состояние записи и проигрывания видно в `ai/status` (поле `replay`), итог пишется в журнал.
- Формат один: JSON `shards-replay/3` — `seed`, `tick_hz`, `check_every`, `check_detail`, необязательный `start`
  (снапшот, с которого реплей начинается), `end`, `events`, `checks`; хэши hex-строками. События — `call`, `input` и
  `assert` (правила). У проверки, кроме `tick` и `hash`, есть `parts`: `{"entities": {"e6": {"Health": "…", "Transform":
  "…"}}, "game": {"content": "…"}}`. Всё, что добавлено к формату, необязательно: файл `/3` без `check_every` и
  `check_detail` (так их писала сборка до разбивки) читается как проверки раз в 60 тиков без разбивки и проверяется по
  общему хэшу, а расхождение в нём называет только тик (`no details: the recording keeps hashes only (check detail none)`).
  Общий хэш считается так же, байт в байт, при любой подробности. Реплеи `shards-replay/2` (всегда с тика 0, без правил,
  интервала и разбивки) эта сборка тоже проигрывает и проверяет по общему хэшу (`no details: replay format 2`). Реплей
  `shards-replay/1` хэшировал текст и этой сборкой не проверяется: `play` отвечает `this replay was recorded by an older
  build (…); record it again`, а `--verify` выходит с кодом 2.
- Реплей начинается с тика 0 или со снапшота: см. «Снапшоты». Реплей со снапшота хранит ту же разбивку проверок.
- Эталонные реплеи (`tests/replays/` песочницы и шаблона) записаны с разбивкой по компонентам: упавший в CI `--verify`
  сразу называет сущность, компонент и значение. Файлы от этого крупнее: у песочницы 64–380 КБ, у шаблона 16–240 КБ.

### Что разошлось

Отчёт, журнал и вывод `--verify` называют сущности и компоненты первой разошедшейся проверки, а ниже — текущее значение
каждого такого компонента в живой игре, одной строкой. Реальный вывод: реплей записан с `--check-every 1`, потом в копиях
файла поменяли аргумент записанного вызова.

```bash
cargo sandbox --headless --paused --check-every 1 --port 0 --port-file /tmp/g.port &
cargo shards wait && cargo shards act set id=e6 component=Health path=current value=400
cargo shards act attack target=e6 repeat=true && cargo shards step 100
cargo shards act teleport id=e1 x=-19 z=-19 && cargo shards step 157
cargo shards save-replay replays/boss.json   # 257 ticks, 8 events, 258 checks, seed 42

# в копии файла set ставит здоровье 390 вместо 400
cargo sandbox --replay replays/boss-hp.json --verify
# replay finished at tick 257: DIVERGED at tick 1: e6 Boss: Health differs [257 ticks in 1.07s]
#   e6 now: Health { current: 390.0, max: 600.0 }

# в копии файла teleport ставит колонну на x=-18 вместо -19
cargo sandbox --replay replays/boss-pillar.json --verify
# replay finished at tick 257: DIVERGED at tick 101: e1 Pillar: Transform differs [257 ticks in 0.64s]
#   e1 now: Transform { translation: Vec3(-18.0, 0.0, -19.0), rotation: Quat(0.0, -0.0, -0.0, 1.0), scale: Vec3(1.0, 1.0, 1.0) }
```

- Что может быть в списке: `e6 Boss: Health, Transform differ` (разошлись компоненты, в том числе пропавшие или лишние),
  `e15 missing (Skeleton despawned at tick 40)` (сущность есть в записи, но не в игре; в скобках — что о её id знает индекс),
  `e27 extra (Skeleton)` (есть в игре, но не в записи), `replay check content differs` (значение `add_replay_check`). В
  списке не больше 10 сущностей, остальные — одной строкой `… and 7 more`.
- Значение — отладочный вывод reflection без путей модулей, не длиннее 240 символов. Объясняется только первое
  расхождение, следующие проверки расходятся уже из-за него.
- В журнал попадают ошибка `replay diverged at tick 60: e6 Boss: Health differs` и по записи `replay: e6 now: …` на каждое
  значение. В `ai/status` поле `replay.last_report` содержит то же.
- Частоту проверок задают при записи: `--check-every N` у игры или `check_every` у `EngineConfig` (по умолчанию 60). Она
  хранится в файле, и проигрывание проверяет на тех же тиках; `--check-every` вместе с `--replay` — ошибка. С частотой 60
  тик в отчёте — первая проверка после расхождения; чтобы найти точный тик, реплей пишут с `--check-every 1`.
- Подробность: `--check-detail` или `check_detail` у `EngineConfig`. `components` (по умолчанию) — хэш каждого компонента,
  отчёт называет сущности, компоненты и значения; `entities` — хэш сущности целиком, отчёт называет сущности (`e6 Boss
  differs`); `none` — только общий хэш, отчёт называет тик (`no details: the recording keeps hashes only (check detail
  none)`). Общий хэш от подробности не зависит.
- Размер: реплей выше (арена, 14 сущностей, проверка на каждом тике) с `components` весит 2.2 МБ (около 8.6 КБ на тик; с
  частотой 60 — столько же на секунду игры), с `entities` — 183 КБ, с `none` — 19 КБ.

## Снапшоты (E11.4)

Снапшот — состояние симуляции на одном тике, в файле: можно сохранить в любой тик и вернуться к нему в этой игре или в
свежей, собранной так же. Дальше `step` идёт так, как шла бы исходная сессия.

```bash
shards save-snapshot snapshots/boss_phase2.json   # или act snapshot_save path=…; без пути: snapshots/tick<T>.json
shards load-snapshot snapshots/boss_phase2.json   # или act snapshot_load path=…; --force — даже если игра другая
```

«Перейти к фазе босса» — это снапшот, снятый в этой фазе и загруженный по имени: сценарий `snapshot.scenario` доводит стража
до второй фазы, снимает `snapshots/boss_phase2.json`, увозит игру куда угодно и одной командой `snapshot_load` возвращает бой во
вторую фазу (тест `go_to_boss_phase_two_is_a_snapshot_loaded_by_name` делает то же в процессе). Снятые снапшоты в репозиторий
не кладут (`snapshots/` в `.gitignore`), как и `replays/`.

**Что в снапшоте.** Каждый элемент состояния симуляции — одно из четырёх:

- **сохраняется**: все сущности с `AiId` под своим id и их отражаемые компоненты (игры и модулей движка, плюс `Transform` и
  `Name`; типы `bevy_*`, которые добавляет окно, не входят); ссылка на сущность внутри компонента пишется как `e12`
  (`AiId`) или `c7` (ключ модуля: коллайдер без id, `add_snapshot_entity_key`); тик; счётчик `AiId` и надгробия;
  ресурсы, которые модуль зарегистрировал `add_snapshot_resource::<R>()` (`ColliderIds`, `LevelCounter` — номера
  экземпляров уровней, `RegionBuild` — регион в мире, см. ниже);
- **пересобирается** после загрузки (`add_snapshot_rebuild`): кэши, выведенные из сохранённого, — `PhysicsWorld`,
  запомненные значения входов записи реплея, **навсетка региона** (`navmesh`: строится заново для региона и твёрдых тел,
  которые снапшот вернул; NPC игры, загруженной из снапшота, ходят теми же маршрутами, что в исходной);
- **проверяется** (`add_snapshot_check`): то, вокруг чего построен мир. Снапшот хранит хэши базы контента, коллайдеров без
  `AiId` (геометрия уровня) и размещённых уровней и не грузится там, где они другие: `content differs (snapshot …, this
  game …): the content files differ …`. `force=true` грузит всё равно;
- **не состояние**: настройки (инструменты, хуки, реестры), журнал, сессии агентов (они называют сущности по id), всё, что
  показывает только окно. Seed и частота тиков — в заголовке и должны совпасть (`start the game with --seed 7`);
  версия движка в заголовке тоже: другая минорная (`0.2` и `0.3`) — отказ (`force` пропускает), другой патч — только
  предупреждение. `ContentDb` не сохраняется: это файлы.

**Регион в снапшоте.** Мир строится вокруг региона, поэтому он идёт особым путём: `add_snapshot_prepare(name, covers, fn)`
запускает модуль **до** проверок (после seed, версии, контента и компонентов), чтобы он построил заново то, что делает из
сохранённого ресурса; проверки из `covers` (`static_colliders`, `levels`) идут после. Мир сохраняет `RegionBuild`: канонический
манифест региона, первый `ColliderId` и номер экземпляра уровня, с которыми он строился. При загрузке регион снапшота строится
заново, с теми же id, в игре, где другой регион или никакого; если снапшот снят без региона, регион в мире выгружается;
манифест, который контент не может построить, отказывает до любых изменений. Счётчики коллайдеров и экземпляров уровней
снапшот ставит свои сразу после этого: уровень, поставленный после загрузки, получает номер исходной сессии, и тайл региона
не займёт номер уровня, который поставил агент. Проверки из `covers` могут отказать, когда регион уже построен заново (агент
поставил `spawn_level` другие уровни): тогда загрузка возвращает мир, каким он был до неё (тот же регион, сущности под теми
же id, счётчики, ресурсы), строки попытки уходят из журнала, и одна строка говорит, что случилось:
`snapshot of tick 332 not loaded (this game is not the one the snapshot was taken in): the world is as it was before the load`.
Запись реплея идёт дальше, как будто загрузки не было. Настоящий вывод: снапшот, снятый в дебрях в
разгар боя стража логова, загружен в свежую игру на арене.

```text
$ shards save-snapshot snapshots/wilds_fight.json                  # игра 1: регион wilds, на 332-м тике страж бьёт игрока
saved /…/snapshots/wilds_fight.json: tick 332, 30 entities, 3 resources
$ shards load-snapshot snapshots/wilds_fight.json                  # игра 2: свежая, на арене, региона нет
loaded /…/snapshots/wilds_fight.json: tick 332, 30 entities, 3 resources; region: wilds seed 42 v1 built again from its manifest (199 entities); the recording now starts at tick 332
  332 [sim] snapshot loaded: …/snapshots/wilds_fight.json (tick 332, 30 entities, 3 resources; region: wilds seed 42 v1 built again from its manifest (199 entities))
$ shards step 120                                                  # то же, что в игре 1
ran 120 ticks (332 → 452)
  ~ e25 Skeleton "Den Guard": +ground 1.6m/s; fight e5 · token · leash 9/10m → fight e5 · circle · leash 7/10m; -active claw 0.10s (zone cone 1.8m 100°); moved 1.8m S
```

Состояние вне сущностей с `AiId`, которое не подпадает ни под одно из четырёх, — единственная ловушка. Поэтому загрузка
кончается проверкой: мир сохраняется снова и сравнивается с файлом, а `state_hash` (с `add_replay_check`-ами) — с хэшем
снапшота. Расхождение — ошибка с первыми местами: `the restored state differs from the snapshot in 1 place(s): …` с
советом, что зарегистрировать, а не продолжение, которое тихо пойдёт иначе. Компонент, который нельзя записать (не
отражается, не сериализуется, бесконечность в `f32`), называется сразу при сохранении (`snapshot leaves out: …`) и в
файле (`left_out`).

**Загрузка** (`snapshot_load`, control-инструмент, Dev): все сущности с `AiId` заменяются сущностями снапшота под теми же
id, ставятся тик, счётчики и ресурсы, пересобираются кэши. Ошибка (другой seed, контент, неизвестный компонент) возвращается
до любых изменений; отказ после подготовки региона возвращает мир, каким он был (см. выше). Запись реплея после загрузки начинается с этого снапшота, всё записанное раньше отбрасывается
(`save-replay` до загрузки, если оно нужно). Загрузка во время проигрывания реплея заканчивает проигрывание.

**Реплей со снапшота** (`shards-replay/3`, поле `start`): `cargo sandbox --replay FILE --verify` загружает снапшот сразу после
загрузки игры, до первого тика, и дальше проигрывает события и сверяет проверки как обычно. Снапшот, который эта сборка
не принимает (другой контент или версия), — код выхода 2 с причиной. Ограничение: то, что произошло до снапшота и меняет
«то, вокруг чего построен мир» (правка контента, `spawn_level`), в такой реплей не входит; свежая игра должна быть уже
такой. Регион исключение: он идёт в снапшоте. Известная щель: при проигрывании реплея `content_apply`, который добавляет
файл уровня тайла, и `regen` в тот же тик не успевают загрузить файл.

**Доказательство** (`crates/sandbox/tests/sandbox/snapshots.rs`): сессия идёт до тика N, снапшот, дальше до тика M
(состояние A); свежая игра загружает снапшот, получает те же входы и идёт до M (состояние B). A и B — одно состояние до
байта (полный снапшот, `state_hash`), журнал от N до M одинаков. Проверено на середине переката, комбо, телеграфа, планирования (glide),
плавания, на 9 точках боя с Хранителем, при перемотке в той же игре и на реплее со снапшота (`--verify`); для мира — снимок
посреди обхода колонн по навсетке дебрей, загруженный в свежую арену (`a_snapshot_in_the_generated_region_goes_on_the_same_in_a_fresh_arena`),
уровни-тайлы, возвращающиеся под своими номерами, подземелье в игре с другим регионом, жетоны, планы, маршруты и восприятие NPC на десяти тиках боя и сгенерированный
регион с тремя NPC на навсетке и ящиками, вырезаемыми и возвращаемыми (`shards_npc/tests/npc/snapshot.rs`).
Почему не `DynamicWorld` Bevy: тот же слой reflection под ним, но сущности названы битами `Entity`, а не `AiId`; порядок
вставки компонентов зависит от архетипа, а `AiId` должен встать раньше хуков; файл не стабилен по байтам.
## Сохранения (E8.1–5)

Сохранение — прогресс игрока, наложенный на мир, который игра построила как обычно. Это не снапшот: снапшот (`snapshot_save`)
хранит точное состояние симуляции на одном тике и нужен разработчику, а сохранение — явные структуры, которые игра назвала
сама. Ничего, что не названо, в файл не попадает. Крейт `crates/shards_save`, модуль `save` (`ai/status`), в зонтике
`shards::save`.

Два вида: **персонаж** (прогресс каждого игрока, лежит у него) и **мир** (хоста). У каждого есть слоты (`slot1`, `ford-camp`:
`[a-z0-9_-]`, до 32 знаков, не пути).

```rust
app.save_component::<Bag>(SaveKind::Character, "bag", 1)        // компонент на теле игрока
   .save_component::<Opened>(SaveKind::World, "chests", 1)       // карта SaveKey -> компонент по сущностям с SaveKey
   .save_resource::<Calendar>("calendar", 1)
   .add_save_section::<Quests>()                                 // структура с capture / apply / missing
   .add_save_migration("bag", 1, |old| Ok(old))                  // JSON версии 1 -> версия 2
   .autosave_on::<CampRested>(SaveWhat::All);                    // пишется в конце тика
```

- **Секции.** `SaveSection`: `NAME`, `VERSION`, `KIND`, `capture`, `apply`, `missing`. Секции применяются в порядке
  регистрации. Неизвестная файлу секция получает `missing()`, неизвестная игре пропускается с предупреждением. Цепочка
  миграций с дырой (`add_save_migration` не хватает на шаг) роняет приложение при сборке с именем секции. Реестр —
  инструмент `save_registry`. Движок регистрирует свои: `region` (мир: регион, зерно, версия генератора, хэш манифеста; при
  применении регион строится заново и отказывает до любых изменений, если выходит другим), `position` (позиция и поворот
  тела; в другом регионе тело остаётся где стоит, журнал говорит почему) и `vitals` (здоровье, выносливость, заряды).
  Загруженное `vitals` возвращает к жизни тело, умершее после сохранения (`shards_action::revive`, как делает dev-`heal`,
  и событие `Healed`, чтобы игра отменила то, что сделал её `Died`); сохранение с нулевым здоровьем оставляет мёртвым
  (`check_deaths`).
- **Файл** `<save_dir>/<kind>/<slot>.save`: первая строка `shards-save/1 crc32=<hex> len=<байты>`, дальше канонический JSON
  (ключи по алфавиту) с `{kind, slot, label, game, engine, tick, sections: {имя: {version, data}}}`.
- **Запись** атомарна: `.tmp`, `sync_all`, ротация `.bak1`→`.bak2`→`.bak3`, `rename`, fsync папки. Трёх копий хватает на
  три предыдущих сохранения. Битый основной файл в копии не превращается: он уходит в `.damaged`.
- **Чтение.** Основной файл, а при любой порче (контрольная сумма, обрезан, не JSON, миграция не удалась) — первая хорошая
  копия; основной файл переписывается из неё, журнал: `save: world/slot1.save is damaged (checksum mismatch); restored from
  backup 1 (tick 50110)`. Все копии плохи — ошибка называет каждую, мир не тронут. Остаток `.tmp` (сбой при записи)
  убирается.
- **Сохранение из более новой версии — не порча.** Секция выше версии этой сборки или конверт `shards-save/2`
  (`Fault::Newer`) отказывает с ясной ошибкой `world/slot1 was saved by a newer version of the game (section inventory v7
  > v5); nothing was changed`: без перехода к старой копии (потерялся бы прогресс), без `.damaged`, без перезаписи.
  **Запись в такой слот тоже отказывает**, пока любая копия слота новее (`so this build does not overwrite it`): ротация
  не может ни переименовать новый файл, ни выдавить его через три сохранения. Выход: другой слот или новая версия игры.
  Выбрано это, а не «оставить как копию»: копию вытеснили бы три следующих сохранения. `save_list` показывает такой файл
  как `newer`.
- **Когда.** `save_now(world, SaveWhat, reason)` пишет сразу (награда, находка). `autosave_on::<E>(what)` и
  `SaveRequests::request` ставят запрос; он исполняется в `FixedLast` того же тика, после симуляции, с дебаунсом по виду
  (`*.save.ron`, `SaveDef { debounce_secs }`). **При выходе** (хук `add_exit_hook`) ожидающие запросы пишутся сразу, не
  глядя на дебаунс; `flush_saves(world)` — для собственного выхода игры. Сохранение никогда не меняет симуляцию (тест
  сравнивает хэш состояния с автосохранениями и без).
- **Реплей не пишет файлов.** Пока проигрывается реплей (`--replay`, `--verify`), на диск не идёт ничего: ни `save_now`
  из системы, ни инструмент игры, ни `save_game`, ни исправление основного файла при загрузке, ни `save_damage`. Ответ
  `save_now` — `Ok` с `written: false` (код игры видит то же, что в записи), журнал: `save: not written during replay
  playback (world/slot1, reward)`, инструменты отвечают ошибкой `save: not written during replay playback: …`. Реплей
  сверяет состояние, а не тексты ответов, так что расхождения это не даёт.

**Запуск.** `--save-dir DIR` — папка внутри папки игры (не абсолютный путь, без `..`, как `replays/`; иначе код 2),
`--world SLOT` и `--character SLOT` — текущие слоты, куда пишут `save_now` и автосохранения. Одиночная игра ещё и
**загружает** их до первого тика вызовом `load_game` (агент `launch`, в журнале `launch: load_game …`); запись реплея
начинается со снапшота этого состояния, так что `--verify` проходит без флагов. Слота нет — новая игра, строка в журнале
(`save: --world fresh: no world save in that slot yet, starting a new game`), первое сохранение создаёт его. Слот есть, но не
читается (все копии порчены, новее сборки) — `Error` в журнале, игра начинается как есть и ничего не перезаписывает.
Хост и клиент сети грузят слоты по-своему: `shards::save::load_launch_slot(world, kind, slot)` и `load_launch_slots(world)`
— крючок для кооператива.

**Инструменты.** `save_game what=world|character|all slot= label=` (control, не пишется в реплей; персонаж — Player,
мир — Dev), `load_game what= slot=` (Dev, action: применяет секции и перезапускает запись от загруженного состояния,
`replay::restart_from_current`, поэтому реплей после загрузки проходит `--verify`), `save_check what= slot=` (хэш
сохраняемого состояния и, со слотом, сравнение с файлом по секциям), `save_list` (слоты, подписи, тики, копии и их
здоровье), `save_registry`, `save_damage what= slot= how=flip|truncate|garbage copy=` (Dev: портит файл только внутри
`save_dir`, для проверки восстановления). Сценарий `crates/sandbox/scenarios/saves.scenario` проходит весь путь: сохранить,
потерять состояние, загрузить, испортить файл, получить восстановление из копии, испортить всё, умереть, загрузить снова;
эталонный реплей `tests/replays/saves.json` записан с него.

**Доказательство.** `crates/shards_save/tests/save/` — 1000 случайных состояний туда и обратно, матрица порчи, образцы
файлов версии 1 (`tests/samples/`, не переписываются), автосохранение и хэш состояния, `load_game` и `--verify`, запуск с
`--world`/`--character`, реплей, который сохраняет и ничего не пишет (байты и времена изменения слота не тронуты), выход,
новые сохранения; `crates/sandbox/tests/sandbox/saves.rs` — секции движка и оживление мёртвого тела.

## Песочница

Песочница (`crates/sandbox`) — тестовая арена движка, на которой проверяется каждый модуль. Она собрана только на
`shards`, как игра. Своего кода в ней немного: мир арены по контенту, фазы босса, останки, условия для `step`, визуал окна.
Монстры — NPC движка (`shards_npc`): их поведение, восприятие, директор жетонов и планы атак тоже контент. Три области:

- **арена** (с точки (0, 0)): игрок, три скелета и страж Frost Warden — бой-прототип;
- **полоса препятствий** от x = 40 (E2);
- **дебри** (`wilds`): сгенерированный регион 8 × 8 тайлов по 16 м к югу от арены, северо-западный угол в (−64, 48), то есть
  x −64..64, z 48..176 (южная стена арены на z 21.5). Игрок попадает туда по `regen region=wilds seed=42` (`shards act`), у ворот
  в клетке (1, 1). Девять тайлов: трава, четыре дороги, колонны, руина из файла Blender (`assets/tiles/ruin.gltf`), ворота и
  зал с колоннадой и логовом (ворота и зал расставлены регионом, не жребием). Точки интереса: ворота, логово со стражем,
  святилище, два лагеря скелетов. Тайлы запечены в `assets/nav/` (коммитятся), `ruin.level.ron` заставляет `content check`
  читать файл руины узел за узлом.

Всё, что числа и определения, — контент в `crates/sandbox/assets/content/` с горячей перезагрузкой:

| файл | что в нём |
|---|---|
| `world/arena.layout.ron` | арена 40×40 м: пол, стены, колонны, ворота на восток; кого и где спавнить |
| `world/course.layout.ron` | полоса препятствий от x = 40: ступени, уступ 1.3 м, бассейн 10 м, пандус на обрыв 18 м над каньоном, площадка цели через 32 м пропасти, восходящий поток посередине |
| `characters/characters.prefab.ron` | префабы `player`, `skeleton`, `skeleton_captain`, `frost_warden`, `pillar` |
| `characters/bodies.locomotion.ron` | как ходят игрок, скелет и страж |
| `combat/player.action.ron` | комбо `light_1`–`light_3`, `heavy`, перекат (i-frames 0.18 с), блок с идеальным блоком, фляга |
| `combat/warden.action.ron` | четыре атаки стража (`slash`, `slam`, `sweep`, `shockwave`), спокойные и в ярости |
| `combat/skeleton.action.ron`, `combat/fighters.moveset.ron`, `combat/tuning.combat_tuning.ron` | коготь скелета, мувсеты, буфер ввода |
| `npc/monsters.behavior.ron`, `monsters.perception.ron`, `monsters.director.ron`, `monsters.attack_plan.ron` | как монстры думают: автоматы `skeleton` и `warden` (с состоянием `calm`, из которого сценарии не выпускают монстра), зрение, директоры `default` (два жетона на одного игрока) и `boss` (страж бьёт один), планы атак стража |
| `region/wilds.*.ron` | дебри: тайлы, правила сокетов, точки интереса, регион, запасная карта, настройки навсетки |
| `region/camps.prefab.ron`, `region/ruin.level.ron` | лагерные скелеты (`Home` 12 м) и страж логова (10 м); файл руины и маркеры, обязательные в нём |
| `world/watch.layout.ron` | третий скелет арены (`e14`) |
| `world/world.bounds.ron` | границы мира для правила `bodies_in_world`: запас вокруг построенных layout и региона, высота, ниже которой тело выпало |

`layout` — вид контента самой песочницы: коробки (`solids`), пандусы, объёмы (вода, поток, триггер), ориентиры для ИИ и
префабы для спавна. Слои спавнятся по порядку id, внутри — по порядку списков, так что `AiId` и `ColliderId` одинаковы в
каждом запуске: e1–e4 колонны, e5 игрок, e6 страж, e7–e8 скелеты, e9–e13 ориентиры полосы `Stairs`, `Ledge`, `Pool`,
`Cliff`, `Goal`, e14 третий скелет; регион (`regen`) добавляет свои сущности после них. Правка layout на лету перестраивает геометрию на следующем тике; ориентиры сохраняют id, персонажи остаются.

**Бой — прототип из GDD 39.** У игрока комбо из трёх лёгких ударов через окна отмены, тяжёлый удар с гипербронёй на
замахе, перекат, блок (держать; первые 0.15 с — идеальный блок, он и есть парирование: урона нет, атакующий шатается),
фляга (45 HP, 3 заряда). Выносливость тратят удары, перекат, блок и спринт; стойкость есть у всех. Страж — NPC: он охраняет
свой пост и дерётся с игроками в пределах поводка 26 м (скелеты арены — 30 м), без лечения при возврате. План атак
`warden` выбирает из атак мувсета те, что достают до цели (`SimRng`), и никогда не делает две «зоны» подряд (тег `zone` в
контенте); пауза после каждой — 1.2–1.8 с, в ярости 1.0–1.4 с, и она отсчитывается, пока страж ничего не делает (оглушение
после парирования отдыхом не считается). Предупреждение (`e6 winds up slam (0.50s) ⚠ circle 4m`) приходит ровно за 0.50 с до удара. С половины здоровья —
фаза 2: второй мувсет, короче замахи и отдых. Скелеты: один коготь, вздрагивают от ударов, их тела исчезают через 1.5 с;
тело стража остаётся (dev `heal` его поднимает).

**Правила (инварианты).** После каждого тика песочница проверяет `health_in_range` (здоровье в `0..=max`) и
`bodies_in_world` (каждый персонаж над ареной с коридором, полосой или загруженным регионом, см. «Инварианты»). Ни один
сценарий, тест и эталонный реплей их не нарушает.

**Полоса препятствий (E2).** Прыжок (1.2 м) не перепрыгивает ничего: уступ 1.3 м берётся перелезанием, бассейн 10 м —
вплавь с выходом через `jump`, до площадки цели долетает только планер и только с подъёмом в потоке. Площадка —
триггер: `e5 reached goal`.

**Сценарии-эталоны** (`crates/sandbox/scenarios/`), агент со scope `player`; время двигает агент сценария:

- `obstacle_course.scenario` — `path_to` и `clearance`, ступени, уступ, бассейн, пандус, планер через поток, цель;
  инварианты: жив, ни разу не ниже старта;
- `combat_prototype.scenario` — страж побеждён за 11 раундов. Каждый раунд читается по предупреждению (`step --until
  "journal:e6 winds up"`), дальше перекат из зоны, блок, идеальный блок с тяжёлым ударом, обмен ударами под гипербронёй,
  фляга, комбо. Тест прогоняет его после шести разных пауз (другие атаки в другом порядке) — победа каждый раз;
- `boss_fight`, `dodge`, `errors` — переход в фазу 2, перекат против попадания, ошибки, которые учат;
- `npc_director.scenario` (E6) — три скелета на одного игрока в режиме бога, 30 с боя: условие песочницы `three_attacking`
  (трое бьют одновременно) не выполняется ни на одном тике, но каждый скелет получает свою очередь, жетоны ходят по кругу;
- `npc_navmesh.scenario` (E6) — в дебрях страж логова видит игрока сквозь колоннаду, обходит колонны по навсетке
  (`npc_detour`; `nav_path` игрока показывает те же 16.8 м вместо прямых 10), бьёт и никогда не выходит за поводок
  (`npc_beyond_leash`); когда игрок уходит на север, доходит до края зоны и возвращается к логову;
- `region.scenario` (E5) — `regen region=wilds seed=42` и хэш манифеста `d9d721509840d909`, игрок у ворот, путь на два тайла
  вперёд и обратно по навсетке (`move_to path=true`, остановка на `near_poi`), другое зерно — другой манифест, то же зерно —
  тот же, до байта;
- `snapshot.scenario` (E11.4) — бой со стражем доводится до второй фазы, снимается снапшот, игра уезжает, `snapshot_load`
  одной командой возвращает бой и его можно продолжить.

Сценарии начинают с `heal`/`teleport`/`set`/`npc_set_state … calm`, поэтому идут на свежей игре и после других, в любом порядке:
CI гоняет их вперёд и потом назад в одной игре. Сессии шести из них записаны как эталонные реплеи в `tests/replays/`
(`obstacle_course`, `combat_prototype`, `npc_director`, `npc_navmesh`, `region`, `snapshot`; см. «Реплеи»).

## Тесты и CI

```bash
cargo test --workspace          # юнит-тесты, сценарии и эталонные реплеи; первая сборка Bevy долгая
cargo test -p sandbox --test sandbox          # песочница: сценарии, реплеи, контент, полоса, бой
cargo test --manifest-path template/Cargo.toml # шаблон отдельно: свой workspace и свой Cargo.lock
```

Всего 1001 тест в `cargo test --workspace` (на 14 октября 2026; ещё 21 `#[ignore]`: запись эталонных реплеев, проверка
окна под Xvfb и примеры в документации) и 23 теста шаблона (ещё один `#[ignore]`: запись его эталонов):

| крейт | тестов | что проверяют |
|---|---|---|
| `ai_core` | 77 | снапшоты (сохранение и загрузка, отказ для чужой игры, ссылки на сущности, реплей со снапшота без загрузки), id (перенумерация, занятый id), журнал, RNG, компас и LOS, схемы, шаги симуляции, пути снимков, реплеев и снапшотов (только `shots/`, `replays/`, `snapshots/`; абсолютные пути, `..`, симлинки), формат реплея и отчёт, запрет BRP-правки `AiId`, занятый порт, `instance` в `ai/status`, файл порта, токен у каждого метода BRP, запуск сценариев в процессе, инварианты (правило игры и сессии, срабатывание один раз, маршрут к хосту), время тика, части проверок реплея и отчёт о расхождении |
| `shards_script` | 35 | без Bevy: условия и аргументы, слова как в shell, разбор каждой команды сценария и текст каждой ошибки, запуск на поддельной игре (ожидания, `mark`, инварианты, `session`, `seed`, потерянная игра, шаг, оборванный нарушенным инвариантом), условия `warning` и `error` |
| `shards_cli` | 12 | порт из `--port-file`, строка `ready`, токен по номеру порта; `shards scenario` против поддельной игры на localhost (коды выхода 0/1/2); код выхода по исходу `step` (3 — игра остановилась), входы `fuzz` из зерна |
| `shards_core` | 14 | загрузка не сдвигает тики, пауза, `after_loading`, нажатия живут один тик, `move_to`, `ai/status` называет модули |
| `shards_content` | 22 | определения и их файлы, повторы и ссылки называют файл и поле, `check_dir`, префабы (`extends`, переопределения), перезагрузка пишется в реплей и проигрывается, сломанная правка отклоняется, правка файла в работающей игре |
| `shards_motion` | 154 | физика (запросы, слои, порядок по `ColliderId`), контроллер (ступени, перелезание, плавание, планер, прыжок, стены), числа из контента, ввод (режимы, контексты, переназначение, сохранение), камера, `clearance` и `path_to`, инструменты, реплей и независимость от индексов `Entity` |
| `shards_action` | 137 | действия, буфер, отмены и комбо, `can_start`, попадания и конвейер урона (уклонение, блок, парирование, стойкость, гиперброня), статусы, зоны и подсказка «выйди», анимация (`select_clip`), геометрия телеграфов, инструменты и их ошибки, реплей боя |
| `shards_level` | 79 | glTF и узлы, свойства (`collider`, `volume`, `prefab`, компоненты по имени, текстовый JSON), маркеры и их ошибки, коллайдеры нужных размеров и слоёв, вид `level` и его проверка в `content check`, спавн (мировые позиции, порядок, id), горячая перезагрузка, реплей и хэш `level`, картинка уровня |
| `shards_world` | 72 | формат тайлов, регионов, комнат (ошибки называют файл и ближайший id); решатель, проверки (недоступность, уклон, расстояния, дорога никуда), сотни и тысяча зёрен, манифест байт в байт, запасная карта, паника шага игры с номером зерна; подземелья; смена региона через `RegionLoading`, стартовый регион, `goto_tile`, реплей со сменами регионов, снапшот в регионе; тайлы из файлов уровней во всех четвертях оборота; активные зоны |
| `shards_npc` | 82 | автомат поведения и его ошибки, зрение/слух/память, жетоны директора (включая смену числа игроков и приказы агента), планировщик (запреты, бюджет, отдых), запекание и сшивка (в том числе ступени на краю тайла), вырезание в любом порядке, маршруты и обход колонн, поводок, `npc_state`, реплей и снапшот на десяти тиках боя и в сгенерированном регионе |
| `shards_save` | 87 | секции и реестр, конверт `shards-save/1` и контрольная сумма, атомарная запись, три копии и восстановление, матрица порчи, миграции, сохранения из более новой версии, слоты, автосохранение, инструменты `save_*` и `load_game` |
| `shards_net` | 80 | без Bevy: транспорты (хаб в памяти, UDP), имитация связи, канал (надёжные и последние сообщения, блобы, подтверждения, RTT, потери, тайм-ауты), формат датаграмм |
| `shards` | 17 | реплей со снапшота (`--verify`: тот же результат, код 2 для чужого снапшота), режим без окна — те же плагины, тестовое приложение, `testing::ok` на сбое геймплея, анимационный мост на настоящей модели со скином |
| `sandbox` | 133 | в одном бинарнике `tests/sandbox`: снапшоты (`snapshots.rs`: состояние после загрузки то же, что в исходной сессии, на перекате, комбо, телеграфе, планировании, плавании, флаконе, боях NPC и в сгенерированном регионе), тайл руины (`tiles.rs`: файл из тестового генератора glTF совпадает с закоммиченным), сценарии (`scenario_files`: каждый файл на свежей игре, все подряд в одной игре, бой-прототип после шести разных пауз), эталонные реплеи, действия, контент, полоса препятствий, детерминизм, видимость, права scope, реплеи, `kill` и SIGHUP; инварианты (`invariants.rs`: правила песочницы, сессии, остановка в реальном времени, реплей нарушения, снапшот); отдельно `system_failures` (сбой геймплея, краш-реплей), `log_capture` (строки лога в `step`) и `perf` (время тика: бюджет, журнал, `ai/status`): у них свои процессы |

Тесты шаблона (`template/tests/game`, 23) собираются в одном бинарнике: сценарии, детерминизм, реплеи, контент, регион
(тысяча зёрен долины, манифест байт в байт, экран загрузки без окна, колонна руины, рейдер на навсетке), манифест (единственная
зависимость — `shards`).

**Окно против режима без окна.** Снимки и окно требуют дисплея и GPU, поэтому в `cargo test` их нет. Одинаковость окна и
режима без окна проверяют реплеи под Xvfb и программным Vulkan (`apt install xvfb mesa-vulkan-drivers
libxkbcommon-x11-0`):

```bash
for r in tests/replays/*.json; do xvfb-run -a cargo sandbox --replay $r --verify --windowed; done
# replay finished at tick 2307: identical to the recording (39 checks) [2307 ticks in 6.69s, 44 frames]
# replay finished at tick 1010: identical to the recording (17 checks) [1010 ticks in 3.00s, 22 frames]
# …и так для каждого файла: npc_director (1923 тика), npc_navmesh (1261), region (353), snapshot (369)
xvfb-run -a cargo test -p shards --test action_render -- --ignored --nocapture   # телеграфы, оверлей, маркеры: две картинки
```

**CI** (`.github/workflows/ci.yml`, на каждый push и pull request; общие шаги — `.github/actions/setup`: системные пакеты,
тулчейн из `rust-toolchain.toml`, кеш cargo). Все команды заданий можно запустить руками; на GitHub Actions файл ещё не
запускался, а команды прогнаны локально.

| задание | что делает |
|---|---|
| `test` | `cargo test --workspace --locked`: юнит-тесты, сценарии песочницы (в том числе мир и NPC), эталонные реплеи (`--verify`) |
| `clippy` | `cargo clippy --all-targets --locked -- -D warnings` для движка и для шаблона |
| `template` | `template/Cargo.lock` не расходится с корневым; `cargo test` шаблона; `cargo run -- --check-content`, `--verify` его реплеев и `--worldgen-report valley --seeds 1..1000` через бинарник игры |
| `headless-cli` | `cargo sandbox --check-content` (уровни тайлов и запечённая навигация тоже), `--worldgen-report wilds --seeds 1..1000`; живая песочница без окна (`--port 0 --port-file`), `shards wait`, `shards scenario crates/sandbox/scenarios/*.scenario`, потом те же сценарии в обратном порядке в той же игре; `kill` в конце |
| `replays-window` | те же реплеи (песочницы и шаблона) в окне под Xvfb: `--verify --windowed`; необязательное (`continue-on-error`), пока не докажет стабильность |

## Что дальше

Подробно, по пунктам ТЗ, — [docs/engine-progress.md](docs/engine-progress.md). Фазы A (прототип боя: E1–E3 и части E4 и
E11) и B (мир: E5, E6, уровни из Blender, снапшоты) собраны и проверены локально; GitHub Actions для них не запускался.
Впереди:

1. **Остатки фазы B:** стриминг геометрии тайлов для физики (сейчас по расстоянию грузится только картинка), проверка, что
   сокеты файла уровня совпадают с сокетами тайла в RON, `path_to` и `clearance` по навсетке (сейчас по навсетке только
   `nav_path` и `move_to path=true`), навсетка под несколько тел.
2. **Фаза C — кооператив:** сеть (E7), сохранения и настройки (E8), агент, привязанный к игроку, стенд «хост + клиенты».
3. **Фаза D — контент и интерфейс:** виджеты, экраны, звук, локализация (E9); дерево интерфейса в `observe`.
4. **Фаза E — платформа:** рендер и производительность, Steam, Windows, скриншоты без окна (E10).
5. Горячая перезагрузка кода (`--features dynamic`, subsecond).

## Соглашения и сборка

- В движке нет LLM: ни встроенных агентов, ни вызовов моделей, ни API-ключей (см. [AGENTS.md](AGENTS.md)).
- Код и комментарии на английском, документация на русском.
- Версия Bevy закреплена (`0.20.0`). Обновлять её стоит осознанно, отдельным шагом: API Bevy меняется в каждом релизе.
  Сторонние крейты (сейчас `parry3d =0.31.1`, `ron =0.12.2`, `gltf =1.4.1` в `shards_level`) берутся после проверки поддержки Bevy 0.20, закрепляются точной
  версией и прячутся за API движка.
- `.cargo/config.toml` ставит `jobs = 4`, потому что на этой машине мало RAM. Если памяти больше, увеличьте. Зависимости
  собираются с `opt-level = 3`, свой код с `opt-level = 1`.
- Версии и теги: SemVer, все крейты движка делят одну версию (`[workspace.package]`), тег ставит мейнтейнер; правила — в
  [CHANGELOG.md](CHANGELOG.md).
- Features `sandbox` (и `shards`): `render`, `remote` и `hot_reload` (по умолчанию), `audio`, `gamepad`, `wayland`,
  `dynamic`. Без `render` игра собирается только для `--headless`.
