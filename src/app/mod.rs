use theme::PaletteExt as _;
use xuan::i18n::tr;
mod ai_regions;
mod canvas;
mod chrome;
mod clipboard;
mod color_range;
mod commands;
mod crop_tool;
mod develop;
mod develop_controls;
mod develop_preview;
mod dialogs;
mod drops;
mod eyedropper;
mod filter_controls;
mod filter_preview;
mod font_picker;
mod gpu_preview;
mod grid_settings;
mod guides;
mod icons;
mod jobs;
mod keybindings;
mod layer_effects_dialog;
mod layers;
mod layout_grid;
mod levels_controls;
#[cfg(target_os = "macos")]
mod macos;
mod menus;
mod navigator;
mod palette;
mod panels;
mod panes;
mod paths_dialog;
mod pen_tool;
mod photoshop;
mod pixel_grid;
mod plugin_consent;
mod plugin_dialogs;
mod plugin_files;
mod plugin_install;
mod plugin_models;
mod plugin_panes;
mod plugin_sessions;
mod plugins;
mod providers;
mod recent;
mod rulers;
mod selection_dialogs;
mod settings;
mod shortcuts;
mod snap;
mod stroke_dialog;
mod stroke_smoothing;
mod surfaces;
mod system_theme;
mod tablet;
mod tabs;
#[cfg(test)]
mod tests;
mod text_controls;
mod theme;
mod trim_dialog;
mod widgets;
#[cfg(target_os = "linux")]
mod window_theme;

pub use chrome::native_window;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::Result;
use egui::{Pos2, TextureHandle, Vec2};
use image::{GrayImage, RgbaImage};
use uuid::Uuid;
use xuan::{
    document::{Adjustment, Document, Layer, Mask, Point, Transform},
    effects::Filter,
    history::History,
    io, operations,
    paint::{self, Brush, PaintMode, ShapeKind},
    render,
    selection::SelectionMode,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Move,
    Marquee,
    Lasso,
    Wand,
    Crop,
    Brush,
    Pencil,
    Erase,
    Heal,
    Clone,
    Blur,
    /// Dodge, Burn or Sponge, by `EditorApp::tone_mode`; O selects it and Shift+O cycles the mode.
    Dodge,
    Gradient,
    /// Fills the area around a click with the foreground colour; shares G with Gradient.
    Bucket,
    Shape,
    /// Draws and edits Bézier paths.
    Pen,
    Text,
    Dropper,
    Hand,
    Zoom,
    /// Marks regions for a plugin action; shown only while one is open.
    Region,
}

impl Tool {
    const ALL: [Self; 21] = [
        Self::Move,
        Self::Marquee,
        Self::Lasso,
        Self::Wand,
        Self::Crop,
        Self::Brush,
        Self::Pencil,
        Self::Erase,
        Self::Heal,
        Self::Clone,
        Self::Blur,
        Self::Dodge,
        Self::Gradient,
        Self::Bucket,
        Self::Shape,
        Self::Pen,
        Self::Text,
        Self::Dropper,
        Self::Hand,
        Self::Zoom,
        Self::Region,
    ];

    fn label(self) -> &'static str {
        match self {
            Self::Move => tr("Move / Transform"),
            Self::Marquee => tr("Marquee"),
            Self::Lasso => tr("Lasso"),
            Self::Wand => tr("Magic Wand"),
            Self::Crop => tr("Crop"),
            Self::Brush => tr("Brush"),
            Self::Pencil => tr("Pencil"),
            Self::Erase => tr("Eraser"),
            Self::Heal => tr("Spot Healing"),
            Self::Clone => tr("Clone Stamp"),
            Self::Blur => tr("Blur / Smudge"),
            Self::Dodge => tr("Dodge / Burn"),
            Self::Gradient => tr("Gradient"),
            Self::Bucket => tr("Paint Bucket"),
            Self::Shape => tr("Shape"),
            Self::Pen => tr("Pen"),
            Self::Text => tr("Text"),
            Self::Dropper => tr("Eyedropper"),
            Self::Hand => tr("Hand"),
            Self::Zoom => tr("Zoom"),
            Self::Region => tr("AI Region"),
        }
    }
    fn is_brush(self) -> bool {
        matches!(
            self,
            Self::Brush
                | Self::Pencil
                | Self::Erase
                | Self::Heal
                | Self::Clone
                | Self::Blur
                | Self::Dodge
        )
    }
    fn is_selection(self) -> bool {
        matches!(self, Self::Marquee | Self::Lasso | Self::Wand)
    }
    fn hint(self) -> &'static str {
        match self {
            Self::Move => tr(
                "Click to select · Click outside to deselect · Drag to move · Handles to resize · Space to pan",
            ),
            Self::Marquee => {
                tr("Drag to select · Shift add · Alt subtract · Ctrl+D deselect · Delete clears")
            }
            Self::Lasso => tr(
                "Draw a selection · Shift add · Alt subtract · Enter closes polygon · Escape cancels",
            ),
            Self::Wand => {
                tr("Click to select similar colours · Shift add · Alt subtract · Ctrl+D deselect")
            }
            Self::Crop => tr(
                "Drag to crop · Drag handles to adjust · Enter applies · Escape cancels · Space to pan",
            ),
            Self::Pencil => tr(
                "Drag to draw hard pixels · [ ] size · Shift-click straight line · 1–0 opacity · Space to pan",
            ),
            Self::Brush | Self::Erase => tr(
                "Drag to paint · [ ] size · Shift-click straight line · 1–0 opacity · Space to pan",
            ),
            Self::Heal => tr("Paint over blemishes · [ ] size · Space to pan"),
            Self::Clone => tr("Alt-click to set source · Drag to clone · [ ] size · Space to pan"),
            Self::Blur => tr("Drag to retouch · [ ] size · 1–0 strength · Space to pan"),
            Self::Dodge => tr(
                "Drag to lighten, darken or change saturation · [ ] size · Shift-click straight line · 1–0 exposure · Shift+O switches Dodge / Burn / Sponge",
            ),
            Self::Gradient => tr("Drag to draw gradient · Shift locks angle · Escape cancels"),
            Self::Bucket => tr(
                "Click to fill similar colours with the foreground colour · 1–0 opacity · Shift+G switches to Gradient",
            ),
            Self::Shape => tr(
                "Drag to draw a new shape · Shift constrains proportions · Alt draws from center",
            ),
            Self::Pen => tr(
                "Click for a corner, drag for a curve · Click the first anchor to close · Enter finishes · Drag anchors and handles · Alt-drag a handle breaks it · Click a segment to add an anchor, an anchor to delete it · Alt-click converts",
            ),
            Self::Text => tr("Click to add text · Click text to edit · Use Move to transform"),
            Self::Dropper => {
                tr("Click to sample the composition · X swaps foreground and background")
            }
            Self::Hand => tr("Drag to pan · Scroll to zoom · Ctrl+0 fits canvas"),
            Self::Zoom => tr("Click to zoom in · Alt-click to zoom out · Ctrl+1 actual pixels"),
            Self::Region => {
                tr("Drag to mark a region for the plugin · Click a region to edit its details")
            }
        }
    }
}

struct Session {
    document: Document,
    history: History,
    path: Option<PathBuf>,
    /// The file the document was opened from, also for images (whose `path` stays empty so
    /// Save asks where to write the project). Copy Path and Reopen Closed Tab use it.
    source: Option<PathBuf>,
    title: String,
    zoom: f32,
    pan: Vec2,
    fit: bool,
    dirty_preview: bool,
    texture: Option<TextureHandle>,
    gpu: Option<gpu_preview::GpuPreview>,
    motion_blur_preview: Option<[f32; 2]>,
    preview_size: [u32; 2],
    composite: Option<Arc<RgbaImage>>,
    thumbnails: HashMap<(Uuid, bool), layers::LayerThumbnail>,
    navigator: navigator::ThumbnailCache,
    collapsed: HashSet<Uuid>,
    sample_cache: Option<eyedropper::SampleCache>,
    /// Full renders made for eyedropper sampling; lets tests check the cache.
    sample_renders: usize,
    /// Boxes drawn with the AI Region tool, and the selected one.
    ai_boxes: Vec<ai_regions::AiBox>,
    ai_selected: Option<usize>,
}

impl Session {
    fn new(mut document: Document, title: String, path: Option<PathBuf>) -> Self {
        document.id = Uuid::new_v4();
        document.promote_image_masks();
        Self {
            document,
            history: History::default(),
            source: path.clone(),
            path,
            title,
            zoom: 1.0,
            pan: Vec2::ZERO,
            fit: true,
            dirty_preview: true,
            texture: None,
            gpu: None,
            motion_blur_preview: None,
            preview_size: [0, 0],
            composite: None,
            thumbnails: HashMap::new(),
            navigator: navigator::ThumbnailCache::default(),
            collapsed: HashSet::new(),
            sample_cache: None,
            sample_renders: 0,
            ai_boxes: Vec::new(),
            ai_selected: None,
        }
    }

