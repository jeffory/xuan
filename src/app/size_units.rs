//! Units in File → New, Image → Canvas Size and Image → Image Size (issue 94): Width and Height
//! in pixels, print units or percent, Image Size's Resample and Canvas Size's Relative.
//!
//! The dialogs keep whole pixels in `EditorApp::dimensions`. What the fields show is worked out
//! from those pixels every frame, and a value typed or dragged becomes whole pixels at once, so
//! switching units never changes the size and never drifts.
use std::ops::RangeInclusive;

use xuan::{
    document::MAX_SIDE,
    i18n::tr,
    units::{self, ResolutionUnit, Unit},
};

use super::widgets;

/// The size dialogs' units and the size they opened with.
#[derive(Clone, Debug, PartialEq)]
pub(super) struct SizeUnits {
    /// The unit Width and Height are shown in.
    pub unit: Unit,
    pub resolution_unit: ResolutionUnit,
    /// Image Size: a new size or resolution resamples the pixels. Off, the pixels stay as they
    /// are and only the print size and resolution change.
    pub resample: bool,
    /// Canvas Size: Width and Height are what is added to (or taken from) the current size.
    pub relative: bool,
    /// The document's size when the dialog opened: 100% for Percent, what Relative adds to,
    /// and the pixels Image Size keeps without Resample.
    pub original: [u32; 2],
    /// The print size, in inches, a change of resolution keeps; set by the first change after
    /// the size was edited, so dragging the resolution does not round the size again and again.
    print: Option<[f64; 2]>,
}

impl Default for SizeUnits {
    fn default() -> Self {
        Self {
            unit: Unit::Pixels,
            resolution_unit: ResolutionUnit::PerInch,
            resample: true,
            relative: false,
            original: [1, 1],
            print: None,
        }
    }
}

impl SizeUnits {
    /// Starts a dialog for a document of `original` pixels, shown in `unit` (Percent only where
    /// `percent` is allowed).
    pub fn open(&mut self, original: [u32; 2], unit: Unit, resolution_unit: ResolutionUnit) {
        *self = Self {
            unit,
            resolution_unit,
            original,
            ..Self::default()
        };
    }

    fn reference(&self, axis: usize) -> f64 {
        f64::from(self.original[axis])
    }

    /// What a zero in the field stands for: nothing, or with Relative the current size.
    fn base(&self, axis: usize) -> f64 {
        if self.relative {
            self.reference(axis)
        } else {
            0.0
        }
    }

    /// What the Width (`axis` 0) or Height (1) field shows for `pixels` at `ppi`.
    pub fn shown(&self, pixels: u32, axis: usize, ppi: f32) -> f64 {
        self.unit.from_pixels(
            f64::from(pixels) - self.base(axis),
            f64::from(ppi),
            self.reference(axis),
        )
    }

    /// The whole pixels a field's `value` stands for, within 1..=[`MAX_SIDE`]. `None` when it
    /// cannot be converted, as with a print unit and no usable resolution.
    pub fn pixels(&self, value: f64, axis: usize, ppi: f32) -> Option<u32> {
        let pixels = self
            .unit
            .to_pixels(value, f64::from(ppi), self.reference(axis))?;
        units::whole_pixels(pixels + self.base(axis))
    }

    /// The values a field may hold: those of 1 to [`MAX_SIDE`] pixels.
    pub fn range(&self, axis: usize, ppi: f32) -> RangeInclusive<f64> {
        self.shown(1, axis, ppi)..=self.shown(MAX_SIDE, axis, ppi)
    }

    /// One pixel in the field's unit: how far a field moves for each point dragged.
    pub fn step(&self, axis: usize, ppi: f32) -> f64 {
        let step = self
            .unit
            .from_pixels(1.0, f64::from(ppi), self.reference(axis));
        if step > 0.0 { step } else { 1.0 }
    }

