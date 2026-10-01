# Platform parity (C4b, C4c, PO-4, recursive records) — adversarial review

**Date:** 2026-10-01 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/parity` at `d32f60a` (14 commits, `main` merged in) · **Read:** `CLAUDE.md`; ADR-032 and its amendment A; the bar the
Swift side met (`.10x/reviews/2026-09-30-swift-error-channel-review.md`); the SDE record `.10x/decisions/sde/parity.md`;
`docs/ERRORS.md`; `docs/SPEC.md` 10.2, 10.3, 17.1, 17.2; the Kotlin runtime (`UndraCallError.kt`, `UndraCore.kt`,
`ConnectedCore.kt`, `InprocTransport.kt`, `LoadOptions.kt`, `Errors.kt`, `UnloadedCore.kt`, `UndraStore.kt`, `Mirror.kt`,
`PortRegistry.kt`), `kotlin.rs` and its goldens; the TypeScript runtime (`call-error.ts`, `base-error.ts`, `core.ts`,
`object.ts`, `worker.ts`, `transport/wasm-worker.ts`, `transport/wasm-main.ts`, `transport/worker-protocol.ts`), `ts.rs` and
its goldens; `crates/undra-ffi/tests/wasm/ts-runtime.test.mjs`; the Swift containment graph and placeholder search
(`swift.rs`), the `recursive` golden, `typecheck_swift.rs`, `tests/fixtures/swift-run/recursive.swift`;
`contract-tests/scenarios.md` S05.6, S15.9/10, S16.5, S17 and the three runners; the playground's generated Kotlin and
TypeScript and the Android and web call sites.

## Verdict

The piece brings Kotlin and TypeScript to the bar ADR-032 set for Swift: one closed `UndraCallError` per runtime, one
mapping function per runtime whose table matches Swift's, commands that report instead of throwing or rejecting, store
`apply` that reports and skips, a placeholder `shared`, and a TypeScript `snapshot()`/`restore()` with the semantics of
the native ones in both wasm modes; worker mode no longer traps on its first clock read. Two Medium findings were open
and are fixed here: the TypeScript `onError` guard did not cover TypeScript's own commands (a handler that called a failing
command looped on the microtask queue and froze the page, while the docs said it could not recurse), and the latent
recursive-placeholder bug the SDE recorded (generated Kotlin that throws `IllegalStateException` when the store is created;
a TypeScript generator stack overflow). Seven Low findings are fixed; the rest are follow-ups. No High finding.

## Findings

| # | Sev | Area | Where (at `d32f60a`) | Finding | Status |
|---|---|---|---|---|---|
| M1 | Medium | R6, TS `onError` | `runtimes/ts/@undra/runtime/src/core.ts:533-547` (`report`, `#reporting`) | The recursion guard is a synchronous flag, but every TypeScript method is `async`: a command the handler calls (`onError: () => void inbox.refetch()`, a plausible "retry") fails in a microtask *after* the handler returned and the flag is down, so `report` calls the handler again, which calls the command again. On a closed core that is an endless microtask chain: no timer, render or input ever runs again. Reproduced (the bounded test below saw 50 handler calls for one failure). ADR-032 amendment A decision 5, SPEC 17.1 and `docs/ERRORS.md` all said a handler that calls a failing command "cannot recurse". Kotlin and Swift commands are synchronous, so their guards hold. | **Fixed**: a call `#request` starts while the handler runs (`#reporting`) has its failure remembered in a `WeakSet` (a shared transport failure, as `close()` hands every pending call, is copied first, so other callers' reports are unaffected), and `report` only logs a remembered failure. Tests: `a handler that calls a failing command is not called again for it ...` (red without the guard: 50 calls) and `the failure the handler's call shares with other pending calls still reaches the handler for them`. Docs: `AttachOptions.onError`, `report`, SPEC 17.1, `docs/ERRORS.md` (which also now says that work a handler *schedules*, a Kotlin coroutine or a TS timer, is outside every guard but Swift's task-local). |
| M2 | Medium | R6, bindgen (the SDE's "latent bug") | `crates/undra-bindgen/src/kotlin.rs:499-564` (`zero_named`, depth guard at `:501`); `crates/undra-bindgen/src/ts.rs:744-798` (`zero_named`, no guard) | A store signal of `enum Sum { Add(Box<Sum>, Box<Sum>), Neg(Box<Sum>), Zero }` (valid Rust, accepted by the macros and by bindgen validation): Kotlin expands the first variant exponentially and writes the depth guard's `error("recursive default")` into the property initializer, so the generated store throws `IllegalStateException` the moment it is created, outside the closed set (reproduced against `d32f60a`'s generator: one 28 KB initializer holding 512 `error(...)` calls); TypeScript recurses without bound and the generator overflows its stack (reproduced: `fatal runtime error: stack overflow`, SIGABRT, so `undra bindgen` aborts). The Kotlin depth guard also fired for a type that is not recursive at all, once records nest nine deep. | **Fixed (decision: fix here, it was local)**: the Swift generator's path-based search moved to a shared `crates/undra-bindgen/src/zero.rs` (`ZeroState::named`: a type on the current path is refused, a success is memoised, a step budget bounds a schema of types with no finite value), and Kotlin and TypeScript now use it the way Swift did: an enum's placeholder is its first variant that can be built without the enum itself; an optional, array or map is empty. For a schema without recursion the first variant always succeeds, so every non-recursive golden, the CLI golden and the playground are byte-identical (`git status` after `UPDATE_GOLDEN=1`). The `recursive` golden gained `Sum` (base case last) as a store signal, so `typecheck_swift`, `typecheck_kotlin`, `typecheck_ts` and `run_ts` compile it on all three; generator tests `kotlin_and_typescript_placeholders_of_a_recursive_type_use_its_base_case`, `kotlin_placeholders_of_deeply_nested_records_are_built_whole`, `kotlin_and_typescript_survive_a_type_that_has_no_value_at_all`, and unit tests in `zero.rs`. A type with no finite value at all (`enum E { A(Box<E>) }`) still gets an unusable placeholder on every platform (`fatalError` / `error(...)` / `undefined as never`); no Rust core can hold such a store (see I8). |
| L1 | Low | Mapping parity | `kotlin.rs:1407-1441` (async constructor); `core.ts:454-461` (`construct`) | The Swift review made the null handle of an async constructor `.malformed` (its L2). Kotlin checked it only in the synchronous `ConnectedCore.construct`; a Kotlin async constructor and every TypeScript constructor (all go through `core.construct`) built an object around handle 0, whose calls then failed `Refused`. | **Fixed**: generated Kotlin async constructors throw `UndraCallError.Malformed("the core returned the null handle for a constructor")` (three bindgen goldens and the CLI golden regenerated; the playground has none); TypeScript `UndraCore.construct` rejects `UndraTransportError("protocol")`, which maps to `Malformed`, without counting the handle. Test `rejects the null handle as a protocol failure ...` (`core.test.ts`). |
| L2 | Low | Mapping parity | `runtimes/kotlin/.../InprocTransport.kt:214-217`, `:232-234` | A reply or stream item from JNI that does not decode was turned into a fake core answer: a reply into status 5 (so `Refused("the core sent a malformed reply ...")`), a stream item into a flag-2 error item carrying a String (so `Panicked("the core sent a malformed stream item ...")`, or, on a stream with an `E`, possibly decoded as that `E`). Swift reports both as `.malformed`. Unreachable without a broken ABI, but the closed set said something false. | **Fixed**: `TransportEvents.onMalformed(callId, UndraProtocolException)`; `ConnectedCore` fails the pending call or stream with it (`Malformed` after mapping). `InprocTransportTests` updated (they pinned the old fake statuses), and a `CallErrorTests` case drives a call and a stream through it. |
| L3 | Low | Worker ports | `transport/wasm-worker.ts:462-485` (`#warnSyncPort`) | The "custom sync port" warning fires whenever a port answers inline, which is also what an *asynchronous* port does when its implementation returns bytes directly (a hand-written `PortImpl`, or a standard adapter whose argument decoding throws before its first `await`). For those the reply is delivered and used, yet the warning said "has already failed as unavailable and this reply is discarded". Harmless (one line per port, behaviour correct) but undocumented and misleading. Generated non-sync adapters are `async`, so they never trigger it. | **Fixed**: the message says what the transport can know ("if the core called it synchronously ...; if the port is not declared sync, the reply was delivered and this warning can be ignored"); the class doc, the method doc and SPEC 17.1 describe the false-positive case. |
| L4 | Low | TS `report` never throws | `runtimes/ts/@undra/runtime/src/platform.ts:8-10` (`errorMessage`) | `String(error)` throws for a value without a string form (`Object.create(null)`, a throwing `toString`), so `report` (through `asCallError`) could throw and a command's promise could reject. | **Fixed**: `errorMessage` falls back to `Object.prototype.toString`. Test `a thrown value without a string form is still reported ...`. |
| L5 | Low | Tests | `test/call-error.test.ts:367-374` | "a command on it only logs" (the placeholder) asserted nothing about the log. | **Fixed**: it spies on `console.error` and checks the one record. |
| L6 | Low | Docs | `docs/ERRORS.md` (Commands) | A command's argument-encoding failure is reported, but as which case was not said (`Malformed`, with the original as the `cause`: the closed set has no "invalid argument" case); the guard's limits for scheduled work were not said. | **Fixed** (with M1's text). |
| L7 | Low | ADR text | ADR-032 amendment A, decision 1 | Says Kotlin `CancelledByCore` is a `data object`; the code and SPEC 17.2 make it a class without fields, which is right: a singleton `Throwable` shares one stack trace and one suppressed list across every throw. | **Fixed**: the amendment says so ("corrected in the review"). |
| I1 | Info | Worker protocol | `worker-protocol.ts`, ADR-049 (proposed) | The snapshot/restore control messages carry `bytes` where ADR-049 says `data`, and the piece announces `features: ["snapshot"]` in `ready` instead of bumping `WORKER_PROTOCOL_VERSION` to 3. Additive and safe today (a host never sends them to a worker that did not announce them; tested). | Follow-up for A7 (ADR-049): rename to `data` and fold `features` into protocol 3. Not a blocker. |
| I2 | Info | Parity | Swift runtime | A failed port implementation and a malformed change-set reach `onError` on Kotlin and TypeScript but are only logged on Swift. Fixing it means calling `report` from core callbacks on the Swift side (threading and the task-local guard), and amendment A states Swift is unchanged. | Follow-up (recorded in `docs/ERRORS.md`, "Differences that remain"). |
| I3 | Info | Parity | PA-6, PA-7, PA-8 | `stats()` shapes, Kotlin's missing `isClosed`/`schemaHash`, close semantics (ADR-034) and connection status (B2) still differ. Adding `isClosed` alone is small but is a public API decision that belongs with the `stats()` shapes. | Follow-ups, as the SDE record says. |
| I4 | Info | Commands | `kotlin.rs`, `ts.rs` (commands) | The split "commands encode inside the `try`, every other call encodes before it" is right: a command cannot throw, and a click handler can do nothing with a `RangeError`; any other call lets a value the wire cannot represent propagate as the programming error it is, which is what Kotlin's `require`, TypeScript's `RangeError` and Swift's codec preconditions mean. A TypeScript non-command call rejects with it rather than throwing synchronously (every method is `async`); that is the platform's convention. | Accepted. |
| I5 | Info | Guards | all three runtimes | Work a handler schedules (a Kotlin coroutine it launches, a TypeScript timer) and that calls a failing command is reported again; a handler that does so loops through the dispatcher (the UI stays responsive). Swift's `Task { }` inherits the task-local guard. | Documented (L6); "do not call into Undra from it" stands. |
| I6 | Info | Mapping | `Errors.kt` `panicInfo`, `errors.ts` `reason` | A panic body whose message decodes but whose backtrace does not keeps the message on Swift and becomes `"<undecodable panic report>"` on Kotlin and TypeScript. Unreachable from a real core. | No change. |
| I7 | Info | Mapping | `call-error.ts` `classify` | TypeScript maps `UndraSchemaMismatchError` (a remote core that came back with another schema) to `Unavailable` with transport reason `"closed"`; Swift and Kotlin say connection lost. `TransportFailure` has no such reason. | No change. |
| I8 | Info | bindgen | `validate.rs` | A type with no finite value (`enum E { A(Box<E>) }`, `struct S { s: Box<S> }`) is accepted by validation; the generators survive it (tested on all three) but write a placeholder that cannot work. Such a type can never cross (it has no value) and no store can hold it. | Follow-up: an E0001 "this type has no finite value; give it a base case" in validation, which would also let the three fallbacks go. |
| I9 | Info | R12, worker ports | `crates/undra-ffi/src/builtin.rs`, `adapters/system.ts` | In worker mode `Rng.fill` is `crypto.getRandomValues` (a CSPRNG; `cryptoRng` throws rather than falls back when WebCrypto is missing), `Clock.now_ms` is `Date.now`, and `Clock.monotonic_ns` is the shell's `Monotonic` over `Date.now` (`fetch_max`: never goes back; millisecond resolution, and it stalls while the wall clock steps back), where `wasm-main` uses `performance.now`. These are host adapters behind the ports, which is what R12 asks. | No change. |
| I10 | Info | JNI | `UndraNative.callSync` | The JNI shim returns `null` for `callSync` only when its own guard trips; Kotlin declares the result non-null and would hit an `NullPointerException` in `Payloads.Reply.decode`, outside the closed set. Pre-existing and unreachable in practice. | No change. |

## Attack 1: completeness of R6 on Kotlin and TypeScript

Every path by which a status byte, transport failure, decode failure or cancellation reaches generated code, and where it
ends (after the fixes):

| Source | Kotlin: raised at, as | TypeScript: raised at, as | Ends as |
|---|---|---|---|
| reply status 1 | `ConnectedCore.replyBody`/`complete`: `UndraReplyException(ERROR)` | `#onReply`: `UndraReplyError(1)` | `E` (with a domain), else `Malformed`; a body that does not decode as `E`: `Malformed` |
| status 2, 3, 5 | same, `UndraReplyException` | same, `UndraReplyError` | `Panicked`, `CancelledByCore`, `Refused` |
| status 0 or 4 as a failure, status > 5 | `fromStatus` | `#onReply` (> 5: `UndraTransportError("protocol")`), `fromReply` | `Malformed` |
| the transport refuses the payload (`undra_call` non-zero) | `submit`: `UndraReplyException(BAD_REQUEST)` | n/a (`send` throws) | `Refused` |
| re-entrant call from a core callback | `InprocTransport.checkNotInCallback`: `UndraReplyException(BAD_REQUEST, "E_REENTRANT: ...")` | n/a (single thread; the wasm core refuses itself) | `Refused` |
| core closed by the host, placeholder `shared` | `ensureOpen`, `failAll`, `UnloadedCore`: `UndraTransportException(CLOSED)` | `#assertOpen`, `#dispose`: `UndraTransportError("closed")` | `Unavailable` |
| connection lost, remote timeout, interrupted | `closedException(cause)` (`CONNECTION_LOST`), `blockingCall` (`TIMEOUT`, `INTERRUPTED`); a plain `UndraException` from `RemoteTransport` | remote transport: `UndraTransportError("closed" / "timeout")` | `Unavailable` |
| remote core changed schema | `UndraSchemaMismatchException` | `UndraSchemaMismatchError` | `Unavailable` |
| wasm core trapped | n/a | `WasmMainTransport`: `UndraTransportError("trap")`, core closed | `Unavailable` |
| malformed reply bytes | in-process sync: `UndraProtocolException`; JNI callback: `onMalformed` (L2); reply for another call: `UndraProtocolException` | `UndraTransportError("protocol")` | `Malformed` |
| result, handle or stream item does not decode | `WireException` (every reader failure is one; `require` only guards constant arguments) | `WireError` ("nothing else is ever thrown for malformed input", `reader.ts`) | `Malformed` |
| null handle | sync: `ConnectedCore.construct`; async: generated check (L1) | `UndraCore.construct` (L1) | `Malformed` |
| stream ended by the core (flag 2, String) | `onStreamItem`: `UndraReplyException(ERROR)` | `#onStreamItem`: `UndraReplyError(1)` | `E` first (with a domain), then `CancelledByCore` / `Panicked`; undecodable: `Malformed` |
| observe fails (store `init` / `create`) | `observeAll`: closes the store, throws mapped | `_observeAll`: closes, rejects mapped (an observe timeout, `UndraError("observe")`, is `Malformed`) | `Unavailable` / `Refused` / `Malformed` |
| caller cancelled | `CancellationException` (passes `classify` and `Flow.catch` unchanged) | the signal's reason (not an `UndraError`, passes `classify`) | itself, never wrapped |
| store change does not decode | generated `apply`: decode, `finish()`, assign; `catch (Exception)` reports | generated `_apply` reports | skipped, reported |
| malformed change-set, failed port | `Mirror` / `PortRegistry` → `reportFromCore` (delivery thread) | `Mirror.onError` / `#portFailure` → `report` | reported |

Greps over `examples/playground/generated/{kotlin,ts}`, the nine bindgen goldens per language and the CLI golden for `!!`,
`error(`, `check(`, `require(`, `TODO(`, `IllegalStateException`, `IllegalArgumentException`, `throw new Error`,
`process.exit` and `throw <Type>`: the only throws are `WireException.InvalidTag` in decoders (mapped to `Malformed`),
`UndraPortException` in port adapters (the host side, by design) and the mapped rethrows. The runtimes' `require`s guard
constant arguments (`minItemBytes`, `methodId == target.methodId`, mirror bounds) and host-side encoding (unpaired
surrogates, negative lengths), never reply data. `Error`s (`OutOfMemoryError`, `StackOverflowError`, `LinkageError`) are
not caught on Kotlin, by decision.

**Commands.** Kotlin: the whole body, encoding included, is `try { ... } catch (e: Exception) { core.report(e, op) }`;
`report` catches what the handler throws (an `Error` propagates, documented as the debug-trap hook). TypeScript: the whole
`async` body is one `try`/`catch` calling `report`, which constructs its values from `instanceof` checks and getters that
do not throw, logs through a guarded sink and contains the handler; after L4 nothing in it can throw, so the promise cannot
reject and `onClick={() => void todos.toggle(id)}` cannot produce an unhandled rejection. `onError` fires exactly once per
failed command: the runtime itself reports nothing on that path (`release` on a closed core is silent), and the generated
`_apply` reports its own failures so the mirror's catch never sees them. M1 closed the one way a single failure turned
into unboundedly many.

**R6 verdict, Kotlin and TypeScript.** R6 holds on both after this review. Every boundary failure that reaches generated
Kotlin ends as the method's own `E`, `CancellationException` or an `UndraCallError` case, and every one that reaches
generated TypeScript as `E`, the abort reason or an `UndraCallError` case; the one shape that cannot throw (commands, store
`apply`, failed ports and malformed change-sets) hands an `UndraUnhandledError` to `onError` after an error-level log and
returns, on Kotlin without throwing into a click handler and on TypeScript with a promise that never rejects. What remains
outside the set is deliberate and documented: a value the wire cannot represent passed to a non-command call (a programming
error, as Swift's preconditions are), a JVM `Error`, and a type with no finite value, which no core can hold (I8). The two
holes found here, a placeholder that threw when a store was created (M2) and a reporting loop that froze the page (M1), are
fixed and tested.

## Attack 2: mapping parity

| Input | Swift `mapped` | Kotlin `classify`/`fromStatus` | TypeScript `classify`/`fromReply` |
|---|---|---|---|
| platform cancellation | itself | itself | itself |
| already `UndraCallError` | itself | itself | itself |
| status 1 + domain | `E`, else `.malformed` | `E`, else `Malformed` | `E`, else `Malformed` |
| status 1 without domain, 0, 4 | `.malformed` | `Malformed` | `Malformed` |
| status 2 | `.panicked(message, backtrace)` | `Panicked(panicMessage, backtrace)` | `Panicked(panicMessage, backtrace)` |
| status 3 | `.cancelledByCore` | `CancelledByCore` | `CancelledByCore` |
| status 5 | `.refused(reason)` (core text) | `Refused(reason)` | `Refused(reason)` |
| transport | `.unavailable` | `Unavailable` | `Unavailable` (`"protocol"`: `Malformed`) |
| protocol, wire | `.malformed` | `Malformed` | `Malformed` |
| mode error | `.refused` | `Refused` | `Refused` |
| schema mismatch (remote) | `.unavailable(.connectionLost)` | `Unavailable(CONNECTION_LOST)` | `Unavailable` (reason `closed`, I7) |
| foreign error | itself | itself | itself |

Stream mapping: all three try `E` first and then the core's String, `"cancelled: "` prefix → `CancelledByCore`, anything
else → `Panicked(text, "")`, undecodable → `Malformed`. The hook for ADR-036 is single per runtime: flag 3 becomes one
`UndraReplyException(status, body)` / `UndraReplyError` built in `ConnectedCore.onStreamItem` / `UndraCore.#onStreamItem`,
and `classify` maps it like a failed reply; the String branch (`fromStreamItem`) stays only for older cores. (Before L2
the Kotlin in-process transport also produced flag-2 Strings for malformed items; it no longer does.)

## Attack 3: TypeScript `snapshot()` / `restore()`

Semantics match Swift and Kotlin: same handles after a restore (ADR-022), the restore re-delivers every observed signal
(ADR-023) and `restore()` resolves only after the mirror shows them (`mirror.flush()` after the transport resolves; the
test `snapshot, mutate, restore: the same handle shows the snapshot's value as soon as restore resolves` runs in both modes
with a frame scheduler that never fires on its own, so only that flush can have applied them). A refused snapshot rejects
`UndraRestoreError(code)` and leaves the core usable (`a snapshot that is not one is refused ...`, both modes, real core).
In worker mode the request and its ack are control messages; the worker posts `restored` through `post()`, which flushes
the batch first, so the change-sets and the cancelled calls' replies are handed to the host's handler before the ack is
read; ADR-031's frame drain cannot run in between (one thread), and the explicit flush makes the outcome independent of
it (`restore resolves after the change-sets and the cancellations it produced were delivered, in that order`). A snapshot
crosses modes. Field name and protocol version: I1.

## Attack 4: worker mode

The fix is right: in `crates/undra-ffi/src/wasm.rs` only a `port_call` answer of 2 falls through to `builtin::answer`;
answering 1 (async) for Clock left `port_call_sync` without a reply and the infallible proxy panicked into a trap. The
worker now answers 2 for Clock, Rng and Log and posts nothing (I9 for what serves them). The three other fixes each have a
test that fails on the old code: I ran `test/worker-ports.test.ts` against `main`'s `worker.ts` and `wasm-worker.ts` in a
scratch copy of the runtime, and 9 of its 16 cases fail there, including `every envelope the worker posts, before
\`ready\` too, carries the schema hash`, `a port call the core makes while it initialises is answered through the main
thread` (2 s timeout instead of an answer) and `a protocol failure before \`ready\` fails the start at once` (5 s timeout).
The sync-port warning: L3.

## Attack 5: recursive records (Swift)

The containment graph (`Cycles`) adds an edge only for a field (or payload field) of type `B` or `Option<B>`; `Vec`, `Map`
and `Bytes` add none, which is right for Swift (`[Tree]` is a heap buffer). The generated wire and `Codable` code handle
`Vec<Self>` unchanged (`Tree` round-trips on the wire and through JSON in `recursive.swift`). Mutual recursion
(`Parent`/`Child`) boxes both fields on the cycle; a record/enum cycle (`Group`/`Expr`) boxes the record field and makes
the enum `indirect`; a self-recursive enum or error is `indirect`. The public shape stays a plain optional, the box is
immutable and replaced on every set, so value semantics, `==`, hashing, `Sendable` and the JSON shape are a plain
optional's; `recursive.swift` checks each, `typecheck_swift.rs` compiles every golden case against the real runtime in
Swift 6 mode. Non-recursive Swift goldens are byte-identical to `main` (`git diff main --name-only` lists Swift files
only under `golden/recursive`). The recursive-first-variant enum: M2.

## Attack 6: contracts

S05.6, S15.9, S15.10, S16.5 and S17.1/2/5/6 assert the closed-set case by type on Kotlin and TypeScript (and Swift), not
only "some error"; S15.10 checks that the core's `"cancelled: ..."` String is `CancelledByCore` on all three; S17 wasm step
6 runs the playground core in worker mode through a query (Clock), a mutation (Rng) and the core's log. Kotlin's
re-entrancy change does not weaken ADR-020/ADR-023's guarantee: `checkNotInCallback` still runs before every `native.*`
entry, so a call from a core callback never reaches the core and cannot deadlock; only the exception changed, from a plain
`UndraException` (which `classify` would have read as a lost connection) to the reply the core itself sends, `Refused`
with `E_REENTRANT`. `PortTests` still proves the sync-port case is told off, not deadlocked.

## Attack 7: the "still different" list

Fixed here: nothing on that list was small and local (I2 touches Swift's callback threading, I3 is a public API decision
with PA-8). Follow-ups: I1 (A7/ADR-049), I2 (Swift `onError` for ports and change-sets), I3 (PA-6/7/8, ADR-034, B2), I8
(validation of types with no finite value).

## Verification (after the fixes)

| Check | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` with `UNDRA_REQUIRE_TOOLCHAINS=1`, `tsc`, kotlinc and the Kotlin jars on `PATH` (typecheck_kotlin, typecheck_swift, typecheck_ts, run_ts ran, none skipped) | 2,255 passed, 0 failed, 10 ignored (2,250 at `d32f60a`, also run here first; 5 new) |
| Kotlin runtime `scripts/test-local.sh` | 528 cases in 32 suites, 0 failed, 2 skipped (native smoke, as before; 1 new) |
| TypeScript runtime `npm test` / `npm run typecheck` | 1,010 tests (4 new) / clean |
| `swift test` in `runtimes/swift/UndraRuntime` | 425 tests, 0 failures |
| `bash crates/undra-ffi/tests/wasm/run.sh` | `raw.test.mjs` 19/19, `ts-runtime.test.mjs` 24/24, none skipped |
| `bash contract-tests/run-all.sh` | 54/54 (S01 to S18 on ts, kotlin, swift) |
| `contract-tests/ts` `npm run typecheck` | clean |
| playground web `npm test` / `npm run build` | 102 tests / built |
| Android `./gradlew :app:assembleDebug` | BUILD SUCCESSFUL (the composite build recompiled the runtime and the app) |
| `undra bindgen -C examples/playground --check` | up to date (schema hash `0xabdf844b53e0bc10`) |
