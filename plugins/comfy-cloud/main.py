#!/usr/bin/env python3
"""Comfy Cloud plugin for Xuan: runs API-format ComfyUI workflows through
Comfy API v2 (https://docs.comfy.org/api-reference/v2/overview).

Workflows live in the ``workflows`` folder next to this file, or in the
plugin's data directory, as exported by ComfyUI's *Export (API)*. The plugin
fills them in by node title; see README.md.
"""
import copy
import io
import json
import mimetypes
import os
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import uuid

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "sdk", "python"))
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from xuan_plugin import (  # noqa: E402
    INSUFFICIENT_CREDITS,
    INTERNAL_ERROR,
    INVALID_PARAMS,
    RATE_LIMITED,
    Cancelled,
    Job,
    NeedsSetup,
    Plugin,
    RpcError,
    ui,
)

plugin = Plugin()
HERE = os.path.dirname(os.path.abspath(__file__))
TERMINAL = {"completed", "success", "succeeded", "failed", "error", "cancelled", "canceled", "lost", "non_retryable_error"}
history = []  # recent jobs for the pane
history_lock = threading.Lock()


# --- HTTP -----------------------------------------------------------------

# Hosts the manifest declares under permissions.network; outputs are only
# downloaded from these (or from the configured server).
NETWORK = ("cloud.comfy.org", "*.run.comfy.app")


def host_allowed(host, patterns=NETWORK):
    """Whether ``host`` is one of ``patterns``; ``*.x`` matches subdomains of x."""
    host = (host or "").lower().rstrip(".")
    if not host:
        return False
    for pattern in patterns:
        pattern = pattern.lower()
        if pattern.startswith("*."):
            if host.endswith(pattern[1:]) and len(host) > len(pattern) - 1:
                return True
        elif host == pattern:
            return True
    return False


def _origin(parsed):
    port = parsed.port or {"https": 443, "http": 80}.get(parsed.scheme)
    return (parsed.scheme, (parsed.hostname or "").lower(), port)


def same_server(url, base):
    """Whether ``url`` is on exactly the configured server: same scheme, host
    and port, and no user info (``https://server@evil.example`` is evil.example)."""
    target = urllib.parse.urlsplit(url)
    if target.username is not None or target.password is not None:
        return False
    return bool(target.hostname) and _origin(target) == _origin(urllib.parse.urlsplit(base))


def download_target(url, base, patterns=NETWORK):
    """Resolve an output URL. Returns ``(url, send_key)``: the API key goes only
    to the configured server over https; other https hosts the manifest declares
    get no key; anything else (other hosts, http, file:, …) is refused."""
    if url.startswith("/") and not url.startswith("//"):
        url = base + url
    target = urllib.parse.urlsplit(url)
    if same_server(url, base) and target.scheme in ("https", "http"):
        return url, target.scheme == "https"
    if (
        target.scheme == "https"
        and target.username is None
        and target.password is None
        and host_allowed(target.hostname, patterns)
    ):
        return url, False
    raise RpcError(INTERNAL_ERROR, f"refusing to download from {url[:200]}")


