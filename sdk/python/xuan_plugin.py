"""Write Xuan plugins in Python with no dependencies.

A plugin is a program that reads JSON-RPC requests from stdin and answers on
stdout; see docs/PLUGINS.md in the Xuan repository for the protocol. This
module does the plumbing: start-up, threads, request routing, progress and
cancellation, so a plugin is a few decorated functions::

    from xuan_plugin import Plugin, ui

    plugin = Plugin()

    @plugin.action("invert")
    def invert(job):
        job.progress(0.1, "reading")
        ...
        return [job.image("out.png", name="Inverted")]

    @plugin.pane("info")
    def info(pane):
        return ui.column(ui.heading("Hello"), ui.label(pane.reason))

    if __name__ == "__main__":
        plugin.run()

Handlers run on worker threads, so they may call back into the editor
(``plugin.host.document()`` and friends) while the editor keeps servicing
messages. Python 3.8 or newer.
"""

from __future__ import annotations

import base64
import json
import os
import sys
import threading
import traceback
from typing import Any, Callable, Dict, List, Optional

PROTOCOL = 1

# Error codes the host understands.
PARSE_ERROR = -32700
INVALID_REQUEST = -32600
METHOD_NOT_FOUND = -32601
INVALID_PARAMS = -32602
INTERNAL_ERROR = -32603
CANCELLED = -32800
NEEDS_SETUP = -32001
INSUFFICIENT_CREDITS = -32002
RATE_LIMITED = -32003
# Never sent by the editor: a request it did not answer in time, which was
# withdrawn with ``request/cancel``.
TIMED_OUT = -32004


class RpcError(Exception):
    """Raise from a handler to answer with a JSON-RPC error."""

    def __init__(self, code: int, message: str, data: Any = None):
        super().__init__(message)
        self.code = code
        self.message = message
        self.data = data

    def to_json(self) -> Dict[str, Any]:
        error: Dict[str, Any] = {"code": self.code, "message": self.message}
        if self.data is not None:
            error["data"] = self.data
        return error


class Cancelled(RpcError):
    def __init__(self, message: str = "Cancelled"):
        super().__init__(CANCELLED, message)


class NeedsSetup(RpcError):
    """The plugin is missing a setting; the editor offers to open its settings."""

    def __init__(self, message: str):
        super().__init__(NEEDS_SETUP, message)


class _Transport:
    """Line-delimited JSON over stdin/stdout, safe to write from any thread."""

    def __init__(self) -> None:
        self._lock = threading.Lock()
        self._out = sys.stdout.buffer
        self._next_id = 1
        self._pending: Dict[int, "_Waiter"] = {}

    def send(self, message: Dict[str, Any]) -> None:
        line = json.dumps(message, separators=(",", ":"), ensure_ascii=False) + "\n"
        with self._lock:
            self._out.write(line.encode("utf-8"))
            self._out.flush()

    def request(self, method: str, params: Any, timeout: Optional[float]) -> Any:
        with self._lock:
            request_id = self._next_id
            self._next_id += 1
            waiter = _Waiter()
            self._pending[request_id] = waiter
        self.send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
        if not waiter.event.wait(timeout):
            with self._lock:
                waiting = self._pending.pop(request_id, None)
            if waiting is not None:
                # Withdraw it, so a prompt the editor shows for it closes and
                # a late answer does nothing.
                self.send({"jsonrpc": "2.0", "method": "request/cancel", "params": {"id": request_id}})
            raise RpcError(TIMED_OUT, f"the editor did not answer {method} in time")
        if waiter.error is not None:
            raise RpcError(
                waiter.error.get("code", INTERNAL_ERROR),
                waiter.error.get("message", "error"),
                waiter.error.get("data"),
            )
        return waiter.result

    def resolve(self, message: Dict[str, Any]) -> None:
        with self._lock:
            waiter = self._pending.pop(message.get("id"), None)
        if waiter is None:
            return
        if "error" in message and message["error"] is not None:
            waiter.error = message["error"]
        else:
            waiter.result = message.get("result")
        waiter.event.set()

    def fail_all(self) -> None:
        with self._lock:
            waiters = list(self._pending.values())
            self._pending.clear()
        for waiter in waiters:
            waiter.error = {"code": INTERNAL_ERROR, "message": "the editor went away"}
            waiter.event.set()


class _Waiter:
    def __init__(self) -> None:
        self.event = threading.Event()
        self.result: Any = None
        self.error: Optional[Dict[str, Any]] = None


