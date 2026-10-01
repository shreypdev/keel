# Swift error channel: implementation brief (ADR-032)

**Date:** 2026-09-30 · **Status:** ready once ADR-032 is accepted (open decisions 1-3 assumed at
their recommended values; if the integrator picks otherwise, sections 2.3, 3.4 and 3.8 change) ·
**Roles:** implementer `sde` (Swift + Rust), reviewer `senior-engineer` or `qa-engineer` (adversarial,
`docs/AGENT_WORKFLOW.md` section 3).

Read first, in this order: `CLAUDE.md`; `.10x/adrs/ADR-032-swift-error-channel.md` (the decision,
the tables, the alternatives); `docs/SPEC.md` 3.4, 5.1, 5.9, 10.1, 17.3; `contract-tests/scenarios.md`
S05, S06, S15, S17. Identifiers are the Undra ones of this branch; if `wt/rename` has merged to `main`
first, merge `main`, run `scripts/rename-keel-to-undra.sh`, regenerate the goldens and re-run
everything (ADR-030).

## 1. Scope

In scope:

* `runtimes/swift/UndraRuntime` (new public API, tests, README).
* `crates/undra-bindgen` (Swift generator, its tests, its goldens, its README) and the one template
  comment in `crates/undra-cli/src/config.rs`.
* `examples/playground/generated/swift` (regenerated, never edited by hand) and
  `examples/playground/ios` (one closure type, the `onError` hook).
* `contract-tests/scenarios.md` and the three runners (`contract-tests/{swift,kotlin,ts}`), plus
  `contract-tests/swift/NOTES.md`.
* `docs/SPEC.md` 10.1 and 17.3; `site/docs/api-swift.html`, `site/docs/cli.html`.

Out of scope (do not touch): the Kotlin and TypeScript generators and runtimes (their generated output
must stay byte-identical); `undra-runtime`, `undra-ffi`, `undra-wire`, `undra-meta`, `undra-macros`; the
wire and the C ABI; `.10x/status.md` and `.10x/handoff.md` (integrator only); the blog (`wt/blog`).

## 2. Swift runtime

All paths under `runtimes/swift/UndraRuntime/Sources/UndraRuntime/`.

### 2.1 New file `Core/CallError.swift`

`import Foundation` (for `LocalizedError`). Every public item gets a `///` summary.

```swift
/// Why a call into the core did not produce its result, when the reason is neither the method's own
/// error type nor the cancellation of the calling task (ADR-032).
///
/// Generated methods throw their own error (`TodoError`), `CancellationError`, or this.
public enum UndraCallError: Error, Sendable, Equatable {
    /// The core cancelled the call: a restore replaced or invalidated the object it ran on, or the
    /// core shut down while it ran (reply status 3). A cancelled Swift task throws
    /// `CancellationError` instead.
    case cancelledByCore
    /// The core panicked while running the call (reply status 2). The core caught the panic and keeps
    /// working.
    case panicked(message: String, backtrace: String)
    /// The core refused the call without running it (reply status 5): the object was closed or
    /// replaced by a restore, the call was made from inside one of the core's callbacks
    /// (`E_REENTRANT`), or the request could not be decoded. `reason` is the core's text.
    case refused(reason: String)
    /// The core cannot be reached: this `UndraCore` is shut down or not loaded, or the remote
    /// connection closed or timed out.
    case unavailable(UndraTransportError)
    /// The core answered with something the bindings cannot read. After a successful schema check
    /// this is a bug in Undra; please report it with the text.
    case malformed(String)
}
```

`CustomStringConvertible` and `LocalizedError` (`errorDescription` returns `description`). Texts:

| Case | `description` |
|---|---|
| `.cancelledByCore` | `the Undra core cancelled the call (a restore replaced its object, or the core shut down)` |
| `.panicked(m, _)` | `the Undra core panicked: <m>` |
| `.refused(r)` | `the Undra core refused the call: <r>` |
| `.unavailable(t)` | `the Undra core is unavailable: <t.description>` |
| `.malformed(s)` | `the Undra core sent a reply the bindings cannot read: <s>` |

The mapping functions (generated code calls them; also usable by hand):

