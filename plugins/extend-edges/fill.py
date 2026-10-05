"""Fill new canvas with the standard library only.

``edges`` is a placeholder for a generative outpainting model. Anything with
the same shape can replace it: a backend is a callable

    backend(width, height, rgba, mask, options, progress, check) -> bytes

``rgba`` is the extended source: 8-bit straight-alpha RGBA bytes, transparent
over the new canvas. ``mask`` is ``width * height`` grey bytes, 255 over the
new canvas and 0 over the old image, on the same grid. The result is
``width * height * 4`` RGBA bytes; only its pixels under the mask are shown.
``options`` holds the action's inputs (``fill`` is ``mirror`` or ``repeat``).
``progress(fraction)`` reports 0..1 and ``check()`` raises when the user
cancelled. ``load_backend`` finds extra backends as ``backend_<name>.py``
files next to this one; each defines ``outpaint`` with the signature above.
See README.md.
"""
import importlib.util
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
FILLS = ("mirror", "repeat")


def old_area(width, height, mask):
    """The bounds ``(x0, y0, x1, y1)`` of the old image (mask 0), with
    exclusive ends, or None when everything is new."""
    x0, y0, x1, y1 = width, height, 0, 0
    for y in range(height):
        row = mask[y * width:(y + 1) * width]
        if 0 not in row:
            continue
        first = row.index(0)
        last = width - 1 - row[::-1].index(0)
        x0, x1 = min(x0, first), max(x1, last + 1)
        y0, y1 = min(y0, y), y + 1
    if x1 <= x0:
        return None
    return x0, y0, x1, y1


def mirror(i, low, high):
    """Reflect ``i`` into ``low..high`` (exclusive), repeating the edge
    pixel like a mirror held against it: ... 1 0 | 0 1 2 | 2 1 ..."""
    n = high - low
    k = (i - low) % (2 * n)
    return low + (k if k < n else 2 * n - 1 - k)


def repeat(i, low, high):
    """Clamp ``i`` into ``low..high`` (exclusive): the edge pixel repeats."""
    return min(max(i, low), high - 1)


def edges_backend(width, height, rgba, mask, options, progress, check):
    """Fill every masked pixel from the old image, mirrored or repeated
    outwards from its edges."""
    box = old_area(width, height, mask)
    if box is None:
        raise ValueError("The source has no old image to extend from")
    x0, y0, x1, y1 = box
    pick = mirror if options.get("fill", "mirror") == "mirror" else repeat
    columns = [pick(x, x0, x1) for x in range(width)]
    out = bytearray(rgba)
    for y in range(height):
        if y % 16 == 0:
            check()
        row = y * width
        source_row = pick(y, y0, y1) * width
        for x in range(width):
            if mask[row + x]:
                i = (row + x) * 4
                j = (source_row + columns[x]) * 4
                out[i:i + 4] = rgba[j:j + 4]
        if y % 16 == 15 or y == height - 1:
            progress((y + 1) / height)
    return bytes(out)


BUILTIN = {"edges": edges_backend}


def load_backend(name, folder=HERE):
    """The named backend's function. ``edges`` is built in; other names load
    ``backend_<name>.py`` from ``folder`` and use its ``outpaint`` function."""
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
    return module.outpaint
