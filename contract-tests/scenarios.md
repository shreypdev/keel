# Contract scenarios

This is the definition of "the platforms agree" (SPEC section 14, blueprint section 13): eighteen
scenarios, each run by every platform runtime against the **real playground core**
(`examples/playground/core`, the same Rust crate the apps run), through the real boundary:

| Platform | Runner | Boundary under test |
|---|---|---|
| TypeScript | `contract-tests/ts` (vitest) | `@undra/runtime` `WasmMainTransport` over the real `undra_core.wasm` |
| Kotlin | `contract-tests/kotlin` (kotlinc + JVM) | `dev.undra.runtime` `UndraCore` over JNI and the real `libundra_core` |
| Swift | `contract-tests/swift` (XCTest) | `UndraRuntime` `UndraCore` over the C ABI and the real static core |

Every runner prints one line per scenario, `SCENARIO S07 PASS|FAIL|SKIP <title>`, and
`contract-tests/check.sh` fails unless all eighteen ids are `PASS` (a `SKIP` needs its reason here,
in the platform notes of the scenario).

## The harness (the same on every platform)

The core is built by the `undra` CLI (`undra build --platform web|host|ios`) and its bindings by
`undra bindgen` (`examples/playground/generated/`). A runner uses the **generated bindings** for
everything a UI would use and the runtime's own API (`UndraCore`) for what bindings do not expose
(raw signal updates, statistics, snapshots, cancellation, schema checks).

* **One core per process** on the native platforms (`undra_init` is once per process), so scenarios
  create their own stores and objects, and the state they share (the query cache, the offline queue,
  the Clock) is isolated by **list names** (`s12`, `s13`, `s14`) and left clean.
