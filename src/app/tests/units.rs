//! Physical units and print resolution (issue 94): resolutions read on open, and the size
//! dialogs' units.
use super::*;

fn resolution(app: &EditorApp) -> f32 {
    app.session().unwrap().document.resolution
}

#[test]
fn opening_an_image_keeps_the_resolution_its_file_states() {
    let dir = tempfile::tempdir().unwrap();
    let mut source = Document::new(4, 3).unwrap();
    source.resolution = 300.0;
    for extension in ["png", "jpg", "tif"] {
        let path = dir.path().join(format!("print.{extension}"));
        io::export(&source, &path, &io::ExportOptions::default()).unwrap();
        let (_, mut app) = app();
        assert!(app.open_path(&path, false), "{extension}: {:?}", app.error);
        assert_eq!(resolution(&app), 300.0, "{extension}");
    }
    // A file that states none opens at 72 ppi.
    let plain = dir.path().join("plain.png");
    RgbaImage::new(4, 3).save(&plain).unwrap();
    let (_, mut app) = app();
    assert!(app.open_path(&plain, false));
    assert_eq!(resolution(&app), xuan::units::DEFAULT_RESOLUTION);
    // Importing as a layer leaves the document's own resolution alone.
    app.session_mut().unwrap().document.resolution = 150.0;
    assert!(app.open_path(&dir.path().join("print.png"), true));
    assert_eq!(resolution(&app), 150.0);
}
