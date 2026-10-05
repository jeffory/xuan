//! Golden-image regression tests for rendering.
//!
//! A handful of deterministic scenes are rendered through the real CPU
//! compositor (and RAW Develop pipeline) at small sizes and compared against
//! the PNGs in `testdata/goldens/`. The CPU output is the source of truth; the
//! ignored GPU test in `gpu::goldens` renders the same scenes on wgpu and
//! compares them against the same files with a looser tolerance.
//!
//! * `XUAN_UPDATE_GOLDENS=1 cargo test goldens` rewrites the goldens from the
//!   CPU renderer (only files whose pixels changed are written).
//! * On a mismatch, `expected.png`, `actual.png` and `diff.png` are written to
//!   `target/golden-failures/<backend>/<scene>/` (override the root with
//!   `XUAN_GOLDEN_FAILURES`); CI uploads that directory.
//!
//! Scenes are built with integer arithmetic where possible. The renderers use
//! `f32` transcendental functions (`sin`, `powf`, ...) from the platform's libm,
//! which may differ by an ulp between glibc and the MSVC runtime, so a pixel can
//! round one step differently on Windows. The tolerances below absorb that while
//! still catching any visible regression (a wrong blend mode, a missing mask, a
//! colour shift or a flipped layer moves thousands of pixels by tens of levels).

use std::{
    path::{Path, PathBuf},
    sync::{Arc, atomic::AtomicBool},
};

use image::{GrayImage, Luma, Rgba, RgbaImage};

use crate::{
    blend::BlendMode,
    document::{Adjustment, Document, Layer, Mask, Point, Transform},
    paint::{self, ShapeKind},
    raw::{DecodedRaw, DevelopSettings},
    text::{TextRenderer, TextStyle},
};

/// Side of the square scenes, in pixels.
const SIZE: u32 = 256;

/// How far a rendering may drift from its golden.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Tolerance {
    /// Largest per-channel difference that is not counted as a mismatch.
    pub channel: u8,
    /// Fraction of pixels allowed to exceed `channel`.
    pub budget: f32,
}

impl Tolerance {
    const fn new(channel: u8, budget: f32) -> Self {
        Self { channel, budget }
    }
}

/// Composites: identical code on Linux and Windows except libm ulps, which move
/// a value by at most one 8-bit step after rounding. Two levels leaves headroom
/// for an ulp landing next to a rounding boundary twice (e.g. in a bilinear
/// sample and again in the blend); 0.2% of 65,536 pixels is 131 pixels.
const CPU: Tolerance = Tolerance::new(2, 0.002);
/// Develop chains many `powf`/`exp`/`log` steps plus sharpening and colour
/// noise reduction, which spread a one-ulp difference to neighbours.
const CPU_RAW: Tolerance = Tolerance::new(3, 0.005);
/// GPU: different arithmetic (fused multiply-add, shader transcendental
/// functions, fixed-point texture filtering and mipmapped downscaling). On
/// Mesa lavapipe (CI's `--gpu` job) every scene is within one level of the CPU
/// goldens; hardware drivers may approximate more, so allow a few levels.
const GPU: Tolerance = Tolerance::new(4, 0.005);
/// GPU Develop chains more transcendental functions than compositing.
const GPU_RAW: Tolerance = Tolerance::new(6, 0.01);

/// The RAW fixture developed by the RAW scenes (see testdata/raw/fixtures.txt).
const RAW_FIXTURE: &str = "nikon-d70-nef";
/// Every scene name, including RAW scenes skipped without the fixture. Golden
/// files that match none of these fail the test so stale files are noticed.
const SCENES: &[&str] = &[
    "demo",
    "blend_modes",
    "layer_mask",
    "clipping_mask",
    "adjustment_layers",
    "text_and_shapes",
    "raw_default",
    "raw_negative",
    "raw_quarter_turn",
];

pub(crate) enum Content {
    /// Composite the document at the given output size.
    Composite {
        document: Document,
        width: u32,
        height: u32,
    },
    /// Develop a decoded RAW at 8 bits.
    Develop {
        raw: Arc<DecodedRaw>,
        settings: Box<DevelopSettings>,
    },
}

pub(crate) struct Scene {
    pub name: &'static str,
    pub content: Content,
    pub cpu: Tolerance,
    pub gpu: Tolerance,
}