* **Adapters** the runner supplies at load time:
  * `Clock`: a **manual clock**, settable and advanceable by the test, starting at
    `1_700_000_000_000` ms (used by the query cache for staleness; timers are not the clock's).
  * `Http`: an **in-memory server** (`FakeServer`): routes by method and exact URL, records every
    request (method, url, headers, body), can delay a reply by N ms and can answer with a network
    error (`HttpError.Network("offline")`). Default reply for an unknown route: status 404.
  * `Kv`: in-memory. `Log`: captures `(level, target, message)`. `Rng`, `Timer`: the platform
    defaults (real timers; a real `setTimeout` / Rust timer thread).
  * `Connectivity`: the test emits events itself (`core.event(Connectivity.changed, online, kind)`;
    each runtime has a helper or the raw call).
* **Server fixtures.** Base URL `https://playground.test`. A list `L` lives at
  `GET /lists/L/todos` (JSON array of `{"id":u32,"title":string,"done":bool}`), `POST
  /lists/L/todos` (body `{"title":..}`, answer 201 with the created object) and `PATCH
  /lists/L/todos/ID` (body `{"done":..}`, answer 200 with the object). `Idempotency-Key` is sent on
  POST.
* **Waiting.** Anything asynchronous is awaited with a **5 second timeout** (polling every 10 ms or
  using the runtime's own wait), never with a bare sleep, except where a scenario says "for 200 ms
  nothing happens".
* **Signals.** "Observe" means `Store.create()` / `init` (which constructs, observes every signal
  and applies the initial change-set before returning), or for raw checks `core.construct` +
  `core.mirror.register` + `core.observe(handle, ALL_SIGNALS, true)`.
* **Statistics** (`core.stats()`) carry `live_handles`, `active_calls`, `open_streams`,
  `transactions` (== change-sets delivered), `panics` and `crossings.{calls, replies, change_sets,
  stream_items, cancelled, bad_requests}`. A scenario that counts uses **deltas** between two
  readings, since other scenarios share the core.

Ids: a method's id is `fnv1a32("Type.method")`, a free function's `fnv1a32("fn.name")`; the bindings
carry them (`UndraIds`).

## The scenarios

### S01 primitives round-trip

Every primitive the wire has makes the round trip through the language's codec unchanged.

1. `echo_primitives` with the **typical** value: `flag=true, tiny=-8, small=-16000, int=-2000000000,
   long=-9000000000000000000, byte=255, word=65535, dword=4000000000, qword=18000000000000000000,
   single=1.5, double=-2.25e100, text="héllo, wörld ✓", blob=[0,1,2,254,255], span=1.500000123 s,
   at=1700000000123 ms, id=12345678-9abc-def0-0102-030405060708`.
2. `echo_primitives` with the **extremes**: `tiny=-128, small=-32768, int=-2147483648,
   long=-9223372036854775808, qword=18446744073709551615, single=-0.0, double=+infinity,
   text="" , blob=[] `, and with `long=9223372036854775807, tiny=127, small=32767, int=2147483647,
   double=NaN` (compared as NaN), `text` = 10,000 characters of `"ü"`, `blob` = 65,536 bytes
   `i % 251`.
3. `ping()` (no arguments, no result) completes.
4. `Bytes`, `String` and `Uuid` are not aliased: mutating the returned value does not change what was
   sent.

Expected: every field equal to what was sent (64-bit integers exactly: `bigint` in TypeScript, `Long`
/ `ULong` in Kotlin, `Int64` / `UInt64` in Swift).

### S02 records, enums and errors

1. `echo_figure` for `Circle{radius=2.5}`, `Rect{width=2, height=3.5}`, `Label("")`, `Label("héllo")`,
   `Empty`: each returns an equal value of the same variant.
2. `echo_composite` with `name="everything", tags=["a","","ü"], figure=Some(Rect{2,3.5}),
   history=[Circle{1}, Label("x"), Empty], scores={"high":99,"low":-3}, names={7:"seven",1:"one"},
   limit=Some(7)` and again with `figure=None, tags=[], history=[], scores={}, names={}, limit=None`.
3. `area(Rect{2,4})` is `8.0`; `area(Circle{1})` is `pi` (to 1e-12).
4. Errors as values: `area(Label("hat"))` fails with `LabError.Rejected(code=1, reason="`hat` has no
   area")`; `area(Empty)` fails with `LabError.Empty`; `parse_count("")` with `Empty`,
   `parse_count("1234567890")` with `TooLong(max=9)`, `parse_count("4x2")` with `NotANumber("4x2")`;
   `parse_count(" 42 ")` is `42`.
5. The error's message is the core's `Display`: `TooLong(9)` -> `"longer than 9 characters"`.

Expected: typed errors arrive as the language's typed error (Swift `throws(LabError)`, Kotlin sealed
`LabError` exception, TypeScript `LabError` subclass), never as a generic failure.

### S03 sync call

The synchronous path (`callSync` / the `undra_call_sync` ABI; wasm-main in TypeScript) answers without
waiting for an event loop.

1. `add(40, 2)` through the sync path is `42`; `add(2147483647, 1)` is `-2147483648`;
   `greet("Ada")` is `"Hello, Ada, from the playground core"`.
2. The sync path has no suspension: in Swift and Kotlin it is called from a plain (non-async) function;
   in TypeScript `core.callSync` returns bytes (not a promise) in `wasm-main` mode.
3. 10,000 sync `add` calls in a loop return the right sums and `crossings.calls` grew by exactly
   10,000 (the runner measures and prints the mean ns per call; no budget is asserted here).
4. A sync call of an **async** method (`add_later`) is refused: status `bad_request` (TypeScript: the
   reply error with status 5), and the core is unharmed (`add(1,1) == 2` afterwards).

### S04 async call

1. `add_later(20, 22, 50)` resolves to `42` after at least 45 ms and less than 2 s.
2. Three concurrent calls `add_later(i, 0, d)` with `(i, d) = (1, 400), (2, 50), (3, 200)` resolve
   in delay order `2, 3, 1` and with the right values (the delays are 150 ms apart on purpose: a stalled
   runner must not be able to reorder them).
3. `Probe.wait(10)` resolves to `10`.
4. A call made from inside a change observer or another call's completion (re-entrancy on the
   platform side: the continuation of `add_later` calls `add_later` again) works: the second call
   resolves. (Checks the runtimes never invoke callbacks under the core lock.)

### S05 error propagation

