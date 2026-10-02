# Kotlin column of the contract tests

`run.sh` runs the scenarios of `../scenarios.md` that Kotlin runs (S01 to S20, S23 to S25 for the opt-in ports, S26,
S27, S28, S31 to S33 and S35) on the JVM, through `dev.undra.runtime.UndraCore` over the real JNI shim and the real `libplayground_core` of
`examples/playground/core`, and pipes the verdicts through `../check.sh kotlin`. Sources are in `src/dev/undra/contract/`:
one file per scenario (`S01Primitives.kt` ... `S19DerivedKeyedList.kt`, `S20Storage.kt`, `S23`..`S25`, `S26TwoCores.kt`,
`S35QueryHandles.kt`), the harness (`Check.kt`, `Scenarios.kt`, `Main.kt`, `World.kt`, `Handover.kt`), the fakes of
scenarios.md's harness section (`ManualClock`, `FakeServer`, `MemoryKv`, `CapturingLog`) and the build-B process of S14, S15
and S35 (`MigrationBuildB.kt`).

## How it is arranged

* **One core at a time, and S16 loads it.** `UndraCore.load` claims the native library until that core is closed
  (ADR-034), so S16 runs first: its failing load (`expectedSchemaHash = generated ^ 1`) leaves nothing behind (the
  runtime compares the hash before `undra_init`), and the load right after it is the one every other scenario uses.
  If S16 cannot load the core, the others fail with "the core is not loaded". S17, the last scenario, closes that
  core and loads a fresh one with the same options (`World.options`).
* **Generated bindings for what a UI does, `UndraCore` for the rest.** Stores are `Todos`, `Counter`,
  `BigList`, `RemoteTodosQueryHandle` and the free functions of the bindings. Raw signal entries (S08 to S11),
  raw calls (S03, S05), snapshot and restore (S15) and statistics use `RawStore` and `UndraCore` directly.
* **Adapters** go in through `LoadOptions(adapters = ...)`: a manual Clock (starts at `1_700_000_000_000` ms),
  the in-memory `Http` server, an in-memory `Kv`, a capturing `Log`. The `Kv` (`MemoryKv`) records every operation
  (`get`, `set`, `delete`, `list`, with the key and the failure it answered with) and fails on demand with a
  `StorageError` (`fail(kind, key, error, times)`, `heal()`); it is a `KeyValueBackend` served through the runtime's
  own table, `StoragePort.KV.portImpl(backend)`, so a failure reaches the core exactly as `FileKv`'s or the Android
  adapters' do (port status 1 and the encoded error). Its first `get` of `undra.query.queue2` fails `Locked`
  (`Bootstrap`), as on Swift: S14 waits until the core read the queue again, S20 step 4 checks what it did meanwhile. `Rng` and `Timer` are
  the runtime's JVM defaults, and so are `SecureStore` and `Fs` (kept in a throwaway `undra.data.dir`).
  The adapters are `PortImpl`s built on `StandardPorts` and `StandardRecords`; no transport is reimplemented.
  Each of the four is wrapped by `PortCallCounter` (`World.portCalls`), which counts the calls it receives by port
  name, for S17.7.
