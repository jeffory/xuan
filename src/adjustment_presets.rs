//! Adjustment presets: a named stack of adjustment layers, kept in the configuration folder and
//! added to any document again (upstream Compositor's request #171).
//!
//! A preset keeps each layer's name, visibility, opacity, blend mode and adjustment settings, in
//! order from the bottom. Applying it adds ordinary adjustment layers above the active layer;
//! they share nothing with the preset, so editing them leaves it alone. A Color Lookup's table
//! is kept in the preset as `.cube` text, so the preset needs no other file.
//!
//! Each preset is one JSON file in `adjustment-presets/` beside `config.toml`:
//! `{"format": "me.silverl.xuan.adjustment-preset", "version": 1, "name", "layers": [{"name",
//! "visible", "opacity", "blend", "adjustment", "table"?}]}`. Files are read as untrusted
//! input: sizes are bounded and everything is validated, and a file that fails is skipped.

use std::{
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::Arc,
};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    blend::BlendMode,
    document::{Adjustment, Document, Layer, MAX_LAYERS},
    lut::Lut,
};

const FORMAT: &str = "me.silverl.xuan.adjustment-preset";
const VERSION: u32 = 1;
/// Longest preset name, in characters.
pub const MAX_NAME: usize = 100;
/// Most layers in one preset.
pub const MAX_LAYERS_PER_PRESET: usize = 100;
/// Most presets read from the folder.
pub const MAX_PRESETS: usize = 1000;
/// Largest preset file: room for a few of the largest Color Lookup tables.
pub const MAX_FILE_BYTES: u64 = 64 * 1024 * 1024;
/// The folder beside `config.toml` that holds the presets.
pub const FOLDER: &str = "adjustment-presets";

/// One adjustment layer of a preset.
#[derive(Clone, Debug, PartialEq)]
pub struct PresetLayer {
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    pub blend: BlendMode,
    pub adjustment: Adjustment,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    pub name: String,
    /// Bottom to top, as in a document.
    pub layers: Vec<PresetLayer>,
}

#[derive(Serialize, Deserialize)]
struct StoredLayer {
    name: String,
    visible: bool,
    opacity: f32,
    blend: BlendMode,
    adjustment: Adjustment,
    /// A Color Lookup's table, as `.cube` text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    table: Option<String>,
}

#[derive(Serialize, Deserialize)]
struct StoredPreset {
    format: String,
    version: u32,
    name: String,
    layers: Vec<StoredLayer>,
}

/// A preset name, trimmed: one line of 1 to [`MAX_NAME`] characters.
pub fn check_name(name: &str) -> Result<String> {
    let name = name.trim();
    ensure!(!name.is_empty(), "Give the preset a name");
    ensure!(
        name.chars().count() <= MAX_NAME,
        "Preset names are limited to {MAX_NAME} characters"
    );
    ensure!(
        !name.chars().any(char::is_control),
        "Preset names must be on one line"
    );
    Ok(name.to_owned())
}

impl Preset {
    /// The selected adjustment layers of `document`, from the bottom up, under `name`. Other
    /// selected layers are left out.
    pub fn from_document(document: &Document, name: &str) -> Result<Self> {
        let name = check_name(name)?;
        let layers: Vec<_> = document
            .layers
            .iter()
            .filter(|layer| document.selected.contains(&layer.id))
            .filter_map(|layer| {
                Some(PresetLayer {
                    name: layer.name.clone(),
                    visible: layer.visible,
                    opacity: layer.opacity,
                    blend: layer.blend,
                    adjustment: layer.adjustment.clone()?,
                })
            })
            .collect();
        ensure!(!layers.is_empty(), "Select one or more adjustment layers");
        ensure!(
            layers.len() <= MAX_LAYERS_PER_PRESET,
            "A preset holds at most {MAX_LAYERS_PER_PRESET} layers"
        );
        let preset = Self { name, layers };
        preset.validate()?;
        Ok(preset)
    }

    pub fn validate(&self) -> Result<()> {
        check_name(&self.name)?;
        ensure!(
            (1..=MAX_LAYERS_PER_PRESET).contains(&self.layers.len()),
            "A preset holds 1 to {MAX_LAYERS_PER_PRESET} layers"
        );
        for layer in &self.layers {
            ensure!(
                !layer.name.trim().is_empty() && layer.name.len() <= 16_384,
                "Invalid layer name"
            );
            ensure!(
                layer.opacity.is_finite() && (0.0..=1.0).contains(&layer.opacity),
                "Invalid opacity"
            );
            crate::effects::validate_adjustment(&layer.adjustment)?;
        }
        Ok(())
    }