fn composite(name: &'static str, document: Document) -> Scene {
    Scene {
        name,
        content: Content::Composite {
            width: document.width,
            height: document.height,
            document,
        },
        cpu: CPU,
        gpu: GPU,
    }
}

/// All scenes. RAW scenes are omitted (with a message) when the fixture has not
/// been fetched, unless `XUAN_REQUIRE_RAW_FIXTURES` is set, which panics.
pub(crate) fn scenes() -> Vec<Scene> {
    let mut scenes = vec![
        Scene {
            name: "demo",
            content: Content::Composite {
                document: crate::demo::document(),
                width: SIZE,
                height: SIZE * 3 / 4,
            },
            cpu: CPU,
            gpu: GPU,
        },
        composite("blend_modes", blend_modes()),
        composite("layer_mask", layer_mask()),
        composite("clipping_mask", clipping_mask()),
        composite("adjustment_layers", adjustment_layers()),
        composite("text_and_shapes", text_and_shapes()),
    ];
    if let Some(raw) = raw_fixture() {
        let raw = Arc::new(raw);
        let default = DevelopSettings::default();
        let mut negative = DevelopSettings {
            negative: crate::raw::analyze_negative(&raw, &default),
            ..Default::default()
        };
        negative.negative.enabled = true;
        let quarter_turn = DevelopSettings {
            quarter_turns: 1,
            ..Default::default()
        };
        for (name, settings) in [
            ("raw_default", default),
            ("raw_negative", negative),
            ("raw_quarter_turn", quarter_turn),
        ] {
            scenes.push(Scene {
                name,
                content: Content::Develop {
                    raw: raw.clone(),
                    settings: Box::new(settings),
                },
                cpu: CPU_RAW,
                gpu: GPU_RAW,
            });
        }
    }
    for scene in &scenes {
        assert!(
            SCENES.contains(&scene.name),
            "unlisted scene {}",
            scene.name
        );
    }
    scenes
}

/// Render a scene on the CPU. No GPU processor is current on test threads, so
/// `render_scaled` and `raw::render` take their CPU reference paths.
pub(crate) fn render_cpu(content: &Content) -> RgbaImage {
    assert!(crate::gpu::current().is_none());
    match content {
        Content::Composite {
            document,
            width,
            height,
        } => crate::render::render_scaled(document, *width, *height),
        Content::Develop { raw, settings } => {
            crate::raw::render(raw, settings, &AtomicBool::new(false)).unwrap()
        }
    }
}

// Scene construction. Pixel values use integer arithmetic so the inputs are
// bit-identical on every platform.

/// Red rises to the right, green downwards and blue towards the top left.
fn gradient() -> RgbaImage {
    RgbaImage::from_fn(SIZE, SIZE, |x, y| {
        Rgba([
            (x * 255 / (SIZE - 1)) as u8,
            (y * 255 / (SIZE - 1)) as u8,
            (255 - (x + y) * 255 / (2 * SIZE - 2)) as u8,
            255,
        ])
    })
}

/// A fully saturated hue ramp along x, desaturating towards grey along y.
fn hue_ramp(width: u32, height: u32) -> RgbaImage {
    RgbaImage::from_fn(width, height, |x, y| {
        let h = x * 1536 / width; // six 256-step sextants
        let (sextant, f) = (h / 256, h % 256);
        let rgb = match sextant {
            0 => [255, f, 0],
            1 => [255 - f, 255, 0],
            2 => [0, 255, f],
            3 => [0, 255 - f, 255],
            4 => [f, 0, 255],
            _ => [255, 0, 255 - f],
        };
        let grey = y * 255 / (height - 1).max(1);
        Rgba([
            ((rgb[0] * (255 - grey) + 128 * grey) / 255) as u8,
            ((rgb[1] * (255 - grey) + 128 * grey) / 255) as u8,
            ((rgb[2] * (255 - grey) + 128 * grey) / 255) as u8,
            255,
        ])
    })
}

fn checker(cell: u32, a: [u8; 4], b: [u8; 4]) -> RgbaImage {
    RgbaImage::from_fn(SIZE, SIZE, |x, y| {
        Rgba(if (x / cell + y / cell).is_multiple_of(2) {
            a
        } else {
            b
        })
    })
}

fn document(layers: Vec<Layer>) -> Document {
    let mut document = Document::new(SIZE, SIZE).unwrap();
    document.layers = layers;
    document
}

