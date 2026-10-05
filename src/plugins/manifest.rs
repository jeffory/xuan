//! `plugin.toml`: what a plugin offers and needs. See `docs/PLUGINS.md`.
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The protocol version this host speaks.
pub const PROTOCOL: u32 = 1;
pub const MANIFEST_FILE: &str = "plugin.toml";
const MAX_ID: usize = 64;

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
}

impl Permissions {
    pub fn is_empty(&self) -> bool {
        self.network.is_empty()
            && self.secrets.is_empty()
            && self.document == DocumentAccess::Read
            && self.filesystem == FilesystemAccess::None
    }
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
}

impl Input {
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

    fn validate(&self, nested: bool) -> Result<()> {
        validate_id(&self.id).with_context(|| format!("input `{}`", self.id))?;
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
}

impl Default for Source {
    fn default() -> Self {
        Self {
            from: SourceKind::Layer,
            max_side: None,
            crop_to_regions: false,
            padding: 0.25,
        }
    }
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
        Ok(manifest)
    }

    pub fn load(dir: &Path) -> Result<Self> {
        let path = dir.join(MANIFEST_FILE);
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("Cannot read {}", path.display()))?;
        Self::parse(&text, dir).with_context(|| format!("Invalid {}", path.display()))
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
            ensure!(
                action
                    .source
                    .max_side
                    .is_none_or(|side| (16..=30_000).contains(&side)),
                "action `{}` max_side must be between 16 and 30000",
                action.id
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
}
