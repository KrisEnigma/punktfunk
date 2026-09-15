// Stream-mode presets grouped by aspect ratio — the twin of `punktfunk_core::resolutions` (and
// Android's `Resolutions` in Settings.kt). A picker shows one family at a time behind an aspect
// switch, plus its own native rows. Picking a family moves to its size nearest the current height
// (`nearest`), so the switch and the list always agree without picker-side state. Pure; tested in
// `ResolutionsTests`.

import Foundation

public enum Resolutions {
    /// One family of sizes with the same shape.
    public struct Aspect {
        /// The switch label: "16:9".
        public let label: String
        /// Width over height of the shape.
        public let shape: Double
        /// Common panels of this shape, ascending. All sides even (the host rejects odd modes).
        public let sizes: [(w: Int, h: Int)]
    }

    /// Families in the order the switch shows them: most common first.
    public static let aspects: [Aspect] = [
        Aspect(
            label: "16:9", shape: 16 / 9,
            sizes: [(1280, 720), (1920, 1080), (2560, 1440), (3840, 2160), (5120, 2880)]),
        Aspect(
            label: "16:10", shape: 16 / 10,
            sizes: [(1280, 800), (1920, 1200), (2560, 1600), (2880, 1800), (3840, 2400)]),
        Aspect(
            label: "21:9", shape: 21 / 9,
            sizes: [(2560, 1080), (3440, 1440), (3840, 1600), (5120, 2160)]),
        Aspect(label: "32:9", shape: 32 / 9, sizes: [(3840, 1080), (5120, 1440), (7680, 2160)]),
        Aspect(
            label: "3:2", shape: 3 / 2,
            sizes: [(2160, 1440), (2256, 1504), (2880, 1920), (3000, 2000)]),
        Aspect(label: "4:3", shape: 4 / 3, sizes: [(1024, 768), (1600, 1200), (2048, 1536)]),
    ]

    /// Shape tolerance for `aspectOf`. "21:9" panels are really 2.37–2.40, so 4 % keeps them in
    /// one family and still parts 16:10 (1.60) from 3:2 (1.50).
    static let tolerance = 0.04

    /// The family `w`×`h` belongs to by shape, not by membership: a custom 1500×1000 is 3:2.
    /// `nil` for a zero side (native) or a shape no family has.
    public static func aspectOf(_ w: Int, _ h: Int) -> Int? {
        guard w > 0, h > 0 else { return nil }
        let shape = Double(w) / Double(h)
        return aspects.firstIndex { abs(shape / $0.shape - 1) < tolerance }
    }

    /// The size in family `aspect` nearest in height to `h`; a native `0` looks for 1080. Ties go
    /// to the smaller size.
    public static func nearest(_ aspect: Int, height h: Int) -> (w: Int, h: Int) {
        let h = h == 0 ? 1080 : h
        return aspects[aspect].sizes.min { abs($0.h - h) < abs($1.h - h) }!
    }
}
