"""The workflows the plugin offers: which of Comfy's templates each one runs
and which node inputs Xuan fills in. Inputs are addressed as
``"<node class>.<input>"`` so they keep working when Comfy renumbers a
template's nodes.
"""
import copy
import math
import re
from dataclasses import dataclass, field
from typing import Dict, Optional, Tuple

from convert import Unsupported, upstream


@dataclass(frozen=True)
class Recipe:
    id: str
    label: str  # the model, as the user picks it
    template: str  # name in https://cloud.comfy.org/templates/index.json
    output: str  # class of the node whose images are the result
    prompt: Tuple[str, ...] = ()
    seed: Tuple[str, ...] = ()
    image: Tuple[str, ...] = ()  # each gets the uploaded source image
    reference: Tuple[str, ...] = ()  # each gets the uploaded reference image, when one is sent
    mask: Tuple[str, ...] = ()  # each gets the uploaded mask (white is repainted), when one is sent
    size: Optional[str] = None  # a combo of "(2K) 2848x1600 (16:9)"-style presets
    match_size: Optional[Tuple[str, str, str]] = None  # (preset combo, width, height): follow the source's shape
    match_area: int = 2048 * 2048  # pixels match_size aims for
    match_step: int = 0  # width and height are multiples of this; 0 takes the inputs' own step
    # How the model's size follows inputs.target: "custom" (match_size
    # inputs), "presets" (the size combo), "source" (it keeps image 1's size)
    # or "none".
    size_rule: str = "none"
    quality: Optional[str] = None
    background: Optional[str] = None
    regions: Optional[str] = None  # a CreateBoundingBoxes node
    fixed: Dict[str, object] = field(default_factory=dict)
    select: Dict[str, str] = field(default_factory=dict)  # dynamic combo -> option, applied while converting

    def targets(self):
        """Every input this recipe sets."""
        found = list(self.prompt) + list(self.seed) + list(self.image) + list(self.reference) + list(self.mask) + list(self.fixed)
        found += [t for t in (self.size, self.quality, self.background) if t]
        if self.match_size:
            found += list(self.match_size)
        if self.regions:
            found += [f"{self.regions}.{name}" for name in ("width", "height", "editor_state")]
        return found


SEEDREAM = "ByteDanceSeedreamNodeV3"
IDEOGRAM_T2I = "IdeogramTextToImageApi"
IDEOGRAM_EDIT = "IdeogramEditApi"
IDEOGRAM_PRECISE = "IdeogramPreciseEditApi"
SEPARATION = "ByteDanceSeedreamLayerSeparationNodeV2"
GPT = "OpenAIGPTImageNodeV2"
BRIA = "BriaRemoveImageBackground"

_SEEDREAM_T2I = dict(
    output="SaveImageAdvanced",
    prompt=(f"{SEEDREAM}.prompt",),
    seed=(f"{SEEDREAM}.model.seed",),
    size=f"{SEEDREAM}.model.size_preset",
    match_size=(f"{SEEDREAM}.model.size_preset", f"{SEEDREAM}.model.width", f"{SEEDREAM}.model.height"),
    size_rule="custom",
    fixed={f"{SEEDREAM}.model.watermark": False},
)
_GPT_LAYER = dict(
    output="SaveImage",
    prompt=(f"{GPT}.prompt",),
    seed=(f"{GPT}.seed",),
    reference=(f"{GPT}.model.images.image_1",),
    match_size=(f"{GPT}.model.size", f"{GPT}.model.custom_width", f"{GPT}.model.custom_height"),
    match_area=1536 * 1536,
    match_step=16,
    quality=f"{GPT}.model.quality",
    size_rule="custom",
    fixed={f"{GPT}.model.background": "transparent", f"{GPT}.n": 1},
)
# Fill region: GPT repaints the white part of a mask over the picture.
_GPT_FILL = dict(_GPT_LAYER, mask=(f"{GPT}.model.mask",), fixed={f"{GPT}.model.background": "opaque", f"{GPT}.n": 1})
_SEPARATION = dict(
    template="api_bytedance_seedream_5_0_layer_separation",
    output="SaveImageAdvanced",
    prompt=(f"{SEPARATION}.model.prompt",),
    seed=(f"{SEPARATION}.model.seed",),
    image=(f"{SEPARATION}.model.image",),
    fixed={f"{SEPARATION}.model.watermark": False, f"{SEPARATION}.model.crop_layers": False},
)

