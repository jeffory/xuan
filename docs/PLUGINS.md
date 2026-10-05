# Plugins

Xuan plugins are separate programs that talk to the editor over JSON-RPC 2.0 on
their standard input and output. They can add menu actions that edit or generate
images, sidebar panes with their own user interface, file formats, and settings.
The editor owns every interaction with the user: a plugin declares what it needs
in a manifest, and Xuan draws the controls, runs the on-canvas tools, keeps
undo history, and asks for permissions. A plugin never touches the document
directly; it asks the host for pixels and returns results.

Any language works. The repository ships a Rust SDK crate (`sdk/xuan-plugin`), a
Python module with no dependencies (`sdk/python/xuan_plugin.py`), and example
plugins under `plugins/`:

| Folder | Language | Shows |
| --- | --- | --- |
| `plugins/histogram` | Python | A pane that follows the document, settings, data-URL images |
| `plugins/invert-regions` | Rust | A region action with per-region fields, a pane that reads the composite |
| `plugins/comfy-cloud` | Python | Network jobs with progress, cancel and errors; secrets; `ask` results; three actions |

Both SDKs read requests on the main thread and run handlers on worker threads,
so a handler may call the editor (`host.document()`, `host.export_layer()`, …)
while a job runs. In Python, decorate functions on a `Plugin`; in Rust, chain
closures on `Plugin::new()` and call `run()`. `cargo build --release` in
`plugins/invert-regions` builds the Rust example; the Python examples run as
they are with `python3` on `PATH`.

## Installing

A plugin is a folder holding a `plugin.toml` manifest and its program. Xuan loads
every folder in the user plugins directory at start-up and from
**Plugins → Manage Plugins…**:

| Platform | Directory |
| --- | --- |
| Linux | `$XDG_CONFIG_HOME/xuan/plugins/` (default `~/.config/xuan/plugins/`) |
| Windows | `%APPDATA%\xuan\plugins\` |

Set `XUAN_PLUGIN_PATH` (a `:`/`;`-separated list of directories) to load plugins
from other places, for example a development checkout.

Plugins run as ordinary processes with the user's rights. Xuan shows the
permissions a plugin declares before it runs for the first time and records the
grant in `config.toml`; it cannot enforce them. Install plugins you trust.

## Manifest

```toml
[plugin]
id = "comfy-cloud"               # [a-z0-9-]+, unique, stable across versions
name = "Comfy Cloud"
version = "0.1.0"
description = "Generate and edit images with ComfyUI workflows on Comfy Cloud."
command = ["python3", "main.py"]  # run inside the plugin folder
protocol = 1                      # protocol version this plugin speaks

[permissions]
network = ["cloud.comfy.org"]     # hosts the plugin connects to (informational)
secrets = ["api_key"]             # settings of type "secret" it receives
document = "edit"                 # "read" (default) or "edit"
filesystem = "none"               # "none" (default), "read" or "write" outside its folders

[[settings]]
id = "api_key"
type = "secret"
label = "API key"
help = "Create one at platform.comfy.org."

[[settings]]
id = "max_side"
type = "integer"
label = "Longest side sent to the service"
default = 2048
min = 256
max = 8192

[[actions]]
id = "precise-edit"
label = "Ideogram Precise Edit…"
menu = "Filter"                   # File, Edit, Image, Layer, Select, Filter or Plugins (default)
shortcut = "Ctrl+Shift+E"         # optional default; the user can change it later
kind = "edit"                     # "edit" needs an image, "generate" does not, "command" returns nothing
source = { from = "layer", max_side = 2048, crop_to_regions = true, padding = 0.25 }
result = { into = "layer", mask_to_regions = true }
description = "Draw boxes over the parts to change and describe each one."

[[actions.inputs]]
id = "regions"
type = "regions"                  # on-canvas tool: numbered boxes or the selection
label = "Edits"
min = 1
fields = [
  { id = "desc", type = "text", label = "Instruction" },
  { id = "type", type = "enum", label = "Kind", values = ["obj", "text"], default = "obj" },
]

[[actions.inputs]]
id = "model"
type = "enum"
label = "Model"
values = [{ id = "ideogram-4.5", label = "Ideogram 4.5" }]
default = "ideogram-4.5"

[[actions.inputs]]
id = "seed"
type = "seed"                     # integer with a "random" button

[[panes]]
id = "jobs"
title = "Comfy jobs"
refresh = "manual"                # or "document" to re-render after edits

