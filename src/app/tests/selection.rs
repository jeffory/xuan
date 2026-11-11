//! Select → Expand…, Contract…, Layer's Pixels, Mask's Black Areas, Color Range… and
//! Subject, and the Remove Background commands.
use super::*;

pub(super) fn wait_for_job(app: &mut EditorApp) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
    while app.job.is_some() {
        app.poll_job();
        assert!(std::time::Instant::now() < deadline, "job did not finish");
        std::thread::yield_now();
    }
}

fn selected(app: &EditorApp) -> Vec<u8> {
    app.session()
        .unwrap()
        .document
        .selection
        .as_ref()
        .map(|s| s.as_raw().clone())
        .unwrap_or_default()
}

fn enabled(app: &EditorApp, id: &str) -> bool {
    (commands::find(id).unwrap().enabled)(app)
}

#[test]
fn expand_and_contract_dialogs_modify_the_selection_as_one_step() {
    let (context, mut app) = app();
    app.dimensions = [9, 9];
    app.new_document();
    // Nothing selected: the commands are unavailable.
    assert!(!enabled(&app, "expand_selection"));
    app.edit_selection("Pixel", |doc| {
        let mut mask = GrayImage::new(9, 9);
        mask.put_pixel(4, 4, image::Luma([255]));
        doc.selection = Some(Arc::new(mask));
    });
    assert!(enabled(&app, "expand_selection"));
    let revision = app.session().unwrap().history.revision;
    app.command("expand_selection");
    assert_eq!(app.dialog, Some(Dialog::SelectionAmount));
    app.selection_amount.as_mut().unwrap().amount = 2;
    // Enter applies the dialog.
    context.begin_pass(egui::RawInput {
        events: vec![egui::Event::Key {
            key: egui::Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
    app.selection_amount_dialog(&context);
    let _ = context.end_pass();
    assert_eq!(app.dialog, None);
    wait_for_job(&mut app);
    assert_eq!(app.session().unwrap().history.revision, revision + 1);
    let disc = selected(&app);
    assert_eq!(disc.iter().filter(|v| **v == 255).count(), 13);
    assert_eq!(app.expand_amount, 2);
    // Contract by 2 brings the disc back to its centre.
    app.modify_selection(selection_dialogs::AmountOperation::Contract, 2);
    wait_for_job(&mut app);
    let pixel = selected(&app);
    assert_eq!(pixel.iter().filter(|v| **v == 255).count(), 1);
    assert_eq!(pixel[4 * 9 + 4], 255);
    app.command("undo");
    assert_eq!(selected(&app), disc);
}

#[test]
fn layer_pixels_and_mask_black_areas_load_as_selections() {
    let (_, mut app) = app();
    app.dimensions = [4, 1];
    app.new_document();
    let mut layer = Layer::image(
        "Alpha",
        RgbaImage::from_fn(4, 1, |x, _| {
            image::Rgba([0, 0, 0, [0, 255, 255, 64][x as usize]])
        }),
    );
    let mut mask = xuan::document::Mask::white();
    mask.pixels = Arc::new(GrayImage::from_raw(4, 1, vec![255, 0, 255, 255]).unwrap());
    layer.mask = Some(mask);
    app.session_mut().unwrap().document.insert(layer);
    assert!(enabled(&app, "select_layer_pixels"));
    assert!(enabled(&app, "select_mask_black"));
    app.command("select_layer_pixels");
    assert_eq!(selected(&app), [0, 255, 255, 64]);
    app.command("select_mask_black");
    assert_eq!(selected(&app), [0, 255, 0, 0]);
    app.command("undo");
    assert_eq!(selected(&app), [0, 255, 255, 64]);
    app.session_mut()
        .unwrap()
        .document
        .active_mut()
        .unwrap()
        .mask = None;
    assert!(!enabled(&app, "select_mask_black"));
}

/// A 96x72 document: an orange disc on a mottled green background.
pub(super) fn subject_document(app: &mut EditorApp) {
    app.dimensions = [96, 72];
    app.new_document();
    let pixels = RgbaImage::from_fn(96, 72, |x, y| {
        let n = ((x * 7 + y * 13) % 5) as u8 * 6;
        image::Rgba(if in_subject(x, y) {
            [230, 140 + n, 40, 255]
        } else {
            [60 + n, 120, 60 + n, 255]
        })
    });
    let document = &mut app.session_mut().unwrap().document;
    document.layers[0].pixels = Some(Arc::new(pixels));
}

pub(super) fn in_subject(x: u32, y: u32) -> bool {
    (x as f32 + 0.5 - 48.0).powi(2) + (y as f32 + 0.5 - 36.0).powi(2) <= 20.0f32.powi(2)
}

/// Pixels whose selection (≥ 128) disagrees with the disc.
pub(super) fn wrong_pixels(mask: &[u8]) -> usize {
    (0..96 * 72)
        .filter(|&i| (mask[i] >= 128) != in_subject(i as u32 % 96, i as u32 / 96))
        .count()
}

#[test]
fn select_subject_and_remove_background_use_the_graph_cut() {
    let (_, mut app) = app();
    subject_document(&mut app);
    app.command("select_subject");
    assert!(app.job.is_some());
    wait_for_job(&mut app);
    assert_eq!(app.status, tr("Select Subject"));
    let wrong = wrong_pixels(&selected(&app));
    assert!(wrong < 30, "{wrong} pixels wrong");
    app.command("deselect");
    app.command("remove_background");
    wait_for_job(&mut app);
    assert_eq!(app.error, None);
    let session = app.session().unwrap();
    // The mask becomes a mask layer attached to the image.
    let image = session.document.layers[0].id;
    let child = session
        .document
        .layers
        .iter()
        .find(|l| l.parent == Some(image));
    let mask = &child.unwrap().mask.as_ref().unwrap().pixels;
    assert!(wrong_pixels(mask.as_raw()) < 30);
    assert_eq!(session.history.undo_name(), Some(tr("Remove Background")));
    // The flat-colour matte is still offered.
    app.command("undo");
    assert_eq!(app.session().unwrap().document.layers.len(), 1);
    app.command("remove_flat_background");
    wait_for_job(&mut app);
    assert_eq!(app.session().unwrap().document.layers.len(), 2);
}

/// Two red discs on green, left at x = 30 and right at x = 90, both at y = 36.
fn two_objects(app: &mut EditorApp) {
    app.dimensions = [120, 72];
    app.new_document();
    let pixels = RgbaImage::from_fn(120, 72, |x, y| {
        let n = ((x * 7 + y * 13) % 5) as u8 * 5;
        image::Rgba(if in_object(x, y, 30.0) || in_object(x, y, 90.0) {
            [220, 40 + n, 40, 255]
        } else {
            [40 + n, 150, 60, 255]
        })
    });
    app.session_mut().unwrap().document.layers[0].pixels = Some(Arc::new(pixels));
}

fn in_object(x: u32, y: u32, cx: f32) -> bool {
    (x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - 36.0).powi(2) <= 16.0f32.powi(2)
}

fn object_errors(app: &EditorApp, inside: impl Fn(u32, u32) -> bool) -> usize {
    let mask = selected(app);
    (0..120 * 72)
        .filter(|&i| (mask[i] >= 128) != inside(i as u32 % 120, i as u32 / 120))
        .count()
}

#[test]
fn object_mode_selects_the_clicked_or_boxed_object() {
    let (context, mut app) = app();
    two_objects(&mut app);
    app.set_tool(Tool::Wand);
    app.wand_object = true;
    let session = app.session_mut().unwrap();
    session.zoom = 4.0;
    session.fit = false;
    click_canvas(
        &context,
        &mut app,
        Point::new(90.5, 36.5),
        egui::Modifiers::NONE,
    );
    wait_for_job(&mut app);
    assert!(object_errors(&app, |x, y| in_object(x, y, 90.0)) < 20);
    assert_eq!(app.status, tr("Object Selection"));
    // Shift-dragging a box around the other adds it.
    drag(
        &context,
        &mut app,
        Point::new(8.0, 12.0),
        Point::new(54.0, 60.0),
        egui::Modifiers::SHIFT,
    );
    wait_for_job(&mut app);
    let both = |x, y| in_object(x, y, 30.0) || in_object(x, y, 90.0);
    assert!(object_errors(&app, both) < 40);
    app.command("undo");
    assert!(object_errors(&app, |x, y| in_object(x, y, 90.0)) < 20);
}
