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
2. Declare the model in `plugin.toml` and list it in the action, so that Xuan
   downloads it (after asking, with its host, size and licence), checks its
   size and SHA-256, and keeps it in the plugin's models folder, where it
   survives plugin updates. The plugin needs no network permission for this:
   Xuan downloads, the plugin only reads the file. The values below are
   placeholders; use the URL, size and SHA-256 of the exact file you ship
   against, and prefer `.onnx` or `.safetensors` files to pickles.

   ```toml
   [[models]]
   id = "u2net"
   url = "https://example.com/models/u2net.onnx"
   sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
   size = 175997641          # bytes, exact
   license = "Apache-2.0"
   source = "U²-Net (Qin et al., 2020)"

   [[actions]]
   id = "select-bright"
   # ...
   models = ["u2net"]
   ```

   In `main.py`, `job.model_path("u2net")` returns the verified file (or
   reports a setup error if it is missing); pass it to the backend in
   `options`.
3. Add `backend_u2net.py` with a `segment` function that resizes the image to
   the model's input size, runs the session, calls `check()` and `progress()`
   along the way, and returns the predicted matte as grey bytes at the size it
   was given (or return a smaller mask and change `main.py` to write that
   size: `fit_source=True` places a mask of any size over the source).
4. Set **Backend** to `u2net` in the plugin settings. A missing backend file
   reports a setup error with a button to the settings.
