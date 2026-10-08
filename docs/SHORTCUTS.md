# Keyboard and pointer controls

These are the default shortcuts. Each one can be changed, removed or added to in **Edit → Settings… → Keyboard Shortcuts**; the menus and **Help → Keyboard Shortcuts** (F1) always show the ones in effect. **Ctrl+K** opens the command palette, which finds any command by name and shows its shortcut.

On macOS, **Ctrl** in these tables is ⌘ Command (the Control key also works) and the menus show Mac symbols, such as ⇧⌘S for Save As. Control+Tab still switches tabs, and ⌘H hides Xuan there, so Show Transform Controls has no default shortcut on a Mac.

<!-- BEGIN GENERATED from the command registry (src/app/commands.rs); refresh with XUAN_UPDATE_DOCS=1 cargo test documented_shortcuts -->
| Category | Command | Shortcut |
| --- | --- | --- |
| File | New Canvas… | Ctrl+N |
| File | Open… | Ctrl+O |
| File | Import Image as Layer… | Ctrl+Shift+O |
| File | Save | Ctrl+S |
| File | Save As… | Ctrl+Shift+S |
| File | Export Image… | Ctrl+Alt+Shift+S |
| File | Close Project | Ctrl+W |
| File | Quit | Ctrl+Q |
| Edit | Settings… | Ctrl+, |
| Edit | Undo | Ctrl+Z |
| Edit | Redo | Ctrl+Shift+Z / Ctrl+Y |
| Edit | Cut | Ctrl+X |
| Edit | Copy | Ctrl+C |
| Edit | Copy Merged | Ctrl+Shift+C |
| Edit | Paste | Ctrl+V |
| Edit | Fill Foreground | Alt+Backspace |
| Edit | Fill Background | Ctrl+Backspace |
| Edit | Clear Pixels | Delete / Backspace |
| Edit | Content-Aware Fill | Shift+F5 |
| Edit | Free Transform | Ctrl+T |
| Image | Levels | Ctrl+L |
| Image | Hue/Saturation | Ctrl+U |
| Image | Curves | Ctrl+M |
| Image | Invert | Ctrl+I |
| Layer | New Layer | Ctrl+Shift+N |
| Layer | Duplicate Layers | Ctrl+J |
| Layer | Group Layers | Ctrl+G |
| Layer | Ungroup Layers | Ctrl+Shift+G |
| Layer | Merge Down / Selected | Ctrl+E |
| Layer | Create / Release Clipping Mask | Ctrl+Alt+G |
| Select | Select All | Ctrl+A |
| Select | Deselect | Ctrl+D |
| Select | Inverse Selection | Ctrl+Shift+I |
| Select | Select Subject | Ctrl+Alt+A |
| View | Fit Canvas | Ctrl+0 |
| View | Actual Pixels | Ctrl+1 |
| View | Zoom In | Ctrl+Plus / Ctrl+= |
| View | Zoom Out | Ctrl+Minus |
| View | Show Transform Controls | Ctrl+H |
| View | Show Grid | Ctrl+' |
| View | Show Guides | Ctrl+; |
| View | Rulers | Ctrl+R |
| View | Snap | Ctrl+Shift+; / Ctrl+Shift+: |
| View | Lock Guides | Ctrl+Alt+; |
| Window | Next Tab | Ctrl+Tab / Ctrl+PageDown |
| Window | Previous Tab | Ctrl+Shift+Tab / Ctrl+PageUp |
| Window | Tab 1 | Alt+1 |
| Window | Tab 2 | Alt+2 |
| Window | Tab 3 | Alt+3 |
| Window | Tab 4 | Alt+4 |
| Window | Tab 5 | Alt+5 |
| Window | Tab 6 | Alt+6 |
| Window | Tab 7 | Alt+7 |
| Window | Tab 8 | Alt+8 |
| Window | Last Tab | Alt+9 |
| Window | Reopen Closed Tab | Ctrl+Shift+T |
| Tools | Move / Transform | V |
| Tools | Marquee | M |
| Tools | Switch Rectangle / Ellipse Marquee | Shift+M |
| Tools | Lasso | L |
| Tools | Switch Freehand / Polygonal Lasso | Shift+L |
| Tools | Magic Wand | W |
| Tools | Crop | C |
| Tools | Brush | B |
| Tools | Switch between Brush and Pencil | Shift+B |
| Tools | Eraser | E |
| Tools | Spot Healing | J |
| Tools | Clone Stamp | S |
| Tools | Blur / Smudge | R |
| Tools | Gradient | G |
| Tools | Shape | U |
| Tools | Switch Rectangle / Ellipse Shape | Shift+U |
| Tools | Pen | P |
| Tools | Text | T |
| Tools | Eyedropper | I |
| Tools | Hand | H |
| Tools | Zoom | Z |
| Tools | Swap Colours | X |
| Tools | Reset Colours | D |
| Tools | Decrease Brush Size | [ |
| Tools | Increase Brush Size | ] |
| Tools | Decrease Brush Hardness | Shift+[ |
| Tools | Increase Brush Hardness | Shift+] |
| Help | Command Palette… | Ctrl+K |
| Help | Keyboard Shortcuts | F1 |
<!-- END GENERATED -->

