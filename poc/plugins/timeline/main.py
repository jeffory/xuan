#!/usr/bin/env python3
"""Proof of concept: an animation timeline in a sidebar pane.

Every top-level pixel layer is a frame, bottom to top, as in GIMP. A layer
named "Walk 2 (120ms)" holds its frame for 120 ms. The pane lists the
frames, scrubs by showing one frame at a time, plays the frames back in the
pane, and can show the previous frame as an onion skin. See README.md for
what this proves and where the SDK falls short."""
import os
import re
import sys
import threading
import time

sys.path.append(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "sdk", "python"))

from xuan_plugin import Plugin, ui  # noqa: E402

plugin = Plugin()
state = {"current": 0, "playing": False, "fps": 12.0, "onion": False, "sent": 0, "elapsed": 0.0}
lock = threading.Lock()
DELAY = re.compile(r"\((\d+)\s*ms\)\s*$")


def frames(document):
    layers = (document or {}).get("layers", [])
    return [l for l in layers if l.get("parent") is None and l.get("kind") != "group"]


def delay_ms(layer):
    match = DELAY.search(layer.get("name", ""))
    return int(match.group(1)) if match else None


def show_frame(document, index):
    """Scrub: make one frame visible. Each call is an undo step, because
    visibility is document state and document/edit is the only way in."""
    layers = frames(document)
    if not layers:
        return
    edits = []
    for i, layer in enumerate(layers):
        on = i == index or (state["onion"] and i == index - 1)
        edits.append({"op": "set", "layer": layer["id"], "visible": on,
                      "opacity": 0.3 if state["onion"] and i == index - 1 else 1.0})
    plugin.host.edit("Show frame %d" % (index + 1), edits)


def build(preview=None):
    document = plugin.host.document()
    layers = frames(document)
    if not layers:
        return ui.column(ui.label("Add layers: each top-level layer is one frame.", muted=True))
    current = min(state["current"], len(layers) - 1)
    items = [ui.item(l["id"], "%d  %s" % (i + 1, l["name"]),
                     detail="%d ms" % (delay_ms(l) or round(1000 / state["fps"])))
             for i, l in enumerate(layers)]
    children = []
    if preview:
        children.append(ui.image(preview, width=240))
    children += [
        ui.row(ui.button("prev", "◀"),
               ui.button("play", "Stop" if state["playing"] else "Play", primary=True),
               ui.button("next", "▶")),
        ui.slider("fps", state["fps"], 1, 30, label="Frames per second"),
        ui.checkbox("onion", "Onion skin (previous frame at 30%)", state["onion"]),
        ui.listing("frames", items, selected=layers[current]["id"]),
        ui.label("Sent %d preview frames in %.1f s" % (state["sent"], state["elapsed"]), muted=True, small=True),
    ]
    return ui.column(*children)


def play():
    """Play back in the pane: export every frame once, then swap the image."""
    document = plugin.host.document()
    layers = frames(document)
    paths = [plugin.host.export_layer(l["id"], "pixels", max_side=256)["path"] for l in layers]
    started, sent, index = time.monotonic(), 0, state["current"]
    while state["playing"] and paths:
        index = (index + 1) % len(paths)
        with lock:
            state["current"] = index
        plugin.update_pane("timeline", build(preview=paths[index]))
        sent += 1
        state["sent"], state["elapsed"] = sent, time.monotonic() - started
        wait = (delay_ms(layers[index]) or 1000 / state["fps"]) / 1000
        time.sleep(wait)


@plugin.pane("timeline")
def pane(request):
    document = plugin.host.document()
    count = len(frames(document))
    if request.widget == "play":
        state["playing"] = not state["playing"]
        if state["playing"]:
            threading.Thread(target=play, daemon=True).start()
    elif request.widget == "fps":
        state["fps"] = float(request.value)
    elif request.widget == "onion":
        state["onion"] = bool(request.value)
        show_frame(document, state["current"])
    elif request.widget in ("prev", "next") and count:
        step = -1 if request.widget == "prev" else 1
        state["current"] = (state["current"] + step) % count
        show_frame(document, state["current"])
    elif request.widget == "frames":
        ids = [l["id"] for l in frames(document)]
        if request.value in ids:
            state["current"] = ids.index(request.value)
            show_frame(document, state["current"])
    return build()


if __name__ == "__main__":
    plugin.run()
