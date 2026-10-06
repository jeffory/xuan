//! The MCP tools and how each maps onto plugin protocol requests. Every
//! tool that changes a document sends one `document/edit` or `host/run`
//! request, so it is one undo step in Xuan and shows at once. The `batch`
//! tool sends the edits of several edit tools as one `document/edit`.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use rmcp::model::{CallToolResult, ContentBlock, JsonObject, Tool, ToolAnnotations};
use serde_json::{Map, Value, json};

use xuan_plugin::CancelToken;

use crate::editor::{CANCELLED, Editor, EditorError, TIMED_OUT, WITHDRAWN};

/// Most edits Xuan takes in one `document/edit` request.
pub const MAX_EDITS: usize = 1000;
/// Largest image a client may send for `create_image_layer`, decoded.
pub const MAX_IMAGE_BYTES: usize = 64 * 1024 * 1024;
/// Largest request this plugin sends Xuan, as JSON. Xuan stops a plugin
/// that writes a line over 16 MiB; this leaves room for the envelope.
pub const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024 - 64 * 1024;
/// Longest side of previews unless the client asks for another.
pub const DEFAULT_PREVIEW: u64 = 1024;

/// What a tool call runs with.
pub struct Context<'a> {
    pub editor: &'a dyn Editor,
    /// The client's MCP session, which Xuan's edit prompt is asked for.
    pub session: Option<&'a str>,
    /// A folder of the plugin's where images from the client are written
    /// for Xuan to read.
    pub incoming: &'a Path,
    /// Withdraws the call's requests in Xuan when the client cancels or the
    /// server stops waiting for the user.
    pub cancel: &'a CancelToken,
}

type Run = fn(&Context, Map<String, Value>) -> Result<Vec<ContentBlock>, String>;
/// Turns a tool's arguments into the edits it makes, without sending them.
type Planner = fn(&Context, Map<String, Value>) -> Result<Plan, String>;

/// What a tool does when called.
#[derive(Clone, Copy)]
enum Action {
    /// Reads, files, commands and history: not part of a batch.
    Run(Run),
    /// Edits: the tool plans them, and they are sent as one `document/edit`
    /// request, on their own or with the other steps of a batch.
    Edit(Planner),
    /// Runs its own way when called alone; in a batch, plans edits (or says
    /// why it cannot).
    Mixed(Run, Planner),
}

/// The edits a tool makes, for one `document/edit` request, and what it
/// answers once Xuan has applied them.
struct Plan {
    /// The undo step's name.
    name: &'static str,
    edits: Vec<Value>,
    reply: Reply,
    /// Images written for Xuan to read, removed with the plan.
    files: Incoming,
}

impl Plan {
    fn new(name: &'static str, edits: Vec<Value>, reply: Reply) -> Self {
        Self {
            name,
            edits,
            reply,
            files: Incoming::default(),
        }
    }
}

/// What an edit tool answers when called on its own.
#[derive(Clone, PartialEq)]
enum Reply {
    /// `{"ok": true}`.
    Ok,
    /// `{"ok": true, "layer": …}`.
    Layer(Value),
    /// `{"ok": true, "active": …}`.
    Active(Value),
    /// The ids of the layers the request added.
    Added,
    /// The selection after the edit.
    Selection,
    /// The canvas size after the edit.
    Size,
}

/// Files in the plugin's incoming folder, removed when dropped.
#[derive(Default)]
struct Incoming(Vec<PathBuf>);

impl Drop for Incoming {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

/// One tool: its MCP definition and what it does.
struct Spec {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    /// Properties of the input object and the required ones.
    properties: Value,
    required: &'static [&'static str],
    kind: Kind,
    run: Action,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// Reads the document; never asks to edit.
    Read,
    /// Changes the document: one undo step, after the session's answer.
    Edit,
    /// Goes through the user: a save dialog or an open prompt.
    File,
}

/// Said in the description of every tool that may wait for the user.
const EDIT_WAITS: &str = "The first edit of a session waits until the user answers Xuan's prompt to allow edits from this session. If they do not answer in time, the call fails without changing anything: then ask the user to answer the prompt in Xuan instead of retrying.";
const FILE_WAITS: &str = "The call waits until the user answers in Xuan. If they do not answer in time, it fails without doing anything: then ask the user instead of retrying.";

const LAYER: &str = "A layer id from get_document";
const MODE: &str = "How it combines with the current selection";

fn mode() -> Value {
    json!({"type": "string", "enum": ["replace", "add", "subtract", "intersect"], "description": MODE})
}
fn layer() -> Value {
    json!({"type": "string", "description": LAYER})
}
fn optional_layer() -> Value {
    json!({"type": "string", "description": "A layer id; the active layer when left out"})
}
fn number(description: &str) -> Value {
    json!({"type": "number", "description": description})
}
fn integer(description: &str) -> Value {
    json!({"type": "integer", "description": description})
}
fn color(description: &str) -> Value {
    json!({"type": "string", "pattern": "^#([0-9a-fA-F]{6}|[0-9a-fA-F]{8})$", "description": description})
}
fn layers() -> Value {
    json!({"type": "array", "items": {"type": "string"}, "minItems": 1, "description": "Layer ids"})
}
fn points() -> Value {
    json!({"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2},
           "description": "[[x, y], …] in document pixels"})
}
fn stroke_points() -> Value {
    json!({"type": "array", "items": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 3},
           "description": "[[x, y], …] in document pixels, or [x, y, pressure] with pen pressure 0–1"})
}
fn svg_path(description: &str) -> Value {
    json!({"type": "string", "minLength": 1, "description": description})
}
fn fill_rule() -> Value {
    json!({"type": "string", "enum": ["nonzero", "evenodd"],
           "description": "Which parts of the path are inside, as SVG's fill-rule (default nonzero); with evenodd an inner subpath always cuts a hole"})
}
const SVG_PATH: &str = "SVG path data in document pixels, as in an SVG <path d=…>: M, L, H, V, C, S, Q, T, A and Z, lowercase for relative, e.g. \"M 0 700 C 120 640 380 640 512 700 Z\". Open subpaths are closed";
fn name() -> Value {
    json!({"type": "string", "description": "Layer name"})
}
fn above() -> Value {
    json!({"type": "string", "description": "Put it above this layer id (default: above the active layer)"})
}
/// An adjustment or filter: a name for those without settings, else an
/// object.
fn effect(description: &str) -> Value {
    json!({"oneOf": [{"type": "string"}, {"type": "object"}], "description": description})
}

const FILTERS: &str = "A filter as Xuan's .xuan files store it, an object with one key: {\"GaussianBlur\": {\"radius\": 0–100}}, {\"MotionBlur\": {\"distance\": 0–200, \"angle\": -180–180}}, {\"Noise\": {\"amount\": 0–100, \"monochrome\": bool}} or {\"LensCorrection\": {\"distortion\": -50–50, \"vignette\": -100–100}} (a positive vignette darkens the corners, a negative one brightens them). The same JSON sent as a string is accepted too.";
const ADJUSTMENTS: &str = r#"An adjustment as Xuan's .xuan files store it, every field given: the string "Invert", or an object with one key: {"HueSaturation": {"hue": -360–360, "saturation": -100–100, "lightness": -100–100, "colorize": bool}}; {"HueRanges": {"settings": {"range": 0–6, "colorize": bool, "invert_range": bool, "adjustments": 7 × [hue -360–360, saturation -100–100, lightness -100–100] for master, reds, yellows, greens, cyans, blues and magentas, "bands": 7 × [4 hue degrees where each range ramps in, is full, is full until, ramps out]}}}; {"Levels": {"black": 0–255, "gamma": 0.01–10, "white": 0–255 and above black, "output_black": 0–255, "output_white": 0–255}}; {"LevelsChannels": {"ranges": 4 × [black, gamma, white, output_black, output_white] for master, red, green and blue}}; {"Curves": {"points": [{"x": 0–1, "y": 0–1}, …] (2–32 points, x rising from 0 to 1)}}; {"CurvesChannels": {"channels": 4 point lists for master, red, green and blue}}; {"Exposure": {"exposure": -20–20, "offset": -1–1, "gamma": 0.01–10}}; {"GradientMap": {"shadows": [r,g,b,a], "highlights": [r,g,b,a]}} (0–255); {"Grain": {"amount": 0–100 (2 is visible, 8 is heavy), "monochrome": bool, "seed": integer}}; {"FilmGrain": {"amount": 0–100, "size": 0.1–100, "roughness": 0–100, "seed": integer}}; {"BlackWhite": {"weights": [6 numbers, -200–300], "tint": bool, "tint_hue": 0–360, "tint_saturation": 0–100}}; or {"ColorBalance": {"shadows": [3 numbers, -100–100], "midtones": […], "highlights": […], "preserve_luminosity": bool}}. The same JSON sent as a string is accepted too."#;

/// `run_command` commands that start a job and return before it ends.
const BACKGROUND: [&str; 3] = [
    "content_fill",
    "remove_background",
    "remove_flat_background",
];

/// `host/run` commands the `run_command` tool offers. Xuan decides what is
/// allowed: saving, opening, the clipboard and settings never are.
const COMMANDS: [&str; 26] = [
    "flatten",
    "duplicate",
    "new_layer",
    "delete_layer",
    "move_out",
    "mask",
    "new_mask_layer",
    "delete_mask",
    "disable_mask",
    "link_mask",
    "clip",
    "flip_h",
    "flip_v",
    "flip_canvas_h",
    "flip_canvas_v",
    "invert",
    "clear",
    "fill_fg",
    "fill_bg",
    "content_fill",
    "remove_background",
    "remove_flat_background",
    "fit",
    "actual",
    "zoom_in",
    "zoom_out",
];

