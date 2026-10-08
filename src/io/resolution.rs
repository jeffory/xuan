//! The print resolution an image file states (issue 94): PNG `pHYs`, JPEG JFIF density or Exif,
//! TIFF `XResolution` and `ResolutionUnit`, and WebP Exif. Read from the bytes already in memory,
//! never past their end; a file that states none, or one no document may have, gives `None` and
//! the document keeps [`DEFAULT_RESOLUTION`](crate::units::DEFAULT_RESOLUTION).

use crate::units::valid_resolution;

/// Metres in an inch, for PNG's pixels per metre.
const METRES_PER_INCH: f64 = 0.0254;
const CM_PER_INCH: f64 = 2.54;

/// The horizontal resolution `bytes` state, in pixels per inch.
pub fn read(bytes: &[u8]) -> Option<f32> {
    let ppi = if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        png(bytes)
    } else if bytes.starts_with(&[0xFF, 0xD8]) {
        jpeg(bytes)
    } else if bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP") {
        webp(bytes)
    } else {
        tiff(bytes)
    }?;
    valid_resolution(ppi).then_some(ppi as f32)
}

/// `count` pixels per unit, where an inch holds `units_per_inch` units, in pixels per inch. A
/// format that stores whole numbers per metre or centimetre cannot hold 300 ppi exactly, so the
/// whole resolution that stores as `count` is preferred: 11811 pixels per metre is 300 ppi.
fn whole_ppi(count: u32, units_per_inch: f64) -> f64 {
    let exact = f64::from(count) * units_per_inch;
    let whole = exact.round();
    if (whole / units_per_inch).round() == f64::from(count) {
        whole
    } else {
        exact
    }
}

fn be16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// PNG: the `pHYs` chunk, when its unit is the metre (unit 0 states only the aspect ratio).
fn png(bytes: &[u8]) -> Option<f64> {
    let mut at = 8;
    while let (Some(length), Some(kind)) = (be32(bytes, at), bytes.get(at + 4..at + 8)) {
        let data = at + 8;
        match kind {
            b"pHYs" => {
                let x = be32(bytes, data)?;
                return (*bytes.get(data + 8)? == 1).then(|| whole_ppi(x, METRES_PER_INCH));
            }
            // pHYs comes before the image data.
            b"IDAT" | b"IEND" => return None,
            _ => {
                at = data
                    .checked_add(usize::try_from(length).ok()?)?
                    .checked_add(4)?
            }
        }
    }
    None
}

/// JPEG: the JFIF header's density in dots per inch or per centimetre, else Exif's.
fn jpeg(bytes: &[u8]) -> Option<f64> {
    let mut exif = None;
    let mut at = 2;
    loop {
        // Markers may be padded with any number of 0xFF bytes.
        while bytes.get(at) == Some(&0xFF) && bytes.get(at + 1) == Some(&0xFF) {
            at += 1;
        }
        if *bytes.get(at)? != 0xFF {
            break;
        }
        let marker = *bytes.get(at + 1)?;
        at += 2;
        match marker {
            // Restart markers and TEM stand alone.
            0xD0..=0xD7 | 0x01 => continue,
            // The scan, or the end: no header follows.
            0xDA | 0xD9 => break,
            _ => {}
        }
        let length = usize::from(be16(bytes, at)?);
        let segment = bytes.get(at + 2..at.checked_add(length)?)?;
        match marker {
            0xE0 if segment.starts_with(b"JFIF\0") => {
                let units = *segment.get(7)?;
                let x = u32::from(be16(segment, 8)?);
                match units {
                    1 => return Some(f64::from(x)),
                    2 => return Some(whole_ppi(x, CM_PER_INCH)),
                    _ => {}
                }
            }
            0xE1 if exif.is_none() => {
                exif = segment.strip_prefix(b"Exif\0\0").and_then(tiff);
            }
            _ => {}
        }
        at += length;
    }
    exif
}

/// WebP: the `EXIF` chunk.
fn webp(bytes: &[u8]) -> Option<f64> {
    let mut at = 12;
    while let Some(kind) = bytes.get(at..at + 4) {
        let size = usize::try_from(u32::from_le_bytes(
            bytes.get(at + 4..at + 8)?.try_into().ok()?,
        ))
        .ok()?;
        let data = bytes.get(at + 8..(at + 8).checked_add(size)?)?;
        if kind == b"EXIF" {
            // Some writers keep JPEG's "Exif" prefix.
            return tiff(data.strip_prefix(b"Exif\0\0").unwrap_or(data));
        }
        // Chunks are padded to an even size.
        at = at + 8 + size + (size & 1);
    }
    None
}

