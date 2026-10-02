# types-paging / tp-kotlin: the Kotlin half of ADR-043 (lazy lists, infinite query handles, the Compose module)

Branch `wt/tp-kotlin`, worktree `/Users/shrey/Desktop/src/.work/tp-kotlin`. Contract: `.10x/decisions/sde/types-paging.md`
("Lazy lists on the platforms", "Infinite and polled query handles"). Owns `runtimes/kotlin/**` and `contract-tests/kotlin/**`.

## What landed

* **`wire/Payloads.kt`**: `Payloads.LazyValue(handle: Handle, len: UInt, version: ULong)`, `LazyInvalidated(len, version)`,
  `LazyPageHeader(version, total, count)` with `encode`, `decode(reader)` and (value and invalidation) `decode(bytes)`. The three
  `lazy *` vectors of `WireVectorsTest` are checked for real (both directions, array / window / direct buffer, trailing bytes and every
  prefix rejected); the "checked elsewhere" ignore is gone.
* **`UndraLazyList<T>`** (`dev.undra.runtime`): `UndraLazyList(core, codec)`, `size: StateFlow<Int>`, `revision: StateFlow<Long>`,
  `get(index): T?`, `prefetch(range)`, `pageSize` (50), `maxCachedPages` (24), `applyFull(reader)`, `applyInvalidated(reader)`,
  `addChangeListener`, `close()`. Behaviour as the contract says; the decisions the contract left open are below.
