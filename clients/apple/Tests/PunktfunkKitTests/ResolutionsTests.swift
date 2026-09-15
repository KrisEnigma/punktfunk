import XCTest

import PunktfunkShared

/// Pins the aspect-family table (`Resolutions`) the pickers share with the Rust and Kotlin
/// twins: every listed size finds its own family, families are told apart by shape, and a family
/// switch lands on the size nearest the current height.
final class ResolutionsTests: XCTestCase {
    func testEveryListedSizeMapsBackToItsFamily() {
        for (i, a) in Resolutions.aspects.enumerated() {
            for s in a.sizes {
                XCTAssertEqual(Resolutions.aspectOf(s.w, s.h), i, "\(s.w)x\(s.h) → \(a.label)")
                XCTAssertTrue(s.w % 2 == 0 && s.h % 2 == 0, "\(s.w)x\(s.h) has an odd side")
            }
            XCTAssertEqual(a.sizes.map(\.h), a.sizes.map(\.h).sorted(), "\(a.label) ascending")
        }
    }

    func testShapeNotMembership() {
        XCTAssertEqual(Resolutions.aspectOf(1500, 1000), 4, "a custom 3:2")
        XCTAssertEqual(Resolutions.aspectOf(3456, 2234), 1, "a MacBook panel reads 16:10")
        XCTAssertNil(Resolutions.aspectOf(2556, 1179), "a phone panel is nobody's")
        XCTAssertNil(Resolutions.aspectOf(0, 0), "native")
    }

    func testNearestFollowsHeightAndNativeMeans1080() {
        XCTAssertTrue(Resolutions.nearest(0, height: 0) == (1920, 1080))
        XCTAssertTrue(Resolutions.nearest(1, height: 1080) == (1920, 1200))
        XCTAssertTrue(Resolutions.nearest(2, height: 1440) == (3440, 1440))
        XCTAssertTrue(Resolutions.nearest(3, height: 2160) == (7680, 2160))
        XCTAssertTrue(Resolutions.nearest(4, height: 800) == (2160, 1440))
        XCTAssertTrue(Resolutions.nearest(5, height: 1000) == (1600, 1200))
    }
}
