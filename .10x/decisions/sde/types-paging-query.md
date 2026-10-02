# SDE: types-paging, sub-piece `tp-query` — interval polling and infinite queries (ADR-043 decisions 1, 2 and the Rust half of 4) — 2026-10-01

Branch `wt/tp-query`, off the foundation commit `6013803`. Implements ADR-043 decision 1 (polling) and decision 2
(infinite queries) in `undra-query`, the query macro and the facade, with E0073, the SPEC text for those parts and the
bench row `query/infinite_append_page_50`. The contract with the other sub-pieces is `types-paging.md`; this file is
what was settled while building and what the integrator needs at the seams.

## What was built

* **`undra-query/src/poll.rs`** (new). Interval polling. `Polling` per entry (the per-observer overrides, the armed
  timer, the moment the interval counts from); `Shared::reschedule_poll` brings an entry's timer in line with what
  is wanted (observers, online, `Active` or `poll_in_background`, no fetch in flight); `poll_tick`; `set_poll_interval`;
  `reschedule_all`. The timer is a task sleeping on the Timer port holding a `WeakCtx` (ADR-034), the moments come from
  the Clock port (R12). `MIN_POLL_INTERVAL_MS` (1 s) and `MAX_POLL_INTERVAL_MS` (a week) are public.
* **`undra-query/src/paged.rs`** (new). Infinite queries: `PageRec`/`PagedState` (the entry's pages), `PagedVTable`
  (the type-erased half, built by `paged_vtable::<Q>()`, reached from the generic engine only through fn pointers so a
  core with ordinary queries does not link it), the refresh and next-page tasks, `Shared::fetch_next_page`,
  `complete_pages`, `InfiniteHandle<Q>` (seven signals, `data` keyed through `attach_keyed(.., Q::item_key)`), the
  persisted form.
* **`defs.rs`**: `QueryDef` gains `INTERVAL_MS`, `POLL_IN_BACKGROUND` and the hidden `PAGED`; `Page<T, C = String>`,
  `InfiniteQueryDef` (`Item`, `Cursor`, `REFETCH_PAGES`, `PERSIST_PAGES`, `fetch_page`, `item_key`), `PageResult`,
  `fetch_first_page`. **`shared.rs`**: entry fields `paged` and `polling`, `Inflight.next_page`, `View.list`,
  `Entry::has_data` (the old `data.is_none()` tests), the polling hooks in observe / release / start_fetch / complete /
  abort / write, `settle_success`. **`handle.rs`**: `Link` (what a handle holds of the cache), `set_poll_interval` on
  every handle, `HandleOps::{set_poll_interval, fetch_next_page}`. **`dispatch.rs`**: the two new method ids.
  **`storage.rs`**: the persisted closure and migration of an infinite entry. **`mutation.rs`**:
  `CacheView::update_items`. **`client.rs`**: `QueryClient::infinite`. **`inspect.rs`**: an infinite entry's list in the
  devtools document. **`lib.rs`** and the README: docs.
* **`undra-macros/src/impl_/query.rs`**: `interval`, `poll_in_background`, `infinite`, `item_key`, `refetch_pages`,
  `persist_pages`, `#[undra(cursor)]`, `Page<T, C>` recognition, E0073, E0040's interval floor; the generated
  `InfiniteQueryDef` impl with the key function (the same encoded-key-field hash as `#[undra::store]`, with a const
  check of the field name that reports E0073 on the string). New diag constant and table row for E0073 in `diag.rs`.
  Snapshots `query_polling`, `query_infinite` (own helper in `query.rs`, so no shared test file is touched); the existing
  `query` and `mutation` snapshots regenerated; UI tests `e0040_interval_below_a_second`, `e0073_paging_options`,
  `e0073_cursor_parameter`, `e0073_page_return`, `e0073_item_key_names_no_field`; behaviour tests in `tests/queries.rs`.
* **Facade** `crates/undra/src/query.rs`: re-exports (`Page`, `InfiniteQueryDef`, `InfiniteHandle`, the method ids, the
  interval bounds; `PagedVTable`, `paged_vtable`, `fetch_first_page` hidden, for generated code).
* **SPEC**: 2.2 (`QueryDef` fields), 4.5, 9 (polling and infinite bullets; the triggers list is now true), 12 (E0040 text,
  the E0073 row). Minimal hunks.
* **Tests**: `undra-query/tests/polling.rs` (26), `tests/infinite.rs` (31 incl. a proptest against a naive model of pages
  that also applies every change-set to a host mirror), `tests/lifecycle.rs` (a poll timer does not pin the runtime), unit
  tests in `poll.rs`, `undra-macros` unit tests (21 in `impl_/query.rs`), the UI tests above.
* **Bench**: `bench/common/query_rows.rs` (rows `query/infinite_append_page_50` and its baseline `query/keyed_push_50`),
  wired into `workloads::all()`/`group()` and `benches/query.rs`; budgets and the ratio in `bench/budgets.toml`; rows in
  `bench/RESULTS.md`.

## Decisions and deviations

1. **The schema's `returns` is `Result<Vec<T>, E>`, not `Vec<T>`** (ADR-043 2.2 and the foundation's validation say
   `Vec<T>`). With `Vec<T>` alone the error type is gone from the schema, and bindgen (`query_types`) types the handle's
   `error` signal as `Option<String>` while the core sends `Option<E>`: the first error would not decode. bindgen already
   reads both. **One seam outside my files**: `undra-meta/src/validate.rs` (`infinite_problem`) now looks through a
   `Result` for the `Vec<T>` (plus a test); a plain `Vec<T>` stays valid.