* **Waiting** is a poll every 10 ms with a 5 s limit (`awaitUntil`, `awaitEq`), "for 200 ms nothing happens" is
  `holdsFor`. Each look first drains the mirror on the main thread (`drainLoadedMirror`): since ADR-031 a store
  shows what the core did on its own at the next frame (the runtime's 16.67 ms grid on the JVM), so without the
  drain a poll can see a value a frame old — S12 once read `fetching == false` before the change-set that set it
  to `true` had been applied, and refetched into a fetch still in flight. A scenario that runs longer than 120 s is reported as failed.
* **The main thread.** `UndraDispatchers.main` is the single `undra-main` thread of a JVM. The mirror applies
  change-sets there, at the next frame for what the core sends on its own (ADR-031), so a test that takes a
  mark in the list of raw entries first drains the mirror on that thread (`RawStore.mark` -> `flushMainThread`,
  which calls `mirror.flush()` there); without it a change-set of the previous step can arrive after the mark.
  S18 makes its calls on the main thread itself, where a synchronous call drains before it returns, and adds a
  Kotlin-only last check that does not drain: a burst made off the main thread must reach its store at a frame of
  the runtime's own pacer, so the frame path stays covered although every wait drains.

## Two builds (S14 steps 7 to 9, S15 steps 11 to 14, S35 step 10)

`run.sh` builds the core twice with the undra CLI: build B first (`UNDRA_PLAYGROUND_V2=1 undra build --platform host`,
which the core's `build.rs` turns into `cfg(playground_v2)`; the CLI rebuilds when only that variable changes), copied
to `build/core-b`, then build A, copied to `build/core-a`. Each JVM loads its own copy (`-Djava.library.path`), so a
later `undra build` by another runner cannot swap a library under a run. The script stops if the two are identical.

The main JVM runs the scenarios against build A. S14 step 7 writes `build/migration/s14.json` (every key and value of the
harness `Kv` once both notes wait in the queue, in hex, and the `Idempotency-Key` of the failed `save_note` POST); S15
step 11 writes `s15.json` (snapshots `P` and `L` and the `Profile` handle); S35 step 2 writes `s35.json` (the snapshot, as hex, and
the values of the remote, ticker, feed, library and roster handles, as decimal strings). Each scenario deletes its file first, and
`run.sh` deletes the directory before the run (`UNDRA_CONTRACT_HANDOVER` names it), so a failed scenario never hands
over an older run's data.

Then a second JVM runs `Main` with `UNDRA_CONTRACT_PHASE=B` and build B's library (`MigrationBuildB.kt`). It refuses to
run against build A (it compares `undra_schema_hash` with the generated hash: both ids then print `FAIL`), loads build
B with the hash the library reports and with a fresh `ManualClock`, `FakeServer`, `CapturingLog` and a `MemoryKv`
holding exactly the handed-over contents (and no injected failure), emits `Connectivity.changed(false, None)` right
after the load, and drives the core through `UndraCore`'s raw API (`configure_remote`, `storage_status`, `add` and
`Profile.describe` by `fnv1a32` id; the generated `StorageStatus`, `RemoteConfig`, `QueryStatus` and `RemoteTodo` only as
codecs). It prints `SCENARIO S14 FAIL` / `SCENARIO S15 FAIL` / `SCENARIO S35 FAIL` lines when a build-B step fails and an
informational `MIGRATION S14 build B ok: <the dead letter>` / `MIGRATION S15 build B ok` / `MIGRATION S35 build B ok`
otherwise; its output is appended to `build/run.log`, which `check.sh` reads (the last line of an id counts). S14 and S15 share
the first core; S35 step 10 gets a new one (see the S35 notes below).

## Reading the scenarios on the JVM

* S01.4: a Kotlin `String` and a `UUID` cannot be mutated, so only `ByteArray` can be aliased; the check
  asserts the returned array is another array and that writing to it leaves the sent one alone.
* S03.2: `add` and `greet` are plain (non-`suspend`) functions; the scenario calls them from a plain function,
  which the compiler enforces. S03.3 prints the mean cost as `note S03 mean ... ns per sync add call`.
* S06.2 and S06.5: cancelling is `Deferred.cancelAndJoin()`; "after completion" is cancelling a finished
  `Deferred`, whose only observable effect would be `crossings.cancelled`, which is asserted not to move.
* S07.1: Kotlin has no way to "stop reading a `Flow` without cancelling" except not returning from the collector;
  the collector suspends inside `collect`, which also stops the runtime granting more credit.
* S07.6: the generated `Probe.ticksThenFail` maps the flag-2 item through `UndraCallError.mappedStream(error, LabError)`, so the collection
  throws `LabError.Rejected(code = 7, reason = "stopped at 3")` itself.
* S07.7: "read 2 items" is the S07.1 idiom again: the collector suspends after its second item until the restore has
  returned, then reads on, so it drains what the core sent before the restore (at most the credit window) and then
  sees the failure. The core sends flag 3 with status 3, the runtime raises `UndraReplyException(CANCELLED)`,
  and the generated `mappedStream` makes it `UndraCallError.CancelledByCore`. The 1 s is measured from the start of `restore`. The restore is
  of a snapshot taken a moment before, so the stores of other scenarios keep their values; objects that are not
  stores (only the probe is still in use) are invalidated.
* S12.1: there is no `loading` status; the scenario's "loading" is `QueryStatus.FETCHING`. The GET is delayed
  by 50 ms so that the state in between can be seen.
* S13: `data` is recorded with a `StateFlow` collector on `Dispatchers.Unconfined` (`Recorder`), so every value
  the store sets is seen; the scenario spaces its changes by the 50 ms network delay.
* S05.6, S15.9, S15.10, S16.5, S17.1/2/5/6 (ADR-032, amendment A) read the failure model of the generated bindings: a call
  fails as `UndraCallError` (closed objects and stale handles are `Refused`, a call in flight across a restore is
  `CancelledByCore`, a panic is `Panicked`, a closed core is `Unavailable` with transport reason `CLOSED`), a cancelled typed
  call is a `CancellationException` (S06.6, unchanged), and a command (`Counter.increment()`, `Probe.reset()`) returns and
  reports to `LoadOptions.onError`: `World.takeUnhandled()` returns what the Bootstrap's handler recorded. The raw statuses
  of `UndraCore.callSync` and `construct` (S05.4, S15.7) are still `UndraReplyException`.
* S17.5: a call made from inside the `Log` port is refused by the Kotlin runtime itself (`InprocTransport` knows it is inside
  a callback and answers with the same bad request the core would: `UndraCallError.Refused`, reason `E_REENTRANT`), before the
  core can; the Swift column sees the core's own refusal. The scenario asserts `E_REENTRANT` on both.
* S17.6 ends the core (`core.close()`), and S17 is the last entry of `SCENARIOS`, so nothing but S17.7 runs after it. The generated
  `add(1, 2)` passes the closed core explicitly; `UndraCore.shared` is forgotten on close and is the closed placeholder
  afterwards (`UndraCore.current == null`), which the step checks with a generated constructor.
* S17.7: the timer-paced task is `Stress.start(StressMode.FIREHOSE, 1000u)`, started after step 5. Its generator
  reads the `Clock` port every 10 ms (the manual clock does not move, so it commits nothing), and the step waits for
  three such reads before step 6 starts the shutdown. The port-call counts are read when `close()` returns and must
  not change for 200 ms. On the JVM `InprocTransport.close()` detaches from the core before
  the core's `UndraCoreNative.shutdown()`, so a call during the shutdown itself would be answered "unavailable" by the transport,
  never by an adapter. The fresh core is loaded with the same `LoadOptions` (the same adapter instances), and its
  `add(1, 2)` gets the fresh core explicitly. It hydrates the query cache from the `Kv` contents S12 to S14 left, so
  `Kv` calls start again after the reload. That is the new core's own work, and it is why the quiet window ends
  before the load. Because the transport detaches first, neither port-call window can show a task of the old core
  that kept running: its calls go to the old, detached callbacks (never the fresh core's), and the sleep it had set
  on the default `Timer` adapter is never reported back to the closed core. The check with teeth is the native core's own report: once `close()` returned,
  `UndraCoreNative.statsJson()` (no core loaded) must say `runtime_threads == 0`, the `undra-core`, timer and blocking
  threads joined; the same after the fresh core's close. (Review of runtime-lifecycle: a mutant whose shutdown only
  released the global slot, leaving the generator running, passed both windows and the reload; it fails here.)
* S16.5: S16 runs first, so `UndraCore.shared` is still the placeholder after its failing load; the step checks that
  generated calls with the default core fail `Unavailable` (reason `CLOSED`) and that the placeholder never becomes `current`.
* S16: the order (S16 first) is described above. Step 2 ("a subsequent load succeeds") is the load every other
  scenario uses.
* S14.6: the core empties the offline queue by *deleting* `undra.query.queue2`; the scenario accepts that or a written
  empty queue. As on Swift, the Kotlin column also checks that the first non-empty write of the queue (taken from the
  `Kv`'s log before the network returns) holds one item, and that the `undra.types.<fingerprint>` key of that item's
  fingerprint (the `u64` at offset 18) was written before it. The query handle is closed however the steps end, so a
  failure in S14 does not leave a handle that S15.8's count of live handles sees.
* S14.7 replays build A's notes after the handover: once the `Kv` contents and the idempotency key are written, the
  notes' POST answers 201 and the device goes online, so both notes replay in build A and the queue is empty for the rest
  of the run (S17.7's reload would otherwise hydrate and retry it, and S20's failing writes would meet it). Build B
  starts from the contents kept before that.
* S15.11 and S15.14 run in build A after step 10 (`Profile` "ada" visited twice, snapshot `P`; a `Legacy` of score 5,
  snapshot `L`; the 8 zero bytes refused with `UndraRestoreException.BAD_SNAPSHOT`); 12 to 14 run in build B.
* S20 runs its native variant: steps 1, 2, 4 (the harness failed the first `get` of `undra.query.queue2` with `Locked`
  when S16 loaded the core) and 5; step 3 prints nothing. Step 1 also checks that a write of the `s20` entry was
  attempted and failed `Full`. The entry's key is computed (`undra.query.cache2.<query id>.<fnv1a64 of the encoded
  arguments>`). S20 runs after S18 and before S17, which shuts the core down.
* S35 (ADR-059) steps 1 to 9 use the generated wrappers and restore into this core. Step 2 hands the snapshot and the five
  handle values over to the build-B process (`Handover.QueryHandles`). Steps 3 and 8 need "the entries the mirror applied to
  the remote and feed wrappers during the restore" and a wrapper's `apply` is protected, so `Tap` replaces the wrapper's mirror
  registration with one that records each entry and then calls the wrapper's own `apply` (the generated override, found by
  reflection: its name is mangled because it takes a `UInt`); the wrapper behaves as it would without the tap (steps 4 and 6
  read what it shows). A `StateFlow` does not repeat an equal value, so a `Recorder` on `remote.data`, `remote.status` and
  `feed.data` only shows blinks; the tap is what proves nothing was sent. The reads wait for a 200 ms quiet window
  (`quietFor`, `holdsFor` with nothing to check) because a fetch a restore started would only show at the server a moment later.
  `live_handles` after the first restore is the reading before it less one (the `Probe`, a plain object the restore makes stale);
  the second restore is compared with that reading. The counter is moved to 6 before the second restore so that it has
  something to put back. Step 1's feed: S32 leaves the `feed/false` entry with its pages in the query cache, so the handle may
  show them at once; step 1 asks for pages only until the feed has 100 rows.
* S35 step 10 (build-B process) needs a fresh runtime: it counts what a restore adds (7: `Counter`, `Library` and its two page
  servers, and the three query handles build B honours), and the core S14's and S15's steps used still holds the stores S15's
  restore of `P` made. So the process closes that core and loads another with an empty `Kv`, a new `FakeServer` and a new
  `CapturingLog` (one core per process, loaded one after the other, as S17.7 does), then reads `live_handles` and restores.
  Observing the remote handle is the first use of it, and it must see `fetching` with `data` absent: the server answers the list
  after 300 ms so that the mirror cannot fold the fetch's result into the first drain. The roster handle is refused through
  `callSync` of its `refetch`: `UndraReplyException`, status `BAD_REQUEST`, and the `CapturingLog` holds the WARN `restore:
  Handle(index=<i>, gen=<g>) (0x..) is not re-issued: the types it was made from changed`.

## The opt-in ports (S23, S24, S25; ADR-047, ADR-048)

* **Adapters.** S23 and S24 use the defaults `UndraCore.load` installs on a JVM (`JvmAdapters.standard`):
  `ClientWebSocketAdapter`, the runtime's own RFC 6455 client, and `JdkHttpSseAdapter` (`java.net.http`). Both are served by
  the runtime's bindings (`WebSocketPortAdapter`, `SsePortAdapter`). S25 registers `dbPort(JdbcDbAdapter(<temp dir>))` itself
  (`core.registerPort`) and deletes the directory afterwards. The scenarios run after S18 and before S17 (which ends the core).
* **The server.** `RealtimeServer` starts `contract-tests/servers/realtime-server.mjs` with Node on first use
  (`--port 0 --exit-on-stdin-close`, `READY <port>`); the runner closes its stdin before exiting. `/stats` is read with the
  runner's `Json`.
* **S23.3 (credit).** The default adapter's reader thread reads ahead at most the room of the binding's buffer (the window of
  the core's latest pull, 16 before the first), and a `receive` answers a burst as one reply (it waits until `max` are there,
  2 ms pass with nothing new, or 8 ms after the first). Without the coalescing the first pull raced the reader and saw a few
  messages, and the core pulled 3 times; with it `pulls()` is 1 or 2. A flood of 2,000 messages of 64 KiB under a stalled
  reader lets the server write only what fits in the socket buffers (the runtime's `RealtimeAdapterTests`).
* **S23.4.** The refused upgrade's status is the HTTP status line's (`WebSocketUpgradeException`): 401.
* **S23.5.** `Live.abandon()` drops the Rust connection, whose `Drop` closes it through the port with 1001; the binding's close
  waits for the server's echo, so `/stats` shows 1001 at once.
* **S24.** On the JVM the SSE adapter is `java.net.http`, not `HttpURLConnection`: the JDK's `HttpURLConnection.disconnect()`
  waits for a blocked read of a chunked body to return (measured: it was still blocked after 3 s against `/sse/hang`), so
  step 3's "the server saw the client leave" could not hold. Android's `HttpURLConnection` aborts the read
  (`UrlConnectionSseAdapter`, the Android default; `android-adapters`' `RealtimeOnDeviceTest` checks it on the emulator).
  The parser starts its last event id from the request's `Last-Event-ID` (the HTML standard keeps it across reconnections),
  which is why step 2's `three` carries id `"2"`. An `id` field with an empty value resets it: the event's `id` is then `null`.
* **S25** needs the SQLite JDBC driver on the class path (`org.xerial:sqlite-jdbc`, which `:runtime` does not depend on):
  `run.sh` adds `$UNDRA_SQLITE_JDBC` (scripts/env.sh sets it). Without it S25 reports `SKIP no SQLite JDBC driver on the class
  path`, or fails with `UNDRA_REQUIRE_TOOLCHAINS=1`. sqlite-jdbc reports the extended result code through
  `org.sqlite.SQLiteException.getResultCode()` (read by reflection: no compile-time dependency) and wraps SQLite's message as
  `[SQLITE_X] description (message)`; `DbError` carries the inner message. Its `executeQuery` refuses a statement without a
  result set (`PRAGMA foreign_keys = ON`), so the adapter runs every query with `execute()`.
* **S25.2.** The scenario checks that the mirror holds both notes (the first `add`, onto an empty list, arrives as a full value).

## Findings about what the runner needs from the runtime

* `UndraStats` has `liveHandles`, `transactions`, `panics` and a few more, but not the `crossings.*` counters
  scenarios.md counts (`calls`, `change_sets`, `cancelled`, `bad_requests`) nor `schema_hash`; `Stats.kt` reads
  them from `UndraStats.raw`, the core's statistics document, with a small JSON reader.
* There is no public way to wait until the mirror has applied everything delivered so far (`UndraCore.observe`
  does it internally). The runner posts a task to `UndraDispatchers.main` and waits for it.
