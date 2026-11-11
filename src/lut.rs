//! Colour lookup tables from `.cube` files, for Color Lookup adjustment layers.
//!
//! The reader follows Adobe's Cube LUT Specification 1.0: an optional `TITLE`, either
//! `LUT_1D_SIZE` (2–65,536 entries) or `LUT_3D_SIZE` (2–65 points a side), optional
//! `DOMAIN_MIN` and `DOMAIN_MAX`, then one line of three numbers per entry, red changing
//! fastest. DaVinci Resolve's `LUT_1D_INPUT_RANGE` and `LUT_3D_INPUT_RANGE` set the domain of
//! all three channels. Lines starting with `#` are comments; other keywords are ignored. A file
//! with both a 1D and a 3D table (Resolve's shaper LUTs) is refused, as is anything malformed,
//! truncated or too large: a `.cube` file is untrusted input.
//!
//! Tables apply to the pixel's stored (sRGB-encoded) values, as Photoshop's Color Lookup does.
//! [`Lut::apply`] is the reference; `color_lookup` in `composite.wgsl` mirrors it on the GPU.

use std::{fmt, fs::File, io::Read, path::Path};

use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};

/// Most points along each side of a 3D table.
pub const MAX_3D_SIZE: u32 = 65;
/// Most entries in a 1D table.
pub const MAX_1D_SIZE: u32 = 65_536;
/// Largest `.cube` file read: a 65³ table at full precision is about 12 MiB.
pub const MAX_FILE_BYTES: u64 = 32 * 1024 * 1024;
/// Longest `TITLE` kept, in characters.
const MAX_TITLE: usize = 256;
/// Largest magnitude of a domain bound or table value. Values this large are surely a broken
/// file, and keeping them finite keeps every interpolation finite.
const MAX_VALUE: f32 = 1.0e6;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Interpolation {
    /// Splits each cell of a 3D table into six tetrahedra and blends four of its corners:
    /// smoother along the neutral axis than trilinear, and what Resolve uses.
    #[default]
    Tetrahedral,
    /// Blends all eight corners of the cell.
    Trilinear,
}

impl Interpolation {
    pub const ALL: [Self; 2] = [Self::Tetrahedral, Self::Trilinear];

    pub fn name(self) -> &'static str {
        match self {
            Self::Tetrahedral => "Tetrahedral",
            Self::Trilinear => "Trilinear",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dimension {
    /// A curve per channel: `table[i]` holds the red, green and blue outputs for input
    /// `i / (size - 1)` of each channel.
    One,
    /// A cube: `table[r + g * size + b * size²]` is the colour for the lattice point (r, g, b).
    Three,
}

/// A parsed, validated colour lookup table.
#[derive(Clone, PartialEq)]
pub struct Lut {
    /// The file's `TITLE`, or empty.
    pub title: String,
    pub dimension: Dimension,
    /// Entries of a 1D table, or points along each side of a 3D one.
    pub size: u32,
    /// The input values mapped to the first and last entry of each channel.
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    pub table: Vec<[f32; 3]>,
}

impl Default for Lut {
    /// The smallest identity table, which leaves every colour as it is.
    fn default() -> Self {
        Self::identity(Dimension::Three, 2)
    }
}

impl fmt::Debug for Lut {
    // The table can hold hundreds of thousands of entries; they are left out.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Lut")
            .field("title", &self.title)
            .field("dimension", &self.dimension)
            .field("size", &self.size)
            .field("domain_min", &self.domain_min)
            .field("domain_max", &self.domain_max)
            .finish_non_exhaustive()
    }
}

impl Lut {
    /// A table that maps every colour to itself.
    pub fn identity(dimension: Dimension, size: u32) -> Self {
        let step = |i: u32| i as f32 / (size - 1) as f32;
        let table = match dimension {
            Dimension::One => (0..size).map(|i| [step(i); 3]).collect(),
            Dimension::Three => (0..size.pow(3))
                .map(|i| [step(i % size), step(i / size % size), step(i / size / size)])
                .collect(),
        };
        Self {
            title: String::new(),
            dimension,
            size,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
            table,
        }
    }

