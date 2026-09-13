// The core formats the stats overlay; these pin the Swift half: the line decode, and a live tier
// cycle that must never move the stored default.

import PunktfunkShared
import XCTest

@testable import PunktfunkKit

final class HudLineTests: XCTestCase {
    func testDecodesRoleTaggedLines() {
        let lines = PunktfunkConnection.HudLine.decode(
            "0\t120 fps · 24.3 Mb/s\n3\tlost 3 (2.4%)\nno tab here\n9\tunknown role\n")
        XCTAssertEqual(lines.map(\.text), ["120 fps · 24.3 Mb/s", "lost 3 (2.4%)", "unknown role"])
        XCTAssertEqual(lines.map(\.role), [.primary, .warn, .primary])
        XCTAssertEqual(PunktfunkConnection.HudLine.decode(""), [])
    }

    func testCycleMovesOnlyTheLiveSession() {
        let key = DefaultsKey.statsVerbosity
        let saved = UserDefaults.standard.string(forKey: key)
        defer {
            if let saved {
                UserDefaults.standard.set(saved, forKey: key)
            } else {
                UserDefaults.standard.removeObject(forKey: key)
            }
            SessionSettings.end()
        }
        UserDefaults.standard.set("detailed", forKey: key)
        var live = EffectiveSettings()
        live.statsVerbosity = "compact"
        SessionSettings.begin(live)
        StatsVerbosity.cycle()
        XCTAssertEqual(SessionSettings.current.statsVerbosity, "normal")
        XCTAssertEqual(UserDefaults.standard.string(forKey: key), "detailed", "the stored tier moved")
    }
}
