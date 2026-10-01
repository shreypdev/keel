# Errors in generated code: Swift, Kotlin and TypeScript

What an engineer sees when an Undra call goes wrong, and that it is the same on every platform. The decisions and
their alternatives are in `.10x/adrs/ADR-032-swift-error-channel.md` (Swift, then its amendment A for Kotlin and
TypeScript); the shapes are in `docs/SPEC.md` 10.1 to 10.3 and 17.1 to 17.3.

**Generated code never stops the app, and never lets a raw runtime failure escape.** No reply status, cancellation,
lost connection or undecodable byte reaches `fatalError` (Swift), an uncaught exception in a click handler (Kotlin) or
an unhandled promise rejection (TypeScript). Every failure is a typed value.

## A call fails with one of three things

A generated method that can fail does exactly this, on every platform:

| | Swift | Kotlin | TypeScript |
|---|---|---|---|
| **its own error `E`**: the Rust method returns `Result<_, TodoError>` and the core answered with the error | `throw`s the enum itself: `catch TodoError.emptyTitle`, `catch let error as TodoError` | `throw`s the sealed class: `catch (e: TodoError)` | rejects with the class: `error instanceof TodoError`, `error.kind === "emptyTitle"` |
| **the caller's cancellation** | `CancellationError` (the task was cancelled; the call is cancelled in the core too) | `CancellationException` (the coroutine was cancelled; same) | the `AbortSignal`'s reason, an `AbortError` by default (the call is cancelled in the core too) |
| **`UndraCallError`**: everything else that goes wrong with the call itself | `enum UndraCallError: Error` | `sealed class UndraCallError : UndraException` | `abstract class UndraCallError extends UndraError`, `kind` is the discriminant |

`UndraCallError` is a closed set of five cases:

| Case | When |
|---|---|
| `cancelledByCore` / `CancelledByCore` | the core cancelled the call: a restore replaced or invalidated the object it ran on, or the core shut down while it ran. **Not** a platform cancellation: your task was not cancelled, and a write that never landed must not look like a quiet exit |
| `panicked` / `Panicked` (message, backtrace) | the core panicked while running the call. The core caught the panic and keeps working (a wasm core traps instead: see `unavailable`) |
| `refused` / `Refused` (reason) | the core would not run the call: the object was closed or replaced by a restore, the call was made from inside one of the core's own callbacks (`E_REENTRANT`), or its arguments could not be decoded |
| `unavailable` / `Unavailable` (transport) | the core cannot be reached: it was shut down or never loaded, a wasm core trapped, or the remote connection closed or timed out (including an `undra dev` core that came back with another schema) |
| `malformed` / `Malformed` (detail) | the core answered with something the bindings cannot read. After a successful schema check this is a bug in Undra: please report it with the text |

All three read well as text: Swift `error.localizedDescription`, Kotlin `error.message`, TypeScript `error.message`.

Argument validation is not an outcome of the call. A value the wire cannot represent (a negative `Duration`, a map with
two keys that encode alike, a TypeScript `number` outside a `u8`) is a programming error: it propagates unchanged
(`precondition` in the Swift codec, `WireException` / `IllegalArgumentException` in Kotlin, `RangeError` / `TypeError` in
TypeScript) before anything is sent. Commands are the one exception, below.

