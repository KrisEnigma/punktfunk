// The Mac's window (design/apple-touch-ui-overhaul.md §4): a source list with Hosts and one row
// per library shelf, the chosen destination beside it, and ⌘1 / ⌘2 / ⌥⌘I in the View menu. The
// touch UI's tabs are this list's rows; the host page is HomeView's inspector.

import PunktfunkKit
import SwiftUI
#if os(macOS)

/// Where the Mac window points: the host grid, or one shelf's library.
enum MacDestination: Hashable {
    case hosts
    case shelf(LibraryTarget)
}

struct MacShellView<Hosts: View>: View {
    @ObservedObject var store: HostStore
    @Binding var selection: MacDestination
    let hosts: Hosts
    let onLaunch: (LibraryTarget, String) -> Void
    let onConnectShelf: (LibraryTarget) -> Void
    let onConnectHost: (StoredHost) -> Void
    @ObservedObject private var presets = PresetStore.shared
    @AppStorage(DefaultsKey.libraryShelf) private var shelfID = ""
    @AppStorage(DefaultsKey.defaultHost) private var defaultHostID = ""

    var body: some View {
        NavigationSplitView {
            List(selection: rowSelection) {
                Label("Hosts", systemImage: "desktopcomputer")
                    .tag(MacDestination.hosts)
                Section("Library") {
                    ForEach(shelves) { shelf in
                        Label(
                            shelf.title(in: presets),
                            systemImage: shelf.pinnedPresetID == nil
                                ? "square.grid.2x2" : "slider.horizontal.3")
                            .tag(MacDestination.shelf(shelf))
                    }
                }
            }
            .navigationSplitViewColumnWidth(min: 180, ideal: 210, max: 280)
        } detail: {
            switch selection {
            case .hosts:
                hosts
            case .shelf(let shelf):
                NavigationStack {
                    LibraryView(
                        store: store, target: shelf, onLaunch: { onLaunch(shelf, $0) },
                        onConnect: { onConnectShelf(shelf) }, inTab: true,
                        onConnectHost: onConnectHost)
                }
                .id(shelf.id)
            }
        }
        .onChange(of: selection) { _, destination in
            if case .shelf(let shelf) = destination { shelfID = shelf.id }
        }
        .focusedSceneValue(
            \.macNavigation, MacNavigation(showHosts: { selection = .hosts }, showLibrary: showLibrary))
    }

    private var shelves: [LibraryTarget] {
        LibraryTarget.shelves(of: store.hosts, presets: presets)
    }

    /// A click on empty space deselects a List; the window always shows something.
    private var rowSelection: Binding<MacDestination?> {
        Binding(get: { selection }, set: { if let next = $0 { selection = next } })
    }

    /// ⌘2: the remembered shelf, else the default host's, else the first.
    private func showLibrary() {
        let all = shelves
        let byDefault = StartScreen.defaultHost(id: defaultHostID, hosts: store.hosts).host
            .flatMap { host in all.first { $0.id == LibraryTarget(host: host).id } }
        if let shelf = all.first(where: { $0.id == shelfID }) ?? byDefault ?? all.first {
            selection = .shelf(shelf)
        }
    }
}

/// The window's navigation acts, published for the View menu.
struct MacNavigation {
    var showHosts: () -> Void
    var showLibrary: () -> Void
}

private struct MacNavigationKey: FocusedValueKey {
    typealias Value = MacNavigation
}

private struct HostPageToggleKey: FocusedValueKey {
    typealias Value = () -> Void
}

extension FocusedValues {
    var macNavigation: MacNavigation? {
        get { self[MacNavigationKey.self] }
        set { self[MacNavigationKey.self] = newValue }
    }

    /// Closes the host inspector, or opens it on the default host (`HomeView`).
    var hostPageToggle: (() -> Void)? {
        get { self[HostPageToggleKey.self] }
        set { self[HostPageToggleKey.self] = newValue }
    }
}

/// ⌘1 / ⌘2 / ⌥⌘I, above the sidebar toggle in the View menu.
struct MacNavigationCommands: Commands {
    @FocusedValue(\.macNavigation) private var navigation
    @FocusedValue(\.hostPageToggle) private var hostPage

    var body: some Commands {
        CommandGroup(before: .sidebar) {
            Button("Hosts") { navigation?.showHosts() }
                .keyboardShortcut("1", modifiers: .command)
                .disabled(navigation == nil)
            Button("Library") { navigation?.showLibrary() }
                .keyboardShortcut("2", modifiers: .command)
                .disabled(navigation == nil)
            Button("Host Page") { hostPage?() }
                .keyboardShortcut("i", modifiers: [.option, .command])
                .disabled(hostPage == nil)
            Divider()
        }
    }
}
#endif
