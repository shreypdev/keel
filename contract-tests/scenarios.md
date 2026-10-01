# Contract scenarios

This is the definition of "the platforms agree" (SPEC section 14, blueprint section 13): twenty-two
scenarios against the **real playground core** (`examples/playground/core`, the same Rust crate the
apps run), through the real boundary. S01 to S20 run on every platform; S21 and S22 are about the web
host (worker mode and crash recovery, ADR-049) and run on TypeScript only:

| Platform | Runner | Boundary under test |
|---|---|---|
| TypeScript | `contract-tests/ts` (vitest) | `@undra/runtime` `WasmMainTransport` over the real `undra_core.wasm` |
| Kotlin | `contract-tests/kotlin` (kotlinc + JVM) | `dev.undra.runtime` `UndraCore` over JNI and the real `libundra_core` |
| Swift | `contract-tests/swift` (XCTest) | `UndraRuntime` `UndraCore` over the C ABI and the real static core |

Every runner prints one line per scenario, `SCENARIO S07 PASS|FAIL|SKIP <title>`, and
`contract-tests/check.sh` fails unless every id of the platform is `PASS` (S01 to S20, plus S21 and
S22 on TypeScript; a `SKIP` needs its reason here, in the platform notes of the scenario). That is 62
cells: 20 on Swift, 20 on Kotlin, 22 on TypeScript.

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
  * `Kv`: in-memory, recording every operation (`get`, `set`, `delete`, `list` with the key or
    prefix, in order), with **injectable failures** (ADR-049): the test can make the next `n`
    operations, or every operation until it heals the store, of one kind (or of one key) fail with a
    `StorageError` (`Full`, `Locked`, `Io(..)`, ...), which the adapter answers as the port's typed
    error (reply status 1), exactly like the platform's own adapters do. On Swift and Kotlin the first
    `get` of `undra.query.queue2` fails `Locked` (S20 step 4: what an app launched before the device's
    first unlock reads). `Log`: captures `(level, target, message)`. `Rng`, `Timer`: the platform
    defaults (real timers; a real `setTimeout` / Rust timer thread).
  * `Connectivity`: the test emits events itself (`core.event(Connectivity.changed, online, kind)`;
    each runtime has a helper or the raw call).
* **Server fixtures.** Base URL `https://playground.test`. A list `L` lives at
  `GET /lists/L/todos` (JSON array of `{"id":u32,"title":string,"done":bool}`), `POST
  /lists/L/todos` (body `{"title":..}`, answer 201 with the created object) and `PATCH
  /lists/L/todos/ID` (body `{"done":..}`, answer 200 with the object). `Idempotency-Key` is sent on
  POST.
