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
Клавиатура человека и инструменты агента пишут в один и тот же `Intent`, и нажатия идут через
тот же буфер ввода: уворот или удар, который не удалось начать за 0.2 с, виден в журнале как `rejected`.

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
                      Rules   — инварианты после каждого тика (invariants)
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
| `observe` | текстовый срез: YOU / SEEN (по угрозе) / NOT IN VIEW / LANDMARKS / SINCE, видимость от глаз субъекта; `inspect`, `nearby`, диффы |
| `spatial` | позиции относительно игрока («8.4m NE»), line of sight по `AiOccluder` (подменяемо) |
| `journal` | кольцевой буфер игровых событий (observers) плюс перехват `warn!`/`error!` из tracing: строка получает тик, на котором записана, и попадает в журнал после каждого тика; не больше 64 строк на тик |
| `tools` | реестр: args-struct + Bevy-система, JSON Schema (schemars), scope, `catch_unwind` |
| `builtin` | встроенные инструменты: observe, inspect, step, set/get через reflection, ... |
| `sim` | тик, пауза, синхронный `step` по FixedMain, стоп-условия |
| `invariants` | правила, которые проверяются после каждого тика: игровые (`add_ai_invariant`) и сессионные (`assert_add`); нарушение останавливает игру, как упавшая система |
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
   .add_ai_invariant("health_in_range", |w| /* Some(текст), если нарушено */)  // см. «Инварианты»
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
| **Player** (играет честно, только через `Intent`) | observe, inspect, nearby, line_of_sight, journal, tools, highlight, overlay, move_to, move, attack, dodge, stop |
| **Dev** (всё, включая время) | + pause, resume, speed, step, possess, teleport, despawn, get, set (reflection), spawn, heal, shot, replay_save, assert_add, assert_list, assert_remove, assert_reset |

У каждого инструмента есть вид (`kind` в `shards tools`):

- `query` только читает мир: без записи в журнал и без диффа;
- `action` меняет мир: пишет в журнал, возвращает `CHANGES` и попадает в реплей;
- `control` управляет временем или сессией (pause, resume, speed, step, possess): журнал и дифф
  как у action, но в реплей не попадает. Он меняет, *когда* что-то происходит, а не *что*.
  Исключение — правила (`assert_add`, `assert_remove`, `assert_reset`): мир они не меняют, но
  решают, где игра остановится, поэтому реплей записывает их отдельными событиями.

- **Действие возвращает изменения.** Каждый action-инструмент сравнивает срез мира до и после
  вызова и возвращает `CHANGES` и `JOURNAL`. Агенту не нужен второй `observe`, чтобы узнать, что произошло.
- **Ошибки подсказывают, как исправить вызов:**
  ``bad arguments for teleport: missing field `x`. Expected: teleport(id: integer|string, x: number, y?: number, z: number)``,
  `e57 no longer exists (Skeleton despawned at tick 812); live: …`,
  `e7 no longer exists (Skeleton renumbered to e99 at tick 40)`,
  ``unknown component `Mana`; this entity has: Health, …``,
  `AiId is managed by the engine; despawn and spawn instead`.
- **`dodge` и `attack` объясняют отказ.** Разовый вход (уворот, удар без цели) ждёт в буфере
  `InputBuffer` до 0.2 с, пока персонаж не сможет действовать, а `Intent` остаётся чистым входом.
  Инструмент сразу проверяет то же правило, что и игра (`combat::start_blocker`):
  - можно сейчас — как раньше: `ok: dodging S`, `ok: swinging`;
  - станет можно в пределах 0.2 с — `ok: queued, dodges in 0.12s (S)`, `ok: queued, swings in 0.10s`;
  - иначе ошибка с причиной и временем, а `Intent` не меняется:
    `not enough stamina: 10/25 (ready in 1.0s)`,
    `busy: windup Sword 0.80s (ready in 0.80s); a windup cannot be cancelled`.

  Правила прежние: уворот из `Idle` или `Recover` (он отменяет восстановление), удар только из
  `Idle`. Вход, записанный мимо инструментов (клавиатура, `Intent` напрямую), у которого вышло время,
  попадает в журнал: `e5 dodge rejected: stamina 10/25`, `e5 attack rejected: busy (windup Sword 0.10s)`.
  Ожидающий вход виден в строке сущности: `queued dodge 0.15s`. Это изменение геймплея: реплеи,
  записанные до буфера, расходятся, их нужно записать заново.
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
  `field:ID:Comp.path<N`, `named:NAME`, `warning`, `error` (первая запись журнала этого уровня,
  в том числе `warn!`/`error!` игры). Лимиты: до 216 000 тиков и 20 с реального времени за вызов.
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
- Реплей, записанный в окне, совпадает при проигрывании без окна (проверено вручную под Xvfb):
  рендер-сущности не сдвигают ни `AiId`, ни порядок геймплея.