2. **`update_items` reaches platforms as the minimal keyed patch, not as recorded ops.** The signature is
   `|items: &mut Vec<T>|`; a closure over a `&mut Vec` cannot be recorded (ADR-027), so the write is a raw one and the
   commit diffs by key (an update and a removal are two ops). The pages keep their sizes, cursors and number (the last page
   takes the difference); the next fetch of the entry replaces them with the server's answer. `QueryClient::set::<Q>` and
   `get::<Q>` of an infinite query are the same flattened list (`Output = Vec<T>`), and `set` keeps the cursor of the last
   page.
3. **A successful next page does not move `updated_at`**: the head of the list was not confirmed, so staleness (and the
   persisted entry's age) follow the last refresh. TanStack moves it; this is deliberate.
4. **Refetch is all or nothing**: the pages replace the old ones only when every one arrived (a failure mid-chain keeps the
   old pages and shows the error); an unchanged answer sends nothing (the pages are compared by their encodings).
5. **`fetch_next_page` while any fetch is in flight is a no-op** (no cancel-and-restart as TanStack's default); an
   `invalidate` still restarts the fetch as a refresh and drops a next page in flight.
6. **Polling intervals at run time are clamped** to 1 s ..= 7 days (zero, sub-second, `Duration::MAX`); a hand-written
   `INTERVAL_MS` too. The attribute refuses below 1 s at compile time (E0040, "use a stream for real-time data").
7. **Changing an interval keeps the moment it counts from.** The next poll is `max(now, from + interval)` with `from` the end
   of the last fetch (or when polling started or resumed), so a storm of `set_poll_interval` calls neither starves nor
   advances a poll, and `set_poll_interval(Some(2s))` 20 s after the last fetch polls at once. A fetch that starts or ends
   resets it. Overrides are per observer (one slot per sink id; a released observer's goes with it; a call for a released
   handle is a no-op; through the platform it is the usual unknown-handle error).
8. **Lifecycle input.** Polling is paused and resumed by the lifecycle and connectivity events the client already
   subscribed to (`undra_ports::on_lifecycle_changed` / `on_connectivity_changed`). **The lifecycle input enters at one
   place: `Shared::start` (shared.rs) hands each state to `Shared::on_lifecycle(ctx, state)` (poll.rs)**, which records the
   state, runs what `Active` and `Background` did before (`on_active`, `retry_unreadable_queue`) and reschedules every poll.
   ADR-046's `BackgroundReport`/`run_background` path is not in this tree; to re-point polling at it, call `on_lifecycle`
   from there (or replace the one `app_state` store inside it). `Inactive` pauses like `Background`.
9. **An entry stored with more pages than this build keeps (`persist_pages`) is not used**: only the cursor after the last
   stored page survives a restore, so it could not be persisted again correctly. Hydration keeps the stored bytes; a fetch
   overwrites them.
10. **`observe::<Q>` of an infinite query** (a plain `QueryHandle`) still works: its `data` is the flattened list (made when
    the view is applied). Platforms always get the infinite handle (`QueryVTable.open` is the paged one).
11. Rows must have unique `item_key`s; the recorded append does not look at keys (ADR-027). Documented in `paged.rs`.

## Seams and what the integrator must do

* `undra-meta/src/validate.rs` (decision 1): the one seam outside my paths.
* `crates/undra-macros/tests/snapshots/{record,record_crate_override}.rs` and four `tests/ui/*.stderr` (`d1_not_an_undra_type`,
  `e0066_migrate_target_type`, `h1_builtin_shadowed`, `m5_method_returns_an_object`) are **stale in the foundation commit**
  (`transparent: false`, the `Decimal` impl list) and are `tp-macros`'s; I regenerated only the `query` and `mutation`
  snapshots and left those alone.
* `site/docs/errors.html` is generated (`node site/scripts/build-errors.mjs`): regenerate after merging for E0073 and
  E0040's text (I did not, to avoid a conflict with `tp-macros`'s rows).
* SPEC 12: the E0073 row is after E0071's, the E0040 row's text is extended; `tp-macros` edits E0002/E0006/E0007/E0060/E0070.
* `bench/common/workloads.rs`/`mod.rs`: one line each (`pub mod query_rows;`, `all.extend(super::query_rows::rows_group())`,
  the `"query"` group arm); `tp-lazy` adds `lazy/*` rows nearby, expect a trivial conflict.
* The Kotlin/Swift/TypeScript runtimes call `set_poll_interval` with the wire `Option<Duration>` (`[0]` for `None`) and
  `fetch_next_page` with no arguments; both are `Sync` replies with an empty body; an unknown handle is `BadRequest`,
  `fetch_next_page` on an ordinary handle is `Unknown`.

## Numbers and counts

* **Tests** (`cargo test -p <crate> --no-fail-fast`, debug): `undra-query` 278 passed (0 failed, 3 ignored doc examples), of
  which `tests/polling.rs` 26, `tests/infinite.rs` 31 (the proptest runs 96 cases by default; 4,000 cases were run once, clean),
  `tests/lifecycle.rs` 4; `undra` 37; `undra-testkit` 33; `undra-meta` 163; `undra-macros` 339 passed, 3 failed: the two `record*`
  snapshots and `compile_fail` (four `.stderr` goldens), all stale in the foundation commit and `tp-macros`'s (my five new UI
  cases and the catalogue audit pass). `cargo clippy -p undra-query -p undra-macros -p undra -p undra-meta -p undra-bench
  --all-targets -- -D warnings` and `RUSTDOCFLAGS="-D warnings" cargo doc --no-deps` for the four library crates: clean.
