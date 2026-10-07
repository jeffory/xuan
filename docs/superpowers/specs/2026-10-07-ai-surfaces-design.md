# AI surfaces: plugin actions in the toolbox, the Layers panel and New Image

Status: draft for review · 2026-10-07 · branch `feat/ai-surfaces` (from `feat/comfy-cloud-templates`)

## Goal

Generative plugin actions should feel built into Xuan, not only reachable through a menu and a dialog. Three entry points get them:

- **AI Region tool** in the toolbox: draw a box and say what to do there.
- **New layer with AI** button next to New Layer.
- **Generate** tab in the New Image dialog.

The principle, in the user's words: *keep the simple edits simple; if you need more advanced options, they're there.* Every entry point first shows only a prompt and a Generate button. An **Advanced** section holds the rest, and the full action dialog stays in the menus.

Results should use the best quality for the size they will cover without going under it. They should render one-to-one when the model allows, and a new image can be made exactly the size typed.

## Decisions (user, 2026-10-07)

- **Approach:** plugins declare where their actions appear; Xuan draws the UI with its own widgets. This is option 1 of three. The others were plugin-drawn popovers and Comfy-specific UI in core.
- **AI Region tool:**
  - It is always in the toolbox.
  - Drawing a box opens a small tooltip-style popover with the prompt and Generate. Advanced options are collapsed.
  - It offers three verbs: **Edit** (Precise Edit), **Add** (Generate in region) and **Replace** (Fill region).
- **New layer with AI:** a compact popover under the button.
- **New Image, Generate tab:**
  - The document keeps the model's size, which is at least W×H.
  - An **Exact size** checkbox makes the canvas exactly W×H, with the result as a layer that covers it. Nothing is cropped: the result can be moved to reframe it.

## Part 1: Xuan (host, manifest, SDK)

### Manifest

```toml
[[actions]]
id = "generate-layer"
surfaces = ["layer"]            # new: "layer", "region", "document"; default none
verb = "Add"                    # new: required for "region", short label in the region popover

[[actions.inputs]]
id = "model"
type = "enum"
advanced = true                 # new: behind "Advanced" in popovers and the New Image tab
surfaces = ["menu"]             # new: only where listed ("menu" = the full dialog); default everywhere
```

- `surfaces` on an action lists where it appears besides its menu. Every action keeps its menu item and full dialog; that dialog is the advanced path.
- `advanced = true` applies to an action input or a regions field. In a popover or the New Image tab, it puts the input inside a collapsed **Advanced** section. The full dialog shows every input, as it does today.
- `surfaces` on an input restricts where that input is shown. For example, Generate Image's Shape and Size inputs are menu-only, because in the New Image tab the W×H fields take their place.
- **Validation** (the manifest is refused otherwise):
  - `layer`: an `edit` action whose source is `composite`, or a `generate` action. `result.into` must be `layer`, and the plugin needs `document = "edit"`.
  - `region`: an `edit` action with a `regions` input and a `verb` of at most 24 characters. It needs `document = "edit"`.
  - `document`: a `generate` action whose `result.into` is `document` or `ask`. Xuan always makes a new document.

### Inputs Xuan adds to `action/run` and `action/estimate`

- `inputs.surface` is `"layer"`, `"region"` or `"document"`, and is absent for menu runs.
- `inputs.target` is `{width, height}`: the **document pixels** the result will cover.
  - `layer`: the canvas size.
  - `region`: the box's size. When several boxes go in one job, it is the size of their union.
  - `document`: the W×H typed in New Image.

  The plugin renders at least that size. Region coordinates still arrive in source pixels, as today.

### Results

- New placement: an `image` output may set **`fit: "cover"`**.
  - It scales the image evenly, without stretching, so that it covers the bounds of the source that was sent, and centres it there.
  - Whatever hangs past those bounds stays in the layer.
  - Like every plugin result, the image keeps its own pixels and is placed through the layer's transform.
  - `fit: "source"` still sizes the image to exactly the source bounds, stretching it if its shape differs.
- `layer` and `region` results use `fit: "cover"`. A model renders at least the target, the layer holds all of the model's pixels, and it is shown covering the canvas or the crop.
- `mask_to_regions` still limits what shows to the box, but the rest of the image is in the layer if the user moves it or paints the mask.
- `document` results open a new document at the image's own pixel size, with the dialog's ppi.
- With **Exact size**, the new document is exactly W×H and the result becomes its only layer:
  - The layer is placed with the same cover rule over the canvas, centred, with no empty edge. Any extra hangs past the edges on the longer side.
  - The image is not resampled. As with every plugin result, it keeps the model's pixels and is placed through the layer's transform.
  - Nothing is cropped. The parts past the canvas are kept, so the user can move the layer to reframe; the canvas only clips on export.
  - When the layer extends past the canvas, the status bar says "Move the layer to reframe".

