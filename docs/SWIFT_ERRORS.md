# Errors in generated Swift

What a Swift engineer sees when an Undra call goes wrong. The decision and its alternatives are in
`.10x/adrs/ADR-032-swift-error-channel.md`; the shapes are in `docs/SPEC.md` 10.1 and 17.3.

**Generated Swift never stops the app.** No reply status, cancellation, lost connection or undecodable byte reaches
`fatalError`, `precondition` or `assertionFailure`, in debug or release.

## A call fails with one of three things

A generated method that can fail uses plain `throws` (or `async throws`). What it throws is exactly one of:

| Thrown | When |
|---|---|
| its own error, for example `TodoError` | the Rust method returns `Result<_, TodoError>` and the core answered with the error. Thrown as the enum itself, so `catch TodoError.emptyTitle` and `catch let error as TodoError` work |
| `CancellationError` | the calling task was cancelled (async calls). The call is cancelled in the core too |
| `UndraCallError` | everything else that goes wrong with the call itself |

`UndraCallError` is `Error`, `Sendable`, `Equatable` and `LocalizedError` (`error.localizedDescription` reads well in an alert):

| Case | When |
|---|---|
| `.cancelledByCore` | the core cancelled the call: a restore replaced or invalidated the object it ran on, or the core shut down while it ran. Not a `CancellationError`: your task was not cancelled, and a write that never landed must not look like a quiet exit |
| `.panicked(message:backtrace:)` | the core panicked while running the call. The core caught the panic and keeps working |
| `.refused(reason:)` | the core would not run the call: the object was closed or replaced by a restore, the call was made from inside one of the core's own callbacks (`E_REENTRANT`), or its arguments could not be decoded |
| `.unavailable(UndraTransportError)` | the core cannot be reached: it was shut down or never loaded, or the remote connection closed or timed out (including an `undra dev` core that came back with another schema) |
| `.malformed(String)` | the core answered with something the bindings cannot read. After a successful schema check this is a bug in Undra: please report it with the text |

```swift
.task {
    do {
        _ = try await todos.add(title: draft)
    } catch TodoError.emptyTitle {
        problem = "Enter a title"
    } catch is CancellationError {
        // the view went away; nothing to do
    } catch {
        problem = error.localizedDescription      // an UndraCallError: panicked, refused, ...
    }
}
```

The error a method can throw is named in its `- Throws:` documentation. Constructors throw the same three (an `async` one
is `async throws`), and a stream (`AsyncThrowingStream<T, Error>`) ends with them; cancelling the task that consumes a
stream still ends the loop quietly.

## Commands report instead of throwing

A synchronous method that returns nothing and has no error type, such as `todos.toggle(id:)` or `counter.increment()`, is a
**command**. It stays non-throwing, because SwiftUI calls it from `Button` actions and `Binding` setters that cannot throw:

```swift
Button(todo.title) { todos.toggle(id: todo.id) }     // unchanged, still non-throwing
```

If a command fails, Undra logs the failure at error level (unified log, subsystem `dev.undra.runtime`), passes an
`UndraUnhandledError(operation:error:)` to `LoadOptions.onError`, and returns. A command never writes a store property
itself (stores change only from the core's change-sets), so a refused command leaves the screen showing exactly what the
core holds: there is nothing to roll back.

```swift
try UndraCore.load(.inproc(
    expectedSchemaHash: UndraIds.schemaHash,
    onError: { unhandled in
        // "Todos.toggle failed: the Undra core refused the call: ..."
        logger.error("\(unhandled.description, privacy: .public)")
    }
))
```

`onError` runs synchronously on the thread that made the call (the main actor for a store), so keep it short, and it must
not call into Undra: a failure reported while the handler is running is only logged, so a handler that calls a failing
command cannot recurse. The guard is a task-local value, so a `Task { }` started inside the handler inherits it and what
that task reports is only logged too; `Task.detached` starts clean. The default (`nil`) logs and returns. To stop at the
failing line in debug builds, install `onError: { assertionFailure("\($0)") }`.

Every generated command says so in its documentation (`- Note: A failure is logged and passed to LoadOptions.onError`),
and every stream method names what iterating it throws.

A store whose change from the core cannot be decoded skips it and reports it the same way, with the operation
`"Todos.apply(signal: 2)"`.

## Typed throws are for ports

`throws(E)` remains on **port requirements** (`func request(_:) async throws(HttpError) -> HttpResponse`), where your app is
the implementer and `E` tells it exactly which errors the core understands. `swift_typed_throws = false` in `undra.toml`
turns those into plain `throws`. Calls never use typed throws: Swift's own cancellation entry points (`Task.checkCancellation()`,
`Task.sleep`) throw an untyped `CancellationError`, which a `throws(E)` function cannot pass on.

## Before a core is loaded

`UndraCore.shared`, the default `ctx` of every generated constructor and free function, is a shut-down placeholder when no
core is loaded (before `UndraCore.load` succeeds, or after `shutdown()`): constructors throw `UndraCallError.unavailable(.closed)`,
commands log, `registerPort` is ignored with a warning, the first use logs "Load a core at app startup, before creating any
Undra object", and `UndraCore.current` stays `nil`. Check `UndraCore.current`, not `shared`, to learn whether a core is
loaded.

## What did not change

The raw entry points of `UndraCore` (`callSync`, `call`, `stream`, `construct`) keep throwing `UndraReplyError`,
`UndraTransportError` and friends: they are the API for what the bindings do not expose. Generated code maps them with
`UndraCallError.mapped(_:)` (`mapped(_:domain:)` when the method has an error type, `mapped(streamFailure:)` for streams) and
nothing else. Kotlin and TypeScript already surface every failure as an exception or a rejection, so their generated code
did not change.
