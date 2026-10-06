"""Tests for the Python SDK's output helpers.

    python3 -m unittest discover -s sdk/python
"""
import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from xuan_plugin import Job, NEEDS_SETUP, NeedsSetup, Plugin  # noqa: E402


class Provenance(unittest.TestCase):
    RECORD = {"model": "sdxl", "seed": 7, "steps": 30, "cfg": 7.5, "extra": {"lora": "a"}}

    def test_images_and_documents_carry_provenance_as_given(self):
        image = Job.image("/tmp/a.png", name="A", provenance=self.RECORD)
        self.assertEqual(image["provenance"], self.RECORD)
        self.assertEqual(json.loads(json.dumps(image))["provenance"]["extra"], {"lora": "a"})
        document = Job.new_document("/tmp/d.png", "D", provenance=self.RECORD)
        self.assertEqual(document["provenance"], self.RECORD)
        self.assertEqual(document["kind"], "document")

    def test_outputs_without_provenance_have_no_key(self):
        self.assertNotIn("provenance", Job.image("/tmp/a.png"))
        self.assertNotIn("provenance", Job.image("/tmp/a.png", provenance={}))
        self.assertNotIn("provenance", Job.new_document("/tmp/d.png"))
        self.assertNotIn("provenance", Job.mask("/tmp/m.png"))


class Models(unittest.TestCase):
    def test_paths_come_from_initialize_and_models_changed(self):
        plugin = Plugin()
        plugin._dispatch(
            "initialize",
            {"models_dir": "/d/models", "models": {"net": "/d/models/net.onnx", "bad": 3}},
        )
        self.assertEqual(plugin.models_dir, "/d/models")
        self.assertEqual(plugin.model_path("net"), "/d/models/net.onnx")
        self.assertIsNone(plugin.model_path("bad"))
        job = Job(plugin, {"job": "j", "action": "a"})
        self.assertEqual(job.model_path("net"), "/d/models/net.onnx")
        with self.assertRaises(NeedsSetup) as raised:
            job.model_path("extra")
        self.assertEqual(raised.exception.code, NEEDS_SETUP)
        self.assertIn("Models", raised.exception.message)
        plugin._handle_notification(
            {"method": "models/changed", "params": {"models": {"extra": "/d/models/extra.bin"}}}
        )
        self.assertEqual(job.model_path("extra"), "/d/models/extra.bin")
        self.assertIsNone(plugin.model_path("net"))


class FakeTransport:
    def __init__(self, answers):
        self.answers = answers
        self.sent = []

    def request(self, method, params, timeout):
        self.sent.append((method, params))
        return self.answers.get(method)


class HostRequests(unittest.TestCase):
    def test_edits_documents_and_files_use_the_documented_methods(self):
        from xuan_plugin import Host

        transport = FakeTransport({
            "document/edit": {"ok": True, "layers": ["a", 3, "b"]},
            "document/list": {"documents": [{"id": "d"}]},
            "file/save_as": {"path": "/home/u/x.xuan"},
            "file/export": {"path": "/home/u/x.png"},
            "file/open": {"ok": True, "document": "e"},
        })
        host = Host(transport)
        self.assertEqual(host.edit("Name", [{"op": "add_empty_layer"}]), ["a", "b"])
        self.assertEqual(host.list_documents(), [{"id": "d"}])
        host.activate_document("d")
        self.assertEqual(host.save_as(suggested_name="x"), "/home/u/x.xuan")
        self.assertEqual(host.export_file("png"), "/home/u/x.png")
        self.assertEqual(host.open_file("/home/u/x.png")["document"], "e")
        methods = [method for method, _ in transport.sent]
        self.assertEqual(
            methods,
            ["document/edit", "document/list", "document/activate", "file/save_as", "file/export", "file/open"],
        )
        self.assertEqual(transport.sent[4][1]["format"], "png")
        self.assertNotIn("session", transport.sent[0][1])

    def test_a_session_host_names_its_session(self):
        from xuan_plugin import Host

        transport = FakeTransport({"session/status": {"edits": "ask"}})
        host = Host(transport).with_session("client-1")
        self.assertEqual(host.session_status(), {"edits": "ask"})
        host.edit("x", [])
        host.request("document/get")
        self.assertEqual([params["session"] for _, params in transport.sent], ["client-1"] * 3)


if __name__ == "__main__":
    unittest.main()
