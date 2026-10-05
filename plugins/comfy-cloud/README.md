# Comfy Cloud plugin

Runs ComfyUI workflows on [Comfy Cloud](https://cloud.comfy.org), a Comfy API
deployment, or a local ComfyUI behind the API proxy, through
[Comfy API v2](https://docs.comfy.org/api-reference/v2/overview). Written in
Python with the standard library only.

## Setup

1. Copy or symlink this folder into Xuan's plugins directory (see
   `docs/PLUGINS.md`), or add its parent to `XUAN_PLUGIN_PATH`.
2. In Xuan open **Plugins → Manage Plugins… → Comfy Cloud**, allow the
   permissions and paste an API key from
   [platform.comfy.org](https://platform.comfy.org). Running workflows through
   the API needs an active Comfy Cloud subscription; partner nodes such as
   Ideogram also use credits.
3. Put your workflows in the `workflows` folder next to this file, or in the
   plugin data folder (`plugin-data/comfy-cloud` next to `config.toml`).

## Workflows

Export a workflow from ComfyUI with **Export (API)** (the plain export with
`nodes` and `links` is rejected by the API). The plugin fills nodes in by
their *title* (right-click a node → Title):

| Title | What is set |
| --- | --- |
| `Xuan Source` (or a `Load Image` node) | `image` ← the uploaded source |
| `Xuan Prompt` | `text` / `prompt` / `string` ← the prompt |
| `Xuan Seed` | `seed` / `noise_seed` ← the seed |
| `Xuan Size` | `width`, `height` |

Any string input may also use `{{prompt}}`, `{{seed}}`, `{{width}}`,
`{{height}}` and `{{quality}}` placeholders.

The three actions use these files:

- **Precise Edit…** → `workflows/precise-edit.json`. The boxes you draw become
  an Ideogram-style JSON prompt (`compositional_deconstruction.elements[]` with
  `bbox` in 0–1000 and `desc`), which is written into the `Xuan Prompt` node,
  so you do not need the *Create Bounding Boxes* node in the graph: Load Image →
  Ideogram edit node → Save Image is enough.
- **Generate Image…** → `workflows/text-to-image.json`.
- **Run Comfy Workflow…** → any file name you type.

The shipped JSON files are templates with the titles in place. Replace their
node classes and inputs with an export of your own graph; the API rejects
workflows whose node classes it does not know and names the node in the error.

## Output

Results come back as a proposal above the canvas: **Compare**, **Accept** or
**Discard**. Precise Edit adds a new layer masked to the boxes; Generate asks
whether to add a layer or open a new document.
