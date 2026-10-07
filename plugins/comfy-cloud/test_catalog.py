import json
import os
import sys
import tempfile
import threading
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "sdk", "python"))
sys.path.insert(0, HERE)

from catalog import CHECK_EVERY, RETRY_AFTER, Catalog, recipe_key  # noqa: E402
from recipes import RECIPES  # noqa: E402
from xuan_plugin import INTERNAL_ERROR, RpcError  # noqa: E402

DATA = os.path.join(HERE, "testdata")
with open(os.path.join(DATA, "object_info.json"), "rb") as handle:
    DEFS = json.load(handle)
RECIPE = RECIPES["seedream-pro"]
INDEX = [{"templates": [{"name": RECIPE.template, "date": "2026-07-08"}]}]


def template_bytes(name=RECIPE.template):
    with open(os.path.join(DATA, "templates", name + ".json"), "rb") as handle:
        return handle.read()


def changed(raw, seed):
    workflow = json.loads(raw)
    node = next(n for n in workflow["nodes"] if n["type"] == "ByteDanceSeedreamNodeV3")
    node["widgets_values"][6] = seed
    return json.dumps(workflow).encode()


class FakeClient:
    def __init__(self):
        self.raw = template_bytes()
        self.fail = None
        self.calls = {"template": 0, "node_defs": 0}

    def template(self, name):
        self.calls["template"] += 1
        if self.fail:
            raise RpcError(INTERNAL_ERROR, self.fail)
        return self.raw

    def node_defs(self):
        self.calls["node_defs"] += 1
        return DEFS

    def template_index(self):
        return INDEX


class CatalogTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.data = os.path.join(self.tmp.name, "data")
        self.bundled = os.path.join(self.tmp.name, "snapshots")
        self.now = 1_000_000.0
        self.client = FakeClient()
        self.catalog = self.make()

    def tearDown(self):
        self.tmp.cleanup()

    def make(self, bundled=None):
        return Catalog(self.client, self.data, bundled or self.bundled, clock=lambda: self.now)

    def seed_of(self, state):
        api = state["current"]["api"]
        return next(n for n in api.values() if n["class_type"] == "ByteDanceSeedreamNodeV3")["inputs"]["model.seed"]

    def ship_snapshot(self):
        """Ship what a first conversion produces, as tools/refresh_snapshots.py does."""
        first = self.make(os.path.join(self.tmp.name, "none")).ensure(RECIPE)
        os.makedirs(self.bundled)
        with open(os.path.join(self.bundled, RECIPE.id + ".json"), "w", encoding="utf-8") as handle:
            json.dump(first["current"], handle)
        os.remove(os.path.join(self.data, "recipes", RECIPE.id + ".json"))
        self.client.calls = {"template": 0, "node_defs": 0}
        self.catalog = self.make()

    def test_first_conversion_records_the_template(self):
        state = self.catalog.ensure(RECIPE)
        version = state["current"]
        self.assertEqual(version["template_date"], "2026-07-08")
        self.assertEqual(version["output"], "2")
        self.assertIn("ByteDanceSeedreamNodeV3.model.size_preset", version["specs"])
        self.assertIsNone(state["warning"])

    def test_the_shipped_snapshot_is_used_when_the_template_is_unchanged(self):
        self.ship_snapshot()
        state = self.catalog.ensure(RECIPE)
        self.assertEqual(self.client.calls, {"template": 1, "node_defs": 0})
        self.assertEqual(state["current"]["template_date"], "2026-07-08")

    def test_checks_at_most_once_a_day(self):
        self.catalog.ensure(RECIPE)
        self.now += CHECK_EVERY - 1
        self.catalog.ensure(RECIPE)
        self.assertEqual(self.client.calls["template"], 1)
        self.now += 2
        self.catalog.ensure(RECIPE)
        self.assertEqual(self.client.calls["template"], 2)
        self.catalog.ensure(RECIPE, force=True)
        self.assertEqual(self.client.calls["template"], 3)

    def test_a_changed_template_is_converted_and_the_old_version_kept(self):
        self.catalog.ensure(RECIPE)
        self.client.raw = changed(self.client.raw, 1234)
        state = self.catalog.ensure(RECIPE, force=True)
        self.assertEqual(self.seed_of(state), 1234)
        self.assertEqual(self.seed_of({"current": state["last_good"]}), 42)

    def test_a_broken_update_keeps_the_working_version_and_is_not_retried(self):
        self.catalog.ensure(RECIPE)
        workflow = json.loads(self.client.raw)
        next(n for n in workflow["nodes"] if n["type"] == "ByteDanceSeedreamNodeV3")["type"] = "GetNode"
        self.client.raw = json.dumps(workflow).encode()
        state = self.catalog.ensure(RECIPE, force=True)
        self.assertEqual(self.seed_of(state), 42)
        self.assertIn("can't be used yet", state["warning"])
        self.assertIn("2026-07-08", state["warning"])
        calls = self.client.calls["node_defs"]
        self.catalog = self.make()  # a new process converts nothing it already refused
        self.catalog.ensure(RECIPE, force=True)
        self.assertEqual(self.client.calls["node_defs"], calls)

    def test_network_errors_keep_the_version_and_retry_sooner(self):
        self.catalog.ensure(RECIPE)
        self.client.fail = "cannot reach cloud.comfy.org"
        state = self.catalog.ensure(RECIPE, force=True)
        self.assertEqual(self.seed_of(state), 42)
        self.assertIn("cannot reach", state["warning"])
        self.client.fail = None
        self.now += RETRY_AFTER
        state = self.catalog.ensure(RECIPE)
        self.assertIsNone(state["warning"])

    def test_a_version_converted_for_other_recipe_inputs_is_not_used(self):
        # The plugin now sets inputs the cached conversion has no definitions for.
        self.ship_snapshot()
        self.catalog.ensure(RECIPE)
        path = os.path.join(self.data, "recipes", RECIPE.id + ".json")
        with open(path, "r", encoding="utf-8") as handle:
            state = json.load(handle)
        state["current"]["recipe_key"] = "older-recipe"
        state["current"]["specs"] = {}
        with open(path, "w", encoding="utf-8") as handle:
            json.dump(state, handle)
        state = self.make().state(RECIPE)
        self.assertEqual(state["current"]["recipe_key"], recipe_key(RECIPE))
        self.assertIn("ByteDanceSeedreamNodeV3.model.width", state["current"]["specs"])

    def test_nothing_usable_raises(self):
        self.client.fail = "offline"
        with self.assertRaises(RpcError):
            self.catalog.ensure(RECIPE)

    def test_a_refused_version_falls_back_and_stays_refused(self):
        self.catalog.ensure(RECIPE)
        self.client.raw = changed(self.client.raw, 99)
        self.catalog.ensure(RECIPE, force=True)
        state = self.catalog.mark_bad(RECIPE, "Value not in list")
        self.assertEqual(self.seed_of(state), 42)
        self.assertIn("Value not in list", state["warning"])
        state = self.catalog.ensure(RECIPE, force=True)
        self.assertEqual(self.seed_of(state), 42)
        self.assertIsNone(self.catalog.mark_bad(RECIPE, "again"))  # nothing older to go back to

    def test_an_output_node_without_an_id_is_refused_not_a_crash(self):
        workflow = json.loads(self.client.raw)
        output = next(n for n in workflow["nodes"] if n["type"] == RECIPE.output)
        del output["id"]
        self.client.raw = json.dumps(workflow).encode()
        with self.assertRaises(RpcError) as caught:
            self.catalog.ensure(RECIPE)
        self.assertIn("no id", caught.exception.message)

    def test_a_slow_check_holds_up_neither_other_recipes_nor_jobs_with_a_version(self):
        self.catalog.ensure(RECIPE)
        other = RECIPES["seedream-flash"]
        started, release = threading.Event(), threading.Event()
        fetch, raw = self.client.template, self.client.raw

        def slow(name):
            if name == RECIPE.template:
                started.set()
                release.wait(5)
                self.client.calls["template"] += 1
                return raw
            return fetch(name)

        self.client.template = slow
        checker = threading.Thread(target=self.catalog.ensure, args=(RECIPE,), kwargs={"force": True})
        checker.start()
        try:
            self.assertTrue(started.wait(5))
            self.client.raw = template_bytes(other.template)
            results = []

            def jobs():
                results.append(self.catalog.ensure(other))  # another recipe, while the check waits
                self.now += CHECK_EVERY
                results.append(self.catalog.ensure(RECIPE))  # due, but being checked: runs what it has

            runner = threading.Thread(target=jobs)
            runner.start()
            runner.join(2)
            self.assertFalse(release.is_set())
            self.assertEqual(len(results), 2, "a job waited for another recipe's check")
            self.assertTrue(all(state["current"] for state in results))
        finally:
            release.set()
            checker.join(5)


if __name__ == "__main__":
    unittest.main()
