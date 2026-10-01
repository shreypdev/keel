# ADR-032: generated Swift never stops the process: calls throw, commands report

Status: accepted (2026-09-30; the integrator's choices on the open decisions are recorded at the end). Touches SPEC 10.1 (generated Swift shapes) and 17.3 (Swift runtime API:
`UndraCallError`, `UndraUnhandledError`, `LoadOptions.onError`, `UndraCore.report`, `UndraCore.shared`),
`contract-tests/scenarios.md` (new steps in S05, S06, S15 and S17), the Swift golden files of
`undra-bindgen`, the playground's generated Swift and its iOS app, and the pages that describe Swift
errors. No wire change, no C ABI change, no schema change, no Kotlin or TypeScript generated change
(their goldens stay byte-identical). Constitution: R6 is violated today; R3 decides the shape; R11
applies because a generated public shape changes. Origin: the blog fact-check
(`.work/blog/.10x/reviews/2026-09-30-blog-factcheck.md`, H1 and P1). Implementation brief:
`.10x/specs/2026-09-30-swift-error-channel-impl.md`.

## Context

R6 has two halves. The boundary half holds: every entry is guarded, a panic is caught
(`crates/undra-ffi/src/guard.rs:36`) and the core answers with a status (SPEC 3.4, `docs/SPEC.md:237`).
The second half, "every error is a typed value", does not hold for the Swift host: the generated
wrappers turn most of those statuses into `fatalError`. The policy is deliberate and documented
(`crates/undra-bindgen/src/swift.rs:9-16`, `crates/undra-bindgen/README.md:62`,
`runtimes/swift/UndraRuntime/README.md:74`, `site/docs/api-swift.html:92`): typed throws cannot express
anything but `E`, so anything else was declared "the core and the bindings disagree". That premise is
wrong for most of what actually reaches the trap. Cancellation, a call refused after `close()` or
`shutdown()`, a call refused on re-entry and a call cancelled by `undra_restore` are outcomes the SPEC
defines (`docs/SPEC.md:347`, `:348`, `:411`, `:462`), not disagreements, and a core panic is exactly
what R6 promises the app survives (S17: "The process is alive").

### 1. What reaches `fatalError` today

Paths are under `examples/playground/generated/swift/Sources/PlaygroundCore/Generated/` unless they
name another root; "golden objects" is `crates/undra-bindgen/tests/golden/objects/swift/Sources/GoldenObjects/Generated/`.
`undraUnexpected` is `Errors.swift:327-332` (`-> Never`, `fatalError`); `E.undraFromReply`
(`Errors.swift:319-324`) accepts only an `UndraReplyError` with `status == .error` whose body decodes
as `E`.

