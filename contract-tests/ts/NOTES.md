# Notes on the TypeScript column

`run.sh` runs S01..S17 of `../scenarios.md` against the real wasm build of the playground core
(`examples/playground/build/web/keel_core.wasm`, built by `keel build -C examples/playground --platform web`)
through `@keel/runtime` in `wasm-main` mode, on Node, under vitest. `src/reporter.ts` prints one
`SCENARIO Sxx PASS|FAIL|SKIP <title>` line per scenario; `../check.sh ts` grades them. `NOTE` lines carry
measurements (S03: ns per sync call; S07: how far the producer ran).

## How it is built

* Every scenario boots its own core (`src/harness.ts`): a fresh wasm instance of the module compiled once per
  file. scenarios.md describes the native platforms, where `keel_init` is once per process and scenarios share
  a core and isolate themselves with list names; nothing here depends on that, and the list names are kept so the
  three columns read the same. Scenarios still compare **deltas** of the statistics, never absolute values.
* `boot()` uses `KeelCore.load({ mode: "wasm-main" })`, the call an app makes. `bootRaw()` builds the
  `WasmMainTransport` itself and `KeelCore.attach`es to it (what `load` does for this mode) so S15, S16 and S17
  can reach the wasm exports (`keel_snapshot`, `keel_schema_json`, `keel_schema_hash`) and send `Kind.Restore`.
* Adapters (`src/`): `ManualClock` (starts 1,700,000,000,000 ms), `FakeServer` (routes by method and exact URL,
  records requests, delays, network errors, 404 otherwise), `MemoryKv` (remembers every call), `CapturingLog`.
  `Rng` and `Timer` are the runtime defaults (`crypto`, `setTimeout`). `Connectivity` is emitted by the test with
  the runtime's `emitConnectivity`; the event sources are off.
* `RawStore` is a store without its generated class: `core.construct`, `core.mirror.register`, `core.observe`,
  the entries the core delivered, op codes and all. S08, S09 and S10 count entries with it.

## Where a scenario is read, not copied

* S01: `span` is the TypeScript `Duration` (milliseconds; 1.500000123 s is `1500.000123`), compared as the
  nanoseconds it is on the wire. The fields scenarios.md does not name in the extremes step take their own
  extremes (`byte` 0, `dword` 4294967295, `at` the `Date` minimum, ...). S01.4 also checks that a returned blob
  is not a view into wasm memory (a later call must not reach back into it).
* S07.4: "`produced` stays below 200" is read as the growth since the stream was opened: the counter is
  cumulative (1000 after step 3) and `reset()` is not part of the step.
* S12.1: scenarios.md's `loading` is the core's `QueryStatus.fetching`; the status history is exactly
  `fetching`, `success`.
* S13: "records every value of `data`" subscribes to `handle.data`; a value that arrives twice in a row counts
  once and the `null` before the first fetch is not a list. A `Signal` announces once per mirror flush, and every
  step of S13 is at least 50 ms from the next, so no value is coalesced away.
* S14.6: the core deletes `keel.query.queue` when the queue empties, it does not write an empty queue; "the last
  write is an empty queue" accepts either a `delete` or a `set` whose queue has zero entries. The queue written
  while offline is decoded, and its `Idempotency-Key` is the one both POSTs carried.
* S15.5: the `Uuid` ids of `Todos` count up in their leading bytes, so "above `b`'s" is the string order of the
  canonical form. S15.7: a released handle answers a bad request (stale handle).
* S16.1: "before the core is initialised" is checked through what initialisation does: a core that ran
  `keel_init` reads its cache through the `Kv` port (step 2 shows it); the refused one never touched `Kv` and
  logged nothing.
* S17: the stores of the restarted core are built over the handle the restore brought back with
  `adoptTodos` (see the gap below); the restart itself is `bootRaw()` plus `Kind.Restore`.

## Gaps and defects found (for the integrator)

1. **`KeelCore` has no `snapshot()` / `restore()`** (SPEC 17.1 lists none). The runtime implements
   `Kind.Restore` in `WasmMainTransport.send` and the core exports `keel_snapshot`, but an app holding only a
   `KeelCore` cannot use either. The scenarios reach the transport; an app would have to as well.
2. **Generated stores cannot adopt an existing handle.** After a restore in a fresh core the handles of the
   old core are alive again, but `Todos.create()` always constructs a new one and the constructor is private,
   so there is no supported way to put a store class on a restored handle. S17 casts around the private
   constructor (`adoptTodos`, the one place that does).
3. **Mirror strands a change-set enqueued from a signal subscriber** (`runtimes/ts/@keel/runtime/src/mirror.ts`).
   `flush()` keeps `#flushing` set until `batch()` returns, and `batch()` notifies subscribers as it returns, so
   a subscriber that makes a synchronous core call enqueues its change-set while `#flushing` is true: no flush
   is scheduled and nothing looks at the queue again until an unrelated change-set arrives. Minimal repro:
   `test/findings.test.ts` (an expected-fail test, so it turns red when the runtime is fixed). Fix: re-check
   the queue after `#flushing` is cleared. It does not affect a scenario (S04.4 only needs the observer's own
   call to resolve).
