#!/usr/bin/env python3
"""Proof of concept: OpenRaster import and export with only the standard library.

OpenRaster is a zip of PNG layers plus stack.xml, so the plugin needs no
image code at all: it extracts the PNGs and hands their paths to Xuan. What
Xuan's import result cannot express is reported in the layer names and the
status bar (see README.md)."""
import os
import sys
import xml.etree.ElementTree as ET
import zipfile

sys.path.append(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "..", "sdk", "python"))

from xuan_plugin import Plugin  # noqa: E402

plugin = Plugin()

# OpenRaster composite-op → Xuan blend mode (src/blend.rs names).
BLEND = {
    "svg:src-over": "Normal", "svg:multiply": "Multiply", "svg:screen": "Screen",
    "svg:overlay": "Overlay", "svg:darken": "Darken", "svg:lighten": "Lighten",
    "svg:color-dodge": "ColorDodge", "svg:color-burn": "ColorBurn",
    "svg:hard-light": "HardLight", "svg:soft-light": "SoftLight",
    "svg:difference": "Difference", "svg:exclusion": "Exclusion", "svg:hue": "Hue",
    "svg:saturation": "Saturation", "svg:color": "Color", "svg:luminosity": "Luminosity",
    "svg:plus": "LinearDodge",
}


@plugin.importer("ora")
def import_ora(path, work_dir):
    lost = {"groups": 0, "ops": set()}
    layers = []
    with zipfile.ZipFile(path) as archive:
        stack = ET.fromstring(archive.read("stack.xml"))
        width, height = int(stack.get("w", 1)), int(stack.get("h", 1))
        resolution = float(stack.get("xres", 72))

        def walk(node, prefix, x0, y0, opacity, visible):
            # stack.xml lists the top layer first; Xuan adds bottom to top.
            for child in reversed(list(node)):
                x = x0 + int(child.get("x", 0))
                y = y0 + int(child.get("y", 0))
                alpha = opacity * float(child.get("opacity", 1))
                shown = visible and child.get("visibility", "visible") != "hidden"
                name = child.get("name", "Layer")
                if child.tag == "stack":
                    # The import result has no groups: flatten the tree and
                    # keep the path in the name. Group blend modes are lost.
                    lost["groups"] += 1
                    walk(child, prefix + name + " / ", x, y, alpha, shown)
                elif child.tag == "layer":
                    src = child.get("src", "")
                    if not src.lower().endswith(".png") or ".." in src:
                        continue
                    target = os.path.join(work_dir, "layer-%d.png" % len(layers))
                    with open(target, "wb") as out:
                        out.write(archive.read(src))
                    op = child.get("composite-op", "svg:src-over")
                    if op not in BLEND:
                        lost["ops"].add(op)
                    layers.append({"name": prefix + name, "image": target, "x": x, "y": y,
                                   "opacity": alpha, "visible": shown,
                                   "blend": BLEND.get(op, "Normal")})

        # <image> holds one root <stack>, which is not a group of its own.
        walk(stack.find("stack"), "", 0, 0, 1.0, True)
    if lost["groups"] or lost["ops"]:
        note = "Flattened %d group(s)" % lost["groups"]
        if lost["ops"]:
            note += "; unsupported blend modes: " + ", ".join(sorted(lost["ops"]))
        plugin.host.status(note)
    return {"width": width, "height": height, "resolution": resolution, "layers": layers}


@plugin.exporter("ora")
def export_ora(path, image, document):
    """Export gets only the flattened image, so a layered round trip back to
    Krita needs one layer/export per layer: masks, effects, text and groups
    are not available as such."""
    with zipfile.ZipFile(path, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr(zipfile.ZipInfo("mimetype"), "image/openraster", compress_type=zipfile.ZIP_STORED)
        entries = []
        for index, layer in enumerate(document.get("layers", [])):
            if layer.get("parent") is not None or layer.get("kind") not in ("image", "text", "shape"):
                continue
            export = plugin.host.export_layer(layer["id"])
            name = "data/layer%d.png" % index
            archive.write(export["path"], name)
            entries.append('<layer name="%s" src="%s" x="%d" y="%d" opacity="%.3f" visibility="%s"/>' % (
                layer["name"].replace('"', "'"), name, round(export["x"]), round(export["y"]),
                layer.get("opacity", 1), "visible" if layer.get("visible", True) else "hidden"))
        archive.write(image, "mergedimage.png")
        xml = '<?xml version="1.0" encoding="UTF-8"?>\n<image w="%d" h="%d"><stack>%s</stack></image>' % (
            document["width"], document["height"], "".join(reversed(entries)))
        archive.writestr("stack.xml", xml)
    return None


if __name__ == "__main__":
    plugin.run()