With nothing selected, Clear Pixels' Delete or Backspace deletes the selected layers instead. With the Pen, Delete or Backspace removes the last anchor drawn or the anchor last edited instead. B selects whichever of Brush and Pencil you used last.

## Other keys and pointer controls

These keys belong to the editor and cannot be assigned to commands.

| Action | Shortcut |
| --- | --- |
| Apply text / Cancel text | Ctrl+Enter / Escape |
| Opacity | Number keys 1–9, 0 for 100% |
| Nudge / Larger nudge | Arrow keys / Shift+arrow keys |
| Zoom | mouse wheel |
| Pan | Space-drag or middle-button drag |
| Pan horizontally | Horizontal mouse wheel or Shift+wheel over the canvas |
| Adjust slider or number | Wheel up / down over the control (increase / decrease) |
| Apply crop or polygon / Cancel | Enter / Escape |
| Finish a Pen path | Enter / Escape |

## Changing shortcuts

In **Edit → Settings… → Keyboard Shortcuts**, search for a command by name, alias or key. Click one of its shortcuts and press the new keys; Escape cancels and Backspace removes that shortcut. **+** adds another shortcut, **Reset** restores a command's defaults and **Reset All** restores every default. When the keys are already in use, Xuan names the command that has them and offers **Reassign** (move them to this command) or **Cancel**. Commands that only apply in RAW Develop may share keys with editor commands. Single letters and digits are kept for tools: other commands need Ctrl or Alt.

Only the shortcuts you change are saved, in the `[keybindings]` table of the configuration file, so new defaults in later versions still reach the rest:

```toml
[keybindings]
merge = "Ctrl+Shift+M"          # a different shortcut
invert_selection = ""           # no shortcut
redo = ["Ctrl+Shift+Z", "Ctrl+Y"]
"comfy/upscale" = "Ctrl+Alt+U"  # a plugin action
```

Unknown commands and values that are not shortcuts are ignored, with a note on standard error.

Shortcuts match their modifiers exactly: Ctrl+Shift+I inverts the selection and never also runs Ctrl+I. Plugins can give their actions a shortcut, shown next to the action in its menu. A plugin shortcut always includes Ctrl or Alt (or is one of F1–F24) and never replaces a shortcut in use, including one you assigned; a clashing one is ignored and reported in **Plugins → Manage Plugins…**. Plugin actions are listed under Plugins in Settings, where their shortcuts can be changed like any other. The panes in the right sidebar are shown or hidden from the **Window** menu.

