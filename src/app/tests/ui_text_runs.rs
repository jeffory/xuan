//! Per-letter fonts and colours in the Text window (issue 110): selecting letters in the text
//! field and changing the colour, bold or italic changes just those letters; with nothing
//! selected the change is the whole layer's.

use super::*;
use xuan::text::{RunStyle, TextRun};

/// An editor with a document and the Text window open on new text reading `content`.
fn text_window(content: &str) -> UiTest {
    let mut ui = UiTest::with_document();
    ui.key(egui::Key::T);
    ui.click("Add text…");
    assert!(ui.app().dialog == Some(Dialog::Text));
    // The new text is selected, so typing replaces it.
    ui.type_keys(content);
    assert_eq!(ui.app().text_edit.as_ref().unwrap().style.content, content);
    ui
}

/// Selects `count` letters from the start of the text field's first line.
fn select_from_start(ui: &mut UiTest, count: usize) {
    ui.key(egui::Key::Home);
    for _ in 0..count {
        ui.press(egui::Modifiers::SHIFT, egui::Key::ArrowRight);
    }
}

/// Clicks into the text field near the start of its first line, as the pointer would.
fn focus_text_field(ui: &mut UiTest) {
    let field = ui.harness.get_by_role(Role::MultilineTextInput).rect();
    ui.click_at(field.left_top() + egui::vec2(3.0, 8.0));
}

fn style(ui: &UiTest) -> &xuan::text::TextStyle {
    &ui.app().text_edit.as_ref().unwrap().style
}

/// The layer on the canvas as the Text window is previewing it.
fn previewed(ui: &mut UiTest) -> (xuan::text::TextStyle, RgbaImage) {
    let layer = ui
        .app()
        .session()
        .unwrap()
        .document
        .active()
        .unwrap()
        .clone();
    (
        layer.text.unwrap(),
        (**layer.pixels.as_ref().unwrap()).clone(),
    )
}

#[test]
fn selected_letters_take_their_own_colour_and_weight() {
    let mut ui = text_window("APPle");
    select_from_start(&mut ui, 3);
    assert_eq!(ui.app().text_edit.as_ref().unwrap().selection, Some(0..3));
    assert!(ui.has("Font, colour, bold and italic change the 3 selected letters"));

    // Bold applies to the selection only, even though the checkbox took the focus.
    ui.click("Bold");
    let bold = RunStyle {
        bold: Some(true),
        ..Default::default()
    };
    assert_eq!(
        style(&ui).runs,
        vec![TextRun {
            start: 0,
            end: 3,
            style: bold.clone()
        }]
    );
    assert!(!style(&ui).bold);

    // So does a colour picked from the colour well's map: its top-right corner is pure red.
    ui.harness
        .get_by_role_and_label(Role::ColorWell, "Colour")
        .click();
    ui.settle();
    let map = ui
        .harness
        .output()
        .shapes
        .iter()
        .find_map(|shape| match &shape.shape {
            egui::Shape::Mesh(mesh) if mesh.vertices.len() > 1000 => Some(mesh.calc_bounds()),
            _ => None,
        })
        .expect("saturation/value map");
    ui.click_at(map.right_top() + egui::vec2(-1.0, 1.0));
    // Clicking elsewhere in the window closes the picker.
    ui.click("Size");
    assert!(ui.app().dialog == Some(Dialog::Text));
    let runs = &style(&ui).runs;
    assert_eq!(runs.len(), 1, "{runs:?}");
    assert_eq!((runs[0].start, runs[0].end), (0, 3));
    assert_eq!(runs[0].style.bold, Some(true));
    let red = runs[0].style.color.expect("a colour for the selection");
    assert!(red[0] > 240 && red[1] < 20 && red[2] < 20, "{red:?}");
    assert_eq!(style(&ui).color, [0, 0, 0, 255]);
    assert_eq!(style(&ui).letter_style(4).color, [0, 0, 0, 255]);

    // The canvas previews the letters in their styles.
    let (previewed, pixels) = previewed(&mut ui);
    assert_eq!(&previewed, style(&ui));
    let expected = ui
        .app_mut()
        .text_renderer
        .as_mut()
        .unwrap()
        .render(&previewed)
        .unwrap();
    assert_eq!(pixels, expected);

    // Typing after the selection's last letter continues its style.
    focus_text_field(&mut ui);
    ui.key(egui::Key::Home);
    for _ in 0..3 {
        ui.key(egui::Key::ArrowRight);
    }
    ui.type_keys("P");
    assert_eq!(style(&ui).content, "APPPle");
    assert_eq!((style(&ui).runs[0].start, style(&ui).runs[0].end), (0, 4));
    // Nothing is selected now, so Italic is the whole layer's.
    assert_eq!(ui.app().text_edit.as_ref().unwrap().selection, Some(4..4));
    assert!(!ui.has("Font, colour, bold and italic change the 3 selected letters"));
    ui.click("Italic");
    assert!(style(&ui).italic);
    assert_eq!(style(&ui).runs.len(), 1);

    // Applying keeps the runs on the layer as one undo step; new text starts plain.
    ui.press(egui::Modifiers::CTRL, egui::Key::Enter);
    assert!(ui.app().dialog.is_none());
    let session = ui.app().session().unwrap();
    let text = session.document.active().unwrap().text.clone().unwrap();
    assert_eq!(text.runs.len(), 1);
    assert!(text.italic);
    assert_eq!(session.history.names().count(), 1);
    assert!(ui.app().text_style.runs.is_empty());
    ui.press(egui::Modifiers::COMMAND, egui::Key::Z);
    assert_eq!(ui.app().session().unwrap().document.layers.len(), 1);
}

#[test]
fn selecting_every_letter_changes_the_layer_and_clears_runs() {
    let mut ui = text_window("APPle");
    select_from_start(&mut ui, 2);
    ui.click("Bold");
    assert_eq!(style(&ui).runs.len(), 1);
    // Select all and turn bold off: the layer's own style changes and the run goes away.
    focus_text_field(&mut ui);
    ui.press(egui::Modifiers::COMMAND, egui::Key::A);

    assert_eq!(ui.app().text_edit.as_ref().unwrap().selection, Some(0..5));
    // The checkbox shows the first selected letter's weight: bold.
    ui.click("Bold");
    assert!(!style(&ui).bold);
    assert!(style(&ui).runs.is_empty());
    ui.click("Bold");
    assert!(style(&ui).bold);
    assert!(style(&ui).runs.is_empty());
}
