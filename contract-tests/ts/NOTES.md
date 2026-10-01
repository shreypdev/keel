# Notes on the TypeScript column

`run.sh` runs S01..S18 and S23..S25 of `../scenarios.md` against the real wasm build of the playground core
(`examples/playground/build/web/undra_core.wasm`, built by `undra build -C examples/playground --platform web`)
through `@undra/runtime` in `wasm-main` mode (S17 step 6 in `wasm-worker` mode), on Node, under vitest. `src/reporter.ts` prints one
`SCENARIO Sxx PASS|FAIL|SKIP <title>` line per scenario; `../check.sh ts` grades them. `NOTE` lines carry
measurements (S03: ns per sync call; S07: how far the producer ran).

## How it is built

* Every scenario boots its own core (`src/harness.ts`): a fresh wasm instance of the module compiled once per
  file. scenarios.md describes the native platforms, where `undra_init` is once per process and scenarios share
  a core and isolate themselves with list names; nothing here depends on that, and the list names are kept so the
  three columns read the same. Scenarios still compare **deltas** of the statistics, never absolute values.
* `boot()` uses `UndraCore.load({ mode: "wasm-main" })`, the call an app makes. `bootRaw()` builds the
  `WasmMainTransport` itself and `UndraCore.attach`es to it (what `load` does for this mode) so S16 and S17
  can reach the wasm exports (`undra_snapshot`, `undra_schema_json`, `undra_schema_hash`) and send `Kind.Restore`.
  S15 uses the public `core.snapshot()` / `core.restore()` through plain `boot()`.
* `bootWorker()` is `boot()` in `wasm-worker` mode, for S17 step 6: the worker is `runWorker` (the code of
  `@undra/runtime/worker`) served on one end of a `MessageChannel` in the test's own thread, because vitest cannot
  load TypeScript in a real worker thread; messages cross it as they would cross to a worker (structured clone,
  transferred buffers). Clock, Rng and Log are answered inside the worker, so the world's `ManualClock` is not
  used there; Http, Kv and the Log records still reach the world's adapters. The real worker thread runs in
  `crates/undra-ffi/tests/wasm/ts-runtime.test.mjs`.
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
* S05.6, S15.9, S15.10, S16.5 and the wasm S17.5 (ADR-032, amendment A) read the failure model of the generated bindings: a call
  rejects with `UndraCallError` (closed objects and stale handles are `Refused`, a call or stream in flight across a restore is
  `CancelledByCore`, a trapped core is `Unavailable` with transport reason `trap` or `closed`), a command (`Counter.increment()`,
  `Probe.reset()`) resolves and reports to `onError` (the harness's `runtimeErrors`, which a scenario that causes a report
  asserts and clears; an unasserted report fails the scenario), and a cancelled typed call (S06.6) rejects with the signal's
  reason. The raw `UndraCore.call` of S05.4 and S15.7 still rejects with `UndraReplyError`.
  The native S17.5 (re-entry) and S17.6 (shutdown) have no wasm counterpart: see the platform notes of scenarios.md.
* S07.4: "`produced` stays below 200" is read as the growth since the stream was opened: the counter is
  cumulative (1000 after step 3) and `reset()` is not part of the step.
* S07.6 and S07.7 (ADR-036): the generated `ticksThenFail` maps a failure with `UndraCallError.mappedStream(error, LabErrorCodec)`, so step 6's
  flag-2 item arrives as `LabError.Rejected` and step 7's flag-3 item (status 3) as `UndraCallError.CancelledByCore`
  (the runtime raises `UndraReplyError` with status 3 and the generated code maps it). Step 7 takes its snapshot with `core.snapshot()` and restores with `core.restore()`,
  as S15 does. The items the core had sent against credit before the restore are still delivered
  first (they continue `2, 3, ...` in order); "within 1 s" bounds the reads from the restore to the rejection.