1. Sync typed error: `parse_count("x")` fails with `NotANumber("x")` (see S02) and the failure is
   delivered on the same path as a success (TypeScript rejects, Swift throws, Kotlin throws).
2. Async typed error: `fail_later(10, 7)` fails with `Rejected(code=7, reason="on purpose")`.
3. Store errors: `Todos.add("   ")` fails with `TodoError.EmptyTitle` and changes nothing (no change
   set: `transactions` delta 0); `BigList.remove_at(10000)` fails with
   `ListError.OutOfRange(index=10000, len=10000)`; `BigList.insert_at(10001, "x")` fails likewise
   (`len=10000`) and does not consume an id (the next `insert_at(0, "y")` returns id `10001`).
4. Reply statuses: an unknown method id (`0xDEADBEEF` as a free function) fails as **bad request**;
   calling a method on a **released** handle fails as bad request (stale handle); a constructor given
   undecodable arguments (`Probe` has none: use `RemoteTodosQueryHandle` with an empty argument
   buffer) fails as bad request. Each carries a reason string.
5. After all of the above the core still works (`add(1, 2) == 3`) and `bad_requests` grew by 3.

### S06 cancellation

1. `Probe` = new. Start `probe.hang()`; wait until `counters().started == 1`.
2. Cancel it (TypeScript `AbortSignal.abort()`, Kotlin `Job.cancel()`, Swift `Task.cancel()`).
   The platform call ends as cancelled (TypeScript: rejects; Kotlin: `CancellationException`; Swift:
   `CancellationError` or the runtime's cancelled error).
3. Wait until `counters().cancelled == 1`: the **core dropped the future** (the `Probe` counts it in a
   drop guard). `completed` is still `0`, `active_calls` is back to its value before step 1, and
   `crossings.cancelled` grew by 1.
4. Independence: start `probe.wait(100)` and `probe.hang()` together, cancel only the second; the
   first resolves `100`, `completed == 1`, `cancelled == 2`.
5. Cancelling after completion is a no-op: `probe.wait(1)`, await, cancel; `cancelled` stays 2.

### S07 stream with backpressure

1. `probe.reset()`. Open `probe.ticks(1000)` and read exactly 5 items (`0..4`), then **stop reading**
   without cancelling.
2. For 200 ms nothing more is read; then `counters().produced` is **at least 5 and at most 5 + 64**
   (the credit window: 16 granted at subscribe, topped up as the consumer drains; the core is polled
   at most one item past its credit). In particular it is not 1,000.
3. Resume: read the remaining 995 items; they are `5..999` in order, the stream then ends
   normally, and `produced == 1000`.
4. Early termination: open `probe.ticks(1000000)`, read 3 items, cancel/stop the iteration (TypeScript
   `break` out of `for await`, Kotlin cancel the collector, Swift `break` or cancel the task);
   within 1 s `open_streams` is back to its earlier value and `produced` stays below 200.
5. A short stream ends: `ticks(3)` yields `0,1,2` and completes; `ticks(0)` completes with no items.

### S08 store observe: initial change-set

1. Raw: `core.construct(Todos.new)`, register a mirror callback for the handle, then
   `observe(handle, ALL_SIGNALS, on)`. **Before `observe` returns** (native) / resolves (TypeScript)
   the mirror has received exactly one change-set's worth of entries: signals `0 todos`, `1 filter`,
   `2 visible`, `3 remaining`, all as full values (`op = 0`): `[]`, `all`, `[]`, `0`.
2. Through the generated class: `Todos.create()` / `Todos()` has `todos == []`, `filter == .all`,
   `visible == []`, `remaining == 0` immediately after it returns, with no further await.
3. `Counter` initial: `count 0, changes 0, parity even`. `BigList` initial: `items.count == 10000`,
   `items[0] == Item(1,"Item 1",0)`, `items[9999] == Item(10000,"Item 10000",0)`, `count == 10000`.
4. `observe(off)`: after turning observation off, `Todos.add("x")` delivers nothing to the mirror
   (200 ms); `observe(on)` again delivers one change-set with the **current** values (the added item).
5. Releasing a store (`close()` / `release`) drops `live_handles` by one.

### S09 transaction: a single change-set

1. `Counter` observed. Take `transactions` and `change_sets` readings `t0`. `counter.add(5)`: the mirror
   receives **three entries** for `count = 5`, `changes = 1`, `parity = odd`, and `transactions`
   grew by exactly **1**.
2. Three separate calls `increment, increment, decrement` grow it by exactly 3.
3. `counter.reset()` (two writes in one `ctx.txn`) grows it by exactly 1 and delivers `count = 0`,
   `changes = 0`, `parity = even`.
4. `Bench.bench_touch_signals(100)` grows it by exactly 1 and the mirror receives exactly 100 entries
   (signal ids `1..=100`, each a full value `1`); `bench_touch_signals(1)` delivers exactly 1 entry;
   `bench_touch_signals(1000)` delivers all 128.
5. Writes are not delivered while nothing observes the store: `Counter` not observed, `add(1)` grows
   `transactions` by 0; observing it afterwards delivers the current value once.

### S10 keyed patch

1. `BigList` observed through the raw mirror. The initial entry for `items` (signal 0) is a **full
   value** of 10,000 items.
2. `insert_at(5000, "fresh")` returns `10001`; the next change-set has an `items` entry with
   `op = 1` (keyed patch) of exactly **1 operation**, `Insert{index=5000, item=(10001,"fresh",0)}`,
   the entry's value is **under 100 bytes**, and the `count` computed entry is `10001`.
3. `update_at(42, "renamed")`: one `Update{index=42, item=(43,"renamed",1)}`.
4. `move_item(10, 9000)`: one `Move{from=10, to=9000}` (after the insert above the list has 10,001
   items).
5. `remove_at(0)`: one `Remove{index=0}`.
6. The runner applies each patch to its own mirror copy of the list and, after every step, compares
   it with the list of a freshly observed second `BigList`-equivalent: here simply with the expected
   model (a local array mutated by the same operations): equal in full, at every step.
7. `Bench.bench_list_insert(123)` is also a 1-operation patch on a 10,000-row list.
8. `reset()` after removing the first item yields one `Insert{index=0, item=Item(1,..)}` patch.

### S11 computed

1. `Todos` observed. `add("a")`, `add("b")`, `add("c")` (ids counting up); `toggle(b)`.
   `remaining == 2`, `visible == [a, b(done), c]`.
2. `set_filter(Done)`: the **one** change-set carries `filter = done` and `visible = [b]` (the computed
   was recomputed in the core), `remaining` does not change.
3. `set_filter(Active)`: `visible == [a, c]`. `set_filter(All)` restores all three.
4. `remove(a)`, `clear_done()`: `visible`/`remaining` follow.
5. `Counter.parity`: after `add(3)` odd, after `add(1)` even, after `add(-1)` odd, after `add(0)`
   odd (still); always consistent with `count`.
6. **Reads never cross the boundary**: reading `visible`, `remaining`, `parity`, `items` 1,000 times
   leaves `crossings.calls` unchanged.

### S12 query: fetch, stale, refetch

List `s12`; `configure_remote(base_url="https://playground.test")` once per process. Server `GET
/lists/s12/todos` answers `[{"id":1,"title":"Buy milk","done":false}]`; the runner counts GETs.

1. `RemoteTodosQueryHandle.create("s12")`: status goes `loading -> success` (`idle/fetching` while
   pending); `data == [Buy milk]`, `error == nil`, `fetching == false`, `updatedAt == manual clock now`;
   GET count `1`.
2. Clock `+10 s`. A **second** handle on `s12`: its `data` is there at once, GET count still `1`
   (fresh window is 30 s); both handles show the same data.
3. Clock `+31 s` (41 s since the fetch). A **third** handle: stale, so it fetches: GET count `2`. The
   server now answers with two items; all three handles end with two items and a newer `updatedAt`.
4. `refetch()` on any handle fetches although the data is fresh: GET count `3`.
5. `invalidate()`: marks stale and, being observed, refetches: GET count `4`.
6. The server now answers `503` with body `down`: `refetch()`; after the core's retry (1 retry, about
   1 s of backoff, real timer) `status == error`, `error == RemoteError.Status(code=503)`, `data` still
   holds the last good list.