class _Redirects(urllib.request.HTTPRedirectHandler):
    """Follow redirects only to https (or the same server), and never carry the
    API key to another host."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        new = super().redirect_request(req, fp, code, msg, headers, newurl)
        if new is None:
            return None
        source = urllib.parse.urlsplit(req.full_url)
        target = urllib.parse.urlsplit(newurl)
        if target.scheme != "https" and _origin(target) != _origin(source):
            raise urllib.error.HTTPError(newurl, code, "refusing a non-https redirect", headers, fp)
        if _origin(target) != _origin(source):
            new.remove_header("Authorization")
        return new


_opener = urllib.request.build_opener(_Redirects)


class Client:
    def __init__(self):
        self.base = (plugin.settings.get("base_url") or "https://cloud.comfy.org").rstrip("/")
        self.key = plugin.secrets.get("api_key") or ""
        if not self.key and "cloud.comfy.org" in self.base:
            raise NeedsSetup("Enter your Comfy API key in the plugin settings.")

    def _request(self, method, path, body=None, headers=None, raw=False, url=None, send_key=True):
        url = url or self.base + path
        data = None
        hdrs = {"Accept": "application/json"}
        if self.key and send_key and same_server(url, self.base):
            hdrs["Authorization"] = "Bearer " + self.key
        if body is not None and not raw:
            data = json.dumps(body).encode("utf-8")
            hdrs["Content-Type"] = "application/json"
        elif raw:
            data = body
        hdrs.update(headers or {})
        request = urllib.request.Request(url, data=data, method=method, headers=hdrs)
        try:
            with _opener.open(request, timeout=120) as response:
                payload = response.read()
                if response.headers.get("Content-Type", "").startswith("application/json"):
                    return json.loads(payload or b"null")
                return payload
        except urllib.error.HTTPError as error:
            payload = error.read()
            try:
                detail = json.loads(payload)
            except ValueError:
                detail = {"message": payload.decode("utf-8", "replace")[:300]}
            message = detail.get("error", detail).get("message") if isinstance(detail.get("error", detail), dict) else str(detail)
            code = {402: INSUFFICIENT_CREDITS, 429: RATE_LIMITED}.get(error.code, INTERNAL_ERROR)
            data = None
            if error.code == 429 and error.headers.get("Retry-After"):
                try:
                    data = {"retry_after": float(error.headers["Retry-After"])}
                except ValueError:
                    data = None
            if error.code == 422 and isinstance(detail, dict):
                message = f"{message or 'invalid workflow'}: {json.dumps(detail)[:800]}"
            raise RpcError(code, f"HTTP {error.code}: {message or error.reason}", data)
        except urllib.error.URLError as error:
            raise RpcError(INTERNAL_ERROR, f"cannot reach {self.base}: {error.reason}")

    def upload(self, path):
        """POST /api/v2/assets (multipart); returns the asset record."""
        boundary = "----xuan" + uuid.uuid4().hex
        name = os.path.basename(path)
        content_type = mimetypes.guess_type(name)[0] or "application/octet-stream"
        with open(path, "rb") as handle:
            file_bytes = handle.read()
        body = io.BytesIO()
        for field, value in (("name", name), ("tags", "input")):
            body.write(f"--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"\r\n\r\n{value}\r\n".encode())
        body.write(f"--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: {content_type}\r\n\r\n".encode())
        body.write(file_bytes)
        body.write(f"\r\n--{boundary}--\r\n".encode())
        return self._request("POST", "/api/v2/assets", body.getvalue(), {"Content-Type": f"multipart/form-data; boundary={boundary}"}, raw=True)

    def submit(self, workflow):
        extra = {"api_key_comfy_org": self.key} if self.key else {}
        return self._request("POST", "/api/v2/jobs", {"workflow": workflow, "extra_data": extra}, {"Idempotency-Key": uuid.uuid4().hex})

    def job(self, job_id):
        return self._request("GET", f"/api/v2/jobs/{job_id}")

    def cancel(self, job_id):
        try:
            self._request("POST", f"/api/v2/jobs/{job_id}/cancel", {})
        except RpcError:
            pass

    def download(self, output, destination):
        url = output.get("url") or output.get("download_url")
        asset = output.get("asset_id") or output.get("id")
        if not url and asset:
            url = self.base + f"/api/v2/assets/{asset}/content"
        if not url:
            raise RpcError(INTERNAL_ERROR, f"output without a url: {json.dumps(output)[:200]}")
        url, send_key = download_target(url, self.base)
        payload = self._request("GET", None, raw=True, url=url, send_key=send_key)
        with open(destination, "wb") as handle:
            handle.write(payload)


# --- Workflows ------------------------------------------------------------


def load_workflow(name):
    for folder in (plugin.data_dir, os.path.join(HERE, "workflows")):
        path = os.path.join(folder, name if name.endswith(".json") else name + ".json")
        if os.path.isfile(path):
            with open(path, "r", encoding="utf-8") as handle:
                workflow = json.load(handle)
            if "nodes" in workflow and "links" in workflow:
                raise RpcError(INVALID_PARAMS, f"{path} is a UI export; use Export (API) in ComfyUI")
            return workflow
    raise NeedsSetup(f"Workflow {name} not found in {os.path.join(HERE, 'workflows')} or {plugin.data_dir}")


def fill(workflow, values, source_ref=None, mask_ref=None):
    """Set node inputs by title ("Xuan Prompt", "Xuan Source", …) and expand
    {{placeholders}} in every string input."""
    workflow = copy.deepcopy(workflow)
    for node in workflow.values():
        if not isinstance(node, dict):
            continue
        title = (node.get("_meta") or {}).get("title", "")
        inputs = node.setdefault("inputs", {})
        lowered = title.lower()
        if source_ref is not None and lowered in ("xuan source", "load image") and "image" in inputs:
            inputs["image"] = source_ref
        if mask_ref is not None and lowered == "xuan mask" and "image" in inputs:
            inputs["image"] = mask_ref
        if lowered == "xuan prompt":
            for key in ("text", "prompt", "string", "value"):
                if key in inputs and isinstance(inputs[key], str):
                    inputs[key] = values.get("prompt", "")
        if lowered == "xuan seed":
            for key in ("seed", "noise_seed", "value"):
                if key in inputs:
                    inputs[key] = int(values.get("seed", 0))
        if lowered == "xuan size":
            for key, name in (("width", "width"), ("height", "height")):
                if key in inputs:
                    inputs[key] = int(values.get(name, inputs[key]))
        for key, value in list(inputs.items()):
            if isinstance(value, str) and "{{" in value:
                for name, replacement in values.items():
                    value = value.replace("{{" + name + "}}", str(replacement))
                inputs[key] = value
    return workflow


def _number(value, integer=False):
    """``value`` as a finite number (numeric strings count), else None."""
    if isinstance(value, bool):
        return None
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    if number != number or number in (float("inf"), float("-inf")):
        return None
    if integer:
        return int(number) if number == int(number) and number >= 0 else None
    return number


def provenance_of(workflow, values, request_id, base):
    """What the submitted workflow says about how the image was made, for the
    layer's provenance: model, sampler, steps, cfg and seed from its nodes
    (the first of each), the server and the job id. Nothing from ``extra_data``
    or the API key is looked at. Missing details are left out."""
    found = {}
    for node in workflow.values():
        inputs = node.get("inputs") if isinstance(node, dict) else None
        if not isinstance(inputs, dict):
            continue
        for key, field in (("ckpt_name", "model"), ("unet_name", "model"), ("model_name", "model"), ("sampler_name", "sampler"), ("scheduler", "scheduler")):
            if isinstance(inputs.get(key), str) and inputs[key] and field not in found:
                found[field] = inputs[key][:256]
        for key, field, integer in (("steps", "steps", True), ("cfg", "cfg", False), ("seed", "seed", True), ("noise_seed", "seed", True)):
            number = _number(inputs.get(key), integer)
            if number is not None and field not in found:
                found[field] = number
    if "seed" not in found:
        seed = _number(values.get("seed"), True)
        if seed is not None:
            found["seed"] = seed
    found["service"] = urllib.parse.urlsplit(base).hostname or "Comfy Cloud"
    found["request_id"] = str(request_id)[:256]
    return found


def precise_edit_prompt(job):
    source = job.source or {}
    width = float(source.get("width") or 1)
    height = float(source.get("height") or 1)
    elements = []
    for region in job.regions:
        fields = region.get("fields") or {}
        x0 = region["x"] / width * 1000
        y0 = region["y"] / height * 1000
        x1 = (region["x"] + region["width"]) / width * 1000
        y1 = (region["y"] + region["height"]) / height * 1000
        element = {
            "type": fields.get("type") or "obj",
            "bbox": [int(round(v)) for v in (x0, y0, x1, y1)],
            "desc": fields.get("desc") or "",
        }
        if element["type"] == "text":
            element["text"] = element.pop("desc")
        elements.append(element)
    return json.dumps({"compositional_deconstruction": {"background": job.inputs.get("background") or "", "elements": elements}})


# --- Running --------------------------------------------------------------


def run_workflow(job, workflow_name, values, with_source, with_mask=False):
    client = Client()
    entry = {"id": job.id[:8], "action": job.action, "state": "uploading", "started": time.time()}
    with history_lock:
        history.insert(0, entry)
        del history[10:]
    source_ref = None
    if with_source:
        if not job.source_path:
            raise RpcError(INVALID_PARAMS, "this action needs an image layer")
        job.progress(0.05, "Uploading source")
        asset = client.upload(job.source_path)
        source_ref = asset.get("name") or asset.get("id") or asset
    mask_ref = None
    if with_mask:
        if not job.selection_mask_path:
            raise RpcError(INVALID_PARAMS, "this action needs the selection as a mask")
        job.progress(0.08, "Uploading mask")
        asset = client.upload(job.selection_mask_path)
        mask_ref = asset.get("name") or asset.get("id") or asset
    workflow = fill(load_workflow(workflow_name), values, source_ref, mask_ref)
    job.check_cancelled()
    job.progress(0.1, "Submitting")
    submitted = client.submit(workflow)
    job_id = submitted.get("id") or submitted.get("job_id") or submitted.get("prompt_id")
    if not job_id:
        raise RpcError(INTERNAL_ERROR, f"unexpected submit response: {json.dumps(submitted)[:300]}")
    entry.update(state="queued", remote=job_id)
    deadline = time.time() + float(plugin.settings.get("timeout") or 600)
    delay = 1.0
    while True:
        if job.cancelled:
            client.cancel(job_id)
            entry["state"] = "cancelled"
            raise Cancelled()
        if time.time() > deadline:
            client.cancel(job_id)
            entry["state"] = "timed out"
            raise RpcError(INTERNAL_ERROR, "the job did not finish in time")
        status = client.job(job_id)
        state = str(status.get("status") or status.get("state") or "").lower()
        entry["state"] = state or "running"
        progress = status.get("progress") or {}
        if isinstance(progress, dict):
            fraction = progress.get("fraction") or progress.get("value")
            if fraction is not None:
                job.progress(0.1 + 0.8 * float(fraction), progress.get("message") or state)
            else:
                job.progress(None, state)
        if state in TERMINAL:
            break
        time.sleep(delay)
        delay = min(delay * 1.5, 5.0)
    if state not in ("completed", "success", "succeeded"):
        error = status.get("error") or {}
        message = error.get("message") if isinstance(error, dict) else str(error)
        raise RpcError(INTERNAL_ERROR, f"job {state}: {message or 'no details'}")
    outputs = status.get("outputs") or []
    if isinstance(outputs, dict):
        outputs = [o for node in outputs.values() for o in (node.get("images") or [])] if outputs else []
    images = [o for o in outputs if isinstance(o, dict) and ("image" in str(o.get("type", "image")).lower() or str(o.get("name", o.get("filename", ""))).lower().endswith((".png", ".jpg", ".jpeg", ".webp")))]
    if not images:
        raise RpcError(INTERNAL_ERROR, f"the workflow produced no images: {json.dumps(outputs)[:300]}")
    job.progress(0.95, "Downloading")
    results = []
    for index, output in enumerate(images):
        destination = job.path(f"result-{index + 1}.png")
        client.download(output, destination)
        results.append(destination)
    entry["state"] = "done"
    return results, provenance_of(workflow, values, job_id, client.base)


@plugin.action("precise-edit")
def precise_edit(job):
    values = {"prompt": precise_edit_prompt(job), "seed": job.inputs.get("seed", 0), "quality": job.inputs.get("quality", "medium")}
    results, provenance = run_workflow(job, "precise-edit", values, with_source=True)
    return [image_output(path, "Precise Edit", provenance) for path in results] + [job.text("Comfy Cloud credits were used")]


@plugin.action("inpaint")
def inpaint(job):
    values = {"prompt": job.inputs.get("prompt", ""), "seed": job.inputs.get("seed", 0)}
    results, provenance = run_workflow(job, "inpaint", values, with_source=True, with_mask=True)
    # The selection mask (feathered by the host) also masks the result layer,
    # so only the selected area changes.
    return [image_output(path, "Inpaint", provenance, mask=job.selection_mask_path) for path in results]


@plugin.action("generate")
def generate(job):
    values = {"prompt": job.inputs.get("prompt", ""), "seed": job.inputs.get("seed", 0), "width": job.inputs.get("width", 1024), "height": job.inputs.get("height", 1024)}
    results, provenance = run_workflow(job, "text-to-image", values, with_source=False)
    return [image_output(path, "Generated", provenance) for path in results]


@plugin.action("run-workflow")
def run_custom(job):
    values = {"prompt": job.inputs.get("prompt", ""), "seed": job.inputs.get("seed", 0)}
    results, provenance = run_workflow(job, job.inputs.get("workflow") or "my-workflow.json", values, with_source=True)
    return [image_output(path, "Comfy result", provenance) for path in results]


def image_output(path, name, provenance=None, mask=None):
    return Job.image(path, name=name, mask=mask, provenance=provenance)


@plugin.estimate("precise-edit")
def estimate(job):
    return {"cost": "Comfy Cloud credits apply", "seconds": 30}


@plugin.pane("jobs")
def jobs_pane(pane):
    with history_lock:
        entries = list(history)
    if not entries:
        return ui.column(ui.label("No Comfy jobs yet.", muted=True), ui.link("Comfy Cloud", "https://cloud.comfy.org"))
    items = [ui.item(e["id"], f"{e['action']} · {e['state']}", time.strftime("%H:%M:%S", time.localtime(e["started"]))) for e in entries]
    return ui.column(ui.listing("history", items), ui.button("refresh", "Refresh"))


if __name__ == "__main__":
    plugin.run()
