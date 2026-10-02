import SwiftUI

/// The strip at the top of the to-do and the note list: how many rows are ticked, the newest row and the
/// commands of the selection store. `TodoSelection` and `NoteSelection` are one generic `Selection<T>` of the
/// core (ADR-058), instantiated twice, so the two screens show the same strip over two different stores.
struct SelectionStrip: View {
    /// `"todo"` or `"note"`: the prefix of the accessibility identifiers.
    let kind: String
    let count: UInt32
    let latest: String?
    let picked: [String]
    let selectAll: () -> Void
    let clear: () -> Void
    let newDraft: () -> Void
    /// An extra command of the screen (the to-do list removes what is ticked), or nil.
    var extra: (title: String, action: () -> Void)?

    var body: some View {
        Section {
            HStack {
                Text("\(count) selected")
                    .font(.subheadline.weight(.medium))
                    .accessibilityIdentifier("\(kind)-selected-count")
                Spacer()
                Text(latest.map { "Latest: \($0)" } ?? "Nothing yet")
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("\(kind)-latest")
            }
            HStack {
                Button("Select all", action: selectAll)
                    .accessibilityIdentifier("\(kind)-select-all")
                Button("Clear", action: clear)
                    .disabled(count == 0)
                    .accessibilityIdentifier("\(kind)-select-clear")
                if let extra {
                    Button(extra.title, role: .destructive, action: extra.action)
                        .disabled(count == 0)
                        .accessibilityIdentifier("\(kind)-remove-selected")
                }
                Button("New draft", action: newDraft)
                    .accessibilityIdentifier("\(kind)-new-draft")
            }
            .buttonStyle(.borderless)
            if !picked.isEmpty {
                Text(picked.joined(separator: ", "))
                    .font(.footnote)
                    .foregroundStyle(.secondary)
                    .accessibilityIdentifier("\(kind)-picked")
            }
        }
    }
}
