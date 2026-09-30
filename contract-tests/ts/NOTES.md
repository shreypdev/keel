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

Fixed since (playground finding 5): the Mirror stranded a change-set enqueued from a signal subscriber during the flush; the flush now drains it in a further round (`runtimes/ts/@keel/runtime/test/mirror.test.ts`).

1. **`KeelCore` has no `snapshot()` / `restore()`** (SPEC 17.1 lists none). The runtime implements
   `Kind.Restore` in `WasmMainTransport.send` and the core exports `keel_snapshot`, but an app holding only a
   `KeelCore` cannot use either. The scenarios reach the transport; an app would have to as well.
2. **Generated stores cannot adopt an existing handle.** After a restore in a fresh core the handles of the
   old core are alive again, but `Todos.create()` always constructs a new one and the constructor is private,
   so there is no supported way to put a store class on a restored handle. S17 casts around the private
   constructor (`adoptTodos`, the one place that does).
3. **A failed mutation's rollback removes the placeholder of a later mutation** (keel-query, SPEC 9: "the
   pre-mutation entries are restored"). The restore is a snapshot taken before the mutation ran; a mutation
   that started after it has already put its own optimistic item into the entry, and the restore takes it
   away while its request is in flight (offline and queued, it stays gone until the replay succeeds). The web
   app hit it by accident: Offline turned on while a toggle's PATCH was in flight, then an add. Repro:
   `test/findings.test.ts`, expected-fail. The flow is: toggle (PATCH, 300 ms) then, before it answers, add; the
   PATCH fails; the add's item disappears. A rollback that undoes only its own change would not.
4. **A keyed patch costs O(list) in the core, not O(change)** (keel-runtime / keel-signals, blueprint section 14:
   "lists must be O(change)", web budget for a one-insert patch on 10,000 items: 30 us). S10 prints the numbers
   (node 24, the release-wasm build): `update_at` on the 10,000-row list takes 5 us with nobody observing the store
   and about 1.26 ms with an observing store; the TypeScript side of it (decode, apply to the 10,000-item copy,
   announce) is about 15 us of that. The time is the core computing the patch by comparing the old and the new
   list. The 10k tab of the web app shows the same: about 1.2 to 1.8 ms from click to re-rendered window.
