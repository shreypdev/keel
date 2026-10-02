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
| `wt/tp-ts` (hello app, no `Lazy`) | 69,667 | **21,602** (+32) |
| `wt/tp-ts`, an app that constructs a `LazyList` | 75,331 | 23,317 (the cost of using it, +1,715) |
| main `b800994` (for reference; the foundation predates ts-size-e4) | 68,868 | 21,344 |

`lazy.ts`, the new payload codecs and the React hooks are tree-shaken from a hello app (the skeleton and the whole of `lazy.ts`
measure +0). The +32 gzip bytes (+83 raw) are the seam in `core.ts` that a page call needs: the page target has a 21-byte layout
(`target, handle, offset, limit, call_id`), not the 17-byte head + arguments every other call has, so `encodeTarget` encodes it
with `encodeCall` and `callSync` must not take the head-parts fast path for it. The two checks are written as cheaply as they
can be (a cast, no `typeof`); an alternative with no growth would reach into `UndraCore`'s private members from `lazy.ts` or
patch its prototype, which I judged worse than 32 bytes. **The integrator should run `scripts/wasm-size.sh` on the merged tree:**
on main's current numbers the seam leaves about 124 bytes of the 21,500 budget.

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

1. **SPEC 11.1 / ADR-031 lose the page server's handle when an op 2 follows an op 0 in one drain.** "A full value (op 0) or a
   lazy invalidation (op 2) supersedes everything queued earlier for the key": with the value now carrying the handle, a drain that
   holds `[Full(handle H, len, v1), LazyInvalidated(len', v2)]` for one signal applies only the invalidation, and the host never
   learns `H` (the same after a restore, which re-sends op 0 with a new handle). Over `wasm-main` and React Native `observe` flushes
   inline so the value is applied before any later change; over `wasm-worker` and `remote` a commit right behind the first value
   can land in the same drain. `LazyList` reports an invalidation that arrives before any value (`UndraTransportError("protocol")`,
   through `onError`) rather than ignoring it silently, but cannot recover. The fix is in the folding rule of the three mirrors
   (keep the last op 0 as the base when an op 2 follows it, apply both) or in a core that sends the handle again; it is a SPEC
   11.1 change, so I left the TypeScript `Mirror` alone (its hot path is in the size gate). Swift and Kotlin have the same rule.
2. The brief's README: `runtimes/ts/@undra/runtime` had none; a short one was added.
3. `site/docs/api-typescript.html`, `site/docs/cookbook/pagination.html` and `docs/REACT_NATIVE.md` are outside this piece; they
   should list `LazyList`, `useLazyList`, `useLoadMore`.

## Checks and counts

* `runtimes/ts/@undra/runtime`: `npx vitest run`: **1,763 passed** (61 files; 103 are new: `lazy.test.ts` 76, `lazy-react.test.ts` 18,
  `lazy-adapters.test.ts` 3, `lazy-fuzz.test.ts` 6; the three `lazy *` wire vectors now check encode, decode and re-encode for real).
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