```swift
extension UndraCallError {
    /// The error a generated method without an error type throws for `error`, a failure of
    /// `UndraCore.callSync`, `call` or `construct`, or of decoding their result: `CancellationError`
    /// stays itself, everything else becomes an `UndraCallError`.
    public static func mapped(_ error: any Error) -> any Error
    /// The same for a method whose Rust signature returns `Result<_, E>`: a typed reply becomes `E`.
    public static func mapped<E: UndraError>(_ error: any Error, domain: E.Type) -> any Error
    /// The error a generated stream method ends with, for a failure of `UndraCore.stream`.
    public static func mapped(streamFailure error: any Error) -> any Error
    /// The same for a stream whose Rust signature carries an error type `E`.
    public static func mapped<E: UndraError>(streamFailure error: any Error, domain: E.Type) -> any Error
}
```

Rules for `mapped(_:)`, in this order (unit-test every row):

| Input | Output |
|---|---|
| `CancellationError` | itself |
| `UndraCallError` | itself (idempotent) |
| `UndraReplyError(.cancelled)` | `.cancelledByCore` |
| `UndraReplyError(.panic)` | `.panicked(message:backtrace:)` from the two Strings of the body; `"<undecodable panic report>"` and `""` if they do not decode |
| `UndraReplyError(.badRequest)` | `.refused(reason: error.message ?? "<undecodable reason>")` |
| `UndraReplyError(.error)` | `.malformed("the core answered with a typed error, but this method has none (<n> bytes)")` |
| `UndraReplyError(.streamOpened)` | `.malformed("the core opened a stream where a single reply was expected")` |
| `UndraReplyError(.ok)` | `.malformed("the core answered ok as a failure")` (unreachable; total) |
| `UndraTransportError` | `.unavailable(error)` |
| `UndraProtocolError` | `.malformed(error.description)` |
| `WireError` | `.malformed("the reply does not decode: \(error)")` |
| `UndraModeError` | `.refused(reason: error.description)` (unreachable from generated code) |
| anything else | returned unchanged (document it: generated calls never produce one) |

`mapped(_:domain:)`: if `error` is `UndraReplyError` with `status == .error`, return
`try E.undraDecoded(from: body)` or, if that throws, `.malformed("a \(E.self) that does not decode (<n> bytes)")`;
otherwise `mapped(error)`.

`mapped(streamFailure:)` / `mapped(streamFailure:domain:)`: if `error` is `UndraReplyError` with
`status == .error` (what `UndraCore.onStreamItem` builds for flag 2, `Core/UndraCore.swift:720`): with a
domain, first try `E.undraDecoded(from: body)`; then try to read the body as exactly one String
(`UndraReader`, `readString()`, `finish()`): a String starting with `"cancelled: "` is
`.cancelledByCore`, any other String is `.panicked(message: s, backtrace: "")`; otherwise
`.malformed("a stream error item that does not decode (<n> bytes)")`. Every other input: `mapped(error)`.

```swift
/// A failure no caller could see: a generated command (a synchronous method that returns nothing and
/// has no error type) or a store's change could not be applied. Delivered to `LoadOptions.onError`.
public struct UndraUnhandledError: Error, Sendable, Equatable, CustomStringConvertible {
    /// What failed, as Swift spells it: `"Todos.toggle"`, `"configureRemote"`, `"Todos.apply(signal: 2)"`.
    public let operation: String
    /// Why.
    public let error: UndraCallError
    public init(operation: String, error: UndraCallError)
    // description: "<operation> failed: <error.description>"
}
```

### 2.2 `Core/LoadOptions.swift`

Add, with a doc comment that says when it runs (synchronously on the calling thread; the main actor
for a store), that it must not call into Undra, that the failure has already been logged, and the
one-line debug-trap recipe (`onError: { assertionFailure("\($0)") }`):

```swift
public var onError: (@Sendable (UndraUnhandledError) -> Void)?
```

Add `onError: (@Sendable (UndraUnhandledError) -> Void)? = nil` as the **last** parameter of
`init(mode:adapters:expectedSchemaHash:logLevel:connectTimeout:blockingCallTimeout:)`, of
`static func inproc(adapters:expectedSchemaHash:)` and of `static func remote(url:adapters:expectedSchemaHash:)`.
Existing call sites keep compiling.

### 2.3 `Core/UndraCore.swift`

1. Store the handler: `private let onError: (@Sendable (UndraUnhandledError) -> Void)?`, set from
   `options.onError` in `connect(transport:options:)` (`:112-130`) through the internal `init`.
