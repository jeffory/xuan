//! Proof of concept: `.cube` LUTs as a Xuan plugin. See README.md.
use std::path::Path;

use serde_json::{json, Value};
use xuan_plugin::{codes, Output, Plugin, RpcError};

/// A parsed `.cube` file (Adobe/Resolve "Cube LUT Specification 1.0").
struct Cube {
    title: String,
    /// `LUT_1D_SIZE` entries, or `LUT_3D_SIZE`³ entries with red changing fastest.
    size: usize,
    three_d: bool,
    min: [f32; 3],
    max: [f32; 3],
    table: Vec<[f32; 3]>,
}

fn bad(message: impl Into<String>) -> RpcError {
    RpcError::new(codes::INVALID_PARAMS, message)
}

fn parse(path: &Path) -> Result<Cube, RpcError> {
    let text =
        std::fs::read_to_string(path).map_err(|e| bad(format!("Cannot read the LUT: {e}")))?;
    let (mut title, mut size, mut three_d) = (String::new(), 0usize, false);
    let (mut min, mut max) = ([0.0f32; 3], [1.0f32; 3]);
    let mut table = Vec::new();
    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut words = line.split_whitespace();
        let first = words.next().unwrap_or_default();
        let numbers = |words: std::str::SplitWhitespace| -> Result<[f32; 3], RpcError> {
            let values: Vec<f32> = words.filter_map(|w| w.parse().ok()).collect();
            values
                .try_into()
                .map_err(|_| bad(format!("Line {}: expected three numbers", number + 1)))
        };
        match first {
            "TITLE" => title = line[5..].trim().trim_matches('"').to_owned(),
            "LUT_1D_SIZE" | "LUT_3D_SIZE" => {
                three_d = first == "LUT_3D_SIZE";
                size = words.next().and_then(|w| w.parse().ok()).unwrap_or(0);
                let limit = if three_d { 256 } else { 65_536 };
                if !(2..=limit).contains(&size) {
                    return Err(bad(format!("Line {}: unsupported LUT size", number + 1)));
                }
            }
            "DOMAIN_MIN" => min = numbers(words)?,
            "DOMAIN_MAX" => max = numbers(words)?,
            "LUT_1D_INPUT_RANGE" | "LUT_3D_INPUT_RANGE" => {
                let values: Vec<f32> = words.filter_map(|w| w.parse().ok()).collect();
                if let [lo, hi] = values[..] {
                    (min, max) = ([lo; 3], [hi; 3]);
                }
            }
            _ if first.parse::<f32>().is_ok() => {
                let mut values = [first.parse::<f32>().unwrap_or(0.0), 0.0, 0.0];
                let rest: Vec<f32> = words.filter_map(|w| w.parse().ok()).collect();
                if rest.len() != 2 {
                    return Err(bad(format!("Line {}: expected three numbers", number + 1)));
                }
                values[1..].copy_from_slice(&rest);
                table.push(values);
            }
            _ => {} // Unknown keywords are allowed by the spec.
        }
    }
    let expected = if three_d { size * size * size } else { size };
    if size == 0 || table.len() != expected {
        return Err(bad(format!(
            "The LUT declares {expected} entries but has {}",
            table.len()
        )));
    }
    Ok(Cube {
        title,
        size,
        three_d,
        min,
        max,
        table,
    })
}

impl Cube {
    fn name(&self, path: &Path) -> String {
        if self.title.is_empty() {
            path.file_stem()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned()
        } else {
            self.title.clone()
        }
    }

    /// Input in 0..1 mapped to the table's index space for channel `c`.
    fn index(&self, value: f32, c: usize) -> f32 {
        let t = ((value - self.min[c]) / (self.max[c] - self.min[c])).clamp(0.0, 1.0);
        t * (self.size - 1) as f32
    }

    fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        if !self.three_d {
            return std::array::from_fn(|c| {
                let i = self.index(rgb[c], c);
                let (lo, f) = (i.floor() as usize, i.fract());
                let hi = (lo + 1).min(self.size - 1);
                self.table[lo][c] * (1.0 - f) + self.table[hi][c] * f
            });
        }
        // Trilinear interpolation between the eight surrounding entries.
        let n = self.size;
        let i = [
            self.index(rgb[0], 0),
            self.index(rgb[1], 1),
            self.index(rgb[2], 2),
        ];
        let lo = i.map(|v| v.floor() as usize);
        let hi = lo.map(|v| (v + 1).min(n - 1));
        let f = i.map(f32::fract);
        let at = |r: usize, g: usize, b: usize| self.table[r + g * n + b * n * n];
        let mut out = [0.0f32; 3];
        for (dr, wr) in [(lo[0], 1.0 - f[0]), (hi[0], f[0])] {
            for (dg, wg) in [(lo[1], 1.0 - f[1]), (hi[1], f[1])] {
                for (db, wb) in [(lo[2], 1.0 - f[2]), (hi[2], f[2])] {
                    let w = wr * wg * wb;
                    let v = at(dr, dg, db);
                    for c in 0..3 {
                        out[c] += v[c] * w;
                    }
                }
            }
        }
        out
    }
}