fn specs() -> Vec<Spec> {
    vec![
        // Reading.
        Spec {
            name: "list_documents",
            title: "List open documents",
            description: "The documents open in Xuan's tabs: id, title, size, layer count, whether it is the current one and whether it has unsaved changes.",
            properties: json!({}),
            required: &[],
            kind: Kind::Read,
            run: Action::Run(|cx, _| text(cx.call("document/list", json!({}))?)),
        },
        Spec {
            name: "get_document",
            title: "Describe the document",
            description: "The current document: size, resolution, the active layer, the selection's bounds and every layer (id, name, kind: image, text, shape, raw, group, mask, adjustment or filter; visibility, lock, opacity, blend mode, parent, position and size, rotation, flip_x and flip_y, masks, the image it is attached_to, a shape's style, and for generated layers their provenance). Layers are listed bottom to top. An image's mask is a layer of its own, of kind \"mask\", whose `parent` and `attached_to` are the image; the image lists it under `masks` with the mask layer's id and whether it is `enabled` and `linked` (moves with the image). Adjustment and filter layers attach to an image the same way and then change only that image; elsewhere they change everything below them. With `document`, that document is made current first.",
            properties: json!({"document": {"type": "string", "description": "A document id from list_documents"}}),
            required: &[],
            kind: Kind::Read,
            run: Action::Run(|cx, args| {
                let args = pick(args, &["document"])?;
                if let Some(document) = args.get("document") {
                    cx.call("document/activate", json!({"document": document}))?;
                }
                text(document(cx)?)
            }),
        },
        Spec {
            name: "get_layer",
            title: "Describe a layer",
            description: "One layer of the current document, as get_document lists it.",
            properties: json!({"layer": layer()}),
            required: &["layer"],
            kind: Kind::Read,
            run: Action::Run(|cx, args| {
                let args = pick(args, &["layer"])?;
                let id = string(&args, "layer")?;
                let document = document(cx)?;
                let found = (document["layers"].as_array().into_iter().flatten())
                    .find(|layer| layer["id"] == id)
                    .cloned()
                    .ok_or_else(|| format!("No layer {id}"))?;
                text(found)
            }),
        },
        Spec {
            name: "get_preview",
            title: "See the image",
            description: "The current document flattened, as a PNG image, so you can see the result. `max_side` is the longest side in pixels (default 1024, 64–4096). The first preview may wait while Xuan asks the user to allow sending the image.",
            properties: json!({"max_side": integer("Longest side in pixels")}),
            required: &[],
            kind: Kind::Read,
            run: Action::Run(|cx, args| {
                let args = pick(args, &["max_side"])?;
                let side = max_side(&args, DEFAULT_PREVIEW)?;
                let export = cx.call("document/export", json!({"max_side": side}))?;
                image_result(&export, "Flattened preview")
            }),
        },
        Spec {
            name: "get_layer_image",
            title: "See a layer",
            description: "A layer's own pixels (`what` = \"pixels\", the default) or its mask (\"mask\") as a PNG, with where it sits in the document. Pixels are as stored, before the layer's flip_x and flip_y. For an image, the mask is read from its attached mask layer (`mask_layer` says which); with several, ask for one of them. `max_side` is the longest side (default 512).",
            properties: json!({"layer": layer(), "what": {"type": "string", "enum": ["pixels", "mask"]}, "max_side": integer("Longest side in pixels")}),
            required: &["layer"],
            kind: Kind::Read,
            run: Action::Run(|cx, args| {
                let args = pick(args, &["layer", "what", "max_side"])?;
                let side = max_side(&args, 512)?;
                let what = args.get("what").cloned().unwrap_or(json!("pixels"));
                let export = cx.call(
                    "layer/export",
                    json!({"layer": string(&args, "layer")?, "what": what, "max_side": side}),
                )?;
                image_result(&export, "Layer")
            }),
        },
        Spec {
            name: "get_selection",
            title: "See the selection",
            description: "The selection as a grey PNG cropped to its bounds (white is selected), with its position, or a note that nothing is selected.",
            properties: json!({}),
            required: &[],
            kind: Kind::Read,
            run: Action::Run(|cx, args| {
                pick(args, &[])?;
                let export = cx.call("selection/export", json!({}))?;
                if export.is_null() {
                    return Ok(vec![ContentBlock::text("Nothing is selected.")]);
                }
                image_result(&export, "Selection mask")
            }),
        },
        Spec {
            name: "get_edit_permission",
            title: "Check edit permission",
            description: "Whether this session may edit: \"allowed\", \"denied\" or \"ask\" (the next edit asks the user in Xuan), and whether the user turned on auto mode.",
            properties: json!({}),
            required: &[],
            kind: Kind::Read,
            run: Action::Run(|cx, args| {
                pick(args, &[])?;
                text(cx.call("session/status", json!({}))?)
            }),
        },
        // Layers.
        Spec {
            name: "set_layer",
            title: "Change a layer",
            description: "Set a layer's name, visibility, lock (it can lock a layer; only the user can unlock one), opacity (0–1), blend mode (Normal, Multiply, Screen, Overlay, …), clipping (`clip_to`) and placement (x, y, width, height in document pixels, rotation in degrees). Leave out what should not change. `clip_to` clips the layer to a base below it in the same folder, so it only shows where the base has pixels: a pixel layer, or a group, whose shape is all its layers together (its opacity and mask included). The base's opacity applies to the clipped layer too. A base that is itself clipped passes on its own base. `null` releases the clipping. Groups, mask layers and filter layers cannot be clipped; mask, adjustment and filter layers cannot be bases.",
            properties: json!({
                "layer": layer(), "name": name(), "visible": {"type": "boolean"}, "locked": {"type": "boolean"},
                "opacity": number("0–1"), "blend": {"type": "string", "description": "Blend mode, e.g. Normal, Multiply, Screen, Overlay, SoftLight"},
                "clip_to": {"type": ["string", "null"], "description": "Clip to this layer or group id below the layer in the same folder; null releases the clipping"},
                "x": number("Left edge"), "y": number("Top edge"), "width": number("Width"), "height": number("Height"), "rotation": number("Degrees"),
            }),
            required: &["layer"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let args = pick(
                    args,
                    &[
                        "layer", "name", "visible", "locked", "opacity", "blend", "clip_to", "x",
                        "y", "width", "height", "rotation",
                    ],
                )?;
                // Locks protect layers from the agent: it may lock, but only
                // the user unlocks.
                if args.get("locked") == Some(&json!(false)) {
                    return Err(
                        "Only the user can unlock a layer in Xuan; ask them to unlock it".into(),
                    );
                }
                let layer = args.get("layer").cloned().unwrap_or(Value::Null);
                let mut edits = Vec::new();
                let mut set = op(
                    "set",
                    &args,
                    &["layer", "name", "visible", "locked", "opacity", "blend"],
                );
                // `null` releases the clipping, so it is passed on, unlike other nulls.
                if let Some(clip_to) = args.get("clip_to") {
                    if !(clip_to.is_null() || clip_to.is_string()) {
                        return Err("`clip_to` must be a layer id or null".into());
                    }
                    set.insert("clip_to".into(), clip_to.clone());
                }
                if set.len() > 2 {
                    edits.push(Value::Object(set));
                }
                let transform = op(
                    "transform",
                    &args,
                    &["layer", "x", "y", "width", "height", "rotation"],
                );
                if transform.len() > 2 {
                    edits.push(Value::Object(transform));
                }
                if edits.is_empty() {
                    return Err("Give at least one property to change".into());
                }
                Ok(Plan::new("Set Layer", edits, Reply::Layer(layer)))
            }),
        },
        Spec {
            name: "create_layer",
            title: "Create an empty or mask layer",
            description: "A new empty pixel layer (`kind` = \"empty\", the default) or a mask layer made from the selection (\"mask\"), the size of the canvas.",
            properties: json!({"kind": {"type": "string", "enum": ["empty", "mask"]}, "name": name(), "above": above()}),
            required: &[],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let args = pick(args, &["kind", "name", "above"])?;
                let kind = match args.get("kind").and_then(Value::as_str) {
                    None | Some("empty") => "add_empty_layer",
                    Some("mask") => "add_mask_layer",
                    Some(other) => return Err(format!("Unknown kind {other}")),
                };
                Ok(Plan::new(
                    "Create Layer",
                    vec![Value::Object(op(kind, &args, &["name", "above"]))],
                    Reply::Added,
                ))
            }),
        },
        Spec {
            name: "create_text_layer",
            title: "Create a text layer",
            description: "An editable text layer with its top-left corner at x, y. `size` is in pixels (1–1024, default 48); `family` is a font family name; `color` is #rrggbb or #rrggbbaa.",
            properties: json!({
                "text": {"type": "string"}, "x": number("Left"), "y": number("Top"), "family": {"type": "string"},
                "size": number("Font size in pixels"), "color": color("Text colour"), "bold": {"type": "boolean"},
                "italic": {"type": "boolean"}, "underline": {"type": "boolean"}, "strikethrough": {"type": "boolean"},
                "name": name(), "above": above(),
            }),
            required: &["text"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let keys = [
                    "text",
                    "x",
                    "y",
                    "family",
                    "size",
                    "color",
                    "bold",
                    "italic",
                    "underline",
                    "strikethrough",
                    "name",
                    "above",
                ];
                let args = pick(args, &keys)?;
                Ok(Plan::new(
                    "Create Text Layer",
                    vec![Value::Object(op("add_text_layer", &args, &keys))],
                    Reply::Added,
                ))
            }),
        },
        Spec {
            name: "create_shape_layer",
            title: "Create a shape layer",
            description: "An editable rectangle, ellipse or rounded rectangle filling the box x, y, width, height; or with shape path, an editable, antialiased vector shape filled inside `path` (SVG path data in document pixels, which also places it: give no x, y, width or height). The layer stays a live shape: resizing it redraws the outline.",
            properties: json!({
                "shape": {"type": "string", "enum": ["rectangle", "ellipse", "rounded_rectangle", "path"]},
                "x": number("Left"), "y": number("Top"), "width": number("Width"), "height": number("Height"),
                "path": svg_path(SVG_PATH), "fill_rule": fill_rule(),
                "color": color("Fill colour"), "corner_radius": number("For rounded rectangles"), "name": name(), "above": above(),
            }),
            required: &["shape"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let keys = [
                    "shape",
                    "x",
                    "y",
                    "width",
                    "height",
                    "path",
                    "fill_rule",
                    "color",
                    "corner_radius",
                    "name",
                    "above",
                ];
                let args = pick(args, &keys)?;
                let mut edit = op("add_shape_layer", &args, &keys);
                let shape = match args.get("shape").and_then(Value::as_str) {
                    Some("rectangle") => "Rectangle",
                    Some("ellipse") => "Ellipse",
                    Some("rounded_rectangle") => "RoundedRectangle",
                    Some("path") => "Path",
                    _ => {
                        return Err(
                            "`shape` must be rectangle, ellipse, rounded_rectangle or path".into(),
                        );
                    }
                };
                let given = |key: &str| args.get(key).is_some_and(|v| !v.is_null());
                if shape == "Path" {
                    if !given("path") {
                        return Err("shape path needs `path`, SVG path data".into());
                    }
                    if let Some(key) = ["x", "y", "width", "height"].into_iter().find(|k| given(k))
                    {
                        return Err(format!(
                            "A path shape is placed by its path's coordinates; leave out `{key}`"
                        ));
                    }
                } else {
                    if given("path") || given("fill_rule") {
                        return Err("`path` and `fill_rule` go only with shape path".into());
                    }
                    if let Some(key) = ["x", "y", "width", "height"]
                        .into_iter()
                        .find(|k| !given(k))
                    {
                        return Err(format!(
                            "shape {} needs `{key}`",
                            args["shape"].as_str().unwrap_or_default()
                        ));
                    }
                }
                edit.insert("shape".into(), json!(shape));
                Ok(Plan::new(
                    "Create Shape Layer",
                    vec![Value::Object(edit)],
                    Reply::Added,
                ))
            }),
        },
        Spec {
            name: "create_image_layer",
            title: "Add an image as a layer",
            description: "Add a PNG (base64 in `png_base64`) as a new layer at x, y; with width and/or height it is scaled to that size in document pixels.",
            properties: json!({
                "png_base64": {"type": "string"}, "x": number("Left"), "y": number("Top"), "width": number("Placed width"),
                "height": number("Placed height"), "name": name(), "above": above(), "opacity": number("0–1"),
                "blend": {"type": "string"},
            }),
            required: &["png_base64"],
            kind: Kind::Edit,
            run: Action::Edit(|cx, args| {
                let args = pick(
                    args,
                    &[
                        "png_base64",
                        "x",
                        "y",
                        "width",
                        "height",
                        "name",
                        "above",
                        "opacity",
                        "blend",
                    ],
                )?;
                let bytes = decode_png(string(&args, "png_base64")?)?;
                let path = incoming_file(cx.incoming)?;
                let mut edit = op(
                    "add_layer",
                    &args,
                    &[
                        "x", "y", "width", "height", "name", "above", "opacity", "blend",
                    ],
                );
                edit.insert("image".into(), json!(path));
                let mut plan =
                    Plan::new("Add Image Layer", vec![Value::Object(edit)], Reply::Added);
                // Removed with the plan, once Xuan has read it or on failure.
                plan.files.0.push(path.clone());
                std::fs::write(&path, bytes).map_err(|e| format!("Cannot write the image: {e}"))?;
                Ok(plan)
            }),
        },
        Spec {
            name: "delete_layer",
            title: "Delete a layer",
            description: "Delete a layer (a group with its layers).",
            properties: json!({"layer": layer()}),
            required: &["layer"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let args = pick(args, &["layer"])?;
                Ok(Plan::new(
                    "Delete Layer",
                    vec![Value::Object(op("remove_layer", &args, &["layer"]))],
                    Reply::Ok,
                ))
            }),
        },
        Spec {
            name: "merge_layers",
            title: "Merge layers",
            description: "Merge the layers into one; a single layer merges into the one below it. Returns the merged layer's id.",
            properties: json!({"layers": layers()}),
            required: &["layers"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let args = pick(args, &["layers"])?;
                Ok(Plan::new(
                    "Merge Layers",
                    vec![Value::Object(op("merge_layers", &args, &["layers"]))],
                    Reply::Added,
                ))
            }),
        },
        Spec {
            name: "group_layers",
            title: "Group layers",
            description: "Put the layers in a new group. Returns the group's id.",
            properties: json!({"layers": layers()}),
            required: &["layers"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let args = pick(args, &["layers"])?;
                Ok(Plan::new(
                    "Group Layers",
                    vec![Value::Object(op("group_layers", &args, &["layers"]))],
                    Reply::Added,
                ))
            }),
        },
        Spec {
            name: "ungroup_layer",
            title: "Ungroup",
            description: "Remove a group, keeping its layers.",
            properties: json!({"layer": layer()}),
            required: &["layer"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let args = pick(args, &["layer"])?;
                Ok(Plan::new(
                    "Ungroup Layers",
                    vec![Value::Object(op("ungroup_layers", &args, &["layer"]))],
                    Reply::Ok,
                ))
            }),
        },
        Spec {
            name: "move_layer",
            title: "Move a layer in the stack",
            description: "Move a layer up or down the stack, as dragging it in the Layers panel does. Give `above` or `below` (a layer id) to put it directly above or below that layer, in that layer's group; or give only `parent` (a group id) to put it at the top of that group. A group cannot be moved into itself.",
            properties: json!({
                "layer": layer(),
                "above": {"type": "string", "description": "Put it directly above this layer id"},
                "below": {"type": "string", "description": "Put it directly below this layer id"},
                "parent": {"type": "string", "description": "A group id: the group to move into (at its top) when neither above nor below is given; with above or below it must be that layer's group"},
            }),
            required: &["layer"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let args = pick(args, &["layer", "above", "below", "parent"])?;
                Ok(Plan::new(
                    "Move Layer",
                    vec![Value::Object(op(
                        "move_layer",
                        &args,
                        &["layer", "above", "below", "parent"],
                    ))],
                    Reply::Ok,
                ))
            }),
        },
        Spec {
            name: "select_layers",
            title: "Select layers",
            description: "Select layers in the Layers panel, as clicking them does; the last one becomes the active layer, which run_command and modify_selection act on. To act on a mask, select the mask layer (see get_document).",
            properties: json!({"layers": layers()}),
            required: &["layers"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let args = pick(args, &["layers"])?;
                let active = args["layers"].as_array().and_then(|l| l.last()).cloned();
                Ok(Plan::new(
                    "Select Layers",
                    vec![Value::Object(op("select_layers", &args, &["layers"]))],
                    Reply::Active(active.unwrap_or(Value::Null)),
                ))
            }),
        },
        // Selections.
        Spec {
            name: "select_shape",
            title: "Select a shape",
            description: "Select a rectangle or ellipse (x, y, width, height), a polygon (points) or the inside of an SVG path (`path`, with antialiased edges), combined with the current selection by `mode` (default replace). `feather` softens the new shape's edge by that many pixels (0–256) before it is combined.",
            properties: json!({
                "shape": {"type": "string", "enum": ["rectangle", "ellipse", "polygon", "path"]},
                "x": number("Left"), "y": number("Top"), "width": number("Width"), "height": number("Height"),
                "points": points(), "path": svg_path(SVG_PATH), "fill_rule": fill_rule(),
                "feather": number("Pixels to soften the shape's edge by (0–256, default 0)"), "mode": mode(),
            }),
            required: &["shape"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let args = pick(
                    args,
                    &[
                        "shape",
                        "x",
                        "y",
                        "width",
                        "height",
                        "points",
                        "path",
                        "fill_rule",
                        "feather",
                        "mode",
                    ],
                )?;
                let shape = args.get("shape").and_then(Value::as_str);
                let given = |key: &str| args.get(key).is_some_and(|v| !v.is_null());
                if shape != Some("path") && (given("path") || given("fill_rule")) {
                    return Err("`path` and `fill_rule` go only with shape path".into());
                }
                let edit = match shape {
                    Some("rectangle") => op(
                        "select_rect",
                        &args,
                        &["x", "y", "width", "height", "mode", "feather"],
                    ),
                    Some("ellipse") => {
                        let mut edit = op(
                            "select_rect",
                            &args,
                            &["x", "y", "width", "height", "mode", "feather"],
                        );
                        edit.insert("ellipse".into(), json!(true));
                        edit
                    }
                    Some("polygon") => op("select_polygon", &args, &["points", "mode", "feather"]),
                    Some("path") => {
                        if !given("path") {
                            return Err("shape path needs `path`, SVG path data".into());
                        }
                        op(
                            "select_path",
                            &args,
                            &["path", "fill_rule", "mode", "feather"],
                        )
                    }
                    _ => return Err("`shape` must be rectangle, ellipse, polygon or path".into()),
                };
                Ok(Plan::new(
                    "Select",
                    vec![Value::Object(edit)],
                    Reply::Selection,
                ))
            }),
        },
        Spec {
            name: "select_color",
            title: "Select by colour",
            description: "Select by colour in the flattened image: with x, y, the Magic Wand at that pixel (`tolerance` 0–255, default 32; `contiguous` default true); with `colors` (#rrggbb), Color Range over the whole image (`fuzziness` 0–200, default 40; `invert`).",
            properties: json!({
                "x": number("Pixel to sample"), "y": number("Pixel to sample"), "tolerance": integer("0–255"),
                "contiguous": {"type": "boolean"}, "colors": {"type": "array", "items": color("Colour")},
                "exclude": {"type": "array", "items": color("Colour")}, "fuzziness": integer("0–200"),
                "invert": {"type": "boolean"}, "mode": mode(),
            }),
            required: &[],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let args = pick(
                    args,
                    &[
                        "x",
                        "y",
                        "tolerance",
                        "contiguous",
                        "colors",
                        "exclude",
                        "fuzziness",
                        "invert",
                        "mode",
                    ],
                )?;
                let edit = if args.contains_key("colors") {
                    op(
                        "select_color_range",
                        &args,
                        &["colors", "exclude", "fuzziness", "invert", "mode"],
                    )
                } else {
                    op(
                        "select_color",
                        &args,
                        &["x", "y", "tolerance", "contiguous", "mode"],
                    )
                };
                Ok(Plan::new(
                    "Select Color",
                    vec![Value::Object(edit)],
                    Reply::Selection,
                ))
            }),
        },
        Spec {
            name: "modify_selection",
            title: "Change the selection",
            description: "`all`, `none` or `invert` the selection; `grow` or `shrink` it by `amount` pixels; `feather` its edge by `amount`; select the `subject` (runs in the background); or select a layer's opaque pixels (`layer_pixels`: `layer`, or the active layer; this makes it the active layer).",
            properties: json!({
                "action": {"type": "string", "enum": ["all", "none", "invert", "grow", "shrink", "feather", "subject", "layer_pixels"]},
                "amount": number("Pixels, for grow, shrink and feather (at most 256)"),
                "layer": {"type": "string", "description": "For layer_pixels: a layer id; the active layer when left out"},
            }),
            required: &["action"],
            kind: Kind::Edit,
            run: Action::Mixed(
                |cx, args| {
                    let args = selection_args(args)?;
                    let layer = args.get("layer").filter(|v| !v.is_null());
                    let command = match args.get("action").and_then(Value::as_str) {
                        Some("all") => "select_all",
                        Some("none") => "deselect",
                        Some("invert") => "invert_selection",
                        Some("subject") => "select_subject",
                        Some("layer_pixels") => "select_layer_pixels",
                        _ => return perform(cx, selection_edit(&args)?),
                    };
                    cx.run_on(command, layer.map(|layer| json!([layer])))?;
                    if command == "select_subject" {
                        return Ok(vec![ContentBlock::text(
                            "Select Subject is running in Xuan; the selection changes when it finishes. Check with get_document.",
                        )]);
                    }
                    selection_summary(cx)
                },
                // In a batch, `none` clears the selection with an edit; the
                // other commands cannot be part of one.
                |_, args| {
                    let args = selection_args(args)?;
                    match args.get("action").and_then(Value::as_str) {
                        Some("none") => Ok(Plan::new(
                            "Deselect",
                            vec![json!({"op": "set_selection"})],
                            Reply::Selection,
                        )),
                        Some(action @ ("all" | "invert" | "subject" | "layer_pixels")) => {
                            Err(format!(
                                "modify_selection `{action}` runs a command in Xuan, so it cannot be part of a batch; call it on its own"
                            ))
                        }
                        _ => selection_edit(&args),
                    }
                },
            ),
        },
        // Pixels.
        Spec {
            name: "paint_stroke",
            title: "Paint strokes",
            description: "Paint (or with `erase`, erase) brush strokes on a pixel layer, inside the selection if there is one. For one stroke give `points`, the path it follows (a single point paints one round dab), or `path`, SVG path data such as \"M 0 700 C 120 640 380 640 512 700\" whose curves Xuan flattens to points (one subpath; `taper_in`/`taper_out` taper its ends). For several strokes or dabs give `strokes` instead: a list of {points, color, size, …}, where what a stroke leaves out comes from the top-level arguments. All the strokes are one undo step, e.g. a field of stars as one-point strokes. `size` is the brush diameter (1–2000, default 20), `hardness` and `opacity` 0–1 (default 0.8 and 1), `color` default black. \
A point may be [x, y, pressure] with pressure 0–1 (default 1), as from a pen: it scales the size along the stroke, and the opacity too with `pressure_opacity`, e.g. [[10, 50, 0.1], [60, 40, 1], [110, 50, 0.1]] for a blade thin at both ends. \
Brush dynamics, all off by default: `taper_in` and `taper_out` grow and shrink the stroke over that many pixels at its start and end (size, and opacity with `pressure_opacity`); `spacing` paints separate dabs that far apart as a fraction of the size (e.g. 1.5 for a dotted trail, 0 for a continuous stroke); `scatter` (fraction of the size, 0–10) moves each dab randomly off the path and `scatter_count` (1–16) paints that many at each step; `size_jitter`, `opacity_jitter` and `hue_jitter` (0–1) vary each dab randomly. Scatter or jitter without `spacing` paint dabs at 0.25. `seed` picks the random pattern: the same seed repeats a stroke exactly. One call takes at most 1000 strokes, 10,000 points a stroke and 200,000 pixels of stroke length in all.",
            properties: {
                let mut properties = brush_properties();
                let mut item = properties.clone();
                item.insert("points".into(), stroke_points());
                item.insert("path".into(), svg_path("The stroke's path instead of `points`: SVG path data with one subpath (one M)"));
                properties.insert("layer".into(), optional_layer());
                properties.insert("points".into(), stroke_points());
                properties.insert("path".into(), svg_path("The path the stroke follows, instead of `points`: SVG path data in document pixels with one subpath (one M), e.g. \"M 10 50 C 40 0 80 100 110 50\". Xuan flattens its curves to points"));
                properties.insert(
                    "strokes".into(),
                    json!({
                        "type": "array", "minItems": 1, "maxItems": MAX_EDITS,
                        "items": {
                            "type": "object",
                            "properties": item,
                            "additionalProperties": false,
                        },
                        "description": "Several strokes instead of `points` or `path`; each gives `points` or `path` and takes the top-level values for what it leaves out",
                    }),
                );
                Value::Object(properties)
            },
            required: &[],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let args = pick(args, &STROKE_ARGS)?;
                Ok(Plan::new("Paint Stroke", strokes(&args)?, Reply::Ok))
            }),
        },
        Spec {
            name: "fill",
            title: "Fill",
            description: "Fill the selection (or the whole layer without one) of a pixel layer with a colour. With `path`, fill only the inside of that SVG path (and of the selection, if there is one), with antialiased edges.",
            properties: json!({
                "layer": optional_layer(), "color": color("Fill colour"),
                "path": svg_path(SVG_PATH), "fill_rule": fill_rule(),
            }),
            required: &["color"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let args = pick(args, &["layer", "color", "path", "fill_rule"])?;
                let edit = if args.get("path").is_some_and(|v| !v.is_null()) {
                    op("fill_path", &args, &["layer", "color", "path", "fill_rule"])
                } else if args.get("fill_rule").is_some_and(|v| !v.is_null()) {
                    return Err("`fill_rule` goes only with `path`".into());
                } else {
                    op("fill", &args, &["layer", "color"])
                };
                Ok(Plan::new("Fill", vec![Value::Object(edit)], Reply::Ok))
            }),
        },
        Spec {
            name: "save_path",
            title: "Save a path",
            description: "Keep an SVG path with the document under `name`, as in Xuan's Paths panel, where the user can fill, stroke or select it later. A path of the same name is replaced; without `name` it is called Path 1, Path 2, …. get_document lists the saved paths under `paths`, with their data.",
            properties: json!({"path": svg_path(SVG_PATH), "name": {"type": "string", "description": "The path's name"}}),
            required: &["path"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let args = pick(args, &["path", "name"])?;
                if args.get("path").is_none_or(Value::is_null) {
                    return Err("save_path needs `path`, SVG path data".into());
                }
                Ok(Plan::new(
                    "Save Path",
                    vec![Value::Object(op("add_path", &args, &["path", "name"]))],
                    Reply::Ok,
                ))
            }),
        },
        Spec {
            name: "fill_gradient",
            title: "Fill with a gradient",
            description: "Fill the selection (or the whole layer without one) of a pixel layer with a gradient from `start` to `end` (document pixels): linear, or with `radial` a circle around `start` reaching `end`. `stops` are two or more colours, each at a `position` from 0 (at start) to 1 (at end); beyond the ends the first and last colours continue. Colours with an alpha part let the layer show through. `opacity` 0–1 (default 1); with `mask` the gradient's brightness paints the layer's mask instead of its pixels (white shows, black hides).",
            properties: json!({
                "layer": optional_layer(),
                "start": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2, "description": "[x, y] where position 0 is"},
                "end": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2, "description": "[x, y] where position 1 is; for a radial gradient, a point on its outer edge"},
                "stops": {
                    "type": "array", "minItems": 2, "maxItems": 64,
                    "items": {
                        "type": "object",
                        "properties": {"position": number("0–1"), "color": color("Stop colour")},
                        "required": ["position", "color"],
                        "additionalProperties": false,
                    },
                    "description": "Colour stops, in any order",
                },
                "radial": {"type": "boolean", "description": "Circular instead of linear"},
                "opacity": number("0–1"),
                "mask": {"type": "boolean", "description": "Paint the layer's mask instead of its pixels"},
            }),
            required: &["start", "end", "stops"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let keys = [
                    "layer", "start", "end", "stops", "radial", "opacity", "mask",
                ];
                let args = pick(args, &keys)?;
                Ok(Plan::new(
                    "Gradient",
                    vec![Value::Object(op("gradient", &args, &keys))],
                    Reply::Ok,
                ))
            }),
        },
        Spec {
            name: "apply_filter",
            title: "Apply a filter",
            description: "Apply a filter to a pixel layer inside the selection, or with `as_layer` add it as a non-destructive filter layer (masked by the selection).",
            properties: json!({"filter": effect(FILTERS), "layer": optional_layer(), "as_layer": {"type": "boolean", "description": "Add a non-destructive filter layer instead"}}),
            required: &["filter"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let args = unquote(pick(args, &["filter", "layer", "as_layer"])?, "filter");
                let edit = if args.get("as_layer") == Some(&json!(true)) {
                    op("add_adjustment_layer", &args, &["filter"])
                } else {
                    op("apply_filter", &args, &["filter", "layer"])
                };
                Ok(Plan::new(
                    "Apply Filter",
                    vec![Value::Object(edit)],
                    Reply::Added,
                ))
            }),
        },
        Spec {
            name: "apply_adjustment",
            title: "Apply an adjustment",
            description: "Apply an adjustment to a pixel layer inside the selection, or with `as_layer` add it as a non-destructive adjustment layer (masked by the selection).",
            properties: json!({"adjustment": effect(ADJUSTMENTS), "layer": optional_layer(), "as_layer": {"type": "boolean"}}),
            required: &["adjustment"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let args = unquote(
                    pick(args, &["adjustment", "layer", "as_layer"])?,
                    "adjustment",
                );
                let edit = if args.get("as_layer") == Some(&json!(true)) {
                    op("add_adjustment_layer", &args, &["adjustment"])
                } else {
                    op("apply_adjustment", &args, &["adjustment", "layer"])
                };
                Ok(Plan::new(
                    "Apply Adjustment",
                    vec![Value::Object(edit)],
                    Reply::Added,
                ))
            }),
        },
        // The canvas.
        Spec {
            name: "crop_canvas",
            title: "Crop",
            description: "Crop the canvas to the rectangle x, y, width, height in document pixels.",
            properties: json!({"x": number("Left"), "y": number("Top"), "width": integer("Width"), "height": integer("Height")}),
            required: &["x", "y", "width", "height"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let keys = ["x", "y", "width", "height"];
                let args = pick(args, &keys)?;
                Ok(Plan::new(
                    "Crop",
                    vec![Value::Object(op("crop", &args, &keys))],
                    Reply::Size,
                ))
            }),
        },
        Spec {
            name: "resize_canvas",
            title: "Canvas size",
            description: "Change the canvas size without scaling the content; `anchor` [ax, ay] (0–1) says where the content stays: [0, 0] top-left, [0.5, 0.5] centre (default).",
            properties: json!({"width": integer("Width"), "height": integer("Height"), "anchor": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}}),
            required: &["width", "height"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let keys = ["width", "height", "anchor"];
                let args = pick(args, &keys)?;
                Ok(Plan::new(
                    "Canvas Size",
                    vec![Value::Object(op("resize_canvas", &args, &keys))],
                    Reply::Size,
                ))
            }),
        },
        Spec {
            name: "resize_image",
            title: "Image size",
            description: "Scale the whole document to width × height pixels.",
            properties: json!({"width": integer("Width"), "height": integer("Height")}),
            required: &["width", "height"],
            kind: Kind::Edit,
            run: Action::Edit(|_, args| {
                let keys = ["width", "height"];
                let args = pick(args, &keys)?;
                Ok(Plan::new(
                    "Image Size",
                    vec![Value::Object(op("resize_image", &args, &keys))],
                    Reply::Size,
                ))
            }),
        },
        // History and commands.
        Spec {
            name: "undo",
            title: "Undo",
            description: "Undo the last `steps` changes (default 1, at most 20), as Edit → Undo does.",
            properties: json!({"steps": integer("1–20")}),
            required: &[],
            kind: Kind::Edit,
            run: Action::Run(|cx, args| history(cx, args, "undo")),
        },
        Spec {
            name: "redo",
            title: "Redo",
            description: "Redo the last `steps` undone changes (default 1, at most 20).",
            properties: json!({"steps": integer("1–20")}),
            required: &[],
            kind: Kind::Edit,
            run: Action::Run(|cx, args| history(cx, args, "redo")),
        },
        Spec {
            name: "run_command",
            title: "Run an editor command",
            description: "Run one of Xuan's menu commands on the current document, as its menu item does: flatten, duplicate (the selected layers), new_layer, delete_layer, move_out (of its group), mask (attach a mask layer made from the selection to the image), new_mask_layer, delete_mask, disable_mask, link_mask (these three on a mask layer), clip (clipping mask), flip_h, flip_v (the layer), flip_canvas_h, flip_canvas_v, invert (the layer's pixels), clear (the selected pixels), fill_fg, fill_bg, content_fill (fill the selection from its surroundings), remove_background, remove_flat_background, fit, actual, zoom_in, zoom_out. Commands act on the selected layers, the last one active: give `layers` to select them first, as select_layers does. Returns the ids of the layers the command added. content_fill, remove_background and remove_flat_background run in the background: other edits fail with \"The editor is busy\" until they finish.",
            properties: json!({
                "command": {"type": "string", "enum": COMMANDS},
                "layers": {"type": "array", "items": {"type": "string"}, "minItems": 1, "description": "Layer ids to select first, the last one active (default: the layers selected now); only for commands that edit"},
            }),
            required: &["command"],
            kind: Kind::Edit,
            run: Action::Run(|cx, args| {
                let args = pick(args, &["command", "layers"])?;
                let command = string(&args, "command")?;
                if !COMMANDS.contains(&command) {
                    return Err(format!(
                        "`{command}` is not one of the commands this tool runs"
                    ));
                }
                let answer = cx.run_on(command, args.get("layers").cloned())?;
                // An older Xuan does not say; these always start a job.
                let running = (answer.get("running").and_then(Value::as_bool))
                    .unwrap_or(BACKGROUND.contains(&command));
                if running {
                    return Ok(vec![ContentBlock::text(format!(
                        "`{command}` is running in Xuan; the document changes when it finishes, and until then other edits fail with \"The editor is busy\". Check with get_document or get_preview."
                    ))]);
                }
                text(
                    json!({"ok": true, "layers": answer.get("layers").cloned().unwrap_or(json!([]))}),
                )
            }),
        },
        Spec {
            name: "batch",
            title: "Several edits as one step",
            description: "Apply several edits in one request to Xuan, as ONE undo step: `steps` is a list of {\"tool\": name, \"arguments\": {…}}, each an edit tool with the arguments it takes on its own. The steps run in order, each on the result of the ones before. A later step can name a layer an earlier step created as \"$1\", \"$2\", …: the n-th layer the batch has created so far, in any layer argument (`layer`, `layers`, `above`, `below`, `parent`, `clip_to`). create_layer, create_text_layer, create_shape_layer, create_image_layer, merge_layers, group_layers, and apply_filter or apply_adjustment with `as_layer` each create one. For example create_text_layer, then set_layer with \"layer\": \"$1\" to rotate it. If any step fails, nothing in the batch is applied and the error names the step. The tools a batch takes: set_layer, create_layer, create_text_layer, create_shape_layer, create_image_layer, delete_layer, merge_layers, group_layers, ungroup_layer, move_layer, select_layers, select_shape, select_color, modify_selection (only none, grow, shrink and feather), paint_stroke, fill, save_path, fill_gradient, apply_filter, apply_adjustment, crop_canvas, resize_canvas and resize_image. Reading tools, history, commands (run_command), documents and files cannot be batched. At most 1000 edits in all (a stroke is one edit; set_layer with both properties and placement is two). `name` names the undo step. Returns the ids of the layers the batch created, in order.",
            properties: json!({
                "name": {"type": "string", "description": "The undo step's name, e.g. \"Stars\""},
                "steps": {
                    "type": "array", "minItems": 1, "maxItems": MAX_EDITS,
                    "items": {
                        "type": "object",
                        "properties": {
                            "tool": {"type": "string", "description": "An edit tool's name"},
                            "arguments": {"type": "object", "description": "The tool's arguments"},
                        },
                        "required": ["tool"],
                        "additionalProperties": false,
                    },
                },
            }),
            required: &["steps"],
            kind: Kind::Edit,
            run: Action::Run(batch),
        },
        // Documents and files.
        Spec {
            name: "switch_document",
            title: "Switch document",
            description: "Make an open document the current one, as clicking its tab does.",
            properties: json!({"document": {"type": "string"}}),
            required: &["document"],
            kind: Kind::Read,
            run: Action::Run(|cx, args| {
                let args = pick(args, &["document"])?;
                cx.call("document/activate", Value::Object(args))?;
                text(json!({"ok": true}))
            }),
        },
        Spec {
            name: "save_document",
            title: "Save the project",
            description: concat!(
                "Save a document (the current one by default) as a .xuan project. ",
                "With an absolute `path` (ending in .xuan, in a folder that exists), Xuan asks the user in its own prompt that names the file and folder; ",
                "the user may answer Always Allow, and then later saves and exports to a path happen without asking. ",
                "With `in_place: true`, save the document back to its own .xuan file, as Ctrl+S does, asking the same way. ",
                "Without either, Xuan shows its save dialog with `suggested_name` and the user chooses where. ",
                "An existing file is replaced only with `overwrite: true`, and even with Always Allow, replacing a file Xuan did not write since it started asks the user.",
                " Returns the file name (never the folder), with `asked: false` when it was written without asking, or an error if the user cancelled."
            ),
            properties: json!({
                "document": {"type": "string"},
                "suggested_name": {"type": "string", "description": "The name the save dialog suggests, without `path`"},
                "path": {"type": "string", "description": "Where to save, an absolute path ending in .xuan"},
                "overwrite": {"type": "boolean", "description": "Allow replacing an existing file at `path`"},
                "in_place": {"type": "boolean", "description": "Save to the document's own .xuan file"},
            }),
            required: &[],
            kind: Kind::File,
            run: Action::Run(|cx, args| {
                let mut args = pick(
                    args,
                    &[
                        "document",
                        "suggested_name",
                        "path",
                        "overwrite",
                        "in_place",
                    ],
                )?;
                let in_place = args.remove("in_place");
                if in_place
                    .as_ref()
                    .is_some_and(|value| !value.is_null() && !value.is_boolean())
                {
                    return Err("`in_place` must be true or false".into());
                }
                if in_place == Some(json!(true)) {
                    if let Some(key) = ["suggested_name", "path", "overwrite"]
                        .into_iter()
                        .find(|key| args.contains_key(*key))
                    {
                        return Err(format!(
                            "`in_place` saves to the document's own file; leave out `{key}`"
                        ));
                    }
                    return text(cx.call("file/save", Value::Object(args))?);
                }
                text(cx.call("file/save_as", Value::Object(args))?)
            }),
        },
        Spec {
            name: "export_document",
            title: "Export an image",
            description: concat!(
                "Export a document (the current one by default) as png (default), jpg, tiff or webp. ",
                "With an absolute `path` (ending in .png, .jpg, .jpeg, .tif, .tiff or .webp, which picks the format, in a folder that exists), Xuan asks the user in its own prompt that names the file and folder; ",
                "the user may answer Always Allow, and then later saves and exports to a path happen without asking. ",
                "Without `path`, Xuan shows its save dialog with `suggested_name` and the user chooses where. ",
                "An existing file is replaced only with `overwrite: true`, and even with Always Allow, replacing a file Xuan did not write since it started asks the user.",
                " Returns the file name (never the folder), with `asked: false` when it was written without asking."
            ),
            properties: json!({
                "document": {"type": "string"},
                "format": {"type": "string", "enum": ["png", "jpg", "tiff", "webp"]},
                "suggested_name": {"type": "string", "description": "The name the save dialog suggests, without `path`"},
                "path": {"type": "string", "description": "Where to export, an absolute path with an image extension"},
                "overwrite": {"type": "boolean", "description": "Allow replacing an existing file at `path`"},
            }),
            required: &[],
            kind: Kind::File,
            run: Action::Run(|cx, args| {
                let args = pick(
                    args,
                    &["document", "format", "suggested_name", "path", "overwrite"],
                )?;
                text(cx.call("file/export", Value::Object(args))?)
            }),
        },
        Spec {
            name: "open_document",
            title: "Open a file",
            description: "Ask the user to open the image or .xuan project at the absolute `path` as a new document; Xuan shows the path and opens it only if the user agrees.",
            properties: json!({"path": {"type": "string"}}),
            required: &["path"],
            kind: Kind::File,
            run: Action::Run(|cx, args| {
                let args = pick(args, &["path"])?;
                text(cx.call("file/open", Value::Object(args))?)
            }),
        },
    ]
}