fn placed(mut layer: Layer, x: f32, y: f32) -> Layer {
    layer.transform.x = x;
    layer.transform.y = y;
    layer
}

/// Thirteen 19-pixel strips, one per blend mode from top to bottom in
/// `BlendMode::ALL` order, over the gradient. The right quarter of each strip
/// fades out so partial coverage is covered too.
fn blend_modes() -> Document {
    let mut layers = vec![Layer::image("Gradient", gradient())];
    for (index, mode) in BlendMode::ALL.into_iter().enumerate() {
        let mut strip = hue_ramp(SIZE, 19);
        for (x, _, pixel) in strip.enumerate_pixels_mut() {
            if x >= SIZE * 3 / 4 {
                pixel[3] = (255 - (x - SIZE * 3 / 4) * 3) as u8;
            }
        }
        let mut layer = placed(
            Layer::image(mode.name(), strip),
            0.0,
            4.0 + index as f32 * 19.0,
        );
        layer.blend = mode;
        layers.push(layer);
    }
    document(layers)
}

/// A checkerboard with a radial layer mask over the gradient, and a second
/// layer whose horizontal-ramp mask is placed independently of its pixels.
fn layer_mask() -> Document {
    let radial = GrayImage::from_fn(128, 128, |x, y| {
        let (dx, dy) = (x as i32 - 64, y as i32 - 64);
        Luma([(255 - ((dx * dx + dy * dy) * 255 / (60 * 60)).min(255)) as u8])
    });
    let mut masked = Layer::image(
        "Checker",
        checker(32, [240, 240, 230, 255], [30, 60, 90, 255]),
    );
    masked.mask = Some(Mask {
        pixels: Arc::new(radial),
        ..Mask::white()
    });
    let ramp = GrayImage::from_fn(64, 8, |x, _| Luma([(x * 255 / 63) as u8]));
    let mut band = placed(Layer::image("Band", hue_ramp(SIZE, 48)), 0.0, 200.0);
    band.opacity = 0.8;
    band.mask = Some(Mask {
        pixels: Arc::new(ramp),
        linked: false,
        placement: Some(Transform {
            x: 64.0,
            y: 180.0,
            ..Transform::new(128, 80)
        }),
        ..Mask::white()
    });
    document(vec![Layer::image("Gradient", gradient()), masked, band])
}

/// Stripes clipped to an ellipse, and a screen-blended layer clipped through
/// the stripes (clipping chains resolve to the base).
fn clipping_mask() -> Document {
    let base = paint::shape(
        Point::new(40.0, 32.0),
        Point::new(216.0, 224.0),
        ShapeKind::Ellipse,
        [250, 250, 250, 255],
        0.0,
    )
    .unwrap();
    let stripes = RgbaImage::from_fn(SIZE, SIZE, |x, y| {
        Rgba(if ((x + y) / 16).is_multiple_of(2) {
            [200, 40, 60, 255]
        } else {
            [20, 30, 120, 160]
        })
    });
    let mut clipped = Layer::image("Stripes", stripes);
    clipped.clip_to = Some(base.id);
    clipped.opacity = 0.85;
    let mut glow = placed(Layer::image("Glow", hue_ramp(128, 128)), 120.0, 16.0);
    glow.clip_to = Some(base.id);
    glow.blend = BlendMode::Screen;
    document(vec![
        Layer::image("Gradient", gradient()),
        base,
        clipped,
        glow,
    ])
}

fn adjustment(name: &str, adjustment: Adjustment) -> Layer {
    let mut layer = Layer::blank(name, SIZE, SIZE);
    layer.adjustment = Some(adjustment);
    layer
}