* **Two builds** (S14 steps 7 to 9, S15 steps 11 to 14; ADR-037). The runner also builds the
  playground core a second time, as **build B**: `UNDRA_PLAYGROUND_V2=1 undra build ...` (the core's
  `build.rs` turns it into `cfg(playground_v2)`; see `examples/playground/core/src/updates.rs` for what
  changes), and keeps that artefact apart from build A's. Build B has no generated bindings: the runner
  loads it with the hash the core reports (`undra_schema_hash`) and drives it through the raw API with
  the ids it knows (`Profile.new` / `Profile.describe` / `Legacy.new`, the mutation ids of `save_note` and
  `tag_note`, the function `storage_status`, whose `StorageStatus` record has the same layout in both
  builds). TypeScript loads build B in the same process; Swift and Kotlin load one core per process, so
  they run the build-B steps in a **second process** (Swift: the same test bundle relaunched with build
  B's library in place of build A's; Kotlin: a second JVM with build B's library), handing over the `Kv`
  contents and the snapshots in a file. The build-B process prints only `SCENARIO S14 FAIL ...` /
  `SCENARIO S15 FAIL ...` lines (never `PASS`), so `check.sh`, which reads the last line of an id, fails
  the scenario if build B fails and keeps build A's `PASS` otherwise.
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
6. Through the generated bindings, on closed objects (ADR-032 and its amendment A: every platform fails a call
   with its own `E`, the caller's cancellation or `UndraCallError`, and a command reports): close a `BigList`,
   then `remove_at(0)` fails as **bad request** (Swift `UndraCallError.refused`, Kotlin
   `UndraCallError.Refused`, TypeScript `UndraCallError.Refused`, `kind` `"refused"`), with a reason. Close a
   `Counter`, then `increment()` (a command): it **returns normally** (TypeScript: the promise resolves) and
   `LoadOptions.onError` received exactly one `UndraUnhandledError` with operation `Counter.increment`
   (the platform's spelling, no keyword escapes) whose error is `Refused`. `bad_requests` grew by exactly 2 more,
   the process is alive and `add(1, 2) == 3`. The runner clears what it recorded, and the TypeScript harness
   treats an `onError` report nobody asserted as a failure of the scenario.

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
6. A cancelled call of a method **with a typed error**: start `fail_later(5000, 1)`, wait 100 ms,
   cancel it. The platform call ends as cancelled exactly as in step 2 (Swift `CancellationError`, not
   a `LabError` and not a stopped process; Kotlin `CancellationException`; TypeScript the signal's
   reason), within 1 s; `crossings.cancelled` grew by 1; `add(1, 1) == 2` afterwards.

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
6. A typed error part-way (ADR-036: `impl Stream<Item = Result<T, E>>`, flag 2 carries only the
   stream's own `E`): `probe.ticks_then_fail(5, 3, 7)` yields `0,1,2` and then fails with the
   stream's own error, `LabError.Rejected { code: 7, reason: "stopped at 3" }` (Swift
   `LabError.rejected`, Kotlin `LabError.Rejected`, TypeScript a `LabError` of that variant), never a
   wire error; `ticks_then_fail(3, 9, 7)` yields `0,1,2` and completes normally.
7. A stream the core cancels (ADR-036: flag 3, status 3). Take a snapshot; open
   `probe.ticks_then_fail(1000000, 999999, 1)` (a stream **with** an error type) and read 2 items; then
   `restore` the snapshot. The probe is not a store, so the restore invalidates it and ends the stream:
   the iteration fails as **cancelled by the core** (`UndraCallError.cancelledByCore` in Swift,
   `UndraCallError.CancelledByCore` in Kotlin and TypeScript), not as a `LabError` and
   not as a wire decode error (Kotlin `WireException`, TypeScript `WireError`), within 1 s; `open_streams`
   is back to its value before the step. Close the probe.

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
2. `set_filter(Done)`: the **one** change-set carries `filter = done` and `visible = [b]` (computed in
   the core; since ADR-039 `visible` is a derived list, so a raw mirror receives it as the keyed patch
   that leaves `[b]`), `remaining` does not change.
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

List `s14`; the server serves `[]`. A handle observes it. Before step 1 the runner waits until
`storage_status().queue_readable` is `true` (on Swift and Kotlin the harness failed the first read of the
queue, S20 step 4; the client reads it again after a backoff of about a second).

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
6. The Kv port saw a write of the key `undra.query.queue2` while offline (the queue is persisted, format 2
   of ADR-037: `format u16 = 2, schema_hash u64, count u32, items`, each item with its mutation's
   fingerprint) and the queue was emptied after the replay (the last operation on the key is a `delete`, or
   a `set` of a queue whose count, the `u32` at offset 10, is 0). A key `undra.types.<16 hex digits>` holds
   the description of `create`'s input (written before the queue that needs it).
7. **Build A queues what build B changes.** Offline again; POST `/lists/s14m/notes` fails with
   `HttpError.Network("offline")`. `save_note("s14m", "a")` and `tag_note("s14m", 7)` are started and stay
   pending (`storage_status().pending == 2`). The runner keeps the `Kv` contents (every key and value) and the
   `Idempotency-Key` header of the failed `save_note` POST, and stops build A (Swift and Kotlin: at the end of
   the run, see "Two builds").
8. **Build B reads it.** A core of build B is loaded with a `Kv` holding exactly those contents, offline: the
   runner emits `Connectivity.changed(false, None)` and calls `configure_remote` **before the queue is read**
   (configuration is not persisted, and a replay that started first would fail on an unconfigured endpoint;
   the TypeScript runner holds build B's first `get` of `undra.query.queue2` until then). Within 5 s `storage_status()` says
   `queue_readable`, `pending == 1` (`save_note` gained `pinned: Option<bool>`: migrated by parameter name),
   `migrated == 1`, `dead_lettered == 1`, and `dead_letters == ["tag_note: <reason>"]` with a reason that says
   its input does not migrate (`id` became a `String`); nothing was replayed (no request reached the server).
9. **Replay in build B.** POST `/lists/s14m/notes` answers 201; online: within 5 s the server saw exactly one
   POST with body `save:a` (build B's `save_note` with `pinned == None`) carrying the **same**
   `Idempotency-Key` as build A's attempt; `pending == 0`; the dead letter is still there (never lost, never
   replayed) and so is the key `undra.query.queue.dead` in the `Kv`.

### S15 snapshot and restore

1. `Todos` (observed) with `add("a")`, `add("b")`, `toggle(b)`; `Counter` (observed) with `add(5)`;
   `BigList` (observed).
2. `snapshot = core.snapshot()` (TypeScript: `await core.snapshot()`, the public API of SPEC 17.1);
   it is non-empty and opaque.
3. Mutate: `Todos.add("c")`, `toggle(a)`, `set_filter(Done)`; `Counter.add(10)`; `BigList.remove_at(0)`.
4. `core.restore(snapshot)` (TypeScript: `await core.restore(snapshot)`, which resolves after the restored
   values reached the stores, so the checks that follow need no waiting). The **same handles** still work:
   each store's mirror returns to the snapshot values (`todos == [a, b(done)]`, `filter == all`, `visible`
   likewise, `remaining == 1`, `count == 5`, `changes == 1`, `parity == odd`, `items.count == 10000` and
   `items[0].id == 1`), delivered as change-sets for the observed signals.
5. Identities continue: `Todos.add("d")` returns an id above `b`'s (no collision with `c`'s earlier id is
   required, none with `a` or `b`).
6. A **rejected snapshot** (16 random bytes) fails (`restore` throws / returns non-zero / rejects) and
   leaves every store as it was (TypeScript and Swift: `UndraRestoreError`, whose code is the core's, 5 for
   a malformed snapshot; the core is not closed and the next call works).
7. A handle released **before** the snapshot is not resurrected.
8. Stats: `live_handles` after the restore equals the count of surviving stores (the restore creates
   no extra handles).
9. A call in flight across a restore. `Probe` = new; start `probe.hang()`; wait until
   `counters().started == 1`; take a snapshot; `restore` it. The probe is not a store, so the restore
   invalidates it and cancels its call: `hang()` fails as **cancelled by the core** (Swift
   `UndraCallError.cancelledByCore`, Kotlin `UndraCallError.CancelledByCore`, TypeScript `UndraCallError.CancelledByCore`,
   `kind` `"cancelledByCore"`), not as a platform cancellation. Then, through the generated bindings,
   `probe.counters()` fails as bad request (`Refused`, stale handle) and `probe.reset()` (a command) returns
   normally and `onError` received `Probe.reset` with `Refused`. Close the probe.
10. A stream in flight across a restore. A new `Probe`; open `probe.ticks(1000000)` and read one item; take a
    snapshot and `restore` it. The core ends the stream with an error item carrying its own String
    (`"cancelled: ..."`, SPEC 5.9), and the consumer's iteration ends with **cancelled by the core** (the same
    three spellings as step 9), not with a decoding failure and not as a platform cancellation. (Kotlin and
    TypeScript used to read that String as a typed error; the stream mapping is one function per runtime now.)
    Close the probe.
11. **Build A's snapshots.** `Profile.new("ada")`, `visit()` twice; `describe() == "name=ada;visits=2"`; snapshot
    `P` (no `Legacy` exists yet). Then `Legacy.new(5)`; snapshot `L`. The runner keeps `P`, `L` and the
    `Profile` handle (Swift and Kotlin: for the build-B process). Release the `Legacy`.
12. **Build B restores `P` by name.** In a core of build B, `restore(P)` succeeds; the same `Profile` handle answers
    `describe() == "name=ada;visits=2;theme="`: build B reordered `visits` before `name` and added `theme` with
    `#[undra(default)]`, and the values arrived by name (SPEC 5.9, layout 2). A store type both builds share and
    did not change (the other stores of `P`, if the runner made any) takes the fast path.
13. **Build B refuses `L`.** `restore(L)` fails with the restore error whose code is **7** (`incompatible`:
    Swift `UndraRestoreError` code 7 / `.incompatible`, Kotlin `UndraRestoreException` code 7, TypeScript
    `UndraRestoreError` code 7), because `Legacy.score` changed from `i32` to `String` and no hook converts it;
    the core's Log port received an ERROR record naming `Legacy` and `score`; the core is unchanged (the
    `Profile` still describes as in step 12) and the next call works.