class Host:
    """Requests a plugin can make of the editor."""

    def __init__(self, transport: _Transport, session: Optional[str] = None):
        self._transport = transport
        self.timeout: Optional[float] = 120.0
        # Sent as ``session`` with every request; see ``with_session``.
        self.session = session

    def request(self, method: str, params: Any = None) -> Any:
        if self.session is not None and (params is None or isinstance(params, dict)):
            params = dict(params or {}, session=self.session)
        return self._transport.request(method, params, self.timeout)

    def with_session(self, id: str) -> "Host":
        """A host whose requests belong to the session ``id``.

        A plugin with ``edit_prompt = "session"`` is allowed to edit once per session.
        """
        host = Host(self._transport, session=id)
        host.timeout = self.timeout
        return host

    def session_status(self) -> Dict[str, Any]:
        """``{"edit_prompt", "edits": "allowed" | "denied" | "ask", "auto"}`` for this session."""
        return self.request("session/status", {})

    def notify(self, method: str, params: Any = None) -> None:
        self._transport.send({"jsonrpc": "2.0", "method": method, "params": params})

    def log(self, message: str) -> None:
        """Add a line to the plugin log in Plugins → Manage Plugins…"""
        self.notify("host/log", {"level": "info", "message": str(message)})

    def status(self, message: str) -> None:
        """Show a message in the editor's status bar."""
        self.notify("host/status", {"message": str(message)})

    def document(self) -> Optional[Dict[str, Any]]:
        """The open document: size, layers and selection, or None."""
        return self.request("document/get")

    def export_layer(
        self,
        layer: str,
        what: str = "pixels",
        max_side: Optional[int] = None,
        dir: Optional[str] = None,
    ) -> Dict[str, Any]:
        """Write a layer's pixels or mask as PNG; returns path, size and position.

        ``dir`` must be one of the plugin's folders (a job's ``work_dir``,
        ``data_dir`` or the plugin folder) unless the manifest declares
        ``filesystem = "write"``; by default the host picks a scratch folder."""
        params: Dict[str, Any] = {"layer": layer, "what": what}
        if max_side:
            params["max_side"] = int(max_side)
        if dir:
            params["dir"] = dir
        return self.request("layer/export", params)

    def export_document(self, max_side: Optional[int] = None, dir: Optional[str] = None) -> Dict[str, Any]:
        """Write the flattened document as PNG; ``dir`` as for ``export_layer``."""
        params: Dict[str, Any] = {}
        if max_side:
            params["max_side"] = int(max_side)
        if dir:
            params["dir"] = dir
        return self.request("document/export", params)

    def export_selection(self, dir: Optional[str] = None) -> Optional[Dict[str, Any]]:
        """Write the selection mask cropped to its bounds, or None; ``dir`` as for ``export_layer``."""
        return self.request("selection/export", {"dir": dir} if dir else {})

    def edit(self, name: str, edits: List[Dict[str, Any]]) -> List[str]:
        """Apply edits as one undo step and return the ids of the layers they added.

        Needs ``document = "edit"`` in the manifest.
        """
        result = self.request("document/edit", {"name": name, "edits": edits}) or {}
        return [layer for layer in result.get("layers", []) if isinstance(layer, str)]

    def list_documents(self) -> List[Dict[str, Any]]:
        """The open documents: id, title, width, height, layers, current, modified, saved."""
        return (self.request("document/list") or {}).get("documents", [])

    def activate_document(self, id: str) -> None:
        """Make an open document the current one, as clicking its tab does."""
        self.request("document/activate", {"document": id})

    def save_as(self, document: Optional[str] = None, suggested_name: Optional[str] = None) -> str:
        """Show the save dialog for a document as a ``.xuan`` project; returns the chosen path.

        A cancelled dialog raises ``RpcError`` with the code ``CANCELLED``.
        """
        params = {"document": document, "suggested_name": suggested_name}
        return self.request("file/save_as", params)["name"]

    def export_file(
        self, format: str = "png", document: Optional[str] = None, suggested_name: Optional[str] = None
    ) -> str:
        """Show the save dialog to export a document as png, jpg, tiff or webp; returns the path."""
        params = {"document": document, "format": format, "suggested_name": suggested_name}
        return self.request("file/export", params)["name"]

    def open_file(self, path: str) -> Dict[str, Any]:
        """Ask the user to open the file at the absolute ``path`` as a document."""
        return self.request("file/open", {"path": path})

    def run(self, action: str, inputs: Optional[Dict[str, Any]] = None) -> None:
        """Run an allowed host command, or one of this plugin's own ``plugin/action``."""
        self.request("host/run", {"action": action, "inputs": inputs or {}})

    def open(self, path: Optional[str] = None, url: Optional[str] = None) -> None:
        self.request("host/open", {"path": path} if path else {"url": url})