    /// Image Size without Resample: the resolution at which the document's pixels print `value`
    /// long. `None` for pixels and percent, which do not set a print size, or for a resolution
    /// no document may have.
    pub fn resolution_for(&self, value: f64, axis: usize) -> Option<f32> {
        if !self.unit.is_physical() {
            return None;
        }
        let inches = self.unit.to_pixels(value, 1.0, 0.0)?;
        let ppi = self.reference(axis) / inches;
        units::valid_resolution(ppi).then_some(ppi as f32)
    }

    /// The resolution changed from `before` to `after`. With a size in print units, File → New
    /// and Image Size with Resample keep its print size, so the pixels change; otherwise (and in
    /// Canvas Size) the pixels stay and only the print size changes.
    pub fn resolution_changed(
        &mut self,
        dimensions: &mut [u32; 2],
        before: f32,
        after: f32,
        keeps_print_size: bool,
    ) {
        if !keeps_print_size
            || !self.unit.is_physical()
            || before == after
            || !units::valid_resolution(f64::from(before))
            || !units::valid_resolution(f64::from(after))
        {
            return;
        }
        let print = *self
            .print
            .get_or_insert([0, 1].map(|axis| f64::from(dimensions[axis]) / f64::from(before)));
        for axis in 0..2 {
            if let Some(pixels) = units::whole_pixels(print[axis] * f64::from(after)) {
                dimensions[axis] = pixels;
            }
        }
    }

    /// Width or Height was edited: the next change of resolution keeps this size.
    pub fn size_edited(&mut self) {
        self.print = None;
    }

    /// A preset set the size: changes of resolution keep its exact print size, in inches,
    /// rather than the one its rounded pixels make.
    pub fn keep_print_size(&mut self, inches: [f64; 2]) {
        self.print = Some(inches);
    }
}

/// Parses a size typed into a field showing `unit`: a plain number, or one with any unit
/// (`10cm`, `4 in`), converted into `unit`. Sizes must be positive unless they are `relative`.
pub(super) fn parse_size(
    text: &str,
    unit: Unit,
    ppi: f32,
    reference: f64,
    relative: bool,
) -> Option<f64> {
    let value = units::parse_into(text, unit, f64::from(ppi), reference)?;
    (relative || value > 0.0).then_some(value)
}

/// Width and Height: the pixel size, megapixels and, for Image Size, the memory it takes.
pub(super) fn summary(dimensions: [u32; 2], memory: bool) -> String {
    let [width, height] = dimensions;
    let pixels = u64::from(width) * u64::from(height);
    let mut text = format!(
        "{width} × {height} px · {} {}",
        megapixels(pixels),
        tr("MP")
    );
    if memory {
        text.push_str(" · ");
        text.push_str(&xuan::limits::size(pixels * 4));
    }
    text
}

/// Megapixels with one decimal below 10, as Photoshop shows them: "0.3", "8.7", "24".
fn megapixels(pixels: u64) -> String {
    let megapixels = pixels as f64 / 1_000_000.0;
    if megapixels < 10.0 {
        format!("{megapixels:.1}")
    } else {
        format!("{megapixels:.0}")
    }
}

/// The unit menu beside Width and Height. Whether the unit changed.
pub(super) fn unit_menu(ui: &mut egui::Ui, unit: &mut Unit, choices: &[Unit]) -> bool {
    let before = *unit;
    widgets::PopUp::from_id_salt("size_unit")
        .selected_text(tr(unit.name()))
        .width(110.0)
        .show_ui(ui, |ui| {
            for choice in choices {
                widgets::menu_choice(ui, unit, *choice, tr(choice.name()));
            }
        });
    *unit != before
}

