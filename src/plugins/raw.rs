//! Camera RAW layers for plugins: what `document/get` says about them and how
//! `raw/develop` develops one again with new settings. The editor runs the
//! develop itself as a background job (see `src/app/plugin_raw.rs`); this
//! module checks the request up front and does the work.
use std::sync::atomic::AtomicBool;

use anyhow::{Context, Result, bail, ensure};
use serde_json::{Map, Value, json};
use uuid::Uuid;

use crate::{
    document::{Document, Point},
    raw::{self, DecodedRaw, DevelopSettings, RawAsset, WhiteBalance},
};

/// What `document/get` lists for a RAW layer under `raw`: the camera file,
/// what the camera recorded and the settings it is developed with.
pub fn describe(asset: &RawAsset) -> Value {
    let metadata = &asset.metadata;
    json!({
        "filename": asset.filename,
        "camera": metadata.camera,
        "lens": metadata.lens,
        "iso": metadata.iso,
        "aperture": metadata.aperture,
        "shutter": metadata.shutter,
        "focal_length": metadata.focal_length,
        "width": metadata.width,
        "height": metadata.height,
        "bits": metadata.bits,
        "settings": asset.settings,
    })
}

/// A `raw/develop` request, checked before the job starts.
#[derive(Clone, Debug, PartialEq)]
pub struct Develop {
    /// The RAW layer, resolved to the active layer when the plugin left it out.
    pub layer: Uuid,
    /// The settings to develop with, merged and checked.
    pub settings: DevelopSettings,
    /// Set the exposure from the image, as Develop's Auto button does.
    pub auto_exposure: bool,
    /// A neutral point to take the white balance from, in 0–1 coordinates
    /// of the layer as it is developed now.
    pub neutral_point: Option<Point>,
}

/// Parse and check a `raw/develop` request against the document: the layer
/// must be an unlocked RAW layer, and the settings must be in range. Errors
/// here are the plugin's, so nothing starts.
pub fn parse(document: &Document, params: &Value) -> Result<Develop> {
    let params = params
        .as_object()
        .context("The parameters must be an object")?;
    for key in params.keys() {
        ensure!(
            matches!(
                key.as_str(),
                "layer"
                    | "settings"
                    | "reset"
                    | "auto_exposure"
                    | "neutral_point"
                    | "name"
                    // The edit session, which the host reads.
                    | "session"
            ),
            "Unknown parameter `{key}`; raw/develop takes layer, settings, reset, auto_exposure and neutral_point"
        );
    }
    let layer = match params.get("layer") {
        None | Some(Value::Null) => document
            .active
            .context("No layer is active; give the RAW layer's id")?,
        Some(Value::String(id)) => {
            Uuid::parse_str(id).map_err(|_| anyhow::anyhow!("`layer` must be a layer id"))?
        }
        Some(_) => bail!("`layer` must be a layer id"),
    };
    let target = (document.layers.iter())
        .find(|l| l.id == layer)
        .context("No such layer")?;
    let Some(asset) = &target.raw else {
        bail!(
            "{} is not a RAW layer; open a camera RAW file (.{}) to get one",
            target.name,
            raw::EXTENSIONS.join(", .")
        );
    };
    ensure!(!target.locked, "Unlock the RAW layer before developing it");
    let reset = flag(params, "reset")?;
    let base = if reset {
        DevelopSettings::default()
    } else {
        asset.settings.clone()
    };
    let settings = match params.get("settings") {
        None | Some(Value::Null) => base,
        Some(Value::Object(changes)) => merge(&base, changes)?,
        Some(_) => bail!("`settings` must be an object of Develop settings"),
    };
    settings
        .validate()
        .map_err(|_| anyhow::anyhow!("{}", out_of_range(&settings)))?;
    let neutral_point = match params.get("neutral_point") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let [x, y]: [f32; 2] = serde_json::from_value(value.clone())
                .map_err(|_| anyhow::anyhow!("`neutral_point` must be [x, y], each 0–1"))?;
            ensure!(
                (0.0..=1.0).contains(&x) && (0.0..=1.0).contains(&y),
                "`neutral_point` must be [x, y], each 0–1"
            );
            Some(Point::new(x, y))
        }
    };
    Ok(Develop {
        layer,
        settings,
        auto_exposure: flag(params, "auto_exposure")?,
        neutral_point,
    })
}

