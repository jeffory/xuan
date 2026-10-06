//! Vector paths: SVG path data (`d` strings) parsed into Bézier paths, flattened
//! into points for brush strokes and filled with antialiased coverage for path
//! selections, path fills and path shape layers. The geometry is kurbo's.
use anyhow::{Result, bail, ensure};
use image::{GrayImage, Luma};
use kurbo::{Affine, BezPath, PathEl, Shape};
use serde::{Deserialize, Serialize};

use crate::document::Point;

/// Longest SVG path text accepted, in bytes.
pub const MAX_PATH_BYTES: usize = 256 * 1024;
/// Most segments (lines and curves, with arcs counted as the curves they become) one path may have.
pub const MAX_SEGMENTS: usize = 10_000;
/// Coordinates must stay within this distance of the origin, as layer positions do.
pub const MAX_COORDINATE: f64 = 1_000_000.0;
/// Most points one flattened stroke may have, as for `stroke`'s `points`.
pub const MAX_STROKE_POINTS: usize = 10_000;
/// How far, in pixels, flattened curves may stray from the true curve when filled.
const FILL_TOLERANCE: f64 = 0.05;
/// How far a stroke's points may stray from the curve; the brush hides the rest.
const STROKE_TOLERANCE: f64 = 0.2;
/// Sample rows per pixel when filling; across a row coverage is exact.
const SAMPLES: usize = 16;

/// Which parts of a path count as inside, as SVG's `fill-rule`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FillRule {
    /// Inside wherever the outline winds around a point a nonzero number of times (SVG's default).
    #[default]
    Nonzero,
    /// Inside wherever a ray from the point crosses the outline an odd number of times, so an
    /// inner subpath always cuts a hole.
    Evenodd,
}

/// A validated Bézier path, written in JSON as an SVG path `d` string.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VectorPath(BezPath);

/// One flattened subpath: the points it passes through and whether `Z` closed it.
#[derive(Clone, Debug, PartialEq)]
pub struct Polyline {
    pub points: Vec<Point>,
    pub closed: bool,
}

impl VectorPath {
    /// Parse SVG path data such as `"M 0 700 C 120 640 380 640 512 700 Z"`: the commands
    /// M, L, H, V, C, S, Q, T, A and Z, absolute in capitals and relative in lowercase, with
    /// SVG's implicit repetition. Errors say what was expected and at which character.
    pub fn parse(d: &str) -> Result<Self> {
        Parser::new(d)?.parse().map(Self)
    }

    /// A path from kurbo elements, checked as [`Self::parse`] checks its result.
    pub fn from_bez(path: BezPath) -> Result<Self> {
        let path = Self(path);
        path.validate()?;
        Ok(path)
    }

    pub fn bez(&self) -> &BezPath {
        &self.0
    }

    pub fn is_empty(&self) -> bool {
        self.0.elements().is_empty()
    }

    /// The path as SVG path data, with absolute coordinates.
    pub fn to_svg(&self) -> String {
        self.0.to_svg()
    }

    /// Whether every coordinate is finite and in range and the path is not too long.
    pub fn validate(&self) -> Result<()> {
        let elements = self.0.elements();
        ensure!(
            elements
                .first()
                .is_none_or(|e| matches!(e, PathEl::MoveTo(_))),
            "A path must start with a move"
        );
        let segments = elements
            .iter()
            .filter(|e| !matches!(e, PathEl::MoveTo(_) | PathEl::ClosePath))
            .count();
        ensure!(
            segments <= MAX_SEGMENTS,
            "The path has {segments} segments; at most {MAX_SEGMENTS} are allowed"
        );
        let points = elements.iter().flat_map(|element| match *element {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => vec![p],
            PathEl::QuadTo(a, b) => vec![a, b],
            PathEl::CurveTo(a, b, c) => vec![a, b, c],
            PathEl::ClosePath => Vec::new(),
        });
        for point in points {
            ensure!(
                point.x.is_finite()
                    && point.y.is_finite()
                    && point.x.abs() <= MAX_COORDINATE
                    && point.y.abs() <= MAX_COORDINATE,
                "Path coordinates must be finite and within ±1,000,000"
            );
        }
        Ok(())
    }

    /// The path moved, scaled, rotated or flipped.
    pub fn transformed(&self, affine: Affine) -> Self {
        Self(affine * &self.0)
    }

    /// The exact bounds of the curves (not their control points), or `None` for a path with
    /// no points.
    pub fn bounds(&self) -> Option<kurbo::Rect> {
        (!self.is_empty()).then(|| self.0.bounding_box())
    }

