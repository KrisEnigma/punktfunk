// The Mac's window (design/apple-touch-ui-overhaul.md §4): a source list with Hosts and Library,
// the chosen destination beside it, and ⌘1 / ⌘2 / ⌥⌘I in the View menu. The touch UI's tabs are
// this list's rows; the Library row is the Library tab, whose title menu picks the host.

import PunktfunkKit
import SwiftUI
#if os(macOS)

/// Where the Mac window points: the host grid or the library.
enum MacDestination: Hashable {
    case hosts
    case library
}

struct MacShellView<Hosts: View>: View {
    @ObservedObject var store: HostStore
    @Binding var selection: MacDestination
    let hosts: Hosts
    let onLaunch: (LibraryTarget, String) -> Void
    let onConnectShelf: (LibraryTarget) -> Void
    let onConnectHost: (StoredHost) -> Void

    var body: some View {
        NavigationSplitView {
            List(selection: rowSelection) {
                Label("Hosts", systemImage: "desktopcomputer")
                    .tag(MacDestination.hosts)
                Label("Library", systemImage: "square.grid.2x2")
                    .tag(MacDestination.library)
            }
            .navigationSplitViewColumnWidth(min: 160, ideal: 180, max: 240)
        } detail: {
            switch selection {
            case .hosts:
                hosts
            case .library:
                LibraryTabView(
                    store: store, onLaunch: onLaunch, onConnectShelf: onConnectShelf,
                    onConnectHost: onConnectHost, showHosts: { selection = .hosts })
            }
        }
        .focusedSceneValue(
            \.macNavigation,
            MacNavigation(showHosts: { selection = .hosts }, showLibrary: { selection = .library }))
    }

    /// A click on empty space deselects a List; the window always shows something.
    private var rowSelection: Binding<MacDestination?> {
        Binding(get: { selection }, set: { if let next = $0 { selection = next } })
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