/// The menu beside Resolution: pixels per inch or per centimetre. Whether it changed.
pub(super) fn resolution_unit_menu(ui: &mut egui::Ui, unit: &mut ResolutionUnit) -> bool {
    let before = *unit;
    widgets::PopUp::from_id_salt("resolution_unit")
        .selected_text(tr(unit.name()))
        .width(110.0)
        .show_ui(ui, |ui| {
            for choice in ResolutionUnit::ALL {
                widgets::menu_choice(ui, unit, choice, tr(choice.name()));
            }
        });
    *unit != before
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(unit: Unit, original: [u32; 2]) -> SizeUnits {
        let mut units = SizeUnits::default();
        units.open(original, unit, ResolutionUnit::PerInch);
        units
    }

    #[test]
    fn fields_show_and_set_pixels_in_any_unit() {
        let mm = units(Unit::Millimeters, [100, 100]);
        assert_eq!(mm.pixels(210.0, 0, 300.0), Some(2480));
        assert_eq!(mm.pixels(297.0, 1, 300.0), Some(3508));
        assert!((mm.shown(2480, 0, 300.0) - 209.97).abs() < 0.01);
        let inches = units(Unit::Inches, [1, 1]);
        assert_eq!(inches.pixels(10.0, 0, 300.0), Some(3000));
        assert_eq!(inches.shown(3000, 0, 150.0), 20.0);
        // Percent is of the size the dialog opened with, per side.
        let percent = units(Unit::Percent, [400, 300]);
        assert_eq!(percent.shown(200, 0, 72.0), 50.0);
        assert_eq!(percent.pixels(50.0, 1, 72.0), Some(150));
        // A print unit without a usable resolution sets nothing.
        assert_eq!(inches.pixels(10.0, 0, 0.0), None);
        assert_eq!(inches.pixels(10.0, 0, f32::NAN), None);
        assert_eq!(inches.shown(3000, 0, 0.0), 0.0);
        assert_eq!(inches.step(0, 0.0), 1.0);
        // Values beyond the limits are bounded.
        assert_eq!(inches.pixels(1e12, 0, 300.0), Some(MAX_SIDE));
        assert_eq!(inches.pixels(-3.0, 0, 300.0), Some(1));
    }

    #[test]
    fn relative_sizes_add_to_the_current_size() {
        let mut cm = units(Unit::Centimeters, [1000, 800]);
        cm.relative = true;
        assert_eq!(cm.shown(1000, 0, 254.0), 0.0);
        // 1 cm at 254 ppi is 100 px.
        assert_eq!(cm.pixels(1.0, 0, 254.0), Some(1100));
        assert_eq!(cm.pixels(-2.0, 1, 254.0), Some(600));
        assert_eq!(cm.pixels(-100.0, 1, 254.0), Some(1));
        let range = cm.range(0, 254.0);
        assert!(*range.start() < 0.0 && *range.end() > 0.0);
        let mut percent = units(Unit::Percent, [1000, 800]);
        percent.relative = true;
        assert_eq!(percent.pixels(10.0, 0, 72.0), Some(1100));
        assert_eq!(percent.shown(400, 1, 72.0), -50.0);
    }

    #[test]
    fn switching_units_keeps_the_pixels() {
        let mut units = units(Unit::Pixels, [2481, 3507]);
        let mut dimensions = [2481, 3507];
        for unit in Unit::ALL.into_iter().cycle().take(50) {
            units.unit = unit;
            for (axis, side) in dimensions.iter_mut().enumerate() {
                let shown = units.shown(*side, axis, 299.0);
                *side = units.pixels(shown, axis, 299.0).unwrap();
            }
        }
        assert_eq!(dimensions, [2481, 3507]);
    }

    #[test]
    fn without_resample_the_print_size_sets_the_resolution() {
        let inches = units(Unit::Inches, [3000, 2000]);
        assert_eq!(inches.resolution_for(20.0, 0), Some(150.0));
        assert_eq!(inches.resolution_for(10.0, 1), Some(200.0));
        // Too small or too large a print size needs a resolution no document may have.
        assert_eq!(inches.resolution_for(0.0, 0), None);
        assert_eq!(inches.resolution_for(-1.0, 0), None);
        assert_eq!(inches.resolution_for(1e-6, 0), None);
        assert_eq!(inches.resolution_for(f64::NAN, 0), None);
        assert_eq!(
            units(Unit::Pixels, [3000, 2000]).resolution_for(20.0, 0),
            None
        );
        assert_eq!(
            units(Unit::Percent, [3000, 2000]).resolution_for(20.0, 0),
            None
        );
    }

    #[test]
    fn a_new_resolution_keeps_the_print_size_without_drifting() {
        let mut units = units(Unit::Inches, [3000, 2000]);
        let mut dimensions = [3000, 2000];
        // Dragging the resolution through many values and back.
        let mut ppi = 300.0;
        for next in (301..=377).chain((150..377).rev()).chain(151..=300) {
            units.resolution_changed(&mut dimensions, ppi, next as f32, true);
            ppi = next as f32;
        }
        assert_eq!(dimensions, [3000, 2000]);
        units.resolution_changed(&mut dimensions, 300.0, 150.0, true);
        assert_eq!(dimensions, [1500, 1000]);
        // Editing the size starts again from it.
        units.size_edited();
        dimensions = [1501, 1000];
        units.resolution_changed(&mut dimensions, 150.0, 300.0, true);
        assert_eq!(dimensions, [3002, 2000]);
        // Pixels, Canvas Size and Image Size without Resample keep the pixels.
        units.resolution_changed(&mut dimensions, 300.0, 72.0, false);
        assert_eq!(dimensions, [3002, 2000]);
        units.unit = Unit::Pixels;
        units.resolution_changed(&mut dimensions, 300.0, 72.0, true);
        assert_eq!(dimensions, [3002, 2000]);
        // Bad resolutions change nothing.
        units.unit = Unit::Inches;
        units.size_edited();
        for (before, after) in [(0.0, 72.0), (72.0, f32::NAN), (-1.0, 300.0)] {
            units.resolution_changed(&mut dimensions, before, after, true);
            assert_eq!(dimensions, [3002, 2000]);
        }
        // A large print size is bounded.
        units.resolution_changed(&mut dimensions, 1.0, 9600.0, true);
        assert_eq!(dimensions, [MAX_SIDE, MAX_SIDE]);
    }

    #[test]
    fn a_preset_keeps_its_exact_print_size() {
        // A4 at 300 ppi rounds to 2480 × 3508 px; changes of resolution work from 210 × 297 mm
        // itself rather than from those pixels.
        let mut units = units(Unit::Millimeters, [1, 1]);
        let mut dimensions = [2480, 3508];
        units.keep_print_size([210.0 / 25.4, 297.0 / 25.4]);
        units.resolution_changed(&mut dimensions, 300.0, 150.0, true);
        assert_eq!(dimensions, [1240, 1754]);
        units.resolution_changed(&mut dimensions, 150.0, 72.0, true);
        assert_eq!(dimensions, [595, 842]);
        units.resolution_changed(&mut dimensions, 72.0, 300.0, true);
        assert_eq!(dimensions, [2480, 3508]);
        // Editing the size forgets it.
        units.size_edited();
        dimensions = [2480, 3509];
        units.resolution_changed(&mut dimensions, 300.0, 150.0, true);
        assert_eq!(dimensions, [1240, 1755]);
    }

    #[test]
    fn typed_sizes_parse_in_the_field_unit() {
        assert_eq!(
            parse_size("10cm", Unit::Pixels, 300.0, 0.0, false).and_then(units::whole_pixels),
            Some(1181)
        );
        assert_eq!(parse_size("2", Unit::Inches, 300.0, 0.0, false), Some(2.0));
        assert_eq!(parse_size("0", Unit::Pixels, 300.0, 0.0, false), None);
        assert_eq!(parse_size("-4", Unit::Pixels, 300.0, 0.0, false), None);
        assert_eq!(parse_size("-4", Unit::Pixels, 300.0, 0.0, true), Some(-4.0));
        assert_eq!(parse_size("abc", Unit::Pixels, 300.0, 0.0, false), None);
        assert_eq!(parse_size("10cm", Unit::Pixels, 0.0, 0.0, false), None);
    }

    #[test]
    fn summary_shows_pixels_megapixels_and_memory() {
        assert_eq!(summary([2480, 3508], false), "2480 × 3508 px · 8.7 MP");
        assert_eq!(summary([6000, 4000], false), "6000 × 4000 px · 24 MP");
        assert_eq!(
            summary([1000, 1000], true),
            "1000 × 1000 px · 1.0 MP · 4 MiB"
        );
    }
}