7. The server answers with `not json`: `refetch()` -> eventually `error == RemoteError.BadBody(..)`.
8. Release all handles: `live_handles` is back to its earlier value.

### S13 optimistic mutation and rollback

List `s13`; the server serves `[Buy milk (id 1)]`; POST and PATCH replies are **delayed by 50 ms**. One
handle observes and **records every value of `data`**.

1. `create_remote_todo("s13", "Walk")` while the server answers `500` to POST: the call fails with
   `RemoteError.Status(code=500)`. Recorded `data`: `[milk]`, then `[milk, Walk(id=4294967295)]` (the
   placeholder, visible during the 50 ms), then `[milk]` again: **rolled back**. Final `status ==
   success`.
2. The server now answers `201 {"id":2,"title":"Walk","done":false}` and lists `[milk, Walk(2)]`
   afterwards: the call returns `RemoteTodo(2,"Walk",false)`; recorded: `..., [milk, placeholder], [milk,
   Walk(2)]` (optimistic, then the refetch after success). The POST carried an `Idempotency-Key` header
   (a UUID).
3. `set_remote_done("s13", 1, true)` while PATCH answers `500`: fails with `Status(500)`; recorded:
   `milk` with `done=true` (optimistic), then `done=false` (rollback).
4. `set_remote_done("s13", 1, true)` with PATCH `200`: succeeds; final list has `done=true` after the
   refetch (the server now lists it so).

