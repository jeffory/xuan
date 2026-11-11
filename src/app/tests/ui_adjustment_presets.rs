//! Layer → Adjustment Presets…: saving the selected adjustment layers under a name, applying
//! them to another document as one undo step, and deleting a preset.
use super::*;
use xuan::{
    blend::BlendMode,
    document::Adjustment,
    lut::{Dimension, Lut},
};

/// The adjustments the test stacks up, bottom first.
fn adjustments() -> [Adjustment; 3] {
    let mut table = Lut::identity(Dimension::Three, 4);
    for entry in &mut table.table {
        entry.rotate_left(1);
    }
    [
        Adjustment::ColorBalance {
            shadows: [10.0, 0.0, -10.0],
            midtones: [25.0, 5.0, -20.0],
            highlights: [0.0, 0.0, 5.0],
            preserve_luminosity: true,
        },
        Adjustment::color_lookup("rotate.cube", table),
        Adjustment::Exposure {
            exposure: 0.5,
            offset: 0.0,
            gamma: 1.2,
        },
    ]
}

/// An editor with a document holding an image and the three adjustment layers, all selected.
fn graded() -> UiTest {
    let mut ui = UiTest::with_document();
    let session = ui.app_mut().session_mut().unwrap();
    for (index, adjustment) in adjustments().into_iter().enumerate() {
        let mut layer = Layer::blank(adjustment.name(), 20, 16);
        layer.adjustment = Some(adjustment);
        layer.opacity = 0.5 + index as f32 * 0.2;
        if index == 2 {
            layer.blend = BlendMode::Luminosity;
        }
        session.document.layers.push(layer);
    }
    let document = &mut session.document;
    document.selected = document.layers[1..].iter().map(|l| l.id).collect();
    document.active = document.layers.last().map(|l| l.id);
    ui.settle();
    ui
}

/// The adjustment layers of the current document: name, opacity, blend and settings.
fn stack(ui: &UiTest) -> Vec<(String, f32, BlendMode, Adjustment)> {
    let document = &ui.app().session().unwrap().document;
    document
        .layers
        .iter()
        .filter_map(|l| Some((l.name.clone(), l.opacity, l.blend, l.adjustment.clone()?)))
        .collect()
}

fn open_presets(ui: &mut UiTest) {
    ui.open_menu("Layer");
    ui.click("Adjustment Presets…");
    assert!(ui.app().dialog == Some(Dialog::AdjustmentPresets));
}

fn name_preset(ui: &mut UiTest, name: &str) {
    ui.click_role(Role::TextInput, "Name");
    ui.press(egui::Modifiers::COMMAND, egui::Key::A);
    ui.type_keys(name);
}

#[test]
fn a_saved_preset_applies_matching_layers_to_another_document() {
    let mut ui = graded();
    let saved = stack(&ui);
    open_presets(&mut ui);
    assert!(ui.has("No presets yet. Select adjustment layers and save them below."));
    assert!(ui.has("Save the 3 selected adjustment layers as a preset."));
    name_preset(&mut ui, "Golden hour");
    ui.click("Save Preset");
    assert_eq!(ui.app().adjustment_presets.presets.len(), 1);
    assert!(ui.has("Golden hour"));
    ui.key(egui::Key::Escape);
    assert!(ui.app().dialog.is_none());

    // A new document of another size gets the same stack above its layer, in one undo step.
    ui.app_mut().dimensions = [40, 30];
    ui.app_mut().new_document();
    ui.settle();
    let before = ui.app().session().unwrap().document.layers.len();
    open_presets(&mut ui);
    ui.click("Golden hour");
    ui.click("Apply");
    assert_eq!(stack(&ui), saved);
    let document = &ui.app().session().unwrap().document;
    assert_eq!(document.layers.len(), before + 3);
    assert_eq!(document.layers[before].transform.width, 40.0);
    assert_eq!(document.selected.len(), 3);

    // Changing an applied layer leaves the preset as it was.
    let session = ui.app_mut().session_mut().unwrap();
    let applied = session.document.layers.last_mut().unwrap();
    applied.opacity = 0.05;
    applied.adjustment = Some(Adjustment::Invert);
    let preset = &ui.app().adjustment_presets.presets[0].preset;
    assert_eq!(preset.layers[2].opacity, saved[2].1);
    assert_eq!(preset.layers[2].adjustment, saved[2].3);

    ui.key(egui::Key::Escape);
    ui.press(egui::Modifiers::CTRL, egui::Key::Z);
    assert_eq!(ui.app().session().unwrap().document.layers.len(), before);
}

#[test]
fn saving_needs_adjustment_layers_and_a_name_and_replaces_by_name() {
    let mut ui = graded();
    let image = ui.app().session().unwrap().document.layers[0].id;
    ui.app_mut().session_mut().unwrap().document.selected = [image].into();
    open_presets(&mut ui);
    assert!(ui.has("Select adjustment layers to save them as a preset."));
    assert!(!ui.enabled("Save Preset"));
    ui.key(egui::Key::Escape);

    let mut ui = graded();
    open_presets(&mut ui);
    name_preset(&mut ui, "   ");
    assert!(!ui.enabled("Save Preset"));
    name_preset(&mut ui, "Look");
    ui.click("Save Preset");
    // Saving again under the same name replaces it, now with one layer.
    let top = ui.app().session().unwrap().document.layers[3].id;
    ui.app_mut().session_mut().unwrap().document.selected = [top].into();
    ui.settle();
    assert!(ui.has("Replaces the preset of this name."));
    ui.click("Save Preset");
    let presets = &ui.app().adjustment_presets.presets;
    assert_eq!(presets.len(), 1);
    assert_eq!(presets[0].preset.layers.len(), 1);

    ui.click("Delete");
    assert!(ui.app().adjustment_presets.presets.is_empty());
    assert!(!ui.enabled("Apply"));
}
