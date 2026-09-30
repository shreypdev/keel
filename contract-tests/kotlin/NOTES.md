# Kotlin column of the contract tests

`run.sh` runs the seventeen scenarios of `../scenarios.md` on the JVM, through `dev.keel.runtime.KeelCore`
over the real JNI shim and the real `libkeel_core` of `examples/playground/core`, and pipes the verdicts
through `../check.sh kotlin`. Sources are in `src/dev/keel/contract/`: one file per scenario
(`S01Primitives.kt` ... `S17Panic.kt`), the harness (`Check.kt`, `Scenarios.kt`, `Main.kt`, `World.kt`) and
the fakes of scenarios.md's harness section (`ManualClock`, `FakeServer`, `MemoryKv`, `CapturingLog`).

## How it is arranged

* **One core per process, and S16 loads it.** `KeelCore.load` claims the native library for good, so S16 runs
  first: its failing load (`expectedSchemaHash = generated ^ 1`) leaves nothing behind (the runtime compares
  the hash before `keel_init`), and the load right after it is the one every other scenario uses. If S16 cannot
  load the core, the others fail with "the core is not loaded".
* **Generated bindings for what a UI does, `KeelCore` for the rest.** Stores are `Todos`, `Counter`,
  `BigList`, `RemoteTodosQueryHandle` and the free functions of the bindings. Raw signal entries (S08 to S11),
  raw calls (S03, S05), snapshot and restore (S15) and statistics use `RawStore` and `KeelCore` directly.
* **Adapters** go in through `LoadOptions(adapters = ...)`: a manual Clock (starts at `1_700_000_000_000` ms),
  the in-memory `Http` server, an in-memory `Kv` that records writes, a capturing `Log`. `Rng` and `Timer` are
  the runtime's JVM defaults, and so are `SecureStore` and `Fs` (kept in a throwaway `keel.data.dir`).
  The adapters are `PortImpl`s built on `StandardPorts` and `StandardRecords`; no transport is reimplemented.
* **Waiting** is a poll every 10 ms with a 5 s limit (`awaitUntil`, `awaitEq`), "for 200 ms nothing happens" is
  `holdsFor`. A scenario that runs longer than 120 s is reported as failed.
* **The main thread.** `KeelDispatchers.main` is the single `keel-main` thread of a JVM. The mirror applies
  change-sets there, so a test that takes a mark in the list of raw entries first waits for that thread
  (`RawStore.mark` -> `flushMainThread`); without it a change-set of the previous step can arrive after the mark.

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

* `KeelStats` has `liveHandles`, `transactions`, `panics` and a few more, but not the `crossings.*` counters
  scenarios.md counts (`calls`, `change_sets`, `cancelled`, `bad_requests`) nor `schema_hash`; `Stats.kt` reads
  them from `KeelStats.raw`, the core's statistics document, with a small JSON reader.
* There is no public way to wait until the mirror has applied everything delivered so far (`KeelCore.observe`
  does it internally). The runner posts a task to `KeelDispatchers.main` and waits for it.
