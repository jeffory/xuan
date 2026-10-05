//! Photoshop import tests. Every fixture is written here, by a small PSD/PSB writer that follows
//! the same specification sections the reader does.
use std::io::Write;

use super::*;
use crate::render;

// ---------------------------------------------------------------------------------------------
// Writer

#[derive(Clone)]
struct MaskSpec {
    rect: (i32, i32, i32, i32),
    default: u8,
    flags: u8,
    plane: Vec<u8>,
}

#[derive(Clone)]
struct LayerSpec {
    name: String,
    /// top, left, bottom, right
    rect: (i32, i32, i32, i32),
    /// Channel ID and its uncompressed plane.
    channels: Vec<(i16, Vec<u8>)>,
    compression: u16,
    blend: [u8; 4],
    opacity: u8,
    clipping: bool,
    flags: u8,
    mask: Option<MaskSpec>,
    extra: Vec<([u8; 4], Vec<u8>)>,
}

impl LayerSpec {
    fn blank(name: &str) -> Self {
        Self {
            name: name.into(),
            rect: (0, 0, 0, 0),
            channels: Vec::new(),
            compression: 0,
            blend: *b"norm",
            opacity: 255,
            clipping: false,
            flags: 0,
            mask: None,
            extra: Vec::new(),
        }
    }

    fn pixels(
        name: &str,
        (left, top): (i32, i32),
        (width, height): (u32, u32),
        color: impl Fn(u32, u32) -> [u8; 4],
    ) -> Self {
        let mut planes = vec![Vec::new(); 4];
        for y in 0..height {
            for x in 0..width {
                let c = color(x, y);
                for (i, plane) in planes.iter_mut().enumerate() {
                    plane.push(c[i]);
                }
            }
        }
        let mut layer = Self::blank(name);
        layer.rect = (top, left, top + height as i32, left + width as i32);
        layer.channels = vec![
            (-1, planes[3].clone()),
            (0, planes[0].clone()),
            (1, planes[1].clone()),
            (2, planes[2].clone()),
        ];
        layer
    }

    fn solid(name: &str, origin: (i32, i32), size: (u32, u32), color: [u8; 4]) -> Self {
        Self::pixels(name, origin, size, |_, _| color)
    }

    fn divider() -> Self {
        let mut layer = Self::blank("</Layer group>");
        layer.extra.push((*b"lsct", 3_u32.to_be_bytes().to_vec()));
        layer
    }

    fn folder(name: &str, blend: &[u8; 4]) -> Self {
        let mut layer = Self::blank(name);
        let mut data = 1_u32.to_be_bytes().to_vec();
        data.extend(b"8BIM");
        data.extend(blend);
        layer.extra.push((*b"lsct", data));
        layer.blend = *b"norm";
        layer
    }

    fn with(mut self, key: &[u8; 4], data: Vec<u8>) -> Self {
        self.extra.push((*key, data));
        self
    }

    fn size(&self) -> (usize, usize) {
        let (top, left, bottom, right) = self.rect;
        ((right - left) as usize, (bottom - top) as usize)
    }
}

#[derive(Clone)]
struct PsdSpec {
    psb: bool,
    width: u32,
    height: u32,
    channels: u16,
    depth: u16,
    mode: u16,
    resolution: Option<u32>,
    global_angle: Option<i32>,
    layers: Vec<LayerSpec>,
    merged_compression: u16,
    merged: Option<Vec<Vec<u8>>>,
}

impl PsdSpec {
    fn new(width: u32, height: u32) -> Self {
        Self {
            psb: false,
            width,
            height,
            channels: 3,
            depth: 8,
            mode: 3,
            resolution: None,
            global_angle: None,
            layers: Vec::new(),
            merged_compression: 0,
            merged: None,
        }
    }

    fn layers(width: u32, height: u32, layers: Vec<LayerSpec>) -> Self {
        Self {
            layers,
            ..Self::new(width, height)
        }
    }
}

fn length(out: &mut Vec<u8>, value: usize, long: bool) {
    if long {
        out.extend((value as u64).to_be_bytes());
    } else {
        out.extend((value as u32).to_be_bytes());
    }
}

