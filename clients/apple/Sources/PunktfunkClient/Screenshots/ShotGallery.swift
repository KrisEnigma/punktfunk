// Variant galleries for the shot harness. One scene lays out every state of a component, so a
// card or tile change is reviewed as a sheet of states instead of one mock at a time. The
// catalogs below are the single source: the `14-gallery-*` scenes and the `#Preview`s at the
// bottom mount the same lists. iPad captures fit a whole sheet; a phone shows its top.

#if DEBUG
import PunktfunkKit
import SwiftUI

/// One state of a component, with the caption the gallery prints above it.
struct GalleryVariant: Identifiable {
    let name: String
    let make: @MainActor () -> AnyView
    var id: String { name }
}

/// Caption over content for each variant, in as many columns as fit the width.
struct ShotGalleryView: View {
    let title: String
    let variants: [GalleryVariant]
    /// The narrowest column: a host card wants a phone's width, a poster a fraction of it.
    var minWidth: CGFloat = 340

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 14) {
                Text(title)
                    .font(.geist(24, .bold, relativeTo: .title2))
                LazyVGrid(
                    columns: [GridItem(.adaptive(minimum: minWidth), spacing: 16, alignment: .top)],
                    alignment: .leading, spacing: 20
                ) {
                    ForEach(variants) { variant in
                        VStack(alignment: .leading, spacing: 6) {
                            Text(variant.name)
                                .font(.geist(11, .semibold, relativeTo: .caption2))
                                .tracking(0.8)
                                .textCase(.uppercase)
                                .foregroundStyle(.secondary)
                            variant.make()
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                    }
                }
            }
            .padding()
        }
    }
}

@MainActor
extension ShotMock {
    /// Every state a host card shows, saved and discovered.
    static var hostCardVariants: [GalleryVariant] {
        let battlestation = StoredHost(
            id: battlestationID, name: "Battlestation", address: "192.168.1.20", port: 9777,
            pinnedSHA256: fingerprint, macAddresses: ["a4:b1:c2:d3:e4:f5"], osChain: "windows/11")
        let livingRoom = StoredHost(
            id: livingRoomID, name: "Living Room PC", address: "192.168.1.41", port: 9777,
            pinnedSHA256: hostFingerprint(1), osChain: "linux/fedora/bazzite")
        let office = StoredHost(
            id: officeID, name: "Office NUC", address: "192.168.1.33", port: 9777,
            pinnedSHA256: hostFingerprint(4), profileID: couchProfileID, osChain: "linux/ubuntu")
        let workshop = StoredHost(
            id: workshopID, name: "Workshop", address: "10.0.0.7", port: 9777,
            pinnedSHA256: hostFingerprint(2), macAddresses: ["de:ad:be:ef:00:07"],
            osChain: "linux/arch")
        let bedroom = StoredHost(
            id: bedroomID, name: "Bedroom Mini", address: "192.168.1.77", port: 9777,
            pinnedSHA256: hostFingerprint(6), osChain: "windows/11")
        let unpaired = StoredHost(
            name: "Studio PC", address: "192.168.1.58", port: 9777, osChain: "windows/11")
        let crowded = StoredHost(
            id: editingID, name: "Living Room Gaming PC (behind the TV)",
            address: "fd7a:115c:a1e0::1a2b", port: 9777, pinnedSHA256: hostFingerprint(5),
            profileID: hdrProfileID, osChain: "linux/nobara")
        return [
            GalleryVariant(name: "Online") { card(battlestation) },
            GalleryVariant(name: "Playing") { card(livingRoom, playing: "Hollow Knight") },
            GalleryVariant(name: "Bound preset") { card(office, bound: couchProfileID) },
            GalleryVariant(name: "Pinned preset card") { card(battlestation, pinned: hdrPreset) },
            GalleryVariant(name: "Most recent") { card(battlestation, recent: true) },
            GalleryVariant(name: "Connecting") { card(battlestation, connecting: true) },
            GalleryVariant(name: "Offline, MAC known") { card(workshop, online: false) },
            GalleryVariant(name: "Offline") { card(bedroom, online: false) },
            GalleryVariant(name: "Not paired") { card(unpaired) },
            GalleryVariant(name: "Everything at once") {
                card(crowded, recent: true, bound: hdrProfileID,
                     playing: "Starfall Vale: Definitive Edition")
            },
            GalleryVariant(name: "Discovered") { discovered(pairing: false) },
            GalleryVariant(name: "Discovered, pairing required") { discovered(pairing: true) },
        ]
    }

    /// A poster in each state the touch grid draws.
    static var libraryTileVariants: [GalleryVariant] {
        let byID = Dictionary(
            (games + tileExtras).map { ($0.id, $0) }, uniquingKeysWith: { first, _ in first })
        func tile(_ id: String, running: Bool = false, selected: Bool = false) -> AnyView {
            guard let game = byID[id] else { return AnyView(EmptyView()) }
            return AnyView(GameCard(
                game: game, artLoader: ShotPosterArt.source, selected: selected,
                isRunning: running))
        }
        func desktop(_ title: String) -> AnyView {
            AnyView(GameCard(game: LibraryCollation.desktopEntry(title: title), artLoader: nil))
        }
        return [
            GalleryVariant(name: "Steam") { tile("steam:starfall") },
            GalleryVariant(name: "Custom, platform") { tile("custom:aurora") },
            GalleryVariant(name: "Running") { tile("steam:starfall", running: true) },
            GalleryVariant(name: "Keyboard cursor") { tile("heroic:neon", selected: true) },
            GalleryVariant(name: "Launcher") { tile("steam:launcher") },
            GalleryVariant(name: "No art") { tile("steam:prototype") },
            GalleryVariant(name: "Long title") { tile("custom:collection") },
            GalleryVariant(name: "Desktop tile") { desktop("Desktop") },
            GalleryVariant(name: "Resume tile") { desktop("Resume Hollow Knight") },
        ]
    }

