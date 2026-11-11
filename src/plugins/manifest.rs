//! `plugin.toml`: what a plugin offers and needs. See `docs/PLUGINS.md`.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol version this host speaks.
pub const PROTOCOL: u32 = 1;
pub const MANIFEST_FILE: &str = "plugin.toml";
const MAX_ID: usize = 64;
/// Longest text input value, in bytes, taken from stored or plugin inputs.
pub const MAX_INPUT_TEXT: usize = 64 * 1024;
/// Most regions one action takes, whatever its `max` says.
pub const MAX_REGIONS: usize = 256;
/// Largest region coordinate or size, in document pixels.
const MAX_REGION_COORDINATE: f64 = 1_000_000.0;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub plugin: Info,
    #[serde(default)]
    pub permissions: Permissions,
    #[serde(default)]
    pub settings: Vec<Input>,
    #[serde(default)]
    pub actions: Vec<Action>,
    #[serde(default)]
    pub panes: Vec<Pane>,
    #[serde(default)]
    pub formats: Vec<Format>,
    /// Model files the host downloads and verifies for the plugin.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<Model>,
    /// Built-in algorithms one of the plugin's actions can replace, when the user
    /// picks the plugin in Settings → Selection.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub provides: Vec<Provide>,
    /// Folder the manifest was read from; the plugin runs there.
    #[serde(skip)]
    pub dir: PathBuf,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Info {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default)]
    pub description: String,
    pub command: Vec<String>,
    #[serde(default = "default_protocol")]
    pub protocol: u32,
    /// The Xuan versions the plugin works with, as a semver range such as
    /// `">=0.3, <0.5"`. Plugins whose range excludes this Xuan are refused.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requires_xuan: Option<String>,
}

fn default_protocol() -> u32 {
    PROTOCOL
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Permissions {
    pub network: Vec<String>,
    pub secrets: Vec<String>,
    pub document: DocumentAccess,
    pub filesystem: FilesystemAccess,
    /// `"session"`: the plugin's direct edits (`document/edit` and editing
    /// `host/run` commands) wait until the user allows them for the session.
    /// Left out of stored grants while it has its default, so grants made
    /// before it existed still cover their plugins.
    #[serde(skip_serializing_if = "EditPrompt::is_none")]
    pub edit_prompt: EditPrompt,
}

impl Permissions {
    pub fn is_empty(&self) -> bool {
        self.network.is_empty()
            && self.secrets.is_empty()
            && self.document == DocumentAccess::Read
            && self.filesystem == FilesystemAccess::None
    }
}

/// When the user is asked before a plugin edits documents directly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EditPrompt {
    /// Never: the grant's `document = "edit"` is enough.
    #[default]
    None,
    /// Once per session of the plugin (its process, or a session id it
    /// names), unless the user chose auto mode for it.
    Session,
}

impl EditPrompt {
    pub fn is_none(&self) -> bool {
        *self == Self::None
    }
}

/// A built-in algorithm a plugin action can stand in for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    /// Select → Subject: a mask that becomes the selection.
    SelectSubject,
    /// Filter → Remove Background: a mask the host lays on the source layer as its
    /// layer mask. It changes the document, so it needs `document = "edit"`.
    RemoveBackground,
    /// The Magic tool's Object mode: a mask of the object at a clicked point or inside
    /// a dragged rectangle, which becomes the selection.
    ObjectSelect,
}

impl Capability {
    pub const ALL: [Self; 3] = [
        Self::SelectSubject,
        Self::RemoveBackground,
        Self::ObjectSelect,
    ];

    /// The manifest's name for it.
    pub fn id(self) -> &'static str {
        match self {
            Self::SelectSubject => "select_subject",
            Self::RemoveBackground => "remove_background",
            Self::ObjectSelect => "object_select",
        }
    }

    /// The command it replaces, untranslated.
    pub fn label(self) -> &'static str {
        match self {
            Self::SelectSubject => "Select Subject",
            Self::RemoveBackground => "Remove Background",
            Self::ObjectSelect => "Object Selection",
        }
    }
}

/// `[[provides]]`: the action that provides a capability.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provide {
    pub capability: Capability,
    pub action: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentAccess {
    #[default]
    Read,
    Edit,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FilesystemAccess {
    #[default]
    None,
    Read,
    Write,
}

/// A setting, an action input or a region field. Settings and inputs share
/// the same types, so a plugin author learns one vocabulary.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Input {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: InputKind,
    #[serde(default)]
    pub label: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub help: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub values: Vec<Choice>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub placeholder: String,
    /// Fields of each region, for `regions` inputs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub fields: Vec<Input>,
    /// Behind "Advanced" in popovers and the New Image tab.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub advanced: bool,
    /// Where the input is shown; empty means everywhere.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surfaces: Vec<Surface>,
}

impl Input {
    pub fn shown_on(&self, surface: Surface) -> bool {
        self.surfaces.is_empty() || self.surfaces.contains(&surface)
    }

    pub fn label(&self) -> &str {
        if self.label.is_empty() {
            &self.id
        } else {
            &self.label
        }
    }

    /// The value a fresh dialog starts with.
    pub fn initial(&self) -> Value {
        if let Some(default) = &self.default {
            return default.clone();
        }
        match self.kind {
            InputKind::Text | InputKind::Multiline | InputKind::Path | InputKind::Secret => {
                Value::String(String::new())
            }
            InputKind::Integer | InputKind::Seed => Value::from(self.min.unwrap_or(0.0) as i64),
            InputKind::Number => Value::from(self.min.unwrap_or(0.0)),
            InputKind::Bool => Value::Bool(false),
            InputKind::Enum => self
                .values
                .first()
                .map_or(Value::Null, |choice| Value::String(choice.id.clone())),
            InputKind::Color => Value::String("#ffffff".into()),
            InputKind::Regions => Value::Array(Vec::new()),
        }
    }

    /// `value` made valid for this input: the right type, within `min` and
    /// `max`, one of the `values`, cut to a sane length, or else the initial
    /// value. Inputs stored in a project or sent by a plugin pass through
    /// this before they reach the dialog.
    pub fn coerce(&self, value: &Value) -> Value {
        let number = value.as_f64().filter(|v| v.is_finite());
        let coerced = match self.kind {
            InputKind::Text | InputKind::Multiline | InputKind::Path => value
                .as_str()
                .map(|text| Value::String(clip(text, MAX_INPUT_TEXT).to_owned())),
            // Secrets are never action inputs.
            InputKind::Secret => None,
            InputKind::Integer | InputKind::Seed => number.map(|v| {
                let v = self.clamp(v.round()).clamp(-9.0e15, 9.0e15);
                Value::from(v as i64)
            }),
            InputKind::Number => number.map(|v| Value::from(self.clamp(v))),
            InputKind::Bool => value.as_bool().map(Value::Bool),
            InputKind::Enum => value
                .as_str()
                .filter(|id| self.values.iter().any(|choice| choice.id == *id))
                .map(|id| Value::String(id.to_owned())),
            InputKind::Color => value
                .as_str()
                .filter(|text| super::ui::Node::color(text).is_some())
                .map(|text| Value::String(text.to_owned())),
            InputKind::Regions => value.as_array().map(|items| {
                let limit = self
                    .max
                    .map_or(MAX_REGIONS, |max| (max.max(0.0) as usize).min(MAX_REGIONS));
                Value::Array(
                    items
                        .iter()
                        .filter_map(|item| self.coerce_region(item))
                        .take(limit)
                        .collect(),
                )
            }),
        };
        coerced.unwrap_or_else(|| self.initial())
    }

    fn clamp(&self, value: f64) -> f64 {
        let value = self.min.map_or(value, |min| value.max(min));
        self.max.map_or(value, |max| value.min(max))
    }

    /// A region with finite, bounded coordinates and only declared fields.
    fn coerce_region(&self, item: &Value) -> Option<Value> {
        let number = |key: &str| {
            item.get(key)?
                .as_f64()
                .filter(|v| v.is_finite() && v.abs() <= MAX_REGION_COORDINATE)
        };
        let (x, y) = (number("x")?, number("y")?);
        let (width, height) = (number("width")?, number("height")?);
        if width <= 0.0 || height <= 0.0 {
            return None;
        }
        let stored = item.get("fields");
        let fields: serde_json::Map<String, Value> = (self.fields.iter())
            .map(|field| {
                let value = stored
                    .and_then(|fields| fields.get(&field.id))
                    .map_or_else(|| field.initial(), |value| field.coerce(value));
                (field.id.clone(), value)
            })
            .collect();
        Some(serde_json::json!({
            "x": x, "y": y, "width": width, "height": height, "fields": fields,
        }))
    }