    fn invalidate(&mut self) {
        self.dirty_preview = true;
        self.sample_cache = None;
    }

    fn refresh(&mut self, ctx: &egui::Context, state: Option<&eframe::egui_wgpu::RenderState>) {
        let texture_limit = state.map_or_else(
            || ctx.input(|i| i.max_texture_side as u32),
            |s| s.device.limits().max_texture_dimension_2d,
        );
        // Pixel inspection needs one preview texel per document pixel. Stretching
        // the overview preview makes its texels larger than the pixel grid cells.
        let max_side = if self.zoom >= 1.0 {
            texture_limit
        } else {
            texture_limit.min(if state.is_some() { 4096 } else { 1600 })
        };
        let factor =
            (max_side as f32 / self.document.width.max(self.document.height) as f32).min(1.0);
        let size = [
            (self.document.width as f32 * factor).round().max(1.0) as u32,
            (self.document.height as f32 * factor).round().max(1.0) as u32,
        ];
        // Keep a sharper cached preview when zooming back out. Only edits may
        // reduce its resolution, so zooming and panning do not repeatedly render.
        if !self.dirty_preview && self.preview_size[0] >= size[0] && self.preview_size[1] >= size[1]
        {
            return;
        }
        self.thumbnails.retain(|(id, mask), _| {
            self.document
                .layers
                .iter()
                .any(|layer| layer.id == *id && (!mask || layer.mask.is_some()))
        });
        self.preview_size = size;
        if let Some(state) = state.filter(|s| {
            s.adapter.get_info().device_type != wgpu::DeviceType::Cpu
                && s.device.limits().max_compute_workgroups_per_dimension > 0
                && self.document.layers.iter().all(|l| {
                    l.pixels.as_ref().is_none_or(|p| {
                        p.width().max(p.height()) <= s.device.limits().max_texture_dimension_2d
                    })
                })
        }) {
            let preview = self
                .gpu
                .get_or_insert_with(|| gpu_preview::GpuPreview::new(state));
            if let Some(settings) = self.motion_blur_preview {
                preview.render_with_motion_blur(&self.document, size, Some(settings));
            } else {
                preview.render(&self.document, size);
            }
            self.texture = None;
            self.composite = None;
            self.dirty_preview = false;
            return;
        }
        self.gpu = None;
        let image = render::render_scaled(&self.document, size[0], size[1]);
        let color = egui::ColorImage::from_rgba_unmultiplied(
            [image.width() as usize, image.height() as usize],
            image.as_raw(),
        );
        let options = egui::TextureOptions {
            magnification: egui::TextureFilter::Nearest,
            ..egui::TextureOptions::LINEAR
        };
        if let Some(texture) = &mut self.texture {
            texture.set(color, options);
        } else {
            self.texture =
                Some(ctx.load_texture(format!("canvas-{}", self.document.id), color, options));
        }
        self.composite = Some(Arc::new(image));
        self.dirty_preview = false;
    }
}

/// The Paint Bucket's options; colour and opacity are the brush's.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct BucketSettings {
    tolerance: u8,
    contiguous: bool,
    anti_alias: bool,
    /// Sample: All Layers instead of the current layer.
    all_layers: bool,
}

