"""Tests for the Comfy Cloud plugin's HTTP safety checks, its actions and the
job flow.

Run with the system Python, no packages needed:

    python3 -m unittest discover -s plugins/comfy-cloud
"""
import copy
import email.message
import io
import json
import os
import sys
import tempfile
import tomllib
import unittest
import urllib.error
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import main  # noqa: E402
import comfy_api  # noqa: E402
from catalog import Catalog  # noqa: E402
from recipes import ACTIONS, RECIPES  # noqa: E402
from test_recipes import entry_for  # noqa: E402

BASE = "https://cloud.comfy.org"
HERE = os.path.dirname(os.path.abspath(__file__))
with open(os.path.join(HERE, "plugin.toml"), "rb") as _handle:
    MANIFEST = tomllib.load(_handle)


class Response(io.BytesIO):
    def __init__(self, payload=b"png"):
        super().__init__(payload)
        self.headers = email.message.Message()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False


class DownloadTargets(unittest.TestCase):
    def test_the_key_goes_only_to_the_configured_server_over_https(self):
        self.assertEqual(
            comfy_api.download_target("/api/v2/assets/1/content", BASE),
            (BASE + "/api/v2/assets/1/content", True),
        )
        self.assertEqual(
            comfy_api.download_target(BASE + "/view?x=1", BASE), (BASE + "/view?x=1", True)
        )
        self.assertEqual(
            comfy_api.download_target("https://cloud.comfy.org:443/a", BASE),
            ("https://cloud.comfy.org:443/a", True),
        )

    def test_declared_hosts_are_downloaded_without_the_key(self):
        for url in ("https://abc.run.comfy.app/out.png", "https://storage.googleapis.com/comfy-cloud-assets/x.png?X-Goog-Signature=1"):
            self.assertEqual(comfy_api.download_target(url, BASE), (url, False))

    def test_lookalike_userinfo_and_other_schemes_are_refused(self):
        for url in [
            "https://cloud.comfy.org.evil.com/x",
            "https://cloud.comfy.org@evil.com/x",
            "https://user:pw@cloud.comfy.org/x",
            "https://evil.com/cloud.comfy.org/x",
            "//evil.com/x",
            "http://cloud.comfy.org/x",
            "http://abc.run.comfy.app/out.png",
            "http://storage.googleapis.com/x",
            "https://run.comfy.app.evil.com/x",
            "https://evilrun.comfy.app/x",
            "file:///etc/passwd",
            "ftp://cloud.comfy.org/x",
            "data:text/plain,hi",
        ]:
            with self.subTest(url=url), self.assertRaises(main.RpcError):
                comfy_api.download_target(url, BASE)

    def test_the_host_list_matches_the_manifest(self):
        self.assertEqual(tuple(MANIFEST["permissions"]["network"]), comfy_api.NETWORK)


class Requests(unittest.TestCase):
    def setUp(self):
        self.sent = []
        self.original = comfy_api._opener.open

        def fake_open(request, timeout=None):
            self.sent.append(request)
            return Response()

        comfy_api._opener.open = fake_open
        self.client = comfy_api.Client(BASE, "sk-secret")

    def tearDown(self):
        comfy_api._opener.open = self.original

    def download(self, url):
        with tempfile.TemporaryDirectory() as folder:
            self.client.download({"url": url}, os.path.join(folder, "out.png"))
        return self.sent[-1]

    def test_downloads_attach_the_key_only_for_the_server(self):
        request = self.download("/api/v2/assets/1/content")
        self.assertEqual(request.get_header("Authorization"), "Bearer sk-secret")
        request = self.download("https://abc.run.comfy.app/out.png")
        self.assertIsNone(request.get_header("Authorization"))
        with self.assertRaises(main.RpcError):
            self.download("https://cloud.comfy.org.evil.com/steal")
        with self.assertRaises(main.RpcError):
            self.download("file:///etc/passwd")
        self.assertEqual(len(self.sent), 2)

    def test_templates_are_fetched_without_the_key(self):
        self.client.template("api_ideogram_v4_5_t2i")
        request = self.sent[-1]
        self.assertEqual(request.full_url, "https://cloud.comfy.org/templates/api_ideogram_v4_5_t2i.json")
        self.assertIsNone(request.get_header("Authorization"))

    def test_uploads_send_the_fields_the_api_requires_before_the_file(self):
        with tempfile.TemporaryDirectory() as folder:
            path = os.path.join(folder, "source.png")
            with open(path, "wb") as handle:
                handle.write(b"\x89PNG")
            self.client.upload(path)
        body = self.sent[-1].data.decode("latin-1")
        positions = [body.index(f'name="{field}"') for field in ("content_type", "file_path", "tags", "file")]
        self.assertEqual(positions, sorted(positions))
        self.assertIn('["input"]', body)
        self.assertIn("image/png", body)

    def test_assets_are_referenced_by_id(self):
        self.assertEqual(comfy_api.asset_ref({"id": "a1", "file_path": "input/x.png"}),
                         {"__type": "core/ASSET", "info": {"id": "a1"}})


