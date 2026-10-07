# AI Surfaces Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Plugin actions appear in three places inside Xuan's own UI: an **AI Region** tool, a **New layer with AI** button and a **Generate** tab in New Image. Each shows a simple prompt-first popover with an Advanced section, and results render at least the size they cover.

**Architecture:**
- **Plugins declare where actions appear.** An action lists `surfaces` (`layer`, `region`, `document`, plus a `verb` for regions). Inputs can be marked `advanced`, or limited with `surfaces = ["menu"]`.
- **Xuan draws the UI** with its own widgets. It runs the action through the existing job path, adding `inputs.surface` and `inputs.target`, the document pixels the result will cover.
- **A new placement, `fit: "cover"`,** scales results evenly over their area without stretching. Exact-size new images use the same rule over a W×H canvas.
- **The Comfy plugin picks render sizes** of at least the target, and gains Generate in region and Fill region.

**Tech Stack:**
- Rust 2024 with egui/eframe 0.33, and egui_kittest for UI tests.
- The plugin is Python 3, standard library only, tested with unittest.

**Spec:** `docs/superpowers/specs/2026-10-07-ai-surfaces-design.md`

## Global Constraints

- Action `surfaces` values are only `"layer"`, `"region"` and `"document"`. Input `surfaces` may also list `"menu"`, meaning the full action dialog.
- `verb` is required for `region` and is at most 24 characters. A `verb` without a `region` surface is refused.
- `layer`: an `edit` action with a `composite` source, or a `generate` action; `result.into = "layer"`; the plugin has `document = "edit"`.
- `region`: an `edit` action with a `regions` input and `document = "edit"`.
- `document`: a `generate` action whose `result.into` is `"document"` or `"ask"`.
- `inputs.target` is `{width, height}` in **document pixels**:
  - `layer`: the canvas;
  - `region`: the box, or the union of several boxes;
  - `document`: the W×H typed.
- `inputs.surface` is the surface name; menu runs carry neither key.
- `fit: "cover"`:
  - It scales evenly to cover the source bounds, centred, and never stretches.
  - The overflow stays in the layer, and the image keeps its own pixels.
  - It is refused for `replace` results and for actions without a source.
- **Exact size:**
  - The canvas is exactly W×H, with one layer covering it, centred. Nothing is cropped.
  - When the layer overflows the canvas, the status reads `Move the layer to reframe`.
  - Checkbox hint: `Make the canvas exactly W × H; the image covers it and can be moved`.
- The ✦ button's tooltip and accessibility label is `New layer with AI`. The tool's label is `AI Region`.
- Popover values are remembered per action for the session only. AI Region boxes are per document, last only for the session, are not undo steps, and are removed once sent.
- Ask the user before every paid Comfy run.
- Conventional commits, never "Fix #N".
- The Python plugin uses only the standard library.

## Review Focus

1. **A plugin that isn't allowed yet, is disabled or is offline, started from a surface.** It must show the permission prompt or the status line, never fail silently. *Test: Task 4.*
2. **Generate with an empty prompt.** The button is disabled until the first basic text input has text. *Test: Task 4.*
3. **Switching tabs with boxes or a popover open.** Boxes belong to their document, and the region popover closes when the document changes. *Test: Task 6.*
4. **Drawing, editing and deleting AI Region boxes.** None of these adds undo steps or marks the document modified. *Test: Task 6.*
5. **An extreme aspect ratio with Exact size or cover,** for example a 4096×1024 result on a 1000×1000 canvas. The scale is correct, the transform is valid, and the overflow is kept. *Test: Task 2 and Task 3.*

---

## File map

| File | Responsibility |
| --- | --- |
| `src/plugins/manifest.rs` | `Surface`; `Action.surfaces`, `Action.verb`; `Input.advanced`, `Input.surfaces`; validation |
| `src/plugins/jobs.rs` | `Fit::Cover`, `Prepared::placed_at`, cover in `place_layer` and `place_mask`, refused in `replace_pixels`; `cover_transform()` for documents |
| `src/app/surfaces.rs` (new) | `SurfaceRun`, `run_from_surface`, the shared `surface_form` widget, remembered values, the layer popover, `surface_actions()` |
| `src/app/ai_regions.rs` (new) | `AiBox`, box editing (add, select, delete, selection), the region popover, grouping boxes into jobs |
| `src/app/plugins.rs` | `ActionEdit.surface`, `PluginJob.surface`, `surface` and `target` in `action_params`, exact-size documents in `apply_job_result`, `PendingStart::Retry`, `close_plugin_action` keeps the tool |
| `src/app/layers.rs` | the ✦ footer button |
| `src/app/icons.rs`, `assets/svg/sparkles.svg` | the ✦ icon |
| `src/app/mod.rs` | `Session.ai_boxes`, `Session.ai_selected`, `EditorApp.surface_popup`, `Tool::Region` label and hint, module wiring |
| `src/app/canvas.rs` | the Region tool in tool mode: gestures, clicks, drawing boxes |
| `src/app/panels.rs` | tool rail visibility, the AI Region options bar |
| `src/app/dialogs.rs` | New Image Blank/Generate tabs |
| `sdk/python/xuan_plugin.py`, `sdk/xuan-plugin/src/lib.rs` | `fit="cover"` in `Job.image`/`Job.mask`, and `Output::fit_cover` |
| `plugins/comfy-cloud/recipes.py` | `size_rule`, `cover_size`, `choose_size`, `mask` role |
| `plugins/comfy-cloud/main.py` | targets, `fit="cover"`, `generate-in-region`, `fill-region` |
| `plugins/comfy-cloud/plugin.toml` | surfaces, verbs, `advanced`, input surfaces, the new actions |
| `docs/PLUGINS.md`, `docs/USAGE.md`, `assets/locales/zh-CN.tsv`, plugin README | docs and strings |

Run Rust tests with `cargo test --locked --bin xuan -- <filter>` (app) or `cargo test --locked --lib <filter>` (library). Run plugin tests with `python3 -m unittest discover -s plugins/comfy-cloud`. Before each Rust commit, run `cargo fmt` and `cargo clippy --locked --all-targets` (clean).

---

### Task 1: Manifest — surfaces, verb, advanced inputs

**Files:**
- Modify: `src/plugins/manifest.rs`: `Action` (~line 395), `Input` (~line 173), `Manifest::validate` (~line 1016) and its tests (~line 1228 onward)

**Interfaces:**
- Produces:
  - `pub enum Surface { Menu, Layer, Region, Document }`, serde lowercase, `Copy + Eq + Hash`.
  - On `Action`: `pub surfaces: Vec<Surface>`, `pub verb: String`, `pub fn on(&self, surface: Surface) -> bool`.
  - On `Input`: `pub advanced: bool`, `pub surfaces: Vec<Surface>`, `pub fn shown_on(&self, surface: Surface) -> bool`.

- [ ] **Step 1: Write the failing tests** (in `manifest.rs` `mod tests`)

```rust
#[test]
fn actions_declare_surfaces_and_inputs_can_be_advanced() {
    let text = r#"
[plugin]
id = "ai"
name = "AI"
command = ["sh", "p.sh"]

[permissions]
document = "edit"

[[actions]]
id = "layer"
label = "Layer…"
surfaces = ["layer"]
source = { from = "composite" }

[[actions.inputs]]
id = "prompt"
type = "multiline"

[[actions.inputs]]
id = "seed"
type = "seed"
advanced = true

[[actions.inputs]]
id = "aspect"
type = "enum"
values = ["1:1"]
surfaces = ["menu"]

[[actions]]
id = "edit"
label = "Edit…"
surfaces = ["region"]
verb = "Edit"

[[actions.inputs]]
id = "regions"
type = "regions"
fields = [{ id = "desc", type = "text" }, { id = "kind", type = "enum", values = ["obj"], advanced = true }]

[[actions]]
id = "new"
label = "New…"
kind = "generate"
surfaces = ["document"]
result = { into = "ask" }
"#;
    let manifest = Manifest::parse(text, Path::new("/p")).unwrap();
    let layer = manifest.action("layer").unwrap();
    assert!(layer.on(Surface::Layer) && !layer.on(Surface::Region));
    assert!(!layer.inputs[0].advanced && layer.inputs[1].advanced);
    assert!(layer.inputs[0].shown_on(Surface::Layer));
    assert!(layer.inputs[2].shown_on(Surface::Menu) && !layer.inputs[2].shown_on(Surface::Layer));
    let edit = manifest.action("edit").unwrap();
    assert_eq!(edit.verb, "Edit");
    assert!(edit.regions_input().unwrap().fields[1].advanced);
    assert!(manifest.action("new").unwrap().on(Surface::Document));
}

#[test]
fn surfaces_are_checked_against_the_action() {
    let dir = Path::new("/p");
    let base = |action: &str, document: &str| {
        format!(
            "[plugin]\nid = \"ai\"\nname = \"AI\"\ncommand = [\"sh\"]\n\n[permissions]\ndocument = \"{document}\"\n\n{action}"
        )
    };
    let refused = |action: &str, document: &str| {
        format!("{:#}", Manifest::parse(&base(action, document), dir).unwrap_err())
    };
    // layer: composite edit or generate, into layer, document = edit
    assert!(refused("[[actions]]\nid = \"a\"\nlabel = \"A\"\nsurfaces = [\"layer\"]\nsource = { from = \"layer\" }", "edit").contains("layer"));
    assert!(refused("[[actions]]\nid = \"a\"\nlabel = \"A\"\nkind = \"generate\"\nsurfaces = [\"layer\"]", "read").contains("document = \"edit\""));
    // region: regions input and a short verb
    assert!(refused("[[actions]]\nid = \"a\"\nlabel = \"A\"\nsurfaces = [\"region\"]\nverb = \"Edit\"", "edit").contains("regions"));
    let regions = "\n\n[[actions.inputs]]\nid = \"r\"\ntype = \"regions\"";
    assert!(refused(&format!("[[actions]]\nid = \"a\"\nlabel = \"A\"\nsurfaces = [\"region\"]{regions}"), "edit").contains("verb"));
    assert!(refused(&format!("[[actions]]\nid = \"a\"\nlabel = \"A\"\nsurfaces = [\"region\"]\nverb = \"{}\"{regions}", "x".repeat(25)), "edit").contains("24"));
    assert!(refused("[[actions]]\nid = \"a\"\nlabel = \"A\"\nverb = \"Edit\"", "edit").contains("verb"));
    // document: generate into document or ask
    assert!(refused("[[actions]]\nid = \"a\"\nlabel = \"A\"\nsurfaces = [\"document\"]", "edit").contains("generate"));
    // menu is not an action surface; no repeats
    assert!(refused("[[actions]]\nid = \"a\"\nlabel = \"A\"\nkind = \"generate\"\nsurfaces = [\"menu\"]", "edit").contains("menu"));
    assert!(refused("[[actions]]\nid = \"a\"\nlabel = \"A\"\nkind = \"generate\"\nsurfaces = [\"document\", \"document\"]\nresult = { into = \"document\" }", "edit").contains("twice"));
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --locked --lib plugins::manifest::tests::actions_declare plugins::manifest::tests::surfaces_are`
Expected: compile errors, because `Surface`, `on` and `advanced` don't exist yet.

- [ ] **Step 3: Implement**

Add, near `Menu`:

```rust
/// Where an action appears besides its menu, and where an input is shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    /// The action's full dialog, opened from its menu (inputs only).
    Menu,
    /// The New layer with AI button in the Layers panel.
    Layer,
    /// The AI Region tool.
    Region,
    /// The Generate tab of New Image.
    Document,
}
```

In `Action`, after `models`:

```rust
    /// Places besides its menu where the action appears.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surfaces: Vec<Surface>,
    /// Short label in the AI Region popover (`region` surface).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub verb: String,
```

In `impl Action`:

```rust
    pub fn on(&self, surface: Surface) -> bool {
        self.surfaces.contains(&surface)
    }
```

In `Input`, after `fields`:

```rust
    /// Behind "Advanced" in popovers and the New Image tab.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub advanced: bool,
    /// Where the input is shown; empty means everywhere.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surfaces: Vec<Surface>,
```

In `impl Input`:

```rust
    pub fn shown_on(&self, surface: Surface) -> bool {
        self.surfaces.is_empty() || self.surfaces.contains(&surface)
    }
```

In `Manifest::validate`, inside `for action in &self.actions`, after the `max_side` check:

