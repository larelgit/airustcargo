# Меню арены по-русски. Чего здесь нет, берётся из английского (fallback: "en").

pause-title = Пауза
pause-resume = Продолжить
pause-settings = Настройки
pause-quit = Выйти

settings-title = Настройки
settings-back = Назад

settings-graphics = Графика
settings-fullscreen = Во весь экран
settings-vsync = Вертикальная синхронизация
settings-render-scale = Масштаб отрисовки
settings-shadows = Тени

settings-audio = Звук
settings-master = Общая громкость
settings-music = Музыка
settings-sfx = Эффекты
settings-voice = Голос
settings-ui = Интерфейс

settings-controls = Управление
settings-look-speed = Скорость обзора
settings-aim-speed = Скорость прицеливания
settings-invert-y = Инверсия обзора
settings-hold = Удержание как переключение

controls-keys = Клавиши и кнопки
controls-jump = Прыжок
controls-dodge = Уклонение
controls-light = Лёгкая атака
controls-heavy = Тяжёлая атака
controls-block = Блок
controls-parry = Парирование
controls-interact = Взаимодействие
controls-use-item = Использовать предмет
controls-lock-on = Захват цели
controls-sprint = Бег
controls-glide = Планирование
controls-press = Нажмите клавишу или кнопку… (Esc — отмена)
controls-taken = { $input } уже используется для «{ $action }»

settings-accessibility = Доступность
settings-text-size = Размер текста
settings-flashes = Вспышки
settings-shake = Тряска камеры
settings-blur = Размытие движения
settings-hit-stop = Задержка при ударе
# settings-fov is left to English: the check of content still passes, and the journal says once
# that Russian lacks it.

settings-language = Язык

quit-title = Покинуть арену?
quit-no = Нет
quit-yes = Выйти

leave = Покинуть { $place }?
place-arena = арену

defeated = { $count ->
    [one] Повержен { $count } враг
    [few] Повержено { $count } врага
   *[many] Повержено { $count } врагов
}
