# Using Xuan

## Install and launch

Xuan runs on Windows 10/11 (x86_64) with DirectX 12 or Vulkan, and on Linux with Wayland or X11 and working Vulkan drivers. Mesa software Vulkan can also run the editor. On Linux, native file dialogs use the desktop portal; install the portal backend for your desktop if dialogs do not appear.

Download the package for your operating system from [Releases](https://github.com/silverling/xuan/releases). GitHub releases provide x86_64 builds.

### Windows

Extract `xuan-<version>-windows-x86_64.zip` into a writable folder and double-click `xuan.exe`. To open a sample composition or specific files from PowerShell:

```powershell
.\xuan.exe --demo
.\xuan.exe photograph.png composition.xuan
```

The portable package needs no installer or administrator access. Keep the accompanying `share` directory for documentation and licenses. To remove the application, delete the extracted folder. Releases are unsigned.

To verify a download in PowerShell, compare the hash to the matching `.sha256` file:

```powershell
Get-FileHash .\xuan-<version>-windows-x86_64.zip -Algorithm SHA256
Get-Content .\xuan-<version>-windows-x86_64.zip.sha256
```

### Linux

For a standalone AppImage, make the downloaded file executable and launch it:

```sh
chmod +x xuan-<version>-x86_64.AppImage
./xuan-<version>-x86_64.AppImage --demo
```

Replace `<version>` with the downloaded version. The AppImage bundles Xuan, its X11/Wayland client libraries, glibc, and a matching dynamic loader; it needs no installation or administrator access. It can run on systems with older glibc, including Ubuntu 20.04. Your system still supplies the Vulkan loader, graphics drivers, and desktop portal. On systems with a newer glibc than the bundled copy, the launcher uses the system's matching libc and loader to support newer graphics drivers. If FUSE is unavailable, run `./xuan-<version>-x86_64.AppImage --appimage-extract-and-run --demo`. To remove it, delete the AppImage. It does not install a launcher or file associations.

Native Linux releases (`.deb`, `.rpm`, and `.tar.gz`) require glibc **2.35** or newer, as provided by Ubuntu 22.04. Use the AppImage on older distributions.

On Debian or Ubuntu, install the downloaded `.deb` with APT so runtime dependencies are installed too:

```sh
sudo apt install ./xuan-*-linux-x86_64.deb
```

On Fedora or another compatible RPM distribution using DNF:

```sh
sudo dnf install ./xuan-*-linux-x86_64.rpm
```

Install one version at a time. These packages add Xuan to the application menu and provide the `xuan` command. Remove them with `sudo apt remove xuan` or `sudo dnf remove xuan`.

For a portable `xuan-<version>-linux-<architecture>.tar.gz` archive, extract it and run:

```sh
scripts/install.sh                 # installs under ~/.local
~/.local/bin/xuan
```

The installer adds a desktop launcher, icons, and the `.xuan` file association. Use `scripts/install.sh /custom/prefix` to choose another location, and add the installation prefix's `bin` directory to `PATH`. You can also run `bin/xuan` directly from the extracted archive.

Each Linux download has a matching `.sha256` file. From the download directory, verify it with `sha256sum --check <package-file>.sha256`.

### Sources and documentation

The separate `xuan-<version>-source.tar.gz` download is for rebuilding the application. The binary packages include a `SOURCES.md` notice under `share/doc/xuan` (`/usr/share/doc/xuan` for Debian/RPM installations) with a link to the matching source release.

Once `xuan` is on `PATH`:

```sh
xuan --demo
xuan photograph.png composition.xuan
```

To compile the application or produce a release archive, see the [development guide](DEVELOPMENT.md). Linux archives built on newer distributions may require a newer glibc; build from source on your target distribution if needed.

## Workspace

Xuan has a charcoal theme, contextual controls above the canvas, a vertical tool rail, document tabs, and a sidebar of panes on the right. The menu bar shares the titlebar with the window controls. Drag the titlebar to move the window, double-click to maximize, or drag an edge to resize.

The sidebar is a stack of panes, each with a header. Click a header to collapse or expand the pane, drag a header up or down to reorder the stack, and drag the line between two panes to resize them; the Layers pane takes whatever space is left. The **Navigator** pane above it shows the whole image with the visible area outlined. The **Window** menu shows or hides each pane, and **Window → Reset Panel Layout** restores the default (Navigator, then Layers, then any plugin panes). The arrangement is saved with your settings. Plugins can add panes of their own (see [plugins](PLUGINS.md)).

### Command palette

Press **Ctrl+K** (or choose **Help → Command Palette…**) to search every command, tool and plugin action. A filter field has focus as soon as the palette opens; type to narrow the list, and press **Ctrl+K** again, **Esc**, or click outside to close it.

- **Matching** is fuzzy: the letters you type must appear in order, and word starts and consecutive letters rank higher. It looks at the command's name (the matched letters are highlighted), its category, other names such as “hsl” for Hue / Saturation, and its identifier. Several words must all match, so “layer new” works. With the interface in Chinese, the translated names and the English names and aliases both work.
- **Order:** better matches first, then the commands you used most recently. With an empty filter the commands you ran lately come first, followed by everything grouped by category.
- **Each row** shows the name, the category in grey and the shortcut now in effect on the right, so the palette also teaches the shortcuts.
- **Keys:** **↑** / **↓** and **Page Up** / **Page Down** move the selection, **Enter** runs it and closes the palette. Hovering highlights a row and clicking runs it.
- **Unavailable commands** (no document open, a job running, or a command of the other workspace) are greyed out and cannot run. In RAW Develop the palette lists the Develop commands and the ones that still apply.
- The palette does not open over a dialog or while a text field has focus. The last 10 commands run from it are saved with your settings (`recent_commands`). Rebind Ctrl+K under **Settings → Keyboard Shortcuts**.

## Editing tools

- **Navigator:** a sidebar pane, above Layers by default, shows a thumbnail of the whole composited document with a blue box marking the part currently visible on the canvas; it follows panning and zooming live. Drag the box to pan, or click anywhere on the thumbnail to centre the view there. Below it, type a zoom percentage into the field and press Enter, use the **−** / **+** buttons, or drag the slider (logarithmic, 1% to 6400%); all zoom about the centre of the canvas. Like every pane it can be collapsed from its header, resized, moved, or hidden from the **Window** menu, and the arrangement is remembered between sessions. The thumbnail is cached and refreshes shortly after you stop editing, switch tabs, or undo/redo. The Navigator is not shown in the RAW Develop workspace, which has its own view controls.
- **Layers:** folders, Photoshop's 27 blend modes (in Photoshop's menu order and grouping: Normal and Dissolve; the darkening modes; the lightening modes; the contrast modes; Difference, Exclusion, Subtract and Divide; then Hue, Saturation, Color and Luminosity), opacity (select a folder to set its own opacity, which dims everything inside it on top of each layer's own; a folder's blend mode stays Normal, so its layers still blend with what is below them, as in upstream Compositor), visibility, locks, drag reordering/nesting, duplication, merge, clipping masks, linked or independent raster masks, and copying layers to another project tab.
- **Layer effects:** **Layer → Layer Effects…** (also in a layer's right-click menu) adds a stroke (outside or inside), a drop shadow, a color overlay, an inner shadow, an outer glow and an inner glow to a pixel or text layer, with upstream Compositor's defaults. Tick an effect to show it, select it to change its size, angle, distance, blur, color and opacity, or remove it. Changes show on the canvas as you make them; **OK** keeps them as one undo step and **Cancel** puts back what the layer had. Effects are kept with the layer rather than painted into it, so they follow every later edit and can be changed or hidden at any time; the Layers panel marks such layers with “fx”. They follow the layer's mask, and the layer's opacity, folders, blend mode and clipping apply to them as well. Effects are drawn on the GPU when one is available.
- **Transforms:** move, scale, rotate, flip, free perspective distortion, numeric controls, shared transforms for several layers or folders, and snapping to guides, the grid, layers and the canvas (see [Rulers, guides and grid](#rulers-guides-and-grid)). Original source pixels remain available during transforms. Move / Transform has **Ignore Transparent Pixels** checked by default to select only at visible pixels; uncheck it to select and drag anywhere inside a layer's bounds.
- **Selections:** rectangle, ellipse, freehand/polygonal lasso, contiguous/global magic wand, add/subtract/intersect, inverse, feather, outline movement, and moving or duplicating selected pixels.
- **Select menu:** **Layer's Pixels** selects the active layer's opacity (ignoring its mask), **Mask's Black Areas** selects what the active layer's mask hides (grey is partly selected), and **Expand…** / **Contract…** grow or shrink the selection by 1–500 pixels with rounded corners. Contract also shrinks away from the canvas edges, as in Compositor. Soft (feathered) selections keep their soft edge; each is one undo step and runs in the background, so a large amount can be cancelled.
- **Select → Color Range…** selects every pixel near colours you click on the canvas, anywhere in the image, as Compositor's does. The panel opens beside the canvas; click the image to pick a colour (the eyedropper's bubble shows it), **Shift**-click to add another and **Alt**-click to take one away, or choose **Pick**, **Add** or **Remove** for plain clicks. **Fuzziness** (0–200) is how far a colour may be from a picked one per channel: within half of it a pixel is fully selected, and the selection fades out linearly up to the full amount, so the edge stays soft. **Invert** selects everything else, such as all but a green screen. The selection and a black-and-white preview update as you go; **OK** keeps it as one undo step and **Cancel** puts back the selection you had. Colours are matched against the image as shown when the panel opened.
- **Select → Subject** (**Ctrl+Alt+A**) selects the main subject of the image as shown, and **Filter → Remove Background** hides the background of the active layer with a layer mask (kept together with any mask it already has; paint the mask to fix it up). Both use the same classical segmentation, with no machine learning: GrabCut, a graph cut that learns colour models of the subject and the background from a border-seeded start and refines them five times, run on a copy at most 512 pixels on its longer side, then scaled back up with a guided filter that follows the image's own edges and gives a soft, anti-aliased edge. It works best when the subject stands out in colour from its surroundings and lies away from the image's edges; a subject that runs off the image (such as a portrait's shoulders) still reaches the edge. Both run in the background with a progress bar and can be cancelled. A plugin can replace the built-in algorithm with a machine-learning model; see [Selection providers](#selection-providers).
- **Magic tool, Object mode:** choose **Object** next to **Wand** in the Magic Wand's options (as Compositor's Magic tool offers Wand and Object). Click an object to select it, or drag a box around it; **Shift** adds to the selection and **Alt** subtracts. It runs the same graph cut as Select Subject on the image as shown: a click marks that spot as certainly the object and keeps only the part connected to it, and a box marks everything outside it as background.
- **Filter → Remove Flat Background (edge colors)** is the earlier matte: it removes the colours connected to the image's edges, within the Magic Wand's **Tolerance**. It works at full resolution, so it keeps hairline detail (logos, line art, product shots on white) that the graph cut's reduced copy can lose, but it needs a flat, even background.
- **Paint:** brush, pencil, eraser, aligned/unaligned clone stamp with layer/all-layer sampling, spot healing, blur/smudge, gradients, rectangles, rounded rectangles, ellipses, and eyedropper. Live shapes redraw at the new size until their pixels are edited.
- **Eyedropper (I):** a bubble beside the pointer shows the colour under it (top half) over the current brush colour (bottom half), and flips away from the canvas edges. Press and drag to sample continuously; the brush colour updates live and the release keeps it. **Esc** while dragging restores the colour from before the press. The tool options choose the source (current layer or all visible layers) and the sample size (single pixel, 3×3 or 5×5 average). Documents with filter layers are rendered once per edit and reused, so dragging stays smooth.
- **Text:** editable multiline text layers, searchable installed font families, size and color, bold, italic, underline, and strikethrough. Click with the Text tool (T) to place or edit text; double-click a text layer to reopen its live preview. Move and transform text with the Move tool.
- **Adjustments:** editable Hue/Saturation color ranges, per-channel Levels and Curves, Exposure, Gradient Map, Grain, Black & White (Photoshop's per-color-family weights, defaulting to reds 40%, yellows 60%, greens 40%, cyans 60%, blues 20% and magentas 80%, with an optional tint), Color Balance (cyan–red, magenta–green and yellow–blue shifts for shadows, midtones and highlights, with Preserve Luminosity), and Invert. Apply directly or add an adjustment layer, with live preview and selection coverage.
- **Filters:** Gaussian and Motion Blur with expanded bounds, Add Noise, Lens Correction, content-aware fill, **Remove Background** and **Remove Flat Background** (see below). Filtering and expensive retouching run in cancellable workers.
- **RAW Develop:** Nikon NEF/NRW, Canon CR2/CR3/CRW, Fujifilm RAF, and Sony ARW open in a dedicated Develop workspace. Adjust white balance, exposure, tone curves, HSL, monochrome and split toning, noise reduction, sharpening, manual lens correction, crop, and brush/gradient masks. Compare before/after and inspect clipping or full-resolution detail. Develop creates an embedded RAW layer; double-click it to edit the original RAW again. Save `.xuan` to retain the source and adjustments, or export a 16-bit sRGB TIFF directly from Develop. See [RAW workflow and limits](RAW.md).
- **Documents:** independent tab histories, crop, canvas/image size, high-quality downsampling, pixel grid, rulers, guides and a layout grid, pasting copied images or image files as layers, Copy Merged, and save-on-close prompts. Undo retains up to 64 steps with a 512 MiB asset budget, keeping at least one step.

Double-click a layer name to rename it inline. Press Enter or click elsewhere to
save, or Escape to cancel. **Rename…** in the layer's context menu opens the same
inline editor. Double-click elsewhere on a text, RAW, filter, or adjustment row
to reopen its settings.

See [keyboard shortcuts](SHORTCUTS.md) for tool and command bindings.

### Selection providers

Select Subject, Remove Background and Object mode use Xuan's built-in classical segmentation by default. Under **Settings → Selection**, each can instead use a plugin that provides it (for example one that runs a segmentation model; see "Providers" in [PLUGINS.md](PLUGINS.md#providers)). The command then runs that plugin, under its usual permissions and prompts, and shows its result as a proposal to **Accept** or **Discard**. If the chosen plugin is disabled, missing, or uses the network while **Disable plugins that use the network** is on, the built-in algorithm runs instead and the status bar says why.

### Rulers, guides and grid

**View → Rulers** (Ctrl+R) shows rulers along the top and left of the canvas,
measured in document pixels from the image's top-left corner. Numbered ticks are
roughly 70 points apart at any zoom, in steps of 1, 2, 5, 10, 20, 25, 50, 100 pixels
and so on, with ten small ticks between them.

Drag from the top ruler to create a horizontal guide, or from the left ruler for a
vertical one. With the Move tool, drag a guide to move it (the pointer changes over
it; transform handles take priority), or drag it back onto a ruler to delete it.
Escape cancels the drag. Guides are cyan lines across the whole view, including the
area around the canvas. **View → Show → Guides** (Ctrl+;) hides and shows them,
**View → Lock Guides** (Ctrl+Alt+;) stops them from being created or moved, and
**View → Clear Guides** removes them all. Creating, moving, deleting and clearing
guides are undo steps, and guides follow the canvas through Crop, Canvas Size,
Image Size and Flip Canvas. Guides are saved in `.xuan` projects.

**View → Show → Grid** (Ctrl+') draws a non-printing layout grid over the
document: a major line every 64 pixels split into eight subdivisions by default.
**View → Grid Settings…** sets the color (Light Gray by default, eight other presets
or a custom color from the swatch), the style of the major lines (lines, dashed
lines or dots), their opacity (45% by default; subdivisions are fainter), the pixels
between gridlines (2–4096) and the subdivisions (1–64, no finer than a pixel). The
grid shows while the dialog is open and changes preview live; **Cancel** puts the
previous grid back and **Restore Defaults** returns to 64 pixels, eight
subdivisions, Light Gray lines at 45%. **OK** saves the grid in the current project
(an undo step) and makes it the default for projects without a grid of their own.
Subdivisions closer than 4 points on screen are left out. The layout grid is
separate from the pixel grid: both can be on, and the layout grid is drawn on top,
on the same physical screen pixels, so where both mark a pixel boundary they share
one line.

**View → Snap** (Ctrl+Shift+;) turns snapping on and off, and **View → Snap To**
chooses the targets: **Guides**, **Grid**, **Layers** (the edges and centres of
other visible layers) and **Document Bounds** (the canvas edges and centre). All but
the grid are on by default; hidden guides or a hidden grid never snap. Snapping
applies when moving layers and selected pixels, dragging resize handles (of an
unrotated layer), drawing marquees, shapes and crops (their start and dragged
corner), moving a selection outline, and dragging guides. A target pulls when it is
within 10 screen points, whatever the zoom; the nearest one wins, and a guide wins
a tie over the canvas, layers and grid. A magenta line marks what the drag snapped
to. Hold Ctrl while dragging to move freely.

The rulers, grid and guide visibility, Lock Guides, the snap settings and the
default grid are app preferences, saved in the configuration file.

### Pencil

The Pencil paints hard-edged, non-antialiased pixels. Each dab covers exactly the
pixels whose centres fall inside the tip at full coverage times the opacity, so
there is no feathering and no partial-alpha edge. Choose a **Round** or **Square**
tip in the contextual header; size is in whole pixels. Size 1 paints a single pixel.
Odd sizes are centred on the pixel under the pointer and even sizes on the nearest
pixel corner. Strokes are drawn as Bresenham lines between pointer samples, so they
have no gaps, and a pixel is applied at most once per stroke, so overlapping dabs
do not darken at opacity below 100%. Shift-click draws a straight line from the last
point. Pressure scales the size in whole pixels. The Pencil works on layer pixels,
layer masks and mask layers and respects selections, locked layers and undo. Press
Shift+B to switch between the Brush and the Pencil. It pairs well with the pixel grid
shown at high zoom.

### Brush stroke smoothing

Select a painting tool and increase **Smoothing** in its toolbar to reduce small
shakes in mouse or pen strokes. **0%** (the default) turns smoothing off; higher
values produce steadier curves with more distance between the pointer and the
painted tip. The brush outline follows the painted tip, and the smoothing distance
stays consistent on screen when you zoom. Release the mouse or lift the pen to
finish the stroke at its final input position, as a single undo step.

Smoothing applies to the brush, eraser, clone stamp, blur/smudge, spot healing,
and painting on layer masks. Pressure and tilt continue to control the brush.
Shift-click straight lines bypass smoothing. RAW Develop's mask brushes are
unchanged. **Hardness** controls edge softness independently of stroke smoothing.

### Drawing tablets

Wacom, Parblo, and other tablets supported by your system's driver can draw and
operate the interface with the pen. Linux uses native Wayland tablet-v2 or
XInput2 on X11/XWayland. Windows uses Windows Ink; enable **Windows Ink** in the
tablet driver's settings for Xuan. A driver that supplies only mouse events
still works as a mouse, without pressure, tilt, or eraser identification.

Pressure controls brush size by default. Open **Pen dynamics** in the painting
toolbar to control these independently:

- **Pressure: size** scales the selected brush size with pen pressure.
- **Pressure: opacity** scales the selected opacity with pen pressure.
- **Tilt: shape** flattens and rotates the brush footprint with the pen's tilt.
  The cursor outline previews the footprint. This option starts disabled.

These controls apply to raster painting, erasing, clone stamp, blur/smudge,
healing, and layer masks. Missing pressure data uses the selected size and
opacity; missing tilt data gives a circular brush. Flipping a pen with an eraser
tip temporarily erases without changing the selected tool. Hovering does not
paint, and lifting the tip ends the stroke as one undo step. Fast strokes retain
intermediate pen samples and interpolate their size, opacity, and tilt.

Pen buttons use the platform's secondary and middle pointer actions;
middle-drag pans the canvas. Configure express keys and touch rings as keyboard
shortcuts in your driver or compositor; Xuan uses its normal shortcut bindings.
There is no separate tablet-button mapping editor. RAW Develop's local adjustment
brushes retain their existing fixed-size behavior.

No root access or direct access to `/dev/input` is needed. The tablet must first
work in the desktop session. When reporting a problem, include the tablet model,
driver, desktop/compositor, and whether Xuan is using Wayland, X11, or Windows Ink.

### Attached image effects

An image can contain multiple masks, filters, and adjustment layers. Use its
chevron to expand or collapse the attached layers. Each child affects only that
image; effects run from the bottom child upward. Drag children to reorder them,
use their eye icons to bypass them, and double-click a filter or adjustment to
edit its settings. The original image pixels remain unchanged.

**Layer → New Adjustment Layer** and **Layer → New Filter Layer** create standalone
layers that affect the stack below. Drag an effect onto the middle of an image
row to attach it. Drag it beside an outside row or use **Move Out of Parent** to
detach it. The existing **Filter** menu still applies raster edits directly.

**Add Layer Mask** adds a new mask child to the selected image (or the image of
the selected child); it can be used repeatedly. **New Mask Layer** creates a
standalone mask. Select a mask child to paint it, and use **Link / Unlink** to
control whether it follows its image's transforms. Older single image masks are
shown as children when a project is opened. Save as `.xuan` to preserve the stack.

## Files and export

Use **File → Open Compositor Package…** to import an original `.comp` folder package (format versions 1–11, as written by Compositor up to 1.4.5). Save it as `.xuan` to keep editing in Xuan. Folders, opacity, masks, clipping, adjustment layers, guides, live shapes and editable text come across. Parts Xuan cannot show yet are left out, and a summary lists them after opening. Photoshop blend modes, layer effects, and Black & White and Color Balance adjustment layers come across unchanged. Gaussian Blur, Motion Blur and Add Noise adjustments become filter layers. Text keeps its content, font, size and color; its alignment, spacing, paragraph box and per-letter colors or fonts are not represented, so the original rendering stays until you edit the text. Line shapes and very large text arrive as plain pixels. See [FORMAT.md](FORMAT.md#importing-compositor-packages) for the details. Image export supports PNG, JPEG, TIFF, and WebP; JPEG has a quality preview and PNG/JPEG carry print resolution.

Photoshop files (`.psd` and `.psb`, 8-bit RGB) open with **File → Open**, import into the open document with **Import as Layer** (inside a folder named after the file), or can be dragged onto the window. CMYK, Lab, Grayscale, Indexed and 16- or 32-bit files are refused; convert them to RGB Color, 8 Bits/Channel in Photoshop first. Folders, layer masks, opacity, fill opacity, visibility, clipping and all of Photoshop's blend modes stay editable, as do Levels, Curves, Exposure, Invert, Black & White and Color Balance adjustment layers, strokes, shadows, glows and color overlays, solid-filled rectangle and ellipse shapes, and simple horizontal text (which keeps Photoshop's rendering until you edit it). Other vector shapes, smart objects, fill layers and vertical or warped text come in as pixels; bevels, satin, gradient and pattern overlays and other adjustment layers are left out. When anything changes, a list of the conversions appears first and nothing is applied until you choose **Import** (Enter); **Cancel** (Esc) leaves your documents as they were. Files too large for the 100-megapixel budget have their layers cropped to the canvas. See [FORMAT.md](FORMAT.md#importing-photoshop-files) for the full mapping and limits.

HEIC/HEIF photos (`.heic`, `.heif`, and `.hif`, including uppercase extensions) open directly on Linux and Windows using the bundled decoder. Use File → Open, import as a layer, or drag a photo into the editor. The primary still image is imported, including tiled images and container rotation/mirroring; sequences and unsupported HEVC coding features report an error. Images use the editor's 8-bit raster pipeline and are limited to 512 MiB per file, 30,000 pixels per side, and 100 megapixels. Saved `.xuan` projects embed the decoded pixels, so the original HEIC file is no longer required. HEIC export is not supported. Nikon NEF/NRW, Canon CR2/CR3/CRW, Fujifilm RAF, and Sony ARW import use the bundled Rawler library. No external converter is required for these formats. See the [project format](FORMAT.md) for details about saved documents.

### Drag and drop

Drag image, Photoshop or RAW files onto the window to add them. With no document open they
open as new documents. With a document open, one prompt covers the whole drop:
**Insert as layer** (default, Enter), **Open as new document**, or **Cancel**
(Esc). `.xuan` projects and project folders always open as new documents without
a prompt; in a mixed drop they open after you answer, even if you cancel. Drops
that arrive while a dialog, background job, error, save prompt, or Develop is
open are queued and handled once it closes.

## Plugins

Plugins add menu actions, sidebar panes and file formats. Install one with
**Plugins → Install from Folder or Zip…** (or drop its folder or `.zip` on
**Plugins → Manage Plugins…**): Xuan checks it, shows what it is and what it
asks for, and copies it only when you press **Install**; installing it again
updates it. You can also place its folder in the plugins directory shown under
**Plugins → Manage Plugins…** (`~/.config/xuan/plugins/` on Linux,
`%APPDATA%\xuan\plugins\` on Windows) and press **Reload**. Every plugin asks
before it first runs. Plugins that need a Python environment or models come
with a setup script for you to run once; Xuan never runs it (see
[Setup convention](PLUGINS.md#setup-convention)). Plugins that declare permissions, such as
network access or an API key, ask for them before they first run. Their
settings are edited in the same window.

A plugin is a program that runs with your rights. Xuan controls what it sends
the plugin and what it does for it, but it cannot stop the plugin itself from
reading your files, or from contacting a server unless it blocks the
plugin's network (below), so install plugins you trust. When
a plugin that says it uses the network is about to receive your image,
regions or text, Xuan names the hosts it declared, lists what will be sent and
waits for **Send** or **Cancel**; **Don't ask again for this plugin** skips
the question until the plugin's folder, command or permissions change, or you
press **Ask Again** in **Plugins → Manage Plugins…**. To keep such plugins
from running at all, turn on **Disable plugins that use the network** in
**Settings → General** or at the bottom of **Plugins → Manage Plugins…**:
their panes show why they are empty and their actions are greyed out.

On Linux, **Block network for plugins that don't declare it**, next to it,
keeps every plugin that declares no network hosts from opening network
connections, `localhost` included; the permission dialog and **Manage
Plugins…** then show **Network blocked by Xuan (Linux)** for it. It is off by
default for now. A plugin that talks to a server on your own machine, such as
ComfyUI or Ollama, has to declare it as a host to keep working. If Xuan cannot
block a plugin's network, it does not start the plugin. See
[Blocking the network](PLUGINS.md#blocking-the-network).

A plugin action opens a dialog built from the inputs it declared. Actions that
work on marked parts of the image switch to the **Region** tool: drag boxes
over the canvas, or add the current selection as a region, and fill in each
box's details. The result arrives as a proposal above the canvas with
**Compare**, **Accept** and **Discard**; accepting is one undo step. Generated
layers remember what produced them, so **Layer → Re-run Plugin Action…** can
repeat the action with changes.

The repository ships examples under `plugins/`: a histogram pane (Python), a
region inverter (Rust) and a Comfy Cloud client that edits or generates images
with ComfyUI workflows. See [plugins](PLUGINS.md) for the manifest, the
protocol and the SDKs.

## Current limits

The photo editor uses an 8-bit sRGB raster pipeline. RAW Develop uses floating-point camera data and offers direct 16-bit TIFF output with an sRGB profile; its photo-layer render uses the existing 8-bit pipeline. Imported raster ICC profiles are not converted or preserved. `.comp` versions 1–11 and 8-bit RGB Photoshop PSD/PSB files can be imported; Xuan writes neither format. Selections and undo history are session state and are not saved in project archives.

Select Subject and Remove Background use a classical graph cut (below) instead of Apple's Vision foreground model, so results differ from Compositor on cluttered photos; a plugin can supply a machine-learning model instead. Content-aware fill uses a portable texture-matching implementation, so its results differ from Compositor. Spot healing follows Compositor's algorithm and offers the same Content-Aware, Create Texture and Proximity Match modes. Initial zoomed-out canvas previews are capped at 4096 pixels per side. At 100% zoom and above, the preview uses full document resolution up to the device's texture limit; the pixel grid (see below) appears when individual document pixels can be displayed. Filter Apply uses full layer dimensions and export uses full document dimensions. Imports, saves, and raster adjustments can temporarily occupy the UI thread. Vulkan is the verified rendering path; OpenGL surface availability depends on the driver.

## Language and settings

Open **Edit → Settings…** (Ctrl+,). The sidebar's **General** category contains the
language selector: **English** or **简体中文**. Changes apply immediately and are
saved automatically. Chinese glyphs are bundled with the application.

The **Appearance** category sets the **Window title bar**:

- **Compact** (the default): the menus share the title bar with monochrome
  minimize, maximize, and close buttons on the right. Under GNOME, the buttons
  follow the desktop's `button-layout` setting for side and order.
- **System**: the desktop draws the title bar, window buttons, and resize borders,
  and the menus sit in a normal bar below it.
- **macOS**: the menus share the title bar with macOS-style buttons on the left.

The title bar changes immediately. Compact and macOS windows have rounded corners,
which need a window created with transparency: after switching from **System**,
corners stay square until Xuan restarts.

The **Pixel grid** settings control the overlay: zoomed in past a threshold (500% by default), Xuan outlines individual image
pixels with a thin, semi-transparent grey grid that stays visible on light and
dark content. It fades in over the first quarter of the threshold above it, so it
does not pop on. Lines follow the document's pixel boundaries at any pan, zoom, and
display scale, and are snapped to physical screen pixels. The grid is only a screen
overlay: it is never part of exports, copies, or saved files. It also appears in the
RAW Develop canvas, where one image pixel is one RAW output pixel, for pixel peeping.

Turn it on or off with **View → Pixel Grid** (on by default). Set the zoom level in
**Settings → Appearance → Show pixel grid above** (200% to 6400%, default 500%).
Both settings are saved. **View → Pixel Grid** has no default shortcut, but you can
assign one under **Keyboard Shortcuts**. A canvas whose preview is
limited by the graphics card's texture size cannot show one texel per pixel, so
the grid is hidden there.

The **Keyboard Shortcuts** category lists every command, tool and plugin action by
menu, with the keys that run it. Search by name or key, click a shortcut and press
new keys (Escape cancels, Backspace removes it), add a second shortcut with **+**, and
restore defaults per command with **Reset** or all at once with **Reset All**. Keys
that another command already uses prompt to **Reassign** or **Cancel**. Single letters
are kept for tools, and Enter, Escape, Space, Tab, the arrows and the number keys
belong to the editor. Menus, tool tips and **Help → Keyboard Shortcuts** (F1, which
also has a **Customize…** button) show the keys in effect. See
[keyboard shortcuts](SHORTCUTS.md) for the defaults.

Quit with **File → Quit** (Ctrl+Q). It asks about unsaved changes and open Develop
sessions, like closing the window.

Preferences are separate from projects and window layout:

- Linux: `$XDG_CONFIG_HOME/xuan/config.toml`, or `~/.config/xuan/config.toml` when
  `XDG_CONFIG_HOME` is unset or not an absolute path.
- Windows: `%APPDATA%\xuan\config.toml`.

The file is created when a preference changes. For example:

```toml
language = "zh-CN" # Use "en" for English (the default).
title_bar = "system" # Or "compact" (the default) or "macos".
rulers = true # View → Rulers (off by default).
show_grid = false # View → Show → Grid.
show_guides = true # View → Show → Guides.
lock_guides = false # View → Lock Guides.
recent_commands = ["merge", "tool_hand"] # Run from the command palette, newest first (up to 10).

[snap] # View → Snap and Snap To.
enabled = true
guides = true
grid = false
layers = true
bounds = true

[grid] # The default layout grid (View → Grid Settings…).
spacing = 64
subdivisions = 8
color = "light_gray" # Or light_blue, light_red, green, medium_blue, yellow, magenta, cyan, black, custom.
custom_color = [179, 179, 179]
style = "lines" # Or "dashed_lines" or "dots".
opacity = 45

[keybindings] # Only the shortcuts you changed (Settings → Keyboard Shortcuts).
merge = "Ctrl+Shift+M"
invert_selection = "" # No shortcut.
```

Missing options keep their defaults, so files from older releases still load.

Invalid or unreadable configuration is reported and the app starts with defaults.
Saving uses an atomic replacement and preserves other TOML options. An invalid
existing TOML file must be corrected before settings can be saved. Operating-system
file dialogs and technical diagnostics may follow the system language or English.
