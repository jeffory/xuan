//! The command registry: every command and tool the editor offers, with its label, category,
//! default key bindings, where it applies and whether plugins may run it.
//!
//! Key dispatch (`shortcuts.rs`), the shortcut hints in the menus, the shortcut reference (F1),
//! Settings → Keyboard Shortcuts, plugins' `host/run` and the docs check all read this table, so
//! they cannot drift apart. Built-in commands are static ([`COMMANDS`]); plugin actions are added
//! when plugins are installed ([`Keymap::build`]).
//!
//! Most commands run through [`EditorApp::command`]; tools and a few key-only actions say how they
//! run in [`Run`]. [`EditorApp::run_command`] runs any entry exactly as its menu item or key would.

use std::fmt;

use egui::{Key, Modifiers};
use xuan::{
    i18n::tr,
    plugins::{LoadError, Manifest, manifest::Shortcut},
};

use super::{EditorApp, Tool};

/// A key with exactly these modifiers. Only Ctrl, Alt and Shift are used.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(super) struct Chord {
    pub mods: Modifiers,
    pub key: Key,
}

const NONE: Modifiers = Modifiers::NONE;
const CTRL: Modifiers = Modifiers::CTRL;
const SHIFT: Modifiers = Modifiers::SHIFT;
const ALT: Modifiers = Modifiers::ALT;

const fn chord(mods: Modifiers, key: Key) -> Chord {
    Chord { mods, key }
}
const fn bare(key: Key) -> Chord {
    chord(NONE, key)
}
const fn ctrl(key: Key) -> Chord {
    chord(CTRL, key)
}
const fn shift(key: Key) -> Chord {
    chord(SHIFT, key)
}
const fn ctrl_shift(key: Key) -> Chord {
    chord(CTRL.plus(SHIFT), key)
}
const fn alt(key: Key) -> Chord {
    chord(ALT, key)
}
const fn ctrl_alt(key: Key) -> Chord {
    chord(CTRL.plus(ALT), key)
}

impl Chord {
    /// The chord for a key event, keeping only Ctrl, Alt and Shift.
    pub fn from_event(mods: Modifiers, key: Key) -> Self {
        let mut chord = Modifiers::NONE;
        chord.ctrl = mods.ctrl || (mods.command && !mods.mac_cmd);
        chord.alt = mods.alt;
        chord.shift = mods.shift;
        Self { mods: chord, key }
    }

    /// Keys that need Shift on common layouts, so Shift does not tell two chords apart.
    fn shift_insensitive(self) -> bool {
        matches!(self.key, Key::Plus | Key::Quote)
    }

    /// Whether a key press with `pressed` modifiers is this chord. Modifiers must match
    /// exactly, so Ctrl+Shift+E is never Ctrl+E, except Shift for `+` and `'`.
    pub fn matches(self, pressed: Modifiers, key: Key) -> bool {
        key == self.key
            && if self.shift_insensitive() {
                pressed.matches_logically(self.mods)
            } else {
                pressed.matches_exact(self.mods)
            }
    }

    /// Whether two chords are the same key press, so they cannot both be bound.
    pub fn same(self, other: Self) -> bool {
        let without_shift = |mods: Modifiers| Modifiers {
            shift: false,
            ..mods
        };
        self.key == other.key
            && (self.mods == other.mods
                || (self.shift_insensitive()
                    && without_shift(self.mods) == without_shift(other.mods)))
    }

    /// No Ctrl or Alt: a letter or digit like this would type into the canvas tools.
    pub fn is_plain_character(self) -> bool {
        !self.mods.ctrl && !self.mods.alt && (letter(self.key) || digit(self.key))
    }

    /// Parses `Ctrl+Shift+E`, `Alt+Backspace`, `Ctrl+Plus`, `Ctrl++` or `F1`. Modifier names
    /// are case-insensitive; `Cmd` and `Option` are accepted for Ctrl and Alt.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        let (modifiers, key) = if let Some(rest) = text.strip_suffix("++") {
            (rest, "+")
        } else if text == "+" {
            ("", "+")
        } else {
            match text.rsplit_once('+') {
                Some((modifiers, key)) => (modifiers, key),
                None => ("", text),
            }
        };
        let key = key.trim();
        let key = Key::from_name(key).or_else(|| Key::from_name(&key.to_ascii_uppercase()))?;
        let mut mods = Modifiers::NONE;
        for part in modifiers
            .split('+')
            .map(str::trim)
            .filter(|p| !p.is_empty())
        {
            match part.to_ascii_lowercase().as_str() {
                "ctrl" | "control" | "cmd" | "command" => mods.ctrl = true,
                "shift" => mods.shift = true,
                "alt" | "option" => mods.alt = true,
                _ => return None,
            }
        }
        Some(Self { mods, key })
    }

    /// The chord as shown in the interface: `Ctrl++`, `Ctrl+−`, `Ctrl+Alt+Shift+S`.
    pub fn label(self) -> String {
        let key = match self.key {
            Key::Plus => "+",
            Key::Minus => "−",
            key => key_text(key),
        };
        format!("{}{key}", self.modifier_prefix())
    }

    fn modifier_prefix(self) -> String {
        let mut text = String::new();
        for (on, name) in [
            (self.mods.ctrl, "Ctrl+"),
            (self.mods.alt, "Alt+"),
            (self.mods.shift, "Shift+"),
        ] {
            if on {
                text.push_str(name);
            }
        }
        text
    }
}

/// Written to the configuration file and the documentation: `Ctrl+Plus`, `Ctrl+;`, `Shift+[`.
impl fmt::Display for Chord {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}{}", self.modifier_prefix(), key_text(self.key))
    }
}

fn key_text(key: Key) -> &'static str {
    match key {
        Key::Plus => "Plus",
        Key::Minus => "Minus",
        Key::Quote => "'",
        Key::ArrowDown | Key::ArrowLeft | Key::ArrowRight | Key::ArrowUp => key.name(),
        key => key.symbol_or_name(),
    }
}

fn letter(key: Key) -> bool {
    let name = key.name();
    name.len() == 1 && name.as_bytes()[0].is_ascii_uppercase()
}

fn digit(key: Key) -> bool {
    let name = key.name();
    name.len() == 1 && name.as_bytes()[0].is_ascii_digit()
}

