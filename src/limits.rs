//! Size limits that follow the computer's memory.
//!
//! Layers are kept in memory as 8-bit RGBA, so how large a document can be depends on how
//! much memory Xuan runs with. Each limit is at least what Xuan always allowed (100
//! megapixels, 512 MiB of undo and RAW data) and grows in proportion to the memory:
//!
//! - one canvas, layer or mask: a pixel for every 64 bytes, leaving room for the full-size
//!   buffers that compositing, effects and filters allocate (2 gigapixels with 128 GiB);
//! - the layers a project brings in when it is opened or imported: a pixel for every 16 bytes,
//!   so they take at most a quarter of the memory (8 gigapixels with 128 GiB); masks have a
//!   budget of the same size;
//! - undo history: an eighth of the memory; embedded RAW files: a thirty-second.
//!
//! The memory is the physical memory, or the control group's limit when that is lower (as in a
//! container). `XUAN_MEMORY` (bytes, or with a K, M, G or T suffix) overrides it, for example
//! to try the limits of a smaller machine. The library's unit tests always use the minimum
//! limits; the app's tests see the real ones, so they only rely on [`MAX_SIDE`].

use std::sync::OnceLock;

/// Longest canvas, layer or mask side. JPEG cannot store more, and it keeps every pixel index
/// of an image (`MAX_SIDE²`, below 2³²) within `u32`.
pub const MAX_SIDE: u32 = 65_535;

const MIB: u64 = 1024 * 1024;
const GIB: u64 = 1024 * MIB;
/// The least pixels any limit allows: Xuan's limit before it followed the memory.
const MIN_PIXELS: u64 = 100_000_000;
const MIN_HISTORY: u64 = 512 * MIB;
const MIN_RAW: u64 = 512 * MIB;
const MIN_FILE: u64 = 512 * MIB;
const MIN_PHOTOSHOP_FILE: u64 = GIB;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// The memory the limits follow, or `None` when it is unknown.
    pub memory: Option<u64>,
    /// Most pixels in one canvas, layer or mask.
    pub image_pixels: u64,
    /// Most layer pixels that opening or importing a file may add; masks have their own
    /// budget of the same size.
    pub project_pixels: u64,
    /// Bytes of pixels and RAW data that undo history may hold.
    pub history_bytes: u64,
    /// Bytes of RAW files embedded in one project.
    pub raw_bytes: u64,
}

impl Limits {
    /// The limits for `memory` bytes, or the minimum limits when it is unknown.
    pub fn for_memory(memory: Option<u64>) -> Self {
        let bytes = memory.unwrap_or(0);
        Self {
            memory,
            image_pixels: (bytes / 64).clamp(MIN_PIXELS, u64::from(MAX_SIDE).pow(2)),
            project_pixels: (bytes / 16).max(MIN_PIXELS),
            history_bytes: (bytes / 8).max(MIN_HISTORY),
            raw_bytes: (bytes / 32).max(MIN_RAW),
        }
    }

    /// Largest image file read whole: an image at the limit, uncompressed at 16 bits per
    /// channel, with room for metadata.
    pub fn file_bytes(&self) -> u64 {
        (self.image_pixels * 8 + 64 * MIB).max(MIN_FILE)
    }

    /// Largest Photoshop file read whole: a quarter of the memory, like the project's layers.
    pub fn photoshop_file_bytes(&self) -> u64 {
        (self.project_pixels * 4).max(MIN_PHOTOSHOP_FILE)
    }
}

/// The limits for this computer, worked out once.
pub fn get() -> Limits {
    static LIMITS: OnceLock<Limits> = OnceLock::new();
    *LIMITS.get_or_init(|| Limits::for_memory(memory()))
}

/// A pixel count for messages: "100 megapixels", "2,147 megapixels".
pub fn megapixels(pixels: u64) -> String {
    format!("{} megapixels", grouped(pixels / 1_000_000))
}