fn packbits(row: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < row.len() {
        let mut run = 1;
        while i + run < row.len() && run < 128 && row[i + run] == row[i] {
            run += 1;
        }
        if run >= 2 {
            out.push((1 - run as i32) as i8 as u8);
            out.push(row[i]);
            i += run;
        } else {
            let start = i;
            let mut count = 0;
            while i < row.len() && count < 128 && !(i + 1 < row.len() && row[i + 1] == row[i]) {
                i += 1;
                count += 1;
            }
            if count == 0 {
                i += 1;
                count = 1;
            }
            out.push((count - 1) as u8);
            out.extend(&row[start..start + count]);
        }
    }
    out
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

/// A channel as stored: compression, then data.
fn encode(compression: u16, plane: &[u8], width: usize, height: usize, psb: bool) -> Vec<u8> {
    let mut out = compression.to_be_bytes().to_vec();
    match compression {
        0 => out.extend(plane),
        1 => {
            let rows: Vec<Vec<u8>> = (0..height)
                .map(|y| packbits(&plane[y * width..(y + 1) * width]))
                .collect();
            // Row byte counts: 2 bytes each, 4 in PSB files.
            for row in &rows {
                if psb {
                    out.extend((row.len() as u32).to_be_bytes());
                } else {
                    out.extend((row.len() as u16).to_be_bytes());
                }
            }
            for row in rows {
                out.extend(row);
            }
        }
        2 => out.extend(zlib(plane)),
        3 => {
            let mut deltas = plane.to_vec();
            for y in 0..height {
                for x in (1..width).rev() {
                    let i = y * width + x;
                    deltas[i] = plane[i].wrapping_sub(plane[i - 1]);
                }
            }
            out.extend(zlib(&deltas));
        }
        other => out.extend(vec![other as u8; 8]),
    }
    out
}

const LONG_KEYS: [&[u8; 4]; 13] = PSB_LONG_KEYS;

fn record(layer: &LayerSpec, psb: bool) -> (Vec<u8>, Vec<u8>) {
    let (top, left, bottom, right) = layer.rect;
    let (width, height) = layer.size();
    let mut channels = Vec::new();
    for (id, plane) in &layer.channels {
        channels.push((*id, encode(layer.compression, plane, width, height, psb)));
    }
    if let Some(mask) = &layer.mask {
        let (t, l, b, r) = mask.rect;
        let (w, h) = ((r - l) as usize, (b - t) as usize);
        channels.push((-2, encode(layer.compression, &mask.plane, w, h, psb)));
    }
    let mut out = Vec::new();
    for v in [top, left, bottom, right] {
        out.extend(v.to_be_bytes());
    }
    out.extend((channels.len() as u16).to_be_bytes());
    for (id, data) in &channels {
        out.extend(id.to_be_bytes());
        length(&mut out, data.len(), psb);
    }
    out.extend(b"8BIM");
    out.extend(layer.blend);
    out.push(layer.opacity);
    out.push(layer.clipping as u8);
    out.push(layer.flags);
    out.push(0);
    let mut extra = Vec::new();
    match &layer.mask {
        Some(mask) => {
            extra.extend(20_u32.to_be_bytes());
            let (t, l, b, r) = mask.rect;
            for v in [t, l, b, r] {
                extra.extend(v.to_be_bytes());
            }
            extra.push(mask.default);
            extra.push(mask.flags);
            extra.extend([0, 0]);
        }
        None => extra.extend(0_u32.to_be_bytes()),
    }
    extra.extend(0_u32.to_be_bytes());
    let name = layer.name.as_bytes();
    let name = &name[..name.len().min(255)];
    extra.push(name.len() as u8);
    extra.extend(name);
    while !extra.len().is_multiple_of(4) {
        extra.push(0);
    }
    for (key, data) in &layer.extra {
        extra.extend(b"8BIM");
        extra.extend(key);
        length(&mut extra, data.len(), psb && LONG_KEYS.contains(&key));
        extra.extend(data);
        if data.len() % 2 == 1 {
            extra.push(0);
        }
    }
    out.extend((extra.len() as u32).to_be_bytes());
    out.extend(extra);
    let data = channels.into_iter().flat_map(|(_, d)| d).collect();
    (out, data)
}

/// The file, and where its image data section (the merged image) starts.
fn write(spec: &PsdSpec) -> (Vec<u8>, usize) {
    let mut out = b"8BPS".to_vec();
    out.extend((if spec.psb { 2_u16 } else { 1 }).to_be_bytes());
    out.extend([0; 6]);
    out.extend(spec.channels.to_be_bytes());
    out.extend(spec.height.to_be_bytes());
    out.extend(spec.width.to_be_bytes());
    out.extend(spec.depth.to_be_bytes());
    out.extend(spec.mode.to_be_bytes());
    out.extend(0_u32.to_be_bytes());
    let mut resources = Vec::new();
    if let Some(ppi) = spec.resolution {
        resources.extend(b"8BIM");
        resources.extend(1005_u16.to_be_bytes());
        resources.extend([0, 0]);
        resources.extend(16_u32.to_be_bytes());
        for _ in 0..2 {
            resources.extend((ppi << 16).to_be_bytes());
            resources.extend([0, 1, 0, 1]);
        }
    }
    if let Some(angle) = spec.global_angle {
        resources.extend(b"8BIM");
        resources.extend(1037_u16.to_be_bytes());
        resources.extend([0, 0]);
        resources.extend(4_u32.to_be_bytes());
        resources.extend(angle.to_be_bytes());
    }
    out.extend((resources.len() as u32).to_be_bytes());
    out.extend(resources);
    if spec.layers.is_empty() {
        length(&mut out, 0, spec.psb);
    } else {
        let mut info = (spec.layers.len() as i16).to_be_bytes().to_vec();
        let mut data = Vec::new();
        for layer in &spec.layers {
            let (header, channels) = record(layer, spec.psb);
            info.extend(header);
            data.extend(channels);
        }
        info.extend(data);
        if info.len() % 2 == 1 {
            info.push(0);
        }
        let mut section = Vec::new();
        length(&mut section, info.len(), spec.psb);
        section.extend(info);
        section.extend(0_u32.to_be_bytes());
        length(&mut out, section.len(), spec.psb);
        out.extend(section);
    }
    let image_data = out.len();
    let (w, h) = (spec.width as usize, spec.height as usize);
    let planes = spec
        .merged
        .clone()
        .unwrap_or_else(|| vec![vec![255; w * h]; usize::from(spec.channels)]);
    out.extend(spec.merged_compression.to_be_bytes());
    if spec.merged_compression == 1 {
        let rows: Vec<Vec<u8>> = planes
            .iter()
            .flat_map(|p| (0..h).map(move |y| packbits(&p[y * w..(y + 1) * w])))
            .collect();
        for row in &rows {
            if spec.psb {
                out.extend((row.len() as u32).to_be_bytes());
            } else {
                out.extend((row.len() as u16).to_be_bytes());
            }
        }
        for row in rows {
            out.extend(row);
        }
    } else {
        for plane in planes {
            out.extend(plane);
        }
    }
    (out, image_data)
}

fn bytes(spec: &PsdSpec) -> Vec<u8> {
    write(spec).0
}

fn open(spec: &PsdSpec) -> (Document, ImportReport) {
    read(&bytes(spec), PixelBudget::default()).unwrap()
}

fn error(bytes: &[u8]) -> String {
    format!("{:#}", read(bytes, PixelBudget::default()).unwrap_err())
}

// Descriptors ---------------------------------------------------------------------------------

enum D {
    Doub(f64),
    Unit(f64),
    Long(i32),
    Bool(bool),
    Text(&'static str),
    Enum(&'static str, &'static str),
    Obj(&'static str, Vec<(&'static str, D)>),
    List(Vec<D>),
    Raw(Vec<u8>),
}

fn id(out: &mut Vec<u8>, key: &str) {
    if key.len() == 4 {
        out.extend(0_u32.to_be_bytes());
    } else {
        out.extend((key.len() as u32).to_be_bytes());
    }
    out.extend(key.as_bytes());
}

fn unicode(out: &mut Vec<u8>, text: &str) {
    let units: Vec<u16> = text.encode_utf16().collect();
    out.extend((units.len() as u32).to_be_bytes());
    for unit in units {
        out.extend(unit.to_be_bytes());
    }
}

fn value(out: &mut Vec<u8>, value: &D) {
    match value {
        D::Doub(v) => {
            out.extend(b"doub");
            out.extend(v.to_be_bytes());
        }
        D::Unit(v) => {
            out.extend(b"UntF#Pxl");
            out.extend(v.to_be_bytes());
        }
        D::Long(v) => {
            out.extend(b"long");
            out.extend(v.to_be_bytes());
        }
        D::Bool(v) => {
            out.extend(b"bool");
            out.push(*v as u8);
        }
        D::Text(v) => {
            out.extend(b"TEXT");
            unicode(out, v);
        }
        D::Enum(kind, v) => {
            out.extend(b"enum");
            id(out, kind);
            id(out, v);
        }
        D::Obj(class, items) => {
            out.extend(b"Objc");
            out.extend(descriptor_bytes(class, items));
        }
        D::List(values) => {
            out.extend(b"VlLs");
            out.extend((values.len() as u32).to_be_bytes());
            for v in values {
                self::value(out, v);
            }
        }
        D::Raw(data) => {
            out.extend(b"tdta");
            out.extend((data.len() as u32).to_be_bytes());
            out.extend(data);
        }
    }
}

fn descriptor_bytes(class: &str, items: &[(&str, D)]) -> Vec<u8> {
    let mut out = Vec::new();
    unicode(&mut out, "");
    id(&mut out, class);
    out.extend((items.len() as u32).to_be_bytes());
    for (key, v) in items {
        id(&mut out, key);
        value(&mut out, v);
    }
    out
}

/// A versioned descriptor (version 16), optionally after a block version.
fn block(version: Option<u32>, class: &str, items: Vec<(&'static str, D)>) -> Vec<u8> {
    let mut out = Vec::new();
    if let Some(version) = version {
        out.extend(version.to_be_bytes());
    }
    out.extend(16_u32.to_be_bytes());
    out.extend(descriptor_bytes(class, &items));
    out
}

fn solid_color(r: f64, g: f64, b: f64) -> Vec<u8> {
    block(
        None,
        "null",
        vec![(
            "Clr ",
            D::Obj(
                "RGBC",
                vec![
                    ("Rd  ", D::Doub(r)),
                    ("Grn ", D::Doub(g)),
                    ("Bl  ", D::Doub(b)),
                ],
            ),
        )],
    )
}

fn origination(
    kind: i32,
    (left, top, right, bottom): (f64, f64, f64, f64),
    radius: f64,
) -> Vec<u8> {
    let mut origin = vec![
        ("keyOriginType", D::Long(kind)),
        (
            "keyOriginShapeBBox",
            D::Obj(
                "unitRect",
                vec![
                    ("Top ", D::Unit(top)),
                    ("Left", D::Unit(left)),
                    ("Btom", D::Unit(bottom)),
                    ("Rght", D::Unit(right)),
                ],
            ),
        ),
    ];
    if kind == 2 {
        origin.push((
            "keyOriginRRectRadii",
            D::Obj(
                "radii",
                vec![
                    ("topRight", D::Unit(radius)),
                    ("topLeft", D::Unit(radius)),
                    ("bottomLeft", D::Unit(radius)),
                    ("bottomRight", D::Unit(radius)),
                ],
            ),
        ));
    }
    block(
        Some(1),
        "null",
        vec![("keyDescriptorList", D::List(vec![D::Obj("null", origin)]))],
    )
}

fn engine_string(text: &str) -> Vec<u8> {
    let mut out = b"(\xFE\xFF".to_vec();
    for unit in text.encode_utf16() {
        for byte in unit.to_be_bytes() {
            if matches!(byte, b'(' | b')' | b'\\') {
                out.push(b'\\');
            }
            out.push(byte);
        }
    }
    out.push(b')');
    out
}

struct TextSpec {
    content: &'static str,
    vertical: bool,
    scale: f64,
    rotation: f64,
    size: f64,
    second_size: Option<f64>,
    justification: u8,
    warp: &'static str,
}

impl Default for TextSpec {
    fn default() -> Self {
        Self {
            content: "Hello\rWorld",
            vertical: false,
            scale: 1.0,
            rotation: 0.0,
            size: 24.0,
            second_size: None,
            justification: 0,
            warp: "warpNone",
        }
    }
}

fn type_tool(spec: &TextSpec) -> Vec<u8> {
    let run = |size: f64| {
        format!(
            "<< /StyleSheet << /StyleSheetData << /Font 0 /FontSize {size} /FauxBold true \
             /FillColor << /Type 1 /Values [ 1.0 1.0 .5 0.0 ] >> >> >> >>"
        )
    };
    let mut runs = run(spec.size);
    if let Some(size) = spec.second_size {
        runs.push(' ');
        runs.push_str(&run(size));
    }
    let mut engine = b"\n\n<<\n\t/EngineDict\n\t<<\n\t\t/Editor << /Text ".to_vec();
    engine.extend(engine_string(&format!("{}\r", spec.content)));
    engine.extend(
        format!(
            " >>\n\t\t/ParagraphRun << /RunArray [ << /ParagraphSheet << /Properties << \
             /Justification {} >> >> >> ] >>\n\t\t/StyleRun << /RunArray [ {runs} ] >>\n\t\t\
             /Rendered << /Shapes << /Children [ << /ShapeType 0 >> ] >> >>\n\t>>\n\t\
             /ResourceDict << /FontSet [ << /Name ",
            spec.justification
        )
        .as_bytes(),
    );
    engine.extend(engine_string("Arial-BoldItalicMT"));
    engine.extend(b" /Type 1 >> ] >>\n>>");
    let (sin, cos) = spec.rotation.to_radians().sin_cos();
    let mut out = 1_u16.to_be_bytes().to_vec();
    for v in [
        cos * spec.scale,
        sin * spec.scale,
        -sin * spec.scale,
        cos * spec.scale,
        10.0,
        20.0,
    ] {
        out.extend(v.to_be_bytes());
    }
    out.extend(50_u16.to_be_bytes());
    out.extend(block(
        None,
        "TxLr",
        vec![
            ("Txt ", D::Text(spec.content)),
            (
                "Ornt",
                D::Enum("Ornt", if spec.vertical { "Vrtc" } else { "Hrzn" }),
            ),
            ("EngineData", D::Raw(engine)),
        ],
    ));
    out.extend(1_u16.to_be_bytes());
    out.extend(block(
        None,
        "warp",
        vec![("warpStyle", D::Enum("warpStyle", spec.warp))],
    ));
    out.extend([0; 16]);
    out
}

// ---------------------------------------------------------------------------------------------
// Tests

fn red_gradient(x: u32, y: u32) -> [u8; 4] {
    [200, (x * 40) as u8, (y * 30) as u8, 255]
}

#[test]
fn reads_a_layered_rgb_file() {
    let mut top = LayerSpec::solid("Top", (2, 1), (2, 3), [10, 20, 30, 128]);
    top.opacity = 128;
    top.flags = 2;
    let unicode = {
        let mut data = Vec::new();
        unicode(&mut data, "图层 ✓");
        data
    };
    let mut spec = PsdSpec::layers(
        6,
        5,
        vec![
            LayerSpec::pixels("Back", (0, 0), (6, 5), red_gradient).with(b"luni", unicode),
            top,
        ],
    );
    spec.resolution = Some(300);
    let (document, report) = open(&spec);
    assert!(report.is_empty(), "{:?}", report.summary());
    assert_eq!(report.source(), ImportSource::Photoshop);
    assert_eq!((document.width, document.height), (6, 5));
    assert_eq!(document.resolution, 300.0);
    assert_eq!(document.layers.len(), 2);
    let [back, top] = &document.layers[..] else {
        unreachable!()
    };
    assert_eq!(back.name, "图层 ✓");
    let pixels = back.pixels.as_ref().unwrap();
    assert_eq!(pixels.dimensions(), (6, 5));
    assert_eq!(pixels.get_pixel(3, 2).0, red_gradient(3, 2));
    assert_eq!(top.name, "Top");
    assert!(!top.visible && back.visible);
    assert!((top.opacity - 128.0 / 255.0).abs() < 1e-6);
    assert_eq!((top.transform.x, top.transform.y), (2.0, 1.0));
    assert_eq!((top.transform.width, top.transform.height), (2.0, 3.0));
    assert_eq!(
        top.pixels.as_ref().unwrap().get_pixel(1, 2).0,
        [10, 20, 30, 128]
    );
    // The topmost root layer is active, as in upstream.
    assert_eq!(document.active, Some(top.id));
}

#[test]
fn legacy_names_and_fill_opacity() {
    let layer = LayerSpec::solid("Caf_", (0, 0), (1, 1), [1, 2, 3, 255]);
    let spec = PsdSpec::layers(1, 1, vec![layer.with(b"iOpa", vec![128])]);
    let mut file = bytes(&spec);
    // Legacy names are Mac OS Roman: 0x8E is é.
    let at = file.windows(5).position(|w| w == b"\x04Caf_").unwrap();
    file[at + 4] = 0x8E;
    let (document, _) = read(&file, PixelBudget::default()).unwrap();
    assert_eq!(document.layers[0].name, "Café");
    assert!((document.layers[0].opacity - 128.0 / 255.0).abs() < 1e-6);
    assert_eq!(MAC_ROMAN.chars().count(), 128);
}

#[test]
fn folders_nest_and_keep_opacity() {
    let mut outer = LayerSpec::folder("Outer", b"pass");
    outer.opacity = 128;
    let spec = PsdSpec::layers(
        4,
        4,
        vec![
            LayerSpec::solid("Bottom", (0, 0), (4, 4), [0, 0, 255, 255]),
            LayerSpec::divider(),
            LayerSpec::solid("In outer", (0, 0), (2, 2), [255, 0, 0, 255]),
            LayerSpec::divider(),
            LayerSpec::solid("In inner", (1, 1), (2, 2), [0, 255, 0, 255]),
            LayerSpec::folder("Inner", b"norm"),
            outer,
            LayerSpec::folder("Multiply folder", b"mul "),
        ],
    );
    let mut spec = spec;
    // The last folder has no divider of its own; it is read as an empty folder.
    let (document, report) = open(&spec);
    let names: Vec<_> = document.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "Bottom",
            "In outer",
            "In inner",
            "Inner",
            "Outer",
            "Multiply folder"
        ]
    );
    let find = |name| document.layers.iter().find(|l| l.name == name).unwrap();
    let (outer, inner) = (find("Outer"), find("Inner"));
    assert!(outer.group && inner.group && find("Multiply folder").group);
    assert_eq!(outer.parent, None);
    assert_eq!(inner.parent, Some(outer.id));
    assert_eq!(find("In outer").parent, Some(outer.id));
    assert_eq!(find("In inner").parent, Some(inner.id));
    assert_eq!(find("Bottom").parent, None);
    assert!((outer.opacity - 128.0 / 255.0).abs() < 1e-6);
    assert_eq!(report.count(Dropped::FolderBlendMode("Multiply")), 1);
    assert_eq!(report.lines().len(), 1);
    // A folder's opacity dims what is inside it.
    let image = render::render(&document);
    assert!(image.get_pixel(0, 0)[0] > 100 && image.get_pixel(0, 0)[2] > 100);

    // Folders deeper than Xuan allows are refused; the deepest allowed nesting opens.
    for (depth, ok) in [(MAX_FOLDER_DEPTH, true), (MAX_FOLDER_DEPTH + 1, false)] {
        let mut layers = vec![LayerSpec::divider(); depth];
        layers.push(LayerSpec::solid("Deep", (0, 0), (1, 1), [0; 4]));
        layers.extend((0..depth).map(|i| LayerSpec::folder(&format!("F{i}"), b"pass")));
        spec.layers = layers;
        let result = read(&bytes(&spec), PixelBudget::default());
        assert_eq!(result.is_ok(), ok, "depth {depth}");
        if !ok {
            assert!(format!("{:#}", result.unwrap_err()).contains("nested too deeply"));
        }
    }
    // A folder that is never closed marks a damaged file.
    spec.layers = vec![
        LayerSpec::divider(),
        LayerSpec::solid("Orphan", (0, 0), (1, 1), [0; 4]),
    ];
    assert!(error(&bytes(&spec)).contains("damaged"));
}

