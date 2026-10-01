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
| `unavailable` / `Unavailable` (transport) | the core cannot be reached: it was shut down or never loaded, a wasm core trapped, or the remote connection closed or timed out (including an `undra dev` core that came back with another schema, or one that restarted without this core's objects: a lost session). While a remote core is reconnecting (ADR-051) every call, and every call in flight when the connection dropped, fails with `Unavailable` at once: Swift `.unavailable(.connectionLost)`, Kotlin `Unavailable` over an `UndraTransportException` of reason `CONNECTION_LOST`, TypeScript `Unavailable` over an `UndraTransportError("closed")` |
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

## How a stream ends

A stream's loop ends quietly when the stream is finished, and otherwise fails with one of the same three
(`docs/SPEC.md` 3.7, ADR-036), on every platform:

| The core ends the stream with | The loop fails with |
|---|---|
| the stream's own error, from a method whose Rust signature returns `Result<impl Stream<Item = T>, E>` (a failed opening) or `impl Stream<Item = Result<T, E>>` (an `Err(e)` item) | `E`, for example `catch FeedError.unauthorised` |
| a failure, because a restore replaced the stream's object, the core shut down, or its owner let go of it | `UndraCallError.cancelledByCore` |
| a failure, because the stream panicked | `UndraCallError.panicked(message, backtrace)` |
| a failure, because the core refused the call (a stale handle, `E_REENTRANT`) | `UndraCallError.refused(reason)`, the case a failed call with status 5 maps to |
| nothing it can read: an item or a failure body that does not decode, or a typed-error item on a stream without an error type (or one that does not decode as `E`) | `UndraCallError.malformed` |
| the core is unreachable: shut down, a lost connection | `UndraCallError.unavailable` |

The core says which one it is on the wire: a typed error and a failure are different stream items, and a failure carries
the same status as a failed call. Nothing is read from the text of a message. Cancelling the task that consumes the stream
still ends the loop quietly.

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
`try`/`catch` in Kotlin and TypeScript, so a value the wire cannot represent is reported (as `Malformed`, whose `cause` is
the `IllegalArgumentException`, `WireException` or `RangeError`), not thrown into the handler.

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
A port that has an error type answers its failures with it (`HttpError`, `FsError` and, since ADR-049, `StorageError` for `Kv`
and `SecureStore`: the Android adapters answer a lost network with `HttpError.Network`, a refused permission or a path outside
the root with `FsError.Denied`, a full disk with `FsError.Full` or `StorageError.Full`, a Keystore key that needs the user with
`StorageError.Locked` and ciphertext that no longer opens with `StorageError.Corrupt`): the core gets a typed reply, the query
client treats storage as best-effort (ADR-049: a WARN and a counter, never a crash), and the failure never reaches `onError`.
An exception that is not that type (a bug in an adapter) answers the core `unavailable`, is logged at ERROR naming the port,
the method and the adapter, and on Kotlin and TypeScript arrives here as `Malformed`, whose message names the exception.
`Unavailable` is not used for a device that is offline: it means the *core* cannot be reached.

The handler runs synchronously on the thread (Swift: the task) that made the call: the main actor for a store, the main
thread for a Compose click. Keep it short, and do not call into Undra from it: a failure reported while a handler runs
on the same thread is only logged, so a handler that calls a failing command cannot recurse (a task-local in Swift, a
thread-local in Kotlin; in TypeScript, where a command fails after the handler returned, the runtime remembers the calls
the handler started and only logs their failures). Work the handler schedules for later is outside the guard, except a
Swift `Task { }`, which inherits the task-local: a Kotlin coroutine it launches or a TypeScript timer it sets that calls
a failing command is reported again, and loops if it does the same again. A failure found inside a core callback (a
malformed change-set, a failed port) reaches a Kotlin handler on the runtime's delivery thread, never on the core's. An
exception the handler throws is logged and dropped (Kotlin: an `Error` propagates, so a debug build can crash on
purpose). The default, no handler, logs and returns. To stop at the failing line in a debug build: Swift `onError: {
assertionFailure("\($0)") }`, Kotlin `onError = { throw AssertionError(it) }`.

**A lost connection is not an `onError` event.** Over `undra dev` a command tapped while the dev server is away fails
with `Unavailable` too, and the connection state (`connectionState`, `connection`, `onConnectionChange`; ADR-051) already
says so, once. A failure that is the connection being down (`Unavailable` while a remote core is `reconnecting`, or
`closed` for a reason other than your own `close()` / `shutdown()`) is therefore logged at warning level and **not** handed
to `onError`: a crash reporter would otherwise get one event per tap for as long as the laptop sleeps. Everything else
`Unavailable` is still reported: a command on a core you closed yourself (a programming error), a timeout, and a wasm core
that trapped. The same rule on all three platforms (Kotlin keys on the transport reason `CONNECTION_LOST`, Swift and
TypeScript on the reason and the connection state).

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
`UndraReplyError`, `UndraTransportError`, `UndraRestoreError`. The remote transport's failures are among them: Kotlin's
`RemoteTransport` throws `UndraTransportException` (reason `CONNECTION_LOST` for a connection that is down, failed or could
not be made, `CLOSED` for one the app closed, `TIMEOUT`, `INTERRUPTED`), never a bare `UndraException`, so the mapping
makes it `Unavailable` by its type. What ends a remote core for good (`UndraSchemaMismatchException`,
`UndraSessionLostException`; TypeScript `UndraSchemaMismatchError`, `UndraSessionLostError`) is the `cause` of the
transport error that fails the calls after it, and maps to `Unavailable` on its own where it reaches a call unwrapped. They are the API for what bindings do not expose, and the
contract suite relies on their statuses. Everything the runtime throws on purpose, including the wire layer's errors
(`WireError`, `WireException`), is an `UndraError` / `UndraException` (Swift: the types are separate structs and enums).

Generated code maps them with one function per runtime and nothing else:

| Platform | A method without `E` | with `E` | a stream | a stream with `E` |
|---|---|---|---|---|
| Swift | `UndraCallError.mapped(error)` | `mapped(error, domain: E.self)` | `mapped(streamFailure:)` | `mapped(streamFailure:domain:)` |
| Kotlin | `UndraCallError.mapped(e)` | `mapped(e, E)` (the error's companion codec) | `mappedStream(error)` | `mappedStream(error, E)` |
| TypeScript | `UndraCallError.mapped(error)` | `mapped(error, ECodec)` | `mappedStream(error)` | `mappedStream(error, ECodec)` |

A stream ends in the vocabulary of a failed reply (ADR-036): the stream mappings read the failure's status
(`CANCELLED`, `PANIC`, `BAD_REQUEST` become `cancelledByCore`, `panicked`, `refused`, exactly as for a call), a typed-error
item as the stream's own `E` (`malformed` when it does not decode, or when the stream has no error type), and guess
nothing from the text of a message.

## Differences that remain

* A failed **port** implementation (an untyped throw) and a malformed change-set reach `onError` on Kotlin and TypeScript; on
  Swift they are logged at ERROR (a failed port answers the core `unavailable`, ADR-032 row k). A typed storage failure
  (`StorageError`) is the port's answer, not a failure of the adapter, on all three.
* TypeScript has an abort path the others spell differently (`AbortSignal` versus task or coroutine cancellation), and
  its methods are all `Promise`s: a command's promise resolves rather than being `void`.
* A wasm core cannot contain a panic (SPEC 7: `panic=abort`): the call fails as `unavailable` (reason `trap`) and the
  core is closed, where a native core answers `panicked` and keeps working. **With `recovery` on** (TypeScript,
  ADR-049, SPEC 17.1) the core is restarted from its last snapshot instead: the call that trapped, every other call and
  stream in flight, and every call made until the restart completes fail as `unavailable` with the transport reason
  `restarted` (they may or may not have run, and nothing retries them); `onPanic` gets the panic report first, then
  `onCoreRestarted` and `onError` get one `UndraCoreRestarted` (an `UndraUnhandledError` whose `error` is `panicked`)
  saying how old the snapshot was, how many calls were rejected and how many objects went stale. A call on an object
  that is not a store (a query handle excepted, which is re-created) is then `refused`. One trap more than
  `maxRestarts` within `perMs` and the core stays closed, as without recovery.
* In `wasm-worker` mode a synchronous port must run in the worker (`worker: { ports }`, SPEC 17.1): registering one on
  the main thread fails `load` with `UndraError("options")` (and a later `registerPort` throws it), naming the port,
  where it used to fail each of the core's calls to it as unavailable. A web platform without WebCrypto fails `load` with
  `UndraTransportError("unsupported")` rather than run with predictable random bytes.
