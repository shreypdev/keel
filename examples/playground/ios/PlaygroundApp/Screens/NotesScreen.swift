import PlaygroundCore
import SwiftUI

/// Notes kept in SQLite through the opt-in `Db` port (ADR-048). The list is the core's `Notes`
/// store; the database file is the platform's (`SQLiteDbAdapter`, in Application Support), so the
/// notes are still there after a relaunch. Every change is written to the database first and then
/// shows in `notes.notes`, which the view only reads.
///
/// The strip under the field is the generic code of the core (ADR-058): the ticks live in a
/// `NoteSelection` (the `Selection<T>` the to-do screen instantiates for to-dos), the "Latest" line is
/// `newest(rows:)` and "New draft" is `draft(Note.self, title:)`.
struct NotesScreen: View {
    let notes: Notes
    let selection: NoteSelection
    @State private var draft = ""
    @State private var problem: String?
    @State private var latest: Note?

    /// The database the screen opens.
    static let database = "playground"

    var body: some View {
        NavigationStack {
            List {
                Section {
                    HStack {
                        TextField("New note", text: $draft)
                            .submitLabel(.done)
                            .onSubmit(add)
                            .accessibilityIdentifier("note-input")
                        Button("Add", action: add)
                            .disabled(title.isEmpty)
                            .accessibilityIdentifier("note-add")
                    }
                    if let problem {
                        Text(problem)
                            .font(.footnote)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("note-problem")
                    }
                }
                SelectionStrip(
                    kind: "note",
                    count: selection.count,
                    latest: latest?.title,
                    picked: selection.rows.map(\.title),
                    selectAll: { selection.selectAll(rows: notes.notes) },
                    clear: { selection.clear() },
                    newDraft: newDraft
                )
                Section {
                    ForEach(notes.notes, id: \.id) { note in
                        HStack {
                            Button {
                                run { try await notes.toggle(id: note.id) }
                            } label: {
                                Label(note.title, systemImage: note.done ? "checkmark.circle.fill" : "circle")
                                    .foregroundStyle(note.done ? .secondary : .primary)
                            }
                            Spacer()
                            Button {
                                selection.toggle(note)
                            } label: {
                                Image(systemName: isSelected(note) ? "checkmark.square.fill" : "square")
                            }
                            .buttonStyle(.borderless)
                            .accessibilityLabel("Select \(note.title)")
                            .accessibilityIdentifier("note-select")
                        }
                        .swipeActions {
                            Button(role: .destructive) {
                                run { try await notes.remove(id: note.id) }
                            } label: {
                                Label("Delete", systemImage: "trash")
                            }
                        }
                        .accessibilityIdentifier("note-row")
                    }
                } footer: {
                    Text(notes.version == 0 ? "Opening the database…" : "SQLite database \"\(NotesScreen.database)\", schema version \(notes.version)")
                        .accessibilityIdentifier("notes-version")
                }
            }
            .onChange(of: notes.notes, initial: true) {
                latest = try? newest(rows: notes.notes)
            }
            .navigationTitle("Notes")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) {
                    Text("\(notes.notes.count) notes")
                        .font(.headline)
                        .accessibilityIdentifier("notes-count")
                }
            }
            .task {
                // Opening migrates the database and loads every note; once open, the store keeps it.
                if notes.version == 0 {
                    run { _ = try await notes.open(name: NotesScreen.database) }
                }
            }
        }
    }

    private func isSelected(_ note: Note) -> Bool {
        selection.rows.contains { $0.id == note.id }
    }

    /// A draft is a note the core made but did not store: it is ticked, not saved to the database.
    private func newDraft() {
        let title = self.title
        if let row = try? PlaygroundCore.draft(Note.self, title: title.isEmpty ? "Untitled" : title) {
            selection.toggle(row)
        }
    }

    private var title: String {
        return draft.trimmingCharacters(in: .whitespaces)
    }

    private func add() {
        let title = self.title
        guard !title.isEmpty else {
            return
        }
        run {
            _ = try await notes.add(title: title)
            draft = ""
        }
    }

    /// Runs a database command; its typed `DbError` (or any other failure) shows under the field.
    private func run(_ command: @escaping @MainActor () async throws -> Void) {
        Task {
            do {
                try await command()
                problem = nil
            } catch {
                problem = error.localizedDescription
            }
        }
    }
}