```rust
            let mut listed = std::collections::HashSet::new();
            for surface in &action.surfaces {
                ensure!(listed.insert(*surface), "action `{}` lists a surface twice", action.id);
                match surface {
                    Surface::Menu => bail!(
                        "action `{}`: every action is in its menu; surfaces lists only layer, region or document",
                        action.id
                    ),
                    Surface::Layer => {
                        ensure!(
                            action.kind == ActionKind::Generate
                                || (action.kind == ActionKind::Edit && action.source.from == SourceKind::Composite),
                            "action `{}`: the layer surface needs a generate action or an edit action with a composite source",
                            action.id
                        );
                        ensure!(action.result.into == ResultInto::Layer, "action `{}`: the layer surface needs result.into = \"layer\"", action.id);
                        ensure!(self.permissions.document == DocumentAccess::Edit, "action `{}`: the layer surface needs document = \"edit\"", action.id);
                    }
                    Surface::Region => {
                        ensure!(
                            action.kind == ActionKind::Edit && action.regions_input().is_some(),
                            "action `{}`: the region surface needs an edit action with a regions input",
                            action.id
                        );
                        ensure!(self.permissions.document == DocumentAccess::Edit, "action `{}`: the region surface needs document = \"edit\"", action.id);
                        ensure!(!action.verb.trim().is_empty(), "action `{}`: the region surface needs a verb", action.id);
                        ensure!(action.verb.chars().count() <= 24, "action `{}`: the verb must be at most 24 characters", action.id);
                    }
                    Surface::Document => ensure!(
                        action.kind == ActionKind::Generate
                            && matches!(action.result.into, ResultInto::Document | ResultInto::Ask),
                        "action `{}`: the document surface needs a generate action whose result goes into a document or asks",
                        action.id
                    ),
                }
            }
            ensure!(
                action.verb.is_empty() || action.on(Surface::Region),
                "action `{}` has a verb but no region surface",
                action.id
            );
```