14. **A snapshot from before layout 2 is refused** (R7: what an older host kept is checked when it is
    loaded): the 8 bytes `00000000 00000000` (ADR-022's empty snapshot, `count, floor`) fail with code **5**
    (malformed) in either build, and the core is unchanged. (Live peers with different schema hashes still
    refuse each other at load: S16.)

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
5. **No core loaded.** (Kotlin and Swift: first, before the load that sticks; the failed load of step 1 left
   nothing behind. TypeScript: the harness never makes a core shared, so it holds throughout.) `UndraCore.current`
   is nil/null; `UndraCore.shared` does not throw on access but is a closed placeholder; a generated call with
   the default core (`add(1, 2)`) and a generated constructor with the default core (`Counter` create) fail as
   **unavailable** (Swift `UndraCallError.unavailable(.closed)`, Kotlin `UndraCallError.Unavailable` whose
   transport reason is `CLOSED`, TypeScript `UndraCallError.Unavailable`, `transport.reason == "closed"`), with a
   message that says to load a core; a failure reported on the placeholder (`UndraCore.shared`'s `report`) only
   logs; `current` is still nil/null afterwards.

### S17 panic containment

**Native (Kotlin over JNI, Swift over the C ABI)** — a panic unwinds to the boundary:

1. `explode("kaboom")` fails with reply status **panic** and a message containing `kaboom` (plus a
   backtrace string). The process is alive. Swift and Kotlin call the generated `explode` (Swift:
   `UndraCallError.panicked`, Kotlin: `UndraCallError.Panicked`, whose `panicMessage` contains `kaboom`).
2. `explode_later(10, "later")` (an async call) fails the same way (Swift `UndraCallError.panicked`, Kotlin
   `UndraCallError.Panicked`).
3. The core keeps working: `add(1, 2) == 3`; a store constructed before still updates;
   `stats().panics` grew by 2.
4. The Log port received a record with level >= 4 (error/fatal) and target `undra::panic` for each.
5. Re-entry is refused, not deadlocked or aborted: while the core is logging the panic of
   `explode("reenter")`, the runner's Log adapter (a synchronous port, called on the thread that holds
   the core lock) calls the generated `add(1, 1)` once. That call is **refused**, as a bad request whose reason
   contains `E_REENTRANT` (Swift `UndraCallError.refused`, the core's own refusal; Kotlin `UndraCallError.Refused`,
   raised by the runtime's guard, which answers with the same reply before the call reaches the core);
   `explode` itself fails as in step 1; afterwards `add(1, 2) == 3`.
6. Shutdown with a typed call in flight (it ends the core). Start
   `fail_later(5000, 1)`, wait 100 ms, shut the core down: the call fails as **closed** (Swift
   `UndraCallError.unavailable(.closed)`, Kotlin `UndraCallError.Unavailable` with transport reason `CLOSED`)
   within 1 s. Then, on the shut-down core, the generated `add(1, 2)` fails the same way and
   `Counter.increment()` on a store of that core returns (`onError` received `Unavailable`, closed). The
   shut-down core is no longer the shared one: `UndraCore.current` is nil/null, and a generated constructor
   with the default core fails as in S16.5. The process is alive.
7. Closing ends the core's work, and a new load starts fresh (ADR-034; the last step of the run). Before
   step 6's shutdown a timer-paced core task is running (`Stress.start`); for 200 ms after the shutdown
   no port call reaches the runner's adapters (the runner counts the calls its Clock, Log, Http and Kv
   adapters receive). Then `UndraCore.load` with the same options succeeds again in the same process
   (Kotlin: `close()` reached the JNI `UndraNative.shutdown`; Swift: `undra_shutdown`), `stats()` of the
   new core reports no live handles, the generated `add(1, 2) == 3` runs on it, for 200 ms its Clock
   adapter receives no call (the new core runs no timer-paced task), and it closes cleanly. After each
   close, the native core reports no thread of its own still running: with no core loaded,
   `undra_stats_json` (Kotlin `UndraNative.statsJson()`) says `runtime_threads == 0`. That is the check
   that sees a task which survived the shutdown: its port calls never reach the runner's adapters (Kotlin
   detaches the transport first; the native shutdown retires the port registrations and the Swift
   adapters are detached), so the windows alone cannot.