RECIPES = {
    recipe.id: recipe
    for recipe in (
        Recipe("seedream-pro", "Seedream 5.0 Pro", "api_bytedance_seedream_5_0_pro_t2i", **_SEEDREAM_T2I),
        Recipe("seedream-flash", "Seedream 5.0 Flash", "api_bytedance_seedream_5_0_flash_t2i", **_SEEDREAM_T2I),
        Recipe(
            "ideogram", "Ideogram 4.5", "api_ideogram_v4_5_t2i", "SaveImageAdvanced",
            prompt=(f"{IDEOGRAM_T2I}.model.prompt",), seed=(f"{IDEOGRAM_T2I}.model.seed",), size=f"{IDEOGRAM_T2I}.model.size",
            size_rule="presets",
        ),
        Recipe(
            "ideogram-edit", "Ideogram 4.5 Edit", "api_ideogram_v4_5_image_edit", "SaveImageAdvanced",
            prompt=(f"{IDEOGRAM_EDIT}.model.prompt",), seed=(f"{IDEOGRAM_EDIT}.model.seed",),
            image=(f"{IDEOGRAM_EDIT}.model.images.image_1",), fixed={f"{IDEOGRAM_EDIT}.model.size": "source"},
            size_rule="source",
        ),
        Recipe(
            "seedream-pro-edit", "Seedream 5.0 Pro Edit", "api_bytedance_seedream_5_0_pro_image_edit", "SaveImageAdvanced",
            prompt=(f"{SEEDREAM}.prompt",), seed=(f"{SEEDREAM}.model.seed",), image=(f"{SEEDREAM}.model.images.image_1",),
            match_size=(f"{SEEDREAM}.model.size_preset", f"{SEEDREAM}.model.width", f"{SEEDREAM}.model.height"),
            size_rule="custom",
            fixed={f"{SEEDREAM}.model.watermark": False},
        ),
        Recipe(
            "ideogram-precise", "Ideogram 4.5 Precise Edit", "api_ideogram_v4_5_precise_image_edit", "SaveImageAdvanced",
            seed=(f"{IDEOGRAM_PRECISE}.model.seed",), image=(f"{IDEOGRAM_PRECISE}.model.images.image_1", "CreateBoundingBoxes.background"),
            quality=f"{IDEOGRAM_PRECISE}.model.quality", background="BuildJsonPromptIdeogram.background", regions="CreateBoundingBoxes",
            size_rule="source",
        ),
        Recipe("split-flash", "Seedream 5.0 Flash", **_SEPARATION),
        Recipe("split-pro", "Seedream 5.0 Pro", select={f"{SEPARATION}.model": "seedream 5.0 pro"}, **_SEPARATION),
        Recipe("gpt-flare-layer", "GPT Image 2.5 Flare", "api_openai_gpt_image_25_flare_t2i", **_GPT_LAYER),
        Recipe("gpt-sunburst-layer", "GPT Image 2.5 Sunburst", "api_openai_gpt_image_25_sunburst_t2i", **_GPT_LAYER),
        Recipe("gpt-flare-fill", "GPT Image 2.5 Flare", "api_openai_gpt_image_25_flare_t2i", **_GPT_FILL),
        Recipe("gpt-sunburst-fill", "GPT Image 2.5 Sunburst", "api_openai_gpt_image_25_sunburst_t2i", **_GPT_FILL),
        Recipe("bria-remove-background", "Bria RMBG 2.0", "utility_bria_remove_image_background", "SaveImage", image=(f"{BRIA}.image",)),
    )
}

# Action -> the recipe each value of its "model" input runs; the first is the default.
ACTIONS = {
    "generate": ("seedream-pro", "seedream-flash", "ideogram"),
    "edit": ("ideogram-edit", "seedream-pro-edit"),
    "precise-edit": ("ideogram-precise",),
    "split-layers": ("split-pro", "split-flash"),
    "generate-layer": ("gpt-flare-layer", "gpt-sunburst-layer"),
    "remove-background": ("bria-remove-background",),
    "generate-in-region": ("gpt-flare-layer", "gpt-sunburst-layer"),
    "fill-region": ("gpt-flare-fill", "gpt-sunburst-fill"),
}


