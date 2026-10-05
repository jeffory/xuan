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


if __name__ == "__main__":
    unittest.main()
