//! Canvas size presets for File → New, and the arithmetic behind its Swap and Keep aspect ratio
//! controls (issues 95 and 140).
//!
//! There are two lists. Pixel presets (Full High Definition, a square post) are offered while
//! File → New shows pixels; physical presets (A4, Letter) keep their size in millimetres,
//! centimetres or inches and are offered while it shows a print unit. Each preset has a group,
//! shown as a heading in the menu, and the presets of a group are kept together.
//!
//! The built-in lists are used until the user changes one in the app. Both lists are then kept
//! in `canvas-presets.toml` beside `config.toml`; Xuan never rewrites that file otherwise, so
//! hand edits and comments stay, and deleting it restores the built-in lists:
//!
//! ```toml
//! version = 1
//!
//! [[pixel]]
//! group = "Screens"
//! name = "Full High Definition"
//! width = 1920
//! height = 1080
//!
//! [[physical]]
//! group = "ISO A"
//! name = "A4"
//! width = 210
//! height = 297
//! unit = "mm"        # mm, cm, in, pt or pica
//! resolution = 300   # optional, in pixels per inch
//! ```
//!
//! The file is read as untrusted input: its size, the number of presets and the length of names
//! are bounded, and an entry that fails is skipped with a note. A list the file leaves out is the
//! built-in one.
use std::{
    fs::{self, File},
    io::{Read, Write},
    path::Path,
};

use anyhow::{Context, Result, bail, ensure};

use crate::{
    document::MAX_SIDE,
    units::{self, Unit},
};

/// The file beside `config.toml` that keeps the lists once the user changes one.
pub const FILE: &str = "canvas-presets.toml";
const VERSION: i64 = 1;
/// Most presets in one list.
pub const MAX_PRESETS: usize = 200;
/// Longest preset or group name, in characters.
pub const MAX_NAME: usize = 64;
/// Largest presets file read.
pub const MAX_FILE_BYTES: u64 = 1024 * 1024;
/// Most notes kept about skipped entries.
const MAX_PROBLEMS: usize = 20;
/// The resolution of the built-in physical presets, as in Photoshop's print presets.
pub const PRINT_RESOLUTION: f32 = 300.0;
/// The group of a preset saved or read without one.
pub const DEFAULT_GROUP: &str = "Saved";

/// Which list: pixel sizes, or physical (print) sizes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Kind {
    #[default]
    Pixel,
    Physical,
}

impl Kind {
    /// The list File → New offers while its fields show `unit`.
    pub fn of(unit: Unit) -> Self {
        if unit.is_physical() {
            Self::Physical
        } else {
            Self::Pixel
        }
    }

    /// The list's name in the file.
    fn key(self) -> &'static str {
        match self {
            Self::Pixel => "pixel",
            Self::Physical => "physical",
        }
    }
}

/// A preset's size: whole pixels, or a print size in a physical unit with the resolution
/// picking it sets, if any.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Size {
    Pixels([u32; 2]),
    Physical {
        width: f64,
        height: f64,
        unit: Unit,
        /// Pixels per inch; without one, picking the preset keeps the dialog's resolution.
        resolution: Option<f32>,
    },
}

/// A named canvas size.
#[derive(Clone, Debug, PartialEq)]
pub struct Preset {
    /// Shown in the menu as typed, translated when it is a built-in name.
    pub name: String,
    /// The heading the preset is shown under.
    pub group: String,
    pub size: Size,
}

/// A preset or group name, trimmed: one line of 1 to [`MAX_NAME`] characters.
pub fn check_name(name: &str) -> Result<String> {
    let name = name.trim();
    ensure!(!name.is_empty(), "Give the preset a name");
    ensure!(
        name.chars().count() <= MAX_NAME,
        "Names are limited to {MAX_NAME} characters"
    );
    ensure!(
        !name.chars().any(char::is_control),
        "Names must be on one line"
    );
    Ok(name.to_owned())
}

/// `value` with at most `decimals` decimals and no trailing zeros: 210, 8.5, 7.25.
pub fn format_number(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    let text = if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.')
    } else {
        &text
    };
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

/// `value` rounded to the decimals a field in `unit` shows, as a saved physical preset keeps it.
pub fn round_to_field(value: f64, unit: Unit) -> f64 {
    let scale = 10_f64.powi(unit.decimals() as i32);
    (value * scale).round() / scale
}

impl Preset {
    pub fn pixels(name: &str, group: &str, size: [u32; 2]) -> Self {
        Self {
            name: name.into(),
            group: group.into(),
            size: Size::Pixels(size),
        }
    }

    pub fn physical(
        name: &str,
        group: &str,
        [width, height]: [f64; 2],
        unit: Unit,
        resolution: Option<f32>,
    ) -> Self {
        Self {
            name: name.into(),
            group: group.into(),
            size: Size::Physical {
                width,
                height,
                unit,
                resolution,
            },
        }
    }

    pub fn kind(&self) -> Kind {
        match self.size {
            Size::Pixels(_) => Kind::Pixel,
            Size::Physical { .. } => Kind::Physical,
        }
    }

    /// The whole pixels the preset makes at `ppi`; a pixel preset ignores it. `None` when a
    /// print size cannot be converted at `ppi`.
    pub fn pixels_at(&self, ppi: f32) -> Option<[u32; 2]> {
        match self.size {
            Size::Pixels(size) => Some(size),
            Size::Physical {
                width,
                height,
                unit,
                ..
            } => {
                let side =
                    |value| units::whole_pixels(unit.to_pixels(value, f64::from(ppi), 0.0)?);
                Some([side(width)?, side(height)?])
            }
        }
    }

    /// The print size in inches; `None` for a pixel preset.
    pub fn inches(&self) -> Option<[f64; 2]> {
        match self.size {
            Size::Pixels(_) => None,
            Size::Physical {
                width,
                height,
                unit,
                ..
            } => Some([
                unit.to_pixels(width, 1.0, 0.0)?,
                unit.to_pixels(height, 1.0, 0.0)?,
            ]),
        }
    }

