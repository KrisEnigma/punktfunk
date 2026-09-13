// The Library's Customize panel (design/apple-touch-ui-overhaul.md §2.5): every section with a
// switch, in the order the tab draws them, dragged to reorder. It writes
// `punktfunk.librarySections`, which the tab reads; a switched-off section stays listed here.
// A sheet on the iPhone, a plain popover on the Mac, where each row carries a drag grip.

import PunktfunkKit
import SwiftUI
#if os(iOS) || os(macOS)

struct LibrarySectionsPanel: View {
    @AppStorage(DefaultsKey.librarySections) private var storedLayout = ""
    #if DEBUG
    /// Shot harness: a layout that never touches the device's own.
    var shotLayout: String?
    #endif
    @Environment(\.dismiss) private var dismiss
    private let note = "Drag to reorder. A section with nothing to show stays hidden until it has "
        + "something."

    private var layout: LibrarySectionLayout {
        #if DEBUG
        if let shotLayout { return LibrarySectionLayout(stored: shotLayout) }
        #endif
        return LibrarySectionLayout(stored: storedLayout)
    }

    var body: some View {
        #if os(macOS)
        VStack(alignment: .leading, spacing: 10) {
            Text("Customize Library")
                .font(.headline)
            // A Mac list paints its own fill and insets its rows. Without both gone the rows sat in
            // a dark slab, indented past the popover's own padding.
            List { rows }
                .listStyle(.plain)
                .scrollContentBackground(.hidden)
                .contentMargins(0, for: .scrollContent)
                .padding(.horizontal, -8) // the plain list's own row indent, so rows meet the title
            Text(note)
                .font(.caption)
                .foregroundStyle(.secondary)
                .fixedSize(horizontal: false, vertical: true)
            restoreButton
        }
        .padding(16)
        #else
        NavigationStack {
            List {
                Section {
                    rows
                } footer: {
                    Text(note)
                }
                Section { restoreButton }
            }
            .environment(\.editMode, .constant(.active))
            .navigationBarTitleDisplayMode(.inline)
            .navigationTitle("Customize Library")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
        #endif
    }

    private var rows: some View {
        ForEach(layout.entries) { entry in
            HStack {
                Toggle(isOn: isOn(entry.section)) {
                    Label {
                        Text(entry.section.label)
                    } icon: {
                        Image(systemName: entry.section.symbol)
                            #if os(macOS)
                            .frame(width: 18) // one icon width, so the names line up
                            #endif
                    }
                }
                #if os(macOS)
                Spacer()
                // A Mac list draws no handle of its own; the whole row drags.
                Image(systemName: "line.3.horizontal")
                    .foregroundStyle(.tertiary)
                    .help("Drag to reorder")
                #endif
            }
            #if os(macOS)
            .listRowInsets(EdgeInsets(top: 3, leading: 0, bottom: 3, trailing: 0))
            #endif
        }
        .onMove(perform: move)
    }

    private var restoreButton: some View {
        Button("Restore Default Order") { storedLayout = "" }
            .disabled(storedLayout.isEmpty)
    }

    private func isOn(_ section: LibrarySection) -> Binding<Bool> {
        Binding(
            get: { layout.entries.first { $0.section == section }?.isOn ?? true },
            set: { on in
                var next = layout
                if let at = next.entries.firstIndex(where: { $0.section == section }) {
                    next.entries[at].isOn = on
                }
                storedLayout = next.stored
            })
    }

    private func move(from source: IndexSet, to destination: Int) {
        var next = layout
        next.entries.move(fromOffsets: source, toOffset: destination)
        storedLayout = next.stored
    }
}
#endif
