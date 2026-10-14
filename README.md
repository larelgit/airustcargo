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
xvfb-run -a cargo sandbox --replay tests/replays/combat_prototype.json --verify --windowed  # то же в окне
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

Управление в окне — карта ввода движка (`InputMap::standard`, переназначается инструментом `rebind`):
WASD или стрелки — ходить, мышь — камера, Space — прыжок, Shift — спринт, F — планер, Ctrl — перекат,
ЛКМ — лёгкий удар (комбо), ПКМ — тяжёлый, Q — блок (держать), C — фляга, Tab — захват цели; геймпад
тоже. Клавиши разработчика: P — пауза, N или `.` — один тик на паузе, F1 — оверлей с id, F2 —
вылечить/воскресить, F3 — коллайдеры, F12 — снимок с метками в `shots/`.
Клавиатура человека и инструменты агента пишут в один и тот же `Intent`, и нажатия идут через
тот же буфер ввода: удар или перекат, который не удалось начать за 0.15 с, виден в журнале как `rejected`.

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
| `crates/sandbox` | тестовая арена, собранная только на `shards`, как игра: арена (игрок, 2 скелета, босс Frost Warden с четырьмя атаками и двумя фазами) и полоса препятствий; мир, персонажи и числа — контент в `assets/content/` (см. «Песочница») |
| `crates/shards_cli` | бинарник `shards`, тонкий клиент к `ai/*` |
| `crates/shards_script` | без Bevy: формат сценариев, их запуск, разбор условий `--until`; общий для `shards` и тестов |

Модули `ai_core`:

| модуль | ответственность |
|---|---|
| `id` | `AiId` (`e42`): стабильный монотонный id, никогда не переиспользуется; надгробия для удалённых. Id назначает только движок: `set` и BRP `world.mutate_components` его не меняют, а новая вставка перенумеровывает сущность (надгробие `renumbered to e99`) |
| `describe` | только явно зарегистрированные компоненты видны ИИ, у каждого короткий describe |
| `observe` | текстовый срез: YOU / SEEN (по угрозе) / NOT IN VIEW / LANDMARKS / SINCE, видимость от глаз субъекта; `inspect`, `nearby`, диффы |
| `spatial` | позиции относительно игрока («8.4m NE»), line of sight по `AiOccluder` (подменяемо) |
| `journal` | кольцевой буфер игровых событий (observers) плюс перехват `warn!`/`error!` из tracing: строка получает тик, на котором записана, и попадает в журнал после каждого тика; не больше 64 строк на тик |
| `tools` | реестр: args-struct + Bevy-система, JSON Schema (schemars), scope, `catch_unwind` |
| `builtin` | встроенные инструменты: observe, inspect, step, set/get через reflection, ... |
| `sim` | тик, пауза, синхронный `step` по FixedMain, стоп-условия |
| `replay` | запись входов и action-вызовов с тика 0, проигрывание, хэши состояния, отчёт о расхождении |
| `rng` | детерминированный RNG: счётчик по (seed, tick, AiId, salt) |
| `scenario` | запуск файлов сценариев в процессе через `call_tool` (`run_file`, `run_str`) |
| `session` | агенты: имя, scope (Player/Dev), «тело», курсор журнала |
| `remote` | методы `ai/*` для BRP (feature `remote`) |
| `highlight` | гизмо-подсветка и оверлей id поверх сцены (feature `render`) |
| `shot` | снимок окна с метками `eN` и легендой в пикселях картинки (feature `render`) |

## Что видит ИИ

```text
tick 207 (3.45s) | paused | speed 1x | seed 42 | you e5
YOU  e5 Player  hp 100/100  stam 100/100  poise 60/60  flask 3/3  at (0.0, 12.0) facing N
ALERT in hit zone of e6 shockwave (lands in 0.50s): move 1.2m W to leave it
SEEN (by threat)
  e6 Boss "Frost Warden"  hp 600/600  poise 140/140  windup shockwave 0.50s (zone line 9m×1.6m)  phase 1  7.4m N
  e7 Skeleton  ground 3.6m/s  hp 40/40  poise 20/20  4.5m NW
  e8 Skeleton  hp 40/40  poise 20/20  18.4m NE
LANDMARKS  e3 Pillar 8.6m NW · e4 Pillar 8.6m NE · e1 Pillar 20.2m N · e2 Pillar 20.2m N
SINCE seq 0
  207 e6 winds up shockwave (0.50s) ≈ line 9m×1.6m
```

- Видимость считается только от субъекта («YOU»): луч от его глаз (`pos + EYE_HEIGHT`) к цели
  в радиусе `AiPerception::radius` (40 м). Если сущность за колонной, к ней приписывается `occluded`.
  Камера окна на это не влияет: с окном и без вывод одинаков, даже пока следящая камера догоняет
  игрока после большого `step`, а после `possess` агент смотрит глазами своего тела.