def recipe_for(action, model=None):
    ids = ACTIONS[action]
    return RECIPES[model if model in ids else ids[0]]


# --- Filling in a converted workflow -------------------------------------

def cover_size(target, spec_w, spec_h, step=0):
    """The smallest size of the target's shape the model accepts that is at
    least ``target``, as ``(width, height, under)``: rounded up to the step,
    scaled up evenly to the minimum, and only scaled down when the model's
    maximum is smaller. A shape the limits cannot hold (a long banner) keeps
    each side within them; Xuan's cover placement absorbs the change of
    shape. ``under`` says the size is smaller than the target."""
    ow, oh = (spec_w[1] if len(spec_w) > 1 else {}), (spec_h[1] if len(spec_h) > 1 else {})
    step = step or max(int(ow.get("step") or 1), int(oh.get("step") or 1), 1)
    lo_w, hi_w = ow.get("min", 1), ow.get("max", 1 << 16)
    lo_h, hi_h = oh.get("min", 1), oh.get("max", 1 << 16)
    w, h = float(max(target[0], 1)), float(max(target[1], 1))
    grow = max(lo_w / w, lo_h / h, 1.0)
    w, h = w * grow, h * grow
    shrink = min(hi_w / w, hi_h / h, 1.0)
    w, h = w * shrink, h * shrink

    def snap(value, lo, hi):
        # Rounded first so float noise (2048.0000001) does not add a step.
        steps = round(value / step, 6)
        value = int((math.floor(steps) if shrink < 1.0 else math.ceil(steps)) * step)
        return min(max(value, math.ceil(lo / step) * step), math.floor(hi / step) * step)

    w, h = snap(w, lo_w, hi_w), snap(h, lo_h, hi_h)
    return w, h, w < target[0] or h < target[1]


def preset_at_least(options, target, tolerance=0.03):
    """The smallest preset of the target's shape that covers it, as
    ``(label, False)``; else the largest of the closest shape, ``(label,
    True)``; None without presets."""
    want = target[0] / max(target[1], 1)
    sized = []
    for option in options or []:
        match = PRESET.match(option) if isinstance(option, str) else None
        if match:
            sized.append((option, int(match["w"]), int(match["h"])))
    if not sized:
        return None
    shape = [s for s in sized if abs(math.log((s[1] / s[2]) / want)) <= tolerance]
    big = [s for s in shape if s[1] >= target[0] and s[2] >= target[1]]
    if big:
        return min(big, key=lambda s: s[1] * s[2])[0], False
    pool = shape or sorted(sized, key=lambda s: abs(math.log((s[1] / s[2]) / want)))[:1]
    return max(pool, key=lambda s: s[1] * s[2])[0], True


def choose_size(entry, recipe, target):
    """The size inputs to set so the model renders at least ``target``, as
    ``({input: value}, note)``; the note says when it cannot."""
    specs = entry.get("specs") or {}
    if recipe.size_rule == "custom" and recipe.match_size:
        combo, width, height = recipe.match_size
        w, h, under = cover_size(target, specs.get(width) or ["INT", {}], specs.get(height) or ["INT", {}], recipe.match_step)
        options = (specs.get(combo) or [None, {}])[1].get("options") or []
        note = f"{recipe.label} renders at most {w}×{h}; Xuan scales it up to {target[0]}×{target[1]}" if under else None
        return {combo: "Custom" if "Custom" in options else "custom", width: w, height: h}, note
    if recipe.size_rule == "presets" and recipe.size:
        chosen = preset_at_least((specs.get(recipe.size) or [None, {}])[1].get("options"), target)
        if chosen:
            note = f"{recipe.label} renders at most {chosen[0]}; Xuan scales it up" if chosen[1] else None
            return {recipe.size: chosen[0]}, note
    return {}, None


# "(2K) 2848x1600 (16:9)" or plain "1536x1024"
PRESET = re.compile(r"^(?:\((?P<tier>[\d.]+K)\) )?(?P<w>\d+)x(?P<h>\d+)(?: \((?P<a>\d+):(?P<b>\d+)\))?$")


def _find(graph, target):
    kind, name = target.split(".", 1)
    found = [node_id for node_id, node in graph.items() if node.get("class_type") == kind]
    if len(found) != 1:
        raise Unsupported(f"the workflow has {len(found)} {kind} nodes, expected one")
    return found[0], name