(Add `bail` to the `anyhow` import if it isn't there.)

- [ ] **Step 4: Run the tests**

Run: `cargo test --locked --lib plugins::manifest`
Expected: all tests pass, old and new.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --locked --all-targets
git add src/plugins/manifest.rs
git commit -m "feat(plugins): actions declare surfaces and inputs can be advanced"
```

---

### Task 2: `fit: "cover"` placement (host and SDKs)

**Files:**
- Modify: `src/plugins/jobs.rs`: `Fit` (~line 81), `Prepared::placed_size` (~line 187), `place_layer` (~line 558), `place_mask` (~line 632), `replace_pixels` (~line 776), `unsourced_placement` (~line 595), tests
- Modify: `sdk/python/xuan_plugin.py`: `Job.image`, `Job.mask` (~lines 348–420); `sdk/python/test_xuan_plugin.py`
- Modify: `sdk/xuan-plugin/src/lib.rs`: add `fit_cover` next to `fit_source` (~line 811)

**Interfaces:**
- Produces:
  - `Fit::Cover`.
  - `pub fn cover(pixels: (f32, f32), area: (f32, f32)) -> (f32, f32, f32, f32)`: x, y, width and height of the covering rectangle, relative to the area.
  - `Prepared::placed_at(&self, pixels, x, y, placed) -> Result<(f32, f32, (f32, f32))>`: the export-pixel top-left and the source-pixel size.
  - Python: `Job.image(..., fit=None)`, where `fit` is `"source"` or `"cover"`; `fit_source=True` still works.
  - Rust SDK: `Output::fit_cover()`.

- [ ] **Step 1: Write the failing tests** (`jobs.rs` tests)

```rust
#[test]
fn cover_scales_evenly_and_centres_without_cropping() {
    // A wide image over a square area: height matches, width hangs over evenly.
    assert_eq!(cover((400.0, 100.0), (200.0, 200.0)), (-300.0, 0.0, 800.0, 200.0));
    // Same shape: exactly the area.
    assert_eq!(cover((100.0, 50.0), (200.0, 100.0)), (0.0, 0.0, 200.0, 100.0));
    // Extreme ratio from the review focus: 4096x1024 on 1000x1000.
    let (x, y, w, h) = cover((4096.0, 1024.0), (1000.0, 1000.0));
    assert_eq!((y, h), (0.0, 1000.0));
    assert!((w - 4000.0).abs() < 1e-3 && (x + 1500.0).abs() < 1e-3);
}

#[test]
fn cover_results_are_placed_over_the_source_and_keep_their_pixels() {
    let dir = tempfile::tempdir().unwrap();
    let document = document();
    let source = Source { from: SourceKind::Composite, max_side: Some(200), ..Source::default() };
    let prepared = prepare(&document, &source, &[], dir.path()).unwrap();
    // Twice as wide as the source's shape: covers the height, overflows the sides.
    let image = RgbaImage::new(800, 150);
    let placed = Placed { fit: Some(Fit::Cover), ..Placed::default() };
    let layer = place_layer(&prepared, "Wide", image, 0.0, 0.0, &placed, None).unwrap();
    assert_eq!(layer.pixels.as_ref().unwrap().dimensions(), (800, 150));
    let t = layer.transform;
    assert!((t.height - 300.0).abs() < 1e-3, "{t:?}");
    assert!((t.width - 1600.0).abs() < 1e-3, "{t:?}");
    assert!((t.x - (0.0 + (400.0 - 1600.0) / 2.0)).abs() < 1e-3, "{t:?}");
    // Replacing the source layer cannot hang over: refused.
    let pixels = RgbaImage::new(4, 4);
    assert!(replace_pixels(&prepared, &pixels, &RgbaImage::new(2, 2), 0.0, 0.0, &placed).is_err());
    // Without a source there is nothing to cover.
    assert!(unsourced_placement((8, 8), 0.0, 0.0, &placed).is_err());
}
```

(`document()` is the existing test helper: a 400×300 canvas. With a composite source the crop is the whole 400×300 canvas.)

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --locked --lib plugins::jobs::tests::cover`
Expected: compile errors, because `cover` and `Fit::Cover` don't exist yet.

- [ ] **Step 3: Implement**

```rust
pub enum Fit {
    Source,
    /// Scale evenly to cover the source bounds, centred; overflow stays.
    Cover,
}

/// The rectangle an image of `pixels` covers `area` with: scaled evenly to
/// the larger of the two ratios and centred, as `(x, y, width, height)`
/// relative to the area.
pub fn cover(pixels: (f32, f32), area: (f32, f32)) -> (f32, f32, f32, f32) {
    let scale = (area.0 / pixels.0.max(1.0)).max(area.1 / pixels.1.max(1.0));
    let (width, height) = (pixels.0 * scale, pixels.1 * scale);
    ((area.0 - width) / 2.0, (area.1 - height) / 2.0, width, height)
}
```

In `placed_size`, replace the `fit` branch with:

```rust
        match placed.fit {
            Some(Fit::Source) => return Ok((self.crop.2, self.crop.3)),
            Some(Fit::Cover) => {
                let (_, _, width, height) = cover((w, h), (self.crop.2, self.crop.3));
                return Ok((width, height));
            }
            None => {}
        }
```

Add the following to `impl Prepared`:

```rust
    /// Where an image of `pixels` lands: its top-left in export pixels and
    /// its size in source pixels. `cover` centres it over the crop.
    pub fn placed_at(&self, pixels: (u32, u32), x: f32, y: f32, placed: &Placed) -> Result<(f32, f32, (f32, f32))> {
        let size = self.placed_size(pixels, placed)?;
        if placed.fit == Some(Fit::Cover) {
            let (dx, dy) = ((self.crop.2 - size.0) / 2.0, (self.crop.3 - size.1) / 2.0);
            return Ok((x + dx * self.scale, y + dy * self.scale, size));
        }
        Ok((x, y, size))
    }
```

In `place_layer`, replace the `size` and transform lines with:

```rust
    let (x, y, size) = prepared.placed_at(image.dimensions(), x, y, placed)?;
    ...
    layer.transform = prepared.placement_sized(x, y, size);
```

Make the same change in `place_mask`. In `replace_pixels`, at the top:

```rust
    ensure!(placed.fit != Some(Fit::Cover), "A result that replaces the source layer cannot use fit = \"cover\"");
```

In `unsourced_placement`, change the message to `"fit needs an action with a source"`. The existing check already refuses any `fit`.

Python SDK (`Job.image` and `Job.mask`): add a `fit: Optional[str] = None` parameter, then:

```python
        if fit_source:
            fit = fit or "source"
        if fit:
            if fit not in ("source", "cover"):
                raise ValueError("fit is \"source\" or \"cover\"")
            output["fit"] = fit
```

Update the docstrings to say that `fit="cover"` scales the result evenly to cover the source, centred, keeping what hangs over. Add a test to `sdk/python/test_xuan_plugin.py`:

```python
    def test_image_outputs_can_cover_the_source(self):
        self.assertEqual(Job.image("a.png", fit="cover")["fit"], "cover")
        self.assertEqual(Job.image("a.png", fit_source=True)["fit"], "source")
        with self.assertRaises(ValueError):
            Job.image("a.png", fit="stretch")
```

Rust SDK, next to `fit_source`:

```rust
    /// Scale an image or mask evenly to cover the source that was sent,
    /// centred (`fit = "cover"`); what hangs over stays in the layer.
    pub fn fit_cover(mut self) -> Self {
        if let Self::Image { width, height, fit, .. } | Self::Mask { width, height, fit, .. } = &mut self {
            *width = None;
            *height = None;
            *fit = Some("cover".into());
        }
        self
    }
```

Add a unit test in the Rust SDK's tests that mirrors the existing `fit_source` test, asserting `"fit":"cover"` in the serialized output.

- [ ] **Step 4: Run the tests**

Run:
- `cargo test --locked --lib plugins::jobs`
- `cargo test --locked -p xuan-plugin`
- `python3 -m unittest discover -s sdk/python`

Expected: all pass.

- [ ] **Step 5: Extend the app-level output test**

In `src/app/tests/plugins.rs`, `image_outputs_may_declare_their_placed_size`, add `(json!({"fit": "cover"}), false)` to the list, since the mock job has no source.

Run: `cargo test --locked --bin xuan -- image_outputs_may_declare`
Expected: PASS.

- [ ] **Step 6: Commit**

```bash
cargo fmt && cargo clippy --locked --all-targets
git add src/plugins/jobs.rs src/app/tests/plugins.rs sdk/python sdk/xuan-plugin
git commit -m "feat(plugins): fit = \"cover\" places a result evenly over its source"
```

---

### Task 3: Surface runs: `inputs.surface`, `inputs.target` and exact-size documents

**Files:**
- Create: `src/app/surfaces.rs`
- Modify:
  - `src/app/mod.rs`: `mod surfaces;`
  - `src/app/plugins.rs`: `ActionEdit` (~line 218), `PluginJob` (~line 147), `PendingStart` (~line 141), `start_plugin_action_with` (both `ActionEdit` literals), `action_params` (~line 1638), `run_plugin_action` (the `PluginJob` literal), `close_plugin_action` (~line 1626), `apply_job_result` (the `new_documents` loop, ~line 2128)
- Modify the tests: `src/app/tests/plugins.rs` (`mock_job` gets `surface: None`), `src/app/tests/ui.rs` (`status_bar::job` gets `surface: None`)

**Interfaces:**
- Consumes: `Surface` (Task 1) and `jobs::cover` (Task 2).
- Produces:
  - `pub(super) struct SurfaceRun { pub surface: Surface, pub target: (u32, u32), pub exact: bool, pub resolution: f32 }`, which is `Clone + Debug + PartialEq`.
  - On `EditorApp`: `pub(super) fn run_from_surface(&mut self, plugin: &str, action: &str, given: &Map<String, Value>, regions: Vec<Region>, run: SurfaceRun) -> bool`. It returns whether a job started or is waiting on a prompt.
  - `ActionEdit.surface: Option<SurfaceRun>` and `PluginJob.surface: Option<SurfaceRun>`.

- [ ] **Step 1: Write the failing tests** (`src/app/tests/plugins.rs`, `mod unix`)

```rust
#[test]
fn surface_runs_tell_the_plugin_where_they_came_from_and_the_target_size() {
    let dir = tempfile::tempdir().unwrap();
    let (context, mut app) = app();
    install_mock(&mut app, dir.path());
    app.dimensions = [64, 48];
    app.new_document();
    app.command("fill_fg");
    frame(&context, &mut app);
    let run = crate::app::surfaces::SurfaceRun { surface: xuan::plugins::manifest::Surface::Region, target: (20, 10), exact: false, resolution: 72.0 };
    let regions = vec![xuan::plugins::jobs::Region::rect(4.0, 4.0, 20.0, 10.0)];
    let given = serde_json::Map::from_iter([("prompt".to_owned(), serde_json::json!("a hat"))]);
    assert!(app.run_from_surface("mock", "echo", &given, regions, run));
    assert_eq!(app.plugins.jobs.len(), 1);
    run_until(&context, &mut app, |app| app.dialog == Some(Dialog::PluginProposal));
    let received = std::fs::read_to_string(dir.path().join("received.log")).unwrap();
    let line = received.lines().find(|l| l.contains("\"action/run\"")).unwrap();
    assert!(line.contains("\"surface\":\"region\""), "{line}");
    assert!(line.contains("\"target\":{\"height\":10,\"width\":20}") || line.contains("\"target\":{\"width\":20,\"height\":10}"), "{line}");
    assert!(line.contains("\"prompt\":\"a hat\""), "{line}");
    // The Region tool stays selected after a surface run.
    assert_ne!(app.tool, Tool::Move);
}

#[test]
fn a_surface_run_waits_for_permission_and_never_runs_silently() {
    // Review focus 1: not yet allowed → the permission prompt; disabled or offline → a status.
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    let fixture = dir.path().join("fixture.png");
    RgbaImage::new(2, 2).save(&fixture).unwrap();
    std::fs::write(dir.path().join("plugin.sh"), script(&fixture)).unwrap();
    std::fs::write(dir.path().join("plugin.toml"), MANIFEST).unwrap();
    app.install_plugins(vec![Manifest::load(dir.path()).unwrap()], vec![]);
    app.dimensions = [16, 16];
    app.new_document();
    app.command("fill_fg");
    let run = crate::app::surfaces::SurfaceRun { surface: xuan::plugins::manifest::Surface::Layer, target: (16, 16), exact: false, resolution: 72.0 };
    assert!(app.run_from_surface("mock", "echo", &serde_json::Map::new(), Vec::new(), run.clone()));
    assert_eq!(app.dialog, Some(Dialog::PluginPermissions));
    assert!(app.plugins.jobs.is_empty());
    app.dialog = None;
    app.config.plugins.entry("mock".into()).or_default().enabled = false;
    assert!(!app.run_from_surface("mock", "echo", &serde_json::Map::new(), Vec::new(), run));
    assert!(app.plugins.jobs.is_empty());
}
```

Also add this to the non-unix part of the file (it feeds a result in directly):

```rust
#[test]
fn an_exact_size_document_result_covers_a_canvas_of_that_size() {
    use serde_json::json;
    let dir = tempfile::tempdir().unwrap();
    let (_context, mut app) = app();
    install_mock(&mut app, dir.path());
    // 40x10 from the model for a 20x20 canvas: covers the height, hangs over the sides.
    let wide = dir.path().join("wide.png");
    RgbaImage::from_pixel(40, 10, image::Rgba([9, 9, 9, 255])).save(&wide).unwrap();
    let mut job = mock_job_without_document();
    job.into = xuan::plugins::manifest::ResultInto::Document;
    job.surface = Some(crate::app::surfaces::SurfaceRun { surface: xuan::plugins::manifest::Surface::Document, target: (20, 20), exact: true, resolution: 300.0 });
    app.apply_job_result(&job, json!({"outputs": [{"kind": "image", "path": wide, "name": "A fox"}]})).unwrap();
    let document = &app.session().unwrap().document;
    assert_eq!((document.width, document.height), (20, 20));
    assert_eq!(document.resolution, 300.0);
    let layer = &document.layers[0];
    assert_eq!(layer.pixels.as_ref().unwrap().dimensions(), (40, 10));
    assert_eq!((layer.transform.x, layer.transform.y, layer.transform.width, layer.transform.height), (-30.0, 0.0, 80.0, 20.0));
    assert!(app.status.contains(tr("Move the layer to reframe")));
    // Without exact the document takes the image's own size.
    job.surface.as_mut().unwrap().exact = false;
    app.apply_job_result(&job, json!({"outputs": [{"kind": "image", "path": wide}]})).unwrap();
    let document = &app.session().unwrap().document;
    assert_eq!((document.width, document.height), (40, 10));
}
```

Add a helper next to `mock_job`:

```rust
/// A finished job of the mock plugin that started with no document open.
fn mock_job_without_document() -> crate::app::plugins::PluginJob {
    crate::app::plugins::PluginJob {
        id: uuid::Uuid::new_v4(),
        plugin: "mock".into(),
        action: "echo".into(),
        label: "Echo".into(),
        document: uuid::Uuid::nil(),
        _work_dir: xuan::plugins::private_dir("xuan-job-").unwrap(),
        prepared: xuan::plugins::jobs::Prepared::none(),
        regions: Vec::new(),
        inputs: serde_json::json!({}),
        into: xuan::plugins::manifest::ResultInto::Layer,
        mask_to_regions: false,
        progress: None,
        message: String::new(),
        cancelled: false,
        consented: false,
        provider: None,
        surface: None,
    }
}
```

- [ ] **Step 2: Run them to see them fail**

Run: `cargo test --locked --bin xuan -- surface_runs a_surface_run an_exact_size`
Expected: compile errors, because the `surfaces` module and `run_from_surface` don't exist yet.

- [ ] **Step 3: Implement**

`src/app/surfaces.rs`:

```rust
//! Plugin actions started from Xuan's own UI (the Layers panel, the AI
//! Region tool and New Image) instead of their dialog.
use serde_json::{Map, Value};
use xuan::{
    i18n::tr,
    plugins::{
        jobs::Region,
        manifest::{InputKind, ResultInto, Surface},
    },
};

use super::{
    Dialog, EditorApp,
    plugins::{ActionEdit, PendingStart},
};

/// How a surface started an action.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SurfaceRun {
    pub surface: Surface,
    /// Document pixels the result will cover.
    pub target: (u32, u32),
    /// New Image: make the canvas exactly `target`.
    pub exact: bool,
    /// New Image: the new document's resolution.
    pub resolution: f32,
}

impl EditorApp {
    /// Run `action` from a surface with the values the surface collected
    /// (missing ones take their defaults) and, for the region surface, its
    /// boxes. The usual prompts apply: permission, send consent and model
    /// downloads. Returns false when nothing will happen, with the reason in
    /// the status bar.
    pub(super) fn run_from_surface(
        &mut self,
        plugin: &str,
        action: &str,
        given: &Map<String, Value>,
        regions: Vec<Region>,
        run: SurfaceRun,
    ) -> bool {
        let Some(spec) = self.plugins.manifest(plugin).and_then(|m| m.action(action)).cloned() else {
            return false;
        };
        if !self.plugin_enabled(plugin) {
            self.status = format!("{} {}", self.plugins.source(plugin), tr("is disabled"));
            return false;
        }
        if self.plugin_offline(plugin) {
            self.status = format!("{} {}", self.plugins.source(plugin), tr("uses the network, and plugins that use the network are disabled."));
            return false;
        }
        if !self.plugin_granted(plugin) {
            self.plugins.permission_request = Some((plugin.into(), PendingStart::Retry));
            self.dialog = Some(Dialog::PluginPermissions);
            return true;
        }
        let mut values = Map::new();
        for input in spec.inputs.iter().filter(|i| i.kind != InputKind::Regions) {
            let value = given.get(&input.id).map_or_else(|| input.initial(), |v| input.coerce(v));
            values.insert(input.id.clone(), value);
        }
        if !self.action_models_ready(plugin, action, &Value::Object(values.clone())) {
            return true;
        }
        let into = if run.surface == Surface::Document { ResultInto::Document } else { ResultInto::Layer };
        self.plugins.action = Some(ActionEdit {
            plugin: plugin.into(),
            action: action.into(),
            values,
            regions,
            selected: None,
            estimate: None,
            previous_tool: self.tool,
            into,
            consented: false,
            provider: None,
            surface: Some(run),
        });
        self.run_plugin_action();
        true
    }
}
```

Check the real signature of `action_models_ready` (`plugins.rs`) and pass what it takes.

In `plugins.rs`:
- Add `Retry` to `PendingStart`, documented as "the user starts it again from the surface". Find where `PendingStart` is matched after a grant (`grant_plugin` or the permission dialog) and make `Retry` do nothing.
- Add `pub surface: Option<super::surfaces::SurfaceRun>` to `ActionEdit` and to `PluginJob`. Set `surface: None` in both `ActionEdit` literals in `start_plugin_action_with`.
- In `run_plugin_action`, before `let job = Uuid::new_v4();`, add `let surface = (self.plugins.action.as_ref()).and_then(|edit| edit.surface.clone());` and pass `surface` into the `PluginJob` literal.
- In `action_params`, after the provider block:

```rust
        if let Some(run) = &edit.surface {
            inputs.insert("surface".into(), json!(run.surface));
            inputs.insert("target".into(), json!({"width": run.target.0, "height": run.target.1}));
        }
```

- In `close_plugin_action`, keep the tool for surface runs:

```rust
        if let Some(edit) = self.plugins.action.take()
            && edit.surface.is_none()
            && self.tool == Tool::Region
```

- In `apply_job_result`'s `for (name, image, provenance) in new_documents` loop:

```rust
        let exact = job.surface.as_ref().filter(|run| run.surface == Surface::Document);
        for (name, image, provenance) in new_documents {
            let (pixels_w, pixels_h) = image.dimensions();
            let (width, height) = match exact {
                Some(run) if run.exact => run.target,
                _ => (pixels_w, pixels_h),
            };
            let mut document = Document::new(width, height)?;
            if let Some(run) = exact {
                document.resolution = run.resolution;
            }
            let mut layer = Layer::image(&name, image);
            if exact.is_some_and(|run| run.exact) {
                let (x, y, w, h) = jobs::cover((pixels_w as f32, pixels_h as f32), (width as f32, height as f32));
                layer.transform = xuan::document::Transform { x, y, width: w, height: h, ..xuan::document::Transform::new(pixels_w, pixels_h) };
                if w > width as f32 + 0.5 || h > height as f32 + 0.5 {
                    self.status = tr("Move the layer to reframe").into();
                }
            }
            // …the rest of the existing loop (generated, provenance, Session::new…)
        }
```

`Document::new(...)?` must be inside the function's `Result` flow, as today. Add `use xuan::plugins::manifest::Surface;` and `mod surfaces;` in `mod.rs`.

- [ ] **Step 4: Run the tests**

Run: `cargo test --locked --bin xuan -- surface_runs a_surface_run an_exact_size image_outputs plugin`
Expected: PASS.

- [ ] **Step 5: Run the whole app suite and commit**

```bash
cargo test --locked --bin xuan 2>&1 | grep 'test result'
cargo fmt && cargo clippy --locked --all-targets
git add src/app
git commit -m "feat(plugins): surface runs send their surface and target size; exact-size documents"
```

---

### Task 4: Shared surface form, remembered values, layer popover and the ✦ button

**Files:**
- Modify: `src/app/surfaces.rs` (form, popover, remembered values, `surface_actions`), `src/app/mod.rs` (`surface_popup` field, called each frame), `src/app/layers.rs` (`layer_footer`, `Actions`), `src/app/icons.rs`
- Create: `assets/svg/sparkles.svg`
- Test: `src/app/tests/ui.rs`, new `mod surfaces`

**Interfaces:**
- Consumes: `run_from_surface` and `SurfaceRun` (Task 3); `Input::shown_on` and `advanced` (Task 1); `plugin_dialogs::input_widget`.
- Produces:
  - `pub(super) fn surface_actions(&self, surface: Surface) -> Vec<(String, String, String)>`: plugin, action and label for enabled plugins, in manifest order.
  - `pub(super) fn surface_form(&mut self, ui: &mut egui::Ui, plugin: &str, action: &str, surface: Surface, salt: &str) -> bool`: draws the basic inputs, then Advanced, then the estimate; Generate is left to the caller. Returns whether the first basic text input has text.
  - `pub(super) enum SurfacePopup { Layer { plugin: String, action: String, anchor: egui::Pos2 }, Region { document: uuid::Uuid, index: usize } }` and `EditorApp.surface_popup: Option<SurfacePopup>`.
  - `self.plugins.surface_values: HashMap<(String, String), Map<String, Value>>`, the values remembered per action.

- [ ] **Step 1: Write the failing UI test** (`src/app/tests/ui.rs`)

```rust
/// Plugin actions in the Layers panel, the toolbox and New Image.
mod surfaces {
    use super::*;

    /// A plugin with a layer action whose prompt is basic and seed advanced.
    pub(super) fn install(ui: &mut UiTest, dir: &std::path::Path) {
        std::fs::write(dir.join("plugin.toml"), r#"
[plugin]
id = "ai"
name = "AI"
command = ["sh", "-c", "cat > /dev/null"]

[permissions]
document = "edit"

[[actions]]
id = "layer"
label = "Generate Layer…"
surfaces = ["layer"]
source = { from = "composite" }

[[actions.inputs]]
id = "prompt"
type = "multiline"
label = "What to add"

[[actions.inputs]]
id = "seed"
type = "seed"
label = "Seed"
advanced = true
"#).unwrap();
        let manifest = xuan::plugins::Manifest::load(dir).unwrap();
        ui.app_mut().install_plugins(vec![manifest], vec![]);
        ui.app_mut().grant_plugin("ai", true);
    }

    #[test]
    fn the_ai_layer_button_opens_a_prompt_first_popover() {
        let dir = tempfile::tempdir().unwrap();
        let mut ui = UiTest::with_document();
        assert!(!ui.has("New layer with AI"));
        install(&mut ui, dir.path());
        ui.settle();
        ui.click("New layer with AI");
        assert!(ui.has("What to add"));
        assert!(ui.has("Advanced"));
        assert!(!ui.has("Seed"), "advanced inputs start collapsed");
        // Review focus 2: no prompt, no Generate.
        assert!(!ui.enabled("Generate"));
        ui.type_in_text_field("a red kite");
        assert!(ui.enabled("Generate"));
        ui.click("Advanced");
        assert!(ui.has("Seed"));
    }
}
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test --locked --bin xuan -- ui::surfaces`
Expected: FAIL, because "New layer with AI" isn't found.

- [ ] **Step 3: Implement**

`assets/svg/sparkles.svg`, a single-colour 24×24 icon in the style of the other SVGs (stroke `currentColor` is not needed, because `svg()` tints it):

```svg
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="#000" stroke-width="1.8" stroke-linejoin="round"><path d="M12 3l1.9 5.6L19.5 10.5l-5.6 1.9L12 18l-1.9-5.6L4.5 10.5l5.6-1.9z"/><path d="M19 15l.8 2.2L22 18l-2.2.8L19 21l-.8-2.2L16 18l2.2-.8z"/></svg>
```

`icons.rs`: add `"ai_layer" => egui::include_image!("../../assets/svg/sparkles.svg"),` to `action_button`.

`surfaces.rs` additions:

```rust
/// A popover a surface opened.
pub(super) enum SurfacePopup {
    /// From the New layer with AI button, anchored under it.
    Layer { plugin: String, action: String, anchor: egui::Pos2 },
    /// An AI Region box's popover (Task 7).
    Region { document: uuid::Uuid, index: usize },
}

impl EditorApp {
    /// Enabled plugins' actions on `surface`: (plugin, action, label without "…").
    pub(super) fn surface_actions(&self, surface: Surface) -> Vec<(String, String, String)> {
        self.plugins.manifests.iter()
            .filter(|m| self.plugin_enabled(&m.plugin.id))
            .flat_map(|m| m.actions.iter().filter(move |a| a.on(surface))
                .map(move |a| (m.plugin.id.clone(), a.id.clone(), a.label.trim_end_matches('…').to_owned())))
            .collect()
    }

    /// The values a surface shows for an action: last used, else defaults.
    fn surface_values(&mut self, plugin: &str, action: &str) -> &mut Map<String, Value> {
        let spec = self.plugins.manifest(plugin).and_then(|m| m.action(action)).cloned();
        self.plugins.surface_values.entry((plugin.into(), action.into())).or_insert_with(|| {
            spec.map(|spec| spec.inputs.iter().filter(|i| i.kind != InputKind::Regions)
                .map(|i| (i.id.clone(), i.initial())).collect()).unwrap_or_default()
        })
    }

    /// An action's inputs for `surface`: basic ones, then a collapsed
    /// "Advanced" section. Returns whether the first basic text input has
    /// text (Generate needs it).
    pub(super) fn surface_form(&mut self, ui: &mut egui::Ui, plugin: &str, action: &str, surface: Surface, salt: &str) -> bool {
        let Some(spec) = self.plugins.manifest(plugin).and_then(|m| m.action(action)).cloned() else {
            return false;
        };
        let shown: Vec<_> = spec.inputs.iter()
            .filter(|i| i.kind != InputKind::Regions && i.shown_on(surface)).cloned().collect();
        let values = self.surface_values(plugin, action);
        let mut ready = true;
        let mut first_text = true;
        for input in shown.iter().filter(|i| !i.advanced) {
            let value = values.entry(input.id.clone()).or_insert_with(|| input.initial());
            super::plugin_dialogs::input_widget(ui, input, value, (salt, "basic"));
            if first_text && matches!(input.kind, InputKind::Text | InputKind::Multiline) {
                ready = value.as_str().is_some_and(|text| !text.trim().is_empty());
                first_text = false;
            }
        }
        let advanced: Vec<_> = shown.iter().filter(|i| i.advanced).collect();
        if !advanced.is_empty() {
            egui::CollapsingHeader::new(tr("Advanced")).id_salt((salt, "advanced")).default_open(false).show(ui, |ui| {
                for input in advanced {
                    let value = values.entry(input.id.clone()).or_insert_with(|| input.initial());
                    super::plugin_dialogs::input_widget(ui, input, value, (salt, "advanced"));
                }
            });
        }
        ready
    }

    /// The layer popover: label, form, Generate.
    pub(super) fn surface_popups(&mut self, ctx: &egui::Context) {
        let Some(SurfacePopup::Layer { plugin, action, anchor }) = &self.surface_popup else {
            if matches!(self.surface_popup, Some(SurfacePopup::Region { .. })) {
                self.region_popup(ctx); // Task 7
            }
            return;
        };
        let (plugin, action, anchor) = (plugin.clone(), action.clone(), *anchor);
        let label = self.surface_actions(Surface::Layer).into_iter()
            .find(|(p, a, _)| *p == plugin && *a == action).map(|(_, _, l)| l).unwrap_or_default();
        let mut generate = false;
        let response = egui::Area::new(egui::Id::new(("surface_popup", &plugin, &action)))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(anchor)
            .show(ctx, |ui| {
                super::theme::frame(&ctx.palette()).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                    ui.set_width(300.0);
                    ui.label(egui::RichText::new(&label).strong());
                    let ready = self.surface_form(ui, &plugin, &action, Surface::Layer, "layer_popup");
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let button = ui.add_enabled(ready, super::widgets::Button::new(tr("Generate")).primary());
                        generate = button.clicked() || (ready && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)));
                    });
                });
            });
        let close = ctx.input(|i| i.key_pressed(egui::Key::Escape)) || response.response.clicked_elsewhere();
        if generate {
            let values = self.surface_values(&plugin, &action).clone();
            let target = self.session().map_or((1, 1), |s| (s.document.width, s.document.height));
            self.surface_popup = None;
            self.run_from_surface(&plugin, &action, &values, Vec::new(), SurfaceRun { surface: Surface::Layer, target, exact: false, resolution: 72.0 });
        } else if close {
            self.surface_popup = None;
        }
    }
}
```

`clicked_elsewhere` on the Area response closes it when the user clicks outside. If a click on the ✦ button that opened it closes it on the same frame, open the popover on the frame after the click instead: the footer sets `actions.ai_layer` and `apply_layer_actions` sets `surface_popup`.

`plugins.rs` `PluginState`: add `pub surface_values: std::collections::HashMap<(String, String), Map<String, Value>>`. `mod.rs` `EditorApp`: add `surface_popup: Option<surfaces::SurfacePopup>`, initialised to `None`. In the frame layout, after `self.plugin_action_dialog(ctx);`, call `self.surface_popups(ctx);`.

`layers.rs`:
- `Actions` gets `ai_layer: Option<(String, String, egui::Pos2)>`.
- In `layer_footer`, after the filter button:

```rust
                        let layer_actions = self.surface_actions(Surface::Layer);
                        if !layer_actions.is_empty() {
                            let button = icons::action_button(ui, "ai_layer");
                            button.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tr("New layer with AI")));
                            let button = button.on_hover_text(tr("New layer with AI"));
                            let anchor = button.rect.left_top();
                            if layer_actions.len() == 1 {
                                if button.clicked() {
                                    let (plugin, action, _) = layer_actions[0].clone();
                                    actions.ai_layer = Some((plugin, action, anchor));
                                }
                            } else {
                                egui::Popup::menu(&button).show(|ui| {
                                    for (plugin, action, label) in &layer_actions {
                                        if ui.button(label).clicked() {
                                            actions.ai_layer = Some((plugin.clone(), action.clone(), anchor));
                                            ui.close();
                                        }
                                    }
                                });
                            }
                        }
```

`layer_footer` takes `&self`, so `surface_actions(&self)` is fine there. In `apply_layer_actions`:

```rust
        if let Some((plugin, action, anchor)) = actions.ai_layer {
            self.surface_popup = Some(super::surfaces::SurfacePopup::Layer { plugin, action, anchor });
        }
```

`zh-CN.tsv`: add `New layer with AI\t用 AI 新建图层`, `Advanced\t高级` (only if missing; check with grep first), `Generate\t生成` (if missing), and `is disabled\t已停用`.

- [ ] **Step 4: Run the test**

Run: `cargo test --locked --bin xuan -- ui::surfaces`
Expected: PASS. If "Generate" or "Advanced" is ambiguous on screen, use `has_role`.

- [ ] **Step 5: Commit**

```bash
cargo test --locked --bin xuan 2>&1 | grep 'test result'
cargo fmt && cargo clippy --locked --all-targets
git add src/app assets
git commit -m "feat(app): New layer with AI button with a prompt-first popover"
```

---

### Task 5: The AI Region tool in tool mode: boxes, drawing and options bar

**Files:**
- Create: `src/app/ai_regions.rs`
- Modify:
  - `src/app/mod.rs`: `Session` gets `ai_boxes: Vec<AiBox>` and `ai_selected: Option<usize>`, initialised in `Session::new`; the `Tool::Region` label becomes `tr("AI Region")` and its hint mentions boxes; `mod ai_regions;`
  - `src/app/plugins.rs`: `region_tool_available`
  - `src/app/canvas.rs`: gesture start (~1373), release (~1876), click (~1205), drawing (~686)
  - `src/app/panels.rs`: a `tool_options` arm for `Tool::Region`
  - `src/app/tests/plugins.rs`: the test from the previous fix, `the_region_tool_is_offered_only_to_actions_with_regions`, is updated
- Test: `src/app/tests/ui.rs`, `mod surfaces`

**Interfaces:**
- Consumes: `surface_actions(Surface::Region)` (Task 4) and `Region` (jobs.rs).
- Produces:
  - `pub(super) struct AiBox { pub region: Region, pub plugin: String, pub action: String, pub values: Map<String, Value> }`, where `region.fields` holds the region fields.
  - On `EditorApp`: `pub(super) fn add_ai_box(&mut self, start: Point, end: Point)` (boxes are drawn in `canvas.rs` with the same fill, stroke and number badge as dialog regions), `pub(super) fn add_ai_box_from_selection(&mut self)`, `pub(super) fn select_ai_box_at(&mut self, point: Point) -> bool`, `pub(super) fn delete_ai_box(&mut self, index: usize)`, `pub(super) fn clear_ai_boxes(&mut self)`.

- [ ] **Step 1: Write the failing tests** (`ui.rs` `mod surfaces`)

```rust
    /// Adds a region action ("Edit") to the plugin from `install`.
    fn install_region(ui: &mut UiTest, dir: &std::path::Path) {
        std::fs::write(dir.join("plugin.toml"), r#"
[plugin]
id = "ai"
name = "AI"
command = ["sh", "-c", "cat > /dev/null"]

[permissions]
document = "edit"

[[actions]]
id = "edit"
label = "Precise Edit…"
surfaces = ["region"]
verb = "Edit"
source = { crop_to_regions = true }

[[actions.inputs]]
id = "regions"
type = "regions"
fields = [{ id = "desc", type = "text", label = "Instruction" }]
"#).unwrap();
        ui.app_mut().install_plugins(vec![xuan::plugins::Manifest::load(dir).unwrap()], vec![]);
        ui.app_mut().grant_plugin("ai", true);
    }

    #[test]
    fn ai_region_boxes_belong_to_their_document_and_are_not_edits() {
        let dir = tempfile::tempdir().unwrap();
        let mut ui = UiTest::with_document();
        install_region(&mut ui, dir.path());
        assert!(ui.app().region_tool_available());
        let history = ui.app().session().unwrap().history.names().count();
        ui.app_mut().add_ai_box(Point::new(2.0, 2.0), Point::new(12.0, 10.0));
        ui.app_mut().add_ai_box(Point::new(0.5, 0.5), Point::new(1.0, 1.0)); // too small: ignored
        let session = ui.app().session().unwrap();
        assert_eq!(session.ai_boxes.len(), 1);
        assert_eq!(session.ai_boxes[0].action, "edit");
        assert_eq!(session.ai_boxes[0].region.fields["desc"], serde_json::json!(""));
        // Review focus 4: no undo steps, not modified.
        assert_eq!(session.history.names().count(), history);
        assert!(!session.history.modified());
        // Review focus 3: another document has its own (no) boxes, and the popover closes.
        ui.app_mut().surface_popup = Some(crate::app::surfaces::SurfacePopup::Region { document: ui.app().session().unwrap().document.id, index: 0 });
        ui.app_mut().dimensions = [10, 10];
        ui.app_mut().new_document();
        ui.settle();
        assert!(ui.app().session().unwrap().ai_boxes.is_empty());
        assert!(ui.app().surface_popup.is_none());
        ui.app_mut().current = 0;
        assert_eq!(ui.app().session().unwrap().ai_boxes.len(), 1);
        ui.app_mut().delete_ai_box(0);
        assert!(ui.app().session().unwrap().ai_boxes.is_empty());
    }
```

Use the real method names: check `history.names()`, and a modified check such as `history.is_modified()` or `modified()`, in `src/history.rs`.

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test --locked --bin xuan -- ai_region_boxes`
Expected: compile errors, because `add_ai_box` and `ai_boxes` don't exist yet.

- [ ] **Step 3: Implement**

`src/app/ai_regions.rs`:

```rust
//! The AI Region tool without an open dialog: boxes on the canvas, each with
//! the region action (verb) and values it will run with.
use serde_json::{Map, Value};
use xuan::{document::Point, i18n::tr, plugins::{jobs::Region, manifest::{InputKind, Surface}}};

use super::EditorApp;

/// A box drawn with the AI Region tool.
#[derive(Clone, Debug)]
pub(super) struct AiBox {
    /// Document pixels; `fields` hold the action's region fields.
    pub region: Region,
    pub plugin: String,
    pub action: String,
    /// The action's other inputs.
    pub values: Map<String, Value>,
}

impl EditorApp {
    /// A new box with the first region action and its defaults.
    fn new_ai_box(&self, mut region: Region) -> Option<AiBox> {
        let (plugin, action, _) = self.surface_actions(Surface::Region).into_iter().next()?;
        let spec = self.plugins.manifest(&plugin)?.action(&action)?.clone();
        let regions = spec.regions_input()?;
        for field in &regions.fields {
            region.fields.insert(field.id.clone(), field.initial());
        }
        let values = spec.inputs.iter().filter(|i| i.kind != InputKind::Regions)
            .map(|i| (i.id.clone(), i.initial())).collect();
        Some(AiBox { region, plugin, action, values })
    }

    pub(super) fn add_ai_box(&mut self, start: Point, end: Point) {
        let (x, y) = (start.x.min(end.x), start.y.min(end.y));
        let (width, height) = ((start.x - end.x).abs(), (start.y - end.y).abs());
        if width < 2.0 || height < 2.0 {
            return;
        }
        if let Some(ai_box) = self.new_ai_box(Region::rect(x, y, width, height)) {
            self.push_ai_box(ai_box);
        }
    }

    pub(super) fn add_ai_box_from_selection(&mut self) {
        let Some(selection) = self.session().and_then(|s| s.document.selection.clone()) else {
            self.status = tr("Make a selection first").into();
            return;
        };
        if let Some(ai_box) = Region::from_mask(selection).and_then(|r| self.new_ai_box(r)) {
            self.push_ai_box(ai_box);
        }
    }

    fn push_ai_box(&mut self, ai_box: AiBox) {
        let Some(session) = self.session_mut() else { return };
        session.ai_boxes.push(ai_box);
        let index = session.ai_boxes.len() - 1;
        session.ai_selected = Some(index);
        let document = session.document.id;
        self.surface_popup = Some(super::surfaces::SurfacePopup::Region { document, index });
    }

    /// Select the topmost box under `point` and open its popover.
    pub(super) fn select_ai_box_at(&mut self, point: Point) -> bool {
        let Some(session) = self.session_mut() else { return false };
        let found = session.ai_boxes.iter().rposition(|b| b.region.contains(point));
        session.ai_selected = found;
        let document = session.document.id;
        self.surface_popup = found.map(|index| super::surfaces::SurfacePopup::Region { document, index });
        found.is_some()
    }

    pub(super) fn delete_ai_box(&mut self, index: usize) {
        if let Some(session) = self.session_mut() && index < session.ai_boxes.len() {
            session.ai_boxes.remove(index);
            session.ai_selected = None;
        }
        self.surface_popup = None;
    }

    pub(super) fn clear_ai_boxes(&mut self) {
        if let Some(session) = self.session_mut() {
            session.ai_boxes.clear();
            session.ai_selected = None;
        }
        self.surface_popup = None;
    }
}
```

`plugins.rs` `region_tool_available`:

```rust
    /// Whether the tool rail offers the Region tool: while the open action has
    /// a regions input, or (as the AI Region tool) when a plugin offers a
    /// region action and a document is open.
    pub(super) fn region_tool_available(&self) -> bool {
        let dialog = self.plugins.action.as_ref().is_some_and(|edit| {
            (self.plugins.manifest(&edit.plugin)).and_then(|m| m.action(&edit.action))
                .is_some_and(|a| a.regions_input().is_some())
        });
        dialog || (self.session().is_some() && !self.surface_actions(Surface::Region).is_empty())
    }
```

Update `the_region_tool_is_offered_only_to_actions_with_regions`. Its manifest has no region surface, so its assertions still hold. Add one assertion: with a `region`-surface action installed and no dialog open, `region_tool_available()` is true.

`canvas.rs`:
- At gesture start (~1373), replace `if self.plugins.action.is_none() { return; }` with `if self.plugins.action.is_none() && !self.region_tool_available() { return; }`.
- At release (~1876):

```rust
        if tool == Tool::Region {
            if self.plugins.action.is_some() {
                self.add_region(gesture.start, gesture.last);
            } else {
                self.add_ai_box(gesture.start, gesture.last);
            }
            return;
        }
```

- On click (~1205): `Tool::Region => if self.plugins.action.is_some() { self.select_region_at(point) } else { self.select_ai_box_at(point); },`
- When drawing (~686), after the `if let Some(edit) = &self.plugins.action { … }` block, draw tool-mode boxes when no dialog is open. Copy that block's painting code (fill, stroke, number badge), but loop over `session.ai_boxes` with `session.ai_selected`.

`panels.rs` `tool_options`: add an arm:

```rust
                                    Tool::Region if self.plugins.action.is_none() => {
                                        if super::widgets::button(ui, tr("Use Selection")).clicked() {
                                            self.add_ai_box_from_selection();
                                        }
                                        if super::widgets::button(ui, tr("Clear")).clicked() {
                                            self.clear_ai_boxes();
                                        }
                                    }
```

Keyboard: when `self.tool == Tool::Region`, no dialog is open and there is a selected box, Delete or Backspace calls `delete_ai_box`. Put this next to the canvas key handling. Find where Delete is handled for selections (`canvas.rs` or `mod.rs`) and check this first.

Review focus 3: when the current document changes, close a region popover for another document. In `surface_popups` (Task 4), at the top:

```rust
        if let Some(SurfacePopup::Region { document, .. }) = &self.surface_popup
            && self.session().map(|s| s.document.id) != Some(*document)
        {
            self.surface_popup = None;
        }
```

`mod.rs`: the `Tool::Region` label becomes `tr("AI Region")`. Its hint becomes `tr("Drag a box and say what to do there · Click a box to change it · Delete removes it")`. When a dialog is open, keep the old hint: `hint()` has no app access, so select the hint where it is drawn in `panels.rs`. Add the strings to `zh-CN.tsv`.

- [ ] **Step 4: Run the tests**

Run: `cargo test --locked --bin xuan -- ai_region_boxes the_region_tool region`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo fmt && cargo clippy --locked --all-targets
git add src/app assets/locales/zh-CN.tsv
git commit -m "feat(app): the AI Region tool draws boxes without a dialog"
```

---

### Task 6: The region popover and Generate (verbs, grouping)

**Files:**
- Modify: `src/app/ai_regions.rs` (`region_popup`, `generate_ai_boxes`, the verb change), `src/app/surfaces.rs` (calls `region_popup`, which is already stubbed in Task 4)
- Test: `src/app/tests/plugins.rs` (`mod unix`; it runs jobs with the mock plugin)

**Interfaces:**
- Consumes: `AiBox` (Task 5); `run_from_surface` and `SurfaceRun` (Task 3); `surface_form` (Task 4). `region_limit` in `plugins.rs` must become `pub(super)`.
- Produces:
  - `pub(super) fn region_popup(&mut self, ctx: &egui::Context)`.
  - `pub(super) fn generate_ai_boxes(&mut self, index: usize) -> usize`: the number of jobs started.
  - `pub(super) fn set_ai_box_action(&mut self, index: usize, plugin: &str, action: &str)`.

- [ ] **Step 1: Write the failing tests** (`tests/plugins.rs` `mod unix`)

```rust
#[test]
fn ai_region_boxes_run_grouped_or_one_by_one_and_are_removed() {
    let dir = tempfile::tempdir().unwrap();
    let (context, mut app) = app();
    let fixture = dir.path().join("fixture.png");
    RgbaImage::new(2, 2).save(&fixture).unwrap();
    std::fs::write(dir.path().join("plugin.sh"), script(&fixture)).unwrap();
    // "echo" is a multi-box Edit; "one" takes one box (Add).
    let manifest = MANIFEST.replace("[[actions]]\nid = \"echo\"", "[[actions]]\nid = \"echo\"\nsurfaces = [\"region\"]\nverb = \"Edit\"")
        + "\n[[actions]]\nid = \"one\"\nlabel = \"One…\"\nsurfaces = [\"region\"]\nverb = \"Add\"\nsource = { crop_to_regions = true }\n\n[[actions.inputs]]\nid = \"regions\"\ntype = \"regions\"\nmax = 1\nfields = [{ id = \"desc\", type = \"text\" }]\n";
    std::fs::write(dir.path().join("plugin.toml"), manifest).unwrap();
    app.install_plugins(vec![Manifest::load(dir.path()).unwrap()], vec![]);
    app.grant_plugin("mock", true);
    app.dimensions = [64, 48];
    app.new_document();
    app.command("fill_fg");
    frame(&context, &mut app);
    for (x, desc) in [(2.0, "hat"), (20.0, "scarf"), (40.0, "")] {
        app.add_ai_box(Point::new(x, 2.0), Point::new(x + 10.0, 12.0));
        let session = app.session_mut().unwrap();
        session.ai_boxes.last_mut().unwrap().region.fields.insert("desc".into(), desc.into());
    }
    // The empty third box is not sent; the two Edit boxes go in one job.
    assert_eq!(app.generate_ai_boxes(0), 1);
    assert_eq!(app.plugins.jobs.len(), 1);
    assert_eq!(app.session().unwrap().ai_boxes.len(), 1);
    // Switch the remaining box to Add and give it text: one job for it.
    app.set_ai_box_action(0, "mock", "one");
    app.session_mut().unwrap().ai_boxes[0].region.fields.insert("desc".into(), "a cat".into());
    assert_eq!(app.generate_ai_boxes(0), 1);
    assert!(app.session().unwrap().ai_boxes.is_empty());
    run_until(&context, &mut app, |app| app.plugins.jobs.is_empty() && app.dialog == Some(Dialog::PluginProposal));
    let received = std::fs::read_to_string(dir.path().join("received.log")).unwrap();
    let runs: Vec<_> = received.lines().filter(|l| l.contains("\"action/run\"")).collect();
    assert_eq!(runs.len(), 2);
    assert!(runs[0].contains("\"action\":\"echo\"") && runs[0].contains("hat") && runs[0].contains("scarf"));
    assert!(runs[1].contains("\"action\":\"one\"") && runs[1].contains("\"surface\":\"region\""));
}
```

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test --locked --bin xuan -- ai_region_boxes_run`
Expected: compile errors, because `generate_ai_boxes` doesn't exist yet.

- [ ] **Step 3: Implement** (in `ai_regions.rs`)

```rust
    /// Change a box's verb: the new action's defaults, keeping the box's
    /// first text field when both actions have one (the prompt).
    pub(super) fn set_ai_box_action(&mut self, index: usize, plugin: &str, action: &str) {
        let Some(spec) = self.plugins.manifest(plugin).and_then(|m| m.action(action)).cloned() else { return };
        let Some(regions) = spec.regions_input().cloned() else { return };
        let Some(session) = self.session_mut() else { return };
        let Some(ai_box) = session.ai_boxes.get_mut(index) else { return };
        let prompt = ai_box.region.fields.values().find_map(|v| v.as_str().map(str::to_owned));
        ai_box.region.fields.clear();
        for field in &regions.fields {
            ai_box.region.fields.insert(field.id.clone(), field.initial());
        }
        if let (Some(prompt), Some(field)) = (prompt, regions.fields.iter().find(|f| matches!(f.kind, InputKind::Text | InputKind::Multiline))) {
            ai_box.region.fields.insert(field.id.clone(), prompt.into());
        }
        ai_box.values = spec.inputs.iter().filter(|i| i.kind != InputKind::Regions).map(|i| (i.id.clone(), i.initial())).collect();
        (ai_box.plugin, ai_box.action) = (plugin.into(), action.into());
    }

    /// Run the box at `index`: with every other box of the same action when
    /// that action takes several (Precise Edit), else alone. Boxes without
    /// text are left. Sent boxes are removed. Returns the jobs started.
    pub(super) fn generate_ai_boxes(&mut self, index: usize) -> usize {
        let Some(session) = self.session() else { return 0 };
        let Some(chosen) = session.ai_boxes.get(index).cloned() else { return 0 };
        let Some(spec) = self.plugins.manifest(&chosen.plugin).and_then(|m| m.action(&chosen.action)).cloned() else { return 0 };
        let Some(input) = spec.regions_input().cloned() else { return 0 };
        let has_text = |b: &AiBox| b.region.fields.values().any(|v| v.as_str().is_some_and(|t| !t.trim().is_empty()));
        let together = super::plugins::region_limit(&input) > 1;
        let picked: Vec<usize> = session.ai_boxes.iter().enumerate()
            .filter(|(i, b)| if together { b.plugin == chosen.plugin && b.action == chosen.action } else { *i == index })
            .filter(|(_, b)| has_text(b))
            .map(|(i, _)| i).collect();
        if picked.is_empty() {
            return 0;
        }
        let boxes: Vec<AiBox> = picked.iter().map(|i| session.ai_boxes[*i].clone()).collect();
        let groups: Vec<Vec<AiBox>> = if together { vec![boxes] } else { boxes.into_iter().map(|b| vec![b]).collect() };
        let mut started = 0;
        for group in groups {
            let (x0, y0) = group.iter().fold((f32::MAX, f32::MAX), |(x, y), b| (x.min(b.region.x), y.min(b.region.y)));
            let (x1, y1) = group.iter().fold((f32::MIN, f32::MIN), |(x, y), b| (x.max(b.region.x + b.region.width), y.max(b.region.y + b.region.height)));
            let target = (((x1 - x0).ceil() as u32).max(1), ((y1 - y0).ceil() as u32).max(1));
            let run = super::surfaces::SurfaceRun { surface: Surface::Region, target, exact: false, resolution: 72.0 };
            let regions = group.iter().map(|b| b.region.clone()).collect();
            if self.run_from_surface(&chosen.plugin, &chosen.action, &group[0].values, regions, run) {
                started += 1;
            }
        }
        if let Some(session) = self.session_mut() {
            for i in picked.iter().rev() {
                session.ai_boxes.remove(*i);
            }
            session.ai_selected = None;
        }
        self.surface_popup = None;
        started
    }
```

`run_plugin_action` closes the action after each job starts (`close_plugin_action`), so calling `run_from_surface` in a loop starts one job per group. If a group hits a prompt (consent or permission), stop the loop and leave the remaining boxes in place: check `self.dialog.is_some()` after each call and `break`.

The popover (`region_popup`):

```rust
    pub(super) fn region_popup(&mut self, ctx: &egui::Context) {
        let Some(super::surfaces::SurfacePopup::Region { index, .. }) = self.surface_popup else { return };
        let Some(session) = self.session() else { return };
        let Some(ai_box) = session.ai_boxes.get(index).cloned() else { self.surface_popup = None; return };
        let (Some(viewport), zoom, pan) = (self.canvas_rect, session.zoom, session.pan) else { return };
        let size = egui::vec2(session.document.width as f32, session.document.height as f32);
        let origin = super::canvas::image_origin(viewport, size, zoom, pan);
        let anchor = origin + egui::vec2(ai_box.region.x + ai_box.region.width, ai_box.region.y) * zoom + egui::vec2(8.0, 0.0);
        let verbs = self.surface_actions(Surface::Region);
        let mut change = None;
        let mut generate = false;
        let response = egui::Area::new(egui::Id::new(("region_popup", index)))
            .order(egui::Order::Foreground).fixed_pos(anchor).constrain(true)
            .show(ctx, |ui| {
                super::theme::frame(&ctx.palette()).inner_margin(egui::Margin::same(12)).show(ui, |ui| {
                    ui.set_width(300.0);
                    if verbs.len() > 1 {
                        ui.horizontal(|ui| {
                            for (plugin, action, _) in &verbs {
                                let verb = self.plugins.manifest(plugin).and_then(|m| m.action(action)).map(|a| a.verb.clone()).unwrap_or_default();
                                if ui.selectable_label(ai_box.plugin == *plugin && ai_box.action == *action, verb).clicked() {
                                    change = Some((plugin.clone(), action.clone()));
                                }
                            }
                        });
                    }
                    let ready = self.ai_box_form(ui, index);
                    let count = self.boxes_sent_together(index);
                    let label = if count > 1 { format!("{} {} {}", self.verb_of(&ai_box), count, tr("boxes")) } else { tr("Generate").to_owned() };
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        generate = ui.add_enabled(ready, super::widgets::Button::new(label).primary()).clicked()
                            || (ready && ui.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)));
                        if super::widgets::button(ui, tr("Delete")).clicked() {
                            self.delete_ai_box(index);
                        }
                    });
                });
            });
        if let Some((plugin, action)) = change {
            self.set_ai_box_action(index, &plugin, &action);
        } else if generate {
            self.generate_ai_boxes(index);
        } else if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || response.response.clicked_elsewhere() && self.tool != super::Tool::Region {
            self.surface_popup = None;
        }
    }