### UI

**New layer with AI.**
- Layers footer: a ✦ icon button between Filter and Delete. Its tooltip and accessibility label are "New layer with AI".
- It is shown only when an enabled plugin has a `layer` action. With one such action it opens that action's popover. With several, it first opens a menu of their labels.

**Popover.** It is shared by all three surfaces: a small `egui::Popup` or `Area` anchored to the button or the box. It contains:
- the action's label;
- its basic inputs in manifest order (the first multiline or text input gets focus);
- a collapsed **Advanced** section with the advanced inputs;
- the estimate line;
- **Generate**, which is the primary button. Ctrl+Enter also generates.

Esc or a click outside closes it.

Values are remembered per action for the session, so the next popover starts from the last prompt and model. Generate starts the job through the usual path. That path includes the permission prompt and the send-consent prompt for network plugins, and the job shows in the status bar.

**AI Region tool.**
- The Region tool becomes a permanent toolbox tool named **AI Region**. It is shown when an enabled plugin has a `region` action, and keeps its current role inside action dialogs.
- **Dialog mode** is unchanged: while an action dialog with a regions input is open, the tool draws that dialog's regions, and tool-mode boxes are hidden until the dialog closes.
- **Tool mode** is when no dialog is open. Boxes are per document and live only in the session:
  - **Drag** to draw a box. Its popover opens next to it.
  - **Click** a box's number badge to reopen its popover.
  - **Delete** removes the selected box.
  - In the tool options bar, **Use Selection** turns the selection into a box that keeps its mask, and **Clear** removes every box.
- **Region popover:**
  - A segmented verb picker (Edit · Add · Replace) lists the `region` actions of all enabled plugins, labelled by `verb`, then the box's basic fields (for Edit, the instruction), then Advanced and Generate.
  - A box keeps its own verb and values.
- **What Generate runs:**
  - An action whose `regions` input allows several boxes (Precise Edit) runs once with every box that has that verb. The button then reads "Edit 3 boxes".
  - An action limited to one box (`max = 1`, as for Add and Replace) runs once per box, now that several jobs can share a document.
  - Boxes that were sent are removed. Their jobs show in the status bar.

**New Image.**
- When an enabled plugin has a `document` action, the dialog gets a segmented **Blank · Generate** switch at the top.
- **Generate** keeps the W×H and ppi fields and adds:
  - an action picker, shown only when there are several;
  - the action's basic inputs;
  - an **Exact size** checkbox, off by default, with the hint "Make the canvas exactly W × H; the image covers it and can be moved";
  - Advanced;
  - the primary button **Generate** in place of Create canvas.
- The new document is named after the prompt and opens when the job finishes.

### Docs

- `docs/PLUGINS.md` gets a new section, "Surfaces", covering the manifest fields, validation, `inputs.surface`, `inputs.target` and the exact-size rule.
- `docs/USAGE.md` describes the three entry points.
- The SDK docs mention the new inputs. The SDKs need no new code: `job.inputs` already carries them.

## Part 2: Comfy Cloud plugin

### Surfaces

| Action | Surfaces | Basic inputs | Advanced |
| --- | --- | --- | --- |
| Generate Image | `document` | prompt | model, seed (Shape and Size are menu-only) |
| Generate Layer | `layer` | prompt, Match the picture | model, quality, seed |
| Precise Edit (verb **Edit**) | `region` | the box's instruction | Kind, Text to write, background, quality, seed |
| Generate in region (new, verb **Add**) | `region` | what to add | model, quality, seed |
| Fill region (new, verb **Replace**) | `region` | what to paint there | model, quality, seed |

### Size rule

Each recipe describes its model's sizes with one of these rules:
- `custom`: a size combo with a "Custom" option plus width and height inputs, with min, max and step read from the definitions.
- `presets`: labels such as "(2K) 2848x1600 (16:9)" or "1536x1024".
- `source`: the model keeps the size of image 1.

Given `inputs.target` (W, H) and a render aspect, the size is chosen as follows:
1. **custom:**
   - Round W and H up to the step.
   - If a side is below the minimum, scale both up evenly until it isn't.
   - If a side is above the maximum, scale both down to fit. This is the only case that ends under the target, and the plugin says so in its text output.