/// The tools, as `tools/list` describes them.
pub fn list() -> Vec<Tool> {
    specs()
        .into_iter()
        .map(|spec| {
            let mut schema = JsonObject::new();
            schema.insert("type".into(), json!("object"));
            schema.insert("properties".into(), spec.properties.clone());
            if !spec.required.is_empty() {
                schema.insert("required".into(), json!(spec.required));
            }
            schema.insert("additionalProperties".into(), json!(false));
            let annotations = ToolAnnotations::with_title(spec.title)
                .read_only(spec.kind == Kind::Read)
                .destructive(matches!(
                    spec.name,
                    "delete_layer"
                        | "merge_layers"
                        | "crop_canvas"
                        | "resize_canvas"
                        | "resize_image"
                        | "run_command"
                        | "batch"
                ))
                .open_world(false);
            let description = match spec.kind {
                Kind::Read => spec.description.to_owned(),
                Kind::Edit => format!("{} {EDIT_WAITS}", spec.description),
                Kind::File => format!("{} {FILE_WAITS}", spec.description),
            };
            Tool::new(spec.name, description, Arc::new(schema))
                .with_title(spec.title)
                .with_annotations(annotations)
        })
        .collect()
}

/// What the client is told while a call waits: the user sees it in clients
/// that show progress.
pub fn waiting_message(name: &str) -> &'static str {
    match specs()
        .into_iter()
        .find(|spec| spec.name == name)
        .map(|spec| spec.kind)
    {
        Some(Kind::Edit) => "Waiting for the user to allow edits in Xuan",
        Some(Kind::File) => "Waiting for the user to answer in Xuan",
        _ => "Waiting for Xuan",
    }
}

