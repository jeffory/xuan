# Generative and ML features

Design notes for issue #6: how Xuan gets text-to-image, inpainting, background
removal, upscaling and similar features without putting models in the core.
This was written after the plugin system landed (#21, #23, #34; hardening in
#33), so most of the original questions already have an answer. Each section
says what exists, what is missing, and what to do. The plugin protocol itself is
in [PLUGINS.md](PLUGINS.md); the stored metadata is in [FORMAT.md](FORMAT.md).

Related tickets: **#5** (Select Subject, Color Range, ML background removal)
and **#7** (MCP server and client; its design, the host API it added and how
its tools map onto the plugin protocol are in [MCP.md](MCP.md)).

## Summary of recommendations

1. **Keep out-of-process JSON-RPC plugins as the only extension model.** Do not
   add WASM or embedded scripting. Models run in the plugin's own process or
   virtualenv; Xuan never links onnxruntime, torch or a model.
2. **Make network access enforceable or stop calling it a permission.**
   Done in #39 for the cheap part: the grant dialog says `permissions.network`
   is not enforced, a per-run prompt asks before document data goes to a plugin
   that declares hosts, and offline mode disables such plugins. OS-level
   enforcement for plugins that declare no hosts is done on Linux (#43, opt-in
   for now); see the spike in section 5.