```

Helpers in `ai_regions.rs`:

```rust
    /// The box's region fields and its action's inputs: basic first, then a
    /// collapsed "Advanced" section. Returns whether its first text field
    /// has text, which Generate needs.
    fn ai_box_form(&mut self, ui: &mut egui::Ui, index: usize) -> bool {
        let Some(ai_box) = self.session().and_then(|s| s.ai_boxes.get(index)).cloned() else { return false };
        let Some(spec) = self.plugins.manifest(&ai_box.plugin).and_then(|m| m.action(&ai_box.action)).cloned() else { return false };
        let fields: Vec<_> = spec.regions_input().map(|r| r.fields.clone()).unwrap_or_default()
            .into_iter().filter(|f| f.shown_on(Surface::Region)).collect();
        let inputs: Vec<_> = spec.inputs.iter()
            .filter(|i| i.kind != InputKind::Regions && i.shown_on(Surface::Region)).cloned().collect();
        let Some(session) = self.session_mut() else { return false };
        let Some(ai_box) = session.ai_boxes.get_mut(index) else { return false };
        let salt = ("ai_box", index);
        let mut ready = true;
        let mut first_text = true;
        for field in fields.iter().filter(|f| !f.advanced) {
            let value = ai_box.region.fields.entry(field.id.clone()).or_insert_with(|| field.initial());
            super::plugin_dialogs::input_widget(ui, field, value, (salt, "field"));
            if first_text && matches!(field.kind, InputKind::Text | InputKind::Multiline) {
                ready = value.as_str().is_some_and(|t| !t.trim().is_empty());
                first_text = false;
            }
        }
        for input in inputs.iter().filter(|i| !i.advanced) {
            let value = ai_box.values.entry(input.id.clone()).or_insert_with(|| input.initial());
            super::plugin_dialogs::input_widget(ui, input, value, (salt, "input"));
        }
        let advanced_fields: Vec<_> = fields.iter().filter(|f| f.advanced).collect();
        let advanced_inputs: Vec<_> = inputs.iter().filter(|i| i.advanced).collect();
        if !advanced_fields.is_empty() || !advanced_inputs.is_empty() {
            egui::CollapsingHeader::new(tr("Advanced")).id_salt((salt, "advanced")).default_open(false).show(ui, |ui| {
                for field in advanced_fields {
                    let value = ai_box.region.fields.entry(field.id.clone()).or_insert_with(|| field.initial());
                    super::plugin_dialogs::input_widget(ui, field, value, (salt, "field"));
                }
                for input in advanced_inputs {
                    let value = ai_box.values.entry(input.id.clone()).or_insert_with(|| input.initial());
                    super::plugin_dialogs::input_widget(ui, input, value, (salt, "input"));
                }
            });
        }
        ready
    }

    /// How many boxes Generate on box `index` sends in one job.
    fn boxes_sent_together(&self, index: usize) -> usize {
        let Some(session) = self.session() else { return 0 };
        let Some(chosen) = session.ai_boxes.get(index) else { return 0 };
        let together = (self.plugins.manifest(&chosen.plugin)).and_then(|m| m.action(&chosen.action))
            .and_then(|a| a.regions_input()).is_some_and(|r| super::plugins::region_limit(r) > 1);
        if !together {
            return 1;
        }
        session.ai_boxes.iter()
            .filter(|b| b.plugin == chosen.plugin && b.action == chosen.action)
            .filter(|b| b.region.fields.values().any(|v| v.as_str().is_some_and(|t| !t.trim().is_empty())))
            .count()
    }

    fn verb_of(&self, ai_box: &AiBox) -> String {
        (self.plugins.manifest(&ai_box.plugin)).and_then(|m| m.action(&ai_box.action))
            .map(|a| a.verb.clone()).unwrap_or_default()
    }
