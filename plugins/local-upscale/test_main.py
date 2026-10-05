"""Tests for the Local Upscale example.

Run with the system Python, no packages needed:

    python3 -m unittest discover -s plugins/local-upscale
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
import upscale  # noqa: E402
from xuan_plugin import Cancelled, Job, NeedsSetup, RpcError, decode_png, encode_png  # noqa: E402


def nothing(*args):
    return None


def run(width, height, rgba, scale=2, sharpen=0.0):
    return upscale.lanczos_backend(width, height, rgba, scale, nothing, nothing, sharpen=sharpen)


def pixels(width, height, make):
    out = bytearray()
    for y in range(height):
        for x in range(width):
            out += bytes(make(x, y))
    return bytes(out)


def at(rgba, width, x, y):
    i = (y * width + x) * 4
    return tuple(rgba[i : i + 4])


class Resampling(unittest.TestCase):
    def test_output_is_the_requested_size(self):
        w, h, out = run(5, 3, pixels(5, 3, lambda x, y: (x * 40, y * 80, 7, 255)), scale=3)
        self.assertEqual((w, h, len(out)), (15, 9, 15 * 9 * 4))

    def test_a_flat_colour_stays_flat(self):
        w, h, out = run(4, 4, pixels(4, 4, lambda x, y: (10, 120, 250, 255)), scale=2, sharpen=1.0)
        self.assertEqual({at(out, w, x, y) for x in range(w) for y in range(h)}, {(10, 120, 250, 255)})

    def test_a_gradient_stays_in_order_and_in_range(self):
        w, h, out = run(8, 2, pixels(8, 2, lambda x, y: (x * 30, x * 30, x * 30, 255)), scale=2)
        row = [at(out, w, x, 0)[0] for x in range(w)]
        self.assertEqual(row, sorted(row))
        self.assertLess(row[0], 20)
        self.assertGreater(row[-1], 190)

    def test_transparent_pixels_do_not_bleed_their_colour(self):
        # An opaque red pixel next to a transparent pixel that hides bright green.
        data = pixels(2, 1, lambda x, y: (255, 0, 0, 255) if x == 0 else (0, 255, 0, 0))
        w, _, out = run(2, 1, data, scale=2)
        for x in range(w):
            r, g, b, a = at(out, w, x, 0)
            if a > 0:
                self.assertEqual((r, g, b), (255, 0, 0), f"column {x} picked up the hidden green")

    def test_alpha_is_resampled(self):
        data = pixels(4, 1, lambda x, y: (0, 0, 0, 255 if x < 2 else 0))
        w, _, out = run(4, 1, data, scale=2)
        alphas = [at(out, w, x, 0)[3] for x in range(w)]
        self.assertGreater(alphas[0], 230)
        self.assertLess(alphas[-1], 25)

    def test_sharpening_increases_edge_contrast(self):
        data = pixels(8, 8, lambda x, y: (60, 60, 60, 255) if x < 4 else (200, 200, 200, 255))
        w, _, soft = run(8, 8, data, scale=2, sharpen=0.0)
        _, _, sharp = run(8, 8, data, scale=2, sharpen=1.0)
        # Just either side of the edge, between columns 7 and 8.
        self.assertGreater(at(sharp, w, 8, 4)[0] - at(sharp, w, 7, 4)[0], at(soft, w, 8, 4)[0] - at(soft, w, 7, 4)[0])

    def test_cancelling_stops_the_work(self):
        def cancelled():
            raise Cancelled()

        with self.assertRaises(Cancelled):
            upscale.lanczos_backend(2, 2, bytes(16), 2, nothing, cancelled)

    def test_progress_reaches_one(self):
        seen = []
        upscale.lanczos_backend(3, 3, pixels(3, 3, lambda x, y: (1, 2, 3, 255)), 2, seen.append, nothing, sharpen=0.5)
        self.assertEqual(seen[-1], 1.0)
        self.assertEqual(seen, sorted(seen))


class Backends(unittest.TestCase):
    def test_lanczos_is_built_in(self):
        self.assertIs(upscale.load_backend("lanczos"), upscale.lanczos_backend)

    def test_a_backend_file_next_to_the_plugin_is_loaded(self):
        with tempfile.TemporaryDirectory() as folder:
            with open(os.path.join(folder, "backend_double.py"), "w") as handle:
                handle.write(
                    "def upscale(width, height, rgba, scale, progress, check):\n"
                    "    return width * scale, height * scale, bytes(width * scale * height * scale * 4)\n"
                )
            backend = upscale.load_backend("double", folder)
            self.assertEqual(backend(1, 1, bytes(4), 2, nothing, nothing), (2, 2, bytes(16)))

    def test_unknown_and_unsafe_names_are_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            with self.assertRaises(FileNotFoundError):
                upscale.load_backend("missing", folder)
            for name in ["../evil", "a/b", "", "UPPER", "x.y"]:
                with self.assertRaises(ValueError, msg=name):
                    upscale.load_backend(name, folder)


class Action(unittest.TestCase):
    def job(self, folder, width=4, height=3, **inputs):
        source = os.path.join(folder, "source.png")
        with open(source, "wb") as handle:
            handle.write(encode_png(width, height, pixels(width, height, lambda x, y: (x * 50 % 256, y * 50 % 256, 9, 255))))
        return Job(
            main.plugin,
            {
                "job": "j1",
                "action": "upscale",
                "inputs": inputs,
                "work_dir": folder,
                "source": {"path": source, "width": width, "height": height, "scale": 1.0},
            },
        )

    def setUp(self):
        main.plugin.settings = {}
        main.plugin.host.notify = lambda *args, **kwargs: None

    def test_the_action_writes_a_new_document_of_the_right_size(self):
        with tempfile.TemporaryDirectory() as folder:
            outputs = main.upscale_action(self.job(folder, scale="3", sharpen=0.4))
            self.assertEqual(outputs[0]["kind"], "document")
            with open(outputs[0]["path"], "rb") as handle:
                width, height, _ = decode_png(handle.read())
            self.assertEqual((width, height), (12, 9))
            self.assertEqual(outputs[1]["kind"], "text")

    def test_bad_inputs_fall_back_to_safe_values(self):
        with tempfile.TemporaryDirectory() as folder:
            outputs = main.upscale_action(self.job(folder, scale="banana", sharpen="x"))
            with open(outputs[0]["path"], "rb") as handle:
                width, height, _ = decode_png(handle.read())
            self.assertEqual((width, height), (8, 6))

    def test_results_over_the_setting_are_refused_before_any_work(self):
        main.plugin.settings = {"max_output_megapixels": 1}
        with tempfile.TemporaryDirectory() as folder:
            with self.assertRaises(RpcError):
                main.upscale_action(self.job(folder, width=600, height=600, scale="2"))

    def test_a_missing_backend_asks_for_setup(self):
        main.plugin.settings = {"backend": "realesrgan"}
        with tempfile.TemporaryDirectory() as folder:
            with self.assertRaises(NeedsSetup):
                main.upscale_action(self.job(folder))


class Manifest(unittest.TestCase):
    def test_it_asks_for_no_network_and_no_secrets(self):
        with open(os.path.join(HERE, "plugin.toml"), "rb") as handle:
            manifest = tomllib.load(handle)
        permissions = manifest.get("permissions", {})
        self.assertNotIn("network", permissions)
        self.assertNotIn("secrets", permissions)
        self.assertEqual(permissions.get("document", "read"), "read")
        self.assertEqual(permissions.get("filesystem", "none"), "none")


if __name__ == "__main__":
    unittest.main()
