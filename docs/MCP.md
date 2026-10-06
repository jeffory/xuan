# MCP server: design

Design notes for issue #7: letting LLM clients (Claude Code, Claude Desktop,
Codex and others) drive Xuan through the [Model Context
Protocol](https://modelcontextprotocol.io). How to connect a client and what the
tools do is in [AGENTS-GUIDE.md](AGENTS-GUIDE.md); the plugin protocol is in
[PLUGINS.md](PLUGINS.md).

## Decisions

From the issue (2026-10-06):

1. **A first-party plugin, not core.** The server is `plugins/mcp-server`, an
   optional plugin like the generative ones (see [GENERATIVE.md](GENERATIVE.md):
   LLM features live in plugins). Core Xuan only gains generic,
   permission-gated host requests that any plugin may use.
2. **Local HTTP.** MCP Streamable HTTP served by the plugin on `127.0.0.1`
   only, with a bearer token and Host/Origin checks against DNS rebinding. The
   port and token are shown in the plugin's pane for copying.
3. **Server only.** Xuan using external MCP tools is a follow-up.
4. **Permissions.** Reading (list, inspect, preview) works once the client is
   connected with the token. The first edit of a session asks the user; an
   optional auto mode skips that prompt. Every edit is one undo step and shows
   live. Saving, exporting and opening files always go through the user.

## Architecture

```
MCP client ──HTTP, 127.0.0.1, bearer token──▶ mcp-server plugin ──JSON-RPC on stdin/stdout──▶ Xuan
```

The plugin is an ordinary plugin process: Xuan starts it after the user
allowed it, it gets no more access than its grant, and every request it makes
goes through the same checks as any other plugin's.

## Packaging

Every release package ships the plugin built in release mode (issue #51), in
the [bundled plugins folder](PLUGINS.md#bundled-plugins) Xuan finds next to its
executable: `/usr/lib/xuan/plugins/mcp-server/` for the deb and rpm,
`lib/xuan/plugins/mcp-server/` in the tar archive, `usr/lib/xuan/plugins/` in
the AppImage, and `plugins\mcp-server\` beside `xuan.exe` in the Windows zip.
The source archive has its source and lock file, and the SDK it builds on.

- **The same manifest everywhere.** The package keeps the repository's
  `plugin.toml` and its relative layout, so the command is
  `target/release/xuan-mcp-server` in a checkout and in every package. On
  Windows the file is `xuan-mcp-server.exe`; Rust's process spawning adds
  `.exe` to a program path without an extension.
- **Static on Linux.** The Linux packages build it for
  `x86_64-unknown-linux-musl`, with no shared library dependencies. The
  AppImage brings its own glibc for Xuan, but plugins are started with the
  host's loader and C library, which can be older than the build host's.
- **Off until allowed.** Bundled is not trusted: it asks for its grant like
  any plugin. The grant names the folder `<bundled>/mcp-server` instead of
  its path, so it outlives upgrades and the AppImage's per-launch mount
  point; the command and permissions are compared as always.
- **A user copy replaces it.** One installed in the user plugins directory or
  found on `XUAN_PLUGIN_PATH` loads instead, so a newer build can be tried by
  hand; it asks for its own grant.
- `scripts/check-packages.py` fails a package without the plugin, with a
  manifest that differs from the repository's, or (on Linux) with a
  dynamically linked plugin binary. The MCP client never talks
to Xuan directly. The plugin translates each MCP tool call into one or a few
host requests and turns the answers into MCP results. Because the editor
answers plugin requests between frames on its UI thread, a tool call that edits
the document takes effect on the next frame and is drawn at once.

## Tools mapped to the plugin API

| MCP tool (issue) | Host request |
| --- | --- |
| list documents | `document/list` (new) |
| open a document | `file/open` (new): the user confirms the file |
| save / export a document | `file/save_as`, `file/export` (new): the system save dialog |
| switch document | `document/activate` (new) |
| list layers, inspect | `document/get` (layers with id, kind, name, visibility, lock, opacity, blend, parent, placement, mask, provenance) |
| get/set layer properties: name, visibility, lock, opacity, blend | `document/edit` `set` |
| … transform | `document/edit` `transform` (new) |
| create image layer | `document/edit` `add_layer` (a PNG the plugin writes), `add_empty_layer` (new) |
| create text / shape layer | `document/edit` `add_text_layer`, `add_shape_layer` (new) |
| create adjustment / filter layer | `document/edit` `add_adjustment_layer` (new) |
| create mask (layer) | `document/edit` `add_mask_layer` (new), `set_mask`; `host/run` `mask` |
| paint via strokes, fills and gradients | `document/edit` `stroke`, `fill`, `gradient` (new) |
| selections: rect, ellipse, polygon, by colour | `document/edit` `select_rect`, `select_polygon`, `select_color`, `select_color_range`, `grow_selection`, `feather_selection` (new); `host/run` `select_all`, `deselect`, `invert_selection`, `select_subject`, `select_layer_pixels` |
| apply filters and adjustments | `document/edit` `apply_filter`, `apply_adjustment` (new) |
| merge / group | `document/edit` `merge_layers`, `group_layers`, `ungroup_layers` (new); `host/run` `flatten` |
| reorder layers | `document/edit` `move_layer` (new) |
| crop / resize canvas | `document/edit` `crop`, `resize_canvas`, `resize_image` (new), `extend_canvas` |
| several edits as one undo step (#61) | one `document/edit` with every edit; `"$n"` names a layer added earlier in the request (new) |
| undo / redo | `host/run` `undo`, `redo` (already allowed) |
| resources: manifest, thumbnails, preview, selection mask | `document/get`; `layer/export` with `max_side`; `document/export`; `selection/export` |
| provenance | `document/get` lists each generated layer's `provenance` |
| run a plugin action | not exposed: `host/run` only starts the calling plugin's own actions |

Panes give the plugin its status and connection page; settings hold the port.

## Gaps and how they were filled

Everything the issue lists could already be read, but much could not be
changed without a menu dialog. Each gap got a generic host capability, gated
like the existing ones, rather than anything specific to MCP:

- **Filters and adjustments by name.** The menu commands open dialogs, so
  `host/run` cannot use them. New `document/edit` ops `apply_filter`,
  `apply_adjustment` and `add_adjustment_layer` take the filter or adjustment
  as `.xuan` files store it, checked against the same ranges as the dialogs.
- **Layer creation.** `add_text_layer`, `add_shape_layer`, `add_empty_layer`
  and `add_mask_layer` make editable layers like the tools do; text is drawn
  with the editor's own renderer and counts against the pixel budget.
- **Transforms.** `transform` sets a layer's (or a group's) box like Free
  Transform.
- **Painting.** `stroke` paints or erases one brush stroke through a list of
  points, with the Brush tool's coverage rules; `fill` fills the selection;
  `gradient` fills it with a linear or radial gradient through any number of
  colour stops, or paints the mask with one.
- **Selections.** `select_rect` (with `ellipse`), `select_polygon`,
  `select_color` (the Magic Wand) and `select_color_range` (#5's Color Range
  engine) combine with the selection by `mode`; `grow_selection` and
  `feather_selection` modify it. They change only the selection, so a
  `document = "read"` plugin may also return them as a proposal, like
  `set_selection`.
- **Several layers at once.** `merge_layers`, `group_layers` and
  `ungroup_layers` work on the layers they name in one undo step; the
  `host/run` commands would need a separate step to select the layers first.
  `select_layers` selects layers for the other commands that work on the
  selection of layers (duplicate, delete).
- **Copying from a pane.** A pane `button` may carry `copy` text that Xuan
  puts on the clipboard when the user clicks it, for the connection details.
- **Canvas.** `crop`, `resize_canvas` and `resize_image`. They are accepted
  only in `document/edit`, not in action results, whose images are placed on
  the canvas as it was sent.
- **More `host/run` commands** flagged `Edit` in the command registry, where a
  menu command already makes one undoable edit without a dialog:
  `select_layer_pixels`, `select_mask_black`, `feather`, `select_subject`,
  `content_fill`, `remove_background` and `remove_flat_background`. `host/run`
  now refuses commands that are greyed out in their menu.
- **Knowing what was added.** `document/edit` answers with the ids of the
  layers it added.
- **Batching** (#61). Painting detail one tool call at a time took hundreds
  of round trips and undo steps. `paint_stroke` takes `strokes`, sent as one
  `stroke` edit each in one request (a one-point stroke already painted a
  dab). The `batch` tool runs each step's tool only as far as the edits it
  would send, checks them all, and sends them as one `document/edit`, so the
  batch is one undo step and Xuan applies all of it or none. Tools that are
  not edits (reads, files, `host/run` commands, undo and redo) are refused as
  steps rather than run outside the undo step. A step cannot know the id of a
  layer an earlier step creates, so `document/edit` gained layer
  references: `"$n"` in a layer field is the n-th layer the request has
  added so far, resolved by Xuan as the edit runs. The plugin passes
  references through untouched. When a request of several edits fails, Xuan
  names the edit (`Edit 3 (stroke): …`), which the server maps back to the
  step.
- **Documents.** `document/list` and `document/activate`.
- **Undo/redo** needed nothing: they were already `Edit` commands.

All of this needs `document = "edit"` except the selection ops in a result and
`document/list`/`document/activate`. Each `document/edit` request is applied
to a copy and kept only if every edit succeeds, so a request is one undo step
or nothing.

## Opening, saving and exporting

The rule from #33 (F2) stands: a plugin can never save or overwrite a file
silently, and `host/run` keeps refusing `save`, `save_as`, `export`, `open` and
`close`. A plugin that drives the editor still needs to deliver its work, so
three requests route the decision through the user:

- **`file/save_as` and `file/export` open the system's save dialog**, titled
  with the plugin's name and id and prefilled with a suggested name. The user
  picks the folder and the name, or cancels; the system dialog asks before it
  replaces a file. The plugin cannot choose the folder, so it cannot write
  outside what the user picked, and the suggested name is reduced to a plain
  file name. This is the same decision the user makes for **Save As…**, made
  at the moment the plugin asks.
- **`file/open` asks in a prompt that names the file**, with its full path
  after resolving symbolic links, and opens it only on **Open**. Reading the
  file needs no `filesystem` permission because the user approved that file.

Hardening from the review (#49):

- **The path the user confirmed is the path written.** Xuan used to add the
  extension to a name chosen without one, which could replace a file next to
  the one the dialog asked about. Now the dialog opens again, in the chosen
  folder, with the extension added; a second name without it writes nothing.
- **Suggested names** lose bidi controls and invisible characters (which can
  make `gpj.exe` read as `exe.jpg`), and names Windows reserves for devices
  (`CON`, `NUL`, `COM1`, …, whatever the extension) get a leading `_`.
- **No folders in errors.** Xuan's errors for `file/*` requests name files
  only, and the MCP server also reduces any absolute path in an error it
  passes on (from Xuan, the system or itself) to its file name.
- **Request sizes.** rmcp caps HTTP bodies at 4 MiB by default, below the
  64 MiB image `create_image_layer` documents, so large images failed with a
  bare 413. The server now accepts bodies up to 96 MiB (a 64 MiB PNG as base64
  plus the JSON around it), answers a larger declared length with a 413 that
  states the limits, and refuses a tool whose request to Xuan would exceed
  Xuan's 16 MiB plugin message limit with a tool error, rather than sending
  it (Xuan stops a plugin that writes a longer line). Only clients with the
  token get as far as sending a body.
- **Switching documents** (`document/activate`) is limited to once a second
  per plugin, so a client cannot flip the user's tabs under their hands.
  Switching is a view change, like clicking a tab, so it is throttled rather
  than gated behind the edit prompt.

Alternatives considered: an export confined to the plugin's own folders already
exists (`document/export` writes a PNG into its folders) and is enough for
previews, but it does not put a file where the user wants it, and a plugin
handing such a file back for the user to accept would just be a second save
dialog. A per-plugin allow-list of folders was rejected: it is a standing
permission for silent writes, which is exactly what F2 rules out.

The requests wait until no other dialog is open, a plugin has at most one
waiting, and a cancel is the error `-32800`, so a misbehaving client can
neither stack up dialogs nor learn anything beyond the name of the file the user chose.

## Calls that wait for the user (#57)

The edit prompt, the file prompts and dialogs, and the prompt to send an
image can keep a tool call waiting for minutes. MCP clients give up much
sooner: Claude Code reported `The operation timed out` after about a minute,
while the prompt stayed up in Xuan and a late answer still opened the file or
applied the edits the client had counted as failed. Three changes keep the
client and Xuan in step:

- **Progress keeps the client waiting.** When the client sends a
  `progressToken` in `_meta`, the server sends `notifications/progress`
  every 5 seconds while a call runs (`progress` counts up, no `total`, a
  `message` such as "Waiting for the user to allow edits in Xuan"). Such a
  call waits up to 10 minutes. The first notification also opens the HTTP
  answer: on the stateless protocol (2026-07-28) rmcp sends no headers before
  the handler's first message, and Claude Code (2.1.29x) aborts a POST that
  has no response headers after 60 seconds (unless the server's `timeout` or
  `MCP_TOOL_TIMEOUT` is longer) with exactly that `The operation timed out`.
  Claude Code always asks for progress (its `callTool` passes `onprogress`),
  shows the messages, and resets its tool idle timeout (5 minutes for HTTP
  servers, `CLAUDE_CODE_MCP_TOOL_IDLE_TIMEOUT`) on each one; its wall-clock
  limit (the per-server `timeout` or `MCP_TOOL_TIMEOUT`, about 28 hours by
  default) is not extended by progress. The MCP specification lets a client
  reset its timeout on progress but says it should still enforce a maximum;
  the TypeScript SDK does so only with `resetTimeoutOnProgress`.
- **Cancelling withdraws the request in Xuan.** On `notifications/cancelled`,
  or when a stateless client closes its HTTP request, the server withdraws the
  call's waiting request with the plugin protocol's `request/cancel` (see
  "Withdrawing a request" in `docs/PLUGINS.md`). Xuan drops the request and
  its prompt, so a late **Allow** or **Open** does nothing and the prompt does
  not come back after another dialog. A save dialog already open cannot be
  withdrawn: it is the system's, modal, and the save happens if the user
  confirms it. On the legacy session protocol rmcp keeps a call running when
  its HTTP stream drops (the client may resume it), so only the client's
  `notifications/cancelled` or the time limit withdraw it there.
- **The agent is told.** The descriptions of the edit and file tools say the
  call waits for the user and that, if it fails because the user did not
  answer, the agent should ask the user instead of retrying. Without a
  `progressToken` nothing keeps the client waiting, so the server gives up
  after 50 seconds, before the common 60-second client timeout, withdraws the
  request and answers with a tool error saying the user has not answered yet
  and nothing was changed.

The server does not limit waiting calls to one per kind. Edits an agent sends
in parallel in one turn wait behind the same prompt and apply in order once
the user allows them, which is what the agent meant; what stacked up before
were retries of calls the client had already given up on, and those are now
withdrawn when the client gives up. Xuan still caps waiting edits at 64 per
plugin and file requests at one.

## Edit permission per session

Edits through `document/edit` and editing `host/run` commands are immediate,
not proposals: that is what makes an agent useful, and each request is still
one undo step. Since an MCP client may act on instructions the user did not
write (a prompt injection in a web page it read, for example), a plugin may
ask Xuan to gate those edits: `edit_prompt = "session"` in `[permissions]`.

- The first edit of a session holds the request and asks **Allow *Plugin* to
  edit your documents for this session?** with **Allow**, **Deny** and
  **Always Allow (Auto Mode)**.
- A session is the plugin's process lifetime, divided further by an optional
  `session` id the plugin sends with its requests. The MCP server never
  passes on what a client claims: a client's `Mcp-Session-Id` counts only if
  this server issued it (it watches the answers to `initialize` go out), and
  Xuan is sent a label the server made ("MCP client 3"), never client text.
  Each client that starts a session with `initialize` (protocol revisions
  before 2026-07-28) is asked separately. Requests without an issued session,
  including every client on the stateless 2026-07-28 revision, which has no
  sessions, share **one** edit session, and the prompt says so ("every MCP
  client without a session (they share this answer)"): allowing it allows all
  of them until the plugin stops.
- **Allow** and **Deny** hold for that session. **Always Allow** stores
  `edit_without_asking = true` in the plugin's grant, like "Don't ask again"
  for sending; it goes away with the grant when the plugin's folder, command or
  permissions change, and **Plugins → Manage Plugins…** shows it with a
  switch to turn it off.
- Action results keep their per-result **Accept**/**Discard** proposals; the
  session answer covers only direct edits.

**The token is the trust boundary for sessions.** A client that holds the
token and knows another client's session id can send requests in that
session and use its answer. Binding the answer to more than the id is not
practical: Streamable HTTP has no notion of a connection (clients pool
connections, reconnect and may send each request on a new one), rmcp offers
no hook to rotate an id during a session, and the protocol has no client
identity a server could check. The id itself is a random UUID (122 bits)
that only that client and the server see, over loopback; a program that
could read it (by reading the client's memory or traffic) runs as the user
and could read the token file too. Two things limit the damage: **New
Token** also forgets every session id issued so far, so earlier answers
cannot be reused by whoever gets the new token (requests carrying an old id
count as having no session, and labels are never reused), and every session
ends when the plugin stops.

**Providers.** `run_command` `remove_background` and `modify_selection`
`subject` are the `host/run` commands `remove_background` and
`select_subject`, and wait for the session's edit answer like every other
edit. When the user chose a provider plugin for them (see
[PLUGINS.md](PLUGINS.md#providers)), that plugin's action runs exactly as
from the menu: it must itself be allowed, a provider
that declares network hosts asks before the image is sent to it, its model
downloads are confirmed, and its result is a proposal the user accepts or
discards. The MCP server gains no access to the provider plugin, and the
provider none to the MCP client; offline mode makes the command fall back to
the built-in algorithm.

## Network and the sandbox

The server listens on `127.0.0.1`, so the plugin declares `network =
["127.0.0.1"]`. Declared hosts are not matched against anything: any declared
host makes a plugin a network plugin. That is right here, as the MCP client
usually sends what it reads (previews included) to a model in the cloud:

- the plugin is not started under the Linux seccomp filter (#43), which blocks
  every AF_INET and AF_INET6 socket, loopback included, and cannot tell a
  listening loopback socket from a connection to another machine;
- previews and exports outside an action need the send consent (#39) once per
  process, or "Don't ask again";
- offline mode ("Disable plugins that use the network") stops it.

## Not in v1

MCP client support (Xuan calling external MCP tools), stdio transport, running
other plugins' actions from MCP, watching `.xuan` files on disk, and MCP
sampling or elicitation.