/// Where commands are grouped in the menus, Settings and the shortcut reference.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) enum Category {
    File,
    Edit,
    Image,
    Layer,
    Select,
    Filter,
    View,
    Window,
    Tools,
    Develop,
    Plugins,
    Help,
}

impl Category {
    pub const ALL: [Self; 12] = [
        Self::File,
        Self::Edit,
        Self::Image,
        Self::Layer,
        Self::Select,
        Self::Filter,
        Self::View,
        Self::Window,
        Self::Tools,
        Self::Develop,
        Self::Plugins,
        Self::Help,
    ];

    /// Untranslated name; pass it through `tr` to show it.
    pub fn name(self) -> &'static str {
        match self {
            Self::File => "File",
            Self::Edit => "Edit",
            Self::Image => "Image",
            Self::Layer => "Layer",
            Self::Select => "Select",
            Self::Filter => "Filter",
            Self::View => "View",
            Self::Window => "Window",
            Self::Tools => "Tools",
            Self::Develop => "Develop",
            Self::Plugins => "Plugins",
            Self::Help => "Help",
        }
    }
}

/// Where a command's key bindings are active. Two commands may share a chord when their scopes
/// never overlap, such as a Develop command and a tool key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Scope {
    /// The photo editor, with or without a document.
    Editor,
    /// RAW Develop.
    Develop,
    Both,
}

impl Scope {
    pub fn active(self, developing: bool) -> bool {
        match self {
            Self::Editor => !developing,
            Self::Develop => developing,
            Self::Both => true,
        }
    }

    pub fn overlaps(self, other: Self) -> bool {
        self == Self::Both || other == Self::Both || self == other
    }
}

/// Whether a plugin may run a built-in command through `host/run`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum HostRun {
    /// Plugins can never run it: it touches files, the clipboard, settings or other plugins.
    Never,
    /// Any plugin may run it: it only moves the view.
    View,
    /// Plugins with `document = "edit"` may run it: it is one undoable edit of the open document.
    Edit,
}

/// How a command runs.
#[derive(Clone, Copy)]
pub(super) enum Run {
    /// Through [`EditorApp::command`] with the command's id.
    Command,
    /// Selects a tool.
    Tool(Tool),
    /// Anything else.
    App(fn(&mut EditorApp)),
}

/// A built-in command.
pub(super) struct Command {
    /// Stable identifier, used by `EditorApp::command`, `host/run` and `[keybindings]`.
    pub id: &'static str,
    /// Untranslated label; pass it through `tr` to show it.
    pub label: &'static str,
    pub category: Category,
    /// Default key bindings, the first of which is shown in menus.
    pub keys: &'static [Chord],
    pub scope: Scope,
    pub host: HostRun,
    /// Other words to find the command by, such as in a command palette.
    pub aliases: &'static [&'static str],
    pub run: Run,
    /// Whether the command can run now (apart from a running job, which blocks all but Quit).
    pub enabled: fn(&EditorApp) -> bool,
}

const fn cmd(id: &'static str, label: &'static str, category: Category) -> Command {
    Command {
        id,
        label,
        category,
        keys: &[],
        scope: Scope::Editor,
        host: HostRun::Never,
        aliases: &[],
        run: Run::Command,
        enabled: editing,
    }
}

impl Command {
    const fn keys(self, keys: &'static [Chord]) -> Self {
        Self { keys, ..self }
    }
    const fn both(self) -> Self {
        Self {
            scope: Scope::Both,
            ..self
        }
    }
    const fn develop(self) -> Self {
        Self {
            scope: Scope::Develop,
            enabled: develop_ready,
            ..self
        }
    }
    const fn host(self, host: HostRun) -> Self {
        Self { host, ..self }
    }
    const fn aliases(self, aliases: &'static [&'static str]) -> Self {
        Self { aliases, ..self }
    }
    const fn run(self, run: Run) -> Self {
        Self { run, ..self }
    }
    const fn when(self, enabled: fn(&EditorApp) -> bool) -> Self {
        Self { enabled, ..self }
    }

    /// Plain letters are kept for tools, and for Develop, where tools do not apply.
    pub fn plain_keys_allowed(&self) -> bool {
        self.category == Category::Tools || self.scope == Scope::Develop
    }
}

fn always(_: &EditorApp) -> bool {
    true
}
fn not_developing(app: &EditorApp) -> bool {
    app.develop.is_none()
}
/// A document is open in the editor.
fn editing(app: &EditorApp) -> bool {
    app.develop.is_none() && app.session().is_some() && app.color_range.is_none()
}
fn develop_ready(app: &EditorApp) -> bool {
    app.develop.as_ref().is_some_and(|d| d.ready())
}
fn can_view(app: &EditorApp) -> bool {
    app.develop.as_ref().is_none_or(|d| d.ready())
}
/// The tab bar has a document or RAW tab.
fn has_tabs(app: &EditorApp) -> bool {
    !app.sessions.is_empty() || app.develop.is_some() || !app.inactive_develop.is_empty()
}
fn can_close(app: &EditorApp) -> bool {
    app.develop.is_some() || app.session().is_some()
}
fn can_undo(app: &EditorApp) -> bool {
    match &app.develop {
        Some(d) => d.ready() && !d.undo.is_empty(),
        None => app.session().and_then(|s| s.history.undo_name()).is_some(),
    }
}
fn can_redo(app: &EditorApp) -> bool {
    match &app.develop {
        Some(d) => d.ready() && !d.redo.is_empty(),
        None => app.session().and_then(|s| s.history.redo_name()).is_some(),
    }
}
fn raw_layer(app: &EditorApp) -> bool {
    editing(app)
        && app
            .session()
            .and_then(|s| s.document.active())
            .is_some_and(|l| l.raw.is_some() && !l.locked)
}
fn effects_layer(app: &EditorApp) -> bool {
    editing(app)
        && app
            .session()
            .and_then(|s| s.document.active())
            .is_some_and(super::layer_effects_dialog::can_take_effects)
}
fn generated_layer(app: &EditorApp) -> bool {
    editing(app)
        && app
            .session()
            .and_then(|s| s.document.active())
            .is_some_and(|layer| layer.generated.is_some())
}
fn image_layer(app: &EditorApp) -> bool {
    editing(app)
        && app
            .session()
            .and_then(|s| s.document.active())
            .is_some_and(|l| l.pixels.is_some() && !l.group)
}
fn masked_layer(app: &EditorApp) -> bool {
    editing(app)
        && app
            .session()
            .and_then(|s| s.document.active())
            .is_some_and(|l| l.mask.is_some())
}
fn has_selection(app: &EditorApp) -> bool {
    editing(app)
        && app
            .session()
            .is_some_and(|s| s.document.selection.is_some())
}
fn has_guides(app: &EditorApp) -> bool {
    editing(app) && app.session().is_some_and(|s| !s.document.guides.is_empty())
}

