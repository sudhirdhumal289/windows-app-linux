#!/usr/bin/env python3
"""Renders the app icon PNGs in resources/icons from assets/<app-id>.svg.

Run after editing the SVG. Needs PyGObject with librsvg (gir1.2-rsvg-2.0) and pycairo.
"""

import pathlib

import cairo
import gi

gi.require_version("Rsvg", "2.0")
from gi.repository import Rsvg  # noqa: E402

APP_ID = "com.microsoft.windowsapp.linux"
SIZES = (16, 24, 32, 48, 64, 128, 256, 512)

root = pathlib.Path(__file__).resolve().parent.parent
handle = Rsvg.Handle.new_from_file(str(root / "assets" / f"{APP_ID}.svg"))

for size in SIZES:
    surface = cairo.ImageSurface(cairo.FORMAT_ARGB32, size, size)
    viewport = Rsvg.Rectangle()
    viewport.x = viewport.y = 0
    viewport.width = viewport.height = size
    handle.render_document(cairo.Context(surface), viewport)
    out = root / "resources" / "icons" / f"{size}x{size}" / "apps" / f"{APP_ID}.png"
    out.parent.mkdir(parents=True, exist_ok=True)
    surface.write_to_png(str(out))
    print(out.relative_to(root))