```

Because `surface_popups` (Task 4) calls `self.region_popup(ctx)` for `SurfacePopup::Region`, nothing else needs wiring. Clicking on the canvas with the tool active selects or adds boxes, so it doesn't count as "elsewhere".

Add `boxes`, `Delete` and `Use Selection` to `zh-CN.tsv` if they're missing.

- [ ] **Step 4: Run the tests**

Run: `cargo test --locked --bin xuan -- ai_region`
Expected: PASS.

- [ ] **Step 5: Add a UI test that the popover opens with its verb, and commit**

In `ui.rs` `mod surfaces`, using `install_region` from Task 5:

```rust
    #[test]
    fn drawing_an_ai_box_opens_its_popover() {
        let dir = tempfile::tempdir().unwrap();
        let mut ui = UiTest::with_document();
        install_region(&mut ui, dir.path());
        ui.app_mut().set_tool(Tool::Region);
        ui.app_mut().add_ai_box(Point::new(2.0, 2.0), Point::new(12.0, 10.0));
        ui.settle();
        assert!(ui.has("Instruction"));
        assert!(!ui.enabled("Generate"));
    }
```

```bash
cargo test --locked --bin xuan 2>&1 | grep 'test result'
cargo fmt && cargo clippy --locked --all-targets
git add src/app assets/locales/zh-CN.tsv
git commit -m "feat(app): AI Region popovers run boxes by verb, grouped for multi-box actions"
```

---

### Task 7: New Image, the Generate tab

**Files:**
- Modify:
  - `src/app/dialogs.rs`: `size_dialog`
  - `src/app/mod.rs`: `EditorApp` gets `new_image_generate: bool` and `new_image_exact: bool`, both `false`, plus `new_image_action: Option<(String, String)>`
- Test: `src/app/tests/ui.rs`, `mod surfaces`

**Interfaces:**
- Consumes: `surface_actions(Surface::Document)`, `surface_form` (Task 4); `run_from_surface` and `SurfaceRun` with `exact` and `resolution` (Task 3).
- Produces: the tab UI only.

- [ ] **Step 1: Write the failing test**

```rust
    #[test]
    fn new_image_offers_a_generate_tab_only_with_a_document_action() {
        let dir = tempfile::tempdir().unwrap();
        let mut ui = UiTest::new();
        ui.app_mut().command("new");
        ui.settle();
        assert!(!ui.has("Generate"));
        std::fs::write(dir.path().join("plugin.toml"), r#"
[plugin]
id = "ai"
name = "AI"
command = ["sh", "-c", "cat > /dev/null"]

[[actions]]
id = "new"
label = "Generate Image…"
kind = "generate"
surfaces = ["document"]
result = { into = "ask" }

[[actions.inputs]]
id = "prompt"
type = "multiline"
label = "Prompt"
"#).unwrap();
        ui.app_mut().install_plugins(vec![xuan::plugins::Manifest::load(dir.path()).unwrap()], vec![]);
        ui.settle();
        ui.click("Generate");
        assert!(ui.has("Prompt"));
        assert!(ui.has("Exact size"));
        assert!(ui.has("Width"));
        assert!(!ui.enabled("Generate image"));
    }
```

The tab is called "Generate" and the primary button "Generate image", so the two never share a label.

- [ ] **Step 2: Run it to see it fail**

Run: `cargo test --locked --bin xuan -- new_image_offers`
Expected: FAIL.

- [ ] **Step 3: Implement** (in `size_dialog`, only when `dialog == Dialog::New`)

At the top of the window body:

```rust
                let document_actions = if dialog == Dialog::New { self.surface_actions(Surface::Document) } else { Vec::new() };
                if !document_actions.is_empty() {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(!self.new_image_generate, tr("Blank")).clicked() { self.new_image_generate = false; }
                        if ui.selectable_label(self.new_image_generate, tr("Generate")).clicked() { self.new_image_generate = true; }
                    });
                } else {
                    self.new_image_generate = false;
                }
