# types-paging-ts: the TypeScript and React Native half of ADR-043's lazy lists (`wt/tp-ts`)

Sub-piece `tp-ts` of `types-paging`. Owns `runtimes/ts/**`, `runtimes/rn/**` and `contract-tests/ts/**`. The contract is
`types-paging.md`, "Lazy lists on the platforms" and "Infinite and polled query handles"; this record holds what was built, what
the contract left open and how it was settled, the size proof, and what the scenario columns will need.

## What landed

`@undra/runtime` (`runtimes/ts/@undra/runtime`):

* `src/lazy.ts` (new): `LazyList<T>`, exported from the package root. `constructor(core, codec)`, `length: Signal<number>`,
  `revision: Signal<number>`, `get(index)`, `prefetch(start, end)`, `pageSize` (default 50, settable, 1..65,535),
  `maxCachedPages` (default 24, settable), `applyFull(reader)`, `applyInvalidated(reader)`.
* `src/wire/payloads.ts`: `LazyValue`, `LazyInvalidated`, `LazyPageHeader`, `LazyPage<T>` and their `encode*`/`decode*`
  (`readLazyValue`, `readLazyInvalidated`, `readLazyPageHeader` for a reader that is not exhausted, `encodeLazyPage(codec, page)`,
  `decodeLazyPage(codec, bytes, maxCount)`). The page call (`CallTarget.LazyListPage`) already had its encoder.
* `src/core.ts` (the one seam, see "Size"): `CallTargetRef` gains the page target `{ target: LazyListPage, handle, offset, limit }`;
  `encodeTarget` encodes it with `encodeCall`; `callSync` does not take the parts fast path for it.
* `src/react.ts`: `useLazyList(list)` returning `{ length, getItem }` and `useLoadMore(query, { rootMargin })` returning a ref
  callback; types `LazyListView<T>`, `LoadMoreQuery`, `LoadMoreOptions`. `src/vue.ts`, `svelte.ts`, `solid.ts`: a doc paragraph each
  (a `LazyList` needs nothing there); `test/lazy-adapters.test.ts` proves it.
* `README.md` (new; the package had none): the lazy list and infinite query section.
* `package.json` `exports` unchanged.

`@undra/react-native`: no code change, one test file (`test/lazy-list.test.ts`), see "React Native".

## Behaviour: what the contract left open, and how it was settled

* **Requests.** `get` and `prefetch` never call into the core: they queue page numbers and a microtask sends them, lowest page
  first, one page call per page (adjacent pages are not merged into one call: a page is the unit of the version and of the cache).
  A page queued, in flight or failed is not queued again. Synchronous (`core.callSync`) where the transport has it; the first
  `UndraModeError` makes the list ask asynchronously (`core.call`) from then on. Reads made while a synchronous reply is being
  applied (`callSync` flushes the mirror, which may deliver an op 2) are safe: the batch is snapshotted and re-checked.
* **What a read asks for.** The page that holds the row if it is absent or stale, and the pages before and after it if they are
  **absent** (a stale neighbour is left until a read wants it: it is not part of the window, and re-paging it would triple the cost of
  a change). Pages past the end are never asked for.
* **The window.** A page is in the window when a read (or `prefetch`) wanted it since the previous change; pages that were only
  prefetched as neighbours are not. A change (`applyInvalidated`, a newer `applyFull`, a page reply with a newer version) takes the
  length and version at once, asks again for the window's pages, drops pages past the new end and starts a new window; stale rows
  stay visible until the replies land. A reader that stops reading leaves the window, so a quiet list costs nothing per change.
* **Eviction.** The cache is a `Map` in order of touch. Over `maxCachedPages` the pages outside the window go first (oldest first),
  then the oldest of the window; "never evicted while in the window" holds as long as the window fits (a window larger than
  `maxCachedPages` is trimmed to it, oldest read first).
* **Versions.** A reply older than the list's version is dropped and the page asked for again, at most twice (a core that keeps
  answering below what it announced is reported as a protocol error and the page is not asked again until the list changes). A reply
  at the list's version whose `total` disagrees with the announced length, or whose `count` is not `min(limit, total - offset)`, or
  which exceeds the limit, is an error. A newer reply raises length and version exactly like an op 2, re-pages the rest of the
  window, and the op 2 that follows (same version) changes nothing.