2. New public method:

   ```swift
   /// Reports a failure that no caller can see (ADR-032): logs it at error level and passes it to
   /// `LoadOptions.onError`. Generated commands and store `apply` call it.
   public func report(_ error: any Error, operation: String)
   ```

   Body: `let mapped = UndraCallError.mapped(error) as? UndraCallError ?? .malformed(String(describing: error))`;
   `UndraLog.error("\(operation) failed: \(mapped)")`; then, unless a report is already running on this
   thread, call `onError?(UndraUnhandledError(operation: operation, error: mapped))` with the
   "reporting" flag set. Use a task-local for the flag
   (`@TaskLocal static var isReporting = false`, `UndraCore.$isReporting.withValue(true) { ... }`); the
   synchronous `withValue` works outside a task (thread-local fallback). Never trap.
3. `shared` (`:96-101`): when `current` is `nil`, log once (a `Guarded<Bool>`) at error level
   `"UndraCore.shared was used while no core is loaded (before UndraCore.load(_:) succeeded, or after shutdown()); calls on it fail with UndraCallError.unavailable(.closed). Load a core at app startup, before creating any Undra object."`
   and return `UndraCore.unloaded`, a `static let` built with a new internal `UnloadedTransport` and
   already shut down (`State.isShutDown = true` from the start; add an internal init parameter).
   `UnloadedTransport` (new file `Core/UnloadedTransport.swift`, internal, conforms to
   `UndraTransport` in `Core/Transport.swift`): `mode` `.inproc`; `supportsDirectSync` `true`;
   `start` throws `UndraTransportError.closed`; `send` returns `false`; `callSync`, `snapshot`,
   `restore` throw `UndraTransportError.closed`; everything else does nothing; `statsJSON` returns
   `nil`. `current` keeps returning `nil`, so S16's `UndraCore.current == nil` checks hold. Update the
   doc comment of `shared`.
4. Doc comments of `call` (`:228-233`) and `callSync` (`:204-212`): add one sentence each saying that
   generated code maps these errors with `UndraCallError.mapped`.

### 2.4 Other runtime files

* `Core/Errors.swift` header comment (`:1-10`): add `UndraCallError` and `UndraUnhandledError` to the
  list, and in the `UndraReplyError` doc (`:14-19`) say it is what the raw entry points throw, and that
  generated methods throw `UndraCallError` instead.
* `README.md:34` (file list) and `:74` (cancellation): replace the `undraUnexpected` sentence with
  "Generated methods throw `CancellationError` when their task is cancelled, in every shape (ADR-032)."
  Add a short "Errors" paragraph: the three outcomes, commands and `onError`.

### 2.5 Runtime tests