/// A byte count for messages: "512 MiB", "1 GiB", "16.1 GiB".
pub fn size(bytes: u64) -> String {
    if bytes >= GIB {
        let gib = bytes as f64 / GIB as f64;
        let text = format!("{gib:.1}");
        format!("{} GiB", text.strip_suffix(".0").unwrap_or(&text))
    } else {
        format!("{} MiB", bytes.div_ceil(MIB))
    }
}

/// Digits in groups of three: "65,535".
pub fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

fn memory() -> Option<u64> {
    if cfg!(test) {
        return None;
    }
    if let Ok(value) = std::env::var("XUAN_MEMORY") {
        match parse_size(&value) {
            Some(bytes) => return Some(bytes),
            None => eprintln!("xuan: ignoring XUAN_MEMORY={value:?}; use bytes or a size like 16G"),
        }
    }
    detect()
}

/// `16G`, `16GiB`, `16 GB`, `512M`, `1T` or plain bytes; binary units.
fn parse_size(text: &str) -> Option<u64> {
    let text = text.trim();
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let number: u64 = number.parse().ok()?;
    let shift = match unit.trim().to_ascii_lowercase().as_str() {
        "" | "b" => 0,
        "k" | "kb" | "kib" => 10,
        "m" | "mb" | "mib" => 20,
        "g" | "gb" | "gib" => 30,
        "t" | "tb" | "tib" => 40,
        _ => return None,
    };
    number.checked_mul(1 << shift).filter(|&bytes| bytes > 0)
}

#[cfg(target_os = "linux")]
fn detect() -> Option<u64> {
    let total = parse_meminfo(&std::fs::read_to_string("/proc/meminfo").ok()?)?;
    Some(cgroup_limit().map_or(total, |limit| limit.min(total)))
}