## Инварианты

Инвариант — именованное правило, которое проверяется после каждого тика: и в `step`, и в
реальном времени. Нарушенное правило останавливает игру так же, как упавшая система:

- в журнале ошибка `invariant <name> violated at tick N: <текст>`;
- в реальном времени игра встаёт на паузу (`[sim] paused: invariant <name> violated at tick N`),
  следующие тики этого кадра не идут;
- сессия сохраняется в `replays/invariant-<name>-tick<N>.json`;
- `step` останавливается на этом тике и отвечает
  `stopped: invariant <name> violated at tick N (replay: replays/invariant-<name>-tick<N>.json)`,
  а `shards` выходит с кодом 3.

Правила игры задаёт код. Функция получает `&World` и возвращает текст нарушения или `None`:

```rust
app.add_ai_invariant("health_in_range", |w| /* Some("e6 Health.current = -5 outside 0..=4000") */);
```

В песочнице их два: `health_in_range` (здоровье в `0..=max`) и `bodies_in_arena` (все тела внутри
стен арены, `e7 outside the arena: x = 31.2`). Паника внутри правила тоже нарушение:
`the rule itself panicked: …`.

Агент добавляет свои правила на время сессии (scope Dev), условием в синтаксисе `--until`:

```bash
shards assert add hp 'field:6:Health.current<3900'   # остановиться, когда у босса меньше 3900 hp
shards assert add calm error                          # … на первой записи [error] в журнале
shards assert list
# 3 rules, checked after every tick
#   health_in_range  game rule  armed
#   bodies_in_arena  game rule  armed
#   hp  when field:e6:Health.current<3900  armed
shards act attack target=e6 repeat=true
shards step 600
# ran 225 ticks (0 → 225), stopped: invariant hp violated at tick 225 (replay: replays/invariant-hp-tick225.json)
shards assert reset                                   # снова включить сработавшие правила
shards assert remove hp
```

- Сработавшее правило выключено до `assert_reset`, иначе оно срабатывало бы на каждом тике, пока
  мир не исправлен. `assert_reset` включает все сработавшие правила (игровые тоже), и проверка
  продолжается со следующего тика.
- `warning` и `error` срабатывают на запись журнала этого уровня, в том числе на `warn!`/`error!`
  игры. Запись о нарушении другого правила их не будит.
- Имя правила — до 64 букв, цифр, `_` и `-`: из него получается имя файла реплея. Правило игры
  нельзя ни удалить, ни заменить своим.
- `assert add/remove/reset` записываются в реплей и повторяются на том же тике, поэтому реплей,
  сохранённый при нарушении, нарушает правило на том же тике. Сами правила в хэш не входят.
- `--verify` перечисляет нарушения, случившиеся при проигрывании, и выходит с кодом 1, если они были:

```text
invariant hp violated at tick 225: e6 Health.current = 3875
replay finished at tick 225: identical to the recording (4 checks); 1 invariant violated [225 ticks in 0.12s]
```

- Реплей пишется по одному на нарушение. Тестам он не нужен: `crash_replays: false` у
  `AiCorePlugin` выключает и его (или `AiInvariants.replays = false`).

**Фаззинг.** `shards fuzz --seed S --steps N` подаёт N случайных входов игрока (ходьба, удар,
уворот; цели берутся из `observe`), после каждого — `step` на 1–30 тиков (`--max-ticks`). Тот же
seed даёт те же входы. Если правило нарушилось, `fuzz` печатает ответ `step` и путь к реплею
нарушения и выходит с кодом 3; если нет — сохраняет сессию в `replays/fuzz-seed<S>.json`:

