import SwiftUI
import Ios15SampleCore

/// The to-do list. Reading `todos.visible` (or any signal) is a plain property read: the core
/// pushes changes, and `@ObservedObject` re-renders this view when the store publishes one (the store is an
/// `ObservableObject` because this app supports iOS 15 and 16; `todos.$visible` is a Combine publisher).
struct ContentView: View {
    @ObservedObject var todos: Todos
    @ObservedObject var tips: TipsQueryHandle
    @State private var draft = ""
    @State private var problem: String?

    var body: some View {
        // iOS 15 has no navigation stack API: a stack-style `NavigationView` is its equivalent.
        NavigationView {
            List {
                Section {
                    HStack {
                        TextField("What needs doing?", text: $draft)
                            .onSubmit(add)
                            .submitLabel(.done)
                        Button("Add", action: add)
                            .disabled(draft.trimmingCharacters(in: .whitespaces).isEmpty)
                    }
                    if let problem {
                        Text(problem).foregroundStyle(.red).font(.footnote)
                    }
                }
                Section {
                    Picker("Show", selection: filter) {
                        ForEach(Filter.allCases, id: \.self) { Text(label(of: $0)).tag($0) }
                    }
                    .pickerStyle(.segmented)
                }
                Section("Tips") {
                    ForEach(tips.data ?? [], id: \.self) { tip in
                        Text(tip).font(.footnote)
                    }
                    if tips.fetching {
                        ProgressView()
                    }
                }
                Section {
                    ForEach(todos.visible, id: \.id) { todo in
                        Button {
                            todos.toggle(id: todo.id)
                        } label: {
                            Label(todo.title, systemImage: todo.done ? "checkmark.circle.fill" : "circle")
                                .foregroundStyle(todo.done ? .secondary : .primary)
                        }
                    }
                }
            }
            .navigationTitle("\(todos.remaining) left")
            .toolbar {
                Button("Clear done") { todos.clearDone() }
            }
        }
        .navigationViewStyle(.stack)
    }

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