```

Keep the Width, Height and Resolution fields shared. When `self.new_image_generate` is true:
- Replace the description with `tr("Describe the image; it is made at least this size.")`.
- After the resolution row, add:
  - an action picker when `document_actions.len() > 1`, a `ComboBox` setting `new_image_action`;
  - `let ready = self.surface_form(ui, &plugin, &action, Surface::Document, "new_image");`
  - `widgets::checkbox(ui, &mut self.new_image_exact, tr("Exact size")).on_hover_text(tr("Make the canvas exactly W × H; the image covers it and can be moved"));`
- Hide the "Transparent canvas · sRGB" line.
- The primary button becomes `tr("Generate image")`, enabled with `valid.is_ok() && ready`.

On apply in generate mode:

```rust
            let (plugin, action) = self.new_image_action.clone().filter(|(p, a)| document_actions.iter().any(|(q, b, _)| q == p && b == a))
                .or_else(|| document_actions.first().map(|(p, a, _)| (p.clone(), a.clone())))
                .unwrap();
            let values = self.plugins.surface_values.get(&(plugin.clone(), action.clone())).cloned().unwrap_or_default();
            let run = SurfaceRun { surface: Surface::Document, target: (self.dimensions[0], self.dimensions[1]), exact: self.new_image_exact, resolution: self.resolution };
            if self.run_from_surface(&plugin, &action, &values, Vec::new(), run) {
                self.dialog = None;
            }