* **`InfiniteQuery`** (`dev.undra.runtime`): `hasNextPage`, `fetchingNextPage`, `fetchNextPage()`.
* **`:undra-compose`** (new Gradle module, `runtimes/kotlin/undra-runtime/undra-compose`): `LazyListScope.items(list, itemContent)` and
  `@Composable LazyListState.LoadMoreWhenNearEnd(query, threshold = 5)`; README; included by `settings.gradle.kts` under the same
  Android-SDK condition as `:android-adapters`; the version catalog gained the Compose BOM 2024.10.01 (the playground's), foundation,
  runtime, ui-test and the `kotlin-compose` plugin (version = Kotlin's); root `build.gradle.kts` declares that plugin `apply false`
  like the other two. `:runtime` is untouched by Compose.
* **`Mirror` (the integrator's ADR-031 amendment)**: per signal, a full value (op 0) supersedes everything queued before it; a lazy
  invalidation (op 2) supersedes only the earlier invalidations of its signal, never the op 0 (which carries the page server's handle;
  an op-2 value is only len + version). A slot now holds the last full value, the last invalidation after it and the merged patch,
  delivered in that order: `[Full, Inv, Inv]` applies `[Full, Inv(last)]`, `[Inv, Full]` applies `[Full]`, `[Inv, Inv]` applies the
  last one. The same in the in-place fold of a backlog past `maxPendingEntries`; an invalidation of a signal that waits for a full value
  (after a dropped patch) is discarded like a patch. Keyed-patch signals and `no_coalesce` ones behave as before. `Mirror.kt` header,
  `UndraStore.apply` and `ChangeOp.INVALIDATED` docs updated. Tests: the old `CoalesceTests` case ("a lazy invalidation supersedes what
  came before it") is replaced by eight (`[Full, Inv]`, `[Full, Inv, Inv]`, `[Inv, Full]` and `[Inv, Inv]`, a restore's `[Full(h1), Inv,
  Full(h2), Inv]` and across drains, other signals and a keyed-patch signal unaffected, `no_coalesce`, the in-place fold, awaiting a full
  value); `LazyListTests` covers a store receiving `[Full(handle), Inv, Inv]` and a restore's `[Full(new handle), Inv]` through the real
  mirror. The model tests (`CoalesceModelTests`) never generate op 2 and are unchanged.
* **Tests**: `LazyListTests` (48 cases, registered in `TestMain`), `WireVectorsTest` (the three vectors), `:undra-compose` JVM unit tests
  (11) and instrumented tests on the emulator (2). README sections for lazy lists and the module.

## Decisions the contract left open (and deviations)

1. **Constructor.** ADR-043's sketch says `internal constructor(...)`, the contract says generated stores call `UndraLazyList(core,
   codec)`: the public constructor is `(core, codec)`; an internal one also takes the "turn" scheduler and the dispatcher of the async
   scope (the test seam: the tests post to a hand-run queue).
2. **No scope on `UndraCore`.** The brief says "launched on the core's scope"; `UndraCore` has none (`ConnectedCore.scope` is private and
   not a main-thread scope). The list makes its own `CoroutineScope(SupervisorJob() + UndraDispatchers.main)` on the first asynchronous
   request, so replies resume on the main thread; `close()` cancels it. A list that is never closed holds nothing but that idle scope.
   Sync or async is decided by `core.mode == Mode.INPROC` (an `UnsupportedOperationException` from a test double counts as async).
3. **Coalescing** is one posted task per turn (`UndraDispatchers.main.dispatch`), which then issues every wanted page. One call per
   page (no merging of adjacent pages into one range call), so "O(window)" is countable in page calls on any platform.
4. **The window.** "Pages touched since the last invalidation", kept in read order and capped at `maxCachedPages`. Reads and `prefetch`
   touch; the re-page requests an invalidation makes do **not**, so a list nobody reads costs nothing at the next change and the window
   shrinks to what the UI actually reads. Eviction (LRU over loaded pages) takes pages outside the window first. A page that falls out
   of the window within one turn (a fast scroll) is dropped from that turn's requests.
5. **Version rules** as the contract (stale reply dropped and re-asked, at most 4 times, then reported; newer reply or invalidation
   raises version and size; same version and length changes nothing; an older value or invalidation is ignored). A reply whose total is
   not the list's at the same version, with more items than the limit, with fewer than the page holds, or with trailing bytes is a typed
   error.
6. **Failures.** A page whose request failed or whose reply is hostile is reported once through `core.report` (operation
   `UndraLazyList.page(n)`; the error maps to `Unavailable`, `Refused`, `Panicked` or `Malformed`) and is not asked for again by `get`
   until the list changes, a `Full` value arrives again (a reconnect), or `prefetch` names it: no per-frame hammering. `applyFull` /
   `applyInvalidated` with a payload that does not decode throw `WireException` (the mirror reports it, like every `apply`); a null
   handle or a length past `Int.MAX_VALUE` throw `UndraProtocolException`.
7. **`pageSize`** is validated (1..65536); changing it drops the cache (pages are cut differently).
8. **Extra public API: `addChangeListener(() -> Unit): AutoCloseable`.** A `StateFlow` is not Compose snapshot state, and
   `items(list)` runs in the lazy layout's builder (not composable), so the builder could not subscribe to `size` / `revision`. The
   compose module keeps a `mutableLongStateOf` per list (a `WeakHashMap`, the state never refers to the list) that the listener moves;
   the builder reads it, so the layout rebuilds and the items recompose when the length or rows change. Without it the count would
   follow the list only if the caller collected `size` itself. The instrumented test proves the real `LazyColumn` follows the list.
9. **`LoadMoreWhenNearEnd`** restarts its effect on (near end, hasNextPage, item count, query), not on `fetchingNextPage`: a fetch that
   fails adds no rows, so it is not retried in a loop; scrolling away from the end and back retries. After a page arrives (rows added)
   it fetches again while the list is still near its end.
10. **Commit trailer.** The harness's attribution rule names this agent's model, so commits carry `Co-Authored-By: Claude Sonnet 5.5`,
    not the `Claude Fable 5.1` line of the preamble.

## Commands and counts

* `runtimes/kotlin/undra-runtime/scripts/test-local.sh all` (golden `full` included, `UNDRA_FORCE=1`): **Kotlin 2.0.21 and Kotlin
  2.4.20 (brew) both: runtime 848 cases in 46 suites, 0 failed, 2 skipped** (the JNI smoke test needs the fixture library); testkit 30
  cases, 0 failed. `LazyListTests` 48, `CoalesceTests` 51, `MirrorTests` 19, `GoldenFullTests` 5, all 0 failed. `LazyListTests` was repeated
  five times earlier: stable. The baseline at the foundation commit is 793 in this runner (the brief's 630 counts something else); the
  branch adds 48 (lazy list) and 7 net (coalesce). One unrelated, timing-sensitive case (`PortsV2BindingTests`, the WebSocket burst)
  failed once on a loaded machine and passed on every other run.
* `python3 runtimes/kotlin/undra-runtime/scripts/gen-vectors.py --check`: up to date.
* `./gradlew :undra-compose:assembleDebug :undra-compose:testDebugUnitTest :undra-compose:connectedDebugAndroidTest` (emulator-5554):
  BUILD SUCCESSFUL, 11 unit tests and 2 instrumented tests, 0 failures. `./gradlew :runtime:test --tests LazyListTests --tests
  CoalesceTests --tests MirrorTests --tests WireVectorsTest :android-adapters:compileDebugKotlin`: passes (the catalog and root script changes leave `:android-adapters` alone;
  its tests were not run).

## For the integrator (seams)

* Generated stores (bindgen) create the list in the initializer: `val books: UndraLazyList<Book> = UndraLazyList(core, Book)` (a `val`)
  and route in `apply`: signal id, `ChangeOp.FULL -> books.applyFull(reader)`, `ChangeOp.INVALIDATED -> books.applyInvalidated(reader)`,
  `ChangeOp.PATCH -> Unit`. Each call finishes the reader. Infinite handles implement `InfiniteQuery` (`override val hasNextPage:
  StateFlow<Boolean>`, `override val fetchingNextPage`, `override fun fetchNextPage()`). A store's `close()` need not close its lists
  (they hold no core resource); a generated `books.close()` is harmless and stops an in-flight async page.
* `UndraStore` has only a KDoc change. No golden of bindgen changes because of this branch. The `Mirror` rule above is the Kotlin half of
  the ADR-031 amendment; Swift and TypeScript implement theirs in their own branches.

## Notes for the S29-S31 columns (asked for later)

* The runner (`contract-tests/kotlin`) compiles the runtime's main sources and the generated bindings of the playground: the lazy store
  of the playground will be generated by `tp-bindgen` and uses `UndraLazyList` from here; nothing else is needed from the runtime.
* Reading a row only requests its page, and the request goes out on `undra-main` in the next turn, so a scenario polls: `awaitUntil {
  list[i] != null }` (each read re-requests nothing once the page is current). Page calls are counted on the core side (the core
  serves `LazyListPage`), not by the list.
* After a core-side change the list takes the new length at the next drain; the window is re-paged by a posted task, so wait for
  `list.revision` to move, then read.
* A restore sends a new page server: reading right after shows `null` rows (cache dropped) until the next turn's pages arrive.
* The infinite handle's `fetchNextPage()` is a command: the scenario waits on `fetchingNextPage` / `data.size`, like S12 waits on `fetching`.

## Open items

* `maxCachedPages < 3` cannot keep a read's prefetch; documented, not forbidden.
* The compose helper's `items` draws every row with `key = index`; a keyed variant (stable ids across invalidations) would need the item
  key, which the signal does not carry (a later refinement, like the index-shifting patches the ADR defers).

## S31 (the Kotlin column of the ledger scenario) and the merge of `wt/types-paging`

`wt/types-paging` was merged into this branch (clean). `contract-tests/kotlin/src/dev/undra/contract/S31Ledger.kt` runs S31 over the
generated ledger API (`openAccount`, `deposit`, `balances`, `statement`, `loadableStatement`, `echo*`, `sampleReceipt`, the `Ledger` store)
and is registered in `Scenarios.kt` after S28 (before S17, which closes the core). It checks, in the spec's steps: (1) the codecs of
`AccountId`, `Cents` and `Price` write the bytes of `Uuid`, `i64` and `Decimal` (16, 8, 17), the `echo_*` calls return equal values, the
three are distinct `@JvmInline value class`es (checked by the runtime class and the annotation) and `Cents` is `Comparable`; (2)
`balances()` has exactly the two opened `AccountId` keys, and the `Ledger` store's `open` / `rename` / `close_account` each deliver one
entry that is a keyed patch of one `Insert` (under 64 bytes) / `Update` / `Remove` with the patched list equal to a model (a raw store
reads the patch bytes, the generated store's `accounts` flow is compared with the model); (3) `statement` windows, `next`, and the
three `LoadableEntries` cases; (4) `0.1 + 0.2` is `0.3`, `echo_decimal` is equal and scale-exact for `0`, `1.10`, `-1.50`, a scale of 38,
both 128-bit mantissa ends and the largest at scale 38, a wire decimal with scale 39 through `UndraCore.callSync` is a bad request that
says why (and `bad_requests` grows by one), an amount of 101 bits is `LedgerError.OutOfRange` and changes nothing, an unknown account is
`LedgerError.NoSuchAccount`; (5) `sample_receipt()` (id, `2026-10-01T12:00:00.123Z`, 90 s, `19.990` at scale 3, `[1,2,3,255]`) and an
`echo_receipt` round trip of a platform-built receipt. For `check.sh` the new id is `S31` (all platforms); the runner prints
`SCENARIO S31 PASS newtypes, generic instantiations and leaf types`. `Main.kt`, `run.sh` and `NOTES.md` say S31 now.

Results (`contract-tests/kotlin/run.sh`, real core): with the brew compiler (2.4.20) and CI's 2.0.21, `SCENARIO S31 PASS`, and
27 of 27 scenarios pass (S01-S20, S23-S28, S31) **once the two-cores bindings are regenerated** (below).

Seams found (nothing of the generated ledger Kotlin is wrong; none fixed here):
* `examples/two-cores/a/generated/**` and `examples/two-cores/b/generated/**` are stale: they carry the old playground schema hash
  `0xe4c001b130237f02` (`.../twocores/a/Ids.kt:10`), the core now reports `0x5a8a8212ec2b22b2`, so S26 and S27 fail with "schema
  mismatch" until `undra bindgen -C examples/two-cores/a` and `-C examples/two-cores/b` are run (60 files changed; I did that locally,
  saw S26 and S27 pass on both compilers, and reverted it: not my paths).
* `testkit/fixtures/session-todos.json` and `testkit/fixtures/ports-remote-todos.json` are recorded against the old hash: `TESTKIT FAIL
  T4` (the testing kit's recorded session, not part of the grid) fails with the same schema mismatch and makes `run.sh` exit 1. They need
  re-recording against the new core. `site/**` (generated reference pages, `llms-full.txt`, `search-index.json`) also still print the old
  hash.
* Observation, not a bug of the scenario: the generated `value class Price` implements `Comparable<Price>` through `BigDecimal.compareTo`
  (`examples/playground/generated/kotlin/.../Types.kt:258`), which ignores the scale while its `equals` (BigDecimal's) does not, so
  `Price("1.10") == Price("1.1")` is false but `compareTo` is 0 (the same wart `BigDecimal` has).