**wasm (TypeScript)** — the shipped wasm profile aborts on panic (SPEC section 7), so containment means
the host survives and recovers:

1. Before the panic: `snapshot = undra_snapshot()` of a core with a `Todos` holding two items.
2. `explode("kaboom")` makes the call **fail** (it does not hang or crash the test process) with an
   `UndraError` whose message says the core trapped; `core.closed` is true and `onClose` fired; the Log
   adapter received a **fatal** (5) record containing `kaboom` before the trap.
3. The page can **restart**: a fresh `UndraCore.load` of the same module succeeds and the snapshot restores:
   `Todos` (re-created from the restored handle) shows the two items.
4. A second core loaded in the same process before the panic was not affected.
5. On the trapped core, calls through the generated bindings reject with `UndraCallError.Unavailable`
   (its `transport.reason` is `trap` or `closed`) and none hangs: an async call (`add_later`) and a typed one
   (`parse_count`); a store command (`Counter.increment`) resolves and `onError` received `Unavailable`.
6. Worker mode: boot the playground core with `mode: "wasm-worker"` (an in-thread worker over a
   `MessageChannel` is enough in the vitest harness, because the harness cannot load TypeScript in a real
   worker thread; the real thread is covered by the undra-ffi acceptance test,
   `crates/undra-ffi/tests/wasm/ts-runtime.test.mjs`). Observe a `RemoteTodosQueryHandle` (it reads the Clock
   on every observe) against the harness `FakeServer` (Http is an async port: it crosses to the main thread)
   and call `create_remote_todo` (it generates an idempotency key through the Rng). The core does not trap:
   `core.closed` stays false, the data arrives (the list, then the created item), and the core's own log
   records reach the harness Log adapter. (A worker that answered every port call "async" left the first
   clock read without an answer, and the core trapped.)

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

