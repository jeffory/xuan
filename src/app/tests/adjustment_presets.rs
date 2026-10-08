//! Adjustment presets kept beside the configuration file, and the issue's round trip: a preset
//! with a Color Lookup layer applied to another document survives saving and reopening it.
use super::*;
use xuan::{
    adjustment_presets::FOLDER,
    document::Adjustment,
    lut::{Dimension, Lut},
};

#[test]
fn presets_outlive_the_app_and_their_layers_survive_a_saved_project() {
    let directory = tempfile::tempdir().unwrap();
    let config = directory.path().join("config.toml");
    let (_, mut editor) = app_with_document();
    editor.config_path = Some(config.clone());
    editor.load_adjustment_presets();
    assert!(editor.adjustment_presets.presets.is_empty());

    let cube = directory.path().join("warm.cube");
    let mut table = Lut::identity(Dimension::Three, 9);
    for entry in &mut table.table {
        entry[0] = (entry[0] * 1.1).min(1.0);
    }
    std::fs::write(&cube, table.to_cube()).unwrap();
    let mut layer = Layer::blank("Color Lookup", 20, 16);
    layer.adjustment = Some(super::color_lookup::read(&cube).unwrap());
    layer.opacity = 0.5;
    editor.session_mut().unwrap().document.insert(layer);
    editor.save_adjustment_preset("Warm").unwrap();
    assert!(directory.path().join(FOLDER).join("warm.json").is_file());

    // A later session finds it and adds it to a new document.
    let (_, mut later) = app_with_document();
    later.config_path = Some(config);
    later.load_adjustment_presets();
    assert_eq!(later.adjustment_presets.presets.len(), 1);
    later.apply_adjustment_preset(0);
    let project = directory.path().join("graded.xuan");
    let session = &mut later.sessions[later.current];
    session
        .save_project(&project, &mut later.file_watch)
        .unwrap();

    // The reopened project keeps the layer and its table, without the .cube file.
    std::fs::remove_file(&cube).unwrap();
    let (_, mut reopened) = app();
    assert!(reopened.open_path(&project, false));
    let document = &reopened.session().unwrap().document;
    let layer = document.layers.last().unwrap();
    assert_eq!(layer.opacity, 0.5);
    let Some(Adjustment::ColorLookup {
        name, table: kept, ..
    }) = &layer.adjustment
    else {
        panic!("{:?}", layer.adjustment);
    };
    assert_eq!(name, "warm.cube");
    assert_eq!(kept.as_ref(), &table);
}