def check(graph, specs, recipe):
    """Raise Unsupported unless every input the recipe sets exists."""
    for target in recipe.targets():
        node_id, name = _find(graph, target)
        if name not in specs.get(node_id, {}):
            raise Unsupported(f"{target} is no longer an input of the template")


def target_specs(graph, specs, recipe):
    """The definitions of the recipe's inputs, kept with the converted
    workflow so presets and limits are known without the full definitions."""
    return {target: specs[node_id][name] for target in recipe.targets() for node_id, name in [_find(graph, target)]}


def pick_preset(options, aspect, tier):
    """The preset label for ``aspect`` ("16:9") at ``tier`` ("2K"), else the
    closest shape in that tier, else None."""
    presets = [(o, PRESET.match(o)) for o in options or [] if isinstance(o, str)]
    presets = [(o, m) for o, m in presets if m]
    exact = [o for o, m in presets if m["tier"] == tier and m["a"] and f"{m['a']}:{m['b']}" == aspect]
    if exact:
        return exact[0]
    try:
        a, b = (float(v) for v in aspect.split(":"))
    except ValueError:
        return None
    same_tier = [(o, m) for o, m in presets if m["tier"] == tier] or presets
    if not same_tier:
        return None
    want = math.log(a / b)
    return min(same_tier, key=lambda om: abs(math.log(int(om[1]["w"]) / int(om[1]["h"])) - want))[0]