/// Hue/Saturation through a vertical-ramp mask, Levels at partial opacity and
/// Invert clipped to a rounded rectangle.
fn adjustment_layers() -> Document {
    let mut hue = adjustment(
        "Hue/Saturation",
        Adjustment::HueSaturation {
            hue: 120.0,
            saturation: 40.0,
            lightness: -10.0,
            colorize: false,
        },
    );
    hue.mask = Some(Mask {
        pixels: Arc::new(GrayImage::from_fn(1, 64, |_, y| {
            Luma([(255 - y * 255 / 63) as u8])
        })),
        ..Mask::white()
    });
    let mut levels = adjustment(
        "Levels",
        Adjustment::Levels {
            black: 30.0,
            gamma: 1.6,
            white: 220.0,
            output_black: 10.0,
            output_white: 245.0,
        },
    );
    levels.opacity = 0.6;
    let base = paint::shape(
        Point::new(64.0, 64.0),
        Point::new(192.0, 192.0),
        ShapeKind::RoundedRectangle,
        [255, 255, 255, 64],
        24.0,
    )
    .unwrap();
    let mut invert = adjustment("Invert", Adjustment::Invert);
    invert.clip_to = Some(base.id);
    document(vec![
        Layer::image("Gradient", gradient()),
        Layer::image(
            "Squares",
            RgbaImage::from_fn(SIZE, SIZE, |x, y| {
                Rgba(if x / 64 % 2 == y / 64 % 2 {
                    [180, 120, 60, 200]
                } else {
                    [0, 0, 0, 0]
                })
            }),
        ),
        hue,
        levels,
        base,
        invert,
    ])
}

fn text(renderer: &mut TextRenderer, style: TextStyle, x: f32, y: f32) -> Layer {
    let pixels = renderer.render(&style).unwrap();
    let mut layer = placed(Layer::image(style.layer_name(), pixels), x, y);
    layer.text = Some(style);
    layer
}

/// Each shape kind (one rotated and one translucent) and text in the bundled
/// Inter font, regular and bold italic underlined.
fn text_and_shapes() -> Document {
    let shape = |start: (f32, f32), end: (f32, f32), kind, color, radius| {
        paint::shape(
            Point::new(start.0, start.1),
            Point::new(end.0, end.1),
            kind,
            color,
            radius,
        )
        .unwrap()
    };
    let mut rotated = shape(
        (32.0, 150.0),
        (144.0, 230.0),
        ShapeKind::RoundedRectangle,
        [60, 140, 90, 255],
        18.0,
    );
    rotated.transform.rotation = 15.0;
    // The default renderer only knows the bundled Inter font, never system fonts.
    let mut renderer = TextRenderer::default();
    let regular = TextStyle {
        content: "Xuan golden".into(),
        size: 30.0,
        color: [20, 24, 40, 255],
        ..Default::default()
    };
    let emphasis = TextStyle {
        content: "Ag 0.19 fx!".into(),
        size: 26.0,
        color: [180, 30, 40, 230],
        bold: true,
        italic: true,
        underline: true,
        ..Default::default()
    };
    document(vec![
        Layer::image(
            "Paper",
            RgbaImage::from_pixel(SIZE, SIZE, Rgba([238, 232, 220, 255])),
        ),
        shape(
            (16.0, 16.0),
            (112.0, 96.0),
            ShapeKind::Rectangle,
            [40, 90, 200, 255],
            0.0,
        ),
        shape(
            (88.0, 40.0),
            (232.0, 136.0),
            ShapeKind::Ellipse,
            [240, 170, 40, 170],
            0.0,
        ),
        rotated,
        text(&mut renderer, regular, 24.0, 100.0),
        text(&mut renderer, emphasis, 120.0, 196.0),
    ])
}

/// A 256-pixel preview of the RAW fixture, or `None` when it is not fetched.
fn raw_fixture() -> Option<DecodedRaw> {
    let line = include_str!("../testdata/raw/fixtures.txt")
        .lines()
        .find(|line| line.split('|').next().map(str::trim) == Some(RAW_FIXTURE))
        .expect("golden RAW fixture is listed in testdata/raw/fixtures.txt");
    let file = line.split('|').nth(1).unwrap().trim();
    let dir = std::env::var_os("XUAN_RAW_FIXTURE_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/raw/cache"));
    let path = dir.join(file);
    match std::fs::read(&path) {
        Ok(bytes) => Some(crate::raw::decode(&bytes).unwrap().preview(SIZE)),
        Err(error) => {
            let message = format!(
                "RAW fixture {RAW_FIXTURE} missing ({}: {error}); run scripts/fetch-raw-fixtures.sh",
                path.display()
            );
            if std::env::var_os("XUAN_REQUIRE_RAW_FIXTURES").is_some_and(|v| !v.is_empty()) {
                panic!("{message}");
            }
            eprintln!("skipping RAW golden scenes: {message}");
            None
        }
    }
}

// Comparison, failure artifacts and updates.