fn brush_tool(app: &mut EditorApp) {
    app.set_tool(app.brush_variant);
}
fn switch_brush(app: &mut EditorApp) {
    let tool = if app.tool == Tool::Brush {
        Tool::Pencil
    } else {
        Tool::Brush
    };
    app.set_tool(tool);
}
fn marquee_shape(app: &mut EditorApp) {
    app.ellipse = !app.ellipse;
    app.set_tool(Tool::Marquee);
}
fn lasso_mode(app: &mut EditorApp) {
    app.polygonal = !app.polygonal;
    app.set_tool(Tool::Lasso);
}
fn shape_kind(app: &mut EditorApp) {
    use xuan::paint::ShapeKind;
    app.shape_kind = if app.shape_kind == ShapeKind::Ellipse {
        ShapeKind::Rectangle
    } else {
        ShapeKind::Ellipse
    };
    app.set_tool(Tool::Shape);
}
fn swap_colors(app: &mut EditorApp) {
    std::mem::swap(&mut app.brush.color, &mut app.background);
}
fn reset_colors(app: &mut EditorApp) {
    app.brush.color = [0, 0, 0, 255];
    app.background = [255; 4];
}
/// The next brush size down: 15% smaller, but never by less than one pixel,
/// so small sizes step 3, 2, 1 instead of sticking at 3. Stops at 1 px.
pub(super) fn smaller_diameter(diameter: f32) -> f32 {
    let current = diameter.round().max(1.0);
    (diameter / 1.15).round().min(current - 1.0).max(1.0)
}
/// The next brush size up: 15% larger, but never by less than one pixel.
pub(super) fn larger_diameter(diameter: f32) -> f32 {
    let current = diameter.round().max(1.0);
    (diameter * 1.15).round().max(current + 1.0).min(2000.0)
}
fn brush_smaller(app: &mut EditorApp) {
    app.brush.diameter = smaller_diameter(app.brush.diameter);
}
fn brush_larger(app: &mut EditorApp) {
    app.brush.diameter = larger_diameter(app.brush.diameter);
}
fn brush_softer(app: &mut EditorApp) {
    app.brush.hardness = (app.brush.hardness - 0.1).max(0.0);
}
fn brush_harder(app: &mut EditorApp) {
    app.brush.hardness = (app.brush.hardness + 0.1).min(1.0);
}
fn toggle_controls(app: &mut EditorApp) {
    app.show_controls = !app.show_controls;
}
fn show_transform(app: &mut EditorApp) {
    app.set_tool(Tool::Move);
    app.show_controls = true;
}
fn toggle_pixel_grid(app: &mut EditorApp) {
    app.set_pixel_grid(!app.config.pixel_grid);
}
fn develop_rotate_left(app: &mut EditorApp) {
    if let Some(d) = &mut app.develop {
        d.rotate_with_undo(false);
    }
}
fn develop_rotate_right(app: &mut EditorApp) {
    if let Some(d) = &mut app.develop {
        d.rotate_with_undo(true);
    }
}
fn develop_compare(app: &mut EditorApp, compare: super::develop::Compare) {
    if let Some(d) = &mut app.develop {
        d.compare = compare;
    }
}
fn develop_edited(app: &mut EditorApp) {
    develop_compare(app, super::develop::Compare::Edited);
}
fn develop_original(app: &mut EditorApp) {
    develop_compare(app, super::develop::Compare::Original);
}
fn develop_split(app: &mut EditorApp) {
    develop_compare(app, super::develop::Compare::Split);
}
fn develop_side_by_side(app: &mut EditorApp) {
    develop_compare(app, super::develop::Compare::SideBySide);
}
fn develop_clipping(app: &mut EditorApp) {
    if let Some(d) = &mut app.develop {
        d.show_clipping = !d.show_clipping;
    }
}

use Category as C;
use HostRun::{Edit, View};

/// A tool key: plain letters are allowed, and it works without a document.
const fn tool(id: &'static str, label: &'static str, keys: &'static [Chord], run: Run) -> Command {
    cmd(id, label, Category::Tools)
        .keys(keys)
        .run(run)
        .when(not_developing)
}