* S12.1: scenarios.md's `loading` is the core's `QueryStatus.fetching`; the status history is exactly
  `fetching`, `success`.
* S13: "records every value of `data`" subscribes to `handle.data`; a value that arrives twice in a row counts
  once and the `null` before the first fetch is not a list. A `Signal` announces once per mirror flush, and every
  step of S13 is at least 50 ms from the next, so no value is coalesced away.
* S14.6: the core deletes `undra.query.queue` when the queue empties, it does not write an empty queue; "the last
  write is an empty queue" accepts either a `delete` or a `set` whose queue has zero entries. The queue written
  while offline is decoded, and its `Idempotency-Key` is the one both POSTs carried.
* S15.5: the `Uuid` ids of `Todos` count up in their leading bytes, so "above `b`'s" is the string order of the
  canonical form. S15.7: a released handle answers a bad request (stale handle).
* S16.1: "before the core is initialised" is checked through what initialisation does: a core that ran
  `undra_init` reads its cache through the `Kv` port (step 2 shows it); the refused one never touched `Kv` and
  logged nothing.
* S17: the stores of the restarted core are built over the handle the restore brought back with
  `adoptTodos` (see the gap below); the restart itself is `bootRaw()` plus `Kind.Restore`.

## The opt-in ports: S23, S24, S25 (ADR-047, ADR-048)

* The ports are registered the way an app registers them, one line each through `LoadOptions.ports`
  (`boot({ ports })` in `src/harness.ts`): `webSocketPort(nodeWebSocket())`, `ssePort(fetchSse())`,
  `dbPort(nodeSqliteDb({ directory }))` from `@undra/runtime/realtime` and `@undra/runtime/db`. S23 and S24 import
  `startRealtimeServer` from `../servers/realtime-server.mjs` in process (`src/realtime-server.ts`), one server per
  file; S25 roots its adapter in a fresh `mkdtemp` directory deleted after the file.
