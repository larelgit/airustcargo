# AGENTS.md

Инструкции для агентов-кодеров (Claude Code, Codex и другие), которые работают в этом репозитории.

## Главное правило

В движке нет LLM и не должно быть. Не добавляйте встроенных или локальных агентов, моделей,
вызовов API моделей, API-ключей, циклов «модель → инструмент» внутри игры. Игра — среда, которой
управляют снаружи: вы, через CLI `shards`. Всё, что нужно агенту, добавляется как инструмент
(`add_ai_tool` / `add_ai_query`) или как команда `shards`, а не как код, который думает за агента.

## Сборка

- Системные пакеты: `pkg-config libasound2-dev libudev-dev`.
- `cargo build --workspace`, `cargo test --workspace`. Первая сборка Bevy долгая;
  `.cargo/config.toml` ограничивает её четырьмя потоками.

## Как смотреть на игру и действовать в ней

```bash
cargo sandbox --headless --paused &     # без окна, на паузе: время идёт только по step
cargo shards wait
cargo shards observe                    # срез мира текстом + журнал с прошлого раза
cargo shards tools                      # инструменты и сигнатуры (--json: схемы аргументов)
cargo shards act move_to target=e6      # действие → CHANGES и JOURNAL
cargo shards step 120 --until journal:died
cargo shards inspect e6                 # все компоненты сущности
cargo shards save-replay                # сессия с тика 0 → replays/tick<T>.json
cargo shards scenario FILE...           # файл сценария: команды, ожидания, инварианты (README, «Сценарии»)
```

- `shards` запускайте из папки игры или ниже: там лежит токен `.shards/<port>.token`, без него игра
  отвечает `unauthorized`. Из другого места передайте его в `SHARDS_TOKEN`.
- Останавливайте игру `kill` (или Ctrl+C), не `kill -9`: так она удаляет токен и `--port-file`.
- Файлы игра пишет только к себе: реплеи в `replays/`, снимки в `shots/` (`save-replay replays/x.json`,
  `shot -o shots/x.png`). Абсолютные пути и `..` не принимаются.
- Числа и позиции берите из `observe` и `inspect`, не из картинок. `shards shot` (нужно окно) —
  для визуальных багов: PNG с метками `eN` и легенда, где на картинке какой id.
- Ошибки инструментов объясняют, как исправить вызов. Читайте их, а не повторяйте вызов вслепую.
- `dodge` и `attack` без цели отвечают по правилам игры сразу: `ok`, `ok: queued, dodges in 0.12s`
  (вход ждёт в буфере до 0.2 с) или ошибка с причиной и временем (`not enough stamina: 10/25 (ready
  in 1.0s)`, `busy: windup Sword 0.80s …`). На ошибку сделайте `step` на названное время и повторите.
  Входы мимо инструментов, у которых вышло время, видны в журнале: `e5 dodge rejected: …`.
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

- Код и комментарии на английском, документация (README, этот файл) на русском.
- Bevy закреплён на 0.20.0; не обновляйте его попутно с другой задачей.
- Весь геймплей — в `FixedUpdate`, в одном из `SimSet`; случайность только через `SimRng`.
  Тест `gameplay_systems_have_a_total_order` ловит системы без явного порядка.
- Персонажами управляют только через `Intent`: клавиатура, инструменты агента и реплей пишут в него.
  Новый компонент-вход регистрируйте через `replay_input::<C>()`, иначе реплеи разойдутся.
- Проверки реплея (`shards-replay/2`) сверяют игровые компоненты сущностей с `AiVisible`, а не текст
  `observe`. Косметику и кэши исключайте `replay_ignore::<C>()`, состояние вне таких сущностей
  (ресурсы, снаряды без id) добавляйте `add_replay_check(name, |w| …)`.
- Новый инструмент — структура аргументов с doc-комментариями (они становятся описанием схемы)
  и Bevy-система `In<Call<Args>> -> ToolResult`. Вид: `query` читает, `action` меняет мир
  (журнал, дифф, реплей), `control` управляет временем и сессией.
- ИИ видит только сущности с `AiVisible` (его даёт `AiKind`). Что о них сказано, решают
  describe-функции (`register_ai_component`, `register_ai_describer`).
- `AiId` назначает движок. Не пишите его на месте (`get_mut`, `set`, BRP `world.mutate_components`):
  индекс «id → сущность» ведут хуки `on_insert` и `on_discard`. Вставка нового `AiId` перенумеровывает
  сущность, а старый id получает надгробие `renumbered to eN`.

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

- `cargo test --workspace`: юнит-тесты и сценарии в `crates/sandbox/tests/`.
- Поведение, которое можно описать командами `shards`, оформляйте файлом сценария в `<crate>/scenarios/`
  (`*.scenario`, формат в README, «Сценарии»). `tests/scenario_files.rs` песочницы гоняет каждый файл
  в процессе, `shards scenario` — против живой игры; нужен файл, который идёт и на свежей игре, и после
  других: начинайте его с `heal`/`teleport`/`set`, возвращающих мир к исходному.
- Изменение геймплея проверяйте реплеем: `cargo sandbox --replay FILE --verify` называет первый тик,
  где игра разошлась с записью (код выхода 1). Если расхождение ожидаемое, запишите реплей заново.