fn lut_path(job: &xuan_plugin::Job) -> Result<std::path::PathBuf, RpcError> {
    let file: String = job.input("file").unwrap_or_default();
    if file.trim().is_empty() {
        return Err(bad("Choose a .cube file"));
    }
    Ok(file.into())
}

fn main() {
    Plugin::new()
        .action("apply", |job| {
            let path = lut_path(job)?;
            let cube = parse(&path)?;
            let source = job
                .source_path()
                .ok_or_else(|| bad("Select an image layer"))?;
            let started = std::time::Instant::now();
            let mut image = image::open(source).map_err(RpcError::internal)?.to_rgba8();
            let decoded = started.elapsed();
            // 8-bit in, 8-bit out: a 256-entry cache per channel would do
            // for 1D LUTs; 3D LUTs are interpolated per pixel.
            let rows = image.height();
            for (y, row) in image.enumerate_rows_mut() {
                if y % 128 == 0 {
                    job.check_cancelled()?;
                    job.progress(Some(y as f32 / rows as f32), Some("Applying LUT"));
                }
                for (_, _, pixel) in row {
                    let rgb = [0, 1, 2].map(|c| f32::from(pixel.0[c]) / 255.0);
                    let out = cube.apply(rgb);
                    for c in 0..3 {
                        pixel.0[c] = (out[c].clamp(0.0, 1.0) * 255.0).round() as u8;
                    }
                }
            }
            let applied = started.elapsed() - decoded;
            let out = job.path("lut.png");
            image.save(&out).map_err(RpcError::internal)?;
            let total = started.elapsed();
            let name = cube.name(&path);
            Ok(vec![
                Output::image(out, Some(&format!("LUT: {name}")), 0.0, 0.0),
                Output::text(format!(
                    "{} {} LUT, {}x{} px: decode {} ms, apply {} ms, total {} ms",
                    if cube.three_d { "3D" } else { "1D" },
                    cube.size,
                    image.width(),
                    image.height(),
                    decoded.as_millis(),
                    applied.as_millis(),
                    total.as_millis()
                )),
            ])
        })
        .action("curves", |job| {
            let path = lut_path(job)?;
            let cube = parse(&path)?;
            if cube.three_d {
                return Err(bad(
                    "3D LUTs mix the channels, which Curves cannot express; use Apply LUT… instead",
                ));
            }
            // Curves take 2 to 32 points per channel, x from 0 to 1, y in 0..1.
            const POINTS: usize = 32;
            let mut worst = 0.0f32;
            let channels: Vec<Value> = (0..4)
                .map(|channel| {
                    let points: Vec<Value> = (0..POINTS)
                        .map(|k| {
                            let x = k as f32 / (POINTS - 1) as f32;
                            let y = if channel == 0 {
                                x // master curve stays the identity
                            } else {
                                cube.apply([x; 3])[channel - 1].clamp(0.0, 1.0)
                            };
                            json!({"x": x, "y": y})
                        })
                        .collect();
                    if channel > 0 {
                        // How far straight lines between the points stray from the LUT.
                        for i in 0..=1020 {
                            let x = i as f32 / 1020.0;
                            let exact = cube.apply([x; 3])[channel - 1].clamp(0.0, 1.0);
                            let k = ((x * (POINTS - 1) as f32).floor() as usize).min(POINTS - 2);
                            let (a, b) = (&points[k], &points[k + 1]);
                            let (ax, ay) = (
                                a["x"].as_f64().unwrap() as f32,
                                a["y"].as_f64().unwrap() as f32,
                            );
                            let (bx, by) = (
                                b["x"].as_f64().unwrap() as f32,
                                b["y"].as_f64().unwrap() as f32,
                            );
                            let approx = ay + (by - ay) * (x - ax) / (bx - ax);
                            worst = worst.max((approx - exact).abs());
                        }
                    }
                    Value::Array(points)
                })
                .collect();
            let name = cube.name(&path);
            Ok(vec![
                Output::edit(vec![json!({
                    "op": "add_adjustment_layer",
                    "name": format!("LUT: {name}"),
                    "adjustment": {"CurvesChannels": {"channels": channels}},
                })]),
                Output::text(format!(
                    "Curves from a {}-entry 1D LUT; largest difference about {:.1} of 255",
                    cube.size,
                    worst * 255.0
                )),
            ])
        })
        .run();
}