* **Bench** (release, budgets test, Apple M5 Pro while other pieces built): `query/infinite_append_page_50` 7.4 µs best p50
  (criterion median 7.29 µs, 7.07 to 7.59), `query/keyed_push_50` 5.5 µs (5.41 µs); ratio 1.34 to 1.46, gate 2.0; budgets 38 µs
  and 28 µs. The existing `query/*` rows against the foundation commit, alternating runs of both binaries (24 rounds, the
  minimum of each, the machine at load 15 to 25): observe 0.77x (noise), construct and release 1.02x, `refetch()` call 1.02x,
  refetch published to 100 observers 1.01x: no regression.
* **Size (ADR-052)**, `scripts/wasm-size.sh` hello-world web core (no query): **119,496 B gzipped** in this tree against
  **119,694 B** at the foundation commit measured the same way; both are over the recorded 116,706 B (+2.4 to 2.6%, the ceiling
  is 120,000), so the growth over the record is the foundation's (Decimal, the lazy payloads), not this piece's: this piece
  links nothing of `undra-query` into a core without a query. A core with one ordinary `#[undra::query]` (`stale`, `retry`):
  192,431 B at the foundation, **195,126 B** with this piece (+2,695 B, +1.4%): about 1.35 KB is the polling code every query
  carries (any observer may poll any query, so `set_poll_interval` is on every handle), the rest the paging hooks of the
  generic engine (entry fields, `View`, rollback, seed) and the new dispatch arms; the paged engine itself, the persisted
  closure and its migration are reached through `PagedVTable` only. Adding an `interval` query: +486 B; adding one `infinite`
  query (with `persist`): +7,187 B. The JS gate was not measured (no `npm ci` in the worktree).