New `Tests/UndraRuntimeTests/CallErrorTests.swift`, using `FakeTransport` (`onCallSync` returns reply
bytes; `onCall` + the transport's reply helpers for async) and the `TestSupport.swift` helpers:

1. One test per row of the `mapped(_:)` table (build the input directly).
2. `mapped(_:domain:)`: a status-1 body that decodes (returns `E`, compare with `==`), one that does not
   (`.malformed`), and a non-typed status (falls through).
3. Stream mapping: `E` body, `"cancelled: the runtime shut down"` String body, other String body,
   garbage body, each with and without a domain; once more with an `E` that has a `String` payload
   (the ambiguity of ADR-032, Risks), pinning which way it resolves.
4. End to end through `UndraCore` with `FakeTransport`: `callSync` answering status 2, 3, 5 and garbage;
   `call` cancelled before sending and while waiting (expects `CancellationError` after `mapped`); a
   call failed by `shutdown()` (`.unavailable(.closed)`); `callSync` after `shutdown()`.
5. `report`: logs (no crash), calls `onError` exactly once with the operation and the mapped error;
   a handler that calls `report` again on the same thread does not recurse (counter stays 1); no
   handler installed is fine.
6. `UndraCore.shared` with nothing loaded returns a core whose `callSync` throws
   `UndraTransportError.closed` and whose `construct` throws, and `UndraCore.current` stays `nil`.
   (Run it in a test that knows no core is loaded; the existing tests use `connect`, which never sets
   the shared slot.)
7. `UndraCallError.description` / `localizedDescription` for each case are the texts of section 2.1.

## 3. Generator (`crates/undra-bindgen/src/swift.rs`)

### 3.1 Module docs `:9-16`

Replace the failure-policy paragraph with: a method fails with its `E`, `CancellationError`, or
`UndraCallError`; calls use untyped `throws`; synchronous methods that return `()` and have no error type
are commands that report to `UndraCore.report`; typed throws remain on port requirements only
(`Generator::swift_typed_throws`); nothing generated traps (ADR-032).

### 3.2 `errors_file` `:632-648` and `error` `:765-787`

Delete the `undraUnexpected` block (`:638-646`) and the `extension <E> { static func undraFromReply }`
block (`:765-787`). The error enums, their codecs and their `CustomStringConvertible, LocalizedError`
extension stay as they are.

### 3.3 Helpers `:831-854`

* Delete `catch_typed`.
* Add `fn catch_mapped(&self, w, err: Option<&str>)` that writes
  `} catch {` / `throw UndraCallError.mapped(error, domain: <E>.self)` (or `throw UndraCallError.mapped(error)`) / `}`.
* Add `fn catch_report(&self, w, core: &str, operation: &str)` that writes
  `} catch {` / `<core>.report(error, operation: "<operation>")` / `}`. Escape `operation` with
  `swift_string`.
* Rename `throws_clause` to `port_throws_clause`; it is used only by `port` (`:1254`) and keeps its
  behaviour (`throws(E)` when `typed()`, `throws` otherwise).
* `typed()` (`:366`): doc comment "typed throws on port requirements".

### 3.4 `callable` `:1026-1147`

Add `owner: String` to `Site::Method` (`:1528`), filled with `o.name` in `object` (`:915-925`). The
operation string is `"<owner>.<swift method name>"` for methods and `"<swift function name>"` for free
functions and mutations.

Classification (after the stream early-return):

```text
is_command = !c.is_async && err.is_none() && is_unit
throws     = if is_command { "" } else if c.is_async { " async throws" } else { " throws" }
```

Doc lines (replace the `- Throws:` push at `:1054-1056`; streams and commands get none):

| Shape | `- Throws:` line |
|---|---|
| sync, no `E` | `- Throws: ``UndraCallError`` if the call fails in the core or cannot reach it.` |
| sync, `E` | `- Throws: ``<E>``, or ``UndraCallError`` if the call fails in the core or cannot reach it.` |
| async, no `E` | `- Throws: `CancellationError` if the task is cancelled, or ``UndraCallError``.` |
| async, `E` | `- Throws: ``<E>``, `CancellationError` if the task is cancelled, or ``UndraCallError``.` |

Body: always `do { <call>; <return decode> }` followed by `catch_report` for a command or
`catch_mapped(err)` otherwise. The unguarded branch (`:1135-1145`) and the `guarded` variable go away.

Emitted, for the four shapes (indentation as today):

```swift
/// What the probe has seen so far.
/// - Throws: ``UndraCallError`` if the call fails in the core or cannot reach it.
public func counters() throws -> ProbeCounters {
    do {
        let body = try self.core.callSync(
            .objectMethod(handle: self.handle, methodId: UndraIds.Objects.Probe.counters),
            method: UndraIds.Objects.Probe.counters,
            args: []
        )
        return try ProbeCounters.undraDecoded(from: body)
    } catch {
        throw UndraCallError.mapped(error)
    }
}

/// Removes every finished item.
public func clearDone() {
    do {
        _ = try self.core.callSync(
            .objectMethod(handle: self.handle, methodId: UndraIds.Objects.Todos.clearDone),
            method: UndraIds.Objects.Todos.clearDone,
            args: []
        )
    } catch {
        self.core.report(error, operation: "Todos.clearDone")
    }
}

/// Reads an unsigned number of at most nine digits.
/// - Throws: ``LabError``, or ``UndraCallError`` if the call fails in the core or cannot reach it.
public func parseCount(text: String, ctx: UndraCore = .shared) throws -> UInt32 {
    var w = UndraWriter()
    text.undraEncode(&w)
    do {
        let body = try ctx.callSync(
            .freeFunction(methodId: UndraIds.Functions.parseCount),
            method: UndraIds.Functions.parseCount,
            args: w.finish()
        )
        return try UInt32.undraDecoded(from: body)
    } catch {
        throw UndraCallError.mapped(error, domain: LabError.self)
    }
}

/// Waits forever. The only way it ends is cancellation, which the `cancelled` counter shows.
/// - Throws: `CancellationError` if the task is cancelled, or ``UndraCallError``.
public func hang() async throws -> UInt32 {
    do {
        let body = try await self.core.call(
            .objectMethod(handle: self.handle, methodId: UndraIds.Objects.Probe.hang),
            method: UndraIds.Objects.Probe.hang,
            args: []
        )
        return try UInt32.undraDecoded(from: body)
    } catch {
        throw UndraCallError.mapped(error)
    }
}
```

A free-function command reports through its context: `ctx.report(error, operation: "ping")`.

### 3.5 Streams `:1061-1079`

Always pass `mapError`: `mapError: { UndraCallError.mapped(streamFailure: $0, domain: <E>.self) }` with
`E`, `mapError: { UndraCallError.mapped(streamFailure: $0) }` without. Type stays
`AsyncThrowingStream<T, Error>`. Doc: no `- Throws:` (streams are not `throws`); keep the method docs.

### 3.6 Constructors `:934-1019`

`throws` is ` throws` for every constructor (sync or async) whether or not it has `E`; delete the
`match` at `:953-956`. The body is always:

```swift
let handle: UndraHandle
do {
    handle = try ctx.construct(...)              // or: let body = try await ctx.call(.constructor(...)); handle = try UndraHandle.undraDecoded(from: body)
} catch {
    throw UndraCallError.mapped(error, domain: CalcError.self)   // or mapped(error) without E
}
```

followed by `self.init(adopting:core:)` or `return <O>(adopting:core:)` as today. Doc lines: the
sync/async rows of the table in 3.4.

### 3.7 Store `apply` `:1211-1217`

Replace the `assertionFailure` line with
`self.core.report(error, operation: "<Store>.apply(signal: \(signal))")` (a Swift string
interpolation in the emitted code; the store name is a literal). Keep the `PatchError` branch.

### 3.8 Other generator files

* `crates/undra-bindgen/src/lib.rs:79`: doc of `swift_typed_throws` becomes "Swift port requirements
  use `throws(E)` (true) or `throws` (false). Calls never use typed throws (ADR-032)."
* `crates/undra-bindgen/README.md:62`: replace the paragraph with the three outcomes, the command rule,
  and "typed throws only on port requirements".
* `crates/undra-cli/src/config.rs:459`: `# swift_typed_throws = true   # port requirements: \`throws(E)\`; false emits plain \`throws\``.

## 4. Generator tests

In `crates/undra-bindgen/tests/generators.rs`:

* Replace `typed_throws_can_be_turned_off` (`:112-127`) with
  `swift_typed_throws_only_changes_port_requirements`: generate the `ports` case both ways; with the
  option on, port protocols contain `throws(`; off, they do not; the `Objects.swift`, `Stores.swift`
  and `Queries.swift` of the `objects` case are byte-identical both ways.
* `swift_generated_code_never_stops_the_process`: for every case under `tests/golden/` and both values
  of `swift_typed_throws`, no generated `.swift` file contains `fatalError`, `undraUnexpected`,
  `preconditionFailure`, `precondition(`, `assertionFailure`, `assert(` or `try!`.
* `swift_call_shapes_follow_adr_032` on the `objects` case, asserting these exact substrings:
  `public func add(a: Int32, b: Int32) throws -> Int32`, `public func reset() {` followed (within the
  method) by `self.core.report(error, operation: "Calculator.reset")`,
  `public func divide(a: Int64, b: Int64) throws -> Int64`,
  `throw UndraCallError.mapped(error, domain: CalcError.self)`,
  `public func lookup(id: UUID) async throws -> Todo`, `public func compute(input: Double?) async throws -> Double`,
  `) throws -> Calculator {` (for `withPrecision`), `) async throws -> Calculator {` (for `open`),
  `mapError: { UndraCallError.mapped(streamFailure: $0, domain: CalcError.self) }`,
  `mapError: { UndraCallError.mapped(streamFailure: $0) }`; and that `Errors.swift` contains neither
  `undraFromReply` nor `undraUnexpected`.
* `swift_store_apply_reports_undecodable_changes` on the `stores` case: `self.core.report(error, operation: "` and `.apply(signal: \(signal))")` present, `assertionFailure` absent.

## 5. Regenerating

Goldens are regenerated, never edited. Expected surface: the nine `tests/golden/*/swift/` trees and
`examples/playground/generated/swift/`; nothing under `kotlin/` or `ts/` changes. If a Kotlin or
TypeScript file moves, stop: the change leaked out of `swift.rs`.

## 6. Contract suite

### 6.1 `contract-tests/scenarios.md` (paste these, adjusting nothing but numbering if needed)

**S05, new step 6** (after step 5, so step 5's delta of 3 is unchanged):

> 6. Through the generated bindings, on closed objects: close a `BigList`, then `remove_at(0)` fails as
>    **bad request** (Swift `UndraCallError.refused`, Kotlin `UndraReplyException(BAD_REQUEST)`, TypeScript
>    `UndraReplyError` status 5); close a `Counter`, then `increment()`: Swift returns normally and
>    `LoadOptions.onError` received one `UndraUnhandledError` with operation `Counter.increment` and
>    `.refused`; Kotlin throws `UndraReplyException(BAD_REQUEST)`; TypeScript rejects with `UndraReplyError`
>    status 5. `bad_requests` grew by exactly 2 more, the process is alive and `add(1, 2) == 3`.

**S06, new step 6:**

> 6. A cancelled call of a method **with a typed error**: start `fail_later(5000, 1)`, wait 100 ms,
>    cancel it. The platform call ends as cancelled exactly as in step 2 (Swift `CancellationError`, not
>    a `LabError` and not a stopped process; Kotlin `CancellationException`; TypeScript the signal's
>    reason), within 1 s; `crossings.cancelled` grew by 1; `add(1, 1) == 2` afterwards.

**S15, new step 9:**

> 9. A call in flight across a restore. `Probe` = new; start `probe.hang()`; wait until
>    `counters().started == 1`; take a snapshot; `restore` it. The probe is not a store, so the restore
>    invalidates it and cancels its call: `hang()` fails as **cancelled by the core** (Swift
>    `UndraCallError.cancelledByCore`, Kotlin `UndraReplyException(CANCELLED)`, TypeScript `UndraReplyError`
>    status 3), not as a platform cancellation. Then, through the generated bindings, `probe.counters()`
>    fails as bad request (stale handle) and `probe.reset()` does too (Swift: returns normally and
>    `onError` received `Probe.reset` with `.refused`). Close the probe.

**S17, step 1** keeps its text; add the sentence "Swift and Kotlin call the generated `explode`
(Swift: `UndraCallError.panicked`, Kotlin: `UndraReplyException(PANIC)`)." **Step 2:** "Swift:
`UndraCallError.panicked`".

**S17, new native steps 5 and 6:**

> 5. Re-entry is refused, not deadlocked or aborted: while the core is logging the panic of
>    `explode("reenter")`, the runner's Log adapter (a synchronous port, called on the thread that holds
>    the core lock) calls the generated `add(1, 1)` once. That call fails as **bad request** whose reason
>    contains `E_REENTRANT` (Swift `UndraCallError.refused`, Kotlin `UndraReplyException(BAD_REQUEST)`);
>    `explode` itself fails as in step 1; afterwards `add(1, 2) == 3`.
> 6. Shutdown with a typed call in flight (the last step of the run: it ends the core). Start
>    `fail_later(5000, 1)`, wait 100 ms, shut the core down: the call fails as **closed** (Swift
>    `UndraCallError.unavailable(.closed)`, Kotlin `UndraException` "closed") within 1 s. Then, on the
>    shut-down core, the generated `add(1, 2)` fails the same way and `Counter.increment()` on a store of
>    that core returns (Swift; `onError` received `.unavailable(.closed)`) or throws (Kotlin). The
>    process is alive.

**S17 (wasm), new step 5:**

> 5. On the trapped core, calls through the generated bindings reject with `UndraTransportError`
>    (`trap` or `closed`) and none hangs: an async call (`add_later`), a store command
>    (`Counter.increment`) and a typed one (`parse_count`).

### 6.2 Swift runner (`contract-tests/swift/Tests/ContractTests/`)

* `Support.swift`: add untyped siblings of `outcome` and `checkFailure`:
  `func checkThrows<Value, Expected: Error & Equatable>(_ body: () throws -> Value, _ expected: Expected, _ what: @autoclosure () -> String, file:line:) throws`
  and its `async` form; they fail the scenario unless the thrown error `as? Expected == expected`. Move
  the 16 `outcome { () throws(E) ... }` sites to them. Keep `outcome` (port tests may still use it).
* About 30 calls of value-returning sync methods without an error type (`counters()`, `add`, `greet`,
  `echo*`, `version`, `benchAdd`, `benchEchoBytes`) gain `try`; the compiler lists them.
* `Harness/Fixture.swift`: pass `onError` in the `LoadOptions` it builds (`:40`) and record into a
  `Locked<[UndraUnhandledError]>` exposed as `Fixture.shared.unhandled`; scenarios compare deltas.
* `Harness/CapturingLog.swift`: an optional one-shot hook
  `onNextRecord(where: (level, target, message) -> Bool, run: () -> Void)` for S17.5. Verify first, with
  a raw `core.callSync` inside the hook, that the panic record arrives on the thread holding the core
  lock (the refusal is then observable); if it does not, use the `ManualClock` (a sync port the query
  cache reads, S12) as the trigger instead, and say so in `NOTES.md`.
* `S04_S06_Async.swift`: S05 step 6, S06 step 6.
* `S15_S17_Lifecycle.swift`: S15 step 9; S17 step 1 through the generated `explode(reason:ctx:)`
  (delete the raw call and its comment at `:205-214`), step 2 catches `UndraCallError`; steps 5 and 6 (6
  last; S17 is the last scenario in alphabetical order).
* `NOTES.md`: delete the S17.1 deviation (`:45-47`); add S17.5's trigger and "S17.6 ends the core".

### 6.3 Kotlin runner (`contract-tests/kotlin/src/dev/undra/contract/`)

Generated Kotlin does not change. Add S05.6 (`S05ErrorPropagation.kt`), S06.6 (`S06Cancellation.kt`,
`scope.async { failLater(5000u, 1) }`, `cancelAndJoin`), S15.9 (`S15Snapshot.kt`), S17.5 and S17.6
(`S17Panic.kt`; re-entry through `CapturingLog.kt`; 6 only if S17 is the last entry of `SCENARIOS` in
`Scenarios.kt`, otherwise move 6 into whichever scenario runs last and note it in `NOTES.md`).

### 6.4 TypeScript runner (`contract-tests/ts/test/`)

Generated TypeScript does not change. Add S05.6 (`s05-error-propagation.test.ts`), S06.6
(`s06-cancellation.test.ts`, `failLater(5000, 1, core, controller.signal)`), S15.9
(`s15-snapshot-restore.test.ts`, restore through the wasm export as the scenario already does), S17
wasm step 5 (`s17-panic-containment.test.ts`). S17.5 and S17.6 are native-only; add that to the
platform notes of `scenarios.md`.

## 7. Playground iOS app (`examples/playground/ios/PlaygroundApp/`)

* `Screens/BigListScreen.swift:115`: `private func run(_ action: () throws -> Void)`, and the four
  closures at `:89`, `:96`, `:103`, `:109` drop `throws(ListError)` (write `() throws in` or nothing).
* `UndraBootstrap.swift:32` and `:37`: pass `onError:` that logs through an
  `os.Logger(subsystem: "dev.undra.playground", category: "undra")` at `.error`. This is the reference
  use of the API (R10).
* Nothing else should need a change (every other call is a command or already sits in a `do` with a
  catch-all). Build with `smoke.sh`; the XCUITest tour must still pass.

## 8. Docs

* `docs/SPEC.md` 10.1 (`:595-614`): error enum line unchanged; object listing becomes
  `public func add(a: Int32, b: Int32) throws -> Int32`, `public func fetch(url: String) async throws -> String // throws HttpError`;
  store listing `public func add(title: String) async throws -> Todo // throws TodoError`; port line
  unchanged; replace the last paragraph's typed-throws sentence with the ADR-032 rules (three outcomes,
  commands report through `UndraCore.report` and `LoadOptions.onError`, typed throws only on port
  requirements, nothing generated traps).
