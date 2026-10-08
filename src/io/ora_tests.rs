//! OpenRaster tests. Fixtures are written here as Krita writes them (its
//! `kis_open_raster_stack_save_visitor.cpp`): stacks at x = y = 0 with `isolation`, layers with
//! absolute offsets, the mimetype first and stored.
use std::{
    io::{Cursor, Read, Write},
    sync::Arc,
};

use image::{GrayImage, Luma, Rgba};

use super::*;
use crate::{
    document::{Adjustment, Mask, ShapeStyle},
    effects::Filter,
    layer_effects::{LayerEffects, OverlayEffect},
    paint::ShapeKind,
};

// ---------------------------------------------------------------------------------------------
// Fixtures

fn solid(width: u32, height: u32, color: [u8; 4]) -> Vec<u8> {
    png(&RgbaImage::from_pixel(width, height, Rgba(color))).unwrap()
}

/// A ZIP archive of `entries`, in order; `mimetype` is stored, the rest deflated.
fn zip_of(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        let method = if *name == "mimetype" {
            CompressionMethod::Stored
        } else {
            CompressionMethod::Deflated
        };
        archive
            .start_file(
                *name,
                SimpleFileOptions::default().compression_method(method),
            )
            .unwrap();
        archive.write_all(bytes).unwrap();
    }
    archive.finish().unwrap().into_inner()
}

/// An OpenRaster file with `stack` inside the root stack, and the layer images `files`.
fn ora(width: u32, height: u32, stack: &str, files: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let xml = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<image version=\"0.0.1\" w=\"{width}\" h=\"{height}\" xres=\"300\" yres=\"300\">\n<stack>\n{stack}\n</stack>\n</image>\n"
    );
    let mut entries = vec![
        ("mimetype", MIMETYPE.as_bytes().to_vec()),
        ("stack.xml", xml.into_bytes()),
    ];
    entries.extend(files.iter().cloned());
    entries.push(("mergedimage.png", solid(1, 1, [0; 4])));
    entries.push(("Thumbnails/thumbnail.png", solid(1, 1, [0; 4])));
    zip_of(&entries)
}

fn open(bytes: Vec<u8>) -> Result<(Document, ImportReport)> {
    read(Cursor::new(bytes), PixelBudget::default())
}

fn error(bytes: Vec<u8>) -> String {
    format!("{:#}", open(bytes).unwrap_err())
}

fn layer<'a>(document: &'a Document, name: &str) -> &'a Layer {
    document
        .layers
        .iter()
        .find(|l| l.name == name)
        .unwrap_or_else(|| panic!("no layer {name}"))
}

/// One layer with `composite-op` set to `op`.
fn with_op(op: &str) -> (Document, ImportReport) {
    open(ora(
        2,
        2,
        &format!("<layer name=\"L\" src=\"data/l.png\" composite-op=\"{op}\"/>"),
        &[("data/l.png", solid(2, 2, [9, 9, 9, 255]))],
    ))
    .unwrap()
}

// ---------------------------------------------------------------------------------------------
// Import

