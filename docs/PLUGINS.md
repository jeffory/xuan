# Plugins

Xuan plugins are separate programs that talk to the editor over JSON-RPC 2.0 on
their standard input and output. They can add menu actions that edit or generate
images, sidebar panes with their own user interface, file formats, and settings.
The editor owns every interaction with the user: a plugin declares what it needs
in a manifest, and Xuan draws the controls, runs the on-canvas tools, keeps
undo history, and asks for permissions. A plugin never touches the document
directly; it asks the host for pixels and returns results.

See [GENERATIVE.md](GENERATIVE.md) for how generative and ML features fit this system.

Any language works. The repository ships a Rust SDK crate (`sdk/xuan-plugin`), a
Python module with no dependencies (`sdk/python/xuan_plugin.py`), and example
plugins under `plugins/`:

| Folder | Language | Shows |
| --- | --- | --- |
| `plugins/histogram` | Python | A pane that follows the document, settings, data-URL images |
| `plugins/invert-regions` | Rust | A region action with per-region fields, a pane that reads the composite |
| `plugins/comfy-cloud` | Python | Network jobs with progress, cancel and errors; secrets; `ask` results; three actions |
| `plugins/local-upscale` | Python | A local, offline job with no permissions beyond reading; a swappable model backend |
| `plugins/select-bright` | Python | A `mask` result that becomes the selection; a placeholder for a segmentation model |

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

Plugins run as ordinary processes with the user's rights. No plugin starts
until the user allows it: Xuan shows its folder, its command and the
permissions it declares (a plugin that declares none still asks to run), and
records the grant in `config.toml`. Until then its panes show a "Review
Permissions…" button instead of starting it. If the folder, the command or the
permissions change, the plugin asks again. Folders that share a plugin id are
reported and none of them is loaded.