### S14 offline queue replay

List `s14`; the server serves `[]`. A handle observes it.

1. The test emits `Connectivity.changed(online=false, kind=None)` and waits 50 ms.
2. POST `/lists/s14/todos` is scripted to fail with `HttpError.Network("offline")`.
   `create_remote_todo("s14", "Offline item")` is started and **does not finish**: for 200 ms it is
   still pending; the server saw exactly 1 POST; the handle's `data` shows the placeholder.
3. A **non-idempotent** mutation does not queue: `set_remote_done("s14", 1, true)` with the PATCH route
   failing the same way fails **at once** with `RemoteError.Http(HttpError.Network("offline"))`.
4. POST is now scripted to answer `201 {"id":9,"title":"Offline item","done":false}` and the list
   route to `[{"id":9,...}]`. The test emits `Connectivity.changed(online=true, kind=Wifi)`.
5. The pending `create_remote_todo` **resolves** to `RemoteTodo(9,"Offline item",false)`; the server saw
   exactly 2 POSTs; both carry the **same** `Idempotency-Key`; the handle ends with `data == [id 9]`.
6. The Kv port saw a write of the key `undra.query.queue` while offline (the queue is persisted) and the
   queue was emptied after the replay (last write is an empty queue).

### S15 snapshot and restore

1. `Todos` (observed) with `add("a")`, `add("b")`, `toggle(b)`; `Counter` (observed) with `add(5)`;
   `BigList` (observed).
2. `snapshot = core.snapshot()` (TypeScript: the wasm export `undra_snapshot`; Kotlin/Swift:
   `core.snapshot()`); it is non-empty and opaque.
3. Mutate: `Todos.add("c")`, `toggle(a)`, `set_filter(Done)`; `Counter.add(10)`; `BigList.remove_at(0)`.
4. `core.restore(snapshot)`. The **same handles** still work: each store's mirror returns to the snapshot
   values (`todos == [a, b(done)]`, `filter == all`, `visible` likewise, `remaining == 1`, `count == 5`,
   `changes == 1`, `parity == odd`, `items.count == 10000` and `items[0].id == 1`), delivered as change-sets
   for the observed signals.
5. Identities continue: `Todos.add("d")` returns an id above `b`'s (no collision with `c`'s earlier id is
   required, none with `a` or `b`).
6. A **rejected snapshot** (16 random bytes) fails (`restore` throws / returns non-zero / rejects) and
   leaves every store as it was.
7. A handle released **before** the snapshot is not resurrected.
8. Stats: `live_handles` after the restore equals the count of surviving stores (the restore creates
   no extra handles).

### S16 schema mismatch rejection

