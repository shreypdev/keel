# types-paging / tp-lazy: the Rust core of `Lazy<T>` (ADR-043 decision 3)

Sub-piece `tp-lazy`, worktree `wt/tp-lazy` off the foundation commit `6013803`. The contract it builds against is
`.10x/decisions/sde/types-paging.md` ("The Rust surface", the wire, "Lazy lists on the platforms"). This record says what landed, the
design choices the contract left open, the numbers, and what the integrator must do at the seams.

## What landed

### `undra-signals`

* `Lazy<T>` (`crates/undra-signals/src/lazy.rs`, `undra::Lazy`, in the facade's prelude too). Owned list: `new`, `from_vec`, the recorded API of
  `Signal<Vec<T>>` (`push`, `insert`, `remove`, `update_at`, `move_item`, `clear`, `replace`), `len`, `is_empty`, `version`, `with_range`, `get`,
  `to_vec`, `can_write`, `ptr_eq`, `is_attached`, `is_view`. It is a `Signal<LazyState<T>>` (`items` + `version` in **one** value), so
  the write rules of ADR-035 (E0065 off the core), transactions, `Updating` re-entrancy detection and copy-on-write come from `Signal`, and a
  reader sees the items and the version of one instant. Every change moves `version` (before a user closure runs, so a panicking
  `update_at` still announces); a no-op (`move_item(i, i)`, `clear` of an empty list) is not a write. `replace`/`clear` drop the old items after
  the lock is released.
* `Lazy::over(&DerivedList<T>)`: a read-only view (its writes panic with a message that says to write the source). It registers a `Weak`
  `Reactive` on the derived node (the node's dependents list, as a `Computed` does), which marks the view's store slot dirty when the source
  changes. Paging goes through the derived index: new `DerivedNode::page` and `DerivedNode::view_stamp`, `DerivedIndex::for_each_view_source_in`
  (sorted view: `srt.select(start)` then `next`; unsorted: `OsTree<Pass>::select_passing(k)` per row; each plus a `rank` in the positional tree),
  O(log n + limit log n). A page request first **drains** the node (replays the source's recorded ops, rebuilds after a raw write) under the
  source's read lock together with the snapshot, so a stale index is never read. The node gained `State.version`, bumped wherever the
  materialised cache is dropped (`State::changed`), which is "the view may have changed": a source write the filter ignores does not move it,
  so such a write is not announced.
* `LazySource` (`stamp() -> (len, version)`, `len`, `is_empty`, `version`, `encode_page(offset, limit, &mut Writer) -> LazyPage`): **the page
  header is returned by `encode_page`**, read in the same instant as the items, which the runtime puts in front of them. This is a
  deviation from the sketch in the brief (items only): the version could not otherwise be read atomically with the items. `page_window(len, offset,
  limit)` is the one window arithmetic (public, proptested).
* `StoreCell::attach_lazy(&Lazy<T>, id)`, `lazy_sources()`, `set_lazy_handle(id, handle)`, `lazy_handle(id)`, `set_lazy_hooks(LazyHooks)`,
  `lazy_hooks()`. **A `Lazy` slot is a `SlotKind::Derived` slot** (`LazyCell: DerivedSlot`): `DerivedSlot` gained `persisted()` (a derived
  list is rebuilt on restore, a lazy list is store state) and `lazy()`, and `Emitted` gained `Invalidated`. Everything else (isolation of a
  panicking pipeline like a computed, `failed_signals`, abandoned deliveries, observe rollback, `no_coalesce`) is the existing derived-slot
  machinery. Commit: op 2 (`LazyInvalidated { len, version }`, 12 bytes) when `(len, version)` differs from what the host was last told (`announced`),
  nothing otherwise; `observe` (resync) sends op 0 `LazyValue { handle, len, version }` and records it; an unobserved `no_coalesce` slot gets the
  op 0 value; `forget` (unobserve, abandon, rollback) makes the next commit announce again. The 12-byte entry is one allocation of the commit's scratch
  buffer (`reserve(12)`): a commit of an observed `Lazy` allocates exactly one buffer more than the same write to a plain observed counter, whatever
  the list's length (`crates/undra-ffi/tests/lazy_alloc.rs`).
* Snapshot value: an owned list encodes as the `Vec<T>` of its items; **a view encodes as an empty list** (derived data: the store's restore hook rebuilds
  it from the list it is derived from; documented on `Lazy`, in SPEC 4.3 and in the macro docs). Chosen over persisting the view's rows (O(n) duplicated
  bytes beside the source's own).

### `undra-runtime`

* **The registration design** (no C ABI or wasm ABI change): a store whose cell has lazy slots asks the runtime to serve them by calling
  `undra_runtime::serve_lazy_lists(&cell)`, which sets two `fn` pointers (`LazyHooks { register, unregister }`, over `&dyn Any` so
  `undra-signals` knows nothing of the table) on the cell. The object table calls `register` at the end of `place` and `insert_at` (so every way a
  store enters the table: a constructor, a returned store, a restore) and `unregister` at the end of `release`. `register` places one **page
  server entry per lazy signal** (`PageServer { source: Arc<dyn LazySource> }`, type id/name `LazyList`): transient (never in a snapshot,
  `stores()` does not list it), `host_refs = 0` and `table_owned`, so the host's `release` of it is a no-op (it cannot take the list from its store),
  and tells the cell the handle. `unregister` removes the entries and zeroes the cell's handles; a `clear` (teardown, restore) removes everything. The
  page server holds a clone of the `Lazy` the store holds, so it keeps nothing alive beyond the store.
  `#[undra::store]` emits the `serve_lazy_lists` call **only for a store with a `Lazy` field** ("link by use", ADR-052): a core without one links none
  of the registration (the table's hooks are `Option<fn>` checks). A hand-written store calls it itself.
* `LazyPage` dispatch: a built-in `DispatchFn` (`lazy_page_dispatch`) run by `run_dispatcher` (the existing panic guard and classification), with
  `offset`/`limit` passed in the `DispatchCall`'s `args`. Reply `version u64, total u32, count u32` + items. `limit` is cut to
  `MAX_PAGE_ITEMS = 4096` (public constant); `offset` past the end gives `count = 0`; a stale, foreign, null or wrong-type handle is a status-5
  bad request ("... refers to a Counter, not a lazy list" for a live non-lazy object); a panic in the source (an item's `Encode`, a view's pipeline) is a
  status-2 reply. `Runtime::insert_lazy_source(Arc<dyn LazySource>)` (new) and `insert_lazy_list(&LazyList)` hand the host one reference.
* `LazyList` (pre-encoded items) stays, now a `LazySource`; `page(offset, limit)` returns the new layout (docs and tests updated). It bumps its version
  under its write lock, so `(version, total)` of a page are one instant (test `a_page_is_read_at_one_version_while_the_list_changes`).
  `impl UndraObject for LazyList` is kept **and kept above line 100 of `lazy.rs`**: the `m2_store_without_impl` trybuild golden (a `tp-macros` file)
  renders that impl's line, and a line number of three digits widens its gutter.
* Persisted forms: `decode_dyn`, `skip` (stream), the tree and the streamed structural converters treat `Lazy(T)` as the `Vec<T>` it is stored as (or-patterns, no
  type clone): `Lazy<T>` <-> `Vec<T>` <-> `Lazy<U>` convert, item changes follow the list rules, hostile bytes are typed errors. `min_len(Lazy)` is 4.
  The closure of `Lazy(T)` already reached `T` (`closure.rs`); a test now pins it and the fingerprint's reaction.

### `undra-macros` (`impl_/store.rs`, the `Lazy` arm of `impl_/types.rs`), `crates/undra`

* `#[undra::store]` accepts `Lazy<T>`: `SigKind::Lazy`, plain (not computed), value type `Vec<T>` for the type checks, schema type `TypeRefMeta::Lazy(T)`,
  `attach_lazy`, `#[undra(key = "id")]` (checked through the same key function, E0008, which the core never calls), `#[undra(default)]` (empty), restore as
  `Lazy::<T>::from_vec(<decoded Vec<T>>)` (also what a restore hook receives: **one `Lazy<T>` per lazy signal**, which a hook that rebuilds a view ignores),
  `serve_lazy_lists` as above. A bare `Lazy` is E0001 ("needs the type of its rows"); `Lazy` inside `Signal`/`Computed`/`DerivedList` is a tailored E0001
  ("cannot hold a `Lazy`: it is a store field, not a value", with the `Lazy::over(&derived)` fix); anywhere else the `types.rs` arm says "can only be the type of
  a store field, not of {a record or enum field, a parameter, a return type, a signal value}". The E0008 text for a bad key now lists `Lazy<T>`.
* `crates/undra/src/lib.rs`: `pub use undra_signals::Lazy;` and `Lazy` in the prelude. One doc line of `crates/undra-macros/src/lib.rs` (the "Lazy is rejected in v1" bullet).
* UI tests: `e0001_lazy_signal.{rs,stderr}` rewritten (record field, parameter, `Computed<Lazy<Row>>`), `e0008_derived_list_key.stderr` one line.

### Bench, docs

* `bench/common/fixtures.rs` (`Shelf`), `bench/common/workloads.rs` (group `lazy`), `bench/benches/lazy.rs` + `Cargo.toml`, `bench/budgets.toml`, `bench/RESULTS.md` (finding 7 and a
  table), `bench/results/2026-10-01-lazy-lists-layer-a.json`. Rows (gate harness, best of three p50, load 20-35): `lazy/page_50_of_100k` 290 ns (ADR target: core half
  <= 20 us), `lazy/page_50_of_10k` 288 ns, `lazy/view_page_50_of_100k` 3.20 us, `lazy/invalidate` 178 ns (target <= 1 us, 12 bytes asserted before timing), `lazy/invalidate_10k` 171 ns; budgets
  1.5 us, 1.5 us, 16 us, 900 ns, 860 ns; ratios `lazy_page_scaling`, `lazy_invalidate_scaling` (max 2).
* SPEC: 2.1 (the `Lazy` variant comment), 2.2 (the `SignalDef` comment), 3.1 (the `Lazy<T>` value row), 3.3 (the page reply), 3.5 (op 0 value, op 2), 4.3 (the field kinds). Also
  `crates/undra-runtime/docs/runtime-internals.md` (one routing sentence), `crates/undra-signals/README.md` (two table cells).

## Size (ADR-052, R9)

`scripts/wasm-size.sh`, hello-world web core, measured on a **same-path-length** checkout of the foundation commit (the build embeds source paths; a different
path length moved the module by 1.5 KB): before `279,323` bytes / `119,610` gzipped; after `279,383` / `119,757` (**+60 bytes, +147 gzipped**; the gate is 120,000 and `5%` over
the record). The first version of this piece (dedicated slot arms in the store cell, the registration in the table, a `guarded` instantiation in `dispatch`) measured 120,491 gzipped, over the gate; moving the registration behind hooks that only a store with a `Lazy` field sets, reusing the derived slot kind and the existing dispatcher path took it to the above.
The up-front JS chunk (`web/hello-runtime-js`) measures 21,677 against its 21,500 gate in this tree; this piece touches no TypeScript (the foundation's Decimal codec is the cause).

## Checks run (all in `/Users/shrey/Desktop/src/.work/tp-lazy`)

* `cargo test -p undra-signals -p undra-runtime -p undra-macros -p undra-meta --no-fail-fast`: 1,333 passed, 6 failed, 6 ignored. The 6 failures fail identically on the foundation commit and are
  `tp-macros`/foundation items: `undra-macros` lib `snapshots::{record, record_with_crate_override, query, mutation}` (the foundation added `transparent`/`interval_ms` to the emitted
  metadata), `catalogue::every_code_is_emitted_by_code_that_is_not_a_test` (E0073 is not in SPEC section 12), `compile_fail::diagnostics_render_as_documented` (4 of 59: `d1_not_an_undra_type`,
  `e0066_migrate_target_type`, `h1_builtin_shadowed`, `m5_method_returns_an_object` render more `Decode` candidates since the foundation). `e0001_lazy_signal` and `m2_store_without_impl` pass.
* `cargo clippy -p undra-signals -p undra-runtime -p undra-macros -p undra-meta -p undra -p undra-ffi -p undra-bench --all-targets -- -D warnings` clean; `cargo clippy -p undra-ffi --target wasm32-unknown-unknown -- -D warnings` clean;
  `RUSTDOCFLAGS=-D warnings cargo doc` for the five crates clean; `cargo fmt --all -- --check` clean.
* `crates/undra-ffi/tests/wasm/run.sh` (both legs, `npm ci` in `runtimes/ts/@undra/runtime`): raw 22 passed, TypeScript 32 passed.
* `cargo test -p undra-ffi -p undra-query -p undra-testkit -p undra-ports -p undra-bench --no-fail-fast`: no failures; `cargo test --release -p undra-ffi --test lazy_alloc`: 3 passed;
  `cargo test -p undra-bench --test budgets --release` with `UNDRA_BENCH_FILTER=lazy/`: ok.

## Open items for the integrator

1. **ADR-031's fold loses the page-server handle.** The merge rule says an op 2 supersedes everything queued earlier for its key. A drain that holds an op 0 (`LazyValue`: the handle) followed by an op 2
   for the same signal (observe, then a change before the frame) folds to the op 2 alone, and the mirror never learns the handle. The three platform mirrors must fold `Full(LazyValue)` then
   `LazyInvalidated` into the `Full` with the new `len`/`version` (or keep the handle across an op 2). The core cannot avoid it (the wire has no handle in op 2). This is a `tp-swift`/`tp-kotlin`/`tp-ts` point and a SPEC 11.1
   sentence ("supersedes" for op 2, there: only over earlier op 1/op 2 entries).
2. **SPEC text outside my sections that is now stale** (I edited only the six sections named): section 12's E0001 row ("includes `Lazy<T>` (lazy lists are not available in v1)") and section 16.3's
   field-kinds bullet ("`Lazy<T>` is rejected in v1 ... like `undra-bindgen` rejects it"). `undra-bindgen` still refuses a `Lazy` signal (`validate.rs`): `tp-bindgen`.
3. A store with a `Lazy::over` view needs `#[undra::store(restore = "Self::assemble")]` (the macro cannot tell a view from an owned list by its type): without a hook the restore builds an owned, empty list for it. Documented on
   `Lazy` and in SPEC 4.3; a diagnostic would need an attribute (`tp-macros`' `attrs.rs`).
4. The hello-world web core has 243 gzipped bytes of headroom to the 120,000 gate after this piece; the other sub-pieces add to it.
5. Commit trailers follow the preamble (`Co-Authored-By: Claude Fable 5.1`).
