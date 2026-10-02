import PlaygroundCore
import SwiftUI

/// The to-do list. Reading `todos.visible` (or any signal) is a plain property read: the core
/// pushes changes, SwiftUI observes them. The filter, the `visible` list and the count of items
/// left are computed in the core.
///
/// The strip under the filter is the generic code of the core (ADR-058): the ticks live in a
/// `TodoSelection`, the "Latest" line is `newest(rows:)` and "New draft" is `draft(Todo.self, title:)`.
struct TodosScreen: View {
    let todos: Todos
    let selection: TodoSelection
    @State private var draft = ""
    @State private var problem: String?
    @State private var latest: Todo?

    var body: some View {
        NavigationStack {
            List {
                Section {
                    HStack {
                        TextField("What needs doing?", text: $draft)
                            .submitLabel(.done)
                            .onSubmit(add)
                            .accessibilityIdentifier("todo-input")
                        Button("Add", action: add)
                            .disabled(draft.isEmpty)
                            .accessibilityIdentifier("todo-add")
                    }
                    if let problem {
                        Text(problem)
                            .font(.footnote)
                            .foregroundStyle(.red)
                            .accessibilityIdentifier("todo-problem")
                    }
                }
                Section {
                    Picker("Show", selection: filter) {
                        ForEach(Filter.allCases, id: \.self) { filter in
                            Text(label(of: filter)).tag(filter)
                        }
                    }
                    .pickerStyle(.segmented)
                    .accessibilityIdentifier("todo-filter")
                }
                SelectionStrip(
                    kind: "todo",
                    count: selection.count,
                    latest: latest?.title,
                    picked: selection.rows.map(\.title),
                    selectAll: { selection.selectAll(rows: todos.visible) },
                    clear: { selection.clear() },
                    newDraft: newDraft,
                    extra: (title: "Remove selected", action: removeSelected)
                )
                Section {
                    ForEach(todos.visible, id: \.id) { todo in
                        HStack {
                            Button {
                                todos.toggle(id: todo.id)
                            } label: {
                                Label(todo.title, systemImage: todo.done ? "checkmark.circle.fill" : "circle")
                                    .foregroundStyle(todo.done ? .secondary : .primary)
                            }
                            Spacer()
                            Button {
                                selection.toggle(todo)
                            } label: {
                                Image(systemName: isSelected(todo) ? "checkmark.square.fill" : "square")
                            }
                            .buttonStyle(.borderless)
                            .accessibilityLabel("Select \(todo.title)")
                            .accessibilityIdentifier("todo-select")
                        }
                        .swipeActions {
                            Button(role: .destructive) {
                                todos.remove(id: todo.id)
                            } label: {
                                Label("Delete", systemImage: "trash")
                            }
                        }
                        .accessibilityIdentifier("todo-row")
                    }
                }
            }
            .onChange(of: todos.todos, initial: true) {
                // The newest to-do is the core's to say: one call of the function `newest`.
                latest = try? newest(rows: todos.todos)
            }
            .navigationTitle("Todos")
            .navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .principal) {
                    Text("\(todos.remaining) left")
                        .font(.headline)
                        .accessibilityIdentifier("remaining")
                }
                ToolbarItem(placement: .topBarTrailing) {
                    Button("Clear done") { todos.clearDone() }
                        .accessibilityIdentifier("todo-clear-done")
                }
            }
        }
    }

    private func isSelected(_ todo: Todo) -> Bool {
        selection.rows.contains { $0.id == todo.id }
    }

    /// A draft is a to-do the core made but did not store: it is ticked, not added to the list.
    private func newDraft() {
        let title = draft.trimmingCharacters(in: .whitespaces)
        if let row = try? PlaygroundCore.draft(Todo.self, title: title.isEmpty ? "Untitled" : title) {
            selection.toggle(row)
        }
    }

    private func removeSelected() {
        for todo in selection.rows {
            todos.remove(id: todo.id)
        }
        selection.clear()
    }

    /// The filter as the picker edits it: read from the core, written to the core.
    private var filter: Binding<Filter> {
        Binding(get: { todos.filter }, set: { todos.setFilter($0) })
    }

    private func label(of filter: Filter) -> String {
        switch filter {
        case .all: "All"
        case .active: "Active"
        case .done: "Done"
        }
    }

    /// Adds the draft. The core refuses a title that is empty once spaces are trimmed, with the typed
    /// `TodoError.emptyTitle`, which is shown under the field.
    private func add() {
        Task {
            do {
                _ = try await todos.add(title: draft)
                draft = ""
                problem = nil
            } catch {
                problem = error.localizedDescription
            }
        }
    }
}