#[test]
fn layer_masks_keep_their_placement_and_flags() {
    let layer = |name: &str, mask: MaskSpec| {
        let mut layer = LayerSpec::solid(name, (2, 2), (4, 4), [255, 255, 255, 255]);
        layer.mask = Some(mask);
        layer
    };
    let spec = PsdSpec::layers(
        8,
        8,
        vec![
            // White outside, a black 2×2 patch at (3, 3).
            layer(
                "White",
                MaskSpec {
                    rect: (3, 3, 5, 5),
                    default: 255,
                    flags: 0,
                    plane: vec![0; 4],
                },
            ),
            // Black outside, a white 2×2 patch at (0, 0) partly outside the layer, unlinked.
            layer(
                "Black",
                MaskSpec {
                    rect: (0, 0, 3, 3),
                    default: 0,
                    flags: 1,
                    plane: vec![255; 9],
                },
            ),
            // Disabled.
            layer(
                "Disabled",
                MaskSpec {
                    rect: (2, 2, 6, 6),
                    default: 0,
                    flags: 2,
                    plane: vec![0; 16],
                },
            ),
            // Rendered from a vector mask: left out.
            layer(
                "Vector",
                MaskSpec {
                    rect: (2, 2, 6, 6),
                    default: 0,
                    flags: 8,
                    plane: vec![0; 16],
                },
            ),
        ],
    );
    let (document, report) = open(&spec);
    let mask = |i: usize| document.layers[i].mask.as_ref();
    let white = mask(0).unwrap();
    assert!(white.enabled && white.linked);
    // On the layer's own pixels: 4×4 at (2, 2), black where the patch is.
    assert_eq!(
        white.placement,
        Some(
            Rect {
                left: 2,
                top: 2,
                right: 6,
                bottom: 6
            }
            .transform()
        )
    );
    assert_eq!(white.pixels.get_pixel(0, 0)[0], 255);
    assert_eq!(white.pixels.get_pixel(1, 1)[0], 0);
    assert_eq!(white.pixels.get_pixel(2, 2)[0], 0);
    assert_eq!(white.pixels.get_pixel(3, 3)[0], 255);
    let black = mask(1).unwrap();
    assert!(!black.linked);
    assert_eq!(black.pixels.dimensions(), (3, 3));
    assert_eq!(
        black.placement,
        Some(
            Rect {
                left: 0,
                top: 0,
                right: 3,
                bottom: 3
            }
            .transform()
        )
    );
    assert!(!mask(2).unwrap().enabled);
    assert!(mask(3).is_none());
    assert_eq!(report.count(Dropped::VectorMask), 1);

    // Composited: the patch hides the first layer; the second shows only inside its patch.
    let mut only = document.clone();
    only.layers.truncate(1);
    let image = render::render(&only);
    assert_eq!(image.get_pixel(2, 2)[3], 255);
    assert_eq!(image.get_pixel(4, 4)[3], 0);
    only.layers = vec![document.layers[1].clone()];
    let image = render::render(&only);
    assert_eq!(image.get_pixel(2, 2)[3], 255);
    assert_eq!(image.get_pixel(4, 4)[3], 0);
}

#[test]
fn blend_modes_map_through_one_table_and_report_the_rest() {
    let mut layers: Vec<_> = BLEND_MODES
        .iter()
        .map(|(key, name, _)| {
            let mut layer = LayerSpec::solid(name, (0, 0), (1, 1), [9, 9, 9, 255]);
            layer.blend = *key;
            layer
        })
        .collect();
    let mut unknown = LayerSpec::solid("Unknown", (0, 0), (1, 1), [9, 9, 9, 255]);
    unknown.blend = *b"zzzz";
    layers.push(unknown);
    let (document, report) = open(&PsdSpec::layers(1, 1, layers));
    for ((_, name, mode), layer) in BLEND_MODES.iter().zip(&document.layers) {
        assert_eq!(layer.blend, *mode, "{name}");
        // Photoshop's names are Xuan's (and upstream's) names.
        if *name != "Pass Through" {
            assert_eq!(mode.name(), *name);
        }
    }
    // Every mode Xuan has is reachable from a Photoshop key.
    for mode in BlendMode::ALL {
        assert!(BLEND_MODES.iter().any(|(_, _, m)| *m == mode), "{mode:?}");
    }
    // Only the unknown key is reported.
    assert_eq!(document.layers.last().unwrap().blend, BlendMode::Normal);
    assert_eq!(report.lines().len(), 1);
    assert_eq!(
        report.count(Dropped::PhotoshopBlendMode(UNKNOWN_BLEND_MODE)),
        1
    );
    let summary = report.summary().unwrap();
    assert!(summary.contains("Photoshop file"), "{summary}");
    assert!(
        summary.contains("“Unknown” (drawn as Normal): 1"),
        "{summary}"
    );
}

