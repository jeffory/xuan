# Plugins

Xuan plugins are separate programs that talk to the editor over JSON-RPC 2.0 on
their standard input and output. They can add menu actions that edit or generate
images, sidebar panes with their own user interface, file formats, and settings.
The editor owns every interaction with the user: a plugin declares what it needs
in a manifest, and Xuan draws the controls, runs the on-canvas tools, keeps
undo history, and asks for permissions. A plugin never touches the document
directly; it asks the host for pixels and returns results.

See [GENERATIVE.md](GENERATIVE.md) for how generative and ML features fit this system,
and [MCP.md](MCP.md) for the MCP server plugin that lets LLM clients drive Xuan.

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
| `plugins/extend-edges` | Python | Outpainting: an extended source, `extend_canvas` in a result, an image fitted to the new canvas; a placeholder for a generative model |
| `plugins/mcp-server` | Rust | A background server (`on_start`), direct edits behind the session prompt (`edit_prompt = "session"`), consented saving and opening, copy buttons; lets MCP clients drive Xuan (see [AGENTS-GUIDE.md](AGENTS-GUIDE.md)) |

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
from other places, for example a development checkout. Folders whose name starts
with `.` are skipped.

### Bundled plugins

Some plugins come with Xuan: today the [MCP server](MCP.md)
(`plugins/mcp-server`). The release packages put them in a bundled plugins
folder that Xuan finds next to its own executable, and Xuan loads them after
the user's plugins:

| Package | Bundled plugins folder |
| --- | --- |
| deb, rpm | `/usr/lib/xuan/plugins/` |
| Linux tar.gz | `lib/xuan/plugins/` in the extracted folder; `scripts/install.sh` copies it to `<prefix>/lib/xuan/plugins/` (default `~/.local/lib/xuan/plugins/`) |
| AppImage | `usr/lib/xuan/plugins/` inside the image (`AppRun` names it in `XUAN_BUNDLED_PLUGINS`, as Xuan is started through the dynamic loader there) |
| Windows zip | `plugins\` next to `xuan.exe` |

In general it is `<prefix>/lib/xuan/plugins` for `<prefix>/bin/xuan` on Linux,
and `plugins` beside `xuan.exe` on Windows.

**A bundled plugin is off until you allow it.** It asks for permission the
first time it starts, with the same prompt and the same permissions as any
other plugin; nothing in the bundled folder runs on its own. Its grant stores
the folder by name (`<bundled>/mcp-server` in `config.toml`) rather than by
path, so it carries over when Xuan is upgraded, moved, or its AppImage is
mounted somewhere else. The command and permissions are still checked: an
update that changes either asks again.

**Your own copy wins.** A plugin with the same id in the user plugins directory
or on `XUAN_PLUGIN_PATH` replaces the bundled one, which is then not loaded and
not reported as a conflict. That holds even when your copy fails to load, so a
broken copy shows its error instead of the bundled plugin quietly running in its
place. This is how to run a newer build of a bundled plugin: install it with
**Install from Folder or Zip…** (the bundled copy does not block that), or copy
it into the user plugins directory. Your copy has another folder, so it asks
for permission again, and a stored secret for the bundled copy is not handed
to it. Remove your copy to go back to the bundled one. The installer never
writes into the bundled folder; it changes only when Xuan is updated.

### Install from Folder or Zip

**Plugins → Install from Folder or Zip…** (also **Install…** in Manage
Plugins, or drop a folder or `.zip` on either window) installs a plugin into
the user plugins directory as `<plugins dir>/<id>/`. The folder or archive
holds `plugin.toml` at its top level or inside a single top-level folder (as
GitHub's "Download ZIP" makes them).

1. Xuan copies the folder, or extracts the archive, into a private temporary
   folder (readable only by you) and checks it there. Nothing is copied into
   the plugins directory yet.
2. It reads the manifest with the same rules as at start-up, including
   [`requires_xuan`](#compatibility). The command must name a file inside the
   plugin folder (it may not exist yet; see [Setup
   convention](#setup-convention)) or one of the interpreters `python3`,
   `python`, `py`, `uv`, `node`, `deno`, `bun`, `ruby`, `perl`, `sh`, `bash`,
   `pwsh` or `powershell`, and its arguments may not be absolute paths or
   contain `..`.
3. A review shows the plugin's name, id, version, where it comes from, where it
   goes, its command and the permissions it declares, with the same network
   wording as the permission prompt. **Install** (or **Update**) copies it;
   **Cancel** copies nothing.
4. The checked copy, exactly what was reviewed, is built in a hidden folder
   next to the target and renamed into place, so a failed install leaves
   nothing half-copied. Then the plugins are reloaded and Manage Plugins shows
   the new plugin.

**Installing does not allow the plugin to run.** It asks for permission the
first time it starts, like any other plugin, and that prompt shows the folder it
was installed to. The review says so.

**Updating.** If `<plugins dir>/<id>/` already holds a plugin with the same id,
the review offers **Update**, which stops the plugin and replaces the whole
folder (files the old version or its setup created there are gone). The grant
works as always: it is kept when the folder, command and permissions are
unchanged, and otherwise the plugin asks again; the review says which. A folder
named after the id that holds something else, and a plugin with the same id in
another folder (for example on `XUAN_PLUGIN_PATH`), refuse the install: the
same id in two folders loads neither. A [bundled plugin](#bundled-plugins)
with the same id does not refuse it: the installed copy replaces it.

**Archives are untrusted.** An archive is refused if it has:

- an entry with an absolute name, a drive letter or UNC prefix, a `..`
  component, a name Windows cannot create (`CON`, `aux.txt`, `a:b`, `x.`), or a
  control character;
- a symbolic link, device, pipe or socket (only plain files and folders);
- two names that differ only by letter case, a name that is both a file and a
  folder, or a repeated name;
- more than 10,000 entries, more than 512 MiB in total or 256 MiB in one file
  once extracted, an encrypted entry, or an entry over 1 MiB that is more than
  100 times its compressed size (a zip bomb).

Files keep their executable bit from the archive's Unix mode (or the folder's)
and become `rwxr-xr-x` or `rw-r--r--`; setuid, setgid and sticky bits are never
kept. A folder is held to the same size and type rules. `__MACOSX` entries are
skipped. There is no registry and no signing: install plugins from sources you
trust.

### Compatibility

`requires_xuan` in `[plugin]` is an optional [semver](https://semver.org) range
of the Xuan versions the plugin works with, such as `">=0.3, <0.5"` or `"^0.3"`.
A plugin whose range excludes the running Xuan is not loaded, and not
installed, with a message naming both, for example "plugin requires Xuan
>=0.4, but this is Xuan 0.3.0". A pre-release build counts as its release
(`0.4.0-dev` as `0.4.0`). `protocol` is checked separately.

### Setup convention

Plugins that need a Python virtual environment, packages or model files ship a
**setup script** that the user runs once after installing (and again after an
update): `setup.sh` (Linux and macOS) and `setup.ps1` (Windows), or a single
`setup.py`. Xuan never runs it; the plugin's README says to. A setup script:

- runs from the plugin folder and creates the environment there, for example
  `python3 -m venv .venv && .venv/bin/pip install -r requirements.txt`, with
  pinned versions (ideally `--require-hashes`);
- leaves model files to Xuan: declare them in `[[models]]` (see
  [Models](#models)) and Xuan downloads and verifies them into the plugin's
  models folder, which survives updates. Other large downloads go in the
  plugin's data folder, `<config dir>/plugin-data/<id>/` (the `data_dir` the
  plugin gets in `initialize`), so that an update, which replaces the plugin
  folder, keeps them, each checked against a SHA-256 pinned in the script;
- is safe to run again, and prints what it did.

The manifest's command then points into the environment, for example
`command = [".venv/bin/python", "main.py"]` (`.venv\Scripts\python.exe` on
Windows). Such a command is accepted at install time although the file does not
exist yet, and the permission prompt shows it. Until the setup has run, starting
the plugin fails with an error in its log; a plugin can instead start with
`python3` and report a missing environment as a setup error with a clear
message. Network use by the setup script is not covered by the plugin's
permissions or Xuan's network blocking, since Xuan does not run it.

Plugins run as ordinary processes with the user's rights. No plugin starts
until the user allows it: Xuan shows its folder, its command and the
permissions it declares (a plugin that declares none still asks to run), and
records the grant in `config.toml`. Until then its panes show a "Review
Permissions…" button instead of starting it. If the folder, the command or the
permissions change, the plugin asks again. Folders that share a plugin id are
reported and none of them is loaded, except that a copy outside the bundled
folder replaces a [bundled plugin](#bundled-plugins).

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
| Action results that only change the selection or open a new document: `mask`, `document` (and an `image` with `result.into = "document"`), a `set_selection` edit, `text`; always a proposal the user accepts or discards | yes | yes |
| Action results that change the open document's pixels or layers: an `image` placed as a layer or replacing the source, any other `edit` op, `extend_canvas` | no | yes |
| `document/edit`, including its `set_selection` op | no | yes |
| `extend_canvas`, in `document/edit` or in a result's `edit` output | no | yes |
| `host/run` commands that edit the document | no | yes |
| `document/list`, `document/activate`, and `file/save_as`, `file/export` and `file/open` after the user's choice | yes | yes |

A plugin with `document = "read"` can read the document and propose
selections or new documents, but it can't change your image. A `mask` result
changes only the selection, never pixels, and only after the user accepts it,
so it needs no `document = "edit"`: a read-only segmentation plugin can
propose a selection. A `set_selection` edit in a result is treated the same
way, and a new document leaves the open one alone. Anything that would change
the open document's pixels or layers (a new layer, a replaced layer, any
other `edit` op, `extend_canvas`) needs `"edit"`: Xuan refuses the whole
result with an error that names the permission, and nothing is proposed.
Changing the selection directly with `document/edit` still needs `"edit"`.

### Edit sessions

A plugin that edits on someone else's behalf, such as the MCP server whose
edits come from an LLM client, can ask Xuan to check with the user before
its direct edits: `edit_prompt = "session"` under `[permissions]` (it needs
`document = "edit"`). Then `document/edit` and the `host/run` commands that
edit wait, unanswered, until the user answers **Allow *Plugin (plugin id)* to
edit your documents for this session?**, which names the first edit:

- **Allow** applies it and every later direct edit of the session.
- **Deny** refuses them with `-32800` for the rest of the session. For 30
  seconds after a Deny the plugin cannot ask again: edits of its other
  sessions are refused at once, without a prompt, so a client cannot wear the
  user down by starting new sessions. With **With Deny: refuse every session
  until the plugin stops** checked, Deny refuses all of its sessions, those
  already allowed included, until its process stops.
- **Always Allow** turns on **auto mode**: stored as `edit_without_asking =
  true` in the plugin's grant in `config.toml`, it skips the prompt from then
  on. Like "Don't ask again" for sending, it belongs to the grant and goes
  away when the plugin's folder, command or permissions change or the grant
  is revoked. **Plugins → Manage Plugins…** shows it as **Edit without asking
  (auto mode)**, a switch the user can turn off at any time (which forgets the
  sessions already allowed).

A session is the plugin's process: every answer is forgotten when it stops. A
plugin serving several clients names each one's session with a `session`
string (at most 128 characters) in its requests, so each client is asked
separately; requests without one share the process's session. The SDKs add
it for you: `host.with_session(id)` in both. `session/status` (with the same
`session`) answers `{edit_prompt, edits: "allowed" | "denied" | "ask",
auto}`, so a pane can show the state. At most 64 edits of a plugin wait for
the answer; more fail with `-32003`. Edits that wait keep their order and are
applied, each as its own undo step, once the user allowed them. Action
results are not affected: they stay proposals the user accepts or discards,
and reading the document is never held by this prompt.

The manifest is checked too: with `document = "read"`, an action that writes
`result.into = "layer"`, `"replace"` or `"ask"` is rejected when the plugin is
loaded. Leave `result` out for an action that returns only masks, or use
`"document"`. Because a grant is bound to the permissions it was given for,
changing `document` in a manifest asks the user to review the plugin again.

### Network

A plugin that lists hosts under `permissions.network` is treated as one that
sends data off the machine. A plugin that lists none gets no prompts; on
Linux, Xuan can block its network (see [Blocking the
network](#blocking-the-network)), otherwise nothing stops it from connecting.

**Asking before sending.** When an action of such a plugin would send
document data, Xuan shows **Send to *Plugin (plugin id)*?** before anything
is sent. It names the declared hosts and lists exactly what goes with this
run: the source image (the active layer's pixels, the flattened image, or the
flattened image inside the selection, with any crop around the regions,
the new canvas of `source.extend` ("extended by 64 px on the left, …") and
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
  treated as a network plugin. The same goes for a plugin that **listens**
  on this computer, such as the MCP server (`network = ["127.0.0.1"]`):
  declared hosts are not matched against the addresses a plugin uses, and a
  socket filter cannot tell a listening loopback socket from a connection to
  another machine, so any declared host exempts the plugin from the filter.
  Such a plugin should still listen on `127.0.0.1` only and check who
  connects (see `plugins/mcp-server`).
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
requires_xuan = ">=0.3, <0.5"     # optional semver range of Xuan versions; see "Compatibility"

[permissions]
network = ["cloud.comfy.org"]     # hosts it says it connects to (not enforced); see "Network"
secrets = ["api_key"]             # settings of type "secret" it receives
document = "edit"                 # "read" (default) or "edit"
filesystem = "none"               # "none" (default), "read" or "write": where the host reads and writes for it
edit_prompt = "none"              # "none" (default) or "session": see "Edit sessions"

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

[[models]]                        # files Xuan downloads for the plugin; see "Models"
id = "u2net"
url = "https://example.com/models/u2net.onnx"
sha256 = "<64 hex digits>"
size = 175997641
license = "Apache-2.0"
source = "U²-Net (Qin et al., 2020)"
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

#### Extending the canvas (outpainting)

An action that paints beyond the image's edges sets `source.extend` on a
`composite` source. Each side is a number of document pixels, or the id of
an `integer` or `number` input of the action that holds it, so the user can
choose it in the dialog:

```toml
source = { from = "composite", max_side = 2048,
           extend = { left = "amount", top = 0, right = "amount", bottom = 128 } }