```text
$ shards assert add low 'field:5:Health.current<50'
$ shards fuzz --seed 3 --steps 300
fuzz seed 3: the game halted after 35 inputs (ticks 0 → 474)
ran 23 ticks (451 → 474), stopped: invariant low violated at tick 474 (replay: replays/invariant-low-tick474.json)
…
replay: replays/invariant-low-tick474.json
$ cargo sandbox --replay replays/invariant-low-tick474.json --verify     # код 1
invariant low violated at tick 474: e5 Health.current = 46
replay finished at tick 474: identical to the recording (8 checks); 1 invariant violated [474 ticks in 0.25s]
```

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
- **правила**: `assert_add`, `assert_remove`, `assert_reset` с тиком, на котором их вызвали
  (см. «Инварианты»);
- **проверки**: хэш игровых данных каждые `check_every` тиков (60 по умолчанию) и в конце записи:
  для каждой сущности с `AiVisible` по порядку `AiId` — её id и все отражаемые игровые компоненты
  (сериализация через reflection, компоненты в порядке имени типа). Рядом с хэшем лежит его
  разбивка, посчитанная в том же проходе: хэш каждого компонента каждой сущности и каждое значение
  `add_replay_check` под своим именем. По ней отчёт называет, что именно разошлось.

```bash
shards save-replay                          # или ai/act {"tool": "replay_save", "args": {"path": …}}
shards save-replay replays/boss-fight.json  # своё имя, но только внутри replays/
cargo sandbox --replay replays/tick332.json # окно, реальное время; seed и частота из файла
cargo sandbox --replay replays/tick332.json --headless --paused   # дальше шагами через shards step
cargo sandbox --replay replays/tick332.json --verify
# replay finished at tick 332: identical to the recording (6 checks) [332 ticks in 0.13s]
```

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
- Каждая проверка сравнивается с записью. Отчёт называет первую проверку, на которой игра перестала
  совпадать, и что в ней разошлось (подробнее — в «Что разошлось»). После изменения кода это
  регрессия. `--verify` годится для CI: код выхода 0 — совпало, 1 — разошлось, упала система
  геймплея или нарушен инвариант, 2 — реплей не читается или не подходит к игре.
- В конце записи игра становится живой: реплей работает и как точка сохранения для ветвления.
  Любой action-вызов во время проигрывания перехватывает управление и завершает его
  (`replay interrupted at tick 130 of 332: dev took over (teleport)`).
- Во время проигрывания запись продолжается, поэтому ветка сохраняется одним реплеем.
- Состояние записи и проигрывания видно в `ai/status` (поле `replay`), итог пишется в журнал.
- Формат: JSON `shards-replay/3` (`seed`, `tick_hz`, `check_every`, `check_detail`, `end`, `events`,
  `checks`; хэши hex-строками). События — `call`, `input` и `assert` (правила). У проверки, кроме
  `tick` и `hash`, есть `parts`: `{"entities": {"e6": {"Health": "…", "Transform": "…"}}, "game":
  {"weather": "…"}}`. Реплеи `shards-replay/2` (без правил, интервала и разбивки) проигрываются
  и проверяются по общему хэшу: он считается так же, байт в байт, а отчёт о расхождении пишет
  `no details: replay format 2`. Реплей `shards-replay/1` хэшировал текст и этой сборкой не
  проверяется: `play` отвечает `this replay was recorded by an older build (…); record it again`,
  а `--verify` выходит с кодом 2.

### Что разошлось

Отчёт, журнал и вывод `--verify` называют сущности и компоненты первой разошедшейся проверки, а
ниже — текущее значение каждого такого компонента в живой игре, одной строкой. Реальный вывод: реплей
записан с `--check-every 1`, потом в файле поменяли аргумент записанного вызова.

