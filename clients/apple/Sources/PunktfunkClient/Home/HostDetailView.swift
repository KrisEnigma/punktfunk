// The host page (design/apple-touch-ui-overhaul.md §2.4): everything about one saved host that
// is not "connect to it", with the acts its card's menu offers. An iPhone pushes it as one form;
// the Mac, the iPad and the TV show one section at a time beside a sidebar (HostSectionsView). It
// reads the live record by id, so an edit shows at once and a removal closes it.

import PunktfunkKit
import SwiftUI

/// The host page's parts: one form on touch, where the speed test is a page of its own, and one
/// sidebar row each in the Mac's host window.
enum HostSection: String, CaseIterable, Identifiable {
    case overview, presets, connection, speedTest, pairing, power

    var id: Self { self }

    var title: String {
        switch self {
        case .overview: "Overview"
        case .presets: "Presets"
        case .connection: "Connection"
        case .speedTest: "Speed Test"
        case .pairing: "Pairing"
        case .power: "Power"
        }
    }

    var symbol: String {
        switch self {
        case .overview: "desktopcomputer"
        case .presets: "slider.horizontal.3"
        case .connection: "network"
        case .speedTest: "gauge.with.needle"
        case .pairing: "lock"
        case .power: "power"
        }
    }
}

struct HostDetailView: View {
    @ObservedObject var store: HostStore
    let hostID: StoredHost.ID
    /// The surface's per-host builder (`HostActions(host:…)`).
    let actions: (StoredHost) -> HostActions
    /// One section alone (the Mac's host window), or nil for every section in one form.
    var only: HostSection?
    @ObservedObject private var nowPlaying = NowPlayingStore.shared
    @AppStorage(DefaultsKey.defaultHost) private var defaultHostID = ""
    @AppStorage(DefaultsKey.autoWake) private var autoWake = true
    @Environment(\.dismiss) private var dismiss
    @State private var confirmForget = false
    @State private var confirmRemove = false

    var body: some View {
        if let host = store.hosts.first(where: { $0.id == hostID }) {
            page(host, actions(host))
        } else {
            // Removed while open, from here or from anywhere else.
            Color.clear.onAppear { dismiss() }
        }
    }

    private func page(_ host: StoredHost, _ a: HostActions) -> some View {
        let online = store.probedOnline.contains(host.id)
        let playing = online ? nowPlaying.title(for: host) : nil
        let status = HostStatus(
            host: host, isOnline: online, isConnecting: false, nowPlaying: playing,
            autoWake: autoWake)
        return Form {
            if shows(.overview) {
                Section {
                    header(host, status)
                    Button(action: a.connect) {
                        Label(playing.map { "Resume \($0)" } ?? "Connect", systemImage: "play.fill")
                    }
                    if let browse = a.browseLibrary {
                        Button(action: browse) {
                            Label("Browse Library", systemImage: "square.grid.2x2")
                        }
                    }
                }
            }
            if shows(.presets) { presetsSection(host, a) }
            if shows(.connection) { connectionSection(host, a) }
            if shows(.pairing) { pairingSection(host, a) }
            if shows(.power) { powerSection(a) }
            if shows(.overview) {
                if let sendLogs = a.sendLogs {
                    Section {
                        Button("Send Logs to Host", systemImage: "doc.text", action: sendLogs)
                    } footer: {
                        Text("Uploads this device's recent log to the host, for a bug report.")
                    }
                }
                Section {
                    // A Mac form draws even a destructive button in the window's tint.
                Button("Remove Host", role: .destructive) { confirmRemove = true }
                    .tint(.red)
                    // On the button, so iOS 26 opens the dialog from the row that asked.
                    .confirmationDialog(
                        "Remove \(host.displayName)?", isPresented: $confirmRemove,
                        titleVisibility: .visible
                    ) {
                        Button("Remove Host", role: .destructive, action: a.remove)
                    } message: {
                        Text("You can add it again later.")
                    }
                } footer: {
                    Text("Deletes the host from this device. The host itself is untouched.")
                }
            }
        }
        #if os(macOS)
        .formStyle(.grouped)
        #endif
        // A TV shows this only inside its page of sections, which names the host over its sidebar.
        #if !os(tvOS)
        .navigationTitle(pageTitle(host))
        #endif
        #if os(iOS)
        .navigationBarTitleDisplayMode(.inline)
        #endif
    }

    private func shows(_ section: HostSection) -> Bool { only == nil || only == section }

    #if !os(tvOS)
    /// The host's name, except in the iPad's sheet of sections: its sidebar names the host, so the
    /// pane names its section.
    private func pageTitle(_ host: StoredHost) -> String {
        #if os(iOS)
        if let only { return only.title }
        #endif
        return host.displayName
    }
    #endif

    private func header(_ host: StoredHost, _ status: HostStatus) -> some View {
        let m = CardMetrics.current
        return HStack(spacing: m.spacing) {
            monogramTile(
                monogram(host.displayName), osChain: host.osChain, m: m, connecting: false,
                filled: host.pinnedSHA256 != nil)
            VStack(alignment: .leading, spacing: 4) {
                Text(host.displayName)
                    .font(.geist(m.name + 2, .bold, relativeTo: .title3))
                    .lineLimit(2)
                HostStatusLine(status: status, size: m.meta)
            }
        }
        .padding(.vertical, 4)
    }

