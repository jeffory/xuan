#!/usr/bin/env python3
"""Histogram pane for Xuan, using only the standard library.

Draws the red, green and blue histograms of the flattened document into a
small PNG and shows it in a sidebar pane. The pane asks to be re-rendered
after every edit (``refresh = "document"`` in plugin.toml)."""
import math
import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "sdk", "python"))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from xuan_plugin import Plugin, decode_png, encode_png, png_data_url, ui  # noqa: E402

plugin = Plugin()
WIDTH, HEIGHT = 256, 96
state = {"channel": "rgb", "log": False}


def histogram(rgba: bytes):
    counts = [[0] * 256 for _ in range(3)]
    for i in range(0, len(rgba), 4):
        if rgba[i + 3] == 0:
            continue
        counts[0][rgba[i]] += 1
        counts[1][rgba[i + 1]] += 1
        counts[2][rgba[i + 2]] += 1
    return counts


def render(counts, log_scale: bool, channel: str) -> bytes:
    colors = [(235, 90, 90), (90, 200, 110), (90, 140, 255)]
    shown = [0, 1, 2] if channel == "rgb" else ["r", "g", "b"].index(channel)
    shown = shown if isinstance(shown, list) else [shown]
    scale = (lambda v: math.log1p(v)) if log_scale else (lambda v: float(v))
    peak = max(scale(counts[c][v]) for c in shown for v in range(256)) or 1.0
    pixels = bytearray(b"\x1d\x1d\x1d\xff" * (WIDTH * HEIGHT))
    for c in shown:
        r, g, b = colors[c]
        for x in range(256):
            bar = int(round(scale(counts[c][x]) / peak * (HEIGHT - 4)))
            for y in range(HEIGHT - bar, HEIGHT):
                offset = (y * WIDTH + x) * 4
                # Additive blending so overlapping channels read as mixes.
                pixels[offset] = min(255, pixels[offset] + r // 2)
                pixels[offset + 1] = min(255, pixels[offset + 1] + g // 2)
                pixels[offset + 2] = min(255, pixels[offset + 2] + b // 2)
    return encode_png(WIDTH, HEIGHT, bytes(pixels))


@plugin.on_initialize
def initialize():
    state["log"] = bool(plugin.settings.get("log_scale", False))


@plugin.on_settings
def settings_changed():
    state["log"] = bool(plugin.settings.get("log_scale", False))
    plugin.update_pane("histogram", build())


@plugin.pane("histogram")
def pane(request):
    if request.widget == "channel":
        state["channel"] = request.value
    elif request.widget == "log":
        state["log"] = bool(request.value)
    return build()


def build():
    document = plugin.host.document()
    if not document:
        return ui.column(ui.label("Open a document to see its histogram.", muted=True))
    export = plugin.host.export_document(max_side=256)
    with open(export["path"], "rb") as handle:
        _, _, rgba = decode_png(handle.read())
    counts = histogram(rgba)
    total = sum(counts[0])
    mean = [sum(v * n for v, n in enumerate(counts[c])) / max(total, 1) for c in range(3)]
    return ui.column(
        ui.image(png_data_url(render(counts, state["log"], state["channel"])), height=96),
        ui.row(
            ui.select("channel", state["channel"], [("rgb", "RGB"), ("r", "Red"), ("g", "Green"), ("b", "Blue")]),
            ui.checkbox("log", "Log", state["log"]),
        ),
        ui.label("Mean  R %.0f  G %.0f  B %.0f" % tuple(mean), small=True, muted=True),
        ui.label("%d × %d px, %d opaque pixels sampled" % (export["width"], export["height"], total), small=True, muted=True),
    )


if __name__ == "__main__":
    plugin.run()