class Job:
    """One ``action/run``: its inputs, source image and output helpers."""

    def __init__(self, plugin: "Plugin", params: Dict[str, Any]):
        self.plugin = plugin
        self.host = plugin.host
        self.id: str = params.get("job", "")
        self.action: str = params.get("action", "")
        self.inputs: Dict[str, Any] = params.get("inputs") or {}
        self.source: Optional[Dict[str, Any]] = params.get("source")
        self.document: Optional[Dict[str, Any]] = params.get("document")
        self.work_dir: str = params.get("work_dir") or os.getcwd()
        self._cancel = threading.Event()

    @property
    def cancelled(self) -> bool:
        return self._cancel.is_set()

    def check_cancelled(self) -> None:
        """Raise ``Cancelled`` when the user cancelled the job."""
        if self.cancelled:
            raise Cancelled()

    def progress(self, fraction: Optional[float] = None, message: Optional[str] = None) -> None:
        params: Dict[str, Any] = {"job": self.id}
        if fraction is not None:
            params["fraction"] = max(0.0, min(1.0, float(fraction)))
        if message is not None:
            params["message"] = str(message)
        self.host.notify("job/progress", params)

    def path(self, name: str) -> str:
        """A file path inside the job's working directory."""
        return os.path.join(self.work_dir, name)

    def model_path(self, id: str) -> str:
        """The path of a verified ``[[models]]`` file the action needs.

        Raises ``NeedsSetup`` when it is not downloaded. Xuan downloads the
        models an action lists in its manifest before running it."""
        path = self.plugin.model_path(id)
        if path is None:
            raise NeedsSetup(
                f"Model `{id}` is not downloaded. Download it in Plugins → Manage Plugins… → Models."
            )
        return path

    @property
    def source_path(self) -> Optional[str]:
        return self.source.get("path") if self.source else None

    @property
    def selection_mask_path(self) -> Optional[str]:
        """The selection as a grey PNG the size of ``source_path`` (white
        selected, black not), when the action sets ``source.mask = "selection"``."""
        return self.source.get("mask") if self.source else None

    @property
    def extension(self) -> Optional[Dict[str, int]]:
        """How far the source was extended (``source.extend``), as
        ``{"left", "top", "right", "bottom"}`` in document pixels, or None."""
        extend = self.source.get("extend") if self.source else None
        if not extend:
            return None
        return {side: int(extend.get(side) or 0) for side in Job.SIDES}

    @property
    def extend_mask_path(self) -> Optional[str]:
        """A grey PNG the size of ``source_path``: white over the new canvas
        an extended source was padded with, black over the old image."""
        return self.source.get("extend_mask") if self.source else None

    @property
    def regions(self) -> List[Dict[str, Any]]:
        for value in self.inputs.values():
            if isinstance(value, list) and value and isinstance(value[0], dict) and "index" in value[0]:
                return value
        return []

    # Output helpers; return a list of these from an action handler.
    @staticmethod
    def image(
        path: str,
        name: Optional[str] = None,
        x: float = 0,
        y: float = 0,
        mask: Optional[str] = None,
        width: Optional[float] = None,
        height: Optional[float] = None,
        fit_source: bool = False,
        provenance: Optional[Dict[str, Any]] = None,
    ) -> Dict[str, Any]:
        """An image output. Unless the action's ``result.into`` is
        ``document`` (a new tab), it changes the open document, so the manifest
        needs ``document = "edit"``; a ``read`` plugin's result is refused.
        ``width``/``height`` (document units) or
        ``fit_source=True`` (cover the source that was sent) place a result of
        any pixel size at that size, so extra pixels become higher density.

        ``provenance`` records how the image was made; it is shown in the
        layer's info and saved with the project. Known keys: ``model``,
        ``model_hash``, ``weights_sha256`` (64 hex digits), ``sampler``,
        ``scheduler``, ``steps``, ``seed``, ``cfg``, ``service``,
        ``request_id`` and an ``extra`` dict for anything else. Strings are
        limited to 256 bytes, the whole record to 8 KiB; unknown keys fail
        the result. The host removes secret-like keys (``api_key``,
        ``token``, ``authorization``, ``password``, ``secret``...) and your
        secrets' values, but do not put them here."""
        output: Dict[str, Any] = {"kind": "image", "path": path, "x": x, "y": y}
        if name:
            output["name"] = name
        if mask:
            output["mask"] = mask
        if width is not None:
            output["width"] = width
        if height is not None:
            output["height"] = height
        if fit_source:
            output["fit"] = "source"
        if provenance:
            output["provenance"] = provenance
        return output

    MASK_MODES = ("replace", "add", "subtract", "intersect")

    @staticmethod
    def mask(
        path: str,
        mode: str = "replace",
        x: float = 0,
        y: float = 0,
        width: Optional[float] = None,
        height: Optional[float] = None,
        fit_source: bool = False,
    ) -> Dict[str, Any]:
        """A mask output: a grey PNG (white selected, black not, grey partly)
        that becomes the document's selection once the user accepts it.
        ``mode`` combines it with the current selection: ``replace``, ``add``,
        ``subtract`` or ``intersect``. It is placed like ``image``, so
        ``fit_source=True`` lays a mask of any size over the source that was
        sent. Needs no ``document = "edit"``: a selection is not a pixel edit, so a
        ``document = "read"`` plugin may return it."""
        if mode not in Job.MASK_MODES:
            raise ValueError(f"mode must be one of {', '.join(Job.MASK_MODES)}")
        output: Dict[str, Any] = {"kind": "mask", "path": path, "mode": mode, "x": x, "y": y}
        if width is not None:
            output["width"] = width
        if height is not None:
            output["height"] = height
        if fit_source:
            output["fit"] = "source"
        return output

    @staticmethod
    def new_document(
        path: str, name: Optional[str] = None, provenance: Optional[Dict[str, Any]] = None
    ) -> Dict[str, Any]:
        output: Dict[str, Any] = {"kind": "document", "path": path}
        if name:
            output["name"] = name
        if provenance:
            output["provenance"] = provenance
        return output

    @staticmethod
    def edit(edits: List[Dict[str, Any]]) -> Dict[str, Any]:
        """Edits applied with the result as one undo step. Needs
        ``document = "edit"``; a ``read`` plugin may only return a
        ``set_selection`` edit, anything else refuses the whole result."""
        return {"kind": "edit", "edits": edits}

    SIDES = ("left", "top", "right", "bottom")

    @staticmethod
    def extend_canvas(left: int = 0, top: int = 0, right: int = 0, bottom: int = 0) -> Dict[str, Any]:
        """An ``extend_canvas`` edit: grow the canvas by whole document
        pixels on each side, moving layers and guides like Canvas Size. Needs
        ``document = "edit"``. Pass ``**job.extension`` to grow the canvas by
        what the source was extended by; an image returned with
        ``fit_source=True`` then covers the whole new canvas."""
        sides = {"left": left, "top": top, "right": right, "bottom": bottom}
        for side, value in sides.items():
            if not isinstance(value, int) or isinstance(value, bool) or value < 0:
                raise ValueError(f"{side} must be a whole number of pixels, 0 or more")
        return {"op": "extend_canvas", **sides}

    @staticmethod
    def text(text: str) -> Dict[str, Any]:
        return {"kind": "text", "text": text}


