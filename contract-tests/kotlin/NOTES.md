# Kotlin column of the contract tests

`run.sh` runs the eighteen scenarios of `../scenarios.md` on the JVM, through `dev.undra.runtime.UndraCore`
over the real JNI shim and the real `libundra_core` of `examples/playground/core`, and pipes the verdicts
through `../check.sh kotlin`. Sources are in `src/dev/undra/contract/`: one file per scenario
(`S01Primitives.kt` ... `S18CoalescedBurst.kt`), the harness (`Check.kt`, `Scenarios.kt`, `Main.kt`, `World.kt`) and
the fakes of scenarios.md's harness section (`ManualClock`, `FakeServer`, `MemoryKv`, `CapturingLog`).

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
  the in-memory `Http` server, an in-memory `Kv` that records writes, a capturing `Log`. `Rng` and `Timer` are
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

## Reading the scenarios on the JVM

* S01.4: a Kotlin `String` and a `UUID` cannot be mutated, so only `ByteArray` can be aliased; the check
  asserts the returned array is another array and that writing to it leaves the sent one alone.
* S03.2: `add` and `greet` are plain (non-`suspend`) functions; the scenario calls them from a plain function,
  which the compiler enforces. S03.3 prints the mean cost as `note S03 mean ... ns per sync add call`.
* S06.2 and S06.5: cancelling is `Deferred.cancelAndJoin()`; "after completion" is cancelling a finished
  `Deferred`, whose only observable effect would be `crossings.cancelled`, which is asserted not to move.
* S07.1: Kotlin has no way to "stop reading a `Flow` without cancelling" except not returning from the collector;
  the collector suspends inside `collect`, which also stops the runtime granting more credit.
* S07.6: the generated `Probe.ticksThenFail` maps the flag-2 item through `LabError.fromReply`, so the collection
  throws `LabError.Rejected(code = 7, reason = "stopped at 3")` itself.
* S07.7: "read 2 items" is the S07.1 idiom again: the collector suspends after its second item until the restore has
  returned, then reads on, so it drains what the core sent before the restore (at most the credit window) and then
  sees the failure. The core sends flag 3 with status 3, and the runtime raises `UndraReplyException(CANCELLED)`,
  which the generated `fromReply` passes through. The 1 s is measured from the start of `restore`. The restore is
  of a snapshot taken a moment before, so the stores of other scenarios keep their values; objects that are not
  stores (only the probe is still in use) are invalidated.
* S12.1: there is no `loading` status; the scenario's "loading" is `QueryStatus.FETCHING`. The GET is delayed
  by 50 ms so that the state in between can be seen.
* S13: `data` is recorded with a `StateFlow` collector on `Dispatchers.Unconfined` (`Recorder`), so every value
  the store sets is seen; the scenario spaces its changes by the 50 ms network delay.
* S05.6, S06.6, S15.9, S17.5 and S17.6 (ADR-032) are new coverage of Kotlin behaviour that did not change: every generated
  shape throws, so closed objects (S05.6) and stale handles (S15.9) are `UndraReplyException(BAD_REQUEST)`, a call in flight
  across a restore is `UndraReplyException(CANCELLED)` and a cancelled typed call is a `CancellationException`.
* S17.5: a call made from inside the `Log` port is refused by the Kotlin runtime itself (`InprocTransport` knows it is inside
  a callback and throws an `UndraException` "called from inside a core callback"), before the core can answer it with
  `E_REENTRANT`; the Swift column sees the core's bad request. The scenario accepts either refusal.
* S17.6 ends the core (`core.close()`), and S17 is the last entry of `SCENARIOS`, so nothing but S17.7 runs after it. The
  generated `add(1, 2)` passes the closed core explicitly, because `UndraCore.shared` is forgotten on close and would
  fail with "no UndraCore has been loaded" instead of "closed".
* S17.7: the timer-paced task is `Stress.start(StressMode.FIREHOSE, 1000u)`, started after step 5. Its generator
  reads the `Clock` port every 10 ms (the manual clock does not move, so it commits nothing), and the step waits for
  three such reads before step 6 starts the shutdown. The port-call counts are read when `close()` returns and must
  not change for 200 ms. On the JVM `InprocTransport.close()` detaches from the core before
  `UndraNative.shutdown()`, so a call during the shutdown itself would be answered "unavailable" by the transport,
  never by an adapter. The fresh core is loaded with the same `LoadOptions` (the same adapter instances), and its
  `add(1, 2)` gets the fresh core explicitly. It hydrates the query cache from the `Kv` contents S12 to S14 left, so
  `Kv` calls start again after the reload. That is the new core's own work, and it is why the quiet window ends
  before the load. Because the transport detaches first, neither port-call window can show a task of the old core
  that kept running: its calls go to the old, detached callbacks (never the fresh core's), and its sleeps run on the
  core's own timer thread. The check with teeth is the native core's own report: once `close()` returned,
  `UndraNative.statsJson()` (no core loaded) must say `runtime_threads == 0`, the `undra-core`, timer and blocking
  threads joined; the same after the fresh core's close. (Review of runtime-lifecycle: a mutant whose shutdown only
  released the global slot, leaving the generator running, passed both windows and the reload; it fails here.)
* S16: the order (S16 first) is described above. Step 2 ("a subsequent load succeeds") is the load every other
  scenario uses.

## Findings about what the runner needs from the runtime

* `UndraStats` has `liveHandles`, `transactions`, `panics` and a few more, but not the `crossings.*` counters
  scenarios.md counts (`calls`, `change_sets`, `cancelled`, `bad_requests`) nor `schema_hash`; `Stats.kt` reads
  them from `UndraStats.raw`, the core's statistics document, with a small JSON reader.
* There is no public way to wait until the mirror has applied everything delivered so far (`UndraCore.observe`
  does it internally). The runner posts a task to `UndraDispatchers.main` and waits for it.