### S19 derived keyed list

`Todos.visible` is a derived list (ADR-039): it reaches the platform as keyed patches, never as a whole
list after the first; a derived list's patches are ordinary SPEC 3.8 patches, so every runtime's decoder,
applier and mirror (ADR-031) apply them unchanged.

1. `Todos` observed through a raw mirror: the initial `visible` (signal 2) entry is a full value
   (`op = 0`), `[]`.
2. `add("a")`, `add("b")`, `add("c")`: each change-set's `visible` entry is `op = 1` with exactly one
   `Insert` at index 0, 1, 2; `remaining` is 1, 2, 3.
3. `toggle(b)` with filter `All`: `visible` is one `Update{index=1}` (b, done); `remaining == 2`.
4. `set_filter(Active)`: the change-set's `visible` entry is one `Remove{index=1}`; `set_filter(All)`: one
   `Insert{index=1, b}`.
5. Under `Active`, `toggle(a)`: one `Remove{index=0}`; `toggle(a)` again: one `Insert{index=0, a}`.
6. `fill(10000)` (one full value or one patch for `visible`: either is allowed), then `toggle` of a visible
   item: the `visible` entry is `op = 1`, one op, **under 100 bytes**.
7. Through the generated class (`Todos.create()` / `Todos()`), the same steps and `fill`, `remove` and
   `clear_done`: `visible` equals the model (a local list mutated by the same steps) after every step,
   read-your-writes as in S18.
8. Reads never cross: reading `visible` 1,000 times leaves `crossings.calls` unchanged.
9. **Recorded patches.** `contract-tests/derived-vectors.sh` writes (with
   `cargo run -p undra-signals --example derived_vectors`) a recording of 60,000 seeded operations over
   three derived views (a filter; a parameter filter and a sort with heavy ties; a map and a sort on a
   `String`), which the Rust side checks against `filter + stable sort` after every operation: the
   change-sets a host received (about 24,000) and each view's FNV-1a 64 hash after each of them. The
   runner decodes and applies every change-set with its runtime's own change-set decoder, `decodePatch`
   and `applyPatch`, and after **every** change-set the hash of each view's encoding equals the recorded
   one. (TypeScript also feeds the change-sets through a `Mirror`, drained at seeded points, and checks
   at every drain: what ADR-031 merges per drain applies to the same views.)

