from pathlib import Path

app = Path(defines["app"])
files = [str(app)]
symlinks = {"Applications": "/Applications"}

window_rect = ((160, 160), (640, 240))
grid_spacing = 72
icon_size = 96
text_size = 13
icon_locations = {app.name: (138, 116), "Applications": (502, 116)}
background = "builtin-arrow"
