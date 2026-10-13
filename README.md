# Shards: игровой движок с ИИ-интеграцией (Rust + Bevy 0.20)

ИИ не разглядывает пиксели. Движок и так знает всё, что есть в кадре, поэтому
он **описывает мир структурированным текстом** и даёт **типизированный API действий**.
ИИ подключается к движку напрямую, без MCP-прослоек. Есть два пути:

- **в процессе**: `ai_core::call_tool(world, agent, tool, args)` или канал `AiLink`
  из любого потока (для внутриигрового LLM-агента);
- **по localhost**: JSON-RPC методы `ai/*` поверх Bevy Remote Protocol и CLI `shards`
  поверх них (для Claude Code, скриптов, ботов, CI).

Скриншоты здесь вторичный канал, для визуальных багов. Метки `eN` на экране
совпадают с id в тексте.

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
P — пауза, N или `.` — один тик на паузе, F1 — оверлей с id, R — вылечить/воскресить.
Клавиатура человека и инструменты агента пишут в один и тот же `Intent`.

## Архитектура

```text
источники намерений:  игрок · сетевой клиент · ИИ · реплей
                              │  Intent
геймплей:             FixedUpdate, фиксированный тик, seed   (SimSet, SimRng)
                              │
ai_core:              Observe — срез мира текстом            (observe, inspect, nearby)
                      Journal — что случилось с прошлого раза (журнал событий, warn!/error!)
                      Tools   — типизированные действия      (add_ai_tool, Scope)
                      SimCtl  — пауза · шаг · run-until      (sim)
                              │
транспорты:           AiLink (в процессе, для внутриигровых агентов)
                      BRP `ai/*` на 127.0.0.1 → CLI `shards` · скрипты · боты
```

| crate | что это |
|---|---|
| `crates/ai_core` | слой ИИ, не зависит от конкретной игры |
| `crates/sandbox` | тестовая арена: игрок, 2 скелета, босс Frost Warden (2 фазы, телеграфируемые атаки) |
| `crates/shards_cli` | бинарник `shards`, тонкий клиент к `ai/*` |

Модули `ai_core`:

| модуль | ответственность |
|---|---|
| `id` | `AiId` (`e42`): стабильный монотонный id, никогда не переиспользуется; надгробия для удалённых |
| `describe` | только явно зарегистрированные компоненты видны ИИ, у каждого короткий describe |
| `observe` | текстовый срез: YOU / SEEN (по угрозе) / NOT IN VIEW / LANDMARKS / SINCE; `inspect`, `nearby`, диффы |
| `spatial` | позиции относительно игрока («8.4m NE»), line of sight по `AiOccluder` (подменяемо) |
| `journal` | кольцевой буфер игровых событий (observers) плюс перехват `warn!`/`error!` из tracing |
| `tools` | реестр: args-struct + Bevy-система, JSON Schema (schemars), scope, `catch_unwind` |
| `builtin` | встроенные инструменты: observe, inspect, step, set/get через reflection, ... |
| `sim` | тик, пауза, синхронный `step` по FixedMain, стоп-условия |
| `rng` | детерминированный RNG: счётчик по (seed, tick, AiId, salt) |
| `session` | агенты: имя, scope (Player/Dev), «тело», курсор журнала |
| `link` | `AiLink`: mpsc-канал к миру из любого потока |
| `remote` | методы `ai/*` для BRP (feature `remote`) |
| `highlight` | гизмо-подсветка и оверлей id поверх сцены (feature `render`) |

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
   .add_ai_condition("boss_phase_2", |w| /* ... */);
