import PlaygroundCore
import SwiftUI

/// Notes kept in SQLite through the opt-in `Db` port (ADR-048). The list is the core's `Notes`
/// store; the database file is the platform's (`SQLiteDbAdapter`, in Application Support), so the
/// notes are still there after a relaunch. Every change is written to the database first and then
/// shows in `notes.notes`, which the view only reads.
struct NotesScreen: View {
    let notes: Notes
    @State private var draft = ""
    @State private var problem: String?

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
                Section {
                    ForEach(notes.notes, id: \.id) { note in
                        Button {
                            run { try await notes.toggle(id: note.id) }
                        } label: {
                            Label(note.title, systemImage: note.done ? "checkmark.circle.fill" : "circle")
                                .foregroundStyle(note.done ? .secondary : .primary)
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
