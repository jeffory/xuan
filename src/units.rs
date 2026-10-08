//! Physical units for sizes, and the document's print resolution (issue 94).
//!
//! Pixels stay the source of truth: a size typed in centimetres becomes whole pixels through the
//! document's resolution, and a size shown in centimetres is worked out again from the pixels
//! each time, so switching units back and forth never drifts. One inch is 2.54 cm, 72 points
//! (PostScript points) or 6 picas.
use serde::{Deserialize, Serialize};

use crate::document::MAX_SIDE;

/// The resolutions a document may have, in pixels per inch.
pub const MIN_RESOLUTION: f32 = 1.0;
pub const MAX_RESOLUTION: f32 = 9600.0;
/// New documents, and images whose files state no resolution.
pub const DEFAULT_RESOLUTION: f32 = 72.0;

/// Centimetres in an inch.
const CM_PER_INCH: f64 = 2.54;

/// Whether `ppi` is a resolution a document may have: finite and within
/// [`MIN_RESOLUTION`]`..=`[`MAX_RESOLUTION`]. Zero, negative and NaN are refused.
pub fn valid_resolution(ppi: f64) -> bool {
    ppi.is_finite() && (f64::from(MIN_RESOLUTION)..=f64::from(MAX_RESOLUTION)).contains(&ppi)
}

/// A unit for widths, heights and ruler positions.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Unit {
    #[default]
    #[serde(rename = "px")]
    Pixels,
    #[serde(rename = "in")]
    Inches,
    #[serde(rename = "cm")]
    Centimeters,
    #[serde(rename = "mm")]
    Millimeters,
    #[serde(rename = "pt")]
    Points,
    #[serde(rename = "pica")]
    Picas,
    /// Of a reference size: the document's, in Canvas Size, Image Size and the rulers.
    #[serde(rename = "percent")]
    Percent,
}

impl Unit {
    /// Every unit, in menu order.
    pub const ALL: [Self; 7] = [
        Self::Pixels,
        Self::Inches,
        Self::Centimeters,
        Self::Millimeters,
        Self::Points,
        Self::Picas,
        Self::Percent,
    ];
    /// The units of an absolute length, without Percent: File → New's menu.
    pub const LENGTHS: [Self; 6] = [
        Self::Pixels,
        Self::Inches,
        Self::Centimeters,
        Self::Millimeters,
        Self::Points,
        Self::Picas,
    ];

    /// Untranslated display name.
    pub fn name(self) -> &'static str {
        match self {
            Self::Pixels => "Pixels",
            Self::Inches => "Inches",
            Self::Centimeters => "Centimeters",
            Self::Millimeters => "Millimeters",
            Self::Points => "Points",
            Self::Picas => "Picas",
            Self::Percent => "Percent",
        }
    }

    /// The abbreviation shown after a value; also accepted when typed.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::Pixels => "px",
            Self::Inches => "in",
            Self::Centimeters => "cm",
            Self::Millimeters => "mm",
            Self::Points => "pt",
            Self::Picas => "pica",
            Self::Percent => "%",
        }
    }

    /// Units in one inch, for the units that measure print size.
    fn per_inch(self) -> Option<f64> {
        match self {
            Self::Inches => Some(1.0),
            Self::Centimeters => Some(CM_PER_INCH),
            Self::Millimeters => Some(CM_PER_INCH * 10.0),
            Self::Points => Some(72.0),
            Self::Picas => Some(6.0),
            Self::Pixels | Self::Percent => None,
        }
    }

    /// Whether the unit is a print size, so converting it depends on the resolution.
    pub fn is_physical(self) -> bool {
        self.per_inch().is_some()
    }

    /// Decimals shown in a field: enough to tell one pixel from the next at common resolutions.
    pub fn decimals(self) -> usize {
        match self {
            Self::Pixels => 0,
            Self::Inches | Self::Picas => 3,
            Self::Centimeters | Self::Percent => 2,
            Self::Millimeters | Self::Points => 1,
        }
    }

    /// Document pixels in one of this unit at `ppi`, where 100% is `reference` pixels. `None`
    /// when that is not a positive finite number, as for a resolution of 0 or NaN.
    pub fn pixels_per_unit(self, ppi: f64, reference: f64) -> Option<f64> {
        let scale = match self {
            Self::Pixels => 1.0,
            Self::Percent => reference / 100.0,
            _ => ppi / self.per_inch()?,
        };
        (scale.is_finite() && scale > 0.0).then_some(scale)
    }

    /// `pixels` expressed in this unit; 0 when the unit cannot be converted (see
    /// [`Unit::pixels_per_unit`]).
    pub fn from_pixels(self, pixels: f64, ppi: f64, reference: f64) -> f64 {
        self.pixels_per_unit(ppi, reference)
            .map_or(0.0, |scale| pixels / scale)
    }

    /// `value` of this unit in pixels, not rounded. `None` when either is not finite.
    pub fn to_pixels(self, value: f64, ppi: f64, reference: f64) -> Option<f64> {
        let pixels = value * self.pixels_per_unit(ppi, reference)?;
        pixels.is_finite().then_some(pixels)
    }

    /// The unit a typed suffix names, ignoring case: `px`, `in`, `"`, `cm`, `mm`, `pt`, `pc`,
    /// `pica`, `%` and their spelled-out forms.
    pub fn from_suffix(suffix: &str) -> Option<Self> {
        Some(match suffix.trim().to_lowercase().as_str() {
            "px" | "pixel" | "pixels" => Self::Pixels,
            "in" | "inch" | "inches" | "\"" | "″" => Self::Inches,
            "cm" | "centimeter" | "centimeters" | "centimetre" | "centimetres" | "厘米" => {
                Self::Centimeters
            }
            "mm" | "millimeter" | "millimeters" | "millimetre" | "millimetres" | "毫米" => {
                Self::Millimeters
            }
            "pt" | "pts" | "point" | "points" | "点" => Self::Points,
            "pc" | "pica" | "picas" | "派卡" => Self::Picas,
            "%" | "percent" => Self::Percent,
            _ => return None,
        })
    }
}

