import PlaygroundCore
import SwiftUI

/// Ten thousand rows, keyed by `id`. Every button is one call into the core and comes back as a
/// keyed patch of one operation (an insert, an update, a move, a removal), which the generated store
/// applies to its array: SwiftUI sees a one-row change, not a new list. "Stream updates" makes ten
/// such calls a second, on rows that are on screen.
struct BigListScreen: View {
    let list: BigList
    /// The id of the row at the top of the scroll view; the buttons act on the rows around it.
    @State private var topID: UInt32?
    @State private var streaming = false
    /// How many labels the buttons have made, so that every label is new.
    @State private var edits = 0
    @State private var problem: String?

    var body: some View {
        NavigationStack {
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(list.items, id: \.id) { item in
                        ItemRow(item: item)
                        Divider()
                    }
                }
                .scrollTargetLayout()
            }
            .scrollPosition(id: $topID, anchor: .top)
            .safeAreaInset(edge: .bottom) { controls }
            .navigationTitle("10k list")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) {
                    Text("\(list.count.formatted()) rows")
                        .font(.headline)
                        .accessibilityIdentifier("biglist-count")
                }
            }
            .task(id: streaming) {
                guard streaming else {
                    return
                }
                while !Task.isCancelled {
                    try? await Task.sleep(for: .milliseconds(100))
                    update(at: anchor + Int.random(in: 0 ..< Self.visibleRows))
                }
            }
        }
    }

    /// About how many rows fit on screen.
    private static let visibleRows = 12

    private var controls: some View {
        VStack(spacing: 10) {
            if let problem {
                Text(problem).font(.footnote).foregroundStyle(.red)
            }
            HStack {
                Button("Insert top") { insert(at: 0) }.accessibilityIdentifier("biglist-insert-top")
                Button("Insert middle") { insert(at: Int(list.count) / 2) }.accessibilityIdentifier("biglist-insert-middle")
                Button("Update") { update(at: anchor) }.accessibilityIdentifier("biglist-update")
            }
            HStack {
                Button("Move") { move() }.accessibilityIdentifier("biglist-move")
                Button("Remove") { remove() }.accessibilityIdentifier("biglist-remove")
                Button("Reset", role: .destructive) { list.reset() }.accessibilityIdentifier("biglist-reset")
            }
            Toggle("Stream updates", isOn: $streaming)
                .accessibilityIdentifier("biglist-stream")
        }
        .buttonStyle(.bordered)
        .padding()
        .background(.bar)
    }

    // MARK: Actions

    /// The position of the row at the top of the screen.
    private var anchor: Int {
        guard let topID, let index = list.items.firstIndex(where: { $0.id == topID }) else {
            return 0
        }
        return index
    }

    private func insert(at index: Int) {
        edits += 1
        run { () throws(ListError) in
            _ = try list.insertAt(index: UInt32(index), label: "Inserted \(edits)")
        }
    }

    private func update(at index: Int) {
        edits += 1
        run { () throws(ListError) in
            try list.updateAt(index: UInt32(index), label: "Updated \(edits)")
        }
    }

    /// Moves the top row five places down.
    private func move() {
        run { () throws(ListError) in
            try list.moveItem(from: UInt32(anchor), to: UInt32(anchor + 5))
        }
    }

    private func remove() {
        run { () throws(ListError) in
            try list.removeAt(index: UInt32(anchor))
        }
    }

    /// Runs one call into the core; a position the core refuses is a typed `ListError`.
    private func run(_ action: () throws(ListError) -> Void) {
        do {
            try action()
            problem = nil
        } catch {
            problem = error.localizedDescription
        }
    }
}

/// One row: its identity, its label and how many times it was updated.
private struct ItemRow: View {
    let item: Item

    var body: some View {
        HStack {
            Text("#\(item.id)")
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
                .frame(width: 64, alignment: .leading)
            Text(item.label)
            Spacer()
            Text("v\(item.version)")
                .font(.caption.monospacedDigit())
                .foregroundStyle(.secondary)
        }
        .padding(.horizontal)
        .padding(.vertical, 8)
        .accessibilityElement(children: .combine)
        .accessibilityIdentifier("biglist-row-\(item.id)")
    }
}