```bash
cargo sandbox --headless --paused --check-every 1 &
cargo shards wait && cargo shards act set id=e6 component=Health path=current value=2100
cargo shards act attack target=e6 repeat=true && cargo shards step 100
cargo shards act teleport id=e1 x=-19 z=-19 && cargo shards step 157
cargo shards save-replay replays/boss.json   # 257 ticks, 4 events, 258 checks, seed 42

# в копии файла set ставит здоровье 2075 вместо 2100
cargo sandbox --replay replays/boss-hp.json --verify
# replay finished at tick 257: DIVERGED at tick 1: e6 Boss: Health differs [257 ticks in 0.21s]
#   e6 now: Health { current: 2075.0, max: 4000.0 }

# в копии файла teleport ставит колонну на x=-18 вместо -19
cargo sandbox --replay replays/boss-pillar.json --verify
# replay finished at tick 257: DIVERGED at tick 101: e1 Pillar: Transform differs [257 ticks in 0.19s]
#   e1 now: Transform { translation: Vec3(-18.0, 0.0, -19.0), rotation: Quat(0.0, 0.0, 0.0, 1.0), scale: Vec3(1.0, 1.0, 1.0) }
```

- Что может быть в списке: `e6 Boss: Health, Transform differ` (разошлись компоненты, в том числе
  пропавшие или лишние), `e9 missing (Skeleton despawned at tick 40)` (сущность есть в записи, но не
  в игре; в скобках — что о её id знает индекс), `e12 extra (Skeleton)` (есть в игре, но не в записи),
  `replay check weather differs` (значение `add_replay_check`). В списке не больше 10 сущностей,
  остальные — одной строкой `… and 7 more`.
- Значение — отладочный вывод reflection без путей модулей, не длиннее 240 символов. Объясняется
  только первое расхождение, следующие проверки расходятся уже из-за него.
- В журнал попадают ошибка `replay diverged at tick 60: e6 Boss: Health differs` и по записи
  `replay: e6 now: …` на каждое значение. В `ai/status` поле `replay.last_report` содержит то же.
- Частоту проверок задают при записи: `--check-every N` у песочницы или `check_every` у
  `AiCorePlugin` (по умолчанию 60). Она хранится в файле, и проигрывание проверяет на тех же тиках;
  `--check-every` вместе с `--replay` — ошибка. С частотой 60 тик в отчёте — первая проверка после
  расхождения; чтобы найти точный тик, реплей пишут с `--check-every 1`.
- Подробность: `--check-detail` или `check_detail` у `AiCorePlugin`. `components` (по умолчанию) —
  хэш каждого компонента, отчёт называет сущности, компоненты и значения; `entities` — хэш сущности
  целиком, отчёт называет сущности (`e6 Boss differs`); `none` — только общий хэш, отчёт называет тик
  (`no details: the recording keeps hashes only (check detail none)`). Общий хэш от подробности не
  зависит.
- Размер: `components` с `--check-every 1` на арене из 8 сущностей — около 3 КБ на тик (реплей выше
  весит 760 КБ), а с частотой 60 — около 3 КБ на секунду игры. Тот же реплей с `entities` весит
  107 КБ, с `none` — 18 КБ.

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

**CLI `shards`:** observe, step, act, shot, save-replay, tools, inspect, nearby, journal, pause,
resume, speed, status, session, assert, fuzz, raw, wait. Аргументы `act` передаются JSON-объектом или парами `key=value`.
Флаги `--port`/`SHARDS_PORT`, `--port-file`/`SHARDS_PORT_FILE` (порт из файла игры; `--port` важнее),
`--token`/`SHARDS_TOKEN`, `--agent`/`SHARDS_AGENT`, `--json` (у `tools` — со схемами аргументов). `shards wait` ждёт, пока игра
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
  номер в файл (при выходе по Ctrl+C, `kill` или закрытию окна файл удаляется). `shards` читает его
  через `--port-file F` или `SHARDS_PORT_FILE=F`:

```bash
cargo sandbox --headless --paused --seed 1 --port 0 --port-file /tmp/shards-a.port &
cargo sandbox --headless --paused --seed 2 --port 0 --port-file /tmp/shards-b.port &
SHARDS_PORT_FILE=/tmp/shards-a.port cargo shards wait   # ready: tick 0 · pid 4242 · seed 1 · port 40817
SHARDS_PORT_FILE=/tmp/shards-a.port cargo shards step 600 --until named:player_dead
```

**Внутри игры** та же точка входа, что за BRP, — `ai_core::call_tool(world, agent, tool, args)`.
Её вызывают тесты, реплеи и клавиши разработчика в окне (P, N, R, F12).

