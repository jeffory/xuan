# Project format

A `.xuan` file is a ZIP archive containing `manifest.json` and lossless PNG assets. Version 1 uses `format: "me.silverl.xuan"`, a `document` object, and a `pixel_layers` list. Layer images are stored at `images/<UUID>.png`; masks use `images/<UUID>.mask.png`. Pixel bytes are excluded from JSON.

The document records canvas dimensions, DPI, layer order, IDs, parent groups, clipping references, opacity, blending, visibility, locks, transforms, adjustment parameters, and optional live shape styles. Layers are stored bottom to top; each folder's subtree is composited together in hierarchy order. A folder's `opacity` multiplies into every layer inside it (nested folders multiply too); folders are pass-through, so their `blend` is not used. Every version stores and renders folder opacity, so setting it needs no newer version. Masks have enabled/linked flags and an optional independent placement transform.

Text layers also record an optional `text` object with UTF-8 content, font family, pixel size, RGBA color, and bold, italic, underline, and strikethrough flags. Their PNG assets preserve the rendered appearance when fonts are unavailable on another machine. Fonts are discovered from the system and are not embedded in the project; editing unavailable fonts uses the bundled Inter Variable fallback. Text is limited to 16 KiB and font sizes to 1–1024 pixels. Older version 1 files without text metadata remain supported. Text can also follow a path ([version 11](#text-on-a-path-version-11)).

Transforms retain original source pixels. Optional perspective corners are normalized coordinates before affine scale/rotation/flip. Channel adjustments retain separate RGB curves/levels and seven hue ranges. Selections, current multi-selection, clipboard contents, and undo/redo snapshots are not serialized.

Saving validates the document, writes a sibling temporary archive, flushes it, and atomically replaces the destination. Loading validates IDs, hierarchy, clipping cycles, transforms, dimensions, metadata size, decompressed asset size, and aggregate image/mask budgets. Assets are decoded in memory, never extracted using archive paths. `.comp` package reads reject symlinked assets and paths outside the package.

Limits: 65,535 pixels per canvas/image dimension, 10,000 layers, 64 nested group levels and 4 MiB manifest JSON. How many pixels a canvas, layer or mask may have, how many layer and mask pixels a project may bring in when it is opened, how large an encoded asset may be and how much RAW data it may embed follow the computer's memory; see [size limits](USAGE.md#size-limits) (at least 100 megapixels per image, 100 megapixels of layers plus 100 megapixels of masks, 512 MiB per asset and 512 MiB of RAW files). Only opening a project checks the totals; saving checks each image's size, so a document edited in Xuan always saves. Assets of 4 GiB or more are written as ZIP64 entries.

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

The package is untrusted input. Besides the limits above, the importer rejects (and opens nothing for) manifests that break upstream's rules: unknown versions, blend modes, adjustment kinds or shape kinds; fields used before the version that introduced them (folder opacity or guides before 8, blur/noise before 9, `colorRuns` before 10, `fontRuns` before 11); folders with a blend mode other than Normal; more than 1,000 guides, duplicate guide IDs or positions beyond ±1,000,000; values outside upstream's ranges (blur radius 0.1–250, motion angle ±90 and distance 1–2,000, noise 0.1–400, font size 1–2,000, colors 0–1, tracking −100–1,000, leading 0–5,000, paragraph boxes 16–30,000 per side and 200 million square pixels); text over 100,000 UTF-16 units; text runs that overlap, are empty, overflow or end past the text; run font names over 200 characters or with line breaks; text on layers without pixels; malformed `effects` records or effect settings outside upstream's ranges; asset names other than `<layer UUID>.png` / `.mask.png`, symlinked assets and paths leaving the package; more than 64 nested folder levels and folder cycles. The manifest is limited to 4 MiB (and serde_json's nesting limit of 128), canvases to upstream's 30,000 pixels per side, and each asset, image and the layer and mask totals to the [size limits](USAGE.md#size-limits) of the computer opening it. Hierarchy and clipping checks use an index, so a 10,000-layer project validates in linear time.

## Importing Photoshop files

The importer reads Photoshop documents, PSD (version 1) and PSB (version 2, with 8-byte section, channel and PackBits row lengths), following Adobe's *Photoshop File Formats Specification* and upstream Compositor's importer (`Compositor/IO/PSD/`) for scope and mapping. Like upstream, it takes **8-bit RGB** only: Bitmap, Grayscale, Indexed Color, CMYK, Multichannel, Duotone and Lab files, and 1-, 16- and 32-bit files, are refused with a message naming the mode or depth. Import is one-way; Save creates a `.xuan` file.

The file is read in memory: the header, color mode data (skipped), image resources (only ResolutionInfo is used; malformed resources are ignored), layer records, channel data (raw, PackBits, ZIP and ZIP with prediction) and, for files without layer records, the merged image (raw or PackBits). Groups come from section dividers (`lsct`/`lsdk`), names from `luni` (else Mac OS Roman), fill opacity from `iOpa`.

| Photoshop feature | In Xuan |
| --- | --- |
| Pixel layers: position, opacity, visibility, names | Editable |
| Fill opacity | The layer's Fill (`fill`, [version 13](#fill-version-13)): it fades the pixels but not the layer effects. On adjustment layers it is multiplied into the opacity; folders have none |
| Groups (folders) and their opacity | Editable folders. Folders always pass through; another folder blend mode is reported |
| Layer masks: bounds, default color, disabled, linked | Editable masks on the layer's grid (a black-default mask covers only its stored area) |
| Masks rendered from vector data, vector masks on pixel layers | Left out |
| Clipping | Editable clipping to the nearest unclipped layer or folder below in the same folder; a clipped folder, and clipping onto a left-out layer, is released |
| All 27 Photoshop blend modes (Normal through Luminosity, including Dissolve, Darker/Lighter Color, Linear Burn/Dodge, the Light modes, Hard Mix, Exclusion, Subtract, Divide) | Editable. One table (`BLEND_MODES` in `src/io/psd.rs`) maps keys to modes |
| Unknown blend keys | Drawn as Normal and reported |
| Levels, Curves, Exposure, Invert, Black & White (weights and an RGB tint as hue and saturation) and Color Balance adjustment layers | Editable adjustment layers (no blend mode; a non-Normal one is reported) |
| Other adjustment layers (Hue/Saturation, Brightness/Contrast, Vibrance, Photo Filter, Channel Mixer, …) | Left out, with the layer |
| Solid-filled rectangle, rounded rectangle (equal radii) and ellipse shapes without a stroke (`vogk` + `SoCo`/`vscg`) | Editable live shapes |
| Other shapes and vector content | Photoshop's pixels; solid shapes saved without pixels are drawn from their path; others are left out |
| Horizontal type (`TySh`) without rotation, skew or warp, 1–1,024 px | Editable text (content, font, size, color, bold, italic, underline, strikethrough), keeping Photoshop's pixels until edited. Alignment, tracking, leading, paragraph boxes and further style runs are not represented |
| Vertical, warped, rotated or unreadable type | Photoshop's pixels |
| Smart objects, fill layers (solid, gradient, pattern) | Photoshop's pixels (a solid fill saved without pixels covers the canvas) |
| Layer effects (`lfx2`): stroke (solid), drop shadow, inner shadow, outer glow, inner glow (solid color), color overlay, with the effects scale and global light angle | Editable layer effects, switched-off ones included. Effect blend modes other than Photoshop's defaults, spread/choke, noise, centered strokes, glows from the center and sizes beyond Xuan's ranges are approximated and reported. Effects on folders or layers without pixels are left out |
| Bevel and emboss, satin, gradient and pattern overlays, gradient or pattern strokes and glows, several effects of one kind, legacy `lrFX`-only effects | Left out and reported |
| Files without layers | The merged image, as one layer named Background |

Before anything is applied, the app shows what will change (the same `ImportReport` as `.comp` imports, counted per kind); Cancel leaves everything as it was. Files Xuan represents completely open without asking. Imported as a layer, a file's layers arrive in a folder named after it, centered on the canvas.

The file is untrusted input. Every read is bounds checked, and every section, record, block and channel length is checked against the bytes that remain before it is used, so truncated files are rejected wherever they end (only the merged image may be missing when layers exist). Nothing is allocated from a declared size before it has been checked: raw channels must hold `width × height` bytes, PackBits row tables are read and summed against the data (each row needs at least two bytes per 128 pixels) before a plane is allocated, and ZIP channels inflate row by row. Limits:

- Files up to a quarter of the computer's memory, and at least 1 GiB; canvases up to 30,000 pixels per side for `.psd` (Photoshop's rule) and 65,535 for `.psb`, within the image limit of the [size limits](USAGE.md#size-limits).
- At most 10,000 layer records and 56 channels per layer or document.
- Layer and mask bounds up to 300,000 pixels per side (slightly inverted bounds, which Photoshop writes for empty layers, read as empty).
- The layers and masks opening a file may add (see [size limits](USAGE.md#size-limits); at least 100 megapixels of each), less what the open document holds when importing as a layer. When the layers do not fit, or lie beyond ±1,000,000 pixels, every layer and mask is cropped to the canvas (and reported); a file that still does not fit is refused.
- ZIP channels up to 400 MB inflated, or the image limit where that is larger.
- 64 nested folders; unbalanced folders are rejected.
- Descriptors nest at most 32 levels and 100,000 values; text engine data nests at most 64 levels and 1,000,000 values; vector paths up to 10,000 knots.
- PackBits runs that would write past a row or read past its bytes, unknown compression methods, invalid ZIP data and out-of-range bounds reject the file. Unreadable descriptors only turn a layer into pixels.

## OpenRaster import and export

Xuan reads and writes OpenRaster (`.ora`), the layered format Krita, GIMP and MyPaint share, following the OpenRaster 0.0.6 specification and Krita's reader and writer (`plugins/impex/ora/`) for the blend modes OpenRaster has no name for. An `.ora` file is a ZIP archive: `mimetype` (first and stored, `image/openraster`), `stack.xml` (an `image` with `w`, `h`, `xres`, `yres` and one root `stack`), one PNG per layer under `data/`, `mergedimage.png` and `Thumbnails/thumbnail.png`. Opening one is one-way like a Photoshop file: Save creates a `.xuan` file.

| OpenRaster | In Xuan |
| --- | --- |
| `stack` | A folder, with its name, opacity, visibility and `edit-locked`. Xuan's folders pass through, so a stack's own `composite-op` is reported (`isolation="auto"`, Krita's pass-through, needs nothing); an isolated stack, the default, whose layers blend is reported too, since those layers now also blend with what is below the folder. Stack `x` and `y` are ignored, as 0.0.6 says |
| `layer` with a PNG `src` | A pixel layer at its `x`, `y` (negative offsets included), with its name, opacity, visibility, `edit-locked`, and `selected` (the active layer) |
| `composite-op` | One table (`BLEND_MODES` in `src/io/ora.rs`) below, plus Krita's own names for the same modes and the names older Krita versions wrote. `svg:src-atop`, `svg:dst-atop`, `svg:dst-in`, `svg:dst-out`, Krita's `alpha-preserve` and unknown operations are drawn as Normal and reported |
| Krita's `filter` (adjustment) layers, non-PNG sources, layers without `src`, unknown elements | Left out and reported |
| `mergedimage.png`, thumbnail | Not read: Xuan draws the layers itself |

| Xuan blend mode | `composite-op` written (and read) |
| --- | --- |
| Normal, Multiply, Screen, Overlay, Darken, Lighten, Color Dodge, Color Burn, Hard Light, Soft Light, Difference, Color, Luminosity, Hue, Saturation | `svg:src-over`, `svg:multiply`, `svg:screen`, `svg:overlay`, `svg:darken`, `svg:lighten`, `svg:color-dodge`, `svg:color-burn`, `svg:hard-light`, `svg:soft-light`, `svg:difference`, `svg:color`, `svg:luminosity`, `svg:hue`, `svg:saturation` |
| Linear Dodge (Add) | `svg:plus` |
| Dissolve, Linear Burn, Darker Color, Lighter Color, Vivid Light, Linear Light, Pin Light, Hard Mix, Exclusion, Subtract, Divide | Krita's `krita:dissolve`, `krita:linear_burn`, `krita:darker color`, `krita:lighter color`, `krita:vivid_light`, `krita:linear light`, `krita:pin_light`, `krita:hard mix`, `krita:exclusion`, `krita:subtract`, `krita:divide` (other programs draw these as Normal) |

As with Photoshop files, what changes is listed before anything is applied, and a file imported as a layer arrives in a folder named after it.

**Export** (**File → Export Image…**, format ORA, and plugins' `file/export` with `ora`) writes the archive the specification describes: `mimetype` first, stored, with no extra field; `stack.xml` (version 0.0.6, the document's resolution as `xres`/`yres`, and a root stack without attributes); `data/layerN.png`; `mergedimage.png` at the canvas size; and a thumbnail at most 256 pixels a side keeping the aspect ratio. Folders become stacks with `isolation="auto"` (Xuan's folders pass through) and pixel layers are written as their own pixels at their offsets, so exporting and opening again gives the same layers, names, offsets, opacity, visibility, locks, blend modes and pixels. What OpenRaster cannot hold is drawn, alone on a transparent canvas and cropped to what it covers, and listed after the export (the dialog lists it beforehand):

- Text and shape layers, layer effects, layer masks (applied to the pixels), Fill below 100%, layers moved by a fraction of a pixel, scaled, rotated, flipped or warped, and adjustments, filters and masks attached to a layer: the layer is drawn with them, and its opacity, blend mode and visibility stay on the element.
- Clipped layers are drawn into their base.
- Adjustment, filter and mask layers in a folder are merged with the visible layers below them in that folder; hidden ones are left out. Hidden layers below stay as they are.
- A folder with its own mask is drawn as one layer.
- RAW layers are written as their developed pixels.

The archive is untrusted input, under the same rules as `.xuan` projects: at most 30,001 entries; `stack.xml` up to 4 MiB, without DTDs (so no entity expansion) and at most 100,000 XML nodes; a `mimetype` entry reading `image/openraster` is required; layer paths must be relative paths inside the archive (no leading `/`, drive letters or other `:`, backslashes, or empty, `.` or `..` parts) and are refused otherwise; at most 10,000 layers and 64 nested stacks; canvases and layers within the image limit of the [size limits](USAGE.md#size-limits) and offsets within ±1,000,000 pixels. Each layer PNG's size is read from its header before the rest is inflated: its pixels must fit what opening a file may add (less what the open document holds when importing as a layer), and the entry may hold at most its largest possible pixel data plus 16 MiB, so a small archive cannot inflate into gigabytes. Assets are decoded in memory, never extracted.

## Embedded RAW (version 2)

Documents containing RAW layers are written as version 2, preventing older readers from silently dropping the source. Ordinary documents continue to use version 1, and the reader supports both versions.

A RAW image layer has an optional `raw` object containing the original filename, camera metadata, and validated `DevelopSettings`. Its original bytes are stored in `raw/<layer UUID>.nef` (a legacy archive name used for every supported RAW format, including Canon, Fujifilm, and Sony sources); the current developed render stays in `images/<layer UUID>.png`. Loading restores the image without decoding the sensor data. Reopening Develop decodes the embedded bytes, so moving/deleting the original file does not break the project. Missing sources, invalid settings, and oversized sources fail validation. RAW byte buffers are shared by layer duplicates and history snapshots, and included in history's memory accounting.

The archive allows up to 30,001 entries to accommodate an image, mask and RAW source for each layer. Aggregate embedded RAW bytes are limited when a project is opened, to a thirty-second of the computer's memory and at least 512 MiB (see [size limits](USAGE.md#size-limits)). Develop settings store crop/overlay coordinates relative to the full camera-oriented image, before the user’s quarter-turn rotation. `quarter_turns` stores 0–3 clockwise turns and defaults to 0 in older projects; curve knots and all numeric parameters are finite and range-checked. Develop's transient preview, comparison view, clipping indicators, worker state, and local undo history are not serialized.

## Attached effect stacks (version 4)

Documents with image children or filter layers use version 4. The reader still
accepts versions 1–3. A mask, adjustment, or filter layer can name an image layer
as its `parent`; image and folder children remain restricted to folders. Each
image's effect children are evaluated in document order, bottom to top, before
its opacity, blending, and clipping are composited with other images.

Mask children use `standalone_mask: true` and the existing mask PNG assets. Their
parent distinguishes image-only coverage from standalone masks over lower
siblings. The `filter` field stores Gaussian Blur, Motion Blur, Add Noise, or Lens
Correction settings (the vignette's sign changed in [version 12](#lens-correction-vignette-sign-version-12)). Parameters, hierarchy, and cycles are validated on load.
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
Guides follow Crop, Trim, Canvas Size, Image Size, Flip Canvas and Rotate Canvas.

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

## Clipping to a folder (version 9)

A layer's `clip_to` may name a folder (`group: true`) as well as a layer. The
clipping shape is then the folder's composited alpha: its visible pixel layers and
nested folders combined bottom to top as `a + below × (1 − a)`, mask layers inside it
fading what is below them, then the folder's own opacity and mask. Adjustment and filter
layers inside add nothing. As for a layer base, the base's opacity applies to the clipped
layer, and the base's own visibility does not. No layer may clip to a folder whose shape
depends on it: a layer inside the folder, or clipped through a chain to one. A document
in which any layer clips to a folder is written as version 9, so older builds report an
unsupported version instead of a missing clipping source; everything else keeps the lowest version its content needs
(1-8). The reader accepts versions 1-9.

## Model provenance (version 8)

A layer a plugin produced may also carry a `provenance` object beside its `generated`
object: the model, sampler and service request the plugin reported with its result. A
document in which any layer has one is written as version 8; everything else keeps the
lowest version its content needs (1-7), so files that never used it stay readable by older
builds. A version 8 document may also carry anything the earlier versions can, such as
layer effects and `generated`. The reader accepts versions 1-8.

The object is strict. Unknown keys fail validation (anything else belongs under `extra`):

| Key | Type |
| --- | --- |
| `model`, `model_hash`, `sampler`, `scheduler`, `service`, `request_id` | string, at most 256 bytes, no control characters |
| `weights_sha256` | exactly 64 hexadecimal digits |
| `steps` | integer 0-1,000,000 |
| `seed` | integer 0-18,446,744,073,709,551,615 |
| `cfg` | finite number, at most 1,000,000 in magnitude |
| `extra` | object of up to 32 entries with keys of 1-64 bytes; values are strings (256 bytes), finite numbers, booleans, nulls and objects or arrays of up to 32 entries, nested at most 4 levels counting `extra` itself |

The serialized object is at most 8 KiB. Loading and saving refuse a record that breaks
these limits or holds a key that looks like a credential. Plugin results are stored through
the same checks and with secrets removed; see [PLUGINS.md](PLUGINS.md#provenance). The
record is metadata only: pixels, masks and rendering are unaffected, and the layer panel
shows it read-only. It is not C2PA content credentials, and exports do not carry it.

## Vector paths and path shapes (version 10)

A document that keeps paths, or has a path shape layer, is written as version 10;
everything else keeps the lowest version its content needs (1-9), so older builds report
an unsupported version instead of dropping the paths or showing a shape they cannot
redraw. A version 10 document may also carry anything the earlier versions can. The
reader accepts versions 1-10.

Paths are SVG path data (the `d` attribute of an SVG `<path>`), stored as Xuan writes it:
absolute `M`, `L`, `Q`, `C` and `Z` commands with comma-separated coordinates, e.g.
`"M0,700 C120,640 380,640 512,700 Z"` (arcs are stored as the cubic curves they became).
Reading accepts any SVG path data (M, L, H, V, C, S, Q, T, A, Z, absolute or relative), at
most 256 KiB, with at most 10,000 segments and every coordinate finite and within
±1,000,000.

- The `document` object gains an optional `paths` key: up to 1,000 objects `{"id": UUID,
  "name": string, "d": path data}` in the Paths dialog's order, in document pixels. Names
  are 1-256 bytes and not blank; IDs are unique. Paths follow Crop, Trim, Canvas Size, Image
  Size, Flip Canvas and Rotate Canvas.
- A layer's `shape` may have `"kind": "Path"` with a `path` object `{"d": path data,
  "width": number, "height": number, "fill_rule": "nonzero" | "evenodd"}`. The outline is
  in the coordinates of a `width` × `height` box (each 1-300,000), which the layer's
  pixels cover, so the outline stretches with the layer's transform; `fill_rule` defaults
  to `nonzero`, as in SVG, and open subpaths are filled as if closed. `corner_radius` is
  0. A `Path` shape must have `path`, and other kinds must not. The layer's PNG holds the
  antialiased fill (16 sample rows per pixel, exact coverage across each row), so readers
  that do not redraw shapes still show it.

Invalid path data, names or boxes fail validation on load and save.

## Text on a path (version 11)

A document with a text layer set along a path is written as version 11; everything else
keeps the lowest version its content needs (1-10), so older builds report an unsupported
version instead of dropping the path and setting the text in a box the next time it is
edited. A version 11 document may also carry anything the earlier versions can. The reader
accepts versions 1-11.

A layer's `text` object may have a `path` object:

```json
{"d": "M0,40 C50,-30 180,-30 230,40", "width": 236, "height": 64,
 "start_offset": 50, "align": "center", "side": "left", "letter_spacing": 0,
 "rotate": true, "baseline_shift": 0, "size_end": 7, "opacity_start": 0.95,
 "opacity_end": 0.55}
```

- `d` is SVG path data in the coordinates of a `width` × `height` box (each 1-300,000),
  which the layer's pixels cover, as for a path shape's outline, so the path moves,
  stretches and turns with the layer's transform. Only its first subpath is followed.
  Xuan rewrites the path and box whenever it redraws the text, so the box is the layer's
  pixel size.
- `start_offset` (percent of the path's length, −100-100), `align` (`start`, `center` or
  `end`), `side` (`left` or `right`), `letter_spacing` (pixels, −1,000-1,000), `rotate`,
  `baseline_shift` (pixels, −10,000-10,000), `size_end` (1-1,024, optional) and
  `opacity_start` / `opacity_end` (0-1) are as in
  [PLUGINS.md](PLUGINS.md#text-on-a-path); each defaults when left out (0, `start`,
  `left`, 0, true, 0, none, 1, 1).

The layer's PNG holds the text as drawn along the path (glyph outlines filled with 16
sample rows per pixel), so readers that cannot lay text along a path still show it.
Invalid path data, boxes or options fail validation on load and save.

## Lens Correction vignette sign (version 12)

A filter layer's `{"LensCorrection": {"distortion", "vignette"}}` uses Photoshop's sign
from version 12: a negative `vignette` darkens the corners and a positive one brightens
them. Versions 1-11 stored the opposite sign, so the reader negates `vignette` when it
opens an older file, and the project looks as it did. A document with a nonzero vignette
is written as version 12, so older builds report an unsupported version instead of
drawing it inverted; everything else keeps the lowest version its content needs (1-11),
and a Lens Correction with no vignette reads the same in every version. A version 12
document may also carry anything the earlier versions can. The reader accepts versions
1-12.

## Fill (version 13)

A pixel, text or shape layer may have a `fill` (0-1), Photoshop's Fill: it fades the
layer's own pixels but not its layer effects, while `opacity` fades both. In Color Burn,
Linear Burn, Color Dodge, Linear Dodge (Add), Vivid Light, Linear Light, Hard Mix and
Difference it changes the blend instead of fading it (`blend::blend_channel_filled` has
the rule). The key is written only below 1, and a document with such a layer is written as
version 13, so older builds report an unsupported version instead of drawing the layer at
full fill; everything else keeps the lowest version its content needs. A missing `fill`
reads as 1. Folders and mask, adjustment and filter layers have no fill: a value other
than 1 on one, or one outside 0-1, fails validation on load and save. A version 13
document may also carry anything the earlier versions can. The reader accepts versions
1-13.

## Compositor filters (version 14)

Four filters come from upstream Compositor's Filter menu (`Document/Filters.swift`), with its
settings, ranges and defaults. On a filter layer or an attached filter they are stored as:

- `{"Vignette": {"amount", "color", "midpoint", "roundness", "feather", "highlights"}}`:
  `amount` 0-100 (35), `color` an `[r, g, b]` byte triple (black), `midpoint` 0-100 (50),
  `roundness` -100-100 (100), `feather` 0-100 (60) and `highlights` 0-100 (25). A filter
  layer frames the whole canvas and also paints its transparent areas, as upstream's
  vignette on an empty layer does; an attached Vignette frames and recolors its layer's
  pixels.
- `{"Bloom": {"amount", "radius"}}` (Bloom / Glow): `amount` 0-100 (40), `radius` 1-150
  pixels (24).
- `{"TonalContrast": {"amount", "radius", "shadows", "midtones", "highlights"}}`: `amount`
  0-100 (50), `radius` 1-100 pixels (16), and the three strengths -100-100 (40, 60, 30).
- `{"Dither": {...}}`: `style` (`Atkinson`, `FloydSteinberg`, `Bayer2`, `Bayer4`, `Bayer8`,
  `HalftoneDots`, `HalftoneLines`, `HalftoneDiamonds`, `MacPatterns`, `Ascii` or
  `Scanlines`; `Atkinson`), `pixel_size` 1-32 (2), `pixel_shape` (`Square` or `Dot`),
  `cell_size` 4-64 (8), `text_size` 6-64 (14), `line_spacing` 2-32 (4), `glow` 0-100 (35),
  `dots` 0-100 (0), `wobble` 0-64 (0), `angle` -90-90 (45), `levels` 2-8 (2), `diffusion`
  0-100 (100), `density` and `contrast` -100-100 (0), `colors` (`BlackWhite`, `TwoColors` or
  `Original`), `dark` and `light` `[r, g, b]` (black and white), `light_on_dark` (true) and
  `characters` (at most 64, one line; `" .:-=+*#%@"`). Missing Dither fields take these
  defaults.

A document in which any layer uses one of them is written as version 14, so older builds
report an unsupported version instead of failing on a filter they do not know; everything
else keeps the lowest version its content needs (1-13). A version 14 document may also
carry anything the earlier versions can, such as a layer's Fill. The reader accepts
versions 1-14. Upstream's
packages (`.comp` version 11 and earlier) keep these filters only as edits to pixels, not as
adjustment records, so an import has none to bring across as filter layers.