class Pane:
    """A ``pane/render`` request."""

    def __init__(self, plugin: "Plugin", params: Dict[str, Any]):
        self.plugin = plugin
        self.host = plugin.host
        self.id: str = params.get("pane", "")
        self.reason: str = params.get("reason", "open")
        event = params.get("event") or {}
        self.widget: Optional[str] = event.get("widget")
        self.value: Any = event.get("value")
        self.document: Optional[Dict[str, Any]] = params.get("document")


class Secrets(dict):
    """The plugin's secrets: a dict whose ``repr`` names the keys but never
    shows the values, so logging it or a traceback cannot leak them."""

    def __repr__(self) -> str:
        return "Secrets({%s})" % ", ".join(f"{key!r}: '<redacted>'" for key in self)

    __str__ = __repr__


def _models(params: Dict[str, Any]) -> Dict[str, str]:
    """The verified models in ``initialize`` or ``models/changed`` params."""
    models = params.get("models")
    if not isinstance(models, dict):
        return {}
    return {str(id): path for id, path in models.items() if isinstance(path, str)}


class Plugin:
    """Register handlers with the decorators, then call ``run()``."""

    def __init__(self) -> None:
        self._transport = _Transport()
        self.host = Host(self._transport)
        self.settings: Dict[str, Any] = {}
        self.secrets: Secrets = Secrets()
        self.plugin_dir: str = os.getcwd()
        self.data_dir: str = os.environ.get("XUAN_DATA_DIR", os.getcwd())
        # The folder Xuan downloads ``[[models]]`` into (read only), and the
        # verified model files in it by id.
        self.models_dir: str = os.environ.get("XUAN_MODELS_DIR", "")
        self.models: Dict[str, str] = {}
        self.host_info: Dict[str, Any] = {}
        self._actions: Dict[str, Callable[[Job], Any]] = {}
        self._estimates: Dict[str, Callable[[Job], Any]] = {}
        self._panes: Dict[str, Callable[[Pane], Any]] = {}
        self._importers: Dict[str, Callable[..., Any]] = {}
        self._exporters: Dict[str, Callable[..., Any]] = {}
        self._on_initialize: Optional[Callable[[], Any]] = None
        self._on_settings: Optional[Callable[[], Any]] = None
        self._on_document_changed: Optional[Callable[[Dict[str, Any]], Any]] = None
        self._on_models: Optional[Callable[[], Any]] = None
        self._jobs: Dict[str, Job] = {}
        self._jobs_lock = threading.Lock()
        self._stopping = threading.Event()

    # --- decorators ---------------------------------------------------
    def action(self, id: str) -> Callable:
        def register(function: Callable[[Job], Any]) -> Callable[[Job], Any]:
            self._actions[id] = function
            return function

        return register

    def estimate(self, id: str) -> Callable:
        """Answer ``action/estimate``: return ``{"cost": "…", "seconds": n}``."""

        def register(function: Callable[[Job], Any]) -> Callable[[Job], Any]:
            self._estimates[id] = function
            return function

        return register

    def pane(self, id: str) -> Callable:
        def register(function: Callable[[Pane], Any]) -> Callable[[Pane], Any]:
            self._panes[id] = function
            return function

        return register

    def importer(self, format: str) -> Callable:
        """``function(path, work_dir)`` returns ``{"width", "height", "layers": [...]}``."""

        def register(function: Callable[..., Any]) -> Callable[..., Any]:
            self._importers[format] = function
            return function

        return register

    def exporter(self, format: str) -> Callable:
        """``function(path, image, document)`` writes the file."""

        def register(function: Callable[..., Any]) -> Callable[..., Any]:
            self._exporters[format] = function
            return function

        return register

    def on_initialize(self, function: Callable[[], Any]) -> Callable[[], Any]:
        self._on_initialize = function
        return function

    def on_settings(self, function: Callable[[], Any]) -> Callable[[], Any]:
        self._on_settings = function
        return function

    def on_document_changed(self, function: Callable[[Dict[str, Any]], Any]) -> Callable[[Dict[str, Any]], Any]:
        self._on_document_changed = function
        return function

    def on_models(self, function: Callable[[], Any]) -> Callable[[], Any]:
        """Called after ``models/changed``, when a model was downloaded or deleted."""
        self._on_models = function
        return function

    def model_path(self, id: str) -> Optional[str]:
        """The path of a verified ``[[models]]`` file, or ``None`` while it
        is missing, downloading or failed verification."""
        return self.models.get(id)

    # --- runtime ------------------------------------------------------
    def update_pane(self, pane: str, tree: Dict[str, Any]) -> None:
        """Push new contents to a pane without waiting to be asked."""
        self.host.notify("pane/update", {"pane": pane, "tree": tree})

    def run(self) -> None:
        """Serve requests until the editor sends ``shutdown`` or closes stdin."""
        stdin = sys.stdin.buffer
        while not self._stopping.is_set():
            raw = stdin.readline()
            if not raw:
                break
            line = raw.decode("utf-8", "replace").strip()
            if not line:
                continue
            try:
                message = json.loads(line)
            except ValueError:
                sys.stderr.write(f"xuan_plugin: ignoring invalid JSON: {line[:200]}\n")
                continue
            if not isinstance(message, dict):
                continue
            if "method" in message:
                if "id" in message:
                    threading.Thread(target=self._handle_request, args=(message,), daemon=True).start()
                else:
                    self._handle_notification(message)
            elif "id" in message:
                self._transport.resolve(message)
        self._transport.fail_all()

    def _respond(self, request_id: Any, result: Any = None, error: Optional[RpcError] = None) -> None:
        message: Dict[str, Any] = {"jsonrpc": "2.0", "id": request_id}
        if error is not None:
            message["error"] = error.to_json()
        else:
            message["result"] = result
        self._transport.send(message)

    def _handle_request(self, message: Dict[str, Any]) -> None:
        request_id = message["id"]
        method = message.get("method", "")
        params = message.get("params") or {}
        try:
            result = self._dispatch(method, params)
            self._respond(request_id, result)
        except RpcError as error:
            self._respond(request_id, error=error)
        except Exception as error:  # noqa: BLE001 - report every failure to the host
            sys.stderr.write(traceback.format_exc())
            self._respond(request_id, error=RpcError(INTERNAL_ERROR, f"{type(error).__name__}: {error}"))
        if method == "shutdown":
            self._stopping.set()
            try:
                sys.stdout.flush()
            finally:
                os._exit(0)

    def _dispatch(self, method: str, params: Dict[str, Any]) -> Any:
        if method == "initialize":
            self.settings = params.get("settings") or {}
            self.secrets = Secrets(params.get("secrets") or {})
            self.plugin_dir = params.get("plugin_dir") or self.plugin_dir
            self.data_dir = params.get("data_dir") or self.data_dir
            self.models_dir = params.get("models_dir") or self.models_dir
            self.models = _models(params)
            self.host_info = params.get("host") or {}
            if self._on_initialize:
                self._on_initialize()
            return {"protocol": PROTOCOL}
        if method == "shutdown":
            return None
        if method == "action/run":
            job = Job(self, params)
            handler = self._actions.get(job.action)
            if handler is None:
                raise RpcError(METHOD_NOT_FOUND, f"no action {job.action}")
            with self._jobs_lock:
                self._jobs[job.id] = job
            try:
                outputs = handler(job)
            finally:
                with self._jobs_lock:
                    self._jobs.pop(job.id, None)
            if outputs is None:
                outputs = []
            if isinstance(outputs, dict):
                outputs = [outputs]
            return {"outputs": list(outputs)}
        if method == "action/estimate":
            job = Job(self, params)
            handler = self._estimates.get(job.action)
            return handler(job) if handler else {}
        if method == "pane/render":
            pane = Pane(self, params)
            handler = self._panes.get(pane.id)
            if handler is None:
                raise RpcError(METHOD_NOT_FOUND, f"no pane {pane.id}")
            return handler(pane)
        if method == "pane/close":
            return None
        if method == "format/import":
            handler = self._importers.get(params.get("format", ""))
            if handler is None:
                raise RpcError(METHOD_NOT_FOUND, "no importer for this format")
            return handler(params.get("path"), params.get("work_dir"))
        if method == "format/export":
            handler = self._exporters.get(params.get("format", ""))
            if handler is None:
                raise RpcError(METHOD_NOT_FOUND, "no exporter for this format")
            handler(params.get("path"), params.get("image"), params.get("document"))
            return None
        raise RpcError(METHOD_NOT_FOUND, f"unknown method {method}")

    def _handle_notification(self, message: Dict[str, Any]) -> None:
        method = message.get("method", "")
        params = message.get("params") or {}
        if method == "job/cancel":
            with self._jobs_lock:
                job = self._jobs.get(params.get("job", ""))
            if job is not None:
                job._cancel.set()
        elif method == "settings/changed":
            self.settings = params.get("settings") or {}
            self.secrets = Secrets(params.get("secrets") or {})
            if self._on_settings:
                threading.Thread(target=self._on_settings, daemon=True).start()
        elif method == "models/changed":
            self.models = _models(params)
            if self._on_models:
                threading.Thread(target=self._on_models, daemon=True).start()
        elif method == "document/changed":
            if self._on_document_changed:
                threading.Thread(target=self._on_document_changed, args=(params,), daemon=True).start()


