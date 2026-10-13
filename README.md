# Shards: игровой движок с ИИ-интеграцией (Rust + Bevy 0.20)

Движок, чтобы делать игру вместе с внешними агентами-кодерами: Claude Code, Codex и им подобными.
ИИ не разглядывает пиксели. Движок и так знает всё, что есть в кадре, поэтому
он **описывает мир структурированным текстом** и даёт **типизированный API действий**.

**Внутри движка LLM нет**: ни встроенного агента, ни локальной модели, ни API-ключей, ни вызовов
к моделям. Игра — среда. Агент запускает её, смотрит и действует из терминала через CLI `shards`
(JSON-RPC методы `ai/*` поверх Bevy Remote Protocol на 127.0.0.1), без MCP-прослоек. Тем же CLI
пользуются человек в терминале, скрипты и CI. Инструкции для агентов — в [AGENTS.md](AGENTS.md).

Скриншоты здесь вторичный канал, для визуальных багов. Метки `eN` на экране
совпадают с id в тексте, а `shards shot` к каждому снимку пишет легенду: где на картинке какой id.

Одинаковый seed и одинаковые входы дают одинаковую игру, поэтому сессия записывается целиком
как входы. Любой сеанс можно сохранить реплеем, проиграть заново и проверить им новую сборку.

![sandbox](docs/sandbox.png)

## Быстрый старт

```bash
# 1. Системные пакеты (один раз; нужен sudo):
sudo apt install pkg-config libasound2-dev libudev-dev
# (+ libwayland-dev libxkbcommon-dev, если нужен нативный Wayland: --features wayland)

# 2. Окно с игрой:
cargo sandbox
# или без окна, на паузе (CI, параллельные прогоны, работа агента):
cargo sandbox --headless --paused

# 3. Из другого терминала:
cargo shards observe
cargo shards act attack target=e6 repeat=true
cargo shards step --until journal:winds
cargo shards shot                  # PNG окна с метками eN + легенда (нужно окно)
cargo shards save-replay           # сессия с тика 0 → replays/tick<T>.json

# 4. Проиграть реплей (seed и частота тиков берутся из файла):
cargo sandbox --replay replays/tick332.json
cargo sandbox --replay replays/tick332.json --verify   # без окна; код 0 — игра совпала с записью
```

**Rust** ставится через rustup, он добавляет `~/.cargo/bin` в PATH. В уже открытых
терминалах выполните `source ~/.cargo/env` или откройте новый.

**Пока нет sudo**, сборка работает через локальный обходной путь. В
`~/.local/share/rustengine-sysdeps/` распакованы `pkgconf`, `libasound2-dev` и `libudev-dev`,
а также лежит shim `pkg-config`. Перед любой командой `cargo build/test/run` выполните:

```bash
source ~/.local/share/rustengine-sysdeps/env.sh
```

Этот путь нужен потому, что `bevy_remote` тянет цепочку `bevy_dev_tools` → `bevy_audio` → `alsa-sys`,
а `alsa-sys` ищет `alsa.pc` через pkg-config. После `apt install` он не нужен.

Управление в окне: WASD или стрелки — ходить, Space — уворот, J или ЛКМ — удар,
P — пауза, N или `.` — один тик на паузе, F1 — оверлей с id, R — вылечить/воскресить,
F12 — снимок с метками в `shots/`.
Клавиатура человека и инструменты агента пишут в один и тот же `Intent`.

## Архитектура

