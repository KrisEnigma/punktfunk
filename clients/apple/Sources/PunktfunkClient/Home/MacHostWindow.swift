// The Mac's host page (design/apple-touch-ui-overhaul.md §4): a window per host, opened from a
// card's ⓘ or Host Details…, with the page's sections in a sidebar. Streaming, browsing, waking
// and pairing belong to the main window, so the page hands those over and closes.

#if os(macOS)
import PunktfunkKit
import SwiftUI

/// An act the host window hands to the main window.
enum MacHostRequest: Equatable {
    case connect(StoredHost.ID, PresetSelection)
    case browse(StoredHost.ID)
    case wake(StoredHost.ID)
    case pair(StoredHost.ID)
}

/// Carries one request from a host window to a main window. The first main window to `take` it
/// acts on it; `mainWindows` says whether one is open to take it at all.
@MainActor
final class MacHostRouter: ObservableObject {
    static let shared = MacHostRouter()
    @Published private(set) var pending: MacHostRequest?
    var mainWindows = 0

    func send(_ request: MacHostRequest) { pending = request }

    func take() -> MacHostRequest? {
        defer { pending = nil }
        return pending
    }
}

struct MacHostWindow: View {
    static let sceneID = "host"
    let hostID: StoredHost.ID
    @ObservedObject var store: HostStore
    @ObservedObject private var presets = PresetStore.shared
    @ObservedObject private var hostPower = HostPowerStore.shared
    @Environment(\.openWindow) private var openWindow
    @Environment(\.dismiss) private var dismiss
    @State private var section: HostSection
    @State private var editTarget: StoredHost?
    @State private var confirmPower: PendingHostAction?
    /// The last send-logs or power outcome, for its alert.
    @State private var outcome: (title: String, message: String)?

    init(hostID: StoredHost.ID, store: HostStore, section: HostSection = .overview) {
        self.hostID = hostID
        self.store = store
        _section = State(initialValue: section)
    }

    var body: some View {
        NavigationSplitView {
            List(HostSection.allCases, selection: sectionSelection) { section in
                Label(section.title, systemImage: section.symbol)
            }
            .navigationSplitViewColumnWidth(min: 150, ideal: 170, max: 220)
        } detail: {
            Group {
                if section == .speedTest {
                    speedTestPane
                } else {
                    HostDetailView(store: store, hostID: hostID, actions: actions, only: section)
                }
            }
            .navigationSubtitle(section.title)
        }
        .sheet(item: $editTarget) { host in
            AddHostSheet(existing: host, onSave: { store.update($0) })
        }
        .alert(
            confirmPower.map { "\($0.action.label)?" } ?? "",
            isPresented: Binding(
                get: { confirmPower != nil }, set: { if !$0 { confirmPower = nil } })
        ) {
            Button("Cancel", role: .cancel) { confirmPower = nil }
            if let pending = confirmPower {
                Button(pending.action.label, role: .destructive) {
                    confirmPower = nil
                    run(pending.action, on: pending.host)
                }
            }
        } message: {
            Text(
                confirmPower.map {
                    "This ends every stream from \($0.host.displayName) and anything running "
                        + "on it. You'll need to wake or start it again."
                } ?? "")
        }
        .alert(
            outcome?.title ?? "",
            isPresented: Binding(get: { outcome != nil }, set: { if !$0 { outcome = nil } })
        ) {
            Button("OK", role: .cancel) {}
        } message: {
            Text(outcome?.message ?? "")
        }
        .onAppear {
            if let host = store.hosts.first(where: { $0.id == hostID }) { hostPower.refresh(host) }
        }
        // System type sizes, like the Settings window: the touch UI's 17 pt Geist makes a Mac
        // form's rows and footers oversized.
        .font(nil)
    }

    /// The speed test waits for Start here: picking the row is not asking for a burst.
    @ViewBuilder private var speedTestPane: some View {
        if let host = store.hosts.first(where: { $0.id == hostID }) {
            if host.pinnedSHA256 != nil {
                SpeedTestView(host: host, startsOnAppear: false)
                    .navigationTitle(host.displayName)
            } else {
                ContentUnavailableView(
                    "Pair First", systemImage: "lock",
                    description: Text("Pair with \(host.displayName) to test the network speed."))
            }
        }
    }

    /// A click on empty space deselects a List; the window always shows a section.
    private var sectionSelection: Binding<HostSection?> {
        Binding(get: { section }, set: { if let next = $0 { section = next } })
    }

    /// The grid's acts, run from here: sheets and prompts in this window, the rest in the main one.
    private func actions(for host: StoredHost) -> HostActions {
        HostActions(
            host: host, pinned: nil, online: store.probedOnline.contains(host.id), store: store,
            presets: presets.presets, power: hostPower.actions(for: host),
            surface: HostActionSurface(
                connect: { hand(.connect(host.id, $0)) },
                pair: { hand(.pair(host.id)) },
                edit: { editTarget = host },
                browse: { _ in hand(.browse(host.id)) },
                speedTest: { section = .speedTest },
                sendLogs: {
                    Task {
                        let sent = await SendLogs.toHost(host)
                        outcome = (sent.ok ? "Logs sent" : "Couldn't send logs", sent.message)
                    }
                },
                wake: { hand(.wake(host.id)) },
                showDetails: {},
                runPower: { power($0, on: host) }))
    }

    /// Hands `request` to a main window, opening one if none is open, and closes this one.
    private func hand(_ request: MacHostRequest) {
        let router = MacHostRouter.shared
        router.send(request)
        if router.mainWindows == 0 { openWindow(id: PunktfunkClientApp.mainSceneID) }
        dismiss()
    }

    /// Explain an unavailable action, confirm a destructive one, run the rest — as the grid does.
    private func power(_ action: HostAction, on host: StoredHost) {
        guard action.available else {
            outcome = (
                "Couldn't do that",
                action.unavailableReason ?? "\(action.label) isn't available right now")
            return
        }
        if action.danger {
            confirmPower = PendingHostAction(host: host, action: action)
        } else {
            run(action, on: host)
        }
    }

    private func run(_ action: HostAction, on host: StoredHost) {
        Task {
            let done = await hostPower.invoke(action, on: host)
            outcome = (done.ok ? "On its way" : "Couldn't do that", done.message)
        }
    }
}
#endif