def fit_size(width, height, spec_w, spec_h, area=2048 * 2048, step=0):
    """A size with the source's shape and about ``area`` pixels inside the
    width/height limits, or None when the shape cannot fit them."""
    opts_w, opts_h = (spec_w[1] if len(spec_w) > 1 else {}), (spec_h[1] if len(spec_h) > 1 else {})
    if width <= 0 or height <= 0:
        return None
    scale = math.sqrt(area / (width * height))
    step = step or max(int(opts_w.get("step") or 1), int(opts_h.get("step") or 1), 1)
    w = int(round(width * scale / step)) * step
    h = int(round(height * scale / step)) * step
    lo_w, hi_w = opts_w.get("min", 1), opts_w.get("max", 1 << 16)
    lo_h, hi_h = opts_h.get("min", 1), opts_h.get("max", 1 << 16)
    for _ in range(2):  # grow to the minimum, then shrink to the maximum
        grow = max(lo_w / w, lo_h / h, 1.0)
        w, h = int(math.ceil(w * grow / step)) * step, int(math.ceil(h * grow / step)) * step
        shrink = min(hi_w / w, hi_h / h, 1.0)
        w, h = int(w * shrink // step) * step, int(h * shrink // step) * step
    if not (lo_w <= w <= hi_w and lo_h <= h <= hi_h):
        return None
    if abs(math.log((w / h) / (width / height))) > 0.02:
        return None
    return w, h


def bounding_boxes(regions, width, height):
    """``CreateBoundingBoxes`` values for regions given in source pixels: its
    canvas must be a multiple of 16, so the boxes are scaled to that grid."""
    grid_w = max(64, int(round(width / 16)) * 16)
    grid_h = max(64, int(round(height / 16)) * 16)
    sx, sy = grid_w / max(width, 1), grid_h / max(height, 1)
    boxes = []
    for region in regions:
        fields = region.get("fields") or {}
        kind = "text" if fields.get("type") == "text" else "obj"
        boxes.append({
            "x": int(round(region["x"] * sx)),
            "y": int(round(region["y"] * sy)),
            "width": int(round(region["width"] * sx)),
            "height": int(round(region["height"] * sy)),
            "metadata": {
                "type": kind,
                "text": (fields.get("text") or "") if kind == "text" else "",
                "desc": fields.get("desc") or "",
                "palette": [],
            },
        })
    return grid_w, grid_h, boxes


def apply(entry, recipe, values):
    """The workflow to submit: ``entry["api"]`` with the recipe's inputs set
    from ``values`` (prompt, seed, image, aspect, tier, source_size, quality,
    background, regions) and pruned to what the output needs."""
    graph = copy.deepcopy(entry["api"])
    specs = entry.get("specs") or {}

    def put(target, value, link=False):
        node_id, name = _find(graph, target)
        graph[node_id]["inputs"][name] = {"__value__": value} if isinstance(value, list) and not link else value

    for target, value in recipe.fixed.items():
        put(target, value)
    for target, value in (values.get("overrides") or {}).items():
        put(target, value)
    if values.get("prompt") is not None:
        for target in recipe.prompt:
            put(target, values["prompt"])
    if values.get("seed") is not None:
        for target in recipe.seed:
            put(target, int(values["seed"]) % 2147483648)
    def load(asset, targets, title):
        numeric = [int(k) for k in graph if str(k).isdigit()]
        node_id = str(max(numeric, default=0) + 1)
        graph[node_id] = {"class_type": "LoadImage", "inputs": {"image": asset}, "_meta": {"title": title}}
        for target in targets:
            put(target, [node_id, 0], link=True)

    if recipe.image:
        if values.get("image") is None:
            raise ValueError("this workflow needs an image")
        load(values["image"], recipe.image, "Xuan source")
    if recipe.reference and values.get("reference") is not None:
        load(values["reference"], recipe.reference, "Xuan reference")
    if recipe.mask and values.get("mask") is not None:
        numeric = [int(k) for k in graph if str(k).isdigit()]
        node_id = str(max(numeric, default=0) + 1)
        graph[node_id] = {"class_type": "LoadImageMask", "inputs": {"image": values["mask"], "channel": "red"},
                          "_meta": {"title": "Xuan mask"}}
        for target in recipe.mask:
            put(target, [node_id, 0], link=True)
    target = values.get("target")
    if target:
        sets, _ = choose_size(entry, recipe, target)
        for name, value in sets.items():
            put(name, value)
    elif recipe.size and values.get("aspect"):
        options = (specs.get(recipe.size) or [None, {}])[1].get("options")
        preset = pick_preset(options, values["aspect"], values.get("tier") or "2K")
        if preset:
            put(recipe.size, preset)
    if not target and recipe.match_size and values.get("source_size"):
        combo, width, height = recipe.match_size
        size = fit_size(*values["source_size"], specs.get(width) or ["INT", {}], specs.get(height) or ["INT", {}],
                        area=recipe.match_area, step=recipe.match_step)
        options = (specs.get(combo) or [None, {}])[1].get("options") or []
        if size and "Custom" in options:
            put(combo, "Custom")
            put(width, size[0])
            put(height, size[1])
        else:
            w, h = values["source_size"]
            preset = pick_preset(options, f"{w}:{h}", "2K")
            if preset:
                put(combo, preset)
    if recipe.quality and values.get("quality"):
        put(recipe.quality, values["quality"])
    if recipe.background and values.get("background") is not None:
        put(recipe.background, values["background"])
    if recipe.regions:
        width, height = values.get("source_size") or (1024, 1024)
        grid_w, grid_h, boxes = bounding_boxes(values.get("regions") or [], width, height)
        put(f"{recipe.regions}.width", grid_w)
        put(f"{recipe.regions}.height", grid_h)
        put(f"{recipe.regions}.editor_state", boxes)
        node_id, _ = _find(graph, f"{recipe.regions}.width")
        graph[node_id]["inputs"].pop("bboxes", None)  # boxes from upstream would win over ours
        graph[node_id]["inputs"]["last_incoming"] = {"__value__": []}
    return upstream(graph, entry["output"])


def add_alpha_mask(graph, kind):
    """Save the alpha of the image the ``kind`` node outputs as a grey image
    (white is opaque), next to the workflow's own output. Returns the id of
    the new save node."""
    source = next(node_id for node_id, node in graph.items() if node.get("class_type") == kind)
    first = max((int(k) for k in graph if str(k).isdigit()), default=0) + 1
    split, invert, image, save = (str(first + n) for n in range(4))
    # SplitImageWithAlpha gives the mask as ComfyUI keeps it: 1 - alpha.
    graph[split] = {"class_type": "SplitImageWithAlpha", "inputs": {"image": [source, 0]}}
    graph[invert] = {"class_type": "InvertMask", "inputs": {"mask": [split, 1]}}
    graph[image] = {"class_type": "MaskToImage", "inputs": {"mask": [invert, 0]}}
    graph[save] = {"class_type": "SaveImage", "inputs": {"images": [image, 0], "filename_prefix": "xuan_mask"}}
    return save
