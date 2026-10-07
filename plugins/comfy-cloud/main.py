#!/usr/bin/env python3
"""Comfy Cloud plugin for Xuan: generates and edits images with Seedream and
Ideogram through Comfy's own workflow templates, run on Comfy Cloud with
Comfy API v2 (https://docs.comfy.org/api-reference/v2/overview).

recipes.py lists the workflows, catalog.py keeps them up to date with
Comfy's templates, convert.py turns a template into a workflow the API runs
and comfy_api.py talks to the server. See README.md.
"""
import math
import os
import re
import sys
import threading
import time
import urllib.parse
from dataclasses import dataclass, field

# Xuan puts the SDK on PYTHONPATH; the checkout's copy is only a fallback.
sys.path.append(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "sdk", "python"))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from xuan_plugin import (  # noqa: E402
    INTERNAL_ERROR,
    INVALID_PARAMS,
    Cancelled,
    Job,
    NeedsSetup,
    Plugin,
    RpcError,
    encode_gray_png,
    ui,
)

from catalog import Catalog, describe  # noqa: E402
from comfy_api import SUCCEEDED, TERMINAL, Client, asset_ref, rejected_before_running  # noqa: E402
from recipes import ACTIONS, BRIA, GPT, RECIPES, add_alpha_mask, apply, choose_size, recipe_for  # noqa: E402

plugin = Plugin()
HERE = os.path.dirname(os.path.abspath(__file__))
SNAPSHOTS = os.path.join(HERE, "snapshots")
PANE = "comfy"
_catalog = None
_catalog_lock = threading.Lock()
_checking = threading.Event()


def make_client():
    base = plugin.settings.get("base_url") or "https://cloud.comfy.org"
    key = plugin.secrets.get("api_key") or ""
    if not key and "cloud.comfy.org" in base:
        raise NeedsSetup("Enter your Comfy API key in the plugin settings.")
    return Client(base, key)


def catalog(client):
    """The one catalog of this process, talking through ``client``."""
    global _catalog
    with _catalog_lock:
        if _catalog is None:
            _catalog = Catalog(client, plugin.data_dir, SNAPSHOTS)
        _catalog.client = client
        return _catalog


# --- Running --------------------------------------------------------------


def provenance(recipe, version, values, request_id, base):
    """How the image was made, for the layer's Generation info. Never the key."""
    record = {
        "model": recipe.label,
        "service": urllib.parse.urlsplit(base).hostname or "Comfy Cloud",
        "request_id": str(request_id)[:256],
        "extra": {"template": recipe.template, "template_date": (version or {}).get("template_date") or "unknown"},
    }
    seed = values.get("seed")
    if isinstance(seed, int) and not isinstance(seed, bool):
        record["seed"] = seed % 2147483648
    return record


def wait_for(job, client, job_id, stage=""):
    """Poll the job until it ends; cancels it on the user's cancel or the timeout.
    Progress is only a fraction once Comfy reports one: partner models such as
    Seedream report none, so Xuan shows a spinner with the state instead."""
    deadline = time.time() + float(plugin.settings.get("timeout") or 600)
    delay = 1.0
    while True:
        if job.cancelled:
            client.cancel(job_id)
            raise Cancelled()
        if time.time() > deadline:
            client.cancel(job_id)
            raise RpcError(INTERNAL_ERROR, "the job did not finish in time")
        status = client.job(job_id)
        state = str(status.get("status") or "").lower()
        progress = status.get("progress") or {}
        if isinstance(progress, dict) and progress.get("value"):
            job.progress(float(progress["value"]), stage + (progress.get("message") or state.capitalize()))
        else:
            job.progress(None, stage + STATES.get(state, state.capitalize() or "Running"))
        if state in TERMINAL:
            return status
        time.sleep(delay)
        delay = min(delay * 1.5, 5.0)


STATES = {"queued": "Queued on Comfy Cloud", "running": "Running on Comfy Cloud", "succeeded": "Finishing"}


def error_text(status):
    error = status.get("error") or {}
    if not isinstance(error, dict):
        return str(error)[:600]
    return str(error.get("message") or error.get("code") or "no details")[:600]