    /// `value` if it is valid for this input as given, else why not: unlike
    /// [`Input::coerce`], nothing is clamped, cut or replaced by a default,
    /// so a value another plugin sends fails at once rather than running
    /// with something else.
    pub fn check(&self, value: &Value) -> std::result::Result<Value, String> {
        let id = &self.id;
        let number = || {
            value
                .as_f64()
                .filter(|v| v.is_finite())
                .ok_or_else(|| format!("`{id}` must be a number"))
        };
        let in_range = |v: f64| {
            let low = self.min.is_some_and(|min| v < min);
            let high = self.max.is_some_and(|max| v > max);
            match (self.min, self.max) {
                (Some(min), Some(max)) if low || high => {
                    Err(format!("`{id}` must be between {min} and {max}"))
                }
                (Some(min), _) if low => Err(format!("`{id}` must be at least {min}")),
                (_, Some(max)) if high => Err(format!("`{id}` must be at most {max}")),
                _ => Ok(v),
            }
        };
        match self.kind {
            InputKind::Text | InputKind::Multiline | InputKind::Path => {
                let text = value
                    .as_str()
                    .ok_or_else(|| format!("`{id}` must be a string"))?;
                if text.len() > MAX_INPUT_TEXT {
                    return Err(format!("`{id}` is longer than 64 KiB"));
                }
                Ok(value.clone())
            }
            InputKind::Secret => Err(format!("`{id}` is a secret, never an action input")),
            InputKind::Integer | InputKind::Seed => {
                let v = number()?;
                if v.fract() != 0.0 || v.abs() > 9.0e15 {
                    return Err(format!("`{id}` must be a whole number"));
                }
                Ok(Value::from(in_range(v)? as i64))
            }
            InputKind::Number => in_range(number()?).map(Value::from),
            InputKind::Bool => value
                .as_bool()
                .map(Value::Bool)
                .ok_or_else(|| format!("`{id}` must be true or false")),
            InputKind::Enum => value
                .as_str()
                .filter(|choice| self.values.iter().any(|c| c.id == *choice))
                .map(|choice| Value::String(choice.to_owned()))
                .ok_or_else(|| {
                    let ids: Vec<&str> = self.values.iter().map(|c| c.id.as_str()).collect();
                    format!("`{id}` must be one of {}", ids.join(", "))
                }),
            InputKind::Color => value
                .as_str()
                .filter(|text| super::ui::Node::color(text).is_some())
                .map(|text| Value::String(text.to_owned()))
                .ok_or_else(|| format!("`{id}` must be a colour as #rrggbb or #rrggbbaa")),
            InputKind::Regions => {
                let items = value
                    .as_array()
                    .ok_or_else(|| format!("`{id}` must be a list of regions"))?;
                let limit = self
                    .max
                    .map_or(MAX_REGIONS, |max| (max.max(0.0) as usize).min(MAX_REGIONS));
                let regions = |count: usize| match count {
                    1 => "1 region".to_owned(),
                    count => format!("{count} regions"),
                };
                if items.len() > limit {
                    return Err(format!("`{id}` takes at most {}", regions(limit)));
                }
                if let Some(min) = self.min
                    && (items.len() as f64) < min
                {
                    return Err(format!(
                        "`{id}` needs at least {}",
                        regions(min.ceil() as usize)
                    ));
                }
                (items.iter().enumerate())
                    .map(|(index, item)| self.check_region(index + 1, item))
                    .collect::<std::result::Result<Vec<_>, _>>()
                    .map(Value::Array)
            }
        }
    }