## Коды выхода

`shards`:

| код | значит |
|---|---|
| 0 | готово |
| 1 | инструмент отказал; ошибка говорит, почему и как исправить вызов |
| 2 | сбой самого `shards`: игры нет на порту, нет токена, неверная командная строка |
| 3 | игра остановилась: `step` (или `fuzz`) встал на упавшей системе геймплея или нарушенном инварианте; ответ называет причину и реплей |

`cargo sandbox`:

| код | значит |
|---|---|
| 0 | игра закрыта; с `--verify` — реплей совпал с записью, ничего не сломалось |
| 1 | `--verify`: игра разошлась с записью, упала система геймплея или нарушен инвариант |
| 2 | реплей не читается или не подходит к игре; `--port-file` не записать |
| 3 | порт занят (`port 15711 is busy (another game?)`) |
| 128 + N | остановлена сигналом N: 130 — Ctrl+C, 143 — `kill`, 129 — закрытый терминал (SIGHUP) |

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
  …
$ shards act dodge dir=W
ok: dodging W
$ shards step 3
ran 3 ticks (133 → 136)
$ shards act dodge dir=E
error: busy: dodge 0.30s (ready in 0.30s)
$ shards step 8
ran 8 ticks (136 → 144)
$ shards act dodge dir=E
ok: queued, dodges in 0.17s (E)
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
- Гипотезу о баге проверяет правило: `shards assert add NAME COND`, затем `step` или
  `shards fuzz`. Игра остановится на тике нарушения и сама сохранит реплей (см. «Инварианты»).
- Движок не запускает модели и не ходит к их API. Всё, что нужно агенту, добавляется как инструмент
  или команда `shards`, а не как код, который думает за агента.

## Тесты

```bash
source ~/.local/share/rustengine-sysdeps/env.sh   # пока не установлены apt-пакеты
cargo test --workspace
```

Всего 103 теста:

- 33 юнит-теста `ai_core`: id (в том числе перенумерация и занятый id), журнал, RNG,
  компас и LOS, схемы, шаги симуляции, пути снимков (только `shots/`, чужой `x.json` цел), формат
  реплея и отчёт, запрет BRP-правки `AiId`, занятый порт, `instance` в `ai/status`, файл порта,
  пути внутри `replays/`/`shots/` (абсолютные, `..`, симлинки), токен у каждого метода BRP,
  имена правил, правила игры раньше правил сессии, JSON событий `assert`, разбивка проверки в JSON
  и обратно (при любом порядке ключей), файл версии 2 без разбивки, значения без путей модулей,
  текст отчёта о расхождении;
- 7 тестов `shards`: парсер условий (с `warning` и `error`) и аргументов, порт из `--port-file`,
  строка `ready`, поиск токена по номеру порта, коды выхода, входы `fuzz` зависят только от seed;
- 2 теста `crates/sandbox/tests/shutdown.rs` (запускают саму игру): после `kill` (код 143)
  и SIGHUP (код 129) нет ни токена, ни файла порта; под `nohup` SIGHUP игру не останавливает;
- 14 сценариев `crates/sandbox/tests/scenarios.rs`:
  - детерминизм и разные seed;
  - одинаковый результат при пошаговом и цельном прогоне;
  - полный порядок систем и контрольный тест к этой проверке;
  - фаза 2 босса через `set` + `step until named:boss_phase_2`;
  - подсказывающие ошибки;
  - `AiId` нельзя переписать через `set`, а вставка нового id перенумеровывает сущность без дублей
    в `observe`; игровой `AimAssist` виден в подсказке `set`;
  - видимость следует за субъектом: босс за колонной `occluded` для игрока и виден после `possess`
    скелета; с `fov_deg = 120` сущность за спиной в NOT IN VIEW; вывод `observe` с камерой окна
    и без неё одинаков, если убрать `scr(...)`;
  - права scope;
  - паника в инструменте;