    /// Read a `.cube` file, refusing one larger than [`MAX_FILE_BYTES`].
    pub fn load(path: &Path) -> Result<Self> {
        let metadata =
            std::fs::metadata(path).with_context(|| format!("Cannot read {}", path.display()))?;
        // Opening a FIFO or device could block forever.
        ensure!(
            metadata.is_file(),
            "{} is not a regular file",
            path.display()
        );
        let too_large = || {
            format!(
                "Colour lookup tables are limited to {}",
                crate::limits::size(MAX_FILE_BYTES)
            )
        };
        ensure!(metadata.len() <= MAX_FILE_BYTES, too_large());
        let mut bytes = Vec::new();
        File::open(path)
            .with_context(|| format!("Cannot read {}", path.display()))?
            .take(MAX_FILE_BYTES + 1)
            .read_to_end(&mut bytes)?;
        ensure!(bytes.len() as u64 <= MAX_FILE_BYTES, too_large());
        Self::parse_bytes(&bytes)
    }

    /// Parse a `.cube` file's bytes. Text that is not UTF-8 is read as far as the keywords
    /// and numbers go, which are ASCII; only a title can lose characters.
    pub fn parse_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(
            bytes.len() as u64 <= MAX_FILE_BYTES,
            "Colour lookup tables are limited to {}",
            crate::limits::size(MAX_FILE_BYTES)
        );
        Self::parse(&String::from_utf8_lossy(bytes))
    }

    /// Parse the text of a `.cube` file.
    pub fn parse(text: &str) -> Result<Self> {
        let text = text.strip_prefix('\u{feff}').unwrap_or(text);
        let mut title = None;
        let mut size_1d = None;
        let mut size_3d = None;
        let mut domain_min = None;
        let mut domain_max = None;
        let mut range = None;
        let mut table: Vec<[f32; 3]> = Vec::new();
        let mut expected = None;
        for (index, line) in text.lines().enumerate() {
            let number = index + 1;
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let mut words = line.split_ascii_whitespace();
            let keyword = words.next().unwrap_or_default();
            if keyword.starts_with(|c: char| c.is_ascii_alphabetic()) {
                ensure!(
                    table.is_empty(),
                    "Line {number}: `{}` comes after the table data",
                    truncated(keyword)
                );
                match keyword {
                    "TITLE" => {
                        ensure!(title.is_none(), "Line {number}: a second TITLE");
                        title = Some(parse_title(line["TITLE".len()..].trim()));
                    }
                    "LUT_1D_SIZE" | "LUT_3D_SIZE" => {
                        let three = keyword == "LUT_3D_SIZE";
                        let [size] = numbers::<u32, 1>(words, number)?;
                        let limit = if three { MAX_3D_SIZE } else { MAX_1D_SIZE };
                        ensure!(
                            (2..=limit).contains(&size),
                            "Line {number}: {keyword} must be between 2 and {limit}, not {size}"
                        );
                        let slot = if three { &mut size_3d } else { &mut size_1d };
                        ensure!(slot.is_none(), "Line {number}: a second {keyword}");
                        *slot = Some(size);
                    }
                    "DOMAIN_MIN" | "DOMAIN_MAX" => {
                        let values = numbers::<f32, 3>(words, number)?;
                        check_values(&values, number)?;
                        let slot = if keyword == "DOMAIN_MIN" {
                            &mut domain_min
                        } else {
                            &mut domain_max
                        };
                        ensure!(slot.is_none(), "Line {number}: a second {keyword}");
                        *slot = Some(values);
                    }
                    "LUT_1D_INPUT_RANGE" | "LUT_3D_INPUT_RANGE" => {
                        let values = numbers::<f32, 2>(words, number)?;
                        check_values(&values, number)?;
                        ensure!(range.is_none(), "Line {number}: a second input range");
                        range = Some(values);
                    }
                    // Keywords this reader has no use for, such as `LUT_IN_VIDEO_RANGE`.
                    _ => {}
                }
                continue;
            }
            let expected = match expected {
                Some(expected) => expected,
                None => {
                    let count = match (size_1d, size_3d) {
                        (Some(_), Some(_)) => {
                            bail!("Tables with both a 1D and a 3D part are not supported")
                        }
                        (Some(size), None) => size as usize,
                        (None, Some(size)) => (size as usize).pow(3),
                        (None, None) => {
                            bail!("Line {number}: table data before LUT_1D_SIZE or LUT_3D_SIZE")
                        }
                    };
                    table.reserve_exact(count);
                    *expected.insert(count)
                }
            };
            ensure!(
                table.len() < expected,
                "Line {number}: more than the {expected} entries the size gives"
            );
            let values = numbers::<f32, 3>(line.split_ascii_whitespace(), number)?;
            check_values(&values, number)?;
            table.push(values);
        }
        let (dimension, size) = match (size_1d, size_3d) {
            (Some(_), Some(_)) => bail!("Tables with both a 1D and a 3D part are not supported"),
            (Some(size), None) => (Dimension::One, size),
            (None, Some(size)) => (Dimension::Three, size),
            (None, None) => bail!("Not a colour lookup table: no LUT_1D_SIZE or LUT_3D_SIZE"),
        };
        let expected = match dimension {
            Dimension::One => size as usize,
            Dimension::Three => (size as usize).pow(3),
        };
        ensure!(
            table.len() == expected,
            "The table is incomplete: {} of {expected} entries",
            table.len()
        );
        ensure!(
            range.is_none() || (domain_min.is_none() && domain_max.is_none()),
            "Give either DOMAIN_MIN and DOMAIN_MAX or an input range, not both"
        );
        let (domain_min, domain_max) = match range {
            Some([low, high]) => ([low; 3], [high; 3]),
            None => (
                domain_min.unwrap_or([0.0; 3]),
                domain_max.unwrap_or([1.0; 3]),
            ),
        };
        let lut = Self {
            title: title.unwrap_or_default(),
            dimension,
            size,
            domain_min,
            domain_max,
            table,
        };
        lut.validate()?;
        Ok(lut)
    }