fn golden_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/goldens")
}

fn failure_dir(backend: &str) -> PathBuf {
    std::env::var_os("XUAN_GOLDEN_FAILURES")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("target/golden-failures"))
        .join(backend)
}

fn updating() -> bool {
    std::env::var_os("XUAN_UPDATE_GOLDENS").is_some_and(|v| !v.is_empty() && v != "0")
}

fn load(path: &Path) -> Option<RgbaImage> {
    image::open(path).ok().map(|image| image.to_rgba8())
}

/// Maximally compressed PNG, so the goldens stay small in the repository.
fn save(image: &RgbaImage, path: &Path) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    let mut encoder = png::Encoder::new(file, image.width(), image.height());
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_compression(png::Compression::High);
    encoder.set_filter(png::Filter::Adaptive);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(image.as_raw()).unwrap();
    writer.finish().unwrap();
}

/// The largest per-channel difference of a pixel. Colour is ignored where both
/// pixels are fully transparent.
fn pixel_difference(a: &Rgba<u8>, b: &Rgba<u8>) -> u8 {
    let channels = if a[3] == 0 && b[3] == 0 { 3..4 } else { 0..4 };
    channels.map(|c| a[c].abs_diff(b[c])).max().unwrap()
}

/// Mismatches are red (brighter for larger errors), differences within the
/// tolerance are yellow and identical pixels are a dimmed copy of the golden.
fn diff_image(expected: &RgbaImage, actual: &RgbaImage, tolerance: Tolerance) -> RgbaImage {
    RgbaImage::from_fn(expected.width(), expected.height(), |x, y| {
        let (e, a) = (expected.get_pixel(x, y), actual.get_pixel(x, y));
        let difference = pixel_difference(e, a);
        if difference > tolerance.channel {
            Rgba([128u8.saturating_add(difference / 2).max(160), 0, 0, 255])
        } else if difference > 0 {
            Rgba([255, 220, 0, 255])
        } else {
            let luma = (u32::from(e[0]) * 2 + u32::from(e[1]) * 5 + u32::from(e[2])) / 8;
            let v = (luma * u32::from(e[3]) / 255 / 4 + 32) as u8;
            Rgba([v, v, v, 255])
        }
    })
}

/// Compare one rendering with its golden. Returns a description of the failure,
/// after writing `expected.png`, `actual.png` and `diff.png` for inspection.
fn check(
    name: &str,
    actual: &RgbaImage,
    tolerance: Tolerance,
    goldens: &Path,
    failures: &Path,
) -> Option<String> {
    let path = goldens.join(format!("{name}.png"));
    let Some(expected) = load(&path) else {
        let dir = failures.join(name);
        save(actual, &dir.join("actual.png"));
        return Some(format!(
            "{name}: no golden at {} (create it with XUAN_UPDATE_GOLDENS=1 cargo test goldens); \
             rendering written to {}",
            path.display(),
            dir.display()
        ));
    };
    let problem = if expected.dimensions() != actual.dimensions() {
        format!(
            "{name}: size {:?} differs from the golden's {:?}",
            actual.dimensions(),
            expected.dimensions()
        )
    } else {
        let mut over = 0usize;
        let mut worst = (0u8, 0, 0);
        for (x, y, e) in expected.enumerate_pixels() {
            let difference = pixel_difference(e, actual.get_pixel(x, y));
            if difference > tolerance.channel {
                over += 1;
            }
            if difference > worst.0 {
                worst = (difference, x, y);
            }
        }
        let allowed = (tolerance.budget * expected.len() as f32 / 4.0) as usize;
        if over <= allowed {
            if worst.0 > 0 {
                eprintln!(
                    "{name}: {over} pixels over {} (allowed {allowed}), largest difference {} at {},{}",
                    tolerance.channel, worst.0, worst.1, worst.2
                );
            }
            return None;
        }
        format!(
            "{name}: {over} pixels differ by more than {} (allowed {allowed}); largest difference {} at {},{}: expected {:?}, got {:?}",
            tolerance.channel,
            worst.0,
            worst.1,
            worst.2,
            expected.get_pixel(worst.1, worst.2).0,
            actual.get_pixel(worst.1, worst.2).0,
        )
    };
    let dir = failures.join(name);
    save(&expected, &dir.join("expected.png"));
    save(actual, &dir.join("actual.png"));
    if expected.dimensions() == actual.dimensions() {
        save(
            &diff_image(&expected, actual, tolerance),
            &dir.join("diff.png"),
        );
    }
    Some(format!("{problem}; images written to {}", dir.display()))
}