#[test]
fn imports_a_krita_file_with_folders_offsets_opacity_and_visibility() {
    let stack = r#"
<stack name="Group" opacity="0.5" visibility="visible" composite-op="svg:src-over" isolation="auto" x="0" y="0">
  <layer name="Top" src="data/layer3.png" x="2" y="1" opacity="0.75" visibility="visible" composite-op="svg:multiply"/>
  <stack name="Inner" composite-op="svg:src-over" isolation="auto" x="0" y="0">
    <layer name="Deep" src="data/layer2.png" x="-3" y="-2" opacity="1" visibility="visible" composite-op="krita:linear_burn" selected="true"/>
  </stack>
</stack>
<layer name="Hidden" src="data/layer1.png" x="0" y="0" opacity="1" visibility="hidden" composite-op="svg:src-over" edit-locked="true"/>
<layer name="Background" src="data/layer0.png" x="0" y="0" opacity="1" visibility="visible" composite-op="svg:src-over"/>
"#;
    let (document, report) = open(ora(
        8,
        6,
        stack,
        &[
            ("data/layer0.png", solid(8, 6, [255; 4])),
            ("data/layer1.png", solid(8, 6, [0, 0, 0, 255])),
            ("data/layer2.png", solid(4, 3, [0, 255, 0, 255])),
            ("data/layer3.png", solid(3, 2, [255, 0, 0, 255])),
        ],
    ))
    .unwrap();
    assert!(report.is_empty(), "{:?}", report.lines());
    assert_eq!(report.source(), ImportSource::OpenRaster);
    assert_eq!((document.width, document.height), (8, 6));
    assert_eq!(document.resolution, 300.0);

    // Bottom first, each folder after its contents.
    let names: Vec<_> = document.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        ["Background", "Hidden", "Deep", "Inner", "Top", "Group"]
    );
    let group = layer(&document, "Group");
    let inner = layer(&document, "Inner");
    assert!(group.group && inner.group);
    assert_eq!(group.parent, None);
    assert_eq!(group.opacity, 0.5);
    assert_eq!(inner.parent, Some(group.id));
    assert_eq!(layer(&document, "Top").parent, Some(group.id));
    assert_eq!(layer(&document, "Deep").parent, Some(inner.id));

    let top = layer(&document, "Top");
    assert_eq!((top.transform.x, top.transform.y), (2.0, 1.0));
    assert_eq!((top.transform.width, top.transform.height), (3.0, 2.0));
    assert_eq!(top.opacity, 0.75);
    assert_eq!(top.blend, BlendMode::Multiply);
    let deep = layer(&document, "Deep");
    assert_eq!((deep.transform.x, deep.transform.y), (-3.0, -2.0));
    assert_eq!(deep.blend, BlendMode::LinearBurn);
    let hidden = layer(&document, "Hidden");
    assert!(!hidden.visible && hidden.locked);
    assert!(layer(&document, "Background").visible);
    assert_eq!(document.active, Some(deep.id));
    assert_eq!(
        layer(&document, "Background").pixels.as_deref(),
        Some(&RgbaImage::from_pixel(8, 6, Rgba([255; 4])))
    );
}

#[test]
fn renders_like_the_file_describes() {
    // White, under a folder at 50% holding red in Multiply at (1, 0): red × white is red, at
    // half strength, and white where the folder has nothing.
    let stack = r#"
<stack name="Folder" opacity="0.5" isolation="auto">
  <layer name="Red" src="data/red.png" x="1" y="0" composite-op="svg:multiply"/>
</stack>
<layer name="White" src="data/white.png"/>
"#;
    let (document, _) = open(ora(
        2,
        1,
        stack,
        &[
            ("data/white.png", solid(2, 1, [255; 4])),
            ("data/red.png", solid(1, 1, [255, 0, 0, 255])),
        ],
    ))
    .unwrap();
    let image = render::render(&document);
    assert_eq!(image.get_pixel(0, 0).0, [255, 255, 255, 255]);
    let [r, g, b, a] = image.get_pixel(1, 0).0;
    assert_eq!((r, a), (255, 255));
    assert!(
        (127..=128).contains(&g) && g == b,
        "{:?}",
        image.get_pixel(1, 0)
    );
}

#[test]
fn reads_every_blend_mode_and_writes_one_name_for_each() {
    for (name, mode) in BLEND_MODES.iter().chain(&BLEND_ALIASES) {
        let (document, report) = with_op(name);
        assert_eq!(document.layers[0].blend, *mode, "{name}");
        assert!(report.is_empty(), "{name}: {:?}", report.lines());
    }
    for mode in BlendMode::ALL {
        assert_eq!(blend_mode(composite_op(mode)), Some(mode));
        assert_eq!(
            BLEND_MODES.iter().filter(|(_, m)| *m == mode).count(),
            1,
            "{mode:?}"
        );
    }
    assert_eq!(composite_op(BlendMode::Normal), "svg:src-over");
    assert_eq!(composite_op(BlendMode::LinearDodge), "svg:plus");
    assert_eq!(composite_op(BlendMode::Exclusion), "krita:exclusion");
}