#[test]
fn compressed_channels_decode_like_raw_ones() {
    let mut masked = LayerSpec::pixels("Layer", (1, 1), (300, 7), |x, y| {
        // Long runs and noise, so PackBits uses both kinds of run.
        if x < 140 {
            [5, 6, 7, 255]
        } else {
            [
                (x * 7 + y) as u8,
                (x ^ y) as u8,
                (x / 3) as u8,
                (x + y) as u8,
            ]
        }
    });
    masked.mask = Some(MaskSpec {
        rect: (0, 0, 9, 302),
        default: 255,
        flags: 0,
        plane: (0..9 * 302).map(|i| (i % 251) as u8).collect(),
    });
    let reference = open(&PsdSpec::layers(302, 9, vec![masked.clone()])).0;
    let expected = reference.layers[0].pixels.clone().unwrap();
    let expected_mask = reference.layers[0].mask.as_ref().unwrap().pixels.clone();
    for psb in [false, true] {
        for compression in [1, 2, 3] {
            let mut layer = masked.clone();
            layer.compression = compression;
            let mut spec = PsdSpec::layers(302, 9, vec![layer]);
            spec.psb = psb;
            let (document, _) = open(&spec);
            assert_eq!(document.layers[0].pixels.as_ref(), Some(&expected));
            assert_eq!(
                document.layers[0].mask.as_ref().unwrap().pixels,
                expected_mask,
                "compression {compression}, psb {psb}"
            );
        }
    }
}

#[test]
fn psb_files_use_eight_byte_lengths() {
    let mut layer = LayerSpec::pixels("Big", (0, 0), (5, 4), red_gradient)
        .with(b"Lr16", vec![0; 3])
        .with(b"luni", {
            let mut data = Vec::new();
            unicode(&mut data, "Large");
            data
        });
    layer.compression = 1;
    let mut spec = PsdSpec::layers(
        5,
        4,
        vec![
            LayerSpec::divider(),
            layer,
            LayerSpec::folder("Group", b"pass"),
        ],
    );
    spec.psb = true;
    let (file, _) = write(&spec);
    // Version 2, and a 64-bit layer-and-mask length after the empty resources.
    assert_eq!(&file[4..6], &[0, 2]);
    let (document, report) = read(&file, PixelBudget::default()).unwrap();
    assert!(report.is_empty());
    assert_eq!(document.layers[0].name, "Large");
    assert_eq!(
        document.layers[0]
            .pixels
            .as_ref()
            .unwrap()
            .get_pixel(4, 3)
            .0,
        red_gradient(4, 3)
    );
    assert!(document.layers[1].group);
}

#[test]
fn files_without_layers_open_their_merged_image() {
    let (w, h) = (130, 3);
    let planes: Vec<Vec<u8>> = (0..4)
        .map(|c| (0..w * h).map(|i| (i * (c + 1) % 256) as u8).collect())
        .collect();
    for (compression, psb) in [(0, false), (1, false), (1, true)] {
        let mut spec = PsdSpec::new(w as u32, h as u32);
        spec.channels = 4;
        spec.merged_compression = compression;
        spec.merged = Some(planes.clone());
        spec.psb = psb;
        let (document, report) = open(&spec);
        assert!(report.is_empty());
        assert_eq!(document.layers.len(), 1);
        let layer = &document.layers[0];
        assert_eq!(layer.name, "Background");
        let pixels = layer.pixels.as_ref().unwrap();
        // The fourth channel is an extra (alpha) channel, not transparency.
        assert_eq!(
            pixels.get_pixel(129, 2).0,
            [planes[0][389], planes[1][389], planes[2][389], 255]
        );
    }
    // A merged image larger than what is left of the budget is refused.
    let spec = PsdSpec::new(10, 10);
    let result = read(
        &bytes(&spec),
        PixelBudget {
            layers: 99,
            masks: MAX_PIXELS,
        },
    );
    assert!(result.is_err());
    // Unsupported merged compression.
    let mut spec = PsdSpec::new(2, 2);
    spec.merged_compression = 2;
    spec.merged = Some(vec![vec![0; 8]; 3]);
    assert!(error(&bytes(&spec)).contains("compression"));
}

#[test]
fn only_8_bit_rgb_is_accepted() {
    for (mode, name) in [
        (0, "Bitmap"),
        (1, "Grayscale"),
        (2, "Indexed Color"),
        (4, "CMYK"),
        (7, "Multichannel"),
        (8, "Duotone"),
        (9, "Lab"),
    ] {
        let mut spec = PsdSpec::new(2, 2);
        spec.mode = mode;
        let message = error(&bytes(&spec));
        assert!(message.contains(name), "{message}");
        assert!(message.contains("8-bit RGB"), "{message}");
    }
    for depth in [1, 16, 32] {
        let mut spec = PsdSpec::new(2, 2);
        spec.depth = depth;
        let message = error(&bytes(&spec));
        assert!(message.contains(&format!("{depth} bits")), "{message}");
    }
    let mut file = bytes(&PsdSpec::new(2, 2));
    file[5] = 3;
    assert!(error(&file).contains("version"));
    assert!(error(b"GIF89a").contains("not a Photoshop file"));
}

#[test]
fn clipping_masks_follow_their_base() {
    let clip = |name: &str| {
        let mut layer = LayerSpec::solid(name, (0, 0), (2, 2), [1, 1, 1, 255]);
        layer.clipping = true;
        layer
    };
    let unsupported = LayerSpec::blank("Threshold").with(b"thrs", vec![0; 4]);
    let spec = PsdSpec::layers(
        2,
        2,
        vec![
            LayerSpec::solid("Base", (0, 0), (2, 2), [0, 0, 0, 255]),
            clip("A"),
            clip("B"),
            LayerSpec::divider(),
            LayerSpec::folder("Folder", b"pass"),
            clip("On folder"),
            unsupported,
            clip("On left out"),
        ],
    );
    let (document, report) = open(&spec);
    let find = |name| document.layers.iter().find(|l| l.name == name).unwrap();
    let base = find("Base").id;
    assert_eq!(find("A").clip_to, Some(base));
    assert_eq!(find("B").clip_to, Some(base));
    assert_eq!(find("On folder").clip_to, None);
    assert_eq!(find("On left out").clip_to, None);
    assert!(document.layers.iter().all(|l| l.name != "Threshold"));
    assert_eq!(report.count(Dropped::ClippingBase), 2);
    assert_eq!(report.count(Dropped::PhotoshopAdjustment("Threshold")), 1);
}

#[test]
fn adjustment_layers_stay_editable_or_are_left_out() {
    let mut levels = vec![0, 2];
    for record in 0..29 {
        let values: [u16; 5] = if record == 0 {
            [10, 240, 5, 250, 150]
        } else {
            [0, 255, 0, 255, 100]
        };
        for v in values {
            levels.extend(v.to_be_bytes());
        }
    }
    let mut curves = vec![0, 0, 1];
    curves.extend(0b101_u32.to_be_bytes());
    for points in [
        [(0_u16, 0_u16), (200, 128), (255, 255)],
        [(50, 0), (255, 255), (100, 128)],
    ] {
        curves.extend(3_u16.to_be_bytes());
        for (output, input) in points {
            curves.extend(output.to_be_bytes());
            curves.extend(input.to_be_bytes());
        }
    }
    let mut exposure = 1_u16.to_be_bytes().to_vec();
    for v in [1.5_f32, -0.1, 1.2] {
        exposure.extend(v.to_be_bytes());
    }
    let mut multiply = LayerSpec::blank("Invert").with(b"nvrt", Vec::new());
    multiply.blend = *b"mul ";
    let mut balance = Vec::new();
    for v in [10_i16, -20, 30, 0, 0, 0, -100, 100, 5] {
        balance.extend(v.to_be_bytes());
    }
    balance.push(0);
    let black_white = block(
        None,
        "null",
        vec![
            ("Rd  ", D::Long(50)),
            ("Yllw", D::Long(-20)),
            ("Grn ", D::Long(40)),
            ("Cyn ", D::Long(60)),
            ("Bl  ", D::Long(20)),
            ("Mgnt", D::Long(250)),
            ("useTint", D::Bool(true)),
            (
                "tintColor",
                D::Obj(
                    "RGBC",
                    vec![
                        ("Rd  ", D::Doub(255.0)),
                        ("Grn ", D::Doub(127.5)),
                        ("Bl  ", D::Doub(0.0)),
                    ],
                ),
            ),
        ],
    );
    let spec = PsdSpec::layers(
        2,
        2,
        vec![
            LayerSpec::blank("Levels").with(b"levl", levels),
            LayerSpec::blank("Curves").with(b"curv", curves),
            LayerSpec::blank("Exposure").with(b"expA", exposure),
            multiply,
            LayerSpec::blank("Balance").with(b"blnc", balance),
            LayerSpec::blank("B&W").with(b"blwh", black_white),
            LayerSpec::blank("Hue").with(b"hue2", vec![0; 40]),
            LayerSpec::blank("Broken levels").with(b"levl", vec![0, 2, 0]),
            LayerSpec::blank("Broken B&W").with(b"blwh", vec![0; 8]),
        ],
    );
    let (document, report) = open(&spec);
    let names: Vec<_> = document.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(
        names,
        ["Levels", "Curves", "Exposure", "Invert", "Balance", "B&W"]
    );
    assert_eq!(
        document.layers[4].adjustment,
        Some(Adjustment::ColorBalance {
            shadows: [10.0, -20.0, 30.0],
            midtones: [0.0; 3],
            highlights: [-100.0, 100.0, 5.0],
            preserve_luminosity: false,
        })
    );
    let Some(Adjustment::BlackWhite {
        weights,
        tint,
        tint_hue,
        tint_saturation,
    }) = document.layers[5].adjustment
    else {
        panic!("black & white")
    };
    assert_eq!(weights, [50.0, -20.0, 40.0, 60.0, 20.0, 250.0]);
    assert!(tint);
    // An orange tint (255, 128, 0): hue 30°, fully saturated.
    assert!((tint_hue - 30.0).abs() < 0.5 && (tint_saturation - 100.0).abs() < 0.01);
    assert_eq!(
        report.count(Dropped::PhotoshopAdjustment("Black & White")),
        1
    );
    let Some(Adjustment::LevelsChannels { ranges }) = &document.layers[0].adjustment else {
        panic!("levels")
    };
    assert_eq!(ranges[0], [10.0, 1.5, 240.0, 5.0, 250.0]);
    assert_eq!(ranges[1], [0.0, 1.0, 255.0, 0.0, 255.0]);
    let Some(Adjustment::CurvesChannels { channels }) = &document.layers[1].adjustment else {
        panic!("curves")
    };
    assert_eq!(channels[0][1], Point::new(128.0 / 255.0, 200.0 / 255.0));
    // Red is absent from the bitmap; green holds the second curve.
    assert_eq!(channels[1].len(), 2);
    // Points are sorted by input.
    assert_eq!(channels[2][0], Point::new(0.0, 50.0 / 255.0));
    assert_eq!(channels[2][1], Point::new(128.0 / 255.0, 100.0 / 255.0));
    assert!(matches!(
        document.layers[2].adjustment,
        Some(Adjustment::Exposure { exposure, .. }) if exposure == 1.5
    ));
    assert_eq!(document.layers[3].adjustment, Some(Adjustment::Invert));
    assert_eq!(document.layers[3].blend, BlendMode::Normal);
    assert!(document.layers.iter().all(|l| l.pixels.is_none()));
    assert_eq!(report.count(Dropped::PhotoshopBlendMode("Multiply")), 1);
    assert_eq!(
        report.count(Dropped::PhotoshopAdjustment("Hue/Saturation")),
        1
    );
    assert_eq!(report.count(Dropped::PhotoshopAdjustment("Levels")), 1);
}