/// Render every scene with `render` and compare it with its golden. With
/// `update`, goldens whose pixels changed are rewritten instead.
pub(crate) fn run(
    backend: &str,
    update: bool,
    tolerance: impl Fn(&Scene) -> Tolerance,
    mut render: impl FnMut(&Content) -> RgbaImage,
) {
    let failure_dir = failure_dir(backend);
    let _ = std::fs::remove_dir_all(&failure_dir);
    let mut failures = Vec::new();
    for scene in scenes() {
        let actual = render(&scene.content);
        if update {
            let path = golden_dir().join(format!("{}.png", scene.name));
            if load(&path).as_ref() != Some(&actual) {
                save(&actual, &path);
                eprintln!("updated {}", path.display());
            }
        } else if let Some(failure) = check(
            scene.name,
            &actual,
            tolerance(&scene),
            &golden_dir(),
            &failure_dir,
        ) {
            failures.push(failure);
        }
    }
    assert!(
        failures.is_empty(),
        "{} golden image(s) differ on {backend}:\n{}\n\
         If the change is intended, run XUAN_UPDATE_GOLDENS=1 cargo test goldens \
         and review the PNG diff.",
        failures.len(),
        failures.join("\n")
    );
}

#[test]
fn cpu_scenes_match_goldens() {
    run("cpu", updating(), |scene| scene.cpu, render_cpu);
}

#[test]
fn every_golden_belongs_to_a_scene() {
    for entry in std::fs::read_dir(golden_dir()).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_stem().unwrap().to_string_lossy();
        assert!(
            path.extension().is_some_and(|e| e == "png") && SCENES.contains(&name.as_ref()),
            "{} is not a golden scene; remove it or add the scene",
            path.display()
        );
    }
}

#[test]
fn comparison_applies_the_tolerance_and_writes_failure_images() {
    let tmp = tempfile::tempdir().unwrap();
    let (goldens, failures) = (tmp.path().join("goldens"), tmp.path().join("failures"));
    let expected = RgbaImage::from_pixel(10, 10, Rgba([100, 100, 100, 255]));
    let mut actual = expected.clone();
    actual.put_pixel(0, 0, Rgba([103, 100, 100, 255]));
    actual.put_pixel(1, 0, Rgba([102, 100, 100, 255]));

    // A missing golden names the update command and keeps the rendering.
    let failure = check("scene", &actual, CPU, &goldens, &failures).unwrap();
    assert!(failure.contains("XUAN_UPDATE_GOLDENS"), "{failure}");
    assert!(failures.join("scene/actual.png").exists());

    // PNGs round-trip exactly.
    save(&expected, &goldens.join("scene.png"));
    assert_eq!(load(&goldens.join("scene.png")).unwrap(), expected);

    // One pixel over the channel tolerance: fails without a budget, passes with 1%.
    let strict = Tolerance::new(2, 0.0);
    let failure = check("scene", &actual, strict, &goldens, &failures).unwrap();
    assert!(failure.starts_with("scene: 1 pixels"), "{failure}");
    let diff = load(&failures.join("scene/diff.png")).unwrap();
    assert_eq!(diff.get_pixel(0, 0).0, [160, 0, 0, 255]);
    assert_eq!(diff.get_pixel(1, 0).0, [255, 220, 0, 255]);
    assert_eq!(diff.get_pixel(2, 0)[0], diff.get_pixel(2, 0)[1]);
    assert!(failures.join("scene/expected.png").exists());
    assert!(
        check(
            "scene",
            &actual,
            Tolerance::new(2, 0.01),
            &goldens,
            &failures
        )
        .is_none()
    );

    // A size change always fails; colour under full transparency is ignored.
    let small = RgbaImage::new(5, 5);
    assert!(check("scene", &small, CPU, &goldens, &failures).is_some());
    assert_eq!(
        pixel_difference(&Rgba([255, 0, 0, 0]), &Rgba([0, 255, 0, 0])),
        0
    );
    assert_eq!(
        pixel_difference(&Rgba([255, 0, 0, 1]), &Rgba([0, 255, 0, 0])),
        255
    );
}