[[formats]]
id = "jxl"
label = "JPEG XL"
extensions = ["jxl"]
import = true
export = true
```

### Settings

Settings are drawn in **Plugins → Manage Plugins…** from the schema; a plugin
never implements that dialog. Types: `text`, `multiline`, `integer`, `number`,
`bool`, `enum`, `color`, `path` and `secret`. Values live in `config.toml` under
`[plugins.<id>.settings]`; secrets are stored in `secrets.toml` next to it, which
Xuan creates with owner-only permissions on Unix. A plugin receives its settings
with `initialize` and again through `settings/changed`; secrets are included
only when the manifest lists them under `permissions.secrets`.

### Action inputs

Inputs become a dialog, drawn by the host. Types: `text`, `multiline`,
`integer`, `number` (with `min`, `max`, `step`), `seed`, `bool`, `enum`,
`color`, `path` and `regions`. Each has `label`, optional `help`, `default` and
`placeholder`.

`regions` opens the **Region** tool while the dialog is open: the user drags
numbered boxes over the image, or turns the current selection into a region, and
fills in the `fields` the plugin declared for each one. Regions reach the plugin
as pixel rectangles in the coordinate space of the source image it was sent,
plus an optional mask PNG for selection-shaped regions.

### Sources and results

`source.from` chooses what the plugin receives for an `edit` action: `layer`
(the active image layer's pixels, the default), `composite` (the whole document
flattened), `selection` (the composite cropped to the selection) or `none`.
`max_side` downscales large images before sending, and `crop_to_regions` sends
only a padded crop around the regions instead of the whole image. The host
records the scale and offset, so the plugin only ever thinks in source pixels
and every result is placed back where it came from.

`result.into` chooses where image outputs go: `layer` (a new layer above the
source, the default), `replace` (the source layer's pixels), `document` (a new
tab), or `ask` (the dialog offers **New layer** / **New document**). With `mask_to_regions`, a new layer gets a mask built from the
regions with a soft edge, so only the parts the user asked to change show
through and the rest can be painted back.

Results never land silently. An edit arrives as a **proposal**: the layer is
added, a bar above the canvas offers **Compare**, **Accept** and **Discard**,
and only accepting commits a single undo step named after the action. Every
generated layer records which plugin, action, inputs and source produced it, so
**Layer → Re-run Plugin Action…** can repeat it with changes. See
[FORMAT.md](FORMAT.md) for the stored metadata.

## Protocol

Messages are JSON-RPC 2.0 objects, one per line, UTF-8, over the plugin's stdin
(host → plugin) and stdout (plugin → host). Stderr is captured into the plugin
log shown in **Plugins → Manage Plugins…**. Both sides may send requests; both must answer
requests promptly even while a job runs, so plugins should handle `action/run`
on a worker thread or an async task. Images are exchanged as PNG files in
directories the host owns; messages carry paths, never pixels.

### Lifecycle

| Request (host → plugin) | Params | Result |
| --- | --- | --- |
| `initialize` | `protocol`, `host: {name, version}`, `plugin_dir`, `data_dir`, `settings`, `secrets` | `{protocol}` |
| `shutdown` | — | `null`; the process must exit |

`data_dir` is a per-plugin folder that persists between runs. Temporary files
for a job go in the `work_dir` the host passes with each job and are removed when
the job ends.

### Actions

`action/run` is sent once per invocation:

```json
{"jsonrpc":"2.0","id":7,"method":"action/run","params":{
  "job": "0c2d…", "action": "precise-edit", "work_dir": "/tmp/xuan/jobs/0c2d…",
  "inputs": {"model": "ideogram-4.5", "seed": 1475826651,
             "regions": [{"index": 1, "x": 333, "y": 500, "width": 111, "height": 143,
                          "mask": null, "fields": {"desc": "Change the earring", "type": "obj"}}]},
  "source": {"path": "/tmp/xuan/jobs/0c2d…/source.png", "width": 896, "height": 1152,
             "layer": "6f0a…", "scale": 0.5, "offset": {"x": 120, "y": 80}},
  "document": {"id": "…", "width": 1792, "height": 2304, "active": "6f0a…", "layers": [ … ]}
}}
```

While it runs the plugin may notify `job/progress` with `{job, fraction?,
message?}`, and the host may notify `job/cancel` with `{job}`; a cancelled job
should answer with the error code `-32800`. The result lists outputs:

```json
{"outputs": [
  {"kind": "image", "path": "…/result.png", "name": "Precise Edit", "x": 0, "y": 0},
  {"kind": "text", "text": "Used 18 credits"}
]}
```

Output kinds: `image` (a PNG placed at `x`,`y` in source coordinates, optional
`mask` PNG and `name`), `document` (a PNG opened as a new tab), `edit` (a list of
document edits, see below, applied as one undo step), `text` (shown in the
status bar) and `none`. `action/estimate` with the same params may be answered
with `{cost: "≈18 credits", seconds: 20}`; the dialog shows it before running.

### Reading and editing the document

Plugins ask the host for data with these requests. Each is answered on the next
frame.

| Request (plugin → host) | Params | Result |
| --- | --- | --- |
| `document/get` | — | `{id, width, height, resolution, active, selection: {x, y, width, height} \| null, layers: [{id, name, kind, visible, locked, opacity, blend, parent, x, y, width, height, rotation, generated?}]}` |
| `layer/export` | `{layer, what: "pixels" \| "mask", max_side?, dir?}` | `{path, width, height, x, y, scale}` |
| `document/export` | `{max_side?, dir?}` | `{path, width, height, scale}` |
| `selection/export` | `{dir?}` | `{path, x, y, width, height}` or `null` |
| `document/edit` | `{name, edits: [ … ]}` | `{ok: true}`; needs `document = "edit"` |
| `host/run` | `{action, inputs?}` | runs a host command or another plugin's action |
| `host/open` | `{path}` or `{url}` | opens a file as a document or a URL in the browser |

`document/edit` edits, applied together as one undo step named `name`:

- `{"op": "add_layer", "image": path, "name"?, "x"?, "y"?, "mask"?, "above"?, "opacity"?, "blend"?}`
- `{"op": "replace_pixels", "layer", "image", "x"?, "y"?}`
- `{"op": "set", "layer", "name"?, "visible"?, "locked"?, "opacity"?, "blend"?}`
- `{"op": "remove_layer", "layer"}`
- `{"op": "set_mask", "layer", "mask": path | null}`
- `{"op": "set_selection", "mask": path | null}`
- `{"op": "select", "layer"}`

Notifications from the plugin: `host/log` `{level, message}` and `host/status`
`{message}`. Notifications from the host: `document/changed` `{id, revision}`
(sent to plugins with open panes or `refresh = "document"`), `settings/changed`
`{settings, secrets}`.

### Panes

A pane is declarative. The host asks for its contents and draws them with the
editor's own widgets, in the editor's theme:

| Request (host → plugin) | Params | Result |
| --- | --- | --- |
| `pane/render` | `{pane, reason: "open" \| "document" \| "event" \| "refresh", event?: {widget, value}, document?: {…}}` | a widget tree, or `null` to keep the current one |
| `pane/close` | `{pane}` | `null` |

A plugin may also notify `pane/update` `{pane, tree}` at any time, for example
when a background job finishes.

Widget tree nodes (`type` plus fields):

| Type | Fields |
| --- | --- |
| `column`, `row` | `children`, `gap?` |
| `heading`, `label` | `text`, `muted?`, `small?`, `wrap?` |
| `separator` | — |
| `space` | `size` |
| `button` | `id`, `label`, `primary?`, `enabled?` |
| `checkbox` | `id`, `label`, `value` |
| `text` | `id`, `value`, `placeholder?`, `multiline?`, `width?` |
| `number` | `id`, `value`, `min?`, `max?`, `step?`, `suffix?`, `integer?` |
| `slider` | `id`, `value`, `min`, `max`, `label?`, `suffix?`, `logarithmic?` |
| `select` | `id`, `value`, `options: [{id, label}]` |
| `color` | `id`, `value: "#rrggbb" \| "#rrggbbaa"` |
| `image` | `src` (a PNG path or a `data:image/png;base64,…` URL), `width?`, `height?`, `fit?` |
| `progress` | `value` (0–1) or `null` for a spinner, `label?` |
| `list` | `id`, `items: [{id, label, detail?, icon?}]`, `selected?` |
| `swatches` | `id?`, `colors: ["#rrggbb"]`, `selected?` |
| `link` | `label`, `url` |

Every interactive widget has an `id`. When the user changes one, the host sends
`pane/render` with `reason = "event"` and `event = {widget, value}` and replaces
the tree with the answer. Trees are cached per pane; images are reloaded when
the file changes.

### File formats

| Request (host → plugin) | Params | Result |
| --- | --- | --- |
| `format/import` | `{format, path, work_dir}` | `{width, height, resolution?, layers: [{name, image, x?, y?, mask?, opacity?, blend?, visible?}]}` |
| `format/export` | `{format, path, image, document}` | `null` |

Declared extensions appear in the Open, Import and Export dialogs; a built-in
format always wins over a plugin's.

### Errors

Standard JSON-RPC error objects. Reserved codes: `-32800` cancelled,
`-32001` needs setup (the message is shown with a button that opens the
plugin's settings), `-32002` insufficient credits, `-32003` rate limited (`data.retry_after` in seconds).

## Hosting rules

- One plugin process per plugin, started on first use and kept alive; a crash
  is reported and the plugin restarts on its next use.
- The host answers plugin requests on the UI thread between frames; a plugin
  must not expect sub-frame latency.
- Jobs run in the background and the editor remains usable. Only one job per
  document is in flight; a job is cancelled if its document tab closes.
- Pixels never leave the user's machine unless the plugin sends them somewhere;
  the permissions dialog says which hosts a plugin declared.
- `host/run` may call built-in commands (the identifiers in
  [SHORTCUTS.md](SHORTCUTS.md)) and other plugins' actions, so plugins compose;
  a future MCP server exposes the same registry to agents.