* `docs/SPEC.md` 17.3 (`:1069-1088`): add `UndraCallError` with its cases and the four `mapped`
  functions, `UndraUnhandledError`, `LoadOptions.onError`, `func report(_ error: any Error, operation: String)`,
  and "`shared`: the loaded core, or a shut-down placeholder (calls fail with `.unavailable(.closed)`)".
* `site/docs/api-swift.html`: Errors section (`:79-92`): the three outcomes with the catch example from
  the ADR, commands and `onError`, typed throws on ports; Objects listing (`:93-105`) and Stores listing
  (`:106-128`) as in SPEC 10.1. `site/docs/cli.html:119`: the new comment text of 3.8.

## 9. Commands

```bash
cd <worktree>
source scripts/env.sh
export DEVELOPER_DIR=/Applications/Xcode.app/Contents/Developer

# Swift runtime
(cd runtimes/swift/UndraRuntime && swift test)

# Generator, goldens, lints
cargo test -p undra-bindgen
UPDATE_GOLDEN=1 cargo test -p undra-bindgen --test golden
cargo test -p undra-bindgen
git diff --stat -- crates/undra-bindgen/tests/golden | grep -Ev '/swift/|files? changed' || true   # must print nothing
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Playground bindings
cargo build -p undra-cli
target/debug/undra bindgen -C examples/playground
target/debug/undra bindgen -C examples/playground --check
git diff --stat -- examples/playground/generated | grep -Ev '/swift/|files? changed' || true      # must print nothing

# Contract scenarios, three platforms (51/51)
bash contract-tests/swift/run.sh
bash contract-tests/run-all.sh

# Unchanged suites that must stay green
runtimes/kotlin/undra-runtime/scripts/test-local.sh
(cd runtimes/ts/@undra/runtime && npm ci && npm test)
bash crates/undra-ffi/tests/swift/run.sh

# The iOS app builds, installs and passes its tour
bash examples/playground/ios/smoke.sh
```