- 5 сценариев `crates/sandbox/tests/actions.rs`: `dodge` без стамины — ошибка со словом `stamina`,
  и через тик ничего не изменилось; `dodge` во время замаха — ошибка со словом `windup`; действие,
  возможное через 0.1 с (стамина, замах, восстановление перед ударом), — `ok: queued` и начинается
  не позже чем через 12 тиков; правила уворота и удара прежние; вход прямо в `Intent` без стамины
  через 0.2 с даёт в журнале `dodge rejected`;
- 6 тестов `crates/sandbox/tests/system_failures.rs` (свой процесс: очередь сбоев глобальная):
  паника на тике 50 останавливает `step 100` на тике 50 с записью в журнале, следующий `step 10`
  доходит до тика 60 и `FixedMain`/`FixedUpdate` на месте, в реальном времени сбой ставит игру
  на паузу, краш-реплей воспроизводит сбой на том же тике, паника и упавшая команда инструмента —
  не сбой геймплея, без обработчика ответ называет потерянные расписания;
- 5 тестов `crates/sandbox/tests/log_capture.rs` (свой процесс: ставят глобальный логгер):
  предупреждение сохраняет свой тик внутри `step 100`, `step --until journal:` останавливается
  на нём, 100 строк за тик режутся до 64 и одной строки про 36 отброшенных, action возвращает
  свои предупреждения, `warn!` останавливает `step --until warning` и нарушает правило `warning`;
- 8 тестов `crates/sandbox/tests/invariants.rs`:
  - здоровье босса `-5` через `set`: `step 10` встаёт на тике 1 с ошибкой `health_in_range` в
    журнале и в `halted`, следующий `step` идёт дальше; в реальном времени игра встаёт на паузу;
  - правило `assert_add` срабатывает один раз, после `assert_reset` — снова на следующем тике,
    а после исправления и `assert_reset` шаг идёт дальше;
  - `assert_list`, замена и удаление правила, ошибки (плохое имя и условие, правило игры);
  - `step --until warning`/`error` и правила `warning`/`error`: правило не срабатывает на запись
    о нарушении другого правила;
  - тело за стеной ловит `bodies_in_arena`, паника в правиле — тоже нарушение;
  - реплей, сохранённый при нарушении, нарушает правило на том же тике, а `--verify` выходит
    с кодом 1 и называет нарушение;
- 23 сценария `crates/sandbox/tests/replays.rs`:
  - сессия со всеми видами входов (dev-правки, инструменты игрока, клавиатура, spawn, possess,
    вызов после последнего тика) проигрывается побитово одинаково, клавиши во время просмотра игнорируются;
  - изменённая игра даёт отчёт с первым разошедшимся тиком;
  - другое здоровье босса в записанном `set`: отчёт и журнал называют `e6 Boss: Health differs`
    и значение `now: Health { current: 2075.0, max: 4000.0 }`, а `--verify` печатает их и выходит
    с кодом 1;
  - колонна, телепортированная на метр дальше в угол, — единственная названная сущность;
  - реплей с `check_every 1` называет точный тик (38, с частотой 60 — 60);
  - подробность `entities` называет только сущности, `none` — только тик; общий хэш тот же, файл
    в разы меньше;
  - пропавшие (`e9 missing`) и лишние (`e21 extra (Skeleton)`) сущности, список до 10 и
    `… and 2 more`; удалённая сущность названа с тем, что с ней стало;
  - файл версии 2 проверяется по общему хэшу, а расхождение даёт `no details: replay format 2`;
  - `--check-every 0`, `--check-detail all` и `--check-every` вместе с `--replay` — ошибки;
  - action-вызов во время проигрывания перехватывает управление;
  - ветка после проигрывания сохраняется одним реплеем и тоже воспроизводится;
  - вложенные вызовы пишутся один раз;
  - проигрывание требует свежего мира с тем же seed;
  - `replay_save` пишет файл, который читается обратно, и только внутрь `replays/`;
  - проверка хэширует игру, а не текст: проигрывание с лишним describer совпадает;
  - реплей `shards-replay/1` отклоняется с причиной;
  - `replay_ignore` и `add_replay_check` меняют состав хэша, правка исключённого компонента
    при проигрывании не даёт расхождения;
  - компоненты окна не меняют хэш, `Entity` в компоненте хэшируется как `AiId`;
  - несериализуемый компонент пропускается, а в журнале о нём одна строка.

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
