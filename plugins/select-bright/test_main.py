"""Tests for the Select Bright Areas example.

Run with the system Python, no packages needed:

    python3 -m unittest discover -s plugins/select-bright
"""
import os
import sys
import tempfile
import tomllib
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "sdk", "python"))
sys.path.insert(0, HERE)

import main  # noqa: E402
import segment  # noqa: E402
from xuan_plugin import Cancelled, Job, NeedsSetup, RpcError, decode_png, encode_gray_png, encode_png  # noqa: E402


def nothing(*args):
    return None


def pixels(width, height, make):
    out = bytearray()
    for y in range(height):
        for x in range(width):
            out += bytes(make(x, y))
    return bytes(out)


def grey(value, alpha=255):
    return (value, value, value, alpha)


def run(width, height, rgba, threshold=0.5, softness=0.0):
    options = {"threshold": threshold, "softness": softness}
    return segment.luminance_backend(width, height, rgba, options, nothing, nothing)


class Luminance(unittest.TestCase):
    def test_a_hard_threshold_selects_bright_pixels_only(self):
        data = pixels(4, 1, lambda x, y: grey([0, 100, 160, 255][x]))
        self.assertEqual(list(run(4, 1, data, threshold=0.5)), [0, 0, 255, 255])

    def test_softness_gives_partial_coverage_around_the_threshold(self):
        data = pixels(3, 1, lambda x, y: grey([64, 128, 192][x]))
        mask = run(3, 1, data, threshold=0.5, softness=0.4)
        self.assertEqual(mask[0], 0)
        self.assertTrue(100 < mask[1] < 155, mask[1])
        self.assertEqual(mask[2], 255)

    def test_green_counts_more_than_blue(self):
        data = pixels(2, 1, lambda x, y: (0, 200, 0, 255) if x == 0 else (0, 0, 200, 255))
        self.assertEqual(list(run(2, 1, data, threshold=0.4)), [255, 0])

    def test_transparent_pixels_are_not_selected(self):
        data = pixels(2, 1, lambda x, y: grey(255, 0 if x == 0 else 255))
        self.assertEqual(list(run(2, 1, data)), [0, 255])

    def test_cancelling_stops_the_work(self):
        def cancelled():
            raise Cancelled()

        with self.assertRaises(Cancelled):
            segment.luminance_backend(2, 2, bytes(16), {}, nothing, cancelled)

    def test_progress_reaches_one(self):
        seen = []
        segment.luminance_backend(3, 40, bytes(3 * 40 * 4), {}, seen.append, nothing)
        self.assertEqual(seen[-1], 1.0)
        self.assertEqual(seen, sorted(seen))


class Backends(unittest.TestCase):
    def test_luminance_is_built_in(self):
        self.assertIs(segment.load_backend("luminance"), segment.luminance_backend)

    def test_a_backend_file_next_to_the_plugin_is_loaded(self):
        with tempfile.TemporaryDirectory() as folder:
            with open(os.path.join(folder, "backend_all.py"), "w") as handle:
                handle.write(
                    "def segment(width, height, rgba, options, progress, check):\n"
                    "    return bytes([255]) * (width * height)\n"
                )
            backend = segment.load_backend("all", folder)
            self.assertEqual(backend(2, 1, bytes(8), {}, nothing, nothing), b"\xff\xff")

    def test_unknown_and_unsafe_names_are_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            with self.assertRaises(FileNotFoundError):
                segment.load_backend("missing", folder)
            for name in ["../evil", "a/b", "", "UPPER", "x.y"]:
                with self.assertRaises(ValueError, msg=name):
                    segment.load_backend(name, folder)


