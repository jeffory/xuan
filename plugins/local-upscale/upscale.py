"""Image enlargement with the standard library only.

``lanczos`` is a placeholder for a machine-learning super-resolution model.
Anything with the same shape can replace it: a backend is a callable

    backend(width, height, rgba, scale, progress, check) -> (width, height, rgba)

``rgba`` is 8-bit straight-alpha RGBA bytes. ``progress(fraction)`` reports
0..1 and ``check()`` raises when the user cancelled. ``load_backend`` finds
extra backends as ``backend_<name>.py`` files next to this one; each defines
``upscale`` with the signature above. See README.md.
"""
import importlib.util
import math
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))
LANCZOS_A = 3


def lanczos(x):
    if x == 0.0:
        return 1.0
    if abs(x) >= LANCZOS_A:
        return 0.0
    px = math.pi * x
    return LANCZOS_A * math.sin(px) * math.sin(px / LANCZOS_A) / (px * px)


def resize_taps(source, target):
    """Per output index: the source indices to read and their weights."""
    ratio = target / source
    taps = []
    for i in range(target):
        center = (i + 0.5) / ratio - 0.5
        first = math.floor(center) - LANCZOS_A + 1
        indices, weights = [], []
        for j in range(first, first + 2 * LANCZOS_A):
            weight = lanczos(j - center)
            if weight != 0.0:
                indices.append(min(max(j, 0), source - 1))
                weights.append(weight)
        total = sum(weights)
        taps.append((indices, [w / total for w in weights]))
    return taps


def blur_taps(size):
    """A 3-tap [1 2 1] / 4 blur with clamped edges."""
    return [([max(i - 1, 0), i, min(i + 1, size - 1)], [0.25, 0.5, 0.25]) for i in range(size)]


def apply_taps(rows, taps):
    """Weighted sums of whole rows: the one primitive every pass is built from."""
    out = []
    for indices, weights in taps:
        acc = [weights[0] * v for v in rows[indices[0]]]
        for index, weight in zip(indices[1:], weights[1:]):
            acc = [a + weight * v for a, v in zip(acc, rows[index])]
        out.append(acc)
    return out


def transpose(rows):
    return [list(column) for column in zip(*rows)]


def split_planes(width, height, rgba):
    """Premultiplied float planes (lists of rows), and whether alpha is all 255."""
    alpha = rgba[3::4]
    opaque = min(alpha) == 255 if alpha else True
    planes = []
    for c in range(3):
        channel = rgba[c::4]
        if opaque:
            values = list(channel)
        else:
            values = [v * a / 255.0 for v, a in zip(channel, alpha)]
        planes.append([values[y * width : (y + 1) * width] for y in range(height)])
    if not opaque:
        planes.append([list(map(float, alpha[y * width : (y + 1) * width])) for y in range(height)])
    return planes, opaque


def join_planes(width, height, planes, opaque):
    """Back to straight-alpha RGBA bytes, clamped and rounded."""

    def flat(plane):
        return [v for row in plane for v in row]

    out = bytearray(width * height * 4)
    if opaque:
        out[3::4] = b"\xff" * (width * height)
        alpha = None
    else:
        alpha = [min(max(a, 0.0), 255.0) for a in flat(planes[3])]
        out[3::4] = bytes(int(a + 0.5) for a in alpha)
    for c in range(3):
        values = flat(planes[c])
        if alpha is not None:
            values = [min(v * 255.0 / a, 255.0) if a > 0.5 else 0.0 for v, a in zip(values, alpha)]
        out[c::4] = bytes(0 if v < 0 else 255 if v > 255 else int(v + 0.5) for v in values)
    return bytes(out)


def unsharp(planes, amount, progress, check):
    """orig + amount * (orig - blur), on the colour planes only."""
    if amount <= 0:
        return planes
    result = list(planes)
    for c in range(3):
        check()
        rows = planes[c]
        height, width = len(rows), len(rows[0])
        blurred = apply_taps(rows, blur_taps(height))
        blurred = transpose(apply_taps(transpose(blurred), blur_taps(width)))
        result[c] = [[o + amount * (o - b) for o, b in zip(orow, brow)] for orow, brow in zip(rows, blurred)]
        progress()
    return result


def lanczos_backend(width, height, rgba, scale, progress, check, sharpen=0.0):
    target_w, target_h = width * scale, height * scale
    planes, opaque = split_planes(width, height, rgba)
    row_taps = resize_taps(height, target_h)
    column_taps = resize_taps(width, target_w)
    steps = len(planes) * 2 + (3 if sharpen > 0 else 0)
    done = [0]

    def step():
        done[0] += 1
        progress(done[0] / steps)

    resized = []
    for plane in planes:
        check()
        tall = apply_taps(plane, row_taps)
        step()
        check()
        resized.append(transpose(apply_taps(transpose(tall), column_taps)))
        step()
    resized = unsharp(resized, sharpen, step, check)
    return target_w, target_h, join_planes(target_w, target_h, resized, opaque)


BUILTIN = {"lanczos": lanczos_backend}


def load_backend(name, folder=HERE):
    """The named backend's function. ``lanczos`` is built in; other names load
    ``backend_<name>.py`` from ``folder`` and use its ``upscale`` function."""
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
    return module.upscale