    /// The size as the menu shows it after the name: `1920 × 1080`, `210 × 297 mm`,
    /// `8.5 × 11 in`.
    pub fn size_text(&self) -> String {
        match self.size {
            Size::Pixels([width, height]) => format!("{width} × {height}"),
            Size::Physical {
                width,
                height,
                unit,
                ..
            } => format!(
                "{} × {} {}",
                format_number(width, unit.decimals()),
                format_number(height, unit.decimals()),
                unit.suffix()
            ),
        }
    }

    /// Whether the preset may be offered: a valid name and group, and a size File → New can
    /// make. A physical size must be positive and at most [`MAX_SIDE`] inches (one pixel per
    /// inch); with a resolution, the pixels it makes must be a canvas this computer allows.
    pub fn validate(&self) -> Result<()> {
        check_name(&self.name)?;
        check_name(&self.group).context("Invalid group")?;
        match self.size {
            Size::Pixels([width, height]) => {
                crate::document::validate_size(width, height)?;
            }
            Size::Physical {
                width,
                height,
                unit,
                resolution,
            } => {
                ensure!(unit.is_physical(), "Not a print unit: {}", unit.suffix());
                for side in [width, height] {
                    ensure!(
                        side.is_finite() && side > 0.0,
                        "Sizes must be positive numbers"
                    );
                }
                let inches = self.inches().context("Invalid size")?;
                ensure!(
                    inches.iter().all(|side| *side <= f64::from(MAX_SIDE)),
                    "The size is too large"
                );
                if let Some(ppi) = resolution {
                    ensure!(
                        units::valid_resolution(f64::from(ppi)),
                        "Resolutions are {} to {} pixels per inch",
                        units::MIN_RESOLUTION,
                        units::MAX_RESOLUTION
                    );
                    ensure!(
                        inches
                            .iter()
                            .all(|side| (side * f64::from(ppi)).round() <= f64::from(MAX_SIDE)),
                        "The size is too large at this resolution"
                    );
                    let [width, height] = self.pixels_at(ppi).context("Invalid size")?;
                    crate::document::validate_size(width, height)?;
                }
            }
        }
        Ok(())
    }
}

type Table = &'static [(&'static str, &'static [(&'static str, f64, f64)])];

const PIXEL: Table = &[
    (
        "Screens",
        &[
            ("8K Ultra HD", 7680.0, 4320.0),
            ("4K Ultra HD", 3840.0, 2160.0),
            ("Quad HD", 2560.0, 1440.0),
            ("Full High Definition", 1920.0, 1080.0),
            ("High Definition", 1280.0, 720.0),
        ],
    ),
    (
        "Social",
        &[
            ("Square post", 1080.0, 1080.0),
            ("Portrait post", 1080.0, 1350.0),
            ("Landscape post", 1080.0, 566.0),
            ("Story / Reel", 1080.0, 1920.0),
            ("Video thumbnail", 1280.0, 720.0),
            ("Link preview", 1200.0, 630.0),
            ("Banner", 1500.0, 500.0),
        ],
    ),
];

// ISO 216 (A) and 269 (B) in millimetres; US and photo sizes in inches. A0 is left out: at
// 300 ppi it is more pixels than every computer allows. Tabloid and Ledger are one sheet in the
// two orientations, which the menu matches as one.
const MILLIMETRES: Table = &[
    (
        "ISO A",
        &[
            ("A1", 594.0, 841.0),
            ("A2", 420.0, 594.0),
            ("A3", 297.0, 420.0),
            ("A4", 210.0, 297.0),
            ("A5", 148.0, 210.0),
            ("A6", 105.0, 148.0),
        ],
    ),
    ("ISO B", &[("B4", 250.0, 353.0), ("B5", 176.0, 250.0)]),
];

const INCHES: Table = &[
    (
        "US",
        &[
            ("Letter", 8.5, 11.0),
            ("Legal", 8.5, 14.0),
            ("Tabloid / Ledger", 11.0, 17.0),
            ("Half Letter", 5.5, 8.5),
            ("Executive", 7.25, 10.5),
        ],
    ),
    (
        "Photo",
        &[
            ("4 × 6", 4.0, 6.0),
            ("5 × 7", 5.0, 7.0),
            ("8 × 10", 8.0, 10.0),
        ],
    ),
];

/// The pixel and physical presets File → New offers.
#[derive(Clone, Debug, PartialEq)]
pub struct Presets {
    pixel: Vec<Preset>,
    physical: Vec<Preset>,
}

impl Default for Presets {
    /// The built-in lists.
    fn default() -> Self {
        let pixel = PIXEL
            .iter()
            .flat_map(|(group, presets)| {
                presets
                    .iter()
                    .map(|(name, w, h)| Preset::pixels(name, group, [*w as u32, *h as u32]))
            })
            .collect();
        let physical = [(MILLIMETRES, Unit::Millimeters), (INCHES, Unit::Inches)]
            .into_iter()
            .flat_map(|(table, unit)| table.iter().map(move |group| (group, unit)))
            .flat_map(|((group, presets), unit)| {
                presets.iter().map(move |(name, w, h)| {
                    Preset::physical(name, group, [*w, *h], unit, Some(PRINT_RESOLUTION))
                })
            })
            .collect();
        Self { pixel, physical }
    }
}

/// Puts the presets of each group together, groups in the order they first appear and presets
/// in their order within the group.
fn regroup(list: &mut [Preset]) {
    let mut groups: Vec<String> = Vec::new();
    for preset in list.iter() {
        if !groups.contains(&preset.group) {
            groups.push(preset.group.clone());
        }
    }
    list.sort_by_key(|preset| groups.iter().position(|group| *group == preset.group));
}

impl Presets {
    /// Lists of the given presets, each grouped; presets of the other kind are left out.
    pub fn new(pixel: Vec<Preset>, physical: Vec<Preset>) -> Self {
        let mut presets = Self {
            pixel: pixel
                .into_iter()
                .filter(|p| p.kind() == Kind::Pixel)
                .collect(),
            physical: physical
                .into_iter()
                .filter(|p| p.kind() == Kind::Physical)
                .collect(),
        };
        regroup(&mut presets.pixel);
        regroup(&mut presets.physical);
        presets
    }