```

Check `run_plugin_action` for documents: with no document open, `document` is `None`, so the job's document is `Uuid::nil()` and the result goes to a new document through `new_documents`. Make sure `apply_job_result` accepts a nil document for `ResultInto::Document`, since it already handles `job.prepared.export.is_none() && session.is_none()`. If a document *is* open, `into` is still `Document` (Task 3 sets it), so the result opens a new tab.

The new document's title: in Task 3's `new_documents` loop, the `name` comes from the output's `name`. The plugin names it after the prompt (Task 10).

Add the strings `Blank`, `Generate image`, `Exact size`, the hint text and the description to `zh-CN.tsv`.

- [ ] **Step 4: Run the tests**

Run: `cargo test --locked --bin xuan -- new_image_offers ui`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
cargo test --locked --bin xuan 2>&1 | grep 'test result'
cargo fmt && cargo clippy --locked --all-targets
git add src/app assets/locales/zh-CN.tsv
git commit -m "feat(app): New Image can generate the image, optionally at an exact size"
```

---

### Task 8: Host docs

**Files:**
- Modify: `docs/PLUGINS.md` (new section "Surfaces" after "Providers"; `fit` in "Sources and results"; the manifest example), `docs/USAGE.md` (Plugins section)

- [ ] **Step 1: Write the docs.** `PLUGINS.md` "Surfaces" covers:
  - the manifest fields, with the TOML from the spec;
  - each surface's validation rule, as listed in Global Constraints;
  - `inputs.surface` and `inputs.target`, with the target per surface;
  - the popovers, Advanced and remembered values;
  - AI Region box grouping: actions whose `regions` input takes several boxes get them all, those with `max = 1` run per box;
  - Exact size: a W×H canvas with a covering layer, no crop.

  In "Sources and results", document `fit: "cover"` next to `fit: "source"`, including the refusal for `replace` and for actions without a source. Add `surfaces` and `verb` to the manifest example with a comment, and `advanced = true` to one input. `USAGE.md`: one paragraph on the three entry points, and on Advanced being collapsed.

- [ ] **Step 2: Check that the manifest example still parses.** Run `cargo test --locked --lib plugins::manifest::tests::parses_the_documented_manifest`. That test uses its own `EXAMPLE`, so also check that the doc's TOML block parses: paste it into a scratch test or a `python3 -c 'import tomllib…'` call.

- [ ] **Step 3: Commit**

```bash
git add docs/PLUGINS.md docs/USAGE.md
git commit -m "docs(plugins): surfaces, advanced inputs and fit cover"
```

---

### Task 9: Plugin, the size rule

**Files:**
- Modify: `plugins/comfy-cloud/recipes.py` (`Recipe.size_rule`, `cover_size`, `preset_at_least`, `choose_size`, `apply` uses `values["target"]`), `plugins/comfy-cloud/test_recipes.py`

**Interfaces:**
- Produces:
  - `Recipe.size_rule: str`, one of `"custom"`, `"presets"`, `"source"` or `"none"` (the default).
  - `cover_size(target, spec_w, spec_h, step=0) -> (w, h, under: bool)`.
  - `preset_at_least(options, target) -> (label, under: bool)` or `None`.
  - `apply(entry, recipe, values)`: when `values["target"]` is set, it uses the size rule instead of aspect, tier or match_size, and records `values["_note"]` when the result is under the target.

- [ ] **Step 1: Write the failing tests**

```python
class SizeRuleTest(unittest.TestCase):
    W = ["INT", {"min": 480, "max": 3840, "step": 16}]
    S = ["INT", {"min": 1024, "max": 4514, "step": 2}]

    def test_custom_renders_exactly_or_just_above_never_under(self):
        self.assertEqual(cover_size((1500, 1000), self.W, self.W, 16), (1504, 1008, False))
        self.assertEqual(cover_size((2000, 1000), self.S, self.S, 2), (2048, 1024, False))  # min side scaled up evenly
        self.assertEqual(cover_size((300, 200), self.W, self.W, 16), (720, 480, False))
        w, h, under = cover_size((8000, 4000), self.W, self.W, 16)
        self.assertTrue(under and w <= 3840 and h <= 3840 and abs(w / h - 2) < 0.02)

    def test_presets_take_the_smallest_big_enough(self):
        options = ["auto", "(1K) 1024x1024 (1:1)", "(2K) 2048x2048 (1:1)", "(1K) 1280x720 (16:9)", "(2K) 2560x1440 (16:9)"]
        self.assertEqual(preset_at_least(options, (1000, 1000)), ("(1K) 1024x1024 (1:1)", False))
        self.assertEqual(preset_at_least(options, (1100, 1100)), ("(2K) 2048x2048 (1:1)", False))
        self.assertEqual(preset_at_least(options, (1920, 1080)), ("(2K) 2560x1440 (16:9)", False))
        self.assertEqual(preset_at_least(options, (4000, 4000)), ("(2K) 2048x2048 (1:1)", True))

    def test_apply_uses_the_target_for_every_model(self):
        cases = {"seedream-pro": ("ByteDanceSeedreamNodeV3", "model.width", 2048),
                 "gpt-flare-layer": ("OpenAIGPTImageNodeV2", "model.custom_width", 2048),
                 "ideogram": ("IdeogramTextToImageApi", "model.size", "(2K) 2048x2048 (1:1)")}
        for recipe_id, (kind, key, expected) in cases.items():
            with self.subTest(recipe=recipe_id):
                recipe = RECIPES[recipe_id]
                graph = apply(entry_for(recipe), recipe, {"prompt": "x", "seed": 1, "target": (2048, 2048)})
                self.assertEqual(node_of(graph, kind)["inputs"][key], expected)
```

- [ ] **Step 2: Run them to see them fail**

Run: `python3 -m unittest plugins/comfy-cloud/test_recipes.py` (from the repo root use `-s plugins/comfy-cloud`).
Expected: ImportError for `cover_size`.

- [ ] **Step 3: Implement** (in `recipes.py`)

```python
def cover_size(target, spec_w, spec_h, step=0):
    """The smallest size of the target's shape the model accepts that is at
    least ``target``; ``under`` when the model's maximum is smaller."""
    ow, oh = (spec_w[1] if len(spec_w) > 1 else {}), (spec_h[1] if len(spec_h) > 1 else {})
    step = step or max(int(ow.get("step") or 1), int(oh.get("step") or 1), 1)
    lo_w, hi_w, lo_h, hi_h = ow.get("min", 1), ow.get("max", 1 << 16), oh.get("min", 1), oh.get("max", 1 << 16)
    w, h = float(max(target[0], 1)), float(max(target[1], 1))
    grow = max(lo_w / w, lo_h / h, 1.0)
    w, h = w * grow, h * grow
    shrink = min(hi_w / w, hi_h / h, 1.0)
    under = shrink < 1.0
    w, h = w * shrink, h * shrink
    up = math.floor if under else math.ceil
    return int(up(w / step) * step), int(up(h / step) * step), under


def preset_at_least(options, target, tolerance=0.03):
    """The smallest preset of the target's shape covering it, else the
    largest of the closest shape (``under``)."""
    want = target[0] / max(target[1], 1)
    sized = []
    for option in options or []:
        match = PRESET.match(option) if isinstance(option, str) else None
        if match:
            sized.append((option, int(match["w"]), int(match["h"])))
    if not sized:
        return None
    shape = [s for s in sized if abs(math.log((s[1] / s[2]) / want)) <= tolerance]
    big = [s for s in shape if s[1] >= target[0] and s[2] >= target[1]]
    if big:
        return min(big, key=lambda s: s[1] * s[2])[0], False
    pool = shape or sorted(sized, key=lambda s: abs(math.log((s[1] / s[2]) / want)))[:1]
    return max(pool, key=lambda s: s[1] * s[2])[0], True
```

`Recipe` gets `size_rule: str = "none"`:
- Seedream t2i recipes: `size_rule="custom"`, plus `match_size=(f"{SEEDREAM}.model.size_preset", f"{SEEDREAM}.model.width", f"{SEEDREAM}.model.height")` in `_SEEDREAM_T2I`.
- GPT layer recipes: `"custom"` (they already have `match_size` and `match_step=16`).
- `ideogram`: `"presets"` (uses `size`).
- `seedream-pro-edit`: `"custom"`.
- `ideogram-edit` and `ideogram-precise`: `"source"`.
- Split recipes: `"none"`.

`targets()` already includes `match_size`.

In `apply`, before the existing `size` and `match_size` handling:

```python
    target = values.get("target")
    if target and recipe.size_rule == "custom" and recipe.match_size:
        combo, width, height = recipe.match_size
        w, h, under = cover_size(target, specs.get(width) or ["INT", {}], specs.get(height) or ["INT", {}], recipe.match_step)
        options = (specs.get(combo) or [None, {}])[1].get("options") or []
        put(combo, "Custom" if "Custom" in options else "custom")
        put(width, w)
        put(height, h)
        if under:
            values["_note"] = f"{recipe.label} renders at most {w}×{h}; Xuan scales it up to {target[0]}×{target[1]}"
    elif target and recipe.size_rule == "presets" and recipe.size:
        chosen = preset_at_least((specs.get(recipe.size) or [None, {}])[1].get("options"), target)
        if chosen:
            put(recipe.size, chosen[0])
            if chosen[1]:
                values["_note"] = f"{recipe.label} renders at most {chosen[0]}; Xuan scales it up"
```

Make the existing `aspect`/`tier` and `source_size` branches `elif`s that only run without a target. Keep `apply`'s signature and return value: it returns the graph. Put the choice in one pure function that `apply` and `main.run` both call:

```python
def choose_size(entry, recipe, target):
    """The size inputs to set so the model renders at least ``target``,
    as ``({input: value}, note)``; the note says when it cannot."""
    specs = entry.get("specs") or {}
    if recipe.size_rule == "custom" and recipe.match_size:
        combo, width, height = recipe.match_size
        w, h, under = cover_size(target, specs.get(width) or ["INT", {}], specs.get(height) or ["INT", {}], recipe.match_step)
        options = (specs.get(combo) or [None, {}])[1].get("options") or []
        note = f"{recipe.label} renders at most {w}×{h}; Xuan scales it up to {target[0]}×{target[1]}" if under else None
        return {combo: "Custom" if "Custom" in options else "custom", width: w, height: h}, note
    if recipe.size_rule == "presets" and recipe.size:
        chosen = preset_at_least((specs.get(recipe.size) or [None, {}])[1].get("options"), target)
        if chosen:
            return {recipe.size: chosen[0]}, (f"{recipe.label} renders at most {chosen[0]}; Xuan scales it up" if chosen[1] else None)
    return {}, None
```

In `apply`: `if values.get("target"): sets, _ = choose_size(entry, recipe, values["target"]); for t, v in sets.items(): put(t, v)` replaces the two branches shown above. In `main.run`, after `apply`: `_, note = choose_size(version, recipe, values["target"]) if values.get("target") else ({}, None)`, and when there's a note, add it to `notes`. Add a test: `choose_size` for GPT with target 8000×4000 returns a note, and for 1500×1000 returns none.

- [ ] **Step 4: Run all the plugin tests**

Run: `python3 -m unittest discover -s plugins/comfy-cloud`
Expected: all pass, with the existing tests updated if `apply` now returns a pair.

- [ ] **Step 5: Commit**

```bash
git add plugins/comfy-cloud
git commit -m "feat(comfy-cloud): render at least the size a result covers, exactly when the model allows"
```

---

### Task 10: Plugin, surfaces, targets, cover placement, Generate in region and Fill region

**Files:**
- Modify: `plugins/comfy-cloud/plugin.toml`, `main.py`, `recipes.py` (the `mask` role, new recipes), `test_main.py`, `test_recipes.py`, `README.md`; regenerate `snapshots/`

**Interfaces:**
- Consumes: `cover_size` and `choose_size`/`apply` with a target (Task 9); `fit="cover"` in the SDK (Task 2).
- Produces:
  - actions `generate-in-region` (verb "Add") and `fill-region` (verb "Replace");
  - recipes `gpt-flare-fill` and `gpt-sunburst-fill`, which use the GPT templates with `mask=(f"{GPT}.model.mask",)`;
  - `ACTIONS` gains `"generate-in-region": ("gpt-flare-layer", "gpt-sunburst-layer")` and `"fill-region": ("gpt-flare-fill", "gpt-sunburst-fill")`.

- [ ] **Step 1: Write the failing tests** (`test_main.py`, in `Running`)

