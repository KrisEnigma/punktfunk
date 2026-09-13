// The Library tab (design/apple-touch-ui-overhaul.md §2.5) and the Mac's Library row: one shelf,
// picked from the host filter over it and remembered. `LibraryView` owns the fetch, cache, wake
// and art; this view picks the shelf, and what to say with no paired host. A written
// `libraryTarget` lands here through ContentView's `showShelfInTab` or `showShelfInSidebar`.

#if os(iOS) || os(macOS)
import PunktfunkKit
import SwiftUI

#if os(iOS)
/// The touch UI's two destinations: the remote-desktop face and the gaming face.
enum TouchTab: Hashable {
    case hosts
    case library
}
#endif

struct LibraryTabView: View {
    @ObservedObject var store: HostStore
    let onLaunch: (LibraryTarget, String) -> Void
    let onConnectShelf: (LibraryTarget) -> Void
    /// Stream a saved host's desktop: the Desktops section.
    let onConnectHost: (StoredHost) -> Void
    let showHosts: () -> Void
    #if DEBUG
    /// Shot harness: a canned catalog in place of the fetch.
    var shotPhase: ShotLibraryPhase?
    #endif
    @ObservedObject private var presets = PresetStore.shared
    @AppStorage(DefaultsKey.libraryShelf) private var shelfID = ""
    @AppStorage(DefaultsKey.defaultHost) private var defaultHostID = ""

    /// Every shelf there is: each paired host, then each preset pinned to it.
    private var shelves: [LibraryTarget] {
        LibraryTarget.shelves(of: store.hosts, presets: presets)
    }

    /// The remembered shelf, else the default host's, else the first one.
    private var shelf: LibraryTarget? {
        let all = shelves
        if let remembered = all.first(where: { $0.id == shelfID }) { return remembered }
        if let host = StartScreen.defaultHost(id: defaultHostID, hosts: store.hosts).host,
           let fallback = all.first(where: { $0.id == LibraryTarget(host: host).id }) {
            return fallback
        }
        return all.first
    }

    var body: some View {
        NavigationStack {
            if let shelf {
                library(shelf)
                    .id(shelf.id)
                    .safeAreaInset(edge: .top, spacing: 0) {
                        let all = shelves
                        if all.count > 1 {
                            ShelfFilter(shelves: all, current: shelf.id) { shelfID = $0 }
                        }
                    }
            } else {
                LibraryNoHostView(showHosts: showHosts)
            }
        }
    }

    private func library(_ shelf: LibraryTarget) -> LibraryView {
        #if DEBUG
        LibraryView(
            store: store, target: shelf, onLaunch: { onLaunch(shelf, $0) },
            onConnect: { onConnectShelf(shelf) }, inTab: true, onConnectHost: onConnectHost,
            shotPhase: shotPhase)
        #else
        LibraryView(
            store: store, target: shelf, onLaunch: { onLaunch(shelf, $0) },
            onConnect: { onConnectShelf(shelf) }, inTab: true, onConnectHost: onConnectHost)
        #endif
    }
}

/// The Library's host filter: one chip per shelf, the current one filled. It took over from a title
/// menu that hid the choice.
private struct ShelfFilter: View {
    let shelves: [LibraryTarget]
    let current: String
    let pick: (String) -> Void
    @ObservedObject private var presets = PresetStore.shared

    var body: some View {
        ScrollView(.horizontal, showsIndicators: false) {
            HStack(spacing: 8) {
                ForEach(shelves) { shelf in
                    chip(shelf)
                }
            }
            .padding(.horizontal)
            .padding(.vertical, 8)
        }
    }

    private func chip(_ shelf: LibraryTarget) -> some View {
        let on = shelf.id == current
        return Button { pick(shelf.id) } label: {
            HStack(spacing: 6) {
                if let mark = osIconImage(for: shelf.host.osChain) {
                    mark.resizable().scaledToFit().frame(width: 14, height: 14)
                }
                Text(shelf.title(in: presets))
                    .lineLimit(1)
            }
            .font(.geist(13, .semibold, relativeTo: .subheadline))
            .foregroundStyle(on ? Color.white : Color.primary)
            .padding(.horizontal, 12)
            .padding(.vertical, 6)
            .background(Capsule().fill(on ? AnyShapeStyle(Color.brand) : AnyShapeStyle(.regularMaterial)))
            .overlay { if !on { Capsule().strokeBorder(.quaternary, lineWidth: 1) } }
        }
        .buttonStyle(.plain)
        .accessibilityAddTraits(on ? .isSelected : [])
    }
}
#endif
