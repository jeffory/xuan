#!/usr/bin/env python3
"""Select Bright Areas for Xuan: make a selection on this machine, standard library only.

The default backend selects pixels by brightness. It is a placeholder for a
segmentation model (see README.md). The plugin declares no network access
and no document edits: it returns a mask, which Xuan proposes as the new
selection."""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "sdk", "python"))
sys.path.insert(0, HERE)

from xuan_plugin import INVALID_PARAMS, Job, NeedsSetup, Plugin, RpcError, decode_png, encode_gray_png  # noqa: E402

import segment  # noqa: E402

plugin = Plugin()
MAX_INPUT_PIXELS = 4_000_000


def number(value, default, low, high):
    try:
        value = float(value)
    except (TypeError, ValueError):
        value = default
    if value != value:  # NaN
        value = default
    return min(max(value, low), high)


def run_segment(job, png):
    """Decode the source and return ``(width, height, grey mask bytes)``."""
    width, height, rgba = decode_png(png)
    if width * height > MAX_INPUT_PIXELS:
        raise RpcError(INVALID_PARAMS, f"The source is {width}x{height}; this placeholder takes at most 4 megapixels.")
    options = {
        "threshold": number(job.inputs.get("threshold", 60), 60, 0, 100) / 100,
        "softness": number(job.inputs.get("softness", 10), 10, 0, 50) / 100,
    }
    name = str(plugin.settings.get("backend") or "luminance")
    try:
        backend = segment.load_backend(name)
    except (ValueError, FileNotFoundError) as error:
        raise NeedsSetup(str(error))
    job.progress(0.0, f"Finding bright areas in {width}x{height} with {name}")
    mask = backend(width, height, rgba, options, job.progress, job.check_cancelled)
    if len(mask) != width * height:
        raise RpcError(INVALID_PARAMS, f"The {name} backend returned a mask of the wrong size")
    return width, height, bytes(mask)


@plugin.action("select-bright")
def select_bright(job):
    if not job.source_path:
        raise RpcError(INVALID_PARAMS, "Open an image first.")
    with open(job.source_path, "rb") as handle:
        png = handle.read()
    width, height, mask = run_segment(job, png)
    path = job.path("mask.png")
    with open(path, "wb") as handle:
        handle.write(encode_gray_png(width, height, mask))
    mode = str(job.inputs.get("mode") or "replace")
    if mode not in Job.MASK_MODES:
        mode = "replace"
    share = round(100 * sum(mask) / (255 * max(len(mask), 1)))
    # The source may have been sent downscaled: fit_source lays the mask over
    # all of it, and Xuan scales it back up to the document smoothly.
    return [job.mask(path, mode=mode, fit_source=True), job.text(f"Selected about {share}% of the image")]


@plugin.estimate("select-bright")
def estimate(job):
    return {"cost": "Free, on this computer", "seconds": 2}


if __name__ == "__main__":
    plugin.run()