/// Rounds a pixel length to whole pixels within 1..=[`MAX_SIDE`]: what a size field may hold.
/// `None` for NaN or infinity.
pub fn whole_pixels(pixels: f64) -> Option<u32> {
    pixels
        .is_finite()
        .then(|| pixels.round().clamp(1.0, f64::from(MAX_SIDE)) as u32)
}

/// A number typed into a size field, with an optional unit after it: `10cm`, `4 in`, `210mm`,
/// `12pt`, `-5 px`. A plain number is in `default`. `None` when the text is not a finite number
/// or names no unit [`Unit::from_suffix`] knows. A comma may stand for the decimal point.
pub fn parse(text: &str, default: Unit) -> Option<(f64, Unit)> {
    let text = text.trim();
    let end = text
        .char_indices()
        .find(|&(i, c)| {
            !(c.is_ascii_digit() || c == '.' || c == ',' || (i == 0 && "+-−".contains(c)))
        })
        .map_or(text.len(), |(i, _)| i);
    let (number, suffix) = text.split_at(end);
    let number = number.replace(',', ".").replace('−', "-");
    let value: f64 = number.parse().ok()?;
    if !value.is_finite() {
        return None;
    }
    let unit = if suffix.trim().is_empty() {
        default
    } else {
        Unit::from_suffix(suffix)?
    };
    Some((value, unit))
}

/// Parses `text` for a field showing `field` units: a value in another unit is converted to
/// `field` through `ppi` and `reference` (100%). `None` when it cannot be read or converted.
pub fn parse_into(text: &str, field: Unit, ppi: f64, reference: f64) -> Option<f64> {
    let (value, unit) = parse(text, field)?;
    if unit == field {
        return Some(value);
    }
    let pixels = unit.to_pixels(value, ppi, reference)?;
    field.pixels_per_unit(ppi, reference)?;
    Some(field.from_pixels(pixels, ppi, reference))
}

/// How a resolution is shown and typed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResolutionUnit {
    #[default]
    #[serde(rename = "ppi")]
    PerInch,
    #[serde(rename = "ppcm")]
    PerCentimeter,
}

impl ResolutionUnit {
    pub const ALL: [Self; 2] = [Self::PerInch, Self::PerCentimeter];