```

Маркеры сущностей: `AiKind("Boss")`, `AiImportance(10.0)`, `AiLandmark`, `AiOccluder`, `AiPlayer`.

## Инструменты

Инструмент — это структура аргументов (serde + schemars) и обычная Bevy-система
`In<Call<Args>> -> ToolResult`. Схема и сигнатура генерируются автоматически.

| scope | инструменты |
|---|---|
| **Player** (играет честно, только через `Intent`) | observe, inspect, nearby, line_of_sight, journal, tools, highlight, overlay, move_to, move, attack, dodge, stop |
| **Dev** (всё, включая время) | + pause, resume, speed, step, possess, teleport, despawn, get, set (reflection), spawn, heal |

- **Действие возвращает изменения.** Каждый action-инструмент сравнивает срез мира до и после
  вызова и возвращает `CHANGES` и `JOURNAL`. Агенту не нужен второй `observe`, чтобы узнать, что произошло.
- **Ошибки подсказывают, как исправить вызов:**
  ``bad arguments for teleport: missing field `x`. Expected: teleport(id: integer|string, x: number, y?: number, z: number)``,
  `e57 no longer exists (Skeleton despawned at tick 812); live: …`,
  ``unknown component `Mana`; this entity has: Health, …``.
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

## Транспорты

**BRP `ai/*`.** JSON-RPC 2.0, `POST http://127.0.0.1:15702`:

| метод | params |
|---|---|
| `ai/act` | `{agent?, tool, args?}` |
| `ai/observe` | `{agent?, since_seq?}` |
| `ai/step` | `{agent?, ticks?, until?}` |
| `ai/journal` | `{agent?, since_seq?, last?}` |
| `ai/tools` | `{agent?, format?: "full" \| "llm"}` |
| `ai/session` | `{agent, scope?, subject?, clear_subject?}` |
| `ai/status` | — |

Стандартные методы BRP (`world.query`, `world.get_components`, …) тоже доступны: `shards raw …`.
BRP даёт полный доступ к миру, поэтому сервер слушает только 127.0.0.1 и включается feature `remote`.
Релизная сборка без BRP:

```bash
cargo build --release -p sandbox --no-default-features --features render
```

**CLI `shards`:** observe, step, act, tools, inspect, nearby, journal, pause, resume, speed,
status, session, raw, wait. Аргументы `act` передаются JSON-объектом или парами `key=value`.
Флаги `--port`/`SHARDS_PORT`, `--agent`/`SHARDS_AGENT`, `--json`.

**Параллельные headless-инстансы.** У каждого свой порт:

```bash
cargo sandbox --headless --paused --seed 1 --port 15711 &
cargo sandbox --headless --paused --seed 2 --port 15712 &
SHARDS_PORT=15711 cargo shards step 600 --until named:player_dead
```

**В процессе:** `ai_core::call_tool` или `AiLink::call(agent, tool, args)` из любого потока.
Запрос обрабатывается в `Last` ближайшего кадра, ответ приходит через канал.

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

## Подключение LLM

`shards tools --llm` (или `ai/tools {"format": "llm"}`) отдаёт `[{name, description, input_schema}]`.
Это ровно формат `tools` в Claude Messages API, его можно передавать как есть. Цикл такой:
`observe` → модель → `tool_use` → `ai/act` → результат (с CHANGES и JOURNAL) → модель.

Ограничения из дизайн-документа:

- в релизе API-ключ живёт только на вашем сервере, не в клиенте;
- без API игра остаётся полностью играбельной;
- у агента в игре scope `Player`: он ходит только через `Intent`, как человек.

## Тесты

```bash
source ~/.local/share/rustengine-sysdeps/env.sh   # пока не установлены apt-пакеты
cargo test --workspace
```

Всего 23 теста:

- 12 юнит-тестов `ai_core`: id, журнал, RNG, компас и LOS, схемы, шаги симуляции;
- 2 теста парсера условий в `shards`;
- 9 сценариев `crates/sandbox/tests/scenarios.rs`:
  - детерминизм и разные seed;
  - одинаковый результат при пошаговом и цельном прогоне;
  - полный порядок систем и контрольный тест к этой проверке;
  - фаза 2 босса через `set` + `step until named:boss_phase_2`;
  - подсказывающие ошибки;
  - права scope;
  - паника в инструменте.

## Статус по плану внедрения

| # | шаг | статус |
|---|---|---|
| 1 | Intent + FixedUpdate + seed | ✅ |
| 2 | AiId, текстовый срез, журнал | ✅ |
| 3 | pause / step / BRP / CLI | ✅ |
| 4 | скриншоты с метками | ◐ метки готовы (оверлей и подсветка рисуются поверх сцены); захват `ai/shot` следующий |
| 5 | реестр инструментов, схемы, scope | ✅ |
| 6 | внутриигровой агент | ◐ транспорт `AiLink` готов; LLM-цикл (tokio + Claude API) следующий |
| 7 | реплеи / снапшоты / параллельный headless | ◐ параллельные headless-инстансы работают; реплеи и снапшоты впереди |

## Дальше

1. `ai/shot` и `shards shot --marks`: Bevy `Screenshot` → PNG. Оверлей включается за кадр до съёмки,
   CLI ждёт файл. Отдельный инструмент нужен потому, что окно без фокуса композитор обновляет с задержкой.
2. Внутриигровой агент: поток tokio, два цикла (LLM ставит намерения; Rust-логика исполняет их
   каждый тик), всё через `AiLink`, ключ API на прокси-сервере.
3. Реплеи: запись `Intent` по тикам вместе с seed; снапшоты через `DynamicScene`, чтобы
   сохраняться, откатываться и ветвить сценарии.
4. Дерево UI в `observe`, `path_exists` поверх навмеша.
5. LOS для воксельного мира: подменить `AiLineOfSight` на DDA по чанкам.
6. Hot reload (`--features dynamic`, subsecond).

## Соглашения и сборка

- Код и комментарии на английском, документация на русском.
- Версия Bevy закреплена (`0.20.0`). Обновлять её стоит осознанно, отдельным шагом:
  API Bevy меняется в каждом релизе.
- `.cargo/config.toml` ставит `jobs = 4`, потому что на этой машине мало RAM. Если памяти больше, увеличьте.
  Зависимости собираются с `opt-level = 3`, свой код с `opt-level = 1`.
- Features `sandbox`: `render` и `remote` (по умолчанию), `audio`, `gamepad`, `wayland`, `dynamic`.
  Без `render` игра собирается только для `--headless`.
