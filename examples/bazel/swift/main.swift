// The Swift consumer of the Bazel example (ADR-061): the generated bindings (`undra_swift_library`), the Swift runtime and the
// host build of the core (`undra_core`), all built by Bazel on macOS, and the real core answering in process. Every check prints
// one `bazel swift:` line; any failure exits 1, which is what makes `bazel test //swift:hello_test` fail.
import Foundation
import HelloCore
import UndraRuntime

var failed = false

func check(_ what: String, _ ok: Bool) {
    print("bazel swift: \(ok ? "ok  " : "FAIL") \(what)")
    if !ok { failed = true }
}

let core = try UndraHelloCore.load(.inproc())
check("the core loaded: \(UndraHelloCore.namespace)", UndraHelloCore.core === core)

let hello = try greeting(name: "Bazel")
check("a function crosses the boundary: \"\(hello)\"", hello == "Hello, Bazel, from the bazel-hello core")

let todos = try Todos()
_ = try await todos.add(title: "Write the rules")
_ = try await todos.add(title: "Run bazel test")
check("a store's signals mirror the core: 2 items remain, the mirror says \(todos.remaining)", todos.remaining == 2)

var typed = false
do {
    _ = try await todos.add(title: "   ")
} catch let error as TodoError {
    typed = error == .emptyTitle
}
check("a typed error from Rust arrives as a Swift error", typed)

if let first = todos.todos.first {
    todos.toggle(id: first.id)
}
check("a command changes the signals the UI observes: \(todos.remaining) remaining", todos.remaining == 1)

core.shutdown()
print("bazel swift: \(failed ? "FAILED" : "passed")")
exit(failed ? 1 : 0)
