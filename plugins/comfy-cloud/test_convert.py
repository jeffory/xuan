import copy
import json
import os
import sys
import unittest

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from convert import Unsupported, to_api, upstream  # noqa: E402

HERE = os.path.dirname(os.path.abspath(__file__))
DATA = os.path.join(HERE, "testdata")


def load(*parts):
    with open(os.path.join(DATA, *parts), "r", encoding="utf-8") as handle:
        return json.load(handle)


DEFS = load("object_info.json")
ORACLE = {k: v for k, v in load("oracle.json").items() if not k.startswith("_")}
# Inputs whose list values Comfy's summary leaves out (it shows lists as links).
UNREPORTED = {"editor_state", "color_palette"}


def template(name):
    return load("templates", name + ".json")


def save_node(workflow):
    return next(str(n["id"]) for n in workflow["nodes"] if n["type"] == "SaveImageAdvanced")


class TemplatesTest(unittest.TestCase):
    """Every shipped template converts to what Comfy's own converter builds."""

    def test_literals_match_comfy(self):
        for name, expected in ORACLE.items():
            with self.subTest(template=name):
                workflow = template(name)
                graph, _ = to_api(workflow, DEFS, save_node(workflow))
                self.assertEqual(set(graph), set(expected))
                for node_id, node in expected.items():
                    ours = graph[node_id]
                    self.assertEqual(ours["class_type"], node["class_type"])
                    for key, value in node["inputs"].items():
                        self.assertEqual(ours["inputs"].get(key), value, f"{node_id}.{key}")
                    literals = {k for k, v in ours["inputs"].items() if not (isinstance(v, list))}
                    extra = literals - set(node["inputs"]) - UNREPORTED
                    truncated = {"prompt", "model.prompt"}
                    self.assertTrue(extra <= truncated, f"{node_id} has unexpected inputs {extra}")

    def test_links_follow_the_template(self):
        graph, _ = to_api(template("api_ideogram_v4_5_precise_image_edit"), DEFS, "7")
        self.assertEqual(graph["20"]["inputs"]["model.images.image_1"], ["2", 0])
        self.assertEqual(graph["20"]["inputs"]["model.prompt"], ["5", 0])
        self.assertEqual(graph["4"]["inputs"]["background"], ["2", 0])
        self.assertEqual(graph["3"]["inputs"]["element"], ["4", 2])
        self.assertEqual(graph["7"]["inputs"]["images"], ["20", 0])
        separation, _ = to_api(template("api_bytedance_seedream_5_0_layer_separation"), DEFS, "23")
        self.assertEqual(separation["20"]["inputs"], {"images.image0": ["32", 0], "images.image1": ["32", 2]})
        self.assertEqual(separation["31"]["inputs"], {"image": ["20", 0], "alpha": ["22", 0]})

    def test_lists_are_wrapped_and_previews_dropped(self):
        graph, _ = to_api(template("api_ideogram_v4_5_precise_image_edit"), DEFS, "7")
        boxes = graph["4"]["inputs"]["editor_state"]
        self.assertEqual(boxes["__value__"][0]["metadata"]["desc"], "Change the earring to pearl earring")
        self.assertEqual(graph["3"]["inputs"]["color_palette"], {"__value__": []})
        self.assertNotIn("8", graph)  # ImageCompare only previews
        self.assertNotIn("18", graph)  # notes

    def test_specs_describe_inputs(self):
        _, specs = to_api(template("api_bytedance_seedream_5_0_pro_t2i"), DEFS, "2")
        presets = specs["3"]["model.size_preset"][1]["options"]
        self.assertIn("(2K) 2848x1600 (16:9)", presets)
        self.assertEqual(specs["3"]["model.images.image_1"][0], "IMAGE")

    def test_select_switches_a_dynamic_option(self):
        workflow = template("api_bytedance_seedream_5_0_layer_separation")
        graph, _ = to_api(workflow, DEFS, "23", select={"ByteDanceSeedreamLayerSeparationNodeV2.model": "seedream 5.0 pro"})
        inputs = graph["32"]["inputs"]
        self.assertEqual(inputs["model"], "seedream 5.0 pro")
        self.assertEqual(inputs["model.prompt_optimization"], "standard")  # pro only, default
        self.assertEqual(inputs["model.seed"], 498392743)  # kept
        self.assertEqual(inputs["model.image"], ["15", 0])  # link kept


def graph_of(*nodes, links=()):
    return {"nodes": list(nodes), "links": [list(link) for link in links]}


def node(node_id, kind, values=None, inputs=(), mode=0):
    return {"id": node_id, "type": kind, "mode": mode, "inputs": list(inputs), "widgets_values": values}


