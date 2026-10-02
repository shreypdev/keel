# types-paging: ADR-042 (newtypes, generic instantiations, leaf types) and ADR-043 (polling, infinite queries, `Lazy<T>`)

Piece `types-paging`, the last code pieces of the boundary plan. Worktree `wt/types-paging`. The decisions are the ADRs' (their
Decision sections are the specification, accepted in direction by Amendment D); this record holds the contract the sub-pieces
build against, what was settled during the implementation, and (at the end) deviations, numbers and counts.

## Sub-pieces and who owns which files

The work is split along seams that touch disjoint files, each in its own worktree branch off the foundation commit and merged
back here. Rule: **an agent edits only the paths it owns** (plus new files); a change anywhere else is made by the owner or
reported. Nobody edits `.10x/status.md`, `.10x/handoff.md`, `docs/SPEC.md`, the ADRs, `contract-tests/scenarios.md` or the shared
`.10x/decisions/sde/types-paging.md`: each sub-piece records itself in `.10x/decisions/sde/types-paging-<name>.md`.

| Branch | Owns |
|---|---|
| `wt/tp-macros` | `crates/undra-macros/**` except `impl_/query.rs`, `impl_/store.rs` and the `Lazy`/`infinite` parts of `impl_/types.rs`; `crates/undra/**` (the facade: feature forwarding, `undra::query::Page` re-export is `tp-query`'s); the `trybuild` UI tests, the diagnostics catalogue, `docs/ERRORS.md` + `site/docs/errors.html` rows for E0002/E0007/E0060/E0070 |
| `wt/tp-lazy` | `crates/undra-signals/**` (`Lazy<T>`, `attach_lazy`, `Lazy::over`), `crates/undra-runtime/src/lazy.rs` and the page dispatch in `runtime.rs`, snapshot/restore of a `Lazy` signal, `store.rs` and the `Lazy` arm of `types.rs` in the macros, `bench` rows `lazy/*` |
| `wt/tp-query` | `crates/undra-query/**`, `crates/undra-macros/src/impl_/query.rs`, `crates/undra/src/query.rs`, `QueryDef` trait additions, `bench` row `query/infinite_append_page_50`, `crates/undra-testkit` if it needs the new handles |
| `wt/tp-bindgen` | `crates/undra-bindgen/**` (emitters, model, validate, goldens, diagnostics goldens), `crates/undra-cli` where it names types, `schema_docs` |
| `wt/tp-swift` | `runtimes/swift/**`, `contract-tests/swift/**` |
| `wt/tp-kotlin` | `runtimes/kotlin/**` (a new `undra-compose` module), `contract-tests/kotlin/**` |
| `wt/tp-ts` | `runtimes/ts/**`, `runtimes/rn/**`, `contract-tests/ts/**` |

`main` of this branch already has (the **foundation**, done): `TypeRef::Decimal`, `RecordDef.transparent`, `QueryDef.interval_ms /
poll_in_background / infinite: Option<InfiniteDef { cursor, item_key }>` (+ `*Meta` mirrors; all written only when set, so no
existing hash moves), `Schema::is_valid_map_key`, the validation (E0007 for a bad transparent record, E0073 for a bad infinite
query), `ids::SET_POLL_INTERVAL_METHOD_ID` / `ids::FETCH_NEXT_PAGE_METHOD_ID`, the closure `transparent` flag and the structural
migration step `T <-> Newtype(T)`, `undra_wire::Decimal` (+ the opt-in leaf features `uuid`, `chrono`, `time`, `rust_decimal`,
`bytes` and the hidden `WireLeaf<K>` markers in `undra_wire::leaf`), the three lazy payloads in `undra_wire::payload` (`LazyValue`,
`LazyInvalidated`, `LazyPage`), the wire vectors (`decimal_*`, `lazy_*`), and the `Decimal` codec on all three runtimes
(`Foundation.Decimal: UndraCodec`, `Codecs.decimal` over `java.math.BigDecimal`, `Decimal` + `decimalCodec` in `@undra/runtime`).
The bindgen emitters map `Decimal` and nothing else new.

## The contract between the sub-pieces

### Wire (ADR-043 decision 3.2; in `undra_wire::payload`, vectors in `contract-tests/wire-vectors.json`)

* The value of a `Lazy<T>` signal (change-set op 0 `Full`, and a snapshot restore's re-send): `handle u64, len u32, version u64`
  (`LazyValue`). `handle` is the **page server**: a transient object of the runtime's table.
* Change-set op 2 `LazyInvalidated`: `len u32, version u64` (`LazyInvalidated`). Superseding rules of ADR-031 unchanged (an op 2
  supersedes earlier entries of its signal in a drain; the last one wins).
* The page call (target 3, `handle, offset, limit`) is answered with `version u64, total u32, count u32` (`LazyPage`) followed by
  `count` items, each encoded as the item type.

### Schema and handles (ADR-043 decisions 1, 2, 4)

* `QueryDef.interval_ms`, `poll_in_background`, `infinite` as in the foundation. For an `infinite` query `params` excludes the
  cursor, `returns` is `Vec<T>` (a record `T`), `infinite.item_key` names a field of `T`, `infinite.cursor` is the cursor type.
* The query handle (a synthesized `<Name>QueryHandle` object of kind store; its constructor's type id and method id are the
  query id, as today) has the signals **0 data, 1 status, 2 error, 3 fetching, 4 updated_at** for every query, where `data` is
  `Option<Output>` for an ordinary query and `Vec<T>` **keyed by `item_key`** (empty, never `None`) for an infinite one, plus for
  an infinite one **5 `has_next_page: bool`, 6 `fetching_next_page: bool`**.
* Handle methods, all sync commands: `refetch` (`ids`: `fnv1a32("query.refetch")`, existing), `invalidate`
  (`fnv1a32("query.invalidate")`, existing), `set_poll_interval(Option<Duration>)`
  (`ids::SET_POLL_INTERVAL_METHOD_ID`; the argument is the wire `Option<Duration>`, `None` clears the observer's override),
  and for an infinite query `fetch_next_page()` (`ids::FETCH_NEXT_PAGE_METHOD_ID`, no arguments).
* The data signal of an infinite handle is a keyed list: a next page arrives as a `KeyedPatch` of pushes (`Insert` ops at the end),
  a refetch as a full value or a minimal keyed patch of what changed.
* Persisted infinite entries store the first `persist_pages` pages (default 1) under ADR-037's rules.

### Lazy lists on the platforms (ADR-043 decision 3.5)

One runtime class per platform, **not generated code**. Names: Swift `UndraLazyList<Item>` (`@Observable`, iOS 17) and its twin
`UndraLazyListObject<Item>` (`ObservableObject`, every floor: ADR-045), Kotlin `UndraLazyList<T>`, TypeScript `LazyList<T>`.
Generated stores hold one per `Lazy<T>` signal, created in the store's initializer with the core and the item codec, and their
`apply` hands the change-set entry's reader to it:

```text
signal Lazy<Book> `books` (id 3):   op Full            -> books.applyFull(reader)          // a LazyValue
                                    op LazyInvalidated -> books.applyInvalidated(reader)   // a LazyInvalidated
                                    op KeyedPatch      -> ignored
```

(Swift `try books.applyFull(&reader)` / `applyInvalidated(&reader)`, Kotlin `books.applyFull(reader)` /
`applyInvalidated(reader)`, TypeScript `books.applyFull(reader)` / `applyInvalidated(reader)`; each also calls `reader.finish()`.)
Construction: Swift `UndraLazyList(core: core)` (the element type's `UndraCodec` is the generic constraint), Kotlin
`UndraLazyList(core, <item codec>)`, TypeScript `new LazyList(core, <item codec>)`. A store's property is a `let`/`val`/`readonly`.

Behaviour, the same on the three (the numbers are the defaults; `pageSize` and `maxCachedPages` are settable):

* **Count and rows.** `count` (Swift) / `size` as a `StateFlow<Int>` (Kotlin) / `length` as a `Signal<number>` (TypeScript) is the
  length of the last `LazyValue` / `LazyInvalidated`. `item(at index)` returns the cached row or nothing (`nil`, `null`,
  `undefined`) while its page loads, and **requests** the page that holds it plus one page of prefetch on each side, once. An
  index outside `0..<count` returns nothing and requests nothing. `prefetch(range)` requests the pages of a range.
* **Page requests** are page calls (`CallTarget.LazyListPage` in Kotlin, the existing target-3 constructor in Swift and
  TypeScript): synchronous (`callSync`) where the transport allows it (in process; `wasm-main`), else asynchronous; requests made
  within one main-thread turn are coalesced into one batch per drain (a frame, a microtask) and a page already in flight is not
  requested again. A reply's `version` lower than the list's current one is dropped and the page asked for again; a higher one
  raises the list's `version` and count (the op 2 that follows changes nothing).
* **Invalidation (op 2).** The new `len`/`version` are taken at once; cached rows stay visible (stale) while the **window** is
  re-paged: the pages touched since the previous invalidation (at most `maxCachedPages`, default 24; least recently touched pages
  are evicted first, and a page is never evicted while it is in the window). Cost is O(window), never O(list).
* **Observation.** The platform re-renders when `count` changes and when a page arrives or is replaced: Swift `@Observable` reads
  of `count`/`item(at:)` register a dependency (a private revision counter is bumped when pages change); Kotlin exposes
  `val revision: StateFlow<Long>` next to `size`; TypeScript a `revision: Signal<number>` next to `length`. Reading `item(at:)`
  during view rendering is the normal use (it requests, it does not block).
* **Lifetime.** The list holds nothing of the core but the handle number; it is released with its store. After a restore the
  next `LazyValue` carries a new handle and the cache is dropped.
* Swift also conforms to `RandomAccessCollection` (`Index == Int`, `Element == Item?`), `subscript(position:) -> Item?`, `pageSize`,
  `prefetch(_: Range<Int>)`; Kotlin `operator fun get(index: Int): T?`, `fun prefetch(range: IntRange)`, `pageSize`; TypeScript
  `get(index)`, `prefetch(start, end)`, `pageSize`; React `useLazyList(list)` returning `{ length, getItem }` from
  `@undra/runtime/react`; the Compose helper (a new optional Gradle module `undra-compose`) `fun <T> LazyListScope.items(list:
  UndraLazyList<T>, itemContent: @Composable LazyItemScope.(index: Int, item: T?) -> Unit)` and
  `@Composable fun LazyListState.LoadMoreWhenNearEnd(query: InfiniteQuery, threshold: Int = 5)`.

### Infinite and polled query handles on the platforms (decision 4)

Generated per the ADR's shapes: `FeedQuery` has `data`, `hasNextPage`, `fetchingNextPage` (+ the five of every query), methods
`fetchNextPage()`, `refetch()`, `invalidate()`, `setPollInterval(_:)` and, Swift only, `loadMore(ifNeededFor:threshold:)`; every
query handle gets `setPollInterval`. The Kotlin runtime provides `interface InfiniteQuery { val hasNextPage: StateFlow<Boolean>;
val fetchingNextPage: StateFlow<Boolean>; fun fetchNextPage() }` (generated infinite handles implement it); the TypeScript runtime
`useLoadMore(query, { rootMargin })` in `@undra/runtime/react`, structurally typed over `{ hasNextPage: Signal<boolean>;
fetchingNextPage: Signal<boolean>; fetchNextPage(): Promise<void> }`.

### The Rust surface (what `tp-lazy` and `tp-query` provide to each other, to bindgen and to the playground)

* `#[undra::store] struct Library { #[undra(key = "id")] books: Lazy<Book> }` (`undra::Lazy`, re-exported from `undra-signals`);
  `Lazy<T>` has the recorded API of `Signal<Vec<T>>` (`push`, `insert`, `remove`, `update_at`, `move_item`, `clear`, `replace`,
  `len`, `with_range`), `Lazy::over(&DerivedList<T>)` is a read-only lazy view of a derived list pageable through the derived
  index. The runtime serves `LazyPage` calls for any `Arc<dyn LazySource>` in its object table; `undra_runtime::LazyList` stays as
  a convenience source over pre-encoded items.
* `#[undra::query(key = "feed/{filter}", infinite, item_key = "id", stale = "1m", interval = "30s", poll_in_background,
  refetch_pages = N, persist, persist_pages = N)]` with exactly one `#[undra(cursor)] cursor: Option<C>` parameter and a
  `Result<undra::query::Page<T, C>, E>` return (`Page { items: Vec<T>, next: Option<C> }`, `C` defaults to `String`);
  `ctx.query().infinite::<FeedQuery>(params)`; `CacheView::update_items::<Q>(params, |items| ..)`.

## Scenario numbers

`prod-ops` (landing on `main` before this piece) took S29 (panic reports) and S30 (background runs). This piece's contract
scenarios are **S31 newtypes, generics and leaf types**, **S32 paged queries and lazy lists**, **S33 polling**. `prod-ops` also adds a
standard `Diagnostics` port, three standard types and `run_background`, so every schema hash moves once more when this branch
crosses it: regenerate, do not hand-merge. If polling needs a lifecycle signal beyond the existing `Lifecycle` port events, the
`BackgroundReport`/`Lifecycle` path of `prod-ops` is the one to use (the integrator adapts at the final merge of `main`).

## Merge protocol

Each sub-piece commits small (`type(scope): summary`, trailer `Co-Authored-By: Claude Fable 5.1 <noreply@anthropic.com>`) in its own
worktree `/Users/shrey/Desktop/src/.work/tp-<name>`, never pushes, runs only its own checks, and reports its HEAD. The integrator
(`wt/types-paging`) merges the branches in the order `tp-macros`, `tp-lazy`, `tp-query`, `tp-bindgen`, `tp-swift`, `tp-kotlin`,
`tp-ts`, fixes the seams and runs the matrix once.

## Results (integrator, 2026-10-02)

What landed: everything in both ADRs' implementation briefs, whole (R4). The per-ADR lists and the dated deviations are in the ADRs'
"Implementation (2026-10-02) and deviations" sections (ADR-042: nine items, ADR-043: eight items), both flipped to Accepted; ADR-031 carries the
fold-rule amendment (a full value supersedes everything before it; a lazy invalidation supersedes only earlier invalidations of its signal).

Contract scenarios: **S31** newtypes, generic instantiations and leaf types; **S32** paged queries and lazy lists; **S33** polling. The grid is
33 scenarios and 95 cells (31 Swift, 31 Kotlin, 33 TypeScript), all 95 pass; the React Native column runs S31 to S33 as well (S17 and S29 stay
app-tested there, as before).

Sizes (`scripts/wasm-size.sh`, both gates, on this worktree's path, which embeds about 240 bytes more than the canonical one): the hello-world
web core is **116,480 gzipped** (gate 120,000; main's record 119,654), so it did not grow: the merged tree first measured 120,706 (1,052 over main's record: schema fields,
persisted-state arms, the page server), which the lazy page server linked by use (`serve_lazy_lists` hooks only a store with a `Lazy` sets) and one shell sort
(`undra_signals::sort_ids`, replacing three `slice::sort` instantiations) paid back with room to spare. The JavaScript up-front chunk is **22,061**
(gate 22,100, record 22,005: 39 bytes of headroom). The
recorded numbers in `bench/budgets.toml`'s sizes were not re-recorded here (a longer path would pin the record 240 bytes high); re-record on the
canonical path after the merge to `main`.

Bench rows (`bench/budgets.toml`, `bench/RESULTS.md` section 7): `wire/decimal/roundtrip` 45 ns (budget 250), `lazy/page_50_of_100k` 290 ns,
`lazy/page_50_of_10k` 288 ns, `lazy/view_page_50_of_100k` 3.2 us, `lazy/invalidate` 178 ns (12 bytes asserted), `lazy/invalidate_10k` 171 ns,
`query/infinite_append_page_50` 7.3 us against `query/keyed_push_50` 5.4 us (ratio 1.34, gate 2.0); `lazy_alloc` pins one buffer more than a plain
observed write.

Counts on the merged tree: `cargo test --workspace` 3,515 passed, 0 failed, 21 ignored; Swift 861 (observation mode and the ObservableObject
mode), Kotlin 876 (+32 testkit; both compilers), TypeScript 1,844, React Native 109 + 30 C++ checks, the wasm harness 22 + 36, interop (Swift 6 + 22,
JNI 16, C), the playground web 135; `undra bindgen --check --docs` is current on playground, cookbook, fieldbook and two-cores a and b (the iOS 15
sample is generated without `--docs`, as its floor script checks it); the Android `assembleDebug` of the playground builds; site `build-all` is up
to date and `check-links --words` passes (landing prose 342 of 350).

Process notes:
* **An environment incident**: the shared `scratchpad/env.sh` was overwritten by the `objects-followups` agent (it `cd`s into that worktree), so two
  of this piece's commit/merge commands ran in that worktree (commits `dbabbad`, `5ae4aa8`, `458fafa`). Nothing was pushed. That worktree's owner
  has since repaired its branch (`backup/of-foreign-458fafa` keeps the accidental line; none of the three commits is in its history). This piece
  moved to a private `env-tp.sh` with a branch guard (`INCIDENT-types-paging-env.md` in the scratchpad has the reflog).
* The `Decimal` bench row was first recorded with a placeholder (16 ns) and re-measured at 45 ns; ADR-042's deviation 9 says 45.
* Merging `main` (prod-ops, objects-callbacks) moved every schema hash once more, as announced: regenerated, not hand-merged; one golden
  (`h1_leaf_shadowed`: `BackgroundReport` joins the `Encode` candidates) and the session/remote-todos testkit fixtures were re-recorded.
