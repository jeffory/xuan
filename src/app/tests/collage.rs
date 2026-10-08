//! File → New Collage…, Image → Collage Layout… and filling a collage's cells with images
//! inserted as layers.
use super::*;
use xuan::collage::{self, Layout, Template};

/// Three solid photos in a temporary folder: red 40 × 20, green 20 × 40 and blue 10 × 10.
fn photos() -> (tempfile::TempDir, Vec<PathBuf>) {
    let directory = tempfile::tempdir().unwrap();
    let paths = [
        ("red", 40, 20, [255, 0, 0, 255]),
        ("green", 20, 40, [0, 255, 0, 255]),
        ("blue", 10, 10, [0, 0, 255, 255]),
    ]
    .into_iter()
    .map(|(name, width, height, color)| {
        let path = directory.path().join(format!("{name}.png"));
        RgbaImage::from_pixel(width, height, image::Rgba(color))
            .save(&path)
            .unwrap();
        path
    })
    .collect();
    (directory, paths)
}

fn edit(photos: Vec<PathBuf>) -> collage_dialog::CollageEdit {
    collage_dialog::CollageEdit {
        layout: Layout::default(),
        size: [200, 160],
        photos,
        relayout: false,
    }
}

fn document(app: &EditorApp) -> &Document {
    &app.session().unwrap().document
}

fn pixel(app: &EditorApp, x: u32, y: u32) -> [u8; 4] {
    render::render(document(app)).get_pixel(x, y).0
}

fn enabled(app: &EditorApp, id: &str) -> bool {
    (commands::find(id).unwrap().enabled)(app)
}

#[test]
fn a_new_collage_fills_its_cells_with_the_chosen_photos_in_order() {
    let (_directory, paths) = photos();
    let (_, mut app) = app();
    assert!(enabled(&app, "new_collage"), "no document is needed");
    app.command("new_collage");
    assert_eq!(app.dialog, Some(Dialog::Collage));
    app.collage = None;
    app.create_collage(&edit(paths));
    assert!(app.error.is_none(), "{:?}", app.error);
    assert_eq!(app.dialog, None);
    assert_eq!(app.sessions.len(), 1);
    assert_eq!(app.session().unwrap().title, "Collage");
    let cells = collage::cells(document(&app));
    assert_eq!(cells.len(), 4);
    // The first cell is selected, ready for an image to go in.
    assert_eq!(document(&app).active, Some(cells[0]));
    // Cells of 70 × 50 at x 20 and 110, y 20 and 90; the fourth has no photo.
    assert_eq!(pixel(&app, 55, 45), [255, 0, 0, 255]);
    assert_eq!(pixel(&app, 145, 45), [0, 255, 0, 255]);
    assert_eq!(pixel(&app, 55, 115), [0, 0, 255, 255]);
    assert_eq!(pixel(&app, 145, 115), collage::EMPTY_CELL);
    assert_eq!(pixel(&app, 100, 80), [255, 255, 255, 255]);
    assert!(collage::is_empty(document(&app), cells[3]));
    // The next collage starts from this one's choices; nothing to undo in a new document.
    assert_eq!(app.collage_settings.size, [200, 160]);
    assert!(!app.session().unwrap().history.dirty());
    assert!(enabled(&app, "collage_layout"));
}

#[test]
fn photos_that_cannot_be_read_are_reported_and_leave_their_cell_empty() {
    let (directory, mut paths) = photos();
    let broken = directory.path().join("broken.png");
    std::fs::write(&broken, b"not a png").unwrap();
    paths.insert(1, broken);
    // More photos than cells: the last is left out.
    paths.push(paths[0].clone());
    paths.push(paths[0].clone());
    let (_, mut app) = app();
    app.create_collage(&edit(paths));
    let error = app.error.clone().expect("the broken photo is reported");
    assert!(error.contains("broken.png"), "{error}");
    let cells = collage::cells(document(&app));
    assert!(collage::is_empty(document(&app), cells[1]));
    assert!(!collage::is_empty(document(&app), cells[3]));
    assert_eq!(pixel(&app, 145, 45), collage::EMPTY_CELL);
}

#[test]
fn a_layout_without_room_makes_no_collage() {
    let (_, mut app) = app();
    let mut cramped = edit(Vec::new());
    cramped.layout.border = 100;
    app.create_collage(&cramped);
    assert!(app.sessions.is_empty());
    assert!(app.error.is_some());
}

