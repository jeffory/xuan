import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from convert import Unsupported, to_api  # noqa: E402
from recipes import ACTIONS, RECIPES, apply, bounding_boxes, check, fit_size, pick_preset, recipe_for, target_specs  # noqa: E402

DATA = os.path.join(os.path.dirname(os.path.abspath(__file__)), "testdata")
with open(os.path.join(DATA, "object_info.json"), "r", encoding="utf-8") as handle:
    DEFS = json.load(handle)
ASSET = {"__type": "core/ASSET", "info": {"id": "asset-1"}}


def entry_for(recipe):
    """What the catalog keeps for a recipe, built from the fixtures."""
    with open(os.path.join(DATA, "templates", recipe.template + ".json"), "r", encoding="utf-8") as handle:
        workflow = json.load(handle)
    output = next(str(n["id"]) for n in workflow["nodes"] if n["type"] == recipe.output)
    graph, specs = to_api(workflow, DEFS, output, recipe.select)
    check(graph, specs, recipe)
    return {"api": graph, "specs": target_specs(graph, specs, recipe), "output": output}


def node_of(graph, kind):
    return next(n for n in graph.values() if n["class_type"] == kind)


class RecipesTest(unittest.TestCase):
    def test_every_recipe_fits_its_template(self):
        for recipe in RECIPES.values():
            with self.subTest(recipe=recipe.id):
                entry_for(recipe)

    def test_every_action_names_known_recipes(self):
        for action, ids in ACTIONS.items():
            for recipe_id in ids:
                self.assertIn(recipe_id, RECIPES, action)
        self.assertEqual(recipe_for("generate", "nonsense").id, "seedream-pro")
        self.assertEqual(recipe_for("edit", "seedream-pro-edit").id, "seedream-pro-edit")

    def test_generate_sets_prompt_seed_and_preset(self):
        recipe = RECIPES["seedream-pro"]
        graph = apply(entry_for(recipe), recipe, {"prompt": "a red fox", "seed": 7, "aspect": "16:9", "tier": "2K"})
        inputs = node_of(graph, "ByteDanceSeedreamNodeV3")["inputs"]
        self.assertEqual(inputs["prompt"], "a red fox")
        self.assertEqual(inputs["model.seed"], 7)
        self.assertEqual(inputs["model.size_preset"], "(2K) 2848x1600 (16:9)")
        self.assertIs(inputs["model.watermark"], False)
        ideogram = RECIPES["ideogram"]
        graph = apply(entry_for(ideogram), ideogram, {"prompt": "a poster", "seed": 1, "aspect": "3:4", "tier": "1K"})
        self.assertEqual(node_of(graph, "IdeogramTextToImageApi")["inputs"]["model.size"], "(1K) 864x1152 (3:4)")

    def test_edit_rewires_the_source_and_drops_what_it_replaced(self):
        recipe = RECIPES["ideogram-edit"]
        graph = apply(entry_for(recipe), recipe, {"prompt": "make it night", "seed": 3, "image": ASSET})
        kinds = sorted(n["class_type"] for n in graph.values())
        self.assertEqual(kinds, ["IdeogramEditApi", "LoadImage", "SaveImageAdvanced"])
        edit = node_of(graph, "IdeogramEditApi")["inputs"]
        self.assertEqual(edit["model.prompt"], "make it night")
        self.assertEqual(edit["model.size"], "source")
        self.assertEqual(graph[edit["model.images.image_1"][0]]["inputs"]["image"], ASSET)

        seedream = RECIPES["seedream-pro-edit"]
        graph = apply(entry_for(seedream), seedream, {"prompt": "x", "seed": 3, "image": ASSET, "source_size": (1600, 900)})
        self.assertNotIn("Painter", {n["class_type"] for n in graph.values()})
        inputs = node_of(graph, "ByteDanceSeedreamNodeV3")["inputs"]
        self.assertEqual(inputs["model.size_preset"], "Custom")
        self.assertAlmostEqual(inputs["model.width"] / inputs["model.height"], 16 / 9, places=2)

    def test_precise_edit_sends_boxes_in_a_16_pixel_grid(self):
        recipe = RECIPES["ideogram-precise"]
        regions = [{"index": 1, "x": 100, "y": 50, "width": 200, "height": 100, "fields": {"desc": "add a hat", "type": "obj"}},
                   {"index": 2, "x": 0, "y": 0, "width": 50, "height": 20, "fields": {"desc": "", "type": "text", "text": "SALE"}}]
        values = {"seed": 5, "image": ASSET, "source_size": (1000, 500), "regions": regions, "quality": "low", "background": ""}
        graph = apply(entry_for(recipe), recipe, values)
        boxes_node = node_of(graph, "CreateBoundingBoxes")["inputs"]
        self.assertEqual((boxes_node["width"], boxes_node["height"]), (992, 496))
        boxes = boxes_node["editor_state"]["__value__"]
        self.assertEqual(boxes[0]["metadata"]["desc"], "add a hat")
        self.assertEqual(boxes[1]["metadata"], {"type": "text", "text": "SALE", "desc": "", "palette": []})
        self.assertEqual(boxes[0]["x"], 99)
        source = [k for k, n in graph.items() if n["class_type"] == "LoadImage"]
        self.assertEqual(len(source), 1)
        self.assertEqual(boxes_node["background"], [source[0], 0])
        self.assertEqual(node_of(graph, "IdeogramPreciseEditApi")["inputs"]["model.quality"], "low")

    def test_split_layers_pro_has_its_own_inputs(self):
        recipe = RECIPES["split-pro"]
        graph = apply(entry_for(recipe), recipe, {"prompt": "", "seed": 9, "image": ASSET})
        inputs = node_of(graph, "ByteDanceSeedreamLayerSeparationNodeV2")["inputs"]
        self.assertEqual(inputs["model"], "seedream 5.0 pro")
        self.assertEqual(inputs["model.prompt"], "")
        self.assertIs(inputs["model.crop_layers"], False)
        self.assertIn("model.prompt_optimization", inputs)

    def test_generate_layer_is_transparent_and_follows_the_canvas(self):
        recipe = RECIPES["gpt-flare-layer"]
        values = {"prompt": "a red kite", "seed": 4, "quality": "medium", "source_size": (1152, 864)}
        graph = apply(entry_for(recipe), recipe, values)
        inputs = node_of(graph, "OpenAIGPTImageNodeV2")["inputs"]
        self.assertEqual(inputs["model.background"], "transparent")
        self.assertEqual(inputs["model.size"], "Custom")
        width, height = inputs["model.custom_width"], inputs["model.custom_height"]
        self.assertEqual((width % 16, height % 16), (0, 0))
        self.assertAlmostEqual(width / height, 4 / 3, places=2)
        self.assertEqual(inputs["model.quality"], "medium")
        self.assertNotIn("model.images.image_1", inputs)  # no reference sent
        self.assertNotIn("LoadImage", {n["class_type"] for n in graph.values()})

    def test_generate_layer_can_take_the_flattened_image_as_a_reference(self):
        recipe = RECIPES["gpt-sunburst-layer"]
        values = {"prompt": "a kite", "seed": 4, "source_size": (800, 800), "reference": ASSET}
        graph = apply(entry_for(recipe), recipe, values)
        inputs = node_of(graph, "OpenAIGPTImageNodeV2")["inputs"]
        self.assertEqual(graph[inputs["model.images.image_1"][0]]["inputs"]["image"], ASSET)

    def test_remove_background_sends_the_layer_to_bria(self):
        recipe = RECIPES["bria-remove-background"]
        graph = apply(entry_for(recipe), recipe, {"image": ASSET})
        self.assertEqual(sorted(n["class_type"] for n in graph.values()), ["BriaRemoveImageBackground", "LoadImage", "SaveImage"])

    def test_a_template_without_the_bound_input_is_refused(self):
        recipe = RECIPES["seedream-pro"]
        entry = entry_for(recipe)
        del entry["api"][next(k for k, n in entry["api"].items() if n["class_type"] == "ByteDanceSeedreamNodeV3")]
        with self.assertRaises(Unsupported):
            apply(entry, recipe, {"prompt": "x"})