#[test]
fn filled_rectangles_and_ellipses_become_live_shapes() {
    let shape = |name: &str, kind: i32, radius: f64| {
        LayerSpec::solid(name, (2, 3), (6, 4), [1, 2, 3, 255])
            .with(b"SoCo", solid_color(255.0, 128.0, 0.0))
            .with(b"vmsk", vec![0; 8])
            .with(b"vogk", origination(kind, (2.0, 3.0, 8.0, 7.0), radius))
    };
    let stroked = shape("Stroked", 1, 0.0).with(
        b"vstk",
        block(None, "strokeStyle", vec![("strokeEnabled", D::Bool(true))]),
    );
    let unstroked = shape("Unstroked", 1, 0.0).with(
        b"vstk",
        block(
            None,
            "strokeStyle",
            vec![
                ("strokeEnabled", D::Bool(false)),
                ("fillEnabled", D::Bool(true)),
            ],
        ),
    );
    let mut deep = D::Doub(1.0);
    for _ in 0..40 {
        deep = D::Obj("null", vec![("deep", deep)]);
    }
    let hostile = shape("Deep", 5, 0.0).with(b"vstk", block(None, "null", vec![("x", deep)]));
    let spec = PsdSpec::layers(
        10,
        10,
        vec![
            shape("Ellipse", 5, 0.0),
            shape("Rounded", 2, 1.5),
            shape("Path", 7, 0.0),
            stroked,
            unstroked,
            hostile,
            LayerSpec::solid("Masked", (0, 0), (2, 2), [9; 4]).with(b"vmsk", vec![0; 8]),
            LayerSpec::solid("Gradient", (0, 0), (2, 2), [9; 4]).with(b"GdFl", vec![0; 4]),
            LayerSpec::blank("Solid").with(b"SoCo", solid_color(0.0, 0.0, 255.0)),
            LayerSpec::solid("Smart", (0, 0), (2, 2), [9; 4]).with(b"SoLd", vec![0; 4]),
        ],
    );
    let (document, report) = open(&spec);
    let find = |name| document.layers.iter().find(|l| l.name == name).unwrap();
    let ellipse = find("Ellipse");
    let style = ellipse.shape.as_ref().unwrap();
    assert_eq!(style.kind, ShapeKind::Ellipse);
    assert_eq!(style.color, [255, 128, 0, 255]);
    assert_eq!(
        (
            ellipse.transform.x,
            ellipse.transform.y,
            ellipse.transform.width
        ),
        (2.0, 3.0, 6.0)
    );
    assert_eq!(ellipse.pixels.as_ref().unwrap().dimensions(), (6, 4));
    let rounded = find("Rounded").shape.as_ref().unwrap();
    assert_eq!(rounded.kind, ShapeKind::RoundedRectangle);
    assert_eq!(rounded.corner_radius, 1.5);
    assert_eq!(
        find("Unstroked").shape.as_ref().unwrap().kind,
        ShapeKind::Rectangle
    );
    for name in ["Path", "Stroked", "Deep"] {
        let layer = find(name);
        assert!(layer.shape.is_none(), "{name}");
        assert_eq!(
            layer.pixels.as_ref().unwrap().get_pixel(0, 0).0,
            [1, 2, 3, 255]
        );
    }
    assert_eq!(report.count(Dropped::VectorAsPixels), 3);
    assert_eq!(report.count(Dropped::VectorMask), 1);
    assert_eq!(report.count(Dropped::FillLayer), 2);
    assert_eq!(report.count(Dropped::SmartObject), 1);
    // A solid fill stored without pixels is drawn over the canvas.
    let solid = find("Solid").pixels.as_ref().unwrap();
    assert_eq!(solid.dimensions(), (10, 10));
    assert_eq!(solid.get_pixel(9, 9).0, [0, 0, 255, 255]);
}

/// A vector mask block: one closed subpath of sharp knots, in canvas fractions, or only an
/// initial fill rule record when `points` is empty.
fn vector_mask(points: &[(f64, f64)], fill_all: bool) -> Vec<u8> {
    let mut out = vec![0, 0, 0, 3, 0, 0, 0, 0];
    let mut record = |selector: u16, body: Vec<u8>| {
        out.extend(selector.to_be_bytes());
        let mut body = body;
        body.resize(24, 0);
        out.extend(body);
    };
    record(6, Vec::new());
    record(8, u16::from(fill_all).to_be_bytes().to_vec());
    if !points.is_empty() {
        record(0, (points.len() as u16).to_be_bytes().to_vec());
    }
    for &(x, y) in points {
        let mut body = Vec::new();
        for _ in 0..3 {
            body.extend(((y * f64::from(1 << 24)) as i32).to_be_bytes());
            body.extend(((x * f64::from(1 << 24)) as i32).to_be_bytes());
        }
        record(1, body);
    }
    out
}

#[test]
fn shape_layers_without_pixels_are_drawn_from_their_path() {
    let mut vscg = b"SoCo".to_vec();
    vscg.extend(solid_color(0.0, 255.0, 0.0));
    let square = [(0.25, 0.25), (0.75, 0.25), (0.75, 0.75), (0.25, 0.75)];
    let spec = PsdSpec::layers(
        8,
        8,
        vec![
            // Photoshop CC: the fill lives in `vscg`, the path in `vsms`, the origin in `vogk`.
            LayerSpec::solid("CC rectangle", (2, 2), (4, 4), [1; 4])
                .with(b"vscg", vscg)
                .with(b"vsms", vector_mask(&square, false))
                .with(b"vogk", origination(1, (2.0, 2.0, 6.0, 6.0), 0.0)),
            // An old shape layer saved without pixels.
            LayerSpec::blank("Old shape")
                .with(b"SoCo", solid_color(0.0, 0.0, 255.0))
                .with(
                    b"vmsk",
                    vector_mask(&[(0.5, 0.0), (1.0, 1.0), (0.0, 1.0)], false),
                ),
            LayerSpec::blank("Whole canvas")
                .with(b"SoCo", solid_color(255.0, 0.0, 0.0))
                .with(b"vmsk", vector_mask(&[], true)),
            LayerSpec::blank("Gradient shape")
                .with(b"GdFl", vec![0; 8])
                .with(b"vmsk", vector_mask(&square, false)),
            LayerSpec::blank("Broken path")
                .with(b"SoCo", solid_color(255.0, 0.0, 0.0))
                .with(b"vmsk", vec![0; 3]),
        ],
    );
    let (document, report) = open(&spec);
    let find = |name| document.layers.iter().find(|l| l.name == name).unwrap();
    // A hostile path too costly to fill is left out rather than drawn for minutes.
    let canvas = Rect {
        left: 0,
        top: 0,
        right: 30_000,
        bottom: 3_000,
    };
    let zigzag: Vec<(f64, f64)> = (0..MAX_PATH_KNOTS)
        .map(|i| ((i % 2) as f64, i as f64 / MAX_PATH_KNOTS as f64))
        .collect();
    let mut hostile = LayerSpec::blank("Zigzag")
        .with(b"SoCo", solid_color(1.0, 1.0, 1.0))
        .with(b"vmsk", vector_mask(&zigzag, false));
    hostile.channels.clear();
    let (record_bytes, _) = record(&hostile, false);
    let mut reader = Reader::new(&record_bytes);
    let parsed = read_record(&mut reader, false).unwrap();
    let started = std::time::Instant::now();
    assert!(draw_vector(&parsed, canvas, MAX_PIXELS).is_none());
    assert!(started.elapsed().as_secs() < 5);
    let cc = find("CC rectangle");
    assert_eq!(cc.shape.as_ref().unwrap().color, [0, 255, 0, 255]);
    let old = find("Old shape");
    let pixels = old.pixels.as_ref().unwrap();
    assert_eq!((old.transform.x, old.transform.y), (0.0, 0.0));
    assert_eq!(pixels.dimensions(), (8, 8));
    // A triangle with its apex at the top middle.
    assert_eq!(pixels.get_pixel(4, 7).0, [0, 0, 255, 255]);
    assert_eq!(pixels.get_pixel(0, 0)[3], 0);
    assert!(pixels.get_pixel(4, 0)[3] < 255 && pixels.get_pixel(1, 6)[3] > 0);
    let whole = find("Whole canvas").pixels.as_ref().unwrap();
    assert!(whole.pixels().all(|p| p.0 == [255, 0, 0, 255]));
    assert!(find("Gradient shape").pixels.is_none());
    assert!(find("Broken path").pixels.is_none());
    assert_eq!(report.count(Dropped::VectorAsPixels), 2);
    assert_eq!(report.count(Dropped::VectorLeftOut), 2);
}