#[test]
fn reports_composite_operations_xuan_lacks_and_draws_them_as_normal() {
    for op in UNSUPPORTED_OPS {
        let (document, report) = with_op(op);
        assert_eq!(document.layers[0].blend, BlendMode::Normal);
        assert_eq!(report.count(Dropped::OpenRasterBlendMode(op)), 1, "{op}");
    }
    let (document, report) = with_op("krita:gamma_light");
    assert_eq!(document.layers[0].blend, BlendMode::Normal);
    assert_eq!(report.count(Dropped::OpenRasterBlendMode(UNKNOWN_OP)), 1);
    assert!(report.summary().unwrap().contains("OpenRaster"));

    // Krita's alpha-preserve keeps the layer within what is below; Xuan draws it over it.
    let (_, report) = open(ora(
        1,
        1,
        r#"<layer src="data/a.png" composite-op="svg:multiply" alpha-preserve="true"/>"#,
        &[("data/a.png", solid(1, 1, [1, 2, 3, 255]))],
    ))
    .unwrap();
    assert_eq!(
        report.count(Dropped::OpenRasterBlendMode("alpha-preserve")),
        1
    );
}

#[test]
fn reports_what_folders_lose_by_passing_through() {
    let file = |attributes: &str, op: &str| {
        open(ora(
            2,
            2,
            &format!(
                "<stack name=\"F\" {attributes}><layer src=\"data/a.png\" composite-op=\"{op}\"/></stack>"
            ),
            &[("data/a.png", solid(2, 2, [1, 2, 3, 255]))],
        ))
        .unwrap()
        .1
    };
    // A stack's own blend mode.
    let report = file(
        "composite-op=\"svg:screen\" isolation=\"isolate\"",
        "svg:src-over",
    );
    assert_eq!(report.count(Dropped::FolderBlendMode("svg:screen")), 1);
    // Isolation is the default, and matters only when the layers blend.
    let report = file("", "svg:multiply");
    assert_eq!(report.count(Dropped::IsolatedFolder), 1);
    assert!(file("isolation=\"isolate\"", "svg:src-over").is_empty());
    assert!(file("isolation=\"auto\"", "svg:multiply").is_empty());
}

#[test]
fn names_unnamed_layers_and_ignores_stack_offsets() {
    // OpenRaster 0.0.6: a stack's x and y do not move its layers.
    let (document, _) = open(ora(
        4,
        4,
        r#"<stack x="5" y="5"><layer src="data/a.png" x="1" y="-1.4" name=" "/></stack>"#,
        &[("data/a.png", solid(1, 1, [1, 2, 3, 255]))],
    ))
    .unwrap();
    assert_eq!(document.layers[0].name, "Layer");
    assert_eq!(document.layers[1].name, "Folder");
    let t = document.layers[0].transform;
    assert_eq!((t.x, t.y), (1.0, -1.0));
}

#[test]
fn leaves_out_and_reports_what_it_cannot_read() {
    let stack = r#"
<filter name="Blur" type="applications:krita:blur" opacity="1"/>
<layer name="Vector" src="data/shape.svg"/>
<layer name="No source"/>
<text name="Words"/>
<layer name="Kept" src="data/a.png"/>
"#;
    let (document, report) = open(ora(
        2,
        2,
        stack,
        &[
            ("data/a.png", solid(2, 2, [1, 2, 3, 255])),
            ("data/shape.svg", b"<svg/>".to_vec()),
        ],
    ))
    .unwrap();
    assert_eq!(document.layers.len(), 1);
    assert_eq!(document.layers[0].name, "Kept");
    assert_eq!(report.count(Dropped::OpenRasterLeftOut), 4);
}

#[test]
fn opens_nested_stacks_up_to_the_folder_limit() {
    let nest = |depth: usize| {
        let mut xml = "<layer src=\"data/a.png\"/>".to_owned();
        for _ in 0..depth {
            xml = format!("<stack>{xml}</stack>");
        }
        ora(1, 1, &xml, &[("data/a.png", solid(1, 1, [0, 0, 0, 255]))])
    };
    let (document, _) = open(nest(MAX_FOLDER_DEPTH)).unwrap();
    assert_eq!(document.layers.len(), MAX_FOLDER_DEPTH + 1);
    assert!(error(nest(MAX_FOLDER_DEPTH + 1)).contains("nested too deeply"));
}

