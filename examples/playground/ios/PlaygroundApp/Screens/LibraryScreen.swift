import PlaygroundCore
import SwiftUI
import UndraRuntime

/// A list the core owns and this screen pages through (`Lazy<T>`, ADR-043). `books` has ten thousand rows and none of them is
/// ever sent whole: the store holds the length and the version, and a row is read with `books[index]`, which is `nil` while its
/// page loads (a placeholder row) and asks the core for that page and the ones around it. Every button is one call into the core
/// and comes back as one 12-byte entry (a new length and version): the rows on screen stay while their pages are asked for again.
/// `evens` is a read-only lazy view of the even rows of a small list the store does send, paged the same way.
///
/// The rows live in a `LazyVStack`, as in the 10k list: it builds only the rows near the screen, so only their pages are read. A
/// SwiftUI `List` builds the rows of a `ForEach` up front (every one of the ten thousand, on iOS 26), which would read every page;
/// use a lazy stack for a list that is too big to hold.
struct LibraryScreen: View {
    let library: Library
    /// Which list the screen shows: the books, or the lazy view of the even rows of the source.
    @State private var evens = false
    @State private var edits = 0
    @State private var problem: String?

    var body: some View {
        NavigationStack {
            ScrollView {
                LazyVStack(spacing: 0, pinnedViews: .sectionHeaders) {
                    Section {
                        if evens {
                            ForEach(0 ..< library.evens.count, id: \.self) { index in
                                BookRow(books: library.evens, index: index, prefix: "library-even")
                                Divider()
                            }
                        } else {
                            ForEach(0 ..< library.books.count, id: \.self) { index in
                                BookRow(books: library.books, index: index, prefix: "library-row")
                                Divider()
                            }
                        }
                    } header: {
                        VStack(spacing: 6) {
                            Picker("List", selection: $evens) {
                                Text("Books").tag(false)
                                Text("Even rows of the source").tag(true)
                            }
                            .pickerStyle(.segmented)
                            .accessibilityIdentifier("library-filter")
                            if evens {
                                header("\(library.evens.count) even rows of the source", id: "library-evens-count")
                            } else {
                                header("\(library.books.count.formatted()) books", id: "library-count")
                            }
                        }
                        .padding(.horizontal)
                        .padding(.vertical, 6)
                        .background(.bar)
                    }
                }
            }
            .safeAreaInset(edge: .bottom) { controls }
            .navigationTitle("Library")
            .navigationBarTitleDisplayMode(.inline)
        }
    }

    private func header(_ text: String, id: String) -> some View {
        Text(text)
            .font(.subheadline.weight(.semibold))
            .foregroundStyle(.secondary)
            .frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityIdentifier(id)
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
            .padding(.horizontal)
            .padding(.vertical, 8)
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
            .padding(.horizontal)
            .padding(.vertical, 8)
            .redacted(reason: .placeholder)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("Loading row \(index)")
            .accessibilityIdentifier("\(prefix)-placeholder")
        }
    }
}