```swift
// Swift
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

```kotlin
// Kotlin
viewModelScope.launch {
    try {
        store.add(draft)
    } catch (e: TodoError) {
        problem = when (e) { TodoError.EmptyTitle -> "Give it a title first." }
    } catch (e: UndraCallError) {
        problem = e.message                         // Panicked, Refused, Unavailable, ...
    }                                               // a CancellationException ends the coroutine as usual
}
```

```ts
// TypeScript
try {
  await todos.add(draft, controller.signal);
} catch (error) {
  if (error instanceof TodoError) problem = "Give it a title first.";
  else if (error instanceof UndraCallError) problem = error.message;   // switch (error.kind): "panicked", "refused", ...
  else throw error;                                                     // the AbortError of controller.abort()
}
```

In Kotlin both `TodoError` and `UndraCallError` are `UndraException`s, and in TypeScript both are `UndraError`s, so one
`catch (e: UndraException)` / `error instanceof UndraError` shows either to the user.

## Shapes

| Rust | Swift | Kotlin | TypeScript | Fails with |
|---|---|---|---|---|
| `fn f(&self) -> T` | `func f() throws -> T` | `fun f(): T` | `f(): Promise<T>` | `UndraCallError` |
| `fn f(&self) -> Result<T, E>` | `func f() throws -> T` | `fun f(): T` | `f(): Promise<T>` | `E` or `UndraCallError` |
| `fn f(&self)` (a **command**) | `func f()` | `fun f()` | `f(): Promise<void>` | **never**: it reports |
| `async fn f(&self) -> T` | `func f() async throws -> T` | `suspend fun f(): T` | `f(signal?): Promise<T>` | cancellation or `UndraCallError` |
| `async fn f(&self) -> Result<T, E>` | `func f() async throws -> T` | `suspend fun f(): T` | `f(signal?): Promise<T>` | `E`, cancellation or `UndraCallError` |
| `-> impl Stream<Item = T>` (with or without `E`) | `AsyncThrowingStream<T, Error>` | `Flow<T>` | `AsyncIterable<T>` | `E` or `UndraCallError` while iterating; leaving the loop early ends it quietly |
| constructor | `init(ctx:) throws` | `fun create(ctx)` / `constructor(ctx)` | `static create(core): Promise<T>` | `E` or `UndraCallError` |
| store `apply` (a change from the core) | skips and reports | skips and reports | skips and reports | never throws |

Each method names what it can fail with in its documentation (`- Throws:`, `@throws`, `@throws {...}`).

## Commands report instead of failing

A synchronous method that returns nothing and has no error type, such as `todos.toggle(id:)` or `counter.increment()`,
is a **command**. It never throws and never rejects, because that is the shape UI frameworks call from places that cannot
handle a failure: SwiftUI `Button` actions and `Binding` setters, Compose `onClick`, a React handler:

```swift
Button(todo.title) { todos.toggle(id: todo.id) }       // Swift: non-throwing
```
```kotlin
Button(onClick = { store.setFilter(f) }) { ... }       // Kotlin: non-throwing
```
```tsx
<button onClick={() => void todos.toggle(id)}>...</button>   // TypeScript: the promise never rejects
```

If a command fails, Undra logs the failure at error level, passes an `UndraUnhandledError` (`operation`, `error:
UndraCallError`) to the `onError` you gave `UndraCore.load`, and returns. A command never writes a store property or a
signal itself (stores change only from the core's change-sets), so a refused command leaves the screen showing exactly
what the core holds: there is nothing to roll back. A caller that awaits a TypeScript command learns that it was sent and
answered, not that it succeeded; read the effect from the store. A command's arguments are encoded inside its own
`try`/`catch` in Kotlin and TypeScript, so a value the wire cannot represent is reported, not thrown into the handler.

```swift
try UndraCore.load(.inproc(expectedSchemaHash: UndraIds.schemaHash,
    onError: { unhandled in logger.error("\(unhandled.description, privacy: .public)") }))
```
```kotlin
UndraCore.load(LoadOptions(expectedSchemaHash = UndraIds.SCHEMA_HASH,
    onError = { unhandled -> Log.w("app", "${unhandled.operation} failed: ${unhandled.error.message}") }))
```
```ts
await UndraCore.load({ mode: "wasm-main", wasm, expectedSchemaHash: UndraIds.schemaHash,
  onError: (unhandled) => reportToSentry(unhandled) });   // unhandled.operation, unhandled.error.kind