/// For the pane's activity: what a save or export wrote, such as
/// "exported out.png without asking". Only the file name Xuan answered with,
/// stripped of control and bidi characters.
pub fn written(name: &str, result: &CallToolResult) -> Option<String> {
    let verb = match name {
        "save_document" => "saved",
        "export_document" => "exported",
        _ => return None,
    };
    if result.is_error == Some(true) {
        return None;
    }
    let text = (result.content.iter()).find_map(|content| content.as_text())?;
    let answer: Value = serde_json::from_str(&text.text).ok()?;
    let file: String = (answer.get("name")?.as_str()?.chars())
        .filter(|&c| {
            !c.is_control()
                && !matches!(
                    c,
                    '\u{200B}'..='\u{200F}'
                        | '\u{202A}'..='\u{202E}'
                        | '\u{2066}'..='\u{2069}'
                        | '\u{061C}'
                        | '\u{FEFF}'
                )
        })
        .take(120)
        .collect();
    Some(if answer.get("asked") == Some(&Value::Bool(false)) {
        format!("{verb} {file} without asking")
    } else {
        format!("{verb} {file}")
    })
}

/// Run a tool. Errors from Xuan or from the arguments become a tool error
/// the model can read, not a protocol error.
pub fn call(cx: &Context, name: &str, args: Map<String, Value>) -> CallToolResult {
    let Some(spec) = specs().into_iter().find(|spec| spec.name == name) else {
        return CallToolResult::error(vec![ContentBlock::text(format!("Unknown tool {name}"))]);
    };
    let result = match spec.run {
        Action::Run(run) | Action::Mixed(run, _) => run(cx, args),
        Action::Edit(plan) => plan(cx, args).and_then(|plan| perform(cx, plan)),
    };
    match result {
        Ok(content) => CallToolResult::success(content),
        // Where the user keeps files is not the client's business.
        Err(message) => CallToolResult::error(vec![ContentBlock::text(redact_paths(&message))]),
    }
}