class ui:
    """Builders for the widget tree a pane returns."""

    @staticmethod
    def column(*children: Dict[str, Any], gap: Optional[float] = None) -> Dict[str, Any]:
        node: Dict[str, Any] = {"type": "column", "children": list(children)}
        if gap is not None:
            node["gap"] = gap
        return node

    @staticmethod
    def row(*children: Dict[str, Any], gap: Optional[float] = None) -> Dict[str, Any]:
        node: Dict[str, Any] = {"type": "row", "children": list(children)}
        if gap is not None:
            node["gap"] = gap
        return node

    @staticmethod
    def heading(text: str) -> Dict[str, Any]:
        return {"type": "heading", "text": str(text)}

    @staticmethod
    def label(text: str, muted: bool = False, small: bool = False, wrap: bool = True) -> Dict[str, Any]:
        return {"type": "label", "text": str(text), "muted": muted, "small": small, "wrap": wrap}

    @staticmethod
    def separator() -> Dict[str, Any]:
        return {"type": "separator"}

    @staticmethod
    def space(size: float = 8) -> Dict[str, Any]:
        return {"type": "space", "size": size}

    @staticmethod
    def button(
        id: str, label: str, primary: bool = False, enabled: bool = True, copy: Optional[str] = None
    ) -> Dict[str, Any]:
        """A button; with ``copy``, Xuan also copies that text to the clipboard when it is clicked."""
        node = {"type": "button", "id": id, "label": label, "primary": primary, "enabled": enabled}
        if copy is not None:
            node["copy"] = copy
        return node

    @staticmethod
    def checkbox(id: str, label: str, value: bool = False) -> Dict[str, Any]:
        return {"type": "checkbox", "id": id, "label": label, "value": bool(value)}

    @staticmethod
    def text(id: str, value: str = "", placeholder: str = "", multiline: bool = False, width: Optional[float] = None) -> Dict[str, Any]:
        node: Dict[str, Any] = {"type": "text", "id": id, "value": value, "placeholder": placeholder, "multiline": multiline}
        if width is not None:
            node["width"] = width
        return node

    @staticmethod
    def number(id: str, value: float = 0, min: Optional[float] = None, max: Optional[float] = None, step: Optional[float] = None, suffix: str = "", integer: bool = False) -> Dict[str, Any]:
        node: Dict[str, Any] = {"type": "number", "id": id, "value": value, "suffix": suffix, "integer": integer}
        for key, item in (("min", min), ("max", max), ("step", step)):
            if item is not None:
                node[key] = item
        return node

    @staticmethod
    def slider(id: str, value: float, min: float, max: float, label: str = "", suffix: str = "", logarithmic: bool = False) -> Dict[str, Any]:
        return {"type": "slider", "id": id, "value": value, "min": min, "max": max, "label": label, "suffix": suffix, "logarithmic": logarithmic}

    @staticmethod
    def select(id: str, value: str, options: List[Any]) -> Dict[str, Any]:
        items = []
        for option in options:
            if isinstance(option, dict):
                items.append({"id": str(option.get("id", "")), "label": str(option.get("label", option.get("id", "")))})
            elif isinstance(option, (tuple, list)) and len(option) == 2:
                items.append({"id": str(option[0]), "label": str(option[1])})
            else:
                items.append({"id": str(option), "label": str(option)})
        return {"type": "select", "id": id, "value": value, "options": items}

    @staticmethod
    def color(id: str, value: str = "#ffffff") -> Dict[str, Any]:
        return {"type": "color", "id": id, "value": value}

    @staticmethod
    def image(src: str, width: Optional[float] = None, height: Optional[float] = None, fit: bool = True) -> Dict[str, Any]:
        node: Dict[str, Any] = {"type": "image", "src": src, "fit": fit}
        if width is not None:
            node["width"] = width
        if height is not None:
            node["height"] = height
        return node

    @staticmethod
    def progress(value: Optional[float] = None, label: str = "") -> Dict[str, Any]:
        return {"type": "progress", "value": value, "label": label}

    @staticmethod
    def listing(id: str, items: List[Dict[str, Any]], selected: Optional[str] = None) -> Dict[str, Any]:
        node: Dict[str, Any] = {"type": "list", "id": id, "items": items}
        if selected is not None:
            node["selected"] = selected
        return node

    @staticmethod
    def item(id: str, label: str, detail: str = "", icon: Optional[str] = None) -> Dict[str, Any]:
        node: Dict[str, Any] = {"id": id, "label": label, "detail": detail}
        if icon:
            node["icon"] = icon
        return node

    @staticmethod
    def swatches(colors: List[str], id: Optional[str] = None, selected: Optional[str] = None) -> Dict[str, Any]:
        node: Dict[str, Any] = {"type": "swatches", "colors": list(colors)}
        if id is not None:
            node["id"] = id
        if selected is not None:
            node["selected"] = selected
        return node

    @staticmethod
    def link(label: str, url: str) -> Dict[str, Any]:
        return {"type": "link", "label": label, "url": url}


