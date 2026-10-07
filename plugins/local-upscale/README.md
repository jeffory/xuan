# Local Upscale plugin

Enlarges the active layer 2x, 3x or 4x with a Lanczos resampler and an optional
unsharp mask (**Image → Upscale…**). The result is a new layer above the
original, placed at the original's size with `fit = "source"`, so it keeps the
extra pixels as a higher pixel density instead of landing 2x too large.
Standard library only; it declares no network access and no secrets, so the
pixels never leave the machine.

**This is a placeholder for a machine-learning model.** Lanczos adds no detail;
it only enlarges cleanly. It exists to show the shape of a local-model plugin
(source in, result out, progress, cancel, size limits) and to be replaced by a
real super-resolution backend without touching the manifest or the host.

## Try it

```sh
XUAN_PLUGIN_PATH=$PWD/plugins xuan
```

To install it instead, copy this folder into Xuan's plugins directory or use
**Plugins → Install from Folder or Zip…**. It needs nothing else: Xuan puts
the Python SDK it ships (`xuan_plugin`) on the plugin's `PYTHONPATH` (see
"The Python SDK" in `docs/PLUGINS.md`). With a Xuan from before that, copy
`sdk/python/xuan_plugin.py` from the same release next to `main.py`.

Then allow it in **Plugins → Manage Plugins…**, open an image (at most 4
megapixels; select a smaller area first for bigger ones) and run **Image →
Upscale…**. Pure Python takes about 4 seconds per output megapixel. Tests:

```sh
python3 -m unittest discover -s plugins/local-upscale
```

## Swapping in an ONNX model (for example Real-ESRGAN)

A backend is one function, `upscale(width, height, rgba, scale, progress,
check) -> (width, height, rgba)` (see `upscale.py`). To add Real-ESRGAN:

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
   id = "realesrgan-x4"
   url = "https://example.com/models/realesrgan-x4plus.onnx"
   sha256 = "0000000000000000000000000000000000000000000000000000000000000000"
   size = 67040989                  # bytes, exact
   file = "realesrgan-x4plus.onnx"  # optional; defaults to the URL's file name
   license = "BSD-3-Clause"
   source = "Real-ESRGAN (xinntao)"

   [[actions]]
   id = "upscale"
   # ...
   models = ["realesrgan-x4"]
   ```

   In `main.py`, `job.model_path("realesrgan-x4")` returns the verified file
   (or reports a setup error if it is missing); pass it to the backend, for
   example as a keyword argument like `sharpen`.
3. Add `backend_realesrgan.py` with an `upscale` function that tiles the image
   (for example 256 px tiles with an 16 px overlap), runs the session on each
   tile, calls `check()` and `progress(done / total)` between tiles, and
   stitches the result. The model has a fixed factor, so reject other values
   of `scale` or resize the output.
4. Set **Backend** to `realesrgan` in the plugin settings. A missing backend
   file reports a setup error with a button to the settings.

## Limits of the host today

- The result is a layer, not a replacement: `result.into = "replace"` would
  scale the larger image back down to the layer's own pixels and lose the
  point of upscaling.
- Pixels travel as PNG files, which is slow for very large images.