#[test]
fn opens_a_file_from_disk_and_through_load_with_report() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Art.ORA");
    fs::write(
        &path,
        ora(
            3,
            2,
            r#"<layer name="A" src="data/a.png"/>"#,
            &[("data/a.png", solid(3, 2, [1, 2, 3, 255]))],
        ),
    )
    .unwrap();
    assert!(is_openraster(&path));
    assert!(!is_openraster(Path::new("art.png")));
    let (document, report) = crate::io::load_with_report(&path).unwrap();
    assert_eq!(document.layers[0].name, "A");
    assert!(report.is_empty());
    assert!(load(directory.path(), PixelBudget::default()).is_err());
}

#[test]
fn the_checked_in_fixture_opens() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/ora/krita-groups.ora");
    let (document, report) = load(&path, PixelBudget::default()).unwrap();
    assert!(report.is_empty(), "{:?}", report.lines());
    let names: Vec<_> = document.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Paper", "Shadow", "Light", "Shapes"]);
    let shapes = layer(&document, "Shapes");
    assert!(shapes.group);
    assert_eq!(shapes.opacity, 0.8);
    let light = layer(&document, "Light");
    assert_eq!(layer(&document, "Shadow").blend, BlendMode::Multiply);
    assert_eq!(light.blend, BlendMode::Screen);
    assert_eq!((light.transform.x, light.transform.y), (-4.0, 6.0));
    assert_eq!(light.parent, Some(shapes.id));
    // Paper, darkened where the shadow is, lightened where the light is.
    let image = render::render(&document);
    let paper = image.get_pixel(30, 2).0;
    assert!(image.get_pixel(12, 12).0[0] < paper[0]);
    assert!(image.get_pixel(2, 10).0[2] > paper[2]);
}

// ---------------------------------------------------------------------------------------------
// Hostile archives

#[test]
fn refuses_layer_paths_that_leave_the_archive() {
    for src in [
        "../evil.png",
        "data/../../evil.png",
        "/etc/passwd",
        "\\\\server\\share.png",
        "C:/evil.png",
        "data\\evil.png",
        "data//evil.png",
        "./evil.png",
        "data/",
        "",
    ] {
        let xml = format!("<layer src=\"{}\"/>", escape(src));
        let message = error(ora(1, 1, &xml, &[("data/x.png", solid(1, 1, [0; 4]))]));
        assert!(message.contains("unsafe layer path"), "{src}: {message}");
    }
    assert!(check_src("data/layer0.png").is_ok());
    assert!(check_src(&"a".repeat(MAX_SRC_BYTES + 1)).is_err());
}

#[test]
fn refuses_files_without_the_openraster_mimetype() {
    let xml = "<image w=\"1\" h=\"1\"><stack/></image>"
        .as_bytes()
        .to_vec();
    let missing = zip_of(&[("stack.xml", xml.clone())]);
    assert!(error(missing).contains("mimetype entry is missing"));
    let wrong = zip_of(&[("mimetype", b"image/png".to_vec()), ("stack.xml", xml)]);
    assert!(error(wrong).contains("not an OpenRaster file"));
    assert!(error(b"PK not really".to_vec()).contains("not an OpenRaster file"));
    let no_stack = zip_of(&[("mimetype", MIMETYPE.as_bytes().to_vec())]);
    assert!(error(no_stack).contains("stack.xml is missing"));
}

#[test]
fn refuses_too_many_entries() {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    archive.start_file("mimetype", stored).unwrap();
    archive.write_all(MIMETYPE.as_bytes()).unwrap();
    for i in 0..MAX_ARCHIVE_ENTRIES {
        archive.start_file(format!("data/{i}"), stored).unwrap();
    }
    let bytes = archive.finish().unwrap().into_inner();
    assert!(error(bytes).contains("too many entries"));
}

#[test]
fn refuses_zip_bombs_and_oversized_images() {
    // A 1 × 1 PNG followed by far more data than its pixels could need.
    let mut bomb = solid(1, 1, [0; 4]);
    bomb.extend(std::iter::repeat_n(0, (PNG_SLACK + (1 << 20)) as usize));
    let message = error(ora(
        1,
        1,
        r#"<layer src="data/bomb.png"/>"#,
        &[("data/bomb.png", bomb)],
    ));
    assert!(message.contains("too large"), "{message}");

    // A header claiming a side beyond the limit is refused before anything is inflated.
    let mut huge = solid(1, 1, [0; 4]);
    huge[16..20].copy_from_slice(&200_000u32.to_be_bytes());
    let message = error(ora(
        1,
        1,
        r#"<layer src="data/huge.png"/>"#,
        &[("data/huge.png", huge)],
    ));
    assert!(message.contains("Dimensions"), "{message}");

    // So is the canvas.
    let message = error(ora(200_000, 1, "", &[]));
    assert!(message.contains("Dimensions"), "{message}");

    // Layers beyond what the destination may still hold.
    let file = ora(
        4,
        4,
        r#"<layer src="data/a.png"/><layer src="data/a.png"/>"#,
        &[("data/a.png", solid(4, 4, [0; 4]))],
    );
    let budget = PixelBudget {
        layers: 20,
        masks: 0,
    };
    let message = format!("{:#}", read(Cursor::new(file), budget).unwrap_err());
    assert!(message.contains("megapixels"), "{message}");
}