#[cfg(windows)]
fn detect() -> Option<u64> {
    use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
    // SAFETY: an all-zero MEMORYSTATUSEX is valid; the call needs only `dwLength` set.
    let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
    status.dwLength = size_of::<MEMORYSTATUSEX>() as u32;
    // SAFETY: `status` is a writable MEMORYSTATUSEX whose `dwLength` is its size.
    (unsafe { GlobalMemoryStatusEx(&mut status) } != 0 && status.ullTotalPhys > 0)
        .then_some(status.ullTotalPhys)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn detect() -> Option<u64> {
    None
}

/// `MemTotal` from `/proc/meminfo`, in bytes.
#[cfg(any(target_os = "linux", test))]
fn parse_meminfo(meminfo: &str) -> Option<u64> {
    let line = meminfo.lines().find(|l| l.starts_with("MemTotal:"))?;
    let mut words = line["MemTotal:".len()..].split_whitespace();
    let value: u64 = words.next()?.parse().ok()?;
    let unit = words.next().unwrap_or("kB");
    value
        .checked_mul(if unit.eq_ignore_ascii_case("kb") {
            1024
        } else {
            1
        })
        .filter(|&bytes| bytes > 0)
}

/// The tightest memory limit of this process's control group and its parents.
#[cfg(target_os = "linux")]
fn cgroup_limit() -> Option<u64> {
    let membership = std::fs::read_to_string("/proc/self/cgroup").ok()?;
    let root = std::path::Path::new("/sys/fs/cgroup");
    // cgroup v2: a single "0::/path" line; check the group and each parent.
    if let Some(path) = membership.lines().find_map(|l| l.strip_prefix("0::")) {
        let mut dir = root.join(path.trim_start_matches('/'));
        let mut limit: Option<u64> = None;
        while dir.starts_with(root) {
            if let Some(value) = std::fs::read_to_string(dir.join("memory.max"))
                .ok()
                .and_then(|text| parse_cgroup_limit(&text))
            {
                limit = Some(limit.map_or(value, |l| l.min(value)));
            }
            if !dir.pop() {
                break;
            }
        }
        return limit;
    }
    // cgroup v1.
    std::fs::read_to_string(root.join("memory/memory.limit_in_bytes"))
        .ok()
        .and_then(|text| parse_cgroup_limit(&text))
}

/// A control group memory limit: bytes, or none for `max` and v1's "unlimited" huge value.
#[cfg(any(target_os = "linux", test))]
fn parse_cgroup_limit(text: &str) -> Option<u64> {
    let value: u64 = text.trim().parse().ok()?;
    (value > 0 && value < 1 << 60).then_some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_or_small_memory_keeps_the_old_limits() {
        for memory in [None, Some(0), Some(GIB)] {
            let limits = Limits::for_memory(memory);
            assert_eq!(limits.image_pixels, 100_000_000, "{memory:?}");
            assert_eq!(limits.project_pixels, 100_000_000, "{memory:?}");
            assert_eq!(limits.history_bytes, 512 * MIB, "{memory:?}");
            assert_eq!(limits.raw_bytes, 512 * MIB, "{memory:?}");
            assert_eq!(limits.photoshop_file_bytes(), GIB, "{memory:?}");
        }
        // 4 GiB already allows more layers, but not a larger single image.
        let small = Limits::for_memory(Some(4 * GIB));
        assert_eq!(small.image_pixels, 100_000_000);
        assert_eq!(small.project_pixels, 4 * GIB / 16);
        assert_eq!(
            get(),
            Limits::for_memory(None),
            "unit tests use the minimum limits"
        );
    }

    #[test]
    fn limits_grow_with_memory() {
        let limits = Limits::for_memory(Some(128 * GIB));
        assert_eq!(limits.image_pixels, 2 * GIB);
        assert_eq!(limits.project_pixels, 8 * GIB);
        assert_eq!(limits.history_bytes, 16 * GIB);
        assert_eq!(limits.raw_bytes, 4 * GIB);
        assert_eq!(limits.photoshop_file_bytes(), 32 * GIB);
        assert_eq!(limits.file_bytes(), 16 * GIB + 64 * MIB);

        // A 100-megapixel camera (Fujifilm GFX100, 11648 × 8736) fits from 8 GiB up.
        let gfx = 11_648 * 8_736;
        assert!(Limits::for_memory(Some(4 * GIB)).image_pixels < gfx);
        assert!(Limits::for_memory(Some(8 * GIB)).image_pixels >= gfx);

        // However much memory there is, an image keeps within MAX_SIDE².
        let huge = Limits::for_memory(Some(u64::MAX));
        assert_eq!(huge.image_pixels, u64::from(MAX_SIDE).pow(2));
        assert!(huge.image_pixels < 1 << 32);
    }

    #[test]
    fn parses_memory_sizes() {
        assert_eq!(parse_size("16G"), Some(16 * GIB));
        assert_eq!(parse_size(" 16 GiB "), Some(16 * GIB));
        assert_eq!(parse_size("16gb"), Some(16 * GIB));
        assert_eq!(parse_size("512M"), Some(512 * MIB));
        assert_eq!(parse_size("1T"), Some(1024 * GIB));
        assert_eq!(parse_size("4096"), Some(4096));
        for bad in ["", "G", "16X", "-1G", "0", "1.5G", "99999999999999T"] {
            assert_eq!(parse_size(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn reads_meminfo_and_cgroup_limits() {
        let meminfo = "MemTotal:       131151476 kB\nMemFree:  1000 kB\n";
        assert_eq!(parse_meminfo(meminfo), Some(131_151_476 * 1024));
        assert_eq!(parse_meminfo("MemFree: 1 kB\n"), None);
        assert_eq!(parse_cgroup_limit("8589934592\n"), Some(8 * GIB));
        assert_eq!(parse_cgroup_limit("max\n"), None);
        assert_eq!(parse_cgroup_limit("9223372036854771712\n"), None);
    }

    #[test]
    fn formats_limits_for_messages() {
        assert_eq!(megapixels(100_000_000), "100 megapixels");
        assert_eq!(megapixels(2 * GIB), "2,147 megapixels");
        assert_eq!(size(512 * MIB), "512 MiB");
        assert_eq!(size(GIB), "1 GiB");
        assert_eq!(size(16 * GIB + 64 * MIB), "16.1 GiB");
        assert_eq!(grouped(65_535), "65,535");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_000_000), "1,000,000");
    }
}
