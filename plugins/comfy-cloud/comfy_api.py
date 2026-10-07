"""Comfy API v2 client (https://docs.comfy.org/api-reference/v2/overview):
uploads, jobs and downloads, plus the node definitions and public workflow
templates the plugin converts. Standard library only.

The API key is only ever sent to the configured server over https: downloads
from other hosts the manifest declares go without it, redirects that leave the
server drop it, and anything else is refused.
"""
import io
import json
import mimetypes
import os
import urllib.error
import urllib.parse
import urllib.request
import uuid

from xuan_plugin import INSUFFICIENT_CREDITS, INTERNAL_ERROR, RATE_LIMITED, RpcError

# Hosts the manifest declares under permissions.network; outputs are only
# downloaded from these (or from the configured server). Comfy Cloud hands out
# signed storage.googleapis.com links for job outputs.
NETWORK = ("cloud.comfy.org", "*.run.comfy.app", "storage.googleapis.com")
TEMPLATES = "https://cloud.comfy.org/templates"
SUCCEEDED = {"succeeded", "completed", "success"}
TERMINAL = SUCCEEDED | {"failed", "error", "canceled", "cancelled", "expired", "lost", "non_retryable_error"}


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


def asset_ref(asset):
    """How a workflow refers to an uploaded asset (a LoadImage ``image``)."""
    return {"__type": "core/ASSET", "info": {"id": asset["id"]}}


def rejected_before_running(status):
    """Whether a failed job was refused before any node ran: ComfyUI found the
    workflow invalid, so nothing was spent and an older workflow may work."""
    error = status.get("error") or {}
    if not isinstance(error, dict) or status.get("started_at"):
        return False
    if error.get("node_errors"):
        return True
    text = f"{error.get('code', '')} {error.get('message', '')}"
    return any(sign in text for sign in ("failed validation", "invalid_workflow", "missing_node_type", "unknown_node_class", "invalid_prompt"))


class Client:
    def __init__(self, base, key):
        self.base = (base or "https://cloud.comfy.org").rstrip("/")
        self.key = key or ""

    def _request(self, method, path, body=None, headers=None, raw=False, url=None, send_key=True, parse=True):
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
                if parse and response.headers.get("Content-Type", "").startswith("application/json"):
                    return json.loads(payload or b"null")
                return payload
        except urllib.error.HTTPError as error:
            payload = error.read()
            try:
                detail = json.loads(payload)
            except ValueError:
                detail = {"message": payload.decode("utf-8", "replace")[:300]}
            inner = detail.get("error", detail) if isinstance(detail, dict) else detail
            message = inner.get("message") if isinstance(inner, dict) else str(detail)
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
            host = urllib.parse.urlsplit(url).hostname or url
            raise RpcError(INTERNAL_ERROR, f"cannot reach {host}: {error.reason}")

    def upload(self, path, expires_in=86400):
        """POST /api/v2/assets (multipart); returns the asset record. Inputs
        expire after a day: they are only needed while the job runs."""
        boundary = "----xuan" + uuid.uuid4().hex
        name = os.path.basename(path)
        content_type = mimetypes.guess_type(name)[0] or "application/octet-stream"
        with open(path, "rb") as handle:
            file_bytes = handle.read()
        body = io.BytesIO()
        # The API reads the fields in order: everything before the file.
        fields = (("content_type", content_type), ("file_path", name), ("tags", json.dumps(["input"])), ("expires_in", str(expires_in)))
        for field, value in fields:
            body.write(f"--{boundary}\r\nContent-Disposition: form-data; name=\"{field}\"\r\n\r\n{value}\r\n".encode())
        body.write(f"--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"{name}\"\r\nContent-Type: {content_type}\r\n\r\n".encode())
        body.write(file_bytes)
        body.write(f"\r\n--{boundary}--\r\n".encode())
        headers = {"Content-Type": f"multipart/form-data; boundary={boundary}", "Idempotency-Key": uuid.uuid4().hex}
        return self._request("POST", "/api/v2/assets", body.getvalue(), headers, raw=True)

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
        asset = output.get("id") or output.get("asset_id")
        if not url and asset:
            url = self.base + f"/api/v2/assets/{asset}/content"
        if not url:
            raise RpcError(INTERNAL_ERROR, f"output without a url: {json.dumps(output)[:200]}")
        url, send_key = download_target(url, self.base)
        payload = self._request("GET", None, raw=True, url=url, send_key=send_key, parse=False)
        with open(destination, "wb") as handle:
            handle.write(payload)

    def node_defs(self):
        """Every node definition the server has (``/api/object_info``, ~10 MB
        on Comfy Cloud, which serves no per-node lookup)."""
        defs = self._request("GET", "/api/object_info")
        if not isinstance(defs, dict):
            raise RpcError(INTERNAL_ERROR, "the server sent no node definitions")
        return defs

    def template(self, name):
        """The raw JSON of one of Comfy's public workflow templates."""
        return self._request("GET", None, url=f"{TEMPLATES}/{urllib.parse.quote(name)}.json", send_key=False, parse=False)

    def template_index(self):
        return json.loads(self._request("GET", None, url=f"{TEMPLATES}/index.json", send_key=False, parse=False))