SIMPLE = {
    "Gen": {"input": {"required": {
        "prompt": ["STRING", {"default": ""}],
        "seed": ["INT", {"default": 0, "control_after_generate": True}],
        "steps": ["INT", {"default": 20}],
        "mode": ["COMBO", {"options": ["a", "b"]}],
    }}, "input_order": {"required": ["prompt", "seed", "steps", "mode"]}},
    "Save": {"input": {"required": {"images": ["IMAGE", {}], "filename_prefix": ["STRING", {"default": "x"}]}}},
    "Sizes": {"input": {"required": {"palette": ["COLORS", {"default": []}]}}},
}


class EdgeCasesTest(unittest.TestCase):
    def convert(self, workflow, defs=SIMPLE, output=2):
        return to_api(workflow, defs, output)[0]

    def basic(self, values, mode=0):
        return graph_of(
            node(1, "Gen", values, mode=mode),
            node(2, "Save", ["out"], [{"name": "images", "link": 10}]),
            node(3, "Note", ["hello"]),
            links=[(10, 1, 0, 2, 0, "IMAGE")],
        )

    def test_control_value_after_seed_is_skipped(self):
        graph = self.convert(self.basic(["cat", 7, "randomize", 30, "b"]))
        self.assertEqual(graph["1"]["inputs"], {"prompt": "cat", "seed": 7, "steps": 30, "mode": "b"})
        self.assertEqual(graph["2"]["inputs"], {"images": ["1", 0], "filename_prefix": "out"})

    def test_trailing_values_are_ignored_and_missing_ones_default(self):
        graph = self.convert(self.basic(["cat", 7, "fixed", 30, "b", "randomize", "stale"]))
        self.assertEqual(graph["1"]["inputs"]["mode"], "b")
        graph = self.convert(self.basic(["cat", 7]))
        self.assertEqual(graph["1"]["inputs"], {"prompt": "cat", "seed": 7, "steps": 20, "mode": "a"})

    def test_shifted_values_are_refused(self):
        with self.assertRaises(Unsupported):
            self.convert(self.basic([7, "cat", "fixed", 30, "b"]))
        with self.assertRaises(Unsupported):
            self.convert(self.basic(["cat", 7, "fixed", 30, "c"]))

    def test_bypassed_muted_unknown_and_subgraph_nodes_are_refused(self):
        for mode in (2, 4):
            with self.assertRaises(Unsupported):
                self.convert(self.basic(["cat", 7, "fixed", 30, "b"], mode=mode))
        workflow = self.basic(["cat", 7, "fixed", 30, "b"])
        workflow["nodes"][0]["type"] = "GetNode"
        with self.assertRaises(Unsupported):
            self.convert(workflow)
        workflow["nodes"][0]["type"] = "9f1c-uuid"
        workflow["definitions"] = {"subgraphs": [{"id": "9f1c-uuid"}]}
        with self.assertRaises(Unsupported):
            self.convert(workflow)

    def test_reroute_and_primitive_nodes(self):
        workflow = graph_of(
            node(1, "Gen", ["cat", 7, "fixed", 30, "b"], [{"name": "steps", "widget": {"name": "steps"}, "link": 12}]),
            node(4, "Reroute", None, [{"name": "", "link": 11}]),
            node(5, "PrimitiveNode", [30]),
            node(2, "Save", ["out"], [{"name": "images", "link": 10}]),
            links=[(11, 1, 0, 4, 0, "IMAGE"), (10, 4, 0, 2, 0, "IMAGE"), (12, 5, 0, 1, 2, "INT")],
        )
        graph = self.convert(workflow)
        self.assertEqual(graph["2"]["inputs"]["images"], ["1", 0])
        self.assertEqual(graph["1"]["inputs"]["steps"], 30)
        self.assertEqual(set(graph), {"1", "2"})

    def test_widget_fed_by_a_link_uses_the_link(self):
        defs = copy.deepcopy(SIMPLE)
        defs["Text"] = {"input": {"required": {"value": ["STRING", {"default": ""}]}}}
        workflow = graph_of(
            node(1, "Gen", ["ignored", 7, "fixed", 30, "b"], [{"name": "prompt", "widget": {"name": "prompt"}, "link": 13}]),
            node(6, "Text", ["hello"]),
            node(2, "Save", ["out"], [{"name": "images", "link": 10}]),
            links=[(10, 1, 0, 2, 0, "IMAGE"), (13, 6, 0, 1, 0, "STRING")],
        )
        graph = self.convert(workflow, defs)
        self.assertEqual(graph["1"]["inputs"]["prompt"], ["6", 0])
        self.assertEqual(graph["6"]["inputs"], {"value": "hello"})

    def test_upstream_prunes_detached_nodes(self):
        graph = {
            "1": {"class_type": "A", "inputs": {}},
            "2": {"class_type": "B", "inputs": {"x": ["1", 0]}},
            "3": {"class_type": "C", "inputs": {"x": ["1", 0]}},
        }
        self.assertEqual(set(upstream(graph, "2")), {"1", "2"})


if __name__ == "__main__":
    unittest.main()