class Redirects(unittest.TestCase):
    def redirect(self, source, target):
        request = urllib.request.Request(source, headers={"Authorization": "Bearer sk-secret"})
        return comfy_api._Redirects().redirect_request(
            request, io.BytesIO(), 302, "Found", email.message.Message(), target
        )

    def test_the_key_is_dropped_when_a_redirect_leaves_the_server(self):
        same = self.redirect(BASE + "/a", BASE + "/b")
        self.assertEqual(same.get_header("Authorization"), "Bearer sk-secret")
        other = self.redirect(BASE + "/a", "https://storage.example/signed")
        self.assertIsNone(other.get_header("Authorization"))

    def test_redirects_to_plain_http_elsewhere_are_refused(self):
        with self.assertRaises(urllib.error.HTTPError):
            self.redirect(BASE + "/a", "http://storage.example/x")


class Manifest(unittest.TestCase):
    def test_every_action_offers_exactly_its_recipes(self):
        actions = {a["id"]: a for a in MANIFEST["actions"]}
        self.assertEqual(set(actions), set(ACTIONS))
        for action_id, recipe_ids in ACTIONS.items():
            model = next((i for i in actions[action_id].get("inputs", []) if i["id"] == "model"), None)
            if len(recipe_ids) == 1:
                self.assertIsNone(model, action_id)
                continue
            self.assertEqual([v["id"] for v in model["values"]], list(recipe_ids), action_id)
            self.assertEqual(model["default"], recipe_ids[0])

    def test_generate_shapes_have_presets_in_every_model(self):
        aspects = [v["id"] for v in next(i for i in MANIFEST["actions"][0]["inputs"] if i["id"] == "aspect")["values"]]
        for recipe_id in ACTIONS["generate"]:
            recipe = RECIPES[recipe_id]
            options = entry_for(recipe)["specs"][recipe.size][1]["options"]
            for aspect in aspects:
                for tier in ("1K", "2K"):
                    with self.subTest(recipe=recipe_id, aspect=aspect, tier=tier):
                        self.assertTrue(any(o.startswith(f"({tier})") and o.endswith(f"({aspect})") for o in options))


class Provenance(unittest.TestCase):
    def test_the_model_template_and_seed_are_reported_never_the_key(self):
        recipe = RECIPES["seedream-pro"]
        record = main.provenance(recipe, {"template_date": "2026-07-08"}, {"seed": 7, "api_key": "sk-secret"}, "job-1", BASE)
        self.assertEqual(record, {
            "model": "Seedream 5.0 Pro",
            "service": "cloud.comfy.org",
            "request_id": "job-1",
            "seed": 7,
            "extra": {"template": "api_bytedance_seedream_5_0_pro_t2i", "template_date": "2026-07-08"},
        })
        output = main.Job.image("/tmp/a.png", "Generated", provenance=record)
        self.assertNotIn("sk-secret", json.dumps(output))


def read(path):
    with open(path, "r", encoding="utf-8") as handle:
        return handle.read()


class FakeJob:
    def __init__(self, folder, action="generate", inputs=None, source=None):
        self.id = "job-12345678"
        self.action = action
        self.inputs = inputs or {}
        self.source = source
        self.cancelled = False
        self.folder = folder
        self.regions = [v for v in self.inputs.values() if isinstance(v, list)][:1]
        self.regions = self.regions[0] if self.regions else []
        self.messages = []

    @property
    def source_path(self):
        return (self.source or {}).get("path")

    def progress(self, fraction=None, message=None):
        self.messages.append(message)

    def check_cancelled(self):
        pass

    def path(self, name):
        return os.path.join(self.folder, name)