```

- The source image is the flattened document padded with transparent pixels
  by those amounts: its top-left is at (−left, −top) in the document, which
  `action/run` reports as `source.document_x` and `document_y`. The amounts
  reach the plugin as `source.extend` (`{left, top, right, bottom}`, in
  document pixels).
- The host also writes `extend.png` to the job's work directory and gives
  its path as `source.extend_mask`: an 8-bit grey PNG of **exactly the size
  of the source export**, white over the new canvas and black over the old
  image. Export pixels that `max_side` scaling makes straddle the old edge
  count as new. With `source.mask = "selection"`, the selection mask uses the
  same grid and is black over the new canvas.
- The extended size is checked against the document limits (30,000 pixels a
  side, 100 megapixels) before anything is exported; a larger extension
  refuses to start. Sides in the manifest may be at most 30,000, and input
  values are rounded and clamped to 0..30,000.
- `extend` needs an `edit` action with `from = "composite"`. Sending an
  extended source needs only `document = "read"`, but growing the canvas
  with the result needs `document = "edit"`.

The result grows the canvas with an `extend_canvas` edit (see [Reading and
editing the document](#reading-and-editing-the-document)) and places the
painted image with `"fit": "source"`. "Source" means the bounds of the
source as it was sent, which for an extended source include the new canvas.
`image` and `mask` outputs are always placed on the document as it was when
the job started; when the result's edits extend the canvas, they move with
the content by the edits' `left` and `top`, like every existing layer. So an
image fitted to an extended source lands exactly on the new canvas once the
result extends it by the same amounts, at any `max_side`. Without the edit
the same image hangs over the old canvas's edges. Returning `extend.png` as
the image's `mask` keeps the original pixels visible under the new layer:

```json
{"outputs": [
  {"kind": "edit", "edits": [{"op": "extend_canvas", "left": 64, "top": 0, "right": 64, "bottom": 128}]},
  {"kind": "image", "path": "…/outpainted.png", "fit": "source", "mask": "…/extend.png"}
]}
```

The SDKs read the amounts and mask with `job.extension` and
`job.extend_mask_path` in Python (`job.extension()` and
`job.extend_mask_path()` in Rust), and write the edit with
`job.extend_canvas(**job.extension)` in Python or
`Output::edit(vec![edits::extend_canvas(margins)])` in Rust. See
`plugins/extend-edges`.

`result.into` chooses where image outputs go (a plugin with `document = "read"`
may only use `document`; the others change the open document and need
`document = "edit"`): `layer` (a new layer above the
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

### Models

A plugin that runs a local model declares the model files in `[[models]]`
instead of downloading them itself. Xuan downloads each one once, checks it,
keeps it in the plugin's **models folder** and passes its path to the plugin.
The plugin never downloads its declared models, and needs no network
permission for them.

```toml
[[models]]
id = "u2net"                                   # [a-z0-9_-]+, unique in the plugin
url = "https://example.com/models/u2net.onnx"  # https only, no user name or password
sha256 = "<64 hex digits>"                     # of the file, checked while downloading
size = 175997641                               # exact size in bytes, at most 8 GiB
file = "u2net.onnx"                            # optional; default: the URL's file name, or the id
license = "Apache-2.0"                         # optional, shown before downloading
source = "U²-Net (Qin et al., 2020)"           # optional, shown before downloading

