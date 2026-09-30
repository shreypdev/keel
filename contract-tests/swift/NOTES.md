# Swift contract runner: notes

`run.sh` runs the seventeen scenarios of `../scenarios.md` for Swift (`KeelRuntime` over the C ABI,
the real playground core through `libkeel_core.dylib`, the bindings `keel bindgen` generated) and
pipes the `SCENARIO` lines through `../check.sh swift`.

## Layout

| Path | What |
|---|---|
| `Tests/ContractTests/Harness/` | the fakes of scenarios.md: `ManualClock`, `FakeServer` (the `Http` port), `MemoryKv`, `CapturingLog`, and `Fixture` (the one core of the process and its adapters) |
| `Tests/ContractTests/ContractScenarios.swift` and `S*.swift` | one XCTest per scenario, `testS07_streamWithBackpressure` and so on, in one class so that XCTest's alphabetical order is the order of the ids |
| `Tests/ContractTests/Findings.swift` | minimal repros of defects found in merged code (`XCTExpectFailure`: they pass while the defect exists and fail the day it is fixed) |
| `Packages/PlaygroundCore` | a symlink to `examples/playground/generated/swift`, see below |

## Why the generated package is a symlink

SwiftPM names a path dependency after its directory. `examples/playground/generated/swift` and this
package (`contract-tests/swift`) are both called `swift`, which SwiftPM treats as one package
("product 'PlaygroundCore' not found in package 'swift'"). The symlink gives the dependency another
name. It sits four directories below the repository root on purpose, so that the generated
package's own relative path to the runtime (`../../../../runtimes/swift/KeelRuntime`) still resolves
to the same package as the one this manifest names (otherwise SwiftPM warns of conflicting
identities, "will be escalated to an error").

## Deviations from scenarios.md

* **S07 reads `KeelCore.stream`, not the generated `Probe.ticks`, for steps 1 to 4.** The generated
  method copies the runtime's pull-based stream into an unbounded `AsyncThrowingStream` from a task of
  its own, so the consumer applies no backpressure: after reading 5 of 1,000 items and waiting 200 ms
  `produced` is 1,000, not at most 69. (`Findings.swift` has the repro; `WORKAROUND(keel-bindgen)` in
  the scenario.) Step 5 (short streams) uses the generated method. Step 4 compares `produced` against a
  reading taken at its start instead of resetting the probe.
* **S14.6.** The core empties the offline queue by *deleting* the key `keel.query.queue`, it does not
  write an empty queue. The scenario accepts either as "emptied".
* **S16.1 "before the core is initialised".** The Swift runtime calls `keel_init` first and compares
  the hash after (`InprocTransport.start`), then shuts the core down again. The scenario checks what a
  caller can see: `KeelSchemaMismatchError` with `expected` and `got`, hex in the message, no shared
  core left behind, and a later load that works. `Findings.swift` pins the order (with the core
  initialised elsewhere the load fails with `coreInitFailed`, not with the mismatch).
* **S16 shuts the shared core down first** (a load while another core is loaded throws
  `alreadyLoaded` before the hash is compared) and loads a fresh one at the end, so it is ordered
  before S17 and it is the only scenario that does this. S16.4, which scenarios.md lists for
  TypeScript and Kotlin, is also checked here, through `keel_schema_json`.
* **S17.1 uses the raw `KeelCore.callSync` for `explode`.** The generated sync binding treats a panic
  reply as "the core and the bindings disagree" and stops the process on purpose (`keelUnexpected`).
  The async `explodeLater` is called through the binding and throws `KeelReplyError`.
* **S03.2** ("no suspension") is shown by compilation: the sync calls are made from a plain, non-async
  function (`plainSyncCalls`).
* **S06 cancels with `Task.cancel()`** and expects `CancellationError`.
* **S08.1/S09.1 "before observe returns".** `KeelCore.observe` applies the initial change-set to the
  mirror before it returns, so the raw checks read the entries synchronously after `observe()`.
* **S10.8** runs on a fresh `BigList` (after the earlier steps the list has no single missing first item).
* **S15.5.** `Todo.id` is a `UUID` whose first eight bytes are the core's counter, so "an id above
  `b`'s" is compared byte-wise (`KeelUUID` is `Comparable`).

## Findings in merged code

1. keel-bindgen (Swift): the generated stream method ignores backpressure (above).
2. KeelRuntime: the in-process load initialises the core before comparing schema hashes (above).
3. keel-bindgen (Swift): with Swift 6.3, every generated store warns `class 'Todos' must restate
   inherited '@unchecked Sendable' conformance` (five warnings; `KeelStore` is `@unchecked Sendable`,
   the generated `@MainActor @Observable` subclasses do not say it again).
4. Generated bindings and runtime: the standard-port records (`HttpRequest`, `HttpResponse`,
   `Header`, `NetKind`) are internal in `KeelRuntime` and not generated, so an app that supplies its own
   `Http` adapter has to write their wire layout by hand (`@testable` here, a small codec in the app).