3. **Add three small host features that every generative plugin needs:** a
   result that declares its placed size (upscale), canvas extension (outpaint)
   and a `mask` output kind that becomes a selection (segmentation, #5).
   The placed size is done in #35, the `mask` output in #37 and canvas
   extension in #36.
4. **Give models a home:** a per-plugin `models_dir` with a manifest-declared,
   hash-checked download, shown to the user with its size before it starts
   (done in #38).
5. **Defer C2PA, plugin signing and the OS keyring.** Record the provenance we
   already store, add the model and sampler to it (done in #40), and revisit
   when there is a plugin registry.
6. **No machine learning or LLMs in core Xuan** (decided in #5). Core features
   use classical algorithms: Select Subject, Remove Background and the Magic
   tool's Object mode run a GrabCut graph cut with a guided-filter edge
   (`src/segment.rs`). ML is optional and lives in plugins, which replace the
   built-in algorithm through a **provider** (`[[provides]]` in the manifest,
   chosen in Settings → Selection; see "Providers" in
   [PLUGINS.md](PLUGINS.md#providers)).

## 1. Plugin model

**Have.** Out-of-process plugins speaking JSON-RPC 2.0 over stdio, declared by
`plugin.toml` (`src/plugins/manifest.rs`, `protocol.rs`, `host.rs`). Any
language; Rust (`sdk/xuan-plugin`) and Python (`sdk/python/xuan_plugin.py`)
SDKs. A crash cannot take the editor down. The process tree is killed as a
group (Unix process group, Windows Job Object). Grants are bound to folder,
command and permissions (`src/app/plugins.rs`).

**Compared with the other options from the issue.**

| Option | Verdict |
| --- | --- |
| Out-of-process JSON-RPC | Chosen. Works with ComfyUI, Python ML stacks and native binaries. Packages as plain files, so AppImage, deb, rpm and Windows installers need nothing extra. |
| WASM (wasmtime) | Rejected for now. Real sandbox, but ML runtimes with GPU access do not run in it, and it adds a large dependency. Might suit small pixel filters later. |
| Dynamic libraries | Rejected: no stable Rust ABI and `unsafe`. |
| Embedded scripting | Rejected: too weak for ML, and a second API to maintain. |
| MCP as the transport | Not the plugin transport. See below. |

**Missing.** Nothing structural. The process is not sandboxed (see section 5).

**MCP (#7).** MCP is about *LLM clients driving Xuan* and *Xuan calling MCP
tools*. The plugin protocol is about Xuan driving a plugin with host-owned UI,
undo and proposals. Keep them separate. The useful bridge is a generic
"MCP client" plugin: a normal plugin that speaks MCP to a configured server
(image generation, segmentation) and maps its tools to actions. That needs no
host changes and should be built once #7 settles the server side. An MCP
server in Xuan should reuse the `document/*` and `document/edit` code paths
(`src/plugins/edits.rs`) so that one set of limits and one undo model apply.

## 2. Host API

**Have.**

- Read: `document/get`, `layer/export` (pixels or mask, `max_side`),
  `document/export`, `selection/export`. Action sources: `layer`, `composite`,
  `selection`, with `max_side` and `crop_to_regions`.
- Write: results as `image`, `document`, `edit` or `text` outputs, and
  `document/edit` ops (`add_layer`, `replace_pixels`, `set`, `remove_layer`,
  `set_mask`, `set_selection`, `select`). Every result is a proposal with
  Accept/Discard and exactly one undo step.
- Jobs run in the background (`src/plugins/jobs.rs`) with `job/progress`,
  `job/cancel` (error `-32800`), `action/estimate`, and limits on outputs, layers,
  edits and total pixels (a quarter of the memory, at least 100 MP; see
  [size limits](USAGE.md#size-limits)).
- 8-bit sRGB RGBA PNG only.

**Missing or weak.**

- **Pixel transfer.** PNG files are fine up to a few megapixels. A 100 MP
  layer costs a PNG encode, a disk write, a decode in the plugin and the same
  back. Generative models rarely work above 4 MP, and upscalers tile, so this
  is acceptable now. Shared memory would be a large cross-platform change for a
  small gain. Cheaper first steps: uncompressed or fast-compression PNG for
  job files, and `selection`/`crop_to_regions` already limiting what is sent.
  Revisit only if a profile shows the PNG round trip dominating.
- **Placed size of a result.** A result image is placed at its pixel size
  divided by the export scale (`place_layer`, `replace_pixels` in `jobs.rs`).
  A 2x upscale of a layer therefore lands twice as large as the original, and
  the `replace` mode scales it back down, which defeats the purpose. The
  prototype plugin first avoided this by opening a new document. Done (#35):
  `image` outputs take optional `width`/`height` in document units, or
  `fit = "source"`, and the prototype now returns a layer at the source's size.
- **Canvas extension.** No edit op changes the canvas size. Outpainting needs
  "grow the canvas by N pixels on each side, then fill". Add an `extend_canvas`
  edit op, or a source option `extend = {left, top, right, bottom}` that sends a
  padded image plus an alpha mask of the new area. Done (#36), both: a
  composite source with `extend` (fixed pixels or an input's value per side)
  arrives padded with transparent pixels, with `extend_mask`, a grey mask of
  the new area on the same grid as the selection mask. A result's
  `extend_canvas` edit grows the canvas through the Canvas Size code path
  (guides follow), needs `document = "edit"`, and image and mask outputs move
  with the content, so `fit = "source"` covers the new canvas. The send
  prompt says how far the image is extended. `plugins/extend-edges` shows it
  with a mirror/repeat fill where a generative backend would go.
- **Mask as output.** `set_selection` exists but takes a file the plugin must
  write and then apply with `document/edit`, which needs `document = "edit"`.
  Segmentation (#5) wants a plain `mask` output kind: a result that the host
  turns into a selection (replace, add, intersect) after Accept, with the same
  proposal bar. This is the dedicated path #5 needs; the model itself lives in
  a plugin. Done (#37): a `mask` output is a grey PNG, placed like an `image`
  (including `fit = "source"`), that becomes a soft selection combined by
  `replace`, `add`, `subtract` or `intersect`, as a proposal and one undo
  step, without `document = "edit"`. `plugins/select-bright` shows it with a
  backend hook where a segmentation model would go.
- **Higher bit depth** (16-bit or float) is not supported anywhere. Models are
  8-bit; skip until Xuan's own pipeline changes.

## 3. Declarative UI

**Have.** Action inputs are rendered by the host from the manifest: `text`,
`multiline` (prompt box), `integer`, `number`, `seed` with a random button,
`bool`, `enum`, `color`, `path` and on-canvas `regions` with per-region fields.
Menu placement, shortcuts, settings dialog, and sidebar panes with a widget
tree (`src/plugins/ui.rs`, `src/app/plugin_dialogs.rs`, `plugin_panes.rs`).
`action/estimate` shows cost before running.

**Missing.**

- A **mask input** that uses the current selection directly (inpainting without
  boxes). `regions` can turn the selection into a region with a mask PNG, which
  works but is clumsy for a single mask. A `selection` input type or
  `source.from = "selection"` plus the layer would be clearer.
- A **negative prompt / sampler / steps** are just more inputs and need nothing.
- **Result gallery**: models return several candidates. Today each is a layer;
  the proposal bar compares one. Not blocking.

**Recommendation.** No new UI system. Add the `selection` input only if the
region route proves awkward in the first inpainting plugin.

**Surfaces (done).** Generative actions also appear in Xuan's own UI, so they
feel built in rather than reachable only through a menu and a dialog: the
**AI Region** tool, **New layer with AI** in the Layers panel and the
**Generate** tab of New Image (see [Surfaces](PLUGINS.md#surfaces)). The rule
is *keep the simple edits simple; the advanced options are there when needed*:
each surface first shows only a prompt and Generate, the rest sits under a
collapsed **Advanced**, and the full dialog stays in the menus. Plugins
declare where their actions appear and Xuan draws them with its own widgets.
Two alternatives were turned down: plugin-drawn popovers (inconsistent, and a
plugin's UI would sit inside Xuan's chrome) and Comfy-specific UI in core
(ties core to one backend, against keeping ML in plugins).

Results use the best quality for the area they cover without going under it:
a surface run tells the plugin the document pixels it will cover
(`inputs.target`), the plugin renders at least that, exactly when its model
allows, and places the result with `fit = "cover"`, which scales evenly and
keeps what hangs over in the layer, so nothing is stretched or cropped. New
Image keeps the model's size, at least the size typed; **Exact size** makes
the canvas exactly that size with the result as a movable layer covering it,
for reframing.

Not done yet: per-size cost estimates (Comfy's price formulas could drive
them), keeping popover values after Xuan restarts, and plugin-drawn UI inside
surfaces.

## 4. Backends

**Have.** The Comfy Cloud example (`plugins/comfy-cloud`) does text-to-image,
prompt and region-driven editing and layer separation over a hosted API with
secrets, progress, cancel and the documented error codes. It runs Comfy's own
workflow templates, converted to API workflows and kept up to date. The prototype `plugins/local-upscale` shows a fully
local job with no network.

**Recommended shapes** (all are ordinary plugins):

| Goal | Backend | Notes |
| --- | --- | --- |
| Text-to-image | ComfyUI (local or cloud), diffusers, or a hosted API | Comfy example already covers the cloud path. A local ComfyUI plugin is the same code with a `localhost` base URL. |
| Inpaint / outpaint | Same, with a mask workflow | The host features are done: `source.mask = "selection"` (#42) and `source.extend` with `extend_canvas` (#36); `extend-edges` has a backend hook. |
| Background removal, segmentation | ONNX Runtime in the plugin (U2-Net, IS-Net, SAM-class models) | Plugin returns a `mask` output (#37) and declares `[[provides]]` for `select_subject`, `object_select` and/or `remove_background`, so the user can make it replace the built-in GrabCut; `select-bright` is the template. |
| Upscaling | Real-ESRGAN or similar via ONNX Runtime in the plugin | `local-upscale` has a backend hook. |
| Prompt assist, captioning | Ollama or llama.cpp over localhost | A pane or action that returns `text` or fills an input. |
| Style transfer, colorize, denoise | Same ONNX pattern | Per-model plugins. |

**Missing.**

- ~~**Model download and caching.**~~ **Done in #38.** A plugin declares its
  model files in `[[models]]` (`id`, https `url`, `sha256`, `size`, optional
  `file`, `license`, `source`) and lists them in the actions that need them.
  Xuan asks before downloading, showing each model's host, size, licence and
  source, downloads in the background with progress and cancel, checks the
  size and SHA-256 while streaming, and renames the file into the plugin's
  `models_dir` (inside `data_dir`, so it survives updates). Paths reach the
  plugin in `initialize` and `models/changed`, only while the file still
  matches what was verified. Manage Plugins has a Models section to download,
  verify and delete them, and offline mode stops downloads, so "offline after
  first run" is checkable. See "Models" in [PLUGINS.md](PLUGINS.md#models).
- **Python environments.** Each ML plugin needs its own virtualenv, created by
  the user or an install script. The manifest `command` can point into it. A
  documented convention is enough; do not manage environments in the host.
- ~~**Bundled model for #5.**~~ **Decided in #5: none in core.** Select
  Subject works out of the box with the classical GrabCut segmentation. A
  segmentation model ships, if at all, as a plugin with the model as a
  verified download (#38) that the user picks as the provider; the built-in
  algorithm is always the fallback, including in offline mode.
- **GPU selection** is the plugin's business (onnxruntime providers, CUDA).

## 5. Privacy and safety

**Have.**

- No plugin starts before an explicit grant that shows its folder, command and
  permissions, and any change re-prompts.
- `document = "read" | "edit"`, `filesystem`, and secrets are enforced by the
  host for what it does on the plugin's behalf; file access is confined to the
  plugin's folders, with results checked against size limits.
- Secrets go over stdin only, are kept out of SDK debug output and are stored
  owner-only in `secrets.toml`.
- Results are proposals; nothing lands without Accept. Menu items, status and
  prompts name the plugin.

**Missing or weak.**

- **`permissions.network` is only a label for plugins that declare hosts.**
  Such a plugin can contact any host, not only those; the grant dialog says
  so in plain words (#39). For plugins that declare **no** hosts, Linux
  enforcement is done (#43): with "Block network for plugins that don't
  declare it" on, they start under a seccomp filter (see "Blocking the
  network" in [PLUGINS.md](PLUGINS.md#blocking-the-network)). Windows has
  nothing yet; see the spike below.
- **"Pixels leave this machine" prompt.** Done in #39 (see "Network" in
  [PLUGINS.md](PLUGINS.md#network)): for a plugin that declares network hosts,
  each run that sends document data names the hosts and lists what is sent
  (layer, composite or selection pixels, regions and their texts, prompts and
  other text inputs), with **Send**, **Cancel** and "Don't ask again for this
  plugin", stored in the grant. Exports the plugin asks for outside a
  confirmed action wait for a once-per-process answer. The answer is per
  plugin, not per host: the host cannot tell which declared host data goes to.
- **Offline mode.** Done in #39: "Disable plugins that use the network" keeps
  plugins that declare hosts from starting. It relies on the declaration, so it
  is a guarantee only together with OS enforcement for the others, which
  Linux has since #43.
- **Keyring.** Secrets are in a 0600 file (#33 F18). The keyring crates need a
  desktop secret service that headless sessions lack, so a file fallback must
  stay. Treat the keyring as an optional front end, later.
- **Signing.** There is no plugin signing. Plugins are folders the user copies
  in. Signing only makes sense with a registry; skip until distribution grows.
- **C2PA.** Not implemented. Content credentials are a standard way to label
  AI output and some services embed them already. Recommend: do not write C2PA
  from Xuan yet; keep our own provenance (section 6) and preserve a file's
  existing manifest on export only if the library story is simple. Track as a
  separate ticket.
- **Local-model plugins are not sandboxed** either; a model file can be hostile.
  Prefer formats that do not execute code (ONNX, safetensors) over pickles, and
  say so in plugin guidance (PLUGINS.md does, under "Models"). Xuan downloads
  declared models only after asking, checks them against the pinned SHA-256,
  and never unpacks or runs them.

### Spike: enforcing network access in the OS (#39)

This compares the options for making `permissions.network` real. The Linux
step of the recommendation below is done (#43); the Windows step is open. Two goals are possible: **no network at all** for plugins that declare no
hosts, and **only the declared hosts** for the others. The second is much
harder everywhere: hosts are names, the OS filters addresses, and CDNs share
and rotate them, so an allow-list really needs a filtering HTTP(S) proxy that
the sandboxed plugin is forced through. Both goals also cut off `localhost`, so
a plugin that talks to a local ComfyUI or Ollama would have to declare
`localhost` and be treated as a network plugin.

| Option | Platform | What it gives | Feasibility | Packaging impact |
| --- | --- | --- | --- | --- |
| **seccomp filter** set in `pre_exec` (deny `socket()` for `AF_INET`, `AF_INET6`, `AF_PACKET`; keep `AF_UNIX`), e.g. with the pure-Rust `seccompiler` crate | Linux 3.5+ | No network at all, inherited by every child process; unprivileged with `no_new_privs` | Good. Small, testable, one code path in `host.rs` next to the process group setup | None: no helper binary, works in deb, rpm and AppImage, and inside Flatpak or Snap (filters stack) |
| **Landlock** network rules | Linux 6.7+ (ABI 4) | Deny TCP `connect`/`bind`, by port only; UDP not covered | Partial: too new for many LTS kernels, and port rules cannot express hosts | None |
| **bubblewrap** `bwrap --unshare-net` (or `unshare(CLONE_NEWUSER \| CLONE_NEWNET)` directly) | Linux | Empty network namespace (only its own loopback) | Works where unprivileged user namespaces are allowed; Ubuntu 23.10+ restricts them through AppArmor except for `bwrap`'s own profile; not available nested inside Flatpak | `bwrap` becomes a runtime dependency (deb/rpm `Depends`); an AppImage cannot rely on it; host allow-lists still need a proxy (`pasta`/`slirp4netns` plus filtering) |
| **AppContainer** without the `internetClient` capability | Windows 8+ | No network, loopback blocked too | Medium: launch through `STARTUPINFOEX` with `SECURITY_CAPABILITIES`, which composes with the Job Object we already use. The container also loses file access: Xuan must grant its SID read access to the plugin folder and execute access to the interpreter, and a Python installed under the user profile does not run without changing ACLs there | No installer change; needs more `windows` crate features. ACL changes on user folders are hard to undo cleanly |
| **WFP** filters or a firewall rule per program | Windows | Per-program, even per-address rules | Poor: needs administrator rights or a service, and plugins share `python.exe`, so a rule per program cannot tell them apart | An elevated installer component or service; rejected |
| `sandbox-exec` profiles | macOS | Deny network | Deprecated API; macOS is not a release target | — |

**Recommendation.**

1. Do not attempt per-host allow-lists. Keep the per-run prompt for plugins
   that declare hosts; they stay unsandboxed and the prompt is the control.
2. **Done (#43).** Enforce "no network" for plugins that declare **no** hosts
   on Linux with a seccomp filter applied in the child before `exec`. It is
   unprivileged, adds no runtime dependency, works in every package format,
   and turns offline mode into a real guarantee on Linux. It shipped behind
   the opt-in setting "Block network for plugins that don't declare it",
   built with `seccompiler`. The filter is an allow-list rather than the
   deny-list in the table: `socket()` and `socketpair()` fail with `EACCES`
   for every family but `AF_UNIX` and `AF_NETLINK` (so `AF_SMC`, `AF_VSOCK`
   and the like are covered), `io_uring_setup()` fails too, other syscall
   ABIs are killed and x32 numbers are covered. The plugin log notes that
   the network is blocked; a blocked call itself is not logged, since
   `SECCOMP_RET_ERRNO` leaves no trace. It needs Linux 4.14, and a plugin
   whose filter cannot be installed does not start. To make it the default
   after a release, set `BLOCK_UNDECLARED_NETWORK_DEFAULT` in
   `src/config.rs` to `true`: configurations store the setting only once
   the user changes it.
3. On Windows, prototype AppContainer for the same "no network" case only once
   the Linux path has proven the UX; ship it opt-in and document that
   interpreters must be installed for all users. Skip WFP.
4. Leave bubblewrap and Landlock aside: bubblewrap adds a dependency and does
   not work in all the places Xuan is packaged, and Landlock cannot express
   what is needed on the kernels users have.

Track the Linux and Windows steps as separate tickets.

## 6. Format

**Have.** `.xuan` version 6 stores a `generated` object on each plugin-made
layer: plugin id and version, action, the inputs the user chose (including
prompt and seed), source layer, a hash of the pixels sent, and a timestamp.
**Layer → Re-run Plugin Action…** reads it back, with inputs validated against
the manifest on load.

**Missing.**

- ~~**Model identity.**~~ **Done in #40.** An `image` (or `document`) output may
  carry a strict, size-limited `provenance` object (model, weights hash, sampler,
  scheduler, steps, seed, cfg, service, request id and an `extra` map). The host
  stores it next to `generated`, the layers panel shows it read-only under
  **Generation** with a **Copy** button, and the project is saved as format 8
  only when some layer has one. See
  [PLUGINS.md](PLUGINS.md#provenance) and
  [FORMAT.md](FORMAT.md#model-provenance-version-8). The Comfy Cloud example
  reports the checkpoint, sampler, seed and job id its workflow used.
- ~~**Secrets must never reach provenance.**~~ **Done in #40.** Credential-like
  keys, the plugin's declared secret names and any text containing one of its
  secret values are removed before the record is checked or stored.
- **Export.** Flattened PNG/JPEG exports carry no mark that the content is
  generated. See C2PA above.

**Recommendation.** Done: `provenance` is added and shown, and the version is
bumped only when a layer has a field to write. C2PA export stays a separate
research spike (see section 5); the record is Xuan's own metadata, is not a
C2PA manifest and is not written to exports.

## 7. Distribution

**Have.** Plugins are folders with `plugin.toml` in the user plugins
directory (`~/.config/xuan/plugins`, `%APPDATA%\xuan\plugins`) or
`XUAN_PLUGIN_PATH`. The manifest carries id, version, protocol and permissions.
Two folders with one id are rejected. **Plugins → Install from Folder or Zip…**
(#41) checks a folder or an untrusted archive in a private temporary folder,
shows a review, and copies it into the plugins directory, updating a plugin
with the same id in place. `requires_xuan` is a semver range of compatible Xuan
versions, and a `setup` script convention covers venvs and models (see
"Installing" in [PLUGINS.md](PLUGINS.md)).

The issue suggested `$XDG_DATA_HOME`; the implementation uses the config
directory. It is documented and works; moving data-like content out of config is
a cosmetic change and not worth breaking existing installs.

**Missing.**

- **Version comparison.** An update replaces whatever version is installed;
  `version` is shown in the review but not compared, so a downgrade is not
  flagged.
- **Dependencies** (Python, venv) stay the plugin's problem: the `setup`
  convention is documented, not automated. Model files are declared in
  `[[models]]` and downloaded by Xuan (#38).
- **Signing and a registry**: defer (see section 5).

## Goals

| Goal | Already works | Missing | Recommendation |
| --- | --- | --- | --- |
| **Text-to-image** into a new layer | `generate` actions with prompt, seed, size; Comfy Cloud example; `result.into = layer / document / ask`; a send prompt before a network plugin gets the prompt or pixels (#39). | Local ComfyUI/diffusers plugin. Model downloads are done (#38). | Ship a local-ComfyUI variant of the Comfy example, declaring its checkpoints in `[[models]]`. |
| **Inpaint / outpaint** | `regions` with masks and per-region text, `crop_to_regions` with padding, masked result layers, proposal compare; `source.mask = "selection"` with host-side grow and feather (#42); `source.extend` with a new-area mask and an `extend_canvas` edit (#36), shown by `extend-edges`. | A generative outpainting backend. | Host work done; wire an outpainting workflow into a backend of `extend-edges` or the Comfy example. |
| **Background removal / segmentation** | Classical Select Subject, Remove Background and Object mode in core (#5); `selection/export`, `set_selection` and `set_mask` edits, `replace` results, a `mask` output that becomes a selection (#37), providers that replace the built-in algorithm (#5) and the `select-bright` template. | An ONNX segmentation plugin. | Build it on `select-bright`: a backend with the model declared in `[[models]]`, and `[[provides]]` for the capabilities it serves. |
| **Upscaling** | Local plugin pattern, `local-upscale` prototype, tiling-friendly `selection` source. | Large-image speed. Placed size (#35) and model downloads (#38) are done. | An ONNX Real-ESRGAN backend whose model is declared in `[[models]]` (see the `local-upscale` README). |
| **Others** (style transfer, colorize, denoise, captions) | Same job/result machinery; `text` output for captions; panes for assist UIs. | Nothing specific. | Plugins only; no host work. |

## Prototype

`plugins/local-upscale` is a Python plugin using only the standard library: a
Lanczos resampler with an optional unsharp mask, premultiplied-alpha aware. It
declares no network, secrets or edit rights, returns its result as a layer
placed over the source at the source's size, and loads additional backends from `backend_<name>.py`, which is where
an ONNX Real-ESRGAN implementation would go (onnxruntime in the plugin's own
virtualenv, never in Xuan). See its README for how to try it and for the
backend steps. Tests: `python3 -m unittest discover -s plugins/local-upscale`.

`plugins/select-bright` is the same pattern for segmentation: it sends the
composite at most 1024 pixels on a side, returns a grey `mask` output with
`fit = "source"`, and Xuan proposes it as the selection. Its built-in backend
selects by brightness; a `backend_<name>.py` with a `segment` function is
where an ONNX U²-Net or IS-Net model would go. It needs only `document =
"read"`, and it declares `[[provides]] capability = "select_subject"`, so
choosing it in Settings → Selection makes Select → Subject run it instead of
the built-in GrabCut. Tests: `python3 -m unittest discover -s
plugins/select-bright`.

**An ONNX segmentation plugin, step by step.** Start from `select-bright`:

1. Create `.venv` in the plugin folder with `onnxruntime`, `numpy` and
   `pillow` (a `setup.sh`/`setup.ps1` that pins them), and point `command` at
   `.venv/bin/python main.py`. Nothing of this reaches Xuan's binary.
2. Declare the model in `[[models]]` with its https URL, size, SHA-256 and
   licence, and list it in the action's `models`. Xuan asks, downloads,
   verifies and passes the path (`job.model_path("u2net")`).
3. Write `backend_u2net.py`: resize the source to the model's input, run the
   session, return the matte; `fit_source=True` scales a 320×320 matte over
   any source.
4. Declare what it can replace: `select_subject` (and `object_select`, using
   `inputs.point` / `inputs.rect` to crop or prompt the model) need only
   `document = "read"`; `remove_background` needs `document = "edit"`, since
   Xuan lays its mask on the layer.
5. The user picks it under Settings → Selection. If it is disabled, or offline
   mode turns it off, Xuan uses the built-in algorithm and says so.
