//! Non-printing layout aids: user guides and the layout grid.
//!
//! Follows Compositor's `Document/Guides.swift`: guides are stored in the document (and in `.xuan`
//! projects from format version 5), while the grid's spacing, subdivisions and appearance are an
//! app preference that a project may override. Neither is ever rendered into the image.
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Which way a guide runs. A horizontal guide sits at a document Y, a vertical one at an X.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GuideAxis {
    Horizontal,
    Vertical,
}

/// A user-placed alignment line.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Guide {
    pub id: Uuid,
    pub axis: GuideAxis,
    /// Document pixels: Y for a horizontal guide, X for a vertical one.
    pub position: f32,
}

/// Most guides a document may hold, as upstream's project validation.
pub const MAX_GUIDES: usize = 1_000;
/// Furthest a guide may sit from the document origin, in pixels.
pub const MAX_GUIDE_POSITION: f32 = 1_000_000.0;

impl Guide {
    pub fn new(axis: GuideAxis, position: f32) -> Self {
        Self {
            id: Uuid::new_v4(),
            axis,
            position,
        }
    }

    /// Moves the guide with the canvas when content shifts by `dx`/`dy`.
    pub fn offset(&mut self, dx: f32, dy: f32) {
        self.position += match self.axis {
            GuideAxis::Vertical => dx,
            GuideAxis::Horizontal => dy,
        };
    }

    /// Scales the guide with the canvas when the image is resampled.
    pub fn scale(&mut self, sx: f32, sy: f32) {
        self.position *= match self.axis {
            GuideAxis::Vertical => sx,
            GuideAxis::Horizontal => sy,
        };
    }

    /// Mirrors a guide that runs across the flip, so it stays on the same content.
    pub fn mirror(&mut self, horizontal: bool, extent: f32) {
        if (horizontal && self.axis == GuideAxis::Vertical)
            || (!horizontal && self.axis == GuideAxis::Horizontal)
        {
            self.position = extent - self.position;
        }
    }
}

/// Checks a document's guides as a loaded project must have them.
pub fn validate_guides(guides: &[Guide]) -> Result<()> {
    ensure!(guides.len() <= MAX_GUIDES, "Too many guides");
    let mut ids = std::collections::HashSet::new();
    for guide in guides {
        ensure!(ids.insert(guide.id), "Duplicate guide identifiers");
        ensure!(
            guide.position.is_finite() && guide.position.abs() <= MAX_GUIDE_POSITION,
            "Invalid guide position"
        );
    }
    Ok(())
}

/// Grid color choices, after Photoshop's Guides, Grid & Slices settings.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GridColor {
    #[default]
    LightGray,
    LightBlue,
    LightRed,
    Green,
    MediumBlue,
    Yellow,
    Magenta,
    Cyan,
    Black,
    /// The settings' own `custom_color`.
    Custom,
}

impl GridColor {
    pub const ALL: [Self; 10] = [
        Self::LightGray,
        Self::LightBlue,
        Self::LightRed,
        Self::Green,
        Self::MediumBlue,
        Self::Yellow,
        Self::Magenta,
        Self::Cyan,
        Self::Black,
        Self::Custom,
    ];

    /// Untranslated display name.
    pub fn name(self) -> &'static str {
        match self {
            Self::LightGray => "Light Gray",
            Self::LightBlue => "Light Blue",
            Self::LightRed => "Light Red",
            Self::Green => "Green",
            Self::MediumBlue => "Medium Blue",
            Self::Yellow => "Yellow",
            Self::Magenta => "Magenta",
            Self::Cyan => "Cyan",
            Self::Black => "Black",
            Self::Custom => "Custom",
        }
    }

    /// sRGB of a preset; `None` for Custom.
    pub fn rgb(self) -> Option<[u8; 3]> {
        // Upstream's PaletteColor components, scaled to bytes.
        Some(match self {
            Self::LightGray => [179, 179, 179],
            Self::LightBlue => [74, 199, 255],
            Self::LightRed => [255, 102, 102],
            Self::Green => [64, 204, 64],
            Self::MediumBlue => [51, 102, 255],
            Self::Yellow => [255, 255, 0],
            Self::Magenta => [255, 0, 255],
            Self::Cyan => [0, 255, 255],
            Self::Black => [0, 0, 0],
            Self::Custom => return None,
        })
    }
}

/// How the major grid lines are drawn; subdivisions are always fainter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GridStyle {
    #[default]
    Lines,
    DashedLines,
    Dots,
}

impl GridStyle {
    pub const ALL: [Self; 3] = [Self::Lines, Self::DashedLines, Self::Dots];

    /// Untranslated display name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Lines => "Lines",
            Self::DashedLines => "Dashed Lines",
            Self::Dots => "Dots",
        }
    }

    /// On and off lengths in screen points; `None` for a solid line.
    pub fn dashes(self) -> Option<[f32; 2]> {
        match self {
            Self::Lines => None,
            Self::DashedLines => Some([4.0, 3.0]),
            Self::Dots => Some([1.0, 2.0]),
        }
    }
}