* Each file also runs its port once in `wasm-worker` mode (`bootWorker`, tests not named after a scenario, so they
  count in vitest's totals only): `wsEcho`, `sseFollow` and `dbCells` answered by the main thread's binding
  (ADR-049 §2).
* **S23 uses `nodeWebSocket()`, not Node's global `WebSocket`** (scenarios.md names the global). Measured on Node
  24.21 (undici 7.29) against the realtime server: the global `WebSocket` reports a refused upgrade as an `error` and a
  1006 close with no status (step 4's "status 401 where the platform reports it": it does not), throws
  `InvalidAccessError` for `close(1001)` and `close(1008)` (scripts may only send 1000 and 3000-4999, so step 5's 1001
  cannot be sent), and reports a text frame that is not UTF-8 exactly like a drop (1006, no close frame). The runtime's
  `nodeWebSocket()` (Node's `http` upgrade and the runtime's own RFC 6455 framing) reports `Refused(401)`, sends 1001,
  ends a bad text frame with `Protocol` (closing 1007), and stops reading the socket while the core is not pulling, so
  TCP pushes back on the server as on Swift and Kotlin. `browserWebSocket()` over the global runs the runtime's
  failure-injection suite with those three readings (`runtimes/ts/@undra/runtime/test/realtime-adapters.test.ts`),
  and its flood ends `Closed(1008, "the core did not keep up")` after 16 MiB as the brief says (the close frame
  itself carries no code where 1008 is refused to scripts).
* S23.3 reads `live.pulls()` as at most 2 (measured: 1). The binding answers a burst as one reply (a pull with fewer
  than `max` waits for `max`, 2 ms of quiet or 8 ms after its first item), the rule the Swift runner needed.
* S24 runs `fetchSse()` on Node's `fetch` (undici); the body is read only when the core pulls, so `/sse/hang` closed by
  the core is a client that leaves (the server sees it within the second).
* S25 step 2: "a keyed patch each" is not asserted (the first `add` onto an empty list arrives as a full value, SPEC
  3.8); the step checks that the mirror holds both notes, then the toggle.
* S25's steps also run once on the browser's adapter, `waSqliteDb()` (wa-sqlite behind its worker protocol), with the
  worker served in process over a `MessageChannel` by `startDbWorker(port, { storage: "memory" })` from
  `@undra/runtime/db-worker`: wa-sqlite's in-memory VFS, because OPFS exists only in a browser's dedicated workers (the
  web playground's smoke test covers OPFS in Chromium). That test is not named after the scenario: `node:sqlite` is the
  column's S25 adapter. wa-sqlite's `bind_text` cuts text at U+0000 and its `bind_blob` stores an empty array as NULL,
  so the engine binds text and blobs itself with their length (the runtime's Db suite asserts both).
* `node:sqlite` binds a JavaScript `number` as REAL, so `Integer` cells bind as `bigint` and read back as `bigint`
  (`setReadBigInts`); S25.3's `-9007199254740993` crosses exactly. Node's SQLite is built without double-quoted string
  literals (`SQLITE_DQS=0`): `"x"` is an identifier, never a string.

## Gaps and defects found (for the integrator)

Fixed since (playground finding 5): the Mirror stranded a change-set enqueued from a signal subscriber during the flush; the flush now drains it in a further round (`runtimes/ts/@undra/runtime/test/mirror.test.ts`).

1. **`UndraCore` had no `snapshot()` / `restore()`** (gap PA-5). **Fixed.** `await core.snapshot()` and
   `await core.restore(bytes)` exist in both wasm modes (SPEC 17.1); a refused restore rejects with
   `UndraRestoreError`. S15 uses them; S17 still snapshots through the wasm export and sends `Kind.Restore`
   (`src/wasm-exports.ts`), which can move to the public API.
2. **Generated stores cannot adopt an existing handle.** After a restore in a fresh core the handles of the
   old core are alive again, but `Todos.create()` always constructs a new one and the constructor is private,
   so there is no supported way to put a store class on a restored handle. S17 casts around the private
   constructor (`adoptTodos`, the one place that does).
3. **Mirror stranded a change-set enqueued from a signal subscriber** (`runtimes/ts/@undra/runtime/src/mirror.ts`).
   **Fixed.** `flush()` now drains further rounds until the queue is empty (up to the core's 1000-round cap),
   so a subscriber's synchronous core call is applied in the same flush. The repro moved into the runtime's
   own suite, `test/mirror.test.ts`.
4. **A failed mutation's rollback removed the placeholder of a later mutation** (undra-query, SPEC 9: "the
   pre-mutation entries are restored"). **Fixed.** The restore was a snapshot taken before the mutation ran; a
   mutation that started after it had already put its own optimistic item into the entry, and the restore took it
   away while its request was in flight (offline and queued, it stayed gone until the replay succeeded). The web
   app hit it by accident: Offline turned on while a toggle's PATCH was in flight, then an add. The rollback is
   now the inverse of the failed mutation's own writes, by per-entry write stamp: an entry written since (by a
   later optimistic mutation, a fetch, a `set`) is left alone. The repro in `test/findings.test.ts` lost its
   expected-fail mark and is now a regression test that asserts the placeholder survives.
5. **A keyed patch costs O(list) in the core, not O(change)** (undra-runtime / undra-signals, blueprint section 14:
   "lists must be O(change)", web budget for a one-insert patch on 10,000 items: 30 us). S10 prints the numbers
   (node 24, the release-wasm build): `update_at` on the 10,000-row list takes 5 us with nobody observing the store
   and about 1.26 ms with an observing store; the TypeScript side of it (decode, apply to the 10,000-item copy,
   announce) is about 15 us of that. The time is the core computing the patch by comparing the old and the new
   list. The 10k tab of the web app shows the same: about 1.2 to 1.8 ms from click to re-rendered window.