class Action(unittest.TestCase):
    def job(self, folder, width=4, height=2, **inputs):
        source = os.path.join(folder, "source.png")
        with open(source, "wb") as handle:
            handle.write(encode_png(width, height, pixels(width, height, lambda x, y: grey(255 if x < width // 2 else 0))))
        return Job(
            main.plugin,
            {
                "job": "j1",
                "action": "select-bright",
                "inputs": inputs,
                "work_dir": folder,
                "source": {"path": source, "width": width, "height": height, "scale": 1.0},
            },
        )

    def setUp(self):
        main.plugin.settings = {}
        main.plugin.host.notify = lambda *args, **kwargs: None

    def test_the_action_returns_a_mask_fitted_to_the_source(self):
        with tempfile.TemporaryDirectory() as folder:
            outputs = main.select_bright(self.job(folder, mode="add", threshold=50, softness=0))
            mask = outputs[0]
            self.assertEqual((mask["kind"], mask["mode"], mask["fit"]), ("mask", "add", "source"))
            with open(mask["path"], "rb") as handle:
                width, height, rgba = decode_png(handle.read())
            self.assertEqual((width, height), (4, 2))
            self.assertEqual([rgba[i * 4] for i in range(4)], [255, 255, 0, 0])
            self.assertEqual(outputs[1], {"kind": "text", "text": "Selected about 50% of the image"})

    def test_bad_inputs_fall_back_to_safe_values(self):
        with tempfile.TemporaryDirectory() as folder:
            outputs = main.select_bright(self.job(folder, mode="xor", threshold="x", softness=float("nan")))
            self.assertEqual(outputs[0]["mode"], "replace")

    def test_a_missing_backend_asks_for_setup(self):
        main.plugin.settings = {"backend": "u2net"}
        with tempfile.TemporaryDirectory() as folder:
            with self.assertRaises(NeedsSetup):
                main.select_bright(self.job(folder))

    def test_a_backend_returning_the_wrong_size_is_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            original = segment.BUILTIN["luminance"]
            segment.BUILTIN["luminance"] = lambda *args: b"\x00"
            try:
                with self.assertRaises(RpcError):
                    main.select_bright(self.job(folder))
            finally:
                segment.BUILTIN["luminance"] = original

    def test_no_source_is_an_error(self):
        job = Job(main.plugin, {"job": "j", "action": "select-bright", "inputs": {}})
        with self.assertRaises(RpcError):
            main.select_bright(job)


class Sdk(unittest.TestCase):
    def test_mask_outputs_and_grey_pngs(self):
        self.assertEqual(
            Job.mask("/m.png", mode="intersect", width=10),
            {"kind": "mask", "path": "/m.png", "mode": "intersect", "x": 0, "y": 0, "width": 10},
        )
        with self.assertRaises(ValueError):
            Job.mask("/m.png", mode="xor")
        width, height, rgba = decode_png(encode_gray_png(2, 1, bytes([7, 200])))
        self.assertEqual((width, height, rgba), (2, 1, bytes([7, 7, 7, 255, 200, 200, 200, 255])))
        with self.assertRaises(ValueError):
            encode_gray_png(2, 2, bytes(3))


class Manifest(unittest.TestCase):
    def test_it_asks_for_no_network_no_secrets_and_no_edits(self):
        with open(os.path.join(HERE, "plugin.toml"), "rb") as handle:
            manifest = tomllib.load(handle)
        permissions = manifest.get("permissions", {})
        self.assertNotIn("network", permissions)
        self.assertNotIn("secrets", permissions)
        self.assertEqual(permissions.get("document", "read"), "read")
        self.assertEqual(permissions.get("filesystem", "none"), "none")
        modes = [value["id"] for value in manifest["actions"][0]["inputs"][2]["values"]]
        self.assertEqual(tuple(modes), Job.MASK_MODES)

    def test_it_provides_select_subject_through_its_action(self):
        with open(os.path.join(HERE, "plugin.toml"), "rb") as handle:
            manifest = tomllib.load(handle)
        provides = manifest["provides"]
        self.assertEqual(provides, [{"capability": "select_subject", "action": "select-bright"}])
        actions = {action["id"]: action for action in manifest["actions"]}
        self.assertEqual(actions["select-bright"]["kind"], "edit")
        self.assertEqual(actions["select-bright"]["source"]["from"], "composite")


if __name__ == "__main__":
    unittest.main()
