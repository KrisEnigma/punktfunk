// A saved host's page as sections beside a sidebar: the Mac's host window and the iPad's host
// sheet. Acts that belong to the grid's window (connect, browse, wake, pair) go back through
// `handOff`, which closes the page; edits, power, logs and the speed test stay on it.

#if os(iOS) || os(macOS)
import PunktfunkKit
import SwiftUI

/// An act the host page hands back to the window that shows the grid.
enum HostPageRequest: Equatable {
    case connect(StoredHost.ID, PresetSelection)
    case browse(StoredHost.ID)
    case wake(StoredHost.ID)
    case pair(StoredHost.ID)

    var hostID: StoredHost.ID {
        switch self {
        case .connect(let id, _), .browse(let id), .wake(let id), .pair(let id): id
        }
    }
}

struct HostSectionsView: View {
    let hostID: StoredHost.ID
    @ObservedObject var store: HostStore
    /// Runs an act that leaves the page, and closes the page.
    let handOff: (HostPageRequest) -> Void
    @ObservedObject private var presets = PresetStore.shared
    @ObservedObject private var hostPower = HostPowerStore.shared
    @Environment(\.dismiss) private var dismiss
    @State private var section: HostSection
    @State private var editTarget: StoredHost?
    @State private var confirmPower: PendingHostAction?
    /// The last send-logs or power outcome, for its alert.
    @State private var outcome: (title: String, message: String)?

    init(
        hostID: StoredHost.ID, store: HostStore, section: HostSection = .overview,
        handOff: @escaping (HostPageRequest) -> Void
    ) {
        self.hostID = hostID
        self.store = store
        self.handOff = handOff
        _section = State(initialValue: section)
    }

    var body: some View {
        NavigationSplitView {
            List(HostSection.allCases, selection: sectionSelection) { section in
                Label(section.title, systemImage: section.symbol)
            }
            #if os(macOS)
            .navigationSplitViewColumnWidth(min: 150, ideal: 170, max: 220)
            #else
            .navigationTitle(store.hosts.first { $0.id == hostID }?.displayName ?? "")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
            #endif
        } detail: {
            Group {
                if section == .speedTest {
                    speedTestPane
                } else {
                    HostDetailView(store: store, hostID: hostID, actions: actions, only: section)
                }
            }
            #if os(macOS)
            .navigationSubtitle(section.title)
            #endif
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
        #if os(macOS)
        // System type sizes, like the Settings window: the touch UI's 17 pt Geist makes a Mac
        // form's rows and footers oversized.
        .font(nil)
        #endif
    }

    /// The speed test waits for Start here: picking the row is not asking for a burst.
    @ViewBuilder private var speedTestPane: some View {
        if let host = store.hosts.first(where: { $0.id == hostID }) {
            if host.pinnedSHA256 != nil {
                SpeedTestView(host: host, startsOnAppear: false)
                    #if os(macOS)
                    .navigationTitle(host.displayName)
                    #else
                    .navigationTitle(HostSection.speedTest.title)
                    #endif
            } else {
                ContentUnavailableView(
                    "Pair First", systemImage: "lock",
                    description: Text("Pair with \(host.displayName) to test the network speed."))
            }
        }
    }

    /// A click on empty space deselects a List; the page always shows a section.
    private var sectionSelection: Binding<HostSection?> {
        Binding(get: { section }, set: { if let next = $0 { section = next } })
    }

    /// The grid's acts, run from here: sheets and prompts on this page, the rest handed back.
    private func actions(for host: StoredHost) -> HostActions {
        HostActions(
            host: host, pinned: nil, online: store.probedOnline.contains(host.id), store: store,
            presets: presets.presets, power: hostPower.actions(for: host),
            surface: HostActionSurface(
                connect: { handOff(.connect(host.id, $0)) },
                pair: { handOff(.pair(host.id)) },
                edit: { editTarget = host },
                browse: { _ in handOff(.browse(host.id)) },
                speedTest: { section = .speedTest },
                sendLogs: {
                    Task {
                        let sent = await SendLogs.toHost(host)
                        outcome = (sent.ok ? "Logs sent" : "Couldn't send logs", sent.message)
                    }
                },
                wake: { handOff(.wake(host.id)) },
                showDetails: {},
                runPower: { power($0, on: host) }))
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