[[actions]]
id = "select-subject"
label = "Select Subject"
models = ["u2net"]                             # the models this action needs
```

A plugin declares at most 16 models, with distinct ids and file names. A file
name is plain (letters, digits, `.`, `-` and `_`), not hidden and not ending
in `.part`. Prefer ONNX or safetensors files: loading a pickle (`.pt`,
`.pth`, `.ckpt`) runs code from the file. `license` and `source` are single
lines of at most 200 bytes.

**Downloading.** When an action that lists models runs and one of them is
missing or corrupt, Xuan asks first, listing each model with its size, host,
licence and source, with **Download** and **Cancel**; nothing is downloaded
without that answer. **Plugins → Manage Plugins… → Models** downloads models
the same way. Downloads run in the background with the other plugin jobs,
with progress and **Cancel**, and the action runs once its models are ready.
Each download:

- uses https only, with certificates checked against the system's trust
  store; at most 5 redirects are followed, each to https only;
- times out when connecting or waiting for an answer, and otherwise takes as
  long as the declared size needs on a slow link (it can always be cancelled);
- is written to `<models folder>/<id>.part` (owner-only on Unix), stopping as
  soon as it exceeds the declared size; then the size and SHA-256 are
  compared, the file is synced and renamed to its final name. A failed or
  cancelled download removes the `.part` file and leaves an earlier verified
  file in place;
- is never unpacked or run by Xuan: the plugin loads the file itself;
- does not happen in offline mode ("Disable plugins that use the network"),
  even for plugins that declare no hosts. Linux network blocking for plugins
  does not apply, since Xuan downloads, not the plugin.

**The models folder** is `<config dir>/plugin-data/<id>/models/` (inside
`data_dir`, so models survive plugin updates), created owner-only (0700) on
Unix. It belongs to Xuan: host-side file requests may read it but never write
into it, even with `filesystem = "write"`. Without a configuration folder it
is inside the private temporary data folder of the session.

**Verification before use.** Xuan records the SHA-256 of each file with its
size, modification and change times and inode. A model is handed to the plugin
only while its file still matches that record; a file of the wrong size is
**corrupt**, and one that changed since it was hashed is **not verified** and
is hashed again (in the background) before an action that needs it runs.

**In Manage Plugins**, the Models section of a plugin lists each declared
model with its status (not downloaded, downloading or verifying with a
percentage, ready, not verified or corrupt) and its size on disk, with
**Download**, **Verify**, **Delete** and **Delete All Models**, which removes
the plugin's whole models folder. The permission and install reviews list the
models a plugin declares, with their size and host.

### Providers

Xuan's own **Select → Subject**, **Filter → Remove Background** and the Magic
tool's **Object** mode use classical algorithms (a GrabCut graph cut; see
[USAGE.md](USAGE.md#selection-providers)); core Xuan has no machine learning.
A plugin can replace them, for example with a segmentation model, by declaring
which of its actions provides which **capability**:

```toml
[[provides]]
capability = "select_subject"     # or "object_select", "remove_background"
action = "segment"                # one of the plugin's [[actions]]
```

The user picks the provider for each capability under **Settings →
Selection** ("Built-in" by default); the choice is stored in `config.toml` as
`[providers]`, for example `select_subject = "my-segmenter"`, and a
configuration without the table uses the built-in algorithms. When a provider
is chosen, the command runs its action instead:

- The action runs **without its dialog**: its inputs take their defaults, and
  `inputs.capability` says which command it stands in for. For
  `object_select`, `inputs.point` (`{x, y}`, a click) or `inputs.rect`
  (`{x, y, width, height}`, a dragged box) give where the user pointed, in the
  pixels of the source the plugin was sent, like regions.
- The providing action must be an `edit` action with a source (`composite` is
  usual; `layer` suits `remove_background`) and may not have a `regions`
  input. A capability can be provided once per plugin; one action may provide
  several. The manifest is refused otherwise.
- The result's `mask` output (see [Masks](#sources-and-results)) is applied as
  the command's result, as a **proposal** the user accepts or discards. For
  `select_subject` it becomes the selection; for `object_select` it combines
  with the selection as the user's modifier keys asked (Shift adds, Alt
  subtracts), whatever `mode` the output says. Undo shows the command's name.
- For `remove_background`, Xuan lays the mask on the layer that was active,
  as its layer mask (together with any mask it has), and leaves the selection
  alone. That changes the document, so **`remove_background` needs `document =
  "edit"`**; `select_subject` and `object_select` only change the selection
  and work for `document = "read"` plugins. The plugin still returns only a
  mask: the host makes the layer mask, so a provider cannot touch pixels
  through this path.
- Every plugin rule applies as for any other run: the plugin must be allowed
  (a provider waiting for its permission prompt or model download continues
  as the provider once they are done), a plugin that declares network hosts
  asks before the source is sent, and model downloads are confirmed first.
- If the chosen plugin cannot run, because it is not installed, no longer
  provides the capability, is disabled, or uses the network while **Disable
  plugins that use the network** is on, the command uses the built-in
  algorithm and the status bar says why.

`plugins/select-bright` declares `select_subject` as an example; its README
and [GENERATIVE.md](GENERATIVE.md) describe how an ONNX segmentation plugin
fits this slot.

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
| `initialize` | `protocol`, `host: {name, version}`, `plugin_dir`, `data_dir`, `models_dir`, `models: {id: path}`, `settings`, `secrets` | `{protocol}` |
| `shutdown` | — | `null`; the process must exit |

`data_dir` is a per-plugin folder that persists between runs. Temporary files
for a job go in the `work_dir` the host passes with each job and are removed when
the job ends. `models_dir` is the plugin's [models folder](#models) and
`models` maps the id of each declared model that is downloaded and verified to
its file; a model that is missing, downloading or corrupt is left out. The
plugin process also gets `XUAN_PLUGIN_ID`, `XUAN_DATA_DIR` and
`XUAN_MODELS_DIR` in its environment. In the SDKs, `job.model_path("id")`
returns the path or fails with a setup error, and `plugin.model_path("id")`
(Python) or `settings.model_path("id")` and `host.model_path("id")` (Rust)
return it or nothing.

### Files

The host reads and writes files for a plugin only inside its folders: the
plugin folder, `data_dir`, the scratch folder that exports go to by default, and
the `work_dir` of its running jobs and imports. `models_dir` can be read but
is never written into. Paths are resolved first, so a
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
             "mask": "/tmp/xuan/jobs/0c2d…/selection.png",
             "extend": null, "extend_mask": null},
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
status bar) and `none`. An `image` or `document` output may also carry a
`provenance` object, see [Provenance](#provenance). One result may hold at most 64 outputs, 32 new layers
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
source with a higher pixel density. For an extended source those bounds
include the new canvas (see [Extending the
canvas](#extending-the-canvas-outpainting)). `fit` cannot be combined with `width` or
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

### Provenance

A generative plugin can say what it actually used. Add `provenance` to an `image`
(or `document`) output and the host stores it with the layer's `generated` record
(`.xuan` format 8, [FORMAT.md](FORMAT.md#model-provenance-version-8)), and **Generation** in
the layers panel shows it read-only with a **Copy** button for the JSON:

```json
{"kind": "image", "path": "…/result.png", "provenance": {
  "model": "sdxl_base_1.0.safetensors", "weights_sha256": "9e3c…(64 hex digits)",
  "sampler": "euler", "scheduler": "karras", "steps": 28, "seed": 1234, "cfg": 6.5,
  "service": "cloud.comfy.org", "request_id": "job-7f3a", "extra": {"lora": "detail-v2"}}}