2. **presets:** take the smallest preset of that aspect (within 3%) that is at least W×H on both sides. If none is that big, take the largest preset of the closest aspect and say it will be upscaled.
3. **source:** nothing to choose.

Applied to the models:

| Model | Rule |
| --- | --- |
| GPT Image 2.5 | `custom` 480–3840, step 16, so a target such as 1500×1000 renders exactly at 1504×1008 |
| Seedream 5.0 Pro / Flash | `custom` 1024–4514, step 2, so targets of at least 1024 render exactly |
| Ideogram 4.5 Text to Image | `presets` 1K and 2K |
| Ideogram 4.5 Edit and Precise Edit | `source` |
| Seedream layer separation | `auto` |

From the menu (no target), Generate Image keeps its Shape and Size inputs. The match-source sizing of Edit and Generate Layer becomes the same rule with target = source size.

### New actions

**Generate in region** (`generate-in-region`):
- Source: `composite`, `crop_to_regions` with padding 0.5 so the model sees the surroundings.
- Input: `regions` with `max = 1` and one field, `desc` ("What to add").
- Steps:
  1. GPT Image 2.5 draws the object into the crop, opaque, with the crop as the reference.
  2. Seedream 5.0 Flash lifts that object out. This is the Generate Layer pipeline.
  3. The result is placed with `fit: "cover"` and `mask_to_regions`, so it shows within the box.
- `inputs.target` is the box in document pixels. The plugin scales it by the crop-to-box ratio of the source it received (padding included), so GPT renders at least the crop's size in document pixels.

**Fill region** (`fill-region`):
- Source: `composite`, `crop_to_regions` with padding 0.25.
- Input: `regions` with `max = 1` and a `desc` field ("What to paint here").
- The plugin builds the mask:
  - if the box came from a selection, it uses the box's mask PNG;
  - otherwise it draws a white rectangle with `encode_gray_png`, which is quick because zlib is in C.
- Recipe:
  - GPT Image 2.5 with the crop as `model.images.image_1` and the mask as `model.mask`, where white is replaced.
  - A new recipe role, `mask`, inserts `LoadImageMask` (red channel) and wires it in.
- The result is placed with `fit: "cover"` and `mask_to_regions`.
- Generate Layer, Edit Image and Precise Edit switch from `fit: "source"` to `fit: "cover"` too.
- Both new recipes reuse the existing GPT and Seedream templates; there are no new templates.

## Out of scope

- Per-size cost estimates. The estimate still says "Comfy Cloud credits apply". Comfy's price formulas could drive it later.
- Subgraph templates: Qwen Image 2.1, BiRefNet and Ming.
- Keeping popover values after Xuan restarts.
- Plugin-drawn UI inside surfaces. That was option 2.

## Testing

- **Rust:**
  - Manifest parsing and validation of `surfaces`, `verb`, `advanced` and input `surfaces`.
  - **Layer surface:** the ✦ button opens the popover (kittest), Advanced is collapsed, and Generate starts a job with `surface` and `target`.
  - **Region surface:**
    - drawing a box opens its popover;
    - verbs list the region actions;
    - Edit groups its boxes into one job, while Add and Replace run one job per box;
    - boxes are removed once sent;
    - Use Selection keeps the mask.
  - **New Image:**
    - the Generate tab appears only with a `document` action;
    - the job produces a document;
    - Exact size makes a W×H canvas with one layer that covers it, centred, keeping the image's own pixels;
    - the ppi carries over.
  - The cover placement: `fit: "cover"` scales evenly to cover the source bounds, centres, never stretches and keeps the overflow. It works for layer, region and exact-size document results; `fit: "source"` is unchanged.
- **Plugin:**
  - the size rule for each model (exact, rounded up, scaled up to the minimum, preset choice, too big);
  - the new recipes against fixtures and Comfy's own conversion;
  - the job flows for Generate in region and Fill region with the fake server;
  - the manifest's surfaces match `ACTIONS`.
- **Live:** one paid run each of Add and Replace, and a Generate-tab image at an exact size. The user is asked before any paid run.

## Stages

Each stage gets its own commits and tests.

1. **Xuan:** manifest fields, validation, host inputs (`surface`, `target`), and docs.
2. **Xuan:** the shared popover and the New layer with AI button.
3. **Xuan:** the AI Region tool in tool mode.
4. **Xuan:** the New Image Generate tab and exact size.
5. **Plugin:** the size rule.
6. **Plugin:** surfaces and `advanced` in the manifest, Generate in region, Fill region, the README, and the live checks.
