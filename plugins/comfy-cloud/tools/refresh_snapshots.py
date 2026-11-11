#!/usr/bin/env python3
"""Regenerate the converted workflows the plugin ships (``snapshots/``) from
Comfy's current templates, and with ``--testdata`` the test fixtures too.

    python3 plugins/comfy-cloud/tools/refresh_snapshots.py [--testdata]

Needs a Comfy API key for the node definitions: ``COMFY_API_KEY``, or the one
Xuan stores for the plugin in ``secrets.toml``. The key is never printed.
``testdata/oracle.json`` (what Comfy's own converter builds) is not
regenerated: refresh it by hand from the comfy-cloud MCP's
``get_template_schema`` when a template changes.
"""
import argparse
import json
import os
import sys
import tempfile
import tomllib

PLUGIN = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(PLUGIN, "..", "..", "sdk", "python"))
sys.path.insert(0, PLUGIN)

from catalog import Catalog  # noqa: E402
from comfy_api import Client  # noqa: E402
from recipes import RECIPES  # noqa: E402


def api_key():
    if os.environ.get("COMFY_API_KEY"):
        return os.environ["COMFY_API_KEY"]
    config = os.environ.get("XDG_CONFIG_HOME") or os.path.expanduser("~/.config")
    try:
        with open(os.path.join(config, "xuan", "secrets.toml"), "rb") as handle:
            return tomllib.load(handle)["comfy-cloud"]["api_key"]
    except (OSError, KeyError, tomllib.TOMLDecodeError):
        sys.exit("Set COMFY_API_KEY or enter the key in Xuan's Comfy Cloud plugin settings.")


def write(path, data):
    os.makedirs(os.path.dirname(path), exist_ok=True)
    with open(path, "w", encoding="utf-8") as handle:
        json.dump(data, handle, indent=1)
        handle.write("\n")


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--testdata", action="store_true", help="also refresh testdata/templates and testdata/object_info.json")
    args = parser.parse_args()
    client = Client("https://cloud.comfy.org", api_key())
    with tempfile.TemporaryDirectory() as scratch:
        catalog = Catalog(client, scratch, os.path.join(scratch, "none"))
        for recipe in RECIPES.values():
            state = catalog.ensure(recipe, force=True)
            if state.get("warning"):
                sys.exit(f"{recipe.id}: {state['warning']}")
            write(os.path.join(PLUGIN, "snapshots", recipe.id + ".json"), state["current"])
            print(f"{recipe.id}: {recipe.template} ({state['current']['template_date']})")
        if args.testdata:
            classes = set()
            for template in sorted({r.template for r in RECIPES.values()}):
                raw = client.template(template)
                with open(os.path.join(PLUGIN, "testdata", "templates", template + ".json"), "wb") as handle:
                    handle.write(raw)
                classes |= {n.get("type") for n in json.loads(raw).get("nodes") or []}
            defs = catalog._defs or client.node_defs()
            subset = {name: defs[name] for name in sorted(c for c in classes if c in defs)}
            if "LoadImage" in subset:  # its options list the account's own uploads
                subset["LoadImage"]["input"]["required"]["image"][0] = ["example.png"]
            write(os.path.join(PLUGIN, "testdata", "object_info.json"), subset)
            print("testdata refreshed; check testdata/oracle.json by hand")


if __name__ == "__main__":
    main()
