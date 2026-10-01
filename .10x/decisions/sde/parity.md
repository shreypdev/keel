# SDE: platform parity — the Kotlin and TypeScript failure model, TypeScript snapshot/restore, worker-mode ports, recursive records (wt/parity, 2026-10-01)

Implements the "parity" pieces of amendment C of the v1.x plan: **C4b** (an ADR-032 analogue for Kotlin and TypeScript,
gaps PA-1, PA-2, PA-3, PA-4, PA-9), **C4c** (TypeScript `snapshot()`/`restore()`, PA-5), **PO-4** (`wasm-worker` mode
traps on its first Clock/Rng/Log call) and the self-referential record that passed the macro and bindgen but not the Swift
compiler (**TY-1** in `.10x/specs/2026-10-01-v1x-gaps.md`; the task and this record call it TY-5, but ADR-042's TY-5 is
decimals). The constitution items behind it: R6 (every error is a typed value, on every platform) and R3 (generated code
passes native review). The decision is **ADR-032, amendment A** (appended; the ADR's body is unchanged).

## What was built

* **The closed set, per runtime, one mapping function each.** Kotlin `UndraCallError` (sealed: `CancelledByCore`,
  `Panicked(panicMessage, backtrace)`, `Refused(reason)`, `Unavailable(transport)`, `Malformed(detail)`; all under
  `UndraException`), TypeScript `UndraCallError` (an abstract class merged with a namespace of the five classes, `kind` the
  discriminant, `UndraCallFailure` the exhaustive union). The mapping is `UndraCallError.kt` / `call-error.ts`, with the four
  entry points of Swift's `mapped` (`mapped`, `mapped(.., domain)`, `mappedStream`, `mappedStream(.., domain)`) over one private
  `classify` and one private status mapping, so ADR-036's flag 3 lands as one new reply at the stream decoder.
* **One hierarchy.** Kotlin: `WireException` re-rooted under `UndraException`; the text-only transport failures became
  `UndraTransportException` (`CLOSED`, `TIMEOUT`, `CONNECTION_LOST`, `INTERRUPTED`), malformed replies `UndraProtocolException`,
  a refused snapshot `UndraRestoreException(code)`; the in-process re-entrancy guard answers the core's own `E_REENTRANT` bad
  request. TypeScript: `WireError` re-rooted (`kind: "wire"`; `UndraError` moved to `base-error.ts` to break an import
  cycle), `UndraRestoreError(code)` (the agent's piece below).
* **Commands report.** `UndraCore.report(error, operation)` and `LoadOptions.onError` (Kotlin: `(UndraUnhandledError) -> Unit`;
  TypeScript: `onError` now takes an `UndraUnhandledError` instead of `unknown`). Logs at error level, maps with the one
  function, calls the handler synchronously on the calling thread, never throws, a nested report is only logged, a handler's
  `Exception` is logged and dropped. Kotlin and TypeScript commands (a sync method with no result and no error type) are one
  `try`/`catch` that calls it; the TypeScript promise never rejects, and a command's argument encoding is inside its `try`.
  Store `apply`/`_apply` report `"<Store>.apply(signal: N)"` and skip; Kotlin decodes, `finish()`es and only then assigns
  (it assigned first). Malformed change-sets and failed ports now reach `onError` on Kotlin too (TypeScript already had them),
  from the delivery thread when found inside a core callback.
* **`shared` is a placeholder.** Kotlin `UndraCore.shared` and TypeScript `UndraCore.shared` return a closed placeholder
  while nothing is loaded or after the shared core closed; `current` says whether a core is loaded. Calls on it fail
  `Unavailable(CLOSED)`, commands only log, the first use logs the teaching message once.
* **Constructors are calls.** Kotlin `UndraCore.constructObject` (a secondary constructor cannot hold a `try`) and
  `UndraStore.observeAll()`; TypeScript `UndraStore._observeAll()`. Both close the store, so a closed core does not leak a
  handle. TypeScript `_resync` reports a failed observe instead of leaving an unhandled rejection.
* **Generators** (`kotlin.rs`, `ts.rs`): the `fromReply` helper of generated errors is gone; calls, constructors, streams,
  store `apply`, event emitters and the `decodeStream` helper follow the amendment; docs on every shape say what it throws or
  that a command does not.
* **TypeScript snapshot/restore** (C4c) and **worker-mode ports** (PO-4): below (built by a delegated agent, reviewed and
  committed by the integrator of this piece).
* **TY-5**: below.

## Kotlin and TypeScript, per shape (before and after)

| Rust | Kotlin before | Kotlin after | TypeScript before | TypeScript after |
|---|---|---|---|---|
| `fn f(&self) -> T` | throws raw `UndraReplyException`, `UndraException("closed")` or `WireException` | throws `UndraCallError` | `Promise<T>` rejects with raw `UndraReplyError`, `UndraTransportError` or `WireError` | rejects with `UndraCallError` |
| `fn f(&self)` (command) | **throws raw into the Compose `onClick`** | **never throws**; reports | rejects; un-awaited in `onClick` it is an unhandled rejection that never reaches `onError`, and ends a Node process | **never rejects**; reports |
| `fn f(&self) -> Result<T, E>` | `E`, else raw | `E` or `UndraCallError` | `E`, else raw | `E` or `UndraCallError` |
| `async fn f(&self) -> T` | `suspend`, raw | `CancellationException` or `UndraCallError` | `f(signal?)`, raw | the signal's reason or `UndraCallError` |
| `async fn f() -> Result<T, E>` | `E`, else raw | `E`, `CancellationException` or `UndraCallError` | `E`, else raw | `E`, the reason or `UndraCallError` |
| `-> impl Stream<Item = T>` (with or without `E`) | ends with `E` or raw; the core's `"cancelled: ..."` String read as `E` (`WireException` or a wrong variant) | `E` or `UndraCallError`; the String is `CancelledByCore`/`Panicked` | same, as a `WireError` | `E` or `UndraCallError` |
| constructor | raw | `E` or `UndraCallError`; a store with a closed core releases its handle | raw; the handle leaked when `observe` failed | `E` or `UndraCallError`; no leak |
| store `apply` | `java.util.logging` warning, no hook; assigned before `finish()` | reported, skipped, never half applied | `onError(unknown)` with no operation | reported as `Store.apply(signal: N)`, skipped |
| `UndraCore.shared`, nothing loaded | throws on access (inside a default argument) | placeholder; calls fail `Unavailable` | throws `UndraError("state")` on access | placeholder; calls reject `Unavailable` |
| `restore` refused | `UndraException("... code N")` | `UndraRestoreException(code)` | no `restore` | `UndraRestoreError(code)` |
| failed port implementation | warning only | reported to `onError` | `onError(unknown)` | `onError(UndraUnhandledError)` |

## Parity table (what is the same on the three platforms, and what is not)

| Item | Swift | Kotlin | TypeScript |
|---|---|---|---|
| Closed failure set | `UndraCallError` enum (5 cases) | `UndraCallError` sealed class (5 classes) | `UndraCallError` class set, `kind` |
| A call fails with | `E` / `CancellationError` / `UndraCallError` | `E` / `CancellationException` / `UndraCallError` | `E` / the `AbortSignal`'s reason / `UndraCallError` |
| Commands | non-throwing, report | non-throwing, report | `Promise<void>` that never rejects, report |
| Reporting hook | `LoadOptions.onError(UndraUnhandledError)`, `report` | `LoadOptions.onError(UndraUnhandledError)`, `report` | `onError(UndraUnhandledError)`, `report` |
| Recursion guard of the hook | task-local | thread-local | flag |
| Store `apply` failure | reported, skipped | reported, skipped | reported, skipped |
| Malformed change-set | logged | reported | reported |
| Failed port implementation | logged (core sees `unavailable`) | reported | reported |
| `shared` with nothing loaded | placeholder | placeholder | placeholder |
| `current` | `UndraCore.current` | `UndraCore.current` | `UndraCore.current` |
| Raw entry points throw | `UndraReplyError`, transport, protocol, restore | `UndraReplyException`, transport, protocol, restore | `UndraReplyError`, `UndraTransportError`, `UndraRestoreError` |
| Wire errors in the root hierarchy | n/a (separate types) | `WireException : UndraException` | `WireError : UndraError` |
| Stream error item (flag 2) | `E`, else the String | `E`, else the String | `E`, else the String |
| `snapshot`/`restore` | public | public | public (`Promise`s; main and worker) |
| Refused restore | `UndraRestoreError` | `UndraRestoreException` | `UndraRestoreError` |
| Re-entrancy refusal | the core's `E_REENTRANT` bad request → `refused` | the runtime guard's identical reply → `Refused` | n/a (single thread) |
| A wasm core's panic | n/a | n/a | the core traps: `Unavailable` (reason `trap`), the core is closed |
| `stats()`, drain-listener removal, `isClosed`, close semantics, connection status | differ (PA-6, PA-7, PA-8) | differ | differ |

Not touched, and still different (recorded in `docs/ERRORS.md`): the shapes of `stats()` and listener removal, Kotlin's
missing `isClosed`/`schemaHash`, close semantics (ADR-034), connection status (B2), a Swift failed port that is only logged.

## PO-4: worker mode, root cause and fix

`worker.ts` answered every `portCall` with `{ kind: "async" }` after posting a `PortCall` envelope, so the wasm module's
`port_call` import returned 1. In `crates/undra-ffi/src/wasm.rs` only the answer 2 ("unavailable") falls through to
`builtin::answer`, the built-in Clock, Rng and Log bindings over the `now_ms`, `random` and `log` imports; 1 means "the host
will reply through `undra_port_reply`". The core's synchronous `port_call_sync` then found no answer, returned
`PortError::Unavailable`, and the infallible generated proxy panicked (E0062), which on wasm (`panic=abort`) is a trap. A
`undra-query` reads the Clock on every observe, so any query trapped. Reproduced on the fixture core and on the playground core.

Fix: the worker returns `unavailable` for the port ids Clock, Rng and Log and posts nothing, so the built-ins answer from the
worker's own `Date.now`, `crypto.getRandomValues` and `log` import (which the worker already relays to the main thread's Log
adapter). Every other port still crosses to the main thread, asynchronously. A custom port declared `sync` cannot be served
in worker mode (the core cannot block on the main thread); `WasmWorkerTransport` logs a warning once per such port when an
answer comes back inline. Documented in SPEC 17.1. Three more worker bugs found by running a real core there, all pre-existing:
envelopes the core sent during `undra_init` carried schema 0 (the host rejected them and `load` hung to its timeout), the
init-time `PortReply` was refused ("not started"), and a protocol failure before `ready` waited for the timeout.

Tests: a unit test that fails on the old worker (the handler answers the three ports `unavailable` and posts no `PortCall`), a
real core over Node `worker_threads` in `crates/undra-ffi/tests/wasm/ts-runtime.test.mjs` (Clock within seconds of now,
monotonic, 16 distinct random bytes, the `log_line` record reaching the main thread, the Echo port round trip, a host
Clock/Rng not applying, a custom sync port failing loudly), and S17 wasm step 6 on the playground core (a query observed, a
mutation with an idempotency key from the Rng, the core's log records reaching the harness adapter; the harness serves the
worker over a `MessageChannel` in-thread because vitest cannot load TypeScript in a worker thread).

`docs/blueprint.html` said the web template runs in a worker by default (four places, ADR-008 included); the template uses
`wasm-main`. The default is unchanged; the blueprint now says `wasm-worker` is opt-in.

**ADR-049 (proposed, on `main`) is the long-term design** (worker protocol 3, `worker.ports`, `asyncPorts`, the worker
returning 2 for every port it was not given). This piece is the minimal fix and is compatible with it; two names to align when
A7 lands: ADR-049 calls the snapshot payload field `data` (this piece's control messages use `bytes`), and it bumps
`WORKER_PROTOCOL_VERSION` to 3 (this piece did not: the messages are additive and the worker announces `features`).

## C4c: TypeScript `snapshot()` / `restore()`

`UndraCore.snapshot(): Promise<Uint8Array>` and `restore(bytes): Promise<void>` over `undra_snapshot` / `undra_restore`
(`Transport` gained two optional methods). In `wasm-worker` mode they are control messages with an acknowledgement, because
`Kind.Restore` over the envelope path has no answer; the worker posts `restored` through `post()`, which flushes the batch
first, so the change-sets the restore re-delivers (ADR-023) and the replies of the calls it cancelled reach the host before
the ack. `restore` calls `mirror.flush()` once the transport resolves, so the stores show the restored values when the promise
resolves (ADR-022: the same handles stay valid). A refused snapshot rejects `UndraRestoreError(code)` and the core is
unchanged and usable; a closed core rejects `UndraTransportError("closed")`; a socket (no snapshot yet) rejects
`UndraModeError`. A snapshot is opaque: one taken in either mode restores into the other or into a fresh core. The contract
runner uses the public API (S15, S17), and the wasm-export helpers it used are gone. S15 steps 2, 4 and 6 and the platform note
were rewritten; the TypeScript column skipped nothing before (it used the wasm export directly), now it uses the product.

## TY-5 (the audit's TY-1): recursive records, supported

**Decision: support it, do not reject it.** The macros already define `Self`, `Box<T>` (transparent), `Option<Box<Self>>` and
`Vec<Self>` as the idiomatic spelling and test it (`self_is_the_enclosing_type_in_fields_only`, a wire round trip), Kotlin and
TypeScript already compile the shape, and the audit's proposal is an indirection with no schema change and no ADR. ADR-040 and
ADR-042 (read on `main` after the merge) say nothing about recursion: ADR-040 adds objects as values (a different cycle, through
handles), ADR-042 newtypes and generic instantiations (its "TY-5" is decimals). **The macros crate needed no source change**:
the three tests added are in `crates/undra-macros/tests/wire_types.rs` (`ListNode`, mutual `Parent`/`Child`, a `Block`/`Stmt`
record-enum cycle, wire round trips and the schema shapes).

Swift: the generator computes the inline containment graph (edge `A → B` when a field of `A`, or an enum payload field, is `B`
or `Option<B>`; never through `Vec`, `Map` or `Bytes`) and stores a field behind a private reference box exactly when its edge
lies on a cycle; data enums and errors with such a payload become `indirect`. The public shape stays `public var next:
ListNode?`; value semantics, the memberwise `init`, `Hashable`, `Sendable` and the `Codable` JSON shape (a nil child omitted,
a missing key decoded as nil) are those of a plain optional. **Deviation from the brief:** the property-wrapper form
(`@UndraIndirect public var next`) does not compile (Swift rejects a public property whose wrapper type is internal, and a public
wrapper would add a helper type to every generated module's API), so the audit's other form is generated: a private
`UndraIndirect<ListNode>?` storage property with a public computed property and a private `CodingKeys` (`_next` ↔ `"next"`). The
helper class is emitted once into `Types.swift` and only when a record needs it, so no existing golden moved. The depth-8
`fatalError("recursive default")` guard of the placeholder search is gone (a path-based search takes the first enum variant that
does not need the enum itself).

Tests: a golden case `recursive` (linked record, tree with `Vec<Self>`, mutual records, a record/enum cycle, an indirect enum, a
store with signals of the recursive types) for all three languages; seven generator tests; **`typecheck_swift.rs`** (new, closes
item I2 of the Swift error-channel review): every golden case as a target of one scratch SwiftPM package against the real runtime,
Swift 6 mode, plus a Swift executable (`tests/fixtures/swift-run/recursive.swift`) that checks value semantics, `==`, hashing,
JSON round trips and the Undra wire; verified red against the plain recursive struct.

**Latent bug found, not fixed (follow-up):** an enum whose *first variant recurses directly* (`Sum { Add(Sum, Sum), Zero }`) used as
a **store signal** type: Swift is now right (`Sum.zero`), but Kotlin's placeholder expression explodes exponentially and ends in
`error("recursive default")`, and the TypeScript generator's `zero_named` has no depth guard and overflows the stack. A shared,
language-neutral placeholder planner is the fix (the Swift generator's `zero_in` is the model). The `recursive` golden uses
base-case-first enums, so it does not hit it.

## Verification

| Check | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` with `UNDRA_REQUIRE_TOOLCHAINS=1`, `tsc` and JDK 17 on `PATH` (goldens, the Kotlin, TypeScript and Swift typechecks and execution tests included) | 2,250 passed, 0 failed, 10 ignored |
| Kotlin runtime `scripts/test-local.sh` | 527 cases in 32 suites, 0 failed, 2 skipped (native smoke, as before); 27 new (`CallErrorTests`), baseline 500 |
| TypeScript runtime `npm test` / `npm run typecheck` | 31 files, 1,006 tests / clean (baseline 28 files, 931 tests; 75 new, 47 of them transport work) |
| `contract-tests/ts` `npm run typecheck`; playground web `npm test`, `npm run build`, `tsc --noEmit` | clean; 102 tests; built |
| `swift test` in `runtimes/swift/UndraRuntime` | 425 tests, 0 failures (unchanged) |
| `bash crates/undra-ffi/tests/wasm/run.sh` | `raw.test.mjs` 19/19, `ts-runtime.test.mjs` 24/24 (14 new, five on a real `worker_threads` worker), none skipped |
| `bash contract-tests/run-all.sh` | see the grid in the final report |
| Android `./gradlew :app:assembleDebug` (core built with `undra build --platform android --release`) | BUILD SUCCESSFUL |

## Deviations from the brief, and things worth knowing

1. **The audit's "TY-1" is the task's "TY-5"**, and ADR-042's TY-5 is decimals; the decision above follows the audit.
2. **`typecheck_ts` and `run_ts` (and `typecheck_kotlin` before this piece) were silently skipped** when `tsc` (or `kotlinc` 2.x,
   whose `--version` fails) was not on `PATH`: they print "skipping" and pass. Every verification here ran them with
   `UNDRA_REQUIRE_TOOLCHAINS=1` and `runtimes/ts/@undra/runtime/node_modules/.bin` on `PATH`; `on_path` now also tries `-version`.
   CI should set `UNDRA_REQUIRE_TOOLCHAINS=1` where the toolchains exist. When un-skipped, the TS execution tests failed against
   the hand-written base-API stand-in (`tests/fixtures/ts-base`), which now takes the real errors and closed set from the runtime.
3. **`UPDATE_GOLDEN_LANG=kotlin,ts`** limits a golden regeneration to some languages (the other branches were editing other
   generators at the same time).
4. **Commands encode their arguments inside the `try`** (Kotlin and TypeScript); other calls encode first, because a value the
   wire cannot represent is the caller's bug, not an outcome of the call (ADR text, decision 3).
5. **A generated event emitter** (`ConnectivityEvents.changed`) is a command too: a closed core reports instead of throwing from a
   system callback.
6. **The Kotlin re-entrancy guard** now throws the reply the core would send (`UndraReplyException(BAD_REQUEST)`, reason
   `E_REENTRANT`), so S17.5 reads the same on Swift and Kotlin.
7. **Scenario steps added**, specified in `scenarios.md` first: S15.10 (a stream across a restore, all three), S16.5 (no core
   loaded, all three), plus S05.6, S15.9, S17.1/2/5/6 rewritten for the new Kotlin and TypeScript behaviour, S15.2/4/6 and S17
   wasm step 6 for the TypeScript public snapshot API and worker mode. S15.10 on TypeScript reads one item and then stops
   reading (as S07 does): a consumer that never yields starves the macrotask that would run the restore.
8. **Not edited, as instructed:** `RemoteTransport.kt`, the TypeScript remote transport, `WebSocketTransport.swift`,
   `android-adapters/`, the macros crate's sources, `site/`. Site pages that describe the Kotlin or TypeScript failures
   (`site/docs/api-kotlin.html`, `api-typescript.html`, `llms-full.txt`, blog posts that show `UndraReplyError`) are the
   integrator's, as for the Swift piece.

## What is left

* Site pages and blog text for the Kotlin and TypeScript failure model (above); `.10x/status.md` and `.10x/handoff.md`.
* The Kotlin/TypeScript placeholder bug for a recursive-first-variant enum signal (above).
* A7 (ADR-049): worker protocol 3, `worker.ports`, load-time error for main-thread sync ports; rename the snapshot field to `data`.
* A swift-side `onError` for a malformed change-set and a failed port (parity table).
* `UndraCore.stats()` shapes, Kotlin `isClosed`, close semantics and connection status stay with PA-6, PA-7, PA-8.

## After the review (2026-10-01)

`.10x/reviews/2026-10-01-parity-review.md`. Fixed there: the TypeScript `onError` guard now also covers the calls a
handler starts (a TypeScript command fails after the handler returned, so a handler that called a failing command looped on
the microtask queue); the latent placeholder bug above (the Swift search moved to `crates/undra-bindgen/src/zero.rs` and
Kotlin and TypeScript use it; the `recursive` golden gained a base-case-last `Sum` signal); the null handle of a Kotlin async
constructor and of every TypeScript constructor is `Malformed`; a malformed JNI reply or stream item is `Malformed`
(`TransportEvents.onMalformed`) instead of a fake status 5 or flag-2 String; the worker's sync-port warning says what it can
know; `errorMessage` never throws. Follow-ups: ADR-049 field and protocol names, Swift `onError` for ports and change-sets,
PA-6/7/8, and a validation error for a type with no finite value.

## Landing after dev-loop, schema-json, diagnostics and device-bench (2026-10-01)

The branch was merged with `main` (52 commits: schema-json, diagnostics, device-bench, dev-loop) so that it lands by
fast-forward. Conflicts, and what each side contributed:

| File | Resolution |
|---|---|
| `.10x/decisions/sde/_index.md` | all lines kept (dev-loop, schema-json, device-bench, diagnostics, parity) |
| `crates/undra-bindgen/README.md`, `docs/SPEC.md` (standard types) | main's Swift text (all eight standard types public, `UndraAppState` the one spelling that differs) plus this branch's `UndraCallError.mapped(...)` sentence, for the three languages |
| `crates/undra-bindgen/tests/typecheck_swift.rs` (add/add) | this branch's (one package, every case, plus the execution checks of `tests/fixtures/swift-run`); main's ADR-024 paragraph about the stdlib case kept in its header. Main's version was a strict subset |
| `docs/SWIFT_ERRORS.md` (deleted here, edited there) | stays deleted; main's one-row edit (`.unavailable(.connectionLost)` while reconnecting, `UndraSessionLostError`) is folded into `docs/ERRORS.md` |
| `core.ts` | both kept: dev-loop's `connection` signal, `onConnectionChange`, reconnect and re-observe, next to the `onError` guard, `report`, and the closed placeholder |
| `errors.ts` / `base-error.ts` | `UndraError` stays in `base-error.ts`; main's `"sessionLost"` kind is documented there, `UndraSessionLostError` stays in `errors.ts` |
| Kotlin `UndraCore.kt`, `LoadOptions.kt`, `ConnectedCore.kt` | both kept: `connectionState`, `onConnectionChange`, `reconnect`, `constructed`/`observeAgain` next to `onError`, `report`, `constructObject`, the placeholder |
| playground `UndraApp.kt`, `web/src/undra.ts` | dev-loop's start/retry/epoch and `showDevConnection` with this branch's `onError` (Log.w / console.warn) |
| goldens | not merged by hand: regenerated with `UPDATE_GOLDEN=1 cargo test --workspace`; the regeneration changed nothing (the two generators' output was already consistent) |

### Decision: the transport's exceptions and the closed set

dev-loop's runtime throws and fails calls with a bare `UndraException` (message only) wherever a connection is down,
and its in-flight failure on a drop was `UndraException("... reconnecting")`. This branch's closed set maps a bare
`UndraException` to `Unavailable` as a fallback only (for a foreign `Transport`), which is fragile: it depends on the
exact class. Decided, per runtime:

* **Kotlin.** Every failure of a transport is an `UndraTransportException` (a subclass of `UndraException`, so every
  `catch (e: UndraException)` in dev-loop's code and tests still works). `RemoteTransport` throws it, with reason
  `CONNECTION_LOST` for a connection that is down, failed, closed by the server or reconnecting, `CLOSED` when the app
  closed the transport, `TIMEOUT` for a handshake that did not answer, `INTERRUPTED`. `ConnectedCore.onReconnecting`
  fails what is in flight with `UndraTransportException(CONNECTION_LOST, ...)` and `shutDown` fails with
  `closedException(cause)` (the same type; `CONNECTION_LOST` when a cause ended it, `CLOSED` when the app did).
  `UndraCallError.mapped` makes all of them `Unavailable` by type. `UndraSessionLostException` joins
  `UndraSchemaMismatchException` as a case that maps to `Unavailable(CONNECTION_LOST)` on its own (it reaches a call
  wrapped, as the `cause`, in practice). The bare-`UndraException` arm stays, documented as the foreign-transport
  fallback; `FakeTransport` now throws the typed one, as the real transport does.
* **TypeScript.** Already typed (`UndraTransportError("closed")` from the transport and for in-flight calls on a drop).
  Added: `UndraSessionLostError` maps to `Unavailable` (an in-flight call at the final loss rejected with it raw).
* **Swift.** Already typed (`UndraTransportError.connectionLost`, `UndraSessionLostError` mapped). Unchanged.

### Decision: `onError` and a lost connection

A command tapped while `undra dev` is away fails with `Unavailable`, and the connection state (and `onConnectionChange`)
reports the drop once. Handing every such tap to `onError` would send a crash reporter one event per tap for as long as
the laptop sleeps. So `report` does not call the handler for a failure that is the connection being down: it logs it at
warning level (not error) with a pointer to `connectionState`. The rule, per runtime: Kotlin keys on `Unavailable` with
transport reason `CONNECTION_LOST` (thread-safe: a command that fails while the drop is still being announced is covered,
and `closedException(cause)` gives the same reason after the core was lost for good); TypeScript and Swift key on
`Unavailable` while the state is `reconnecting` or `closed` for a reason other than the app's own close (Swift also on
`.connectionLost`, which a call that raced the state change carries). Still reported: a call on a core the app closed
(reason `CLOSED`), a timeout, and a wasm core that trapped (not a connection). Tests: Kotlin `ReconnectCoreTests` and
`RemoteReconnectTests` (over real sockets), TypeScript `remote.test.ts`, Swift `ReconnectCoreTests`.