def size_refused(version, recipe, values, status):
    """Whether Comfy refused the size this run asked for rather than the
    template: the error names a size input set for the target. The template
    is still good, so it is not marked bad."""
    if not values.get("target"):
        return False
    text = error_text(status)
    return any(name.split(".", 1)[-1] in text for name in choose_size(version, recipe, values["target"])[0])


def fit():
    """How results are placed: covering their area where Xuan says it can
    (``fit_cover``), else over the source, which older Xuan understands."""
    return "cover" if "fit_cover" in (plugin.host_info.get("features") or []) else "source"


@dataclass
class Result:
    images: list  # downloaded images of the recipe's output node, in order
    assets: list  # those images' records on the server
    provenance: dict
    notes: list
    extra: dict = field(default_factory=dict)  # name -> downloaded images of nodes `post` added


def run(job, recipe, values, image_path=None, reference_path=None, post=None, download=True, stage="", mask_path=None):
    """Run ``recipe`` with ``values``, uploading the source image and the
    reference image first when given. ``post(workflow)`` may add nodes to the
    workflow and returns ``{name: node id}``; their images land in
    ``Result.extra``. Progress messages start with ``stage``.

    When Comfy refuses a newly converted workflow before running it, the
    previous version is tried once instead."""
    client = make_client()

    def say(message):
        job.progress(None, stage + message)

    say("Checking the workflow")
    books = catalog(client)
    state = books.ensure(recipe)
    notes = [state["warning"]] if state.get("warning") else []
    for role, path in (("image", image_path), ("reference", reference_path), ("mask", mask_path)):
        if path:
            job.check_cancelled()
            say("Uploading the image")
            values = dict(values, **{role: asset_ref(client.upload(path))})
    for attempt in (1, 2):
        version = state["current"]
        workflow = apply(version, recipe, values)
        added = post(workflow) if post else {}
        if attempt == 1 and values.get("target"):
            note = choose_size(version, recipe, values["target"])[1]
            if note:
                notes.append(note)
        job.check_cancelled()
        say("Submitting")
        job_id = client.submit(workflow).get("id")
        if not job_id:
            raise RpcError(INTERNAL_ERROR, "the server did not return a job id")
        status = wait_for(job, client, job_id, stage)
        if str(status.get("status")).lower() in SUCCEEDED:
            break
        if attempt == 1 and rejected_before_running(status) and not size_refused(version, recipe, values, status):
            fallback = books.mark_bad(recipe, error_text(status))
            if fallback:
                state = fallback
                notes.append(fallback["warning"])
                continue
        raise RpcError(INTERNAL_ERROR, f"Comfy job {status.get('status')}: {error_text(status)}")
    outputs = [o for o in status.get("outputs") or [] if isinstance(o, dict) and o.get("type", "image") == "image"]

    def of(node_id):
        return sorted((o for o in outputs if str(o.get("node_id")) == str(node_id)), key=lambda o: str(o.get("name", "")))

    assets = of(version["output"]) or ([] if added else sorted(outputs, key=lambda o: str(o.get("name", ""))))
    if not assets:
        raise RpcError(INTERNAL_ERROR, "the workflow produced no images")
    say("Downloading")
    count = [0]

    def fetch(records):
        paths = []
        for output in records:
            count[0] += 1
            destination = job.path(f"{recipe.id}-{count[0]}.png")
            client.download(output, destination)
            paths.append(destination)
        return paths

    images = fetch(assets) if download else []
    extra = {name: fetch(of(node_id)) for name, node_id in added.items()}
    return Result(images, assets, provenance(recipe, version, values, job_id, client.base), notes, extra)


def texts(notes):
    return [Job.text(note) for note in notes if note]


def source_size(job):
    source = job.source or {}
    if not job.source_path or not source.get("width") or not source.get("height"):
        raise RpcError(INVALID_PARAMS, "this action needs an image layer")
    return int(source["width"]), int(source["height"])


def target_of(job):
    """``inputs.target`` (document pixels a surface run covers), or None."""
    target = job.inputs.get("target") or {}
    try:
        width, height = int(target["width"]), int(target["height"])
    except (KeyError, TypeError, ValueError):
        return None
    return (width, height) if width > 0 and height > 0 else None


def source_doc_size(job):
    """The source sent, in document pixels (the export is scaled down)."""
    width, height = source_size(job)
    scale = float((job.source or {}).get("scale") or 1.0) or 1.0
    return max(1, round(width / scale)), max(1, round(height / scale))