def png_data_url(png_bytes: bytes) -> str:
    """An ``image`` source that embeds PNG bytes instead of writing a file."""
    return "data:image/png;base64," + base64.b64encode(png_bytes).decode("ascii")


def encode_png(width: int, height: int, rgba: bytes) -> bytes:
    """Encode 8-bit RGBA rows as PNG with the standard library only."""
    import struct
    import zlib

    if len(rgba) != width * height * 4:
        raise ValueError("rgba must hold width * height * 4 bytes")
    stride = width * 4
    raw = b"".join(b"\x00" + rgba[y * stride : (y + 1) * stride] for y in range(height))

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 6))
        + chunk(b"IEND", b"")
    )


def encode_gray_png(width: int, height: int, gray: bytes) -> bytes:
    """Encode 8-bit grey rows as PNG, for masks, with the standard library only."""
    import struct
    import zlib

    if len(gray) != width * height:
        raise ValueError("gray must hold width * height bytes")
    raw = b"".join(b"\x00" + gray[y * width : (y + 1) * width] for y in range(height))

    def chunk(kind: bytes, data: bytes) -> bytes:
        return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)

    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 6))
        + chunk(b"IEND", b"")
    )


def decode_png(data: bytes):
    """Decode an 8-bit RGB/RGBA/gray PNG to ``(width, height, rgba bytes)``.

    Covers what the editor writes (non-interlaced, 8-bit). Raises ValueError
    for anything else.
    """
    import struct
    import zlib

    if data[:8] != b"\x89PNG\r\n\x1a\n":
        raise ValueError("not a PNG")
    position = 8
    width = height = 0
    color_type = bit_depth = 0
    idat = []
    while position < len(data):
        (length,) = struct.unpack(">I", data[position : position + 4])
        kind = data[position + 4 : position + 8]
        body = data[position + 8 : position + 8 + length]
        position += 12 + length
        if kind == b"IHDR":
            width, height, bit_depth, color_type, _, _, interlace = struct.unpack(">IIBBBBB", body)
            if bit_depth != 8 or interlace != 0:
                raise ValueError("only 8-bit non-interlaced PNGs are supported")
        elif kind == b"IDAT":
            idat.append(body)
        elif kind == b"IEND":
            break
    channels = {0: 1, 2: 3, 4: 2, 6: 4}.get(color_type)
    if channels is None:
        raise ValueError("palette PNGs are not supported")
    raw = zlib.decompress(b"".join(idat))
    stride = width * channels
    previous = bytearray(stride)
    out = bytearray()
    offset = 0
    for _ in range(height):
        filter_type = raw[offset]
        line = bytearray(raw[offset + 1 : offset + 1 + stride])
        offset += 1 + stride
        for i in range(stride):
            a = line[i - channels] if i >= channels else 0
            b = previous[i]
            c = previous[i - channels] if i >= channels else 0
            if filter_type == 1:
                line[i] = (line[i] + a) & 0xFF
            elif filter_type == 2:
                line[i] = (line[i] + b) & 0xFF
            elif filter_type == 3:
                line[i] = (line[i] + ((a + b) >> 1)) & 0xFF
            elif filter_type == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                predictor = a if pa <= pb and pa <= pc else (b if pb <= pc else c)
                line[i] = (line[i] + predictor) & 0xFF
        previous = line
        if channels == 4:
            out += line
        elif channels == 3:
            for i in range(0, stride, 3):
                out += line[i : i + 3] + b"\xff"
        elif channels == 2:
            for i in range(0, stride, 2):
                out += bytes((line[i], line[i], line[i], line[i + 1]))
        else:
            for value in line:
                out += bytes((value, value, value, 255))
    return width, height, bytes(out)


__all__ = [
    "Plugin",
    "Job",
    "Pane",
    "Host",
    "RpcError",
    "Cancelled",
    "NeedsSetup",
    "ui",
    "png_data_url",
    "encode_png",
    "encode_gray_png",
    "decode_png",
    "PROTOCOL",
    "CANCELLED",
    "NEEDS_SETUP",
    "INSUFFICIENT_CREDITS",
    "RATE_LIMITED",
]