class SnapshotsTest(unittest.TestCase):
    """The converted workflows the plugin ships for its first run."""

    def test_every_recipe_ships_a_snapshot_that_runs(self):
        import hashlib

        for recipe in RECIPES.values():
            with self.subTest(recipe=recipe.id):
                with open(os.path.join(os.path.dirname(DATA), "snapshots", recipe.id + ".json"), "r", encoding="utf-8") as handle:
                    snapshot = json.load(handle)
                self.assertEqual(snapshot["template"], recipe.template)
                values = {"prompt": "x", "seed": 1, "image": ASSET, "aspect": "1:1", "tier": "1K", "source_size": (800, 600),
                          "regions": [{"x": 1, "y": 1, "width": 10, "height": 10, "fields": {"desc": "x"}}], "quality": "low", "background": ""}
                graph = apply(snapshot, recipe, values)
                self.assertIn(snapshot["output"], graph)
                with open(os.path.join(DATA, "templates", recipe.template + ".json"), "rb") as handle:
                    raw = handle.read()
                if hashlib.sha256(raw).hexdigest() == snapshot["template_sha256"]:
                    self.assertEqual(snapshot["api"], entry_for(recipe)["api"])


class SizesTest(unittest.TestCase):
    OPTIONS = ["(1K) 1024x1024 (1:1)", "(2K) 2048x2048 (1:1)", "(2K) 2848x1600 (16:9)", "(2K) 1600x2848 (9:16)", "Custom"]

    def test_pick_preset(self):
        self.assertEqual(pick_preset(self.OPTIONS, "16:9", "2K"), "(2K) 2848x1600 (16:9)")
        self.assertEqual(pick_preset(self.OPTIONS, "1:1", "1K"), "(1K) 1024x1024 (1:1)")
        self.assertEqual(pick_preset(self.OPTIONS, "1920:1080", "2K"), "(2K) 2848x1600 (16:9)")  # closest shape
        self.assertIsNone(pick_preset(["auto", "source"], "16:9", "2K"))
        self.assertEqual(pick_preset(["auto", "1024x1024", "1536x1024", "1024x1536"], "3:2", "2K"), "1536x1024")  # plain sizes

    def test_fit_size(self):
        w_spec = ["INT", {"min": 1024, "max": 4514, "step": 2}]
        self.assertEqual(fit_size(1000, 1000, w_spec, w_spec), (2048, 2048))
        w, h = fit_size(3000, 1000, w_spec, w_spec)
        self.assertTrue(1024 <= h and w <= 4514 and abs(w / h - 3) < 0.05)
        self.assertIsNone(fit_size(5000, 500, w_spec, w_spec))  # 10:1 cannot fit 1024..4514

    def test_bounding_boxes_tiny_source(self):
        w, h, boxes = bounding_boxes([], 20, 20)
        self.assertEqual((w, h, boxes), (64, 64, []))


if __name__ == "__main__":
    unittest.main()
