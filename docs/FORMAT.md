# Project format

A `.xuan` file is a ZIP archive containing `manifest.json` and lossless PNG assets. Version 1 uses `format: "me.silverl.xuan"`, a `document` object, and a `pixel_layers` list. Layer images are stored at `images/<UUID>.png`; masks use `images/<UUID>.mask.png`. Pixel bytes are excluded from JSON.

The document records canvas dimensions, DPI, layer order, IDs, parent groups, clipping references, opacity, blending, visibility, locks, transforms, adjustment parameters, and optional live shape styles. Layers are stored bottom to top; each folder's subtree is composited together in hierarchy order. A folder's `opacity` multiplies into every layer inside it (nested folders multiply too); folders are pass-through, so their `blend` is not used. Every version stores and renders folder opacity, so setting it needs no newer version. Masks have enabled/linked flags and an optional independent placement transform.

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
| `Black & White`, `Color Balance` adjustments (7) | The same adjustment layers (`blackWhiteSettings`, `colorBalanceSettings`; missing fields take upstream's defaults); the project then saves as `.xuan` version 7 |
| `text` (content, `fontName`, `fontSize`, color) | Editable text: the PostScript name becomes a family plus bold/italic (`HelveticaNeue-BoldItalic` → Helvetica Neue, bold, italic) |
| `text` alignment, `tracking`, `leading`, `boxSize`; `colorRuns` (10); `fontRuns` (11) | Not represented. The layer's PNG keeps the original look until the text is edited in Xuan |
| text over 16 KiB or larger than 1024 px | Imported as plain pixels |
| Photoshop blend modes (Linear Burn, Linear Dodge (Add), Soft Light, Hard Light, Vivid Light, Linear Light, Pin Light, Hard Mix, Exclusion, Subtract, Divide) | The same blend modes; the project then saves as `.xuan` version 7 |
| `effects` (stroke, shadow, color overlay, inner shadow, outer/inner glow) | Layer effects with the same settings (colors rounded to 8 bits; missing fields take upstream's defaults); the project then saves as `.xuan` version 7. Effects on folders or adjustment layers, which upstream never draws, are left out |
| `shape` of kind `Line` | Imported as plain pixels |

Whatever is left out or changed is counted, and the app shows a summary after opening the package ("Imported with changes …"). Clipping masks that relied on a left-out layer, or on a filter layer, are released and counted too.

The package is untrusted input. Besides the limits above, the importer rejects (and opens nothing for) manifests that break upstream's rules: unknown versions, blend modes, adjustment kinds or shape kinds; fields used before the version that introduced them (folder opacity or guides before 8, blur/noise before 9, `colorRuns` before 10, `fontRuns` before 11); folders with a blend mode other than Normal; more than 1,000 guides, duplicate guide IDs or positions beyond ±1,000,000; values outside upstream's ranges (blur radius 0.1–250, motion angle ±90 and distance 1–2,000, noise 0.1–400, font size 1–2,000, colors 0–1, tracking −100–1,000, leading 0–5,000, paragraph boxes 16–30,000 per side and 200 million square pixels); text over 100,000 UTF-16 units; text runs that overlap, are empty, overflow or end past the text; run font names over 200 characters or with line breaks; text on layers without pixels; malformed `effects` records or effect settings outside upstream's ranges; asset names other than `<layer UUID>.png` / `.mask.png`, symlinked assets and paths leaving the package; more than 64 nested folder levels and folder cycles. The manifest is limited to 4 MiB (and serde_json's nesting limit of 128), each asset to 512 MiB, and decoded images to 30,000 pixels per side and 100 megapixels of layers plus 100 megapixels of masks. Hierarchy and clipping checks use an index, so a 10,000-layer project validates in linear time.

## Importing Photoshop files

The importer reads Photoshop documents, PSD (version 1) and PSB (version 2, with 8-byte section, channel and PackBits row lengths), following Adobe's *Photoshop File Formats Specification* and upstream Compositor's importer (`Compositor/IO/PSD/`) for scope and mapping. Like upstream, it takes **8-bit RGB** only: Bitmap, Grayscale, Indexed Color, CMYK, Multichannel, Duotone and Lab files, and 1-, 16- and 32-bit files, are refused with a message naming the mode or depth. Import is one-way; Save creates a `.xuan` file.

The file is read in memory: the header, color mode data (skipped), image resources (only ResolutionInfo is used; malformed resources are ignored), layer records, channel data (raw, PackBits, ZIP and ZIP with prediction) and, for files without layer records, the merged image (raw or PackBits). Groups come from section dividers (`lsct`/`lsdk`), names from `luni` (else Mac OS Roman), fill opacity from `iOpa`.

| Photoshop feature | In Xuan |
| --- | --- |
| Pixel layers: position, opacity, visibility, names | Editable |
| Fill opacity | Multiplied into the layer opacity (kept at layer opacity when the layer has effects, as upstream does) |
| Groups (folders) and their opacity | Editable folders. Folders always pass through; another folder blend mode is reported |
| Layer masks: bounds, default color, disabled, linked | Editable masks on the layer's grid (a black-default mask covers only its stored area) |
| Masks rendered from vector data, vector masks on pixel layers | Left out |
| Clipping | Editable clipping to the nearest unclipped layer below in the same folder; clipping onto a folder or a left-out layer is released |
| Blend modes Normal, Darken, Multiply, Color Burn, Lighten, Screen, Color Dodge, Overlay, Difference, Hue, Saturation, Color, Luminosity | Editable |
| Dissolve, Linear Burn, Darker Color, Linear Dodge (Add), Lighter Color, Soft Light, Hard Light, Vivid Light, Linear Light, Pin Light, Hard Mix, Exclusion, Subtract, Divide, unknown keys | Drawn as Normal. One table (`BLEND_MODES` in `src/io/psd.rs`) maps keys to modes |
| Levels, Curves, Exposure, Invert adjustment layers | Editable adjustment layers (no blend mode) |
| Other adjustment layers (Hue/Saturation, Brightness/Contrast, Color Balance, Black & White, …) | Left out, with the layer |
| Solid-filled rectangle, rounded rectangle (equal radii) and ellipse shapes without a stroke (`vogk` + `SoCo`/`vscg`) | Editable live shapes |
| Other shapes and vector content | Photoshop's pixels; solid shapes saved without pixels are drawn from their path; others are left out |
| Horizontal type (`TySh`) without rotation, skew or warp, 1–1,024 px | Editable text (content, font, size, color, bold, italic, underline, strikethrough), keeping Photoshop's pixels until edited. Alignment, tracking, leading, paragraph boxes and further style runs are not represented |
| Vertical, warped, rotated or unreadable type | Photoshop's pixels |
| Smart objects, fill layers (solid, gradient, pattern) | Photoshop's pixels (a solid fill saved without pixels covers the canvas) |
| Layer effects | Left out |
| Files without layers | The merged image, as one layer named Background |

Before anything is applied, the app shows what will change (the same `ImportReport` as `.comp` imports, counted per kind); Cancel leaves everything as it was. Files Xuan represents completely open without asking. Imported as a layer, a file's layers arrive in a folder named after it, centered on the canvas.

The file is untrusted input. Every read is bounds checked, and every section, record, block and channel length is checked against the bytes that remain before it is used, so truncated files are rejected wherever they end (only the merged image may be missing when layers exist). Nothing is allocated from a declared size before it has been checked: raw channels must hold `width × height` bytes, PackBits row tables are read and summed against the data (each row needs at least two bytes per 128 pixels) before a plane is allocated, and ZIP channels inflate row by row. Limits:

- Files up to 1 GiB; canvases up to 30,000 pixels per side and 100 megapixels.
- At most 10,000 layer records and 56 channels per layer or document.
- Layer and mask bounds up to 300,000 pixels per side (slightly inverted bounds, which Photoshop writes for empty layers, read as empty).
- 100 megapixels of layers and 100 megapixels of masks, less what the open document holds when importing as a layer. When the layers do not fit, or lie beyond ±1,000,000 pixels, every layer and mask is cropped to the canvas (and reported); a file that still does not fit is refused.
- ZIP channels up to 400 MB inflated.
- 64 nested folders; unbalanced folders are rejected.
- Descriptors nest at most 32 levels and 100,000 values; text engine data nests at most 64 levels and 1,000,000 values; vector paths up to 10,000 knots.
- PackBits runs that would write past a row or read past its bytes, unknown compression methods, invalid ZIP data and out-of-range bounds reject the file. Unreadable descriptors only turn a layer into pixels.

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
before, without the new keys. The reader accepts versions 1–6; older projects load with
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

## Plugin provenance (version 6)

Documents with a layer produced by a plugin use version 6, which may also
carry the guides and layout grid of version 5. The reader accepts versions
1–6. Such a layer carries a `generated` object: the plugin `id` and
`version`, the `action`, the `inputs` the user chose (regions in
document coordinates with their fields), the `source` layer id, a `source_hash`
of the pixels that were sent (`fnv1a:` prefix), and an RFC 3339 `created`
timestamp. **Layer → Re-run Plugin Action…** uses it to repeat the action with
the same inputs. Nothing else about the layer changes: its pixels and mask are
stored like any image layer, so a reader without the plugin shows the result
unchanged. See [PLUGINS.md](PLUGINS.md).

## Photoshop blend modes, adjustments and layer effects (version 7)

Documents in which a layer uses one of the blend modes added with Photoshop's full
set, a Black & White or Color Balance adjustment, or layer effects are written as
version 7; everything else keeps the lowest version its content
needs (1–6). A version 7 document may also carry anything the earlier versions can,
including plugin provenance. The reader accepts versions 1–7.

A layer's `blend` is one of the original `Normal`, `Multiply`, `Screen`, `Overlay`,
`Darken`, `Lighten`, `Difference`, `ColorDodge`, `ColorBurn`, `Hue`, `Saturation`,
`Color` and `Luminosity`, or, from version 7, `Dissolve`, `LinearBurn`, `DarkerColor`,
`LinearDodge`, `LighterColor`, `SoftLight`, `HardLight`, `VividLight`, `LinearLight`,
`PinLight`, `HardMix`, `Exclusion`, `Subtract` or `Divide`. The formulas are
Photoshop's, computed in sRGB on straight colors. Dissolve keeps a pixel fully
opaque with a probability equal to its coverage (alpha × opacity × masks), using a
fixed hash of the document pixel's coordinates, so the pattern is the same on every
machine and between the CPU and GPU renderers.

Two adjustment kinds are added, following upstream Compositor's settings and ranges:

- `{"BlackWhite": {"weights": [r, y, g, c, b, m], "tint": bool, "tint_hue": number,
  "tint_saturation": number}}`: how bright reds, yellows, greens, cyans, blues and
  magentas become in gray, each −200–300 (%). With `tint`, the gray becomes the
  lightness of a color at `tint_hue` (0–360°) and `tint_saturation` (0–100%).
- `{"ColorBalance": {"shadows": [cr, mg, yb], "midtones": [...], "highlights": [...],
  "preserve_luminosity": bool}}`: shifts toward red (from cyan), green (from magenta)
  and blue (from yellow), each −100–100, for each tonal range.

Values outside these ranges fail validation on load and save.

A pixel or text layer may have an `effects` object, after upstream Compositor's layer
effects. Each key is optional; a missing key is an effect the layer does not have, and
missing fields inside one take the defaults in parentheses. Every effect has `enabled`
(true), a `color` `[r, g, b]` and an `opacity` (0–1); sizes and distances are in the
layer's own pixels:

- `stroke`: `size` 0–500 (4), `inside` (false), opacity 1.
- `drop_shadow` and `inner_shadow`: `angle` −360–360 (90; degrees counterclockwise
  from the right to the light, so 90 drops the shadow straight down), `distance` 0–5000
  (20; 10 for the inner shadow), `blur` 0–500 (20; 10), opacity 0.5, black.
- `color_overlay`: opacity 1.
- `outer_glow` and `inner_glow`: `size` 0–500 (20; 10), opacity 0.75, white.

Effects are drawn when the document is rendered and are never saved as pixels. They
follow the layer's pixels through its own mask; the result is drawn at the layer's
opacity (and its folders'), in its blend mode, and clipped like the layer itself. A
layer clipped to one with effects is clipped to the effects too. Folders, masks,
adjustment and filter layers cannot have effects.