    /// Add the preset's layers above the active layer, in order, and select them. The new layers
    /// cover the canvas and have no mask. Returns their ids, bottom first.
    pub fn apply(&self, document: &mut Document) -> Result<Vec<Uuid>> {
        ensure!(
            document.layers.len() + self.layers.len() <= MAX_LAYERS,
            "Too many layers"
        );
        let mut added = Vec::with_capacity(self.layers.len());
        for preset in &self.layers {
            let mut layer = Layer::blank(&preset.name, document.width, document.height);
            layer.visible = preset.visible;
            layer.opacity = preset.opacity;
            layer.blend = preset.blend;
            layer.adjustment = Some(preset.adjustment.clone());
            added.push(layer.id);
            // Each goes above the one before, which `insert` made active.
            document.insert(layer);
        }
        document.selected = added.iter().copied().collect();
        document.active = added.last().copied();
        Ok(added)
    }

    pub fn to_json(&self) -> Result<Vec<u8>> {
        self.validate()?;
        let stored = StoredPreset {
            format: FORMAT.into(),
            version: VERSION,
            name: self.name.clone(),
            layers: self
                .layers
                .iter()
                .map(|layer| StoredLayer {
                    name: layer.name.clone(),
                    visible: layer.visible,
                    opacity: layer.opacity,
                    blend: layer.blend,
                    table: match &layer.adjustment {
                        Adjustment::ColorLookup { table, .. } => Some(table.to_cube()),
                        _ => None,
                    },
                    adjustment: layer.adjustment.clone(),
                })
                .collect(),
        };
        let json = serde_json::to_vec_pretty(&stored)?;
        ensure!(
            json.len() as u64 <= MAX_FILE_BYTES,
            "The preset is larger than {}",
            crate::limits::size(MAX_FILE_BYTES)
        );
        Ok(json)
    }

    pub fn from_json(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() as u64 <= MAX_FILE_BYTES,
            "Presets are limited to {}",
            crate::limits::size(MAX_FILE_BYTES)
        );
        let stored: StoredPreset = serde_json::from_slice(bytes)?;
        ensure!(stored.format == FORMAT, "Not an adjustment preset");
        ensure!(
            (1..=VERSION).contains(&stored.version),
            "Unsupported adjustment preset version"
        );
        ensure!(
            stored.layers.len() <= MAX_LAYERS_PER_PRESET,
            "A preset holds at most {MAX_LAYERS_PER_PRESET} layers"
        );
        let layers = stored
            .layers
            .into_iter()
            .map(|layer| {
                let mut adjustment = layer.adjustment;
                match (&mut adjustment, layer.table) {
                    (Adjustment::ColorLookup { table, .. }, Some(text)) => {
                        *table = Arc::new(Lut::parse(&text)?);
                    }
                    (Adjustment::ColorLookup { .. }, None) => {
                        bail!("A Color Lookup layer has no table")
                    }
                    (_, Some(_)) => bail!("Only a Color Lookup layer has a table"),
                    (_, None) => {}
                }
                Ok(PresetLayer {
                    name: layer.name,
                    visible: layer.visible,
                    opacity: layer.opacity,
                    blend: layer.blend,
                    adjustment,
                })
            })
            .collect::<Result<_>>()?;
        let preset = Self {
            name: stored.name,
            layers,
        };
        preset.validate()?;
        Ok(preset)
    }

    /// Read a preset file, refusing one larger than [`MAX_FILE_BYTES`].
    pub fn load(path: &Path) -> Result<Self> {
        let metadata = fs::metadata(path)?;
        ensure!(
            metadata.is_file() && metadata.len() <= MAX_FILE_BYTES,
            "Not a preset file, or larger than {}",
            crate::limits::size(MAX_FILE_BYTES)
        );
        let mut bytes = Vec::new();
        File::open(path)?
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        Self::from_json(&bytes)
    }
}

/// A preset and the file it is kept in (none when presets are not saved, as in tests).
#[derive(Clone, Debug)]
pub struct Stored {
    pub preset: Arc<Preset>,
    pub path: Option<PathBuf>,
}

/// The user's presets, sorted by name.
#[derive(Clone, Debug, Default)]
pub struct Library {
    /// Where presets are saved; `None` keeps them in memory only.
    pub folder: Option<PathBuf>,
    pub presets: Vec<Stored>,
}