```

Every key is optional: `model`, `model_hash`, `weights_sha256`, `sampler`,
`scheduler`, `steps`, `seed`, `cfg`, `service`, `request_id` and a free-form `extra`
object. The schema is strict: **unknown keys are rejected**, they do not fall back to
`extra` (put anything else there yourself). Strings are at most 256 bytes without control
characters, `steps` and `seed` are non-negative integers, numbers are finite, `extra` nests
at most four levels with at most 32 entries per object or list, and the whole record is at
most 8 KiB. A malformed record refuses the whole result with a message naming the problem;
`null` or `{}` means none. Only `image` and `document` outputs use it: a `mask` output
becomes a selection, not a layer, so there is nothing to attach it to. With `result.into =
"replace"` the record replaces the layer's previous one. Re-running an action records the
new result's own provenance.

**Secrets are never stored.** Before checking the schema, the host removes, at any depth:
any key whose name looks like a credential (it contains `api_key`, `token`,
`authorization`, `password`/`passwd`, `secret`, `credential`, `bearer`, `cookie` or
`private_key`, ignoring case and punctuation, so `max_tokens` goes too), a key equal to one of the plugin's declared secret
names, and any key, string or list item that contains one of the plugin's current secret
values (of four or more characters). The status line says how many entries were removed,
never their names or values, and error messages do not quote values. The check is a safety
net, not a licence: do not put credentials, signed URLs or prompts you consider private
into it. The SDKs have helpers: `job.image(path, provenance={...})` in Python and
`Output::image(..).with_provenance(json!({...}))` in Rust. `document/get` lists a layer's
`provenance` too.

### Reading and editing the document

Plugins ask the host for data with these requests. Each is answered on the next
frame, except that an export from a plugin that declares network hosts may
wait for the user's answer (see [Network](#network)).

| Request (plugin → host) | Params | Result |
| --- | --- | --- |
| `document/get` | — | `{id, width, height, resolution, active, selection: {x, y, width, height} \| null, layers: [{id, name, kind, visible, locked, opacity, blend, parent, x, y, width, height, rotation, generated?, provenance?}]}` |
| `layer/export` | `{layer, what: "pixels" \| "mask", max_side?, dir?}` (`dir`: one of the plugin's folders) | `{path, width, height, x, y, scale}` |
| `document/export` | `{max_side?, dir?}` | `{path, width, height, scale}` |
| `selection/export` | `{dir?}` | `{path, x, y, width, height}` or `null` |
| `document/edit` | `{name, edits: [ … ]}` | `{ok: true, layers: [id]}` (the layers it added); needs `document = "edit"` |
| `document/list` | — | `{documents: [{id, title, width, height, layers, current, modified, saved}]}`: the open tabs, without their paths |
| `session/status` | `{session?}` | `{edit_prompt, edits, auto}`: how direct edits are handled in the session; see [Edit sessions](#edit-sessions) |
| `document/activate` | `{document}` | makes an open document the current one, as clicking its tab does; a plugin may switch at most once a second (`-32003` with `retry_after` otherwise; naming the current document always succeeds) |
| `host/run` | `{action, inputs?}` | runs an allowed host command, or one of the plugin's own actions as `<plugin>/<action>` with `inputs` pre-filled |
| `host/open` | `{path}` or `{url}` | opens a file as a document or a URL in the browser |
| `file/save_as` | `{document?, suggested_name?}` | `{name}` (the file's name, not its folder) once the user saved the document as a project in the save dialog; see [Files the user chooses](#files-the-user-chooses) |
| `file/export` | `{document?, format?, suggested_name?}` | `{name}` once the user exported the document as an image (`png`, the default, `jpg`, `tiff` or `webp`) |
| `file/open` | `{path}` | `{ok: true, document}` once the user agreed to open the file named by the absolute `path` |

`document/edit` edits, applied together as one undo step named `name`:

- `{"op": "add_layer", "image": path, "name"?, "x"?, "y"?, "mask"?, "above"?, "opacity"?, "blend"?}`
- `{"op": "replace_pixels", "layer", "image", "x"?, "y"?}`
- `{"op": "set", "layer", "name"?, "visible"?, "locked"?, "opacity"?, "blend"?}`
- `{"op": "remove_layer", "layer"}`
- `{"op": "set_mask", "layer", "mask": path | null}`
- `{"op": "set_selection", "mask": path | null}`
- `{"op": "select", "layer"}`
- `{"op": "extend_canvas", "left"?, "top"?, "right"?, "bottom"?}`: grow the
  canvas by whole document pixels on each side (each 0 or more, default 0;
  shrinking is not offered). Layers, mask placements and guides move by
  `left`, `top` exactly as **Image → Canvas Size…** moves them with the
  matching anchor, and the selection is cleared, as Canvas Size does. The new
  size must stay within 30,000 pixels a side and 100 megapixels. Needs
  `document = "edit"`, also in a result's `edit` output.

Layers and their properties:

- `{"op": "select_layers", "layers": [id, …]}`: make these the selected
  layers (the last is active), for the `host/run` commands that work on the
  selected layers, such as `merge`, `group` and `duplicate`.
- `{"op": "merge_layers", "layers": [id, …]}`, `{"op": "group_layers",
  "layers": [id, …]}` and `{"op": "ungroup_layers", "layer"}`: **Layer →
  Merge** (one layer merges down, several merge together), **Group** and
  **Ungroup** on those layers in a single step; the merged layer and the new
  group are reported as added.
- `{"op": "transform", "layer", "x"?, "y"?, "width"?, "height"?, "rotation"?}`:
  move, scale or rotate a layer (a group with its layers) to this box in
  document units, as **Free Transform** does; missing fields keep their value.
  The layer must not be locked.
- `{"op": "add_empty_layer", "name"?, "above"?}` and `{"op": "add_mask_layer",
  "name"?, "above"?}`: an empty pixel layer, or a mask layer made from the
  selection (all white without one), the size of the canvas.
- `{"op": "add_text_layer", "text", "x"?, "y"?, "family"?, "size"?, "color"?,
  "bold"?, "italic"?, "underline"?, "strikethrough"?, "name"?, "above"?}`: an
  editable text layer with its top-left corner at `x`, `y`. `size` is in pixels
  (1–1024, default 48), the text at most 16 KiB; an unknown `family` falls back
  to Xuan's bundled font.
- `{"op": "add_shape_layer", "shape": "Rectangle" | "Ellipse" |
  "RoundedRectangle", "x", "y", "width", "height", "color"?,
  "corner_radius"?, "name"?, "above"?}`: an editable shape layer.
- `{"op": "add_adjustment_layer", "adjustment" | "filter", "name"?,
  "above"?}`: a non-destructive adjustment or filter layer, masked by the
  selection when there is one.

Pixels, inside the selection when there is one (on the given `layer`, which
becomes the active layer, or the active layer; it must be an unlocked pixel
layer, so text and shape layers become pixel layers):

- `{"op": "fill", "layer"?, "color"}`.
- `{"op": "stroke", "layer"?, "points": [[x, y], …], "color"?, "size"?,
  "hardness"?, "opacity"?, "erase"?}`: one brush stroke through the points (at
  most 10,000) in document coordinates; `size` is the brush diameter (1–2000,
  default 20), `hardness` and `opacity` 0–1 (defaults 0.8 and 1), and `erase`
  erases instead of painting. The layer grows to hold the stroke, as with the
  Brush tool.
- `{"op": "apply_filter", "layer"?, "filter"}` and `{"op":
  "apply_adjustment", "layer"?, "adjustment"}`.

Filters and adjustments are written as `.xuan` files store them on filter and
adjustment layers. Filters: `{"GaussianBlur": {"radius"}}` (0–100),
`{"MotionBlur": {"distance", "angle"}}` (0–200, ±180°), `{"Noise": {"amount",
"monochrome"}}` (0–100) and `{"LensCorrection": {"distortion", "vignette"}}`
(±50, ±100). Adjustments: `"Invert"`, `{"HueSaturation": {"hue", "saturation",
"lightness", "colorize"}}`, `{"Levels": {"black", "gamma", "white",
"output_black", "output_white"}}` (levels 0–255), `{"Curves": {"points": [{"x",
"y"}, …]}}` (0–1), `{"Exposure": {"exposure", "offset", "gamma"}}`,
`{"GradientMap": {"shadows": [r, g, b, a], "highlights": [r, g, b, a]}}`,
`{"Grain": {"amount", "monochrome", "seed"}}`, `{"FilmGrain": {"amount", "size",
"roughness", "seed"}}`, `{"BlackWhite": {"weights": [6 numbers], "tint",
"tint_hue", "tint_saturation"}}`, `{"ColorBalance": {"shadows": [3 numbers],
"midtones", "highlights", "preserve_luminosity"}}`, and the per-channel
`{"LevelsChannels": …}`, `{"CurvesChannels": …}` and `{"HueRanges": …}`. Every
field must be given; values outside the ranges the dialogs allow are refused.
Colours are `"#rrggbb"` or `"#rrggbbaa"` (default black).

The selection, combined with the current one by `mode` (`replace`, the
default, `add`, `subtract` or `intersect`):

- `{"op": "select_rect", "x", "y", "width", "height", "ellipse"?, "mode"?}`.
- `{"op": "select_polygon", "points": [[x, y], …], "mode"?}`: at least three
  and at most 10,000 points.
- `{"op": "select_color", "x", "y", "tolerance"?, "contiguous"?, "mode"?}`:
  the Magic Wand at a point of the flattened image (`tolerance` 0–255,
  default 32; `contiguous` default `true`).
- `{"op": "select_color_range", "colors": ["#rrggbb", …], "exclude"?,
  "fuzziness"?, "invert"?, "mode"?}`: **Select → Color Range…** over the
  flattened image (`fuzziness` 0–200, default 40).
- `{"op": "grow_selection", "by"}` grows (positive) or shrinks (negative) the
  selection by whole pixels, and `{"op": "feather_selection", "radius"}`
  softens its edge; both at most 256.

These change only the selection, so, like `set_selection`, a plugin with
`document = "read"` may return them in a result's `edit` output as a
proposal; sending them with `document/edit` needs `document = "edit"`.

The canvas (only with `document/edit`, never in a result, whose images are
placed on the canvas as it was sent; the selection is cleared or rescaled as
the menu commands do):

- `{"op": "crop", "x", "y", "width", "height"}`: as the Crop tool.
- `{"op": "resize_canvas", "width", "height", "anchor"?}`: **Image → Canvas
  Size…**, which may also shrink; `anchor` `[0, 0]` keeps the top-left corner,
  `[0.5, 0.5]` (the default) the centre.
- `{"op": "resize_image", "width", "height"}`: **Image → Image Size…**.

Edits apply in order, each in the document's coordinates at that point: an
`add_layer` after an `extend_canvas` is placed on the grown canvas. A batch
that fails anywhere changes nothing, and every edit respects the same limits
as the editor: locked layers, 30,000 pixels a side and 100 megapixels per
image, at most 1,000 edits and 32 new layers per batch, and the 100-megapixel
budget for the images it reads and the text and shapes it draws. Edits run on
the editor's thread, so a request (or a result, all its batches together)
also has a **work budget**, estimated before anything runs: about 1,000
million pixel visits, where a colour selection costs a render of every layer,
a filter, grow or feather costs the layer or canvas area several times over,
a stroke costs the area each segment sweeps, and a mask or adjustment layer
costs the canvas area; at most 256 MiB of new masks and drawn layers; and
strokes at most 200,000 pixels long in total. A request over budget is
refused as a whole, with an error that says so; send it in smaller parts.

### Files the user chooses

A plugin cannot save, export or overwrite a file on its own, and cannot open
one behind the user's back: `host/run` refuses `save`, `save_as`, `export`,
`open` and `close`. Three requests ask the user instead, and wait for the
answer:

- `file/save_as` shows the system's save dialog for the document (the
  current one, or `document`), titled with the plugin's name and id and
  prefilled with `suggested_name` and `.xuan`. The suggested name is reduced
  to a plain file name: no folders, extension, control characters, bidi
  controls or invisible characters, and a name Windows reserves for a device
  (`CON`, `NUL`, `COM1`, …) gets a leading `_`. The user chooses the folder
  and name, and the system dialog asks before replacing a file. The file is
  written exactly where the user confirmed: if the chosen name lacks `.xuan`,
  the dialog opens again in that folder with the extension added, and if the
  second answer lacks it too nothing is written (`-32602`). The project is
  saved there and from then on lives there, as with **File → Save As…**.
- `file/export` does the same for an image in `format` (`png`, `jpg`, `tiff`
  or `webp`); the document itself is not changed. The name the user confirms
  must end in `.png`, `.jpg`, `.jpeg`, `.tif`, `.tiff` or `.webp` (which picks
  the format written), with the same second dialog otherwise.
- `file/open` shows **Open a file?**, naming the plugin and the file's full
  path (symbolic links resolved), with **Open** and **Cancel**. The path must
  be absolute and name a regular file or a project folder. It opens as a new
  document, as **File → Open…** would.

`file/save_as` and `file/export` answer with the file's name only, never
the folder the user chose; `file/open` with the new document's id. Their
error messages name files the same way, without folders (a symbolic link's
target folder included). Each fails with `-32800` when the user cancels;
for 30 seconds after a cancel, the plugin's file requests fail at once
without a dialog.
Requests wait until no other dialog is open; a plugin has at most one waiting,
and a second fails at once. They need no `document = "edit"`, as they do not
change the open document, and the plugin learns nothing more than the name of the file the
user chose. To hand the user a file without a dialog, write it into the
plugin's own folders (for example with `document/export`) and show it in a
pane.

Notifications from the plugin: `host/log` `{level, message}` and `host/status`
`{message}`. The status bar shows a plugin's message, like the `text` output of
a job, on one line after the plugin's name and id, as in `Mock (plugin mock):
message`, so it cannot pass for Xuan's own. Notifications from the host: `document/changed` `{id, revision}`, sent to
every running plugin after each edit of the current document (any plugin may
read the document with `document/get`, so this reveals nothing more),
`settings/changed` `{settings, secrets}`, and `models/changed` `{models: {id:
path}}`, sent to a running plugin after one of its models was downloaded,
verified or deleted, with the same map as `initialize`.

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
| `button` | `id`, `label`, `primary?`, `enabled?`, `copy?` (text Xuan copies to the clipboard when the user clicks it; the button shows it as a tooltip and says how many lines it has) |
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
- Model files are downloaded only by Xuan, only after the user confirmed the
  list with sizes and hosts, over https, and are checked against the declared
  size and SHA-256 before the plugin gets their path (see [Models](#models)).
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
  `flip_canvas_h`, `flip_canvas_v`, `select_layer_pixels`,
  `select_mask_black`, `feather`, `select_subject`, `content_fill`,
  `remove_background` and `remove_flat_background`. The last four run in the
  background like their menu items and become one undo step when they finish;
  `select_subject` and `remove_background` use the provider the user chose
  (see [Providers](#providers)). A command that is greyed out in its menu, for
  example `merge` with a single layer, is refused. Commands that open, save, export or
  close files, use the clipboard, change settings or manage plugins are
  refused (see [Files the user chooses](#files-the-user-chooses) for saving
  and opening), and a plugin can start only its own actions.