/// Every built-in command, in menu order within each category.
pub(super) const COMMANDS: &[Command] = &[
    // File
    cmd("new", "New Canvas…", C::File)
        .keys(&[ctrl(Key::N)])
        .both()
        .when(always)
        .aliases(&["document", "create"]),
    cmd("open", "Open…", C::File)
        .keys(&[ctrl(Key::O)])
        .both()
        .when(always),
    cmd("open_clipboard", "Open Image from Clipboard", C::File)
        .both()
        .when(always)
        .aliases(&["paste"]),
    cmd("open_comp", "Open Compositor Package…", C::File)
        .both()
        .when(always),
    cmd("import", "Import Image as Layer…", C::File)
        .keys(&[ctrl_shift(Key::O)])
        .when(not_developing)
        .aliases(&["place"]),
    cmd("save", "Save", C::File).keys(&[ctrl(Key::S)]),
    cmd("save_as", "Save As…", C::File).keys(&[ctrl_shift(Key::S)]),
    cmd("export", "Export Image…", C::File)
        .keys(&[chord(CTRL.plus(ALT).plus(SHIFT), Key::S)])
        .aliases(&["png", "jpeg", "webp"]),
    cmd("close", "Close Project", C::File)
        .keys(&[ctrl(Key::W)])
        .both()
        .when(can_close),
    cmd("quit", "Quit", C::File)
        .keys(&[ctrl(Key::Q)])
        .both()
        .when(always)
        .aliases(&["exit"]),
    // Edit
    cmd("settings", "Settings…", C::Edit)
        .keys(&[ctrl(Key::Comma)])
        .both()
        .when(always)
        .aliases(&["preferences", "options", "keyboard shortcuts"]),
    cmd("undo", "Undo", C::Edit)
        .keys(&[ctrl(Key::Z)])
        .both()
        .host(Edit)
        .when(can_undo),
    cmd("redo", "Redo", C::Edit)
        .keys(&[ctrl_shift(Key::Z), ctrl(Key::Y)])
        .both()
        .host(Edit)
        .when(can_redo),
    cmd("cut", "Cut", C::Edit).keys(&[ctrl(Key::X)]),
    cmd("copy", "Copy", C::Edit).keys(&[ctrl(Key::C)]),
    cmd("copy_merged", "Copy Merged", C::Edit).keys(&[ctrl_shift(Key::C)]),
    cmd("paste", "Paste", C::Edit)
        .keys(&[ctrl(Key::V)])
        .when(not_developing),
    cmd("fill_fg", "Fill Foreground", C::Edit)
        .keys(&[chord(ALT, Key::Backspace)])
        .host(Edit),
    cmd("fill_bg", "Fill Background", C::Edit)
        .keys(&[ctrl(Key::Backspace)])
        .host(Edit),
    // Without a selection, the key deletes the selected layers instead; see `run_shortcut`.
    cmd("clear", "Clear Pixels", C::Edit)
        .keys(&[bare(Key::Delete), bare(Key::Backspace)])
        .host(Edit)
        .aliases(&["delete", "erase"]),
    cmd("content_fill", "Content-Aware Fill", C::Edit)
        .keys(&[shift(Key::F5)])
        .host(Edit)
        .aliases(&["remove object", "heal"]),
    cmd("show_transform", "Free Transform", C::Edit)
        .keys(&[ctrl(Key::T)])
        .run(Run::App(show_transform))
        .when(not_developing)
        .aliases(&["scale", "rotate"]),
    // Image
    cmd("levels", "Levels", C::Image).keys(&[ctrl(Key::L)]),
    cmd("hue", "Hue/Saturation", C::Image)
        .keys(&[ctrl(Key::U)])
        .aliases(&["hsl", "color"]),
    cmd("curves", "Curves", C::Image).keys(&[ctrl(Key::M)]),
    cmd("invert", "Invert", C::Image)
        .keys(&[ctrl(Key::I)])
        .host(Edit)
        .aliases(&["negative"]),
    cmd("image_size", "Image Size…", C::Image).aliases(&["resize", "scale"]),
    cmd("canvas_size", "Canvas Size…", C::Image),
    cmd("flip_canvas_h", "Flip Canvas Horizontal", C::Image)
        .host(Edit)
        .aliases(&["mirror"]),
    cmd("flip_canvas_v", "Flip Canvas Vertical", C::Image).host(Edit),
    cmd("rotate_canvas_cw", "Rotate Canvas 90° Clockwise", C::Image)
        .host(Edit)
        .aliases(&["rotate image", "turn"]),
    cmd(
        "rotate_canvas_ccw",
        "Rotate Canvas 90° Counter-Clockwise",
        C::Image,
    )
    .host(Edit)
    .aliases(&["rotate image", "turn"]),
    cmd("rotate_canvas_180", "Rotate Canvas 180°", C::Image)
        .host(Edit)
        .aliases(&["rotate image", "turn"]),
    cmd("crop_to_selection", "Crop to Selection", C::Image)
        .when(has_selection)
        .host(Edit)
        .aliases(&["crop"]),
    cmd("trim", "Trim…", C::Image).aliases(&["crop", "remove margins", "borders"]),
    // Layer
    cmd("new_layer", "New Layer", C::Layer)
        .keys(&[ctrl_shift(Key::N)])
        .host(Edit),
    cmd("duplicate", "Duplicate Layers", C::Layer)
        .keys(&[ctrl(Key::J)])
        .host(Edit),
    cmd("delete_layer", "Delete Layers", C::Layer).host(Edit),
    cmd("develop", "Develop RAW…", C::Layer).when(raw_layer),
    cmd("rasterize_raw", "Rasterize RAW Layer", C::Layer).when(raw_layer),
    cmd("layer_effects", "Layer Effects…", C::Layer)
        .when(effects_layer)
        .aliases(&["shadow", "glow", "stroke", "overlay"]),
    cmd("group", "Group Layers", C::Layer)
        .keys(&[ctrl(Key::G)])
        .host(Edit),
    cmd("ungroup", "Ungroup Layers", C::Layer)
        .keys(&[ctrl_shift(Key::G)])
        .host(Edit),
    cmd("move_out", "Move Out of Group", C::Layer).host(Edit),
    cmd("merge", "Merge Down / Selected", C::Layer)
        .keys(&[ctrl(Key::E)])
        .host(Edit),
    cmd("flatten", "Flatten Image", C::Layer).host(Edit),
    cmd("new_mask_layer", "New Mask Layer", C::Layer).host(Edit),
    cmd("mask", "Add Mask from Selection", C::Layer).host(Edit),
    cmd("disable_mask", "Enable / Disable Mask", C::Layer).host(Edit),
    cmd("link_mask", "Link / Unlink Mask", C::Layer).host(Edit),
    cmd("delete_mask", "Delete Mask", C::Layer).host(Edit),
    cmd("clip", "Create / Release Clipping Mask", C::Layer)
        .keys(&[ctrl_alt(Key::G)])
        .host(Edit),
    cmd("flip_h", "Flip Horizontal", C::Layer).host(Edit),
    cmd("flip_v", "Flip Vertical", C::Layer).host(Edit),
    cmd("rerun_plugin", "Re-run Plugin Action…", C::Layer).when(generated_layer),
    // Select
    cmd("select_all", "Select All", C::Select)
        .keys(&[ctrl(Key::A)])
        .host(Edit),
    cmd("deselect", "Deselect", C::Select)
        .keys(&[ctrl(Key::D)])
        .host(Edit),
    cmd("invert_selection", "Inverse Selection", C::Select)
        .keys(&[ctrl_shift(Key::I)])
        .host(Edit),
    cmd("load_selection", "Load Layer / Mask", C::Select),
    cmd("select_layer_pixels", "Select Layer's Pixels", C::Select)
        .when(image_layer)
        .host(Edit)
        .aliases(&["alpha", "opacity", "transparency"]),
    cmd("select_mask_black", "Select Mask's Black Areas", C::Select)
        .when(masked_layer)
        .host(Edit),
    cmd("select_subject", "Select Subject", C::Select)
        .keys(&[ctrl_alt(Key::A)])
        .host(Edit)
        .aliases(&["foreground", "cutout", "grabcut"]),
    cmd("color_range", "Colour Range…", C::Select).aliases(&["colour", "similar", "green screen"]),
    cmd("expand_selection", "Expand Selection…", C::Select)
        .when(has_selection)
        .aliases(&["grow", "dilate"]),
    cmd("contract_selection", "Contract Selection…", C::Select)
        .when(has_selection)
        .aliases(&["shrink", "erode"]),
    cmd("feather", "Feather 3 px", C::Select).host(Edit),
    cmd("paths", "Paths…", C::Select)
        .when(editing)
        .aliases(&["pen", "bezier", "svg", "vector"]),
    // Filter
    cmd("remove_background", "Remove Background", C::Filter)
        .when(image_layer)
        .host(Edit)
        .aliases(&["cutout", "transparent", "subject", "grabcut"]),
    cmd(
        "remove_flat_background",
        "Remove Flat Background (edge colours)",
        C::Filter,
    )
    .when(image_layer)
    .host(Edit)
    .aliases(&["cutout", "transparent", "white background"]),
    // View
    cmd("fit", "Fit Canvas", C::View)
        .keys(&[ctrl(Key::Num0)])
        .both()
        .host(View)
        .when(can_view),
    cmd("actual", "Actual Pixels", C::View)
        .keys(&[ctrl(Key::Num1)])
        .both()
        .host(View)
        .when(can_view)
        .aliases(&["100%"]),
    cmd("zoom_in", "Zoom In", C::View)
        .keys(&[ctrl(Key::Plus), ctrl(Key::Equals)])
        .both()
        .host(View)
        .when(can_view),
    cmd("zoom_out", "Zoom Out", C::View)
        .keys(&[ctrl(Key::Minus)])
        .both()
        .host(View)
        .when(can_view),
    cmd("toggle_pixel_grid", "Pixel Grid", C::View)
        .both()
        .run(Run::App(toggle_pixel_grid))
        .when(always),
    cmd("toggle_controls", "Show Transform Controls", C::View)
        .keys(&[ctrl(Key::H)])
        .run(Run::App(toggle_controls))
        .when(not_developing),
    cmd("toggle_grid", "Show Grid", C::View).keys(&[ctrl(Key::Quote)]),
    cmd("toggle_guides", "Show Guides", C::View).keys(&[ctrl(Key::Semicolon)]),
    cmd("grid_settings", "Grid Settings…", C::View),
    cmd("toggle_rulers", "Rulers", C::View).keys(&[ctrl(Key::R)]),
    // Shift+; types a colon on many layouts, so either key toggles snapping.
    cmd("toggle_snap", "Snap", C::View).keys(&[ctrl_shift(Key::Semicolon), ctrl_shift(Key::Colon)]),
    cmd("lock_guides", "Lock Guides", C::View).keys(&[ctrl_alt(Key::Semicolon)]),
    cmd("clear_guides", "Clear Guides", C::View).when(has_guides),
    // Window
    cmd("reset_panels", "Reset Panel Layout", C::Window).when(not_developing),
    cmd("next_tab", "Next Tab", C::Window)
        .keys(&[ctrl(Key::Tab), ctrl(Key::PageDown)])
        .both()
        .when(has_tabs)
        .aliases(&["switch tab", "document"]),
    cmd("previous_tab", "Previous Tab", C::Window)
        .keys(&[ctrl_shift(Key::Tab), ctrl(Key::PageUp)])
        .both()
        .when(has_tabs),
    // Alt+1…9 as in Firefox on Linux: Ctrl+1 is Actual Pixels.
    cmd("tab_1", "Tab 1", C::Window)
        .keys(&[alt(Key::Num1)])
        .both()
        .when(has_tabs),
    cmd("tab_2", "Tab 2", C::Window)
        .keys(&[alt(Key::Num2)])
        .both()
        .when(has_tabs),
    cmd("tab_3", "Tab 3", C::Window)
        .keys(&[alt(Key::Num3)])
        .both()
        .when(has_tabs),
    cmd("tab_4", "Tab 4", C::Window)
        .keys(&[alt(Key::Num4)])
        .both()
        .when(has_tabs),
    cmd("tab_5", "Tab 5", C::Window)
        .keys(&[alt(Key::Num5)])
        .both()
        .when(has_tabs),
    cmd("tab_6", "Tab 6", C::Window)
        .keys(&[alt(Key::Num6)])
        .both()
        .when(has_tabs),
    cmd("tab_7", "Tab 7", C::Window)
        .keys(&[alt(Key::Num7)])
        .both()
        .when(has_tabs),
    cmd("tab_8", "Tab 8", C::Window)
        .keys(&[alt(Key::Num8)])
        .both()
        .when(has_tabs),
    cmd("tab_9", "Last Tab", C::Window)
        .keys(&[alt(Key::Num9)])
        .both()
        .when(has_tabs),
    cmd("reopen_closed_tab", "Reopen Closed Tab", C::Window)
        .keys(&[ctrl_shift(Key::T)])
        .both()
        .when(always)
        .aliases(&["undo close", "recent"]),
    cmd("close_other_tabs", "Close Other Tabs", C::Window)
        .both()
        .when(has_tabs),
    cmd("close_tabs_to_right", "Close Tabs to the Right", C::Window)
        .both()
        .when(has_tabs),
    // Tools
    tool(
        "tool_move",
        "Move / Transform",
        &[bare(Key::V)],
        Run::Tool(Tool::Move),
    ),
    tool(
        "tool_marquee",
        "Marquee",
        &[bare(Key::M)],
        Run::Tool(Tool::Marquee),
    ),
    tool(
        "marquee_shape",
        "Switch Rectangle / Ellipse Marquee",
        &[shift(Key::M)],
        Run::App(marquee_shape),
    ),
    tool(
        "tool_lasso",
        "Lasso",
        &[bare(Key::L)],
        Run::Tool(Tool::Lasso),
    ),
    tool(
        "lasso_mode",
        "Switch Freehand / Polygonal Lasso",
        &[shift(Key::L)],
        Run::App(lasso_mode),
    ),
    tool(
        "tool_wand",
        "Magic Wand",
        &[bare(Key::W)],
        Run::Tool(Tool::Wand),
    ),
    tool("tool_crop", "Crop", &[bare(Key::C)], Run::Tool(Tool::Crop)),
    // B selects whichever of Brush and Pencil was used last.
    tool("tool_brush", "Brush", &[bare(Key::B)], Run::App(brush_tool)),
    tool(
        "switch_brush",
        "Switch between Brush and Pencil",
        &[shift(Key::B)],
        Run::App(switch_brush),
    ),
    tool(
        "tool_eraser",
        "Eraser",
        &[bare(Key::E)],
        Run::Tool(Tool::Erase),
    ),
    tool(
        "tool_heal",
        "Spot Healing",
        &[bare(Key::J)],
        Run::Tool(Tool::Heal),
    ),
    tool(
        "tool_clone",
        "Clone Stamp",
        &[bare(Key::S)],
        Run::Tool(Tool::Clone),
    ),
    tool(
        "tool_blur",
        "Blur / Smudge",
        &[bare(Key::R)],
        Run::Tool(Tool::Blur),
    ),
    tool(
        "tool_gradient",
        "Gradient",
        &[bare(Key::G)],
        Run::Tool(Tool::Gradient),
    ),
    tool(
        "tool_shape",
        "Shape",
        &[bare(Key::U)],
        Run::Tool(Tool::Shape),
    ),
    tool(
        "shape_kind",
        "Switch Rectangle / Ellipse Shape",
        &[shift(Key::U)],
        Run::App(shape_kind),
    ),
    tool("tool_pen", "Pen", &[bare(Key::P)], Run::Tool(Tool::Pen)).aliases(&["bezier", "path"]),
    tool("tool_text", "Text", &[bare(Key::T)], Run::Tool(Tool::Text)),
    tool(
        "tool_eyedropper",
        "Eyedropper",
        &[bare(Key::I)],
        Run::Tool(Tool::Dropper),
    )
    .aliases(&["color picker"]),
    tool("tool_hand", "Hand", &[bare(Key::H)], Run::Tool(Tool::Hand)).aliases(&["pan"]),
    tool("tool_zoom", "Zoom", &[bare(Key::Z)], Run::Tool(Tool::Zoom)),
    tool(
        "swap_colors",
        "Swap Colours",
        &[bare(Key::X)],
        Run::App(swap_colors),
    ),
    tool(
        "reset_colors",
        "Reset Colours",
        &[bare(Key::D)],
        Run::App(reset_colors),
    ),
    tool(
        "brush_smaller",
        "Decrease Brush Size",
        &[bare(Key::OpenBracket)],
        Run::App(brush_smaller),
    ),
    tool(
        "brush_larger",
        "Increase Brush Size",
        &[bare(Key::CloseBracket)],
        Run::App(brush_larger),
    ),
    tool(
        "brush_softer",
        "Decrease Brush Hardness",
        &[shift(Key::OpenBracket)],
        Run::App(brush_softer),
    ),
    tool(
        "brush_harder",
        "Increase Brush Hardness",
        &[shift(Key::CloseBracket)],
        Run::App(brush_harder),
    ),
    // Develop
    cmd("develop_rotate_left", "Rotate Left", C::Develop)
        .develop()
        .run(Run::App(develop_rotate_left)),
    cmd("develop_rotate_right", "Rotate Right", C::Develop)
        .develop()
        .run(Run::App(develop_rotate_right)),
    cmd("develop_edited", "Show Edited", C::Develop)
        .develop()
        .run(Run::App(develop_edited)),
    cmd("develop_original", "Show Original", C::Develop)
        .develop()
        .run(Run::App(develop_original))
        .aliases(&["before"]),
    cmd("develop_split", "Split View", C::Develop)
        .develop()
        .run(Run::App(develop_split)),
    cmd("develop_side_by_side", "Side by Side View", C::Develop)
        .develop()
        .run(Run::App(develop_side_by_side)),
    cmd("develop_clipping", "Show Clipping", C::Develop)
        .develop()
        .run(Run::App(develop_clipping)),
    // Plugins
    cmd("plugins", "Manage Plugins…", C::Plugins)
        .both()
        .when(always)
        .aliases(&["extensions"]),
    cmd("install_plugin", "Install from Folder or Zip…", C::Plugins)
        .both()
        .when(always)
        .aliases(&["install plugin", "add plugin", "extensions"]),
    // Help
    cmd("command_palette", "Command Palette…", C::Help)
        .keys(&[ctrl(Key::K)])
        .both()
        .when(always)
        .run(Run::App(super::palette::toggle))
        .aliases(&["search", "commands", "actions", "find command"]),
    cmd("shortcuts", "Keyboard Shortcuts", C::Help)
        .keys(&[bare(Key::F1)])
        .both()
        .when(always)
        .aliases(&["keys", "hotkeys"]),
    cmd("about", "About Xuan", C::Help)
        .both()
        .when(always)
        .aliases(&["version"]),
];

