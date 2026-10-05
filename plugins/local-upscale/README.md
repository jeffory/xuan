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
2. Put the model next to the plugin, or download it once into the plugin's
   `data_dir` (that download is the only network use, so declare the host in
   `permissions.network`). Check its SHA-256 against a value pinned in the
   plugin before loading.
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
