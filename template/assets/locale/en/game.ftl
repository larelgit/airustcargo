# The HUD and menus: assets/content/screens.screen.ron names these as `@id`. src/screens.rs
# formats `hud-trophies` with the number of grunts the hero defeated, and shows `menu-no-save`
# under the main menu's Continue while there is no save.

hud-health = HP
hud-stamina = Stamina
hud-trophies = { $count ->
    [one] { $count } trophy
   *[other] { $count } trophies
}
hud-hint = [Esc] Menu

menu-title = My game
menu-art = A lone tower on a hill under a dusk sky
menu-new-game = New game
menu-continue = Continue
menu-no-save = No saved game yet: start a new one
menu-settings = Settings
menu-quit = Quit

pause-title = Paused
pause-resume = Resume
pause-settings = Settings
pause-quit = Save and quit

settings-title = Settings
settings-back = Back

settings-display = Display
settings-fullscreen = Fullscreen
settings-text-size = Text size
settings-flashes = Flashes
flashes-full = All
flashes-reduced = Reduced
flashes-off = None
settings-shake = Camera shake

settings-sound = Sound
settings-volume = Volume
settings-music = Music
settings-sfx = Effects
settings-ui = Interface

settings-controls = Controls
settings-look = Look speed
settings-invert = Invert look
settings-hold = Hold to toggle

# The rows that change a control: an action's name, what a row says while it waits for the new
# input, and the phrase for an input another action has (`$input`: F, Mouse Right, Pad South;
# `$action`: that action's row label).
controls-keys = Keys and buttons
controls-jump = Jump
controls-dodge = Dodge
controls-light = Light attack
controls-heavy = Heavy attack
controls-block = Block
controls-parry = Parry
controls-interact = Interact
controls-use-item = Use item
controls-lock-on = Lock on
controls-sprint = Sprint
controls-glide = Glide
controls-press = Press a key or button… (Esc cancels)
controls-taken = { $input } is already used by { $action }

settings-language = Language

quit-title = Save and quit?
quit-no = Not yet
quit-yes = Save and quit
