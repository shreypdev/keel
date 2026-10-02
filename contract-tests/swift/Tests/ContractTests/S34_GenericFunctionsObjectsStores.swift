import Foundation
import PlaygroundCore
@testable import UndraRuntime
import XCTest

/// A to-do whose identity carries the counter `n`: the first eight bytes of its `UUID`, big-endian, which is what the core orders by.
private func todo(_ n: UInt8, _ title: String) -> Todo {
    let id = UUID(uuid: (0, 0, 0, 0, 0, 0, 0, n, 0, 0, 0, 0, 0, 0, 0, 0))
    return Todo(id: id, title: title, done: false)
}

/// A note whose id is the counter `n`.
private func note(_ n: Int64, _ title: String) -> Note {
    return Note(id: n, title: title, done: false)
}

extension ContractScenarios {
    // MARK: S34

    /// ADR-058: a generic function is one function per listed type (`newest(rows:)`, `draft(Todo.self, title:)`), a generic store is
    /// one store per alias (`TodoSelection`, `NoteSelection`: two type ids, two handles, two signals) and a generic object is
    /// a class of the alias's name (`RecentTodos`). The `selection` module of the playground core, through the generated bindings.
    func testS34_genericFunctionsObjectsAndStores() async {
        await scenario("S34", "generic functions, objects and stores") {
            let core = try self.core

            // 1. A generic function is one function per type.
            try checkEqual(try newest(rows: [todo(5, "five"), todo(9, "nine"), todo(7, "seven")], ctx: core), todo(9, "nine"), "newest of three to-dos")
            try checkEqual(try newest(rows: [note(2, "two"), note(1, "one")], ctx: core), note(2, "two"), "newest of two notes")
            let noTodo = try newest(rows: [Todo](), ctx: core)
            let noNote = try newest(rows: [Note](), ctx: core)
            try check(noTodo == nil, "newest of no to-dos is nothing")
            try check(noNote == nil, "newest of no notes is nothing")

            // 2. A type that no argument names is named by the caller.
            let first = try draft(Todo.self, title: "first", ctx: core)
            let second = try draft(Todo.self, title: "second", ctx: core)
            try checkEqual(first.title, "first", "the title of a draft")
            try check(!first.done, "a draft is not done")
            try check(first.id != second.id, "every draft has an identity of its own")
            let drafted = try draft(Note.self, title: "a note", ctx: core)
            try checkEqual(drafted.title, "a note", "the title of a drafted note")
            try checkEqual(try newest(rows: [first, second], ctx: core)?.title, "second", "newest of two drafts")

            // 3. Two instantiations are two stores.
            let todoType = UndraIds.Objects.TodoSelection.self
            let noteType = UndraIds.Objects.NoteSelection.self
            try check(todoType.typeId != noteType.typeId, "the two selections have different type ids")
            let todos = try RawStore(core: core, type: todoType.typeId, method: todoType.new)
            let notes = try RawStore(core: core, type: noteType.typeId, method: noteType.new)
            defer {
                todos.close()
                notes.close()
            }
            todos.observe()
            notes.observe()
            try check(todos.handle != notes.handle, "the two selections have two handles")
            todos.clear()
            notes.clear()

            @MainActor func toggled(_ store: RawStore, _ method: UInt32, _ row: some UndraCodec) throws {
                try store.callSync(method, encoded { (w: inout UndraWriter) in row.undraEncode(&w) })
            }
            try toggled(todos, todoType.toggle, todo(3, "three"))
            try await waitUntil("the to-do selection's change-set") { !todos.entries.isEmpty }
            let rows = todos.entries(of: 0)
            try checkEqual(rows.count, 1, "rows entries after ticking a to-do")
            try checkEqual(rows[0].op, .keyedPatch, "form of the rows entry")
            try checkEqual(try rows[0].decodePatch(Todo.self), [PatchOp<Todo>.insert(index: 0, item: todo(3, "three"))], "operations after ticking a to-do")
            try checkEqual(Set(todos.entries.map(\.signal)), Set<UInt32>([0, 1]), "rows and count arrive together")
            try check(notes.entries.isEmpty, "the note selection heard nothing")

            todos.clear()
            try toggled(todos, todoType.toggle, todo(3, "three"))
            try await waitUntil("the second change-set") { !todos.entries(of: 0).isEmpty }
            try checkEqual(try todos.entries(of: 0)[0].decodePatch(Todo.self), [PatchOp<Todo>.remove(index: 0)], "operations after unticking")
            try check(notes.entries.isEmpty, "the note selection still heard nothing")

            todos.clear()
            try toggled(notes, noteType.toggle, note(4, "four"))
            try await waitUntil("the note selection's change-set") { !notes.entries(of: 0).isEmpty }
            try checkEqual(try notes.entries(of: 0)[0].decodePatch(Note.self), [PatchOp<Note>.insert(index: 0, item: note(4, "four"))], "operations after ticking a note")
            try check(todos.entries.isEmpty, "the to-do selection heard nothing")

            // 4. Snapshot and restore keep both.
            let ticked = try TodoSelection(ctx: core)
            let marked = try NoteSelection(ctx: core)
            defer {
                ticked.close()
                marked.close()
            }
            ticked.toggle(todo(1, "one"))
            ticked.toggle(todo(2, "two"))
            marked.toggle(note(8, "eight"))
            try await waitUntil("the selections to show their rows") { ticked.count == 2 && marked.count == 1 }
            let handles = (ticked.handle, marked.handle)
            let snapshot = try core.snapshot()
            ticked.clear()
            marked.toggle(note(9, "nine"))
            try await waitUntil("the clear and the tick") { ticked.rows.isEmpty && marked.count == 2 }

            try core.restore(snapshot)
            try await waitUntil("the selections to return to the snapshot") { ticked.count == 2 && marked.count == 1 }
            try check(ticked.handle == handles.0 && marked.handle == handles.1, "the restore kept both handles")
            try checkEqual(ticked.rows, [todo(1, "one"), todo(2, "two")], "the to-do selection after the restore")
            try checkEqual(marked.rows, [note(8, "eight")], "the note selection after the restore")
            ticked.toggle(todo(1, "one"))
            try await waitUntil("a tick after the restore") { ticked.count == 1 }
            try checkEqual(ticked.rows, [todo(2, "two")], "the restored to-do selection keeps working")
            try checkEqual(marked.rows, [note(8, "eight")], "the note selection is untouched by it")

            // 5. An id that names no instantiation is refused.
            let missing = fnv1a32("fn.newest<Draft>")
            do {
                _ = try core.callSync(.freeFunction(methodId: missing), method: missing, args: [0, 0, 0, 0])
                throw ScenarioFailure(description: "a function id that names no instantiation was accepted")
            } catch let refused as UndraReplyError {
                try checkEqual(refused.status, .badRequest, "the reply to fn.newest<Draft>")
            }
            try checkEqual(UndraIds.Functions.newestTodo, fnv1a32("fn.newest<Todo>"), "the id of newest<Todo>")
            try checkEqual(UndraIds.Functions.newestNote, fnv1a32("fn.newest<Note>"), "the id of newest<Note>")
            try checkEqual(try newest(rows: [todo(1, "one")], ctx: core)?.title, "one", "the core after the refusal")

            // 6. A generic object a function returns is the alias's class.
            let recent = try recentTodos(rows: [todo(1, "one"), todo(2, "two"), todo(3, "three")], limit: 2, ctx: core)
            defer { recent.close() }
            try checkEqual(try recent.rows(), [todo(3, "three"), todo(2, "two")], "the rows of a RecentTodos")
            try checkEqual(try recent.latest(), todo(3, "three"), "the latest of a RecentTodos")
            recent.open(todo(2, "two"))
            try checkEqual(try recent.rows(), [todo(2, "two"), todo(3, "three")], "the rows after opening one")
            let other = try recentTodos(rows: [], limit: 5, ctx: core)
            try check(other !== recent, "a second object is another wrapper")
            let otherLatest = try other.latest()
            try check(otherLatest == nil, "a second object has rows of its own")
            other.close()
            try checkEqual(try recent.latest(), todo(2, "two"), "closing one object leaves the other")
        }
    }
}