#[test]
fn simple_horizontal_text_stays_editable() {
    let text = |name: &str, spec: TextSpec| {
        LayerSpec::solid(name, (10, 4), (30, 12), [255, 255, 0, 255])
            .with(b"TySh", type_tool(&spec))
    };
    let spec = PsdSpec::layers(
        60,
        40,
        vec![
            text("Plain", TextSpec::default()),
            text(
                "Scaled",
                TextSpec {
                    scale: 2.0,
                    justification: 2,
                    ..Default::default()
                },
            ),
            text(
                "Styles",
                TextSpec {
                    second_size: Some(30.0),
                    ..Default::default()
                },
            ),
            text(
                "Vertical",
                TextSpec {
                    vertical: true,
                    ..Default::default()
                },
            ),
            text(
                "Rotated",
                TextSpec {
                    rotation: 30.0,
                    ..Default::default()
                },
            ),
            text(
                "Warped",
                TextSpec {
                    warp: "warpArc",
                    ..Default::default()
                },
            ),
            text(
                "Huge",
                TextSpec {
                    size: 5000.0,
                    ..Default::default()
                },
            ),
            LayerSpec::blank("No pixels").with(b"TySh", type_tool(&TextSpec::default())),
            LayerSpec::solid("Garbage", (0, 0), (1, 1), [0; 4]).with(b"TySh", vec![0, 1, 2]),
        ],
    );
    let (document, report) = open(&spec);
    let find = |name| document.layers.iter().find(|l| l.name == name).unwrap();
    let plain = find("Plain");
    let style = plain.text.as_ref().unwrap();
    assert_eq!(style.content, "Hello\nWorld");
    assert_eq!(style.family, "Arial");
    assert_eq!(style.size, 24.0);
    assert_eq!(style.color, [255, 128, 0, 255]);
    assert!(style.bold && style.italic && !style.underline);
    // Photoshop's rendering is kept until the text is edited.
    assert_eq!(
        plain.pixels.as_ref().unwrap().get_pixel(0, 0).0,
        [255, 255, 0, 255]
    );
    assert_eq!(find("Scaled").text.as_ref().unwrap().size, 48.0);
    assert!(find("Styles").text.is_some());
    for name in [
        "Vertical",
        "Rotated",
        "Warped",
        "Huge",
        "No pixels",
        "Garbage",
    ] {
        assert!(find(name).text.is_none(), "{name}");
    }
    assert_eq!(report.count(Dropped::PhotoshopTextAsPixels), 6);
    assert_eq!(report.count(Dropped::PhotoshopTextStyles), 1);
    assert_eq!(report.count(Dropped::TextLayout), 1);
}

fn rgb_color(r: f64, g: f64, b: f64) -> D {
    D::Obj(
        "RGBC",
        vec![
            ("Rd  ", D::Doub(r)),
            ("Grn ", D::Doub(g)),
            ("Bl  ", D::Doub(b)),
        ],
    )
}

fn effects_block(scale: f64, items: Vec<(&'static str, D)>) -> Vec<u8> {
    let mut all = vec![("Scl ", D::Unit(scale)), ("masterFXSwitch", D::Bool(true))];
    all.extend(items);
    block(Some(0), "null", all)
}

fn mode(name: &'static str) -> D {
    D::Enum("BlnM", name)
}

#[test]
fn layer_effects_map_onto_xuans_or_are_reported() {
    let effects = effects_block(
        200.0,
        vec![
            (
                "DrSh",
                D::Obj(
                    "DrSh",
                    vec![
                        ("enab", D::Bool(true)),
                        ("Md  ", mode("Mltp")),
                        ("Clr ", rgb_color(10.0, 20.0, 30.0)),
                        ("Opct", D::Unit(75.0)),
                        ("uglg", D::Bool(true)),
                        ("lagl", D::Unit(30.0)),
                        ("Dstn", D::Unit(10.0)),
                        ("Ckmt", D::Unit(0.0)),
                        ("blur", D::Unit(5.0)),
                    ],
                ),
            ),
            (
                "IrSh",
                D::Obj(
                    "IrSh",
                    vec![
                        ("enab", D::Bool(false)),
                        ("Md  ", mode("Mltp")),
                        ("Clr ", rgb_color(0.0, 0.0, 0.0)),
                        ("uglg", D::Bool(false)),
                        ("lagl", D::Unit(-45.0)),
                        ("Dstn", D::Unit(3.0)),
                        ("blur", D::Unit(4.0)),
                    ],
                ),
            ),
            (
                "FrFX",
                D::Obj(
                    "FrFX",
                    vec![
                        ("enab", D::Bool(true)),
                        ("Styl", D::Enum("FStl", "InsF")),
                        ("PntT", D::Enum("FrFl", "SClr")),
                        ("Md  ", mode("Nrml")),
                        ("Opct", D::Unit(100.0)),
                        ("Sz  ", D::Unit(3.0)),
                        ("Clr ", rgb_color(255.0, 0.0, 0.0)),
                    ],
                ),
            ),
            (
                "SoFi",
                D::Obj(
                    "SoFi",
                    vec![
                        ("enab", D::Bool(true)),
                        ("Md  ", mode("Nrml")),
                        ("Clr ", rgb_color(0.0, 255.0, 0.0)),
                        ("Opct", D::Unit(50.0)),
                    ],
                ),
            ),
            (
                "OrGl",
                D::Obj(
                    "OrGl",
                    vec![
                        ("enab", D::Bool(true)),
                        ("Md  ", mode("Scrn")),
                        ("Clr ", rgb_color(255.0, 255.0, 190.0)),
                        ("Opct", D::Unit(75.0)),
                        ("blur", D::Unit(8.0)),
                    ],
                ),
            ),
        ],
    );
    // Approximated: a centered stroke, a glow from the center and an unusual shadow mode.
    let approximate = effects_block(
        100.0,
        vec![
            (
                "FrFX",
                D::Obj(
                    "FrFX",
                    vec![
                        ("Styl", D::Enum("FStl", "CtrF")),
                        ("Sz  ", D::Unit(2.0)),
                        ("Clr ", rgb_color(0.0, 0.0, 0.0)),
                    ],
                ),
            ),
            (
                "IrGl",
                D::Obj(
                    "IrGl",
                    vec![
                        ("glwS", D::Enum("IGSr", "SrcC")),
                        ("Clr ", rgb_color(255.0, 255.0, 255.0)),
                        ("blur", D::Unit(600.0)),
                    ],
                ),
            ),
        ],
    );
    // Left out: bevel, and a gradient stroke.
    let unsupported = effects_block(
        100.0,
        vec![
            ("ebbl", D::Obj("ebbl", vec![("enab", D::Bool(true))])),
            (
                "FrFX",
                D::Obj(
                    "FrFX",
                    vec![
                        ("PntT", D::Enum("FrFl", "GrFl")),
                        ("Clr ", rgb_color(0.0, 0.0, 0.0)),
                    ],
                ),
            ),
            ("ChFX", D::Obj("ChFX", vec![("enab", D::Bool(false))])),
        ],
    );
    let off = block(
        Some(0),
        "null",
        vec![
            ("masterFXSwitch", D::Bool(false)),
            ("ebbl", D::Obj("ebbl", vec![])),
        ],
    );
    let mut folder = LayerSpec::folder("Folder", b"pass").with(b"lfx2", effects.clone());
    folder.opacity = 255;
    let mut spec = PsdSpec::layers(
        2,
        2,
        vec![
            LayerSpec::solid("Mapped", (0, 0), (2, 2), [9; 4]).with(b"lfx2", effects.clone()),
            LayerSpec::solid("Approximate", (0, 0), (2, 2), [9; 4]).with(b"lfx2", approximate),
            LayerSpec::solid("Unsupported", (0, 0), (2, 2), [9; 4]).with(b"lfx2", unsupported),
            LayerSpec::solid("Off", (0, 0), (2, 2), [9; 4])
                .with(b"lfx2", off)
                .with(b"iOpa", vec![0]),
            LayerSpec::solid("Legacy", (0, 0), (2, 2), [9; 4]).with(b"lrFX", vec![0; 4]),
            LayerSpec::solid("Effects only", (0, 0), (2, 2), [9; 4])
                .with(b"lfx2", effects.clone())
                .with(b"iOpa", vec![0]),
            LayerSpec::blank("Empty").with(b"lfx2", effects.clone()),
            LayerSpec::solid("Disabled", (0, 0), (2, 2), [9; 4])
                .with(
                    b"lfx2",
                    effects_block(
                        100.0,
                        vec![(
                            "SoFi",
                            D::Obj(
                                "SoFi",
                                vec![("enab", D::Bool(false)), ("Clr ", rgb_color(1.0, 2.0, 3.0))],
                            ),
                        )],
                    ),
                )
                .with(b"iOpa", vec![0]),
            LayerSpec::divider(),
            folder,
            LayerSpec::solid("Broken", (0, 0), (2, 2), [9; 4]).with(b"lfx2", vec![0, 0, 0]),
        ],
    );
    spec.global_angle = Some(60);
    let (document, report) = open(&spec);
    let find = |name| document.layers.iter().find(|l| l.name == name).unwrap();

    let mapped = find("Mapped").effects.as_ref().unwrap();
    let shadow = mapped.drop_shadow.unwrap();
    // The global light, sizes at the 200% effects scale.
    assert_eq!(shadow.angle, 60.0);
    assert_eq!((shadow.distance, shadow.blur), (20.0, 10.0));
    assert_eq!((shadow.color, shadow.opacity), ([10, 20, 30], 0.75));
    let inner = mapped.inner_shadow.unwrap();
    assert!(!inner.enabled && inner.angle == 315.0);
    let stroke = mapped.stroke.unwrap();
    assert!(stroke.inside && stroke.size == 6.0 && stroke.color == [255, 0, 0]);
    let overlay = mapped.color_overlay.unwrap();
    assert_eq!((overlay.color, overlay.opacity), ([0, 255, 0], 0.5));
    assert_eq!(mapped.outer_glow.unwrap().size, 16.0);
    assert!(mapped.inner_glow.is_none());

    let approximate = find("Approximate").effects.as_ref().unwrap();
    assert!(!approximate.stroke.unwrap().inside);
    assert_eq!(approximate.inner_glow.unwrap().size, 500.0);
    assert!(find("Unsupported").effects.is_none());
    assert!(find("Off").effects.is_none());
    // Switched off, the fill opacity applies as usual.
    assert_eq!(find("Off").opacity, 0.0);
    assert!(find("Legacy").effects.is_none());
    // Fill opacity cannot be separated from the effects: the layer keeps its layer opacity.
    assert_eq!(find("Effects only").opacity, 1.0);
    assert!(find("Effects only").effects.is_some());
    assert!(find("Empty").effects.is_none() && find("Folder").effects.is_none());
    // Effects that are all switched off are kept, and the fill opacity applies.
    let disabled = find("Disabled");
    assert!(
        !disabled
            .effects
            .as_ref()
            .unwrap()
            .color_overlay
            .unwrap()
            .enabled
    );
    assert_eq!(disabled.opacity, 0.0);

    assert_eq!(report.count(Dropped::PhotoshopEffectSettings), 1);
    // Unsupported, legacy and unreadable.
    assert_eq!(report.count(Dropped::PhotoshopEffects), 3);
    assert_eq!(report.count(Dropped::FillOpacity), 1);
    // On a layer without pixels and on a folder.
    assert_eq!(report.count(Dropped::LayerEffect), 2);
    // Imported effects render.
    render::render(&document);
}

#[test]
fn layers_too_large_for_the_budget_are_cropped_to_the_canvas() {
    let mut layer = LayerSpec::pixels("Wide", (-5, -2), (40, 14), red_gradient);
    layer.mask = Some(MaskSpec {
        rect: (-2, -5, 12, 35),
        default: 0,
        flags: 0,
        plane: vec![200; 14 * 40],
    });
    layer.compression = 1;
    let spec = PsdSpec::layers(10, 10, vec![layer.clone()]);
    // Fits as is: kept whole, off-canvas pixels included.
    let (document, report) = open(&spec);
    assert!(report.is_empty());
    assert_eq!(document.layers[0].transform.x, -5.0);
    // Over budget: cropped to the 10×10 canvas and reported.
    let budget = PixelBudget {
        layers: 150,
        masks: 150,
    };
    let (document, report) = read(&bytes(&spec), budget).unwrap();
    let layer = &document.layers[0];
    assert_eq!(layer.pixels.as_ref().unwrap().dimensions(), (10, 10));
    assert_eq!((layer.transform.x, layer.transform.y), (0.0, 0.0));
    assert_eq!(
        layer.pixels.as_ref().unwrap().get_pixel(0, 0).0,
        red_gradient(5, 2)
    );
    assert_eq!(layer.mask.as_ref().unwrap().pixels.dimensions(), (10, 10));
    assert_eq!(report.count(Dropped::CroppedToCanvas), 1);
    // Still too large once cropped.
    let budget = PixelBudget {
        layers: 50,
        masks: 150,
    };
    assert!(format!("{:#}", read(&bytes(&spec), budget).unwrap_err()).contains("megapixel"));
    // Bounds beyond Xuan's positions are cropped even within budget.
    let far = LayerSpec::solid("Far", (-2_000_000, 0), (3, 1), [1; 4]);
    let (document, report) = open(&PsdSpec::layers(4, 4, vec![far]));
    assert!(document.layers[0].pixels.is_none());
    assert_eq!(report.count(Dropped::CroppedToCanvas), 1);
}

#[test]
fn opens_through_load_with_report() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("Poster.PSD");
    let spec = PsdSpec::layers(3, 3, vec![LayerSpec::solid("Soft", (0, 0), (3, 3), [1; 4])]);
    let mut spec = spec;
    spec.layers[0].blend = *b"zzzz";
    fs::write(&path, bytes(&spec)).unwrap();
    assert!(is_photoshop(&path) && is_photoshop(Path::new("a.psb")));
    assert!(!is_photoshop(Path::new("a.png")));
    let (document, report) = crate::io::load_with_report(&path).unwrap();
    assert_eq!(document.layers.len(), 1);
    assert_eq!(
        report.count(Dropped::PhotoshopBlendMode(UNKNOWN_BLEND_MODE)),
        1
    );
    // Saved as .xuan and read back unchanged.
    let saved = directory.path().join("poster.xuan");
    crate::io::save(&document, &saved).unwrap();
    let loaded = crate::io::load(&saved).unwrap();
    assert_eq!(render::render(&loaded), render::render(&document));
}