| # | Generated shape (Rust signature) | Example, abort site | What reaches `fatalError` | What propagates |
|---|---|---|---|---|
| a | sync, no error channel, returns a value (`fn f() -> T`) | `Probe.counters()` `Objects.swift:22-33` (`:31`); `add(a:b:)` `:86-100` (`:98`) | **every failure**: status 2 panic; status 5 (stale handle after `close()` or `restore`, `E_REENTRANT`, undecodable args, refused after a foreign shutdown); `UndraTransportError` (host `shutdown()`, remote timeout or disconnect); `UndraProtocolError` (malformed reply); `WireError` (result does not decode) | nothing |
| b | sync, no error channel, `()` (`fn f()`) | `Todos.clearDone()` `Stores.swift:1862-1872` (`:1870`); `Counter.increment()` `:1747-1756` (`:1755`); `RemoteTodosQueryHandle.refetch()` `Queries.swift:39-49` (`:47`) | same as (a) | nothing |
| c | sync, `Result<T, E>` | `area(_:)` `Objects.swift:124-138` (`:135`); `BigList.insertAt` `Stores.swift:1570-1585` (`:1582`) | everything except a status-1 reply that decodes as `E`; a status-1 body that does **not** decode as `E` also aborts | `E` |
| d | async, `Result<T, E>` | `Todos.add` `Stores.swift:1845-1859` (`:1856`); `failLater` `Objects.swift:271-290` (`:287`); mutations `Queries.swift:132-153` (`:150`) | everything in (c), plus `CancellationError` (task cancelled: `runtimes/swift/UndraRuntime/Sources/UndraRuntime/Core/UndraCore.swift:236`, `CallSlot.swift:35-37`, `:87-88`), status 3 (restore, `docs/SPEC.md:411`; shutdown, `:347`), and the `undra_call` rejection (`UndraCore.swift:249-252`, `:624-629`) | `E` |
| e | async, no error channel (`async fn f() -> T`) | `Probe.hang()` `Objects.swift:36-43`; `explodeLater` `:252-266` | nothing | the raw runtime errors: `CancellationError`, `UndraReplyError` (2, 3, 5), `UndraTransportError`, `UndraProtocolError`, `WireError` |
| f | stream, with or without `E` | `Probe.ticks` `Objects.swift:61-70`; golden objects `Calculator.watch` `Objects.swift:176-190` | nothing | `E` (via `mapError`) or `UndraReplyError(.error, body)`, including the `"cancelled: ..."` String body of a stream ended by restore or shutdown (`UndraCore.swift:714-721`); consumer cancellation ends the iteration quietly (`StreamChannel.swift:149-167`) |
| g | constructor, no error channel (`init(ctx:) throws`) | `Todos.init` `Stores.swift:1834-1841` | nothing | the raw runtime errors |
| h | constructor, `Result<Self, E>` | golden objects `Calculator.withPrecision` `Objects.swift:21-40` (`:36`), async `open` `:42-64` (`:59`) | as (c) and (d) | `E` |
| i | store `apply` (mirror, not a call) | `Stores.swift:1967-1973` | `assertionFailure` on an undecodable change: a trap in debug builds, a silent drop in release | n/a |
| j | default argument `ctx: UndraCore = .shared` of every constructor and free function | `UndraCore.swift:96-101` | `fatalError` when no core is loaded: before `load` succeeds or after `shutdown()` (`:465-469` clears the slot) | n/a |
| k | port adapters (the host implements the port) | `UndraCore.swift:740-745`, `:756-760` | nothing: an error that is not `E` answers port status 2 (unavailable) | n/a |

The generator decides this in three places: `swift.rs:1086-1093` (async without `E` gets plain
`throws`, sync without `E` gets nothing), `:1108` and `:1127-1134` (every sync call and every call
with `E` is wrapped, and the wrapper without `E` calls `undraUnexpected`), and `:831-845`
(`catch_typed`). With `swift_typed_throws = false` shapes (c), (d) and (h) rethrow
(`swift.rs:841`), but (a) and (b) still abort, so the documented escape hatch does not fix the
finding.

The contract suite never takes these paths on Swift: S06 cancels the untyped `probe.hang()`
(`contract-tests/swift/Tests/ContractTests/S04_S06_Async.swift:168-177`), S05.4 and S15.7 use the raw
`core.callSync`, and S17.1 calls `UndraCore.callSync` directly because the generated `explode(reason:)`
"stops the process on purpose" (`contract-tests/swift/NOTES.md:45-47`,
`S15_S17_Lifecycle.swift:205-214`). The Kotlin runner calls the generated `explode` and gets an
exception (`contract-tests/kotlin/src/dev/undra/contract/S17Panic.kt:16-17`).

How realistic each trigger is in an app:

* **Cancellation.** A SwiftUI `.task` that awaits `todos.add(title:)` is cancelled when its view
  disappears. Today that is a crash.
* **Closed object.** A view that still holds a store after `close()` and calls `todos.toggle(id:)`:
  `UndraObject.close()` itself documents that later calls "fail" with a bad request
  (`runtimes/swift/UndraRuntime/Sources/UndraRuntime/Core/UndraObject.swift:34-35`); they abort.
* **Shutdown.** `UndraCore.shutdown()` fails every pending call with `UndraTransportError.closed`
  (`UndraCore.swift:463`) and refuses later ones (`:508-510`, `InprocTransport.swift:192-195`); in-flight
  typed calls and every later sync call abort. After shutdown, `Todos()` with the default `ctx` hits
  `UndraCore.shared`'s `fatalError` (row j).
* **Restore.** Every non-store object handle becomes stale (status 5) and every call in flight on a
  replaced or invalidated receiver ends with status 3 (`docs/SPEC.md:411`).
