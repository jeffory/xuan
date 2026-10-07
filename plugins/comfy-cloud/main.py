#!/usr/bin/env python3
"""Comfy Cloud plugin for Xuan: generates and edits images with Seedream and
Ideogram through Comfy's own workflow templates, run on Comfy Cloud with
Comfy API v2 (https://docs.comfy.org/api-reference/v2/overview).

recipes.py lists the workflows, catalog.py keeps them up to date with
Comfy's templates, convert.py turns a template into a workflow the API runs
and comfy_api.py talks to the server. See README.md.
"""
import os
import sys
import threading
import time
import urllib.parse

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "sdk", "python"))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from xuan_plugin import (  # noqa: E402
    INTERNAL_ERROR,
    INVALID_PARAMS,
    Cancelled,
    Job,
    NeedsSetup,
    Plugin,
    RpcError,
    ui,
)

from catalog import Catalog, describe  # noqa: E402
from comfy_api import SUCCEEDED, TERMINAL, Client, asset_ref, rejected_before_running  # noqa: E402
from recipes import ACTIONS, RECIPES, apply, recipe_for  # noqa: E402

plugin = Plugin()
HERE = os.path.dirname(os.path.abspath(__file__))
SNAPSHOTS = os.path.join(HERE, "snapshots")
PANE = "comfy"
history = []  # recent jobs for the pane
history_lock = threading.Lock()
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


def wait_for(job, client, job_id, entry):
    """Poll the job until it ends; cancels it on the user's cancel or the timeout."""
    deadline = time.time() + float(plugin.settings.get("timeout") or 600)
    delay = 1.0
    while True:
        if job.cancelled:
            client.cancel(job_id)
            entry["state"] = "cancelled"
            raise Cancelled()
        if time.time() > deadline:
            client.cancel(job_id)
            entry["state"] = "timed out"
            raise RpcError(INTERNAL_ERROR, "the job did not finish in time")
        status = client.job(job_id)
        state = str(status.get("status") or "").lower()
        entry["state"] = state or "running"
        progress = status.get("progress") or {}
        if isinstance(progress, dict) and progress.get("value") is not None:
            job.progress(0.1 + 0.8 * float(progress["value"]), progress.get("message") or state)
        else:
            job.progress(None, state or "running")
        if state in TERMINAL:
            return status
        time.sleep(delay)
        delay = min(delay * 1.5, 5.0)


def error_text(status):
    error = status.get("error") or {}
    if not isinstance(error, dict):
        return str(error)[:600]
    return str(error.get("message") or error.get("code") or "no details")[:600]


def run(job, recipe, values, image_path=None):
    """Run ``recipe`` with ``values``; returns ``(paths, provenance, notes)``.
    When Comfy refuses a newly converted workflow before running it, the
    previous version is tried once instead."""
    client = make_client()
    entry = {"id": job.id[:8], "action": job.action, "model": recipe.label, "state": "preparing", "started": time.time()}
    with history_lock:
        history.insert(0, entry)
        del history[10:]
    try:
        job.progress(0.02, "Checking the workflow")
        books = catalog(client)
        state = books.ensure(recipe)
        notes = [state["warning"]] if state.get("warning") else []
        if image_path:
            job.check_cancelled()
            entry["state"] = "uploading"
            job.progress(0.05, "Uploading")
            values = dict(values, image=asset_ref(client.upload(image_path)))
        for attempt in (1, 2):
            version = state["current"]
            workflow = apply(version, recipe, values)
            job.check_cancelled()
            job.progress(0.1, "Submitting")
            job_id = client.submit(workflow).get("id")
            if not job_id:
                raise RpcError(INTERNAL_ERROR, "the server did not return a job id")
            entry.update(state="queued", remote=job_id)
            status = wait_for(job, client, job_id, entry)
            if str(status.get("status")).lower() in SUCCEEDED:
                break
            if attempt == 1 and rejected_before_running(status):
                fallback = books.mark_bad(recipe, error_text(status))
                if fallback:
                    state = fallback
                    notes.append(fallback["warning"])
                    continue
            raise RpcError(INTERNAL_ERROR, f"Comfy job {status.get('status')}: {error_text(status)}")
        outputs = [o for o in status.get("outputs") or [] if isinstance(o, dict) and o.get("type", "image") == "image"]
        mine = [o for o in outputs if str(o.get("node_id")) == str(version["output"])]
        images = sorted(mine or outputs, key=lambda o: str(o.get("name", "")))
        if not images:
            raise RpcError(INTERNAL_ERROR, "the workflow produced no images")
        job.progress(0.95, "Downloading")
        paths = []
        for index, output in enumerate(images):
            destination = job.path(f"result-{index + 1}.png")
            client.download(output, destination)
            paths.append(destination)
        entry["state"] = "done"
        return paths, provenance(recipe, version, values, job_id, client.base), notes
    except Cancelled:
        entry["state"] = "cancelled"
        raise
    except Exception:
        if entry["state"] not in ("cancelled", "timed out"):
            entry["state"] = "failed"
        raise