    /// The subpaths flattened to points within `tolerance` pixels of the curves.
    pub fn flatten(&self, tolerance: f64) -> Vec<Polyline> {
        let mut lines: Vec<Polyline> = Vec::new();
        kurbo::flatten(&self.0, tolerance, |element| match element {
            PathEl::MoveTo(p) => lines.push(Polyline {
                points: vec![point(p)],
                closed: false,
            }),
            PathEl::LineTo(p) => {
                if let Some(line) = lines.last_mut() {
                    line.points.push(point(p));
                }
            }
            PathEl::ClosePath => {
                if let Some(line) = lines.last_mut() {
                    line.closed = true;
                }
            }
            // Flattening only produces moves, lines and closes.
            PathEl::QuadTo(..) | PathEl::CurveTo(..) => {}
        });
        lines
    }

    /// The points a brush stroke along this path passes through: one subpath, with a closed
    /// one returning to its start. A path of a single point is one dab.
    pub fn stroke_points(&self) -> Result<Vec<[f32; 2]>> {
        let lines = self.flatten(STROKE_TOLERANCE);
        ensure!(!lines.is_empty(), "The path has no points");
        ensure!(
            lines.len() == 1,
            "A stroke follows one subpath, but this path has {} (each M or m starts one); paint each as its own stroke",
            lines.len()
        );
        let line = &lines[0];
        let mut points: Vec<[f32; 2]> = Vec::with_capacity(line.points.len() + 1);
        let ends = line
            .points
            .iter()
            .chain(line.closed.then(|| &line.points[0]));
        for p in ends {
            if points.last() != Some(&[p.x, p.y]) || points.is_empty() {
                points.push([p.x, p.y]);
            }
        }
        ensure!(
            points.len() <= MAX_STROKE_POINTS,
            "The path flattens to {} points; a stroke takes at most {MAX_STROKE_POINTS}, so split it",
            points.len()
        );
        Ok(points)
    }

    /// How much of each pixel of a `width` × `height` grid the path covers, 0–1, row by row,
    /// with the path mapped onto the grid by `to_pixels`. Open subpaths are closed, as SVG
    /// fills them.
    pub fn coverage(&self, rule: FillRule, to_pixels: Affine, width: u32, height: u32) -> Vec<f32> {
        let (width, height) = (width as usize, height as usize);
        let mut coverage = vec![0.0_f32; width * height];
        // Edges as (top y, bottom y, x at top, dx per y, winding).
        let mut edges: Vec<(f64, f64, f64, f64, i32)> = Vec::new();
        for line in self.transformed(to_pixels).flatten(FILL_TOLERANCE) {
            let points = &line.points;
            for (i, a) in points.iter().enumerate() {
                let b = points[(i + 1) % points.len()];
                let (x0, y0, x1, y1) = (
                    f64::from(a.x),
                    f64::from(a.y),
                    f64::from(b.x),
                    f64::from(b.y),
                );
                if y0 == y1
                    || !(x0.is_finite() && y0.is_finite() && x1.is_finite() && y1.is_finite())
                {
                    continue;
                }
                let (top, bottom, x, winding) = if y0 < y1 {
                    (y0, y1, x0, 1)
                } else {
                    (y1, y0, x1, -1)
                };
                if bottom <= 0.0 || top >= height as f64 {
                    continue;
                }
                edges.push((top, bottom, x, (x1 - x0) / (y1 - y0), winding));
            }
        }
        if edges.is_empty() || width == 0 {
            return coverage;
        }
        edges.sort_by(|a, b| a.0.total_cmp(&b.0));
        let first_row = (edges[0].0.max(0.0) * SAMPLES as f64).floor() as usize;
        let last_row = edges
            .iter()
            .map(|e| e.1)
            .fold(0.0_f64, f64::max)
            .min(height as f64);
        let last_row = ((last_row * SAMPLES as f64).ceil() as usize).min(height * SAMPLES);
        let mut next = 0;
        let mut active: Vec<usize> = Vec::new();
        let mut crossings: Vec<(f64, i32)> = Vec::new();
        let weight = 1.0 / SAMPLES as f64;
        for row in first_row..last_row {
            let y = (row as f64 + 0.5) * weight;
            while next < edges.len() && edges[next].0 <= y {
                active.push(next);
                next += 1;
            }
            active.retain(|&e| edges[e].1 > y);
            crossings.clear();
            for &e in &active {
                let (top, _, x, slope, winding) = edges[e];
                crossings.push((x + (y - top) * slope, winding));
            }
            crossings.sort_by(|a, b| a.0.total_cmp(&b.0));
            let line = &mut coverage[(row / SAMPLES) * width..(row / SAMPLES + 1) * width];
            let mut winding = 0;
            for pair in crossings.windows(2) {
                winding += pair[0].1;
                let inside = match rule {
                    FillRule::Nonzero => winding != 0,
                    FillRule::Evenodd => winding % 2 != 0,
                };
                if !inside {
                    continue;
                }
                let start = pair[0].0.clamp(0.0, width as f64);
                let end = pair[1].0.clamp(0.0, width as f64);
                if end <= start {
                    continue;
                }
                let first = start.floor() as usize;
                let last = (end.ceil() as usize).min(width);
                if last - first == 1 {
                    line[first] += ((end - start) * weight) as f32;
                    continue;
                }
                line[first] += ((first as f64 + 1.0 - start) * weight) as f32;
                for value in &mut line[first + 1..last - 1] {
                    *value += weight as f32;
                }
                line[last - 1] += ((end - (last - 1) as f64) * weight) as f32;
            }
        }
        for value in &mut coverage {
            *value = value.clamp(0.0, 1.0);
        }
        coverage
    }