    /// The shelf in every phase it can be in before, during and after a fetch.
    static var libraryStateVariants: [GalleryVariant] {
        func shelf(_ phase: ShotLibraryPhase) -> AnyView {
            let frame = RoundedRectangle(cornerRadius: 12, style: .continuous)
            return AnyView(NavigationStack {
                LibraryView(
                    store: galleryStore, target: LibraryTarget(host: host), onLaunch: { _ in },
                    onConnect: {}, shotPhase: phase)
            }
            .frame(height: 440)
            .clipShape(frame)
            .overlay { frame.strokeBorder(.quaternary, lineWidth: 1) })
        }
        return [
            GalleryVariant(name: "Loading") { shelf(.loading) },
            GalleryVariant(name: "Error") {
                shelf(.error("Couldn't load the library — the host didn't answer"))
            },
            GalleryVariant(name: "Empty") { shelf(.empty) },
            GalleryVariant(name: "Remembered, waking") { shelf(.catalog(games, staleness: .waking)) },
            GalleryVariant(name: "Remembered, offline") {
                shelf(.catalog(games, staleness: .offline))
            },
            GalleryVariant(name: "Live, one title up") {
                shelf(.catalog(games, running: ["steam:starfall"]))
            },
        ]
    }

    /// Two titles the store mock lacks: one with no art at all, one whose name wraps.
    static let tileExtras: [GameEntry] = decodeGames("""
        [
          {"id": "steam:prototype", "store": "steam", "title": "Untitled Prototype", "art": {}},
          {"id": "custom:collection", "store": "custom", "platform": "PS2",
           "title": "The Extraordinarily Long Title of a Remastered Anniversary Collection",
           "art": {"portrait": "shot://art/ember"}}
        ]
        """)

    /// A `/status` entry for `id`. Decoded: `RunningGame`'s memberwise init is PunktfunkKit's.
    static func running(_ id: String, title: String = "Starfall Vale") -> RunningGame? {
        let json = #"{"app_id": "\#(id)", "title": "\#(title)", "state": "running"}"#
        return try? JSONDecoder().decode(RunningGame.self, from: Data(json.utf8))
    }

    static func decodeGames(_ json: String) -> [GameEntry] {
        (try? JSONDecoder().decode([GameEntry].self, from: Data(json.utf8))) ?? []
    }

    /// One store for every gallery shelf; the shelves read it only to address the host.
    private static let galleryStore = hostStore()

    private static func card(
        _ host: StoredHost, online: Bool = true, connecting: Bool = false, recent: Bool = false,
        bound: String? = nil, pinned: StreamPreset? = nil, playing: String? = nil
    ) -> AnyView {
        let menu = HostPresetMenu(
            profiles: [hdrPreset, couchPreset], boundID: bound,
            pinnedIDs: pinned.map { [$0.id] } ?? [], connectWith: { _ in }, setDefault: { _ in },
            togglePin: { _ in }, copyLink: { _ in })
        return AnyView(HostCardView(
            host: host, isOnline: online, isConnecting: connecting, isMostRecent: recent,
            isBusy: false, onConnect: {}, onPair: {}, onForget: {}, onRemove: {},
            presetMenu: menu, pinnedPreset: pinned, nowPlaying: playing))
    }

    private static func discovered(pairing: Bool) -> AnyView {
        let advert = HostDiscovery.debugAdvert(
            id: pairing ? "studio" : "den", name: pairing ? "Studio PC" : "Den Steam Deck",
            host: pairing ? "192.168.1.58" : "192.168.1.90",
            fingerprintHex: hostFingerprint(pairing ? 3 : 7).hexLower,
            requiresPairing: pairing, allowsTofu: !pairing,
            osChain: pairing ? "windows/11" : "linux/steamos")
        return AnyView(DiscoveredCardView(discovered: advert, isBusy: false, onConnect: {}))
    }
}

/// The touch grid on the mock catalog with one title up — what the Library tab grows from.
struct ShotLibraryTouch: View {
    @StateObject private var store = ShotMock.hostStore()

    var body: some View {
        NavigationStack {
            LibraryView(
                store: store, target: LibraryTarget(host: ShotMock.host), onLaunch: { _ in },
                onConnect: {}, shotPhase: .catalog(ShotMock.games, running: ["steam:starfall"]))
        }
    }
}

#Preview("Host cards") {
    ShotGalleryView(title: "Host cards", variants: ShotMock.hostCardVariants)
        .preferredColorScheme(.dark)
}

#Preview("Library tiles") {
    ShotGalleryView(title: "Library tiles", variants: ShotMock.libraryTileVariants, minWidth: 150)
        .preferredColorScheme(.dark)
}

#Preview("Library states") {
    ShotGalleryView(title: "Library states", variants: ShotMock.libraryStateVariants)
        .preferredColorScheme(.dark)
}

#Preview("Library") {
    ShotLibraryTouch()
        .preferredColorScheme(.dark)
}
#endif