impl Default for BucketSettings {
    fn default() -> Self {
        Self {
            tolerance: 32,
            contiguous: true,
            anti_alias: true,
            all_layers: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dialog {
    New,
    CanvasSize,
    ImageSize,
    Effect,
    Text,
    Export,
    Shortcuts,
    About,
    Settings,
    DropChoice,
    PluginPermissions,
    /// Confirm sending document data to a plugin that declares network hosts.
    PluginConsent,
    /// Confirm opening a file a plugin names.
    PluginFile,
    /// Allow a plugin's direct edits for its session.
    PluginEditSession,
    Plugins,
    /// Plugins → Install from Folder or Zip…: choose, review and install.
    PluginInstall,
    /// Confirm downloading models a plugin declares.
    PluginModels,
    PluginProposal,
    GridSettings,
    /// Layer → Layer Effects…; not `Effect`, which edits adjustments and filters.
    LayerEffects,
    /// Select → Expand… / Contract….
    SelectionAmount,
    /// Image → Trim….
    Trim,
    /// Select → Paths….
    Paths,
    /// Edit → Stroke….
    Stroke,
}

struct EffectEdit {
    original: Document,
    // The histogram uses the immutable original, independent of live preview edits.
    levels_source: Option<RgbaImage>,
    adjustment: Option<Adjustment>,
    filter: Option<Filter>,
    filter_preview: filter_preview::FilterPreview,
    as_layer: bool,
    preview: bool,
    refresh: bool,
    channel: usize,
    target: Option<Uuid>,
}

#[derive(Clone, Copy)]
enum TransformDrag {
    Move,
    Scale(usize),
    Rotate,
    Selection,
    Pixels,
    Distort(usize),
}

#[derive(Clone, Copy)]
struct LayerDrag {
    project: Uuid,
    layer: Uuid,
}

struct Gesture {
    tool: Tool,
    brush: Brush,
    brushes: Vec<Brush>,
    stroke: paint::Stroke,
    smoothing: Option<stroke_smoothing::StrokeSmoother>,
    start: Point,
    last: Point,
    screen_start: Pos2,
    pan_start: Vec2,
    points: Vec<Point>,
    original: Document,
    kind: TransformDrag,
    panning: bool,
    clone_offset: Point,
    source: Option<Arc<RgbaImage>>,
    reference: Option<Transform>,
    /// The selection's bounds when the drag moves a selection outline: [min, max].
    selection_bounds: Option<[Point; 2]>,
}

impl Gesture {
    fn changes_composition(&self, tool: Tool) -> bool {
        !self.panning
            && (matches!(self.kind, TransformDrag::Pixels)
                || matches!(
                    tool,
                    Tool::Move
                        | Tool::Brush
                        | Tool::Pencil
                        | Tool::Erase
                        | Tool::Clone
                        | Tool::Blur
                        | Tool::Dodge
                        | Tool::Gradient
                        | Tool::Shape
                ))
    }
}

pub struct EditorApp {
    config: xuan::config::Config,
    /// Where preferences are saved. `None` (headless sessions and tests) keeps
    /// them in memory only.
    config_path: Option<PathBuf>,
    /// Settings changed in memory (a field mid-drag) but not yet written.
    config_dirty: bool,
    /// Which recent files exist, as of a moment ago.
    recent_exists: recent::RecentExists,
    /// The command registry with the user's key bindings applied.
    keymap: commands::Keymap,
    /// The command palette (Ctrl+K), while it is open.
    palette: Option<palette::Palette>,
    /// Settings → Keyboard Shortcuts: search, key capture and a conflict waiting for an answer.
    key_editor: keybindings::KeyEditor,
    pane_drag: Option<panes::PaneDrag>,
    plugins: plugins::PluginState,
    tablet: Option<tablet::TabletInput>,
    context: egui::Context,
    window_title: String,
    /// The colours installed in the egui context, switched by `sync_palette`.
    palette_applied: theme::Palette,
    /// Reads the desktop's light/dark preference and accent colour in the background.
    /// `None` in tests, which never look at the developer's desktop.
    system_theme: Option<system_theme::Watcher>,
    /// The desktop's theme as last read.
    system: system_theme::SystemTheme,
    job: Option<jobs::Job>,
    develop: Option<develop::Develop>,
    inactive_develop: Vec<develop::Develop>,
    raw_queue: std::collections::VecDeque<(PathBuf, develop::DevelopTarget)>,
    develop_close_requested: Option<develop::DevelopClose>,
    gpu_state: Option<eframe::egui_wgpu::RenderState>,
    processor: Option<Arc<xuan::gpu::Processor>>,
    sessions: Vec<Session>,
    current: usize,
    tool: Tool,
    brush: Brush,
    brush_smoothing: f32,
    /// The tool plain B selects: Brush or Pencil, whichever was used last.
    brush_variant: Tool,
    /// The tool plain G selects: Gradient or Paint Bucket, whichever was used last.
    gradient_variant: Tool,
    pressure_size: bool,
    pressure_opacity: bool,
    /// Pen pressure scales the brush's flow.
    pressure_flow: bool,
    tilt_shape: bool,
    pen_samples: Vec<tablet::Sample>,
    pen_sample: Option<tablet::Sample>,
    pen_stroke: bool,
    background: [u8; 4],
    mask_target: bool,
    ellipse: bool,
    polygonal: bool,
    polygon: Vec<Point>,
    selection_mode: SelectionMode,
    tolerance: u8,
    /// Select → Expand… / Contract…: the open dialog and the amounts it remembers.
    selection_amount: Option<selection_dialogs::AmountEdit>,
    /// Image → Trim…: the choices it remembers.
    trim_settings: trim_dialog::TrimSettings,
    /// Edit → Stroke…, while it is open, and the choices it remembers.
    stroke: Option<stroke_dialog::StrokeEdit>,
    stroke_settings: stroke_dialog::StrokeSettings,
    /// Select → Paths…, while it is open.
    paths_edit: Option<paths_dialog::PathsEdit>,
    /// The Pen tool's path being drawn and the path shown for editing.
    pen: pen_tool::PenState,
    expand_amount: u32,
    contract_amount: u32,
    /// Select → Colour Range…, while open; and the Fuzziness it remembers.
    color_range: Option<color_range::ColorRangeEdit>,
    color_range_fuzziness: u32,
    /// Why the last command used a built-in algorithm instead of the chosen provider.
    provider_notice: Option<String>,
    contiguous: bool,
    /// The Paint Bucket's Tolerance, Contiguous, Anti-alias and Sample options.
    bucket: BucketSettings,
    /// The Magic tool's Object mode: a click or a dragged rectangle selects an object.
    wand_object: bool,
    radial: bool,
    shape_kind: ShapeKind,
    corner_radius: f32,
    text_style: xuan::text::TextStyle,
    text_renderer: Option<xuan::text::TextRenderer>,
    text_edit: Option<text_controls::TextEdit>,
    blur_mode: PaintMode,
    /// What the Dodge / Burn tool does: Dodge, Burn or Sponge.
    tone_mode: PaintMode,
    heal_mode: xuan::retouch::HealMode,
    auto_select: bool,
    ignore_transparent_pixels: bool,
    show_controls: bool,
    lock_ratio: bool,
    clone_source: Option<Point>,
    clone_offset: Option<Point>,
    clone_aligned: bool,
    clone_all: bool,
    dropper: Option<eyedropper::DropperGesture>,
    dropper_size: eyedropper::SampleSize,
    dropper_source: eyedropper::SampleSource,
    last_brush: Option<Point>,
    gesture: Option<Gesture>,
    /// The Crop tool's box and ratio.
    crop: crop_tool::CropTool,
    /// Lines a drag has snapped to, drawn across the canvas while it lasts.
    snap_lines: Vec<snap::SnapLine>,
    /// A guide being dragged out of a ruler or moved.
    guide_drag: Option<guides::GuideDrag>,
    /// View → Grid Settings… while it is open.
    grid_edit: Option<grid_settings::GridEdit>,
    dialog: Option<Dialog>,
    dimensions: [u32; 2],
    /// File → New: Keep aspect ratio, and the width and height it keeps in proportion.
    keep_ratio: bool,
    ratio: [u32; 2],
    resolution: f32,
    /// New Image: the Generate tab instead of a blank canvas, its Exact
    /// size option and the document action it runs (plugin, action).
    new_image_generate: bool,
    new_image_exact: bool,
    new_image_action: Option<(String, String)>,
    anchor: [f32; 2],
    effect: Option<EffectEdit>,
    /// Layer → Layer Effects… while it is open.
    layer_effects: Option<layer_effects_dialog::LayerEffectsEdit>,
    error: Option<String>,
    /// A non-fatal message about a finished operation, such as what an import left out.
    notice: Option<String>,
    /// Photoshop files read and waiting for their conversion report to be accepted.
    photoshop_imports: photoshop::PendingImports,
    /// The latest status message, shown in the status bar for a few seconds.
    status: String,
    /// The status text the status bar last saw and when it changed (UI time).
    status_shown: (String, f64),
    rename: Option<layers::LayerRename>,
    close_tab: Option<usize>,
    /// The tab bar's scroll position, closed-tab history and pending closes.
    tab_strip: tabs::TabStrip,
    close_app: bool,
    drop_prompt: Option<drops::DropPrompt>,
    pending_drops: std::collections::VecDeque<Vec<PathBuf>>,
    allow_close: bool,
    /// When set, `command` only records its name here (UI tests avoid native dialogs this way).
    #[cfg(test)]
    command_trace: Option<Vec<String>>,
    /// Whether the native window was created transparent (needed for rounded corners).
    transparent_window: bool,
    /// The decorations last requested from the window system.
    decorated: bool,
    /// The system draws the window buttons and resize edges (macOS), see [`chrome::NATIVE_BUTTONS`].
    native_buttons: bool,
    /// The title bar style the window was created with.
    startup_title_bar: xuan::config::TitleBar,
    button_layout: chrome::ButtonLayout,
    /// Window-button artwork from the desktop theme, loaded on first use.
    #[cfg(target_os = "linux")]
    window_theme: chrome::SharedWindowTheme,
    /// The pixels Xuan last put on the system clipboard, and where they came from.
    clipboard: Option<(RgbaImage, Point)>,
    /// Whole layers copied with those pixels, pasted while the system clipboard still holds them.
    copied_layers: Option<clipboard::CopiedLayers>,
    system_clipboard: Option<arboard::Clipboard>,
    /// The Export dialog's JPEG and WebP choices, kept while Xuan runs.
    export_options: io::ExportOptions,
    export_format: String,
    export_texture: Option<TextureHandle>,
    /// The export's estimated size: the preview's, scaled up to the document.
    export_bytes: usize,
    export_changed: bool,
    screenshot: Option<PathBuf>,
    screenshot_requested: bool,
    frames: usize,
    canvas_rect: Option<egui::Rect>,
    /// A popover a surface opened (New layer with AI, an AI Region box).
    surface_popup: Option<surfaces::SurfacePopup>,
    /// The popover opened this frame: the click that opened it is not a
    /// click outside it.
    surface_popup_fresh: bool,
    /// A drop-down list was open when this frame's popovers were drawn.
    surface_popup_picking: bool,
    /// Area the canvas occupied last frame, for the Navigator's viewport box.
    canvas_viewport: Option<egui::Rect>,
    /// Viewport, zoom and pan the Navigator last drew; a change schedules a repaint.
    navigator_view: Option<(egui::Rect, f32, Vec2)>,
}

impl EditorApp {
    pub fn new(
        cc: &eframe::CreationContext<'_>,
        paths: Vec<PathBuf>,
        demo: bool,
        screenshot: Option<PathBuf>,
    ) -> Self {
        let processor = cc
            .wgpu_render_state
            .as_ref()
            .filter(|state| {
                state.adapter.get_info().device_type != wgpu::DeviceType::Cpu
                    && state.device.limits().max_storage_buffers_per_shader_stage >= 4
            })
            .map(|state| xuan::gpu::Processor::new(state.device.clone(), state.queue.clone()));
        let mut app = xuan::gpu::scope(processor.clone(), || {
            Self::with_context(&cc.egui_ctx, vec![], demo, screenshot)
        });
        app.load_config();
        app.load_plugins();
        app.button_layout = chrome::ButtonLayout::from_desktop();
        app.watch_system_theme(system_theme::detect(), Some(cc.egui_ctx.clone()));
        app.processor = processor;
        app.gpu_state = cc.wgpu_render_state.clone();
        app.tablet = tablet::TabletInput::new(cc);
        #[cfg(target_os = "macos")]
        macos::install_quit_handler(&cc.egui_ctx);
        // Install the native renderer before the first RAW worker is started.
        xuan::gpu::scope(app.processor.clone(), || {
            for path in paths {
                app.open_path(&path, false);
            }
        });
        app
    }

    pub fn preview_panel(&mut self, name: &str) {
        if self.screenshot.is_none() {
            return;
        }
        match name {
            "brush" => self.set_tool(Tool::Brush),
            "selection" => self.set_tool(Tool::Marquee),
            "gradient" => self.set_tool(Tool::Gradient),
            "bucket" => self.set_tool(Tool::Bucket),
            "shape" => self.set_tool(Tool::Shape),
            "text" => {
                self.set_tool(Tool::Text);
                self.start_text(None, Point::new(100.0, 120.0));
            }
            "export" => {
                self.export_format = "jpg".into();
                self.command(name);
            }
            "levels" | "hue" | "curves" | "new" | "settings" => self.command(name),
            _ => {}
        }
    }

    fn with_context(
        ctx: &egui::Context,
        paths: Vec<PathBuf>,
        demo: bool,
        screenshot: Option<PathBuf>,
    ) -> Self {
        xuan::i18n::set_language(xuan::config::Language::English);
        theme::apply(ctx, &theme::Palette::DARK);
        egui_extras::install_image_loaders(ctx);
        let mut app = Self {
            config: Default::default(),
            config_path: None,
            config_dirty: false,
            recent_exists: Default::default(),
            keymap: Default::default(),
            palette: None,
            key_editor: Default::default(),
            pane_drag: None,
            plugins: Default::default(),
            tablet: None,
            context: ctx.clone(),
            window_title: String::new(),
            palette_applied: theme::Palette::DARK,
            system_theme: None,
            system: Default::default(),
            job: None,
            develop: None,
            inactive_develop: Vec::new(),
            raw_queue: Default::default(),
            develop_close_requested: None,
            gpu_state: None,
            processor: None,
            sessions: Vec::new(),
            current: 0,
            tool: Tool::Move,
            brush: Brush::default(),
            brush_smoothing: 0.0,
            brush_variant: Tool::Brush,
            gradient_variant: Tool::Gradient,
            pressure_size: true,
            pressure_opacity: false,
            pressure_flow: false,
            tilt_shape: false,
            pen_samples: Vec::new(),
            pen_sample: None,
            pen_stroke: false,
            background: [255; 4],
            mask_target: false,
            ellipse: false,
            polygonal: false,
            polygon: Vec::new(),
            selection_mode: SelectionMode::Replace,
            tolerance: 32,
            selection_amount: None,
            trim_settings: Default::default(),
            stroke: None,
            stroke_settings: Default::default(),
            paths_edit: None,
            pen: Default::default(),
            expand_amount: 2,
            contract_amount: 2,
            color_range: None,
            provider_notice: None,
            color_range_fuzziness: xuan::selection_ops::ColorRange::DEFAULT_FUZZINESS,
            contiguous: true,
            bucket: BucketSettings::default(),
            wand_object: false,
            radial: false,
            shape_kind: ShapeKind::Rectangle,
            corner_radius: 16.0,
            text_style: xuan::text::TextStyle::default(),
            text_renderer: None,
            text_edit: None,
            blur_mode: PaintMode::Blur,
            tone_mode: PaintMode::Dodge,
            heal_mode: xuan::retouch::HealMode::ContentAware,
            auto_select: true,
            ignore_transparent_pixels: true,
            show_controls: true,
            lock_ratio: true,
            clone_source: None,
            clone_offset: None,
            clone_aligned: true,
            clone_all: true,
            dropper: None,
            dropper_size: eyedropper::SampleSize::default(),
            dropper_source: eyedropper::SampleSource::AllLayers,
            last_brush: None,
            gesture: None,
            crop: Default::default(),
            snap_lines: Vec::new(),
            guide_drag: None,
            grid_edit: None,
            dialog: None,
            dimensions: [1920, 1080],
            keep_ratio: false,
            ratio: [1920, 1080],
            resolution: 72.0,
            new_image_generate: false,
            new_image_exact: false,
            new_image_action: None,
            anchor: [0.5, 0.5],
            effect: None,
            layer_effects: None,
            error: None,
            notice: None,
            photoshop_imports: Default::default(),
            status: String::new(),
            status_shown: (String::new(), 0.0),
            rename: None,
            close_tab: None,
            tab_strip: Default::default(),
            close_app: false,
            drop_prompt: None,
            pending_drops: Default::default(),
            allow_close: false,
            #[cfg(test)]
            command_trace: None,
            transparent_window: true,
            decorated: false,
            // Tests see the Linux and Windows title bar wherever they run.
            native_buttons: chrome::NATIVE_BUTTONS && !cfg!(test),
            startup_title_bar: xuan::config::TitleBar::Compact,
            button_layout: Default::default(),
            #[cfg(target_os = "linux")]
            window_theme: Default::default(),
            clipboard: None,
            copied_layers: None,
            system_clipboard: None,
            export_options: io::ExportOptions::default(),
            export_format: "png".into(),
            export_texture: None,
            export_bytes: 0,
            export_changed: true,
            screenshot,
            screenshot_requested: false,
            frames: 0,
            canvas_rect: None,
            surface_popup: None,
            surface_popup_fresh: false,
            surface_popup_picking: false,
            canvas_viewport: None,
            navigator_view: None,
        };
        if demo {
            app.add_demo();
        }
        for path in paths {
            app.open_path(&path, false);
        }
        app
    }

    fn input_brush(&self) -> Brush {
        let mut brush = self.brush.clone();
        if self.tilt_shape {
            brush.tilt = self
                .pen_sample
                .and_then(|sample| sample.tilt)
                .unwrap_or([0.0; 2]);
        }
        if let Some(pressure) = self.pen_sample.and_then(|sample| sample.pressure) {
            if self.pressure_size {
                brush.diameter *= pressure.max(0.01);
            }
            if self.pressure_opacity {
                brush.opacity *= pressure;
            }
            if self.pressure_flow {
                brush.flow *= pressure;
            }
        }
        brush
    }

    fn session(&self) -> Option<&Session> {
        self.sessions.get(self.current)
    }
    fn session_mut(&mut self) -> Option<&mut Session> {
        self.sessions.get_mut(self.current)
    }

    fn editing_mask(&self) -> bool {
        self.session()
            .and_then(|s| s.document.active())
            .is_some_and(|layer| {
                layer.standalone_mask || (self.mask_target && layer.mask.is_some())
            })
    }

    fn transforming_mask(&self) -> bool {
        self.editing_mask()
            && self
                .session()
                .and_then(|s| s.document.active())
                .is_some_and(|l| !l.standalone_mask)
    }

    fn edit(&mut self, name: &str, operation: impl FnOnce(&mut Document) -> Result<()>) {
        let Some(session) = self.session_mut() else {
            return;
        };
        session.history.begin(name, &session.document);
        match operation(&mut session.document)
            .and_then(|()| paint::refresh_shapes(&mut session.document))
        {
            Ok(()) => {
                session.document.promote_image_masks();
                session.document.release_clipping_cycles();
                session.history.commit();
                session.invalidate();
                self.status = name.into();
            }
            Err(error) => {
                session.history.cancel(&mut session.document);
                session.invalidate();
                self.error = Some(error.to_string());
            }
        }
    }

    fn edit_selection(&mut self, name: &str, operation: impl FnOnce(&mut Document)) {
        let Some(session) = self.session_mut() else {
            return;
        };
        session.history.begin(name, &session.document);
        operation(&mut session.document);
        session.history.commit();
        self.status = name.into();
    }

    fn edit_continuous(&mut self, name: &str, operation: impl FnOnce(&mut Document) -> Result<()>) {
        let Some(session) = self.session_mut() else {
            return;
        };
        session.history.begin(name, &session.document);
        if let Err(error) = operation(&mut session.document)
            .and_then(|()| paint::refresh_shapes(&mut session.document))
        {
            session.history.cancel(&mut session.document);
            self.error = Some(error.to_string());
        }
        if let Some(session) = self.session_mut() {
            session.invalidate();
        }
    }

    fn new_document(&mut self) {
        match Document::new(self.dimensions[0], self.dimensions[1]) {
            Ok(mut document) => {
                document.resolution = self.resolution;
                if self.config.new_canvas_size != Some(self.dimensions) {
                    self.config.new_canvas_size = Some(self.dimensions);
                    self.save_config();
                }
                self.sessions
                    .push(Session::new(document, tr("Untitled").into(), None));
                self.current = self.sessions.len() - 1;
                self.dialog = None;
                self.mask_target = false;
            }
            Err(error) => self.error = Some(error.to_string()),
        }
    }

    /// Opens `path` as a new tab (or, with `as_layer`, a layer of the current one). Whether it
    /// opened; a file that did is added to File → Open Recent.
    fn open_path(&mut self, path: &Path, as_layer: bool) -> bool {
        let earlier = self.error.take();
        self.open_path_unchecked(path, as_layer);
        let opened = self.error.is_none();
        if opened {
            self.error = earlier;
            if !as_layer {
                self.remember_recent(path);
            }
        }
        opened
    }

    fn open_path_unchecked(&mut self, path: &Path, as_layer: bool) {
        if xuan::raw::is_raw(path) {
            self.queue_raw(path, as_layer);
            return;
        }
        if io::is_photoshop(path) {
            self.open_photoshop(path, as_layer);
            return;
        }
        if !builtin_extension(path)
            && let Some((plugin, format)) = self.plugin_import_format(path)
        {
            self.open_with_plugin(&plugin, &format, path, as_layer);
            return;
        }
        let project = path.is_dir() || path.extension().is_some_and(|e| e == "xuan");
        let mut report = io::ImportReport::default();
        let result = if project {
            io::load_with_report(path).map(|(document, imported)| {
                report = imported;
                document
            })
        } else {
            io::import_image(path)
                .and_then(|image| {
                    if as_layer && !self.sessions.is_empty() {
                        let name = path
                            .file_stem()
                            .unwrap_or_default()
                            .to_string_lossy()
                            .to_string();
                        self.edit(tr("Import Image"), |doc| {
                            let mut layer = Layer::image(name, image);
                            layer.transform.x = (doc.width as f32 - layer.transform.width) * 0.5;
                            layer.transform.y = (doc.height as f32 - layer.transform.height) * 0.5;
                            doc.insert(layer);
                            Ok(())
                        });
                        return Ok(None);
                    }
                    let mut document = Document::new(image.width(), image.height())?;
                    let layer = Layer::image(
                        path.file_stem().unwrap_or_default().to_string_lossy(),
                        image,
                    );
                    document.select(layer.id, false);
                    document.layers = vec![layer];
                    Ok(Some(document))
                })
                .map(|doc| doc.unwrap_or_else(|| self.session().unwrap().document.clone()))
        };
        match result {
            Ok(document) => {
                if !project && as_layer && !self.sessions.is_empty() {
                    return;
                }
                let source = path;
                let path = if path.extension().is_some_and(|e| e == "xuan") {
                    Some(path.to_path_buf())
                } else {
                    None
                };
                let title = path
                    .as_ref()
                    .and_then(|p| p.file_stem())
                    .map(|s| s.to_string_lossy().to_string())
                    .unwrap_or_else(|| {
                        document
                            .layers
                            .first()
                            .map_or(tr("Untitled").into(), |l| l.name.clone())
                    });
                let mut session = Session::new(document, title, path);
                session.source = Some(source.to_path_buf());
                self.sessions.push(session);
                self.current = self.sessions.len() - 1;
                self.mask_target = false;
                self.dialog = None;

                if let Some(summary) = report.summary() {
                    self.status = tr("Imported with changes").into();
                    self.notice = Some(summary);
                }
            }
            Err(error) => {
                self.error = Some(format!(
                    "{} {}\n\n{error:#}",
                    tr("Could not open"),
                    path.display()
                ))
            }
        }
    }

    fn copy_layer_to_project(&mut self, drag: LayerDrag, destination: usize) {
        if self.dialog.is_some() || self.job.is_some() {
            return;
        }
        let Some(source) = self
            .sessions
            .iter()
            .position(|s| s.document.id == drag.project)
        else {
            return;
        };
        let id = drag.layer;
        if source == destination {
            return;
        }
        let document = self.sessions[source].document.clone();
        self.cancel_gesture();
        self.current = destination;
        self.edit(tr("Copy Layers from Project"), |target| {
            operations::copy_layers(&document, target, id)
        });
        self.mask_target = false;
    }

    /// Open a file through the plugin that declared its format. The plugin
    /// answers in the background; [`EditorApp::open_imported`] opens it then.
    fn open_with_plugin(&mut self, plugin: &str, format: &str, path: &Path, as_layer: bool) {
        if let Err(error) = self.start_plugin_import(plugin, format, path, as_layer) {
            self.error = Some(format!(
                "{} {}\n\n{error:#}",
                tr("Could not open"),
                path.display()
            ))
        }
    }

    /// Show a document a plugin imported, as a new tab or a layer.
    fn open_imported(&mut self, document: Document, path: &Path, as_layer: bool) {
        let title = path
            .file_stem()
            .unwrap_or_default()
            .to_string_lossy()
            .to_string();
        if as_layer && !self.sessions.is_empty() {
            let image = xuan::render::render(&document);
            self.edit(tr("Import Image"), |doc| {
                let mut layer = Layer::image(title, image);
                layer.transform.x = (doc.width as f32 - layer.transform.width) * 0.5;
                layer.transform.y = (doc.height as f32 - layer.transform.height) * 0.5;
                doc.insert(layer);
                Ok(())
            });
            return;
        }
        self.sessions.push(Session::new(document, title, None));
        self.current = self.sessions.len() - 1;
        self.session_mut().unwrap().history.mark_modified();
        self.mask_target = false;
    }

    /// Repeat the plugin action that generated the active layer.
    fn rerun_plugin_action(&mut self) {
        let Some(generated) = self
            .session()
            .and_then(|s| s.document.active())
            .and_then(|layer| layer.generated.clone())
        else {
            return;
        };
        if self.plugins.manifest(&generated.plugin).is_none() {
            self.error = Some(format!(
                "{} {}",
                tr("This layer was generated by a plugin that is not installed:"),
                generated.plugin
            ));
            return;
        }
        if let Some(source) = generated.source
            && let Some(session) = self.session_mut()
            && session.document.layers.iter().any(|l| l.id == source)
        {
            session.document.select(source, false);
        }
        self.start_plugin_action_with(
            &generated.plugin,
            &generated.action,
            Some(&generated.inputs),
        );
    }

    fn open_dialog(&mut self, as_layer: bool) {
        let extensions = [
            "xuan", "png", "jpg", "jpeg", "tif", "tiff", "webp", "bmp", "gif", "heic", "heif",
            "hif", "psd", "psb",
        ];
        // Portal file filters may be case-sensitive; cameras commonly use uppercase.
        let plugin_extensions = self.plugin_import_extensions();
        let extensions: Vec<_> = extensions
            .iter()
            .map(|e| e.to_string())
            .chain(xuan::raw::EXTENSIONS.iter().map(|e| e.to_string()))
            .chain(plugin_extensions)
            .flat_map(|extension| [extension.clone(), extension.to_ascii_uppercase()])
            .collect();
        if let Some(paths) = rfd::FileDialog::new()
            .add_filter(tr("Images and Xuan projects"), &extensions)
            .pick_files()
        {
            for path in paths {
                self.open_path(&path, as_layer);
            }
        }
    }

    fn save_current(&mut self, save_as: bool) -> bool {
        let Some(session) = self.session() else {
            return false;
        };
        let path = if save_as || session.path.is_none() {
            rfd::FileDialog::new()
                .add_filter("xuan project", &["xuan"])
                .set_file_name(format!("{}.xuan", session.title))
                .save_file()
        } else {
            session.path.clone()
        };
        let Some(mut path) = path else {
            return false;
        };
        if path.extension().is_none() {
            path.set_extension("xuan");
        }
        let session = self.session_mut().unwrap();
        match io::save(&session.document, &path) {
            Ok(()) => {
                session.title = path
                    .file_stem()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .into();
                session.path = Some(path.clone());
                session.history.mark_saved();
                self.status = tr("Project saved").into();
                self.remember_recent(&path);
                true
            }
            Err(error) => {
                self.error = Some(format!("{}\n\n{error:#}", tr("Could not save project")));
                false
            }
        }
    }

    fn set_tool(&mut self, tool: Tool) {
        self.cancel_gesture();
        // Another tool finishes the path being drawn.
        self.pen_finish();
        self.tool = tool;
        self.release_sample_caches();
        if matches!(tool, Tool::Brush | Tool::Pencil) {
            self.brush_variant = tool;
        }
        if matches!(tool, Tool::Gradient | Tool::Bucket) {
            self.gradient_variant = tool;
        }
        self.polygon.clear();
        self.crop.rect = None;
        self.crop.drag = None;
        if tool == Tool::Crop {
            self.crop_from_selection();
        }
    }

    fn cancel_gesture(&mut self) {
        self.pen_stroke = false;
        self.dropper_cancel();
        if let Some(gesture) = self.gesture.take()
            && !gesture.panning
            && let Some(session) = self.session_mut()
        {
            session.history.cancel(&mut session.document);
            session.invalidate();
        }
        self.snap_lines.clear();
        self.crop.drag = None;
        self.cancel_guide_drag();
    }

    fn start_adjustment(&mut self, adjustment: Adjustment, as_layer: bool) {
        let Some(session) = self.session_mut() else {
            return;
        };
        session.history.begin(adjustment.name(), &session.document);
        self.effect = Some(EffectEdit {
            original: session.document.clone(),
            levels_source: None,
            adjustment: Some(adjustment),
            filter: None,
            filter_preview: filter_preview::FilterPreview::default(),
            as_layer,
            preview: true,
            refresh: true,
            channel: 0,
            target: None,
        });
        self.dialog = Some(Dialog::Effect);
    }

    fn start_filter(&mut self, filter: Filter) {
        let Some(session) = self.session_mut() else {
            return;
        };
        session.history.begin(filter.name(), &session.document);
        self.effect = Some(EffectEdit {
            original: session.document.clone(),
            levels_source: None,
            adjustment: None,
            filter: Some(filter),
            filter_preview: filter_preview::FilterPreview::default(),
            as_layer: false,
            preview: true,
            refresh: true,
            channel: 0,
            target: None,
        });
        self.dialog = Some(Dialog::Effect);
    }

    fn edit_adjustment_layer(&mut self, id: Uuid) {
        let adjustment = self
            .session()
            .and_then(|s| s.document.layers.iter().find(|l| l.id == id))
            .and_then(|l| l.adjustment.clone());
        if let Some(adjustment) = adjustment {
            self.start_adjustment(adjustment, false);
            if let Some(edit) = &mut self.effect {
                edit.target = Some(id);
            }
        }
    }

    fn start_filter_layer(&mut self, filter: Filter) {
        self.start_filter(filter);
        if let Some(edit) = &mut self.effect {
            edit.as_layer = true;
        }
    }

    fn edit_filter_layer(&mut self, id: Uuid) {
        let filter = self
            .session()
            .and_then(|s| s.document.layers.iter().find(|l| l.id == id))
            .and_then(|l| l.filter.clone());
        if let Some(filter) = filter {
            self.start_filter(filter);
            if let Some(edit) = &mut self.effect {
                edit.target = Some(id);
            }
        }
    }

    fn add_demo(&mut self) {
        self.sessions.push(Session::new(
            xuan::demo::document(),
            xuan::demo::TITLE.into(),
            None,
        ));
        self.current = self.sessions.len() - 1;
    }

    fn command(&mut self, command: &str) {
        #[cfg(test)]
        if let Some(trace) = &mut self.command_trace {
            trace.push(command.to_owned());
            return;
        }
        if command == "quit" {
            // Ctrl+Q and the close button reach this while a job runs.
            self.request_quit();
            return;
        }
        if self.job.is_some() {
            return;
        }
        if self.tab_command(command) {
            return;
        }
        if let Some(develop) = &mut self.develop {
            match command {
                "new" | "open" | "open_clipboard" | "open_comp" => self.suspend_develop(),
                "about" | "shortcuts" | "settings" | "reset_panels" | "plugins"
                | "install_plugin" => {}
                "close" => {
                    self.request_develop_close(develop::DevelopClose::Tab);
                    return;
                }
                "undo" | "redo" => {
                    if develop.ready() {
                        develop.undo(command == "redo");
                    }
                    return;
                }
                "fit" | "actual" | "zoom_in" | "zoom_out" => {
                    if develop.ready() {
                        develop.view_command(command);
                    }
                    return;
                }
                _ => return,
            }
        }
        match command {
            "layer_effects" => self.start_layer_effects(None),
            "reset_panels" => self.reset_panes(),
            "plugins" => self.dialog = Some(Dialog::Plugins),
            "install_plugin" => self.open_plugin_install(),
            "rerun_plugin" => self.rerun_plugin_action(),
            "develop" => {
                if let Some(id) = self.session().and_then(|s| s.document.active) {
                    self.start_develop_layer(id);
                }
            }
            "rasterize_raw" => self.edit(tr("Rasterize RAW Layer"), |doc| {
                let layer = doc
                    .active_mut()
                    .ok_or_else(|| anyhow::anyhow!(tr("Select a RAW layer")))?;
                anyhow::ensure!(!layer.locked, tr("The layer is locked"));
                layer.raw = None;
                Ok(())
            }),
            "levels" => self.start_adjustment(
                Adjustment::LevelsChannels {
                    ranges: [xuan::color::DEFAULT_LEVELS; 4],
                },
                false,
            ),
            "hue" => self.start_adjustment(
                Adjustment::HueRanges {
                    settings: Box::default(),
                },
                false,
            ),
            "curves" => self.start_adjustment(
                Adjustment::CurvesChannels {
                    channels: std::array::from_fn(|_| {
                        vec![Point::new(0.0, 0.0), Point::new(1.0, 1.0)]
                    }),
                },
                false,
            ),
            "content_fill" => {
                self.start_job(tr("Content-Aware Fill"), xuan::retouch::content_aware_fill)
            }
            "remove_background" => {
                if !self.run_provider(
                    xuan::plugins::manifest::Capability::RemoveBackground,
                    None,
                    None,
                    xuan::selection::SelectionMode::Replace,
                ) {
                    self.start_progress_job(
                        tr("Remove Background"),
                        xuan::retouch::remove_background,
                    );
                }
            }
            "remove_flat_background" => {
                let tolerance = self.tolerance;
                self.start_job(tr("Remove Flat Background"), move |document, cancel| {
                    xuan::retouch::remove_flat_background(document, tolerance, cancel)
                });
            }
            "select_subject"
                if self.run_provider(
                    xuan::plugins::manifest::Capability::SelectSubject,
                    None,
                    None,
                    xuan::selection::SelectionMode::Replace,
                ) => {}
            "select_subject" => {
                self.start_progress_job(tr("Select Subject"), |doc, progress, cancel| {
                    let image = render::render(doc);
                    let result = xuan::segment::segment(
                        &image,
                        &xuan::segment::Seeds::subject(),
                        progress,
                        cancel,
                    )
                    .ok_or_else(|| anyhow::anyhow!(tr("Cancelled")))?;
                    doc.selection = Some(Arc::new(result.mask));
                    Ok(())
                })
            }
            "new" => {
                if let Some(size) = xuan::canvas_presets::usable(self.config.new_canvas_size) {
                    self.dimensions = size;
                }
                self.dialog = Some(Dialog::New);
            }
            "open" => self.open_dialog(false),
            "open_clipboard" => self.open_clipboard(),
            "import" => self.open_dialog(true),
            "open_comp" => {
                if let Some(path) = rfd::FileDialog::new()
                    .set_title(tr("Open Compositor .comp package folder"))
                    .pick_folder()
                {
                    self.open_path(&path, false);
                }
            }
            "save" => {
                self.save_current(false);
            }
            "save_as" => {
                self.save_current(true);
            }
            "export" => {
                self.dialog = Some(Dialog::Export);
                self.export_changed = true;
            }
            "close" => self.request_project_close(self.current),
            "undo" | "redo" => {
                if let Some(session) = self.session_mut() {
                    if command == "undo" {
                        session.history.undo(&mut session.document);
                    } else {
                        session.history.redo(&mut session.document);
                    }
                    session.invalidate();
                }
            }
            "new_layer" => self.edit(tr("New Layer"), |doc| {
                doc.insert(Layer::blank(
                    format!("{} {}", tr("Layer"), doc.layers.len() + 1),
                    doc.width,
                    doc.height,
                ));
                Ok(())
            }),
            "duplicate" => self.edit(tr("Duplicate Layers"), |doc| {
                operations::duplicate(doc);
                Ok(())
            }),
            "delete_layer" => {
                self.edit(tr("Delete Layers"), |doc| {
                    doc.delete_selected();
                    Ok(())
                });
                self.mask_target = false;
            }
            "group" => self.edit(tr("Group Layers"), |doc| {
                operations::group(doc);
                Ok(())
            }),
            "move_out" => self.edit(tr("Move Out of Group"), |doc| {
                let parent = doc.active().and_then(|l| l.parent);
                let outer = parent
                    .and_then(|id| doc.layers.iter().find(|l| l.id == id))
                    .and_then(|l| l.parent);
                for layer in &mut doc.layers {
                    if doc.selected.contains(&layer.id) && layer.parent == parent {
                        layer.parent = outer;
                    }
                }
                Ok(())
            }),
            "ungroup" => self.edit(tr("Ungroup Layers"), |doc| {
                operations::ungroup(doc);
                Ok(())
            }),
            "merge" => self.edit(tr("Merge Layers"), |doc| {
                operations::merge_selected(doc, true)
            }),
            "flatten" => self.edit(tr("Flatten Image"), |doc| {
                let layer = Layer::image("Flattened", render::render(doc));
                doc.select(layer.id, false);
                doc.layers = vec![layer];
                Ok(())
            }),
            "mask" | "new_mask_layer" => {
                self.edit(tr("Add Layer Mask"), |doc| {
                    if command == "new_mask_layer" || doc.active().is_none() {
                        let mut layer = Layer::mask(tr("Mask"), doc.width, doc.height);
                        layer.mask.as_mut().unwrap().pixels =
                            Arc::new(paint::mask_from_selection(doc, &layer));
                        doc.insert(layer);
                        return Ok(());
                    }
                    if let Some(owner) = doc.active().and_then(|l| doc.attachment_owner(l)) {
                        let image = doc.layers.iter().find(|l| l.id == owner).unwrap();
                        let mut layer = Layer::mask(tr("Mask"), doc.width, doc.height);
                        layer.transform = image.transform;
                        layer.parent = Some(owner);
                        layer.mask.as_mut().unwrap().pixels =
                            Arc::new(paint::mask_from_selection(doc, image));
                        doc.select(layer.id, false);
                        doc.layers.push(layer);
                        return Ok(());
                    }
                    if doc.active().is_some_and(|l| l.standalone_mask) {
                        return Ok(());
                    }
                    let pixels = doc.active().map(|l| paint::mask_from_selection(doc, l));
                    if let (Some(layer), Some(pixels)) = (doc.active_mut(), pixels) {
                        layer.mask = Some(Mask {
                            pixels: Arc::new(pixels),
                            ..Mask::white()
                        });
                    }
                    Ok(())
                });
                self.mask_target = true;
                if let Some(session) = self.session_mut()
                    && let Some(parent) = session.document.active().and_then(|l| l.parent)
                {
                    session.collapsed.remove(&parent);
                }
                self.brush.color = [255; 4];
                self.background = [0, 0, 0, 255];
            }
            "delete_mask" => {
                self.edit(tr("Delete Mask"), |doc| {
                    if let Some(id) = doc.active().filter(|l| l.standalone_mask).map(|l| l.id) {
                        doc.select(id, false);
                        doc.delete_selected();
                        return Ok(());
                    }
                    if let Some(layer) = doc.active_mut() {
                        layer.mask = None;
                    }
                    Ok(())
                });
                self.mask_target = false;
            }
            "disable_mask" => self.edit(tr("Toggle Mask"), |doc| {
                if let Some(mask) = doc.active_mut().and_then(|l| l.mask.as_mut()) {
                    mask.enabled = !mask.enabled;
                }
                Ok(())
            }),
            "link_mask" => self.edit(tr("Link Mask"), |doc| {
                let attached = doc
                    .active()
                    .is_some_and(|l| l.is_effect() && doc.attachment_owner(l).is_some());
                if let Some(layer) = doc.active_mut()
                    && (!layer.standalone_mask || attached)
                    && let Some(mask) = &mut layer.mask
                {
                    mask.linked = !mask.linked;
                    if mask.placement.is_none() {
                        mask.placement = Some(layer.transform);
                    }
                }
                Ok(())
            }),
            "clip" => self.edit(tr("Clipping Mask"), |doc| {
                if let Some(index) = doc.layers.iter().position(|l| Some(l.id) == doc.active) {
                    let lower = doc.layers[..index]
                        .iter()
                        .rev()
                        // A folder can be the base too: its layers' combined shape.
                        .find(|l| l.parent == doc.layers[index].parent && !l.is_effect())
                        .map(|l| l.clip_to.unwrap_or(l.id));
                    if !doc.layers[index].group
                        && !doc.layers[index].standalone_mask
                        && doc.layers[index].filter.is_none()
                    {
                        doc.layers[index].clip_to = if doc.layers[index].clip_to.is_some() {
                            None
                        } else {
                            lower
                        };
                    }
                }
                Ok(())
            }),
            "select_all" => self.edit_selection(tr("Select All"), |doc| {
                doc.selection = Some(Arc::new(GrayImage::from_pixel(
                    doc.width,
                    doc.height,
                    image::Luma([255]),
                )));
            }),
            "deselect" => self.edit_selection(tr("Deselect"), |doc| {
                doc.selection = None;
            }),
            "invert_selection" => self.edit_selection(tr("Invert Selection"), |doc| {
                if let Some(selection) = &doc.selection {
                    let mut pixels = (**selection).clone();
                    image::imageops::invert(&mut pixels);
                    doc.selection = Some(Arc::new(pixels));
                }
            }),
            "load_selection" => {
                let mask = self.editing_mask();
                self.edit_selection(tr("Load Selection"), |doc| {
                    operations::selection_from_layer(doc, mask);
                });
            }
            "select_layer_pixels" => self.edit_selection(tr("Load Layer Selection"), |doc| {
                operations::selection_from_layer_pixels(doc);
            }),
            "select_mask_black" => self.edit_selection(tr("Load Mask Selection"), |doc| {
                operations::selection_from_mask_black(doc);
            }),
            "color_range" => self.open_color_range(),
            "paths" => self.open_paths(),
            "expand_selection" => {
                self.open_selection_amount(selection_dialogs::AmountOperation::Expand)
            }
            "contract_selection" => {
                self.open_selection_amount(selection_dialogs::AmountOperation::Contract)
            }
            "feather" => self.edit_selection(tr("Feather Selection"), |doc| {
                if let Some(selection) = &doc.selection {
                    doc.selection = Some(Arc::new(xuan::gpu::blur_gray(selection, 3.0)));
                }
            }),
            "fill_fg" | "fill_bg" | "clear" => {
                let color = if command == "fill_bg" {
                    self.background
                } else {
                    self.brush.color
                };
                let mask = self.editing_mask();
                self.edit(
                    if command == "clear" {
                        tr("Clear Pixels")
                    } else {
                        tr("Fill")
                    },
                    |doc| paint::fill(doc, color, command == "clear", mask),
                );
            }
            "copy" | "copy_merged" | "cut" => {
                let Some(session) = self.session() else {
                    return;
                };
                let document = &session.document;
                if command == "cut" && document.active.is_none() {
                    self.error = Some(tr("Select a layer before cutting pixels.").into());
                    return;
                }
                // Without a selection, Copy takes the selected layers whole, with their
                // flattened pixels on the system clipboard for other apps.
                let layers = (command == "copy" && document.selection.is_none())
                    .then(|| operations::selected_roots(document))
                    .filter(|roots| !roots.is_empty())
                    .map(|roots| clipboard::CopiedLayers {
                        document: document.clone(),
                        roots,
                    });
                // A marquee can remain active after the Move tool deselects every layer.
                // Copy its visible contents when there is no layer to copy from.
                let merged = command == "copy_merged"
                    || (document.active.is_none() && document.selection.is_some());
                let copied = match &layers {
                    Some(layers) => Some((
                        operations::render_layers(document, &layers.roots),
                        Point::default(),
                    )),
                    None => operations::copy_pixels(document, merged),
                };
                let Some((pixels, point)) = copied else {
                    self.error = Some(if document.selection.is_some() {
                        tr("The selection is empty.").into()
                    } else {
                        tr("Select a layer or make a selection before copying.").into()
                    });
                    return;
                };
                self.connect_clipboard();
                if let Some(clipboard) = &mut self.system_clipboard
                    && let Err(error) = clipboard.set_image(arboard::ImageData {
                        width: pixels.width() as usize,
                        height: pixels.height() as usize,
                        bytes: std::borrow::Cow::Borrowed(pixels.as_raw()),
                    })
                {
                    self.error = Some(format!(
                        "{}\n\n{error}",
                        tr("Could not copy to the system clipboard")
                    ));
                    return;
                }
                self.status = match &layers {
                    Some(layers) => format!("{} {}", tr("Copied layers:"), layers.roots.len()),
                    None => format!(
                        "{} {} × {} px",
                        tr("Copied"),
                        pixels.width(),
                        pixels.height()
                    ),
                };
                if self.system_clipboard.is_none() {
                    self.status
                        .push_str(tr(" within Xuan; system clipboard unavailable"));
                }
                self.clipboard = Some((pixels, point));
                self.copied_layers = layers;
                if command == "cut" {
                    self.command("clear");
                }
            }
            "paste" => self.paste_clipboard(None),
            "flip_h" | "flip_v" => self.edit(tr("Flip Layer"), |doc| {
                if let Some(mut transform) = operations::transform_box(doc, false) {
                    if command == "flip_h" {
                        transform.flip_x = !transform.flip_x;
                    } else {
                        transform.flip_y = !transform.flip_y;
                    }
                    operations::apply_transform(doc, transform, false)?;
                }
                Ok(())
            }),
            "flip_canvas_h" | "flip_canvas_v" => self.edit(tr("Flip Canvas"), |doc| {
                operations::flip_canvas(doc, command == "flip_canvas_h");
                Ok(())
            }),
            "rotate_canvas_cw" | "rotate_canvas_ccw" | "rotate_canvas_180" => {
                let rotation = match command {
                    "rotate_canvas_cw" => operations::CanvasRotation::Clockwise,
                    "rotate_canvas_ccw" => operations::CanvasRotation::CounterClockwise,
                    _ => operations::CanvasRotation::Half,
                };
                self.edit(tr("Rotate Canvas"), |doc| {
                    operations::rotate_canvas(doc, rotation);
                    Ok(())
                });
                if let Some(session) = self.session_mut() {
                    session.fit = true;
                }
            }
            "crop_to_selection" => {
                self.edit(tr("Crop to Selection"), operations::crop_to_selection);
                if let Some(session) = self.session_mut() {
                    session.fit = true;
                }
            }
            "trim" => self.open_trim(),
            "stroke" => self.open_stroke(),
            "canvas_size" | "image_size" => {
                if let Some(session) = self.session() {
                    let dimensions = [session.document.width, session.document.height];
                    let resolution = session.document.resolution;
                    self.dimensions = dimensions;
                    self.resolution = resolution;
                    self.dialog = Some(if command == "canvas_size" {
                        Dialog::CanvasSize
                    } else {
                        Dialog::ImageSize
                    });
                }
            }
            "fit" => {
                if let Some(session) = self.session_mut() {
                    session.fit = true;
                }
            }
            "actual" => {
                if let Some(session) = self.session_mut() {
                    session.zoom = 1.0;
                    session.pan = Vec2::ZERO;
                    session.fit = false;
                }
            }
            "zoom_in" | "zoom_out" => {
                if let Some(session) = self.session_mut() {
                    session.zoom = (session.zoom * if command == "zoom_in" { 1.25 } else { 0.8 })
                        .clamp(*canvas::ZOOM_LIMITS.start(), *canvas::ZOOM_LIMITS.end());
                    session.fit = false;
                }
            }
            "invert" => {
                let mask = self.editing_mask();
                self.edit(tr("Invert"), |doc| {
                    xuan::effects::apply_adjustment(doc, &Adjustment::Invert, mask)
                });
            }
            "toggle_rulers" => self.set_view_option(|config| config.rulers = !config.rulers),
            "toggle_grid" => self.set_view_option(|config| config.show_grid = !config.show_grid),
            "toggle_guides" => {
                self.cancel_guide_drag();
                self.set_view_option(|config| config.show_guides = !config.show_guides);
            }
            "toggle_snap" => {
                self.set_view_option(|config| config.snap.enabled = !config.snap.enabled)
            }
            "lock_guides" => {
                self.cancel_guide_drag();
                self.set_view_option(|config| config.lock_guides = !config.lock_guides);
            }
            "clear_guides" => self.clear_guides(),
            "grid_settings" => self.open_grid_settings(),
            "settings" => self.dialog = Some(Dialog::Settings),
            "shortcuts" => self.dialog = Some(Dialog::Shortcuts),
            "about" => self.dialog = Some(Dialog::About),
            _ => {}
        }
    }
}

impl eframe::App for EditorApp {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        if let Some(tablet) = &mut self.tablet {
            self.pen_samples = tablet.update(ctx, input);
        }
    }

    fn on_exit(&mut self) {
        // Stop the tablet queue before eframe destroys its Wayland window/display.
        self.tablet = None;
        self.plugins.stop_all();
    }

    fn clear_color(&self, _visuals: &egui::Visuals) -> [f32; 4] {
        // Let the desktop show through outside the rounded client frame.
        [0.0; 4]
    }

    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        #[cfg(target_os = "macos")]
        if macos::take_quit_request() {
            self.request_quit();
        }
        self.show(ctx);
        // After the frame, so an edit it made is already counted when macOS asks to quit.
        #[cfg(target_os = "macos")]
        macos::set_needs_prompt(self.quit_needs_prompt());
    }
}

impl EditorApp {
    fn show(&mut self, ctx: &egui::Context) {
        xuan::i18n::set_language(self.config.language);
        if self.processor.is_none() {
            self.processor = self
                .gpu_state
                .as_ref()
                .filter(|state| {
                    state.adapter.get_info().device_type != wgpu::DeviceType::Cpu
                        && state.device.limits().max_storage_buffers_per_shader_stage >= 4
                })
                .map(|state| xuan::gpu::Processor::new(state.device.clone(), state.queue.clone()));
        }
        xuan::gpu::scope(self.processor.clone(), || self.show_with_processor(ctx));
        // Tabs can close or switch during the frame, too.
        self.forget_crop_of_other_documents();
    }

