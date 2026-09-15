//! Stream-mode presets grouped by aspect ratio. One table so every client's
//! picker offers the same families and sizes; twins in
//! `PunktfunkShared/Resolutions.swift` and Android `Settings.kt`.
//!
//! A picker shows one family at a time behind an aspect switch, plus its own
//! native / match-window rows. Picking a family moves to its size nearest the
//! current height ([`nearest`]), so the switch and the list always agree
//! without any picker-side state. Pure; tested here.

/// One family of sizes with the same shape.
pub struct Aspect {
    /// The switch label: `"16:9"`.
    pub label: &'static str,
    /// `(width, height)` units of the shape.
    pub ratio: (u32, u32),
    /// Common panels of this shape, ascending. All sides even (the host
    /// rejects odd modes).
    pub sizes: &'static [(u32, u32)],
}

/// Families in the order the switch shows them: most common first.
pub const ASPECTS: [Aspect; 6] = [
    Aspect {
        label: "16:9",
        ratio: (16, 9),
        sizes: &[
            (1280, 720),
            (1920, 1080),
            (2560, 1440),
            (3840, 2160),
            (5120, 2880),
        ],
    },
    Aspect {
        label: "16:10",
        ratio: (16, 10),
        sizes: &[
            (1280, 800),
            (1920, 1200),
            (2560, 1600),
            (2880, 1800),
            (3840, 2400),
        ],
    },
    Aspect {
        label: "21:9",
        ratio: (21, 9),
        sizes: &[(2560, 1080), (3440, 1440), (3840, 1600), (5120, 2160)],
    },
    Aspect {
        label: "32:9",
        ratio: (32, 9),
        sizes: &[(3840, 1080), (5120, 1440), (7680, 2160)],
    },
    Aspect {
        label: "3:2",
        ratio: (3, 2),
        sizes: &[(2160, 1440), (2256, 1504), (2880, 1920), (3000, 2000)],
    },
    Aspect {
        label: "4:3",
        ratio: (4, 3),
        sizes: &[(1024, 768), (1600, 1200), (2048, 1536)],
    },
];

/// Shape tolerance for [`aspect_of`]. "21:9" panels are really 2.37–2.40, so
/// 4 % keeps them in one family and still parts 16:10 (1.60) from 3:2 (1.50).
const TOLERANCE: f64 = 0.04;

/// The family `w`×`h` belongs to by shape, not by membership: a custom
/// 1500×1000 is 3:2. `None` for a zero side (native) or a shape no family has.
pub fn aspect_of(w: u32, h: u32) -> Option<usize> {
    if w == 0 || h == 0 {
        return None;
    }
    let shape = f64::from(w) / f64::from(h);
    ASPECTS.iter().position(|a| {
        let want = f64::from(a.ratio.0) / f64::from(a.ratio.1);
        (shape / want - 1.0).abs() < TOLERANCE
    })
}

/// The size in family `aspect` nearest in height to `h`; a native `0`
/// looks for 1080. Ties go to the smaller size.
pub fn nearest(aspect: usize, h: u32) -> (u32, u32) {
    let h = if h == 0 { 1080 } else { h };
    *ASPECTS[aspect]
        .sizes
        .iter()
        .min_by_key(|(_, sh)| sh.abs_diff(h))
        .expect("every family lists a size")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_listed_size_maps_back_to_its_family() {
        for (i, a) in ASPECTS.iter().enumerate() {
            for &(w, h) in a.sizes {
                assert_eq!(aspect_of(w, h), Some(i), "{w}x{h} → {}", a.label);
                assert!(w % 2 == 0 && h % 2 == 0, "{w}x{h} has an odd side");
            }
            assert!(
                a.sizes.windows(2).all(|p| p[0].1 <= p[1].1),
                "{} ascending",
                a.label
            );
        }
    }

    #[test]
    fn shape_not_membership() {
        assert_eq!(aspect_of(1500, 1000), Some(4), "a custom 3:2");
        assert_eq!(
            aspect_of(3456, 2234),
            Some(1),
            "a MacBook panel reads 16:10"
        );
        assert_eq!(aspect_of(2556, 1179), None, "a phone panel is nobody's");
        assert_eq!(aspect_of(0, 0), None, "native");
        assert_eq!(aspect_of(1920, 0), None);
    }

    #[test]
    fn nearest_follows_height_and_native_means_1080() {
        assert_eq!(nearest(0, 0), (1920, 1080));
        assert_eq!(nearest(1, 1080), (1920, 1200));
        assert_eq!(nearest(2, 1440), (3440, 1440));
        assert_eq!(nearest(3, 2160), (7680, 2160));
        assert_eq!(nearest(4, 800), (2160, 1440));
        assert_eq!(nearest(5, 1000), (1600, 1200));
    }
}