```python
    def test_surface_runs_render_at_least_the_target_and_cover_it(self):
        recipe = RECIPES["seedream-pro"]
        self.install(recipe, entry_for(recipe))
        job = FakeJob(self.tmp.name, inputs={"prompt": "a fox", "surface": "document", "target": {"width": 1600, "height": 900}})
        outputs = main.generate(job)
        node = next(n for n in self.server.submitted[0].values() if n["class_type"] == "ByteDanceSeedreamNodeV3")["inputs"]
        self.assertEqual((node["model.size_preset"], node["model.width"], node["model.height"]), ("Custom", 1820, 1024))
        self.assertEqual(outputs[0]["name"], "a fox")

    def test_fill_region_sends_the_box_as_a_mask_and_covers_the_crop(self):
        recipe = RECIPES["gpt-flare-fill"]
        self.install(recipe, entry_for(recipe))
        source = {"path": os.path.join(self.tmp.name, "crop.png"), "width": 300, "height": 200}
        regions = [{"index": 1, "x": 50, "y": 40, "width": 200, "height": 120, "mask": None, "fields": {"desc": "a cat"}}]
        job = FakeJob(self.tmp.name, "fill-region", {"regions": regions, "surface": "region", "target": {"width": 400, "height": 240}}, source)
        outputs = main.fill_region(job)
        graph = self.server.submitted[0]
        gpt = next(n for n in graph.values() if n["class_type"] == "OpenAIGPTImageNodeV2")["inputs"]
        self.assertEqual(graph[gpt["model.mask"][0]]["class_type"], "LoadImageMask")
        self.assertEqual(graph[gpt["model.images.image_1"][0]]["class_type"], "LoadImage")
        self.assertIn("a cat", gpt["prompt"])
        self.assertEqual(len(self.server.uploads), 2)  # the crop and the mask
        self.assertEqual(outputs[0]["fit"], "cover")

    def test_generate_in_region_lifts_the_object_out_of_the_crop(self):
        for recipe_id in ("gpt-flare-layer", main.LIFT_RECIPE):
            self.install(RECIPES[recipe_id], entry_for(RECIPES[recipe_id]))
        source = {"path": os.path.join(self.tmp.name, "crop.png"), "width": 300, "height": 200}
        regions = [{"index": 1, "x": 75, "y": 50, "width": 150, "height": 100, "mask": None, "fields": {"desc": "a kite"}}]
        job = FakeJob(self.tmp.name, "generate-in-region", {"regions": regions, "surface": "region", "target": {"width": 150, "height": 100}}, source)
        outputs = main.generate_in_region(job)
        scene = next(n for n in self.server.submitted[0].values() if n["class_type"] == "OpenAIGPTImageNodeV2")["inputs"]
        # The crop is twice the box, so GPT renders at least 300x200 doc px (min side 480).
        self.assertGreaterEqual(scene["model.custom_width"], 300)
        self.assertEqual(len(self.server.submitted), 2)
        self.assertEqual(outputs[0]["fit"], "cover")
```

The `FakeJob.regions` property must return `inputs["regions"]`. Update `FakeJob.__init__`: `self.regions = (inputs or {}).get("regions", [])`.

Recipe tests (`test_recipes.py`): add `gpt-flare-fill` to the list that must fit its template. `test_every_recipe_fits_its_template` covers this automatically once the recipe exists. Also check that `apply` with `mask` inserts `LoadImageMask` with `channel` `"red"`.

- [ ] **Step 2: Run them to see them fail**

Run: `python3 -m unittest discover -s plugins/comfy-cloud`
Expected: failures, because `fill_region` and the `gpt-flare-fill` recipe don't exist yet.

- [ ] **Step 3: Implement**

`recipes.py`:
- `Recipe` gets `mask: Tuple[str, ...] = ()`, and `targets()` includes it.
- In `apply`, after `reference`:

```python
    if recipe.mask and values.get("mask") is not None:
        numeric = [int(k) for k in graph if str(k).isdigit()]
        node_id = str(max(numeric, default=0) + 1)
        graph[node_id] = {"class_type": "LoadImageMask", "inputs": {"image": values["mask"], "channel": "red"}, "_meta": {"title": "Xuan mask"}}
        for target in recipe.mask:
            put(target, [node_id, 0], link=True)
```

- New recipes: `Recipe("gpt-flare-fill", "GPT Image 2.5 Flare", "api_openai_gpt_image_25_flare_t2i", **_GPT_FILL)` and the Sunburst equivalent. They use:

```python
_GPT_FILL = dict(_GPT_LAYER, mask=(f"{GPT}.model.mask",), fixed={f"{GPT}.model.background": "opaque", f"{GPT}.n": 1})
```

`main.py`:
- **Targets:**

```python
def target_of(job):
    target = job.inputs.get("target") or {}
    try:
        return int(target["width"]), int(target["height"])
    except (KeyError, TypeError, ValueError):
        return None
```

  - `generate`: `values["target"] = target_of(job)` when present. Name the output after the prompt, cut to 40 characters like `generate_layer`, when `job.inputs.get("surface") == "document"`.
  - `edit`, `generate_layer`, `precise_edit`: pass `target_of(job)`, falling back to the source size in document pixels (`source_size` divided by `job.source["scale"]`).
  - Every `fit_source=True` becomes `fit="cover"`.
  - The size note from Task 9 is already in `result.notes`, which every action returns as text outputs.

- **`run` gains `mask_path=None`.** It uploads like `reference_path`, under the role `"mask"`.

- **`fill_region`:**

```python
@plugin.action("fill-region")
def fill_region(job):
    recipe = recipe_for("fill-region", job.inputs.get("model"))
    regions = job.regions
    prompt = ((regions[0].get("fields") or {}).get("desc") or "").strip() if regions else ""
    if not prompt:
        raise RpcError(INVALID_PARAMS, "Describe what to paint in the box")
    width, height = source_size(job)
    mask = regions[0].get("mask") or rect_mask(job.path("mask.png"), (width, height), regions[0])
    values = {"prompt": FILL + prompt, "seed": seed_of(job), "quality": job.inputs.get("quality") or "medium",
              "target": scaled_target(job, (width, height))}
    result = run(job, recipe, values, reference_path=job.source_path, mask_path=mask)
    return [Job.image(p, name=layer_name(prompt), fit="cover", provenance=result.provenance) for p in result.images] + texts(result.notes)
```

  Its helpers:
  - `FILL = "Repaint only the white area of the mask with what is described below, matching the picture's colours, lighting, perspective and style.\n\n"`
  - `rect_mask(path, size, region)` writes a grey PNG with `xuan_plugin.encode_gray_png`. It is white inside `int(x)…int(x + width)` × `int(y)…int(y + height)` and black elsewhere. Build the rows with `bytes` multiplication, one row at a time.
  - `scaled_target(job, source)` turns the box target into the crop's size in document pixels: `target × (source.width / region.width)`, from `job.regions[0]`.
  - `layer_name` is the same prompt-to-name helper as `generate_layer`. Factor it out.

- **`generate_in_region`:** the Generate Layer reference path. It reuses `SCENE`, `LIFT` and `LIFT_RECIPE`, the scene is the crop (`job.source_path`), and the target is `scaled_target(job, source_size(job))`. Factor the shared two-step body out of `generate_layer` into `draw_and_lift(job, recipe, prompt, values, reference_path)`, which returns `(layers, record, notes)`, and call it from both.

- **The regions input of the new actions.** The prompt comes from the region's `desc` field, not a `prompt` input.

`plugin.toml`:
- **Generate Image:** add `surfaces = ["document"]`. Its `aspect` and `resolution` inputs get `surfaces = ["menu"]`; `model` and `seed` get `advanced = true`.
- **Generate Layer:** `surfaces = ["layer"]`; `model`, `quality` and `seed` get `advanced = true`.
- **Precise Edit:** `surfaces = ["region"]`, `verb = "Edit"`. The region fields `type` and `text` get `advanced = true`, and the inputs `background`, `quality` and `seed` get `advanced = true`.
- **New actions:**

```toml
[[actions]]
id = "generate-in-region"
label = "Generate in Region…"
menu = "Layer"
kind = "edit"
surfaces = ["region"]
verb = "Add"
description = "Draw a box and describe what to add there: GPT Image 2.5 draws it into the picture and Seedream lifts it out as a layer that fits."
source = { from = "composite", max_side = 2048, crop_to_regions = true, padding = 0.5 }
result = { into = "layer", mask_to_regions = true }

[[actions.inputs]]
id = "regions"
type = "regions"
label = "Box"
min = 1
max = 1
fields = [{ id = "desc", type = "text", label = "What to add", placeholder = "A hot air balloon" }]

[[actions.inputs]]
id = "model"
type = "enum"
label = "Model"
advanced = true
values = [
  { id = "gpt-flare-layer", label = "GPT Image 2.5 Flare (faster)" },
  { id = "gpt-sunburst-layer", label = "GPT Image 2.5 Sunburst (more detail)" },
]
default = "gpt-flare-layer"

[[actions.inputs]]
id = "quality"
type = "enum"
label = "Quality"
advanced = true
values = [{ id = "low", label = "Low" }, { id = "medium", label = "Medium" }, { id = "high", label = "High" }]
default = "medium"

[[actions.inputs]]
id = "seed"
type = "seed"
label = "Seed"
advanced = true

[[actions]]
id = "fill-region"
label = "Fill Region…"
menu = "Filter"
kind = "edit"
surfaces = ["region"]
verb = "Replace"
description = "Draw a box (or use the selection) and describe what belongs there: GPT Image 2.5 repaints only that area."
source = { from = "composite", max_side = 2048, crop_to_regions = true, padding = 0.25 }
result = { into = "layer", mask_to_regions = true }

[[actions.inputs]]
id = "regions"
type = "regions"
label = "Box"
min = 1
max = 1
fields = [{ id = "desc", type = "text", label = "What to paint here", placeholder = "A wooden bench" }]

[[actions.inputs]]
id = "model"
type = "enum"
label = "Model"
advanced = true
values = [
  { id = "gpt-flare-fill", label = "GPT Image 2.5 Flare (faster)" },
  { id = "gpt-sunburst-fill", label = "GPT Image 2.5 Sunburst (more detail)" },
]
default = "gpt-flare-fill"

[[actions.inputs]]
id = "quality"
type = "enum"
label = "Quality"
advanced = true
values = [{ id = "low", label = "Low" }, { id = "medium", label = "Medium" }, { id = "high", label = "High" }]
default = "medium"

[[actions.inputs]]
id = "seed"
type = "seed"
label = "Seed"
advanced = true
```

`test_main.Manifest.test_every_action_offers_exactly_its_recipes` already checks that each action's model values match `ACTIONS`. The Generate Image shape test reads `MANIFEST["actions"][0]`, which is unchanged. Add a test that every action with `surfaces` has the matching manifest rules: `region` needs a `verb` and a regions input; `document` needs `kind = "generate"`.

Regenerate the snapshots (no credits are spent): `python3 plugins/comfy-cloud/tools/refresh_snapshots.py`. Then restore the timestamp-only changes with `git checkout` on the existing snapshot files, as before, and keep the new `gpt-*-fill.json`.

- [ ] **Step 4: Run all the tests**

Run:
- `python3 -W error::ResourceWarning -m unittest discover -s plugins/comfy-cloud`
- `cargo test --locked --bin xuan -- bundled_plugins` (the manifest loads with the new fields)

Expected: all pass.

- [ ] **Step 5: README and commit**

In the README's Actions table, add Generate in Region and Fill Region, and a paragraph on the AI Region tool, the ✦ button and the New Image tab. Mention that everything renders at least the size it covers.

```bash
git add plugins/comfy-cloud
git commit -m "feat(comfy-cloud): surfaces, Generate in region and Fill region, results cover their area"
```

---

### Task 11: Verify end to end, rebuild, live checks

- [ ] **Step 1: Run the full suites.**
  - `cargo test --locked 2>&1 | grep -E 'test result|FAILED'`
  - `cargo clippy --locked --all-targets`
  - `cargo fmt --check`
  - `python3 -m unittest discover -s plugins/comfy-cloud`
  - `python3 -m unittest discover -s sdk/python`

- [ ] **Step 2: Rebuild.** Run `cargo build --release --locked`. Copy the plugin's runtime files into `~/.config/xuan/plugins/comfy-cloud`, keeping `xuan_plugin.py`. Copy the SDK's new `xuan_plugin.py` too, because `fit="cover"` needs it.

- [ ] **Step 3: Live checks. Ask the user first**, with costs from the price formulas. Use the `scratchpad/comfy/live.py` harness to run:
  - `generate` with a `document` target of 1600×900 (Seedream Pro, about $0.09);
  - `fill-region` on the lighthouse poster with a box (GPT Flare, medium, ≤ $0.064);
  - `generate-in-region` with a box (≈ $0.11).

  For each, check:
  - the render size is at least the target;
  - the fill changes only the box;
  - the lifted object sits in the box.

- [ ] **Step 4: Push** with `git push -u origin feat/ai-surfaces`, but only if the user asks.
