use super::*;
use crate::app::{
    canvas,
    navigator::{
        ThumbnailCache, fit_size, format_zoom_percent, pan_to_centre, parse_zoom_percent,
        slider_to_zoom, thumb_to_doc, viewport_box, visible_doc_rect, zoom_to_slider,
    },
};
use egui::{Rect, pos2, vec2};

const IMAGE: Vec2 = vec2(400.0, 200.0);
const THUMB: Rect = Rect::from_min_max(pos2(10.0, 20.0), pos2(110.0, 70.0));

fn approx(a: Rect, b: Rect) -> bool {
    (a.min - b.min).length() < 1e-3 && (a.max - b.max).length() < 1e-3
}

#[test]
fn fully_visible_image_fills_the_thumbnail() {
    let viewport = vec2(1000.0, 800.0);
    let rect = viewport_box(THUMB, viewport, IMAGE, 1.0, Vec2::ZERO).unwrap();
    assert!(approx(rect, THUMB));
    // Panned, but still entirely inside the viewport.
    let rect = viewport_box(THUMB, viewport, IMAGE, 1.0, vec2(100.0, -50.0)).unwrap();
    assert!(approx(rect, THUMB));
}

#[test]
fn zoomed_in_box_covers_the_visible_region() {
    // At 2x a 500x400 viewport shows 250x200 document pixels, centred.
    let rect = viewport_box(THUMB, vec2(500.0, 400.0), IMAGE, 2.0, Vec2::ZERO).unwrap();
    // Full height; x from 75 to 325 of 400, i.e. 18.75%..81.25% of the width.
    let expected = Rect::from_min_max(pos2(10.0 + 18.75, 20.0), pos2(10.0 + 81.25, 70.0));
    assert!(approx(rect, expected), "{rect:?}");
}

#[test]
fn panning_moves_the_box_opposite_to_the_image() {
    let viewport = vec2(500.0, 400.0);
    let centred = viewport_box(THUMB, viewport, IMAGE, 2.0, Vec2::ZERO).unwrap();
    // Dragging the image left by 100px shows 50 more document pixels on the right.
    let moved = viewport_box(THUMB, viewport, IMAGE, 2.0, vec2(-100.0, 0.0)).unwrap();
    assert!((moved.center().x - centred.center().x - 12.5).abs() < 1e-3);
}

#[test]
fn partially_visible_box_is_clipped_to_the_thumbnail() {
    // The image hangs off the left edge of the viewport.
    let rect = viewport_box(THUMB, vec2(300.0, 400.0), IMAGE, 1.0, vec2(200.0, 0.0)).unwrap();
    assert!((rect.left() - THUMB.left()).abs() < 1e-3);
    assert!(rect.right() < THUMB.right() && rect.right() > THUMB.left());
    assert!(THUMB.contains_rect(rect));
}

#[test]
fn no_box_when_the_image_is_off_screen() {
    assert!(viewport_box(THUMB, vec2(300.0, 300.0), IMAGE, 1.0, vec2(5000.0, 0.0)).is_none());
}

#[test]
fn visible_region_matches_the_canvas_mapping() {
    let viewport = Rect::from_min_size(pos2(50.0, 30.0), vec2(500.0, 400.0));
    let (zoom, pan) = (1.5, vec2(37.0, -12.0));
    let visible = visible_doc_rect(viewport.size(), IMAGE, zoom, pan);
    let origin = canvas::image_origin(viewport, IMAGE, zoom, pan);
    // The document point under the viewport's top-left corner.
    let corner = (viewport.min - origin) / zoom;
    assert!((visible.min.to_vec2() - corner).length() < 1e-3);
}

#[test]
fn click_centres_the_view_and_round_trips_with_the_box() {
    let viewport = vec2(500.0, 400.0);
    let zoom = 2.0;
    let click = pos2(70.0, 30.0);
    let pan = pan_to_centre(thumb_to_doc(THUMB, IMAGE, click), IMAGE, zoom);
    let rect = viewport_box(THUMB, viewport, IMAGE, zoom, pan).unwrap();
    assert!((rect.center().x - click.x).abs() < 1e-3);
    // Clicking the middle restores a zero pan.
    let middle = thumb_to_doc(THUMB, IMAGE, THUMB.center());
    assert!(pan_to_centre(middle, IMAGE, zoom).length() < 1e-3);
}

#[test]
fn clicks_outside_the_thumbnail_clamp_to_the_image() {
    let doc = thumb_to_doc(THUMB, IMAGE, pos2(-500.0, 900.0));
    assert_eq!(doc, pos2(0.0, IMAGE.y));
}

#[test]
fn thumbnail_fits_the_available_area() {
    let size = fit_size(IMAGE, vec2(100.0, 170.0));
    assert_eq!(size, vec2(100.0, 50.0));
}

