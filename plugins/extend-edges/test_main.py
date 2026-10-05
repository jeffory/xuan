"""Tests for the Extend Edges example.

Run with the system Python, no packages needed:

    python3 -m unittest discover -s plugins/extend-edges
"""
import os
import sys
import tempfile
import tomllib
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, os.path.join(HERE, "..", "..", "sdk", "python"))
sys.path.insert(0, HERE)

import fill  # noqa: E402
import main  # noqa: E402
from xuan_plugin import Cancelled, Job, NeedsSetup, RpcError, decode_png, encode_gray_png, encode_png  # noqa: E402


def nothing(*args):
    return None


def padded(width, height, box, colour):
    """An extended source: transparent, with ``colour(x, y)`` inside ``box``,
    and its new-area mask."""
    x0, y0, x1, y1 = box
    rgba = bytearray(width * height * 4)
    mask = bytearray([255]) * (width * height)
    for y in range(y0, y1):
        for x in range(x0, x1):
            i = y * width + x
            rgba[i * 4:i * 4 + 4] = bytes(colour(x - x0, y - y0))
            mask[i] = 0
    return bytes(rgba), bytes(mask)


def red(value):
    return (value, 0, 0, 255)


def reds(rgba, width, y):
    return [rgba[(y * width + x) * 4] for x in range(width)]


class Fill(unittest.TestCase):
    # Old image 3x1 at x 2..5 of a 7x3 source, with red 10, 20, 30.
    def source(self):
        return padded(7, 3, (2, 1, 5, 2), lambda x, y: red(10 * (x + 1)))

    def test_the_old_area_is_found_from_the_mask(self):
        rgba, mask = self.source()
        self.assertEqual(fill.old_area(7, 3, mask), (2, 1, 5, 2))
        self.assertIsNone(fill.old_area(2, 2, bytes([255]) * 4))

    def test_mirror_reflects_the_edges(self):
        rgba, mask = self.source()
        out = fill.edges_backend(7, 3, rgba, mask, {"fill": "mirror"}, nothing, nothing)
        self.assertEqual(reds(out, 7, 1), [20, 10, 10, 20, 30, 30, 20])
        # Rows above and below mirror the single old row.
        self.assertEqual(reds(out, 7, 0), reds(out, 7, 1))
        self.assertTrue(all(out[i] == 255 for i in range(3, len(out), 4)))

    def test_repeat_copies_the_edge_pixels(self):
        rgba, mask = self.source()
        out = fill.edges_backend(7, 3, rgba, mask, {"fill": "repeat"}, nothing, nothing)
        self.assertEqual(reds(out, 7, 2), [10, 10, 10, 20, 30, 30, 30])

    def test_the_old_image_is_left_alone(self):
        rgba, mask = self.source()
        out = fill.edges_backend(7, 3, rgba, mask, {}, nothing, nothing)
        self.assertEqual(out[(7 + 2) * 4:(7 + 5) * 4], rgba[(7 + 2) * 4:(7 + 5) * 4])

    def test_mirroring_wraps_past_a_narrow_image(self):
        self.assertEqual([fill.mirror(i, 0, 2) for i in range(-4, 6)], [0, 1, 1, 0, 0, 1, 1, 0, 0, 1])

    def test_nothing_old_is_an_error(self):
        with self.assertRaises(ValueError):
            fill.edges_backend(1, 1, bytes(4), bytes([255]), {}, nothing, nothing)

    def test_cancelling_stops_the_work(self):
        def cancelled():
            raise Cancelled()

        rgba, mask = self.source()
        with self.assertRaises(Cancelled):
            fill.edges_backend(7, 3, rgba, mask, {}, nothing, cancelled)

    def test_progress_reaches_one(self):
        rgba, mask = padded(4, 40, (1, 1, 3, 39), lambda x, y: red(9))
        seen = []
        fill.edges_backend(4, 40, rgba, mask, {}, seen.append, nothing)
        self.assertEqual(seen[-1], 1.0)
        self.assertEqual(seen, sorted(seen))


class Backends(unittest.TestCase):
    def test_edges_is_built_in(self):
        self.assertIs(fill.load_backend("edges"), fill.edges_backend)

    def test_a_backend_file_next_to_the_plugin_is_loaded(self):
        with tempfile.TemporaryDirectory() as folder:
            with open(os.path.join(folder, "backend_white.py"), "w") as handle:
                handle.write(
                    "def outpaint(width, height, rgba, mask, options, progress, check):\n"
                    "    return bytes([255]) * (width * height * 4)\n"
                )
            backend = fill.load_backend("white", folder)
            self.assertEqual(backend(1, 1, bytes(4), b"\xff", {}, nothing, nothing), b"\xff" * 4)

    def test_unknown_and_unsafe_names_are_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            with self.assertRaises(FileNotFoundError):
                fill.load_backend("missing", folder)
            for name in ["../evil", "a/b", "", "UPPER", "x.y"]:
                with self.assertRaises(ValueError, msg=name):
                    fill.load_backend(name, folder)


