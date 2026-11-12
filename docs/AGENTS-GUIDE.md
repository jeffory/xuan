# Agents guide

How LLM agents (Claude Code, Claude Desktop, Codex and other MCP clients) can
see and edit images in Xuan, and what keeps that safe. Xuan exposes its open
documents through the **MCP Server** plugin (`plugins/mcp-server`), which speaks
the [Model Context Protocol](https://modelcontextprotocol.io) over Streamable
HTTP on `127.0.0.1`. The design behind it is in [MCP.md](MCP.md), and the plugin
protocol it uses in [PLUGINS.md](PLUGINS.md).

## Setting it up

1. **Get the plugin**: Xuan's release packages include it, in the [bundled
   plugins folder](PLUGINS.md#bundled-plugins), so there is nothing to
   install. From a source checkout, build it (Rust 1.88 or later) with
   `cargo build --release` in `plugins/mcp-server`.
2. **Load it** (source builds only): start Xuan with `XUAN_PLUGIN_PATH`
   pointing at the repository's `plugins` folder, or install the folder with
   **Plugins → Install from Folder or Zip…**. Either copy replaces the bundled
   one.
3. **Allow it**: the bundled plugin is off until you do. Open **Window → MCP
   Server** and press **Review
   Permissions…**. The prompt says that it edits documents, asks before each
   session's first edit, and connects to `127.0.0.1`.
4. **Copy the connection**: the pane shows `Listening on
   http://127.0.0.1:8765/mcp` and has **Copy** buttons for the URL, the token,
   a ready-made Claude Code command and a JSON client entry.

### Claude Code

Press **Copy Command** and run it in a terminal:

```sh
claude mcp add --transport http xuan http://127.0.0.1:8765/mcp --header "Authorization: Bearer <token>"
```

Then ask Claude about the open image, for example "Look at my image in Xuan
and make the sky warmer", and allow the first edit when Xuan asks.

**Keep the token out of shared places.** `claude mcp add` stores the server
for you alone by default (user or local scope); do not add it with `--scope
project`, which writes a `.mcp.json` into the project that is easily
committed, and do not commit any client configuration that holds the token.
Typing the command puts the token in your shell history: paste it with a
leading space where your shell skips such lines, or refer to an environment
variable instead, for example `--header "Authorization: Bearer
$XUAN_MCP_TOKEN"` with the variable set from the pane's **Copy Token**, or
`${XUAN_MCP_TOKEN}` inside a JSON configuration that expands variables. If the
token leaks, press **New Token**.

### Other clients

Most clients that speak Streamable HTTP read an `mcpServers` entry like the one
**Copy JSON** puts on the clipboard:

```json
{
  "mcpServers": {
    "xuan": {
      "type": "http",
      "url": "http://127.0.0.1:8765/mcp",
      "headers": {"Authorization": "Bearer <token>"}
    }
  }
}
```

A client that only runs servers as local commands (stdio) can reach it through
an HTTP bridge such as `mcp-remote`, passing the same URL and header.

### Without a window (`--headless`)

`xuan --headless` runs the editor with no window and serves the MCP Server, for
agents on a machine without a display, over SSH, in a container, or when the
desktop's graphics stack will not start Xuan. It opens no window and creates
no GPU device, so nothing loads the display server, EGL, Vulkan or GL;
documents are edited with the CPU paths Xuan uses on computers without a
suitable GPU, so large filters and RAW develops are slower than with a
window.

```sh
xuan --headless photo.NEF               # serve the MCP server, with photo.NEF open
xuan --headless --allow-files           # also let clients open and save files
xuan --headless --mcp-port 0 --demo     # any free port, the sample document
```

It prints the server's URL, the file that holds the token, and a Claude Code
command that reads it:

```text
MCP server: http://127.0.0.1:8765/mcp
Token file: /home/you/.config/xuan/plugin-data/mcp-server/token
Claude Code: claude mcp add --transport http xuan http://127.0.0.1:8765/mcp --header "Authorization: Bearer $(cat '/home/you/.config/xuan/plugin-data/mcp-server/token')"
```

The token is the one the MCP Server pane shows, so a client already set up
for Xuan with a window connects to a headless Xuan too. Only one of them can
have the port: while Xuan with a window serves MCP, start the headless one
with another `--mcp-port`. Errors, the status line and the plugin's log go to standard error. Ctrl+C (or
SIGTERM) stops it and the plugin.

Nobody is there to answer Xuan's prompts, so the command line answers them:

- **Edits** are allowed: starting Xuan headless is the answer to the edit
  prompt and to the prompt before images go to the client.
- **Files** are refused unless you add `--allow-files`. With it,
  `open_document` opens the file it names and `save_document` and
  `export_document` write to the absolute `path` they give, without asking;
  replacing a file still needs `overwrite: true`. There is no save dialog, so
  a save or export without `path` is always refused.
