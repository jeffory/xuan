#!/usr/bin/env python3
"""Extend Edges for Xuan: outpainting on this machine, standard library only.

Xuan sends the flattened image already padded with new, transparent canvas
(``source.extend``) and a mask of that new area. The default backend fills
it by mirroring or repeating the edges; it is a placeholder for a generative
outpainting model (see README.md). The result asks Xuan to extend the canvas
by the same amounts and places the filled image over the whole new canvas,
masked to the new area, so the original pixels stay untouched."""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "sdk", "python"))
sys.path.insert(0, HERE)

from xuan_plugin import INVALID_PARAMS, NeedsSetup, Plugin, RpcError, decode_png, encode_png  # noqa: E402

import fill  # noqa: E402

plugin = Plugin()
MAX_INPUT_PIXELS = 4_500_000


def read_png(path):
    with open(path, "rb") as handle:
        return decode_png(handle.read())


@plugin.action("outpaint")
def outpaint(job):
    if not job.source_path:
        raise RpcError(INVALID_PARAMS, "Open an image first.")
    extension = job.extension
    if not extension or not job.extend_mask_path:
        raise RpcError(INVALID_PARAMS, "Xuan did not extend the source; check source.extend in plugin.toml.")
    width, height, rgba = read_png(job.source_path)
    if width * height > MAX_INPUT_PIXELS:
        raise RpcError(INVALID_PARAMS, f"The source is {width}x{height}; this placeholder takes at most 4.5 megapixels.")
    mask_width, mask_height, mask_rgba = read_png(job.extend_mask_path)
    if (mask_width, mask_height) != (width, height):
        raise RpcError(INVALID_PARAMS, "The new-area mask does not match the source")
    mask = mask_rgba[::4]
    options = {"fill": job.inputs.get("fill") if job.inputs.get("fill") in fill.FILLS else "mirror"}
    name = str(plugin.settings.get("backend") or "edges")
    try:
        backend = fill.load_backend(name)
    except (ValueError, FileNotFoundError) as error:
        raise NeedsSetup(str(error))
    job.progress(0.0, f"Filling the new edges of {width}x{height} with {name}")
    try:
        result = backend(width, height, rgba, mask, options, job.progress, job.check_cancelled)
    except ValueError as error:
        raise RpcError(INVALID_PARAMS, str(error))
    if len(result) != width * height * 4:
        raise RpcError(INVALID_PARAMS, f"The {name} backend returned an image of the wrong size")
    path = job.path("outpainted.png")
    with open(path, "wb") as handle:
        handle.write(encode_png(width, height, bytes(result)))
    document = job.document or {}
    new_width = int(document.get("width", 0)) + extension["left"] + extension["right"]
    new_height = int(document.get("height", 0)) + extension["top"] + extension["bottom"]
    return [
        # Grow the canvas by what the source was extended by. The image is
        # then fitted over the extended source, which is the new canvas, and
        # masked to the new area so the original pixels stay as they were.
        job.edit([job.extend_canvas(**extension)]),
        job.image(path, name="Outpainted edges", mask=job.extend_mask_path, fit_source=True),
        job.text(f"Extended the canvas to {new_width}x{new_height}"),
    ]


@plugin.estimate("outpaint")
def estimate(job):
    return {"cost": "Free, on this computer", "seconds": 2}


if __name__ == "__main__":
    plugin.run()
