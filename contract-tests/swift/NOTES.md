# Swift contract runner: notes

`run.sh` runs the scenarios of `../scenarios.md` that Swift runs (S01 to S20, S23 to S26, S29 and S30: `UndraRuntime` over
the C ABI table, the real playground core through `libplayground_core.dylib` and, for S26, `libplayground_a.dylib` and
`libplayground_b.dylib` in the same process, the bindings `undra bindgen` generated) and pipes the `SCENARIO` lines
through `../check.sh swift`. The build-B steps of S14 and S15 run in a second process over the second build of the
core (see "Two builds" below).

## Layout

| Path | What |
|---|---|
| `Tests/ContractTests/Harness/` | the fakes of scenarios.md: `ManualClock`, `FakeServer` (the `Http` port), `MemoryKv` (records every operation, fails on demand with a `StorageError`), `CapturingLog`, `PortCalls` (`CountingAdapter`, which counts the calls each adapter receives), `RealtimeServer` (starts `../servers/realtime-server.mjs` with Node for S23 and S24 and reads its `/stats`), `Fixture` (the one core of the process, its adapters and its `LoadOptions`) and `Handover` (what build A leaves for the build-B process) |
| `Tests/ContractTests/MigrationBuildB.swift` | not a scenario of its own: the build-B steps of S14 (8, 9) and S15 (12 to 14), run by `run.sh` in a second process and skipped in the main run |
| `Tests/ContractTests/ApplyReportTests.swift` | not a scenario: a generated store skips a change it cannot decode and reports it through `onError` (ADR-032, decision 6); it runs before the scenarios, on the core they share |
| `Tests/ContractTests/ContractScenarios.swift` and `S*.swift` | one XCTest per scenario, `testS07_streamWithBackpressure` and so on, in one class so that XCTest's alphabetical order is the order of the ids |
| `Packages/PlaygroundCore` | a symlink to `examples/playground/generated/swift`, see below |

## Why the generated package is a symlink

SwiftPM names a path dependency after its directory. `examples/playground/generated/swift` and this
package (`contract-tests/swift`) are both called `swift`, which SwiftPM treats as one package
("product 'PlaygroundCore' not found in package 'swift'"). The symlink gives the dependency another
name. It sits four directories below the repository root on purpose, so that the generated
package's own relative path to the runtime (`../../../../runtimes/swift/UndraRuntime`) still resolves
to the same package as the one this manifest names (otherwise SwiftPM warns of conflicting
identities, "will be escalated to an error").

## Two builds (S14 steps 7 to 9, S15 steps 11 to 14)