class FakeServer:
    """Answers like Comfy Cloud; ``refuse`` makes the first job fail validation."""

    def __init__(self, refuse=False):
        self.refuse = refuse
        self.png = None  # bytes to download instead of the output's name
        self.base = BASE
        self.submitted = []
        self.uploads = []

    def upload(self, path):
        self.uploads.append(path)
        return {"id": "asset-1"}

    def submit(self, workflow):
        self.submitted.append(workflow)
        return {"id": f"job-{len(self.submitted)}"}

    def job(self, job_id):
        if self.refuse and job_id == "job-1":
            return {"status": "failed", "started_at": None, "outputs": [],
                    "error": {"code": "node_execution_error", "message": "Prompt outputs failed validation: model.size_preset"}}
        saves = [k for k, n in self.submitted[-1].items() if n["class_type"] in ("SaveImageAdvanced", "SaveImage")]
        outputs = [{"node_id": "99", "type": "image", "name": "preview.png", "url": "https://storage.googleapis.com/p"}]
        for node in saves:
            outputs += [{"node_id": node, "type": "image", "name": name, "url": f"https://storage.googleapis.com/{name}"}
                        for name in ("b.png", "a.png")]
        return {"status": "succeeded", "started_at": "now", "outputs": outputs}

    def cancel(self, job_id):
        pass

    def download(self, output, destination):
        if self.png:
            with open(destination, "wb") as handle:
                handle.write(self.png)
            return
        with open(destination, "w", encoding="utf-8") as handle:
            handle.write(output["name"])