def scaled_target(job):
    """The crop the plugin got for a box, in document pixels: the box target
    scaled by the crop's size over the box's (padding included)."""
    target = target_of(job)
    width, height = source_size(job)
    region = job.regions[0] if job.regions else {}
    if target and region.get("width") and region.get("height"):
        return max(1, round(target[0] * width / region["width"])), max(1, round(target[1] * height / region["height"]))
    return source_doc_size(job)


def layer_name(prompt):
    return prompt if len(prompt) <= 40 else prompt[:39].rstrip() + "…"


def region_prompt(job):
    region = job.regions[0] if job.regions else {}
    return str((region.get("fields") or {}).get("desc") or "").strip()


def rect_mask(path, size, region):
    """A grey PNG of ``size``, white inside the region's rectangle."""
    width, height = size
    x0 = min(width, max(0, int(region["x"])))
    x1 = min(width, max(x0, math.ceil(region["x"] + region["width"])))
    y0 = min(height, max(0, int(region["y"])))
    y1 = min(height, max(y0, math.ceil(region["y"] + region["height"])))
    black = bytes(width)
    inside = bytes(x0) + b"\xff" * (x1 - x0) + bytes(width - x1)
    gray = b"".join(inside if y0 <= y < y1 else black for y in range(height))
    with open(path, "wb") as handle:
        handle.write(encode_gray_png(width, height, gray))
    return path


def seed_of(job):
    try:
        return int(job.inputs.get("seed") or 0)
    except (TypeError, ValueError):
        return 0


# --- Actions --------------------------------------------------------------


@plugin.action("generate")
def generate(job):
    recipe = recipe_for("generate", job.inputs.get("model"))
    values = {"prompt": job.inputs.get("prompt") or "", "seed": seed_of(job),
              "aspect": job.inputs.get("aspect") or "1:1", "tier": job.inputs.get("resolution") or "2K"}
    if target_of(job):
        values["target"] = target_of(job)
    if not values["prompt"].strip():
        raise RpcError(INVALID_PARAMS, "Describe the image to generate")
    result = run(job, recipe, values)
    name = layer_name(values["prompt"].strip()) if job.inputs.get("surface") == "document" else recipe.label
    return [Job.image(path, name=name, provenance=result.provenance) for path in result.images] + texts(result.notes)


@plugin.action("edit")
def edit(job):
    recipe = recipe_for("edit", job.inputs.get("model"))
    values = {"prompt": job.inputs.get("prompt") or "", "seed": seed_of(job), "source_size": source_size(job),
              "target": target_of(job) or source_doc_size(job)}
    if not values["prompt"].strip():
        raise RpcError(INVALID_PARAMS, "Describe the change")
    result = run(job, recipe, values, image_path=job.source_path)
    images = [Job.image(path, name=recipe.label, fit=fit(), provenance=result.provenance) for path in result.images]
    return images + texts(result.notes)


@plugin.action("precise-edit")
def precise_edit(job):
    recipe = recipe_for("precise-edit")
    regions = [r for r in job.regions if (r.get("fields") or {}).get("desc") or (r.get("fields") or {}).get("text")]
    if not regions:
        raise RpcError(INVALID_PARAMS, "Draw a box and describe what to change in it")
    values = {"seed": seed_of(job), "source_size": source_size(job), "regions": regions,
              "quality": job.inputs.get("quality") or "medium", "background": job.inputs.get("background") or ""}
    result = run(job, recipe, values, image_path=job.source_path)
    images = [Job.image(path, name="Precise Edit", fit=fit(), provenance=result.provenance) for path in result.images]
    return images + texts(result.notes)


@plugin.action("split-layers")
def split_layers(job):
    recipe = recipe_for("split-layers", job.inputs.get("model"))
    width, height = source_size(job)
    if min(width, height) < 512:
        raise RpcError(INVALID_PARAMS, "Seedream needs an image of at least 512 × 512 pixels")
    values = {"prompt": job.inputs.get("prompt") or "", "seed": seed_of(job), "source_size": (width, height)}
    result = run(job, recipe, values, image_path=job.source_path)
    # The workflow saves the background plate first, then the layers.
    names = ["Background"] + [f"Layer {n}" for n in range(1, len(result.images))]
    outputs = [Job.image(path, name=name, fit_source=True, provenance=result.provenance)
               for path, name in zip(result.images, names)]
    return outputs + texts(result.notes)