impl Context<'_> {
    /// A request, with Xuan's error turned into a message for the model.
    pub fn call(&self, method: &str, params: Value) -> Result<Value, String> {
        // Refused here with a clear message rather than sent: Xuan would
        // stop the whole plugin over a message that large.
        let size = serde_json::to_vec(&params).map_or(0, |bytes| bytes.len());
        if size > MAX_REQUEST_BYTES {
            return Err(format!(
                "The request is too large for Xuan ({} MiB of JSON; at most 16 MiB). Send less at once, for example fewer points or a shorter text",
                size.div_ceil(1024 * 1024)
            ));
        }
        self.editor
            .request(self.session, method, params, self.cancel)
            .map_err(|error| explain(method, &error))
    }

    fn edit(&self, name: &str, edits: Vec<Value>) -> Result<Value, String> {
        if edits.len() > MAX_EDITS {
            return Err(format!(
                "Xuan takes at most {MAX_EDITS} edits in one request and this has {}; split it into several calls",
                edits.len()
            ));
        }
        self.call("document/edit", json!({"name": name, "edits": edits}))
    }

    fn run(&self, action: &str) -> Result<Value, String> {
        self.run_on(action, None)
    }

    /// Run a command on `layers`, selected first, or on the selected layers.
    fn run_on(&self, action: &str, layers: Option<Value>) -> Result<Value, String> {
        let mut params = json!({"action": action});
        if let Some(layers) = layers.filter(|layers| !layers.is_null()) {
            params["layers"] = layers;
        }
        self.call("host/run", params)
    }
}

