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

Xuan has charcoal and light themes (Settings → Appearance → Theme), contextual controls above the canvas, a vertical tool rail, document tabs, and a sidebar of panes on the right. The menu bar shares the titlebar with the window controls. Drag the titlebar to move the window, double-click to maximize, or drag an edge to resize.

The sidebar is a stack of panes, each with a header. Click a header to collapse or expand the pane, drag a header up or down to reorder the stack, and drag the line between two panes to resize them; the Layers pane takes whatever space is left. The **Navigator** pane above it shows the whole image with the visible area outlined. The **Window** menu shows or hides each pane, and **Window → Reset Panel Layout** restores the default (Navigator, then Layers, then any plugin panes). The arrangement is saved with your settings. Plugins can add panes of their own (see [plugins](PLUGINS.md)).

### Document tabs

Each open document and RAW Develop session has a tab, in a bar directly above the canvas
(below the tool options). The tabs are styled after KDE's: flat, with a document icon, the
title and a **✕**, thin separators between them, and the selected tab a little lighter than
the bar with an accent line along its top edge. The others get a faint fill under the pointer.
The **+** button after the last tab starts a new canvas. Hover a tab for its full path.

- **Close** a tab with its **✕** (shown on the selected tab and the one under the
  pointer), with the middle mouse button, or with **Ctrl+W**. A tab with unsaved changes
  shows a dot instead of the **✕**, which turns into the **✕** under the pointer; closing
  it asks first.
- **Reorder** document tabs by dragging them; a line shows where the tab will land.
- **Double-click** the empty part of the bar, or click **+**, for a new canvas.
- **New canvas size:** **File → New…** (Ctrl+N) takes a Width, Height and Resolution. The
  **Preset** menu fills Width and Height from **Screens** (4K, 1440p, 1080p, 720p) or **Social**
  (square, portrait and landscape posts, story / reel, video thumbnail, link preview, banner);
  it shows the preset the fields match, in either orientation, or **Custom** once you type
  another size. **Swap** exchanges Width and Height (portrait / landscape), and **Keep aspect
  ratio** makes the other side follow the one you type, in the proportion it had when you ticked
  the box or picked a preset. Sizes obey the same limits as every canvas. The size of the last
  canvas you created is offered next time (paper sizes wait on physical units).
- **Right-click** a tab for **Close Tab**, **Close Other Tabs**, **Close Tabs to the
  Right**, **Reopen Closed Tab**, **Copy Path** and **Show in Folder** (which selects the
  file in the file manager on Windows, macOS and Linux desktops that support it, and
  otherwise opens its folder). Closing several tabs stops at the first one whose
  unsaved-changes prompt you cancel.
- **Keys:** **Ctrl+Tab** / **Ctrl+Page Down** for the next tab, **Ctrl+Shift+Tab** /
  **Ctrl+Page Up** for the previous one, **Alt+1** to **Alt+8** for the first eight tabs and
  **Alt+9** for the last (as in Firefox on Linux; **Ctrl+1** stays Actual Pixels), and
  **Ctrl+Shift+T** to reopen the last closed file. All of them can be changed in Settings →
  Keyboard Shortcuts, where **Close Other Tabs** and **Close Tabs to the Right** can be given
  keys too.