/// Keys the editor handles itself, which no command can be bound to.
pub(super) const RESERVED: &[(Chord, &str)] = &[
    (ctrl(Key::Enter), "apply_text"),
    (bare(Key::Enter), "apply"),
    (bare(Key::Escape), "cancel"),
    (bare(Key::Space), "pan"),
    (bare(Key::Tab), "focus"),
    (bare(Key::ArrowLeft), "nudge"),
    (bare(Key::ArrowRight), "nudge"),
    (bare(Key::ArrowUp), "nudge"),
    (bare(Key::ArrowDown), "nudge"),
    (shift(Key::ArrowLeft), "nudge"),
    (shift(Key::ArrowRight), "nudge"),
    (shift(Key::ArrowUp), "nudge"),
    (shift(Key::ArrowDown), "nudge"),
];

/// A built-in command by id.
pub(super) fn find(id: &str) -> Option<&'static Command> {
    COMMANDS.iter().find(|command| command.id == id)
}

/// What `host/run` allows plugins for a built-in command id.
pub(super) fn host_run(id: &str) -> HostRun {
    find(id).map_or(HostRun::Never, |command| command.host)
}

/// The command that selects a tool, for its tooltip.
pub(super) fn tool_command(tool: Tool) -> Option<&'static str> {
    Some(match tool {
        Tool::Move => "tool_move",
        Tool::Marquee => "tool_marquee",
        Tool::Lasso => "tool_lasso",
        Tool::Wand => "tool_wand",
        Tool::Crop => "tool_crop",
        Tool::Brush => "tool_brush",
        Tool::Pencil => "switch_brush",
        Tool::Erase => "tool_eraser",
        Tool::Heal => "tool_heal",
        Tool::Clone => "tool_clone",
        Tool::Blur => "tool_blur",
        Tool::Gradient => "tool_gradient",
        Tool::Shape => "tool_shape",
        Tool::Pen => "tool_pen",
        Tool::Text => "tool_text",
        Tool::Dropper => "tool_eyedropper",
        Tool::Hand => "tool_hand",
        Tool::Zoom => "tool_zoom",
        Tool::Region => return None,
    })
}