/// The tools a batch takes.
pub fn batchable() -> Vec<&'static str> {
    specs()
        .into_iter()
        .filter(|spec| !matches!(spec.run, Action::Run(_)))
        .map(|spec| spec.name)
        .collect()
}

/// Send a tool's edits as one request and answer as the tool does.
fn perform(cx: &Context, plan: Plan) -> Result<Vec<ContentBlock>, String> {
    let answer = cx.edit(plan.name, plan.edits)?;
    match plan.reply {
        Reply::Ok => text(json!({"ok": true})),
        Reply::Layer(layer) => text(json!({"ok": true, "layer": layer})),
        Reply::Active(active) => text(json!({"ok": true, "active": active})),
        Reply::Added => added(answer),
        Reply::Selection => selection_summary(cx),
        Reply::Size => size_summary(cx),
    }
}

/// The `batch` tool: every step's edits in one `document/edit` request, so
/// they are one undo step, and Xuan applies all of them or none.
fn batch(cx: &Context, args: Map<String, Value>) -> Result<Vec<ContentBlock>, String> {
    let args = pick(args, &["name", "steps"])?;
    let name = match args.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) if !name.trim().is_empty() && name.chars().count() <= 100 => {
            Some(name.trim().to_owned())
        }
        Some(_) => return Err("`name` must be a short text (at most 100 characters)".into()),
    };
    let steps = (args.get("steps").and_then(Value::as_array))
        .filter(|steps| !steps.is_empty())
        .ok_or("`steps` must be a list of at least one {\"tool\", \"arguments\"}")?;
    let specs = specs();
    let mut plans: Vec<(&str, Plan)> = Vec::with_capacity(steps.len());
    for (index, step) in steps.iter().enumerate() {
        let number = index + 1;
        let step = step
            .as_object()
            .ok_or_else(|| format!("Step {number} must be {{\"tool\", \"arguments\"}}"))?;
        if let Some(unknown) =
            (step.keys()).find(|key| !matches!(key.as_str(), "tool" | "arguments"))
        {
            return Err(format!(
                "Step {number}: unknown key `{unknown}`; a step is {{\"tool\", \"arguments\"}}"
            ));
        }
        let tool = (step.get("tool").and_then(Value::as_str))
            .ok_or_else(|| format!("Step {number}: `tool` must be a tool's name"))?;
        let arguments = match step.get("arguments") {
            None | Some(Value::Null) => Map::new(),
            Some(Value::Object(arguments)) => arguments.clone(),
            Some(_) => return Err(format!("Step {number}: `arguments` must be an object")),
        };
        let spec = (specs.iter().find(|spec| spec.name == tool))
            .ok_or_else(|| format!("Step {number}: there is no tool `{tool}`"))?;
        let planner = match spec.run {
            Action::Edit(planner) | Action::Mixed(_, planner) => planner,
            Action::Run(_) => {
                return Err(format!(
                    "Step {number}: {} cannot be part of a batch, which takes only these edit tools: {}. Call it on its own, before or after the batch. Nothing was changed.",
                    spec.name,
                    batchable().join(", ")
                ));
            }
        };
        let plan = planner(cx, arguments).map_err(|error| {
            format!(
                "Step {number} ({}): {error}. Nothing was changed.",
                spec.name
            )
        })?;
        plans.push((spec.name, plan));
    }
    // Which step each edit came from, to say which one Xuan refused.
    let mut owners = Vec::new();
    let mut edits = Vec::new();
    for (index, (_, plan)) in plans.iter_mut().enumerate() {
        owners.extend(std::iter::repeat_n(index, plan.edits.len()));
        edits.append(&mut plan.edits);
    }
    let name = name.unwrap_or_else(|| {
        let first = plans[0].1.name;
        if plans.iter().all(|(_, plan)| plan.name == first) {
            first.to_owned()
        } else {
            "Batch Edit".to_owned()
        }
    });
    let answer = cx
        .edit(&name, edits)
        .map_err(|error| blame(error, &owners, &plans))?;
    let mut result = json!({
        "ok": true,
        "steps": plans.len(),
        "layers": answer.get("layers").cloned().unwrap_or(json!([])),
    });
    let selection = plans.iter().any(|(_, plan)| plan.reply == Reply::Selection);
    let size = plans.iter().any(|(_, plan)| plan.reply == Reply::Size);
    if selection || size {
        let document = document(cx)?;
        if selection {
            result["selection"] = document["selection"].clone();
        }
        if size {
            result["width"] = document["width"].clone();
            result["height"] = document["height"].clone();
        }
    }
    text(result)
}

