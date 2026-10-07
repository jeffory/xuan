"""Keeps each recipe's workflow up to date with Comfy's template.

Once a day per recipe the template is downloaded again (it is public and
small). When it changed, it is converted with the server's node definitions
and checked against the recipe; if that works the new version is used and
the old one kept as ``last_good``. If it does not, the plugin keeps the
version it has and says why. The plugin ships a converted snapshot of every
recipe (``snapshots/``) for the first run.

State per recipe, in ``<data dir>/recipes/<id>.json``::

    {"current": {...}, "last_good": {...} | null, "checked_at": <unix time>,
     "bad": {"<template sha256>": "<reason>"}, "warning": "..." | null,
     "retry": bool}

where a version is ``{"template", "template_sha256", "template_date",
"converted_at", "output", "api", "specs"}``.
"""
import datetime
import hashlib
import json
import os
import threading
import time

from convert import Unsupported, to_api
from recipes import check, target_specs
from xuan_plugin import INTERNAL_ERROR, RpcError

CHECK_EVERY = 24 * 3600
RETRY_AFTER = 3600


def _read(path):
    try:
        with open(path, "r", encoding="utf-8") as handle:
            return json.load(handle)
    except (OSError, ValueError):
        return None


def _write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    temporary = path + ".tmp"
    with open(temporary, "w", encoding="utf-8") as handle:
        json.dump(data, handle, indent=1)
    os.replace(temporary, path)


def template_date(index, name):
    """The ``date`` Comfy's template index gives ``name``, if any."""
    for category in index or []:
        for entry in (category or {}).get("templates") or []:
            if isinstance(entry, dict) and entry.get("name") == name:
                return entry.get("date")
    return None


def recipe_key(recipe):
    """What a converted version was made for: the inputs the recipe sets and
    the options it selects. A version made for other inputs lacks their
    definitions, so it is converted again."""
    text = json.dumps([sorted(recipe.targets()), sorted(recipe.select.items())])
    return hashlib.sha256(text.encode()).hexdigest()[:16]


def describe(version):
    """How a version is named to the user: by Comfy's date for its template."""
    if not version:
        return "none"
    date = version.get("template_date") or (version.get("converted_at") or "")[:10]
    return f"the version from {date}" if date else "the shipped version"


class Catalog:
    def __init__(self, client, data_dir, bundled_dir, clock=time.time):
        self.client = client
        self.dir = os.path.join(data_dir, "recipes")
        self.bundled = bundled_dir
        self.clock = clock
        self.lock = threading.Lock()
        self._refreshing = {}
        self._defs = None
        self._index = None

    def _path(self, recipe):
        return os.path.join(self.dir, recipe.id + ".json")

    def state(self, recipe):
        """The saved state, else one built from the shipped snapshot."""
        saved = _read(self._path(recipe))
        if isinstance(saved, dict) and (saved.get("current") or {}).get("recipe_key") == recipe_key(recipe):
            return saved
        snapshot = _read(os.path.join(self.bundled, recipe.id + ".json"))
        return {"current": snapshot, "last_good": None, "checked_at": 0, "bad": {}, "warning": None, "retry": False}

    def _due(self, state, force):
        wait = RETRY_AFTER if state.get("retry") else CHECK_EVERY
        return force or not state.get("current") or self.clock() - (state.get("checked_at") or 0) >= wait

    def ensure(self, recipe, force=False):
        """The version to run, checking Comfy for a newer template when due.
        Raises RpcError when there is no usable version at all.

        ``self.lock`` guards only the state files, never a download, so a
        slow check of one recipe does not hold up jobs for the others. A job
        that finds its recipe being checked runs the version it has, unless
        there is none yet, and then waits for the check."""
        with self.lock:
            state = self.state(recipe)
            refreshing = self._refreshing.setdefault(recipe.id, threading.Lock())
        if self._due(state, force):
            if refreshing.acquire(blocking=force or not state.get("current")):
                try:
                    with self.lock:
                        state = self.state(recipe)  # another thread may just have checked
                    if self._due(state, force):
                        state = self._refresh(recipe, state)
                finally:
                    refreshing.release()
        if not state.get("current"):
            raise RpcError(INTERNAL_ERROR, state.get("warning") or f"No {recipe.label} workflow is available")
        return state

    def mark_bad(self, recipe, reason):
        """Comfy refused the current version: go back to the last good one.
        Returns the new state, or None when there is nothing to go back to."""
        with self.lock:
            state = self.state(recipe)
            current, previous = state.get("current"), state.get("last_good")
            if not current or not previous:
                return None
            state.setdefault("bad", {})[current.get("template_sha256")] = reason
            state["warning"] = (f"Comfy refused the latest {recipe.label} workflow ({describe(current)}): {reason}. "
                                f"Using {describe(previous)}.")
            state["current"], state["last_good"] = previous, None
            _write(self._path(recipe), state)
            return state

    def _refresh(self, recipe, state):
        state["checked_at"] = self.clock()
        current = state.get("current") or {}
        try:
            raw = self.client.template(recipe.template)
            sha = hashlib.sha256(raw).hexdigest()
            if sha == current.get("template_sha256") and current.get("recipe_key") == recipe_key(recipe):
                state["warning"], state["retry"] = None, False
            elif sha in (state.get("bad") or {}):
                state["retry"] = False  # already known not to work; keep the warning
            else:
                try:
                    version = self._convert(recipe, raw, sha)
                except (Unsupported, ValueError) as error:
                    state.setdefault("bad", {})[sha] = str(error)
                    state["warning"] = (f"Comfy updated the {recipe.label} workflow but it can't be used yet: {error}. "
                                        f"Using {describe(current)}.") if current else f"Comfy's {recipe.label} workflow can't be used: {error}"
                    state["retry"] = False
                else:
                    if current:
                        state["last_good"] = current
                    state["current"], state["warning"], state["retry"] = version, None, False
        except RpcError as error:
            state["retry"] = True
            state["warning"] = (f"Couldn't check Comfy for a newer {recipe.label} workflow ({error.message}); "
                                f"using {describe(current)}.") if current else f"Couldn't get the {recipe.label} workflow from Comfy: {error.message}"
        with self.lock:
            _write(self._path(recipe), state)
        return state

    def _convert(self, recipe, raw, sha):
        workflow = json.loads(raw)
        outputs = [n for n in workflow.get("nodes") or [] if isinstance(n, dict) and n.get("type") == recipe.output and not n.get("mode")]
        if len(outputs) != 1:
            raise Unsupported(f"the template has {len(outputs)} {recipe.output} nodes, expected one")
        if not isinstance(outputs[0].get("id"), (int, str)):
            raise Unsupported(f"the template's {recipe.output} node has no id")
        output = str(outputs[0]["id"])
        if self._defs is None:
            self._defs = self.client.node_defs()
        graph, specs = to_api(workflow, self._defs, output, recipe.select)
        check(graph, specs, recipe)
        if self._index is None:
            try:
                self._index = self.client.template_index()
            except (RpcError, ValueError):
                self._index = []
        return {
            "template": recipe.template,
            "template_sha256": sha,
            "template_date": template_date(self._index, recipe.template),
            "converted_at": datetime.datetime.fromtimestamp(self.clock(), datetime.timezone.utc).isoformat(timespec="seconds"),
            "output": output,
            "api": graph,
            "specs": target_specs(graph, specs, recipe),
            "recipe_key": recipe_key(recipe),
        }