    /// A selection mask of a `width` × `height` canvas from the inside of the path, given in
    /// document pixels, with antialiased edges.
    pub fn mask(&self, rule: FillRule, width: u32, height: u32) -> GrayImage {
        let coverage = self.coverage(rule, Affine::IDENTITY, width, height);
        let mut mask = GrayImage::new(width, height);
        for (pixel, value) in mask.pixels_mut().zip(coverage) {
            *pixel = Luma([(value * 255.0).round() as u8]);
        }
        mask
    }
}

/// Most paths one document keeps.
pub const MAX_DOCUMENT_PATHS: usize = 1000;

/// A path saved with the document, as in Photoshop's Paths panel (format 10).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NamedPath {
    pub id: uuid::Uuid,
    pub name: String,
    pub d: VectorPath,
}

impl NamedPath {
    pub fn new(name: impl Into<String>, d: VectorPath) -> Self {
        Self {
            id: uuid::Uuid::new_v4(),
            name: name.into(),
            d,
        }
    }
}

/// "Path 1", "Path 2", …: the first such name no path has.
pub fn next_path_name(paths: &[NamedPath]) -> String {
    (1..)
        .map(|n| format!("Path {n}"))
        .find(|name| paths.iter().all(|p| &p.name != name))
        .unwrap_or_default()
}

/// Check a document's paths: their number, ids, names and data.
pub fn validate_paths(paths: &[NamedPath]) -> Result<()> {
    ensure!(
        paths.len() <= MAX_DOCUMENT_PATHS,
        "A document keeps at most {MAX_DOCUMENT_PATHS} paths"
    );
    let mut ids = std::collections::HashSet::new();
    for path in paths {
        ensure!(ids.insert(path.id), "Duplicate path identifiers");
        ensure!(
            !path.name.trim().is_empty() && path.name.len() <= 256,
            "Invalid path name"
        );
        path.d.validate()?;
    }
    Ok(())
}

/// Move every path of a document by `affine`, as Crop, Canvas Size, Image Size and Flip
/// Canvas move its content.
pub fn transform_paths(paths: &mut [NamedPath], affine: Affine) {
    for path in paths {
        path.d = path.d.transformed(affine);
    }
}

fn point(p: kurbo::Point) -> Point {
    Point::new(p.x as f32, p.y as f32)
}

impl Serialize for VectorPath {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.to_svg())
    }
}

impl<'de> Deserialize<'de> for VectorPath {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        Self::parse(&text).map_err(serde::de::Error::custom)
    }
}

/// SVG path data, read command by command.
struct Parser<'a> {
    text: &'a str,
    bytes: &'a [u8],
    at: usize,
    path: BezPath,
    segments: usize,
}

const COMMANDS: &str = "M, L, H, V, C, S, Q, T, A or Z (lowercase for relative coordinates)";

impl<'a> Parser<'a> {
    fn new(text: &'a str) -> Result<Self> {
        ensure!(
            text.len() <= MAX_PATH_BYTES,
            "The SVG path is {} bytes; at most {MAX_PATH_BYTES} are allowed",
            text.len()
        );
        Ok(Self {
            text,
            bytes: text.as_bytes(),
            at: 0,
            path: BezPath::new(),
            segments: 0,
        })
    }