/// Xuan's error for a batch, naming the step whose edit it refused. Xuan
/// says `Edit 3 (stroke): …` when a request of several edits fails.
fn blame(error: String, owners: &[usize], plans: &[(&str, Plan)]) -> String {
    let Some(message) = error.strip_prefix("Xuan: ") else {
        // Not Xuan refusing an edit: the user, a timeout or the size.
        return error;
    };
    let numbered = (message.strip_prefix("Edit ")).and_then(|rest| {
        let (number, rest) = rest.split_once(' ')?;
        let (_, detail) = rest.split_once("): ")?;
        Some((number.parse::<usize>().ok()?.checked_sub(1)?, detail))
    });
    let (step, detail) = match numbered {
        Some((edit, detail)) => (owners.get(edit).copied(), detail),
        None if owners.len() == 1 => (Some(owners[0]), message),
        None => (None, message),
    };
    match step {
        Some(step) => format!(
            "Step {} ({}): Xuan: {detail}. Nothing in the batch was applied.",
            step + 1,
            plans[step].0
        ),
        None => format!("{error}. Nothing in the batch was applied."),
    }
}

/// `modify_selection`'s arguments; `layer` goes only with `layer_pixels`.
fn selection_args(args: Map<String, Value>) -> Result<Map<String, Value>, String> {
    let args = pick(args, &["action", "amount", "layer"])?;
    let layer = args.get("layer").filter(|v| !v.is_null());
    if layer.is_some() && args.get("action") != Some(&json!("layer_pixels")) {
        return Err("`layer` goes only with layer_pixels".into());
    }
    Ok(args)
}

/// `modify_selection`'s edits for grow, shrink and feather.
fn selection_edit(args: &Map<String, Value>) -> Result<Plan, String> {
    let amount = args.get("amount").and_then(Value::as_f64).unwrap_or(0.0);
    match args.get("action").and_then(Value::as_str) {
        Some(action @ ("grow" | "shrink")) => {
            let by = if action == "grow" { amount } else { -amount };
            Ok(Plan::new(
                "Modify Selection",
                vec![json!({"op": "grow_selection", "by": by.round() as i64})],
                Reply::Selection,
            ))
        }
        Some("feather") => Ok(Plan::new(
            "Feather Selection",
            vec![json!({"op": "feather_selection", "radius": amount})],
            Reply::Selection,
        )),
        _ => Err("Unknown `action`".into()),
    }
}

/// `paint_stroke`'s arguments.
const STROKE_ARGS: [&str; 19] = [
    "layer",
    "points",
    "path",
    "strokes",
    "color",
    "size",
    "hardness",
    "opacity",
    "erase",
    "pressure_opacity",
    "spacing",
    "taper_in",
    "taper_out",
    "scatter",
    "scatter_count",
    "size_jitter",
    "opacity_jitter",
    "hue_jitter",
    "seed",
];
/// What each stroke of `strokes` may set, the top-level value otherwise.
const BRUSH: [&str; 15] = [
    "color",
    "size",
    "hardness",
    "opacity",
    "erase",
    "pressure_opacity",
    "spacing",
    "taper_in",
    "taper_out",
    "scatter",
    "scatter_count",
    "size_jitter",
    "opacity_jitter",
    "hue_jitter",
    "seed",
];

/// The schema of the brush settings `paint_stroke` and each of its
/// `strokes` take.
fn brush_properties() -> Map<String, Value> {
    let properties = json!({
        "color": color("Paint colour"),
        "size": number("Brush diameter, 1–2000 (default 20)"),
        "hardness": number("0–1 (default 0.8)"),
        "opacity": number("0–1 (default 1)"),
        "erase": {"type": "boolean"},
        "pressure_opacity": {"type": "boolean", "description": "Point pressure and taper also scale the opacity"},
        "spacing": {"type": "number", "minimum": 0, "maximum": 10, "description": "Distance between dabs as a fraction of the size, e.g. 0.25; 0 (default) is continuous"},
        "taper_in": {"type": "number", "minimum": 0, "description": "Pixels over which the stroke grows from nothing at its start"},
        "taper_out": {"type": "number", "minimum": 0, "description": "Pixels over which the stroke shrinks to nothing at its end"},
        "scatter": {"type": "number", "minimum": 0, "maximum": 10, "description": "How far dabs move randomly off the path, as a fraction of the size"},
        "scatter_count": {"type": "integer", "minimum": 1, "maximum": 16, "description": "Dabs at each spacing step (default 1)"},
        "size_jitter": {"type": "number", "minimum": 0, "maximum": 1, "description": "How much smaller each dab may randomly be"},
        "opacity_jitter": {"type": "number", "minimum": 0, "maximum": 1, "description": "How much more transparent each dab may randomly be"},
        "hue_jitter": {"type": "number", "minimum": 0, "maximum": 1, "description": "How far each dab's hue may randomly turn; 1 is up to half the colour wheel either way"},
        "seed": {"type": "integer", "minimum": 0, "description": "Random pattern for scatter and jitter (default 0); the same seed repeats the stroke exactly"},
    });
    match properties {
        Value::Object(map) => map,
        _ => unreachable!(),
    }
}

/// `paint_stroke`'s `stroke` edits: one for `points` or `path`, or one per
/// item of `strokes` with the top-level brush for what it leaves out.
fn strokes(args: &Map<String, Value>) -> Result<Vec<Value>, String> {
    let given = |key: &str| args.get(key).filter(|value| !value.is_null());
    let mut shared = op("stroke", args, &["layer"]);
    shared.extend(op("stroke", args, &BRUSH));
    let line = match (given("points"), given("path")) {
        (Some(_), Some(_)) => return Err("Give `points` or `path`, not both".into()),
        (Some(points), None) => Some(("points", points)),
        (None, Some(path)) => Some(("path", path)),
        (None, None) => None,
    };
    match (line, given("strokes")) {
        (Some(_), Some(_)) => {
            Err("Give `points` or `path` for one stroke or `strokes` for several, not both".into())
        }
        (None, None) => {
            Err("Give `points` or `path` for one stroke or `strokes` for several".into())
        }
        (Some((key, line)), None) => {
            shared.insert(key.into(), line.clone());
            Ok(vec![Value::Object(shared)])
        }
        (None, Some(list)) => {
            let list = (list.as_array()).filter(|list| !list.is_empty()).ok_or(
                "`strokes` must be a list of at least one {\"points\": …} or {\"path\": …}",
            )?;
            let strokes = list.iter().enumerate().map(|(index, stroke)| {
                let number = index + 1;
                let stroke = stroke.as_object().ok_or_else(|| {
                    format!("Stroke {number} must be an object with `points` or `path`")
                })?;
                if let Some(unknown) = (stroke.keys()).find(|key| {
                    !matches!(key.as_str(), "points" | "path") && !BRUSH.contains(&key.as_str())
                }) {
                    return Err(format!("Stroke {number}: unknown argument `{unknown}`"));
                }
                let has = |key: &str| stroke.get(key).is_some_and(|v| !v.is_null());
                match (has("points"), has("path")) {
                    (false, false) => {
                        return Err(format!("Stroke {number} has no `points` or `path`"));
                    }
                    (true, true) => {
                        return Err(format!(
                            "Stroke {number}: give `points` or `path`, not both"
                        ));
                    }
                    _ => {}
                }
                let mut edit = shared.clone();
                edit.extend(
                    (stroke.iter())
                        .filter(|(_, value)| !value.is_null())
                        .map(|(key, value)| (key.clone(), value.clone())),
                );
                Ok(Value::Object(edit))
            });
            strokes.collect()
        }
    }
}