    /// Check a table built in memory as [`Lut::parse`] checks a file.
    pub fn validate(&self) -> Result<()> {
        let (limit, entries) = match self.dimension {
            Dimension::One => (MAX_1D_SIZE, self.size as usize),
            Dimension::Three => (MAX_3D_SIZE, (self.size as usize).pow(3)),
        };
        ensure!(
            (2..=limit).contains(&self.size),
            "A colour lookup table's size must be between 2 and {limit}, not {}",
            self.size
        );
        ensure!(
            self.table.len() == entries,
            "The table is incomplete: {} of {entries} entries",
            self.table.len()
        );
        ensure!(
            self.title.chars().count() <= MAX_TITLE && !self.title.contains(['"', '\n', '\r']),
            "Invalid colour lookup table title"
        );
        for channel in 0..3 {
            let (low, high) = (self.domain_min[channel], self.domain_max[channel]);
            ensure!(
                low.abs() <= MAX_VALUE && high.abs() <= MAX_VALUE && low < high,
                "DOMAIN_MIN must be below DOMAIN_MAX in every channel"
            );
        }
        ensure!(
            self.table
                .iter()
                .flatten()
                .all(|v| v.is_finite() && v.abs() <= MAX_VALUE),
            "Colour lookup table values must be numbers within ±{MAX_VALUE}"
        );
        Ok(())
    }

    /// Bytes the table takes in memory.
    pub fn bytes(&self) -> usize {
        self.table.len() * std::mem::size_of::<[f32; 3]>()
    }

    /// The table as `.cube` text, which [`Lut::parse`] reads back exactly: Rust writes the
    /// shortest decimal that rounds to the same `f32`.
    pub fn to_cube(&self) -> String {
        use std::fmt::Write;
        let mut text = String::with_capacity(self.table.len() * 30 + 128);
        if !self.title.is_empty() {
            let _ = writeln!(text, "TITLE \"{}\"", self.title);
        }
        let keyword = match self.dimension {
            Dimension::One => "LUT_1D_SIZE",
            Dimension::Three => "LUT_3D_SIZE",
        };
        let _ = writeln!(text, "{keyword} {}", self.size);
        let [r, g, b] = self.domain_min;
        let _ = writeln!(text, "DOMAIN_MIN {r} {g} {b}");
        let [r, g, b] = self.domain_max;
        let _ = writeln!(text, "DOMAIN_MAX {r} {g} {b}");
        for [r, g, b] in &self.table {
            let _ = writeln!(text, "{r} {g} {b}");
        }
        text
    }