# With Match the picture, GPT first draws the object into the flattened image
# (asked for a transparent background alongside a reference image, it redraws
# the whole scene instead), then Seedream lifts just that object out.
SCENE = ("Add what is described below to this picture, where it belongs, matching the picture's colours, "
         "lighting, perspective and style. Change nothing else.\n\n")
LIFT = "Separate only this into its own layer: {}. Everything else stays in the background."
LAYER_ALONE = "\n\nOn a transparent background, with nothing else in the picture."
LIFT_RECIPE = "split-flash"


def draw_and_lift(job, recipe, prompt, values):
    """GPT draws ``prompt`` into the source picture (opaque), then Seedream
    lifts just that object out. Returns ``(layers, provenance, notes)``."""
    scene = run(job, recipe, dict(values, prompt=SCENE + prompt, overrides={f"{GPT}.model.background": "opaque"}),
                reference_path=job.source_path, stage="Drawing it into the picture: ")
    lift_recipe = RECIPES[LIFT_RECIPE]
    lift = run(job, lift_recipe, {"prompt": LIFT.format(prompt), "seed": values["seed"]},
               image_path=scene.images[0], stage="Lifting it out: ")
    layers = lift.images[1:]  # the first is the background plate
    if not layers:
        raise RpcError(INTERNAL_ERROR, "Seedream found nothing to lift out of the picture; try describing it differently")
    record = dict(scene.provenance, model=f"{recipe.label} + {lift_recipe.label}")
    record["extra"] = dict(scene.provenance["extra"], lifted_with=lift_recipe.template)
    return layers, record, scene.notes + lift.notes


def as_layers(paths, prompt, record):
    name = layer_name(prompt)
    names = [name] if len(paths) == 1 else [f"{name} {n}" for n in range(1, len(paths) + 1)]
    return [Job.image(path, name=layer, fit=fit(), provenance=record) for path, layer in zip(paths, names)]


@plugin.action("generate-layer")
def generate_layer(job):
    recipe = recipe_for("generate-layer", job.inputs.get("model"))
    prompt = (job.inputs.get("prompt") or "").strip()
    if not prompt:
        raise RpcError(INVALID_PARAMS, "Describe what to put on the new layer")
    values = {"seed": seed_of(job), "source_size": source_size(job), "quality": job.inputs.get("quality") or "medium",
              "target": target_of(job) or source_doc_size(job)}
    if job.inputs.get("reference", True) is False:
        result = run(job, recipe, dict(values, prompt=prompt + LAYER_ALONE))
        layers, record, notes = result.images, result.provenance, result.notes
    else:
        layers, record, notes = draw_and_lift(job, recipe, prompt, values)
    return as_layers(layers, prompt, record) + texts(notes)


@plugin.action("generate-in-region")
def generate_in_region(job):
    """Draw a box and say what to add: GPT draws it into the crop around the
    box, Seedream lifts it out, and the layer shows within the box."""
    recipe = recipe_for("generate-in-region", job.inputs.get("model"))
    prompt = region_prompt(job)
    if not prompt:
        raise RpcError(INVALID_PARAMS, "Describe what to add in the box")
    values = {"seed": seed_of(job), "quality": job.inputs.get("quality") or "medium", "target": scaled_target(job)}
    layers, record, notes = draw_and_lift(job, recipe, prompt, values)
    return as_layers(layers, prompt, record) + texts(notes)


# Fill region: GPT repaints only the box (white in the mask).
FILL = ("Repaint only the white area of the mask with what is described below, matching the picture's colours, "
        "lighting, perspective and style.\n\n")


@plugin.action("fill-region")
def fill_region(job):
    """Draw a box (or use the selection) and say what belongs there: GPT
    Image repaints only that area of the picture."""
    recipe = recipe_for("fill-region", job.inputs.get("model"))
    prompt = region_prompt(job)
    if not prompt:
        raise RpcError(INVALID_PARAMS, "Describe what to paint in the box")
    size = source_size(job)
    region = job.regions[0]
    mask = region.get("mask") or rect_mask(job.path("mask.png"), size, region)
    values = {"prompt": FILL + prompt, "seed": seed_of(job), "quality": job.inputs.get("quality") or "medium",
              "target": scaled_target(job)}
    result = run(job, recipe, values, reference_path=job.source_path, mask_path=mask)
    return as_layers(result.images, prompt, result.provenance) + texts(result.notes)