/// What the model is told when Xuan refuses.
fn explain(method: &str, error: &EditorError) -> String {
    if matches!(error.code, WITHDRAWN | TIMED_OUT) {
        return not_answered(method);
    }
    if error.code != CANCELLED {
        return format!("Xuan: {}", error.message);
    }
    match method {
        "document/edit" | "host/run" => "The user did not allow edits from this session in Xuan. Ask the user before trying again.".into(),
        "file/save_as" | "file/export" | "file/save" | "file/open" => "The user cancelled.".into(),
        _ if method.ends_with("/export") => "The user did not allow sending the image to this MCP server.".into(),
        _ => format!("Xuan: {}", error.message),
    }
}

/// What the model is told when the server stopped waiting for the user: the
/// request was withdrawn in Xuan, its prompt closed, and nothing happened.
/// Retrying at once would only ask the user again.
pub fn not_answered(method: &str) -> String {
    let what = match method {
        "document/edit" | "host/run" => {
            "The user has not yet answered Xuan's prompt to allow edits from this session"
        }
        "file/save_as" | "file/export" | "file/save" | "file/open" => {
            "The user has not yet answered Xuan's dialog for this request"
        }
        _ if method.ends_with("/export") => {
            "The user has not yet answered Xuan's prompt to allow sending the image to this MCP server"
        }
        _ => "Xuan did not answer in time",
    };
    format!(
        "{what}, so the request was withdrawn and nothing was changed. Do not retry right away: ask the user whether they want this, and try again once they say so."
    )
}

/// The arguments, refusing names the tool does not take.
fn pick(args: Map<String, Value>, allowed: &[&str]) -> Result<Map<String, Value>, String> {
    if let Some(unknown) = args.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("Unknown argument `{unknown}`"));
    }
    Ok(args)
}

/// The arguments with `key` read as JSON when a client sent it as a string,
/// as some send `"\"Invert\""` or an object in quotes.
fn unquote(mut args: Map<String, Value>, key: &str) -> Map<String, Value> {
    if let Some(Value::String(text)) = args.get(key) {
        let text = text.trim();
        let value = serde_json::from_str::<Value>(text)
            .ok()
            .filter(|value| value.is_string() || value.is_object())
            .unwrap_or_else(|| json!(text));
        args.insert(key.into(), value);
    }
    args
}

/// An edit op with the given arguments copied over when present.
fn op(name: &str, args: &Map<String, Value>, keys: &[&str]) -> Map<String, Value> {
    let mut edit = Map::new();
    edit.insert("op".into(), json!(name));
    for key in keys {
        if let Some(value) = args.get(*key).filter(|v| !v.is_null()) {
            edit.insert((*key).into(), value.clone());
        }
    }
    edit
}

fn string<'a>(args: &'a Map<String, Value>, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("`{key}` must be a string"))
}

fn max_side(args: &Map<String, Value>, default: u64) -> Result<u64, String> {
    match args.get("max_side") {
        None | Some(Value::Null) => Ok(default),
        Some(value) => value
            .as_u64()
            .map(|side| side.clamp(64, 4096))
            .ok_or_else(|| "`max_side` must be a positive integer".into()),
    }
}

fn text(value: Value) -> Result<Vec<ContentBlock>, String> {
    Ok(vec![ContentBlock::text(
        serde_json::to_string_pretty(&value).unwrap_or_default(),
    )])
}

fn added(answer: Value) -> Result<Vec<ContentBlock>, String> {
    text(json!({"ok": true, "layers": answer.get("layers").cloned().unwrap_or(json!([]))}))
}

fn document(cx: &Context) -> Result<Value, String> {
    let document = cx.call("document/get", json!({}))?;
    if document.is_null() {
        return Err("No document is open in Xuan".into());
    }
    Ok(document)
}

fn selection_summary(cx: &Context) -> Result<Vec<ContentBlock>, String> {
    text(json!({"ok": true, "selection": document(cx)?["selection"]}))
}

fn size_summary(cx: &Context) -> Result<Vec<ContentBlock>, String> {
    let document = document(cx)?;
    text(json!({"ok": true, "width": document["width"], "height": document["height"]}))
}

fn history(
    cx: &Context,
    args: Map<String, Value>,
    action: &str,
) -> Result<Vec<ContentBlock>, String> {
    let args = pick(args, &["steps"])?;
    let steps = args.get("steps").and_then(Value::as_u64).unwrap_or(1);
    if !(1..=20).contains(&steps) {
        return Err("`steps` must be between 1 and 20".into());
    }
    for done in 0..steps {
        if let Err(error) = cx.run(action) {
            if done == 0 {
                return Err(error);
            }
            return text(json!({"ok": true, "steps": done, "note": error}));
        }
    }
    text(json!({"ok": true, "steps": steps}))
}

/// Read an exported PNG into an image result and remove the file.
pub fn image_result(export: &Value, what: &str) -> Result<Vec<ContentBlock>, String> {
    let (data, info) = read_export(export)?;
    Ok(vec![
        ContentBlock::image(data, "image/png"),
        ContentBlock::text(format!("{what}: {info}")),
    ])
}

/// The base64 PNG of an export Xuan wrote, and where it sits, as text.
pub fn read_export(export: &Value) -> Result<(String, Value), String> {
    let path = export
        .get("path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .ok_or("Xuan did not write the image")?;
    let name = path.file_name().unwrap_or_default().to_string_lossy();
    let bytes = std::fs::read(&path).map_err(|e| format!("Cannot read {name}: {e}"));
    let _ = std::fs::remove_file(&path);
    let mut info = export.clone();
    if let Some(map) = info.as_object_mut() {
        map.remove("path");
    }
    Ok((STANDARD.encode(bytes?), info))
}

fn decode_png(text: &str) -> Result<Vec<u8>, String> {
    let text = text.trim();
    let text = text.strip_prefix("data:image/png;base64,").unwrap_or(text);
    if text.len() > MAX_IMAGE_BYTES / 3 * 4 + 4 {
        return Err("The image is larger than 64 MiB".into());
    }
    let bytes = STANDARD
        .decode(text)
        .map_err(|_| "`png_base64` is not base64".to_owned())?;
    if !bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Err("`png_base64` is not a PNG".into());
    }
    Ok(bytes)
}

fn incoming_file(dir: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("Cannot create the plugin's folder for images: {e}"))?;
    Ok(dir.join(format!("{}.png", &crate::auth::generate()[..24])))
}

/// A message with every absolute path in it reduced to its file name, so
/// errors from Xuan or from the system never tell a client where the user
/// keeps files. Paths in quotes are taken whole, spaces included; others
/// end at a space. URLs and relative paths are left alone.
pub fn redact_paths(message: &str) -> String {
    let chars: Vec<char> = message.chars().collect();
    let mut out = String::with_capacity(message.len());
    let mut i = 0;
    while i < chars.len() {
        let before = i.checked_sub(1).map(|j| chars[j]);
        let boundary = before.is_none_or(|c| {
            c.is_whitespace()
                || matches!(
                    c,
                    '"' | '\'' | '`' | '(' | '[' | '{' | '<' | '=' | ',' | ';'
                )
        });
        let length = if boundary { path_start(&chars[i..]) } else { 0 };
        if length == 0 {
            out.push(chars[i]);
            i += 1;
            continue;
        }
        // The path runs to the closing quote, or else to a space.
        let quote = before.filter(|c| matches!(c, '"' | '\'' | '`'));
        let ends = |c: char| {
            c.is_whitespace() || matches!(c, '"' | '\'' | '`' | ')' | ']' | '}' | '>' | ',' | ';')
        };
        let mut end = i + length;
        while end < chars.len() {
            let c = chars[end];
            let stop = match quote {
                Some(q) => c == q,
                None => ends(c),
            };
            if !stop {
                end += 1;
                continue;
            }
            // `/home/me/My Pictures/a.png`: a folder name with a space goes
            // on while the next word still holds a separator.
            if quote.is_none() && c == ' ' {
                let word = (end + 1..chars.len())
                    .find(|&j| ends(chars[j]))
                    .unwrap_or(chars.len());
                let next = &chars[end + 1..word];
                if path_start(next) == 0 && next.iter().any(|&c| matches!(c, '/' | '\\')) {
                    end = word;
                    continue;
                }
            }
            break;
        }
        // A sentence's full stop or colon is not part of the path.
        if quote.is_none() {
            while end > i + length && matches!(chars[end - 1], '.' | ':') {
                end -= 1;
            }
        }
        let path: String = chars[i..end].iter().collect();
        let name = path
            .split(['/', '\\'])
            .rfind(|part| !part.is_empty())
            .unwrap_or_default();
        out.push_str(name);
        i = end;
    }
    out
}

/// How many characters start an absolute path here (0: none): `/x` on
/// Unix, `C:\x` or `C:/x` on Windows (also with `\\` as `Debug` writes
/// it), `\\server` for a share.
fn path_start(chars: &[char]) -> usize {
    let at = |i: usize| chars.get(i).copied();
    let named = |c: Option<char>| {
        c.is_some_and(|c| !c.is_whitespace() && !matches!(c, '/' | '\\' | '"' | '\'' | '`'))
    };
    match (at(0), at(1), at(2)) {
        (Some('/'), next, _) if named(next) => 1,
        (Some(drive), Some(':'), Some('/' | '\\'))
            if drive.is_ascii_alphabetic()
                && at(3).is_some_and(|c| !c.is_whitespace() && !matches!(c, '"' | '\'' | '`')) =>
        {
            3
        }
        (Some('\\'), Some('\\'), Some(c))
            if !c.is_whitespace() && !matches!(c, '"' | '\'' | '`') =>
        {
            2
        }
        _ => 0,
    }
}
