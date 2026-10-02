import PlaygroundCore
import SwiftUI
import UndraRuntime

/// A list the core owns and this screen pages through (`Lazy<Item>`, ADR-043). `books` has ten thousand rows and none of them is
/// ever sent whole: the store holds the length and the version, and a row is read with `books[index]`, which is `nil` while its
/// page loads (a placeholder row) and asks the core for that page and the ones around it. Every button is one call into the core
/// and comes back as one 12-byte entry (a new length and version): the rows on screen stay while their pages are asked for again.
/// `evens` is a read-only lazy view of the even rows of a small list the store does send, paged the same way.
struct LibraryScreen: View {
    let library: Library
    @State private var edits = 0
    @State private var problem: String?

    var body: some View {
        NavigationStack {
            List {
                Section {
                    ForEach(0 ..< library.books.count, id: \.self) { index in
                        BookRow(books: library.books, index: index, prefix: "library-row")
                    }
                } header: {
                    Text("\(library.books.count.formatted()) books")
                        .accessibilityIdentifier("library-count")
                }
                Section {
                    ForEach(0 ..< library.evens.count, id: \.self) { index in
                        BookRow(books: library.evens, index: index, prefix: "library-even")
                    }
                } header: {
                    Text("\(library.evens.count) even rows of the source")
                        .accessibilityIdentifier("library-evens-count")
                }
            }
            .accessibilityIdentifier("library-list")
            .safeAreaInset(edge: .bottom) { controls }
            .navigationTitle("Library")
            .navigationBarTitleDisplayMode(.inline)
        }
    }

    private var controls: some View {
        VStack(spacing: 10) {
            if let problem {
                Text(problem).font(.footnote).foregroundStyle(.red)
            }
            HStack {
                Button("Add row") { library.addRows(count: 1) }.accessibilityIdentifier("library-add")
                Button("Rename first") { rename() }.accessibilityIdentifier("library-rename")
                Button("Remove first") { removeFirst() }.accessibilityIdentifier("library-remove")
            }
            HStack {
                Button("Drop source row") { library.dropSource(count: 1) }.accessibilityIdentifier("library-drop-source")
                Button("Reset", role: .destructive) { library.reset(count: 10_000) }.accessibilityIdentifier("library-reset")
            }
        }
        .buttonStyle(.bordered)
        .padding()
        .background(.bar)
    }

    private func rename() {
        edits += 1
        run {
            try library.rename(index: 0, label: "Renamed \(edits)")
        }
    }

    private func removeFirst() {
        run {
            try library.removeAt(index: 0)
        }
    }

    /// Runs one call into the core. A position the core refuses is a `ListError`.
    private func run(_ action: () throws -> Void) {
        do {
            try action()
            problem = nil
        } catch {
            problem = error.localizedDescription
        }
    }
}

/// One row of a lazy list: the row once its page has arrived, a placeholder until then. Reading `books[index]` is what asks for the
/// page, and what makes this view update when the page arrives or the core changes the row.
private struct BookRow: View {
    let books: UndraLazyList<Item>
    let index: Int
    let prefix: String

    var body: some View {
        if let item = books[index] {
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
            .accessibilityElement(children: .combine)
            .accessibilityIdentifier("\(prefix)-\(index)")
        } else {
            HStack {
                Text("#0000")
                    .font(.caption.monospacedDigit())
                    .frame(width: 64, alignment: .leading)
                Text("Loading this page")
                Spacer()
            }
            .redacted(reason: .placeholder)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Loading row \(index)")
            .accessibilityIdentifier("\(prefix)-placeholder")
        }
    }
}
