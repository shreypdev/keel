// Execution test of the `generic_functions` golden case (ADR-058): the generated Swift is built with the real runtime
// and this file is its `main.swift`. The instantiations of a generic function are overloads of its own name, the type
// token of one whose parameter no argument fixes is its first parameter, and every instantiation has an id constant of
// its own. The checks that call through an overload only have to compile: no core is loaded here, so each call is
// inside a closure that is never run, and a wrong overload (or an ambiguous one) is a compile error.

import Foundation
import GoldenGeneric_functions
import UndraRuntime

nonisolated(unsafe) var failures: [String] = []

func check(_ ok: Bool, _ what: String) {
    if !ok { failures.append(what) }
}

@MainActor
func overloads(_ core: UndraCore) {
    let todo = Todo(id: 1, title: "t")
    let note = Note(id: 2, body: "n")
    // The argument's element type picks the overload and the result type follows it.
    let calls: [() throws -> Void] = [
        { let latest: Todo? = try newest(rows: [todo], ctx: core); _ = latest },
        { let latest: Note? = try newest(rows: [note], ctx: core); _ = latest },
        // A type parameter no argument fixes is named by a leading token.
        { let blank: Todo = try draft(Todo.self, ctx: core); _ = blank },
        { let blank: Note = try draft(Note.self, ctx: core); _ = blank },
        // The token is first, and a parameter called `type` does not collide with it.
        { let made: Note = try make(Note.self, type: "kind", ctx: core); _ = made },
        // A function that is not generic stands beside the families.
        { _ = try countRows(ctx: core) },
    ]
    _ = calls
    let asyncCalls: [() async throws -> Void] = [
        { let loaded: Todo = try await load(Todo.self, id: 9, ctx: core); _ = loaded },
        { try await save(todo, ctx: core) },
    ]
    _ = asyncCalls
    // Streams and commands.
    let stream: AsyncThrowingStream<Note, Error> = rows(Note.self, count: 3, ctx: core)
    _ = stream
    forget(todo, ctx: core)
    let library = { (library: Library) in
        let pinned: [Note] = try library.pinned(Note.self)
        _ = pinned
    }
    _ = library
}

// Every instantiation has an id constant of its own, named after the function and the type.
check(UndraIds.Functions.newestTodo != UndraIds.Functions.newestNote, "newest has one id per type")
check(UndraIds.Functions.draftTodo != UndraIds.Functions.draftNote, "draft has one id per type")
check(UndraIds.Functions.loadNote != UndraIds.Functions.saveNote, "different functions have different ids")
check(UndraIds.Objects.Library.pinnedTodo != UndraIds.Objects.Library.pinnedNote, "pinned has one id per type")
check(UndraIds.Objects.Library.rememberNote != UndraIds.Objects.Library.count, "a family and a plain method")
// The id is the hash of the instantiation's schema name: `fnv1a32("fn.newest<Todo>")`.
check(UndraIds.Functions.newestTodo == 0xb9743b7d, "newest<Todo> is fnv1a32(\"fn.newest<Todo>\")")

if failures.isEmpty {
    print("generic_functions: all checks passed")
} else {
    for failure in failures { print("FAILED: \(failure)") }
    exit(1)
}
