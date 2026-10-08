//! Color Lookup adjustments: choosing a `.cube` file, and the controls of the adjustment
//! dialog. Reading and applying the table is [`xuan::lut`].

use std::{
    path::{Path, PathBuf},
    sync::Arc,
};

use egui::RichText;
use xuan::{
    document::Adjustment,
    i18n::tr,
    lut::{Dimension, Interpolation, Lut},
};

use super::{EditorApp, theme::PaletteExt as _, widgets};

/// The file dialog's filter; portal filters may be case-sensitive.
const EXTENSIONS: [&str; 2] = ["cube", "CUBE"];

fn pick() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .add_filter(tr("Colour lookup tables"), &EXTENSIONS)
        .pick_file()
}

/// A Color Lookup adjustment for the table in `path`, named after the file.
pub(super) fn read(path: &Path) -> anyhow::Result<Adjustment> {
    let table = Lut::load(path)?;
    let name = path
        .file_name()
        .map_or_else(|| "table.cube".into(), |n| n.to_string_lossy().into_owned());
    Ok(Adjustment::color_lookup(name, table))
}

impl EditorApp {
    /// Image → Adjustments → Color Lookup… and Layer → New Adjustment Layer → Color Lookup…:
    /// choose a `.cube` file, then adjust as with any other adjustment.
    pub(super) fn start_color_lookup(&mut self, as_layer: bool) {
        if self.session().is_some()
            && let Some(path) = pick()
        {
            self.load_color_lookup(&path, as_layer);
        }
    }

    /// Open the adjustment dialog for the table in `path`. A file that is not a valid table is
    /// reported, and the document is left as it was.
    pub(super) fn load_color_lookup(&mut self, path: &Path, as_layer: bool) -> bool {
        match read(path) {
            Ok(adjustment) => {
                self.start_adjustment(adjustment, as_layer);
                true
            }
            Err(error) => {
                self.error = Some(format!(
                    "{}\n\n{error:#}",
                    tr("Could not load the colour lookup table")
                ));
                false
            }
        }
    }
}

/// The adjustment dialog's controls: which table, how to blend its entries, and a button to
/// load another. Returns whether anything changed.
pub(super) fn controls(
    ui: &mut egui::Ui,
    name: &mut String,
    interpolation: &mut Interpolation,
    table: &mut Arc<Lut>,
) -> bool {
    let mut changed = false;
    ui.label(RichText::new(name.as_str()).strong());
    let details = match table.dimension {
        Dimension::One => tr("1D table, {} entries"),
        Dimension::Three => tr("3D table, {} points a side"),
    }
    .replace("{}", &table.size.to_string());
    let details = if table.title.is_empty() {
        details
    } else {
        format!("{} · {details}", table.title)
    };
    ui.label(RichText::new(details).small().color(ui.palette().muted));
    ui.add_space(4.0);
    // A 1D table has one curve per channel, so there is nothing to choose.
    ui.add_enabled_ui(table.dimension == Dimension::Three, |ui| {
        ui.horizontal(|ui| {
            ui.label(tr("Interpolation"));
            let options = Interpolation::ALL.map(|i| (i, tr(i.name())));
            changed |= widgets::segmented(ui, interpolation, &options).changed();
        });
    });
    ui.add_space(4.0);
    let error_id = ui.id().with("color_lookup_error");
    if widgets::button(ui, tr("Load Table…")).clicked()
        && let Some(path) = pick()
    {
        match read(&path) {
            Ok(Adjustment::ColorLookup {
                name: new_name,
                table: new_table,
                ..
            }) => {
                *name = new_name;
                *table = new_table;
                changed = true;
                ui.data_mut(|d| d.remove::<String>(error_id));
            }
            Ok(_) => {}
            Err(error) => {
                let message = format!(
                    "{}: {error:#}",
                    tr("Could not load the colour lookup table")
                );
                ui.data_mut(|d| d.insert_temp(error_id, message));
            }
        }
    }
    if let Some(error) = ui.data(|d| d.get_temp::<String>(error_id)) {
        ui.label(RichText::new(error).color(ui.palette().error));
    }
    changed
}