```text
источники намерений:  игрок · сетевой клиент · агент (через shards) · реплей
                              │  Intent
геймплей:             FixedUpdate, фиксированный тик, seed   (SimSet, SimRng)
                              │
ai_core:              Observe — срез мира текстом            (observe, inspect, nearby)
                      Journal — что случилось с прошлого раза (журнал событий, warn!/error!)
                      Tools   — типизированные действия      (add_ai_tool, Scope)
                      SimCtl  — пауза · шаг · run-until      (sim)
                      Replay  — запись входов, проигрывание  (replay)
                              │
транспорт:            BRP `ai/*` на 127.0.0.1 → CLI `shards` ← Claude Code, Codex, скрипты, CI
```

| crate | что это |
|---|---|
| `crates/ai_core` | слой ИИ, не зависит от конкретной игры |
| `crates/sandbox` | тестовая арена: игрок, 2 скелета, босс Frost Warden (2 фазы, телеграфируемые атаки) |
| `crates/shards_cli` | бинарник `shards`, тонкий клиент к `ai/*` |

Модули `ai_core`:

| модуль | ответственность |
|---|---|
| `id` | `AiId` (`e42`): стабильный монотонный id, никогда не переиспользуется; надгробия для удалённых. Id назначает только движок: `set` и BRP `world.mutate_components` его не меняют, а новая вставка перенумеровывает сущность (надгробие `renumbered to e99`) |
| `describe` | только явно зарегистрированные компоненты видны ИИ, у каждого короткий describe |
| `observe` | текстовый срез: YOU / SEEN (по угрозе) / NOT IN VIEW / LANDMARKS / SINCE; `inspect`, `nearby`, диффы |
| `spatial` | позиции относительно игрока («8.4m NE»), line of sight по `AiOccluder` (подменяемо) |
| `journal` | кольцевой буфер игровых событий (observers) плюс перехват `warn!`/`error!` из tracing |
| `tools` | реестр: args-struct + Bevy-система, JSON Schema (schemars), scope, `catch_unwind` |
| `builtin` | встроенные инструменты: observe, inspect, step, set/get через reflection, ... |
| `sim` | тик, пауза, синхронный `step` по FixedMain, стоп-условия |
| `replay` | запись входов и action-вызовов с тика 0, проигрывание, хэши состояния, отчёт о расхождении |
| `rng` | детерминированный RNG: счётчик по (seed, tick, AiId, salt) |
| `session` | агенты: имя, scope (Player/Dev), «тело», курсор журнала |
| `remote` | методы `ai/*` для BRP (feature `remote`) |
| `highlight` | гизмо-подсветка и оверлей id поверх сцены (feature `render`) |
| `shot` | снимок окна с метками `eN` и легендой в пикселях картинки (feature `render`) |

## Что видит ИИ

```text
tick 133 (2.22s) | paused | speed 1x | seed 42 | you e5
YOU  e5 Player  hp 100/100  stam 60/80  at (0.0, 4.7) facing N
SEEN (by threat)
  e6 Boss "Frost Warden"  hp 4000/4000  phase 1  windup IceSlam 0.32s  5.8m N
  e7 Skeleton  hp 40/40  4.6m W
  e8 Skeleton  hp 40/40  6.3m NE
LANDMARKS  e3 Pillar 7.4m W · e4 Pillar 7.4m E · e1 Pillar 13.6m NW · e2 Pillar 13.6m NE
SINCE seq 0
  93 e6 winds up IceSlam (1.00s)
```

- Видимость считается так: попадание в кадр камеры (`AiCamera`, `world_to_viewport`; в окне
  есть `scr(x,y)`) плюс луч от глаз персонажа. Если сущность за колонной, к ней приписывается `occluded`.
- SEEN отсортирован по угрозе: `AiImportance` по убыванию, потом расстояние.
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
   .replay_input::<Intent>();                       // вход для реплеев (см. «Реплеи»)
```

Маркеры сущностей: `AiKind("Boss")`, `AiImportance(10.0)`, `AiLandmark`, `AiOccluder`, `AiPlayer`.

## Инструменты

Инструмент — это структура аргументов (serde + schemars) и обычная Bevy-система
`In<Call<Args>> -> ToolResult`. Схема и сигнатура генерируются автоматически.

| scope | инструменты |
|---|---|
| **Player** (играет честно, только через `Intent`) | observe, inspect, nearby, line_of_sight, journal, tools, highlight, overlay, move_to, move, attack, dodge, stop |
| **Dev** (всё, включая время) | + pause, resume, speed, step, possess, teleport, despawn, get, set (reflection), spawn, heal, shot, replay_save |

У каждого инструмента есть вид (`kind` в `shards tools`):

- `query` только читает мир: без записи в журнал и без диффа;
- `action` меняет мир: пишет в журнал, возвращает `CHANGES` и попадает в реплей;
- `control` управляет временем или сессией (pause, resume, speed, step, possess): журнал и дифф
  как у action, но в реплей не попадает. Он меняет, *когда* что-то происходит, а не *что*.

- **Действие возвращает изменения.** Каждый action-инструмент сравнивает срез мира до и после
  вызова и возвращает `CHANGES` и `JOURNAL`. Агенту не нужен второй `observe`, чтобы узнать, что произошло.
- **Ошибки подсказывают, как исправить вызов:**
  ``bad arguments for teleport: missing field `x`. Expected: teleport(id: integer|string, x: number, y?: number, z: number)``,
  `e57 no longer exists (Skeleton despawned at tick 812); live: …`,
  `e7 no longer exists (Skeleton renumbered to e99 at tick 40)`,
  ``unknown component `Mana`; this entity has: Health, …``,
  `AiId is managed by the engine; despawn and spawn instead`.
- **Паника в инструменте не роняет игру.** Агент получает ошибку, запись уходит в журнал,
  инструмент перерегистрируется при следующем вызове.

## Время и детерминизм

- Весь геймплей идёт в `FixedUpdate`, по умолчанию 60 Гц, в жёстком порядке
  `SimSet::Input → Think → Act → Move → Resolve → Cleanup`.
- `SimRng` — счётчиковый RNG по ключу (seed, tick, AiId, salt). Результат не зависит от
  порядка систем и от того, сколько раз кто-то «крутил» генератор.
- `step` синхронно прогоняет `FixedMain` ровно N тиков, игра при этом стоит на паузе.
  Есть стоп-условия: `journal:TEXT`, `gone:ID`, `shows:ID:TEXT`, `near:A:B:M`,
  `field:ID:Comp.path<N`, `named:NAME`. Лимиты: до 216 000 тиков и 20 с реального времени за вызов.
- **Проверка порядка систем.** `AiCorePlugin` включает ambiguity detection для `FixedUpdate`:
  две системы, которые трогают одни данные без явного порядка, дают warning, и он попадает в журнал.
  Тест `gameplay_systems_have_a_total_order` превращает это в ошибку, а контрольный тест
  проверяет, что сама проверка ловит такую систему.
- Тесты подтверждают: один seed и одни вызовы дают побитово одинаковый мир и журнал;
  `step 300` даёт то же, что 5 × `step 60` с `observe` и кадрами рендера между ними.
- Реплей, записанный в окне, совпадает при проигрывании без окна (проверено вручную под Xvfb):
  рендер-сущности не сдвигают ни `AiId`, ни порядок геймплея.

## Реплеи

Запись идёт всегда, с тика 0, и почти ничего не стоит. В неё попадает:

- **входы**: компоненты, зарегистрированные через `replay_input` (в песочнице это `Intent`).
  Сохраняется каждое изменение, сделанное *между* тиками (клавиатура, сетевой клиент, инструмент
  агента), с тиком, к которому оно относится. Изменения, которые делают системы геймплея внутри
  тика, детерминированы и не хранятся. Компонент клонируется, только когда он изменился;
- **action-вызовы** (`teleport`, `set`, `spawn`, `attack`, …) вместе со scope и «телом» агента.
  Вызовы, сделанные изнутри другого инструмента, не пишутся: их повторит внешний вызов.
  `control`-инструменты (pause, step, possess) не пишутся. Сырые правки через BRP
  (`world.insert_components`) идут мимо инструментов и тоже не пишутся;
- **проверки**: хэш AI-видимого состояния (id, фрагменты, точные позиции) каждые 60 тиков
  и в конце записи.

```bash
shards save-replay                          # или ai/act {"tool": "replay_save", "args": {"path": …}}
cargo sandbox --replay replays/tick332.json # окно, реальное время; seed и частота из файла
cargo sandbox --replay replays/tick332.json --headless --paused   # дальше шагами через shards step
cargo sandbox --replay replays/tick332.json --verify
# replay finished at tick 332: identical to the recording (6 checks) [332 ticks in 0.13s]
```

- Проигрывание требует свежего мира: тик 0, тот же seed и та же частота тиков. События
  применяются между тиками, ровно там, где они случились. Входы, которых нет в записи
  (человек жмёт клавиши во время просмотра), откатываются.
- Каждая проверка сравнивается с записью. Отчёт называет первый тик, на котором игра перестала
  совпадать: `DIVERGED at tick 60`. После изменения кода это регрессия. `--verify` годится для CI:
  код выхода 0 — совпало, 1 — разошлось, 2 — реплей не читается или не подходит к игре.
- В конце записи игра становится живой: реплей работает и как точка сохранения для ветвления.
  Любой action-вызов во время проигрывания перехватывает управление и завершает его
  (`replay interrupted at tick 130 of 332: dev took over (teleport)`).
- Во время проигрывания запись продолжается, поэтому ветка сохраняется одним реплеем.
- Состояние записи и проигрывания видно в `ai/status` (поле `replay`), итог пишется в журнал.
- Формат: JSON `shards-replay/1` (`seed`, `tick_hz`, `end`, `events`, `checks`; хэши hex-строками).

## Скриншоты

`shot` (scope Dev) снимает окно игры средствами самого Bevy, это не захват рабочего стола.
Поэтому окно без фокуса, которое композитор перерисовывает с задержкой, всё равно даёт текущий кадр.

```text
$ shards shot
saved /…/shots/tick93-1.png (1280x720, tick 93, marks on)
LEGEND (image pixels)
  e5 Player  px(640,484)
  e6 Boss "Frost Warden"  px(640,370)
  e7 Skeleton  px(361,523)
  …
```

- Оверлей с id включается на время снимка (кадр до съёмки, чтобы гизмо-линии успели дойти до GPU).
  Сам снимок ждёт, пока GPU докомпилирует пайплайны: сразу после старта меши и метки иначе
  пропускаются.
- Легенда считается в том кадре, который снимается. После `step` следящая камера ещё догоняет
  игрока, и координаты на момент запроса были бы неверны. Легенда пишется рядом с PNG как
  `<имя>.json` (`path`, `width`, `height`, `tick`, `marks`, `legend`).
- Оба файла пишутся во временные и переименовываются, так что появившийся PNG уже целиком на диске.
  Старые файлы с тем же именем удаляются при запросе. В журнале появляется `screenshot saved: …`.
- Инструмент сразу возвращает пути, `shards shot` ждёт файл (`--timeout`, по умолчанию 10 с).
  Флаги: `--no-marks`, `-o PATH`. Без окна (`--headless`) инструмент отвечает ошибкой
  с подсказкой использовать `observe`.

## Транспорты

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
| `ai/status` | — (в ответе `instance: {pid, seed, port, started_at}`: какая игра ответила) |

Стандартные методы BRP (`world.query`, `world.get_components`, …) тоже доступны: `shards raw …`.
BRP даёт полный доступ к миру, поэтому сервер слушает только 127.0.0.1 и включается feature `remote`.
Релизная сборка без BRP:

```bash
cargo build --release -p sandbox --no-default-features --features render
```

**CLI `shards`:** observe, step, act, shot, save-replay, tools, inspect, nearby, journal, pause,
resume, speed, status, session, raw, wait. Аргументы `act` передаются JSON-объектом или парами `key=value`.
Флаги `--port`/`SHARDS_PORT`, `--port-file`/`SHARDS_PORT_FILE` (порт из файла игры; `--port` важнее),
`--agent`/`SHARDS_AGENT`, `--json` (у `tools` — со схемами аргументов). `shards wait` ждёт, пока игра
ответит, и печатает, кто ответил: `ready: tick 0 · pid 4242 · seed 1 · port 15711` (ту же строку игра
печатает при старте, её же первой строкой выводит `shards status`).

**Параллельные headless-инстансы.** У каждого свой порт:

```bash
cargo sandbox --headless --paused --seed 1 --port 15711 &
cargo sandbox --headless --paused --seed 2 --port 15712 &
SHARDS_PORT=15711 cargo shards step 600 --until named:player_dead
```

- Занятый порт — ошибка: игра не запускается, пишет `port 15711 is busy (another game?). Pick another
  with --port` и выходит с кодом 3. Без этой проверки вторая игра работала бы без сервера, а `shards`
  молча управлял бы первой. Проверка доступна любой игре: `ai_core::remote::ensure_port_free(port)`.
- Чтобы не выбирать порты вручную, `--port 0` берёт свободный порт, а `--port-file F` записывает его
  номер в файл (при выходе по Ctrl+C или закрытию окна файл удаляется). `shards` читает его через
  `--port-file F` или `SHARDS_PORT_FILE=F`:

```bash
cargo sandbox --headless --paused --seed 1 --port 0 --port-file /tmp/shards-a.port &
cargo sandbox --headless --paused --seed 2 --port 0 --port-file /tmp/shards-b.port &
SHARDS_PORT_FILE=/tmp/shards-a.port cargo shards wait   # ready: tick 0 · pid 4242 · seed 1 · port 40817
SHARDS_PORT_FILE=/tmp/shards-a.port cargo shards step 600 --until named:player_dead
```

**Внутри игры** та же точка входа, что за BRP, — `ai_core::call_tool(world, agent, tool, args)`.
Её вызывают тесты, реплеи и клавиши разработчика в окне (P, N, R, F12).

## Пример сеанса (реальный вывод)

```text
$ shards act attack target=e6 repeat=true
ok: attacking e6 until it dies
CHANGES
  ~ e5 Player: +order attack e6 (repeat)
$ shards step --until journal:winds
ran 93 ticks (0 → 93), stopped: journal: 93 e6 winds up IceSlam (1.00s)
CHANGES
  ~ e5 Player: moved 9.3m N
  ~ e6 Boss "Frost Warden": +windup IceSlam 0.98s; moved 4.9m S
  …
$ shards act dodge away_from=e6
ok: dodging S
$ shards step 40
ran 40 ticks (93 → 133)
CHANGES
  ~ e5 Player: stam 80/80 → stam 60/80; moved 2.0m S
  ~ e6 Boss "Frost Warden": windup IceSlam 0.98s → windup IceSlam 0.32s
```

## Агенты

Агент — внешний агент-кодер (Claude Code, Codex, …) с терминалом. Ему не нужны ни SDK, ни
определения инструментов: он вызывает `shards` через bash и читает вывод как текст.

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
- По умолчанию у агента scope `Dev`. Чтобы проверить, как игра играется честно, переключите его
  в `Player` (`shards session --scope player`): тогда он ходит только через `Intent`, как человек.
- У каждого агента своё имя (`--agent`/`SHARDS_AGENT`), а с ним свой курсор журнала, scope и «тело».
  Агенты могут работать с одной игрой или каждый со своим headless-инстансом на своём порту.
- Найденный баг сохраняется реплеем (`shards save-replay`), его воспроизводит человек или CI.
- Движок не запускает модели и не ходит к их API. Всё, что нужно агенту, добавляется как инструмент
  или команда `shards`, а не как код, который думает за агента.

## Тесты

```bash
source ~/.local/share/rustengine-sysdeps/env.sh   # пока не установлены apt-пакеты
cargo test --workspace
```

Всего 45 тестов:

- 23 юнит-теста `ai_core`: id (в том числе перенумерация и занятый id), журнал, RNG,
  компас и LOS, схемы, шаги симуляции, пути снимков, формат реплея и отчёт, запрет
  BRP-правки `AiId`, занятый порт, `instance` в `ai/status`, файл порта;
- 4 теста `shards`: парсер условий и аргументов, порт из `--port-file`, строка `ready`;
- 11 сценариев `crates/sandbox/tests/scenarios.rs`:
  - детерминизм и разные seed;
  - одинаковый результат при пошаговом и цельном прогоне;
  - полный порядок систем и контрольный тест к этой проверке;
  - фаза 2 босса через `set` + `step until named:boss_phase_2`;
  - подсказывающие ошибки;
  - `AiId` нельзя переписать через `set`, а вставка нового id перенумеровывает сущность без дублей
    в `observe`; игровой `AimAssist` виден в подсказке `set`;
  - права scope;
  - паника в инструменте;
- 7 сценариев `crates/sandbox/tests/replays.rs`:
  - сессия со всеми видами входов (dev-правки, инструменты игрока, клавиатура, spawn, possess,
    вызов после последнего тика) проигрывается побитово одинаково, клавиши во время просмотра игнорируются;
  - изменённая игра даёт отчёт с первым разошедшимся тиком;
  - action-вызов во время проигрывания перехватывает управление;
  - ветка после проигрывания сохраняется одним реплеем и тоже воспроизводится;
  - вложенные вызовы пишутся один раз;
  - проигрывание требует свежего мира с тем же seed;
  - `replay_save` пишет файл, который читается обратно.

Снимки требуют окна и GPU, поэтому в `cargo test` их нет. Без дисплея их можно проверить через
Xvfb и программный Vulkan (`apt install xvfb mesa-vulkan-drivers`):

```bash
Xvfb :99 &
DISPLAY=:99 cargo sandbox --paused &
cargo shards wait && cargo shards shot
```

## Статус по плану внедрения

| # | шаг | статус |
|---|---|---|
| 1 | Intent + FixedUpdate + seed | ✅ |
| 2 | AiId, текстовый срез, журнал | ✅ |
| 3 | pause / step / BRP / CLI | ✅ |
| 4 | скриншоты с метками | ✅ `ai/shot` / `shards shot`: PNG с метками и легенда в пикселях |
| 5 | реестр инструментов, схемы, scope | ✅ |
| 6 | внутриигровой агент | ✗ исключён: в движке нет LLM, агенты внешние (Claude Code, Codex, …) и работают через `shards` |
| 7 | реплеи / снапшоты / параллельный headless | ◐ реплеи (запись, проигрывание, `--verify`, ветвление) и параллельные headless-инстансы работают; снапшоты впереди |

## Дальше

1. Снапшоты через `DynamicScene`: сохраняться и откатываться в любой тик, а не только
   проигрывать с нуля. Реплей тогда начинается со снапшота.
2. Снимки без окна: камера рендерит в `Image`, `Screenshot::image` (для headless-инстансов с GPU).
3. Дерево UI в `observe`, `path_exists` поверх навмеша.
4. LOS для воксельного мира: подменить `AiLineOfSight` на DDA по чанкам.
5. Hot reload (`--features dynamic`, subsecond).

## Соглашения и сборка

- В движке нет LLM: ни встроенных агентов, ни вызовов моделей, ни API-ключей (см. [AGENTS.md](AGENTS.md)).
- Код и комментарии на английском, документация на русском.
- Версия Bevy закреплена (`0.20.0`). Обновлять её стоит осознанно, отдельным шагом:
  API Bevy меняется в каждом релизе.
- `.cargo/config.toml` ставит `jobs = 4`, потому что на этой машине мало RAM. Если памяти больше, увеличьте.
  Зависимости собираются с `opt-level = 3`, свой код с `opt-level = 1`.
- Features `sandbox`: `render` и `remote` (по умолчанию), `audio`, `gamepad`, `wayland`, `dynamic`.
  Без `render` игра собирается только для `--headless`.
