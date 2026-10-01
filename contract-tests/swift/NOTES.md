# Swift contract runner: notes

`run.sh` runs the twenty scenarios of `../scenarios.md` for Swift (`UndraRuntime` over the C ABI
table, the real playground core through `libplayground_core.dylib` and, for S26, `libplayground_a.dylib` and
`libplayground_b.dylib` in the same process, the bindings `undra bindgen` generated) and
pipes the `SCENARIO` lines through `../check.sh swift`.

## Layout

| Path | What |
|---|---|
| `Tests/ContractTests/Harness/` | the fakes of scenarios.md: `ManualClock`, `FakeServer` (the `Http` port), `MemoryKv`, `CapturingLog`, `PortCalls` (`CountingAdapter`, which counts the calls each adapter receives), and `Fixture` (the one core of the process, its adapters and its `LoadOptions`) |
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
* **S14.6.** The core empties the offline queue by *deleting* the key `undra.query.queue`, it does not
  write an empty queue. The scenario accepts either as "emptied".
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