    /// The 1-based character position of byte `at`, for messages.
    fn position(&self, at: usize) -> usize {
        self.text[..at.min(self.text.len())].chars().count() + 1
    }

    /// What is at byte `at`, quoted, or "the end of the path".
    fn found(&self, at: usize) -> String {
        match self.text.get(at..).and_then(|rest| rest.chars().next()) {
            Some(c) => format!("`{c}`"),
            None => "the end of the path".into(),
        }
    }

    fn skip_space(&mut self) {
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| b.is_ascii_whitespace())
        {
            self.at += 1;
        }
    }

    /// Whitespace, then at most one comma and more whitespace: what separates arguments.
    fn skip_separator(&mut self) {
        self.skip_space();
        if self.bytes.get(self.at) == Some(&b',') {
            self.at += 1;
            self.skip_space();
        }
    }

    fn starts_number(&self) -> bool {
        self.bytes
            .get(self.at)
            .is_some_and(|b| b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.'))
    }

    /// One number: an optional sign, digits with an optional fraction, an optional exponent.
    fn number(&mut self, command: char, what: &str) -> Result<f64> {
        self.skip_separator();
        let start = self.at;
        let mut end = start;
        if matches!(self.bytes.get(end), Some(b'-' | b'+')) {
            end += 1;
        }
        let digits = |mut i: usize| {
            while self.bytes.get(i).is_some_and(u8::is_ascii_digit) {
                i += 1;
            }
            i
        };
        let integer_end = digits(end);
        let mut mantissa = integer_end > end;
        end = integer_end;
        if self.bytes.get(end) == Some(&b'.') {
            let fraction_end = digits(end + 1);
            mantissa |= fraction_end > end + 1;
            end = fraction_end;
        }
        if !mantissa {
            bail!(
                "SVG path: expected {what} for `{command}` at character {}, found {}",
                self.position(start),
                self.found(start)
            );
        }
        if matches!(self.bytes.get(end), Some(b'e' | b'E')) {
            let mut exponent = end + 1;
            if matches!(self.bytes.get(exponent), Some(b'-' | b'+')) {
                exponent += 1;
            }
            let exponent_end = digits(exponent);
            if exponent_end > exponent {
                end = exponent_end;
            }
        }
        let value: f64 = self.text[start..end].parse().map_err(|_| {
            anyhow::anyhow!(
                "SVG path: `{}` at character {} is not a number",
                &self.text[start..end],
                self.position(start)
            )
        })?;
        ensure!(
            value.is_finite() && value.abs() <= MAX_COORDINATE,
            "SVG path: `{}` at character {} is out of range (at most ±1,000,000)",
            &self.text[start..end],
            self.position(start)
        );
        self.at = end;
        Ok(value)
    }

    /// An arc flag: a single `0` or `1`, which may be written without a separator after it.
    fn flag(&mut self, command: char, what: &str) -> Result<bool> {
        self.skip_separator();
        let start = self.at;
        match self.bytes.get(start) {
            Some(b'0') => {
                self.at += 1;
                Ok(false)
            }
            Some(b'1') => {
                self.at += 1;
                Ok(true)
            }
            _ => bail!(
                "SVG path: expected {what} (0 or 1) for `{command}` at character {}, found {}",
                self.position(start),
                self.found(start)
            ),
        }
    }

    fn pair(
        &mut self,
        command: char,
        what: &str,
        relative_to: Option<kurbo::Point>,
    ) -> Result<kurbo::Point> {
        let x = self.number(command, &format!("the x of {what}"))?;
        let y = self.number(command, &format!("the y of {what}"))?;
        let origin = relative_to.unwrap_or(kurbo::Point::ORIGIN);
        let point = kurbo::Point::new(origin.x + x, origin.y + y);
        self.check(point)?;
        Ok(point)
    }

    fn check(&self, point: kurbo::Point) -> Result<()> {
        ensure!(
            point.x.abs() <= MAX_COORDINATE && point.y.abs() <= MAX_COORDINATE,
            "SVG path: a point before character {} lies beyond ±1,000,000",
            self.position(self.at)
        );
        Ok(())
    }

    fn segment(&mut self) -> Result<()> {
        self.segments += 1;
        ensure!(
            self.segments <= MAX_SEGMENTS,
            "SVG path: more than {MAX_SEGMENTS} segments"
        );
        Ok(())
    }

    fn parse(mut self) -> Result<BezPath> {
        self.skip_space();
        ensure!(self.at < self.bytes.len(), "The SVG path is empty");
        let mut current = kurbo::Point::ORIGIN;
        let mut start = current;
        // The command to repeat when numbers follow without one.
        let mut previous: Option<u8> = None;
        // The last control point of a cubic (for S) or quadratic (for T) segment.
        let mut cubic_control: Option<kurbo::Point> = None;
        let mut quad_control: Option<kurbo::Point> = None;
        // After Z, a drawing command starts a new subpath at the closed one's start.
        let mut closed = false;
        loop {
            self.skip_separator();
            let Some(&byte) = self.bytes.get(self.at) else {
                break;
            };
            let at = self.at;
            let command = if byte.is_ascii_alphabetic() {
                self.at += 1;
                byte
            } else if self.starts_number()
                && let Some(previous) = previous.filter(|p| !matches!(p, b'z' | b'Z'))
            {
                match previous {
                    b'M' => b'L',
                    b'm' => b'l',
                    other => other,
                }
            } else {
                bail!(
                    "SVG path: expected a command ({COMMANDS}) at character {}, found {}",
                    self.position(at),
                    self.found(at)
                );
            };
            let name = command as char;
            if previous.is_none() && !matches!(command, b'M' | b'm') {
                bail!(
                    "SVG path: a path must start with a move (`M x y`), found {} at character {}",
                    self.found(at),
                    self.position(at)
                );
            }
            let relative = command.is_ascii_lowercase();
            let origin = relative.then_some(current);
            if closed && !matches!(command, b'M' | b'm' | b'Z' | b'z') {
                self.path.move_to(current);
            }
            closed = false;
            let (mut next_cubic, mut next_quad) = (None, None);
            match command.to_ascii_uppercase() {
                b'M' => {
                    current = self.pair(name, "the point to move to", origin)?;
                    start = current;
                    self.path.move_to(current);
                }
                b'L' => {
                    current = self.pair(name, "the line's end", origin)?;
                    self.segment()?;
                    self.path.line_to(current);
                }
                b'H' => {
                    let x = self.number(name, "the line's end x")?;
                    current.x = if relative { current.x + x } else { x };
                    self.check(current)?;
                    self.segment()?;
                    self.path.line_to(current);
                }
                b'V' => {
                    let y = self.number(name, "the line's end y")?;
                    current.y = if relative { current.y + y } else { y };
                    self.check(current)?;
                    self.segment()?;
                    self.path.line_to(current);
                }
                b'C' => {
                    let a = self.pair(name, "the first control point", origin)?;
                    let b = self.pair(name, "the second control point", origin)?;
                    let end = self.pair(name, "the curve's end", origin)?;
                    self.segment()?;
                    self.path.curve_to(a, b, end);
                    next_cubic = Some(b);
                    current = end;
                }
                b'S' => {
                    let a = cubic_control.map_or(current, |c| current + (current - c));
                    let b = self.pair(name, "the second control point", origin)?;
                    let end = self.pair(name, "the curve's end", origin)?;
                    self.segment()?;
                    self.path.curve_to(a, b, end);
                    next_cubic = Some(b);
                    current = end;
                }
                b'Q' => {
                    let a = self.pair(name, "the control point", origin)?;
                    let end = self.pair(name, "the curve's end", origin)?;
                    self.segment()?;
                    self.path.quad_to(a, end);
                    next_quad = Some(a);
                    current = end;
                }
                b'T' => {
                    let a = quad_control.map_or(current, |c| current + (current - c));
                    self.check(a)?;
                    let end = self.pair(name, "the curve's end", origin)?;
                    self.segment()?;
                    self.path.quad_to(a, end);
                    next_quad = Some(a);
                    current = end;
                }
                b'A' => {
                    let rx = self.number(name, "the x radius")?;
                    let ry = self.number(name, "the y radius")?;
                    let rotation = self.number(name, "the x-axis rotation")?;
                    let large_arc = self.flag(name, "the large-arc flag")?;
                    let sweep = self.flag(name, "the sweep flag")?;
                    let end = self.pair(name, "the arc's end", origin)?;
                    let arc = kurbo::SvgArc {
                        from: current,
                        to: end,
                        radii: kurbo::Vec2::new(rx.abs(), ry.abs()),
                        x_rotation: rotation.to_radians(),
                        large_arc,
                        sweep,
                    };
                    // A degenerate arc (no radius, or ends that coincide) is a line, as SVG says.
                    match kurbo::Arc::from_svg_arc(&arc) {
                        Some(arc) => {
                            let mut result = Ok(());
                            arc.to_cubic_beziers(0.1, |a, b, c| {
                                if result.is_ok() {
                                    result = self.segment();
                                    self.path.curve_to(a, b, c);
                                }
                            });
                            result?;
                        }
                        None if end != current => {
                            self.segment()?;
                            self.path.line_to(end);
                        }
                        None => {}
                    }
                    current = end;
                }
                b'Z' => {
                    self.path.close_path();
                    current = start;
                    closed = true;
                }
                _ => bail!(
                    "SVG path: unknown command {} at character {}; use {COMMANDS}",
                    self.found(at),
                    self.position(at)
                ),
            }
            cubic_control = next_cubic;
            quad_control = next_quad;
            previous = Some(command);
        }
        let path = VectorPath(self.path);
        path.validate()?;
        Ok(path.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn error(d: &str) -> String {
        format!("{:#}", VectorPath::parse(d).unwrap_err())
    }

    #[test]
    fn parses_every_command_absolute_and_relative() {
        let path = VectorPath::parse(
            "M 10 20 L 30 20 H 40 V 30 h -5 v 5 l -5 0 C 10 40 0 30 10 20 Z \
             m 100 0 c 10 0 20 10 20 20 s -10 20 -20 20 q -10 0 -10 -10 t 0 -10 \
             a 5 5 0 0 1 10 0 z",
        )
        .unwrap();
        let elements = path.bez().elements();
        assert_eq!(elements[0], PathEl::MoveTo((10.0, 20.0).into()));
        assert_eq!(elements[1], PathEl::LineTo((30.0, 20.0).into()));
        assert_eq!(elements[2], PathEl::LineTo((40.0, 20.0).into()));
        assert_eq!(elements[3], PathEl::LineTo((40.0, 30.0).into()));
        assert_eq!(elements[4], PathEl::LineTo((35.0, 30.0).into()));
        assert_eq!(elements[5], PathEl::LineTo((35.0, 35.0).into()));
        assert_eq!(elements[6], PathEl::LineTo((30.0, 35.0).into()));
        assert_eq!(
            elements[7],
            PathEl::CurveTo((10.0, 40.0).into(), (0.0, 30.0).into(), (10.0, 20.0).into())
        );
        assert_eq!(elements[8], PathEl::ClosePath);
        // `m` after `Z` is relative to the closed subpath's start.
        assert_eq!(elements[9], PathEl::MoveTo((110.0, 20.0).into()));
        assert_eq!(
            elements[10],
            PathEl::CurveTo(
                (120.0, 20.0).into(),
                (130.0, 30.0).into(),
                (130.0, 40.0).into()
            )
        );
        // `s` reflects the previous second control point about the current point.
        assert_eq!(
            elements[11],
            PathEl::CurveTo(
                (130.0, 50.0).into(),
                (120.0, 60.0).into(),
                (110.0, 60.0).into()
            )
        );
        assert_eq!(
            elements[12],
            PathEl::QuadTo((100.0, 60.0).into(), (100.0, 50.0).into())
        );
        // `t` reflects the previous quadratic control point.
        assert_eq!(
            elements[13],
            PathEl::QuadTo((100.0, 40.0).into(), (100.0, 40.0).into())
        );
        // The arc becomes cubic curves ending at its end point.
        let Some(PathEl::CurveTo(_, _, end)) = elements[elements.len() - 2].into() else {
            panic!("{elements:?}");
        };
        assert!((end - kurbo::Point::new(110.0, 40.0)).hypot() < 1e-9);
        assert_eq!(elements.last(), Some(&PathEl::ClosePath));
    }

    #[test]
    fn implicit_repeats_compact_numbers_and_lines_after_close() {
        // Numbers after M are lines; signs and dots separate numbers; flags need no space.
        let path = VectorPath::parse("M0,0 10,0-5.5.5e1Z L 3 3 A1 1 0 1110 10").unwrap();
        let elements = path.bez().elements();
        assert_eq!(elements[1], PathEl::LineTo((10.0, 0.0).into()));
        assert_eq!(elements[2], PathEl::LineTo((-5.5, 5.0).into()));
        assert_eq!(elements[3], PathEl::ClosePath);
        // A line after Z starts a new subpath at the closed one's start.
        assert_eq!(elements[4], PathEl::MoveTo((0.0, 0.0).into()));
        assert_eq!(elements[5], PathEl::LineTo((3.0, 3.0).into()));
        assert!(matches!(elements.last(), Some(PathEl::CurveTo(..))));
        // The written form reads back as the same path.
        assert_eq!(VectorPath::parse(&path.to_svg()).unwrap(), path);
    }

    #[test]
    fn parse_errors_say_what_was_expected_and_where() {
        assert_eq!(error(""), "The SVG path is empty");
        assert_eq!(error("   "), "The SVG path is empty");
        assert_eq!(
            error("L 10 10"),
            "SVG path: a path must start with a move (`M x y`), found `L` at character 1"
        );
        assert_eq!(
            error("M 0 0 X 5 5"),
            "SVG path: unknown command `X` at character 7; use M, L, H, V, C, S, Q, T, A or Z (lowercase for relative coordinates)"
        );
        assert_eq!(
            error("M 0 0 C 10 10 20"),
            "SVG path: expected the y of the second control point for `C` at character 17, found the end of the path"
        );
        assert_eq!(
            error("M 0 0 L 10 x"),
            "SVG path: expected the y of the line's end for `L` at character 12, found `x`"
        );
        assert_eq!(
            error("M 0 0 A 5 5 0 2 0 10 10"),
            "SVG path: expected the large-arc flag (0 or 1) for `A` at character 15, found `2`"
        );
        assert_eq!(
            error("M 0 0 Z 5 5"),
            "SVG path: expected a command (M, L, H, V, C, S, Q, T, A or Z (lowercase for relative coordinates)) at character 9, found `5`"
        );
        assert_eq!(
            error("M 0 0 L 1e7 0"),
            "SVG path: `1e7` at character 9 is out of range (at most ±1,000,000)"
        );
        assert!(error("M 0 0 l 900000 0 l 900000 0").contains("beyond ±1,000,000"));
        // Positions count characters, not bytes.
        assert_eq!(
            error("M 0 0 L é"),
            "SVG path: expected the x of the line's end for `L` at character 9, found `é`"
        );
        let long = format!("M 0 0{}", " L 1 1".repeat(MAX_SEGMENTS + 1));
        assert!(error(&long).contains("more than 10000 segments"));
        assert!(error(&" ".repeat(MAX_PATH_BYTES + 1)).contains("at most 262144"));
    }

    #[test]
    fn serde_writes_and_reads_the_d_string() {
        let path = VectorPath::parse("M 0 0 Q 5 10 10 0 Z").unwrap();
        let json = serde_json::to_value(&path).unwrap();
        assert_eq!(json, serde_json::json!("M0,0 Q5,10 10,0 Z"));
        assert_eq!(serde_json::from_value::<VectorPath>(json).unwrap(), path);
        let bad = serde_json::from_value::<VectorPath>(serde_json::json!("M 0")).unwrap_err();
        assert!(
            bad.to_string()
                .contains("expected the y of the point to move to")
        );
    }

    #[test]
    fn stroke_points_follow_one_subpath() {
        let line = VectorPath::parse("M 0 0 L 10 0 L 10 10 Z").unwrap();
        assert_eq!(
            line.stroke_points().unwrap(),
            vec![[0.0, 0.0], [10.0, 0.0], [10.0, 10.0], [0.0, 0.0]]
        );
        assert_eq!(
            VectorPath::parse("M 4 5").unwrap().stroke_points().unwrap(),
            vec![[4.0, 5.0]]
        );
        // A curve flattens to points close to it, from its start to its end.
        let curve = VectorPath::parse("M 0 0 C 0 100 100 100 100 0").unwrap();
        let points = curve.stroke_points().unwrap();
        assert!(points.len() > 10, "{points:?}");
        assert_eq!(points[0], [0.0, 0.0]);
        assert_eq!(*points.last().unwrap(), [100.0, 0.0]);
        let middle = curve.bez().segments().next().unwrap();
        for p in &points {
            let nearest = kurbo::ParamCurveNearest::nearest(
                &middle,
                kurbo::Point::new(f64::from(p[0]), f64::from(p[1])),
                1e-6,
            );
            assert!(nearest.distance_sq.sqrt() < 0.25, "{p:?}");
        }
        let two = VectorPath::parse("M 0 0 L 1 1 M 5 5 L 6 6").unwrap();
        assert!(
            format!("{:#}", two.stroke_points().unwrap_err())
                .contains("this path has 2 (each M or m starts one)")
        );
    }

    #[test]
    fn coverage_is_exact_inside_and_outside_and_antialiased_on_curves() {
        // A square from 2 to 6: whole pixels in, whole pixels out.
        let square = VectorPath::parse("M 2 2 H 6 V 6 H 2 Z").unwrap();
        let mask = square.mask(FillRule::Nonzero, 8, 8);
        for (x, y, pixel) in mask.enumerate_pixels() {
            let inside = (2..6).contains(&x) && (2..6).contains(&y);
            assert_eq!(pixel[0], if inside { 255 } else { 0 }, "{x},{y}");
        }
        // Half pixels on a fractional edge.
        let half = VectorPath::parse("M 0.5 0 H 2 V 1 H 0.5 Z").unwrap();
        assert_eq!(half.mask(FillRule::Nonzero, 2, 1).as_raw(), &[128, 255]);
        // A circle of radius 20: full inside, empty outside, partial pixels along the curve
        // that sum to about its area.
        let circle =
            VectorPath::parse("M 12 32 A 20 20 0 1 0 52 32 A 20 20 0 1 0 12 32 Z").unwrap();
        let coverage = circle.coverage(FillRule::Nonzero, Affine::IDENTITY, 64, 64);
        let at = |x: usize, y: usize| coverage[y * 64 + x];
        assert_eq!(at(32, 32), 1.0);
        assert_eq!(at(0, 0), 0.0);
        assert_eq!(at(60, 32), 0.0);
        let area: f32 = coverage.iter().sum();
        let exact = std::f32::consts::PI * 400.0;
        // Flattened chords lie just inside the curve, a few hundredths of a pixel at most.
        assert!((area - exact).abs() < exact * 0.004, "{area} vs {exact}");
        let edge = coverage.iter().filter(|c| **c > 0.0 && **c < 1.0).count();
        assert!(edge > 100, "{edge} antialiased pixels");
        // The pixel from (46, 45) to (47, 46) has corners 19.1 and 20.5 from the centre, so
        // the edge cuts it.
        assert!(at(46, 45) > 0.2 && at(46, 45) < 0.9, "{}", at(46, 45));
    }

    #[test]
    fn fill_rules_and_open_subpaths() {
        // Two squares wound the same way: nonzero fills the inner one, evenodd cuts a hole.
        let nested = VectorPath::parse("M 0 0 H 10 V 10 H 0 Z M 3 3 H 7 V 7 H 3 Z").unwrap();
        assert_eq!(
            nested.mask(FillRule::Nonzero, 10, 10).get_pixel(5, 5)[0],
            255
        );
        assert_eq!(nested.mask(FillRule::Evenodd, 10, 10).get_pixel(5, 5)[0], 0);
        assert_eq!(
            nested.mask(FillRule::Evenodd, 10, 10).get_pixel(1, 1)[0],
            255
        );
        // Wound the other way, the inner square is a hole under both rules.
        let hole = VectorPath::parse("M 0 0 H 10 V 10 H 0 Z M 3 3 V 7 H 7 V 3 Z").unwrap();
        assert_eq!(hole.mask(FillRule::Nonzero, 10, 10).get_pixel(5, 5)[0], 0);
        // An open triangle is filled as if closed.
        let open = VectorPath::parse("M 0 0 L 10 0 L 0 10").unwrap();
        let mask = open.mask(FillRule::Nonzero, 10, 10);
        assert_eq!(mask.get_pixel(1, 1)[0], 255);
        assert_eq!(mask.get_pixel(8, 8)[0], 0);
        // Mapped onto a grid: scaled down by two.
        let scaled = VectorPath::parse("M 0 0 H 8 V 8 H 0 Z").unwrap().coverage(
            FillRule::Nonzero,
            Affine::scale(0.5),
            6,
            6,
        );
        assert_eq!(scaled.iter().sum::<f32>(), 16.0);
        // Paths far off the grid cover nothing.
        let away = VectorPath::parse("M -100 -100 h 10 v 10 h -10 z").unwrap();
        assert!(
            away.mask(FillRule::Nonzero, 4, 4)
                .as_raw()
                .iter()
                .all(|v| *v == 0)
        );
    }

    #[test]
    fn bounds_follow_the_curve_not_its_control_points() {
        let arch = VectorPath::parse("M 0 100 C 0 0 100 0 100 100").unwrap();
        let bounds = arch.bounds().unwrap();
        assert_eq!((bounds.x0, bounds.x1, bounds.y1), (0.0, 100.0, 100.0));
        assert!((bounds.y0 - 25.0).abs() < 1e-9, "{bounds:?}");
        assert!(VectorPath::default().bounds().is_none());
    }
}