/// TIFF, and the TIFF structure inside Exif: `XResolution` in the first directory, in the unit
/// `ResolutionUnit` names (inches when it is absent; none at all for 1).
fn tiff(bytes: &[u8]) -> Option<f64> {
    let little = match bytes.get(..4)? {
        b"II*\0" => true,
        b"MM\0*" => false,
        _ => return None,
    };
    let u16_at = |at: usize| -> Option<u16> {
        let b: [u8; 2] = bytes.get(at..at.checked_add(2)?)?.try_into().ok()?;
        Some(if little {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    };
    let u32_at = |at: usize| -> Option<u32> {
        let b: [u8; 4] = bytes.get(at..at.checked_add(4)?)?.try_into().ok()?;
        Some(if little {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    };
    let directory = usize::try_from(u32_at(4)?).ok()?;
    let entries = u16_at(directory)?;
    let mut resolution = None;
    let mut unit = 2;
    for index in 0..usize::from(entries) {
        let entry = directory.checked_add(2 + index * 12)?;
        let (tag, kind) = (u16_at(entry)?, u16_at(entry + 2)?);
        match (tag, kind) {
            // XResolution, a RATIONAL stored at an offset.
            (282, 5) => {
                let at = usize::try_from(u32_at(entry + 8)?).ok()?;
                let (n, d) = (u32_at(at)?, u32_at(at + 4)?);
                resolution = (d != 0).then(|| f64::from(n) / f64::from(d));
            }
            // ResolutionUnit, a SHORT stored in place.
            (296, 3) => unit = u16_at(entry + 8)?,
            _ => {}
        }
    }
    let resolution = resolution?;
    let ppi = match unit {
        2 => resolution,
        3 => resolution * CM_PER_INCH,
        _ => return None,
    };
    // Rationals hold 300 exactly, but per-centimetre values are often rounded.
    let whole = ppi.round();
    Some(if (ppi - whole).abs() < 0.01 {
        whole
    } else {
        ppi
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A TIFF header and one directory with XResolution and, when given, ResolutionUnit.
    fn tiff_bytes(little: bool, n: u32, d: u32, unit: Option<u16>) -> Vec<u8> {
        let u16b = |v: u16| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let u32b = |v: u32| {
            if little {
                v.to_le_bytes()
            } else {
                v.to_be_bytes()
            }
        };
        let mut out = Vec::new();
        out.extend_from_slice(if little { b"II*\0" } else { b"MM\0*" });
        out.extend_from_slice(&u32b(8));
        let entries: u16 = if unit.is_some() { 2 } else { 1 };
        out.extend_from_slice(&u16b(entries));
        let rational_at = 8 + 2 + u32::from(entries) * 12 + 4;
        out.extend_from_slice(&u16b(282));
        out.extend_from_slice(&u16b(5));
        out.extend_from_slice(&u32b(1));
        out.extend_from_slice(&u32b(rational_at));
        if let Some(unit) = unit {
            out.extend_from_slice(&u16b(296));
            out.extend_from_slice(&u16b(3));
            out.extend_from_slice(&u32b(1));
            out.extend_from_slice(&u16b(unit));
            out.extend_from_slice(&[0, 0]);
        }
        out.extend_from_slice(&u32b(0));
        out.extend_from_slice(&u32b(n));
        out.extend_from_slice(&u32b(d));
        out
    }

    fn jfif(units: u8, density: u16) -> Vec<u8> {
        let mut out = vec![0xFF, 0xD8, 0xFF, 0xE0, 0, 16];
        out.extend_from_slice(b"JFIF\0\x01\x02");
        out.push(units);
        out.extend_from_slice(&density.to_be_bytes());
        out.extend_from_slice(&density.to_be_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    fn with_exif(mut jpeg: Vec<u8>, exif: &[u8]) -> Vec<u8> {
        let length = u16::try_from(exif.len() + 8).unwrap();
        jpeg.extend_from_slice(&[0xFF, 0xE1]);
        jpeg.extend_from_slice(&length.to_be_bytes());
        jpeg.extend_from_slice(b"Exif\0\0");
        jpeg.extend_from_slice(exif);
        jpeg.extend_from_slice(&[0xFF, 0xDA, 0, 2, 0xFF, 0xD9]);
        jpeg
    }

    #[test]
    fn reads_tiff_in_both_byte_orders_and_units() {
        for little in [true, false] {
            assert_eq!(read(&tiff_bytes(little, 300, 1, None)), Some(300.0));
            assert_eq!(read(&tiff_bytes(little, 600, 2, Some(2))), Some(300.0));
            assert_eq!(read(&tiff_bytes(little, 11811, 100, Some(3))), Some(300.0));
            assert_eq!(read(&tiff_bytes(little, 2995, 10, Some(2))), Some(299.5));
            // No unit, no division, out of range.
            assert_eq!(read(&tiff_bytes(little, 300, 1, Some(1))), None);
            assert_eq!(read(&tiff_bytes(little, 300, 0, Some(2))), None);
            assert_eq!(read(&tiff_bytes(little, 0, 1, Some(2))), None);
            assert_eq!(read(&tiff_bytes(little, 96_000, 1, Some(2))), None);
        }
    }

    #[test]
    fn reads_jpeg_jfif_then_exif() {
        assert_eq!(read(&jfif(1, 300)), Some(300.0));
        assert_eq!(read(&jfif(2, 118)), Some(300.0));
        // JFIF without a unit states an aspect ratio; Exif then gives the resolution.
        assert_eq!(read(&jfif(0, 1)), None);
        let exif = tiff_bytes(false, 240, 1, Some(2));
        assert_eq!(read(&with_exif(jfif(0, 1), &exif)), Some(240.0));
        assert_eq!(read(&with_exif(vec![0xFF, 0xD8], &exif)), Some(240.0));
        // JFIF's own density wins.
        assert_eq!(read(&with_exif(jfif(1, 150), &exif)), Some(150.0));
    }

    #[test]
    fn reads_png_phys_and_webp_exif() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        let chunk = |png: &mut Vec<u8>, kind: &[u8], data: &[u8]| {
            png.extend_from_slice(&u32::try_from(data.len()).unwrap().to_be_bytes());
            png.extend_from_slice(kind);
            png.extend_from_slice(data);
            png.extend_from_slice(&[0; 4]);
        };
        chunk(&mut png, b"IHDR", &[0; 13]);
        let mut phys = 11811_u32.to_be_bytes().to_vec();
        phys.extend_from_slice(&11811_u32.to_be_bytes());
        phys.push(1);
        let mut aspect = png.clone();
        chunk(&mut png, b"pHYs", &phys);
        assert_eq!(read(&png), Some(300.0));
        *phys.last_mut().unwrap() = 0;
        chunk(&mut aspect, b"pHYs", &phys);
        assert_eq!(read(&aspect), None);

        let exif = tiff_bytes(true, 72, 1, Some(2));
        let mut webp = b"RIFF\0\0\0\0WEBP".to_vec();
        webp.extend_from_slice(b"VP8X");
        webp.extend_from_slice(&3_u32.to_le_bytes());
        webp.extend_from_slice(&[0, 0, 0, 0]); // padded to even
        webp.extend_from_slice(b"EXIF");
        webp.extend_from_slice(&u32::try_from(exif.len()).unwrap().to_le_bytes());
        webp.extend_from_slice(&exif);
        assert_eq!(read(&webp), Some(72.0));
    }

    #[test]
    fn truncated_and_hostile_files_give_none() {
        let samples = [
            tiff_bytes(true, 300, 1, Some(2)),
            jfif(1, 300),
            with_exif(jfif(0, 1), &tiff_bytes(false, 240, 1, Some(2))),
        ];
        for sample in &samples {
            for end in 0..sample.len() {
                // Never panics, whatever is cut off.
                let _ = read(&sample[..end]);
            }
        }
        // An IFD offset or chunk length pointing far away.
        let mut far = tiff_bytes(true, 300, 1, None);
        far[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(read(&far), None);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&u32::MAX.to_be_bytes());
        png.extend_from_slice(b"tEXt");
        assert_eq!(read(&png), None);
        let mut webp = b"RIFF\0\0\0\0WEBPEXIF".to_vec();
        webp.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(read(&webp), None);
        assert_eq!(read(b""), None);
        assert_eq!(read(b"GIF89a"), None);
    }
}
