import PlaygroundCore
import SwiftUI

/// The to-do list. Reading `todos.visible` (or any signal) is a plain property read: the core
/// pushes changes, SwiftUI observes them. The filter, the `visible` list and the count of items
/// left are computed in the core.
struct TodosScreen: View {
    let todos: Todos
    @State private var draft = ""
    @State private var problem: String?

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
                Section {
                    ForEach(todos.visible, id: \.id) { todo in
                        Button {
                            todos.toggle(id: todo.id)
                        } label: {
                            Label(todo.title, systemImage: todo.done ? "checkmark.circle.fill" : "circle")
                                .foregroundStyle(todo.done ? .secondary : .primary)
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