#[test]
fn images_inserted_as_layers_fill_the_active_cell_then_the_empty_ones() {
    let (_directory, paths) = photos();
    let (_, mut app) = app();
    app.create_collage(&edit(Vec::new()));
    let cells = collage::cells(document(&app));
    // Start from the second cell's frame: the images fill it, then cells 3 and 4.
    let frame = collage::frame(document(&app), cells[1]).unwrap();
    app.sessions[0].document.select(frame, false);
    let before = app.session().unwrap().history.revision;
    app.insert_files(&paths);
    assert!(app.error.is_none(), "{:?}", app.error);
    assert_eq!(app.session().unwrap().history.revision, before + 3);
    assert!(collage::is_empty(document(&app), cells[0]));
    assert_eq!(pixel(&app, 145, 45), [255, 0, 0, 255]);
    assert_eq!(pixel(&app, 55, 115), [0, 255, 0, 255]);
    assert_eq!(pixel(&app, 145, 115), [0, 0, 255, 255]);
    assert_eq!(app.collage_filled, None, "the batch is over");

    // One image on its own replaces the photo of the active cell (the blue one's).
    app.insert_files(&paths[..1]);
    assert_eq!(pixel(&app, 145, 115), [255, 0, 0, 255]);
    let photos = document(&app)
        .layers
        .iter()
        .filter(|l| l.parent == Some(cells[3]))
        .count();
    assert_eq!(photos, 2, "frame and one photo");
    app.command("undo");
    assert_eq!(pixel(&app, 145, 115), [0, 0, 255, 255]);

    // Without a cell selected, an image is an ordinary layer.
    let background = document(&app).collage.as_ref().unwrap().background.unwrap();
    app.sessions[0].document.select(background, false);
    let count = document(&app).layers.len();
    app.insert_files(&paths[..1]);
    assert_eq!(document(&app).layers.len(), count + 1);
    assert_eq!(document(&app).active().unwrap().clip_to, None);
}

#[test]
fn dropped_images_inserted_as_layers_fill_cells() {
    let (_directory, paths) = photos();
    let (_, mut app) = app();
    app.create_collage(&edit(Vec::new()));
    app.queue_drop(paths[..2].to_vec());
    app.process_drops();
    assert_eq!(app.dialog, Some(Dialog::DropChoice));
    app.drop_choose_for_test(Some(true));
    assert_eq!(pixel(&app, 55, 45), [255, 0, 0, 255]);
    assert_eq!(pixel(&app, 145, 45), [0, 255, 0, 255]);
}

#[test]
fn collage_layout_lays_the_collage_out_again_in_one_undo_step() {
    let (_directory, paths) = photos();
    let (_, mut app) = app();
    app.dimensions = [20, 16];
    app.new_document();
    assert!(!enabled(&app, "collage_layout"), "not a collage");
    app.command("collage_layout");
    assert_eq!(app.dialog, None);

    app.create_collage(&edit(paths));
    app.command("collage_layout");
    assert_eq!(app.dialog, Some(Dialog::Collage));
    let opened = app.collage.take().unwrap();
    assert!(opened.relayout);
    assert_eq!(opened.layout, Layout::default());
    assert_eq!(opened.size, [200, 160]);
    let before = app.session().unwrap().history.revision;
    let layout = Layout {
        template: Template::LargeLeft,
        color: [0, 0, 0, 255],
        ..Layout::default()
    };
    app.apply_collage_layout(layout);
    assert!(app.error.is_none(), "{:?}", app.error);
    assert_eq!(app.dialog, None);
    assert_eq!(app.session().unwrap().history.revision, before + 1);
    assert_eq!(collage::cells(document(&app)).len(), 3);
    assert_eq!(pixel(&app, 100, 80), [0, 0, 0, 255]);
    assert_eq!(app.collage_settings.layout, layout);
    app.command("undo");
    assert_eq!(collage::cells(document(&app)).len(), 4);
    assert_eq!(pixel(&app, 100, 80), [255, 255, 255, 255]);

    // A layout that does not fit is refused and changes nothing.
    app.apply_collage_layout(Layout {
        border: 500,
        ..Layout::default()
    });
    assert!(app.error.is_some());
    assert_eq!(collage::cells(document(&app)).len(), 4);
}
