# Plugin SDK proofs of concept

Small plugins written to find out what Xuan's plugin SDK can do today
(2026-10-08, develop at v0.5.0). They are experiments, not shipped plugins.
`src/app/tests/plugin_pocs.rs` runs each one against the real editor,
headless, and prints what it found:

```sh
(cd poc/plugins/lut-loader && cargo build --release)
(cd poc/plugins/animated-gif && cargo build --release)
cargo test --bin xuan plugin_pocs -- --nocapture --test-threads=1
cargo test --bin xuan lut_round_trip -- --ignored --nocapture
```

| Plugin | Language | Question | Answer |
| --- | --- | --- | --- |
| `lut-loader` | Rust | Can someone load `.cube` LUTs today? | Yes, destructively: Apply LUT… bakes a 1D or 3D LUT into a new layer (2.4 s in the plugin for 24 MP, about 9 s from Run to proposal in a test build). A 1D LUT can become a non-destructive Curves layer, but Curves allows 32 points, so a gamma LUT is off by up to 6/255. A 3D LUT has no non-destructive home. |
| `animated-gif` | Rust | Can someone save animated GIFs today? | Yes, from each top-level layer's own pixels, with delays from layer names. Flips, opacity, masks, effects and groups are ignored, because `layer/export` returns stored pixels. Rendering what the canvas shows needs visibility toggling through `document/edit`: 4 undo steps for 3 frames, and the document is marked modified. Switching tabs mid-export makes `layer/export` read the wrong document. GIF import is impossible: `.gif` is built in, so a 3-frame GIF opens as 1 layer and the plugin is never asked. |
| `timeline` | Python | Can someone build an animation timeline? | Partly. It only fits in the right sidebar. Playback in the pane works (25 image swaps a second against 30 requested). Scrubbing and onion skin are document edits, so 3 scrubs add 3 undo steps and mark the document modified. |
| `openraster` | Python | Can `.ora` (and so OpenRaster/.comp-style layered formats) move to a plugin? | Pixel layers, offsets, opacity, visibility, blend modes and resolution come across. Groups do not: the import result has no groups, so they are flattened into the names and group blend modes are lost. |

See the "Xuan plugin SDK review" doc in the project for the analysis.
