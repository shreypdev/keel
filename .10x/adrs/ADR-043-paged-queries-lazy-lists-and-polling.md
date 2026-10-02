# ADR-043: interval polling, infinite queries as keyed lists, and `Lazy<T>` for lists the host pages through

Status: **Accepted (implemented by `types-paging`, 2026-10-02; the deviations are at the end)** (proposed 2026-10-01, `wt/boundary-adrs`; Amendment B "data layer completion", catalogue M-1 and
finding 2, gap audit RX-2; re-scopes Track E3). Touches SPEC 2.1/2.2 (`QueryDef` gains two optional fields;
`Lazy<T>` becomes a legal store signal), 3.1/3.3/3.5 (the `Lazy<T>` value, the page reply, op 2 carry a length
and a version), 4.3, 4.5 (`interval`, `infinite`, `#[undra(cursor)]`), 9, 10, 11.1, 12 (E0001's `Lazy` text
goes; E0073) and 17; `undra-meta`, `undra-macros`, `undra-signals`, `undra-runtime`, `undra-query`,
`undra-bindgen`, the three runtimes and a new optional Compose helper module. **Wire: three encodings that
nothing emits today change (the `Lazy<T>` value, the lazy page reply, op 2's value), in the pre-publication
wire revision of Amendment C. Schema: optional fields written only when set; no existing hash moves. No C ABI
or wasm ABI change.** Constitution R5, R11, R12.

## Context

SPEC 9 lists "`interval_ms` if set" among the refetch triggers, but the code says "neither `QueryDef` nor the
schema carries an interval, so timed refetching is not in the v1 contract" (`crates/undra-query/src/lib.rs:35-36`,
`:86-87`); there is no periodic loop anywhere in `undra-query` (the persist loop is a debounce, `shared.rs:1140-1183`).
The query macro accepts `key`, `stale`, `persist`, `retry`, `idempotent` (`crates/undra-macros/src/impl_/query.rs:93-170`).

Paging has every low-level piece and no product:

* `TypeRef::Lazy` exists (`crates/undra-meta/src/type_ref.rs:78`), the wire reserves `Lazy<T>` as a `u64` handle
  (SPEC 3.1), a page call (target 3: `handle, offset, limit`, SPEC 3.3) and change-set op 2 "lazy list
  invalidated (value empty; the host re-pages)" (SPEC 3.5); the runtime has `LazyList` (pre-encoded items,
  `page(offset, limit)` → `total, count, items`, `crates/undra-runtime/src/lazy.rs:29-127`) and answers page
  calls itself (`crates/undra-runtime/src/runtime.rs:1168-1179`). Swift, Kotlin and TypeScript decode op 2 and
  the page target.
* But the macro refuses it: "`Lazy<T>` is not available in v1: lazy lists cannot be mirrored yet … use a
  `Vec<T>`, or a method that takes an offset and a limit and returns one page" (`crates/undra-macros/src/impl_/types.rs:664-672`);
  bindgen refuses it ("lazy lists need a runtime API that SPEC section 17 does not define yet",
  `crates/undra-bindgen/src/validate.rs:815-820`); op 2 is never emitted; no platform has an API.
* The only "page" today is an ordinary query keyed by a page number (the `queries` golden: `todos:{page}`
  returning `Page { items, total }`), so an infinite feed is one handle per page, stitched together by app
  code on every platform.

The catalogue: "It has no paged or infinite queries: `Lazy<T>` is rejected in v1, but E3 ('lazy-list
ergonomics') is scoped as if it existed" (finding 2); matrix row 10 "Paged and infinite lists, and polling":
KMP yes (Jetpack Paging), Undra no. The blueprint claim "Lazy collections … a 50,000-row table is never
serialized" is on the list the post must not reuse (§11.2). ADR-039 §8 left the composition of `Lazy<T>` with
derived lists to this ADR.

Two different needs hide under "paging", and they want different machinery:

* **A server feed** (timeline, search results): pages come from the network one at a time, the user scrolls a
  few hundred rows, and the host must render every loaded row. That is a **keyed list that grows**; ADR-027
  already sends an append as O(page) recorded ops.
* **A large local collection** (50,000 rows from `Db`, a derived view over them): the data is already in the
  core and the host must not hold or receive it all. That is **`Lazy<T>`**: the host pages a window.

## Decision

### 1. Interval polling

1. `#[undra::query(key = "..", interval = "30s")]` (≥ `1s`; below is E0040 with "use a stream for real-time
   data"), optionally with `poll_in_background`. Schema: `QueryDef.interval_ms: Option<u64>` and
   `QueryDef.poll_in_background: bool`, both written only when set. `QueryDef` (the trait) gains
   `const INTERVAL_MS: Option<u64> = None` and `const POLL_IN_BACKGROUND: bool = false`.
2. **Semantics** (TanStack's `refetchInterval`, not fixed-rate): while an entry has at least one observer, the
   client believes the device is online, and the app is `Active` (or the query polls in background), the next
   refetch is scheduled `interval` after the **end** of the previous fetch (success, or failure after its
   retries). A tick that finds a fetch in flight does nothing. `Background`/`Inactive` and offline pause it;
   `Active` and online resume it with the existing triggers (stale refetch; online refetches all observed),
   then reschedule from the end of that fetch. Releasing the last observer cancels the timer.
3. **Per-observer override.** Each query handle gets a `set_poll_interval(Option<Duration>)` method (a fixed
   method id, `fnv1a32("QueryHandle.set_poll_interval")`, beside `refetch` and `invalidate`); the entry polls at
   the smallest interval among its observers (the attribute is each new observer's default). A screen polls
   faster only while it is visible by setting it in `onAppear` and clearing it in `onDisappear`.
4. **Determinism (R12).** Timers are `ctx.sleep` (the Timer port), time is the Clock port; the polling task
   holds a `WeakCtx` (ADR-034). Under `FakeClock` a test advances time and sees each poll.

### 2. Infinite queries

1. **Shape.**

   ```rust
   #[undra::query(key = "feed/{filter}", infinite, item_key = "id", stale = "1m")]
   async fn feed(ctx: &Ctx, filter: Filter, #[undra(cursor)] cursor: Option<String>) -> Result<Page<Post, String>, ApiError> { .. }
   ```

   `undra::query::Page<T, C = String> { pub items: Vec<T>, pub next: Option<C> }` is a plain Rust struct the
   query macro recognises by spelling in the success type of an `infinite` query; it is never a schema type,
   so it needs no generic instantiation (ADR-042). Exactly one parameter is `#[undra(cursor)]` of type
   `Option<C>` (`None` = first page); it is not part of the key and not a platform parameter. `item_key` names
   the field of `T` that identifies a row (required: keyed patches and list identity need it). Violations are
   **E0073** (what/why/fix).
2. **Schema.** The `QueryDef` keeps `params` without the cursor, `returns` = `Vec<T>` (what the handle's `data`
   carries), and gains `infinite: Option<InfiniteDef { cursor: TypeRef, item_key: String }>` written only when
   set.
3. **The handle.** Signals 0–4 as for every query (`data`, `status`, `error`, `fetching`, `updated_at`), with
   `data: Vec<T>` **keyed by `item_key`** (empty, not `None`, before the first page), plus 5 `has_next_page:
   bool` and 6 `fetching_next_page: bool`. Methods: `refetch`, `invalidate`, `set_poll_interval` and
   `fetch_next_page` (fixed id `fnv1a32("QueryHandle.fetch_next_page")`).
4. **Cache entry.** The pages (each its cursor and its items) and the next cursor; `data` is their
   concatenation, held in a keyed `Signal<Vec<T>>` so it is also a valid ADR-039 source for a Rust store
   (`ctx.query().infinite::<FeedQuery>(params).items()` → `.derive().filter(..)`).
   * `fetch_next_page` with a next cursor and no fetch in flight fetches it and **appends with the recorded
     `push` operation**: the change-set is the new rows (ADR-027), O(page), however long the list is. A failure
     keeps `data`, sets `error` and clears `fetching_next_page`; retries as for any fetch.
   * A **refetch** (stale, invalidate, foreground, online, poll) re-fetches the loaded pages in order from the
     first, each with the cursor the previous response returned (cursors can change), up to the number of
     pages loaded or `refetch_pages = N` if set (then the rest is dropped). The result replaces `data` with a raw
     write, so the commit diffs by key and sends only what changed (ADR-027's raw path; a full value when more
     than half changed). `data` stays visible while `fetching`.
   * Mutations: `CacheView::update_items::<Q>(params, |items: &mut Vec<T>| ..)` edits the flattened list with
     recorded operations inside the optimistic transaction; rollback is the existing write-stamp rule.
5. **Persistence.** `persist` on an infinite query stores the first `persist_pages` pages (default 1) in
   ADR-037's format (the entry data is the pages and the next cursor; the fingerprint is the closure of `T` and
   `C`). A cold start shows the first page at once and refetches when stale.
6. **Not in v1.x:** previous pages (bidirectional paging), `max_pages` (it needs previous pages to be
   reversible), and an infinite query whose rows outgrow memory (use a store with a `Lazy<T>` fed by the
   query).

### 3. `Lazy<T>`: a list the host pages through

1. **Rust.** `Lazy<T>` (in `undra-signals`, re-exported) is a core-owned list with the recorded API of
   `Signal<Vec<T>>` (`push`, `insert`, `remove`, `update_at`, `move_item`, `clear`, `replace`, `len`,
   `with_range`), legal as a `#[undra::store]` field (`#[undra(key = "id")]` optional). Items are stored typed;
   a page is encoded on request (a 50-row page is microseconds). `Lazy::over(&derived)` is a read-only lazy
   view of an ADR-039 `DerivedList<T>` that pages through the derived index (`select(k)`, O(log n + limit));
   it lands after ADR-039's implementation. The runtime's `LazyList` becomes the type-erased page server
   (`Arc<dyn LazySource>`: `len`, `version`, `encode_page`).
2. **Wire** (in the pre-publication revision; nothing emits these today):
   * the `Lazy<T>` value (op 0 of its signal): `handle u64, len u32, version u64`;
   * op 2 (`LazyInvalidated`): `len u32, version u64` (was empty);
   * the page reply (target 3): `version u64, total u32, count u32, items` (`version` added in front).
   A host therefore knows the new length when it applies op 2, without a round trip, and can tell which version
   a page was read at. ADR-031's merge rules are unchanged (op 2 supersedes earlier entries for its key).
3. **Commit.** Any change to a `Lazy<T>` marks its slot dirty; an observed slot's commit sends op 2 with the
   new length and version — O(1) bytes whatever changed. The host keeps showing the rows it has (stale while it
   re-pages) and re-requests the pages of its visible window, so a change costs O(window), never O(list).
   (Index-only patches that let the host shift its cached pages without re-paging are a later refinement; the
   window is bounded by the screen.)
4. **Snapshots.** A `Lazy<T>` is store state: the snapshot carries its items, ADR-037's fingerprints cover
   `Lazy(T)`, and restore re-sends op 0 with the new page-server handle (the handle is transient).
5. **Generated shapes.** A `Lazy<T>` signal is a runtime type on each platform, not generated code:

   ```swift
   @MainActor @Observable
   public final class UndraLazyList<Element: UndraCodec & Sendable>: RandomAccessCollection {
       public var startIndex: Int { 0 }
       public var endIndex: Int { count }
       public private(set) var count: Int
       /// The row, or `nil` while its page loads; reading an unloaded row requests its page.
       public subscript(position: Int) -> Element? { get }
       public var pageSize: Int                      // default 50; one page of prefetch on each side
       public func prefetch(_ range: Range<Int>)
   }
   // a store: public private(set) var books: UndraLazyList<Book>
   ScrollView { LazyVStack {          // a lazy container: SwiftUI's `List` builds every row up front (see the deviations)
       ForEach(0..<library.books.count, id: \.self) { i in
           if let book = library.books[i] { BookRow(book) } else { BookRow.placeholder }
       }
   } }
   ```

   ```kotlin
   class UndraLazyList<T> internal constructor(…) {
       val size: StateFlow<Int>
       operator fun get(index: Int): T?           // null while loading; requests the page
       fun prefetch(range: IntRange)
   }
   // undra-compose (new optional module, Compose dependency):
   fun <T> LazyListScope.items(list: UndraLazyList<T>, itemContent: @Composable LazyItemScope.(index: Int, item: T?) -> Unit)
   ```

   ```ts
   export class LazyList<T> {
     readonly length: Signal<number>;
     get(index: number): T | undefined;            // undefined while loading; requests the page
     prefetch(start: number, end: number): void;
   }
   // @undra/runtime/react
   export function useLazyList<T>(list: LazyList<T>): { length: number; getItem(index: number): T | undefined };
   // for TanStack Virtual / react-window: useVirtualizer({ count: length, ... })
   ```

   Page requests are synchronous calls where the transport allows (`callSync` in process and in
   `wasm-main`), else asynchronous; requests are coalesced per drain.

### 4. Generated infinite-query shapes

```swift
@MainActor @Observable
public final class FeedQuery: UndraStore, @unchecked Sendable {
    public private(set) var data: [Post] = []
    public private(set) var status: QueryStatus = .idle
    public private(set) var error: ApiError? = nil
    public private(set) var fetching: Bool = false
    public private(set) var updatedAt: Date? = nil
    public private(set) var hasNextPage: Bool = false
    public private(set) var fetchingNextPage: Bool = false
    public convenience init(filter: Filter, ctx: UndraCore = .shared) throws
    public func fetchNextPage()                                     // a command (ADR-032)
    public func refetch()
    public func invalidate()
    public func setPollInterval(_ interval: Duration?)
    /// Fetches the next page when `item` is within `threshold` rows of the end.
    public func loadMore(ifNeededFor item: Post, threshold: Int = 5)
}
List(feed.data) { post in PostRow(post).onAppear { feed.loadMore(ifNeededFor: post) } }
```

(`Post` gains `Identifiable` when its `item_key` field is named `id`; otherwise views pass `id: \.<key>`.)

```kotlin
class FeedQuery private constructor(core: UndraCore, handle: Long) : UndraStore(core, handle), InfiniteQuery {
    val data: StateFlow<List<Post>>; val hasNextPage: StateFlow<Boolean>; val fetchingNextPage: StateFlow<Boolean>
    // status, error, fetching, updatedAt as for every query
    override fun fetchNextPage(); fun refetch(); fun invalidate(); fun setPollInterval(interval: Duration?)
    companion object { fun create(filter: Filter, ctx: UndraCore = UndraCore.shared): FeedQuery }
}
// undra-compose: @Composable fun LazyListState.LoadMoreWhenNearEnd(query: InfiniteQuery, threshold: Int = 5)
```

```ts
export class FeedQuery extends UndraStore {
  readonly data: Signal<Post[]>; readonly hasNextPage: Signal<boolean>; readonly fetchingNextPage: Signal<boolean>;
  static create(filter: Filter, core?: UndraCore): Promise<FeedQuery>;
  fetchNextPage(): Promise<void>; refetch(): Promise<void>; invalidate(): Promise<void>;
  setPollInterval(ms: number | null): Promise<void>;
}
// @undra/runtime/react: useLoadMore(query, { rootMargin }) → a ref callback for a sentinel element (IntersectionObserver)
```

## Alternatives considered

* **Infinite queries as `Lazy<T>`.** The host must render every loaded feed row anyway, and a lazy list would
  add a round trip per page window on top of the network page; appending recorded ops is already O(page).
* **One handle per page (today's pattern), with platform helpers to stitch them.** Every platform reimplements
  refetch order, cursor chaining and identity across pages; the cache cannot refetch consistently.
* **Jetpack Paging's `PagingSource` as the Kotlin API.** Paging owns the data and loads pages itself; Undra's
  core owns the data and the mirror already has it. A `PagingSource` adapter over `UndraLazyList` can be added
  in a separate artifact if teams ask; it is not the primary API.
* **Fixed-rate polling** (`setInterval` semantics). Overlaps when a fetch is slower than the interval and
  hammers a struggling server; TanStack chose end-to-start for the same reason.
* **Keep op 2 empty and let the host ask for the length.** Costs a round trip per change and flicker on the
  asynchronous transports; three nothing-emits-it encodings can change now for free, later only with a wire
  break.
* **Keyed patches for `Lazy<T>` with item bodies.** The core does not know which rows the host holds; sending
  bodies would send rows the host never asked for. Op 2 plus window re-paging is bounded by the screen.

## Consequences

* Polling, infinite feeds and large local lists exist on all three platforms with native shapes; SPEC 9 stops
  promising something the code lacks; catalogue row 10 moves to "yes"; the blueprint's lazy-collections claim
  becomes true (as `Lazy<T>`).
* The `QueryHandle` signal table grows two signals for infinite queries; ordinary query handles are unchanged.
* A new optional Kotlin module, `undra-compose` (Compose helpers for `UndraLazyList` and `InfiniteQuery`), keeps
  the runtime module at stdlib + coroutines (CLAUDE.md). `@undra/runtime/react` gains `useLazyList` and
  `useLoadMore`.
* E3 ("lazy-list ergonomics") is this ADR's implementation.

## Risks

* **Refetching many pages** after a long scroll costs one request per page (as in TanStack); `refetch_pages`
  bounds it, and the docs say so.
* **Row identity in `UndraLazyList`** is by index for rows not yet loaded; SwiftUI animations across a re-page
  can be wrong. Keys of loaded rows are known; a keyed identity API can follow.
* **Wire timing.** The three encodings must ride the ADR-036/037 revision or wait for a major version. If that
  revision ships before this ADR is implemented, its implementers should already land the encodings (they are
  codec-only changes with vectors) so this ADR stays wire-free.

## Implementation brief

1. `crates/undra-meta`: `QueryDef.interval_ms`, `poll_in_background`, `infinite: Option<InfiniteDef>` (+ meta
   mirrors), all written only when set; validation (`Lazy<T>` legal as a signal; `item_key` names a field of the
   item record); hash-stability tests.
2. `crates/undra-wire`: the three encodings (`LazyValue`, op 2 value, page reply) with proptests and vectors in
   `contract-tests/wire-vectors.json`.
3. `crates/undra-macros`: `query.rs` (`interval`, `poll_in_background`, `infinite`, `item_key`,
   `refetch_pages`, `persist_pages`, `#[undra(cursor)]`, `Page<T, C>` recognition, E0073, E0040's interval
   floor); `types.rs`/`store.rs` (accept `Lazy<T>` as a signal; remove the "not available in v1" text).
4. `crates/undra-signals`: `Lazy<T>` (recorded ops, version counter, `attach_lazy` on `StoreCell` emitting op 2
   on commit); later `Lazy::over(&DerivedList<T>)` with ADR-039's index.
5. `crates/undra-runtime`: `LazySource` page server replacing `LazyList`'s pre-encoded storage; page dispatch
   writing the version; restore re-sends op 0.
6. `crates/undra-query`: the polling scheduler per entry (observer intervals, lifecycle/connectivity pause,
   end-to-start scheduling, `WeakCtx`); infinite entries (pages, cursors, `fetch_next_page`, sequential
   refetch, recorded appends, `update_items`), persistence of the first pages in ADR-037's format; the new fixed
   method ids in the dispatch layer.
7. `crates/undra-bindgen`: infinite handles (two signals, three methods, `loadMore(ifNeededFor:)`,
   `Identifiable` when the key is `id`), `setPollInterval` on every handle, `Lazy<T>` signals as runtime
   `UndraLazyList`/`LazyList` properties; remove the `validate.rs:815-820` refusal. Golden cases `polling`,
   `infinite`, `lazy`.
8. Runtimes: Swift `UndraLazyList` (+ ADR-045's `ObservableObject` variant), Kotlin `UndraLazyList` and the
   `InfiniteQuery` interface, the new `undra-compose` module, TS `LazyList`, `useLazyList`, `useLoadMore`.
9. Contract scenarios (provisional numbers): **S23 "lazy list"** (observe, page a window, a change sends op 2
   with the new length, re-page; a page reply's version), **S24 "infinite query"** (first page, `fetchNextPage`
   appends as a patch, refetch re-chains cursors and sends only the change, persisted first page after a
   restart), **S25 "polling"** (two polls at a 1 s interval; a `Background` event pauses, `Active` resumes; the
   per-observer override).
10. Bench: `lazy/page_50_of_100k` (core half ≤ 20 µs), `lazy/invalidate` (≤ 1 µs, 12 bytes),
    `query/infinite_append_page_50` (O(page): ≤ 2× a 50-op keyed patch); rows in `bench/RESULTS.md`.
11. Docs: SPEC 2, 3, 4, 9, 10, 11.1, 12, 17; the cookbook's "pagination and infinite lists" (H1) and "polling"
    pages.

## Dependencies

ADR-034 (`WeakCtx` for polling tasks), ADR-037 (the persisted format of infinite entries and of `Lazy` in
snapshots), ADR-039 (`Lazy::over(&DerivedList)` follows its implementation). The wire part rides the
ADR-036/037 revision. Polling (decision 1) is independent and can land first.

## Implementation (2026-10-02, `wt/types-paging`) and deviations

All eleven items of the brief landed (records: `.10x/decisions/sde/types-paging.md`, `types-paging-lazy.md`, `types-paging-query.md`,
`types-paging-bindgen.md`, `types-paging-swift.md`, `types-paging-kotlin.md`, `types-paging-ts.md`). The contract scenarios are **S32** (paged
queries and lazy lists) and **S33** (polling) on the three platforms (S23 to S25 were the real-time ports; S29 and S30 went to ADR-046, S31 to
ADR-042); the playground has the `Library`, the `feed` query and the `ticker` query (`paging.rs`) and a screen for each on iOS, Android and the web;
the guides are `site/docs/paging.html` and `polling.html`. Deviations, each dated 2026-10-02:

1. **ADR-031's fold rule is amended** (found by the TypeScript implementer, confirmed by the core's): "op 2 supersedes earlier entries for its key"
   lost the page-server handle when an op 0 and an op 2 of one signal landed in one drain (observe, then a change before the frame; a restore's new
   handle). In every mirror, per signal, a **full value supersedes everything before it and a lazy invalidation supersedes only earlier
   invalidations of its signal, never the full value**; delivery order is kept (`[Full, Inv, Inv]` delivers `[Full, Inv(last)]`, `[Inv, Full]`
   delivers `[Full]`). SPEC 11.1, the three mirrors and their model tests carry it (ADR-031 has the amendment).
2. **Wire.** As decided (a 12-byte op 2, the 20-byte op 0 value, the page reply header), with one addition in the core: `LazySource::encode_page`
   returns the `LazyPage` header, because the version could not otherwise be read atomically with the items; `MAX_PAGE_ITEMS` = 4,096 caps a
   page call's limit. A `Lazy::over(&derived)` view snapshots as an empty list and its store needs a `restore` hook to rebuild it (the macro cannot
   tell a view from an owned list by its type). A store with a `Lazy` field registers one transient page server per signal through two hooks only
   such a store sets, so a core without one links none of the page-server code (the hello web core measured below).
3. **An infinite query's schema `returns` is `Result<Vec<T>, E>`**, not `Vec<T>` (decision 2.2): without `E` bindgen would type the handle's `error`
   as `String` while the core sends `Option<E>`; `undra-meta`'s E0073 check accepts both. `update_items` reaches platforms as the minimal keyed patch,
   not as recorded ops (a closure over `&mut Vec` cannot be recorded, ADR-027); a next page does not move `updated_at`; a persisted entry with more
   pages than `persist_pages` is not used; a poll interval is clamped to 1 s .. 7 days (`MIN_POLL_INTERVAL_MS`, `MAX_POLL_INTERVAL_MS`). Polling is
   linked into every query (about 1.35 KB on a core with queries: any observer may poll any query); a core with no query links none of it.
4. **SwiftUI's `List` does not work over a lazy list** (the sketch of decision 3.5 used it): on iOS 26.5 `List` builds every row of its `ForEach`
   up front (all 10,000 measured in one pass), so a 24-page cache thrashes and the visible rows never fill (4,297 page installs in two minutes);
   a `LazyVStack` in a `ScrollView` builds the rows near the screen (3 page calls). The sketch above, the runtime's docs and the guide say to use
   a lazy container; `UndraLazyListObject` (the `ObservableObject` twin) is observed directly (`@ObservedObject`), not through its store.
5. **Platform details.** Swift: the `RandomAccessCollection` conformance is `@preconcurrency` (a `@MainActor` class cannot satisfy its nonisolated
   requirements in Swift 6; isolated conformances need a newer runtime than the iOS 15 floor), one page call per page, `pageSize` clamped to
   1...4,096 (review, 2026-10-02: it was 1,048,576, Kotlin's 65,536 and TypeScript's 65,535, past the core's cut of a page call at 4,096 rows, so a larger page never
   arrived whole and its rows never loaded; all three now stop at 4,096), `UndraStats.hostPageCalls`. Kotlin: the list makes its own scope on `UndraDispatchers.main` for asynchronous calls, `addChangeListener`
   bridges `StateFlow` to Compose, a public `version`. TypeScript: a refused page call (status 5) means the page server is gone and the list stops
   asking quietly (closing a store with a page queued must not report), `LazyList.version`, `useLoadMore` takes an optional `error` signal on the
   query and does not fetch while it is set. A read of a loaded page asks once for the page beyond it when it is not loaded, so scrolling stays one
   page ahead (S32 step 2 says so).
6. **`Lazy<T>` composes with a keyed list** as decided; the Kotlin `undra-compose` module is built by Gradle only where an Android SDK is found
   (like `:android-adapters`); a `PagingSource` adapter stays an open item.
7. **Lifecycle.** Polling pauses on the existing `Lifecycle` and `Connectivity` events through one function (`Shared::on_lifecycle`), which also
   flushes what waits out its debounce when the app goes to the background (ADR-046).
8. **Bench rows and budgets** (`bench/RESULTS.md`): `lazy/page_50_of_100k` 290 ns (core half; the ADR's target was 20 us), `lazy/page_50_of_10k`
   288 ns, `lazy/view_page_50_of_100k` 3.2 us, `lazy/invalidate` 178 ns (12 bytes asserted) and `lazy/invalidate_10k` 171 ns, two scaling ratios
   (100,000 rows against 10,000: 1.0), `query/infinite_append_page_50` 7.3 us against `query/keyed_push_50` 5.4 us (ratio 1.34, gate 2.0); a commit
   of an observed `Lazy` allocates one buffer more than the same write to a plain observed counter at any length (`lazy_alloc`).