class Running(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.server = FakeServer()
        self.original = (main.make_client, main._catalog)
        main.make_client = lambda: self.server
        main._catalog = Catalog(self.server, self.tmp.name, os.path.join(self.tmp.name, "none"), clock=lambda: 0)

    def tearDown(self):
        main.make_client, main._catalog = self.original
        self.tmp.cleanup()

    def install(self, recipe, current, last_good=None):
        state = {"current": current, "last_good": last_good, "checked_at": 0, "bad": {}, "warning": None, "retry": False}
        os.makedirs(os.path.join(self.tmp.name, "recipes"), exist_ok=True)
        with open(os.path.join(self.tmp.name, "recipes", recipe.id + ".json"), "w", encoding="utf-8") as handle:
            json.dump(state, handle)

    def test_generate_runs_the_recipe_and_keeps_only_the_outputs_images(self):
        recipe = RECIPES["seedream-pro"]
        self.install(recipe, entry_for(recipe))
        job = FakeJob(self.tmp.name, inputs={"prompt": "a fox", "model": "seedream-pro", "aspect": "16:9", "resolution": "1K", "seed": 5})
        outputs = main.generate(job)
        self.assertEqual([read(o["path"]) for o in outputs], ["a.png", "b.png"])
        self.assertEqual(outputs[0]["provenance"]["seed"], 5)
        node = next(n for n in self.server.submitted[0].values() if n["class_type"] == "ByteDanceSeedreamNodeV3")
        self.assertEqual(node["inputs"]["model.size_preset"], "(1K) 1312x736 (16:9)")

    def test_a_refused_new_version_falls_back_once_to_the_last_good_one(self):
        self.server.refuse = True
        recipe = RECIPES["seedream-pro"]
        good = entry_for(recipe)
        new = copy.deepcopy(good)
        new["template_sha256"], new["template_date"] = "new", "2026-11-01"
        good["template_sha256"], good["template_date"] = "old", "2026-07-08"
        self.install(recipe, new, good)
        job = FakeJob(self.tmp.name, inputs={"prompt": "a fox", "seed": 1})
        outputs = main.generate(job)
        self.assertEqual(len(self.server.submitted), 2)
        notes = [o["text"] for o in outputs if o["kind"] == "text"]
        self.assertTrue(any("2026-07-08" in n and "failed validation" in n for n in notes), notes)
        self.assertEqual(main._catalog.state(recipe)["current"]["template_sha256"], "old")

    def test_edit_uploads_the_source_and_places_the_result_over_it(self):
        recipe = RECIPES["ideogram-edit"]
        self.install(recipe, entry_for(recipe))
        source = {"path": os.path.join(self.tmp.name, "source.png"), "width": 800, "height": 600}
        job = FakeJob(self.tmp.name, action="edit", inputs={"prompt": "night", "seed": 2}, source=source)
        outputs = main.edit(job)
        self.assertEqual(self.server.uploads, [source["path"]])
        self.assertEqual(outputs[0]["fit"], "source")
        load = next(n for n in self.server.submitted[0].values() if n["class_type"] == "LoadImage")
        self.assertEqual(load["inputs"]["image"], {"__type": "core/ASSET", "info": {"id": "asset-1"}})

    def test_generate_layer_alone_asks_gpt_for_a_transparent_background(self):
        recipe = RECIPES["gpt-flare-layer"]
        self.install(recipe, entry_for(recipe))
        source = {"path": os.path.join(self.tmp.name, "flat.png"), "width": 1152, "height": 864}
        job = FakeJob(self.tmp.name, "generate-layer", {"prompt": "a red kite", "reference": False, "seed": 1}, source)
        outputs = main.generate_layer(job)
        self.assertEqual(self.server.uploads, [])
        node = next(n for n in self.server.submitted[0].values() if n["class_type"] == "OpenAIGPTImageNodeV2")
        self.assertEqual(node["inputs"]["model.background"], "transparent")
        self.assertNotIn("model.images.image_1", node["inputs"])
        self.assertEqual(outputs[0]["fit"], "source")
        self.assertTrue(outputs[0]["name"].startswith("a red kite"))

    def test_generate_layer_draws_into_the_picture_then_lifts_the_object_out(self):
        for recipe_id in ("gpt-sunburst-layer", main.LIFT_RECIPE):
            self.install(RECIPES[recipe_id], entry_for(RECIPES[recipe_id]))
        source = {"path": os.path.join(self.tmp.name, "flat.png"), "width": 1152, "height": 864}
        job = FakeJob(self.tmp.name, "generate-layer",
                      {"prompt": "a red kite", "model": "gpt-sunburst-layer", "seed": 1}, source)
        outputs = main.generate_layer(job)
        scene, lift = self.server.submitted
        gpt = next(n for n in scene.values() if n["class_type"] == "OpenAIGPTImageNodeV2")["inputs"]
        self.assertEqual(gpt["model.background"], "opaque")
        self.assertIn("a red kite", gpt["prompt"])
        self.assertEqual(scene[gpt["model.images.image_1"][0]]["class_type"], "LoadImage")
        separation = next(n for n in lift.values() if n["class_type"] == "ByteDanceSeedreamLayerSeparationNodeV2")["inputs"]
        self.assertIn("a red kite", separation["model.prompt"])
        # The flattened image goes up first, then the picture GPT drew.
        self.assertEqual(self.server.uploads[0], source["path"])
        self.assertTrue(self.server.uploads[1].startswith(self.tmp.name))
        # The first separated image is the background plate; the rest are the object.
        images = [o for o in outputs if o["kind"] == "image"]
        self.assertEqual([read(o["path"]) for o in images], ["b.png"])
        self.assertEqual(images[0]["name"], "a red kite")
        self.assertIn("+", images[0]["provenance"]["model"])

    def test_remove_background_answers_xuan_with_a_mask_comfy_made(self):
        recipe = RECIPES["bria-remove-background"]
        self.install(recipe, entry_for(recipe))
        source = {"path": os.path.join(self.tmp.name, "layer.png"), "width": 2, "height": 1}
        provider = main.remove_background(FakeJob(self.tmp.name, "remove-background", {"capability": "remove_background"}, source))
        self.assertEqual(provider[0]["kind"], "mask")
        self.assertEqual(read(provider[0]["path"]), "a.png")  # only the mask was downloaded
        graph = self.server.submitted[0]
        kinds = {n["class_type"] for n in graph.values()}
        self.assertTrue({"SplitImageWithAlpha", "InvertMask", "MaskToImage"} <= kinds)
        menu = main.remove_background(FakeJob(self.tmp.name, "remove-background", {}, source))
        self.assertEqual((menu[0]["kind"], menu[0]["name"]), ("image", "Cut-out"))
        self.assertNotIn("SplitImageWithAlpha", {n["class_type"] for n in self.server.submitted[1].values()})

    def test_actions_refuse_missing_input_before_anything_is_sent(self):
        source = {"path": "/tmp/x.png", "width": 300, "height": 300}
        cases = [
            (main.generate, FakeJob(self.tmp.name, inputs={"prompt": "  "})),
            (main.edit, FakeJob(self.tmp.name, "edit", {"prompt": "x"})),
            (main.precise_edit, FakeJob(self.tmp.name, "precise-edit", {"regions": [{"index": 1, "x": 0, "y": 0, "width": 9, "height": 9, "fields": {}}]}, source)),
            (main.split_layers, FakeJob(self.tmp.name, "split-layers", {}, source)),
        ]
        for action, job in cases:
            with self.subTest(action=action.__name__), self.assertRaises(main.RpcError):
                action(job)
        self.assertEqual(self.server.submitted, [])
        self.assertEqual(self.server.uploads, [])


class SdkSecrets(unittest.TestCase):
    def test_secrets_never_show_their_values_when_printed(self):
        import xuan_plugin

        plugin = xuan_plugin.Plugin()
        plugin._dispatch("initialize", {"secrets": {"api_key": "sk-live-123"}})
        self.assertEqual(plugin.secrets.get("api_key"), "sk-live-123")
        for text in (repr(plugin.secrets), str(plugin.secrets), f"{plugin.secrets}", repr([plugin.secrets])):
            self.assertNotIn("sk-live", text)
            self.assertIn("api_key", text)


if __name__ == "__main__":
    unittest.main()
