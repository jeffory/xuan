"""Tests for the Python SDK's output helpers.

    python3 -m unittest discover -s sdk/python
"""
import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from xuan_plugin import Job  # noqa: E402


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


if __name__ == "__main__":
    unittest.main()