1. `UndraCore.load` with `expectedSchemaHash = generatedHash ^ 1` fails **before the core is initialised**
   with the runtime's schema-mismatch error carrying `expected` and `got` (`got == generatedHash`)
   and a message that names both in hex (TypeScript `UndraSchemaMismatchError`, Kotlin
   `UndraSchemaMismatch`, Swift `UndraSchemaMismatchError`).
2. A subsequent load with `UndraIds.schemaHash` succeeds (the failed attempt did not leave the process
   half-initialised).
3. The hash in the bindings equals the hash the core reports (`stats().schema_hash`) and the hash of the
   schema the core exports (`undra_schema_hash`).
4. (TypeScript and Kotlin) the core's JSON schema (`undra_schema_json`) lists the playground's types
   (`Todos`, `Counter`, `BigList`, `Bench`, `Probe`, the queries) and the standard ports.

### S17 panic containment

**Native (Kotlin over JNI, Swift over the C ABI)** — a panic unwinds to the boundary:

1. `explode("kaboom")` fails with reply status **panic** and a message containing `kaboom` (plus a
   backtrace string). The process is alive.
2. `explode_later(10, "later")` (an async call) fails the same way.
3. The core keeps working: `add(1, 2) == 3`; a store constructed before still updates;
   `stats().panics` grew by 2.
4. The Log port received a record with level >= 4 (error/fatal) and target `undra::panic` for each.

**wasm (TypeScript)** — the shipped wasm profile aborts on panic (SPEC section 7), so containment means
the host survives and recovers:

1. Before the panic: `snapshot = undra_snapshot()` of a core with a `Todos` holding two items.
2. `explode("kaboom")` makes the call **fail** (it does not hang or crash the test process) with an
   `UndraError` whose message says the core trapped; `core.closed` is true and `onClose` fired; the Log
   adapter received a **fatal** (5) record containing `kaboom` before the trap.
3. The page can **restart**: a fresh `UndraCore.load` of the same module succeeds and the snapshot restores:
   `Todos` (re-created from the restored handle) shows the two items.
4. A second core loaded in the same process before the panic was not affected.

### S18 coalesced burst

The core commits one change-set per transaction; the platform mirror merges what arrived before it
drains (ADR-031, SPEC section 11). `Stress.burst(mode, n)` commits `n` transactions in a tight loop,
one write each, without `ctx.txn`. The calls are made **from the main thread** (Swift: the main
actor; Kotlin: `UndraDispatchers.main`), where a synchronous call drains the mirror before it
returns; TypeScript awaits them.

1. Raw: `Stress` constructed and observed through a raw mirror callback (registered without
   `no_coalesce` ids). Take a `transactions` reading. `burst(Firehose, 1000)`: when the call returns
   (resolves), **with no further wait or flush**, the raw callback has run **exactly once**, with one
   full value (`op = 0`) for signal `0 value` equal to `1000`; `transactions` grew by exactly
   **1000**; the mirror's counters say `changeSetsReceived` grew by 1000 and `entriesApplied` by 1.
2. Through the generated class: `Stress.create()` / `Stress()`, `burst(Firehose, 1000)`: `value ==
   1000` as soon as the call returns (read-your-writes).
3. Through the generated class, `no_coalesce`: `burst(Progress, 10)`: `progress == 10` as soon as the
   call returns, and the mirror applied all 10 entries (`entriesApplied` grew by 10), because the
   generated store registered `progress` as `no_coalesce`. (TypeScript also checks that a subscriber
   heard `1, 2, ..., 10`; a Kotlin `StateFlow` conflates and SwiftUI renders once per frame, so they
   check the mirror's counter only.)

## Platform notes

* TypeScript: S03 runs only in `wasm-main` mode (the only one with `callSync`); snapshot goes through the
  wasm export because `UndraCore` has no public `snapshot()` (finding: SPEC 17.1 does not list one).
* The Kotlin runner runs on the JVM with a single-thread "main" executor (`UndraDispatchers`), the Swift
  runner on the main actor; both load the real native library.
* Timing constants (50 ms delays, 200 ms quiet windows) are chosen for a loaded CI machine; do not
  shrink them.