- Tabs shrink as more open, down to a minimum width. Beyond that the bar scrolls: use the
  **‹** / **›** arrows or the mouse wheel over the tabs, or **⌄** for a list of all tabs.

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
- **Layers:** folders, Photoshop's 27 blend modes (in Photoshop's menu order and grouping: Normal and Dissolve; the darkening modes; the lightening modes; the contrast modes; Difference, Exclusion, Subtract and Divide; then Hue, Saturation, Color and Luminosity), opacity (select a folder to set its own opacity, which dims everything inside it on top of each layer's own; a folder's blend mode stays Normal, so its layers still blend with what is below them, as in upstream Compositor), visibility, locks, drag reordering/nesting, duplication, merge, clipping masks (**Layer → Clipping Mask** clips the active layer to the layer or folder below it; a folder clips to all of its visible layers together, including masks inside it and the folder's own opacity and mask, so the clipped layer follows later edits to the folder's layers; the base's opacity applies to the clipped layers, as in Photoshop; ungrouping the folder releases them), linked or independent raster masks, and copying layers to another project tab.
- **Layer effects:** **Layer → Layer Effects…** (also in a layer's right-click menu) adds a stroke (outside or inside), a drop shadow, a color overlay, an inner shadow, an outer glow and an inner glow to a pixel or text layer, with upstream Compositor's defaults. Tick an effect to show it, select it to change its size, angle, distance, blur, color and opacity, or remove it. Changes show on the canvas as you make them; **OK** keeps them as one undo step and **Cancel** puts back what the layer had. Effects are kept with the layer rather than painted into it, so they follow every later edit and can be changed or hidden at any time; the Layers panel marks such layers with “fx”. They follow the layer's mask, and the layer's opacity, folders, blend mode and clipping apply to them as well. Effects are drawn on the GPU when one is available.
- **Transforms:** move, scale, rotate, flip, free perspective distortion, numeric controls, shared transforms for several layers or folders, and snapping to guides, the grid, layers and the canvas (see [Rulers, guides and grid](#rulers-guides-and-grid)). Original source pixels remain available during transforms. Move / Transform has **Ignore Transparent Pixels** checked by default to select only at visible pixels; uncheck it to select and drag anywhere inside a layer's bounds.
- **Selections:** rectangle, ellipse, freehand/polygonal lasso, contiguous/global magic wand, add/subtract/intersect, inverse, feather, outline movement, and moving or duplicating selected pixels.
- **Select menu:** **Layer's Pixels** selects the active layer's opacity (ignoring its mask), **Mask's Black Areas** selects what the active layer's mask hides (grey is partly selected), and **Expand…** / **Contract…** grow or shrink the selection by 1–500 pixels with rounded corners. Contract also shrinks away from the canvas edges, as in Compositor. Soft (feathered) selections keep their soft edge; each is one undo step and runs in the background, so a large amount can be cancelled.
- **Edit → Stroke…** draws a line of colour along the selection's edge on the active pixel layer, as Photoshop's Stroke does: **Width** 1–250 pixels, **Colour** (the foreground colour to start with), **Location** **Inside**, **Centre** (half each side; an odd width puts the extra pixel outside) or **Outside** the edge, **Opacity**, and **Preserve transparency** to paint only over what the layer already shows and keep its alpha. The line reaches as Expand and Contract do, so outside corners round off and a 10 px Outside stroke on a rectangle adds exactly 10 pixels beyond each side; its side on the selection's edge follows that edge exactly, so it meets a fill of the same selection without a seam, and its far side is antialiased, smooth on ellipses and lassos. A feathered selection gives a soft line, and the canvas edges count as an edge (an Inside stroke of Select All borders the canvas). A moved, scaled or rotated layer takes the line where it shows on the canvas and grows to hold it. The line shows on the canvas as the settings change; **Apply** keeps it as one undo step and **Cancel** puts the layer back. The dialog remembers its settings (not the colour) for next time. It needs a selection and an unlocked pixel layer (not text, a shape, RAW, a folder or an adjustment), with the layer's pixels rather than its mask being edited; the command palette finds it as **Stroke…**.
- **Image menu canvas commands:** **Rotate Canvas** turns the whole document 90° clockwise, 90° counter-clockwise or 180°. Layers, masks, guides, paths and the selection turn with it and pixels are not resampled, so text and shapes stay editable (a layer effect's light angle stays as it was, as with Photoshop's global light). **Crop to Selection** crops the canvas to the selection's bounds and is greyed out without a selection. **Trim…** crops away margins that are transparent or the colour of the top-left or bottom-right pixel of the visible image, on the sides you tick. Each is one undo step, and none discards the pixels outside the new canvas. Plugins and the MCP server have `rotate_canvas`, `trim` and (as the `crop_to_selection` command) the same.
- **Select → Paths…** keeps vector paths with the document, as Photoshop's Paths panel does (saved in `.xuan` format 10; they follow Crop, Trim, Canvas Size, Image Size, Flip Canvas and Rotate Canvas). Paste SVG path data (`M 0 70 C 12 64 38 64 51 70 Z`: M, L, H, V, C, S, Q, T, A and Z, lowercase for relative) and **Add Path**; select a path to rename or delete it, or to run an action on it, each one undo step: **Fill Path** fills its inside with the foreground colour on the active layer (within the selection, if there is one), **Stroke Path** paints each subpath with the current brush, its size, hardness, opacity and dynamics included (taper the ends with the brush's taper settings), or with the Pencil's hard pixels when **Stroke with** is set to **Pencil**, **Make Selection** selects its inside with antialiased edges, softened by **Feather** and combined by **New**, **Add**, **Subtract** or **Intersect**, and **Shape Layer** makes an editable vector shape layer in the foreground colour that is redrawn from its outline when resized. **Even-odd fill** makes inner subpaths holes. A malformed path says what was expected and at which character. **Make Path from Selection** adds the outline of the selection (where it is at least half selected) as a new path of straight segments, within a pixel of the selection's pixel edges; holes stay holes. Paths come from plugins and the MCP server too (`save_path`), and are drawn with the [Pen tool](#pen-tool). The selected path is shown on the canvas; **Edit with Pen** closes the dialog and picks the Pen to edit it.
- **Select → Color Range…** selects every pixel near colours you click on the canvas, anywhere in the image, as Compositor's does. The panel opens beside the canvas; click the image to pick a colour (the eyedropper's bubble shows it), **Shift**-click to add another and **Alt**-click to take one away, or choose **Pick**, **Add** or **Remove** for plain clicks. **Fuzziness** (0–200) is how far a colour may be from a picked one per channel: within half of it a pixel is fully selected, and the selection fades out linearly up to the full amount, so the edge stays soft. **Invert** selects everything else, such as all but a green screen. The selection and a black-and-white preview update as you go; **OK** keeps it as one undo step and **Cancel** puts back the selection you had. Colours are matched against the image as shown when the panel opened.
- **Select → Subject** (**Ctrl+Alt+A**) selects the main subject of the image as shown, and **Filter → Remove Background** hides the background of the active layer with a layer mask (kept together with any mask it already has; paint the mask to fix it up). Both use the same classical segmentation, with no machine learning: GrabCut, a graph cut that learns colour models of the subject and the background from a border-seeded start and refines them five times, run on a copy at most 512 pixels on its longer side, then scaled back up with a guided filter that follows the image's own edges and gives a soft, anti-aliased edge. It works best when the subject stands out in colour from its surroundings and lies away from the image's edges; a subject that runs off the image (such as a portrait's shoulders) still reaches the edge. Both run in the background with a progress bar and can be cancelled. A plugin can replace the built-in algorithm with a machine-learning model; see [Selection providers](#selection-providers).
- **Magic tool, Object mode:** choose **Object** next to **Wand** in the Magic Wand's options (as Compositor's Magic tool offers Wand and Object). Click an object to select it, or drag a box around it; **Shift** adds to the selection and **Alt** subtracts. It runs the same graph cut as Select Subject on the image as shown: a click marks that spot as certainly the object and keeps only the part connected to it, and a box marks everything outside it as background.
- **Filter → Remove Flat Background (edge colors)** is the earlier matte: it removes the colours connected to the image's edges, within the Magic Wand's **Tolerance**. It works at full resolution, so it keeps hairline detail (logos, line art, product shots on white) that the graph cut's reduced copy can lose, but it needs a flat, even background.
- **Paint:** brush, pencil, eraser, aligned/unaligned clone stamp with layer/all-layer sampling, spot healing, blur/smudge, gradients, paint bucket, rectangles, rounded rectangles, ellipses, vector path shapes (from **Select → Paths…**, plugins or MCP; edit their outlines with the [Pen tool](#pen-tool)), and eyedropper. Live shapes redraw at the new size until their pixels are edited.
- **Paint Bucket (Shift+G):** shares **G** with the Gradient, as in Photoshop: **Shift+G** switches between them and **G** picks whichever you used last. A click fills the area around the clicked pixel with the foreground colour, in one undo step. The area is the Magic Wand's: pixels whose colour is within **Tolerance** (0–255 per channel) of the clicked one, only those connected to it with **Contiguous** ticked, or every such pixel on the layer without it. **Anti-alias** also partly fills the pixels just past the area's edge, by how close their colour is, so a fill inside anti-aliased line art reaches the lines without leaving a light ring. **Sample** compares colours on the **Current Layer** or on **All Layers** as shown, which lets you colour line art on a layer of its own from an empty layer above it. **Opacity** (also the number keys) blends the fill. The fill stays inside the selection, refuses locked layers, and paints the mask when the mask is the target (comparing the mask's greys).
- **Eyedropper (I):** a bubble beside the pointer shows the colour under it (top half) over the current brush colour (bottom half), and flips away from the canvas edges. Press and drag to sample continuously; the brush colour updates live and the release keeps it. **Esc** while dragging restores the colour from before the press. The tool options choose the source (current layer or all visible layers) and the sample size (single pixel, 3×3 or 5×5 average). Documents with filter layers are rendered once per edit and reused, so dragging stays smooth.
- **Text:** editable multiline text layers, searchable installed font families, size and color, bold, italic, underline, and strikethrough. Click with the Text tool (T) to place or edit text; double-click a text layer to reopen its live preview. Move and transform text with the Move tool.
- **Text on a path:** in the Text window, **Path** sets the text along one of the document's paths (from **Select → Paths…**), and **None (text box)** puts it back in a box. Each letter moves to its distance along the path and turns to follow it. **Start** (a percentage of the path's length) and the Start / Center / End alignment place the line, **Letter spacing** and **Baseline shift** space it, **Flip to the other side** runs it under the path the other way, **Turn letters with the path** can be unticked for upright letters, and **End size** and **Opacity** ramp the size and opacity from the first letter to the last (17 px shrinking to 7 px, or 95% fading to 55%). Letters past the end of an open path are hidden; on a closed path the text wraps around. The path belongs to the layer from then on: moving, scaling or rotating the layer takes it along, and changing the document path later does not. Text on a path is edited in the Text window, as all text is; the canvas shows it along its path. Saved in `.xuan` format 11. Plugins and the MCP server can set text on a path too (`create_text_layer` and `set_layer` with `path`).
- **Adjustments:** editable Hue/Saturation color ranges, per-channel Levels and Curves, Exposure, Gradient Map, Grain, Black & White (Photoshop's per-color-family weights, defaulting to reds 40%, yellows 60%, greens 40%, cyans 60%, blues 20% and magentas 80%, with an optional tint), Color Balance (cyan–red, magenta–green and yellow–blue shifts for shadows, midtones and highlights, with Preserve Luminosity), and Invert. Apply directly or add an adjustment layer, with live preview and selection coverage.
- **Filters:** Gaussian and Motion Blur with expanded bounds, Add Noise, Lens Correction, content-aware fill, **Remove Background** and **Remove Flat Background** (see below). Filtering and expensive retouching run in cancellable workers.
- **RAW Develop:** Nikon NEF/NRW, Canon CR2/CR3/CRW, Fujifilm RAF, and Sony ARW open in a dedicated Develop workspace. Adjust white balance, exposure, tone curves, HSL, monochrome and split toning, noise reduction, sharpening, manual lens correction, crop, and brush/gradient masks. Compare before/after and inspect clipping or full-resolution detail. Develop creates an embedded RAW layer; double-click it to edit the original RAW again. Save `.xuan` to retain the source and adjustments, or export a 16-bit sRGB TIFF directly from Develop. See [RAW workflow and limits](RAW.md).
- **Documents:** independent tab histories, crop, canvas/image size, high-quality downsampling, pixel grid, rulers, guides and a layout grid, pasting copied images or image files as layers, copying and pasting whole layers between documents, Copy Merged, and save-on-close prompts. Undo retains up to 64 steps within a memory budget that grows with the computer's memory (see [Size limits](#size-limits)), keeping at least one step.

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
guides are undo steps, and guides follow the canvas through Crop, Trim, Canvas Size,
Image Size, Flip Canvas and Rotate Canvas. Guides are saved in `.xuan` projects.

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

### Crop tool

The Crop tool (**C**) crops the canvas to a box you draw and adjust before applying it.

- **Ratio:** the options bar's **Ratio** menu keeps the box **Free** (any shape; hold Shift for a
  square), at the canvas's **Original** proportions, at **1:1**, **4:3**, **3:4**, **3:2**,
  **2:3**, **16:9**, **9:16**, **9:20**, **5:4** or **4:5**, or at a **Custom** W : H typed beside
  the menu. **Swap** exchanges the ratio's width and height (16:9 becomes 9:16) and turns the box
  with it. A ratio is kept to the pixel: a 9:16 box is always 9k × 16k pixels, so it grows in
  whole steps of the ratio (an odd canvas's own ratio, such as 1001:997, is too fine for that, and
  its sides round to the nearest pixel instead). The ratio stays chosen while Xuan runs, and
  choosing another refits the box inside the one you have.
- **Drawing and adjusting:** drag to draw the box; its size in pixels shows below it and in the
  options bar. Then drag a corner or edge handle to resize it, keeping the ratio (an edge grows
  the other sides about the box's middle), or drag inside it, or use the arrow keys (Shift for 10
  pixels), to move it. The box stays on the canvas, and the canvas outside it is dimmed, with the
  rule of thirds shown while you adjust it. Dragging outside the box draws a new one.
- **From a selection:** picking the Crop tool (pressing **C**) with a selection starts the box at
  the selection's bounds, or at the largest box of the ratio inside them.
- **Apply or cancel:** **Apply** in the options bar, or **Enter**, crops the canvas to the box as
  one undo step, like **Image → Crop to Selection**, and nothing outside the new canvas is
  discarded from the layers. **Cancel**, **Escape** or choosing another tool drops the box.

### Pen tool

The Pen (**P**) draws Bézier paths, as Photoshop's Pen does, and edits them.

- **Drawing:** click to add a corner anchor; press and drag to add a smooth anchor whose handles stay symmetric, the out handle under the pointer. A line (or, after a smooth anchor, a curve) follows the pointer from the last anchor. Click the first anchor to close the path. **Enter** or **Escape** finishes an open path, and so does choosing another tool; Delete or Backspace removes the last anchor while drawing. A finished path is added to the document's paths as **Path 1**, **Path 2**… in one undo step, listed in **Select → Paths…**, and shown for editing.
- **Editing:** the Pen also edits the path it shows, so there is no separate Direct Selection tool. Click any document path's outline to show it (or select it in **Select → Paths…**); **Escape** hides it. Drag an anchor to move it with its handles, and drag a handle to bend the curve: on a smooth anchor the other handle turns to stay opposite, keeping its length, and **Alt**-drag moves one handle alone, making the anchor a corner. Click a segment to add an anchor there (the curve keeps its exact shape; drag instead of clicking to move the new anchor at once), and click an anchor to delete it. **Alt**-click an anchor to turn a smooth anchor into a corner, or a corner into a smooth anchor with handles along its neighbours; **Alt**-drag from an anchor pulls out new symmetric handles. Delete or Backspace removes the anchor last added, moved or converted. Each finished drag or click is one undo step.
- **Path shape layers:** when the active layer is a path shape layer, the Pen shows and edits its outline the same way. The outline stays in the layer's own coordinates; the layer grows or shrinks to the outline's new bounds, keeping its rotation and flips, and is redrawn. A warped shape layer's outline cannot be edited.

Paths are drawn over the canvas in screen space, so their lines stay 1 pixel wide at any zoom.

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

### Brush flow

The Brush and Eraser have **Flow** (1–100%, 100% by default) next to
**Opacity**, as in Photoshop. Flow is how much paint one pass lays down;
**Opacity** is the most a stroke can reach. At Flow 20% one pass paints about a
fifth of the opacity, and going back over the same spot without lifting builds
the paint up toward the opacity, never past it. A new stroke starts building
again on top. At 100% the whole opacity goes down at once and overlaps within
a stroke don't darken, as before. Flow works the same when erasing and when
painting a layer mask.

Below 100% the stroke is painted as close dabs (10% of the size apart, or the
**Spacing** from **Brush dynamics**), each laying down a share of the flow, so
one pass lays down the same amount whatever the spacing and however fast the
pointer moves. **Pressure: flow** in **Pen dynamics** scales the flow with
pen pressure. The Pencil and the retouching tools ignore flow. Flow stays set
while Xuan runs, like the size and opacity. Plugins and MCP clients set it per
stroke (`flow`).

### Brush dynamics

The Brush, Pencil and Eraser have a **Brush dynamics** menu in their toolbar.
Everything in it is off by default.

- **Spacing** paints separate dabs this far apart, as a percentage of the size;
  0% paints a continuous stroke. 150% makes a dotted line.
- **Taper in** and **Taper out** grow the stroke from nothing over that many
  pixels at its start, and shrink it at its end, without a tablet. **Taper:
  size** and **Taper: opacity** choose what the taper changes. The end taper is
  drawn when you release the mouse or lift the pen, once the stroke's length
  is known.
- **Scatter** moves each dab randomly off the stroke, up to that percentage of
  the size, and **Count** paints that many dabs at each step.
- **Size jitter**, **Opacity jitter** and **Hue jitter** vary each dab
  randomly: up to that much smaller, more transparent, or turned around the
  colour wheel (100% reaches the opposite hue).

Scatter, a count above 1 or a jitter without spacing paint dabs at 25%. Every
stroke gets a new random pattern. Dabs do not darken where they overlap within
one stroke, as with a continuous stroke. **Reset** turns the dynamics off.
Brush dynamics stay set until you change them while Xuan runs, like the size
and hardness. Plugins and MCP clients set them per stroke (see
[PLUGINS.md](PLUGINS.md)).

### Paint symmetry

The Brush, Pencil and Eraser also have a **Symmetry** menu, which paints every
stroke again as you draw it:

- **Vertical** mirrors the stroke left and right across a vertical axis.
- **Horizontal** mirrors it top and bottom across a horizontal axis.
- **Radial** turns it around the centre into **Segments** copies (2–32), for
  mandalas, starbursts and snowflakes.
- **Off** paints one stroke.

The axis goes through the centre, which is the middle of the canvas until you
type a **Centre X** and **Centre Y** in pixels; **Centre on canvas** puts it
back in the middle. While a Brush, Pencil or Eraser is selected and symmetry is
on, the axis or the radial spokes are drawn as dashed lines over the canvas,
with a circle at the centre.

Each copy is the same stroke mirrored or turned, with the same spacing, taper,
scatter and jitter, so the result stays symmetric. All copies are one stroke:
where they cross they don't darken each other, and one **Undo** removes them
all. Other tools ignore symmetry. Like the brush dynamics, symmetry stays set
while Xuan runs. Plugins and MCP clients set it per stroke.

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
- **Pressure: flow** (Brush and Eraser) scales the selected flow with pen
  pressure, so pressing harder builds up paint faster.
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

Photoshop files (`.psd` and `.psb`, 8-bit RGB) open with **File → Open**, import into the open document with **Import as Layer** (inside a folder named after the file), or can be dragged onto the window. CMYK, Lab, Grayscale, Indexed and 16- or 32-bit files are refused; convert them to RGB Color, 8 Bits/Channel in Photoshop first. Folders, layer masks, opacity, fill opacity, visibility, clipping and all of Photoshop's blend modes stay editable, as do Levels, Curves, Exposure, Invert, Black & White and Color Balance adjustment layers, strokes, shadows, glows and color overlays, solid-filled rectangle and ellipse shapes, and simple horizontal text (which keeps Photoshop's rendering until you edit it). Other vector shapes, smart objects, fill layers and vertical or warped text come in as pixels; bevels, satin, gradient and pattern overlays and other adjustment layers are left out. When anything changes, a list of the conversions appears first and nothing is applied until you choose **Import** (Enter); **Cancel** (Esc) leaves your documents as they were. Files whose layers don't fit the [size limits](#size-limits) have their layers cropped to the canvas. See [FORMAT.md](FORMAT.md#importing-photoshop-files) for the full mapping and limits.

HEIC/HEIF photos (`.heic`, `.heif`, and `.hif`, including uppercase extensions) open directly on Linux and Windows using the bundled decoder. Use File → Open, import as a layer, or drag a photo into the editor. The primary still image is imported, including tiled images and container rotation/mirroring; sequences and unsupported HEVC coding features report an error. Images use the editor's 8-bit raster pipeline and follow the [size limits](#size-limits). Saved `.xuan` projects embed the decoded pixels, so the original HEIC file is no longer required. HEIC export is not supported. Nikon NEF/NRW, Canon CR2/CR3/CRW, Fujifilm RAF, and Sony ARW import use the bundled Rawler library. No external converter is required for these formats. See the [project format](FORMAT.md) for details about saved documents.

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

Some plugins edit for someone else, such as the **MCP Server**, which lets an
AI agent like Claude Code work on your image (see the
[agents guide](AGENTS-GUIDE.md)). Before such a plugin's first edit in a
session, Xuan asks **Allow … to edit your documents for this session?**:
**Allow** or **Deny** hold until the plugin stops or starts a new session,
and **Always Allow** turns on auto mode, which **Edit without asking (auto
mode)** in **Plugins → Manage Plugins…** turns off again. Each edit is still
one undo step. A plugin can never save or open files by itself: saving or
exporting for it opens the usual save dialog, titled with the plugin's name,
where you choose the place, and opening a file it names asks first, showing
the file's path.

Plugin actions run in the background while you keep working, and you can
start another one, even on the same document. Running jobs are shown at the
right of the status bar with their progress and **Cancel**; with several, a
count such as **1 of 2** lists them all. Their results arrive one at a time as
proposals to **Accept** or **Discard**. Messages from plugins and from Xuan
appear in the same place for a few seconds.

Plugins that generate images can also offer them where you work, not only in
their menus: the sparkles button next to New Layer (**New layer with AI**), the
**AI Region** tool in the toolbox (draw a box, say what to do there: edit,
add or replace), and a **Generate** tab in **File → New…**, where **Exact
size** makes the canvas exactly the size typed, with the image as a layer you
can move to reframe it. Each shows just a prompt and **Generate**; the model,
quality and other options are under **Advanced**.

The repository ships examples under `plugins/`: a histogram pane (Python), a
region inverter (Rust) and a Comfy Cloud client that generates and edits images
and layers with Comfy's own workflow templates. See [plugins](PLUGINS.md) for
the manifest, the protocol and the SDKs.

## Current limits

The photo editor uses an 8-bit sRGB raster pipeline. RAW Develop uses floating-point camera data and offers direct 16-bit TIFF output with an sRGB profile; its photo-layer render uses the existing 8-bit pipeline. Imported raster ICC profiles are not converted or preserved. `.comp` versions 1–11 and 8-bit RGB Photoshop PSD/PSB files can be imported; Xuan writes neither format. Selections and undo history are session state and are not saved in project archives.

Select Subject and Remove Background use a classical graph cut (below) instead of Apple's Vision foreground model, so results differ from Compositor on cluttered photos; a plugin can supply a machine-learning model instead. Content-aware fill uses a portable texture-matching implementation, so its results differ from Compositor. Spot healing follows Compositor's algorithm and offers the same Content-Aware, Create Texture and Proximity Match modes. Initial zoomed-out canvas previews are capped at 4096 pixels per side. At 100% zoom and above, the preview uses full document resolution up to the device's texture limit; the pixel grid (see below) appears when individual document pixels can be displayed. Filter Apply uses full layer dimensions and export uses full document dimensions. Imports, saves, and raster adjustments can temporarily occupy the UI thread. Vulkan is the verified rendering path; OpenGL surface availability depends on the driver.

### Size limits

Layers are kept in memory, 4 bytes per pixel, so how large a document can be follows the
computer's memory. No limit is ever below 100 megapixels, what Xuan allowed before.

| Limit | Rule | 16 GiB | 32 GiB | 64 GiB | 128 GiB |
| --- | --- | --- | --- | --- | --- |
| One canvas, layer or mask | a pixel per 64 bytes of memory | 268 MP | 536 MP | 1,073 MP | 2,147 MP |
| Layers that opening or importing a file adds (masks have the same again) | a quarter of the memory | 1,073 MP | 2,147 MP | 4,294 MP | 8,589 MP |
| Undo history | an eighth of the memory | 2 GiB | 4 GiB | 8 GiB | 16 GiB |
| RAW files embedded in one project | a thirty-second of the memory | 512 MiB | 1 GiB | 2 GiB | 4 GiB |

- Sides are at most 65,535 pixels, JPEG's own limit. WebP export stores at most 16,383
  pixels a side and TIFF export at most 4 GiB; export PNG for anything larger. Photoshop
  `.psd` files are at most 30,000 pixels a side (Photoshop's rule; `.psb` files can be
  larger), and so are Compositor packages.
- Only opening and importing files, and plugin results, count the total. Editing in Xuan is
  never refused for it, and **Save** never refuses a document for its size. A project made on
  a computer with more memory may be too large to open on one with less; the message says
  what this computer allows.
- An image file is read whole, so it may be up to 8 bytes per pixel of the image limit (a
  16-bit RGBA TIFF) and at least 512 MiB; a Photoshop file up to a quarter of the memory and
  at least 1 GiB.
- The memory is the physical memory, or the control group's limit when that is lower, as in
  a container. To try the limits of another computer, start Xuan with `XUAN_MEMORY` set to
  its memory, for example `XUAN_MEMORY=16G xuan` (bytes, or a K, M, G or T suffix).

## Language and settings

Open **Edit → Settings…** (Ctrl+,). The sidebar's **General** category contains the
language selector: **English** or **简体中文**. Changes apply immediately and are
saved automatically. Chinese glyphs are bundled with the application.

The **Appearance** category's **Theme** chooses the interface colours:

- **System** (the default) follows your desktop's light or dark preference, and
  stays dark when the desktop states none. On Linux, Xuan asks the desktop portal
  (KDE Plasma, GNOME and Flatpak), then KDE's colour scheme, GNOME's
  `color-scheme` setting and the GTK theme name; on Windows, the "Choose your app
  mode" setting. A change on the desktop reaches Xuan within a few seconds.
- **Light mode** and **Dark mode** always use those colours.

Xuan also takes your desktop's accent colour for highlights, selected menu rows,
default buttons and sliders (the portal's or KDE's accent, GNOME 47's accent colour,
or Windows' accent colour), made a little lighter or darker where needed so text on it
stays readable. Without one it uses its own blue. Turn off **Use system accent
colour** (on by default) to always use Xuan's blue. Both settings are saved.

The change applies immediately. Only the interface changes colour: your document,
exports and the colours of selection outlines, guides and handles drawn over it are
the same in both themes.

It also sets the **Window title bar**:

- **Compact** (the default): the menus share the title bar with minimize,
  maximize, and close buttons on the right. Under GNOME the buttons follow the
  desktop's `button-layout` setting for side and order, and under KDE Plasma the
  `ButtonsOnLeft` / `ButtonsOnRight` of `kwinrc`. On macOS the menus sit beside the
  system's own window buttons instead.
- **System**: the desktop draws the title bar, window buttons, and resize borders,
  and the menus sit in a normal bar below it.

On Linux, **Window buttons** (shown for Compact) chooses the artwork:

- **Match desktop theme** (the default) draws the buttons with the images of your
  desktop theme, including their hover, pressed, and unfocused states and the restore
  button, so they match native windows. Xuan looks for them at run time, in this
  order: the decoration images that KDE Plasma's GTK integration (kde-gtk-config)
  writes to `~/.config/gtk-3.0` and `gtk-4.0` (KDE only); the images of the current
  GTK theme (`gtk-theme-name` in `settings.ini`, or GNOME's setting) under
  `~/.local/share/themes`, `~/.themes` and `$XDG_DATA_DIRS`; then the
  `window-close-symbolic`, `window-minimize-symbolic`, `window-maximize-symbolic` and
  `window-restore-symbolic` icons of the current icon theme, coloured like the title
  bar text. The file names are read from the theme's CSS, not assumed. A state a theme
  has no image for is shown with the normal image and a light highlight. A theme's
  own hover and pressed images, including the close button's, are drawn as they are.
  When the close button comes from the icon theme, hovering or pressing it draws a
  circle in your colour scheme's negative (red) colour, read from `kdeglobals`
  (`ForegroundNegative`), as Breeze does. Changing the
  decoration, colour scheme, or icon theme updates the buttons within a few seconds,
  without restarting Xuan. A line under the option says which of these sources is in
  use and which file draws the hovered close button.
- **Built-in** always draws the monochrome buttons that come with Xuan. They are
  also used when the theme provides no usable images, and on Windows.

The title bar changes immediately. Compact windows have rounded corners, which need
a window created with transparency: after switching from **System**, corners stay
square until Xuan restarts. On macOS a title bar change applies after Xuan restarts.
Earlier releases also offered a drawn macOS-style title bar; a configuration file
that still names it (`title_bar = "macos"`) opens with Compact.

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
title_bar = "system" # Or "compact" (the default).
window_buttons = "theme" # Compact buttons: "theme" (the default on Linux) or "builtin".
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
