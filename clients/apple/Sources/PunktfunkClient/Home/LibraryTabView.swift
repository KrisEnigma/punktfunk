// The Library tab (design/apple-touch-ui-overhaul.md §2.5): one shelf, picked from the title
// menu and remembered across launches. `LibraryView` still owns the fetch, the cache, the wake
// and the art; this view picks which shelf it shows, and what the tab says with no paired host.
// A write to `libraryTarget` lands here through ContentView's `showShelfInTab`.

#if os(iOS)
import PunktfunkKit
import SwiftUI

/// The touch UI's two destinations: the remote-desktop face and the gaming face.
enum TouchTab: Hashable {
    case hosts
    case library
}

struct LibraryTabView: View {
    @ObservedObject var store: HostStore
    let onLaunch: (LibraryTarget, String) -> Void
    let onConnectShelf: (LibraryTarget) -> Void
    /// Stream a saved host's desktop: the Desktops section.
    let onConnectHost: (StoredHost) -> Void
    let showHosts: () -> Void
    @ObservedObject private var presets = PresetStore.shared
    @AppStorage(DefaultsKey.libraryShelf) private var shelfID = ""
    @AppStorage(DefaultsKey.defaultHost) private var defaultHostID = ""

    /// Every shelf there is: each paired host, then each preset pinned to it.
    private var shelves: [LibraryTarget] {
        store.hosts.filter { $0.pinnedSHA256 != nil }.flatMap { host in
            [LibraryTarget(host: host)]
                + presets.catalog.pinned(for: host).map {
                    LibraryTarget(host: host, profile: .preset($0.id))
                }
        }
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
                LibraryView(
                    store: store, target: shelf, onLaunch: { onLaunch(shelf, $0) },
                    onConnect: { onConnectShelf(shelf) }, inTab: true,
                    onConnectHost: onConnectHost)
                    .id(shelf.id)
                    .toolbarTitleMenu { shelfPicker(current: shelf) }
            } else {
                ContentUnavailableView {
                    Label("No Library Yet", systemImage: "square.grid.2x2")
                } description: {
                    Text("Pair a host to browse its games here.")
                } actions: {
                    Button("Show Hosts", action: showHosts)
                }
            }
        }
    }

    @ViewBuilder private func shelfPicker(current: LibraryTarget) -> some View {
        let all = shelves
        if all.count > 1 {
            Picker("Shelf", selection: Binding(get: { current.id }, set: { shelfID = $0 })) {
                ForEach(all) { shelf in
                    Text(shelf.title(in: presets)).tag(shelf.id)
                }
            }
        }
    }
}
#endif
