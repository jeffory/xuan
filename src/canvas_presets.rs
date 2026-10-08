//! Canvas size presets for File → New and the arithmetic behind its Swap and Keep aspect ratio
//! controls. Plain data and pure functions, so the dialog only draws them.
use crate::document::MAX_SIDE;

/// A named canvas size, in pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Preset {
    /// Shown in the menu (translated there); the size is shown beside it.
    pub name: &'static str,
    pub width: u32,
    pub height: u32,
}

/// A heading and the presets under it, in menu order.
#[derive(Clone, Copy, Debug)]
pub struct Group {
    pub name: &'static str,
    pub presets: &'static [Preset],
}

const fn preset(name: &'static str, width: u32, height: u32) -> Preset {
    Preset {
        name,
        width,
        height,
    }
}

// Paper sizes (A3, A4, Letter, ...) are defined in millimetres or inches, so they wait on
// physical units (issue 94) and are not offered yet.
pub const GROUPS: &[Group] = &[
    Group {
        name: "Screens",
        presets: &[
            preset("4K", 3840, 2160),
            preset("1440p", 2560, 1440),
            preset("1080p", 1920, 1080),
            preset("720p", 1280, 720),
        ],
    },
    Group {
        name: "Social",
        presets: &[
            preset("Square post", 1080, 1080),
            preset("Portrait post", 1080, 1350),
            preset("Landscape post", 1080, 566),
            preset("Story / Reel", 1080, 1920),
            preset("Video thumbnail", 1280, 720),
            preset("Link preview", 1200, 630),
            preset("Banner", 1500, 500),
        ],
    },
];

/// Every preset, in menu order.
pub fn all() -> impl Iterator<Item = &'static Preset> {
    GROUPS.iter().flat_map(|group| group.presets.iter())
}

/// The preset a size shows as in the menu, or `None` for Custom. A size matches a preset in
/// either orientation, so a swapped 1080p still shows as 1080p; an exact match wins over a
/// swapped one (1080 × 1920 is Story / Reel, not a turned 1080p).
pub fn find(width: u32, height: u32) -> Option<&'static Preset> {
    all()
        .find(|p| p.width == width && p.height == height)
        .or_else(|| all().find(|p| p.width == height && p.height == width))
}

/// `size` with width and height exchanged: portrait becomes landscape and back.
pub fn swapped(size: [u32; 2]) -> [u32; 2] {
    [size[1], size[0]]
}

/// The side that keeps `reference` (width, height) in proportion when `edited` is the new
/// length of the other side. Rounds to the nearest pixel and stays within `1..=MAX_SIDE`.
/// A reference with a zero side has no proportion, so `fallback` is returned unchanged.
pub fn linked_side(reference_edited: u32, reference_other: u32, edited: u32, fallback: u32) -> u32 {
    if reference_edited == 0 || reference_other == 0 {
        return fallback;
    }
    let scaled = (u128::from(reference_other) * u128::from(edited)
        + u128::from(reference_edited) / 2)
        / u128::from(reference_edited);
    u32::try_from(scaled).map_or(MAX_SIDE, |v| v.clamp(1, MAX_SIDE))
}

/// The remembered size when it is still a usable canvas on this computer.
pub fn usable(size: Option<[u32; 2]>) -> Option<[u32; 2]> {
    size.filter(|[w, h]| crate::document::validate_size(*w, *h).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_is_sane() {
        let mut names = std::collections::HashSet::new();
        for p in all() {
            assert!(names.insert(p.name), "duplicate preset {}", p.name);
            assert!(crate::document::validate_size(p.width, p.height).is_ok());
        }
        assert!(GROUPS.iter().all(|g| !g.presets.is_empty()));
        assert_eq!(find(3840, 2160).unwrap().name, "4K");
    }

    #[test]
    fn exact_swapped_and_custom_matches() {
        assert_eq!(find(1920, 1080).unwrap().name, "1080p");
        assert_eq!(find(1080, 1350).unwrap().name, "Portrait post");
        // Turned presets keep their name.
        assert_eq!(find(1350, 1080).unwrap().name, "Portrait post");
        assert_eq!(find(1080, 1920).unwrap().name, "Story / Reel");
        assert_eq!(find(2160, 3840).unwrap().name, "4K");
        // A size in two groups shows as the first.
        assert_eq!(find(1280, 720).unwrap().name, "720p");
        assert_eq!(find(1081, 1080), None);
        assert_eq!(find(0, 0), None);
    }

    #[test]
    fn swap_turns_portrait_into_landscape() {
        assert_eq!(swapped([1080, 1920]), [1920, 1080]);
        assert_eq!(swapped(swapped([3, 7])), [3, 7]);
        assert_eq!(swapped([5, 5]), [5, 5]);
    }

    #[test]
    fn linked_side_rounds_to_the_nearest_pixel() {
        // 16:9
        assert_eq!(linked_side(1920, 1080, 960, 0), 540);
        assert_eq!(linked_side(1920, 1080, 1000, 0), 563); // 562.5 rounds up
        assert_eq!(linked_side(1920, 1080, 1001, 0), 563); // 563.06
        assert_eq!(linked_side(1080, 1920, 1, 0), 2); // 1.78
        // Heights scale to widths the same way.
        assert_eq!(linked_side(1080, 1920, 540, 0), 960);
    }

    #[test]
    fn linked_side_respects_limits_and_zero() {
        assert_eq!(linked_side(1000, 1000, 0, 7), 1);
        assert_eq!(linked_side(1000, 10, 1, 7), 1); // 0.01 stays a pixel
        assert_eq!(linked_side(1, 100, MAX_SIDE, 7), MAX_SIDE);
        assert_eq!(linked_side(1, u32::MAX, u32::MAX, 7), MAX_SIDE);
        assert_eq!(linked_side(0, 100, 50, 33), 33);
        assert_eq!(linked_side(100, 0, 50, 33), 33);
    }

    #[test]
    fn remembered_size_must_be_usable() {
        assert_eq!(usable(Some([800, 600])), Some([800, 600]));
        assert_eq!(usable(Some([0, 600])), None);
        assert_eq!(usable(Some([MAX_SIDE + 1, 1])), None);
        assert_eq!(usable(None), None);
    }
}
