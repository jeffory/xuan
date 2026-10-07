# Comfy Cloud plugin

Generates and edits images with Seedream and Ideogram on
[Comfy Cloud](https://cloud.comfy.org), using Comfy's own workflow templates
and keeping them up to date. It talks to
[Comfy API v2](https://docs.comfy.org/api-reference/v2/overview) and is
written in Python with the standard library only.

## Setup

1. Copy or symlink this folder into Xuan's plugins directory (see
   `docs/PLUGINS.md`), or add its parent to `XUAN_PLUGIN_PATH`.
2. In Xuan open **Plugins → Manage Plugins… → Comfy Cloud**, allow the
   permissions and paste an API key from
   [platform.comfy.org](https://platform.comfy.org). Running workflows through
   the API needs an active Comfy Cloud subscription, and Seedream and
   Ideogram use credits.

## Actions

| Action | Menu | Models | What it does |
| --- | --- | --- | --- |
| **Generate Image…** | Plugins | Seedream 5.0 Pro, Seedream 5.0 Flash, Ideogram 4.5 | A new image from a prompt, in one of seven shapes at about 1 or 4 megapixels. You choose between a new layer and a new document. |
| **Edit Image…** | Filter | Ideogram 4.5, Seedream 5.0 Pro | Repaints the active layer as a new layer, following a description of the change. |
| **Precise Edit…** | Filter | Ideogram 4.5 | Draw boxes, or turn the selection into one, and describe each change. Only what you asked for changes, and the new layer is masked to the boxes. A box of kind **Text** writes the text you give it. |
| **Split into Layers…** | Layer | Seedream 5.0 Pro, Seedream 5.0 Flash | Separates the flattened image into a background plate and layers you can move, all added as new layers. You can say what to separate, or leave it empty to find the main elements. |
| **Generate Layer…** | Layer | GPT Image 2.5 Flare, GPT Image 2.5 Sunburst | Draws what you describe on a transparent background, as a new layer the size of the canvas. With **Match the picture** on (the default), GPT draws it into the flattened image, then Seedream 5.0 Flash lifts just that object out, so the layer fits the picture's colours, lighting and layout (two steps: about $0.05 and a minute more). Off, only the prompt is sent and GPT draws on a transparent background directly. |
| **Remove Background (Bria)** | Filter | Bria RMBG 2.0 | Cuts the active layer's subject out, as a new layer. Choose **Comfy Cloud** under **Settings → Selection** to use it for Xuan's own **Filter → Remove Background**, which then gives the layer a mask you can paint, and **Select → Subject**. |

Results arrive as a proposal above the canvas, with **Compare**, **Accept**
and **Discard**. Each result layer records its provenance (Layers panel →
Generation): the model, the seed, the Comfy template and its date, the
server and the Comfy job id. The API key is never included.

## Where the workflows come from

Each action runs one of Comfy's published templates, listed in `recipes.py`
with the node inputs Xuan fills in. Inputs are addressed by node class and
input name, such as `ByteDanceSeedreamNodeV3.model.seed`, so they keep
working when Comfy renumbers a template's nodes.

| Recipe | Template |
| --- | --- |
| Generate, Seedream 5.0 Pro | `api_bytedance_seedream_5_0_pro_t2i` |
| Generate, Seedream 5.0 Flash | `api_bytedance_seedream_5_0_flash_t2i` |
| Generate, Ideogram 4.5 | `api_ideogram_v4_5_t2i` |
| Edit, Ideogram 4.5 | `api_ideogram_v4_5_image_edit` |
| Edit, Seedream 5.0 Pro | `api_bytedance_seedream_5_0_pro_image_edit` |
| Precise Edit | `api_ideogram_v4_5_precise_image_edit` |
| Split into Layers | `api_bytedance_seedream_5_0_layer_separation` |
| Generate Layer, GPT Image 2.5 Flare | `api_openai_gpt_image_25_flare_t2i` |
| Generate Layer, GPT Image 2.5 Sunburst | `api_openai_gpt_image_25_sunburst_t2i` |
| Remove Background | `utility_bria_remove_image_background` |

Templates are public at `https://cloud.comfy.org/templates/<name>.json`, but
they are saved in the editor's format, which the API does not run.
`convert.py` turns them into API workflows using the server's node
definitions (`/api/object_info`). It handles dynamic combos, seed control
values, Reroute and Primitive nodes, and list values. A template that uses
anything else, such as subgraphs or bypassed nodes, is refused.

**Updates.** At most once a day per workflow, the plugin downloads the
template again. When it changed, the plugin converts the new version, checks
that every input it fills in still exists, and switches to it, keeping the
old one. When the new version can't be used, the plugin keeps the one it has
and says why on its page in **Plugins → Manage Plugins…** and in the status
bar after a job.
When Comfy refuses a new version before running it, the job is retried once
with the previous version, at no cost. The node definitions (about 10 MB)
are only downloaded when a template has changed. **Check for updates** on the
plugin's page in Manage Plugins checks every workflow now. (Xuan 0.4 and older
show that page's contents as a **Comfy Cloud** pane in the sidebar instead.)

The plugin's data folder (`plugin-data/comfy-cloud/recipes/` next to
`config.toml`) holds the converted versions. `snapshots/` holds the versions
shipped with the plugin, used until the first check. Regenerate them, and the
test fixtures, with:

```sh
python3 plugins/comfy-cloud/tools/refresh_snapshots.py --testdata
```

This needs the API key in `COMFY_API_KEY` or saved in Xuan. It downloads
the templates and node definitions only, so no credits are spent.

## Tests

The converter is checked against what Comfy's own converter builds for each
template (`testdata/oracle.json`). The update rules, the recipes and the
checks that keep the API key on the configured server also have tests. All
of them run offline with the system Python:

```sh
python3 -m unittest discover -s plugins/comfy-cloud
```
