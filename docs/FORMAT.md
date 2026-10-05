# Project format

A `.xuan` file is a ZIP archive containing `manifest.json` and lossless PNG assets. Version 1 uses `format: "me.silverl.xuan"`, a `document` object, and a `pixel_layers` list. Layer images are stored at `images/<UUID>.png`; masks use `images/<UUID>.mask.png`. Pixel bytes are excluded from JSON.

The document records canvas dimensions, DPI, layer order, IDs, parent groups, clipping references, opacity, blending, visibility, locks, transforms, adjustment parameters, and optional live shape styles. Layers are stored bottom to top; each folder's subtree is composited together in hierarchy order. Masks have enabled/linked flags and an optional independent placement transform.

Text layers also record an optional `text` object with UTF-8 content, font family, pixel size, RGBA color, and bold, italic, underline, and strikethrough flags. Their PNG assets preserve the rendered appearance when fonts are unavailable on another machine. Fonts are discovered from the system and are not embedded in the project; editing unavailable fonts uses the bundled Inter Variable fallback. Text is limited to 16 KiB and font sizes to 1–1024 pixels. Older version 1 files without text metadata remain supported.

Transforms retain original source pixels. Optional perspective corners are normalized coordinates before affine scale/rotation/flip. Channel adjustments retain separate RGB curves/levels and seven hue ranges. Selections, current multi-selection, clipboard contents, and undo/redo snapshots are not serialized.

Saving validates the document, writes a sibling temporary archive, flushes it, and atomically replaces the destination. Loading validates IDs, hierarchy, clipping cycles, transforms, dimensions, metadata size, decompressed asset size, and aggregate image/mask budgets. Assets are decoded in memory, never extracted using archive paths. `.comp` package reads reject symlinked assets and paths outside the package.

Limits: 30,000 pixels per canvas/image dimension, 100 megapixels per canvas, 100 megapixels of layer assets plus 100 megapixels of masks, 10,000 layers, 64 nested group levels, 4 MiB manifest JSON, and 512 MiB encoded asset files.

## Importing Compositor packages

The importer reads Compositor `.comp` directory packages, format versions 1–11 (Compositor 1.4.5 writes 11), including Swift's alternating-key enum dictionaries, individual color channels, mask placement/link flags, grain parameters, and live shape styles. Import is one-way: Save creates a `.xuan` file and leaves the `.comp` package untouched. A newer version is refused rather than half read.

| Upstream field (version) | In Xuan |
| --- | --- |
| folder `opacity` (8) | Folder opacity, multiplied into every layer inside |
| top-level `guides` (8) | `guides`; the project then saves as `.xuan` version 5. Packages hold no layout grid (upstream keeps it as an app preference), so `grid` stays unset |
| `Invert` adjustment (7) | Invert adjustment layer |
| `Gaussian Blur`, `Motion Blur`, `Add Noise` adjustments (9) | Filter layers. Settings beyond Xuan's ranges are reduced (blur radius to 100, motion distance to 200, noise amount to 100); the motion angle is negated because upstream measures it counterclockwise; Gaussian noise becomes uniform and the noise seed is not kept. Filter layers ignore blend modes and clipping |
| `Black & White`, `Color Balance` adjustments (7) | Left out, with the layer |
| `text` (content, `fontName`, `fontSize`, color) | Editable text: the PostScript name becomes a family plus bold/italic (`HelveticaNeue-BoldItalic` → Helvetica Neue, bold, italic) |
| `text` alignment, `tracking`, `leading`, `boxSize`; `colorRuns` (10); `fontRuns` (11) | Not represented. The layer's PNG keeps the original look until the text is edited in Xuan |
| text over 16 KiB or larger than 1024 px | Imported as plain pixels |
| Photoshop blend modes Xuan lacks (Linear Burn, Linear Dodge (Add), Soft Light, Hard Light, Vivid Light, Linear Light, Pin Light, Hard Mix, Exclusion, Subtract, Divide) | Drawn as Normal |
| `effects` (stroke, shadow, color overlay, inner shadow, outer/inner glow) | Left out |
| `shape` of kind `Line` | Imported as plain pixels |

Whatever is left out or changed is counted, and the app shows a summary after opening the package ("Imported with changes …"). Clipping masks that relied on a left-out layer, or on a filter layer, are released and counted too.