    /// One list, in menu order.
    pub fn list(&self, kind: Kind) -> &[Preset] {
        match kind {
            Kind::Pixel => &self.pixel,
            Kind::Physical => &self.physical,
        }
    }

    fn list_mut(&mut self, kind: Kind) -> &mut Vec<Preset> {
        match kind {
            Kind::Pixel => &mut self.pixel,
            Kind::Physical => &mut self.physical,
        }
    }

    /// Each group of a list in menu order, with the indices of its presets.
    pub fn groups(&self, kind: Kind) -> Vec<(&str, std::ops::Range<usize>)> {
        let mut groups: Vec<(&str, std::ops::Range<usize>)> = Vec::new();
        for (index, preset) in self.list(kind).iter().enumerate() {
            match groups.last_mut() {
                Some((group, range)) if *group == preset.group => range.end = index + 1,
                _ => groups.push((&preset.group, index..index + 1)),
            }
        }
        groups
    }

    /// The index of the preset named `name` in a list.
    pub fn position(&self, kind: Kind, name: &str) -> Option<usize> {
        self.list(kind).iter().position(|p| p.name == name)
    }

    /// The preset a size shows as in the menu, or `None` for Custom. A size matches a preset in
    /// either orientation, so a swapped Full High Definition still shows as that; an exact match
    /// wins over a swapped one (1080 × 1920 is Story / Reel, not a turned Full High Definition).
    /// A physical preset matches the pixels it makes at `ppi`: its print size to half a pixel.
    pub fn find(&self, kind: Kind, size: [u32; 2], ppi: f32) -> Option<usize> {
        let pixels: Vec<_> = self.list(kind).iter().map(|p| p.pixels_at(ppi)).collect();
        (pixels.iter().position(|p| *p == Some(size)))
            .or_else(|| pixels.iter().position(|p| *p == Some(swapped(size))))
    }

    /// Adds `preset` at the end of its group (a new group goes last), or replaces the preset of
    /// the same name in its list. Returns its index.
    pub fn insert(&mut self, mut preset: Preset) -> Result<usize> {
        preset.name = check_name(&preset.name)?;
        preset.group = check_name(&preset.group).context("Invalid group")?;
        preset.validate()?;
        let kind = preset.kind();
        let name = preset.name.clone();
        let list = self.list_mut(kind);
        match list.iter().position(|p| p.name == name) {
            Some(index) if list[index].group == preset.group => list[index] = preset,
            Some(index) => {
                list.remove(index);
                list.push(preset);
            }
            None => {
                ensure!(
                    list.len() < MAX_PRESETS,
                    "A list holds at most {MAX_PRESETS} presets"
                );
                list.push(preset);
            }
        }
        regroup(list);
        self.position(kind, &name).context("Preset not kept")
    }

    /// Renames the preset at `index`; a new `group` moves it to the end of that group. Returns
    /// its new index.
    pub fn rename(&mut self, kind: Kind, index: usize, name: &str, group: &str) -> Result<usize> {
        let name = check_name(name)?;
        let group = check_name(group).context("Invalid group")?;
        let list = self.list_mut(kind);
        ensure!(index < list.len(), "No such preset");
        ensure!(
            !(list.iter().enumerate()).any(|(i, p)| i != index && p.name == name),
            "Another preset has this name"
        );
        list[index].name = name.clone();
        if list[index].group != group {
            // To the end of its new group, as a new preset would go.
            let mut preset = list.remove(index);
            preset.group = group;
            list.push(preset);
            regroup(list);
        }
        self.position(kind, &name).context("Preset not kept")
    }

    /// Removes the preset at `index`.
    pub fn remove(&mut self, kind: Kind, index: usize) -> Option<Preset> {
        let list = self.list_mut(kind);
        (index < list.len()).then(|| list.remove(index))
    }

    /// Moves the preset at `index` one place up or down within its group. Returns its new
    /// index, or `None` when it is already first (or last) in its group.
    pub fn move_within_group(&mut self, kind: Kind, index: usize, up: bool) -> Option<usize> {
        let list = self.list_mut(kind);
        let other = if up {
            index.checked_sub(1)?
        } else {
            index.checked_add(1)?
        };
        if other >= list.len() || index >= list.len() || list[other].group != list[index].group {
            return None;
        }
        list.swap(index, other);
        Some(other)
    }

    /// Reads the lists from the text of a presets file. Entries that fail are skipped and
    /// described in the notes returned; a list the file leaves out is the built-in one. Fails
    /// when the text is not TOML or is of a newer version.
    pub fn from_toml(text: &str) -> Result<(Self, Vec<String>)> {
        let table: toml::Table = text.parse()?;
        match table.get("version") {
            None => {}
            Some(toml::Value::Integer(version)) if (1..=VERSION).contains(version) => {}
            Some(version) => bail!("Unsupported canvas presets version {version}"),
        }
        let built_in = Self::default();
        let mut problems = Vec::new();
        let mut note = |problem: String| {
            if problems.len() < MAX_PROBLEMS {
                problems.push(problem);
            }
        };
        let mut read = |kind: Kind| -> Vec<Preset> {
            let key = kind.key();
            let entries = match table.get(key) {
                None => return built_in.list(kind).to_vec(),
                Some(toml::Value::Array(entries)) => entries,
                Some(_) => {
                    note(format!("`{key}` is not a list of presets"));
                    return built_in.list(kind).to_vec();
                }
            };
            let mut list: Vec<Preset> = Vec::new();
            for (number, entry) in entries.iter().enumerate() {
                if list.len() == MAX_PRESETS {
                    note(format!(
                        "{key}: only the first {MAX_PRESETS} presets are kept"
                    ));
                    break;
                }
                match read_entry(kind, entry) {
                    Ok(preset) if list.iter().any(|p| p.name == preset.name) => note(format!(
                        "{key} preset {}: another preset is named {:?}",
                        number + 1,
                        preset.name
                    )),
                    Ok(preset) => list.push(preset),
                    Err(error) => note(format!("{key} preset {}: {error:#}", number + 1)),
                }
            }
            list
        };
        let pixel = read(Kind::Pixel);
        let physical = read(Kind::Physical);
        Ok((Self::new(pixel, physical), problems))
    }

