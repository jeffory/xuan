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
| save / export a document | `file/save_as`, `file/export` (new): the system save dialog, or with `path` Xuan's own prompt (#54); `file/save` saves in place |
| switch document | `document/activate` (new) |
| list layers, inspect | `document/get` (layers with id, kind, name, visibility, lock, opacity, fill, blend, parent, placement, flips, masks, the image an effect is attached to, shape style, text and its path, provenance) |
| choose the active layer | `document/edit` `select_layers` (MCP `select_layers`); `host/run` with `layers` (MCP `run_command` `layers`, `modify_selection` `layer`) |
| get/set layer properties: name, visibility, lock, opacity, fill, blend, clipping | `document/edit` `set` (`clip_to`: a layer or group id below in the same folder, or `null` to release) |
| … transform | `document/edit` `transform` (new) |
| create image layer | `document/edit` `add_layer` (a PNG the plugin writes), `add_empty_layer` (new) |
| create text / shape layer | `document/edit` `add_text_layer` (with an SVG `path` for text on a path), `add_shape_layer` (new) |
| edit text, set it on a path (#64) | `document/edit` `set_text` (new) |
| create adjustment / filter layer | `document/edit` `add_adjustment_layer` (new) |
| create mask (layer) | `document/edit` `add_mask_layer` (new), `set_mask`; `host/run` `mask` |
| paint via strokes, fills and gradients | `document/edit` `stroke` (with `points` or an SVG `path`), `fill`, `fill_path`, `gradient` (new) |
| vector paths (#62) | SVG path data in `select_path`, `stroke` `path`, `fill_path`, `add_shape_layer` `Path`; `add_path` keeps a named path with the document (new) |
| selections: rect, ellipse, polygon, path, by colour | `document/edit` `select_rect`, `select_polygon`, `select_path`, `select_color`, `select_color_range`, `grow_selection`, `feather_selection` (new); `host/run` `select_all`, `deselect`, `invert_selection`, `select_subject`, `select_layer_pixels` |
| apply filters and adjustments | `document/edit` `apply_filter`, `apply_adjustment` (new) |
| merge / group | `document/edit` `merge_layers`, `group_layers`, `ungroup_layers` (new); `host/run` `flatten` |
| reorder layers | `document/edit` `move_layer` (new) |
| crop / resize / rotate / trim canvas | `document/edit` `crop`, `resize_canvas`, `resize_image` (new), `extend_canvas`, `rotate_canvas`, `trim`; `host/run` `crop_to_selection` |
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
  An out-of-range setting is refused with the field and its range. The MCP
  tools declare them as a string or an object, accept the same JSON sent as
  a string (`"\"Invert\""`), and list every variant Xuan accepts; a test in
  `src/plugins/edits.rs` reads serde's list of variants and fails when the
  tool descriptions miss one. Upstream Compositor's Vignette, Bloom / Glow, Tonal
  Contrast and Dither are among them (`{"Vignette": …}`, `{"Bloom": …}`,
  `{"TonalContrast": …}`, `{"Dither": …}`), with upstream's ranges and
  defaults as [FORMAT.md](FORMAT.md#compositor-filters-version-14) lists them;
  a Dither may leave out any setting to take its default.
- **Layer creation.** `add_text_layer`, `add_shape_layer`, `add_empty_layer`
  and `add_mask_layer` make editable layers like the tools do; text is drawn
  with the editor's own renderer and counts against the pixel budget.
- **Transforms.** `transform` sets a layer's (or a group's) box like Free
  Transform.
- **Painting.** `stroke` paints or erases one brush stroke through a list of
  points, with the Brush tool's coverage rules; points may carry pen pressure
  (`[x, y, pressure]`), and the brush dynamics (#63: taper, spacing, scatter,
  size/opacity/hue jitter with a seed) are optional fields; `fill` fills the selection;
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
- **Canvas.** `crop`, `resize_canvas`, `resize_image`, `rotate_canvas` and `trim`. They are accepted
  only in `document/edit`, not in action results, whose images are placed on
  the canvas as it was sent.
- **More `host/run` commands** flagged `Edit` in the command registry, where a
  menu command already makes one undoable edit without a dialog:
  `rotate_canvas_cw`, `rotate_canvas_ccw`, `rotate_canvas_180`, `crop_to_selection`,
  `select_layer_pixels`, `select_mask_black`, `feather`, `select_subject`,
  `content_fill`, `remove_background` and `remove_flat_background`. `host/run`
  now refuses commands that are greyed out in their menu.
- **Knowing what was added.** `document/edit` answers with the ids of the
  layers it added, and so does `host/run` for built-in commands (`duplicate`,
  `new_layer`, `mask`, …), with `running` true when the command started a
  job that is still running (`content_fill`, `remove_background`,
  `remove_flat_background`). The MCP tool then says so: until the job ends,
  other edits fail with "The editor is busy".
- **Choosing the layer.** The `host/run` commands act on the selected layers,
  the last one active. `host/run` takes `layers`, which it selects first as
  clicking them would, so `run_command` and `modify_selection layer_pixels`
  can name their layers in one call instead of depending on whatever is
  active; a refused command leaves the selection as it was. The MCP tool
  `select_layers` (over the `select_layers` edit op) selects layers on their
  own, for the user's view or before several commands.
- **State a client could not see.** `document/get` reports shape layers as
  `"kind": "shape"` with their style, each layer's `flip_x` and `flip_y`, and
  masks: an image's mask is a child layer of kind `mask` (made by the `mask`
  command), so the image lists its mask layers under `masks` with their
  `enabled` and `linked` state, and each effect layer attached to an image
  (mask, adjustment or filter) names it in `attached_to`. `layer/export`
  with `what = "mask"` on an image reads its attached mask layer and says
  which in `mask_layer`.
- **Brush dynamics** (#63). Every stroke an agent drew was the same width
  from end to end, and stars, dust or a dotted trail took one call or one
  stroke per dab. `paint_stroke` points may be `[x, y, pressure]`; pressure
  goes through the tablet's path (it scales the size between samples, and
  the opacity with `pressure_opacity`). `taper_in`/`taper_out` taper without
  pressure, `spacing` paints separate dabs, and `scatter`, `scatter_count`,
  `size_jitter`, `opacity_jitter` and `hue_jitter` vary them. The random
  numbers are a hash of `seed` and the dab's index along the stroke, so a
  replay paints identical pixels. Each dab is painted as a zero-length
  segment by the existing brush, so the GPU and CPU paths and the stroke's
  coverage rules are shared and agree. All are off by default; a stroke that
  uses none of them is painted exactly as before.
- **Brush flow** (#104). A stroke laid its whole opacity down at once, so
  building paint up gradually took one stroke per coat. `paint_stroke` (at
  the top level and in each item of `strokes`) and the `stroke` edit take
  `flow` (0–1, default 1), the Brush's Flow: below 1, each dab moves a pixel's
  coverage a share of the way to its own (Photoshop's flow under an opacity
  cap), shared so that one pass lays down `flow` at any spacing, and going
  back over a spot builds up to `opacity`. A flow below 1 paints dabs 0.1 of
  the size apart unless `spacing` says otherwise. At 1 a stroke paints
  exactly as before.
- **Fill opacity** (#105). A layer could only be faded as a whole, effects
  included, so text with only its stroke showing, or a Hard Mix layer softened
  into a contrast boost, could not be made. `set_layer` takes `fill` (0–1),
  Photoshop's Fill on pixel, text and shape layers: it fades the layer's own
  pixels but not its layer effects, and in the eight modes Photoshop treats
  specially (Color Burn, Linear Burn, Color Dodge, Linear Dodge, Vivid Light,
  Linear Light, Hard Mix, Difference) it weakens the blend instead of fading
  it. `get_document` reports each layer's `fill`.
- **Vector paths** (#62). Every curve had to be computed outside Xuan and
  sent as dense point lists, and changing one meant recomputing it. Agents
  write SVG path data easily, so the tools take it: `select_shape` with
  `shape: "path"` selects inside a path (antialiased, with `feather` and
  `mode`); `paint_stroke` takes `path` in place of `points` (one per stroke,
  also in `strokes`), which Xuan flattens to points within 0.2 pixels, so the
  brush dynamics and limits are those of a point stroke; `fill` with `path`
  sends `fill_path`; `create_shape_layer` with `shape: "path"` makes an
  editable, antialiased vector shape layer (`ShapeKind::Path`), redrawn from
  its outline when it is resized and saved in `.xuan` format 10; and
  `save_path` keeps a named path with the document for the Paths dialog.
  Parsing is Xuan's own (so errors name the command, what was expected and the
  character), and the geometry (flattening, exact curve bounds, SVG arcs) is
  the kurbo crate's. `get_document` lists the saved paths and each path
  shape's outline in document coordinates. `select_shape` also takes
  `feather` for rectangles, ellipses and polygons.
- **Text on a path** (#64). Letters along a curve took one text layer per
  letter, each placed and rotated by hand from a spline sampled outside Xuan,
  and the line could no longer be edited as text. `create_text_layer` takes
  `path` (SVG path data in document pixels, which places the layer) and
  `path_options` (`start_offset`, `align`, `side`, `letter_spacing`,
  `rotate`, `baseline_shift`, `size_end`, `opacity_start`, `opacity_end`), so
  one call sets "17 letters shrinking from 17 to 7 px and fading from 95% to
  55%" along the curve as one editable layer. `set_layer` takes `text`,
  `path` (`null` puts the text back in a box) and `path_options` (merged into
  the layer's options), sent as the new `set_text` edit before any placement.
  Glyph positions and advances come from cosmic-text and become distances
  along the flattened path; `get_document` describes each text layer's text
  and its path in document coordinates. Saved as `.xuan` format 11.
- **Paint symmetry** (#66). The 12 rays of a starburst were drawn one at a
  time, and a mirrored figure meant computing x → 512 − x for every point.
  `paint_stroke` (at the top level and in each item of `strokes`) and the
  `stroke` edit take `symmetry: {mode: "vertical" | "horizontal" | "radial",
  segments?, center?: [x, y]}`, the Symmetry menu of the Brush, Pencil and
  Eraser. The stroke's pieces (swept segments or dabs, with their scatter and
  jitter) are laid out once and each is painted for every copy, mirrored or
  turned around the centre (the canvas centre by default), so copies are
  exact mirror images and one stroke's coverage keeps crossing copies from
  darkening each other. The work budget and stroke length limit count every
  copy. Without `symmetry` a stroke paints exactly as before.
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
- **Letters in their own font or colour** (#110). A word with one red letter
  took a text layer per colour, placed side by side by hand. `create_text_layer`
  and `set_layer` take `runs` (`start`, `end` in Unicode code points, and any of
  `family`, `color`, `bold`, `italic`), sent as `runs` on `add_text_layer` and
  `set_text`; new `text` keeps each unchanged letter's style, and `runs: []`
  clears them. `get_document` lists each text layer's `runs`. Saved as `.xuan`
  format 17.
- **Documents.** `document/list` and `document/activate`.
- **Undo/redo** needed nothing: they were already `Edit` commands.

All of this needs `document = "edit"` except the selection ops in a result and
`document/list`/`document/activate`. Each `document/edit` request is applied
to a copy and kept only if every edit succeeds, so a request is one undo step
or nothing.

## Opening, saving and exporting

#33 (F2) ruled that a plugin can never save or overwrite a file silently;
#54 relaxed that into an opt-in the user grants per plugin (below).
`host/run` keeps refusing `save`, `save_as`, `export`, `open` and `close`. A
plugin that drives the editor still needs to deliver its work, so the file
requests route the decision through the user:

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
dialog. A per-plugin allow-list of folders was rejected for #33 as a standing
permission for silent writes; #54 below adds such a permission, without the
folder list, because agent workflows need it.

### Saving without the dialog (#54)

The save dialog kept every write in the user's hands, but an MCP client
cannot operate it, so a batch of edits and exports, or a save at the end of a
long task, needed someone at the computer for each file. The maintainer's
decision: ask the user the first time, and offer to always allow.

- **`path`.** `file/save_as` and `file/export` take an optional absolute
  `path` (and `overwrite`); `file/save` saves a document back to its own
  `.xuan` file, as Ctrl+S. Without `path`, the save dialog is unchanged.
- **Encoding (#115).** `export_document` and `file/export` also take
  `quality` (1–100, JPEG and lossy WebP) and `lossless` (WebP). WebP is lossy
  when `quality` is given without `lossless`; what is left out follows the
  user's **Export image** dialog, which a request never changes.
- **Xuan's own prompt.** In place of the system dialog Xuan shows **Save a
  file?** (or **Export an image?**, **Save the project?**), naming the plugin,
  the document, the file name, its folder (resolved), and whether it writes a
  new file or replaces one: **Save**, **Always Allow** or **Cancel**. It is a
  plugin prompt like **Open a file?**: it comes back after another dialog
  replaced it (#56), `request/cancel` closes it and a late answer does
  nothing (#57), and the MCP server keeps the client waiting with progress
  notifications meanwhile.
- **Always Allow** is `save_without_asking` in the plugin's grant, next to
  `edit_without_asking`. It shows as **Save and export without asking** in
  **Plugins → Manage Plugins…**, where it can be turned off, and is dropped
  when the plugin's folder, command or permissions change. The issue
  proposed limiting it to folders the user lists; that was not done, because
  the prompt already names the folder each time it asks, the parent must
  exist, and a folder list would be a second setting to explain. Writes are
  visible instead.
- **Overwrites.** Replacing a file always needs `overwrite: true`, prompt or
  not; without it the request fails before any prompt, so a client cannot
  replace a file by accident. With Always Allow, a write replaces a file
  without asking only if Xuan wrote that file for the same plugin since it
  started (through its own `file/save_as`, `file/export` or `file/save`,
  asked or not), or for `file/save`, the document's own project. A file the
  user saved or exported themselves, or one written for another plugin,
  shows the prompt again, which says it replaces the file. The review of the
  first version caught that a run-wide list let an agent replace a project
  the user had just saved (say `B.xuan`) with another document; the list is
  now per plugin, kept until Xuan quits and forgotten when the plugin's grant
  is revoked or changes. So the grant covers new files and the plugin's own
  output, never the user's work. Folders and symbolic links at the path are
  never replaced, and a file that appears at a path the prompt called new is
  not replaced.
- **Path rules.** The path must be absolute and its folder must exist; the
  file name must already be plain (no control, bidi or invisible characters,
  reserved characters, leading dot or Windows device name), and is refused
  rather than cleaned so the prompt names the file written; the extension
  must match the format (`.xuan`, or the image extension, which picks the
  format; a `format` that disagrees is refused). Errors name files, never
  folders, as before.
- **Visibility.** Each write made without asking is shown in the status bar
  ("Exported without asking: …"), in the plugin's log, and in the MCP Server
  pane's recent tool calls ("export_document: exported out.png without
  asking"); the pane also says when Always Allow is on. Answers carry
  `asked: false` for such writes.

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
  its prompt, so a late **Allow**, **Open** or **Save** does nothing and the prompt does
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