    /// Starts following the desktop's theme as `source` reports it. Waits briefly for the first
    /// answer, so the first frame already has the right colours.
    fn watch_system_theme(
        &mut self,
        source: Box<dyn system_theme::Source>,
        repaint: Option<egui::Context>,
    ) {
        let mut watcher = system_theme::Watcher::spawn(
            source,
            system_theme::POLL_INTERVAL,
            system_theme::FIRST_READ_TIMEOUT,
            repaint,
        );
        self.system = watcher.poll().0;
        self.system_theme = Some(watcher);
    }

    /// Installs the palette the Theme setting and the desktop ask for, when it changed.
    fn sync_palette(&mut self, ctx: &egui::Context) {
        if let Some(watcher) = &mut self.system_theme {
            self.system = watcher.poll().0;
        }
        let palette = theme::palette_for(
            self.config.theme,
            self.system.dark,
            self.system.accent.filter(|_| self.config.system_accent),
        );

        if palette != self.palette_applied {
            theme::set_palette(ctx, &palette);
            self.palette_applied = palette;
            ctx.request_repaint();
        }
    }

    fn show_with_processor(&mut self, ctx: &egui::Context) {
        self.forget_crop_of_other_documents();
        self.sync_move_options();
        self.sync_palette(ctx);

        self.poll_job();
        self.poll_develop(ctx);
        self.frames += 1;
        if ctx.input(|i| i.viewport().close_requested()) && !self.begin_quit() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        let dropped = ctx.input(|i| i.raw.dropped_files.clone());
        self.queue_drop(dropped.into_iter().filter_map(|f| f.path).collect());
        if self.job.is_some()
            && self.dialog.is_none()
            && self.develop_close_requested.is_none()
            && self.error.is_none()
            && self.close_tab.is_none()
            && !self.close_app
            && !ctx.wants_keyboard_input()
            && ctx.input_mut(|i| {
                self.keymap
                    .keys("quit")
                    .iter()
                    .any(|c| shortcuts::consume_exact(i, c.mods, c.key))
            })
        {
            // Like the close button, Ctrl+Q works during a job; quitting cancels it.
            self.request_quit();
        }
        if !self.drops_blocked() {
            self.shortcuts(ctx);
            self.process_drops();
        }
        // Keep antialiased panel seams opaque while preserving the rounded window corners.
        ctx.layer_painter(egui::LayerId::background()).rect_filled(
            ctx.content_rect(),
            self.window_corner_radius(ctx),
            ctx.palette().panel,
        );
        self.sync_decorations(ctx);
        self.publish_dialog_chrome(ctx);
        self.window_resize(ctx);
        self.poll_plugins();
        self.menus(ctx);
        if self.develop.is_some() {
            // The Develop workspace has its own toolbar; RAW tabs stay reachable above it.
            self.tabs(ctx);
            self.develop_workspace(ctx);
        } else {
            // Tool options, then the tabs as the canvas's own header.
            self.tool_options(ctx);
            self.tabs(ctx);
            self.status_bar(ctx);
            self.tool_rail(ctx);
            self.sidebar(ctx);
            self.canvas(ctx);
            self.plugin_action_dialog(ctx);
            self.surface_popups(ctx);
            self.color_range_dialog(ctx);
        }
        self.dialogs(ctx);
        self.command_palette(ctx);
        // A proposal is accepted only through its Accept button. Anything that
        // closed or replaced its dialog discards it.
        if self.plugins.proposal.is_some() && self.dialog != Some(Dialog::PluginProposal) {
            self.resolve_proposal(false);
        }
        if self.gesture.is_none()
            && self.effect.is_none()
            && self.color_range.is_none()
            && self.layer_effects.is_none()
            && self.stroke.is_none()
            && self.text_edit.is_none()
            && self.job.is_none()
            && self.plugins.proposal.is_none()
            && !ctx.input(|i| i.pointer.any_down())
            && let Some(session) = self.session_mut()
        {
            session.history.commit();
        }
        let title = if let Some(develop) = &self.develop {
            format!("{} — {} — Xuan", develop.title, tr("Develop"))
        } else {
            self.session().map_or("Xuan".to_owned(), |s| {
                format!(
                    "{}{} —  Xuan",
                    s.title,
                    if s.history.edited() { " •" } else { "" }
                )
            })
        };
        if title != self.window_title {
            self.window_title = title.clone();
            // Viewport commands request another repaint, even for an unchanged title.
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
        }
        if self.screenshot.is_some()
            && !self.screenshot_requested
            && self.frames >= 5
            && ctx.input(|i| i.time) >= 0.5
            && self
                .develop
                .as_ref()
                .is_none_or(|d| d.ready_for_screenshot())
        {
            self.screenshot_requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::default()));
        }
        for event in ctx.input(|i| i.events.clone()) {
            if let egui::Event::Screenshot { image, .. } = event
                && let Some(path) = self.screenshot.take()
            {
                let bytes: Vec<_> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
                match image::save_buffer(
                    &path,
                    &bytes,
                    image.width() as u32,
                    image.height() as u32,
                    image::ColorType::Rgba8,
                ) {
                    Ok(()) => {
                        println!("Screenshot saved to {}", path.display());
                        self.allow_close = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    Err(error) => self.error = Some(error.to_string()),
                }
            }
        }
        if self.screenshot.is_some() {
            ctx.request_repaint();
        }
    }
}

/// Files Xuan opens itself, which a plugin format cannot override.
fn builtin_extension(path: &Path) -> bool {
    path.is_dir()
        || xuan::raw::is_raw(path)
        || io::is_photoshop(path)
        || path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            matches!(
                e.to_ascii_lowercase().as_str(),
                "xuan"
                    | "png"
                    | "jpg"
                    | "jpeg"
                    | "tif"
                    | "tiff"
                    | "webp"
                    | "bmp"
                    | "gif"
                    | "heic"
                    | "heif"
                    | "hif"
            )
        })
}