    /// Untranslated display name.
    pub fn name(self) -> &'static str {
        match self {
            Self::PerInch => "Pixels/inch",
            Self::PerCentimeter => "Pixels/cm",
        }
    }

    /// The abbreviation shown after a value.
    pub fn suffix(self) -> &'static str {
        match self {
            Self::PerInch => "ppi",
            Self::PerCentimeter => "px/cm",
        }
    }

    /// `ppi` in this unit.
    pub fn from_ppi(self, ppi: f64) -> f64 {
        match self {
            Self::PerInch => ppi,
            Self::PerCentimeter => ppi / CM_PER_INCH,
        }
    }

    /// `value` of this unit in pixels per inch.
    pub fn to_ppi(self, value: f64) -> f64 {
        match self {
            Self::PerInch => value,
            Self::PerCentimeter => value * CM_PER_INCH,
        }
    }

    /// Parses a typed resolution in this unit; a `ppi`, `dpi` or `px/cm` suffix picks the unit.
    /// `None` unless the result, in pixels per inch, is a [`valid_resolution`].
    pub fn parse(self, text: &str) -> Option<f64> {
        let text = text.trim().to_lowercase();
        let (number, unit) = [
            ("px/cm", Self::PerCentimeter),
            ("ppcm", Self::PerCentimeter),
            ("ppi", Self::PerInch),
            ("dpi", Self::PerInch),
            ("px/in", Self::PerInch),
        ]
        .into_iter()
        .find_map(|(suffix, unit)| Some((text.strip_suffix(suffix)?.trim(), unit)))
        .unwrap_or((text.as_str(), self));
        let (value, _) = parse(number, Unit::Pixels).filter(|(_, u)| *u == Unit::Pixels)?;
        let ppi = unit.to_ppi(value);
        valid_resolution(ppi).then(|| self.from_ppi(ppi))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-9 * b.abs().max(1.0)
    }

    #[test]
    fn a4_at_300_ppi_is_2480_by_3508() {
        let width = Unit::Millimeters.to_pixels(210.0, 300.0, 0.0).unwrap();
        let height = Unit::Millimeters.to_pixels(297.0, 300.0, 0.0).unwrap();
        assert_eq!(whole_pixels(width), Some(2480));
        assert_eq!(whole_pixels(height), Some(3508));
    }

    #[test]
    fn units_convert_through_the_inch() {
        // 300 px at 300 ppi is one inch.
        let cases = [
            (Unit::Pixels, 300.0),
            (Unit::Inches, 1.0),
            (Unit::Centimeters, 2.54),
            (Unit::Millimeters, 25.4),
            (Unit::Points, 72.0),
            (Unit::Picas, 6.0),
        ];
        for (unit, expected) in cases {
            assert!(
                close(unit.from_pixels(300.0, 300.0, 0.0), expected),
                "{unit:?}"
            );
            assert!(
                close(unit.to_pixels(expected, 300.0, 0.0).unwrap(), 300.0),
                "{unit:?}"
            );
        }
        // 10 in at 300 ppi is 3000 px; 3000 px at 150 ppi prints 20 in wide.
        assert_eq!(
            whole_pixels(Unit::Inches.to_pixels(10.0, 300.0, 0.0).unwrap()),
            Some(3000)
        );
        assert!(close(Unit::Inches.from_pixels(3000.0, 150.0, 0.0), 20.0));
        // Percent is of the reference size and ignores the resolution.
        assert!(close(Unit::Percent.from_pixels(50.0, 300.0, 200.0), 25.0));
        assert!(close(
            Unit::Percent.to_pixels(150.0, f64::NAN, 200.0).unwrap(),
            300.0
        ));
        assert!(close(Unit::Pixels.from_pixels(42.0, f64::NAN, 0.0), 42.0));
    }

    #[test]
    fn round_trips_do_not_drift() {
        for ppi in [1.0, 72.0, 96.0, 150.0, 299.5, 300.0, 1200.0, 9600.0] {
            for unit in Unit::ALL {
                for pixels in [1_u32, 7, 100, 2480, 3508, 30_000, MAX_SIDE] {
                    let mut px = pixels;
                    // Shown, then committed again, many times over.
                    for _ in 0..20 {
                        let shown = unit.from_pixels(f64::from(px), ppi, 1000.0);
                        px = whole_pixels(unit.to_pixels(shown, ppi, 1000.0).unwrap()).unwrap();
                    }
                    assert_eq!(px, pixels, "{unit:?} at {ppi} ppi");
                }
            }
        }
    }

    #[test]
    fn rounding_and_bounds() {
        assert_eq!(whole_pixels(2480.49), Some(2480));
        assert_eq!(whole_pixels(2480.5), Some(2481));
        assert_eq!(whole_pixels(0.2), Some(1));
        assert_eq!(whole_pixels(-50.0), Some(1));
        assert_eq!(whole_pixels(1e30), Some(MAX_SIDE));
        assert_eq!(whole_pixels(f64::NAN), None);
        assert_eq!(whole_pixels(f64::INFINITY), None);
    }

    #[test]
    fn bad_resolutions_never_convert() {
        for ppi in [0.0, -72.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            assert_eq!(Unit::Centimeters.pixels_per_unit(ppi, 100.0), None, "{ppi}");
            assert_eq!(Unit::Centimeters.to_pixels(10.0, ppi, 100.0), None, "{ppi}");
            assert_eq!(
                Unit::Centimeters.from_pixels(10.0, ppi, 100.0),
                0.0,
                "{ppi}"
            );
            assert!(!valid_resolution(ppi), "{ppi}");
        }
        // Pixels and percent do not need one.
        assert_eq!(Unit::Pixels.to_pixels(10.0, f64::NAN, 0.0), Some(10.0));
        assert_eq!(Unit::Percent.to_pixels(10.0, 0.0, 0.0), None);
        // Huge values convert without overflow and are bounded when rounded.
        let huge = Unit::Inches.to_pixels(1e300, 9600.0, 0.0).unwrap();
        assert_eq!(whole_pixels(huge), Some(MAX_SIDE));
        assert_eq!(Unit::Inches.to_pixels(f64::MAX, 9600.0, 0.0), None);
        assert!(valid_resolution(1.0) && valid_resolution(9600.0));
        assert!(!valid_resolution(9600.01) && !valid_resolution(0.99));
    }

    #[test]
    fn parses_typed_units() {
        assert_eq!(parse("10cm", Unit::Pixels), Some((10.0, Unit::Centimeters)));
        assert_eq!(parse(" 4 in ", Unit::Pixels), Some((4.0, Unit::Inches)));
        assert_eq!(
            parse("210MM", Unit::Pixels),
            Some((210.0, Unit::Millimeters))
        );
        assert_eq!(parse("12pt", Unit::Pixels), Some((12.0, Unit::Points)));
        assert_eq!(parse("3 picas", Unit::Pixels), Some((3.0, Unit::Picas)));
        assert_eq!(parse("2pc", Unit::Pixels), Some((2.0, Unit::Picas)));
        assert_eq!(parse("8.5\"", Unit::Pixels), Some((8.5, Unit::Inches)));
        assert_eq!(parse("50%", Unit::Pixels), Some((50.0, Unit::Percent)));
        assert_eq!(parse("1920", Unit::Pixels), Some((1920.0, Unit::Pixels)));
        assert_eq!(parse("7", Unit::Inches), Some((7.0, Unit::Inches)));
        assert_eq!(parse("-5 px", Unit::Inches), Some((-5.0, Unit::Pixels)));
        assert_eq!(
            parse("2,5 cm", Unit::Pixels),
            Some((2.5, Unit::Centimeters))
        );
        assert_eq!(parse(".5in", Unit::Pixels), Some((0.5, Unit::Inches)));
        for bad in [
            "",
            "cm",
            "abc",
            "10 furlongs",
            "1.2.3",
            "NaN",
            "inf",
            "1e999",
            "--1",
        ] {
            assert_eq!(parse(bad, Unit::Pixels), None, "{bad}");
        }
    }

    #[test]
    fn typed_values_convert_into_the_field_unit() {
        // 10 cm at 300 ppi, typed into a pixel field.
        let px = parse_into("10cm", Unit::Pixels, 300.0, 0.0).unwrap();
        assert_eq!(whole_pixels(px), Some(1181));
        // 1 in typed into a millimetre field.
        assert!(close(
            parse_into("1in", Unit::Millimeters, 300.0, 0.0).unwrap(),
            25.4
        ));
        // A plain number stays in the field's unit.
        assert_eq!(parse_into("12", Unit::Points, 300.0, 0.0), Some(12.0));
        // Physical units need a resolution.
        assert_eq!(parse_into("10cm", Unit::Pixels, 0.0, 0.0), None);
        assert_eq!(parse_into("10cm", Unit::Pixels, f64::NAN, 0.0), None);
        assert_eq!(parse_into("10px", Unit::Inches, 0.0, 0.0), None);
        // Percent of the reference.
        assert_eq!(parse_into("50%", Unit::Pixels, 72.0, 640.0), Some(320.0));
    }

    #[test]
    fn resolutions_parse_and_convert() {
        let ppi = ResolutionUnit::PerInch;
        let ppcm = ResolutionUnit::PerCentimeter;
        assert_eq!(ppi.parse("300"), Some(300.0));
        assert_eq!(ppi.parse("300 dpi"), Some(300.0));
        assert!(close(ppi.parse("118.11 px/cm").unwrap(), 299.9994));
        assert!(close(ppcm.parse("300ppi").unwrap(), 300.0 / 2.54));
        assert_eq!(ppcm.parse("100"), Some(100.0));
        for bad in ["0", "-72", "abc", "", "NaN", "9601", "1e9", "5cm"] {
            assert_eq!(ppi.parse(bad), None, "{bad}");
        }
        assert!(close(ppcm.to_ppi(ppcm.from_ppi(300.0)), 300.0));
    }

    #[test]
    fn units_are_stored_by_short_names() {
        #[derive(Serialize, Deserialize)]
        struct Units {
            unit: Unit,
            resolution: ResolutionUnit,
        }
        let text = toml::to_string(&Units {
            unit: Unit::Millimeters,
            resolution: ResolutionUnit::PerCentimeter,
        })
        .unwrap();
        assert_eq!(text, "unit = \"mm\"\nresolution = \"ppcm\"\n");
        for unit in Unit::ALL {
            let text = toml::to_string(&Units {
                unit,
                resolution: ResolutionUnit::PerInch,
            })
            .unwrap();
            assert_eq!(toml::from_str::<Units>(&text).unwrap().unit, unit);
            assert_eq!(Unit::from_suffix(unit.suffix()), Some(unit));
        }
    }
}