#[test]
fn slider_round_trips_and_clamps() {
    for zoom in [0.01, 0.1, 0.558, 1.0, 7.3, 64.0] {
        let back = slider_to_zoom(zoom_to_slider(zoom));
        assert!((back / zoom - 1.0).abs() < 1e-4, "{zoom} -> {back}");
    }
    assert_eq!(zoom_to_slider(0.01), 0.0);
    assert_eq!(zoom_to_slider(64.0), 1.0);
    assert_eq!(zoom_to_slider(1000.0), 1.0);
    assert_eq!(zoom_to_slider(0.0001), 0.0);
    assert_eq!(slider_to_zoom(-1.0), 0.01);
    assert_eq!(slider_to_zoom(2.0), 64.0);
    // Logarithmic: 100% sits mid-way, not near the left end.
    assert!(zoom_to_slider(1.0) > 0.4 && zoom_to_slider(1.0) < 0.6);
    assert!(zoom_to_slider(0.1) < zoom_to_slider(1.0));
}

#[test]
fn zoom_field_parses_and_formats() {
    assert_eq!(parse_zoom_percent("150"), Some(1.5));
    assert_eq!(parse_zoom_percent(" 12.5 % "), Some(0.125));
    assert_eq!(parse_zoom_percent("0"), Some(0.01));
    assert_eq!(parse_zoom_percent("999999"), Some(64.0));
    assert_eq!(parse_zoom_percent("abc"), None);
    assert_eq!(parse_zoom_percent("NaN"), None);
    assert_eq!(format_zoom_percent(1.0), "100%");
    assert_eq!(format_zoom_percent(0.558), "55.8%");
}

#[test]
fn zooming_about_the_centre_scales_the_pan() {
    let (mut zoom, mut pan) = (1.0, vec2(40.0, -20.0));
    canvas::zoom_about(&mut zoom, &mut pan, 2.0, Vec2::ZERO);
    assert_eq!((zoom, pan), (2.0, vec2(80.0, -40.0)));
}

#[test]
fn cache_renders_once_then_debounces_edits() {
    let ctx = egui::Context::default();
    let id = Uuid::new_v4();
    let mut cache = ThumbnailCache::default();
    let image = || RgbaImage::new(4, 4);
    // The first thumbnail is immediate; later frames reuse it.
    assert!(cache.refresh(&ctx, (id, 0), 0.0, image).is_none());
    for t in [0.1, 0.5, 9.0] {
        assert!(cache.refresh(&ctx, (id, 0), t, image).is_none());
    }
    assert_eq!(cache.renders, 1);
    // An edit waits for the debounce, which restarts while edits keep coming.
    assert!(cache.refresh(&ctx, (id, 1), 10.0, image).is_some());
    assert!(cache.refresh(&ctx, (id, 2), 10.2, image).is_some());
    assert!(cache.refresh(&ctx, (id, 2), 10.3, image).is_some());
    assert_eq!(cache.renders, 1);
    assert!(cache.refresh(&ctx, (id, 2), 10.5, image).is_none());
    assert_eq!(cache.renders, 2);
    assert!(cache.refresh(&ctx, (id, 2), 11.0, image).is_none());
    assert_eq!(cache.renders, 2);
}

fn timed_frame(context: &egui::Context, app: &mut EditorApp, time: f64) {
    let _ = context.run(
        egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 860.0))),
            time: Some(time),
            ..Default::default()
        },
        |ctx| app.show(ctx),
    );
}

#[test]
fn app_renders_the_thumbnail_once_across_frames_and_again_after_an_edit() {
    let (context, mut app) = app();
    app.dimensions = [40, 30];
    app.new_document();
    let renders = |app: &EditorApp| app.session().unwrap().navigator.renders;
    for frame in 0..4 {
        timed_frame(&context, &mut app, frame as f64 * 0.01);
    }
    assert_eq!(renders(&app), 1);
    // An edit bumps the history revision; the rebuild waits out the debounce.
    app.session_mut().unwrap().history.mark_modified();
    timed_frame(&context, &mut app, 5.0);
    assert_eq!(renders(&app), 1);
    timed_frame(&context, &mut app, 5.1);
    assert_eq!(renders(&app), 1);
    timed_frame(&context, &mut app, 5.4);
    assert_eq!(renders(&app), 2);
    timed_frame(&context, &mut app, 6.0);
    assert_eq!(renders(&app), 2);
}

#[test]
fn zoom_controls_reuse_the_session_view_state() {
    let (context, mut app) = app();
    app.dimensions = [40, 30];
    app.new_document();
    timed_frame(&context, &mut app, 0.0);
    timed_frame(&context, &mut app, 0.1);
    assert!(app.canvas_viewport.is_some());
    assert!(app.navigator_view.is_some());
}