/// The non-printing layout grid: a major line every `spacing` pixels, split into `subdivisions`.
/// Defaults to 64 px in eight parts (a line every 8 px), light gray lines at 45%, as upstream.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GridSettings {
    /// Pixels between major lines.
    pub spacing: u32,
    /// Parts each major square is split into; never finer than a pixel.
    pub subdivisions: u32,
    pub color: GridColor,
    /// Used while `color` is Custom; kept when a preset is chosen, so switching back finds it.
    pub custom_color: [u8; 3],
    pub style: GridStyle,
    /// The major lines' opacity, in percent.
    pub opacity: u32,
}

impl Default for GridSettings {
    fn default() -> Self {
        Self {
            spacing: 64,
            subdivisions: 8,
            color: GridColor::LightGray,
            custom_color: [179, 179, 179],
            style: GridStyle::Lines,
            opacity: 45,
        }
    }
}

impl GridSettings {
    pub const SPACING_RANGE: std::ops::RangeInclusive<u32> = 2..=4096;
    pub const SUBDIVISION_RANGE: std::ops::RangeInclusive<u32> = 1..=64;
    pub const OPACITY_RANGE: std::ops::RangeInclusive<u32> = 1..=100;

    /// Whether the values are in range, with no subdivision finer than a pixel.
    pub fn is_valid(&self) -> bool {
        Self::SPACING_RANGE.contains(&self.spacing)
            && Self::SUBDIVISION_RANGE.contains(&self.subdivisions)
            && self.subdivisions <= self.spacing
            && Self::OPACITY_RANGE.contains(&self.opacity)
    }

    /// The settings forced into range, for hand-edited preference files.
    pub fn normalized(mut self) -> Self {
        self.spacing = self
            .spacing
            .clamp(*Self::SPACING_RANGE.start(), *Self::SPACING_RANGE.end());
        self.subdivisions = self
            .subdivisions
            .clamp(
                *Self::SUBDIVISION_RANGE.start(),
                *Self::SUBDIVISION_RANGE.end(),
            )
            .min(self.spacing);
        self.opacity = self
            .opacity
            .clamp(*Self::OPACITY_RANGE.start(), *Self::OPACITY_RANGE.end());
        self
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(self.is_valid(), "Invalid grid settings");
        Ok(())
    }

    /// Document pixels between adjacent lines, subdivisions included.
    pub fn step(&self) -> f32 {
        self.spacing as f32 / self.subdivisions.max(1) as f32
    }

    /// The `index`th line from the origin, in whole pixels. Counted from the origin rather than
    /// added up, so an uneven step doesn't drift off the majors.
    pub fn line(&self, index: i64) -> f32 {
        (index as f32 * self.step()).round()
    }

    /// Every line, subdivisions included, from `start` to `end` and within `0..=length`.
    pub fn lines(&self, start: f32, end: f32, length: f32) -> Vec<f32> {
        let step = self.step();
        if step.is_nan() || step <= 0.0 || !start.is_finite() || !end.is_finite() || length < 0.0 {
            return Vec::new();
        }
        let first = (start.max(0.0) / step - 0.001).ceil().max(0.0) as i64;
        let last = ((end.min(length) / step) + 0.001).floor() as i64;
        (first..=last).map(|i| self.line(i)).collect()
    }

    /// The line nearest `value` within `0..=length`.
    pub fn nearest_line(&self, value: f32, length: f32) -> Option<f32> {
        let step = self.step();
        if step.is_nan() || step <= 0.0 || !value.is_finite() || length < 0.0 {
            return None;
        }
        let count = (length / step + 0.001).floor() as i64;
        let index = ((value / step).round() as i64).clamp(0, count);
        // Rounding the line to whole pixels can move it past a neighbour's midpoint.
        [index - 1, index, index + 1]
            .into_iter()
            .filter(|i| (0..=count).contains(i))
            .map(|i| self.line(i))
            .min_by(|a, b| (a - value).abs().total_cmp(&(b - value).abs()))
    }

    pub fn is_major(&self, value: f32) -> bool {
        (value.round() as i64).rem_euclid(i64::from(self.spacing.max(1))) == 0
    }

    pub fn rgb(&self) -> [u8; 3] {
        self.color.rgb().unwrap_or(self.custom_color)
    }

    /// The majors' opacity, 0–1.
    pub fn major_alpha(&self) -> f32 {
        self.opacity
            .clamp(*Self::OPACITY_RANGE.start(), *Self::OPACITY_RANGE.end()) as f32
            / 100.0
    }

    /// Subdivisions at a little over half the majors' opacity: 28% beside the default 45%.
    pub fn subdivision_alpha(&self) -> f32 {
        self.major_alpha() * 28.0 / 45.0
    }

    /// Back to the default grid and look, keeping the custom color for a later switch back.
    pub fn restore_defaults(&mut self) {
        *self = Self {
            custom_color: self.custom_color,
            ..Self::default()
        };
    }
}