- NOT IN VIEW — необязательный конус обзора: `AiPerception { fov_deg: Some(120.0), .. }`. Всё, что
  вне конуса вокруг `facing` субъекта, попадает туда. По умолчанию (`None`) обзор круговой и этого
  раздела нет.
- `scr(x,y)` — отдельная пометка: если в окне есть `AiCamera` и сущность в её кадре, к строке
  дописывается место на экране, например `e8 Skeleton  hp 40/40  6.3m NE  occluded  scr(905,342)`.
  На SEEN, `occluded` и NOT IN VIEW она не влияет. Легенда `shot` по-прежнему считается от камеры:
  она описывает картинку.
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
   .replay_input::<Intent>()                        // вход для реплеев (см. «Реплеи»)
   .replay_ignore::<Sparkle>()                      // косметика: не входит в проверки реплея
   .add_replay_check("weather", |w| /* u64 */);     // данные вне AiVisible-сущностей
```

Маркеры сущностей: `AiKind("Boss")`, `AiImportance(10.0)`, `AiLandmark`, `AiOccluder`, `AiPlayer`.

## Инструменты

Инструмент — это структура аргументов (serde + schemars) и обычная Bevy-система
`In<Call<Args>> -> ToolResult`. Схема и сигнатура генерируются автоматически.

| scope | инструменты |
|---|---|
| **Player** (играет честно, только через `Intent`) | observe, inspect, nearby, line_of_sight, journal, tools, highlight, overlay, content_list, content_get; движение: move_to (`path=true`), move, stop, jump, sprint, glide, clearance, path_to, input_bindings; бой: light_attack, heavy_attack, block, parry, dodge, use_item, use_technique, lock_on, attack |
| **Dev** (всё, включая время) | + pause, resume, speed, step, possess, teleport, despawn, get, set (reflection), spawn_prefab, heal, god, stagger, content_check, content_reload, content_apply, rebind, physics_overlay, shot, replay_save |

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
- **Инструменты боя объясняют отказ.** Нажатие ждёт в буфере ввода (0.15 с), пока действие не станет
  возможным, а `Intent` остаётся чистым входом. Инструмент сразу проверяет то же правило, что и машина
  действий (`can_start`):
  - можно сейчас — `ok: roll (W)`, `ok: light_1`;
  - станет можно в пределах буфера — `ok: queued, dodge starts in 0.05s (S)`, `ok: queued, heavy starts in 0.10s`;
  - иначе ошибка с причиной и временем, а `Intent` не меняется:
    `not enough stamina: 10/20 (ready in 0.2s)`,
    `busy: windup heavy 0.50s (ready in 0.90s, when a cancel window opens)`.

  Что чем отменяется, решает контент: замах лёгкого удара уступает перекату и блоку, тяжёлого — нет.
  Вход, записанный мимо инструментов (клавиатура, `Intent` напрямую), у которого вышло время, попадает
  в журнал: `e5 dodge rejected: stamina 7/20`. Ожидающий вход виден в строке сущности: `queued heavy 0.10s`.
- **Паника в инструменте не роняет игру.** Агент получает ошибку, запись уходит в журнал,
  инструмент перерегистрируется при следующем вызове. Расписания тиков при этом на месте; если
  паника всё-таки унесла их (игра отключила перехват, `catch_system_panics: false`), ответ так и
  говорит: `the game lost its FixedMain, FixedUpdate schedules`. Команда, которую поставил инструмент
  и которая упала, записывается как ошибка инструмента (`tool orphan: command … failed`): это не сбой
  геймплея, игра не встаёт на паузу.

## Время и детерминизм

- Весь геймплей идёт в `FixedUpdate`, по умолчанию 60 Гц, в жёстком порядке
  `SimSet::Input → Think → Act → Move → Resolve → Cleanup`.
- `SimRng` — счётчиковый RNG по ключу (seed, tick, AiId, salt). Результат не зависит от
  порядка систем и от того, сколько раз кто-то «крутил» генератор.
- `step` синхронно прогоняет `FixedMain` ровно N тиков, игра при этом стоит на паузе.
  Есть стоп-условия: `journal:TEXT`, `gone:ID`, `shows:ID:TEXT`, `near:A:B:M`,
  `field:ID:Comp.path<N`, `named:NAME`. Лимиты: до 216 000 тиков и 20 с реального времени за вызов.
- **Логи внутри `step`.** `warn!`/`error!` попадают в журнал с тиком, на котором записаны, сразу после
  этого тика, а не в конце шага. Поэтому `step --until journal:TEXT` останавливается на тике
  предупреждения. За тик в журнал идёт не больше 64 строк, остальные сводятся в одну:
  `70 [warn] 36 log lines dropped (limit 64 per tick)`. Предупреждение, которое вызвал action,
  приходит в его же `JOURNAL`.
- **Сбой геймплея останавливает игру, а не убивает её.** В Bevy 0.20 паника системы уходит в
  `FallbackErrorHandler`, а тот по умолчанию паникует снова. Эта паника уносит из мира `FixedMain`
  и `FixedUpdate`: каждый следующий `step` падал бы с `cannot run a tick here`, а в реальном времени
  тики молча переставали бы идти. `AiCorePlugin` ставит свой обработчик (`catch_system_panics`,
  по умолчанию `true`). Он пишет ошибку в лог и в очередь сбоев, а очередь разбирается после каждого
  тика:
  - `step` останавливается на тике сбоя и отвечает
    `stopped: gameplay system <name> panicked at tick 50: <msg>; crash replay: replays/crash-tick50.json`,
    в журнале — `system <name> panicked at tick 50: <msg>`;
  - в реальном времени игра встаёт на паузу (`[sim] paused: system <name> panicked at tick 50`),
    следующие тики этого кадра не идут;
  - сессия сохраняется в `replays/crash-tick<N>.json` (по одному файлу на упавшую систему).
    Воспроизвести сбой: `cargo sandbox --replay replays/crash-tick50.json` (или `--headless --paused`
    и `shards step`), игра упадёт на том же тике. `--verify` выходит с кодом 1, пока система падает;
  - следующий `step` идёт дальше: расписания на месте. `AiCorePlugin` включает ambiguity detection для `FixedUpdate`:
  две системы, которые трогают одни данные без явного порядка, дают warning, и он попадает в журнал.
  Тест `gameplay_systems_have_a_total_order` превращает это в ошибку, а контрольный тест
  проверяет, что сама проверка ловит такую систему.
- Тесты подтверждают: один seed и одни вызовы дают побитово одинаковый мир и журнал;
  `step 300` даёт то же, что 5 × `step 60` с `observe` и кадрами рендера между ними.
- Реплей одинаков с окном и без: `--verify --windowed` проигрывает его в окне (рендер, камера, карта
  ввода, визуал игры работают) и даёт тот же отчёт, что и `--verify` без окна. Рендер-сущности не
  сдвигают ни `AiId`, ни `ColliderId`, ни порядок геймплея.

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
- **проверки**: хэш игровых данных каждые 60 тиков и в конце записи: для каждой сущности с
  `AiVisible` по порядку `AiId` — её id и все отражаемые игровые компоненты (сериализация через
  reflection, компоненты в порядке имени типа).

```bash
shards save-replay                          # или ai/act {"tool": "replay_save", "args": {"path": …}}
shards save-replay replays/boss-fight.json  # своё имя, но только внутри replays/
cargo sandbox --replay replays/tick332.json # окно, реальное время; seed и частота из файла
cargo sandbox --replay replays/tick332.json --headless --paused   # дальше шагами через shards step
cargo sandbox --replay replays/tick332.json --verify
# replay finished at tick 332: identical to the recording (6 checks) [332 ticks in 0.13s]
xvfb-run -a cargo sandbox --replay tests/replays/obstacle_course.json --verify --windowed  # в окне, по 60 тиков на кадр
# replay finished at tick 1010: identical to the recording (17 checks) [1010 ticks in 2.03s, 21 frames]
```

**Эталонные реплеи** лежат в `tests/replays/` (папка в git, в отличие от `replays/`): полоса
препятствий и бой-прототип, сыгранные их сценариями на свежей игре. Тест песочницы проверяет каждый
файл самой игрой (`sandbox --replay FILE --verify`), CI — ещё и в окне под Xvfb. Если геймплей
поменялся намеренно, запишите их заново:
`cargo test -p sandbox --test sandbox -- --ignored record_reference_replays`.

- Игровые данные определяет то же правило, что и в `inspect`: всё, кроме типов `bevy_*` и внутренних
  маркеров `ai_core`, плюс `Transform` и `Name`. Поэтому компоненты, которые добавляет только окно
  (`Visibility`, `GlobalTransform`, меши), хэш не меняют, а `Entity` внутри компонента хэшируется
  как `AiId`. Подписи и describe-фрагменты — это текст для ИИ, в хэш они не входят: новая формулировка
  или новый describer не ломают старые реплеи.
- Состав хэша задаёт игра: `replay_ignore::<C>()` исключает компонент (косметика, кэши),
  `add_replay_check(name, fn(&World) -> u64)` добавляет данные вне `AiVisible`-сущностей (снаряды
  без id, ресурсы). Компонент, который не сериализуется, один раз попадает в журнал
  (`replay checks leave out …`) и дальше пропускается.
- `replay_save` пишет только внутрь `replays/` рабочей папки игры (вложенные папки можно).
  Абсолютный путь, `..` и симлинк, который ведёт наружу, — ошибка с подсказкой:
  `path must be inside replays/, e.g. replays/boss-fight.json`.
- Проигрывание требует свежего мира: тик 0, тот же seed и та же частота тиков. События
  применяются между тиками, ровно там, где они случились. Входы, которых нет в записи
  (человек жмёт клавиши во время просмотра), откатываются.
- Каждая проверка сравнивается с записью. Отчёт называет первый тик, на котором игра перестала
  совпадать: `DIVERGED at tick 60`. После изменения кода это регрессия. `--verify` годится для CI:
  код выхода 0 — совпало, 1 — разошлось или упала система геймплея, 2 — реплей не читается
  или не подходит к игре.
- В конце записи игра становится живой: реплей работает и как точка сохранения для ветвления.
  Любой action-вызов во время проигрывания перехватывает управление и завершает его
  (`replay interrupted at tick 130 of 332: dev took over (teleport)`).
- Во время проигрывания запись продолжается, поэтому ветка сохраняется одним реплеем.
- Состояние записи и проигрывания видно в `ai/status` (поле `replay`), итог пишется в журнал.
- Формат: JSON `shards-replay/2` (`seed`, `tick_hz`, `end`, `events`, `checks`; хэши hex-строками).
  Реплей `shards-replay/1` хэшировал текст и этой сборкой не проверяется: `play` отвечает
  `this replay was recorded by an older build (…); record it again`, а `--verify` выходит с кодом 2.

## Сценарии

Сценарий — текстовый файл: команды `shards`, ожидания и инварианты. Один и тот же файл идёт в
`cargo test` без окна и против запущенной игры (`shards scenario FILE...`). Смысл у него одинаковый:
формат и все правила (что делает команда, как проверяется ожидание, откуда считает `mark`) лежат в
`shards_script`, а у двух запускателей только транспорт: `ai_core::call_tool` и BRP `ai/*`.

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

- **Кто что делает.** Команды идут от агента `scenario` со scope `dev` (`session --agent` переключает
  его; флаг `--agent` у `shards` на сценарии не действует). Ожидания и инварианты читают мир другим агентом, `scenario-check` со scope `dev`: сценарий
  может играть за агента со scope `player` и всё равно видеть всё, а проверки не двигают курсор
  журнала проверяемого агента. Записи журнала с вызовами агентов (`[agent] …`) в `expect journal`
  не попадают: `step --until journal:died` не найдёт сам себя.
- **Регистр.** `expect result` различает регистр, чтобы можно было проверить точный текст ошибки
  (`Expected: teleport(`). `journal`, `stopped` и `shows` не различают, как `--until journal:`/`shows:`.
- **Инварианты** проверяются после каждой следующей команды `act`, `act?` и `step`, а не между тиками
  одного `step`. Первое нарушение проваливает сценарий и называет команду, которая его вызвала:
  `invariant `field e5 Health.current > 0` (line 12) broke: e5 Health.current is 0, which is not > 0`.
  Длинный шаг режьте на части или сторожите условием: `step 600 --until field:e5:Health.current<1`.
- **Оборванный шаг** проваливает сценарий, хотя вызов `step` успешен: упала система геймплея
  (`stopped: gameplay system … panicked`) или тики не уложились в 20 с реального времени. Падение,
  которого сценарий ждёт, разрешает `act? step …`.
- **`expect !journal`** (и такой же инвариант) проваливается, если часть записей с `mark` вытеснена из
  буфера журнала и не прочитана: среди них могла быть искомая. Ставьте `mark` чаще.
- **Ошибки разбора** называют строку: ``obstacle.scenario:12: unknown command `stpe`; expected act, act?,
  step, …``. Показываются сразу все плохие строки, и пока они есть, не запускается ничего.
- **Мир берётся как есть.** Сценарий не перезапускает игру. Чтобы файл шёл и на свежей игре, и после
  других, он сам приводит мир в порядок первыми командами (`heal`, `teleport`, `set`): так сделаны
  `crates/sandbox/scenarios/*.scenario`.

В тесте:

```rust
let mut app = sandbox::test_app(42);
let report = ai_core::scenario::run_file(app.world_mut(), "scenarios/boss_fight.scenario");
assert!(report.passed, "{report}");   // отчёт — построчный журнал и вердикт
```

Против игры (`--quiet` печатает только упавшие команды и вердикты, `--json` — отчёты в JSON):

```text
$ shards scenario crates/sandbox/scenarios/boss_fight.scenario
ok    8 seed 42 → seed 42
…
ok   39 step 900 --until near:e5:e6:3.2 → ran 129 ticks (150 → 279), stopped: e5 within 3.2m of e6
ok   40 expect stopped "within" → e5 within 3.2m of e6
…
PASS crates/sandbox/scenarios/boss_fight.scenario (49 commands, 452 ticks)
```

Код выхода: 0 — все прошли, 1 — сценарий не прошёл (`FAIL file:line: сообщение`), 2 — файл не читается или
не разбирается, игра недоступна. Если игра пропала посреди файла, остальные файлы не запускаются.

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
  `<имя>.png.legend.json` (`path`, `width`, `height`, `tick`, `marks`, `legend`): такое имя не
  столкнётся с чужим `<имя>.json`.
- Снимки пишутся только внутрь `shots/` рабочей папки игры: `-o shots/boss.png` можно, абсолютный
  путь, `..` и выход через симлинк — нет (`path must be inside shots/, e.g. shots/boss.png`).
- Оба файла пишутся во временные и переименовываются, так что появившийся PNG уже целиком на диске.
  Старый снимок и его легенда с тем же именем удаляются при запросе, и только внутри `shots/`.
  В журнале появляется `screenshot saved: …`.
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
BRP даёт полный доступ к миру, поэтому сервер слушает только 127.0.0.1, включается feature `remote`
и пускает только с токеном:

- при старте игра пишет случайный токен в `.shards/<port>.token` своей рабочей папки (права 0600)
  и удаляет файл при выходе: по Ctrl+C, `kill`, закрытию окна или терминала (если игра запущена
  не через `nohup`). После `kill -9` файл остаётся, следующая игра на этом порту его перепишет;
- каждый метод — `ai/*`, стандартные `world.*`, `registry.*`, `rpc.discover` и `+watch` — требует
  `params.token`. Без него или с чужим токеном ответ один:
  `unauthorized: run shards from the game folder or set SHARDS_TOKEN`;
- `shards` берёт токен из `SHARDS_TOKEN` (`--token`) или из `.shards/<port>.token`, поднимаясь от
  текущей папки вверх, и сам добавляет его к параметрам. Своему клиенту передайте его явно:

```bash
TOKEN=$(cat .shards/15702.token)
curl -d '{"jsonrpc":"2.0","id":1,"method":"ai/status","params":{"token":"'$TOKEN'"}}' http://127.0.0.1:15702
```

Релизная сборка без BRP:

```bash
cargo build --release -p sandbox --no-default-features --features render
```

**CLI `shards`:** observe, step, act, scenario, shot, save-replay, tools, inspect, nearby, journal, pause,
resume, speed, status, session, raw, wait. Аргументы `act` передаются JSON-объектом или парами `key=value`.
Флаги `--port`/`SHARDS_PORT`, `--port-file`/`SHARDS_PORT_FILE` (порт из файла игры; `--port` важнее),
`--token`/`SHARDS_TOKEN`, `--agent`/`SHARDS_AGENT`, `--json` (у `tools` — со схемами аргументов). `shards wait` ждёт, пока игра
ответит и загрузится (контент; пока идёт загрузка, `step` отвечает ошибкой), и печатает, кто ответил: `ready: tick 0 · pid 4242 · seed 1 · port 15711` (ту же строку игра
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
  номер в файл (при выходе по Ctrl+C, `kill` или закрытию окна файл удаляется). `shards` читает его
  через `--port-file F` или `SHARDS_PORT_FILE=F`:

```bash
cargo sandbox --headless --paused --seed 1 --port 0 --port-file /tmp/shards-a.port &
cargo sandbox --headless --paused --seed 2 --port 0 --port-file /tmp/shards-b.port &
SHARDS_PORT_FILE=/tmp/shards-a.port cargo shards wait   # ready: tick 0 · pid 4242 · seed 1 · port 40817
SHARDS_PORT_FILE=/tmp/shards-a.port cargo shards step 600 --until named:player_dead
```

**Внутри игры** та же точка входа, что за BRP, — `ai_core::call_tool(world, agent, tool, args)`.
Её вызывают тесты, реплеи и клавиши разработчика в окне (P, N, F2, F3, F12).

## Пример сеанса (реальный вывод)

```text
$ shards step --until journal:winds
ran 207 ticks (0 → 207), stopped: journal: 207 e6 winds up shockwave (0.50s) ≈ line 9m×1.6m
CHANGES
  ~ e6 Boss "Frost Warden": +windup shockwave 0.50s (zone line 9m×1.6m); moved 10.6m S
  ~ e7 Skeleton: +ground 3.6m/s; moved 11.1m SE
$ shards act dodge
ok: roll (W)
$ shards step 40
ran 40 ticks (207 → 247)
CHANGES
  ~ e5 Player: stam 100/100 → stam 88/100; moved 4.0m W
  ~ e6 Boss "Frost Warden": windup shockwave 0.50s (zone line 9m×1.6m) → active shockwave 0.08s (zone line 9m×1.6m)
  …
$ shards act attack target=e6 repeat=true
ok: attacking e6 with light until it dies
$ shards step --until journal:light_2
ran 109 ticks (247 → 356), stopped: journal: 356 e5 -> e6 light_2 -22 (hp 558)
CHANGES
  ~ e5 Player: hp 100/100 → hp 92/100; stam 88/100 → stam 84/100; poise 60/60 → poise 50/60; …
  ~ e6 Boss "Frost Warden": hp 600/600 → hp 558/600; poise 140/140 → poise 114/140; …
JOURNAL
  264 e7 -> e5 claw -8 (hp 92)
  338 e5 -> e6 light_1 -20 (hp 580)
$ shards act heavy_attack
ok: queued, heavy starts in 0.10s
$ shards act dodge dir=S
ok: queued, dodge starts in 0.05s (S)
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

Всего 380 тестов в `cargo test --workspace` (ещё 12 `#[ignore]`: примеры в документации и запись
эталонных реплеев). Среди них:

- 33 юнит-теста `ai_core`: id (в том числе перенумерация и занятый id), журнал, RNG,
  компас и LOS, схемы, шаги симуляции, пути снимков (только `shots/`, чужой `x.json` цел), формат
  реплея и отчёт, запрет BRP-правки `AiId`, занятый порт, `instance` в `ai/status`, файл порта,
  пути внутри `replays/`/`shots/` (абсолютные, `..`, симлинки), токен у каждого метода BRP;
  запуск сценариев в процессе (7): проходящий сценарий, инвариант называет сломавшую его команду,
  проверки видят всё, пока сценарий играет за `player`, отказ `session`, `seed`, ошибки разбора и
  нечитаемый файл — отчёты, а не паники;
- 7 тестов `shards`: порт из `--port-file`, строка `ready`, поиск токена по номеру порта; `shards scenario`
  против поддельной игры на localhost (коды выхода 0/1/2, отказ игры на `session`, журнал без
  записей агентов, файлы разбираются до запуска);
- 33 юнит-теста `shards_script` (без Bevy): парсер условий и аргументов, слова как в shell, разбор
  каждой команды и текст каждой ошибки, запуск на поддельной игре (ожидания, `mark`, инварианты,
  `session`, `seed`, потерянная игра), разбор текста журнала;
- 53 теста песочницы в одном бинарнике `crates/sandbox/tests/sandbox/` (и `#[ignore]`-тест, который
  записывает эталонные реплеи):
  - `scenario_files` (3): каждый файл из `crates/sandbox/scenarios/` проходит на свежей игре
    (`sandbox::test_app(42)`) и все подряд в одной игре, как их гонит `shards scenario`; бой-прототип
    выигран после шести разных пауз перед ним;
  - `reference_replays` (1): каждый `tests/replays/*.json` проверяется самой игрой (`--verify`);
  - `actions` (10): перекат без выносливости — ошибка с временем, `Intent` не тронут; замах лёгкого удара
    уступает перекату, тяжёлого — нет; комбо из трёх ударов; блок и идеальный блок (страж шатается);
    i-frames переката; фляга и её заряды; просроченный вход в журнале; предупреждение стража за 0.50 с и
    никаких двух «зон» подряд; фаза 2 со вторым мувсетом; тела скелетов исчезают, стража — остаётся;
  - `content` (5): контент чист; мир спавнится по layout; правка числа меняет следующий удар и реплей
    совпадает; правка layout перестраивает поток, ориентиры сохраняют id; префабы;
  - `course` (4): прыжок с разбега не перелетает бассейн, прыжок с обрыва без планера — в каньон,
    планер без потока не долетает, с потоком — садится на площадку цели;
  - `scenarios` (14): детерминизм и разные seed, пошаговый и цельный прогон, полный порядок систем и
    контроль этой проверки, фаза 2 по `set`, подсказывающие ошибки, `AiId` под управлением движка,
    подсказки `set`, видимость (колонна-коллайдер закрывает босса, `possess`, конус обзора, камера окна),
    права scope, паника в инструменте;
  - `replays` (14): прежние проверки реплеев на новой игре (входы всех видов, расхождение на тике 60,
    перехват, ветка, вложенные вызовы, свежий мир, файлы только в `replays/`, хэш игры, а не текста,
    `shards-replay/1`, `replay_ignore`/`add_replay_check`, компоненты окна и `Entity` в хэше,
    несериализуемый компонент);
  - `shutdown` (2): после `kill` (код 143) и SIGHUP (код 129) нет ни токена, ни файла порта; под `nohup`
    SIGHUP игру не останавливает;
- 7 тестов `crates/sandbox/tests/system_failures.rs` (свой процесс: очередь сбоев глобальная):
  паника на тике 50 останавливает `step 100` на тике 50 с записью в журнале, следующий `step 10`
  доходит до тика 60 и `FixedMain`/`FixedUpdate` на месте, в реальном времени сбой ставит игру
  на паузу, краш-реплей воспроизводит сбой на том же тике, паника и упавшая команда инструмента —
  не сбой геймплея, без обработчика ответ называет потерянные расписания;
- 4 теста `crates/sandbox/tests/log_capture.rs` (свой процесс: ставят глобальный логгер):
  предупреждение сохраняет свой тик внутри `step 100`, `step --until journal:` останавливается
  на нём, 100 строк за тик режутся до 64 и одной строки про 36 отброшенных, action возвращает
  свои предупреждения.

Снимки требуют окна и GPU, поэтому в `cargo test` их нет. Без дисплея их можно проверить через
Xvfb и программный Vulkan (`apt install xvfb mesa-vulkan-drivers libxkbcommon-x11-0`):

```bash
Xvfb :99 &
DISPLAY=:99 cargo sandbox --paused &
cargo shards wait && cargo shards shot
# видимость не зависит от камеры: после шага с движением два observe подряд дают одинаковые
# SEEN и NOT IN VIEW, хотя камера ещё догоняет игрока (меняются только scr(x,y))
cargo shards act move dir=W meters=9 && cargo shards step 300
cargo shards observe; sleep 1; cargo shards observe
```

## Бой: действия, урон, статусы (`shards_action`)

Атака, блок, парирование, перекат и фляга — одно и то же: **действие**, временная шкала из контента
(`*.action.ron`). Одна машина действий на бойца (`Fighter`) ведёт игрока, врагов и боссов; **мувсет**
(`*.moveset.ron`) говорит, какой ввод какое действие запускает, плюс слоты `stagger`, `hit`, `death`.

```ron
// content/combat/hero.action.ron
{
    "light_1": (
        net_id: 1,
        phases: [("windup", 0.2), ("active", 0.1), ("recovery", 0.3)],
        cost: (stamina: 10),
        hits: [(span: Phase("active"), shape: Cone(range: 2.0, angle: 100), damage: 20, poise: 10)],
        cancels: [(span: Secs(0.3, 0.55), into: [Combo("light", "light_2"), Input("dodge")])],
        displacement: [(0.0, 0.0), (0.3, 0.5)],
    ),
    "roll": (net_id: 5, phases: [("roll", 0.4)], cost: (stamina: 25), iframes: [Secs(0.0, 0.18)],
             displacement: [(0.35, 4.0)], steer: Move),
    "block": (net_id: 6, phases: [("raise", 0.1), ("hold", 0.3), ("lower", 0.1)],
              hold: (input: "block", span: Phase("hold")), move_speed: 0.4,
              guard: (span: Phases("raise", "hold"), perfect: Phase("raise"), damage_mult: 0.1)),
}
```

- Окна — это `Span`: фаза (`Phase("active")`), от фазы до фазы или секунды (`Secs(0.3, 0.45)`). Есть окна
  удара (сфера, капсула, конус, коробка, круг, линия по земле; урон, урон стойкости, статусы, флаги
  `Unblockable`/`GuardBreak`/`NoParry`), неуязвимости, блока и идеального блока, гиперброни, отмены
  (комбо — цепочка переходов `Combo`), удержания (блок держится, пока зажат ввод). Тик, в котором
  действие кончается, ещё его часть: окна удара и защиты его видят. Окно защиты, которое начинается до
  промежутка удержания и кончается внутри него, валидация контента отвергает: оно открывалось бы на
  каждом круге.
- Смещение (`displacement`) действие отдаёт контроллеру через `LocomotionInput::external`: двигает тело
  только контроллер, поэтому выпад останавливается о стену.
- Нажатие ждёт в буфере ввода `CombatTuning::input_buffer` (0.15 с). Одно правило `can_start` отвечает и
  машине, и инструментам: `ok: light_1`, `ok: queued, light starts in 0.10s`, `not enough stamina: 20/40
  (ready in 1.5s)`, `busy: windup light_1 0.17s (ready in 0.27s, when a cancel window opens)`. Нажатие, чьё
  время вышло, попадает в журнал: `e5 light rejected: busy (active light_1 0.10s)`.
- Попадания ищутся в `SimSet::Resolve` после движения: формы ударов пересекаются с хёртбоксами (коллайдеры
  со слоем `HURTBOX`, его получает коллайдер `Fighter`), атакующие и цели по порядку `AiId`, одна цель —
  один раз за действие. Конвейер урона: бессмертие (`god`) → неуязвимость → парирование / блок / пробитие
  защиты → броня → хуки игры (`add_damage_hook`) → здоровье, статусы, стойкость (пробитие — `stagger`),
  смерть.
- Ресурсы бойца: `Health`, `Stamina` (восстановление после задержки, спринт её тратит; выжатый досуха спринт не включается, пока
  кнопку не отпустят), `Poise`, `Armor`,
  `Team`, `Charges` (фляга), `Dead`. Статусы (`*.status.ron`): накопление до порога, спад, длительность,
  сопротивления, общий лимит активных; сами эффекты — хуки игры (`add_status_effect`).
- Телеграфы: событие `Telegraph` с символом и звуковой меткой, `hit_zones(world)` для декалей и ИИ,
  `threat_indicators(world, viewer, forward)` для предупреждений за кадром.

Что видит ИИ: `windup slam 0.78s (zone circle 4.5m) hyper-armor`, `stam 75/100`, `poise broken 0.8s`,
`bleed 60%`, `queued light 0.13s`; под строкой `YOU` — подсказка
`ALERT in hit zone of e2 slam (lands in 0.78s): move 3.0m S to leave it` (путь по одному из 8 румбов,
округлённый вверх: сделав ровно так, из зоны выходят); в журнале
`e2 winds up slam (0.80s) ⚠ circle 4.5m`, `e1 -> e2 heavy -45 (hp 55)`, `e1 blocked e2 swipe: -2 hp,
-15 stamina (hp 98)`, `e2 staggered (parried)`. Инструменты игрока (только через `Intent`): `light_attack`,
`heavy_attack`, `block on=…`, `parry`, `dodge dir=…|away_from=…`, `use_technique slot=1`, `use_item
item=flask`, `lock_on target=…|off=true`, `attack target=… input=… repeat=true`; разработчика: `god`, `heal`,
`stagger`.

## Песочница: арена и полоса препятствий

Песочница (`crates/sandbox`) собрана только на `shards`, как игра. Своего кода в ней немного: мир по
контенту, мозги монстров (пишут `Intent`, как клавиатура), фазы босса, останки, условия для `step`,
визуал окна. Всё, что числа и определения, — контент в `crates/sandbox/assets/content/` с горячей
перезагрузкой:

| файл | что в нём |
|---|---|
| `world/arena.layout.ron` | арена 40×40 м: пол, стены, колонны, ворота на восток; кого и где спавнить |
| `world/course.layout.ron` | полоса препятствий от x = 40: ступени, уступ 1.3 м, бассейн 10 м, пандус на обрыв 18 м над каньоном, площадка цели через 32 м пропасти, восходящий поток посередине |
| `characters/characters.prefab.ron` | префабы `player`, `skeleton`, `skeleton_captain`, `frost_warden`, `pillar` |
| `characters/bodies.locomotion.ron` | как ходят игрок, скелет и страж |
| `combat/player.action.ron` | комбо `light_1`–`light_3`, `heavy`, перекат (i-frames 0.18 с), блок с идеальным блоком, фляга |
| `combat/warden.action.ron` | четыре атаки стража (`slash`, `slam`, `sweep`, `shockwave`), спокойные и в ярости |
| `combat/skeleton.action.ron`, `combat/fighters.moveset.ron`, `combat/tuning.combat_tuning.ron` | коготь скелета, мувсеты, буфер ввода |

`layout` — вид контента самой песочницы: коробки (`solids`), пандусы, объёмы (вода, поток, триггер),
ориентиры для ИИ и префабы для спавна. Слои спавнятся по порядку id, внутри — по порядку списков, так
что `AiId` и `ColliderId` одинаковы в каждом запуске: e1–e4 колонны, e5 игрок, e6 страж, e7–e8 скелеты,
e9–e13 ориентиры полосы `Stairs`, `Ledge`, `Pool`, `Cliff`, `Goal`. Правка layout на лету перестраивает
геометрию на следующем тике; ориентиры сохраняют id, персонажи остаются.

**Бой — прототип из GDD 39.** У игрока комбо из трёх лёгких ударов через окна отмены, тяжёлый удар с
гипербронёй на замахе, перекат, блок (держать; первые 0.15 с — идеальный блок, он и есть парирование:
урона нет, атакующий шатается), фляга (45 HP, 3 заряда). Выносливость тратят удары, перекат, блок и
спринт; стойкость есть у всех. Страж выбирает из своих атак те, что достают до цели (`SimRng`), и
никогда не делает две «зоны» подряд (тег `zone` в контенте); между атаками отдыхает. Предупреждение
(`e6 winds up slam (0.50s) ⚠ circle 4m`) приходит ровно за 0.50 с до удара. С половины здоровья — фаза 2:
второй мувсет, короче замахи и отдых. Скелеты: один коготь, вздрагивают от ударов, их тела исчезают
через 1.5 с; тело стража остаётся (dev `heal` его поднимает).

**Полоса препятствий (E2).** Прыжок (1.2 м) не перепрыгивает ничего: уступ 1.3 м берётся
перелезанием, бассейн 10 м — вплавь с выходом через `jump`, до площадки цели долетает только планер и
только с подъёмом в потоке. Площадка — триггер: `e5 reached goal`.

**Сценарии-эталоны** (`crates/sandbox/scenarios/`), агент со scope `player`; время двигает агент
сценария:
- `obstacle_course.scenario` — `path_to` и `clearance`, ступени, уступ, бассейн, пандус, планер через
  поток, цель; инварианты: жив, ни разу не ниже старта;
- `combat_prototype.scenario` — страж побеждён за 11 раундов. Каждый раунд читается по предупреждению
  (`step --until "journal:e6 winds up"`), дальше перекат из зоны, блок, идеальный блок с тяжёлым ударом,
  обмен ударами под гипербронёй, фляга, комбо. Тест прогоняет его после шести разных пауз (другие атаки
  в другом порядке) — победа каждый раз;
- `boss_fight`, `dodge`, `errors` — переход в фазу 2, перекат против попадания, ошибки, которые учат.

Сессии двух первых записаны как эталонные реплеи в `tests/replays/` (см. «Реплеи»).

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
- Features `sandbox` (и `shards`): `render`, `remote` и `hot_reload` (по умолчанию), `audio`, `gamepad`,
  `wayland`, `dynamic`. Без `render` игра собирается только для `--headless`.