- **The grant is for this run only.** The MCP Server runs even if you have
  not allowed it in Xuan, and your settings are read but never written:
  nothing a headless run does changes the Xuan you use with a window. Turning
  off plugins that use the network in Xuan's settings still stops it.

The plugin must be installed as for a window (a release package includes
it; a source checkout needs `cargo build --release` in `plugins/mcp-server`
and `XUAN_PLUGIN_PATH` pointing at the repository's `plugins` folder). Files
named on the command line open as they would in a window, except that a
camera RAW file opens developed with the default settings instead of in a
Develop tab.

## Security model

Xuan treats the MCP client as a remote party that should not get more than
the user agreed to, even though it runs on the same computer: an agent may
follow instructions hidden in something it read.

- **This computer only.** The server listens on `127.0.0.1`, never on other
  interfaces.
- **A fixed port is a name anyone on this computer can take.** Your client
  sends the token to whatever listens on `127.0.0.1:8765`. If another
  program got there first (while Xuan was closed, say), it receives the token
  and your client's requests. So the server never moves to another port by
  itself: when its port is taken, the pane says **Port 8765 is in use by
  another program** and waits until you pick another port in the settings or
  press **Listen on a Free Port** (and then set up your clients again). If you
  see that warning without knowing why, press **New Token**.
- **A token.** Every request needs `Authorization: Bearer <token>`. The
  token is 32 random bytes, kept in the plugin's data folder readable only by
  you (`plugin-data/mcp-server/token`). **New Token** in the pane replaces it
  and cuts off every client that used the old one.
- **No web pages.** A request whose `Host` is not `127.0.0.1` or `localhost`
  with the server's port is refused (so a site that rebinds its DNS name to
  127.0.0.1 gets nowhere), and so is any request that carries an `Origin`
  header, which browsers add and MCP clients do not.
- **The plugin's grant.** The plugin runs only after you allowed it, with the
  permissions you saw: `document = "edit"`, `network = ["127.0.0.1"]`,
  `edit_prompt = "session"`. If the plugin changes, Xuan asks again.