The package is untrusted input. Besides the limits above, the importer rejects (and opens nothing for) manifests that break upstream's rules: unknown versions, blend modes, adjustment kinds or shape kinds; fields used before the version that introduced them (folder opacity or guides before 8, blur/noise before 9, `colorRuns` before 10, `fontRuns` before 11); folders with a blend mode other than Normal; more than 1,000 guides, duplicate guide IDs or positions beyond ±1,000,000; values outside upstream's ranges (blur radius 0.1–250, motion angle ±90 and distance 1–2,000, noise 0.1–400, font size 1–2,000, colors 0–1, tracking −100–1,000, leading 0–5,000, paragraph boxes 16–30,000 per side and 200 million square pixels); text over 100,000 UTF-16 units; text runs that overlap, are empty, overflow or end past the text; run font names over 200 characters or with line breaks; text on layers without pixels; malformed `effects` records; asset names other than `<layer UUID>.png` / `.mask.png`, symlinked assets and paths leaving the package; more than 64 nested folder levels and folder cycles. The manifest is limited to 4 MiB (and serde_json's nesting limit of 128), each asset to 512 MiB, and decoded images to 30,000 pixels per side and 100 megapixels of layers plus 100 megapixels of masks. Hierarchy and clipping checks use an index, so a 10,000-layer project validates in linear time.

## Embedded RAW (version 2)

Documents containing RAW layers are written as version 2, preventing older readers from silently dropping the source. Ordinary documents continue to use version 1, and the reader supports both versions.

A RAW image layer has an optional `raw` object containing the original filename, camera metadata, and validated `DevelopSettings`. Its original bytes are stored in `raw/<layer UUID>.nef` (a legacy archive name used for every supported RAW format, including Canon, Fujifilm, and Sony sources); the current developed render stays in `images/<layer UUID>.png`. Loading restores the image without decoding the sensor data. Reopening Develop decodes the embedded bytes, so moving/deleting the original file does not break the project. Missing sources, invalid settings, and oversized sources fail validation. RAW byte buffers are shared by layer duplicates and history snapshots, and included in history's memory accounting.

The archive allows up to 30,001 entries to accommodate an image, mask and RAW source for each layer. Aggregate embedded RAW bytes are limited to 512 MiB. Develop settings store crop/overlay coordinates relative to the full camera-oriented image, before the user’s quarter-turn rotation. `quarter_turns` stores 0–3 clockwise turns and defaults to 0 in older projects; curve knots and all numeric parameters are finite and range-checked. Develop's transient preview, comparison view, clipping indicators, worker state, and local undo history are not serialized.

## Attached effect stacks (version 4)

Documents with image children or filter layers use version 4. The reader still
accepts versions 1–3. A mask, adjustment, or filter layer can name an image layer
as its `parent`; image and folder children remain restricted to folders. Each
image's effect children are evaluated in document order, bottom to top, before
its opacity, blending, and clipping are composited with other images.

Mask children use `standalone_mask: true` and the existing mask PNG assets. Their
parent distinguishes image-only coverage from standalone masks over lower
siblings. The `filter` field stores Gaussian Blur, Motion Blur, Add Noise, or Lens
Correction settings. Parameters, hierarchy, and cycles are validated on load.
Source pixels are retained; intermediate effect rasters are not saved. Legacy
single image masks are promoted to child layers when opened in the editor.

## Guides and layout grid (version 5)

Documents with guides or a layout grid of their own are written as version 5. Other
documents keep the lowest version their content needs (1–4) and are written exactly as
before, without the new keys. The reader accepts versions 1–5; older projects load with
no guides and the app's default grid.

The `document` object gains two optional keys:

- `guides`: up to 1,000 objects `{"id": UUID, "axis": "horizontal" | "vertical",
  "position": number}`. A horizontal guide sits at a document Y, a vertical one at an X,
  in document pixels; positions may be fractional or outside the canvas but must be finite
  and within ±1,000,000. IDs are unique. Guides are listed in creation order.
- `grid`: the project's layout grid, `{"spacing": 2–4096, "subdivisions": 1–64 and no more
  than spacing, "color": "light_gray" | "light_blue" | "light_red" | "green" |
  "medium_blue" | "yellow" | "magenta" | "cyan" | "black" | "custom", "custom_color":
  [r, g, b], "style": "lines" | "dashed_lines" | "dots", "opacity": 1–100}`. Missing
  fields take the defaults (64, 8, light gray, [179, 179, 179], lines, 45). When the key
  is absent the app's default grid (View → Grid Settings…) is used.

Invalid guides or grid settings fail validation on load and save. Rulers, grid and guide
visibility, Lock Guides and the Snap To settings are app preferences, not project data.
Guides follow Crop, Canvas Size, Image Size and Flip Canvas.

## Photoshop blend modes (version 6)

Documents in which a layer uses one of the blend modes added with Photoshop's full
set are written as version 6; everything else keeps the lowest version its content
needs (1–5). The reader accepts versions 1–6.

A layer's `blend` is one of the original `Normal`, `Multiply`, `Screen`, `Overlay`,
`Darken`, `Lighten`, `Difference`, `ColorDodge`, `ColorBurn`, `Hue`, `Saturation`,
`Color` and `Luminosity`, or, from version 6, `Dissolve`, `LinearBurn`, `DarkerColor`,
`LinearDodge`, `LighterColor`, `SoftLight`, `HardLight`, `VividLight`, `LinearLight`,
`PinLight`, `HardMix`, `Exclusion`, `Subtract` or `Divide`. The formulas are
Photoshop's, computed in sRGB on straight colors. Dissolve keeps a pixel fully
opaque with a probability equal to its coverage (alpha × opacity × masks), using a
fixed hash of the document pixel's coordinates, so the pattern is the same on every
machine and between the CPU and GPU renderers.
