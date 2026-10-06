//! The MCP tools and how each maps onto plugin protocol requests. Every
//! tool that changes a document sends one `document/edit` or `host/run`
//! request, so it is one undo step in Xuan and shows at once.
use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use base64::{Engine, engine::general_purpose::STANDARD};
use rmcp::model::{CallToolResult, ContentBlock, JsonObject, Tool, ToolAnnotations};
use serde_json::{Map, Value, json};

use crate::editor::{CANCELLED, Editor, EditorError};

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
}

type Run = fn(&Context, Map<String, Value>) -> Result<Vec<ContentBlock>, String>;

/// One tool: its MCP definition and what it does.
struct Spec {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    /// Properties of the input object and the required ones.
    properties: Value,
    required: &'static [&'static str],
    kind: Kind,
    run: Run,
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
fn name() -> Value {
    json!({"type": "string", "description": "Layer name"})
}
fn above() -> Value {
    json!({"type": "string", "description": "Put it above this layer id (default: above the active layer)"})
}

const FILTERS: &str = "A filter as Xuan's .xuan files store it: {\"GaussianBlur\": {\"radius\": 0–100}}, {\"MotionBlur\": {\"distance\": 0–200, \"angle\": -180–180}}, {\"Noise\": {\"amount\": 0–100, \"monochrome\": bool}} or {\"LensCorrection\": {\"distortion\": -50–50, \"vignette\": -100–100}}.";
const ADJUSTMENTS: &str = "An adjustment as Xuan's .xuan files store it, every field given: \"Invert\", {\"HueSaturation\": {\"hue\": -180–180, \"saturation\": -100–100, \"lightness\": -100–100, \"colorize\": bool}}, {\"Levels\": {\"black\": 0–255, \"gamma\": 0.1–10, \"white\": 0–255, \"output_black\": 0–255, \"output_white\": 0–255}}, {\"Curves\": {\"points\": [{\"x\": 0–1, \"y\": 0–1}, …]}}, {\"Exposure\": {\"exposure\": -20–20, \"offset\": -0.5–0.5, \"gamma\": 0.01–9.99}}, {\"GradientMap\": {\"shadows\": [r,g,b,a], \"highlights\": [r,g,b,a]}}, {\"Grain\": {\"amount\", \"monochrome\", \"seed\"}}, {\"BlackWhite\": {\"weights\": [6 numbers, -200–300], \"tint\": bool, \"tint_hue\": 0–360, \"tint_saturation\": 0–100}} or {\"ColorBalance\": {\"shadows\": [3 numbers, -100–100], \"midtones\": […], \"highlights\": […], \"preserve_luminosity\": bool}}.";

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
            run: |cx, _| text(cx.call("document/list", json!({}))?),
        },
        Spec {
            name: "get_document",
            title: "Describe the document",
            description: "The current document: size, resolution, the selection's bounds and every layer (id, name, kind, visibility, lock, opacity, blend mode, parent group, position and size, rotation, mask, and for generated layers their provenance). Layers are listed bottom to top. With `document`, that document is made current first.",
            properties: json!({"document": {"type": "string", "description": "A document id from list_documents"}}),
            required: &[],
            kind: Kind::Read,
            run: |cx, args| {
                let args = pick(args, &["document"])?;
                if let Some(document) = args.get("document") {
                    cx.call("document/activate", json!({"document": document}))?;
                }
                text(document(cx)?)
            },
        },
        Spec {
            name: "get_layer",
            title: "Describe a layer",
            description: "One layer of the current document, as get_document lists it.",
            properties: json!({"layer": layer()}),
            required: &["layer"],
            kind: Kind::Read,
            run: |cx, args| {
                let args = pick(args, &["layer"])?;
                let id = string(&args, "layer")?;
                let document = document(cx)?;
                let found = (document["layers"].as_array().into_iter().flatten())
                    .find(|layer| layer["id"] == id)
                    .cloned()
                    .ok_or_else(|| format!("No layer {id}"))?;
                text(found)
            },
        },
        Spec {
            name: "get_preview",
            title: "See the image",
            description: "The current document flattened, as a PNG image, so you can see the result. `max_side` is the longest side in pixels (default 1024, 64–4096). The first preview may wait while Xuan asks the user to allow sending the image.",
            properties: json!({"max_side": integer("Longest side in pixels")}),
            required: &[],
            kind: Kind::Read,
            run: |cx, args| {
                let args = pick(args, &["max_side"])?;
                let side = max_side(&args, DEFAULT_PREVIEW)?;
                let export = cx.call("document/export", json!({"max_side": side}))?;
                image_result(&export, "Flattened preview")
            },
        },
        Spec {
            name: "get_layer_image",
            title: "See a layer",
            description: "A layer's own pixels (`what` = \"pixels\", the default) or its mask (\"mask\") as a PNG, with where it sits in the document. `max_side` is the longest side (default 512).",
            properties: json!({"layer": layer(), "what": {"type": "string", "enum": ["pixels", "mask"]}, "max_side": integer("Longest side in pixels")}),
            required: &["layer"],
            kind: Kind::Read,
            run: |cx, args| {
                let args = pick(args, &["layer", "what", "max_side"])?;
                let side = max_side(&args, 512)?;
                let what = args.get("what").cloned().unwrap_or(json!("pixels"));
                let export = cx.call(
                    "layer/export",
                    json!({"layer": string(&args, "layer")?, "what": what, "max_side": side}),
                )?;
                image_result(&export, "Layer")
            },
        },
        Spec {
            name: "get_selection",
            title: "See the selection",
            description: "The selection as a grey PNG cropped to its bounds (white is selected), with its position, or a note that nothing is selected.",
            properties: json!({}),
            required: &[],
            kind: Kind::Read,
            run: |cx, args| {
                pick(args, &[])?;
                let export = cx.call("selection/export", json!({}))?;
                if export.is_null() {
                    return Ok(vec![ContentBlock::text("Nothing is selected.")]);
                }
                image_result(&export, "Selection mask")
            },
        },
        Spec {
            name: "get_edit_permission",
            title: "Check edit permission",
            description: "Whether this session may edit: \"allowed\", \"denied\" or \"ask\" (the next edit asks the user in Xuan), and whether the user turned on auto mode.",
            properties: json!({}),
            required: &[],
            kind: Kind::Read,
            run: |cx, args| {
                pick(args, &[])?;
                text(cx.call("session/status", json!({}))?)
            },
        },
        // Layers.
        Spec {
            name: "set_layer",
            title: "Change a layer",
            description: "Set a layer's name, visibility, lock (it can lock a layer; only the user can unlock one), opacity (0–1), blend mode (Normal, Multiply, Screen, Overlay, …) and placement (x, y, width, height in document pixels, rotation in degrees). Leave out what should not change.",
            properties: json!({
                "layer": layer(), "name": name(), "visible": {"type": "boolean"}, "locked": {"type": "boolean"},
                "opacity": number("0–1"), "blend": {"type": "string", "description": "Blend mode, e.g. Normal, Multiply, Screen, Overlay, SoftLight"},
                "x": number("Left edge"), "y": number("Top edge"), "width": number("Width"), "height": number("Height"), "rotation": number("Degrees"),
            }),
            required: &["layer"],
            kind: Kind::Edit,
            run: |cx, args| {
                let args = pick(
                    args,
                    &[
                        "layer", "name", "visible", "locked", "opacity", "blend", "x", "y",
                        "width", "height", "rotation",
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
                let set = op(
                    "set",
                    &args,
                    &["layer", "name", "visible", "locked", "opacity", "blend"],
                );
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
                cx.edit("Set Layer", edits)?;
                text(json!({"ok": true, "layer": layer}))
            },
        },
        Spec {
            name: "create_layer",
            title: "Create an empty or mask layer",
            description: "A new empty pixel layer (`kind` = \"empty\", the default) or a mask layer made from the selection (\"mask\"), the size of the canvas.",
            properties: json!({"kind": {"type": "string", "enum": ["empty", "mask"]}, "name": name(), "above": above()}),
            required: &[],
            kind: Kind::Edit,
            run: |cx, args| {
                let args = pick(args, &["kind", "name", "above"])?;
                let kind = match args.get("kind").and_then(Value::as_str) {
                    None | Some("empty") => "add_empty_layer",
                    Some("mask") => "add_mask_layer",
                    Some(other) => return Err(format!("Unknown kind {other}")),
                };
                added(cx.edit(
                    "Create Layer",
                    vec![Value::Object(op(kind, &args, &["name", "above"]))],
                )?)
            },
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
            run: |cx, args| {
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
                added(cx.edit(
                    "Create Text Layer",
                    vec![Value::Object(op("add_text_layer", &args, &keys))],
                )?)
            },
        },
        Spec {
            name: "create_shape_layer",
            title: "Create a shape layer",
            description: "An editable rectangle, ellipse or rounded rectangle filling the box x, y, width, height.",
            properties: json!({
                "shape": {"type": "string", "enum": ["rectangle", "ellipse", "rounded_rectangle"]},
                "x": number("Left"), "y": number("Top"), "width": number("Width"), "height": number("Height"),
                "color": color("Fill colour"), "corner_radius": number("For rounded rectangles"), "name": name(), "above": above(),
            }),
            required: &["shape", "x", "y", "width", "height"],
            kind: Kind::Edit,
            run: |cx, args| {
                let keys = [
                    "shape",
                    "x",
                    "y",
                    "width",
                    "height",
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
                    _ => {
                        return Err(
                            "`shape` must be rectangle, ellipse or rounded_rectangle".into()
                        );
                    }
                };
                edit.insert("shape".into(), json!(shape));
                added(cx.edit("Create Shape Layer", vec![Value::Object(edit)])?)
            },
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
            run: |cx, args| {
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
                std::fs::write(&path, bytes).map_err(|e| format!("Cannot write the image: {e}"))?;
                let mut edit = op(
                    "add_layer",
                    &args,
                    &[
                        "x", "y", "width", "height", "name", "above", "opacity", "blend",
                    ],
                );
                edit.insert("image".into(), json!(path));
                let result = cx.edit("Add Image Layer", vec![Value::Object(edit)]);
                let _ = std::fs::remove_file(&path);
                added(result?)
            },
        },
        Spec {
            name: "delete_layer",
            title: "Delete a layer",
            description: "Delete a layer (a group with its layers).",
            properties: json!({"layer": layer()}),
            required: &["layer"],
            kind: Kind::Edit,
            run: |cx, args| {
                let args = pick(args, &["layer"])?;
                cx.edit(
                    "Delete Layer",
                    vec![Value::Object(op("remove_layer", &args, &["layer"]))],
                )?;
                text(json!({"ok": true}))
            },
        },
        Spec {
            name: "merge_layers",
            title: "Merge layers",
            description: "Merge the layers into one; a single layer merges into the one below it. Returns the merged layer's id.",
            properties: json!({"layers": layers()}),
            required: &["layers"],
            kind: Kind::Edit,
            run: |cx, args| {
                let args = pick(args, &["layers"])?;
                added(cx.edit(
                    "Merge Layers",
                    vec![Value::Object(op("merge_layers", &args, &["layers"]))],
                )?)
            },
        },
        Spec {
            name: "group_layers",
            title: "Group layers",
            description: "Put the layers in a new group. Returns the group's id.",
            properties: json!({"layers": layers()}),
            required: &["layers"],
            kind: Kind::Edit,
            run: |cx, args| {
                let args = pick(args, &["layers"])?;
                added(cx.edit(
                    "Group Layers",
                    vec![Value::Object(op("group_layers", &args, &["layers"]))],
                )?)
            },
        },
        Spec {
            name: "ungroup_layer",
            title: "Ungroup",
            description: "Remove a group, keeping its layers.",
            properties: json!({"layer": layer()}),
            required: &["layer"],
            kind: Kind::Edit,
            run: |cx, args| {
                let args = pick(args, &["layer"])?;
                cx.edit(
                    "Ungroup Layers",
                    vec![Value::Object(op("ungroup_layers", &args, &["layer"]))],
                )?;
                text(json!({"ok": true}))
            },
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
            run: |cx, args| {
                let args = pick(args, &["layer", "above", "below", "parent"])?;
                cx.edit(
                    "Move Layer",
                    vec![Value::Object(op(
                        "move_layer",
                        &args,
                        &["layer", "above", "below", "parent"],
                    ))],
                )?;
                text(json!({"ok": true}))
            },
        },
        // Selections.
        Spec {
            name: "select_shape",
            title: "Select a shape",
            description: "Select a rectangle or ellipse (x, y, width, height) or a polygon (points), combined with the current selection by `mode` (default replace).",
            properties: json!({
                "shape": {"type": "string", "enum": ["rectangle", "ellipse", "polygon"]},
                "x": number("Left"), "y": number("Top"), "width": number("Width"), "height": number("Height"),
                "points": points(), "mode": mode(),
            }),
            required: &["shape"],
            kind: Kind::Edit,
            run: |cx, args| {
                let args = pick(
                    args,
                    &["shape", "x", "y", "width", "height", "points", "mode"],
                )?;
                let edit = match args.get("shape").and_then(Value::as_str) {
                    Some("rectangle") => {
                        op("select_rect", &args, &["x", "y", "width", "height", "mode"])
                    }
                    Some("ellipse") => {
                        let mut edit =
                            op("select_rect", &args, &["x", "y", "width", "height", "mode"]);
                        edit.insert("ellipse".into(), json!(true));
                        edit
                    }
                    Some("polygon") => op("select_polygon", &args, &["points", "mode"]),
                    _ => return Err("`shape` must be rectangle, ellipse or polygon".into()),
                };
                cx.edit("Select", vec![Value::Object(edit)])?;
                selection_summary(cx)
            },
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
            run: |cx, args| {
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
                cx.edit("Select Color", vec![Value::Object(edit)])?;
                selection_summary(cx)
            },
        },
        Spec {
            name: "modify_selection",
            title: "Change the selection",
            description: "`all`, `none` or `invert` the selection; `grow` or `shrink` it by `amount` pixels; `feather` its edge by `amount`; select the `subject` (runs in the background); or select the active layer's opaque pixels (`layer_pixels`).",
            properties: json!({
                "action": {"type": "string", "enum": ["all", "none", "invert", "grow", "shrink", "feather", "subject", "layer_pixels"]},
                "amount": number("Pixels, for grow, shrink and feather (at most 256)"),
            }),
            required: &["action"],
            kind: Kind::Edit,
            run: |cx, args| {
                let args = pick(args, &["action", "amount"])?;
                let amount = args.get("amount").and_then(Value::as_f64).unwrap_or(0.0);
                let command = match args.get("action").and_then(Value::as_str) {
                    Some("all") => "select_all",
                    Some("none") => "deselect",
                    Some("invert") => "invert_selection",
                    Some("subject") => "select_subject",
                    Some("layer_pixels") => "select_layer_pixels",
                    Some("grow") | Some("shrink") => {
                        let by = if args["action"] == "grow" {
                            amount
                        } else {
                            -amount
                        };
                        cx.edit(
                            "Modify Selection",
                            vec![json!({"op": "grow_selection", "by": by.round() as i64})],
                        )?;
                        return selection_summary(cx);
                    }
                    Some("feather") => {
                        cx.edit(
                            "Feather Selection",
                            vec![json!({"op": "feather_selection", "radius": amount})],
                        )?;
                        return selection_summary(cx);
                    }
                    _ => return Err("Unknown `action`".into()),
                };
                cx.run(command)?;
                if command == "select_subject" {
                    return Ok(vec![ContentBlock::text(
                        "Select Subject is running in Xuan; the selection changes when it finishes. Check with get_document.",
                    )]);
                }
                selection_summary(cx)
            },
        },
        // Pixels.
        Spec {
            name: "paint_stroke",
            title: "Paint a stroke",
            description: "Paint (or with `erase`, erase) one brush stroke through the points on a pixel layer, inside the selection if there is one. `size` is the brush diameter (1–2000, default 20), `hardness` and `opacity` 0–1.",
            properties: json!({
                "layer": optional_layer(), "points": points(), "color": color("Paint colour"), "size": number("Brush diameter"),
                "hardness": number("0–1"), "opacity": number("0–1"), "erase": {"type": "boolean"},
            }),
            required: &["points"],
            kind: Kind::Edit,
            run: |cx, args| {
                let keys = [
                    "layer", "points", "color", "size", "hardness", "opacity", "erase",
                ];
                let args = pick(args, &keys)?;
                cx.edit(
                    "Paint Stroke",
                    vec![Value::Object(op("stroke", &args, &keys))],
                )?;
                text(json!({"ok": true}))
            },
        },
        Spec {
            name: "fill",
            title: "Fill",
            description: "Fill the selection (or the whole layer without one) of a pixel layer with a colour.",
            properties: json!({"layer": optional_layer(), "color": color("Fill colour")}),
            required: &["color"],
            kind: Kind::Edit,
            run: |cx, args| {
                let args = pick(args, &["layer", "color"])?;
                cx.edit(
                    "Fill",
                    vec![Value::Object(op("fill", &args, &["layer", "color"]))],
                )?;
                text(json!({"ok": true}))
            },
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
            run: |cx, args| {
                let keys = [
                    "layer", "start", "end", "stops", "radial", "opacity", "mask",
                ];
                let args = pick(args, &keys)?;
                cx.edit(
                    "Gradient",
                    vec![Value::Object(op("gradient", &args, &keys))],
                )?;
                text(json!({"ok": true}))
            },
        },
        Spec {
            name: "apply_filter",
            title: "Apply a filter",
            description: "Apply a filter to a pixel layer inside the selection, or with `as_layer` add it as a non-destructive filter layer (masked by the selection).",
            properties: json!({"filter": {"description": FILTERS}, "layer": optional_layer(), "as_layer": {"type": "boolean", "description": "Add a non-destructive filter layer instead"}}),
            required: &["filter"],
            kind: Kind::Edit,
            run: |cx, args| {
                let args = pick(args, &["filter", "layer", "as_layer"])?;
                let edit = if args.get("as_layer") == Some(&json!(true)) {
                    op("add_adjustment_layer", &args, &["filter"])
                } else {
                    op("apply_filter", &args, &["filter", "layer"])
                };
                added(cx.edit("Apply Filter", vec![Value::Object(edit)])?)
            },
        },
        Spec {
            name: "apply_adjustment",
            title: "Apply an adjustment",
            description: "Apply an adjustment to a pixel layer inside the selection, or with `as_layer` add it as a non-destructive adjustment layer (masked by the selection).",
            properties: json!({"adjustment": {"description": ADJUSTMENTS}, "layer": optional_layer(), "as_layer": {"type": "boolean"}}),
            required: &["adjustment"],
            kind: Kind::Edit,
            run: |cx, args| {
                let args = pick(args, &["adjustment", "layer", "as_layer"])?;
                let edit = if args.get("as_layer") == Some(&json!(true)) {
                    op("add_adjustment_layer", &args, &["adjustment"])
                } else {
                    op("apply_adjustment", &args, &["adjustment", "layer"])
                };
                added(cx.edit("Apply Adjustment", vec![Value::Object(edit)])?)
            },
        },
        // The canvas.
        Spec {
            name: "crop_canvas",
            title: "Crop",
            description: "Crop the canvas to the rectangle x, y, width, height in document pixels.",
            properties: json!({"x": number("Left"), "y": number("Top"), "width": integer("Width"), "height": integer("Height")}),
            required: &["x", "y", "width", "height"],
            kind: Kind::Edit,
            run: |cx, args| {
                let keys = ["x", "y", "width", "height"];
                let args = pick(args, &keys)?;
                cx.edit("Crop", vec![Value::Object(op("crop", &args, &keys))])?;
                size_summary(cx)
            },
        },
        Spec {
            name: "resize_canvas",
            title: "Canvas size",
            description: "Change the canvas size without scaling the content; `anchor` [ax, ay] (0–1) says where the content stays: [0, 0] top-left, [0.5, 0.5] centre (default).",
            properties: json!({"width": integer("Width"), "height": integer("Height"), "anchor": {"type": "array", "items": {"type": "number"}, "minItems": 2, "maxItems": 2}}),
            required: &["width", "height"],
            kind: Kind::Edit,
            run: |cx, args| {
                let keys = ["width", "height", "anchor"];
                let args = pick(args, &keys)?;
                cx.edit(
                    "Canvas Size",
                    vec![Value::Object(op("resize_canvas", &args, &keys))],
                )?;
                size_summary(cx)
            },
        },
        Spec {
            name: "resize_image",
            title: "Image size",
            description: "Scale the whole document to width × height pixels.",
            properties: json!({"width": integer("Width"), "height": integer("Height")}),
            required: &["width", "height"],
            kind: Kind::Edit,
            run: |cx, args| {
                let keys = ["width", "height"];
                let args = pick(args, &keys)?;
                cx.edit(
                    "Image Size",
                    vec![Value::Object(op("resize_image", &args, &keys))],
                )?;
                size_summary(cx)
            },
        },
        // History and commands.
        Spec {
            name: "undo",
            title: "Undo",
            description: "Undo the last `steps` changes (default 1, at most 20), as Edit → Undo does.",
            properties: json!({"steps": integer("1–20")}),
            required: &[],
            kind: Kind::Edit,
            run: |cx, args| history(cx, args, "undo"),
        },
        Spec {
            name: "redo",
            title: "Redo",
            description: "Redo the last `steps` undone changes (default 1, at most 20).",
            properties: json!({"steps": integer("1–20")}),
            required: &[],
            kind: Kind::Edit,
            run: |cx, args| history(cx, args, "redo"),
        },
        Spec {
            name: "run_command",
            title: "Run an editor command",
            description: "Run one of Xuan's menu commands on the current document, as its menu item does: flatten, duplicate (the selected layers), new_layer, delete_layer, move_out (of its group), mask (add a mask from the selection), new_mask_layer, delete_mask, disable_mask, link_mask, clip (clipping mask), flip_h, flip_v (the layer), flip_canvas_h, flip_canvas_v, invert (the layer's pixels), clear (the selected pixels), fill_fg, fill_bg, content_fill (fill the selection from its surroundings), remove_background, remove_flat_background, fit, actual, zoom_in, zoom_out.",
            properties: json!({"command": {"type": "string", "enum": COMMANDS}}),
            required: &["command"],
            kind: Kind::Edit,
            run: |cx, args| {
                let args = pick(args, &["command"])?;
                let command = string(&args, "command")?;
                if !COMMANDS.contains(&command) {
                    return Err(format!(
                        "`{command}` is not one of the commands this tool runs"
                    ));
                }
                cx.run(command)?;
                text(json!({"ok": true}))
            },
        },
        // Documents and files.
        Spec {
            name: "switch_document",
            title: "Switch document",
            description: "Make an open document the current one, as clicking its tab does.",
            properties: json!({"document": {"type": "string"}}),
            required: &["document"],
            kind: Kind::Read,
            run: |cx, args| {
                let args = pick(args, &["document"])?;
                cx.call("document/activate", Value::Object(args))?;
                text(json!({"ok": true}))
            },
        },
        Spec {
            name: "save_document",
            title: "Save the project",
            description: "Ask the user to save a document (the current one by default) as a .xuan project: Xuan shows its save dialog with `suggested_name`, and the user chooses where. Returns the file name, or an error if the user cancelled.",
            properties: json!({"document": {"type": "string"}, "suggested_name": {"type": "string"}}),
            required: &[],
            kind: Kind::File,
            run: |cx, args| {
                let args = pick(args, &["document", "suggested_name"])?;
                text(cx.call("file/save_as", Value::Object(args))?)
            },
        },
        Spec {
            name: "export_document",
            title: "Export an image",
            description: "Ask the user to export a document as png (default), jpg, tiff or webp: Xuan shows its save dialog and the user chooses where. Returns the file name.",
            properties: json!({"document": {"type": "string"}, "format": {"type": "string", "enum": ["png", "jpg", "tiff", "webp"]}, "suggested_name": {"type": "string"}}),
            required: &[],
            kind: Kind::File,
            run: |cx, args| {
                let args = pick(args, &["document", "format", "suggested_name"])?;
                text(cx.call("file/export", Value::Object(args))?)
            },
        },
        Spec {
            name: "open_document",
            title: "Open a file",
            description: "Ask the user to open the image or .xuan project at the absolute `path` as a new document; Xuan shows the path and opens it only if the user agrees.",
            properties: json!({"path": {"type": "string"}}),
            required: &["path"],
            kind: Kind::File,
            run: |cx, args| {
                let args = pick(args, &["path"])?;
                text(cx.call("file/open", Value::Object(args))?)
            },
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
                ))
                .open_world(false);
            Tool::new(spec.name, spec.description, Arc::new(schema))
                .with_title(spec.title)
                .with_annotations(annotations)
        })
        .collect()
}