- **Reading** (the document's structure and layers, previews, the selection)
  works once the client has the token. Pixels are document data: since the
  plugin declares a network address, Xuan asks **Send to MCP Server?** before
  the first image goes to it, and the answer lasts until the plugin stops (or
  choose "Don't ask again"). MCP clients usually send what they see to a model
  in the cloud, which is why this prompt exists.
- **Editing** asks once per client session: **Allow MCP Server (plugin
  mcp-server) to edit your documents for this session?** with **Allow**,
  **Deny** and **Always Allow**. A session is an MCP session the server
  issued to a client that called `initialize`; the prompt names it with a
  label the server made ("MCP client 2"), never with anything the client
  sent. Clients on the stateless protocol revision (2026-07-28 and later),
  which has no sessions, and requests that carry no issued session all share
  **one** session, shown as "every MCP client without a session": allowing
  it allows every such client with the token. Every session ends when the
  plugin stops. **Always
  Allow** turns on **auto mode**, which skips the prompt from then on; it is a
  switch in **Plugins → Manage Plugins… → MCP Server** ("Edit without asking"),
  and it is dropped if the plugin's folder, command or permissions change.
- **Every edit is one undo step** and shows on the canvas at once. Edits obey
  the same rules as the menus: documents stay within the editor's
  [size limits](USAGE.md#size-limits) (65,535 pixels a side; how many pixels
  follows the computer's memory, at least 100 megapixels), a request that
  fails part way changes nothing, and a
  request too heavy for the editor's thread is refused. Locked layers keep
  their pixels and placement, and the MCP server can lock a layer but never
  unlock one (the plugin protocol itself lets a plugin with `document =
  "edit"` unlock a layer, as the Layers panel does).
- **Files go through you.** Without a `path`, saving and exporting open
  Xuan's save dialog with a suggested name, and you choose where (or cancel).
  The file is written exactly where you confirmed: if the name you typed
  lacks the extension, the dialog asks again with it added rather than
  writing next to the file you saw. Suggested names are cleaned of folders,
  invisible and right-to-left control characters and Windows device names
  (`CON`, `NUL`, …). Opening a file shows its full path and opens it only if
  you agree.
- **Saving to a path the client names** (`save_document` or
  `export_document` with `path`, or `save_document` with `in_place`) shows
  Xuan's own prompt instead, because a client cannot operate the system
  dialog: it names the client, the file, its folder, and whether it replaces
  a file, with **Save**, **Always Allow** and **Cancel**. **Always Allow**
  lets the client save and export to paths without asking from then on; each
  such write shows in the status bar, the plugin's log and the MCP Server
  pane's recent tool calls, with the file name. It is a switch in **Plugins →
  Manage Plugins… → MCP Server** ("Save and export without asking"), and it
  is dropped if the plugin's folder, command or permissions change. The path
  must be absolute, in a folder that exists, with the format's extension and
  a plain file name. An existing file is replaced only when the call says
  `overwrite: true`; even with Always Allow, replacing a file the client
  did not write itself since Xuan started (one you saved, say) asks you
  again. The client learns only file names,
  never folders, in answers and in error messages. Xuan's other file
  commands, the clipboard and settings are out of reach.
- **Other plugins' actions** are out of reach too, unless you turn on **Run
  other plugins' actions** for the MCP Server in **Plugins → Manage
  Plugins…**. Then a client may list your other plugins' actions and run them,
  such as Comfy Cloud's **Generate Image…**, which can send your image to a
  third party and spend credits. The first run of each plugin in a client
  session asks **Run “Generate Image” for MCP Server (plugin mcp-server)?**,
  showing the plugin, the prompt and options, and where the result goes, with
  **Allow**, **Always Allow** and **Cancel**. **Always Allow** shows as **Run
  them without asking** in Manage Plugins. The plugin's own rules still apply,
  exactly as from its menu: it must itself be allowed, offline mode stops a
  network plugin, model downloads are confirmed, a plugin that uses the
  network asks before the image is sent to it (naming the MCP Server as who
  started the run), and the result is a proposal you accept or discard. The
  job shows in the status bar, where you can cancel it, and in the MCP Server
  pane. Both settings are dropped if the plugin's folder, command or
  permissions change. A client cannot set an action's file inputs.
- **Switching documents** is limited to once a second, so a client cannot
  flip your tabs about while you work.
- **Providers.** `run_command` `remove_background` and `modify_selection`
  `subject` run like their menu items, after the session's edit answer. If
  you chose a plugin as the provider for Select Subject or Remove Background,
  it runs
  under its own permissions and prompts, as from the menu (its send prompt,
  model downloads, and the result as a proposal you accept or discard).
- **Sessions and the token.** Anyone with the token who also knows another
  client's session id could use that session's edit answer. Session ids are
  random and seen only by that client and the server, so in practice the
  token is what to protect. **New Token** also forgets every session issued
  so far, so earlier **Allow** answers stop counting.
- **Offline mode** (**Disable plugins that use the network**) stops the
  server. On Linux, Xuan's network blocking for plugins does not apply to it:
  the seccomp filter cannot tell a socket listening on 127.0.0.1 from a
  connection to another machine, so it only filters plugins that declare no
  addresses.

The server cannot see or change anything else on your computer through Xuan,
but like every plugin it runs as you: install it only from a source you trust.

## Tools

Ids come from `get_document`. Coordinates and sizes are in document pixels,
with the origin at the top-left of the canvas. Colours are `#rrggbb` or
`#rrggbbaa`. Tools that change the document wait for the session's answer the
first time; refusals come back as tool errors that say so.

A call that waits for you (the edit prompt, a file dialog, the prompt to send
an image) keeps the client informed with progress notifications for up to 10
minutes, so Claude Code shows that it is waiting rather than timing out. If
the client gives up or cancels the call, the prompt in Xuan closes and
nothing happens; answering it late does nothing. A client that does not ask
for progress gets an error after 50 seconds saying you have not answered
yet, and the agent is told to ask you rather than retry.

Size limits: an HTTP request body may be up to 96 MiB, room for the largest
image `create_image_layer` takes (64 MiB of PNG, sent as base64). Everything
else a tool sends on to Xuan must fit in Xuan's 16 MiB plugin message limit.
A larger body is answered with HTTP 413 and a message giving these limits;
larger tool arguments with a tool error saying the request is too large.

| Tool | What it does | Plugin protocol |
| --- | --- | --- |
| `list_documents` | The open documents | `document/list` |
| `get_document` | Size, selection and all layers; with `document`, switches to it first | `document/get` |
| `get_layer` | One layer | `document/get` |
| `get_preview` | The flattened image as a PNG (`max_side`, default 1024), or only a `region` of it | `document/export` |
| `get_layer_image` | A layer's pixels or mask as a PNG (an image's mask comes from its attached mask layer); with `region`, the layer alone as it sits in the document | `layer/export` |
| `sample_pixels` | The colour at points, as numbers, averaged over a `radius`; of the image or one `layer` | `document/sample` |
| `get_histogram` | Luminance and RGB histograms with mean, median, range and clipped share; of the image, a `region` or one `layer` | `document/histogram` |
| `get_selection` | The selection mask as a PNG with its position | `selection/export` |
| `get_edit_permission` | Whether this session may edit, and auto mode | `session/status` |
| `set_layer` | Name, visibility, lock, opacity, blend mode, clipping (`clip_to`), position, size, rotation (about the centre or a `pivot`), `cover` to scale up until no canvas corner is empty, `flip_x` / `flip_y` (about the layer's centre; a group mirrors as a whole), layer effects (`effects`: stroke, shadows, colour overlay, glows); a text layer's `text`, `runs`, `path` and `path_options` | `set`, `set_text`, `transform` |
| `create_layer` | An empty layer or a mask layer from the selection | `add_empty_layer`, `add_mask_layer` |
| `create_text_layer` | Editable text in a box, or with `path` along an SVG path | `add_text_layer` |
| `create_shape_layer` | Rectangle, ellipse or rounded rectangle; or with `shape: "path"` an editable vector shape from SVG path data | `add_shape_layer` |
| `create_image_layer` | A PNG the client sends (its bytes as base64, not a file path: the tool cannot read files) | `add_layer` |
| `delete_layer` | Delete a layer or group | `remove_layer` |
| `merge_layers`, `group_layers`, `ungroup_layer` | Merge, group, ungroup | `merge_layers`, `group_layers`, `ungroup_layers` |
| `move_layer` | Move a layer above or below another, or into a group | `move_layer` |
| `select_layers` | Select layers, the last one active, as clicking them does | `select_layers` |
| `select_shape` | Rectangle, ellipse, polygon or SVG path selection with a `mode` and optional `feather` | `select_rect`, `select_polygon`, `select_path` |
| `select_color` | Magic Wand at a point, or Color Range by colours | `select_color`, `select_color_range` |
| `modify_selection` | All, none, invert, grow, shrink, feather, subject, layer pixels (of `layer`) | `host/run`, `grow_selection`, `feather_selection` |
| `paint_stroke` | Brush strokes (or eraser; `mode` dodge, burn or sponge to lighten, darken or (de)saturate what is under the brush; `mask: true` to paint a layer's mask) through points: one with `points` or an SVG `path`, several with `strokes`; a single point is a dab. Points may be `[x, y, pressure]`; optional taper, spacing, scatter and jitter (with a `seed`), and mirror or radial `symmetry` | `stroke` |
| `fill` | Fill the selection with a colour, or with `path` the inside of an SVG path, or with `mask: true` the layer's mask | `fill`, `fill_path` |
| `save_path` | Keep an SVG path with the document under a name, for the Paths dialog | `add_path` |
| `fill_gradient` | Fill the selection with a linear or radial gradient through two or more colour stops, or paint the mask | `gradient` |
| `apply_filter` | Blur, motion blur, noise, lens correction, vignette, bloom, tonal contrast, unsharp mask (sharpen), dither; or a filter layer | `apply_filter`, `add_adjustment_layer` |
| `apply_adjustment` | Levels, curves, hue/saturation, exposure, …; or an adjustment layer | `apply_adjustment`, `add_adjustment_layer` |
| `auto_adjust` | Auto `contrast`, `tone` or `white_balance` (grey-world, or from a `neutral` point that should be grey), applied or as an adjustment layer | `auto_adjust` |
| `set_adjustment` | Change an existing adjustment or filter layer's settings in place; `get_document` lists the current ones under `adjustment` and `filter` | `set_adjustment` |
| `crop_canvas`, `resize_canvas`, `resize_image` | Crop to a rectangle, to the largest box of a `ratio` (around a `center`), or `straighten` by an angle and crop the empty corners away; Canvas Size; Image Size | `crop`, `crop_to`, `resize_canvas`, `resize_image` |
| `rotate_canvas`, `trim_canvas` | Rotate Canvas (any angle clockwise; `expand` grows the canvas to fit, the default), Trim (transparent, top-left or bottom-right colour margins, per side); `run_command` `crop_to_selection` crops to the selection | `rotate_canvas`, `trim` |
| `undo`, `redo` | Up to 20 steps | `host/run` |
| `batch` | Several edit tools' steps as one undo step, all or nothing | one `document/edit` |
| `run_command` | Flatten, duplicate, flip (several layers flip together as a group), rotate canvas, crop to selection, invert, clear, content-aware fill, remove background, masks, zoom… on the active layer, or on `layers`; returns the ids of new layers | `host/run` |
| `switch_document` | Switch tabs | `document/activate` |
| `save_document` | Save as a `.xuan` project: through the save dialog, or to an absolute `path` (or back to its own file with `in_place`) after Xuan's prompt or under Always Allow; `overwrite` to replace a file | `file/save_as`, `file/save` |
| `export_document` | Export PNG, JPEG, TIFF or WebP: through the save dialog, or to an absolute `path` after Xuan's prompt or under Always Allow; `overwrite` to replace a file; `quality` (1–100) for JPEG and lossy WebP, `lossless` for WebP; with `path`, `layer`, `region` and `max_side` write one layer, a rectangle or a smaller copy as a PNG | `file/export` |
| `open_document` | Open an image or project, after the user agrees; a camera RAW file opens as a RAW layer developed with the default settings | `file/open` |
| `develop_raw` | Develop a camera RAW layer again with new Develop settings (white balance, exposure, tone, colour, detail, lens, crop, rotation), `auto_exposure`, or a white balance from a `neutral_point`; only the settings given change | `raw/develop` |
| `list_plugin_actions` | Other plugins' actions, each with a JSON schema of its inputs; needs **Run other plugins' actions** | `plugins/actions` |
| `run_plugin_action` | Run one, as from its menu without its dialog, after the user allows it; returns once the job runs | `host/run` |
| `get_jobs` | The status bar's jobs, and what became of the runs this server started | `jobs/list` |

Filters and adjustments use the shapes `.xuan` files store, for example
`{"GaussianBlur": {"radius": 4}}`, `"Invert"`, `{"HueSaturation": {"hue": 20,
"saturation": 10, "lightness": 0, "colorize": false}}` or `{"Levels": {"black":
10, "gamma": 1.2, "white": 245, "output_black": 0, "output_white": 255}}`; the
tool descriptions list them all with their ranges, and
[PLUGINS.md](PLUGINS.md#reading-and-editing-the-document) has the full list.
A setting out of range is refused with the field and its range.

### Curves: SVG paths

Draw curves with SVG path data instead of dense point lists: `select_shape`
with `shape: "path"`, `paint_stroke` with `path`, `fill` with `path` and
`create_shape_layer` with `shape: "path"` all take a `d` string in document
pixels, such as `"M 0 700 C 120 640 380 640 512 700 Z"` (M, L, H, V, C, S, Q,
T, A, Z; lowercase is relative). For example, a hill as an editable shape, a
glowing thread along a curve, and a selection of the hill's top:

```json
{"tool": "batch", "arguments": {"steps": [
  {"tool": "create_shape_layer", "arguments": {"shape": "path", "color": "#3a6b45",
    "path": "M 0 700 C 120 640 380 640 512 700 L 512 768 L 0 768 Z"}},
  {"tool": "create_layer", "arguments": {"name": "Thread"}},
  {"tool": "paint_stroke", "arguments": {"layer": "$2", "color": "#ffe9a8", "size": 4,
    "taper_in": 40, "taper_out": 40, "path": "M 60 300 C 160 180 300 420 450 260"}},
  {"tool": "select_shape", "arguments": {"shape": "path", "feather": 6,
    "path": "M 0 700 C 120 640 380 640 512 700 Z"}}
]}}
```

A stroke follows one subpath (one `M`); give several strokes for several. Fills
and selections close open subpaths and are antialiased; `fill_rule: "evenodd"`
makes inner subpaths holes. A path shape layer stays a vector: `set_layer` can
move, stretch or rotate it and the outline is redrawn, and `get_document`
reports its outline (`shape.path`) where it is now. `save_path` keeps a path
with the document so the user can fill, stroke or select it again from
**Select → Paths…**; `get_document` lists them under `paths`. A malformed path
is refused with what was expected and the character, e.g. `SVG path: expected
the y of the line's end for `L` at character 11, found the end of the path`.

### Text on a path

Letters along a curve are one text layer: give `create_text_layer` a `path`
instead of `x` and `y`. Each letter is moved to its distance along the path
and turned to its direction, and the line stays editable text. `path_options`
says how it follows the path: `start_offset` (percent of the path's length),
`align` (`start`, `center` or `end` at that point), `side` (`left`, the
default, stands letters on top of a path drawn left to right; `right` flips
them under it, reading the other way), `letter_spacing` and `baseline_shift`
in pixels, `rotate: false` for upright letters, and ramps from the first
letter to the last: `size_end` (from `size`) and `opacity_start` /
`opacity_end`. Letters past the end of an open path are hidden; on a closed
path the text wraps around. For example, letters rising from a book, shrinking
and fading as they go:

```json
{"tool": "create_text_layer", "arguments": {"text": "αβγ אבג абв ꦲꦤꦕ", "size": 17,
  "color": "#2a2a2a", "path": "M 260 620 C 300 520 220 430 300 330 S 380 200 330 120",
  "path_options": {"size_end": 7, "opacity_start": 0.95, "opacity_end": 0.55,
                   "letter_spacing": 4}}}
```

`set_layer` changes it later: `text` for new words, `path` to move it to
another curve (`null` puts it back in a box), `path_options` for any of the
options (the others are kept). Moving, scaling or rotating the layer takes
the path along; `get_document` reports the layer's `text`, with its `path`
where it is now and its `path_options`.

### Letters in their own font or colour

One text layer can mix fonts and colours: `runs` on `create_text_layer` (and
`set_layer`) lists letters with their own `family`, `color`, `bold` or
`italic`. `start` and `end` count Unicode code points from the start of the
text, `end` excluded:

```json
{"tool": "create_text_layer", "arguments": {"text": "APPle", "color": "#000000",
  "runs": [{"start": 0, "end": 3, "family": "Georgia", "color": "#ff0000"}]}}
```

New `text` from `set_layer` keeps each unchanged letter's style; `runs: []`
puts every letter back in the layer's own style. `get_document` lists a text
layer's `runs`.

### Layers, masks and the active layer

`run_command` and `modify_selection` `layer_pixels` act on the active layer
(and `duplicate` and `delete_layer` on all selected layers). Pass `layers`
(or `layer`) to choose them in the same call, or select them first with
`select_layers`. `get_document` lists each layer's `kind` (`image`, `text`,
`shape` with its style, `raw`, `group`, `mask`, `adjustment` or `filter`) and
its `flip_x` and `flip_y`; `get_layer_image` returns pixels before the flips.
`run_command` `flip_h` or `flip_v` on several layers mirrors them together
across their combined bounds, as Photoshop does, so they also trade places;
each then reports its flip. To flip layers each in place, send `set_layer`
with `flip_x` or `flip_y` for each, in one `batch` for one undo step.

Layer effects (**Layer → Layer Effects…**: a stroke, drop shadow, colour
overlay, inner shadow, outer glow and inner glow) are kept with the layer and
redrawn as it changes, so a glow never goes stale. `get_document` lists a
layer's under `effects` (`null` when it has none), in the shape `.xuan` files
store them, and `set_layer` `effects` takes that object back whole: it
replaces all the layer's effects, so read them, change one and send them all.
Settings left out take the defaults a new effect gets; colours are
`[r, g, b]`; `null` removes them. Settings are held to what the Layer Effects
dialog offers, in whole numbers but for opacities: a stroke 1–500 pixels, a
glow 0–250, a shadow's distance 0–1000, blur 0–250 and light angle −180–180
(in the layer's own pixels, so the shadow turns and flips with the layer).
For a warm glow on a layer:

```json
{"layer": "<id>", "effects": {"outer_glow": {"size": 24, "color": [255, 217, 120], "opacity": 0.8}}}
```

Only pixel, text and shape layers take effects, not groups, masks,
adjustment or filter layers.

A mask is a layer of its own. `run_command` `mask` attaches one, made from the
selection, to the active image: a layer of kind `mask` whose `parent` and
`attached_to` are the image. The image lists it under `masks`, with whether it
is `enabled` and `linked` (moves with the image). To disable, link or delete
it, run `disable_mask`, `link_mask` or `delete_mask` with `layers` set to the
mask layer's id. Adjustment and filter layers attached to an image the same
way change only that image.

`set_layer` with `clip_to` clips a layer to a base below it in the same folder,
as **Layer → Clipping Mask** does: the layer then shows only where the base
has pixels. The base can be a pixel layer or a group. A group's shape is all
of its visible layers together, including mask layers inside it and the
group's own opacity and mask, so a shading layer clipped to a figure's group
follows every later edit to the figure. The base's opacity also applies to
the clipped layer, as in Photoshop. A base that is itself clipped passes on
its own base. `"clip_to": null` releases the clipping. Groups, mask layers and
filter layers cannot be clipped, and mask, adjustment and filter layers
cannot be bases. `get_document` lists each layer's `clip_to`.

`content_fill`, `remove_background` and `remove_flat_background` run in the
background: `run_command` returns while they run, and other edits fail with
"The editor is busy" until they finish.

### Batches

`batch` takes `{"name"?, "steps": [{"tool", "arguments"}, …]}` and sends the
edits of every step to Xuan in one `document/edit` request: one undo step
named `name`, applied all or nothing. Each step is an edit tool with the
arguments it takes on its own. A later step names a layer an earlier step
created as `"$1"`, `"$2"`, …, the n-th layer the batch has created so far
(each `create_*` step creates one, as do `merge_layers`, `group_layers` and
`apply_filter` or `apply_adjustment` with `as_layer`):

```json
{"name": "Title", "steps": [
  {"tool": "create_text_layer", "arguments": {"text": "X", "x": 40, "y": 30, "size": 64}},
  {"tool": "set_layer", "arguments": {"layer": "$1", "opacity": 0.8, "rotation": -12}},
  {"tool": "create_layer", "arguments": {"name": "Stars", "above": "$1"}},
  {"tool": "paint_stroke", "arguments": {"layer": "$2", "color": "#ffffff", "size": 3,
    "strokes": [{"points": [[12, 9]]}, {"points": [[80, 22]]}, {"points": [[140, 15]], "size": 5}]}}
]}
```

- Steps are checked before anything is sent; a step with bad arguments fails
  the call without contacting Xuan. If Xuan refuses an edit (a locked layer,
  a reference to a layer not created yet), it applies none of them. Either way
  the error names the step, and nothing changes.
- Only edit tools may be steps (`select_layers` too): `modify_selection` only with `none`, `grow`,
  `shrink` or `feather`. Reading tools, `undo`, `redo`, `run_command`, the
  document and file tools and `batch` itself are refused.
- A batch is held to the limits of one request: at most 1,000 edits (each
  stroke is one, `set_layer` with both properties and placement two), 32 new
  layers, 200,000 pixels of strokes, the work budget and Xuan's 16 MiB message
  limit. Split larger work into several calls.
- The answer lists the ids of the layers created, in order, and the selection
  or canvas size when a step changed them.
- A selection a step makes stays active for the steps after it, and for later
  calls, until it is replaced or cleared. The answer's `notes` name each step
  that ran inside it (`"Step 2 (fill) ran inside the selection (448, 292,
  184×216)."`), and a warning comes before the JSON for a step that changed
  nothing inside it.

### Other plugins' actions

Once the user turned on **Run other plugins' actions**, `list_plugin_actions`
lists what the other installed plugins offer, for example Comfy Cloud:

```json
{"plugin": "comfy-cloud", "action": "generate", "label": "Generate Image…",
 "kind": "generate", "source": "none", "result": "ask", "into": ["layer", "document"],
 "network": true, "allowed": true, "available": true,
 "inputs": {"type": "object", "additionalProperties": false, "properties": {
   "prompt": {"type": "string", "title": "Prompt", "description": "For example: A lighthouse on a cliff at dusk, painted in gouache"},
   "model": {"type": "string", "enum": ["seedream-pro", "seedream-flash", "ideogram"], "default": "seedream-pro", …},
   "aspect": {…}, "resolution": {…}, "seed": {"type": "integer", "title": "Seed"}}}}
```

- `run_plugin_action` with `plugin`, `action` and `inputs` runs it. Inputs
  left out take their defaults; a wrong value fails at once and names the
  input. An action whose `result` is `"ask"` needs `into`: `"layer"` (a new
  layer in the current document) or `"document"`. `layers` selects layers
  first, for actions that work on the active layer (`source: "layer"`).
- The first run of each plugin in a session waits for the user, and a plugin
  that uses the network asks before the image is sent; the call keeps the
  client informed meanwhile, as other prompts do. A refusal says nothing ran:
  ask the user before trying again.
- The call returns as soon as the job runs, with its `job` id. Cloud jobs
  often take 30–120 seconds. Call `get_jobs` every 10 seconds or so: `running`
  has the job's progress and latest message, and `finished` says what became
  of it: `proposed` (its result is shown to the user), then `accepted` or
  `discarded`, or `failed` with the error, or `cancelled`. The result is in
  the document only once the user accepted it; check with `get_document` or
  `get_preview` then. Other edits keep working while a job runs.
- These runs may cost the user money. Run what they asked for, not
  variations to choose from, unless they asked for those.

### Resources

| URI | Contents |
| --- | --- |
| `xuan://document` | The current document as JSON (as `get_document`) |
| `xuan://document/preview.png` | The flattened image, longest side 1024 |
| `xuan://document/selection.png` | The selection mask cropped to its bounds |
| `xuan://layers/<id>/thumbnail.png` | Each layer's pixels, longest side 256 |

### Camera RAW

`open_document` opens a camera RAW file (`.nef`, `.nrw`, `.cr2`, `.cr3`,
`.crw`, `.raf`, `.arw`) as a document with one RAW layer, developed with
Xuan's default settings. The layer keeps the camera file, so developing it
again loses nothing: `develop_raw` changes its Develop settings and renders
the whole image again from the sensor data, as the Develop workspace does.

- `get_document` lists the RAW layer's camera, lens, ISO, aperture, shutter
  and focal length, and its current `settings`, under `raw`.
- `settings` holds only what changes, by the names `.xuan` files use, e.g.
  `{"exposure": 0.7, "highlights": -40, "shadows": 30, "vibrance": 15}`;
  `reset: true` starts from the defaults. A misspelt or out-of-range setting
  is refused with its name and range.
- White balance: `{"white_balance": "Temperature", "temperature": 5200,
  "tint": 8}`, back to the camera's with `"AsShot"`, or `neutral_point: [x,
  y]` (0–1 across the layer as it looks now) on something that should be
  grey or white.
- `auto_exposure: true` sets the exposure as Develop's **Auto** button does.
- Crop and straighten with `crop` (`[left, top, right, bottom]`, 0–1),
  `rotation` (degrees) and `quarter_turns`; the layer changes size.

A develop takes a few seconds on a large file; it runs in the background and
the call returns when it is done. Look at the result with `get_preview`.

### Working well with Xuan

- Look before you edit: `get_document`, then `get_preview`. Check again after
  a change; previews are cheap.
- Measure rather than judge by eye: `sample_pixels` for the colour of
  something that should be neutral or for whether a corner is empty,
  `get_histogram` for exposure, contrast and clipping. Both answer in numbers
  and cost far less than an image. For analysis in a program of your own,
  `export_document` with `path` and `max_side`, `region` or `layer` writes a
  PNG to a file, so the pixels never pass through the conversation.
- Prefer non-destructive changes the user can adjust: adjustment and filter
  layers (`as_layer: true`), masks, and new layers over painting on the
  original.
- Local adjustments go through masks: add the adjustment with `as_layer`,
  `fill` its mask black (`mask: true`), then bring it back where it is wanted
  with `paint_stroke` (`mask: true`, white, a large soft brush) or
  `fill_gradient` with `mask`. `set_adjustment` then tunes the strength
  without repainting.
- Name the layers you create, so the user can find them.
- Selections limit fills, strokes, filters and adjustments, and become the
  mask of new adjustment layers. A selection stays active for every later step
  and call until it is replaced or cleared, so clear it with `modify_selection`
  `none` when done. While one is active, `paint_stroke`, `fill`,
  `fill_gradient`, and `apply_filter`, `apply_adjustment` and `auto_adjust`
  without `as_layer` answer with `clipped_to_selection` (its bounds) and
  `changed`; when nothing changed, a warning comes first. Check the selection
  before blaming the path, the colour or the layer.
- Each tool call is one undo step. If the user did not like a change,
  `undo` it rather than trying to reverse it by hand.
- Group work that belongs together, so it is one undo step and one round
  trip: `paint_stroke` with `strokes` for many strokes or dabs (a one-point
  stroke paints a single round dab), and `batch` for a sequence of edits.
- Taper strokes instead of painting them one width: give points a pressure,
  `[[x, y, 0.2], [x, y, 1], [x, y, 0.2]]`, or set `taper_in`/`taper_out` in
  pixels (grass blades, rays, threads fading into the distance). For dust,
  snow, stars or foliage, one stroke with `spacing`, `scatter`,
  `scatter_count` and the jitters replaces many dabs; a wide `spacing` (e.g.
  1.5) makes a dotted trail. Keep `seed` to repeat a stroke exactly, change it
  for a different pattern.
- Let symmetry repeat a stroke instead of computing mirrored coordinates:
  `"symmetry": {"mode": "vertical"}` mirrors a figure's left half onto its
  right across the canvas centre (or `"center": [x, y]`), and `{"mode":
  "radial", "segments": 12, "center": [x, y]}` turns one ray into a
  starburst's twelve. Copies don't darken each other where they cross.
- Ask the user before saving; the save dialog and the save prompt are theirs
  to answer. To save without the dialog, pass `path` (absolute, in a folder
  that exists); the first such save waits for the user's answer in Xuan.
  Pass `overwrite: true` only when you mean to replace that file.

## The `.xuan` format for agents

A `.xuan` project is a ZIP archive holding `manifest.json` and lossless PNGs
(`images/<layer id>.png`, masks as `images/<layer id>.mask.png`); the format is
specified in [FORMAT.md](FORMAT.md). An agent that has a project file but no
running Xuan can read it with any ZIP tool:

- `manifest.json` has `format: "me.silverl.xuan"`, a `version`,
  `pixel_layers` (the ids of the layers that have a PNG) and the `document`:
  its size, resolution, guides, layout grid and `layers`, bottom to top, each
  with its id, name, `parent` group, `clip_to`, visibility, lock,
  opacity, `blend`, `transform` (position, size, rotation, flips, optional
  perspective corners), and depending on its kind an `adjustment`, `filter`,
  `text` style, `shape` style, `raw` develop settings, layer `effects`, and for
  plugin results a `generated` record with `provenance`.
- Pixels are never in the JSON. A layer's PNG holds its source pixels, which
  the transform places on the canvas; text and shape layers keep a rendered
  PNG next to their style so other readers can show them.
- Selections, the undo history and the clipboard are not stored.

Editing a `.xuan` file by hand is possible but fragile: Xuan validates IDs,
the hierarchy, clipping, sizes and budgets when it loads a project and refuses
the whole file when something is off, and it does not reload files that change
on disk. Use the MCP tools to change a project that is open in Xuan, and
`save_document` to write it.
