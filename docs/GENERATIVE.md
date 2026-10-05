# Generative and ML features

Design notes for issue #6: how Xuan gets text-to-image, inpainting, background
removal, upscaling and similar features without putting models in the core.
This was written after the plugin system landed (#21, #23, #34; hardening in
#33), so most of the original questions already have an answer. Each section
says what exists, what is missing, and what to do. The plugin protocol itself is
in [PLUGINS.md](PLUGINS.md); the stored metadata is in [FORMAT.md](FORMAT.md).

Related tickets: **#5** (Select Subject, Color Range, ML background removal)
and **#7** (MCP server and client).

## Summary of recommendations

1. **Keep out-of-process JSON-RPC plugins as the only extension model.** Do not
   add WASM or embedded scripting. Models run in the plugin's own process or
   virtualenv; Xuan never links onnxruntime, torch or a model.
2. **Make network access enforceable or stop calling it a permission.**
   Done in #39 for the cheap part: the grant dialog says `permissions.network`
   is not enforced, a per-run prompt asks before document data goes to a plugin
   that declares hosts, and offline mode disables such plugins. OS-level
   enforcement is a separate step; see the spike in section 5.
3. **Add three small host features that every generative plugin needs:** a
   result that declares its placed size (upscale), canvas extension (outpaint)
   and a `mask` output kind that becomes a selection (segmentation, #5).
   The placed size is done in #35 and the `mask` output in #37.
4. **Give models a home:** a per-plugin `models_dir` with a manifest-declared,
   hash-checked download, shown to the user with its size before it starts.
5. **Defer C2PA, plugin signing and the OS keyring.** Record the provenance we
   already store, add the model and prompt to it, and revisit when there is a
   plugin registry.

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
  edits and total pixels (100 MP).
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
  padded image plus an alpha mask of the new area.
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

## 4. Backends

**Have.** The Comfy Cloud example (`plugins/comfy-cloud`) does text-to-image and
region-driven editing over a hosted API with secrets, progress, cancel and the
documented error codes. The prototype `plugins/local-upscale` shows a fully
local job with no network.

**Recommended shapes** (all are ordinary plugins):

| Goal | Backend | Notes |
| --- | --- | --- |
| Text-to-image | ComfyUI (local or cloud), diffusers, or a hosted API | Comfy example already covers the cloud path. A local ComfyUI plugin is the same code with a `localhost` base URL. |
| Inpaint / outpaint | Same, with a mask workflow | Needs the host features in section 2. |
| Background removal, segmentation | ONNX Runtime in the plugin (U2-Net, IS-Net, SAM-class models) | Plugin returns a `mask` output (#37); `select-bright` has a backend hook. #5 decides which model and licence ship. |
| Upscaling | Real-ESRGAN or similar via ONNX Runtime in the plugin | `local-upscale` has a backend hook. |
| Prompt assist, captioning | Ollama or llama.cpp over localhost | A pane or action that returns `text` or fills an input. |
| Style transfer, colorize, denoise | Same ONNX pattern | Per-model plugins. |

**Missing.**

- **Model download and caching.** Plugins download models themselves, with no
  shared location, size confirmation or hash check. A model is often 50 MB to
  several GB. Add a per-plugin `models_dir` (next to `data_dir`) and an optional
  `[[models]]` manifest table with `id`, `url`, `sha256`, `size`. The host shows
  the size and host, downloads once with a progress bar, verifies the hash, and
  gives the path to the plugin. This also makes "offline after first run"
  checkable.
- **Python environments.** Each ML plugin needs its own virtualenv, created by
  the user or an install script. The manifest `command` can point into it. A
  documented convention is enough; do not manage environments in the host.
- **Bundled model for #5.** If Select Subject must work out of the box, ship it
  as a first-party plugin with the model as a downloadable (not in the binary).
  Core Xuan keeps its edge-colour matte as the offline fallback.
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

- **`permissions.network` is only a label.** The plugin process can contact any
  host. Since #39 the grant dialog says so in plain words. Real enforcement
  needs OS sandboxing; see the spike below.
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
  is a guarantee only together with OS enforcement for the others.
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
  say so in plugin guidance.

### Spike: enforcing network access in the OS (#39)

No code yet; this compares the options for making `permissions.network`
real. Two goals are possible: **no network at all** for plugins that declare no
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
2. Enforce "no network" for plugins that declare **no** hosts on Linux with a
   seccomp filter applied in the child before `exec`. It is unprivileged, adds
   no dependency, works in every package format, and turns offline mode into
   a real guarantee on Linux. Ship it first behind a setting, report a blocked
   `socket()` in the plugin log, and make it the default after a release.
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

- **Model identity.** The inputs hold a model *choice* if the plugin exposes
  one, but a plugin cannot record what it actually used (resolved checkpoint,
  weights hash, sampler, steps, service request id). Add an optional
  `provenance` object to an `image` output (free-form JSON, size-limited) that
  the host stores next to `generated` and shows in the layer's properties.
  This is a format change (a new optional field; bump to the next version
  when it is written).
- **Secrets must never reach provenance.** Today inputs cannot contain secrets;
  keep it that way when adding `provenance`.
- **Export.** Flattened PNG/JPEG exports carry no mark that the content is
  generated. See C2PA above.

**Recommendation.** Add `provenance` and show it. Do not bump the version
without a field to write.

## 7. Distribution

**Have.** Plugins are folders with `plugin.toml` in the user plugins
directory (`~/.config/xuan/plugins`, `%APPDATA%\xuan\plugins`) or
`XUAN_PLUGIN_PATH`. The manifest carries id, version, protocol and permissions.
Two folders with one id are rejected.

The issue suggested `$XDG_DATA_HOME`; the implementation uses the config
directory. It is documented and works; moving data-like content out of config is
a cosmetic change and not worth breaking existing installs.

**Missing.**

- **An install flow.** Users copy a folder by hand. Add **Plugins → Install
  from Folder or Zip…** that validates the manifest, shows the same permission
  review, and copies into the plugins directory. No registry yet.
- **Version and update handling.** `version` is displayed but never compared;
  there is no compatibility range for the host. Add `requires_xuan` (a semver
  range) and refuse plugins that need a newer protocol with a clear message.
- **Dependencies** (Python, venv, models) are the plugin's problem; document a
  `setup` convention rather than automate it.
- **Signing and a registry**: defer (see section 5).

## Goals

| Goal | Already works | Missing | Recommendation |
| --- | --- | --- | --- |
| **Text-to-image** into a new layer | `generate` actions with prompt, seed, size; Comfy Cloud example; `result.into = layer / document / ask`; a send prompt before a network plugin gets the prompt or pixels (#39). | Local ComfyUI/diffusers plugin; model download story. | Ship a local-ComfyUI variant of the Comfy example; add `models_dir`. |
| **Inpaint / outpaint** | `regions` with masks and per-region text, `crop_to_regions` with padding, masked result layers, proposal compare. | Canvas extension for outpainting; a simple "selection as mask" input; hard-edge versus feathered mask control. | Add `extend` to the source and an `extend_canvas` edit op. |
| **Background removal / segmentation** | `selection/export`, `set_selection` and `set_mask` edits, `replace` results, a `mask` output that becomes a selection (#37) and the `select-bright` prototype. | A shipped model and licence decision (#5). | The host `mask` output is done; do the model and Select Subject UI in #5, implemented as a first-party ONNX plugin. |
| **Upscaling** | Local plugin pattern, `local-upscale` prototype, tiling-friendly `selection` source. | Result placed-size control; model download; large-image speed. | Add `width`/`height` to image outputs; later an ONNX Real-ESRGAN backend. |
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
"read"`. Tests: `python3 -m unittest discover -s plugins/select-bright`.