class Action(unittest.TestCase):
    def job(self, folder, extend=True, **inputs):
        """A job for a 3x2 document extended by 1 on the left and right and
        2 at the bottom: a 5x4 source."""
        rgba, mask = padded(5, 4, (1, 0, 4, 2), lambda x, y: red(50 * (x + 1)))
        source = os.path.join(folder, "source.png")
        with open(source, "wb") as handle:
            handle.write(encode_png(5, 4, rgba))
        extend_mask = os.path.join(folder, "extend.png")
        with open(extend_mask, "wb") as handle:
            handle.write(encode_gray_png(5, 4, mask))
        params = {"path": source, "width": 5, "height": 4, "scale": 1.0, "mask": None}
        if extend:
            params["extend"] = {"left": 1, "top": 0, "right": 1, "bottom": 2}
            params["extend_mask"] = extend_mask
        return Job(
            main.plugin,
            {
                "job": "j1",
                "action": "outpaint",
                "inputs": inputs,
                "work_dir": folder,
                "source": params,
                "document": {"width": 3, "height": 2, "layers": []},
            },
        )

    def setUp(self):
        main.plugin.settings = {}
        main.plugin.host.notify = lambda *args, **kwargs: None

    def test_the_result_extends_the_canvas_and_covers_it(self):
        with tempfile.TemporaryDirectory() as folder:
            job = self.job(folder, fill="repeat")
            edit, image, text = main.outpaint(job)
            self.assertEqual(
                edit,
                {"kind": "edit", "edits": [{"op": "extend_canvas", "left": 1, "top": 0, "right": 1, "bottom": 2}]},
            )
            self.assertEqual((image["kind"], image["fit"], image["mask"]), ("image", "source", job.extend_mask_path))
            self.assertEqual(text, {"kind": "text", "text": "Extended the canvas to 5x4"})
            with open(image["path"], "rb") as handle:
                width, height, rgba = decode_png(handle.read())
            self.assertEqual((width, height), (5, 4))
            self.assertEqual(reds(rgba, 5, 0), [50, 50, 100, 150, 150])
            self.assertEqual(reds(rgba, 5, 3), [50, 50, 100, 150, 150])

    def test_an_unknown_fill_mirrors(self):
        with tempfile.TemporaryDirectory() as folder:
            _, image, _ = main.outpaint(self.job(folder, fill="smear"))
            with open(image["path"], "rb") as handle:
                width, height, rgba = decode_png(handle.read())
            self.assertEqual(reds(rgba, 5, 3), [50, 50, 100, 150, 150])
            self.assertEqual(reds(rgba, 5, 2), [50, 50, 100, 150, 150])

    def test_a_source_that_was_not_extended_is_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            with self.assertRaises(RpcError):
                main.outpaint(self.job(folder, extend=False))

    def test_a_missing_backend_asks_for_setup(self):
        main.plugin.settings = {"backend": "sdxl"}
        with tempfile.TemporaryDirectory() as folder:
            with self.assertRaises(NeedsSetup):
                main.outpaint(self.job(folder))

    def test_a_backend_returning_the_wrong_size_is_refused(self):
        with tempfile.TemporaryDirectory() as folder:
            original = fill.BUILTIN["edges"]
            fill.BUILTIN["edges"] = lambda *args: b"\x00"
            try:
                with self.assertRaises(RpcError):
                    main.outpaint(self.job(folder))
            finally:
                fill.BUILTIN["edges"] = original

    def test_no_source_is_an_error(self):
        job = Job(main.plugin, {"job": "j", "action": "outpaint", "inputs": {}})
        with self.assertRaises(RpcError):
            main.outpaint(job)


class Sdk(unittest.TestCase):
    def test_extend_canvas_edits(self):
        self.assertEqual(
            Job.extend_canvas(left=4, bottom=2),
            {"op": "extend_canvas", "left": 4, "top": 0, "right": 0, "bottom": 2},
        )
        for bad in [-1, 1.5, "4", True]:
            with self.assertRaises(ValueError, msg=repr(bad)):
                Job.extend_canvas(top=bad)

    def test_a_job_reads_the_extension_from_its_source(self):
        job = Job(main.plugin, {"source": {"path": "/s.png", "extend": {"left": 3, "bottom": 1}, "extend_mask": "/e.png"}})
        self.assertEqual(job.extension, {"left": 3, "top": 0, "right": 0, "bottom": 1})
        self.assertEqual(job.extend_mask_path, "/e.png")
        plain = Job(main.plugin, {"source": {"path": "/s.png", "extend": None}})
        self.assertIsNone(plain.extension)
        self.assertIsNone(plain.extend_mask_path)
        self.assertIsNone(Job(main.plugin, {}).extension)


class Manifest(unittest.TestCase):
    def test_it_edits_the_document_but_asks_for_no_network_or_secrets(self):
        with open(os.path.join(HERE, "plugin.toml"), "rb") as handle:
            manifest = tomllib.load(handle)
        permissions = manifest.get("permissions", {})
        self.assertNotIn("network", permissions)
        self.assertNotIn("secrets", permissions)
        self.assertEqual(permissions.get("document"), "edit")
        self.assertEqual(permissions.get("filesystem", "none"), "none")
        action = manifest["actions"][0]
        self.assertEqual(action["source"]["from"], "composite")
        self.assertEqual(set(action["source"]["extend"].values()), {"amount"})
        fills = [value["id"] for value in action["inputs"][1]["values"]]
        self.assertEqual(tuple(fills), fill.FILLS)


if __name__ == "__main__":
    unittest.main()