/// Whether a number key or reserved key: never available for commands.
pub(super) fn reserved(chord: Chord) -> Option<&'static str> {
    if !chord.mods.ctrl && !chord.mods.alt && digit(chord.key) {
        return Some("opacity");
    }
    RESERVED
        .iter()
        .find(|(reserved, _)| reserved.same(chord))
        .map(|(_, name)| *name)
}

/// How a plugin action is labelled in menus and the shortcut list: its label
/// followed by the plugin's name, so it never passes for a built-in command.
pub(super) fn plugin_action_label(label: &str, plugin_name: &str) -> String {
    format!("{label} · {plugin_name}")
}

/// An entry of the registry: a built-in command or a plugin action.
pub(super) enum Kind {
    Builtin(&'static Command),
    Plugin {
        plugin: String,
        action: String,
        label: String,
    },
}

pub(super) struct Entry {
    /// The built-in id, or `plugin/action` for a plugin action.
    pub id: String,
    pub kind: Kind,
    /// Bindings before the user changed anything.
    pub defaults: Vec<Chord>,
    /// Bindings in effect.
    pub keys: Vec<Chord>,
}

impl Entry {
    /// The translated label.
    pub fn label(&self) -> &str {
        match &self.kind {
            Kind::Builtin(command) => tr(command.label),
            Kind::Plugin { label, .. } => label,
        }
    }