    /// Map a colour through the table. Inputs outside the domain take the nearest edge;
    /// the result is not clamped.
    pub fn apply(&self, rgb: [f32; 3], interpolation: Interpolation) -> [f32; 3] {
        let last = (self.size - 1) as f32;
        let position: [f32; 3] = std::array::from_fn(|c| {
            let (low, high) = (self.domain_min[c], self.domain_max[c]);
            let unit = (rgb[c] - low) / (high - low);
            // NaN (from a NaN input) goes to the first entry.
            if unit >= 0.0 {
                unit.min(1.0) * last
            } else {
                0.0
            }
        });
        // The lower lattice point of each axis and how far past it the colour lies; the last
        // cell takes the top edge, so `base + 1` stays inside the table.
        let base = position.map(|p| (p.floor() as u32).min(self.size - 2));
        let fraction: [f32; 3] = std::array::from_fn(|c| position[c] - base[c] as f32);
        match self.dimension {
            Dimension::One => std::array::from_fn(|c| {
                let low = self.table[base[c] as usize][c];
                let high = self.table[base[c] as usize + 1][c];
                low + (high - low) * fraction[c]
            }),
            Dimension::Three => {
                let n = self.size as usize;
                let origin = base[0] as usize + base[1] as usize * n + base[2] as usize * n * n;
                let corner =
                    |r: usize, g: usize, b: usize| self.table[origin + r + g * n + b * n * n];
                let [fr, fg, fb] = fraction;
                match interpolation {
                    Interpolation::Trilinear => {
                        let lerp = |a: [f32; 3], b: [f32; 3], t: f32| -> [f32; 3] {
                            std::array::from_fn(|c| a[c] + (b[c] - a[c]) * t)
                        };
                        let c00 = lerp(corner(0, 0, 0), corner(1, 0, 0), fr);
                        let c10 = lerp(corner(0, 1, 0), corner(1, 1, 0), fr);
                        let c01 = lerp(corner(0, 0, 1), corner(1, 0, 1), fr);
                        let c11 = lerp(corner(0, 1, 1), corner(1, 1, 1), fr);
                        lerp(lerp(c00, c10, fg), lerp(c01, c11, fg), fb)
                    }
                    Interpolation::Tetrahedral => {
                        // The tetrahedron holding the colour runs from the cell's origin to its
                        // far corner through the two corners that order the fractions.
                        let (first, second, [t0, t1, t2]) = if fr > fg {
                            if fg > fb {
                                (corner(1, 0, 0), corner(1, 1, 0), [fr, fg, fb])
                            } else if fr > fb {
                                (corner(1, 0, 0), corner(1, 0, 1), [fr, fb, fg])
                            } else {
                                (corner(0, 0, 1), corner(1, 0, 1), [fb, fr, fg])
                            }
                        } else if fb > fg {
                            (corner(0, 0, 1), corner(0, 1, 1), [fb, fg, fr])
                        } else if fb > fr {
                            (corner(0, 1, 0), corner(0, 1, 1), [fg, fb, fr])
                        } else {
                            (corner(0, 1, 0), corner(1, 1, 0), [fg, fr, fb])
                        };
                        let (start, end) = (corner(0, 0, 0), corner(1, 1, 1));
                        std::array::from_fn(|c| {
                            start[c] * (1.0 - t0)
                                + first[c] * (t0 - t1)
                                + second[c] * (t1 - t2)
                                + end[c] * t2
                        })
                    }
                }
            }
        }
    }
}

/// A title in quotes, or the rest of the line without them; at most [`MAX_TITLE`] characters
/// and on one line, so [`Lut::to_cube`] can write it back.
fn parse_title(rest: &str) -> String {
    let inner = rest
        .strip_prefix('"')
        .map(|r| r.rsplit_once('"').map_or(r, |(inner, _)| inner))
        .unwrap_or(rest);
    inner
        .chars()
        .filter(|c| *c != '"' && !c.is_control())
        .take(MAX_TITLE)
        .collect::<String>()
        .trim()
        .to_owned()
}