* **Re-entry.** A synchronous port implementation (the app's own `Log`, `Kv` or `Clock`) runs inside
  the core's callback (`UndraCore.swift:731-746`); a generated sync call made from it is refused with
  `E_REENTRANT` (status 5, `docs/SPEC.md:348`, `:462`) and aborts.
* **Remote dev core.** `callSync` over the WebSocket blocks and can time out or lose the connection
  (`UndraCore.swift:532-545`, `:785-787`); every sync call then aborts the app under development.
* **Panic.** The core contains it and keeps working (S17); the Swift app does not.

### 2. What Kotlin and TypeScript do in the same situations

| Failure | Kotlin (generated + `dev.undra.runtime`) | TypeScript (generated + `@undra/runtime`) | Swift today |
|---|---|---|---|
| typed error (status 1) | throws `E`, an `UndraException` subclass: `generated/kotlin/.../Errors.kt:164-166` (`fromReply`), `Objects.kt:123-136` | rejects with `E`, an `UndraError` subclass: `generated/ts/src/errors.ts:21-26`, `objects.ts:145-155` | throws `E` |
| calling task cancelled | `CancellationException` (`runtimes/kotlin/.../ConnectedCore.kt:132-137`, `suspendCancellableCoroutine`) | rejects with the signal's reason, an `AbortError` by default (`runtimes/ts/@undra/runtime/src/core.ts:146-151`, `:359-360`) | (d), (h): abort; (e): `CancellationError` |
| core cancelled the call (status 3) | `UndraReplyException(CANCELLED)` (`ConnectedCore.kt:319-331`) | `UndraReplyError`, status 3 (`core.ts:690-691`) | (c), (d), (h): abort; (e): `UndraReplyError(.cancelled)` |
| panic (status 2) | `UndraReplyException(PANIC)`, through the generated `explode` (`S17Panic.kt:16-17`) | wasm traps (`panic=abort`, SPEC 7): the call rejects with `UndraTransportError("trap")`, the core closes, the page lives (`contract-tests/ts/test/s17-panic-containment.test.ts:42-54`) | (a)-(d), (h): abort |
| refused (status 5: stale handle, `E_REENTRANT`, bad args) | `UndraReplyException(BAD_REQUEST)` (`ConnectedCore.kt:211-218`, `:319-320`) | `UndraReplyError`, status 5 (`core.ts:690-691`) | (a)-(d), (h): abort |
| host closed or shut the core down | `UndraException("this UndraCore is closed")` (`ConnectedCore.kt:236-238`, `:300-315`) | `UndraTransportError("closed")` (`core.ts:482`, `:512`) | (a)-(d), (h): abort |
| remote timeout, disconnect | `UndraException` (`ConnectedCore.kt:196-207`) | `UndraTransportError("timeout" / "closed")` (`errors.ts:155-168`) | (a)-(d), (h): abort |
| malformed reply, undecodable result or `E` | `UndraException("malformed reply")` (`ConnectedCore.kt:116-120`) or `WireException` from `decodeAll` | `UndraTransportError("protocol")` (`core.ts:345`, `:641`) or `WireError` from `decodeValue` | (a)-(d), (h): abort |
| undecodable change-set entry | logged at warn, entry skipped (`Mirror.kt:188-200`) | logged at level 4 and passed to `LoadOptions.onError` (`core.ts:313`, `:771-778`) | trap in debug, drop in release |
| no core loaded (default `ctx`) | `UndraCore.shared` throws `UndraException` (`UndraCore.kt:43-45`) | `UndraCore.shared` throws (`core.ts:201`) | `fatalError` |

Kotlin and TypeScript have one channel for every call failure (an exception, a rejection) with typed
classes on it; TypeScript adds a hook (`LoadOptions.onError`, `core.ts:95`) for failures nobody awaits.
The Swift design below is the idiomatic equivalent of that, not a copy: `throws` where a Swift API
would throw, `CancellationError` where Swift concurrency expects it, and a reporting hook for the one
shape a Swift engineer would not make throwing.

### 3. Does Kotlin or TypeScript generated code have the same class of issue?

No. Searched the generated trees and the runtimes for `fatalError`, `preconditionFailure`, `error(`,
`check(`, `require(`, `!!`, `TODO(`, `IllegalStateException`, `assert`, `process.exit`, `throw new Error`:

* `examples/playground/generated/kotlin`: no match. `examples/playground/generated/ts/src`: only doc
  comments (`objects.ts:251`, `types.ts:51`).
* Kotlin runtime: `require`/`check` validate arguments in the codecs and the stats JSON parser
  (`wire/UndraReader.kt`, `wire/UndraWriter.kt:212`, `UndraStats.kt:109-218`) and the method id of a call
  (`ConnectedCore.kt:453`, always equal in generated code). They throw catchable exceptions and none
  depends on a reply status.
* TypeScript runtime: `throw new Error` only while loading the wasm module (`transport/wasm-main.ts:106`,
  `:118`) and in the SecureStore adapter (`adapters/secure.ts:72`, `:98`). Every generated method is
  `async`, so every failure is a rejection.
* What remains is each platform's own convention for an error nobody handles: an uncaught Kotlin
  exception on Android's main thread ends the app, and Node (not browsers) ends the process on an
  unhandled rejection. Both are the app's unhandled typed value, catchable at the call site; neither
  is generated code choosing to stop. R6 holds there.
* The Swift runtime has argument preconditions of the same kind as Kotlin's `require`
  (`Wire/UndraWriter.swift:144`, `Wire/UndraReader.swift:254-290`, `Wire/UndraTypes.swift:209`): host
  misuse of the codec API, not reachable from a reply, and Swift's convention for that. They stay. Its
  `assert` in `UndraCore.swift:581` is debug-only and unreachable from generated code. Its
  `fatalError` in `UndraCore.shared` (`:100`) is reachable from generated defaults: row (j), decided
  below.

### 4. Swift facts the decision rests on

Checked with Apple Swift 6.3.3, `-swift-version 6 -strict-concurrency=complete`, in a scratch package
(not committed):

* The standard library's cancellation-aware entry points are untyped: `Task.checkCancellation()` and
  `Task.sleep(for:)` cannot be called from a `throws(CancellationError)` function ("thrown expression
  type 'any Error' cannot be converted"). Cancellation is an ordinary thrown `CancellationError`;
  callers let it propagate or end quietly with `catch is CancellationError`. A typed-throws function can
  pass it on only if its error type can hold it.
* In a `do` whose body throws only `E`, the catch-all binds `error: E`, so `switch error` is exhaustive.
  Catch clauses with patterns (`catch .emptyTitle`) are never treated as exhaustive, even when they list
  every case and even with `-enable-upcoming-feature FullTypedThrows`; such a `do` still needs a
  catch-all. Typed throws therefore buys an exhaustive `switch` inside the catch-all, not exhaustive
  catch clauses.
* A closure written `() throws(ListError) -> Void` around a call that becomes untyped no longer
  compiles. The playground app (`examples/playground/ios/PlaygroundApp/Screens/BigListScreen.swift:115-122`)
  and the Swift contract runner (16 sites, `outcome { () throws(LabError) ... }`) write that.
* SwiftUI calls store commands from closures that cannot throw: `Button` actions
  (`TodosScreen.swift:44`, `:69`; `CounterScreen.swift:33`; `RemoteScreen.swift:64`), a `Binding`
  setter (`TodosScreen.swift:78`, `set: { todos.setFilter($0) }`) and `.refreshable`
  (`RemoteScreen.swift:68`). The site's headline example is `Button(todo.title) { todos.toggle(id: todo.id) }` (`site/docs/api-swift.html`, Stores).
* The proposed shapes below compile against a mock of the proposed runtime API, including a
  `@MainActor @Observable` store whose command reports and whose async method throws.

## Decision

1. **No generated Swift traps on the outcome of a call.** `undraUnexpected` is deleted. No reply
   status, transport failure, cancellation or undecodable byte reaches `fatalError`,
   `preconditionFailure`, `assertionFailure` or `precondition` from generated code, in any build
   configuration. A bindgen test enforces it over every golden case.

2. **A generated call fails with exactly one of three things** (SPEC 10.1, 17.3):
   * its own error `E` (reply status 1), thrown as `E` itself, so `catch TodoError.emptyTitle` and
     `catch let error as TodoError` keep working;
   * `CancellationError`, when the calling task was cancelled (async calls only; already what
     `UndraCore.call` throws);
   * `UndraCallError`, a new runtime enum, for every failure of the call itself:

   | `UndraCallError` case | From |
   |---|---|
   | `.cancelledByCore` | status 3: a restore replaced or invalidated the receiver, or the core shut down while the call ran (a stream ended with a `"cancelled: ..."` error item) |
   | `.panicked(message:backtrace:)` | status 2 (a stream panic carries the message and an empty backtrace) |
   | `.refused(reason:)` | status 5 and the `undra_call` rejection: closed or stale handle, `E_REENTRANT`, undecodable arguments, unknown method, refused after a foreign shutdown; `reason` is the core's text |
   | `.unavailable(UndraTransportError)` | this `UndraCore` was shut down, is not loaded, or its remote connection closed or timed out |
   | `.malformed(String)` | a reply that does not decode, a result or an `E` that does not decode, a typed error on a method without one, a stream where a single reply was expected, the null handle. After a successful schema check this is an Undra bug |

   `UndraCallError` is `Error, Sendable, Equatable, CustomStringConvertible, LocalizedError`, so
   `error.localizedDescription` (what the playground shows, `TodosScreen.swift:98`) reads well. A core
   cancellation is not a `CancellationError`: the caller's task was not cancelled, Kotlin and TypeScript
   keep the two apart (`UndraReplyException(CANCELLED)` versus `CancellationException`, status 3 versus
   `AbortError`), and a call silently swallowed by a `catch is CancellationError` would hide that a
   write never landed.

3. **Shapes.** Calls use untyped `throws`; the domain error is documented in `- Throws:`.

   | Rust | Swift today | Swift after ADR-032 |
   |---|---|---|
   | `fn f(&self) -> T`, `T` not `()` | `func f() -> T` | `func f() throws -> T` |
   | `fn f(&self)` | `func f()` | `func f()`, unchanged: a **command** (decision 4) |
   | `fn f(&self) -> Result<T, E>` | `func f() throws(E) -> T` | `func f() throws -> T` |
   | `async fn f(&self) -> T` | `func f() async throws -> T` | unchanged signature; errors mapped |
   | `async fn f(&self) -> Result<T, E>` | `func f() async throws(E) -> T` | `func f() async throws -> T` |
   | `fn f(&self) -> impl Stream<Item = T>` (with or without `E`) | `-> AsyncThrowingStream<T, Error>` | unchanged type; failures mapped |
   | constructor `new` | `init(ctx:) throws` / `async throws` | unchanged signature; errors mapped |
   | constructor `-> Result<Self, E>` | `static func x() throws(E) -> O` | `static func x() throws -> O` |
   | `#[undra::port]` method returning `Result<T, E>` (the host implements it) | `func m() async throws(E) -> T` | unchanged |

   Typed throws stay where the host is the implementer: a port requirement `throws(HttpError)` tells
   the implementer exactly which errors the core can understand, and the adapter already maps anything
   else to "unavailable" (row k). `swift_typed_throws` keeps its name and now governs only port
   requirements.

4. **Commands report instead of throwing.** A synchronous method that returns `()` and has no error
   type stays non-throwing. When it fails, the runtime logs the failure at error level
   (`UndraLog.error`, unified log subsystem `dev.undra.runtime`), passes an `UndraUnhandledError(operation:
   error:)` to `LoadOptions.onError` (new, optional, the TypeScript runtime's `onError` for Swift) and
   returns. It never traps, in debug or release; a team that wants a debug trap installs
   `onError: { assertionFailure("\($0)") }`. This is the one shape where the call site cannot handle a
   failure anyway (button actions, binding setters), the effect of a command is observed through the
   store's state, and the failure cases are the lifecycle and bug cases above, not domain outcomes.

   **The store's `@Observable` mutation path.** Generated stores write their properties only in
   `apply`, from the mirror (`public private(set) var`, `Stores.swift:1821-1826`); a command never writes
   a property optimistically. A command refused with status 5 (`E_REENTRANT`, a closed store, a shut-down
   core) never ran in the core, so no change-set follows and the store keeps showing exactly the core's
   state: there is nothing to roll back. A command that panicked part-way leaves whatever the core
   committed before the panic, and the mirror delivers that (SPEC 5.5: a poisoned store keeps working).
   Either way the UI is truthful, the failure is logged and reported, and the action returns.

   `onError` runs synchronously on the thread that made the call (the main actor for a store) and must
   not call into Undra; a failure reported while `onError` is running on the same thread is only logged,
   so a handler that calls a failing command cannot recurse.

5. **The mapping lives in the runtime, once.** Generated code wraps every call in one
   `do`/`catch` and hands the error to `UndraCallError.mapped(_:)` or `UndraCallError.mapped(_:domain:)`
   (streams: `mapped(streamFailure:)` / `mapped(streamFailure:domain:)`, because a stream's error item
   carries either `E` or a String, SPEC 3.7 and 5.9). The function returns `E`, `CancellationError` or an
   `UndraCallError`; generated code throws what it returns. Commands call `core.report(error,
   operation:)`. The raw byte API of `UndraCore` (`callSync`, `call`, `stream`, `construct`) keeps
   throwing `UndraReplyError` and friends: it is the API for what bindings do not expose, and the
   contract runners rely on its statuses. The success path is unchanged (a Swift `do` costs nothing
   until something throws).

6. **Store `apply`** reports an undecodable change through the same `core.report` (operation
   `"<Store>.apply(signal: N)"`) and skips the entry, matching Kotlin (log and skip) and TypeScript (log
   and `onError`). A `PatchError` still re-observes the signal as today. No re-observe on an undecodable
   full value: it would decode the same bytes again.

7. **`UndraCore.shared` with no core loaded** returns a permanently shut-down placeholder instead of
   trapping: constructors given it throw `UndraCallError.unavailable(.closed)`, commands report, and the
   first use logs the existing teaching message once ("Load a core at app startup ..."). `current` still
   returns `nil`. This closes row (j), the only runtime trap reachable from generated code. It is
   separable from 1-6 (open decision 3).

### The recommended shape, by example

```swift
// Generated (Stores.swift): a store with a typed async method, a command, and a query.
@MainActor @Observable
public final class Todos: UndraStore, @unchecked Sendable {
    public private(set) var todos: [Todo] = []

    /// Adds an item at the end of the list.
    /// - Throws: ``TodoError``, `CancellationError` if the task is cancelled, or ``UndraCallError``.
    public func add(title: String) async throws -> Todo {
        var w = UndraWriter()
        title.undraEncode(&w)
        do {
            let body = try await self.core.call(
                .objectMethod(handle: self.handle, methodId: UndraIds.Objects.Todos.add),
                method: UndraIds.Objects.Todos.add,
                args: w.finish()
            )
            return try Todo.undraDecoded(from: body)
        } catch {
            throw UndraCallError.mapped(error, domain: TodoError.self)
        }
    }

    /// Flips the `done` flag of the item with `id`; unknown ids are ignored.
    public func toggle(id: UUID) {
        var w = UndraWriter()
        id.undraEncode(&w)
        do {
            _ = try self.core.callSync(
                .objectMethod(handle: self.handle, methodId: UndraIds.Objects.Todos.toggle),
                method: UndraIds.Objects.Todos.toggle,
                args: w.finish()
            )
        } catch {
            self.core.report(error, operation: "Todos.toggle")
        }
    }
}

// App code.
Button(todo.title) { todos.toggle(id: todo.id) }          // unchanged, still non-throwing
.task {
    do {
        _ = try await todos.add(title: draft)
    } catch TodoError.emptyTitle {
        problem = "Enter a title"
    } catch is CancellationError {
        // the view went away; nothing to do
    } catch {
        problem = error.localizedDescription                // an UndraCallError: panicked, refused, ...
    }
}
```

## Alternatives considered

* **(a) `async throws(UndraCallError<E>)` with `.typed(E)`, `.cancelled`, `.core(...)`** (the
  fact-check's first preference). It keeps a typed channel, but: cancellation stops being a
  `CancellationError`, so `catch is CancellationError`, `Task.isCancelled`-style helpers and every
  library that special-cases it treat a cancelled Undra call as a real failure; methods without `E`
  need `UndraCallError<Never>`; every existing `catch TodoError.x` and every `throws(E)` closure breaks
  (more than with the decision); and the benefit is smaller than it looks, because Swift 6.3 still
  requires a catch-all after pattern catches (section 4), so the gain is an exhaustive `switch` inside
  that catch-all. A Swift engineer who never saw Rust writes `async throws` for a cancellable call, as
  Swift's own `Task.sleep` does. Rejected for R3. It can be added later as an opt-in generator mode
  without touching the decision's shapes.
* **(b) Keep `throws(E)` and route everything else through a side channel.** Impossible for anything
  that returns a value or is async: the function must return a `T` or throw an `E`, and a cancelled
  call has neither. It works only for `()`-returning commands, which is where it is adopted
  (decision 4).
* **(c) Untyped `throws` on everything, commands included.** Clean in the type system and Apple's own
  direction for I/O (`FileHandle.write(contentsOf:) throws` replaced a non-throwing call that raised),
  but every store mutation would need `try?` inside `Button`, `Binding` and `.refreshable` closures,
  and the failures a command can meet are lifecycle misuse and core bugs, not outcomes a call site can
  act on. Recorded as open decision 1 because it is the closest alternative.
* **(d) Throw the runtime's existing errors (`UndraReplyError`, `UndraTransportError`,
  `UndraProtocolError`, `WireError`) instead of a new enum.** That is what shape (e) does today. Four
  unrelated types to catch, and `UndraReplyError` exposes wire status bytes and an undecoded body, which
  fails native review. One enum with named reasons is what a Swift engineer would write.
* **(e) Status 3 as `CancellationError`.** Hides a write that never landed behind the quiet-exit idiom,
  and diverges from Kotlin and TypeScript. Rejected (decision 2).
* **(f) A handler whose default traps in debug and logs in release** (the fact-check's second option).
  The contract suite and XCTest run debug builds, so S17 through the generated `explode` would stop the
  test process; developers would learn one behaviour in debug and ship another. Kept as a one-line
  opt-in instead (decision 4).
* **(g) Return a default value, or a `Result`, from sync methods without an error channel.** A
  fabricated `ProbeCounters()` or `0` is a lie the caller cannot detect; a `Result` return is not what
  Swift code with `throws` available looks like.
* **(h) Only add a contract step and document the abort** (the fact-check's third option). Leaves R6
  broken for the platform the product leads with.

## Consequences

* **SPEC.** 10.1: the shapes table above, the three-outcome rule, the command rule, and "typed throws
  only on port requirements". 17.3: `UndraCallError` (and its `mapped` functions), `UndraUnhandledError`,
  `LoadOptions.onError`, `UndraCore.report(_:operation:)`, and `shared`'s placeholder. 3.4 and 5.1 do
  not change.
* **Goldens.** All nine Swift trees change (`Errors.swift` loses `undraUnexpected` and every
  `undraFromReply`; calls, constructors, streams and `apply` change). Kotlin and TypeScript trees are
  byte-identical, which the review checks with `git diff --stat`.
* **Swift runtime.** New public API as above; `UndraReplyError` and the raw entry points unchanged; one
  new internal transport for the placeholder core. Unit tests per status and per error type with the
  existing `FakeTransport`.
* **Contract suite.** New steps, specified in the brief: S05.6 (calls on a closed object through the
  generated bindings), S06.6 (cancel a typed async call, `fail_later`), S15.9 (a call in flight across
  `restore` ends cancelled-by-core; the invalidated object then refuses calls through the bindings),
  S17.1 through the generated `explode` on Swift (the NOTES deviation goes), S17.5 (native: a call from
  inside a sync port callback is refused with `E_REENTRANT`), S17.6 (native, last: shutdown with a typed
  call in flight, and calls on the shut-down core). Kotlin and TypeScript get the same steps where
  their platform can express them; their generated code does not change, so their steps are new
  coverage, not fixes. The Swift runner records `onError` the way the TypeScript harness already records
  `runtimeErrors` (`contract-tests/ts/src/harness.ts:55`, `:126-128`).
* **Playground iOS app.** One closure type changes (`BigListScreen.swift:115`, `throws(ListError)` to
  `throws`); `UndraBootstrap` installs an `onError` that logs, so the reference app shows the API (R10).
* **Docs.** `site/docs/api-swift.html` (Errors, Objects, Stores), `site/docs/cli.html:119`, the
  `undra.toml` template comment (`crates/undra-cli/src/config.rs:459`), `crates/undra-bindgen/README.md:62`,
  `runtimes/swift/UndraRuntime/README.md:74`, `contract-tests/swift/NOTES.md`. Blog post 4 (on
  `wt/blog`) describes the abort; the integrator updates it when this lands.
* **Source compatibility of generated Swift.** What breaks, all with a compiler error that points at
  the line: closures annotated `throws(E)` around a call; a `do` with only typed pattern catches in a
  non-throwing context (needs a catch-all); calls to sync methods without `E` that return a value
  (need `try`); code that caught `UndraReplyError` from an untyped async method (now `UndraCallError`).
  What keeps compiling: `catch TodoError.x`, `catch let e as TodoError`, `catch { error.localizedDescription }`,
  every command call, every stream loop, every constructor call. Kotlin and TypeScript apps are
  unaffected.
* **Performance.** No boundary crossing changes; the success path adds nothing; no benchmark is owed
  (R4, R9).
* **Rename.** This piece uses the Undra identifiers of its branch. Whichever of this piece and
  `wt/rename` merges second crosses the other with `scripts/rename-keel-to-undra.sh` (ADR-030), and
  regenerates the goldens rather than editing them.

## Risks

* **Lost compile-time exhaustiveness over `E`.** Mitigated by `- Throws:` documentation and by `E`
  being a closed enum (`catch let e as TodoError { switch e { ... } }` is exhaustive). An opt-in typed
  mode (alternative a) remains possible later without breaking this shape.
* **A command that fails in release goes unnoticed** if the app installs no `onError`. Mitigated: it is
  always logged at error level, it changed nothing, and the playground shows how to surface it. This
  is the TypeScript behaviour for an un-awaited rejection.
* **Stream error items are ambiguous on the wire.** For a stream with an error type, SPEC 3.7 says the
  error item carries `E`, while restore and shutdown end it with a String (SPEC 5.9). The stream
  mapping tries `E`, then the String; an `E` whose encoding happens to read as a String could be
  misclassified. Recorded as a v2 wire item (distinct flags for "cancelled" and "panicked"); not
  fixed here (R7).
* **`onError` re-entrancy.** A handler that calls Undra from inside a refused re-entrant call would
  recurse; the runtime only logs nested reports on the same thread (task-local flag).
* **Behaviour change for untyped async methods.** Code that matched `UndraReplyError(.panic)` from
  `explodeLater` must match `UndraCallError.panicked`. The contract runner is the only known caller.
* **The placeholder `shared` core defers a missing `load` to the first call.** Mitigated by the logged
  message and by `.unavailable(.closed)`'s description.

## Open decisions for the integrator

1. **Commands: report (recommended) or throw (alternative c).** Throwing is purer and breaks every
   SwiftUI action that calls a store method; reporting keeps the site's headline example as it is.
2. **Default `onError` in debug builds: log only (recommended) or `assertionFailure`.** The
   recommendation keeps R6 literal and the contract suite runnable in debug.
3. **Decision 7 (`UndraCore.shared` placeholder) in this piece or a follow-up.** Same constitution
   argument, different file; it is small and covered by the same review.
4. **Version.** The generated Swift shape breaks some app source (consequences). R7 makes only wire
   breaks major. Recommended: ship in the next minor with a migration note, as a fix to an R6
   violation; the alternative is to hold it for 2.0.
5. **Names.** `UndraCallError.cancelledByCore` (versus `.cancelled`), `UndraUnhandledError`,
   `LoadOptions.onError` (chosen for parity with TypeScript), `UndraCallError.mapped`.

### Resolved by the integrator (2026-09-30)

1. **Commands report, they do not throw** (the recommended value of open decision 1). Alternative (c) stays
   recorded as the closest alternative.
2. **The debug default of `onError` is log-only** (open decision 2): no `assertionFailure` in any build
   configuration. A team that wants a debug trap installs `onError: { assertionFailure("\($0)") }`.
3. **The `UndraCore.shared` placeholder ships in this piece** (open decision 3), with decision 7 as written.
4. **No migration note is needed** (open decision 4): the project has not shipped 1.0, so source
   compatibility of generated Swift is not yet a promise. The compile errors listed under Consequences
   are the whole migration.
5. Names are accepted as proposed (open decision 5).
