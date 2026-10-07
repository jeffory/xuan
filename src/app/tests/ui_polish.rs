//! Small consistency fixes from the design review (#80).

use super::*;

fn left_of_each(ui: &UiTest, label: &str) -> Vec<(f32, f32)> {
    let mut found: Vec<(f32, f32)> = ui
        .harness
        .get_all_by_role_and_label(Role::Button, label)
        .map(|node| (node.rect().top(), node.rect().left()))
        .collect();
    found.sort_by(|a, b| a.0.total_cmp(&b.0));
    found
}

#[test]
fn zoom_out_comes_before_zoom_in_in_the_tab_bar_and_the_navigator() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    ui.isolate_config(directory.path());
    ui.harness.set_size(Vec2::new(1280.0, 860.0));
    ui.harness.run_steps(5);
    let minus = left_of_each(&ui, "−");
    let plus = left_of_each(&ui, "+");
    // One pair in the tab bar (higher up) and one in the Navigator.
    assert_eq!(minus.len(), 2, "{minus:?}");
    assert_eq!(plus.len(), 2, "{plus:?}");
    for (out, zoom_in) in minus.iter().zip(&plus) {
        assert!(
            (out.0 - zoom_in.0).abs() < 8.0,
            "same row: {out:?} {zoom_in:?}"
        );
        assert!(
            out.1 < zoom_in.1,
            "− at {out:?} should be left of + at {zoom_in:?}"
        );
    }
}

#[test]
fn opening_a_live_preview_dialog_does_not_mark_the_document_edited() {
    use xuan::effects::Filter;
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    ui.isolate_config(directory.path());
    for command in [
        "levels",
        "hue",
        "curves",
        "blur",
        "layer_effects",
        "text",
        "color_range",
    ] {
        match command {
            "blur" => ui
                .app_mut()
                .start_filter(Filter::GaussianBlur { radius: 4.0 }),
            "text" => ui.app_mut().start_text(None, Point::default()),
            _ => ui.app_mut().command(command),
        }
        ui.settle();
        assert!(
            !ui.app().session().unwrap().history.edited(),
            "{command} marked the document edited when it opened"
        );
        ui.click("Cancel");
        ui.settle();
    }
}

#[test]
fn a_committed_edit_marks_the_document_edited() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    ui.isolate_config(directory.path());
    ui.app_mut().command("fill_fg");
    ui.settle();
    assert!(ui.app().session().unwrap().history.edited());
}

#[test]
fn british_spelling_is_used_for_colour_labels() {
    let directory = tempfile::tempdir().unwrap();
    let mut ui = UiTest::with_document();
    ui.isolate_config(directory.path());
    assert!(ui.has("Background colour"));
    assert!(!ui.has("Background color"));
    assert_eq!(xuan::i18n::tr("Color Overlay"), "Colour Overlay");
    assert_eq!(xuan::i18n::tr("Color Dodge"), "Colour Dodge");
}