### S20 storage failures are typed (ADR-049)

The storage ports have an error channel: an adapter that cannot store answers `StorageError`, and the core
neither panics nor traps (a wasm core would have trapped before ADR-049). List `s20`; the server serves one item.

1. The harness `Kv` fails every `set` with `StorageError.Full`. `remote_todos("s20")` is observed; it shows the
   item. After 300 ms (the write is debounced 250 ms) no key `undra.query.cache2.<query id>.*` of the `s20`
   entry is in the `Kv`; `storage_status().write_failed` grew by at least 1; the `Log` port received a WARN
   record of target `undra::query` whose message contains `storage is full`, exactly once however many writes
   failed; `stats().panics` did not grow and the core answers the next call. (In a core that has not yet
   stored the description of the query's type, the first failing write is the key `undra.types.<fingerprint>`,
   written before the entry, and the entry's own write is not attempted: either way no entry key is stored.)
2. The `Kv` heals. `invalidate()` on the handle refetches; within 5 s the `Kv` holds the `s20` entry, a value
   whose first two bytes are `02 00` (format 2) and whose fingerprint (bytes 10 to 18) is the one of the key
   `undra.types.<fingerprint>` it also holds. The data still shows.
3. A failed read of a cache entry starts it empty: (TypeScript) a fresh core whose `Kv` holds that entry and
   fails one `get` with `StorageError.Io("busy")` shows no data for `s20` until it fetched, and the entry's key
   is still in the `Kv` afterwards; (Swift, Kotlin) not run, the core is not reloaded.
4. An unreadable queue is never overwritten (ADR-049 decision 1.4).
   * TypeScript: a fresh core is loaded with a `Kv` whose `get` of `undra.query.queue2` fails `Locked`, and is
     told it is offline. `storage_status().queue_readable == false`. `save_note("s20", "late")` (POST failing
     with `HttpError.Network("offline")`) stays pending in memory: for 500 ms the `Kv` sees **no** `set` of
     `undra.query.queue2` and `pending == 1`. The `Kv` heals; `Lifecycle.changed(Active)`: within 5 s
     `queue_readable`, a `set` of `undra.query.queue2` (count 1); online: counted from the online event, the note
     replays exactly once (body `save:late`; its first attempt, made while offline, is not counted) and
     `pending == 0`.
   * Swift and Kotlin: the harness failed the first `get` of `undra.query.queue2` with `Locked` at load. Now
     `queue_readable == true`, the `Kv`'s operation log shows the failed `get`, then a successful `get` of the
     key, and no `set` of the key between the two.
5. No step of this scenario panicked or trapped (`stats().panics` unchanged), and the core is the one loaded
   at the start (TypeScript: the fresh cores of steps 3 and 4 are closed).

### S21 worker mode answers synchronous ports in the worker (ADR-049; TypeScript only)

The playground core in `wasm-worker` mode, loaded with `worker.ports` pointing at a module that registers the
playground's `Locale` port (`hello()` answers `"Hola"`).

1. `remote_todos("s21")` observed: it shows the server's data and `updated_at` is within a minute of `Date.now()`
   (the core read the `Clock` in the worker); `create_remote_todo("s21", "x")` reaches the server with an
   `Idempotency-Key` that is a UUID v4 made from the worker's `crypto.getRandomValues` (two calls carry
   different keys); a log record the core wrote reaches the main thread's `Log` adapter. Nothing trapped.