Copy an image in another app, or copy one or more image files in a file manager, then use Ctrl+V (or Edit → Paste) to add them as layers. External images are centered on the canvas; a new document is created if none is open. Local file URLs and absolute file paths can also be pasted. Multiple files are imported together in one undo step, without changing the source files. When a text field has focus, Ctrl+V pastes text into that field.

Use **File → Open Image from Clipboard** to open copied pixels in a new document sized to the image, even when another document is open. Copied image files open in separate tabs. Clipboard images preserve transparency and prompt to save when closed.

For a marquee selection, Ctrl+C copies the active layer's selected pixels. If no layer is active, it copies the visible canvas within the selection. Ctrl+V places those pixels on a new layer at their original position. Ctrl+Shift+C always copies the visible composite; Ctrl+X requires an active layer. Successful copies show the copied dimensions in the status bar.

Shift with a selection adds coverage, Alt subtracts, and Shift+Alt intersects. Drag inside a selection to move its outline; hold Ctrl to move selected pixels, or Ctrl+Alt to duplicate them. The contextual header also offers explicit selection modes.

Drag from the top or left ruler (View → Rulers) to create a guide. With the Move tool, drag a guide to move it, or drop it on a ruler to delete it; Escape cancels the drag. Hold Ctrl while dragging layers, handles, marquees, shapes, selections or guides to bypass View → Snap To.

Move handles scale the selected layers, the circular handle rotates them, and Ctrl-dragging a corner applies perspective distortion. Shift constrains movement or rotation; the Link control toggles the size ratio. Alt-drag duplicates a layer. The mask thumbnail targets the mask for painting and transformations. Its context menu controls linking and visibility.

Alt-click sets a Clone Stamp source. Shift-click continues a straight brush line. Clone alignment and sampling are configured in the contextual header. Use the same header to switch marquee/lasso/shape variants and linear/radial gradients.

With the Text tool, click the canvas to add text or click existing text to edit it. The text dialog provides multiline input, a searchable list of installed font families, pixel size, color, bold, italic, underline, and strikethrough. Enter starts a new line; Ctrl+Enter applies the preview as one undo step; Escape cancels. Double-click a text layer or choose “Edit text…” from its context menu to reopen it. Use Move to position, scale, or rotate text. Painting or applying pixel filters converts a text layer to pixels; undo restores its text settings.

Each font in the selector previews its own typeface. While the selector is open, Up/Down moves through the filtered fonts and updates the canvas preview. Enter or Escape closes the selector while keeping the preview; Escape again cancels the text edit.

With the Move tool, click visible layer content to select it, or click empty canvas or the surrounding workspace to deselect. Shift-click toggles layers in the selection. Auto Select is enabled by default; turn it off to keep the current selection while moving, or hold Ctrl to select from the canvas temporarily.

Drag a layer row or thumbnail to reorder it. Drop on the upper or lower half of a row to place it above or below that layer; drop in the center of a folder row to nest it. The highlighted line or outline marks the destination. Alt-drag duplicates it. Drop onto another project tab to copy the layer and its descendants. “Move Out of Group” is in the row's context menu. Shift/Ctrl-click layer rows toggles multi-selection.

Supported RAW files (NEF/NRW, CR2/CR3/CRW, RAF, and ARW) enter RAW Develop before becoming layers. In Develop, Ctrl+Z / Ctrl+Shift+Z undo and redo RAW settings; Escape exits the white-balance picker or mask drawing. Drag pans, wheel zooms, and the Fit / 100% buttons set the inspection scale. In Split view, drag near the comparison divider to move it; drag elsewhere or Alt-drag to pan. Space-drag or middle-button drag pans in every view, including with a picker or mask tool active. Side by side zooms both images around their pane centers and pans them together. Double-click a RAW layer (or its image with Move selected) to reopen Develop. Use Develop to commit, or Cancel to retain the previous layer state.

**Ctrl+,** opens Settings in both the photo editor and RAW Develop. **Escape** closes Settings or cancels a Develop eyedropper. **Escape** while dragging with the Eyedropper restores the previous colour.