    pub fn category(&self) -> Category {
        match &self.kind {
            Kind::Builtin(command) => command.category,
            Kind::Plugin { .. } => Category::Plugins,
        }
    }

    pub fn scope(&self) -> Scope {
        match &self.kind {
            Kind::Builtin(command) => command.scope,
            Kind::Plugin { .. } => Scope::Editor,
        }
    }

    pub fn aliases(&self) -> &[&'static str] {
        match &self.kind {
            Kind::Builtin(command) => command.aliases,
            Kind::Plugin { .. } => &[],
        }
    }

    pub fn plain_keys_allowed(&self) -> bool {
        match &self.kind {
            Kind::Builtin(command) => command.plain_keys_allowed(),
            Kind::Plugin { .. } => false,
        }
    }

    /// The first binding as shown in menus, or an empty string.
    pub fn shortcut(&self) -> String {
        self.keys.first().map(|c| c.label()).unwrap_or_default()
    }

    pub fn customised(&self) -> bool {
        self.keys != self.defaults
    }
}

/// Why a chord cannot simply be assigned to a command.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Refusal {
    /// The editor handles this key itself.
    Reserved,
    /// A plain letter or digit for a command that is not a tool.
    NeedsModifier,
    /// Another command whose scope overlaps uses it (its id).
    Conflict(String),
}

/// The registry with the user's bindings applied.
pub(super) struct Keymap {
    entries: Vec<Entry>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self::build(&toml::Table::new(), &[]).0
    }
}

impl Keymap {
    /// Built-ins and the actions of `manifests`, with `overrides` (the `[keybindings]` table)
    /// applied. A plugin's manifest shortcut is used unless it names an unknown key or a chord
    /// already in use; those are returned as load errors.
    pub fn build(overrides: &toml::Table, manifests: &[Manifest]) -> (Self, Vec<LoadError>) {
        let custom = |id: &str| overrides.get(id).and_then(override_keys);
        let mut entries: Vec<Entry> = COMMANDS
            .iter()
            .map(|command| Entry {
                id: command.id.to_owned(),
                kind: Kind::Builtin(command),
                defaults: command.keys.to_vec(),
                keys: custom(command.id).unwrap_or_else(|| command.keys.to_vec()),
            })
            .collect();
        let builtins = entries.len();
        let mut errors = Vec::new();
        let mut manifest_keys = Vec::new();
        for manifest in manifests {
            for action in &manifest.actions {
                let id = format!("{}/{}", manifest.plugin.id, action.id);
                let shortcut = action
                    .shortcut
                    .as_deref()
                    .and_then(|s| Shortcut::parse(s).ok());
                let overridden = custom(&id);
                manifest_keys.push((manifest, action, shortcut, overridden.is_some()));
                entries.push(Entry {
                    keys: overridden.unwrap_or_default(),
                    id,
                    kind: Kind::Plugin {
                        plugin: manifest.plugin.id.clone(),
                        action: action.id.clone(),
                        label: plugin_action_label(&action.label, &manifest.plugin.name),
                    },
                    defaults: Vec::new(),
                });
            }
        }
        // Manifest shortcuts, in plugin order, after every built-in and user binding.
        for (index, (manifest, action, shortcut, overridden)) in
            manifest_keys.into_iter().enumerate()
        {
            let index = builtins + index;
            let Some(shortcut) = shortcut else {
                continue;
            };
            // Like the manifest errors beside it, this is plugin-author facing.
            let mut report = |problem: String| {
                if !overridden {
                    errors.push(LoadError {
                        dir: manifest.dir.clone(),
                        error: format!(
                            "action `{}`: shortcut {shortcut} {problem}; it is ignored",
                            action.id
                        ),
                    });
                }
            };
            let Some(chord) = plugin_chord(&shortcut) else {
                report("names a key Xuan does not know".into());
                continue;
            };
            if let Some(name) = reserved(chord) {
                report(format!("is already used by Xuan ({name})"));
                continue;
            }
            let owner = entries.iter().enumerate().find(|(i, entry)| {
                *i != index
                    && entry.scope().overlaps(Scope::Editor)
                    && entry.keys.iter().any(|k| k.same(chord))
            });
            if let Some((i, entry)) = owner {
                if i < builtins {
                    report(format!("is already used by Xuan ({})", entry.id));
                } else {
                    report(format!("is already used by {}", entry.id));
                }
                continue;
            }
            let entry = &mut entries[index];
            entry.defaults = vec![chord];
            if !overridden {
                entry.keys = vec![chord];
            }
        }
        (Self { entries }, errors)
    }

