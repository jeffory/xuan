# Extend Edges plugin

Adds new canvas around the image and fills it
(**Image → Outpaint (Fill Edges)…**). **Extend by** sets the new canvas on
each side in pixels, and **Fill** mirrors the image at its edges or repeats
the edge pixels outwards. The result is a proposal: the canvas grows, a
new layer covers all of it, masked to the new area so the original pixels
stay as they were, and **Accept** keeps it as one undo step (**Discard**
puts the old canvas back). Standard library only; it declares no network
access and no secrets, so the pixels never leave the machine. It declares
`document = "edit"` because growing the canvas is a document change.

**This is a placeholder for a generative model.** Mirrored edges are not
outpainting. The plugin exists to show the shape of an outpainting plugin:
the host's `source.extend` option, the new-area mask, and a result that
extends the canvas, with progress, cancel and a backend hook, so that a
real model can replace the fill without touching the manifest or the host.

## How it works

The manifest asks for the flattened image extended by the **Extend by**
input on every side:

```toml
source = { from = "composite", max_side = 2048,
           extend = { left = "amount", top = "amount", right = "amount", bottom = "amount" } }
```

Xuan sends `source.png`, already padded with transparent pixels, and
`extend.png`, a grey mask of the same size that is white over the new
canvas and black over the old image. Both are scaled together when the
image is larger than `max_side`. The plugin fills the white area and
returns:

1. an `edit` output with `extend_canvas` by the same amounts
   (`job.extend_canvas(**job.extension)`), and
2. an `image` output with `fit = "source"` and `extend.png` as its mask.

`fit = "source"` covers the bounds of the source that was sent, which is
the extended canvas; Xuan applies the edit and moves the image with the
content, so the layer lands exactly on the new canvas at any `max_side`.

## Try it

```sh
XUAN_PLUGIN_PATH=$PWD/plugins xuan
```

Then allow it in **Plugins → Manage Plugins…**, open an image and run
**Image → Outpaint (Fill Edges)…**. Tests:

```sh
python3 -m unittest discover -s plugins/extend-edges
```

## Swapping in a generative backend

A backend is one function, `outpaint(width, height, rgba, mask, options,
progress, check) -> bytes` returning `width * height * 4` RGBA bytes (see
`fill.py`). Only the pixels under the mask are shown, so a backend may
return the whole image as the model painted it.

1. Put the backend in `backend_<name>.py` next to `main.py` and set
   **Backend** to that name in the plugin settings. A missing backend file
   reports a setup error with a button to the settings.
2. A local model runs from a virtual environment inside the plugin folder,
   started with `command = [".venv/bin/python", "main.py"]`; Xuan itself
   never links against a model runtime.
3. A model on a server (ComfyUI, a hosted API) needs its host in
   `permissions.network`. Xuan then asks before it sends the image and says
   that it is "extended by" the chosen amounts. The `comfy-cloud` example
   shows the network side: upload `source.png` with `extend.png` as the
   inpainting mask.