```

What reaches `onError`, on every platform: a failed command, and a change from the core that a store cannot decode
(operation `"Todos.apply(signal: 2)"`; the change is skipped, never half applied). Kotlin and TypeScript also report a
malformed change-set (dropped whole) and a port implementation that failed (operation `"port 0x... method 0x..."`).

The handler runs synchronously on the thread (Swift: the task) that made the call: the main actor for a store, the main
thread for a Compose click. Keep it short, and do not call into Undra from it: a failure reported while a handler runs on
the same thread is only logged, so a handler that calls a failing command cannot recurse (a task-local in Swift, a
thread-local in Kotlin, a flag in TypeScript). A failure found inside a core callback (a malformed change-set, a failed
port) reaches a Kotlin handler on the runtime's delivery thread, never on the core's. An exception the handler throws is
logged and dropped (Kotlin: an `Error` propagates, so a debug build can crash on purpose). The default, no handler, logs
and returns. To stop at the failing line in a debug build: Swift `onError: { assertionFailure("\($0)") }`, Kotlin
`onError = { throw AssertionError(it) }`.

## Before a core is loaded

`UndraCore.shared`, the default `ctx` / `core` of every generated constructor and free function, is a closed placeholder
when no core is loaded (before `UndraCore.load` succeeds, or after the shared core was shut down or closed). Using it is a
programming error but not a crash: constructors and calls on it fail with `UndraCallError.unavailable(.closed)` /
`Unavailable` (whose transport reason is `closed`, and whose text says to load a core), commands only log, the first use
logs "Load a core at app startup, before creating any Undra object", and `registerPort` on it is ignored with a warning.
`UndraCore.current` (Swift: `current`, Kotlin: `current`, TypeScript: `current`) stays `nil` / `null`: **check it, not
`shared`**, to learn whether a core is loaded.

## Where the mapping lives

The raw entry points of every runtime (`callSync`, `call`, `stream`, `construct`, `snapshot`, `restore`) throw the runtime's
own errors: Swift `UndraReplyError`, `UndraTransportError`, `UndraProtocolError`, `UndraRestoreError`; Kotlin
`UndraReplyException`, `UndraTransportException`, `UndraProtocolException`, `UndraRestoreException`; TypeScript
`UndraReplyError`, `UndraTransportError`, `UndraRestoreError`. They are the API for what bindings do not expose, and the
contract suite relies on their statuses. Everything the runtime throws on purpose, including the wire layer's errors
(`WireError`, `WireException`), is an `UndraError` / `UndraException` (Swift: the types are separate structs and enums).

Generated code maps them with one function per runtime and nothing else:

| Platform | A method without `E` | with `E` | a stream | a stream with `E` |
|---|---|---|---|---|
| Swift | `UndraCallError.mapped(error)` | `mapped(error, domain: E.self)` | `mapped(streamFailure:)` | `mapped(streamFailure:domain:)` |
| Kotlin | `UndraCallError.mapped(e)` | `mapped(e, E)` (the error's companion codec) | `mappedStream(error)` | `mappedStream(error, E)` |
| TypeScript | `UndraCallError.mapped(error)` | `mapped(error, ECodec)` | `mappedStream(error)` | `mappedStream(error, ECodec)` |

A stream's error item carries the stream's own `E` or, when the core ended the stream itself (a restore or shutdown:
`"cancelled: ..."`, or a panic), a `String`; the stream mappings read `E` first, then the `String`. (ADR-036 replaces the
guess with a distinct wire flag; the mapping function is where it will land, on all three.)

## Differences that remain

* A failed **port** implementation and a malformed change-set reach `onError` on Kotlin and TypeScript; on Swift they are
  logged (a failed port answers the core `unavailable`, ADR-032 row k).
* TypeScript has an abort path the others spell differently (`AbortSignal` versus task or coroutine cancellation), and
  its methods are all `Promise`s: a command's promise resolves rather than being `void`.
* A wasm core cannot contain a panic (SPEC 7: `panic=abort`): the call fails as `unavailable` (reason `trap`) and the
  core is closed, where a native core answers `panicked` and keeps working.
* A custom `sync` port cannot serve the core in `wasm-worker` mode (SPEC 17.1).