    pub fn entries(&self) -> &[Entry] {
        &self.entries
    }

    pub fn get(&self, id: &str) -> Option<&Entry> {
        self.entries.iter().find(|entry| entry.id == id)
    }

    /// The bindings in effect for a command.
    pub fn keys(&self, id: &str) -> &[Chord] {
        self.get(id).map_or(&[], |entry| &entry.keys)
    }

    /// The first binding of a command as shown in menus, or an empty string.
    pub fn shortcut(&self, id: &str) -> String {
        self.get(id).map(Entry::shortcut).unwrap_or_default()
    }

    /// The command a key press runs. An exact match wins over one that ignores Shift.
    pub fn lookup(&self, pressed: Modifiers, key: Key, developing: bool) -> Option<&Entry> {
        let active = self
            .entries
            .iter()
            .filter(|entry| entry.scope().active(developing));
        active
            .clone()
            .find(|entry| {
                entry
                    .keys
                    .iter()
                    .any(|c| c.key == key && pressed.matches_exact(c.mods))
            })
            .or_else(|| {
                active
                    .clone()
                    .find(|entry| entry.keys.iter().any(|c| c.matches(pressed, key)))
            })
    }

    /// Whether `chord` can be bound to `id` as it is, and if not, why.
    pub fn check(&self, id: &str, chord: Chord) -> Result<(), Refusal> {
        let Some(entry) = self.get(id) else {
            return Err(Refusal::Reserved);
        };
        if reserved(chord).is_some() {
            return Err(Refusal::Reserved);
        }
        if chord.is_plain_character() && !entry.plain_keys_allowed() {
            return Err(Refusal::NeedsModifier);
        }
        match self.entries.iter().find(|other| {
            other.id != id
                && other.scope().overlaps(entry.scope())
                && other.keys.iter().any(|k| k.same(chord))
        }) {
            Some(other) => Err(Refusal::Conflict(other.id.clone())),
            None => Ok(()),
        }
    }
}

/// The bindings an override value names: `"Ctrl+E"`, `""` for none, or a list of chords.
/// `None` when it cannot be read, so the defaults stay.
pub(super) fn override_keys(value: &toml::Value) -> Option<Vec<Chord>> {
    match value {
        toml::Value::String(text) if text.trim().is_empty() => Some(Vec::new()),
        toml::Value::String(text) => Some(vec![Chord::parse(text)?]),
        toml::Value::Array(items) => items
            .iter()
            .map(|item| item.as_str().and_then(Chord::parse))
            .collect(),
        _ => None,
    }
}

/// The override value for these bindings, as written to `[keybindings]`.
pub(super) fn override_value(keys: &[Chord]) -> toml::Value {
    match keys {
        [] => toml::Value::String(String::new()),
        [key] => toml::Value::String(key.to_string()),
        keys => toml::Value::Array(
            keys.iter()
                .map(|key| toml::Value::String(key.to_string()))
                .collect(),
        ),
    }
}

/// Notes about `[keybindings]` entries that are ignored: unknown built-in commands and values
/// that are not shortcuts. Plugin actions (`plugin/action`) are kept quietly, since the plugin
/// may be installed later.
pub(super) fn override_problems(overrides: &toml::Table) -> Vec<String> {
    let mut notes = Vec::new();
    for (id, value) in overrides {
        if !id.contains('/') && find(id).is_none() {
            notes.push(format!("ignoring unknown command `{id}` in [keybindings]"));
        } else if override_keys(value).is_none() {
            notes.push(format!(
                "ignoring [keybindings] {id} = {value}: not a shortcut such as \"Ctrl+E\""
            ));
        }
    }
    notes
}

/// Stores `keys` for `id` in `overrides`, or removes the override when they are the defaults.
pub(super) fn set_override(overrides: &mut toml::Table, keymap: &Keymap, id: &str, keys: &[Chord]) {
    if keymap.get(id).is_some_and(|entry| entry.defaults == keys) {
        overrides.remove(id);
    } else {
        overrides.insert(id.to_owned(), override_value(keys));
    }
}

/// The egui chord for a plugin shortcut, or `None` for a key egui does not know.
pub(super) fn plugin_chord(shortcut: &Shortcut) -> Option<Chord> {
    let key = Key::from_name(&shortcut.key)?;
    let mut mods = Modifiers::NONE;
    mods.ctrl = shortcut.ctrl;
    mods.shift = shortcut.shift;
    mods.alt = shortcut.alt;
    Some(Chord { mods, key })
}

impl EditorApp {
    /// Rebuilds the bindings after the configuration or the plugins changed.
    pub(super) fn rebuild_keymap(&mut self) {
        let (keymap, errors) = Keymap::build(&self.config.keybindings, &self.plugins.manifests);
        self.keymap = keymap;
        self.plugins.set_shortcut_errors(errors);
    }

    /// Whether a command can run now: its scope and predicate allow it and no job is running.
    pub(super) fn command_enabled(&self, id: &str) -> bool {
        if self.job.is_some() && id != "quit" {
            return false;
        }
        match self.keymap.get(id).map(|entry| &entry.kind) {
            Some(Kind::Builtin(command)) => {
                command.scope.active(self.develop.is_some()) && (command.enabled)(self)
            }
            Some(Kind::Plugin { plugin, .. }) => {
                self.develop.is_none() && self.plugin_available(plugin)
            }
            None => false,
        }
    }

    /// Runs a command exactly as its menu item does. A command palette runs commands this way.
    pub(super) fn run_command(&mut self, id: &str) {
        match self.keymap.get(id).map(|entry| &entry.kind) {
            Some(Kind::Builtin(command)) => match command.run {
                Run::Command => self.command(id),
                Run::Tool(tool) => self.set_tool(tool),
                Run::App(run) => run(self),
            },
            Some(Kind::Plugin { plugin, action, .. }) => {
                let (plugin, action) = (plugin.clone(), action.clone());
                self.start_plugin_action(&plugin, &action);
            }
            None => self.command(id),
        }
    }

    /// Runs the command bound to a key. Clear Pixels' key deletes the selected layers when
    /// nothing is selected.
    pub(super) fn run_shortcut(&mut self, id: &str) {
        if id == "clear"
            && self
                .session()
                .is_some_and(|s| s.document.selection.is_none())
        {
            self.run_command("delete_layer");
        } else {
            self.run_command(id);
        }
    }
}
