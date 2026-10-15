# The arena's menus (content/ui/menus.screen.ron names these as `@id`).

pause-title = Paused
pause-resume = Resume
pause-settings = Settings
pause-quit = Quit

settings-title = Settings
settings-back = Back

settings-graphics = Graphics
settings-fullscreen = Fullscreen
settings-vsync = VSync
settings-render-scale = Render scale
settings-shadows = Shadows

settings-audio = Audio
settings-master = Master
settings-music = Music
settings-sfx = Effects
settings-voice = Voice

settings-controls = Controls
settings-look-speed = Look speed
settings-invert-y = Invert look
settings-hold = Hold to toggle

settings-accessibility = Accessibility
settings-text-size = Text size
settings-flashes = Flashes
settings-shake = Camera shake
settings-blur = Motion blur
settings-fov = Field of view

settings-language = Language

quit-title = Quit the arena?
quit-no = No
quit-yes = Quit

# Phrases an agent builds with `text`. `$place` is a whole phrase of its own
# (`text id=leave args={"place": "@place-arena"}`), never a word glued in code; a number picks the
# plural form (`text id=defeated args={"count": 3}`).
leave = Leave { $place }?
place-arena = the arena

defeated = { $count ->
    [one] { $count } enemy defeated
   *[other] { $count } enemies defeated
}