    /// The lists as a presets file.
    pub fn to_toml(&self) -> String {
        let quoted = |text: &str| toml::Value::String(text.into()).to_string();
        let mut text = String::from(
            "# Xuan canvas presets: the sizes File → New offers. Delete this file to restore the\n\
             # built-in lists. Pixel sizes are in pixels; physical sizes are in `unit` (mm, cm,\n\
             # in, pt or pica), with an optional `resolution` in pixels per inch.\n",
        );
        text.push_str(&format!("version = {VERSION}\n"));
        for kind in [Kind::Pixel, Kind::Physical] {
            if self.list(kind).is_empty() {
                text.push_str(&format!("{} = []\n", kind.key()));
            }
        }
        for kind in [Kind::Pixel, Kind::Physical] {
            for preset in self.list(kind) {
                text.push_str(&format!(
                    "\n[[{}]]\ngroup = {}\nname = {}\n",
                    kind.key(),
                    quoted(&preset.group),
                    quoted(&preset.name)
                ));
                match preset.size {
                    Size::Pixels([width, height]) => {
                        text.push_str(&format!("width = {width}\nheight = {height}\n"));
                    }
                    Size::Physical {
                        width,
                        height,
                        unit,
                        resolution,
                    } => {
                        text.push_str(&format!(
                            "width = {width}\nheight = {height}\nunit = {}\n",
                            quoted(unit.suffix())
                        ));
                        if let Some(ppi) = resolution {
                            text.push_str(&format!("resolution = {ppi}\n"));
                        }
                    }
                }
            }
        }
        text
    }

    /// Reads the presets file at `path`: `None` when there is none, so the built-in lists apply,
    /// and otherwise the lists with notes about skipped entries. Fails when the file cannot be
    /// read, is larger than [`MAX_FILE_BYTES`], or is not a presets file.
    pub fn read(path: &Path) -> Result<Option<(Self, Vec<String>)>> {
        let file = match File::open(path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("Cannot read {}", path.display()));
            }
        };
        let read = || -> Result<(Self, Vec<String>)> {
            ensure!(file.metadata()?.is_file(), "Not a file");
            let mut bytes = Vec::new();
            file.take(MAX_FILE_BYTES + 1).read_to_end(&mut bytes)?;
            ensure!(
                bytes.len() as u64 <= MAX_FILE_BYTES,
                "Presets files are limited to {}",
                crate::limits::size(MAX_FILE_BYTES)
            );
            Self::from_toml(std::str::from_utf8(&bytes)?)
        };
        read()
            .map(Some)
            .with_context(|| format!("Cannot read {}", path.display()))
    }

    /// Writes both lists to `path`. A file there that cannot be read is the user's to fix or
    /// delete, so it is left untouched and this fails.
    pub fn write(&self, path: &Path) -> Result<()> {
        Self::read(path)?;
        let parent = path.parent().context("Presets path has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("Cannot create {}", parent.display()))?;
        let mut file = tempfile::NamedTempFile::new_in(parent)?;
        file.write_all(self.to_toml().as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(path)
            .with_context(|| format!("Cannot save {}", path.display()))?;
        Ok(())
    }
}

/// Deletes the presets file at `path`, so the built-in lists apply again.
pub fn restore_defaults(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("Cannot delete {}", path.display())),
    }
}

/// One `[[pixel]]` or `[[physical]]` entry of a presets file.
fn read_entry(kind: Kind, entry: &toml::Value) -> Result<Preset> {
    let entry = entry.as_table().context("Not a table")?;
    let text = |key: &str| -> Result<Option<&str>> {
        match entry.get(key) {
            None => Ok(None),
            Some(toml::Value::String(text)) => Ok(Some(text)),
            Some(_) => bail!("`{key}` is not text"),
        }
    };
    let number = |key: &str| -> Result<Option<f64>> {
        match entry.get(key) {
            None => Ok(None),
            Some(toml::Value::Integer(value)) => Ok(Some(*value as f64)),
            Some(toml::Value::Float(value)) => Ok(Some(*value)),
            Some(_) => bail!("`{key}` is not a number"),
        }
    };
    let name = check_name(text("name")?.context("No name")?)?;
    let group = check_name(text("group")?.unwrap_or(DEFAULT_GROUP)).context("Invalid group")?;
    let width = number("width")?.context("No width")?;
    let height = number("height")?.context("No height")?;
    let unit = match text("unit")? {
        Some(unit) => {
            Some(Unit::from_suffix(unit).with_context(|| format!("Unknown unit {unit:?}"))?)
        }
        None => None,
    };
    let preset = match kind {
        Kind::Pixel => {
            ensure!(
                unit.is_none_or(|unit| unit == Unit::Pixels),
                "Pixel presets are in pixels"
            );
            let side = |value: f64| -> Result<u32> {
                ensure!(
                    value.fract() == 0.0 && (1.0..=f64::from(MAX_SIDE)).contains(&value),
                    "Sizes are whole numbers of pixels from 1 to {MAX_SIDE}"
                );
                Ok(value as u32)
            };
            Preset::pixels(&name, &group, [side(width)?, side(height)?])
        }
        Kind::Physical => {
            let unit = unit.context("No unit")?;
            let resolution = number("resolution")?
                .map(|ppi| {
                    ensure!(
                        units::valid_resolution(ppi),
                        "Resolutions are {} to {} pixels per inch",
                        units::MIN_RESOLUTION,
                        units::MAX_RESOLUTION
                    );
                    Ok(ppi as f32)
                })
                .transpose()?;
            Preset::physical(&name, &group, [width, height], unit, resolution)
        }
    };
    preset.validate()?;
    Ok(preset)
}