fn flag(params: &Map<String, Value>, key: &str) -> Result<bool> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(value)) => Ok(*value),
        Some(_) => bail!("`{key}` must be true or false"),
    }
}

/// `base` with `changes` laid over it: objects merge key by key, anything
/// else replaces. Every key must be one the settings have, so a misspelt
/// setting is refused rather than silently ignored.
pub fn merge(base: &DevelopSettings, changes: &Map<String, Value>) -> Result<DevelopSettings> {
    let mut value = serde_json::to_value(base)?;
    merge_into(&mut value, changes, "")?;
    serde_json::from_value(value)
        .map_err(|error| anyhow::anyhow!("Invalid Develop settings: {error}"))
}

fn merge_into(target: &mut Value, changes: &Map<String, Value>, prefix: &str) -> Result<()> {
    let object = target.as_object_mut().context("Not an object")?;
    for (key, change) in changes {
        let name = format!("{prefix}{key}");
        let Some(current) = object.get_mut(key) else {
            let mut known: Vec<&str> = object.keys().map(String::as_str).collect();
            known.sort_unstable();
            bail!(
                "Unknown Develop setting `{name}`; the settings are {}",
                known.join(", ")
            );
        };
        match (current, change) {
            (current @ Value::Object(_), Value::Object(inner)) => {
                merge_into(current, inner, &format!("{name}."))?;
            }
            (current, change) => *current = change.clone(),
        }
    }
    Ok(())
}

/// Which setting is out of range, for the error: the first one that alone
/// makes otherwise default settings fail.
fn out_of_range(settings: &DevelopSettings) -> String {
    let Ok(Value::Object(given)) = serde_json::to_value(settings) else {
        return "A Develop setting is out of range".into();
    };
    let defaults = DevelopSettings::default();
    for (key, value) in given {
        let mut single = Map::new();
        single.insert(key.clone(), value);
        if merge(&defaults, &single).is_ok_and(|s| s.validate().is_err()) {
            let range = RANGES
                .iter()
                .find(|(name, _)| *name == key)
                .map_or(String::new(), |(_, range)| format!(" ({range})"));
            return format!("The Develop setting `{key}` is out of range{range}");
        }
    }
    // Only a combination fails, such as a crop that keeps less than 1%.
    match settings.validate() {
        Err(error) => format!("{error:#}"),
        Ok(()) => "A Develop setting is out of range".into(),
    }
}

/// The ranges of the settings agents use most, for errors and tool docs.
pub const RANGES: &[(&str, &str)] = &[
    ("white_balance", "\"AsShot\", \"Temperature\" or \"Custom\""),
    (
        "temperature",
        "2000–25000 K, with white_balance \"Temperature\"",
    ),
    ("tint", "-150–150"),
    (
        "custom_wb",
        "[r, g, b] multipliers 0.01–100, with white_balance \"Custom\"",
    ),
    ("exposure", "-10–10 EV"),
    ("brightness", "-100–100"),
    ("contrast", "-100–100"),
    ("highlights", "-100–100"),
    ("shadows", "-100–100"),
    ("whites", "-100–100"),
    ("blacks", "-100–100"),
    ("saturation", "-100–100"),
    ("vibrance", "-100–100"),
    ("clarity", "-100–100"),
    ("texture", "-100–100"),
    ("dehaze", "-100–100"),
    ("tone_balance", "-100–100"),
    ("luminance_noise", "0–100"),
    ("color_noise", "0–100"),
    ("defringe", "0–100"),
    ("sharpen", "0–200"),
    ("sharpen_radius", "0.3–5"),
    ("sharpen_threshold", "0–1"),
    ("distortion", "-100–100"),
    ("chromatic_red", "-100–100"),
    ("chromatic_blue", "-100–100"),
    ("vignette", "-100–100"),
    ("rotation", "-45–45 degrees"),
    ("quarter_turns", "0–3"),
    ("perspective", "[horizontal, vertical], each -100–100"),
    (
        "crop",
        "[left, top, right, bottom], 0–1, keeping at least 1% each way",
    ),
    ("curves", "4 × 5 knots 0–1 for master, red, green, blue"),
    (
        "hsl",
        "8 hue bands × [hue, saturation, lightness], each -100–100",
    ),
    ("bw_mix", "[r, g, b], each -1–2, with monochrome"),
    ("shadow_tone", "[hue 0–360, saturation 0–100]"),
    ("highlight_tone", "[hue 0–360, saturation 0–100]"),
];