`run.sh` builds the core twice: build B first (`UNDRA_PLAYGROUND_V2=1 undra build --platform host`, which
the core's `build.rs` turns into `cfg(playground_v2)`; the CLI rebuilds when only that variable changes),
staged in `.build/core-b`, then build A, staged in `.build/core` where `Package.swift` links it. The script
stops if the two libraries are identical.

The main `swift test` runs S01 to S20 against build A. S14 step 7 writes `.build/migration/s14.json` (every
key and value of the harness `Kv` once both notes wait in the queue, and the `Idempotency-Key` of the failed
`save_note` POST); S15 step 11 writes `s15.json` (snapshots `P` and `L` and the `Profile` handle). Each
scenario deletes its file first, and `run.sh` deletes the directory before the run, so a failed scenario
never hands over an older run's data.

Then `run.sh` copies build B's library over `.build/core/libundra_core.dylib` and runs `swift test
--skip-build --filter MigrationBuildB` with `UNDRA_CONTRACT_PHASE=B`: the same test bundle, relaunched,
now loading build B. `MigrationBuildB` refuses to run against build A (it compares `undra_schema_hash()`
with the generated hash), loads build B with the hash it reports and with a fresh `ManualClock`,
`FakeServer`, `CapturingLog` and a `MemoryKv` holding exactly the handed-over contents (and no injected
failure), emits `Connectivity.changed(false, None)` right after the load, and drives the core through
`UndraCore`'s raw API (`configure_remote`, `storage_status`, `add` and `Profile.describe` by `fnv1a32` id;
the generated `StorageStatus` only as a codec). It prints `SCENARIO S14 FAIL` / `SCENARIO S15 FAIL` lines
when a build-B step fails and an informational `MIGRATION S14 build B ok: <the dead letter>` /
`MIGRATION S15 build B ok` otherwise; its output is appended to `.build/contract.log`, which `check.sh`
reads (the last line of an id counts). Build A's library is put back afterwards, also when something
fails. A filtered `run.sh` (any arguments) skips this phase.

## S29 and S30 (ADR-046)

`Fixture` loads every core with `onPanic` and the default `DiagnosticsAdapter`; `onPanic` records each `UndraPanicReport` with whether it was
delivered on the main thread (`Fixture.panics`). S29 reads it; its step "a reporter that throws" is Kotlin's and TypeScript's (a Swift
`onPanic` cannot throw). S30 drives `core.runInBackground(deadline:)` against the harness `Http` and `Kv` and reads
`stats().background`; its counters are checked as deltas, since the core may have counted runs before.

## Deviations from scenarios.md

* **S07 reads the generated `Probe.ticks` throughout.** The generated method returns
  `UndraCore.stream(..., decode:)`, which pulls an item from the core when the consumer asks for the next
  one, so the credit follows the consumer. Besides the scenario's bound (`produced` at most 69 after reading 5
  and waiting 200 ms) the Swift column checks the runtime's own window: at most 16 items sent plus the one
  the producer made and holds for credit. Step 4 compares `produced` against a reading taken at its start
  instead of resetting the probe.
* **S07.6 and S07.7 use the probe of steps 1 to 5** (step 7's restore invalidates it; the scenario's
  `defer` closes it). Step 6 reads `ticksThenFail` through the generated `mapError`
  (`UndraCallError.mapped(streamFailure:domain: LabError.self)`). In step 7 the items the core had sent
  before the restore (up to the credit window) are still read, in order, before the loop fails with
  `UndraCallError.cancelledByCore`; the runner checks they continue the sequence and does not bound their
  number.
* **S14.6.** The core empties the offline queue by *deleting* the key `undra.query.queue2`, it does not
  write an empty queue. The scenario accepts either as "emptied". The Swift column also checks that the
  first non-empty write of the queue holds one item and that the `undra.types.<fingerprint>` key of that
  item's fingerprint (the `u64` at offset 18) was written before it.
* **S14.7 replays build A's notes after the handover.** Once the `Kv` contents and the idempotency key are
  written to the handover file, the runner answers the notes' POST with 201 and goes online, so both notes
  replay in build A and the queue is empty for the rest of the run (the reloads of S16 to S18 would
  otherwise hydrate and retry it, and S20's failing writes would meet it). Build B starts from the contents
  kept before that.
* **S20 runs its native variant**: steps 1, 2, 4 (the harness failed the first `get` of `undra.query.queue2`
  with `Locked` when the process loaded its first core; the `Kv`'s operation log survives the reloads of S16
  to S18) and 5; step 3 prints nothing. Step 1 also checks that a write of the `s20` entry was attempted and
  failed `Full`. The `s20` entry's key is computed (`undra.query.cache2.<query id>.<fnv1a64 of the encoded
  arguments>`). S20 is the last scenario of the main run.
* **S16.1 "before the core is initialised".** `InprocTransport.start` reads `undra_schema_hash()` (which
  needs no running core) and compares it before it claims the process or calls `undra_init`. The scenario
  checks what a caller can see (`UndraSchemaMismatchError` with `expected` and `got`, hex in the message, no
  shared core left behind, and a later load that works), and step 1b makes "before" observable: with the
  core initialised by another embedder (`undra_init` called directly), `load(wrong hash)` still throws the
  mismatch, where a runtime that initialises first would fail with `coreInitFailed`.
* **S16 shuts the shared core down first** (step 1b needs a process in which no core is
  initialised, and `undra_init` is once per process) and loads a fresh one at the end, so it is ordered
  before S17; it is the only scenario that starts by shutting the core down. S16.4, which scenarios.md lists for
  TypeScript and Kotlin, is also checked here, through `undra_schema_json`.
* **S17.5 is triggered from the Log adapter.** The core logs the panic of `explode("reenter")` through the
  harness's `CapturingLog`, a synchronous port, on the thread that runs the call and holds the core's
  lock; `CapturingLog.onNextRecord(where:run:)` runs the generated `add(1, 1)` from there, once. The call
  fails as `UndraCallError.refused` naming `E_REENTRANT`, which is also the proof that the hook runs on that
  thread (a thread that did not hold the lock would have been answered normally). No `ManualClock` fallback
  was needed.
* **S17.6 and S17.7 end the core and load a new one, but S17 is not the last scenario XCTest runs.**
  XCTest runs the tests in alphabetical order, so S18 runs after S17; it finds the shared core shut down
  and `Fixture.core()` loads the core a third time, which exercises "a new load starts fresh" once more.
  S17.7's own load uses `Fixture.loadOptions()` directly (not `Fixture.core()`, which also calls
  `configureRemote`), so `live_handles == 0` is read on a bare core, and it shuts that core down again.
* **S17.7 counts every adapter.** `Fixture.makeAdapters()` wraps each adapter in a `CountingAdapter`, so
  the calls to `Rng` and `Timer` are counted too, besides the scenario's Clock, Log, Http and Kv; none
  may arrive. The generator (`Stress.start(mode: .firehose, perSecond: 1_000)`) is shown running before
  the shutdown by its Clock calls and, with the manual clock moved on by a second, by `generated > 0`. The
  200 ms window is measured from the moment `shutdown()` returned (the readings are taken then), so it
  includes step 6's checks. The window alone cannot show a task of the old core that kept running (the
  shutdown retires the port registrations and detaches the adapters first, so its calls are answered
  "unavailable" and its host timers never fire), so the step also reads `undra_stats_json()` with no core
  loaded and requires `runtime_threads == 0` after each shutdown (review of runtime-lifecycle: a mutant
  whose shutdown only released the global slot passed the window and the reload; it fails here).
* **Commands are asserted through `onError`.** A generated command (a synchronous method that returns
  nothing and has no error type: `Counter.increment()`, `Probe.reset()`) does not throw; it reports to
  `LoadOptions.onError` (ADR-032). `Fixture` installs a handler that records every `UndraUnhandledError`
  in `Fixture.shared.unhandled`, and S05.6, S15.9 and S17.6 compare counts before and after, the way the
  scenarios compare statistics.
* **S03.2** ("no suspension") is shown by compilation: the sync calls are made from a plain, non-async
  function (`plainSyncCalls`).
* **S06 cancels with `Task.cancel()`** and expects `CancellationError`.
* **S08.1/S09.1 "before observe returns".** `UndraCore.observe` applies the initial change-set to the
  mirror before it returns, so the raw checks read the entries synchronously after `observe()`.
* **S10.8** runs on a fresh `BigList` (after the earlier steps the list has no single missing first item).
* **S15.5.** `Todo.id` is a `UUID` whose first eight bytes are the core's counter, so "an id above
  `b`'s" is compared byte-wise (`UndraUUID` is `Comparable`).

* **S23, S24 and S25 run on the runtime's default adapters**, as scenarios.md asks: `URLSessionWebSocketAdapter`
  and `URLSessionSseAdapter` against `../servers/realtime-server.mjs` (started once, on first use, by `Fixture.realtime()`;
  it exits with this process through `--exit-on-stdin-close`), and `SQLiteDbAdapter` rooted in a fresh temporary
  directory (`Fixture.databases`, deleted when S25 ends). They run last, on the core S18 loaded.
* **S23.3 "pulls is at most 2" holds because the Swift binding answers a burst at once.** URLSession hands a burst to
  the binding one `receive()` at a time, tens of microseconds apart, as fast as the core pulls; answering each pull
  with the one message that happened to be there made `read(5)` cost five or six pulls. The binding's pull therefore
  waits for a burst that is still arriving (it answers once `max` messages are there, once no message arrived for
  2 ms, at the latest 8 ms after its first message, or at the stream's end), so the first pull of `read(5)` gets 16.
  This is ADR-047 §3's "a burst of 16 frames is one crossing"; a lone message waits 2 ms
  (`runtimes/swift/UndraRuntime/Sources/UndraRuntime/Ports/PulledInbox.swift`).
* **S24.3 `sse_follow(HTTP/sse/hang, nil, 0)`** never polls its stream, so the request goes out when `close()` finishes
  the pending open (crates/undra-ports/src/sse.rs); the runner waits up to 1 s for the server's `clientClosed`.
* **S25.2 "a keyed patch each" is checked on a second add.** The first note added to an empty list reaches the mirror
  as a full value, not a keyed patch: `Notes.add` writes with `Signal::update`, which the core diffs against the
  host's baseline, and the diff sends the full value when no key overlaps (crates/undra-signals/src/store.rs, the
  doc of the keyed baseline). The runner checks the generated store's notes after each add, and the op on a raw
  store over `":memory:"`: the second add onto a one-note list is a keyed patch. (Reported to the integrator: the
  scenario text promises a patch for the first add too.)

## Findings in merged code

Fixed since the first run (playground findings 3, 4 and 6): the generated stream methods buffered without
bound (S07 now reads them), the in-process load initialised the core before comparing schema hashes (S16
step 1b), and generated stores warned about a missing `@unchecked Sendable` under Swift 6.3.

Fixed with S17.7: the harness's `ManualClock` answered `Clock.monotonic_ns` in milliseconds (the manual
reading since the start, not multiplied by 1,000,000). Nothing read it until the `Stress` generator,
which then never earned an update; it now answers in nanoseconds, as the Kotlin and TypeScript clocks do.

Still open:

1. Generated bindings and runtime: the standard-port records (`HttpRequest`, `HttpResponse`,
   `Header`, `NetKind`) are internal in `UndraRuntime` and not generated, so an app that supplies its own
   `Http` adapter has to write their wire layout by hand (`@testable` here, a small codec in the app).
* **S15.10 and S16.5 (ADR-032 amendment A) are new coverage of Swift behaviour that did not change.** S15.10 opens
  `probe.ticks(count: 1_000_000)`, reads one item, restores, and expects the loop to end with `.cancelledByCore` (not a
  `CancellationError`). S16.5 runs right after S16's failing load, with no core loaded: `UndraCore.shared` is the closed
  placeholder, so `PlaygroundCore.add` and `Counter()` with the default `ctx` fail `.unavailable(.closed)`, a report on it only
  logs, and `UndraCore.current` stays `nil`; S17.6 ends by checking the same after `shutdown()`.
