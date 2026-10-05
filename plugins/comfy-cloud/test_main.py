"""Tests for the Comfy Cloud example's HTTP safety checks.

Run with the system Python, no packages needed:

    python3 -m unittest discover -s plugins/comfy-cloud
"""
import email.message
import io
import os
import sys
import tempfile
import tomllib
import unittest
import urllib.error
import urllib.request

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import main  # noqa: E402

BASE = "https://cloud.comfy.org"


class Response(io.BytesIO):
    def __init__(self, payload=b"png"):
        super().__init__(payload)
        self.headers = email.message.Message()

    def __enter__(self):
        return self

    def __exit__(self, *args):
        return False


def client(key="sk-secret", base=BASE):
    client = object.__new__(main.Client)
    client.base = base
    client.key = key
    return client


class DownloadTargets(unittest.TestCase):
    def test_the_key_goes_only_to_the_configured_server_over_https(self):
        self.assertEqual(
            main.download_target("/api/v2/assets/1/content", BASE),
            (BASE + "/api/v2/assets/1/content", True),
        )
        self.assertEqual(
            main.download_target(BASE + "/view?x=1", BASE), (BASE + "/view?x=1", True)
        )
        self.assertEqual(
            main.download_target("https://cloud.comfy.org:443/a", BASE),
            ("https://cloud.comfy.org:443/a", True),
        )

    def test_declared_hosts_are_downloaded_without_the_key(self):
        url = "https://abc.run.comfy.app/out.png"
        self.assertEqual(main.download_target(url, BASE), (url, False))

    def test_lookalike_userinfo_and_other_schemes_are_refused(self):
        for url in [
            "https://cloud.comfy.org.evil.com/x",
            "https://cloud.comfy.org@evil.com/x",
            "https://user:pw@cloud.comfy.org/x",
            "https://evil.com/cloud.comfy.org/x",
            "//evil.com/x",
            "http://cloud.comfy.org/x",
            "http://abc.run.comfy.app/out.png",
            "https://run.comfy.app.evil.com/x",
            "https://evilrun.comfy.app/x",
            "file:///etc/passwd",
            "ftp://cloud.comfy.org/x",
            "data:text/plain,hi",
        ]:
            with self.subTest(url=url), self.assertRaises(main.RpcError):
                main.download_target(url, BASE)

    def test_the_host_list_matches_the_manifest(self):
        with open(os.path.join(os.path.dirname(main.__file__), "plugin.toml"), "rb") as handle:
            manifest = tomllib.load(handle)
        self.assertEqual(tuple(manifest["permissions"]["network"]), main.NETWORK)


class Inpaint(unittest.TestCase):
    def test_the_manifest_asks_for_the_selection_mask(self):
        with open(os.path.join(os.path.dirname(main.__file__), "plugin.toml"), "rb") as handle:
            manifest = tomllib.load(handle)
        action = next(a for a in manifest["actions"] if a["id"] == "inpaint")
        self.assertEqual(action["source"]["mask"], "selection")
        self.assertEqual(action["source"]["from"], "composite")

    def test_the_mask_node_gets_the_uploaded_mask(self):
        workflow = main.load_workflow("inpaint")
        filled = main.fill(workflow, {"prompt": "a hat", "seed": 7}, "src-asset", "mask-asset")
        by_title = {n["_meta"]["title"]: n for n in filled.values()}
        self.assertEqual(by_title["Xuan Source"]["inputs"]["image"], "src-asset")
        self.assertEqual(by_title["Xuan Mask"]["inputs"]["image"], "mask-asset")
        self.assertEqual(by_title["Xuan Prompt"]["inputs"]["value"], "a hat")
        # Without a mask the template is left alone.
        plain = main.fill(workflow, {}, "src-asset")
        self.assertEqual(
            next(n for n in plain.values() if n["_meta"]["title"] == "Xuan Mask")["inputs"]["image"],
            "selection.png",
        )

    def test_the_result_is_masked_by_the_selection(self):
        self.assertEqual(main.image_output("a.png", "X", mask="m.png")["mask"], "m.png")
        self.assertNotIn("mask", main.image_output("a.png", "X"))


class Requests(unittest.TestCase):
    def setUp(self):
        self.sent = []
        self.original = main._opener.open

        def fake_open(request, timeout=None):
            self.sent.append(request)
            return Response()

        main._opener.open = fake_open

    def tearDown(self):
        main._opener.open = self.original

    def download(self, url):
        with tempfile.TemporaryDirectory() as folder:
            client().download({"url": url}, os.path.join(folder, "out.png"))
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


class Redirects(unittest.TestCase):
    def redirect(self, source, target):
        request = urllib.request.Request(source, headers={"Authorization": "Bearer sk-secret"})
        return main._Redirects().redirect_request(
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
