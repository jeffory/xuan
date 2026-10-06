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
  the same rules as the menus: documents stay within 30,000 pixels a side
  and 100 megapixels, a request that fails part way changes nothing, and a
  request too heavy for the editor's thread is refused. Locked layers keep
  their pixels and placement, and the MCP server can lock a layer but never
  unlock one (the plugin protocol itself lets a plugin with `document =
  "edit"` unlock a layer, as the Layers panel does).
- **Files go through you.** Saving and exporting open Xuan's save dialog with
  a suggested name, and you choose where (or cancel); opening a file shows its
  full path and opens it only if you agree. The client cannot pick where a
  file goes, replace one without the save dialog asking you to confirm, or
  open something silently. The file is written exactly where you confirmed:
  if the name you typed lacks the extension, the dialog asks again with it
  added rather than writing next to the file you saw. Suggested names are
  cleaned of folders, invisible and right-to-left control characters and
  Windows device names (`CON`, `NUL`, …). The client learns only file names,
  never folders, in answers and in error messages. Xuan's other file
  commands, the clipboard, settings and other plugins are out of reach.
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
| `get_preview` | The flattened image as a PNG (`max_side`, default 1024) | `document/export` |
| `get_layer_image` | A layer's pixels or mask as a PNG (an image's mask comes from its attached mask layer) | `layer/export` |
| `get_selection` | The selection mask as a PNG with its position | `selection/export` |
| `get_edit_permission` | Whether this session may edit, and auto mode | `session/status` |
| `set_layer` | Name, visibility, lock, opacity, blend mode, clipping (`clip_to`), position, size, rotation | `set`, `transform` |
| `create_layer` | An empty layer or a mask layer from the selection | `add_empty_layer`, `add_mask_layer` |
| `create_text_layer` | Editable text | `add_text_layer` |
| `create_shape_layer` | Rectangle, ellipse or rounded rectangle | `add_shape_layer` |
| `create_image_layer` | A PNG the client sends (base64) | `add_layer` |
| `delete_layer` | Delete a layer or group | `remove_layer` |
| `merge_layers`, `group_layers`, `ungroup_layer` | Merge, group, ungroup | `merge_layers`, `group_layers`, `ungroup_layers` |
| `move_layer` | Move a layer above or below another, or into a group | `move_layer` |
| `select_layers` | Select layers, the last one active, as clicking them does | `select_layers` |
| `select_shape` | Rectangle, ellipse or polygon selection with a `mode` | `select_rect`, `select_polygon` |
| `select_color` | Magic Wand at a point, or Color Range by colours | `select_color`, `select_color_range` |
| `modify_selection` | All, none, invert, grow, shrink, feather, subject, layer pixels (of `layer`) | `host/run`, `grow_selection`, `feather_selection` |
| `paint_stroke` | Brush strokes (or eraser) through points: one with `points`, several with `strokes`; a single point is a dab. Points may be `[x, y, pressure]`; optional taper, spacing, scatter and jitter (with a `seed`), and mirror or radial `symmetry` | `stroke` |
| `fill` | Fill the selection with a colour | `fill` |
| `fill_gradient` | Fill the selection with a linear or radial gradient through two or more colour stops, or paint the mask | `gradient` |
| `apply_filter` | Blur, motion blur, noise, lens correction; or a filter layer | `apply_filter`, `add_adjustment_layer` |
| `apply_adjustment` | Levels, curves, hue/saturation, exposure, …; or an adjustment layer | `apply_adjustment`, `add_adjustment_layer` |
| `crop_canvas`, `resize_canvas`, `resize_image` | Crop, Canvas Size, Image Size | `crop`, `resize_canvas`, `resize_image` |
| `undo`, `redo` | Up to 20 steps | `host/run` |
| `batch` | Several edit tools' steps as one undo step, all or nothing | one `document/edit` |
| `run_command` | Flatten, duplicate, flip, invert, clear, content-aware fill, remove background, masks, zoom… on the active layer, or on `layers`; returns the ids of new layers | `host/run` |
| `switch_document` | Switch tabs | `document/activate` |
| `save_document` | Save as a `.xuan` project, through the save dialog | `file/save_as` |
| `export_document` | Export PNG, JPEG, TIFF or WebP, through the save dialog | `file/export` |
| `open_document` | Open an image or project, after the user agrees | `file/open` |

Filters and adjustments use the shapes `.xuan` files store, for example
`{"GaussianBlur": {"radius": 4}}`, `"Invert"`, `{"HueSaturation": {"hue": 20,
"saturation": 10, "lightness": 0, "colorize": false}}` or `{"Levels": {"black":
10, "gamma": 1.2, "white": 245, "output_black": 0, "output_white": 255}}`; the
tool descriptions list them all with their ranges, and
[PLUGINS.md](PLUGINS.md#reading-and-editing-the-document) has the full list.
A setting out of range is refused with the field and its range.

### Layers, masks and the active layer

`run_command` and `modify_selection` `layer_pixels` act on the active layer
(and `duplicate` and `delete_layer` on all selected layers). Pass `layers`
(or `layer`) to choose them in the same call, or select them first with
`select_layers`. `get_document` lists each layer's `kind` (`image`, `text`,
`shape` with its style, `raw`, `group`, `mask`, `adjustment` or `filter`) and
its `flip_x` and `flip_y`; `get_layer_image` returns pixels before the flips.

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

### Resources

| URI | Contents |
| --- | --- |
| `xuan://document` | The current document as JSON (as `get_document`) |
| `xuan://document/preview.png` | The flattened image, longest side 1024 |
| `xuan://document/selection.png` | The selection mask cropped to its bounds |
| `xuan://layers/<id>/thumbnail.png` | Each layer's pixels, longest side 256 |

### Working well with Xuan

- Look before you edit: `get_document`, then `get_preview`. Check again after
  a change; previews are cheap.
- Prefer non-destructive changes the user can adjust: adjustment and filter
  layers (`as_layer: true`), masks, and new layers over painting on the
  original.
- Name the layers you create, so the user can find them.
- Selections limit fills, strokes, filters and adjustments, and become the
  mask of new adjustment layers. Clear it with `modify_selection` `none` when
  done.
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
- Ask the user before saving; the save dialog is theirs to answer.

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