2. `localized_greeting("Ada") == "Hola, Ada"`: the app's synchronous port answered in the worker.
3. A second load in `wasm-worker` mode that registers `Locale` on the **main thread** (`adapters` or
   `registerPort` before load) fails at load with an error that names `Locale` (a generated adapter carries its
   port's name; a hand-written `PortImpl` without one is named by its id) and says to register it in
   `worker.ports`; no core is left running.
4. The worker protocol is version 3: the `init` message carries `asyncPorts` (the host's asynchronous ports, `Http`
   and `Kv` among them) and the `portsModule` URL.

### S22 a trapped web core restarts from its last snapshot (ADR-049; TypeScript only)

`wasm-main`, loaded with `recovery: { snapshotEveryMs: 50, maxRestarts: 3, perMs: 60_000 }`, `onCoreRestarted`,
`onError` and `onClose` recorded.

1. `Counter` observed, `add(5)`; `remote_todos("s22")` observed through its generated handle, showing the server's
   one item. Wait until the `s22` entry is persisted in the `Kv` (its write is debounced 250 ms; the re-created
   handle of step 4 reads it back) and a snapshot was taken.
2. A call is started and left in flight (`add_later(1, 2)` with a delay, or a `Probe.hang()`), then
   `explode("kaboom")` is called: it rejects as a failed call, the in-flight call rejects with
   `UndraTransportError("restarted")` (through generated code: `UndraCallError.Unavailable` whose transport reason is
   `"restarted"`), never retried.
3. Within 5 s `onCoreRestarted` was called once with the panic report (message containing `kaboom`), a
   `restoredFromAgeMs` under a few seconds, `rejectedCalls >= 1` and the stale objects; `onError` received an
   `UndraCoreRestarted` with the same.
4. The same `Counter` wrapper shows `count == 5` (restored, same handle) and `add(1)` makes it 6; the
   `remote_todos("s22")` handle was re-created (its wrapper still shows the item and, after the runner calls
   `configure_remote` again, since configuration is core state outside stores and is lost with the instance
   that trapped, `invalidate()` refetches through it); the `Probe` (not a store) is stale and fails with a typed
   refusal.
5. Three more `explode` calls within the minute: the third restart is the last; the fourth trap leaves the core
   dead: `onClose` reports the trap, `onCoreRestarted` was called 3 times in all, and calls fail as unavailable
   (`closed`).

## Platform notes

* TypeScript: S03 runs only in `wasm-main` mode (the only one with `callSync`); S17 step 6 is the only
  step that runs in `wasm-worker` mode. S15 uses the public `core.snapshot()` / `core.restore()` (SPEC 17.1),
  which both wasm modes have (a socket has none yet: `UndraModeError`).
* The Kotlin runner runs on the JVM with a single-thread "main" executor (`UndraDispatchers`), the Swift
  runner on the main actor; both load the real native library.
* S17 steps 5, 6 and 7 are native-only (Kotlin and Swift): the wasm core cannot call out of a panic
  into a synchronous port (step 5), and a trapped core has nothing left to shut down or reload (steps 6
  and 7; a fresh `UndraCore.load` after the trap is what wasm step 3 already does); the wasm step 5
  covers the same ground for a core that is gone. Steps 6 and 7 end and reload the core, so they are the
  last steps of the last scenario a native runner runs.
* S07 step 7 restores a snapshot the way S15 does on each platform (TypeScript through the wasm export).
* Every platform reports a **command** (a synchronous method that returns nothing and has no error type)
  through `LoadOptions.onError` / `onError` instead of throwing or rejecting (ADR-032, and its amendment A for
  Kotlin and TypeScript); each runner records those reports (`UndraUnhandledError`: operation, error), and
  S05.6, S15.9 and S17.6 assert them. A generated call fails with its own `E`, the caller's cancellation
  (`CancellationException`, `AbortError`, `CancellationError`) or `UndraCallError`, on all three.
* S19 step 9 reads `examples/playground/build/derived-vectors.bin` (`UNDRA_DERIVED_VECTORS` overrides it),
  which each `run.sh` writes through `contract-tests/derived-vectors.sh` when it is missing or older than the
  signals crate. Only TypeScript's mirror is public; Kotlin and Swift apply change-set by change-set.
* Timing constants (50 ms delays, 200 ms quiet windows) are chosen for a loaded CI machine; do not
  shrink them.
* S14 steps 7 to 9 and S15 steps 11 to 14 need build B (see "Two builds"). TypeScript loads both wasm
  modules in one process; Swift and Kotlin run build B's steps in a second process after the main run.
* S20 step 4 differs by platform: a fresh TypeScript core can be loaded with a `Kv` whose queue reads fail, so
  the TypeScript column walks the whole "unreadable, then readable on `Active`" path; Swift and Kotlin load one
  core per process, so their harness fails the first read of the queue at load and S20 checks what that did.
* S21 and S22 are TypeScript-only: worker mode and crash recovery are web features (ADR-049; a native core
  contains a panic without trapping, SPEC 5.6). `check.sh` does not expect them from Swift or Kotlin.
