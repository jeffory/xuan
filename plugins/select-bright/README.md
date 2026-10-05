# Select Bright Areas plugin

Selects the parts of the image brighter than a threshold, with an optional
soft edge (**Select → Select Bright Areas…**). The plugin returns a `mask`
output; Xuan shows it as the proposed selection, and **Accept** makes it the
selection as one undo step. The **Selection** input chooses whether it
replaces, adds to, subtracts from or intersects with the current selection.
Standard library only; it declares no network access, no secrets and no
document edits (changing the selection needs only `document = "read"`), so
the pixels never leave the machine.

**This is a placeholder for a machine-learning model.** Brightness is not
segmentation. The plugin exists to show the shape of a local segmentation
plugin, such as Select Subject (#5): the composite in, a grey mask out,
progress, cancel and a backend hook, so that a real model can replace it
without touching the manifest or the host.

## Try it

```sh
XUAN_PLUGIN_PATH=$PWD/plugins xuan
```

Then allow it in **Plugins → Manage Plugins…**, open an image and run
**Select → Select Bright Areas…**. The composite is sent at most 1024 pixels
on its longest side; the mask comes back at that size with `fit = "source"`
and Xuan scales it over the whole document. Tests:

```sh
python3 -m unittest discover -s plugins/select-bright
```

## Swapping in an ONNX model (for example U²-Net)

A backend is one function, `segment(width, height, rgba, options, progress,
check) -> bytes` returning `width * height` grey values (see `segment.py`).
To add a salient-object model:

1. Create a virtual environment inside the plugin folder and install
   `onnxruntime`, `numpy` and `pillow` there. Xuan itself never links against
   a model runtime. Start the plugin from it with
   `command = [".venv/bin/python", "main.py"]` (`.venv\Scripts\python.exe` on
   Windows), which the permission prompt shows and re-asks about if it changes.
2. Put the model next to the plugin, or download it once into the plugin's
   `data_dir` (that download is the only network use, so declare the host in
   `permissions.network`). Check its SHA-256 against a value pinned in the
   plugin before loading.
3. Add `backend_u2net.py` with a `segment` function that resizes the image to
   the model's input size, runs the session, calls `check()` and `progress()`
   along the way, and returns the predicted matte as grey bytes at the size it
   was given (or return a smaller mask and change `main.py` to write that
   size: `fit_source=True` places a mask of any size over the source).
4. Set **Backend** to `u2net` in the plugin settings. A missing backend file
   reports a setup error with a button to the settings.