/// `size` with width and height exchanged: portrait becomes landscape and back.
pub fn swapped(size: [u32; 2]) -> [u32; 2] {
    [size[1], size[0]]
}

/// The side that keeps `reference` (width, height) in proportion when `edited` is the new
/// length of the other side. Rounds to the nearest pixel and stays within `1..=MAX_SIDE`.
/// A reference with a zero side has no proportion, so `fallback` is returned unchanged.
pub fn linked_side(reference_edited: u32, reference_other: u32, edited: u32, fallback: u32) -> u32 {
    if reference_edited == 0 || reference_other == 0 {
        return fallback;
    }
    let scaled = (u128::from(reference_other) * u128::from(edited)
        + u128::from(reference_edited) / 2)
        / u128::from(reference_edited);
    u32::try_from(scaled).map_or(MAX_SIDE, |v| v.clamp(1, MAX_SIDE))
}

/// The remembered size when it is still a usable canvas on this computer.
pub fn usable(size: Option<[u32; 2]>) -> Option<[u32; 2]> {
    size.filter(|[w, h]| crate::document::validate_size(*w, *h).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(presets: &Presets, kind: Kind, index: Option<usize>) -> Option<&str> {
        Some(presets.list(kind)[index?].name.as_str())
    }

    fn pixel_names(presets: &Presets) -> Vec<&str> {
        (presets.list(Kind::Pixel).iter())
            .map(|p| p.name.as_str())
            .collect()
    }

    #[test]
    fn built_in_lists_are_sane() {
        let presets = Presets::default();
        for kind in [Kind::Pixel, Kind::Physical] {
            let mut names = std::collections::HashSet::new();
            for p in presets.list(kind) {
                assert!(names.insert(&p.name), "duplicate preset {}", p.name);
                assert_eq!(p.kind(), kind);
                p.validate().unwrap();
            }
        }
        assert_eq!(presets.list(Kind::Pixel).len(), 12);
        assert_eq!(presets.list(Kind::Physical).len(), 16);
        let groups: Vec<_> = (presets.groups(Kind::Physical).into_iter())
            .map(|(group, range)| (group, range.len()))
            .collect();
        assert_eq!(
            groups,
            [("ISO A", 6), ("ISO B", 2), ("US", 5), ("Photo", 3)]
        );
        assert_eq!(
            presets.groups(Kind::Pixel),
            [("Screens", 0..5), ("Social", 5..12)]
        );
    }

    #[test]
    fn paper_sizes_make_known_pixels() {
        let presets = Presets::default();
        let pixels = |name: &str, ppi: f32| {
            let index = presets.position(Kind::Physical, name).unwrap();
            presets.list(Kind::Physical)[index].pixels_at(ppi).unwrap()
        };
        assert_eq!(pixels("A4", 300.0), [2480, 3508]);
        assert_eq!(pixels("A4", 150.0), [1240, 1754]);
        assert_eq!(pixels("A4", 72.0), [595, 842]);
        assert_eq!(pixels("A3", 300.0), [3508, 4961]);
        assert_eq!(pixels("A5", 300.0), [1748, 2480]);
        assert_eq!(pixels("A1", 300.0), [7016, 9933]);
        assert_eq!(pixels("B5", 300.0), [2079, 2953]);
        assert_eq!(pixels("Letter", 300.0), [2550, 3300]);
        assert_eq!(pixels("Letter", 72.0), [612, 792]);
        assert_eq!(pixels("Legal", 300.0), [2550, 4200]);
        assert_eq!(pixels("Tabloid / Ledger", 300.0), [3300, 5100]);
        assert_eq!(pixels("Executive", 300.0), [2175, 3150]);
        assert_eq!(pixels("4 × 6", 300.0), [1200, 1800]);
        // A size in centimetres converts the same way.
        let cm = Preset::physical("A4", "ISO A", [21.0, 29.7], Unit::Centimeters, None);
        assert_eq!(cm.pixels_at(300.0), Some([2480, 3508]));
        // No usable resolution, no pixels.
        assert_eq!(cm.pixels_at(0.0), None);
        assert_eq!(cm.pixels_at(f32::NAN), None);
        assert_eq!(
            Preset::pixels("x", "y", [3, 4]).pixels_at(f32::NAN),
            Some([3, 4])
        );
    }

    #[test]
    fn labels_show_the_size_in_its_unit() {
        let presets = Presets::default();
        let text = |kind, name: &str| {
            presets.list(kind)[presets.position(kind, name).unwrap()].size_text()
        };
        assert_eq!(text(Kind::Pixel, "Full High Definition"), "1920 × 1080");
        assert_eq!(text(Kind::Physical, "A4"), "210 × 297 mm");
        assert_eq!(text(Kind::Physical, "Letter"), "8.5 × 11 in");
        assert_eq!(text(Kind::Physical, "Executive"), "7.25 × 10.5 in");
        let cm = Preset::physical("x", "y", [21.0, 29.7], Unit::Centimeters, None);
        assert_eq!(cm.size_text(), "21 × 29.7 cm");
        // More decimals than the field shows are rounded.
        let fine = Preset::physical("x", "y", [8.12345, 1.0], Unit::Inches, None);
        assert_eq!(fine.size_text(), "8.123 × 1 in");
        assert_eq!(format_number(209.97, 1), "210");
        assert_eq!(format_number(-0.0001, 2), "0");
        assert_eq!(round_to_field(209.973, Unit::Millimeters), 210.0);
        assert_eq!(round_to_field(8.50049, Unit::Inches), 8.5);
    }

    #[test]
    fn exact_swapped_and_custom_matches() {
        let presets = Presets::default();
        let find = |size| name(&presets, Kind::Pixel, presets.find(Kind::Pixel, size, 72.0));
        assert_eq!(find([1920, 1080]), Some("Full High Definition"));
        assert_eq!(find([1080, 1350]), Some("Portrait post"));
        // Turned presets keep their name.
        assert_eq!(find([1350, 1080]), Some("Portrait post"));
        assert_eq!(find([1080, 1920]), Some("Story / Reel"));
        assert_eq!(find([2160, 3840]), Some("4K Ultra HD"));
        // A size in two groups shows as the first.
        assert_eq!(find([1280, 720]), Some("High Definition"));
        assert_eq!(find([1081, 1080]), None);
        assert_eq!(find([0, 0]), None);
    }

    #[test]
    fn physical_presets_match_the_pixels_they_make() {
        let presets = Presets::default();
        let find = |size, ppi| {
            let index = presets.find(Kind::Physical, size, ppi);
            name(&presets, Kind::Physical, index)
        };
        assert_eq!(find([2480, 3508], 300.0), Some("A4"));
        assert_eq!(find([3508, 2480], 300.0), Some("A4"));
        assert_eq!(find([1240, 1754], 150.0), Some("A4"));
        assert_eq!(find([2550, 3300], 300.0), Some("Letter"));
        assert_eq!(find([5100, 3300], 300.0), Some("Tabloid / Ledger"));
        // A pixel off, or the same pixels at another resolution, is another size.
        assert_eq!(find([2481, 3508], 300.0), None);
        assert_eq!(find([2480, 3508], 72.0), None);
        assert_eq!(find([2480, 3508], f32::NAN), None);
        // Pixel presets are not offered in print units, nor print sizes in pixels.
        assert_eq!(presets.find(Kind::Pixel, [2480, 3508], 300.0), None);
        assert_eq!(presets.find(Kind::Physical, [1920, 1080], 72.0), None);
        assert_eq!(Kind::of(Unit::Millimeters), Kind::Physical);
        assert_eq!(Kind::of(Unit::Inches), Kind::Physical);
        assert_eq!(Kind::of(Unit::Points), Kind::Physical);
        assert_eq!(Kind::of(Unit::Pixels), Kind::Pixel);
        assert_eq!(Kind::of(Unit::Percent), Kind::Pixel);
    }

    #[test]
    fn saving_renaming_moving_and_deleting() {
        let mut presets = Presets::default();
        // A new group goes last; a new preset of a group, at its end.
        let poster = Preset::pixels(" Poster ", "Mine", [1000, 1500]);
        assert_eq!(presets.insert(poster).unwrap(), 12);
        assert_eq!(presets.list(Kind::Pixel)[12].name, "Poster");
        let icon = Preset::pixels("Icon", "Screens", [512, 512]);
        assert_eq!(presets.insert(icon).unwrap(), 5);
        assert_eq!(
            presets.groups(Kind::Pixel),
            [("Screens", 0..6), ("Social", 6..13), ("Mine", 13..14)]
        );
        // The same name replaces the preset.
        let icon = Preset::pixels("Icon", "Screens", [256, 256]);
        assert_eq!(presets.insert(icon).unwrap(), 5);
        assert_eq!(presets.list(Kind::Pixel)[5].size, Size::Pixels([256, 256]));
        assert_eq!(presets.list(Kind::Pixel).len(), 14);
        // Renaming into another group moves the preset there.
        assert_eq!(
            presets.rename(Kind::Pixel, 5, "App icon", "Mine").unwrap(),
            13
        );
        assert_eq!(&pixel_names(&presets)[12..], ["Poster", "App icon"]);
        assert!(presets.rename(Kind::Pixel, 13, "Poster", "Mine").is_err());
        assert!(presets.rename(Kind::Pixel, 13, " ", "Mine").is_err());
        assert!(presets.rename(Kind::Pixel, 13, "Fine", "\t").is_err());
        assert!(presets.rename(Kind::Pixel, 99, "Fine", "Mine").is_err());
        // Renaming to its own name is fine.
        assert_eq!(
            presets.rename(Kind::Pixel, 13, "App icon", "Mine").unwrap(),
            13
        );
        // Moving stays within the group.
        assert_eq!(presets.move_within_group(Kind::Pixel, 13, true), Some(12));
        assert_eq!(&pixel_names(&presets)[12..], ["App icon", "Poster"]);
        assert_eq!(presets.move_within_group(Kind::Pixel, 12, true), None);
        assert_eq!(presets.move_within_group(Kind::Pixel, 13, false), None);
        assert_eq!(presets.move_within_group(Kind::Pixel, 0, true), None);
        assert_eq!(presets.move_within_group(Kind::Pixel, 99, false), None);
        assert_eq!(
            presets.move_within_group(Kind::Pixel, usize::MAX, false),
            None
        );
        assert_eq!(presets.move_within_group(Kind::Pixel, 0, false), Some(1));
        assert_eq!(pixel_names(&presets)[..2], ["4K Ultra HD", "8K Ultra HD"]);
        // Deleting.
        assert_eq!(presets.remove(Kind::Pixel, 12).unwrap().name, "App icon");
        assert_eq!(presets.remove(Kind::Pixel, 99), None);
        assert_eq!(presets.list(Kind::Pixel).len(), 13);
        // Saving a name again into another group moves it to that group's end.
        let poster = Preset::pixels("Poster", "Screens", [1000, 1500]);
        assert_eq!(presets.insert(poster).unwrap(), 5);
        assert_eq!(
            presets.groups(Kind::Pixel),
            [("Screens", 0..6), ("Social", 6..13)]
        );
        // Physical presets go in their own list.
        let card = Preset::physical("Card", "Mine", [85.0, 55.0], Unit::Millimeters, Some(300.0));
        assert_eq!(presets.insert(card).unwrap(), 16);
        assert_eq!(presets.list(Kind::Pixel).len(), 13);
    }

    #[test]
    fn invalid_presets_are_refused() {
        let mut presets = Presets::default();
        let inches = |name: &str, size: [f64; 2], resolution| {
            Preset::physical(name, "Mine", size, Unit::Inches, resolution)
        };
        let refused = [
            Preset::pixels("", "Mine", [10, 10]),
            Preset::pixels(&"x".repeat(MAX_NAME + 1), "Mine", [10, 10]),
            Preset::pixels("Two\nlines", "Mine", [10, 10]),
            Preset::pixels("Fine", &"g".repeat(MAX_NAME + 1), [10, 10]),
            Preset::pixels("Fine", "", [10, 10]),
            Preset::pixels("Zero", "Mine", [0, 10]),
            Preset::pixels("Wide", "Mine", [MAX_SIDE + 1, 1]),
            inches("NaN", [f64::NAN, 1.0], None),
            inches("Inf", [f64::INFINITY, 1.0], None),
            inches("Neg", [-1.0, 1.0], None),
            inches("Zero", [0.0, 1.0], None),
            inches("Huge", [1e300, 1.0], None),
            inches("Res", [1.0, 1.0], Some(0.0)),
            inches("Res", [1.0, 1.0], Some(f32::NAN)),
            inches("Res", [1.0, 1.0], Some(1e9)),
            // 100 in at 9600 ppi is wider than a canvas may be.
            inches("Big", [100.0, 1.0], Some(9600.0)),
            Preset::physical("Px", "Mine", [10.0, 10.0], Unit::Pixels, None),
            Preset::physical("Pc", "Mine", [10.0, 10.0], Unit::Percent, None),
        ];
        for preset in refused {
            assert!(presets.insert(preset.clone()).is_err(), "{preset:?}");
        }
        assert_eq!(presets, Presets::default());
        // A0 without a resolution is fine: its pixels are checked when it is picked.
        Preset::physical("A0", "ISO A", [841.0, 1189.0], Unit::Millimeters, None)
            .validate()
            .unwrap();
        // The list is bounded.
        let mut full = Presets::new(Vec::new(), Vec::new());
        for n in 0..MAX_PRESETS {
            (full.insert(Preset::pixels(&format!("P{n}"), "Mine", [10, 10]))).unwrap();
        }
        let more = Preset::pixels("One more", "Mine", [10, 10]);
        assert!(full.insert(more).is_err());
        // Replacing one still works when it is full.
        full.insert(Preset::pixels("P0", "Mine", [20, 20])).unwrap();
    }

    #[test]
    fn files_round_trip() {
        let mut presets = Presets::default();
        let quoted = Preset::pixels("Quote \" and \\ slash", "Ünïcode 组", [7, 9]);
        presets.insert(quoted).unwrap();
        let points = Preset::physical("Odd", "Mine", [72.27, 0.5], Unit::Points, None);
        presets.insert(points).unwrap();
        let card = Preset::physical("Card", "Mine", [8.5, 5.5], Unit::Centimeters, Some(299.5));
        presets.insert(card).unwrap();
        let text = presets.to_toml();
        assert!(text.starts_with("# Xuan canvas presets"));
        assert!(text.contains(
            "[[physical]]\ngroup = \"ISO A\"\nname = \"A4\"\nwidth = 210\nheight = 297\nunit = \"mm\"\nresolution = 300\n"
        ));
        let (read, problems) = Presets::from_toml(&text).unwrap();
        assert!(problems.is_empty(), "{problems:?}");
        assert_eq!(read, presets);
        // Empty lists stay empty rather than falling back to the built-in ones.
        let empty = Presets::new(Vec::new(), Vec::new());
        let (read, _) = Presets::from_toml(&empty.to_toml()).unwrap();
        assert_eq!(read, empty);
    }

    #[test]
    fn a_list_left_out_is_the_built_in_one() {
        let text = "[[physical]]\nname = \"Card\"\nwidth = 85\nheight = 55\nunit = \"mm\"\n";
        let (read, problems) = Presets::from_toml(text).unwrap();
        assert!(problems.is_empty());
        assert_eq!(read.list(Kind::Pixel), Presets::default().list(Kind::Pixel));
        assert_eq!(read.list(Kind::Physical).len(), 1);
        let card = &read.list(Kind::Physical)[0];
        assert_eq!(card.group, DEFAULT_GROUP);
        assert_eq!(
            card.size,
            Size::Physical {
                width: 85.0,
                height: 55.0,
                unit: Unit::Millimeters,
                resolution: None
            }
        );
        // Units are read like typed ones.
        let text =
            "pixel = []\n[[physical]]\nname = \"x\"\nwidth = 1\nheight = 2\nunit = \"Inches\"\n";
        let (read, _) = Presets::from_toml(text).unwrap();
        assert!(read.list(Kind::Pixel).is_empty());
        assert!(matches!(
            read.list(Kind::Physical)[0].size,
            Size::Physical {
                unit: Unit::Inches,
                ..
            }
        ));
    }

    #[test]
    fn hostile_entries_are_skipped_with_a_note() {
        let long = "n".repeat(MAX_NAME + 1);
        let text = format!(
            r#"
version = 1
[[pixel]]
name = "Good"
width = 100
height = 50
[[pixel]]
name = "Negative"
width = -100
height = 50
[[pixel]]
name = "Fraction"
width = 100.5
height = 50
[[pixel]]
name = "Text"
width = "wide"
height = 50
[[pixel]]
name = "{long}"
width = 10
height = 10
[[pixel]]
width = 10
height = 10
[[pixel]]
name = "Good"
width = 1
height = 1
[[pixel]]
name = "Too big"
width = 65536
height = 1
[[pixel]]
name = "Not pixels"
width = 1
height = 1
unit = "mm"
[[pixel]]
name = 5
width = 1
height = 1
[[physical]]
name = "Unknown unit"
width = 1
height = 1
unit = "furlong"
[[physical]]
name = "No unit"
width = 1
height = 1
[[physical]]
name = "Nan"
width = nan
height = 1
unit = "in"
[[physical]]
name = "Inf"
width = inf
height = 1
unit = "in"
[[physical]]
name = "Zero resolution"
width = 1
height = 1
unit = "in"
resolution = 0
[[physical]]
name = "Bad resolution"
width = 1
height = 1
unit = "in"
resolution = "high"
[[physical]]
name = "Group"
group = 3
width = 1
height = 1
unit = "in"
[[physical]]
name = "Fine"
width = 2
height = 3
unit = "in"
"#
        );
        let (read, problems) = Presets::from_toml(&text).unwrap();
        assert_eq!(pixel_names(&read), ["Good"]);
        assert_eq!(read.list(Kind::Physical).len(), 1);
        assert_eq!(read.list(Kind::Physical)[0].name, "Fine");
        assert_eq!(problems.len(), 16, "{problems:#?}");
        assert!(
            problems[0].starts_with("pixel preset 2:"),
            "{}",
            problems[0]
        );
        assert!(
            problems
                .iter()
                .any(|p| p.contains("another preset is named"))
        );
        assert!(problems.iter().any(|p| p.contains("Unknown unit")));
        // Lists that are not lists keep the built-in ones.
        let (read, problems) = Presets::from_toml("pixel = 3\nphysical = \"x\"\n").unwrap();
        assert_eq!(read, Presets::default());
        assert_eq!(problems.len(), 2);
        let (read, problems) = Presets::from_toml("pixel = [1, \"x\", []]").unwrap();
        assert!(read.list(Kind::Pixel).is_empty());
        assert_eq!(problems.len(), 3);
    }

    #[test]
    fn counts_and_notes_are_bounded() {
        let mut text = String::new();
        for n in 0..MAX_PRESETS + 50 {
            text.push_str(&format!(
                "[[pixel]]\nname = \"P{n}\"\nwidth = 10\nheight = 10\n"
            ));
        }
        for _ in 0..100 {
            text.push_str("[[physical]]\nname = \"\"\nwidth = 1\nheight = 1\nunit = \"in\"\n");
        }
        let (read, problems) = Presets::from_toml(&text).unwrap();
        assert_eq!(read.list(Kind::Pixel).len(), MAX_PRESETS);
        assert!(read.list(Kind::Physical).is_empty());
        assert_eq!(problems.len(), MAX_PROBLEMS);
        assert!(problems[0].contains("only the first"));
    }

    #[test]
    fn unreadable_files_fail() {
        for bad in [
            "not = [toml",
            "version = 2",
            "version = \"1\"",
            "version = 0",
        ] {
            assert!(Presets::from_toml(bad).is_err(), "{bad}");
        }
        assert!(Presets::from_toml("version = 1").is_ok());
    }

    #[test]
    fn the_file_is_written_only_when_asked_and_never_over_a_broken_one() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("xuan").join(FILE);
        // No file: the built-in lists.
        assert_eq!(Presets::read(&path).unwrap(), None);
        let mut presets = Presets::default();
        (presets.insert(Preset::pixels("Poster", "Mine", [1000, 1500]))).unwrap();
        presets.write(&path).unwrap();
        let (read, problems) = Presets::read(&path).unwrap().unwrap();
        assert!(problems.is_empty());
        assert_eq!(read, presets);
        // Hand edits, comments included, are read as they are.
        let edited = "# Mine\n[[pixel]]\nname = \"Hand\" # typed\nwidth = 5\nheight = 6\n";
        fs::write(&path, edited).unwrap();
        let (read, _) = Presets::read(&path).unwrap().unwrap();
        assert_eq!(pixel_names(&read), ["Hand"]);
        assert_eq!(fs::read_to_string(&path).unwrap(), edited);
        // A broken file is reported and left untouched.
        fs::write(&path, "pixel = [").unwrap();
        let error = Presets::read(&path).unwrap_err();
        assert!(format!("{error:#}").contains("Cannot read"));
        assert!(presets.write(&path).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "pixel = [");
        // As is one too large to read.
        let large = format!("# {}\n", "x".repeat(MAX_FILE_BYTES as usize));
        fs::write(&path, &large).unwrap();
        assert!(Presets::read(&path).is_err());
        assert!(presets.write(&path).is_err());
        // Restoring the defaults deletes it, and works without one.
        restore_defaults(&path).unwrap();
        assert!(!path.exists());
        restore_defaults(&path).unwrap();
        // A folder in its place cannot be read.
        fs::create_dir_all(&path).unwrap();
        assert!(Presets::read(&path).is_err());
    }

    #[test]
    fn swap_turns_portrait_into_landscape() {
        assert_eq!(swapped([1080, 1920]), [1920, 1080]);
        assert_eq!(swapped(swapped([3, 7])), [3, 7]);
        assert_eq!(swapped([5, 5]), [5, 5]);
    }

    #[test]
    fn linked_side_rounds_to_the_nearest_pixel() {
        // 16:9
        assert_eq!(linked_side(1920, 1080, 960, 0), 540);
        assert_eq!(linked_side(1920, 1080, 1000, 0), 563); // 562.5 rounds up
        assert_eq!(linked_side(1920, 1080, 1001, 0), 563); // 563.06
        assert_eq!(linked_side(1080, 1920, 1, 0), 2); // 1.78
        // Heights scale to widths the same way.
        assert_eq!(linked_side(1080, 1920, 540, 0), 960);
    }

    #[test]
    fn linked_side_respects_limits_and_zero() {
        assert_eq!(linked_side(1000, 1000, 0, 7), 1);
        assert_eq!(linked_side(1000, 10, 1, 7), 1); // 0.01 stays a pixel
        assert_eq!(linked_side(1, 100, MAX_SIDE, 7), MAX_SIDE);
        assert_eq!(linked_side(1, u32::MAX, u32::MAX, 7), MAX_SIDE);
        assert_eq!(linked_side(0, 100, 50, 33), 33);
        assert_eq!(linked_side(100, 0, 50, 33), 33);
    }

    #[test]
    fn remembered_size_must_be_usable() {
        assert_eq!(usable(Some([800, 600])), Some([800, 600]));
        assert_eq!(usable(Some([0, 600])), None);
        assert_eq!(usable(Some([MAX_SIDE + 1, 1])), None);
        assert_eq!(usable(None), None);
    }
}