/// View → Snap and View → Snap To. Hidden guides and a hidden grid never snap, as in Photoshop.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SnapSettings {
    /// The master toggle.
    pub enabled: bool,
    pub guides: bool,
    pub grid: bool,
    pub layers: bool,
    pub bounds: bool,
}

impl Default for SnapSettings {
    fn default() -> Self {
        // Upstream's defaults: everything but the grid.
        Self {
            enabled: true,
            guides: true,
            grid: false,
            layers: true,
            bounds: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grid_defaults_match_upstream() {
        let grid = GridSettings::default();
        assert_eq!((grid.spacing, grid.subdivisions), (64, 8));
        assert_eq!(grid.step(), 8.0);
        assert_eq!(grid.opacity, 45);
        assert_eq!(grid.rgb(), [179, 179, 179]);
        assert!((grid.subdivision_alpha() - 0.28).abs() < 1e-6);
        assert!(grid.is_valid());
    }

    #[test]
    fn grid_lines_stay_on_whole_pixels_and_majors() {
        let grid = GridSettings {
            spacing: 10,
            subdivisions: 3,
            ..GridSettings::default()
        };
        assert_eq!(
            grid.lines(0.0, 20.0, 20.0),
            [0.0, 3.0, 7.0, 10.0, 13.0, 17.0, 20.0]
        );
        assert!(grid.is_major(10.0) && grid.is_major(0.0) && !grid.is_major(7.0));
        // A visible window, clipped to the document.
        assert_eq!(grid.lines(-50.0, 8.0, 20.0), [0.0, 3.0, 7.0]);
        assert_eq!(grid.lines(12.0, 500.0, 20.0), [13.0, 17.0, 20.0]);
        assert_eq!(grid.nearest_line(6.0, 20.0), Some(7.0));
        assert_eq!(grid.nearest_line(-40.0, 20.0), Some(0.0));
        assert_eq!(grid.nearest_line(400.0, 20.0), Some(20.0));
    }

    #[test]
    fn grid_settings_are_validated_and_normalized() {
        let bad = GridSettings {
            spacing: 4,
            subdivisions: 8,
            ..GridSettings::default()
        };
        assert!(!bad.is_valid() && bad.validate().is_err());
        assert_eq!(bad.normalized().subdivisions, 4);
        let wild = GridSettings {
            spacing: 0,
            subdivisions: 0,
            opacity: 900,
            ..GridSettings::default()
        }
        .normalized();
        assert_eq!((wild.spacing, wild.subdivisions, wild.opacity), (2, 1, 100));
        assert!(wild.is_valid());
    }

    #[test]
    fn restore_defaults_keeps_the_custom_color() {
        let mut grid = GridSettings {
            spacing: 100,
            color: GridColor::Custom,
            custom_color: [1, 2, 3],
            style: GridStyle::Dots,
            ..GridSettings::default()
        };
        assert_eq!(grid.rgb(), [1, 2, 3]);
        grid.restore_defaults();
        assert_eq!(
            grid,
            GridSettings {
                custom_color: [1, 2, 3],
                ..GridSettings::default()
            }
        );
    }

    #[test]
    fn guides_follow_canvas_operations_and_validate() {
        let mut vertical = Guide::new(GuideAxis::Vertical, 10.0);
        let mut horizontal = Guide::new(GuideAxis::Horizontal, 20.0);
        vertical.offset(5.0, 7.0);
        horizontal.offset(5.0, 7.0);
        assert_eq!((vertical.position, horizontal.position), (15.0, 27.0));
        vertical.scale(2.0, 0.5);
        horizontal.scale(2.0, 0.5);
        assert_eq!((vertical.position, horizontal.position), (30.0, 13.5));
        vertical.mirror(true, 100.0);
        horizontal.mirror(true, 100.0);
        assert_eq!((vertical.position, horizontal.position), (70.0, 13.5));

        assert!(validate_guides(&[vertical, horizontal]).is_ok());
        assert!(validate_guides(&[vertical, vertical]).is_err());
        let far = Guide::new(GuideAxis::Vertical, 2.0e6);
        assert!(validate_guides(&[far]).is_err());
        let nan = Guide::new(GuideAxis::Vertical, f32::NAN);
        assert!(validate_guides(&[nan]).is_err());
        let many: Vec<_> = (0..=MAX_GUIDES)
            .map(|i| Guide::new(GuideAxis::Vertical, i as f32))
            .collect();
        assert!(validate_guides(&many).is_err());
    }

    #[test]
    fn guide_json_is_compact_and_stable() {
        let guide = Guide {
            id: Uuid::nil(),
            axis: GuideAxis::Horizontal,
            position: 12.5,
        };
        let json = serde_json::to_string(&guide).unwrap();
        assert_eq!(
            json,
            r#"{"id":"00000000-0000-0000-0000-000000000000","axis":"horizontal","position":12.5}"#
        );
        assert_eq!(serde_json::from_str::<Guide>(&json).unwrap(), guide);
    }
}