impl Library {
    /// The presets in `folder`, and a note for each file that could not be read. A missing
    /// folder is an empty library.
    pub fn open(folder: Option<PathBuf>) -> (Self, Vec<String>) {
        let mut library = Self {
            folder,
            presets: Vec::new(),
        };
        let mut problems = Vec::new();
        let Some(folder) = &library.folder else {
            return (library, problems);
        };
        let entries = match fs::read_dir(folder) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return (library, problems);
            }
            Err(error) => {
                problems.push(format!("{}: {error}", folder.display()));
                return (library, problems);
            }
        };
        let mut paths: Vec<_> = entries
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|e| e == "json"))
            .collect();
        paths.sort();
        for path in paths.into_iter().take(MAX_PRESETS) {
            match Preset::load(&path) {
                Ok(preset) => library.presets.push(Stored {
                    preset: Arc::new(preset),
                    path: Some(path),
                }),
                Err(error) => problems.push(format!("{}: {error:#}", path.display())),
            }
        }
        library.sort();
        (library, problems)
    }

    fn sort(&mut self) {
        self.presets
            .sort_by_cached_key(|stored| stored.preset.name.to_lowercase());
    }

    pub fn find(&self, name: &str) -> Option<usize> {
        self.presets.iter().position(|s| s.preset.name == name)
    }

    /// Keep `preset`, replacing one of the same name, and write it to its file.
    pub fn save(&mut self, preset: Preset) -> Result<()> {
        let json = preset.to_json()?;
        let existing = self.find(&preset.name);
        let path = match &self.folder {
            Some(folder) => {
                let path = match existing.and_then(|i| self.presets[i].path.clone()) {
                    Some(path) => path,
                    None => free_path(folder, &preset.name)?,
                };
                fs::create_dir_all(folder)
                    .with_context(|| format!("Cannot create {}", folder.display()))?;
                let mut file = tempfile::NamedTempFile::new_in(folder)?;
                file.write_all(&json)?;
                file.as_file().sync_all()?;
                file.persist(&path)
                    .with_context(|| format!("Cannot save {}", path.display()))?;
                Some(path)
            }
            None => None,
        };
        let stored = Stored {
            preset: Arc::new(preset),
            path,
        };
        match existing {
            Some(index) => self.presets[index] = stored,
            None => {
                ensure!(
                    self.presets.len() < MAX_PRESETS,
                    "Xuan keeps at most {MAX_PRESETS} presets"
                );
                self.presets.push(stored);
            }
        }
        self.sort();
        Ok(())
    }

    /// Forget the preset at `index` and delete its file.
    pub fn delete(&mut self, index: usize) -> Result<()> {
        let stored = self.presets.get(index).context("No such preset")?;
        if let Some(path) = &stored.path {
            match fs::remove_file(path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => {
                    return Err(error).with_context(|| format!("Cannot delete {}", path.display()));
                }
            }
        }
        self.presets.remove(index);
        Ok(())
    }
}