* **Restart.** `applyFull` with a handle other than the one the list has (including the first) clears cache, queue, in-flight
  bookkeeping and bumps a generation so replies to older requests are dropped; the next read asks the new server. The same handle
  and a newer version is a change; the same value again changes nothing except that pages whose request failed are tried again
  (what a reconnect's re-observe delivers).
* **Failures** (a page call refused, a transport that cannot send, an undecodable or inconsistent reply, a closed core) go through
  `UndraCore.report(error, "LazyList.page")`, so `onError` and the log see them; `get` never throws. A failed page is not asked for
  again until the list changes, a value is re-applied, or `prefetch` asks. `applyFull`/`applyInvalidated` throw a `WireError` for a
  malformed value (as generated stores' `decodeValue` does) and leave the list as it was. An invalidation that arrives before any
  value (no page server) is reported and ignored (see Findings).
* **`useLoadMore`.** An observer exists only while the sentinel is attached, the query has a next page, is not fetching and (if
  the query has an `error` signal, as generated handles do) has no error: a failing server is not hammered while the sentinel
  stays in view; the app offers a retry. When a page arrives and the sentinel is still in view the effect re-runs (its inputs
  changed), the new observer reports the sentinel at once and the next page follows. `error?: Signal<unknown>` is the one optional
  addition to the structural type of the contract.
* **`useLazyList`.** Subscribes to `length` and `revision` with `useSignal` (so `useSyncExternalStore`, no tearing, SSR-safe);
  `getItem` is a new function after every arrival so memoised rows that take it re-render; `null`/`undefined` give an empty view.

## Size (ADR-052/056)

`scripts/web-size-runtime.mjs` (the JS half of `scripts/wasm-size.sh`) on the hello project of `target/wasm-size/hello` (the
`web/src/undra.ts` of an `undra init` app), `@undra/runtime` from this worktree, up-front chunk after minify, gzipped as the script does:

| tree | raw | gzip |
|---|---|---|
| foundation `6013803` | 69,584 | **21,570** |
| `wt/tp-ts` (hello app, no `Lazy`), with the `core.ts` seam | 69,667 | 21,602 (+32) |
| `wt/tp-ts` with the mirror amendment (final) | 69,714 | **21,638** (+68 over the foundation) |
| `wt/tp-ts`, an app that constructs a `LazyList` | 75,331 | 23,317 (the cost of using it, +1,715) |
| main `b800994` (for reference; the foundation predates ts-size-e4) | 68,868 | 21,344 |

`lazy.ts`, the new payload codecs and the React hooks are tree-shaken from a hello app (the skeleton and the whole of `lazy.ts`
measure +0). The +32 gzip bytes (+83 raw) are the seam in `core.ts` that a page call needs: the page target has a 21-byte layout
(`target, handle, offset, limit, call_id`), not the 17-byte head + arguments every other call has, so `encodeTarget` encodes it
with `encodeCall` and `callSync` must not take the head-parts fast path for it. The two checks are written as cheaply as they
can be (a cast, no `typeof`); an alternative with no growth would reach into `UndraCore`'s private members from `lazy.ts` or
patch its prototype, which I judged worse than 32 bytes. **The integrator should run `scripts/wasm-size.sh` on the merged tree:**
on main's current numbers (21,344) the seam and the mirror amendment together leave about 88 bytes of the 21,500 budget.

## React Native

Nothing is needed. A page call is an ordinary `Call` payload (target 3); `NativeTransport` is `synchronous` and has `callSync`,
which hands the payload to the module's `callSync` (`undra_call_sync`), and neither the TypeScript transport nor the C++
module (`UndraJsi.cpp`, `UndraHost.cpp`: they forward the bytes of a call and never read its target) looks at it. Op 0 and op 2
arrive through the inbox like every change-set. `test/lazy-list.test.ts` (5 tests) drives a `LazyList` through `NativeTransport`
over `FakeNative` (a scripted page server): the batched synchronous page calls, the 21-byte payload, an op 2 from a core thread
re-paging the window, a refused call reported, a core stopped under the runtime reported. The generated TypeScript bindings and
the React hooks are the web ones, so `docs/REACT_NATIVE.md` needs no change beyond a mention that `useLazyList` works there
(not edited: not mine).

## Findings for the integrator

1. **ADR-031 amendment (integrator's decision, implemented in the TypeScript `Mirror`): a full value supersedes everything before
   it for its signal; a lazy invalidation supersedes only earlier invalidations, never the full value** (an op-2 value is only
   length and version, the op 0 carries the page server's handle). The finding was that SPEC 11.1's old rule lost the handle when
   `[Full(handle), LazyInvalidated]` reached one drain (over `wasm-worker`/`remote`, or after a restore that re-sends op 0).
   `Slot.full` is now a list of what to deliver, in order (the value, the last invalidation, or both: `setFull` is one line);
   `_applySlot`, `_compact`, `_requeue` and `_markDropped` iterate it. `[Full, Inv]` delivers both, `[Full, Inv, Inv]` the value and
   the last invalidation, `[Inv, Full]` the value, `[Inv, Inv]` the last, `[Full, Inv, Full]` the last value; patches and
   ordinary signals are untouched; a compaction keeps the pair. Tests: `coalesce.test.ts` (the old "an invalidation supersedes what
   came before it" is replaced by six cases of the new rule, incl. a compaction), `coalesce-model.test.ts` (every store has a lazy
   signal: invalidations and restarts with a new handle in the random histories; it fails with the old rule), `lazy.test.ts`
   (a `LazyList` behind the real mirror: first value + invalidation in one drain, a restart's handle + invalidation in one drain).
   `LazyList` still reports an invalidation that arrives before any value. React Native has no folding of its own (its inbox
   keeps commit order, the TypeScript `Mirror` folds), so it follows. Size of the mirror change: +36 gzip bytes (21,602 -> 21,638);
   the first versions (a `base` and an `inv` field: +86 and +75) were rewritten as the list to get here.
2. The brief's README: `runtimes/ts/@undra/runtime` had none; a short one was added.
3. `site/docs/api-typescript.html`, `site/docs/cookbook/pagination.html` and `docs/REACT_NATIVE.md` are outside this piece; they
   should list `LazyList`, `useLazyList`, `useLoadMore`.

## Checks and counts

* `runtimes/ts/@undra/runtime`: `npx vitest run`: **1,770 passed** (61 files; 110 are new or changed: `lazy.test.ts` 78, `lazy-react.test.ts` 18,
  `lazy-adapters.test.ts` 3, `lazy-fuzz.test.ts` 6, six lazy cases in `coalesce.test.ts`, a lazy signal in `coalesce-model.test.ts`; the three `lazy *` wire vectors now check encode, decode and re-encode for real).
  `npm run typecheck` (tsconfig, build, vite) and `npm run build` pass. There is no lint script in the package.
* `runtimes/rn/@undra/react-native`: `npm test` 97 passed (92 before, 5 new); `npm run typecheck` passes.
  `cpp/test/run.sh` (ASan and UBSan, the playground cores prebuilt in the main checkout through `UNDRA_CORE_DYLIB` and
  `UNDRA_SECOND_CORE_DYLIB`): exit 0, every `ok` (C++ untouched).
* `runtimes/ts/@undra/testkit`: 32 passed, `tsc` clean. `contract-tests/ts`: `tsc --noEmit` clean (the scenarios need the wasm
  builds and were not run: nothing in S01..S28 changed).

## Notes for the scenario columns (S29 to S31, to come)

* `contract-tests/ts` boots the playground core (`wasm-main`, `src/harness.ts`) and uses the generated store classes; the
  scenarios will read a generated `Lazy<T>` property as a `LazyList`. `list.length.peek()`, `list.get(i)` after
  `await Promise.resolve()` (the flush is a microtask; a synchronous core has answered by then), and `list.revision` are what an
  assertion reads; the page calls are visible as `Kind.Call` envelopes with target 3 (`src/raw-store.ts` counts entries the same way:
  op 0, op 2 and the value's `handle/len/version` can be decoded with `decodeLazyValue` / `decodeLazyInvalidated`).
* "O(window)" is checkable by counting page calls: wrap the transport the way `bootRaw()` does and count `Kind.Call` payloads whose
  first byte is 3 (`CallTarget.LazyListPage`).
* Worker mode (`bootWorker()`) answers asynchronously (`UndraModeError` from `callSync`, then `core.call`): the lazy scenarios can
  run there too and exercise the stale-reply path. `remote` is the same.
* The playground store with a `Lazy<T>` and the infinite query handle (`FeedQuery`-shaped, with `fetchNextPage()`) come from the
  integrator's merge of bindgen and the core; `useLoadMore` has no scenario (it is UI) and is covered by `lazy-react.test.ts`.

## S31 in the TypeScript and React Native columns (after the merge of `wt/types-paging`)

* `wt/tp-ts` is `wt/types-paging` (be0602f) plus `contract-tests/ts/test/s31-newtypes-generics-leaf-types.test.ts`,
  `contract-tests/ts/{run.sh,NOTES.md}` (comments and a note) and `runtimes/rn/@undra/react-native/vitest.contract.config.ts`
  (S31 added to its include list). `check.sh` and `run-all.sh` untouched: they need `S31` in `IDS` for `ts` (and for `rn`, whose own
  list does not run S20 and S23 to S26 either).
* `contract-tests/ts/run.sh` (real wasm builds of the playground core, build B, and two-cores a and b): S01 to S25, S28, S31 and the
  web-only S21 and S22 PASS. **S26 and S27 FAIL, only because `examples/two-cores/{a,b}/generated/**` were not regenerated** ("the
  bindings expect 0xe4c001b130237f02 but the core reports 0x5a8a8212ec2b22b2": the playground core gained the ledger). `undra bindgen -C
  examples/two-cores/a` and `.../b` (28 files written each, Swift, Kotlin and TypeScript) make both pass; I reverted that regeneration
  (not my files). The RN contract column: the same list plus S31, 20 PASS, S17 SKIP (app-tested), S27 FAIL for the same reason.
* No generated-code problem in the ledger TypeScript bindings: they type-check under the contract package's strict `tsc` (the four
  `@ts-expect-error` lines of `distinctTypes()` all fire, so the brands are distinct types), the `Map<AccountId, Price>` of
  `balances()` (`objects.ts:1596`, `codecs.map(AccountIdCodec, PriceCodec)`) is a real `Map` keyed by the plain string, `Price`
  comes back as a `Decimal` instance, keyed patches are one op each. Style notes only: the brand is the string-literal property
  `__brand` (`types.ts:37`, `:47`, `:238`), which autocomplete shows on the value; a `unique symbol` brand would hide it.
* `examples/playground/web`: `npm run build` (tsc + vite) passes and `npm test` is 124 passed (11 files) against the new bindings.

## S32, S33, the playground screens (after the merge of tp-lazy, tp-query and the playground's `paging.rs`)

* **S32 and S33 in `contract-tests/ts`** (`test/s32-paged-queries-lazy-lists.test.ts`, `test/s33-polling.test.ts`; `src/paging-steps.ts`,
  `src/page-calls.ts`, `tapEntries` in `src/raw-store.ts`; how each step is read is in NOTES.md). The S32 steps also run on `wasm-worker`
  (`test/paging-worker.test.ts`, not a scenario); the React Native contract column runs both scenarios (`vitest.contract.config.ts`).
* **`LazyList.version`** (a public read-only getter, for the scenario and tools): the version of the list the host has seen.
* **A refused page call is quiet** (`lazy.ts`, `_failedPage`). Closing a store while a page was asked for (a view unmounting, the scenario
  closing its library) made the next flush fail with "stale handle", reported through `onError`; a refusal (reply status 5) now means the page
  server is gone: the list stops asking, quietly, until a value names a page server again (`applyFull` revives it). Any other failure is still
  reported. Found by S32 (the harness fails a scenario on an unreported runtime error), and React's StrictMode double mount would have shown it
  to every app.
* **S32.2 as written does not hold for edge pages**: "reading any row of those pages afterwards makes none". A read of a loaded page asks for the
  page beyond it when that is not loaded (the prefetch that keeps scrolling ahead; the Swift engine does the same, `UndraLazyListEngine.item(at:)`),
  so a row of page 1 or 3 asks for page 0 or 4, once. TypeScript reads page 2 for "none" and counts the two edge pages; the scenario text should say so.
* **The playground web app** (`examples/playground/web`): three tabs, `?screen=library|feed|ticker` (or `#library`, `#feed`, `#ticker`):
  `LibraryView` (`useLazyList(library.books)` drawn as a window by index with placeholders, the `evens` view and the buttons that change them),
  `FeedView` (`FeedQueryHandle`, `useLoadMore` on a sentinel, footer, refresh, "touch even rows", "even rows only"), `TickerView`
  (`TickerQueryHandle`, `setPollInterval` as an override switch, "fail on purpose"). `src/paging.ts` holds the logic that is tested in Node
  (`paging.test.ts`); `smoke/smoke.spec.ts` has one test per screen. No dependency was added.
* Observations (not bugs): a `refetch()` asked for while one is in flight is the one in flight, so a change made in between is not fetched
  (the Feed disables its buttons while `fetching`); `setPollInterval(3 s)` reschedules the pending poll to three seconds after the last fetch
  ended (S33.5 waits for it); `Library._signals` in the generated store lists only `source` (`stores.ts`, `this._signals = [this.source]`), not
  in signal-id order, which nothing reads.