#[test]
fn refuses_damaged_stacks() {
    let image = |stack: &str| ora(2, 2, stack, &[("data/a.png", solid(2, 2, [0; 4]))]);
    for stack in [
        r#"<layer src="data/a.png" opacity="NaN"/>"#,
        r#"<layer src="data/a.png" opacity="lots"/>"#,
        r#"<layer src="data/a.png" x="1e9"/>"#,
        r#"<layer src="data/a.png" y="inf"/>"#,
        "<layer",
    ] {
        assert!(error(image(stack)).contains("damaged"), "{stack}");
    }
    assert!(error(image(r#"<layer src="data/missing.png"/>"#)).contains("missing the layer image"));

    let file = |xml: &str| {
        zip_of(&[
            ("mimetype", MIMETYPE.as_bytes().to_vec()),
            ("stack.xml", xml.as_bytes().to_vec()),
        ])
    };
    // DTDs, and with them entity expansion, are refused.
    let laughs = "<?xml version=\"1.0\"?><!DOCTYPE image [<!ENTITY a \"aaaaaaaaaa\"><!ENTITY b \"&a;&a;&a;&a;&a;&a;&a;&a;\">]><image w=\"1\" h=\"1\"><stack><layer name=\"&b;\"/></stack></image>";
    assert!(error(file(laughs)).contains("damaged"));
    assert!(error(file("<image h=\"1\"><stack/></image>")).contains("image w"));
    assert!(error(file("<picture w=\"1\" h=\"1\"/>")).contains("no image"));
    assert!(error(file("<image w=\"1\" h=\"1\"/>")).contains("no root stack"));
    assert!(error(file("<image w=\"1\" h=\"1\"><stack>\u{0}</stack></image>")).contains("damaged"));
}

#[test]
fn refuses_too_many_layers() {
    let layers = "<layer src=\"data/a.png\"/>".repeat(MAX_LAYERS + 1);
    let message = error(ora(1, 1, &layers, &[("data/a.png", solid(1, 1, [0; 4]))]));
    assert!(message.contains("too many layers"), "{message}");
}

// ---------------------------------------------------------------------------------------------
// Export

fn exported(document: &Document) -> (Vec<u8>, ExportReport) {
    let mut bytes = Cursor::new(Vec::new());
    let report = write(document, &mut bytes).unwrap();
    (bytes.into_inner(), report)
}

fn entry(bytes: &[u8], name: &str) -> Vec<u8> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).unwrap();
    let mut out = Vec::new();
    archive
        .by_name(name)
        .unwrap()
        .read_to_end(&mut out)
        .unwrap();
    out
}

fn pixels(width: u32, height: u32, color: [u8; 4]) -> RgbaImage {
    RgbaImage::from_pixel(width, height, Rgba(color))
}

#[test]
fn writes_the_archive_the_specification_describes() {
    let mut document = Document::new(600, 300).unwrap();
    document.layers = vec![Layer::image("Paper", pixels(600, 300, [200, 10, 10, 255]))];
    document.resolution = 144.0;
    let (bytes, report) = exported(&document);
    assert!(report.is_empty());

    // The mimetype is the first entry, stored, with no extra field, so its content sits at a
    // fixed offset from the start of the file.
    assert_eq!(&bytes[..4], b"PK\x03\x04");
    assert_eq!(u16::from_le_bytes([bytes[8], bytes[9]]), 0, "stored");
    assert_eq!(u16::from_le_bytes([bytes[26], bytes[27]]), 8);
    assert_eq!(
        u16::from_le_bytes([bytes[28], bytes[29]]),
        0,
        "no extra field"
    );
    assert_eq!(&bytes[30..38], b"mimetype");
    assert_eq!(&bytes[38..54], MIMETYPE.as_bytes());
    let mut archive = ZipArchive::new(Cursor::new(&bytes)).unwrap();
    assert_eq!(archive.by_index(0).unwrap().name(), "mimetype");
    assert_eq!(
        archive.by_index(0).unwrap().compression(),
        CompressionMethod::Stored
    );

    let xml = String::from_utf8(entry(&bytes, "stack.xml")).unwrap();
    let tree = roxmltree::Document::parse(&xml).unwrap();
    let image = tree.root_element();
    assert!(image.has_tag_name("image"));
    assert_eq!(image.attribute("version"), Some(VERSION));
    assert_eq!(image.attribute("w"), Some("600"));
    assert_eq!(image.attribute("h"), Some("300"));
    assert_eq!(image.attribute("xres"), Some("144"));
    assert_eq!(image.attribute("yres"), Some("144"));
    let root = image.children().find(|n| n.is_element()).unwrap();
    assert!(root.has_tag_name("stack"));
    // The root stack carries no attributes.
    assert_eq!(root.attributes().len(), 0);
    let layer = root.children().find(|n| n.is_element()).unwrap();
    assert_eq!(layer.attribute("src"), Some("data/layer0.png"));
    assert!(archive.by_name("data/layer0.png").is_ok());

    let merged = image::load_from_memory(&entry(&bytes, "mergedimage.png"))
        .unwrap()
        .to_rgba8();
    assert_eq!(merged.dimensions(), (600, 300));
    assert_eq!(merged.get_pixel(5, 5).0, [200, 10, 10, 255]);
    let thumbnail = image::load_from_memory(&entry(&bytes, "Thumbnails/thumbnail.png"))
        .unwrap()
        .to_rgba8();
    assert_eq!(thumbnail.dimensions(), (256, 128));

    // Small documents keep their size in the thumbnail.
    let small = Document::new(20, 30).unwrap();
    let (bytes, _) = exported(&small);
    let thumbnail = image::load_from_memory(&entry(&bytes, "Thumbnails/thumbnail.png")).unwrap();
    assert_eq!((thumbnail.width(), thumbnail.height()), (20, 30));
    // A layer without pixels is written as one transparent pixel.
    let xml = String::from_utf8(entry(&bytes, "stack.xml")).unwrap();
    assert!(xml.contains("name=\"Layer 1\""), "{xml}");
    let empty = image::load_from_memory(&entry(&bytes, "data/layer0.png")).unwrap();
    assert_eq!((empty.width(), empty.height()), (1, 1));
}

/// A document using everything OpenRaster keeps.
fn layered() -> Document {
    let mut document = Document::new(12, 10).unwrap();
    let mut paper = Layer::image("Paper & “ink” <1>", pixels(12, 10, [240, 230, 220, 255]));
    paper.locked = true;
    let mut folder = Layer::blank("Folder", 12, 10);
    folder.group = true;
    folder.opacity = 0.6;
    let mut inner = Layer::blank("Inner", 12, 10);
    inner.group = true;
    inner.parent = Some(folder.id);
    inner.visible = false;
    let mut art = Layer::image(
        "Art",
        RgbaImage::from_fn(5, 4, |x, y| Rgba([x as u8 * 40, y as u8 * 60, 90, 200])),
    );
    art.parent = Some(folder.id);
    art.transform.x = -2.0;
    art.transform.y = 7.0;
    art.blend = BlendMode::Multiply;
    art.opacity = 0.25;
    let mut deep = Layer::image("Deep", pixels(3, 3, [10, 200, 30, 255]));
    deep.parent = Some(inner.id);
    deep.blend = BlendMode::VividLight;
    deep.transform.x = 9.0;
    let mut hidden = Layer::image("Hidden", pixels(2, 2, [1, 2, 3, 4]));
    hidden.visible = false;
    hidden.blend = BlendMode::Divide;
    document.layers = vec![paper, deep, inner, art, folder, hidden];
    document.active = Some(document.layers[3].id);
    document
}

#[test]
fn round_trips_layers_folders_and_their_properties() {
    let document = layered();
    let (bytes, report) = exported(&document);
    assert!(report.is_empty(), "{:?}", report.lines());
    let (back, import) = open(bytes).unwrap();
    assert!(import.is_empty(), "{:?}", import.lines());
    assert_eq!(back.layers.len(), document.layers.len());
    let parent = |d: &Document, l: &Layer| {
        l.parent
            .map(|id| d.layers.iter().find(|p| p.id == id).unwrap().name.clone())
    };
    for original in &document.layers {
        let copy = layer(&back, &original.name);
        let name = &original.name;
        assert_eq!(copy.group, original.group, "{name}");
        assert_eq!(copy.visible, original.visible, "{name}");
        assert_eq!(copy.locked, original.locked, "{name}");
        assert_eq!(copy.opacity, original.opacity, "{name}");
        assert_eq!(copy.blend, original.blend, "{name}");
        assert_eq!(parent(&back, copy), parent(&document, original), "{name}");
        if !original.group {
            assert_eq!(copy.transform, original.transform, "{name}");
            assert_eq!(copy.pixels, original.pixels, "{name}");
        }
    }
    // Siblings keep their order.
    let order = |d: &Document| -> Vec<String> {
        d.layers
            .iter()
            .filter(|l| l.parent.is_none())
            .map(|l| l.name.clone())
            .collect()
    };
    assert_eq!(order(&back), order(&document));
    assert_eq!(back.active().map(|l| l.name.as_str()), Some("Art"));
    assert_eq!(render::render(&back), render::render(&document));
}

#[test]
fn every_blend_mode_round_trips() {
    let mut document = Document::new(2, 2).unwrap();
    document.layers = BlendMode::ALL
        .iter()
        .enumerate()
        .map(|(i, mode)| {
            let mut layer = Layer::image(format!("L{i}"), pixels(1, 1, [9, 9, 9, 255]));
            layer.blend = *mode;
            layer
        })
        .collect();
    let (bytes, _) = exported(&document);
    let (back, report) = open(bytes).unwrap();
    assert!(report.is_empty());
    let modes: Vec<_> = back.layers.iter().map(|l| l.blend).collect();
    assert_eq!(modes, BlendMode::ALL);
}

#[test]
fn flattens_what_openraster_cannot_hold_and_says_so() {
    let mut document = Document::new(10, 10).unwrap();
    let base = Layer::image("Base", pixels(10, 10, [100, 100, 100, 255]));
    let mut masked = Layer::image("Masked", pixels(4, 4, [255, 0, 0, 255]));
    masked.transform.x = 1.0;
    let mut mask = GrayImage::from_pixel(4, 4, Luma([255]));
    for y in 0..4 {
        mask.put_pixel(0, y, Luma([0]));
    }
    masked.mask = Some(Mask {
        pixels: Arc::new(mask),
        ..Mask::white()
    });
    masked.blend = BlendMode::Screen;
    masked.opacity = 0.5;
    let mut faded = Layer::image("Faded", pixels(2, 2, [0, 0, 255, 255]));
    faded.fill = 0.5;
    let mut scaled = Layer::image("Scaled", pixels(2, 2, [0, 255, 0, 255]));
    scaled.transform.width = 4.0;
    let mut styled = Layer::image("Styled", pixels(2, 2, [0, 255, 0, 255]));
    styled.effects = Some(LayerEffects {
        color_overlay: Some(OverlayEffect {
            enabled: true,
            color: [0, 0, 0],
            opacity: 1.0,
        }),
        ..LayerEffects::default()
    });
    let mut shape = Layer::image("Shape", pixels(2, 2, [5, 5, 5, 255]));
    shape.shape = Some(ShapeStyle {
        kind: ShapeKind::Rectangle,
        color: [5, 5, 5, 255],
        corner_radius: 0.0,
        path: None,
    });
    let mut clipped = Layer::image("Clipped", pixels(10, 10, [0, 0, 0, 255]));
    clipped.clip_to = Some(shape.id);
    let mut invert = Layer::blank("Invert", 10, 10);
    invert.adjustment = Some(Adjustment::Invert);
    let mut off = Layer::blank("Off", 10, 10);
    off.adjustment = Some(Adjustment::Invert);
    off.visible = false;
    let mut hidden = Layer::image("Hidden below", pixels(1, 1, [1, 1, 1, 255]));
    hidden.visible = false;
    let mut folder = Layer::blank("Masked folder", 10, 10);
    folder.group = true;
    folder.mask = Some(Mask::white());
    let mut inside = Layer::image("Inside", pixels(1, 1, [7, 7, 7, 255]));
    inside.parent = Some(folder.id);
    let mut blur = Layer::blank("Blur", 10, 10);
    blur.filter = Some(Filter::GaussianBlur { radius: 1.0 });
    document.layers = vec![
        base, hidden, masked, faded, scaled, styled, shape, clipped, invert, off, inside, folder,
        blur,
    ];
    let expected = export_report(&document);
    let (bytes, report) = exported(&document);
    assert_eq!(report, expected);
    for (item, count) in [
        (Flattened::Mask, 1),
        (Flattened::Fill, 1),
        (Flattened::Transform, 1),
        (Flattened::LayerEffects, 1),
        (Flattened::Shape, 1),
        (Flattened::Clipping, 1),
        (Flattened::Adjustment, 1),
        (Flattened::HiddenEffect, 1),
        (Flattened::Filter, 1),
        (Flattened::FolderMask, 1),
        (Flattened::Text, 0),
    ] {
        assert_eq!(report.count(item), count, "{item:?}");
    }
    let summary = report.summary().unwrap();
    assert!(summary.contains("Layer masks"), "{summary}");

    let (back, import) = open(bytes).unwrap();
    assert!(import.is_empty());
    // Everything below the filter is merged into it; the hidden layer stays, beneath.
    let names: Vec<_> = back.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["Hidden below", "Blur"]);
    assert!(!back.layers[0].visible);
    // The merged layer looks as the document did.
    let (a, b) = (render::render(&back), render::render(&document));
    for (p, q) in a.pixels().zip(b.pixels()) {
        for c in 0..4 {
            assert!(p[c].abs_diff(q[c]) <= 1, "{p:?} != {q:?}");
        }
    }
}

