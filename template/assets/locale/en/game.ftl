# The HUD and menus: assets/content/screens.screen.ron names these as `@id`. src/screens.rs
# formats `hud-trophies` with the number of grunts the hero defeated.

hud-health = HP
hud-stamina = Stamina
hud-trophies = { $count ->
    [one] { $count } trophy
   *[other] { $count } trophies
}
hud-hint = [Esc] Menu

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

settings-controls = Controls
settings-look = Look speed
settings-invert = Invert look
settings-hold = Hold to toggle

settings-language = Language

quit-title = Save and quit?
quit-no = Not yet
quit-yes = Save and quit