## 10. Definition of done

1. `grep -rnE 'fatalError|undraUnexpected|assertionFailure|preconditionFailure|precondition\(' crates/undra-bindgen/tests/golden examples/playground/generated/swift`
   prints nothing; the generator test of section 4 enforces it.
2. `grep -rn fatalError runtimes/swift/UndraRuntime/Sources` prints nothing (the `shared` trap is
   gone, decision 7); the codec preconditions named in ADR-032, context section 3, stay.
3. Every command of section 9 passes; contract scenarios 51/51 with the new steps; Kotlin and
   TypeScript generated trees and goldens byte-identical to `main`.
4. The Swift contract runner calls the generated `explode` in S17.1 and no step works around a
   generated binding; `NOTES.md` says so.
5. Every new public Swift item has a `///` summary; SPEC 10.1 and 17.3, `api-swift.html`, `cli.html`,
   the two READMEs and the config template match the code.
6. `.10x/decisions/sde/swift-error-channel.md` records what was built and any deviation from this brief
   (with the reason); the shared state files are untouched.
7. An adversarial review in `.10x/reviews/2026-MM-DD-swift-error-channel-review.md` with no open High
   or Medium finding.

## 11. Commits (small, in order)

1. `feat(swift-runtime): UndraCallError, UndraUnhandledError, LoadOptions.onError and UndraCore.report (ADR-032)` (with tests)
2. `fix(swift-runtime): UndraCore.shared without a loaded core fails calls instead of trapping (ADR-032)` (with tests)
3. `fix(bindgen): generated Swift never traps; calls throw, commands report (ADR-032)` (generator, its tests, regenerated goldens, README, config comment)
4. `chore(playground): regenerate the Swift bindings; BigListScreen and onError` (generated tree, iOS app)
5. `test(contracts): S05.6, S06.6, S15.9, S17.5-6 on three platforms; S17.1 through the generated binding`
6. `docs(spec): SPEC 10.1 and 17.3, api-swift and cli pages for ADR-032`

Each ends with the attribution trailer the session requires.

## 12. What the reviewer attacks

* Any path in the generated Swift or the runtime that still stops the process on a call outcome,
  including a `try!` or a force cast added while refactoring.
* `CancellationError` really propagates from every async shape (typed and untyped methods, async
  constructors), and a stream's consumer cancellation still ends iteration quietly.
* Status 3 is never reported as `CancellationError`, and a host cancellation never as
  `.cancelledByCore`.
* `report` never traps, does not recurse, and runs `onError` on the calling thread; the playground's
  handler does not call Undra.
* The placeholder `shared` core never becomes `current`, never reaches `undra_*`, and does not leak
  registrations.
* Kotlin and TypeScript output unchanged; new contract steps assert the platform-specific types named in
  6.1, not just "it failed".
* The stream String-body heuristic (ADR-032, Risks) is tested with a typed stream whose error enum has
  a String payload.
