# hlactivate

A tiny joke widget for Wayland desktops: the famous "Activate Windows" watermark, but for Hyprland.

## Installation

You need a Rust toolchain (`cargo`) and `make`.

```sh
git clone https://github.com/cl1ckname/hlactivate
cd hlactivate
make install
```

This builds a release binary and installs it to `~/.local/bin/hlactivate`. Use `make install PREFIX=/some/prefix` to install elsewhere and `make uninstall` to remove it.

To start the widget with Hyprland, add to `hyprland.conf`:

```
exec-once = ~/.local/bin/hlactivate
```

or

```lua
hl.exec_cmd("hlactivate")
```

if you are using `hyprland.lua`.

## Configuration

The default config is embedded into the binary. To override it, create `$XDG_CONFIG_HOME/hlactivate.toml` (usually `~/.config/hlactivate.toml`). You only need the keys you want to change, because missing keys fall back to the defaults. Reload the config without restarting:

```sh
pkill -USR1 hlactivate
```

Default config:

```toml
# Layer: "background" | "bottom" | "top" | "overlay".
# "bottom" — above the wallpaper, below windows (visible only on an empty desktop).
# "overlay" — above everything, like the real Windows one.
layer = "bottom"

# Monitors to show the widget on, e.g. ["eDP-1", "HDMI-A-1"].
# Empty list — all monitors. Names: `hyprctl monitors`.
outputs = []

# Monitor edges to anchor the widget to: "top", "bottom", "left", "right".
anchor = ["bottom", "right"]

# Widget area size in logical pixels. If not set, it is computed from the
# text: the area exactly fits all lines, so the margins below are measured
# from the edge of the text.
# width = 360
# height = 90

# Margins from the edges: [top, right, bottom, left].
margin = [0, 50, 20, 0]

# Font family: "sans-serif", "serif", "monospace" or a name, e.g. "Inter".
font = "sans-serif"

# Text color: "#RRGGBB" or "#RRGGBBAA".
color = "#ffffffbc"

# Widget language: "eng" | "ru"
lang = "eng"

# Text lines. x, y — top-left corner of the line inside the area,
# size — font size; all in logical pixels.
# text — custom line text. If not set, the translation for lang is used:
# the first line is the title, the second one is the subtitle.
# Defining lines in your config replaces the whole list.
[[lines]]
size = 24
x = 0
y = 12

[[lines]]
size = 15
x = 0
y = 46
```