def texts(notes):
    return [Job.text(note) for note in notes if note]


def source_size(job):
    source = job.source or {}
    if not job.source_path or not source.get("width") or not source.get("height"):
        raise RpcError(INVALID_PARAMS, "this action needs an image layer")
    return int(source["width"]), int(source["height"])


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
    if not values["prompt"].strip():
        raise RpcError(INVALID_PARAMS, "Describe the image to generate")
    paths, record, notes = run(job, recipe, values)
    return [Job.image(path, name=recipe.label, provenance=record) for path in paths] + texts(notes)


@plugin.action("edit")
def edit(job):
    recipe = recipe_for("edit", job.inputs.get("model"))
    values = {"prompt": job.inputs.get("prompt") or "", "seed": seed_of(job), "source_size": source_size(job)}
    if not values["prompt"].strip():
        raise RpcError(INVALID_PARAMS, "Describe the change")
    paths, record, notes = run(job, recipe, values, image_path=job.source_path)
    return [Job.image(path, name=recipe.label, fit_source=True, provenance=record) for path in paths] + texts(notes)


@plugin.action("precise-edit")
def precise_edit(job):
    recipe = recipe_for("precise-edit")
    regions = [r for r in job.regions if (r.get("fields") or {}).get("desc") or (r.get("fields") or {}).get("text")]
    if not regions:
        raise RpcError(INVALID_PARAMS, "Draw a box and describe what to change in it")
    values = {"seed": seed_of(job), "source_size": source_size(job), "regions": regions,
              "quality": job.inputs.get("quality") or "medium", "background": job.inputs.get("background") or ""}
    paths, record, notes = run(job, recipe, values, image_path=job.source_path)
    return [Job.image(path, name="Precise Edit", fit_source=True, provenance=record) for path in paths] + texts(notes)


@plugin.action("split-layers")
def split_layers(job):
    recipe = recipe_for("split-layers", job.inputs.get("model"))
    width, height = source_size(job)
    if min(width, height) < 512:
        raise RpcError(INVALID_PARAMS, "Seedream needs an image of at least 512 × 512 pixels")
    values = {"prompt": job.inputs.get("prompt") or "", "seed": seed_of(job), "source_size": (width, height)}
    paths, record, notes = run(job, recipe, values, image_path=job.source_path)
    # The workflow saves the background plate first, then the layers.
    names = ["Background"] + [f"Layer {n}" for n in range(1, len(paths))]
    outputs = [Job.image(path, name=name, fit_source=True, provenance=record) for path, name in zip(paths, names)]
    return outputs + texts(notes)


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


ACTION_LABELS = {"generate": "Generate Image", "edit": "Edit Image", "precise-edit": "Precise Edit", "split-layers": "Split into Layers"}


def pane_tree():
    rows = [ui.heading("Workflows")]
    books = _catalog or Catalog(None, plugin.data_dir, SNAPSHOTS)
    for action, recipe_ids in ACTIONS.items():
        rows.append(ui.label(ACTION_LABELS[action]))
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
    rows += [ui.separator(), ui.heading("Recent jobs")]
    with history_lock:
        entries = list(history)
    if entries:
        items = [ui.item(e["id"], f"{e['model']} · {e['state']}", time.strftime("%H:%M:%S", time.localtime(e["started"]))) for e in entries]
        rows.append(ui.listing("history", items))
    else:
        rows.append(ui.label("No Comfy jobs yet.", muted=True))
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