    @ViewBuilder private func presetsSection(_ host: StoredHost, _ a: HostActions) -> some View {
        if let menu = a.presets, !menu.presets.isEmpty {
            Section {
                // A binding to a deleted preset reads as Default settings, as a connect does.
                let boundID = Binding(
                    get: { menu.presets.contains { $0.id == menu.boundID } ? menu.boundID ?? "" : "" },
                    set: { menu.setDefault($0.isEmpty ? nil : $0) })
                #if os(tvOS)
                TVSelectionRow(
                    title: "Connect with",
                    options: [(label: "Default settings", tag: "")]
                        + menu.presets.map { (label: $0.name, tag: $0.id) },
                    selection: boundID)
                #else
                Picker("Connect with", selection: boundID) {
                    Text("Default settings").tag("")
                    ForEach(menu.presets) { preset in
                        Text(preset.name).tag(preset.id)
                    }
                }
                #endif
                ForEach(menu.presets) { preset in
                    Toggle("Pin \u{201C}\(preset.name)\u{201D} as a card", isOn: Binding(
                        get: { menu.pinnedIDs.contains(preset.id) },
                        set: { _ in menu.togglePin(preset.id) }))
                }
            } header: {
                Text("Presets")
            } footer: {
                Text("A tap on the card connects with the chosen preset. A pinned preset gets its "
                    + "own card next to this host.")
            }
        } else if only == .presets {
            Section {
                Text("No presets yet. Make one in Settings, then choose it here.")
                    .foregroundStyle(.secondary)
            }
        }
    }

    private func connectionSection(_ host: StoredHost, _ a: HostActions) -> some View {
        Section {
            LabeledContent("Address", value: "\(host.address):\(String(host.port))")
            LabeledContent("Management port", value: String(host.effectiveMgmtPort))
            LabeledContent(
                "Wake-on-LAN",
                value: host.wakeMacs.isEmpty ? "Not learned yet" : host.wakeMacs.joined(separator: ", "))
            #if !os(tvOS)
            LabeledContent("Clipboard", value: host.clipboardSync == true ? "Shared" : "Off")
            #endif
            Button("Edit…", systemImage: "pencil", action: a.edit)
            if let speedTest = a.speedTest {
                Button("Test Network Speed…", systemImage: "speedometer", action: speedTest)
            }
            if let copyLink = a.copyLink {
                Button("Copy Link", systemImage: "link", action: copyLink)
            }
        } header: {
            Text("Connection")
        } footer: {
            if host.pinnedSHA256 == nil {
                Text("Pair first to test the network speed.")
            }
        }
    }

    private func pairingSection(_ host: StoredHost, _ a: HostActions) -> some View {
        let paired = host.pinnedSHA256 != nil
        return Section {
            LabeledContent("Status", value: paired ? "Paired" : "Not paired")
            if let pin = host.pinnedSHA256 {
                let hex = pin.hexLower
                LabeledContent("Fingerprint", value: "\(hex.prefix(8))…\(hex.suffix(4))")
            }
            Button(paired ? "Pair Again with PIN…" : "Pair with PIN…", systemImage: "number",
                   action: a.pair)
            if paired {
                Toggle("Default host", isOn: Binding(
                    get: { defaultHostID.lowercased() == host.id.uuidString.lowercased() },
                    set: { defaultHostID = $0 ? host.id.uuidString : "" }))
                Button("Forget Identity…", role: .destructive) { confirmForget = true }
                    .tint(.red)
                    .confirmationDialog(
                        "Forget the identity of \(host.displayName)?", isPresented: $confirmForget,
                        titleVisibility: .visible
                    ) {
                        Button("Forget Identity", role: .destructive, action: a.forget)
                    } message: {
                        Text("The next connect asks for a PIN again.")
                    }
            }
        } header: {
            Text("Pairing")
        } footer: {
            Text(paired
                ? "Start in opens the default host's library. With one paired host, that host is "
                    + "the default without this switch."
                : "Pairing lets this device browse the library, test the network and send logs.")
        }
    }

    @ViewBuilder private func powerSection(_ a: HostActions) -> some View {
        if a.wake != nil || !a.power.isEmpty {
            Section {
                if let wake = a.wake {
                    Button("Wake Host", systemImage: "power", action: wake)
                }
                ForEach(a.power) { action in
                    Button(
                        action.available ? action.label : "\(action.label) (Unavailable)",
                        systemImage: "power", role: action.danger ? .destructive : nil
                    ) { a.runPower(action) }
                }
            } header: {
                Text("Power")
            } footer: {
                Text("Restart and shut down end every stream from this host, and ask first.")
            }
        } else if only == .power {
            Section {
                Text("This host offers this device no power actions.")
                    .foregroundStyle(.secondary)
            }
        }
    }
}