    /// One region of a `regions` input, checked like [`Input::check`]: the
    /// box in document pixels and only the declared fields, which take their
    /// defaults when left out.
    fn check_region(&self, number: usize, item: &Value) -> std::result::Result<Value, String> {
        let id = &self.id;
        let region = item
            .as_object()
            .ok_or_else(|| format!("`{id}` region {number} must be an object"))?;
        if let Some(key) = (region.keys())
            .find(|key| !matches!(key.as_str(), "x" | "y" | "width" | "height" | "fields"))
        {
            return Err(format!(
                "`{id}` region {number} has an unknown key `{key}`; a region is {{x, y, width, height, fields}}"
            ));
        }
        let coordinate = |key: &str| {
            (region.get(key).and_then(Value::as_f64))
                .filter(|v| v.is_finite() && v.abs() <= MAX_REGION_COORDINATE)
                .ok_or_else(|| {
                    format!(
                        "`{id}` region {number} needs `{key}`, a number of document pixels within ±{MAX_REGION_COORDINATE}"
                    )
                })
        };
        let (x, y) = (coordinate("x")?, coordinate("y")?);
        let (width, height) = (coordinate("width")?, coordinate("height")?);
        if width <= 0.0 || height <= 0.0 {
            return Err(format!(
                "`{id}` region {number} must have a width and height above 0"
            ));
        }
        let given = match region.get("fields") {
            None | Some(Value::Null) => serde_json::Map::new(),
            Some(Value::Object(fields)) => fields.clone(),
            Some(_) => {
                return Err(format!(
                    "`{id}` region {number}: `fields` must be an object"
                ));
            }
        };
        if let Some(key) = given
            .keys()
            .find(|key| !self.fields.iter().any(|field| &field.id == *key))
        {
            return Err(format!(
                "`{id}` region {number} has no field `{key}`; its fields are {}",
                (self.fields.iter().map(|f| f.id.as_str()))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        let mut fields = serde_json::Map::new();
        for field in &self.fields {
            let value = match given.get(&field.id) {
                None | Some(Value::Null) => field.initial(),
                Some(value) => field
                    .check(value)
                    .map_err(|error| format!("`{id}` region {number}: {error}"))?,
            };
            fields.insert(field.id.clone(), value);
        }
        Ok(serde_json::json!({
            "x": x, "y": y, "width": width, "height": height, "fields": fields,
        }))
    }

    fn validate(&self, nested: bool) -> Result<()> {
        validate_id(&self.id).with_context(|| format!("input `{}`", self.id))?;
        let mut listed = std::collections::HashSet::new();
        for surface in &self.surfaces {
            ensure!(
                listed.insert(surface),
                "input `{}` lists a surface twice",
                self.id
            );
        }
        match self.kind {
            InputKind::Enum => {
                ensure!(!self.values.is_empty(), "enum `{}` has no values", self.id);
                let mut seen = std::collections::HashSet::new();
                for choice in &self.values {
                    ensure!(
                        !choice.id.is_empty() && seen.insert(&choice.id),
                        "enum `{}` has a duplicate or empty value",
                        self.id
                    );
                }
                if let Some(Value::String(default)) = &self.default {
                    ensure!(
                        self.values.iter().any(|c| &c.id == default),
                        "enum `{}` default is not one of its values",
                        self.id
                    );
                }
            }
            InputKind::Regions => {
                ensure!(!nested, "region fields cannot contain regions");
                let mut seen = std::collections::HashSet::new();
                for field in &self.fields {
                    field.validate(true)?;
                    ensure!(
                        seen.insert(&field.id),
                        "regions `{}` repeats the field `{}`",
                        self.id,
                        field.id
                    );
                }
            }
            InputKind::Integer | InputKind::Number | InputKind::Seed => {
                if let (Some(min), Some(max)) = (self.min, self.max) {
                    ensure!(min <= max, "input `{}` has min above max", self.id);
                }
            }
            _ => {}
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum InputKind {
    Text,
    Multiline,
    Integer,
    Number,
    Seed,
    Bool,
    Enum,
    Color,
    Path,
    Secret,
    Regions,
}

impl InputKind {
    /// What the user wrote or chose from their files: texts, paths and
    /// secrets. A plugin that needs send consent gets none of it before the
    /// user agreed, not even for an estimate.
    pub fn private(self) -> bool {
        matches!(
            self,
            Self::Text | Self::Multiline | Self::Path | Self::Secret
        )
    }
}

/// An enum value: `"obj"` or `{ id = "obj", label = "Object" }`.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Choice {
    pub id: String,
    pub label: String,
}

impl<'de> Deserialize<'de> for Choice {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Raw {
            Id(String),
            Full {
                id: String,
                #[serde(default)]
                label: String,
            },
        }
        Ok(match Raw::deserialize(deserializer)? {
            Raw::Id(id) => Self {
                label: id.clone(),
                id,
            },
            Raw::Full { id, label } => Self {
                label: if label.is_empty() { id.clone() } else { label },
                id,
            },
        })
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Action {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub menu: Menu,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shortcut: Option<String>,
    #[serde(default)]
    pub kind: ActionKind,
    #[serde(default)]
    pub source: Source,
    #[serde(default)]
    pub result: Placement,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub inputs: Vec<Input>,
    /// Ids of the declared models the action needs; Xuan offers to download
    /// missing ones before it runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models: Vec<String>,
    /// Places besides its menu where the action appears.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub surfaces: Vec<Surface>,
    /// Short label in the AI Region popover (`region` surface).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub verb: String,
}

impl Action {
    pub fn needs_image(&self) -> bool {
        self.kind == ActionKind::Edit && self.source.from != SourceKind::None
    }

    pub fn regions_input(&self) -> Option<&Input> {
        self.inputs
            .iter()
            .find(|input| input.kind == InputKind::Regions)
    }

    pub fn on(&self, surface: Surface) -> bool {
        self.surfaces.contains(&surface)
    }

    /// The values to run the action with when another plugin starts it
    /// (`host/run`): each input `inputs` gives, checked with
    /// [`Input::check`], and the default of each it leaves out or sets to
    /// `null`. Unknown inputs are refused, and so are `path` inputs: the
    /// user chooses their own files, so a caller cannot have a plugin read
    /// (and perhaps upload) a file the user never picked.
    pub fn check_inputs(
        &self,
        inputs: Option<&Value>,
    ) -> std::result::Result<serde_json::Map<String, Value>, String> {
        let given = match inputs {
            None | Some(Value::Null) => serde_json::Map::new(),
            Some(Value::Object(given)) => given.clone(),
            Some(_) => return Err("`inputs` must be an object".into()),
        };
        if let Some(key) = given
            .keys()
            .find(|key| !self.inputs.iter().any(|input| &input.id == *key))
        {
            let ids: Vec<&str> = (self.inputs.iter())
                .filter(|input| !matches!(input.kind, InputKind::Path | InputKind::Secret))
                .map(|input| input.id.as_str())
                .collect();
            return Err(if ids.is_empty() {
                format!("The action takes no inputs, not `{key}`")
            } else {
                format!(
                    "The action has no input `{key}`; its inputs are {}",
                    ids.join(", ")
                )
            });
        }
        let mut values = serde_json::Map::new();
        for input in &self.inputs {
            let value = match given.get(&input.id) {
                // Regions an action needs are never left to their default.
                None | Some(Value::Null) if input.kind == InputKind::Regions => {
                    input.check(&input.initial())?
                }
                None | Some(Value::Null) => input.initial(),
                Some(_) if input.kind == InputKind::Path => {
                    return Err(format!(
                        "`{}` is a file the user chooses; another plugin cannot set it",
                        input.id
                    ));
                }
                Some(value) => input.check(value)?,
            };
            values.insert(input.id.clone(), value);
        }
        Ok(values)
    }
}

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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Menu {
    File,
    Edit,
    Image,
    Layer,
    Select,
    Filter,
    #[default]
    Plugins,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActionKind {
    #[default]
    Edit,
    Generate,
    Command,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Source {
    pub from: SourceKind,
    pub max_side: Option<u32>,
    pub crop_to_regions: bool,
    /// Padding around the regions, as a fraction of the crop's size.
    pub padding: f32,
    /// Also send the selection as a mask in the source's coordinates.
    pub mask: SourceMask,
    /// Blur of the mask's edge, in document units.
    pub mask_feather: f32,
    /// Grow (positive) or shrink (negative) the mask, in document units.
    pub mask_grow: i32,
    /// What the mask is when nothing is selected.
    pub mask_empty: MaskEmpty,
    /// Pad a `composite` source with new, transparent canvas on each side.
    pub extend: Option<Extend>,
}

impl Source {
    /// The source with every `extend` side that names an input replaced by
    /// that input's value in `inputs`.
    pub fn with_inputs(&self, inputs: &serde_json::Map<String, Value>) -> Self {
        let mut source = self.clone();
        if let Some(extend) = &mut source.extend {
            for side in extend.sides_mut() {
                if let Amount::Input(id) = side {
                    *side = Amount::Pixels(
                        inputs
                            .get(id.as_str())
                            .and_then(Value::as_f64)
                            .filter(|v| v.is_finite())
                            .map_or(0, |v| v.round().clamp(0.0, f64::from(MAX_EXTEND)) as u32),
                    );
                }
            }
        }
        source
    }
}

/// Largest `extend` side, in document pixels.
pub const MAX_EXTEND: u32 = crate::document::MAX_SIDE;

/// `source.extend`: how many document pixels of new canvas to add on each
/// side of a `composite` source. Each side is a number or the id of an
/// `integer` or `number` input of the action that holds it.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Extend {
    pub left: Amount,
    pub top: Amount,
    pub right: Amount,
    pub bottom: Amount,
}

impl Extend {
    fn sides(&self) -> [&Amount; 4] {
        [&self.left, &self.top, &self.right, &self.bottom]
    }

    fn sides_mut(&mut self) -> [&mut Amount; 4] {
        [
            &mut self.left,
            &mut self.top,
            &mut self.right,
            &mut self.bottom,
        ]
    }

    /// The sides in pixels, once [`Source::with_inputs`] resolved the
    /// inputs; a side still naming an input counts as 0.
    pub fn margins(&self) -> Margins {
        let [left, top, right, bottom] = self.sides().map(|side| match side {
            Amount::Pixels(pixels) => *pixels,
            Amount::Input(_) => 0,
        });
        Margins {
            left,
            top,
            right,
            bottom,
        }
    }
}

/// One side of `source.extend`: pixels, or the id of the input holding them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Amount {
    Pixels(u32),
    Input(String),
}

impl Default for Amount {
    fn default() -> Self {
        Self::Pixels(0)
    }
}

/// Pixels added on each side of the canvas.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Margins {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

impl Margins {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }
}

impl Default for Source {
    fn default() -> Self {
        Self {
            from: SourceKind::Layer,
            max_side: None,
            crop_to_regions: false,
            padding: 0.25,
            mask: SourceMask::None,
            mask_feather: 0.0,
            mask_grow: 0,
            mask_empty: MaskEmpty::Error,
            extend: None,
        }
    }
}

/// Largest `mask_feather` and `mask_grow`, in document units.
pub const MAX_MASK_RADIUS: u32 = 256;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceMask {
    #[default]
    None,
    Selection,
}

/// The mask an action gets when `source.mask = "selection"` and nothing is
/// selected: refuse to run, or send a mask that is all selected or all not.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MaskEmpty {
    #[default]
    Error,
    White,
    Black,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind {
    #[default]
    Layer,
    Composite,
    Selection,
    None,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Placement {
    pub into: ResultInto,
    pub mask_to_regions: bool,
}

impl Default for Placement {
    fn default() -> Self {
        Self {
            into: ResultInto::Layer,
            mask_to_regions: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResultInto {
    #[default]
    Layer,
    Replace,
    Document,
    Ask,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pane {
    pub id: String,
    pub title: String,
    #[serde(default)]
    pub refresh: Refresh,
    #[serde(default)]
    pub placement: PanePlacement,
}

/// Where a pane is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PanePlacement {
    /// In the sidebar, with the built-in panes.
    #[default]
    Sidebar,
    /// On the plugin's page in Plugins → Manage Plugins…, below its settings.
    Settings,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Refresh {
    #[default]
    Manual,
    Document,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Format {
    pub id: String,
    pub label: String,
    pub extensions: Vec<String>,
    #[serde(default)]
    pub import: bool,
    #[serde(default)]
    pub export: bool,
}

/// Most models one plugin declares.
pub const MAX_MODELS: usize = 16;
/// Largest model file, in bytes.
pub const MAX_MODEL_SIZE: u64 = 8 << 30;
/// Longest model file name, `license` or `source`.
const MAX_MODEL_TEXT: usize = 200;

/// A `[[models]]` entry: a file the host downloads over https into the
/// plugin's models folder, checks against `size` and `sha256`, and hands to
/// the plugin by path. The plugin never downloads it itself.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Model {
    pub id: String,
    pub url: String,
    /// SHA-256 of the file, as 64 hexadecimal digits.
    pub sha256: String,
    /// Exact size of the file in bytes.
    pub size: u64,
    /// File name in the models folder; by default the last part of the
    /// URL's path, or the id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    /// The model's licence, shown before downloading.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub license: String,
    /// Where the model comes from (a project or paper), shown before downloading.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub source: String,
}

impl Model {
    /// The name of the verified file in the models folder.
    pub fn file_name(&self) -> String {
        if let Some(file) = &self.file {
            return file.clone();
        }
        url::Url::parse(&self.url)
            .ok()
            .and_then(|url| {
                let last = percent_decode(url.path_segments()?.next_back()?)?;
                valid_model_file(&last).is_ok().then_some(last)
            })
            .unwrap_or_else(|| self.id.clone())
    }

    /// The host the model is downloaded from.
    pub fn host(&self) -> String {
        url::Url::parse(&self.url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned))
            .unwrap_or_default()
    }

    /// The expected SHA-256 in lower case.
    pub fn sha256(&self) -> String {
        self.sha256.to_ascii_lowercase()
    }

    fn validate(&self) -> Result<()> {
        validate_id(&self.id)?;
        let url =
            url::Url::parse(&self.url).with_context(|| format!("`{}` is not a URL", self.url))?;
        ensure!(url.scheme() == "https", "url must use https");
        ensure!(
            url.host_str().is_some_and(|host| !host.is_empty()),
            "url needs a host"
        );
        ensure!(
            url.username().is_empty() && url.password().is_none(),
            "url cannot contain a user name or password"
        );
        ensure!(
            self.sha256.len() == 64 && self.sha256.bytes().all(|b| b.is_ascii_hexdigit()),
            "sha256 must be 64 hexadecimal digits"
        );
        ensure!(
            (1..=MAX_MODEL_SIZE).contains(&self.size),
            "size must be between 1 byte and {} GiB",
            MAX_MODEL_SIZE >> 30
        );
        if let Some(file) = &self.file {
            valid_model_file(file)?;
        }
        for (name, text) in [("license", &self.license), ("source", &self.source)] {
            ensure!(
                text.len() <= MAX_MODEL_TEXT && !text.chars().any(char::is_control),
                "{name} must be one line of at most {MAX_MODEL_TEXT} bytes"
            );
        }
        Ok(())
    }
}

/// A plain file name for the models folder: letters, digits, `.`, `-` and
/// `_`, not hidden and not a partial download.
fn valid_model_file(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= MAX_MODEL_TEXT
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b'_'))
            && !name.starts_with(['.', '-'])
            && !name.to_ascii_lowercase().ends_with(".part"),
        "`{name}` is not a plain file name (use A-Z, a-z, 0-9, ., - and _; not starting with . or - or ending in .part)"
    );
    Ok(())
}

/// `%XX` escapes decoded, or `None` when the result is not UTF-8.
fn percent_decode(text: &str) -> Option<String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && let Some(hex) = text.get(index + 1..index + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            index += 3;
        } else {
            out.push(bytes[index]);
            index += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// A keyboard shortcut such as `Ctrl+Shift+E`, parsed from a manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
    pub key: String,
}

impl Shortcut {
    pub fn parse(text: &str) -> Result<Self> {
        let mut shortcut = Self {
            ctrl: false,
            shift: false,
            alt: false,
            key: String::new(),
        };
        let mut parts = text.split('+').map(str::trim).peekable();
        while let Some(part) = parts.next() {
            if parts.peek().is_some() {
                match part.to_ascii_lowercase().as_str() {
                    "ctrl" | "control" | "cmd" | "command" => shortcut.ctrl = true,
                    "shift" => shortcut.shift = true,
                    "alt" | "option" => shortcut.alt = true,
                    _ => bail!("unknown modifier `{part}` in shortcut `{text}`"),
                }
            } else {
                ensure!(!part.is_empty(), "shortcut `{text}` has no key");
                shortcut.key = if part.chars().count() == 1 || function_key(part).is_some() {
                    part.to_ascii_uppercase()
                } else {
                    part.to_owned()
                };
            }
        }
        ensure!(
            shortcut.ctrl || shortcut.alt || shortcut.is_function_key(),
            "shortcut `{text}` needs Ctrl or Alt"
        );
        Ok(shortcut)
    }

    /// Whether the key is one of F1–F24, the only keys usable without Ctrl or Alt.
    pub fn is_function_key(&self) -> bool {
        function_key(&self.key).is_some()
    }
}

/// The number of a function key name `F1`–`F24` (either case).
fn function_key(name: &str) -> Option<u8> {
    let digits = name.strip_prefix(['f', 'F'])?;
    if digits.is_empty() || digits.len() > 2 || digits.starts_with('0') {
        return None;
    }
    let number: u8 = digits.parse().ok()?;
    (1..=24).contains(&number).then_some(number)
}

impl std::fmt::Display for Shortcut {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.ctrl {
            f.write_str("Ctrl+")?;
        }
        if self.shift {
            f.write_str("Shift+")?;
        }
        if self.alt {
            f.write_str("Alt+")?;
        }
        f.write_str(&self.key)
    }
}

/// `text` cut to at most `max` bytes on a character boundary.
fn clip(text: &str, max: usize) -> &str {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

/// This Xuan's version, which `requires_xuan` ranges are matched against.
pub const XUAN_VERSION: &str = env!("CARGO_PKG_VERSION");

/// Whether a `requires_xuan` range admits `version`. A pre-release build
/// counts as its release (`0.4.0-dev` as `0.4.0`), so plugins need not
/// mention pre-releases in their range.
pub fn check_xuan_version(range: &str, version: &str) -> Result<()> {
    let requirement = semver::VersionReq::parse(range)
        .with_context(|| format!("requires_xuan `{range}` is not a semver range"))?;
    let mut current =
        semver::Version::parse(version).with_context(|| format!("`{version}` is not a version"))?;
    current.pre = semver::Prerelease::EMPTY;
    current.build = semver::BuildMetadata::EMPTY;
    ensure!(
        requirement.matches(&current),
        "plugin requires Xuan {range}, but this is Xuan {version}"
    );
    Ok(())
}

/// Characters that are invisible or reorder text: bidi embeddings,
/// overrides, isolates and marks (which can make `gpj.exe` read as
/// `exe.jpg`), zero-width characters and the byte order mark.
pub fn invisible(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{061C}'
            | '\u{180E}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2069}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
    )
}

/// Input ids Xuan sets itself on some runs (`surface` and `target` on
/// surface runs, `capability`, `point` and `rect` on provider runs), so a
/// plugin's own input of that id would be overwritten.
pub const RESERVED_INPUTS: [&str; 5] = ["surface", "target", "capability", "point", "rect"];

pub fn validate_id(id: &str) -> Result<()> {
    ensure!(
        !id.is_empty()
            && id.len() <= MAX_ID
            && id
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
            && !id.starts_with('-'),
        "`{id}` is not a valid identifier (use a-z, 0-9, - and _)"
    );
    Ok(())
}

impl Manifest {
    pub fn parse(text: &str, dir: &Path) -> Result<Self> {
        let mut manifest: Self = toml::from_str(text)?;
        manifest.dir = dir.to_path_buf();
        manifest.validate()?;
        manifest.check_declared_results(&toml::from_str(text)?)?;
        Ok(manifest)
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("Cannot read {}", path.display()))?;
        Self::parse(&text, dir).with_context(|| format!("Invalid {}", path.display()))
    }

    /// Where an action may appear besides its menu depends on what it is.
    fn check_surfaces(&self, action: &Action) -> Result<()> {
        let id = &action.id;
        let edits = self.permissions.document == DocumentAccess::Edit;
        let mut listed = std::collections::HashSet::new();
        for surface in &action.surfaces {
            ensure!(
                listed.insert(*surface),
                "action `{id}` lists a surface twice"
            );
            match surface {
                Surface::Menu => bail!(
                    "action `{id}`: every action is in its menu; surfaces lists only layer, region or document"
                ),
                Surface::Layer => {
                    ensure!(
                        action.kind == ActionKind::Generate
                            || (action.kind == ActionKind::Edit
                                && action.source.from == SourceKind::Composite),
                        "action `{id}`: the layer surface needs a generate action or an edit action with a composite source"
                    );
                    ensure!(
                        action.result.into == ResultInto::Layer,
                        "action `{id}`: the layer surface needs result.into = \"layer\""
                    );
                    ensure!(
                        edits,
                        "action `{id}`: the layer surface needs document = \"edit\""
                    );
                }
                Surface::Region => {
                    ensure!(
                        action.kind == ActionKind::Edit && action.regions_input().is_some(),
                        "action `{id}`: the region surface needs an edit action with a regions input"
                    );
                    ensure!(
                        edits,
                        "action `{id}`: the region surface needs document = \"edit\""
                    );
                    ensure!(
                        !action.verb.trim().is_empty(),
                        "action `{id}`: the region surface needs a verb"
                    );
                    ensure!(
                        action.verb.chars().count() <= 24,
                        "action `{id}`: the verb must be at most 24 characters"
                    );
                    ensure!(
                        !action.verb.chars().any(|c| c.is_control() || invisible(c)),
                        "action `{id}`: the verb must be plain text, without control or invisible characters"
                    );
                }
                Surface::Document => ensure!(
                    action.kind == ActionKind::Generate
                        && matches!(action.result.into, ResultInto::Document | ResultInto::Ask),
                    "action `{id}`: the document surface needs a generate action whose result goes into a document or asks"
                ),
            }
        }
        ensure!(
            action.verb.is_empty() || action.on(Surface::Region),
            "action `{id}` has a verb but no region surface"
        );
        Ok(())
    }

    /// A plugin with `document = "read"` cannot return layers, so an action
    /// that says so in its `result.into` is refused. An action that leaves
    /// `result` out is fine: it may still return masks and new documents,
    /// and the host refuses anything else when the result arrives. `raw` is
    /// the manifest as written, because the default `into` is `layer`.
    fn check_declared_results(&self, raw: &toml::Value) -> Result<()> {
        if self.permissions.document == DocumentAccess::Edit {
            return Ok(());
        }
        let declared = (raw.get("actions").and_then(toml::Value::as_array))
            .into_iter()
            .flatten();
        for (action, raw) in self.actions.iter().zip(declared) {
            let written = raw.get("result").and_then(|result| result.get("into"));
            let what = match action.result.into {
                _ if written.is_none() => continue,
                ResultInto::Layer => "a new layer",
                ResultInto::Replace => "the source layer",
                ResultInto::Ask => "a new layer or a new document, as asked",
                ResultInto::Document => continue,
            };
            bail!(
                "action `{}` puts its result in {what}, but the plugin has document = \"read\", \
                 which can't change the image: use result.into = \"document\" or declare \
                 document = \"edit\" in [permissions]",
                action.id
            );
        }
        Ok(())
    }

    fn validate(&self) -> Result<()> {
        validate_id(&self.plugin.id)?;
        ensure!(!self.plugin.name.trim().is_empty(), "plugin needs a name");
        ensure!(
            !self.plugin.command.is_empty() && !self.plugin.command[0].is_empty(),
            "plugin needs a command"
        );
        ensure!(
            self.plugin.protocol == PROTOCOL,
            "plugin speaks protocol {} but this Xuan speaks {PROTOCOL}",
            self.plugin.protocol
        );
        ensure!(
            self.permissions.edit_prompt == EditPrompt::None
                || self.permissions.document == DocumentAccess::Edit,
            "edit_prompt = \"session\" needs document = \"edit\""
        );
        if let Some(range) = &self.plugin.requires_xuan {
            check_xuan_version(range, XUAN_VERSION)?;
        }
        let mut ids = std::collections::HashSet::new();
        for setting in &self.settings {
            setting
                .validate(false)
                .with_context(|| format!("setting `{}`", setting.id))?;
            ensure!(
                setting.kind != InputKind::Regions,
                "settings cannot be regions"
            );
            ensure!(
                ids.insert(&setting.id),
                "duplicate setting `{}`",
                setting.id
            );
        }
        for secret in &self.permissions.secrets {
            ensure!(
                self.settings
                    .iter()
                    .any(|s| &s.id == secret && s.kind == InputKind::Secret),
                "secret `{secret}` is not a setting of type secret"
            );
        }
        ids.clear();
        for action in &self.actions {
            validate_id(&action.id).with_context(|| format!("action `{}`", action.id))?;
            ensure!(ids.insert(&action.id), "duplicate action `{}`", action.id);
            ensure!(
                !action.label.trim().is_empty(),
                "action `{}` needs a label",
                action.id
            );
            if let Some(shortcut) = &action.shortcut {
                Shortcut::parse(shortcut).with_context(|| format!("action `{}`", action.id))?;
            }
            let mut inputs = std::collections::HashSet::new();
            let mut regions = 0;
            for input in &action.inputs {
                input
                    .validate(false)
                    .with_context(|| format!("action `{}`", action.id))?;
                ensure!(
                    input.kind != InputKind::Secret,
                    "action `{}` cannot ask for a secret; use a setting",
                    action.id
                );
                ensure!(
                    inputs.insert(&input.id),
                    "action `{}` repeats input `{}`",
                    action.id,
                    input.id
                );
                ensure!(
                    !RESERVED_INPUTS.contains(&input.id.as_str()),
                    "action `{}`: `{}` is an input id Xuan sets itself; reserved are {}",
                    action.id,
                    input.id,
                    RESERVED_INPUTS.join(", ")
                );
                let fields = input.fields.iter();
                for shown in std::iter::once(input).chain(fields) {
                    for surface in &shown.surfaces {
                        ensure!(
                            *surface == Surface::Menu || action.on(*surface),
                            "action `{}`: input `{}` is shown on a surface the action does not list",
                            action.id,
                            shown.id
                        );
                    }
                }
                regions += usize::from(input.kind == InputKind::Regions);
            }
            ensure!(
                regions <= 1,
                "action `{}` can have one regions input",
                action.id
            );
            ensure!(
                regions == 0 || action.kind == ActionKind::Edit,
                "action `{}` needs an image to draw regions on",
                action.id
            );
            ensure!(
                action.source.padding.is_finite() && (0.0..=4.0).contains(&action.source.padding),
                "action `{}` padding must be between 0 and 4",
                action.id
            );
            let source = &action.source;
            ensure!(
                source.mask == SourceMask::Selection
                    || (source.mask_feather == 0.0
                        && source.mask_grow == 0
                        && source.mask_empty == MaskEmpty::Error),
                "action `{}` sets mask_feather, mask_grow or mask_empty without source.mask",
                action.id
            );
            ensure!(
                source.mask == SourceMask::None
                    || (action.kind == ActionKind::Edit && source.from != SourceKind::None),
                "action `{}` needs an edit action with a source to send a mask",
                action.id
            );
            ensure!(
                source.mask_feather.is_finite()
                    && (0.0..=MAX_MASK_RADIUS as f32).contains(&source.mask_feather)
                    && source.mask_grow.unsigned_abs() <= MAX_MASK_RADIUS,
                "action `{}` mask_feather and mask_grow must be within {MAX_MASK_RADIUS}",
                action.id
            );
            if let Some(extend) = &source.extend {
                ensure!(
                    action.kind == ActionKind::Edit && source.from == SourceKind::Composite,
                    "action `{}` can only extend a composite source",
                    action.id
                );
                for side in extend.sides() {
                    match side {
                        Amount::Pixels(pixels) => ensure!(
                            *pixels <= MAX_EXTEND,
                            "action `{}` extend sides must be at most {MAX_EXTEND}",
                            action.id
                        ),
                        Amount::Input(id) => ensure!(
                            action.inputs.iter().any(|input| &input.id == id
                                && matches!(input.kind, InputKind::Integer | InputKind::Number)),
                            "action `{}` extends by `{id}`, which is not one of its integer or number inputs",
                            action.id
                        ),
                    }
                }
            }
            ensure!(
                action
                    .source
                    .max_side
                    .is_none_or(|side| (16..=crate::document::MAX_SIDE).contains(&side)),
                "action `{}` max_side must be between 16 and {}",
                action.id,
                crate::document::MAX_SIDE
            );
            self.check_surfaces(action)?;
        }
        ensure!(
            self.models.len() <= MAX_MODELS,
            "a plugin can declare at most {MAX_MODELS} models"
        );
        ids.clear();
        let mut files = std::collections::HashSet::new();
        for model in &self.models {
            model
                .validate()
                .with_context(|| format!("model `{}`", model.id))?;
            ensure!(ids.insert(&model.id), "duplicate model `{}`", model.id);
            let file = model.file_name();
            ensure!(
                files.insert(file.to_ascii_lowercase()),
                "model `{}` uses the file name `{file}` of another model",
                model.id
            );
        }
        for action in &self.actions {
            for model in &action.models {
                ensure!(
                    self.models.iter().any(|m| &m.id == model),
                    "action `{}` needs model `{model}`, which is not in [[models]]",
                    action.id
                );
            }
        }
        let mut capabilities = std::collections::HashSet::new();
        for provide in &self.provides {
            let capability = provide.capability.id();
            ensure!(
                capabilities.insert(provide.capability),
                "capability `{capability}` is provided twice"
            );
            let action = self.action(&provide.action).with_context(|| {
                format!(
                    "capability `{capability}` names action `{}`, which is not in [[actions]]",
                    provide.action
                )
            })?;
            ensure!(
                action.needs_image(),
                "capability `{capability}` needs an edit action with a source image"
            );
            ensure!(
                action.regions_input().is_none(),
                "capability `{capability}` runs without a dialog, so its action cannot ask for regions"
            );
            ensure!(
                provide.capability != Capability::RemoveBackground
                    || self.permissions.document == DocumentAccess::Edit,
                "capability `remove_background` changes the image (a layer mask), so it needs \
                 document = \"edit\" in [permissions]"
            );
        }
        ids.clear();
        for pane in &self.panes {
            validate_id(&pane.id).with_context(|| format!("pane `{}`", pane.id))?;
            ensure!(ids.insert(&pane.id), "duplicate pane `{}`", pane.id);
            ensure!(
                !pane.title.trim().is_empty(),
                "pane `{}` needs a title",
                pane.id
            );
        }
        ids.clear();
        for format in &self.formats {
            validate_id(&format.id).with_context(|| format!("format `{}`", format.id))?;
            ensure!(ids.insert(&format.id), "duplicate format `{}`", format.id);
            ensure!(
                format.import || format.export,
                "format `{}` neither imports nor exports",
                format.id
            );
            ensure!(
                !format.extensions.is_empty()
                    && format
                        .extensions
                        .iter()
                        .all(|e| { !e.is_empty() && e.bytes().all(|b| b.is_ascii_alphanumeric()) }),
                "format `{}` needs plain extensions such as `jxl`",
                format.id
            );
        }
        Ok(())
    }

    pub fn action(&self, id: &str) -> Option<&Action> {
        self.actions.iter().find(|action| action.id == id)
    }

    /// The action that provides `capability`, if the plugin declares one.
    pub fn provider(&self, capability: Capability) -> Option<&Action> {
        let provide = self.provides.iter().find(|p| p.capability == capability)?;
        self.action(&provide.action)
    }

    pub fn pane(&self, id: &str) -> Option<&Pane> {
        self.panes.iter().find(|pane| pane.id == id)
    }

    /// Extensions (lower case) the plugin can import.
    pub fn import_extensions(&self) -> impl Iterator<Item = (&Format, String)> {
        self.formats.iter().filter(|f| f.import).flat_map(|format| {
            format
                .extensions
                .iter()
                .map(move |e| (format, e.to_ascii_lowercase()))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXAMPLE: &str = r#"
[plugin]
id = "comfy-cloud"
name = "Comfy Cloud"
version = "0.1.0"
command = ["python3", "main.py"]

[permissions]
network = ["cloud.comfy.org"]
secrets = ["api_key"]
document = "edit"

[[settings]]
id = "api_key"
type = "secret"
label = "API key"

[[actions]]
id = "precise-edit"
label = "Ideogram Precise Edit…"
menu = "Filter"
shortcut = "Ctrl+Shift+E"
source = { max_side = 2048, crop_to_regions = true }

[[actions.inputs]]
id = "regions"
type = "regions"
fields = [{ id = "desc", type = "text" }, { id = "type", type = "enum", values = ["obj", { id = "text", label = "Text" }] }]

[[actions.inputs]]
id = "seed"
type = "seed"

[[panes]]
id = "jobs"
title = "Comfy jobs"
refresh = "document"

[[panes]]
id = "workflows"
title = "Workflows"
placement = "settings"

[[formats]]
id = "jxl"
label = "JPEG XL"
extensions = ["JXL"]
import = true
"#;

    #[test]
    fn parses_the_documented_manifest() {
        let manifest = Manifest::parse(EXAMPLE, Path::new("/plugins/comfy")).unwrap();
        assert_eq!(manifest.plugin.protocol, PROTOCOL);
        assert_eq!(manifest.dir, Path::new("/plugins/comfy"));
        let action = manifest.action("precise-edit").unwrap();
        assert_eq!(action.menu, Menu::Filter);
        assert_eq!(action.source.max_side, Some(2048));
        assert!(action.source.crop_to_regions);
        assert_eq!(action.source.padding, 0.25);
        assert_eq!(action.result.into, ResultInto::Layer);
        assert!(action.needs_image());
        let regions = action.regions_input().unwrap();
        assert_eq!(regions.fields[1].values[0].label, "obj");
        assert_eq!(regions.fields[1].values[1].label, "Text");
        assert_eq!(regions.fields[1].initial(), Value::String("obj".into()));
        assert_eq!(action.inputs[1].initial(), Value::from(0));
        assert_eq!(manifest.panes[0].refresh, Refresh::Document);
        assert_eq!(manifest.panes[0].placement, PanePlacement::Sidebar);
        assert_eq!(manifest.panes[1].placement, PanePlacement::Settings);
        let imports: Vec<_> = manifest.import_extensions().map(|(_, e)| e).collect();
        assert_eq!(imports, ["jxl"]);
        assert_eq!(
            Shortcut::parse("Ctrl+Shift+E").unwrap().to_string(),
            "Ctrl+Shift+E"
        );
        assert_eq!(Shortcut::parse("f5").unwrap().key, "F5");
        assert!(Shortcut::parse("E").is_err());
        assert!(Shortcut::parse("Meta+E").is_err());
    }

    #[test]
    fn the_edit_prompt_needs_edit_access_and_stays_out_of_old_grants() {
        let base = "[plugin]\nid = \"p\"\nname = \"P\"\nversion = \"1\"\ncommand = [\"x\"]\n";
        let manifest = Manifest::parse(base, Path::new("/p")).unwrap();
        assert_eq!(manifest.permissions.edit_prompt, EditPrompt::None);
        let asking =
            format!("{base}[permissions]\ndocument = \"edit\"\nedit_prompt = \"session\"\n");
        let manifest = Manifest::parse(&asking, Path::new("/p")).unwrap();
        assert_eq!(manifest.permissions.edit_prompt, EditPrompt::Session);
        let reading = format!("{base}[permissions]\nedit_prompt = \"session\"\n");
        let error = format!(
            "{:#}",
            Manifest::parse(&reading, Path::new("/p")).unwrap_err()
        );
        assert!(error.contains("edit_prompt"), "{error}");
        let unknown =
            format!("{base}[permissions]\ndocument = \"edit\"\nedit_prompt = \"always\"\n");
        assert!(Manifest::parse(&unknown, Path::new("/p")).is_err());
        // The default is left out when a grant stores the permissions.
        let stored = toml::to_string(&Permissions::default()).unwrap();
        assert!(!stored.contains("edit_prompt"), "{stored}");
        let stored = toml::to_string(&manifest.permissions).unwrap();
        assert!(stored.contains("edit_prompt = \"session\""), "{stored}");
    }

    #[test]
    fn shortcuts_need_ctrl_or_alt_unless_they_are_function_keys() {
        for (text, expected) in [
            ("F1", Some("F1")),
            ("f5", Some("F5")),
            ("Shift+F12", Some("Shift+F12")),
            ("F24", Some("F24")),
            ("Ctrl+F", Some("Ctrl+F")),
            ("Alt+f", Some("Alt+F")),
            ("Ctrl+Shift+F", Some("Ctrl+Shift+F")),
            ("Ctrl+F25", Some("Ctrl+F25")),
            // Bare letters and digits, with or without Shift, would steal tool keys.
            ("f", None),
            ("F", None),
            ("Shift+F", None),
            ("Shift+f", None),
            ("E", None),
            ("1", None),
            // Only F1–F24 are function keys.
            ("F0", None),
            ("F25", None),
            ("F01", None),
            ("F100", None),
            ("Fx", None),
            ("Shift+Fn", None),
            ("", None),
            ("Ctrl+", None),
        ] {
            let parsed = Shortcut::parse(text).ok().map(|s| s.to_string());
            assert_eq!(parsed.as_deref(), expected, "{text:?}");
        }
        assert!(Shortcut::parse("F7").unwrap().is_function_key());
        assert!(!Shortcut::parse("Ctrl+F").unwrap().is_function_key());
    }

    #[test]
    fn rejects_inconsistent_manifests() {
        let dir = Path::new(".");
        let broken = |replace: &str, with: &str| {
            let error = Manifest::parse(&EXAMPLE.replace(replace, with), dir).unwrap_err();
            format!("{error:#}")
        };
        assert!(broken("id = \"comfy-cloud\"", "id = \"Comfy Cloud\"").contains("identifier"));
        assert!(broken("secrets = [\"api_key\"]", "secrets = [\"token\"]").contains("secret"));
        assert!(broken("command = [\"python3\", \"main.py\"]", "command = []").contains("command"));
        assert!(
            broken("shortcut = \"Ctrl+Shift+E\"", "shortcut = \"Hyper+E\"").contains("modifier")
        );
        assert!(
            broken(
                "id = \"seed\"\ntype = \"seed\"",
                "id = \"seed\"\ntype = \"regions\""
            )
            .contains("one regions")
        );
        assert!(broken("import = true", "import = false").contains("neither"));
        let mask = |source: &str| {
            let action = format!("[[actions]]\nid = \"m\"\nlabel = \"M\"\nsource = {source}\n");
            Manifest::parse(&format!("{EXAMPLE}\n{action}"), dir)
        };
        let ok = mask("{ from = \"composite\", mask = \"selection\", mask_feather = 8, mask_grow = -4, mask_empty = \"white\" }").unwrap();
        let source = &ok.action("m").unwrap().source;
        assert_eq!(source.mask, SourceMask::Selection);
        assert_eq!((source.mask_feather, source.mask_grow), (8.0, -4));
        assert_eq!(source.mask_empty, MaskEmpty::White);
        assert_eq!(
            ok.action("precise-edit").unwrap().source.mask,
            SourceMask::None
        );
        assert!(mask("{ mask = \"selection\", mask_grow = 999 }").is_err());
        assert!(mask("{ mask = \"selection\", mask_feather = -1 }").is_err());
        assert!(mask("{ from = \"none\", mask = \"selection\" }").is_err());
        assert!(mask("{ mask_grow = 4 }").is_err());
        assert!(mask("{ mask = \"layer\" }").is_err());
        assert!(
            Manifest::parse(
                &format!("{EXAMPLE}\n[[actions]]\nid = \"precise-edit\"\nlabel = \"Again\"\n"),
                dir
            )
            .unwrap_err()
            .to_string()
            .contains("duplicate action")
        );
        let newer = EXAMPLE.replace("command = [", "protocol = 2\ncommand = [");
        assert!(
            Manifest::parse(&newer, dir)
                .unwrap_err()
                .to_string()
                .contains("protocol 2")
        );
    }

    #[test]
    fn extend_takes_pixels_or_a_number_input_of_a_composite_edit() {
        use serde_json::json;
        let action = |source: &str, input: &str| {
            let text = format!(
                "{EXAMPLE}\n[[actions]]\nid = \"out\"\nlabel = \"Out\"\nsource = {source}\n{input}"
            );
            Manifest::parse(&text, Path::new("."))
        };
        let amount = "[[actions.inputs]]\nid = \"amount\"\ntype = \"integer\"\ndefault = 64\n";
        let manifest = action(
            "{ from = \"composite\", max_side = 1024, extend = { left = 32, right = \"amount\" } }",
            amount,
        )
        .unwrap();
        let source = &manifest.action("out").unwrap().source;
        let extend = source.extend.as_ref().unwrap();
        assert_eq!(
            (&extend.left, &extend.top, &extend.right),
            (
                &Amount::Pixels(32),
                &Amount::Pixels(0),
                &Amount::Input("amount".into())
            )
        );
        // Input references count once the dialog's values are known.
        assert_eq!(extend.margins().right, 0);
        let resolve = |value: Value| {
            let inputs = serde_json::Map::from_iter([("amount".to_owned(), value)]);
            let margins = source.with_inputs(&inputs).extend.unwrap().margins();
            (margins.left, margins.right)
        };
        assert_eq!(resolve(json!(100)), (32, 100));
        assert_eq!(resolve(json!(12.6)), (32, 13));
        assert_eq!(resolve(json!(-5)), (32, 0));
        assert_eq!(resolve(json!(1e12)), (32, MAX_EXTEND));
        assert_eq!(resolve(json!("lots")), (32, 0));
        assert!(Margins::default().is_empty());
        // Without extend nothing changes.
        assert!(
            manifest
                .action("precise-edit")
                .unwrap()
                .source
                .extend
                .is_none()
        );

        let error = |source: &str, input: &str| action(source, input).unwrap_err().to_string();
        assert!(error("{ from = \"layer\", extend = { left = 8 } }", "").contains("composite"));
        assert!(error("{ from = \"selection\", extend = { left = 8 } }", "").contains("composite"));
        assert!(
            error("{ from = \"composite\", extend = { top = 65536 } }", "").contains("at most")
        );
        assert!(action("{ from = \"composite\", extend = { top = -1 } }", "").is_err());
        assert!(
            error(
                "{ from = \"composite\", extend = { top = \"missing\" } }",
                ""
            )
            .contains("missing")
        );
        let text = "[[actions.inputs]]\nid = \"amount\"\ntype = \"text\"\n";
        assert!(
            error(
                "{ from = \"composite\", extend = { top = \"amount\" } }",
                text
            )
            .contains("integer or number")
        );
        let generate = "{ from = \"composite\", extend = { left = 8 } }\nkind = \"generate\"";
        assert!(error(generate, "").contains("composite"));
    }

    #[test]
    fn requires_xuan_admits_or_refuses_this_version() {
        let dir = Path::new(".");
        let with = |range: &str| {
            let text = EXAMPLE.replace(
                "command = [",
                &format!("requires_xuan = \"{range}\"\ncommand = ["),
            );
            Manifest::parse(&text, dir).map_err(|e| format!("{e:#}"))
        };
        let range = format!(">={XUAN_VERSION}");
        let ok = with(&range).unwrap();
        assert_eq!(ok.plugin.requires_xuan.as_deref(), Some(range.as_str()));
        assert!(
            Manifest::parse(EXAMPLE, dir)
                .unwrap()
                .plugin
                .requires_xuan
                .is_none()
        );
        let error = with("<0.0.1").unwrap_err();
        assert!(
            error.contains("<0.0.1") && error.contains(XUAN_VERSION),
            "{error}"
        );
        assert!(with("not a range").unwrap_err().contains("semver"));
        assert!(check_xuan_version(">=0.3, <0.5", "0.3.0").is_ok());
        assert!(check_xuan_version(">=0.3, <0.5", "0.4.9").is_ok());
        assert!(check_xuan_version(">=0.3, <0.5", "0.4.0-dev").is_ok());
        let error = check_xuan_version(">=0.3, <0.5", "0.5.0").unwrap_err();
        assert_eq!(
            error.to_string(),
            "plugin requires Xuan >=0.3, <0.5, but this is Xuan 0.5.0"
        );
        assert!(check_xuan_version("^0.2", "0.3.0").is_err());
    }

    #[test]
    fn stored_inputs_are_coerced_to_their_spec() {
        use serde_json::json;
        let input = |text: &str| toml::from_str::<Input>(text).unwrap();
        let integer = input("id = 'a'\ntype = 'integer'\nmin = 1\nmax = 10\ndefault = 5");
        assert_eq!(integer.coerce(&json!(3)), json!(3));
        assert_eq!(integer.coerce(&json!(1e300)), json!(10));
        assert_eq!(integer.coerce(&json!(-4.6)), json!(1));
        assert_eq!(integer.coerce(&json!("7")), json!(5));
        assert_eq!(integer.coerce(&json!(null)), json!(5));
        let seed = input("id = 'a'\ntype = 'seed'");
        assert_eq!(seed.coerce(&json!(1e300)), json!(9_000_000_000_000_000_i64));
        let number = input("id = 'a'\ntype = 'number'\nmax = 1.5");
        assert_eq!(number.coerce(&json!(2)), json!(1.5));
        assert_eq!(number.coerce(&json!(0.25)), json!(0.25));
        let text = input("id = 'a'\ntype = 'text'\ndefault = 'hi'");
        assert_eq!(text.coerce(&json!(42)), json!("hi"));
        let long = "é".repeat(MAX_INPUT_TEXT);
        assert!(text.coerce(&json!(long)).as_str().unwrap().len() <= MAX_INPUT_TEXT);
        let choice = input("id = 'a'\ntype = 'enum'\nvalues = ['x', 'y']");
        assert_eq!(choice.coerce(&json!("y")), json!("y"));
        assert_eq!(choice.coerce(&json!("z")), json!("x"));
        let flag = input("id = 'a'\ntype = 'bool'");
        assert_eq!(flag.coerce(&json!(1)), json!(false));
        let color = input("id = 'a'\ntype = 'color'");
        assert_eq!(color.coerce(&json!("#123456")), json!("#123456"));
        assert_eq!(color.coerce(&json!("red")), json!("#ffffff"));
        let secret = input("id = 'a'\ntype = 'secret'");
        assert_eq!(secret.coerce(&json!("sk")), json!(""));

        let regions = input(
            "id = 'r'\ntype = 'regions'\nmax = 2\nfields = [{ id = 'n', type = 'integer', max = 3 }]",
        );
        let value = regions.coerce(&json!([
            {"x": 1, "y": 2, "width": 3, "height": 4, "fields": {"n": 9, "extra": "x"}},
            {"x": "nan", "y": 2, "width": 3, "height": 4},
            {"x": 1, "y": 2, "width": -3, "height": 4},
            {"x": 1e12, "y": 2, "width": 3, "height": 4},
            {"x": 5, "y": 6, "width": 7, "height": 8},
            {"x": 9, "y": 9, "width": 9, "height": 9},
        ]));
        assert_eq!(
            value,
            json!([
                {"x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0, "fields": {"n": 3}},
                {"x": 5.0, "y": 6.0, "width": 7.0, "height": 8.0, "fields": {"n": 0}},
            ])
        );
        assert_eq!(regions.coerce(&json!("x")), json!([]));
        let unbounded = input("id = 'r'\ntype = 'regions'");
        let many: Vec<_> = (0..1000)
            .map(|_| json!({"x": 0, "y": 0, "width": 1, "height": 1}))
            .collect();
        assert_eq!(
            unbounded.coerce(&json!(many)).as_array().unwrap().len(),
            MAX_REGIONS
        );
    }

    #[test]
    fn inputs_another_plugin_sends_are_checked_not_coerced() {
        use serde_json::json;
        let input = |text: &str| toml::from_str::<Input>(text).unwrap();
        let refused = |input: &Input, value: Value| input.check(&value).unwrap_err();
        let integer = input("id = 'steps'\ntype = 'integer'\nmin = 1\nmax = 10");
        assert_eq!(integer.check(&json!(3)), Ok(json!(3)));
        assert_eq!(integer.check(&json!(4.0)), Ok(json!(4)));
        assert_eq!(
            refused(&integer, json!(11)),
            "`steps` must be between 1 and 10"
        );
        assert_eq!(
            refused(&integer, json!(2.5)),
            "`steps` must be a whole number"
        );
        assert_eq!(refused(&integer, json!("3")), "`steps` must be a number");
        let seed = input("id = 'seed'\ntype = 'seed'");
        assert_eq!(seed.check(&json!(1475826651)), Ok(json!(1475826651)));
        assert!(seed.check(&json!(1e300)).is_err());
        let number = input("id = 'strength'\ntype = 'number'\nmin = 0");
        assert_eq!(number.check(&json!(0.25)), Ok(json!(0.25)));
        assert_eq!(
            refused(&number, json!(-0.5)),
            "`strength` must be at least 0"
        );
        let text = input("id = 'prompt'\ntype = 'multiline'");
        assert_eq!(text.check(&json!("a hat")), Ok(json!("a hat")));
        assert_eq!(
            refused(&text, json!("é".repeat(MAX_INPUT_TEXT))),
            "`prompt` is longer than 64 KiB"
        );
        assert_eq!(refused(&text, json!(42)), "`prompt` must be a string");
        let choice = input("id = 'model'\ntype = 'enum'\nvalues = ['pro', 'flash']");
        assert_eq!(choice.check(&json!("flash")), Ok(json!("flash")));
        assert_eq!(
            refused(&choice, json!("fast")),
            "`model` must be one of pro, flash"
        );
        let flag = input("id = 'a'\ntype = 'bool'");
        assert_eq!(flag.check(&json!(true)), Ok(json!(true)));
        assert!(flag.check(&json!(1)).is_err());
        let color = input("id = 'tint'\ntype = 'color'");
        assert_eq!(color.check(&json!("#12345680")), Ok(json!("#12345680")));
        assert!(color.check(&json!("red")).is_err());
        assert!(
            input("id = 'k'\ntype = 'secret'")
                .check(&json!("sk"))
                .is_err()
        );

        let regions = input(
            "id = 'boxes'\ntype = 'regions'\nmin = 1\nmax = 2\nfields = [{ id = 'n', type = 'integer', max = 3 }, { id = 'desc', type = 'text' }]",
        );
        assert_eq!(
            regions.check(&json!([{"x": 1, "y": 2, "width": 3, "height": 4, "fields": {"n": 2}}])),
            Ok(
                json!([{"x": 1.0, "y": 2.0, "width": 3.0, "height": 4.0, "fields": {"n": 2, "desc": ""}}])
            )
        );
        for (value, says) in [
            (json!([]), "`boxes` needs at least 1 region"),
            (
                Value::Array(vec![json!({"x": 0, "y": 0, "width": 1, "height": 1}); 3]),
                "`boxes` takes at most 2 regions",
            ),
            (
                json!([{"x": 0, "y": 0, "width": 0, "height": 1}]),
                "width and height above 0",
            ),
            (
                json!([{"x": 1e12, "y": 0, "width": 1, "height": 1}]),
                "needs `x`",
            ),
            (
                json!([{"x": 0, "y": 0, "width": 1, "height": 1, "mask": "a.png"}]),
                "unknown key `mask`",
            ),
            (
                json!([{"x": 0, "y": 0, "width": 1, "height": 1, "fields": {"n": 4}}]),
                "`boxes` region 1: `n` must be at most 3",
            ),
            (
                json!([{"x": 0, "y": 0, "width": 1, "height": 1, "fields": {"color": "red"}}]),
                "no field `color`; its fields are n, desc",
            ),
            (json!("x"), "must be a list of regions"),
        ] {
            let error = refused(&regions, value.clone());
            assert!(error.contains(says), "{value}: {error}");
        }
    }

    #[test]
    fn an_actions_inputs_from_another_plugin_take_defaults_and_refuse_files() {
        use serde_json::json;
        let manifest = Manifest::parse(
            "[plugin]\nid = 'p'\nname = 'P'\nversion = '1'\ncommand = ['p']\n\n[[actions]]\nid = 'go'\nlabel = 'Go'\n\n[[actions.inputs]]\nid = 'prompt'\ntype = 'text'\ndefault = 'hello'\n\n[[actions.inputs]]\nid = 'model'\ntype = 'enum'\nvalues = ['a', 'b']\ndefault = 'b'\n\n[[actions.inputs]]\nid = 'reference'\ntype = 'path'\n\n[[actions]]\nid = 'boxes'\nlabel = 'Boxes'\n\n[[actions.inputs]]\nid = 'regions'\ntype = 'regions'\nmin = 1\n",
            Path::new("."),
        )
        .unwrap();
        let go = manifest.action("go").unwrap();
        let values = go.check_inputs(None).unwrap();
        assert_eq!(
            Value::Object(values),
            json!({"prompt": "hello", "model": "b", "reference": ""})
        );
        let values = go
            .check_inputs(Some(&json!({"prompt": "a kite", "model": null})))
            .unwrap();
        assert_eq!(values["prompt"], "a kite");
        assert_eq!(values["model"], "b");
        assert_eq!(
            go.check_inputs(Some(&json!({"reference": "/home/me/.ssh/id_ed25519"})))
                .unwrap_err(),
            "`reference` is a file the user chooses; another plugin cannot set it"
        );
        assert_eq!(
            go.check_inputs(Some(&json!({"size": 3}))).unwrap_err(),
            "The action has no input `size`; its inputs are prompt, model"
        );
        assert_eq!(
            go.check_inputs(Some(&json!({"model": "c"}))).unwrap_err(),
            "`model` must be one of a, b"
        );
        assert!(go.check_inputs(Some(&json!(["a"]))).is_err());
        // Regions the action needs must be given.
        let boxes = manifest.action("boxes").unwrap();
        assert_eq!(
            boxes.check_inputs(None).unwrap_err(),
            "`regions` needs at least 1 region"
        );
        assert!(
            boxes
                .check_inputs(Some(
                    &json!({"regions": [{"x": 0, "y": 0, "width": 4, "height": 4}]})
                ))
                .is_ok()
        );
    }

    #[test]
    fn inputs_start_from_defaults_or_sensible_blanks() {
        let input = |text: &str| toml::from_str::<Input>(text).unwrap();
        assert_eq!(
            input("id = 'a'\ntype = 'number'\nmin = 2.5").initial(),
            Value::from(2.5)
        );
        assert_eq!(
            input("id = 'a'\ntype = 'bool'\ndefault = true").initial(),
            Value::Bool(true)
        );
        assert_eq!(
            input("id = 'a'\ntype = 'color'").initial(),
            Value::String("#ffffff".into())
        );
        assert_eq!(
            input("id = 'a'\ntype = 'regions'").initial(),
            Value::Array(vec![])
        );
        assert_eq!(input("id = 'a'\ntype = 'text'").label(), "a");
    }

    #[test]
    fn models_are_validated() {
        let dir = Path::new(".");
        let sha = "ab".repeat(32);
        let model = |extra: &str| {
            format!(
                "[[models]]\nid = \"net\"\nurl = \"https://models.example/v1/net.onnx\"\nsha256 = \"{sha}\"\nsize = 1024\n{extra}\n"
            )
        };
        let parse = |models: &str, action_models: &str| {
            let text = EXAMPLE.replace(
                "crop_to_regions = true }\n",
                &format!("crop_to_regions = true }}\nmodels = [{action_models}]\n"),
            );
            Manifest::parse(&format!("{text}\n{models}"), dir).map_err(|e| format!("{e:#}"))
        };
        let manifest = parse(
            &model("license = \"Apache-2.0\"\nsource = \"U2-Net\"\r"),
            "\"net\"",
        )
        .unwrap();
        let net = &manifest.models[0];
        assert_eq!((net.size, net.file_name()), (1024, "net.onnx".into()));
        assert_eq!(net.host(), "models.example");
        assert_eq!(net.license, "Apache-2.0");
        assert_eq!(manifest.action("precise-edit").unwrap().models, ["net"]);
        assert!(parse("", "").unwrap().models.is_empty());
        // Upper-case hashes are fine and compared in lower case.
        let upper = model("").replace(&sha, &sha.to_uppercase());
        assert_eq!(parse(&upper, "").unwrap().models[0].sha256(), sha);

        for (models, expected) in [
            (model("").replace("https://", "http://"), "https"),
            (
                model("").replace("https://", "https://user:pw@"),
                "user name",
            ),
            (
                model("").replace("https://models.example/v1/net.onnx", "net.onnx"),
                "URL",
            ),
            (model("").replace(&sha, "abc"), "64 hexadecimal"),
            (model("").replace(&sha, &"zz".repeat(32)), "64 hexadecimal"),
            (model("").replace("size = 1024", "size = 0"), "size"),
            (
                model("").replace("size = 1024", "size = 8589934593"),
                "8 GiB",
            ),
            (model("").replace("size = 1024\n", ""), "size"),
            (
                model("").replace("id = \"net\"", "id = \"Net!\""),
                "identifier",
            ),
            (model("file = \"../escape.onnx\""), "plain file name"),
            (model("file = \".verified.json\""), "plain file name"),
            (model("file = \"net.part\""), "plain file name"),
            (model("file = \"a/b\""), "plain file name"),
            (model("license = \"two\\nlines\""), "one line"),
            (
                model(&format!("source = \"{}\"", "x".repeat(201))),
                "one line",
            ),
            (format!("{}{}", model(""), model("")), "duplicate model"),
            (
                format!(
                    "{}{}",
                    model(""),
                    model("").replace("id = \"net\"", "id = \"copy\"")
                ),
                "file name `net.onnx`",
            ),
            (
                (0..=MAX_MODELS)
                    .map(|n| {
                        model(&format!("file = \"m{n}\""))
                            .replace("id = \"net\"", &format!("id = \"m{n}\""))
                    })
                    .collect(),
                "at most 16 models",
            ),
        ] {
            let error = parse(&models, "").unwrap_err();
            assert!(error.contains(expected), "{expected}: {error}");
        }
        let error = parse(&model(""), "\"other\"").unwrap_err();
        assert!(error.contains("needs model `other`"), "{error}");
        // A file name not usable from the URL falls back to the id.
        let odd = model("").replace("v1/net.onnx", "v1/%2E%2Ehidden");
        assert_eq!(parse(&odd, "").unwrap().models[0].file_name(), "net");
    }

    #[test]
    fn actions_declare_surfaces_and_inputs_can_be_advanced() {
        let text = r#"
    [plugin]
    id = "ai"
    name = "AI"
    version = "0.1.0"
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
        assert!(
            layer.inputs[2].shown_on(Surface::Menu) && !layer.inputs[2].shown_on(Surface::Layer)
        );
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
                "[plugin]\nid = \"ai\"\nname = \"AI\"\nversion = \"0.1.0\"\ncommand = [\"sh\"]\n\n[permissions]\ndocument = \"{document}\"\n\n{action}"
            )
        };
        let refused = |action: &str, document: &str| {
            format!(
                "{:#}",
                Manifest::parse(&base(action, document), dir).unwrap_err()
            )
        };
        // layer: composite edit or generate, into layer, document = edit
        assert!(refused("[[actions]]\nid = \"a\"\nlabel = \"A\"\nsurfaces = [\"layer\"]\nsource = { from = \"layer\" }", "edit").contains("layer"));
        assert!(refused("[[actions]]\nid = \"a\"\nlabel = \"A\"\nkind = \"generate\"\nsurfaces = [\"layer\"]", "read").contains("document = \"edit\""));
        // region: regions input and a short verb
        assert!(
            refused(
                "[[actions]]\nid = \"a\"\nlabel = \"A\"\nsurfaces = [\"region\"]\nverb = \"Edit\"",
                "edit"
            )
            .contains("regions")
        );
        let regions = "\n\n[[actions.inputs]]\nid = \"r\"\ntype = \"regions\"";
        assert!(
            refused(
                &format!(
                    "[[actions]]\nid = \"a\"\nlabel = \"A\"\nsurfaces = [\"region\"]{regions}"
                ),
                "edit"
            )
            .contains("verb")
        );
        assert!(refused(&format!("[[actions]]\nid = \"a\"\nlabel = \"A\"\nsurfaces = [\"region\"]\nverb = \"{}\"{regions}", "x".repeat(25)), "edit").contains("24"));
        assert!(
            refused(
                "[[actions]]\nid = \"a\"\nlabel = \"A\"\nverb = \"Edit\"",
                "edit"
            )
            .contains("verb")
        );
        // document: generate into document or ask
        assert!(
            refused(
                "[[actions]]\nid = \"a\"\nlabel = \"A\"\nsurfaces = [\"document\"]",
                "edit"
            )
            .contains("generate")
        );
        // menu is not an action surface; no repeats
        assert!(refused("[[actions]]\nid = \"a\"\nlabel = \"A\"\nkind = \"generate\"\nsurfaces = [\"menu\"]", "edit").contains("menu"));
        assert!(refused("[[actions]]\nid = \"a\"\nlabel = \"A\"\nkind = \"generate\"\nsurfaces = [\"document\", \"document\"]\nresult = { into = \"document\" }", "edit").contains("twice"));
        // A verb is plain text: no control, bidi or invisible characters.
        for verb in ["Ed\\u0007it", "Ed\\u202Eit", "Ed\\u200Bit"] {
            let action = format!(
                "[[actions]]\nid = \"a\"\nlabel = \"A\"\nsurfaces = [\"region\"]\nverb = \"{verb}\"{regions}"
            );
            assert!(refused(&action, "edit").contains("plain text"), "{verb}");
        }
        // Inputs: shown only on surfaces the action lists, each once, and
        // never under an id Xuan sets itself.
        let generate = "[[actions]]\nid = \"a\"\nlabel = \"A\"\nkind = \"generate\"\nsurfaces = [\"document\"]\nresult = { into = \"document\" }\n\n[[actions.inputs]]\ntype = \"text\"\n";
        assert!(
            refused(
                &format!("{generate}id = \"p\"\nsurfaces = [\"layer\"]"),
                "edit"
            )
            .contains("does not list")
        );
        assert!(
            refused(
                &format!("{generate}id = \"p\"\nsurfaces = [\"menu\", \"menu\"]"),
                "edit"
            )
            .contains("twice")
        );
        assert!(
            Manifest::parse(
                &base(
                    &format!("{generate}id = \"p\"\nsurfaces = [\"menu\", \"document\"]"),
                    "edit"
                ),
                dir
            )
            .is_ok()
        );
        for id in RESERVED_INPUTS {
            assert!(
                refused(&format!("{generate}id = \"{id}\""), "edit").contains("reserved"),
                "{id}"
            );
        }
        let field = format!(
            "[[actions]]\nid = \"a\"\nlabel = \"A\"\nsurfaces = [\"region\"]\nverb = \"Edit\"{regions}\nfields = [{{ id = \"d\", type = \"text\", surfaces = [\"layer\"] }}]"
        );
        assert!(refused(&field, "edit").contains("does not list"));
    }
}
