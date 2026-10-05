#!/usr/bin/env python3
"""Local Upscale for Xuan: enlarge an image on this machine, standard library only.

The default backend is a Lanczos resampler with an optional unsharp mask. It
is a placeholder for a super-resolution model (see README.md). The plugin
declares no network access: the source image is read from the job folder and
the result is written back there."""
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "sdk", "python"))
sys.path.insert(0, HERE)

from xuan_plugin import INVALID_PARAMS, NeedsSetup, Plugin, RpcError, decode_png, encode_png  # noqa: E402

import upscale  # noqa: E402

plugin = Plugin()
MAX_INPUT_PIXELS = 4_000_000


def check_sizes(width, height, scale, limit_megapixels):
    if width * height > MAX_INPUT_PIXELS:
        raise RpcError(
            INVALID_PARAMS,
            f"The source is {width}x{height}; this placeholder upscaler takes at most "
            f"{MAX_INPUT_PIXELS // 1_000_000} megapixels. Select a smaller area.",
        )
    if width * height * scale * scale > limit_megapixels * 1_000_000:
        raise RpcError(
            INVALID_PARAMS,
            f"The result would be {width * scale}x{height * scale}, over the "
            f"{limit_megapixels} megapixel limit in the plugin settings.",
        )


def number(value, default, low, high, kind=float):
    try:
        value = kind(value)
    except (TypeError, ValueError):
        value = default
    return min(max(value, low), high)


def run_upscale(job, png):
    """Decode, enlarge and return ``(width, height, scale, rgba)``."""
    width, height, rgba = decode_png(png)
    scale = number(job.inputs.get("scale", 2), 2, 2, 4, int)
    sharpen = number(job.inputs.get("sharpen", 0.0), 0.0, 0.0, 2.0)
    limit = number(plugin.settings.get("max_output_megapixels") or 16, 16, 1, 100, int)
    check_sizes(width, height, scale, limit)
    name = str(plugin.settings.get("backend") or "lanczos")
    try:
        backend = upscale.load_backend(name)
    except (ValueError, FileNotFoundError) as error:
        raise NeedsSetup(str(error))
    extra = {"sharpen": sharpen} if backend is upscale.lanczos_backend else {}
    job.progress(0.0, f"Upscaling {width}x{height} by {scale}x with {name}")
    out_w, out_h, out = backend(width, height, rgba, scale, job.progress, job.check_cancelled, **extra)
    return out_w, out_h, scale, out


@plugin.action("upscale")
def upscale_action(job):
    if not job.source_path:
        raise RpcError(INVALID_PARAMS, "Open an image first.")
    with open(job.source_path, "rb") as handle:
        png = handle.read()
    out_w, out_h, scale, rgba = run_upscale(job, png)
    path = job.path("upscaled.png")
    with open(path, "wb") as handle:
        handle.write(encode_png(out_w, out_h, rgba))
    return [job.new_document(path, f"Upscaled {scale}x"), job.text(f"Upscaled to {out_w}x{out_h}")]


@plugin.estimate("upscale")
def estimate(job):
    return {"cost": "Free, on this computer", "seconds": 5}


if __name__ == "__main__":
    plugin.run()
