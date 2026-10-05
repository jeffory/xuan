"""Selection masks with the standard library only.

``luminance`` is a placeholder for a machine-learning segmentation model.
Anything with the same shape can replace it: a backend is a callable

    backend(width, height, rgba, options, progress, check) -> bytes

``rgba`` is 8-bit straight-alpha RGBA bytes and the result is
``width * height`` grey bytes: 255 selected, 0 not, values between partly
selected. ``options`` holds the action's inputs (``threshold`` and
``softness`` as fractions). ``progress(fraction)`` reports 0..1 and
``check()`` raises when the user cancelled. ``load_backend`` finds extra
backends as ``backend_<name>.py`` files next to this one; each defines
``segment`` with the signature above. See README.md.
"""
import importlib.util
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))


def luminance_backend(width, height, rgba, options, progress, check):
    """Select pixels by their brightness, with a soft ramp of ``softness``
    centred on ``threshold``. Transparent pixels are never selected."""
    threshold = float(options.get("threshold", 0.6))
    softness = float(options.get("softness", 0.1))
    low = threshold - softness / 2
    out = bytearray(width * height)
    for y in range(height):
        if y % 16 == 0:
            check()
        row = y * width
        for x in range(width):
            i = (row + x) * 4
            r, g, b, a = rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]
            brightness = (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255 * (a / 255)
            if softness <= 0:
                selected = 1.0 if brightness >= threshold else 0.0
            else:
                selected = min(max((brightness - low) / softness, 0.0), 1.0)
            out[row + x] = round(selected * 255)
        if y % 16 == 15 or y == height - 1:
            progress((y + 1) / height)
    return bytes(out)


BUILTIN = {"luminance": luminance_backend}


def load_backend(name, folder=HERE):
    """The named backend's function. ``luminance`` is built in; other names
    load ``backend_<name>.py`` from ``folder`` and use its ``segment``
    function."""
    if name in BUILTIN:
        return BUILTIN[name]
    if not re.fullmatch(r"[a-z0-9_]+", name or ""):
        raise ValueError(f"Invalid backend name {name!r}; use lowercase letters, digits and _")
    path = os.path.join(folder, f"backend_{name}.py")
    if not os.path.isfile(path):
        raise FileNotFoundError(f"Backend {name!r} not found: expected {path}")
    spec = importlib.util.spec_from_file_location(f"backend_{name}", path)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.segment
