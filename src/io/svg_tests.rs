use std::{io::Write, net::TcpListener, path::Path, time::Duration};

use super::*;

const NS: &str = "http://www.w3.org/2000/svg";

fn svg(attributes: &str, body: &str) -> Vec<u8> {
    format!(
        r#"<svg xmlns="{NS}" xmlns:xlink="http://www.w3.org/1999/xlink" {attributes}>{body}</svg>"#
    )
    .into_bytes()
}

fn natural(bytes: Vec<u8>) -> Result<SvgImage> {
    rasterize(bytes, SvgSize::Natural, TIMEOUT)
}

fn fit(bytes: Vec<u8>, width: u32, height: u32) -> Result<SvgImage> {
    rasterize(bytes, SvgSize::Fit { width, height }, TIMEOUT)
}

fn write(dir: &Path, name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

fn gzip(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

#[test]
fn recognises_svg_extensions_in_any_case() {
    for name in ["a.svg", "a.SVG", "a.svgz", "a.SvgZ"] {
        assert!(is_svg(Path::new(name)), "{name}");
    }
    for name in ["a.png", "svg", "a.svg.png", "a"] {
        assert!(!is_svg(Path::new(name)), "{name}");
    }
}

#[test]
fn opens_at_its_width_and_height() {
    let image = natural(svg(r#"width="40" height="20""#, "")).unwrap().image;
    assert_eq!(image.dimensions(), (40, 20));
    // Units convert at 96 DPI: one inch is 96 pixels.
    let image = natural(svg(r#"width="1in" height="0.5in""#, ""))
        .unwrap()
        .image;
    assert_eq!(image.dimensions(), (96, 48));
    // Width and height win over the viewBox.
    let image = natural(svg(r#"width="64" height="32" viewBox="0 0 4 2""#, ""))
        .unwrap()
        .image;
    assert_eq!(image.dimensions(), (64, 32));
}

#[test]
fn opens_at_its_view_box_without_width_and_height() {
    let image = natural(svg(r#"viewBox="10 10 30 12""#, "")).unwrap().image;
    assert_eq!(image.dimensions(), (30, 12));
}

#[test]
fn a_file_without_size_or_view_box_opens_at_the_fallback_size_keeping_aspect() {
    let drawing = r#"<rect width="50" height="25" fill="red"/>"#;
    let image = natural(svg("", drawing)).unwrap().image;
    assert_eq!(image.dimensions(), (FALLBACK_SIDE, FALLBACK_SIDE / 2));
    // Drawn at that size: the rectangle fills it.
    assert_eq!(image.get_pixel(1000, 500).0, [255, 0, 0, 255]);
    let image = natural(svg(
        r#"width="100%" height="100%""#,
        r#"<rect width="10" height="40"/>"#,
    ))
    .unwrap()
    .image;
    assert_eq!(image.dimensions(), (FALLBACK_SIDE / 4, FALLBACK_SIDE));
}

/// A rectangle covering the first 1.5 units of a 4 × 1 drawing: at its own size of four pixels,
/// its edge falls in the middle of a pixel.
const HALF_EDGE: &str = r#"<rect width="1.5" height="1" fill="blue"/>"#;

#[test]
fn imports_a_wide_file_sharp_at_canvas_width() {
    let image = fit(svg(r#"viewBox="0 0 4 1""#, HALF_EDGE), 400, 300)
        .unwrap()
        .image;
    assert_eq!(image.dimensions(), (400, 100));
    // Drawn at the target size, the edge at x = 150 is crisp. A bitmap scaled up from the
    // file's own four pixels would ramp across tens of pixels.
    assert_eq!(image.get_pixel(149, 50).0, [0, 0, 255, 255]);
    assert_eq!(image.get_pixel(150, 50).0[3], 0);
    assert_eq!(image.get_pixel(0, 0).0, [0, 0, 255, 255]);
    assert_eq!(image.get_pixel(399, 99).0[3], 0);
}

#[test]
fn imports_a_tall_file_sharp_at_canvas_height() {
    let drawing = r#"<rect width="1" height="1.5" fill="blue"/>"#;
    let image = fit(svg(r#"width="1" height="4""#, drawing), 300, 200)
        .unwrap()
        .image;
    assert_eq!(image.dimensions(), (50, 200));
    assert_eq!(image.get_pixel(25, 74).0, [0, 0, 255, 255]);
    assert_eq!(image.get_pixel(25, 75).0[3], 0);
}

#[test]
fn import_shrinks_a_file_larger_than_the_canvas() {
    let image = fit(svg(r#"width="2000" height="1000""#, ""), 100, 100)
        .unwrap()
        .image;
    assert_eq!(image.dimensions(), (100, 50));
    // A drawing without a size keeps the aspect of its drawing.
    let image = fit(svg("", r#"<rect width="30" height="10"/>"#), 90, 90)
        .unwrap()
        .image;
    assert_eq!(image.dimensions(), (90, 30));
}

#[test]
fn keeps_transparency_as_straight_alpha() {
    let drawing = r#"<circle cx="50" cy="50" r="40" fill="red"/>
        <rect x="0" y="0" width="10" height="10" fill="lime" fill-opacity="0.5"/>"#;
    let image = natural(svg(r#"width="100" height="100""#, drawing))
        .unwrap()
        .image;
    assert_eq!(image.get_pixel(99, 99).0, [0, 0, 0, 0]);
    assert_eq!(image.get_pixel(50, 50).0, [255, 0, 0, 255]);
    let [r, g, b, a] = image.get_pixel(5, 5).0;
    assert_eq!(
        (r, g, b),
        (0, 255, 0),
        "colour must not be darkened by its alpha"
    );
    assert!(a.abs_diff(128) <= 1, "{a}");
}

#[test]
fn refuses_malformed_files() {
    for (bytes, message) in [
        (
            b"<svg xmlns='http://www.w3.org/2000/svg' width='10'".to_vec(),
            "damaged",
        ),
        (b"not xml at all".to_vec(), "damaged"),
        (vec![0xff, 0xfe, 0x00, 0x3c], "UTF-8"),
        (b"<html><body/></html>".to_vec(), "not an SVG"),
        (svg(r#"width="0" height="10""#, ""), "valid size"),
        (svg(r#"width="-5" height="10""#, ""), "valid size"),
        (vec![0x1f, 0x8b, 0x08, 0x00, 0x01, 0x02], "SVGZ"),
    ] {
        let error = format!("{:#}", natural(bytes).unwrap_err());
        assert!(error.contains(message), "{error}");
    }
}

#[test]
fn refuses_an_entity_expansion_bomb() {
    let mut doctype = String::from(r#"<!DOCTYPE svg [<!ENTITY a0 "lol">"#);
    for i in 1..12 {
        let previous = format!("&a{};", i - 1).repeat(10);
        doctype.push_str(&format!(r#"<!ENTITY a{i} "{previous}">"#));
    }
    doctype.push(']');
    let bytes = format!(
        r#"<?xml version="1.0"?>{doctype}><svg xmlns="{NS}" width="10" height="10"><desc>&a11;</desc></svg>"#
    );
    assert!(natural(bytes.into_bytes()).is_err());
}

#[test]
fn refuses_sizes_over_the_limits() {
    let error = natural(svg(r#"width="70000" height="10""#, ""))
        .unwrap_err()
        .to_string();
    assert!(error.contains("Dimensions must be between"), "{error}");
    // The library's tests run with the minimum limit of 100 megapixels.
    let error = natural(svg(r#"width="20000" height="20000""#, ""))
        .unwrap_err()
        .to_string();
    assert!(error.contains("megapixels"), "{error}");
    let error = natural(svg(r#"width="1e30" height="10""#, ""))
        .unwrap_err()
        .to_string();
    assert!(error.contains("Dimensions"), "{error}");
}

#[test]
fn refuses_files_over_the_byte_limit() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "a.svg", &svg(r#"width="1" height="1""#, ""));
    assert!(read(&path, 1024).is_ok());
    let error = read(&path, 16).unwrap_err().to_string();
    assert!(error.contains("SVG files are limited"), "{error}");
    // The real limit, checked before anything is read.
    let file = File::create(dir.path().join("huge.svg")).unwrap();
    file.set_len(MAX_BYTES + 1).unwrap();
    let error = load(&dir.path().join("huge.svg"), SvgSize::Natural)
        .unwrap_err()
        .to_string();
    assert!(error.contains("64 MiB"), "{error}");
    // Directories, FIFOs and missing files are not read.
    assert!(load(dir.path(), SvgSize::Natural).is_err());
    assert!(load(&dir.path().join("missing.svg"), SvgSize::Natural).is_err());
}

#[test]
fn opens_svgz_and_limits_its_decompressed_size() {
    let plain = svg(
        r#"width="8" height="8""#,
        r#"<rect width="4" height="8" fill="red"/>"#,
    );
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "a.SVGZ", &gzip(&plain));
    let image = load(&path, SvgSize::Natural).unwrap().image;
    assert_eq!(image, natural(plain.clone()).unwrap().image);
    assert_eq!(image.get_pixel(0, 0).0, [255, 0, 0, 255]);
    let error = decompress(&gzip(&plain), 32).unwrap_err().to_string();
    assert!(error.contains("SVG files are limited"), "{error}");
    assert_eq!(decompress(&gzip(&plain), 4096).unwrap(), plain);
}

#[test]
fn does_not_load_linked_files_or_urls() {
    let dir = tempfile::tempdir().unwrap();
    // A file resvg could draw, were it loaded.
    let red = svg(
        r#"width="10" height="10""#,
        r#"<rect width="10" height="10" fill="red"/>"#,
    );
    let linked = write(dir.path(), "red.svg", &red);
    let png = write(dir.path(), "red.png", &[]);
    image::RgbaImage::from_pixel(10, 10, image::Rgba([255, 0, 0, 255]))
        .save(&png)
        .unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let port = listener.local_addr().unwrap().port();
    let body = format!(
        r#"<image href="{svg}" width="10" height="10"/>
        <image xlink:href="file://{svg}" x="10" width="10" height="10"/>
        <image href="red.svg" x="20" width="10" height="10"/>
        <image href="{png}" x="30" width="10" height="10"/>
        <image href="http://127.0.0.1:{port}/red.svg" x="40" width="10" height="10"/>
        <filter id="f"><feImage href="http://127.0.0.1:{port}/f.svg"/></filter>
        <rect y="10" width="10" height="10" filter="url(#f)"/>"#,
        svg = linked.display(),
        png = png.display(),
    );
    let drawn = load(
        &write(
            dir.path(),
            "links.svg",
            &svg(r#"width="50" height="20""#, &body),
        ),
        SvgSize::Natural,
    )
    .unwrap();
    for x in [5, 15, 25, 35, 45] {
        assert_eq!(drawn.image.get_pixel(x, 5).0, [0, 0, 0, 0], "image at {x}");
    }
    assert!(drawn.images >= 5, "{}", drawn.images);
    assert!(drawn.notice().unwrap().contains("linked images"));
    match listener.accept() {
        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
        other => panic!("the import connected to the URL: {other:?}"),
    }
}

#[test]
fn draws_embedded_svg_but_reports_embedded_bitmaps() {
    let embedded = "data:image/svg+xml,%3Csvg%20xmlns='http://www.w3.org/2000/svg'%20width='10'%20height='10'%3E%3Crect%20width='10'%20height='10'%20fill='blue'/%3E%3C/svg%3E";
    let body = format!(
        r#"<image href="{embedded}" width="10" height="10"/>
        <image href="data:image/png;base64,iVBORw0KGgo=" x="10" width="10" height="10"/>"#
    );
    let drawn = natural(svg(r#"width="20" height="10""#, &body)).unwrap();
    assert_eq!(drawn.image.get_pixel(5, 5).0, [0, 0, 255, 255]);
    assert_eq!(drawn.images, 1);
    assert_eq!(drawn.text, 0);
}

#[test]
fn reports_text_it_cannot_draw() {
    let drawn = natural(svg(
        r#"width="20" height="10""#,
        "<text y='8'>Hi</text><g><text y='8'>there</text></g>",
    ))
    .unwrap();
    assert_eq!((drawn.text, drawn.images), (2, 0));
    let notice = drawn.notice().unwrap();
    assert!(
        notice.contains("Text") && notice.contains(": 2"),
        "{notice}"
    );
    assert!(!notice.contains("linked images"), "{notice}");
    let plain = natural(svg(r#"width="2" height="2""#, "")).unwrap();
    assert!(plain.notice().is_none());
}

#[test]
fn gives_up_on_a_file_that_takes_too_long() {
    let heavy = svg(
        r#"width="2000" height="2000""#,
        r#"<filter id="b"><feGaussianBlur stdDeviation="200"/></filter>
        <rect width="2000" height="2000" fill="red" filter="url(#b)"/>"#,
    );
    let error = rasterize(heavy, SvgSize::Natural, Duration::ZERO)
        .unwrap_err()
        .to_string();
    assert!(error.contains("took longer"), "{error}");
}

#[test]
fn import_image_reads_svg_at_its_own_size() {
    let dir = tempfile::tempdir().unwrap();
    let path = write(dir.path(), "logo.svg", &svg(r#"viewBox="0 0 12 7""#, ""));
    assert_eq!(
        crate::io::import_image(&path).unwrap().dimensions(),
        (12, 7)
    );
}