/// Run a tool. Errors from Xuan or from the arguments become a tool error
/// the model can read, not a protocol error.
pub fn call(cx: &Context, name: &str, args: Map<String, Value>) -> CallToolResult {
    let Some(spec) = specs().into_iter().find(|spec| spec.name == name) else {
        return CallToolResult::error(vec![ContentBlock::text(format!("Unknown tool {name}"))]);
    };
    match (spec.run)(cx, args) {
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
            .request(self.session, method, params)
            .map_err(|error| explain(method, &error))
    }

    fn edit(&self, name: &str, edits: Vec<Value>) -> Result<Value, String> {
        self.call("document/edit", json!({"name": name, "edits": edits}))
    }

    fn run(&self, action: &str) -> Result<Value, String> {
        self.call("host/run", json!({"action": action}))
    }
}

/// What the model is told when Xuan refuses.
fn explain(method: &str, error: &EditorError) -> String {
    if error.code != CANCELLED {
        return format!("Xuan: {}", error.message);
    }
    match method {
        "document/edit" | "host/run" => "The user did not allow edits from this session in Xuan. Ask the user before trying again.".into(),
        "file/save_as" | "file/export" | "file/open" => "The user cancelled.".into(),
        _ if method.ends_with("/export") => "The user did not allow sending the image to this MCP server.".into(),
        _ => format!("Xuan: {}", error.message),
    }
}

/// The arguments, refusing names the tool does not take.
fn pick(args: Map<String, Value>, allowed: &[&str]) -> Result<Map<String, Value>, String> {
    if let Some(unknown) = args.keys().find(|key| !allowed.contains(&key.as_str())) {
        return Err(format!("Unknown argument `{unknown}`"));
    }
    Ok(args)
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