/// Develop the layer as asked: decode its camera file with `decode`, take
/// the white balance and exposure from the image when asked, render it at
/// full size and replace the layer's pixels and settings. Runs on a worker.
pub fn develop(
    document: &mut Document,
    request: &Develop,
    decode: impl FnOnce(&[u8]) -> Result<DecodedRaw>,
    cancel: &AtomicBool,
) -> Result<()> {
    let layer = (document.layers.iter_mut())
        .find(|l| l.id == request.layer)
        .context("The RAW layer is no longer available")?;
    let mut asset = layer
        .raw
        .clone()
        .context("The layer is no longer a RAW layer")?;
    let decoded = decode(&asset.bytes)?;
    let mut settings = request.settings.clone();
    if let Some(point) = request.neutral_point {
        // The point is on the layer as the agent saw it: developed with the
        // settings it has now.
        let shown = &asset.settings;
        let aspect = decoded.camera.width() as f32 / decoded.camera.height().max(1) as f32;
        let source = raw::source_point(shown.image_point(point), shown, aspect);
        let wb = raw::sample_white_balance(&decoded, source).context(
            "Cannot take the white balance there: choose a neutral, not clipped area of the image",
        )?;
        settings.custom_wb = wb;
        settings.white_balance = WhiteBalance::Custom;
        settings.tint = 0.0;
    }
    if request.auto_exposure {
        settings.exposure = raw::auto_exposure(&decoded);
    }
    settings.validate()?;
    let pixels = raw::render(&decoded, &settings, cancel)?;
    asset.settings = settings;
    raw::update_layer(layer, asset, pixels)?;
    document.validate()
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::document::Layer;

    fn decoded() -> DecodedRaw {
        // Grey on the left, brighter on the right.
        let camera = image::Rgb32FImage::from_fn(64, 48, |x, _| {
            image::Rgb([if x < 32 { 0.1 } else { 0.3 }; 3])
        });
        DecodedRaw {
            camera,
            as_shot: [1.0; 3],
            camera_to_rgb: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            xyz_to_camera: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            metadata: raw::RawMetadata {
                camera: "Test Camera".into(),
                width: 64,
                height: 48,
                bits: 14,
                ..Default::default()
            },
        }
    }

    fn document() -> (Document, Uuid) {
        let decoded = decoded();
        let asset = RawAsset {
            filename: "test.NEF".into(),
            metadata: decoded.metadata.clone(),
            settings: DevelopSettings::default(),
            bytes: Arc::new(vec![1, 2, 3]),
        };
        let pixels = raw::render(&decoded, &asset.settings, &AtomicBool::new(false)).unwrap();
        let mut document = Document::new(64, 48).unwrap();
        let mut layer = Layer::image("test", pixels);
        layer.raw = Some(asset);
        let id = layer.id;
        document.layers = vec![layer];
        document.select(id, false);
        (document, id)
    }

    #[test]
    fn describes_the_camera_file_and_its_settings() {
        let (document, _) = document();
        let described = crate::plugins::edits::describe(&document);
        let layer = &described["layers"][0];
        assert_eq!(layer["kind"], "raw");
        assert_eq!(layer["raw"]["filename"], "test.NEF");
        assert_eq!(layer["raw"]["camera"], "Test Camera");
        assert_eq!(layer["raw"]["settings"]["exposure"], 0.0);
        assert_eq!(layer["raw"]["settings"]["white_balance"], "AsShot");
    }

    #[test]
    fn settings_merge_over_the_layers_own() {
        let (mut document, id) = document();
        document.layers[0].raw.as_mut().unwrap().settings.contrast = 20.0;
        // The SDK adds the client's edit session to every request.
        assert!(parse(&document, &json!({"session": "s1"})).is_ok());
        let request = parse(
            &document,
            &json!({"settings": {"exposure": 1.5, "white_balance": "Temperature", "temperature": 5200, "negative": {"gamma": 2.0}}}),
        )
        .unwrap();
        assert_eq!(request.layer, id);
        assert_eq!(request.settings.exposure, 1.5);
        assert_eq!(request.settings.contrast, 20.0, "kept");
        assert_eq!(request.settings.white_balance, WhiteBalance::Temperature);
        assert_eq!(request.settings.temperature, 5200.0);
        assert_eq!(request.settings.negative.gamma, 2.0);
        // `reset` starts from Develop's defaults instead.
        let reset = parse(
            &document,
            &json!({"reset": true, "settings": {"exposure": 1}}),
        )
        .unwrap();
        assert_eq!(reset.settings.contrast, 0.0);
    }

    #[test]
    fn bad_requests_are_refused_before_any_work() {
        let (mut document, id) = document();
        let error = |params: Value, document: &Document| {
            format!("{:#}", parse(document, &params).unwrap_err())
        };
        let misspelt = error(json!({"settings": {"exposurre": 1}}), &document);
        assert!(
            misspelt.contains("Unknown Develop setting `exposurre`"),
            "{misspelt}"
        );
        assert!(misspelt.contains("exposure"), "{misspelt}");
        let nested = error(json!({"settings": {"negative": {"gama": 1}}}), &document);
        assert!(nested.contains("`negative.gama`"), "{nested}");
        let range = error(json!({"settings": {"exposure": 11}}), &document);
        assert!(
            range.contains("`exposure` is out of range (-10–10 EV)"),
            "{range}"
        );
        let typed = error(json!({"settings": {"exposure": "bright"}}), &document);
        assert!(typed.contains("Invalid Develop settings"), "{typed}");
        let point = error(json!({"neutral_point": [1.5, 0]}), &document);
        assert!(point.contains("neutral_point"), "{point}");
        let unknown = error(json!({"exposure": 1}), &document);
        assert!(
            unknown.contains("Unknown parameter `exposure`"),
            "{unknown}"
        );
        // Not a RAW layer, and locked ones.
        let plain = Layer::image("plain", image::RgbaImage::new(4, 4));
        let plain_id = plain.id;
        document.layers.push(plain);
        let not_raw = error(json!({"layer": plain_id.to_string()}), &document);
        assert!(not_raw.contains("plain is not a RAW layer"), "{not_raw}");
        document.layers[0].locked = true;
        let locked = error(json!({"layer": id.to_string()}), &document);
        assert!(locked.contains("Unlock"), "{locked}");
    }

    #[test]
    fn develops_the_layer_with_new_settings() {
        let (mut document, id) = document();
        let before = document.layers[0].pixels.clone().unwrap();
        let request = parse(&document, &json!({"settings": {"exposure": 1.0}})).unwrap();
        develop(
            &mut document,
            &request,
            |_| Ok(decoded()),
            &AtomicBool::new(false),
        )
        .unwrap();
        let layer = document.layers.iter().find(|l| l.id == id).unwrap();
        assert_eq!(layer.raw.as_ref().unwrap().settings.exposure, 1.0);
        let after = layer.pixels.as_ref().unwrap();
        assert!(
            after.get_pixel(10, 10)[0] > before.get_pixel(10, 10)[0],
            "brighter: {:?} vs {:?}",
            after.get_pixel(10, 10),
            before.get_pixel(10, 10)
        );
        // A crop changes the layer's size.
        let request = parse(
            &document,
            &json!({"settings": {"crop": [0.0, 0.0, 0.5, 1.0]}}),
        )
        .unwrap();
        develop(
            &mut document,
            &request,
            |_| Ok(decoded()),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(document.layers[0].pixels.as_ref().unwrap().width(), 32);
    }

    #[test]
    fn takes_exposure_and_white_balance_from_the_image() {
        let (mut document, _) = document();
        let request = parse(
            &document,
            &json!({"auto_exposure": true, "neutral_point": [0.25, 0.5]}),
        )
        .unwrap();
        develop(
            &mut document,
            &request,
            |_| Ok(decoded()),
            &AtomicBool::new(false),
        )
        .unwrap();
        let settings = &document.layers[0].raw.as_ref().unwrap().settings;
        assert_eq!(settings.white_balance, WhiteBalance::Custom);
        assert_ne!(settings.exposure, 0.0);
        // A decoder error reaches the plugin as it is.
        let error = develop(
            &mut document,
            &request,
            |_| bail!("Unsupported or damaged RAW file"),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(format!("{error:#}").contains("damaged"));
    }
}