/// Exactly `N` numbers from the rest of a line.
fn numbers<'a, T: std::str::FromStr, const N: usize>(
    mut words: impl Iterator<Item = &'a str>,
    line: usize,
) -> Result<[T; N]> {
    let mut values = Vec::with_capacity(N);
    for word in words.by_ref().take(N) {
        let value = word
            .parse::<T>()
            .ok()
            .with_context(|| format!("Line {line}: `{}` is not a number", truncated(word)))?;
        values.push(value);
    }
    ensure!(
        values.len() == N && words.next().is_none(),
        "Line {line}: expected {N} number{}",
        if N == 1 { "" } else { "s" }
    );
    Ok(values
        .try_into()
        .unwrap_or_else(|_| unreachable!("checked length")))
}

fn check_values(values: &[f32], line: usize) -> Result<()> {
    ensure!(
        values.iter().all(|v| v.is_finite() && v.abs() <= MAX_VALUE),
        "Line {line}: values must be numbers within ±{MAX_VALUE}"
    );
    Ok(())
}

/// A word from the file, cut short for an error message.
fn truncated(word: &str) -> String {
    let mut short: String = word.chars().take(32).collect();
    if short.len() < word.len() {
        short.push('…');
    }
    short
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(actual: [f32; 3], expected: [f32; 3]) {
        for c in 0..3 {
            assert!(
                (actual[c] - expected[c]).abs() < 1e-5,
                "{actual:?} != {expected:?}"
            );
        }
    }

    /// A 2³ table whose only non-black corner is white: trilinear gives r·g·b, tetrahedral the
    /// smallest of the three.
    fn corner_cube() -> Lut {
        let mut lines = String::from("LUT_3D_SIZE 2\n");
        for i in 0..8 {
            lines += if i == 7 { "1 1 1\n" } else { "0 0 0\n" };
        }
        Lut::parse(&lines).unwrap()
    }

    #[test]
    fn parses_a_3d_table_with_title_comments_and_domain() {
        let text = "\u{feff}# Made by hand\r\nTITLE \"Warm \"\"Look\"\r\n\r\nLUT_3D_SIZE 2\r\n\
                    DOMAIN_MIN 0 0 0\r\nDOMAIN_MAX 1 2 4\r\nLUT_IN_VIDEO_RANGE\r\n\
                    0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n# trailing\n";
        let lut = Lut::parse(text).unwrap();
        assert_eq!(lut.title, "Warm Look");
        assert_eq!(lut.dimension, Dimension::Three);
        assert_eq!(lut.size, 2);
        assert_eq!(lut.domain_max, [1.0, 2.0, 4.0]);
        assert_eq!(lut.table[1], [1.0, 0.0, 0.0]);
        // The domain scales green by a half and blue by a quarter.
        close(
            lut.apply([0.5, 1.0, 2.0], Interpolation::Trilinear),
            [0.5, 0.5, 0.5],
        );
    }

    #[test]
    fn parses_1d_tables_and_resolve_input_ranges() {
        let lut = Lut::parse("LUT_1D_SIZE 3\nLUT_1D_INPUT_RANGE 0 2\n0 0 1\n0.25 0.5 0.5\n1 1 0\n")
            .unwrap();
        assert_eq!(lut.dimension, Dimension::One);
        assert_eq!(lut.domain_min, [0.0; 3]);
        assert_eq!(lut.domain_max, [2.0; 3]);
        // Input 0.5 is a quarter of the way along, halfway between the first two entries.
        close(
            lut.apply([0.5, 0.5, 0.5], Interpolation::Tetrahedral),
            [0.125, 0.25, 0.75],
        );
        // Each channel follows its own curve.
        close(
            lut.apply([2.0, 0.0, 1.0], Interpolation::Trilinear),
            [1.0, 0.0, 0.5],
        );
    }

    #[test]
    fn identity_tables_leave_colours_unchanged() {
        for (dimension, size) in [
            (Dimension::Three, 33),
            (Dimension::Three, 2),
            (Dimension::One, 1024),
        ] {
            let lut = Lut::identity(dimension, size);
            for rgb in [
                [0.0, 0.0, 0.0],
                [1.0, 1.0, 1.0],
                [0.1, 0.52, 0.93],
                [0.77, 0.3, 0.3],
            ] {
                for interpolation in Interpolation::ALL {
                    close(lut.apply(rgb, interpolation), rgb);
                }
            }
        }
    }

    #[test]
    fn interpolation_matches_known_values() {
        let lut = corner_cube();
        let rgb = [0.25, 0.5, 0.75];
        close(lut.apply(rgb, Interpolation::Trilinear), [0.09375; 3]);
        close(lut.apply(rgb, Interpolation::Tetrahedral), [0.25; 3]);
        // Both are exact on the lattice. Between, along the neutral axis, trilinear sags to
        // 0.5³ where tetrahedral stays on the diagonal.
        for interpolation in Interpolation::ALL {
            close(lut.apply([1.0, 1.0, 1.0], interpolation), [1.0; 3]);
            close(lut.apply([1.0, 0.0, 1.0], interpolation), [0.0; 3]);
        }
        close(lut.apply([0.5; 3], Interpolation::Trilinear), [0.125; 3]);
        close(lut.apply([0.5; 3], Interpolation::Tetrahedral), [0.5; 3]);

        // Every ordering of the fractions picks its own tetrahedron; a linear table is
        // reproduced exactly by both.
        let n = 5_u32;
        let mut swap = Lut::identity(Dimension::Three, n);
        for entry in &mut swap.table {
            *entry = [entry[1] * 0.5 + 0.1, entry[2], 1.0 - entry[0]];
        }
        for rgb in [
            [0.3, 0.2, 0.1],
            [0.3, 0.1, 0.2],
            [0.2, 0.3, 0.1],
            [0.1, 0.3, 0.2],
            [0.2, 0.1, 0.3],
            [0.1, 0.2, 0.3],
            [0.6, 0.6, 0.6],
        ] {
            for interpolation in Interpolation::ALL {
                close(
                    swap.apply(rgb, interpolation),
                    [rgb[1] * 0.5 + 0.1, rgb[2], 1.0 - rgb[0]],
                );
            }
        }
    }

    #[test]
    fn inputs_outside_the_domain_take_the_edge() {
        let lut = corner_cube();
        for interpolation in Interpolation::ALL {
            close(lut.apply([2.0, 5.0, 1.5], interpolation), [1.0; 3]);
            close(lut.apply([-1.0, 1.0, 1.0], interpolation), [0.0; 3]);
            close(lut.apply([f32::NAN, 1.0, 1.0], interpolation), [0.0; 3]);
        }
    }

    #[test]
    fn writes_cube_text_that_reads_back_exactly() {
        let mut lut = Lut::identity(Dimension::Three, 17);
        lut.title = "Teal & Orange 色".into();
        lut.domain_min = [-0.125, 0.0, 0.1];
        lut.domain_max = [1.5, 1.0, 0.9];
        for (i, entry) in lut.table.iter_mut().enumerate() {
            entry[0] = (i as f32 * 0.123_456_79).sin();
        }
        assert_eq!(Lut::parse(&lut.to_cube()).unwrap(), lut);
        let curve = Lut::identity(Dimension::One, 4096);
        assert_eq!(Lut::parse(&curve.to_cube()).unwrap(), curve);
    }

    #[test]
    fn malformed_and_hostile_files_are_refused_with_a_reason() {
        let cube = |data: &str| format!("LUT_3D_SIZE 2\n{data}");
        let eight = "0 0 0\n".repeat(8);
        let cases = [
            ("", "no LUT_1D_SIZE or LUT_3D_SIZE"),
            ("TITLE \"x\"\n", "no LUT_1D_SIZE or LUT_3D_SIZE"),
            ("0 0 0\n", "before LUT_1D_SIZE"),
            (&cube(&"0 0 0\n".repeat(7)), "incomplete: 7 of 8"),
            (&cube(&"0 0 0\n".repeat(9)), "more than the 8 entries"),
            (
                &cube(&format!("{}0 0\n", "0 0 0\n".repeat(7))),
                "expected 3 numbers",
            ),
            (
                &cube(&format!("{}0 0 0 0\n", "0 0 0\n".repeat(7))),
                "expected 3 numbers",
            ),
            (
                &cube(&format!("{}0 x 0\n", "0 0 0\n".repeat(7))),
                "`x` is not a number",
            ),
            (
                &cube(&format!("{}0 NaN 0\n", "0 0 0\n".repeat(7))),
                "values must be numbers",
            ),
            (
                &cube(&format!("{}0 inf 0\n", "0 0 0\n".repeat(7))),
                "values must be numbers",
            ),
            (
                &cube(&format!("{}0 1e30 0\n", "0 0 0\n".repeat(7))),
                "values must be numbers",
            ),
            (
                &cube(&format!("0 0 0\nDOMAIN_MIN 0 0 0\n{}", "0 0 0\n".repeat(7))),
                "after the table data",
            ),
            ("LUT_3D_SIZE 1\n0 0 0\n", "between 2 and 65, not 1"),
            ("LUT_3D_SIZE 66\n", "between 2 and 65, not 66"),
            ("LUT_3D_SIZE 4294967296\n", "is not a number"),
            ("LUT_3D_SIZE -3\n", "is not a number"),
            ("LUT_3D_SIZE\n", "expected 1 number"),
            ("LUT_1D_SIZE 65537\n", "between 2 and 65536"),
            ("LUT_3D_SIZE 2\nLUT_3D_SIZE 2\n", "a second LUT_3D_SIZE"),
            ("TITLE \"a\"\nTITLE \"b\"\n", "a second TITLE"),
            (
                &format!("LUT_1D_SIZE 2\nLUT_3D_SIZE 2\n{eight}"),
                "both a 1D and a 3D",
            ),
            ("LUT_1D_SIZE 2\nLUT_3D_SIZE 2\n", "both a 1D and a 3D"),
            (
                &cube(&format!("DOMAIN_MIN 0 1 0\nDOMAIN_MAX 1 1 1\n{eight}")),
                "below DOMAIN_MAX",
            ),
            (
                &cube(&format!("DOMAIN_MAX 1 1\n{eight}")),
                "expected 3 numbers",
            ),
            (
                &cube(&format!(
                    "DOMAIN_MIN 0 0 0\nLUT_3D_INPUT_RANGE 0 1\n{eight}"
                )),
                "not both",
            ),
        ];
        for (text, reason) in cases {
            let error = Lut::parse(text).expect_err(text).to_string();
            assert!(error.contains(reason), "{text:?}: {error}");
        }
        // Binary junk is not a table either, and long words are cut short in the message.
        let error = Lut::parse_bytes(&[0xff, 0xfe, 0, 1, 2, b'\n', b'1']).unwrap_err();
        assert!(error.to_string().contains("before LUT_1D_SIZE"), "{error}");
        let error = Lut::parse(&format!("LUT_3D_SIZE 2\n{} 0 0\n", "9".repeat(10_000)))
            .unwrap_err()
            .to_string();
        assert!(error.len() < 200, "{error}");
    }

    #[test]
    fn size_limits_hold_for_files_and_tables() {
        // The largest table allowed parses; its file fits the file limit with room to spare.
        let largest = Lut::identity(Dimension::Three, MAX_3D_SIZE);
        let text = largest.to_cube();
        assert!((text.len() as u64) < MAX_FILE_BYTES / 2);
        assert_eq!(Lut::parse(&text).unwrap().table.len(), 65 * 65 * 65);

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("huge.cube");
        let file = File::create(&path).unwrap();
        file.set_len(MAX_FILE_BYTES + 1).unwrap();
        let error = Lut::load(&path).unwrap_err().to_string();
        assert!(error.contains("limited to"), "{error}");
        assert!(Lut::parse_bytes(&vec![b'\n'; MAX_FILE_BYTES as usize + 1]).is_err());
        assert!(Lut::load(dir.path()).is_err());
        assert!(Lut::load(&dir.path().join("missing.cube")).is_err());

        let mut invalid = Lut::identity(Dimension::Three, 3);
        invalid.table.pop();
        assert!(invalid.validate().is_err());
        let mut invalid = Lut::identity(Dimension::One, 3);
        invalid.table[1][2] = f32::INFINITY;
        assert!(invalid.validate().is_err());
        let mut invalid = Lut::identity(Dimension::One, 3);
        invalid.title = "a\"b".into();
        assert!(invalid.validate().is_err());
    }

    #[test]
    fn titles_are_kept_on_one_line_and_cut_short() {
        assert_eq!(parse_title("\"Film \"Look\" v2\" extra"), "Film Look v2");
        assert_eq!(parse_title("No quotes"), "No quotes");
        assert_eq!(parse_title(&"x".repeat(1000)).len(), MAX_TITLE);
        assert_eq!(parse_title("\"tab\there\""), "tabhere");
    }
}
