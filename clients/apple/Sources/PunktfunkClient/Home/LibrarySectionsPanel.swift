// The Library tab's Customize sheet (design/apple-touch-ui-overhaul.md §2.5): every section with
// a switch, in the order the tab draws them, dragged to reorder. It writes
// `punktfunk.librarySections`, which the tab reads; a switched-off section stays listed here.

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

    private var layout: LibrarySectionLayout {
        #if DEBUG
        if let shotLayout { return LibrarySectionLayout(stored: shotLayout) }
        #endif
        return LibrarySectionLayout(stored: storedLayout)
    }

    var body: some View {
        NavigationStack {
            List {
                Section {
                    ForEach(layout.entries) { entry in
                        Toggle(isOn: isOn(entry.section)) {
                            Label(entry.section.label, systemImage: entry.section.symbol)
                        }
                    }
                    .onMove(perform: move)
                } footer: {
                    Text("Drag to reorder. A section with nothing to show stays hidden until it has "
                        + "something.")
                }
                Section {
                    Button("Restore Default Order") { storedLayout = "" }
                        .disabled(storedLayout.isEmpty)
                }
            }
            #if os(iOS)
            .environment(\.editMode, .constant(.active))
            .navigationBarTitleDisplayMode(.inline)
            #endif
            .navigationTitle("Customize Library")
            .toolbar {
                ToolbarItem(placement: .confirmationAction) {
                    Button("Done") { dismiss() }
                }
            }
        }
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