// Hostile input -------------------------------------------------------------------------------

fn sample() -> PsdSpec {
    let mut masked = LayerSpec::pixels("Masked", (1, 1), (5, 3), red_gradient);
    masked.compression = 1;
    masked.mask = Some(MaskSpec {
        rect: (0, 0, 4, 4),
        default: 255,
        flags: 0,
        plane: vec![7; 16],
    });
    let mut zipped = LayerSpec::pixels("Zipped", (0, 0), (3, 3), red_gradient);
    zipped.compression = 3;
    zipped.clipping = true;
    let mut spec = PsdSpec::layers(
        8,
        6,
        vec![
            LayerSpec::divider(),
            masked,
            zipped,
            LayerSpec::solid("Text", (0, 0), (4, 2), [1; 4])
                .with(b"TySh", type_tool(&TextSpec::default())),
            LayerSpec::solid("Shape", (0, 0), (2, 2), [1; 4])
                .with(b"SoCo", solid_color(1.0, 2.0, 3.0))
                .with(b"vmsk", vec![0; 8])
                .with(b"vogk", origination(1, (0.0, 0.0, 2.0, 2.0), 0.0)),
            LayerSpec::folder("Folder", b"pass"),
        ],
    );
    spec.resolution = Some(144);
    spec
}

#[test]
fn truncation_anywhere_is_rejected_cleanly() {
    for psb in [false, true] {
        let mut spec = sample();
        spec.psb = psb;
        let (file, image_data) = write(&spec);
        assert!(read(&file, PixelBudget::default()).is_ok());
        for end in 0..file.len() {
            let result = read(&file[..end], PixelBudget::default());
            // Files with layers never need the merged image, so only a cut before it fails.
            assert_eq!(
                result.is_err(),
                end < image_data,
                "cut at {end} of {}",
                file.len()
            );
        }
    }
    // Without layers, the merged image is needed in full.
    let spec = PsdSpec::new(4, 3);
    let (file, image_data) = write(&spec);
    for end in image_data..file.len() {
        assert!(read(&file[..end], PixelBudget::default()).is_err());
    }
}

/// Overwrite a big-endian u32 at the first occurrence of `marker` plus `offset`.
fn patch(file: &mut [u8], at: usize, value: &[u8]) {
    file[at..at + value.len()].copy_from_slice(value);
}

/// Offset of the first layer record (after the header, empty color data and resources, and the
/// two section lengths and layer count).
fn first_record(spec: &PsdSpec) -> usize {
    let resources = if spec.resolution.is_some() { 28 } else { 0 };
    let long = if spec.psb { 8 } else { 4 };
    26 + 4 + 4 + resources + long * 2 + 2
}

#[test]
fn huge_declared_sizes_are_rejected_before_allocating() {
    let spec = PsdSpec::layers(4, 4, vec![LayerSpec::solid("A", (0, 0), (4, 4), [1; 4])]);
    let file = bytes(&spec);
    let record = first_record(&spec);
    // Color mode data, resources, the layer and mask section, then layer info, claiming 4 GiB.
    for at in [26, 30, 34, 38] {
        let mut f = file.clone();
        patch(&mut f, at, &u32::MAX.to_be_bytes());
        assert!(error(&f).contains("damaged"), "{at}");
    }
    // A channel claiming 4 GiB of data.
    let mut f = file.clone();
    patch(&mut f, record + 18 + 2, &u32::MAX.to_be_bytes());
    assert!(error(&f).contains("damaged"));
    // PSB: a 64-bit section length beyond any address space.
    let mut psb = spec.clone();
    psb.psb = true;
    for at in [34, 42] {
        let mut f = bytes(&psb);
        patch(&mut f, at, &u64::MAX.to_be_bytes());
        assert!(error(&f).contains("damaged"), "{at}");
    }
    // A canvas beyond Xuan's limits, and an empty one.
    for (width, height) in [(30_001, 1), (0, 1), (20_000, 20_000)] {
        let mut f = file.clone();
        patch(&mut f, 14, &(height as u32).to_be_bytes());
        patch(&mut f, 18, &(width as u32).to_be_bytes());
        assert!(
            read(&f, PixelBudget::default()).is_err(),
            "{width}×{height}"
        );
    }
    // Raw channel data declared for a 300,000 × 300,000 layer.
    let mut f = file.clone();
    patch(&mut f, record, &0_i32.to_be_bytes());
    patch(&mut f, record + 8, &300_000_i32.to_be_bytes());
    patch(&mut f, record + 12, &300_000_i32.to_be_bytes());
    assert!(read(&f, PixelBudget::default()).is_err());
    // ZIP data declared for a layer that would inflate past the per-channel limit.
    let mut zip = LayerSpec::solid("Z", (0, 0), (4, 4), [1; 4]);
    zip.compression = 2;
    let spec = PsdSpec::layers(4, 4, vec![zip]);
    let mut f = bytes(&spec);
    let record = first_record(&spec);
    patch(&mut f, record + 8, &300_000_i32.to_be_bytes());
    patch(&mut f, record + 12, &300_000_i32.to_be_bytes());
    assert!(read(&f, PixelBudget::default()).is_err());
}