@plugin.action("remove-background")
def remove_background(job):
    """From the menu: a cut-out copy as a new layer. As Xuan's Remove
    Background or Select Subject (``inputs.capability``): the cut-out's alpha
    as a mask, which Xuan turns into a layer mask or the selection. Comfy
    saves that mask as its own image, so the plugin never decodes a PNG."""
    recipe = recipe_for("remove-background")
    source_size(job)
    if job.inputs.get("capability"):
        result = run(job, recipe, {}, image_path=job.source_path, download=False,
                     post=lambda workflow: {"mask": add_alpha_mask(workflow, BRIA)})
        if not result.extra.get("mask"):
            raise RpcError(INTERNAL_ERROR, "Comfy returned no mask")
        return [Job.mask(result.extra["mask"][0], fit_source=True)] + texts(result.notes)
    result = run(job, recipe, {}, image_path=job.source_path)
    return [Job.image(result.images[0], name="Cut-out", fit_source=True, provenance=result.provenance)] + texts(result.notes)


def estimate(job):
    return {"cost": "Comfy Cloud credits apply", "seconds": 60 if job.action == "split-layers" else 30}


for _action in ACTIONS:
    plugin.estimate(_action)(estimate)


# --- Pane -----------------------------------------------------------------


def check_all():
    """Check every recipe for a newer template now, then redraw the pane."""
    try:
        books = catalog(make_client())
        for recipe in RECIPES.values():
            try:
                books.ensure(recipe, force=True)
            except RpcError:
                pass  # the state records why
    except (NeedsSetup, RpcError):
        pass
    finally:
        _checking.clear()
        plugin.update_pane(PANE, pane_tree())


def action_labels(path=os.path.join(HERE, "plugin.toml")):
    """Each action's label from plugin.toml without the dialog's ellipsis, so
    the pane names actions as the menus do and cannot fall behind them.

    Reads the ``[[actions]]`` tables line by line, because tomllib is not in
    Python 3.10; test_main checks the result against tomllib."""
    labels, current, in_action = {}, None, False
    try:
        with open(path, encoding="utf-8") as handle:
            lines = handle.read().splitlines()
    except OSError:
        return labels
    for line in lines:
        line = line.strip()
        if line.startswith("["):
            in_action, current = line == "[[actions]]", None
            continue
        match = re.match(r'(id|label)\s*=\s*"([^"\\]*)"', line)
        if not in_action or not match:
            continue
        if match.group(1) == "id":
            current = match.group(2)
        elif current is not None:
            labels.setdefault(current, match.group(2).rstrip("…"))
    return labels


ACTION_LABELS = action_labels()


def pane_tree():
    rows = [ui.heading("Workflows")]
    books = _catalog or Catalog(None, plugin.data_dir, SNAPSHOTS)
    for action, recipe_ids in ACTIONS.items():
        rows.append(ui.label(ACTION_LABELS.get(action, action)))
        for recipe in (RECIPES[recipe_id] for recipe_id in recipe_ids):
            state = books.state(recipe)
            version = state.get("current")
            date = (version or {}).get("template_date")
            rows.append(ui.label(f"{recipe.label} · Comfy template of {date}" if date else f"{recipe.label} · {describe(version)}", muted=True, small=True))
            if state.get("warning"):
                rows.append(ui.label(state["warning"], small=True))
    if _checking.is_set():
        rows.append(ui.progress(None, "Checking Comfy for updates…"))
    else:
        rows.append(ui.button("check", "Check for updates"))
    rows.append(ui.link("Comfy Cloud", "https://cloud.comfy.org"))
    return ui.column(*rows)


@plugin.pane(PANE)
def comfy_pane(pane):
    if pane.widget == "check" and not _checking.is_set():
        _checking.set()
        threading.Thread(target=check_all, daemon=True).start()
    return pane_tree()


if __name__ == "__main__":
    plugin.run()