/// A file name for a new preset: its name made safe on every platform, and not yet taken.
fn free_path(folder: &Path, name: &str) -> Result<PathBuf> {
    let mut stem = String::new();
    for c in name.chars() {
        if c.is_alphanumeric() {
            stem.extend(c.to_lowercase());
        } else if !stem.is_empty() && !stem.ends_with('-') {
            stem.push('-');
        }
        if stem.chars().count() >= 48 {
            break;
        }
    }
    let mut stem = stem.trim_end_matches('-').to_owned();
    // Windows keeps these names for devices, with any extension.
    const RESERVED: [&str; 4] = ["con", "prn", "aux", "nul"];
    let device = RESERVED.contains(&stem.as_str())
        || ((stem.starts_with("com") || stem.starts_with("lpt"))
            && stem.len() == 4
            && stem.ends_with(|c: char| c.is_ascii_digit()));
    if stem.is_empty() || device {
        stem.insert_str(0, "preset-");
        stem = stem.trim_end_matches('-').to_owned();
    }
    for number in 1..=MAX_PRESETS + 1 {
        let file = if number == 1 {
            format!("{stem}.json")
        } else {
            format!("{stem}-{number}.json")
        };
        let path = folder.join(file);
        if !path.exists() {
            return Ok(path);
        }
    }
    bail!("Too many presets named like this one")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lut::{Dimension, Interpolation};

    /// A document with an image and three adjustment layers, all selected with the image.
    fn graded() -> Document {
        let mut document = Document::new(8, 8).unwrap();
        document.layers[0].pixels = Some(Arc::new(image::RgbaImage::from_fn(8, 8, |x, y| {
            image::Rgba([(x * 30) as u8, (y * 30) as u8, 120, 255])
        })));
        let mut table = Lut::identity(Dimension::Three, 5);
        for entry in &mut table.table {
            *entry = [entry[1], entry[0], entry[2] * 0.5];
        }
        for (name, adjustment, opacity, blend) in [
            (
                "Warm",
                Adjustment::ColorBalance {
                    shadows: [10.0, 0.0, -5.0],
                    midtones: [20.0, 5.0, -10.0],
                    highlights: [0.0; 3],
                    preserve_luminosity: true,
                },
                0.8,
                BlendMode::Normal,
            ),
            (
                "Look",
                Adjustment::ColorLookup {
                    name: "look.cube".into(),
                    interpolation: Interpolation::Trilinear,
                    table: Arc::new(table.clone()),
                },
                0.5,
                BlendMode::Normal,
            ),
            (
                "Punch",
                Adjustment::Exposure {
                    exposure: 0.3,
                    offset: 0.0,
                    gamma: 1.1,
                },
                1.0,
                BlendMode::Luminosity,
            ),
        ] {
            let mut layer = Layer::blank(name, 8, 8);
            layer.adjustment = Some(adjustment);
            layer.opacity = opacity;
            layer.blend = blend;
            document.layers.push(layer);
        }
        document.layers[2].visible = false;
        document.selected = document.layers.iter().map(|l| l.id).collect();
        document.active = Some(document.layers[3].id);
        document
    }

    fn settings(document: &Document) -> Vec<(String, bool, f32, BlendMode, Adjustment)> {
        document
            .layers
            .iter()
            .filter_map(|l| {
                Some((
                    l.name.clone(),
                    l.visible,
                    l.opacity,
                    l.blend,
                    l.adjustment.clone()?,
                ))
            })
            .collect()
    }

    #[test]
    fn a_preset_keeps_the_selected_adjustment_layers_in_order() {
        let source = graded();
        let preset = Preset::from_document(&source, "  Golden hour ").unwrap();
        assert_eq!(preset.name, "Golden hour");
        // The image is selected too but is not an adjustment layer.
        assert_eq!(preset.layers.len(), 3);
        assert_eq!(preset.layers[1].name, "Look");
        assert!(!preset.layers[1].visible);

        let mut target = Document::new(20, 10).unwrap();
        let base = target.layers[0].id;
        let added = preset.apply(&mut target).unwrap();
        assert_eq!(added.len(), 3);
        assert_eq!(settings(&target), settings(&source));
        // Above the active layer, in order, selected, and covering this canvas.
        let ids: Vec<_> = target.layers.iter().map(|l| l.id).collect();
        assert_eq!(ids[0], base);
        assert_eq!(ids[1..], added[..]);
        assert_eq!(target.active, Some(added[2]));
        assert_eq!(target.selected.len(), 3);
        assert_eq!(target.layers[1].transform.width, 20.0);
        target.validate().unwrap();

        // Changing an applied layer leaves the preset alone.
        target.layers[1].opacity = 0.1;
        if let Some(Adjustment::ColorBalance { midtones, .. }) = &mut target.layers[1].adjustment {
            midtones[0] = -50.0;
        }
        assert_eq!(preset.layers[0].opacity, 0.8);
        assert_eq!(
            Some(&preset.layers[0].adjustment),
            source.layers[1].adjustment.as_ref()
        );
    }

    #[test]
    fn a_preset_needs_a_name_and_an_adjustment_layer() {
        let mut document = graded();
        assert!(Preset::from_document(&document, "  ").is_err());
        assert!(Preset::from_document(&document, &"x".repeat(MAX_NAME + 1)).is_err());
        assert!(Preset::from_document(&document, "two\nlines").is_err());
        document.selected = [document.layers[0].id].into();
        let error = Preset::from_document(&document, "Image only").unwrap_err();
        assert_eq!(error.to_string(), "Select one or more adjustment layers");
    }

    #[test]
    fn presets_round_trip_through_json_with_their_tables() {
        let preset = Preset::from_document(&graded(), "Golden").unwrap();
        let json = preset.to_json().unwrap();
        let value: serde_json::Value = serde_json::from_slice(&json).unwrap();
        assert_eq!(value["format"], FORMAT);
        assert_eq!(value["version"], 1);
        assert!(
            value["layers"][1]["table"]
                .as_str()
                .unwrap()
                .contains("LUT_3D_SIZE 5")
        );
        assert!(value["layers"][0].get("table").is_none());
        assert_eq!(Preset::from_json(&json).unwrap(), preset);
    }

    #[test]
    fn broken_or_hostile_preset_files_are_refused() {
        let json = Preset::from_document(&graded(), "Golden")
            .unwrap()
            .to_json()
            .unwrap();
        let good: serde_json::Value = serde_json::from_slice(&json).unwrap();
        let changed = |change: &dyn Fn(&mut serde_json::Value)| {
            let mut value = good.clone();
            change(&mut value);
            serde_json::to_vec(&value).unwrap()
        };
        let cases: Vec<(Vec<u8>, &str)> = vec![
            (b"not json".to_vec(), "expected"),
            (
                changed(&|v| v["format"] = "other".into()),
                "Not an adjustment preset",
            ),
            (changed(&|v| v["version"] = 2.into()), "Unsupported"),
            (
                changed(&|v| v["name"] = "".into()),
                "Give the preset a name",
            ),
            (
                changed(&|v| v["layers"] = serde_json::json!([])),
                "1 to 100 layers",
            ),
            (
                changed(&|v| v["layers"][0]["opacity"] = 2.0.into()),
                "Invalid opacity",
            ),
            (
                changed(&|v| {
                    v["layers"][0]["adjustment"]["ColorBalance"]["shadows"][0] = 500.into()
                }),
                "ColorBalance.shadows",
            ),
            (
                changed(&|v| {
                    v["layers"][1]["table"].take();
                }),
                "has no table",
            ),
            (
                changed(&|v| v["layers"][0]["table"] = "LUT_3D_SIZE 2".into()),
                "Only a Color Lookup layer",
            ),
            (
                changed(&|v| v["layers"][1]["table"] = "LUT_3D_SIZE 5\n0 0 0\n".into()),
                "incomplete",
            ),
            (
                changed(&|v| {
                    let layer = v["layers"][0].clone();
                    v["layers"] = serde_json::Value::Array(vec![layer; MAX_LAYERS_PER_PRESET + 1]);
                }),
                "at most 100 layers",
            ),
        ];
        for (bytes, reason) in cases {
            let error = format!("{:#}", Preset::from_json(&bytes).unwrap_err());
            assert!(error.contains(reason), "{reason}: {error}");
        }
        assert!(Preset::from_json(&vec![b' '; MAX_FILE_BYTES as usize + 1]).is_err());
    }

    #[test]
    fn the_library_saves_replaces_lists_and_deletes_preset_files() {
        let directory = tempfile::tempdir().unwrap();
        let folder = directory.path().join(FOLDER);
        let (mut library, problems) = Library::open(Some(folder.clone()));
        assert!(library.presets.is_empty() && problems.is_empty());

        let document = graded();
        for name in ["zebra", "Golden hour", "golden: hour", "CON", "色调"] {
            library
                .save(Preset::from_document(&document, name).unwrap())
                .unwrap();
        }
        let names: Vec<_> = library
            .presets
            .iter()
            .map(|s| s.preset.name.as_str())
            .collect();
        assert_eq!(
            names,
            ["CON", "Golden hour", "golden: hour", "zebra", "色调"]
        );
        let mut files: Vec<_> = fs::read_dir(&folder)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        files.sort();
        assert_eq!(
            files,
            [
                "golden-hour-2.json",
                "golden-hour.json",
                "preset-con.json",
                "zebra.json",
                "色调.json"
            ]
        );

        // Saving under a name in use replaces that preset and its file.
        let mut single = document.clone();
        single.selected = [document.layers[3].id].into();
        library
            .save(Preset::from_document(&single, "zebra").unwrap())
            .unwrap();
        assert_eq!(library.presets.len(), 5);
        assert_eq!(fs::read_dir(&folder).unwrap().count(), 5);

        // A broken file and a file of another kind are skipped; the rest come back.
        fs::write(folder.join("broken.json"), "{").unwrap();
        fs::write(folder.join("notes.txt"), "hello").unwrap();
        let (reopened, problems) = Library::open(Some(folder.clone()));
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(problems[0].contains("broken.json"));
        let zebra = &reopened.presets[reopened.find("zebra").unwrap()];
        assert_eq!(zebra.preset.layers.len(), 1);
        for stored in &reopened.presets {
            let index = library.find(&stored.preset.name).unwrap();
            assert_eq!(stored.preset, library.presets[index].preset);
        }

        let index = library.find("zebra").unwrap();
        library.delete(index).unwrap();
        assert!(library.find("zebra").is_none());
        assert!(!folder.join("zebra.json").exists());
        assert!(library.delete(99).is_err());

        // Without a folder presets stay in memory.
        let mut memory = Library::default();
        memory
            .save(Preset::from_document(&document, "Only here").unwrap())
            .unwrap();
        assert!(memory.presets[0].path.is_none());
        memory.delete(0).unwrap();
    }
}