#[test]
fn layer_bounds_out_of_range_are_rejected() {
    let spec = PsdSpec::layers(4, 4, vec![LayerSpec::solid("A", (0, 0), (2, 2), [1; 4])]);
    let file = bytes(&spec);
    let record = first_record(&spec);
    let bounds = |f: &mut [u8], values: [i32; 4]| {
        for (i, v) in values.into_iter().enumerate() {
            patch(f, record + i * 4, &v.to_be_bytes());
        }
    };
    for (top, left, bottom, right) in [
        (i32::MIN, 0, i32::MAX, 2),     // overflowing height
        (0, -200_000, 2, 200_000),      // wider than PSB allows
        (0, i32::MAX - 1, 2, i32::MIN), // wraps around
        (0, 0, -400_000, 2),            // far inverted
    ] {
        let mut f = file.clone();
        bounds(&mut f, [top, left, bottom, right]);
        assert!(
            error(&f).contains("out of range"),
            "{top} {left} {bottom} {right}"
        );
    }
    // Photoshop writes slightly inverted bounds (a bottom of -1) for empty layers: they are
    // read as empty, and their channel data is never decoded.
    for values in [[2, 0, 1, 2], [0, 2, 2, 1], [0, 0, -1, 0]] {
        let mut f = file.clone();
        bounds(&mut f, values);
        let (document, _) = read(&f, PixelBudget::default()).unwrap();
        assert!(document.layers[0].pixels.is_none(), "{values:?}");
    }
    // Mask bounds are checked the same way.
    let mut layer = LayerSpec::solid("A", (0, 0), (2, 2), [1; 4]);
    layer.mask = Some(MaskSpec {
        rect: (0, 0, 2, 2),
        default: 0,
        flags: 0,
        plane: vec![0; 4],
    });
    let spec = PsdSpec::layers(4, 4, vec![layer]);
    let mut f = bytes(&spec);
    let mask = f
        .windows(8)
        .position(|w| w == [0, 0, 0, 20, 0, 0, 0, 0])
        .unwrap()
        + 4;
    patch(&mut f, mask + 8, &400_000_i32.to_be_bytes());
    assert!(error(&f).contains("out of range"));
}

#[test]
fn packbits_never_overruns() {
    let mut row = [0_u8; 4];
    // A literal run longer than the row.
    assert!(unpack_row(&[4, 1, 2, 3, 4, 5], &mut row).is_err());
    // A repeat run longer than the row.
    assert!(unpack_row(&[(-4_i8) as u8, 9], &mut row).is_err());
    // A literal run past the end of the data.
    assert!(unpack_row(&[3, 1, 2], &mut row).is_err());
    // A repeat run with no value, and a row that ends early.
    assert!(unpack_row(&[(-1_i8) as u8], &mut row).is_err());
    assert!(unpack_row(&[1, 1, 2], &mut row).is_err());
    // -128 is a no-op.
    unpack_row(&[0x80, (-3_i8) as u8, 7], &mut row).unwrap();
    assert_eq!(row, [7; 4]);

    // In a file: row counts beyond the data, below the minimum, and runs past the row.
    let mut layer = LayerSpec::solid("A", (0, 0), (4, 2), [1, 2, 3, 4]);
    layer.compression = 1;
    let spec = PsdSpec::layers(4, 2, vec![layer]);
    let file = bytes(&spec);
    // The first channel (alpha 4): compression, two row counts, two rows of [-3, 4].
    let data = file
        .windows(10)
        .position(|w| w == [0, 1, 0, 2, 0, 2, 0xFD, 4, 0xFD, 4])
        .unwrap();
    for (offset, bytes) in [
        (2, &[0xFF, 0xFF][..]), // row count beyond the data
        (2, &[0, 1]),           // below PackBits' minimum
        (6, &[(-6_i8) as u8]),  // repeat past the row
        (6, &[5]),              // literal past the row's bytes
    ] {
        let mut f = file.clone();
        patch(&mut f, data + offset, bytes);
        assert!(error(&f).contains("damaged"), "{offset} {bytes:?}");
    }
}

#[test]
fn too_many_layers_or_channels_are_rejected() {
    let spec = PsdSpec::layers(2, 2, vec![LayerSpec::solid("A", (0, 0), (1, 1), [1; 4])]);
    let file = bytes(&spec);
    let record = first_record(&spec);
    for count in [10_001_i16, -10_001, i16::MIN] {
        let mut f = file.clone();
        patch(&mut f, record - 2, &count.to_be_bytes());
        assert!(error(&f).contains("too many layers"), "{count}");
    }
    let mut f = file.clone();
    patch(&mut f, record + 16, &57_u16.to_be_bytes());
    assert!(error(&f).contains("damaged"));
    // Thousands of tiny additional information blocks in one record.
    let mut crowded = LayerSpec::blank("Crowded");
    for _ in 0..MAX_LAYER_BLOCKS + 1 {
        crowded = crowded.with(b"zzzz", Vec::new());
    }
    assert!(error(&bytes(&PsdSpec::layers(1, 1, vec![crowded]))).contains("damaged"));
    // Ten thousand layers are fine.
    let many = PsdSpec::layers(1, 1, vec![LayerSpec::blank("x"); MAX_LAYERS]);
    assert_eq!(open(&many).0.layers.len(), MAX_LAYERS);
    // Header channel counts outside 1–56.
    for channels in [0_u16, 57] {
        let mut f = file.clone();
        patch(&mut f, 12, &channels.to_be_bytes());
        assert!(error(&f).contains("damaged"));
    }
}

#[test]
fn unknown_compression_and_broken_zip_are_rejected() {
    let mut layer = LayerSpec::solid("A", (0, 0), (2, 2), [1; 4]);
    layer.compression = 7;
    assert!(error(&bytes(&PsdSpec::layers(2, 2, vec![layer.clone()]))).contains("compression"));
    layer.compression = 2;
    let spec = PsdSpec::layers(2, 2, vec![layer]);
    let mut file = bytes(&spec);
    // Corrupt the first ZIP stream's header.
    let at = file.windows(2).position(|w| w == [0x78, 0x9C]).unwrap();
    file[at] = 0;
    assert!(error(&file).contains("damaged"));
}

#[test]
fn descriptors_and_engine_data_are_bounded() {
    // Nesting beyond the cap fails instead of recursing.
    let mut nested = D::Long(1);
    for _ in 0..descriptor::MAX_DESCRIPTOR_DEPTH + 1 {
        nested = D::List(vec![nested]);
    }
    let data = descriptor_bytes("null", &[("x", nested)]);
    assert!(descriptor::descriptor(&mut Reader::new(&data)).is_err());
    let data = descriptor_bytes("null", &[("x", D::List(vec![D::Long(1)]))]);
    assert!(descriptor::descriptor(&mut Reader::new(&data)).is_ok());
    // A list claiming four billion items, with none present.
    let mut data = descriptor_bytes("null", &[]);
    let count = data.len() - 4;
    patch(&mut data, count, &1_u32.to_be_bytes());
    data.extend(b"\0\0\0\0listVlLs\xFF\xFF\xFF\xFF");
    assert!(descriptor::descriptor(&mut Reader::new(&data)).is_err());
    // Unknown value types.
    let mut data = descriptor_bytes("null", &[]);
    let count = data.len() - 4;
    patch(&mut data, count, &1_u32.to_be_bytes());
    data.extend(b"\0\0\0\0keyXObAr");
    assert!(descriptor::descriptor(&mut Reader::new(&data)).is_err());

    let deep = "[".repeat(descriptor::MAX_ENGINE_DEPTH + 1);
    assert!(descriptor::engine(format!("<< /A {deep}").as_bytes()).is_err());
    for bad in [
        "<< /A (unterminated",
        "<< /A 1.2.3 >>",
        "<<",
        "[ 1 ]",
        "<< /A > >>",
        "",
    ] {
        assert!(descriptor::engine(bad.as_bytes()).is_err(), "{bad}");
    }
    let parsed = descriptor::engine(b"<< /A [ 1 -2.5 .5 true (x\\)y) /Name ] >>").unwrap();
    assert_eq!(
        parsed.get("A").unwrap().array(),
        [
            Engine::Number(1.0),
            Engine::Number(-2.5),
            Engine::Number(0.5),
            Engine::Bool(true),
            Engine::String("x)y".into()),
            Engine::Name("Name".into()),
        ]
    );
}

/// A small deterministic generator (xorshift64*), so failures reproduce.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

#[test]
fn mutated_files_never_panic() {
    let mut random = Random(0x5EED_1234_ABCD_0042);
    for psb in [false, true] {
        let mut spec = sample();
        spec.psb = psb;
        let file = bytes(&spec);
        for _ in 0..1500 {
            let mut mutated = file.clone();
            for _ in 0..1 + random.below(6) {
                let at = random.below(mutated.len());
                match random.below(5) {
                    0 => mutated[at] = 0xFF,
                    1 => mutated[at] = 0,
                    2 => mutated[at] ^= 1 << random.below(8),
                    3 => mutated.insert(at, random.next() as u8),
                    _ => mutated[at] = random.next() as u8,
                }
            }
            if random.below(10) == 0 {
                mutated.truncate(random.below(mutated.len()));
            }
            // Errors are fine; panics, hangs and runaway allocations are not.
            if let Ok((document, _)) = read(&mutated, PixelBudget::default()) {
                document.validate().unwrap();
            }
        }
    }
}