Xuan enforces the permissions for what it does on a plugin's behalf: it edits
documents only for plugins that declare `document = "edit"` (see
[What `document` allows](#what-document-allows)), hands over only
the declared secrets, and reads and writes files only in the plugin's own
folders (see [Files](#files)) unless `filesystem` allows more. It does not
limit what the plugin process itself does, which runs with the user's rights:
it can read any file the user can. A plugin that declares network hosts can
contact any server, not only those; the permission dialog says so, and Xuan
asks before it hands such a plugin document data. On Linux, a plugin that
declares no hosts can be kept off the network altogether (see
[Network](#network)); elsewhere, or with that setting off, nothing stops it
from connecting either. Install plugins you trust.

### What `document` allows

| | `document = "read"` (default) | `document = "edit"` |
| --- | --- | --- |
| `document/get`, `layer/export`, `document/export`, `selection/export` | yes | yes |
| Action results (`image`, `document`, `mask`, `edit`, `text`), as a proposal the user accepts or discards | yes | yes |
| `document/edit`, including its `set_selection` op | no | yes |
| `host/run` commands that edit the document | no | yes |

A `mask` result changes only the selection, never pixels, and only after the
user accepts it, so it needs no `document = "edit"`: a read-only segmentation
plugin can propose a selection. Changing the selection directly with
`document/edit` still needs `"edit"`.

### Network

A plugin that lists hosts under `permissions.network` is treated as one that
sends data off the machine. A plugin that lists none gets no prompts; on
Linux, Xuan can block its network (see [Blocking the
network](#blocking-the-network)), otherwise nothing stops it from connecting.

**Asking before sending.** When an action of such a plugin would send
document data, Xuan shows **Send to *Plugin (plugin id)*?** before anything
is sent. It names the declared hosts and lists exactly what goes with this
run: the source image (the active layer's pixels, the flattened image, or the
flattened image inside the selection, with any crop around the regions and
the `max_side` limit), the selection as a mask when the action asks for one
(only if something is selected), the positions and sizes of the regions and their text
fields, and each non-empty `text`, `multiline` or `path` input. Other inputs
are listed by name, and the document's size and layer names and positions
(which `action/run` always carries) are mentioned. An action that sends none
of this data runs without a prompt.

- **Send** starts the job. While a job the user confirmed runs, the plugin
  may also export other layers or the whole image without another prompt.
- **Cancel** sends nothing. An action with inputs keeps its dialog open; one
  without inputs is dropped.
- **Don't ask again for this plugin** (applied with **Send**) is stored as
  `send_without_asking = true` in the plugin's grant in `config.toml`. It goes
  away with the grant: when the folder, command or permissions change and the
  plugin is reviewed again, when the user revokes it, or when the user presses
  **Ask Again** in **Plugins → Manage Plugins…**.

Until the user chose "Don't ask again", `action/estimate` reaches such a
plugin with `source: null` and without its regions and `text`, `multiline`,
`path` or `secret` inputs, since it is sent as soon as the dialog opens.

**Exports outside an action.** `layer/export`, `document/export` and
`selection/export` from a plugin that declares network hosts are answered at
once while one of its confirmed jobs runs or after "Don't ask again".
Otherwise, for example from a pane, the request is held unanswered and Xuan
asks once, as soon as no other dialog is open, naming what was asked for.
The answer covers every export the plugin asks for until its process stops
(it is disabled, revoked, reloaded, crashes or Xuan quits); a refused export
fails with `-32800`. `document/get`, the `document` field of `action/run` and
`pane/render`, and `document/changed` carry only the document's structure
(size, layer ids, names and positions, the selection's bounds), so they are
not held. Saving through a plugin's file format is not prompted either: the
user chose that format in the Export dialog.

**Offline mode.** **Disable plugins that use the network**, in **Settings →
General** and at the bottom of **Plugins → Manage Plugins…**, is stored as
`disable_network_plugins = true` in `config.toml`. While it is on, plugins
that declare network hosts are stopped and never started: their panes say
why they are empty, their actions are disabled in the menus, the command
palette and shortcuts, and their file formats are not offered. Plugins that
declare no hosts keep working. Turning it off starts their panes again.
Offline mode relies on the declaration, so together with blocking the network
of the other plugins (below) it is a guarantee on Linux; without it, a
plugin that declares no hosts could still connect.

#### Blocking the network

**Block network for plugins that don't declare it**, in **Settings →
General** and at the bottom of **Plugins → Manage Plugins…**, is off by
default for now. Once changed it is stored as `block_undeclared_network =
true` (or `false`) in `config.toml`; while the key is absent the release's
default applies, so a later release can turn it on for everyone who never
chose. It works on Linux only (x86_64 and aarch64); elsewhere it is shown
greyed out and has no effect.

While it is on, a plugin whose manifest lists no `permissions.network` hosts
starts under a seccomp filter that Xuan installs in the plugin process just
before it runs the plugin's command. The filter cannot be removed and is
inherited by every process the plugin starts. In it:

- `socket()` and `socketpair()` fail with `EACCES` (Python raises
  `PermissionError`) for every address family except `AF_UNIX` and
  `AF_NETLINK`: TCP and UDP over IPv4 and IPv6, raw and packet sockets,
  Bluetooth, VSOCK and the rest. **This includes `localhost`**: a plugin that
  talks to a server on the same machine, such as ComfyUI or Ollama, must
  declare it, for example `network = ["localhost"]`, and is then
  treated as a network plugin.
- Unix sockets (`AF_UNIX`) keep working, for local IPC and
  `multiprocessing`; the standard input and output pipes Xuan talks to the
  plugin over are not sockets and are not affected. Netlink sockets only talk to
  the local kernel, never to another machine, and C libraries use them to
  list network interfaces, so they stay allowed too.
- `io_uring_setup()` fails with `EACCES`, since an io_uring can open sockets
  without calling `socket()`.
- The plugin and its subprocesses cannot gain privileges, for example through
  setuid programs such as `sudo` or `ping` (`no_new_privs`).
- System calls through another ABI than the native 64-bit one (i386 `int
  0x80`, arm32 compat) kill the process, so 32-bit plugin programs do not run
  under the filter. The x32 syscall numbers are blocked like the native ones.

Running plugins that declare no hosts restart when the setting changes. The
permission dialog and Manage Plugins show **Network blocked by Xuan (Linux)**
for such plugins, and the plugin's log starts with a note saying the network
is blocked. A blocked call is not reported otherwise.

If the filter cannot be used, because the kernel is older than Linux 4.14 or
was built without seccomp, or because a container or another sandbox
forbids it, the plugin does not start and the error says why. It is never
run unfiltered while the setting is on.

The filter keeps a plugin from opening network connections itself; it is
not a full sandbox. The plugin still runs with your rights, and a program it
reaches over a Unix socket (the D-Bus session bus, the systemd user manager,
a local proxy or the Docker socket) can still connect, or start an
unfiltered process, on its behalf. Plugins that declare hosts are never
filtered: Xuan cannot limit them to their hosts, and the send prompt is
their control.

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
network = ["cloud.comfy.org"]     # hosts it says it connects to (not enforced); see "Network"
secrets = ["api_key"]             # settings of type "secret" it receives
document = "edit"                 # "read" (default) or "edit"
filesystem = "none"               # "none" (default), "read" or "write": where the host reads and writes for it

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
shortcut = "Ctrl+Alt+E"           # optional; see "Shortcuts" below
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
Xuan creates with owner-only permissions on Unix. Secrets are kept in plain
text there rather than in the operating system's keyring: the keyring crates
need a desktop secret service on Linux, which headless sessions and CI lack,
so a plain-file fallback would be needed anyway. A `secrets.toml` that cannot
be read is reported by line number only, without its contents, and is never
overwritten; fix or remove it and press **Reload**. Secrets reach a plugin over
its stdin, never through its environment or command line. The SDKs keep
secret values out of debug output. A plugin receives its settings
with `initialize` and again through `settings/changed`; secrets are included
only when the manifest lists them under `permissions.secrets`.

### Shortcuts

An action's `shortcut` names `Ctrl`, `Shift` and `Alt` modifiers and one key,
such as `Ctrl+Alt+E`. Letters, digits and other keys need Ctrl or Alt; only the
function keys `F1`–`F24` may be used alone or with Shift. Chords match their
modifiers exactly, so `Ctrl+Shift+E` and `Ctrl+E` are different shortcuts. A
shortcut Xuan already uses (see [SHORTCUTS.md](SHORTCUTS.md)), including one
the user assigned, or that an earlier plugin claimed, is ignored and reported in
**Plugins → Manage Plugins…**; the action stays available from its menu. Users
can change or remove an action's shortcut in **Settings → Keyboard Shortcuts**,
where it is stored as `"<plugin>/<action>"`.

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

#### Selection mask

An action that edits only the selected area (inpainting) sets
`source.mask = "selection"`. The host then writes the current selection to
`selection.png` in the job's work directory and gives its path as
`source.mask` in `action/run` (and `action/estimate`, once the user agreed to
send document data). The mask is an 8-bit grey PNG with **exactly the size of
the source export**: the same crop, the same `max_side` scaling and the same
pixels, so pixel `(x, y)` of the mask covers pixel `(x, y)` of `source.path`.
White is selected, black is not, grey is partly selected. It works with every
`source.from` except `none`; with `selection` the crop is the mask's bounds.

```toml
source = { from = "composite", max_side = 2048, mask = "selection",
           mask_grow = 4, mask_feather = 6, mask_empty = "error" }
```

- `mask_grow` grows (positive) or shrinks (negative) the selection by that many
  document pixels, with a square reach. Selections touching the document edge
  do not shrink from the edge.
- `mask_feather` softens the edge with a blur of that radius in document
  pixels. Grow is applied first, then feather. Both are applied on the
  host before export, and both are clamped to 256 (larger values in a manifest
  are rejected). A `selection` source is cropped to the grown and feathered
  mask, so the extra margin is not cut off.
- `mask_empty` says what happens when nothing is selected. `"error"` (the
  default) refuses to start the action and says "Select an area first", before
  anything is sent. `"white"` sends an all-selected mask and `"black"` an
  all-unselected one, for actions where the mask is optional.
- `mask_grow`, `mask_feather` and `mask_empty` need `source.mask`, and
  `source.mask` needs an `edit` action with a source.

The mask is document data: a network plugin's send prompt lists "The selection,
as a mask", and the file lives only in the job's work directory, which the
host deletes with the job. The Rust SDK reads it with
`job.selection_mask_path()` and the Python SDK with `job.selection_mask_path`.
A plugin can return the same file as the `mask` of its image output so the
result layer is masked by the selection (see `plugins/comfy-cloud`). `regions`
stay the right tool for several separate edits with their own text; the
selection mask is for one area.

`result.into` chooses where image outputs go: `layer` (a new layer above the
source, the default), `replace` (the source layer's pixels), `document` (a new
tab), or `ask` (the dialog offers **New layer** / **New document**). With `mask_to_regions`, a new layer gets a mask built from the
regions with a soft edge, so only the parts the user asked to change show
through and the rest can be painted back.

Results never land silently. An edit arrives as a **proposal**: the layer is
added, a bar above the canvas offers **Compare**, **Accept** and **Discard**,
and only accepting commits a single undo step named after the action. Every
generated layer records which plugin, action, inputs and source produced it, so
**Layer → Re-run Plugin Action…** can repeat it with changes. Inputs taken from a
project file or from `host/run` are checked against the action's inputs first:
a value of the wrong type or not among an `enum`'s values falls back to the
default, numbers are clamped to `min` and `max`, texts are cut to 64 KiB, and
regions keep only finite coordinates, declared fields, and at most `max` (and
never more than 256) regions. See
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

### Files

The host reads and writes files for a plugin only inside its folders: the
plugin folder, `data_dir`, the scratch folder that exports go to by default, and
the `work_dir` of its running jobs and imports. Paths are resolved first, so a
symlink cannot lead out of them. This covers the images and masks of results,
`document/edit` and imports, `host/open` paths, and the `dir` of exports. With
`filesystem = "read"` the host reads files anywhere; with `filesystem =
"write"` it also exports into any folder. A request outside these folders is
refused with an invalid-params error.

### Actions

`action/run` is sent once per invocation:

```json
{"jsonrpc":"2.0","id":7,"method":"action/run","params":{
  "job": "0c2d…", "action": "precise-edit", "work_dir": "/tmp/xuan/jobs/0c2d…",
  "inputs": {"model": "ideogram-4.5", "seed": 1475826651,
             "regions": [{"index": 1, "x": 333, "y": 500, "width": 111, "height": 143,
                          "mask": null, "fields": {"desc": "Change the earring", "type": "obj"}}]},
  "source": {"path": "/tmp/xuan/jobs/0c2d…/source.png", "width": 896, "height": 1152,
             "layer": "6f0a…", "scale": 0.5, "offset": {"x": 120, "y": 80},
             "mask": "/tmp/xuan/jobs/0c2d…/selection.png"},
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
`mask` PNG and `name`; see below for its placed size), `document` (a PNG opened as a new tab), `mask` (a grey
PNG that becomes the selection, see below), `edit` (a list of
document edits, see below, applied as one undo step), `text` (shown in the
status bar) and `none`. One result may hold at most 64 outputs, 32 new layers
and documents, and 1,000 edits, and the images it refers to may add up to at
most 100 megapixels; a larger result is refused as a whole. The same layer,
edit and pixel limits apply to one `document/edit` request, and an import may
return up to 1,000 layers within 100 megapixels. `action/estimate` with the same params may be answered
with `{cost: "≈18 credits", seconds: 20}`; the dialog shows it before running.

**Placed size.** An `image` normally lands at its pixel size divided by the
export scale, so a 2x upscale would cover twice the area of its source. Give
it a size instead and the pixels are fitted to that size: `width` and/or
`height` in document units (one side alone keeps the aspect ratio), or
`"fit": "source"` to cover the bounds of the source that was sent, at `x`,`y`
from its top-left. A higher-resolution result then sits exactly over the
source with a higher pixel density. `fit` cannot be combined with `width` or
`height`, and an action without a source cannot use `fit`. Sizes must be
finite, above 0 and at most 30,000 document units; the image's own pixels
still count against the size and 100-megapixel limits. With `result.into =
"replace"` the result is resampled to the size it is placed at, in the source
layer's pixels. The SDKs have helpers: `job.image(path, fit_source=True)` or
`width=`/`height=` in Python, and `Output::image(..).fit_source()` or
`.with_size(width, height)` in Rust.

**Masks.** A `mask` output turns a grey PNG into the document's selection:

```json
{"kind": "mask", "path": "…/mask.png", "mode": "replace", "fit": "source"}
```

White is selected, black is not, and grey is partly selected (Xuan's
selection has 256 levels, so a soft matte stays soft); a colour PNG is read
as its luminance. `mode` combines it with the current selection like the
selection tools do: `replace` (the default), `add` (the larger coverage),
`subtract` (the current coverage minus the mask's) or `intersect` (the
smaller). It is placed exactly like an `image`: at `x`,`y` in source
coordinates, at its pixel size divided by the export scale unless it gives
`width`/`height` or `"fit": "source"`, so a model can return a 320x320 matte
for any source. It is resampled smoothly onto the document's pixels, and
nothing outside it is selected. Several masks apply in order. The selection
changes only through a proposal: the new selection shows at once,
**Compare** shows the old one, **Accept** makes it one undo step named after
the action and **Discard** restores the old one. A mask is read from the
plugin's folders and counts against the result's 100-megapixel budget like
an image; a file that is missing, not an image or larger than 30,000 pixels
on a side refuses the whole result. It needs no `document = "edit"` (see
[What `document` allows](#what-document-allows)). The SDKs have helpers:
`job.mask(path, mode="add", fit_source=True)` in Python (and
`encode_gray_png` to write one), and `Output::mask(path,
MaskMode::Add).fit_source()` in Rust.

### Reading and editing the document

Plugins ask the host for data with these requests. Each is answered on the next
frame, except that an export from a plugin that declares network hosts may
wait for the user's answer (see [Network](#network)).

| Request (plugin → host) | Params | Result |
| --- | --- | --- |
| `document/get` | — | `{id, width, height, resolution, active, selection: {x, y, width, height} \| null, layers: [{id, name, kind, visible, locked, opacity, blend, parent, x, y, width, height, rotation, generated?}]}` |
| `layer/export` | `{layer, what: "pixels" \| "mask", max_side?, dir?}` (`dir`: one of the plugin's folders) | `{path, width, height, x, y, scale}` |
| `document/export` | `{max_side?, dir?}` | `{path, width, height, scale}` |
| `selection/export` | `{dir?}` | `{path, x, y, width, height}` or `null` |
| `document/edit` | `{name, edits: [ … ]}` | `{ok: true}`; needs `document = "edit"` |
| `host/run` | `{action, inputs?}` | runs an allowed host command, or one of the plugin's own actions as `<plugin>/<action>` with `inputs` pre-filled |
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
`{message}`. The status bar shows a plugin's message, like the `text` output of
a job, on one line after the plugin's name and id, as in `Mock (plugin mock):
message`, so it cannot pass for Xuan's own. Notifications from the host: `document/changed` `{id, revision}`, sent to
every running plugin after each edit of the current document (any plugin may
read the document with `document/get`, so this reveals nothing more), and
`settings/changed` `{settings, secrets}`.

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

Trees are limited: 16 levels deep and 2,000 nodes, where every list item,
select option and swatch counts as a node; texts are cut to 4 KiB, ids,
colors and suffixes to 256 bytes, links to 2 KiB, and sizes to 4,096 points.
Image files must be PNGs inside the plugin's folders (see [Files](#files)), at
most 32 MiB and 8,192 pixels per side; an image that fails to load is not
retried until its file changes. `pane/update` is ignored for panes the
manifest does not declare.

### File formats

| Request (host → plugin) | Params | Result |
| --- | --- | --- |
| `format/import` | `{format, path, work_dir}` | `{width, height, resolution?, layers: [{name, image, x?, y?, mask?, opacity?, blend?, visible?}]}` |
| `format/export` | `{format, path, image, document}` | `null` |

Declared extensions appear in the Open, Import and Export dialogs; a built-in
format always wins over a plugin's.

### Errors

Standard JSON-RPC error objects. Reserved codes: `-32800` cancelled (also the answer to an export the user refused),
`-32001` needs setup (the message is shown with a button that opens the
plugin's settings), `-32002` insufficient credits, `-32003` rate limited (`data.retry_after` in seconds).

## Hosting rules

- One plugin process per plugin, started on first use and kept alive; a crash
  is reported and the plugin restarts on its next use.
- The editor never waits for a plugin. It sends `initialize` and holds back
  everything else for that plugin until the answer arrives; a plugin that
  does not answer within 20 seconds is stopped and reported with the end of
  its log. `format/import` and `format/export` run in the background like
  jobs, can be cancelled, and fail after 5 minutes without an answer.
- Stopping a plugin closes its stdin and kills its whole process tree (its
  process group on Unix, its Job Object on Windows) shortly after.
- The host answers plugin requests on the UI thread between frames; a plugin
  must not expect sub-frame latency.
- Jobs run in the background and the editor remains usable. Only one job per
  document is in flight; a job is cancelled if its document tab closes.
- Wherever a plugin's own words appear, Xuan says which plugin they come
  from: menu items and the shortcut list show `Action label · Plugin name`
  (hover a menu item for the plugin's id and folder), and permission
  prompts, errors and proposals name the plugin with its id, for example
  `Mock (plugin mock)`.
- Pixels never leave the user's machine unless the plugin sends them somewhere.
  Xuan asks before handing document data to a plugin that declares network
  hosts, offline mode keeps such plugins from running, and on Linux Xuan can
  block the network of plugins that declare none (see [Network](#network)).
- `host/run` may call the view commands `fit`, `actual`, `zoom_in` and
  `zoom_out`. A plugin that declares `document = "edit"` may also call
  commands that make one undoable document edit: `undo`, `redo`, `new_layer`,
  `duplicate`, `delete_layer`, `group`, `ungroup`, `move_out`, `merge`,
  `flatten`, `mask`, `new_mask_layer`, `delete_mask`, `disable_mask`,
  `link_mask`, `clip`, `select_all`, `deselect`, `invert_selection`,
  `fill_fg`, `fill_bg`, `clear`, `invert`, `flip_h`, `flip_v`,
  `flip_canvas_h` and `flip_canvas_v`. Commands that open, save, export or
  close files, use the clipboard, change settings or manage plugins are
  refused, and a plugin can start only its own actions.