#[test]
fn draws_single_layers_with_their_properties_on_the_element() {
    let mut document = Document::new(6, 6).unwrap();
    let base = Layer::image("Base", pixels(6, 6, [255; 4]));
    let mut masked = Layer::image("Masked", pixels(4, 2, [255, 0, 0, 255]));
    masked.transform.x = 2.0;
    masked.transform.y = 3.0;
    let mut mask = GrayImage::from_pixel(4, 2, Luma([255]));
    mask.put_pixel(0, 0, Luma([0]));
    masked.mask = Some(Mask {
        pixels: Arc::new(mask),
        ..Mask::white()
    });
    masked.blend = BlendMode::Multiply;
    masked.opacity = 0.5;
    masked.visible = false;
    document.layers = vec![base, masked];
    let (bytes, report) = exported(&document);
    assert_eq!(report.count(Flattened::Mask), 1);
    let (back, _) = open(bytes).unwrap();
    let copy = layer(&back, "Masked");
    assert_eq!(copy.blend, BlendMode::Multiply);
    assert_eq!(copy.opacity, 0.5);
    assert!(!copy.visible);
    assert_eq!((copy.transform.x, copy.transform.y), (2.0, 3.0));
    let drawn = copy.pixels.as_deref().unwrap();
    assert_eq!(drawn.dimensions(), (4, 2));
    // The mask is applied to the pixels, which are drawn at full opacity.
    assert_eq!(drawn.get_pixel(0, 0)[3], 0);
    assert_eq!(drawn.get_pixel(1, 0).0, [255, 0, 0, 255]);
}

#[test]
fn exports_to_disk_through_io_export() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("art.ora");
    let document = layered();
    crate::io::export(&document, &path, &crate::io::ExportOptions::default()).unwrap();
    let (back, _) = load(&path, PixelBudget::default()).unwrap();
    assert_eq!(back.layers.len(), document.layers.len());
    let report = export(&document, &directory.path().join("again.ora")).unwrap();
    assert!(report.is_empty() && report.summary().is_none());
}

#[test]
fn escapes_names_for_xml() {
    assert_eq!(
        escape("a&b<c>\"d'\n\t\u{1}e"),
        "a&amp;b&lt;c&gt;&quot;d&apos;&#10;&#9;e"
    );
}
