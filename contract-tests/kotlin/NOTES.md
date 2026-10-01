# Kotlin column of the contract tests

`run.sh` runs the eighteen scenarios of `../scenarios.md` on the JVM, through `dev.undra.runtime.UndraCore`
over the real JNI shim and the real `libundra_core` of `examples/playground/core`, and pipes the verdicts
through `../check.sh kotlin`. Sources are in `src/dev/undra/contract/`: one file per scenario
(`S01Primitives.kt` ... `S18CoalescedBurst.kt`), the harness (`Check.kt`, `Scenarios.kt`, `Main.kt`, `World.kt`) and
the fakes of scenarios.md's harness section (`ManualClock`, `FakeServer`, `MemoryKv`, `CapturingLog`).

## How it is arranged

* **One core per process, and S16 loads it.** `UndraCore.load` claims the native library for good, so S16 runs
  first: its failing load (`expectedSchemaHash = generated ^ 1`) leaves nothing behind (the runtime compares
  the hash before `undra_init`), and the load right after it is the one every other scenario uses. If S16 cannot
  load the core, the others fail with "the core is not loaded".
* **Generated bindings for what a UI does, `UndraCore` for the rest.** Stores are `Todos`, `Counter`,
  `BigList`, `RemoteTodosQueryHandle` and the free functions of the bindings. Raw signal entries (S08 to S11),
  raw calls (S03, S05), snapshot and restore (S15) and statistics use `RawStore` and `UndraCore` directly.
* **Adapters** go in through `LoadOptions(adapters = ...)`: a manual Clock (starts at `1_700_000_000_000` ms),
  the in-memory `Http` server, an in-memory `Kv` that records writes, a capturing `Log`. `Rng` and `Timer` are
  the runtime's JVM defaults, and so are `SecureStore` and `Fs` (kept in a throwaway `undra.data.dir`).
  The adapters are `PortImpl`s built on `StandardPorts` and `StandardRecords`; no transport is reimplemented.
* **Waiting** is a poll every 10 ms with a 5 s limit (`awaitUntil`, `awaitEq`), "for 200 ms nothing happens" is
  `holdsFor`. Each look first drains the mirror on the main thread (`drainLoadedMirror`): since ADR-031 a store
  shows what the core did on its own at the next frame (the runtime's 16.67 ms grid on the JVM), so without the
  drain a poll can see a value a frame old — S12 once read `fetching == false` before the change-set that set it
  to `true` had been applied, and refetched into a fetch still in flight. A scenario that runs longer than 120 s is reported as failed.
* **The main thread.** `UndraDispatchers.main` is the single `undra-main` thread of a JVM. The mirror applies
  change-sets there, at the next frame for what the core sends on its own (ADR-031), so a test that takes a
  mark in the list of raw entries first drains the mirror on that thread (`RawStore.mark` -> `flushMainThread`,
  which calls `mirror.flush()` there); without it a change-set of the previous step can arrive after the mark.
  S18 makes its calls on the main thread itself, where a synchronous call drains before it returns.

## Reading the scenarios on the JVM

* S01.4: a Kotlin `String` and a `UUID` cannot be mutated, so only `ByteArray` can be aliased; the check
  asserts the returned array is another array and that writing to it leaves the sent one alone.
* S03.2: `add` and `greet` are plain (non-`suspend`) functions; the scenario calls them from a plain function,
  which the compiler enforces. S03.3 prints the mean cost as `note S03 mean ... ns per sync add call`.
* S06.2 and S06.5: cancelling is `Deferred.cancelAndJoin()`; "after completion" is cancelling a finished
  `Deferred`, whose only observable effect would be `crossings.cancelled`, which is asserted not to move.
* S07.1: Kotlin has no way to "stop reading a `Flow` without cancelling" except not returning from the collector;
  the collector suspends inside `collect`, which also stops the runtime granting more credit.
* S12.1: there is no `loading` status; the scenario's "loading" is `QueryStatus.FETCHING`. The GET is delayed
  by 50 ms so that the state in between can be seen.
* S13: `data` is recorded with a `StateFlow` collector on `Dispatchers.Unconfined` (`Recorder`), so every value
  the store sets is seen; the scenario spaces its changes by the 50 ms network delay.
* S16: the order (S16 first) is described above. Step 2 ("a subsequent load succeeds") is the load every other
  scenario uses.

## Findings about what the runner needs from the runtime

* `UndraStats` has `liveHandles`, `transactions`, `panics` and a few more, but not the `crossings.*` counters
  scenarios.md counts (`calls`, `change_sets`, `cancelled`, `bad_requests`) nor `schema_hash`; `Stats.kt` reads
  them from `UndraStats.raw`, the core's statistics document, with a small JSON reader.
* There is no public way to wait until the mirror has applied everything delivered so far (`UndraCore.observe`
  does it internally). The runner posts a task to `UndraDispatchers.main` and waits for it.
