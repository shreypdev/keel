# SDE: derived keyed lists (E2, ADR-039) - 2026-10-01

Implemented ADR-039 whole, from the brief `.10x/specs/2026-10-01-derived-keyed-lists-impl.md` and the
integrator's decisions (parameters yes, `DerivedList<T>`/`derive()`, caps 4,096/256, `count()` yes, ties in
source order with `Move` + `Update`). Branch `wt/derived-lists`.

## What was built

* **`undra-signals`, `src/derived/`.** `index.rs`: one arena AVL order-statistic tree with parent pointers
  and a summable aggregate (`OsTree<A>`), instantiated as the positional tree (`Pass`: rows and passing rows
  per subtree, 24 bytes a node, pinned by a test) and the sorted tree (`(key, source position)`, 20 bytes a
  node), and `DerivedIndex<K>` on top (insert / remove / update / move / clear / rebuild / transition, with
  an optional "no ranks" mode). `tap.rs`: `TapList` (by `Weak`) and `SourceTap` (armed / stale / 4,096 ops,
  a spare buffer). `pipeline.rs`: the stages fused into one `for<'a> Fn(&Params, Cow<'a, T>) ->
  Option<(K, Cow<'a, U>)>` (a no-`map` pipeline moves the recorded item into the emitted op, never a second
  clone) and type-erased parameters compared by encoded bytes. `node.rs`: the drain (one routine for reads,
  commits and observe), replay, parameter walk, rebuild, materialise, the full-value encoder, the re-entrancy
  stack and the `in_flight` poison flag. `slot.rs`: the derived slot. `mod.rs`: `Derive` (typestate
  `Unsorted` / `Sorted<K>`), `DerivedList`, `DerivedStats`, `count()`, `Dep for &DerivedList<T>`, the
  purity check (`DERIVING`, debug only).
* **Integration.** `SignalInner.taps`; every recorded op feeds the taps after the change (`signal/list.rs`),
  every raw write marks them stale (`invalidate_log`); `Computed::from_compute`; `OwnedDep::snapshot`;
  `StoreCell::attach_derived` and `SlotKind::Derived` in commit, observe, stop / abandon / rollback,
  snapshot; purity checks in `Signal::snapshot` / `read_locked` / every write and `ComputedInner::current`.
  `DerivedList` in `undra::prelude`.
* **Macros.** `DerivedList<T>` store fields (schema `Vec<T>`, `computed: true`, the key; `attach_derived`
  with the generated key fn; out of snapshots and of the restore hook; computed for E0013). E0008: a
  derived list without a key; a keyed `Computed` now told to become a `DerivedList`. E0001: a
  `DerivedList` without its row type (and `recover` gives it `()` so rustc adds nothing). Two trybuild
  goldens, unit and end-to-end store tests (`tests/derived_stores.rs`, schema validates).
* **Meta / bindgen.** No code change, no golden change (see deviations): a canonical-JSON pin, and a
  generators test that a computed keyed list's declaration equals a computed list's in all three
  languages and only its `apply` has the patch case.
* **Playground.** `Todos` on recorded ops (`push`, `update_at`, `remove`, descending removes in one txn),
  `visible: DerivedList<Todo>` (`filter_with(&filter, ..)`), `remaining = derive().filter(..).count()`,
  `fill(count)`. Bindings regenerated: `visible`'s apply gains the patch case, `fill` appears; **schema hash
  `0xc5f05c376fde398c`** after the last merge of `main` (whose hash was `0xefd907be3070520a`; regenerated, not
  hand-merged). `BigList` unchanged (the brief does not ask).
* **Contract S19** on Swift, Kotlin and TypeScript (19 x 3). Step 9 replays a recording of the 60,000-op
  seeded run (below) through each runtime's own change-set decoder, `decodePatch` and `applyPatch`,
  checking every view's FNV-1a 64 after every change-set; TypeScript also through a `Mirror` drained at
  seeded points (ADR-031 merging). `contract-tests/derived-vectors.sh` writes the recording when stale.
* **Bench.** `Views` and `ChurnViews` fixtures, nine `signals/derived_*` rows, `stress/derived_churn_10k/
  ops_x1000`, two ratios, `derived_churn_10k/sustained` (+ a fault self-test), `ApplyingHost` mirrors
  several lists, `tests/derived_before_after.rs` (ignored; the ADR's table), results JSON.
* **Docs.** SPEC 2.2, 3.8, 4.3, 5.5, 5.9, 10, 11.1, 12, 14, 16.1, 16.3; concepts page "Derived lists"; the
  error-codes page (regenerated); signals README and crate docs; HIGH_FREQUENCY; RESULTS.md finding 5;
  README / ONBOARDING counts.

## Measured (Apple M5 Pro, shared, load 6-10 unless said)

Core, one title change of a visible row, `undra-signals` directly, both slots observed, sink copies:

| Rows | Computed: per change | Computed: change-set | Derived: per change | Derived: change-set | Source alone |
|---|---|---|---|---|---|
| 1,000 | 21.0 µs | 35.4 KB | 292 ns | 158 B | 209 ns, 85 B |
| 10,000 | 176.5 µs | 352.6 KB | 333 ns | 158 B | 250 ns, 85 B |
| 100,000 | 2.24 ms | 3.53 MB | 375 ns | 158 B | 292 ns, 85 B |

Platform apply of `visible`'s entry, full value / one op: TypeScript 104.5 µs / 0.46, 1.07 ms / 1.62 µs,
11.27 ms / 47.3 µs; Kotlin (JVM 17) 15.2 / 0.21, 152 / 1.13, 1,499 / 7.2 µs; Swift -O 81 / 0.38, 832 /
0.38, 8,969 / 0.42 µs (4.9 / 46 / 513 µs while a view holds the array). Gate rows (best of three p50): 392,
379, 555, 378, 607 ns, 6.50 µs (1.05x keyed insert), 217.6 µs, 174.6 µs, 388 ns; every ADR target met.
Ratios: sort scaling 1.04-1.12 (max 4), derived vs keyed 1.36-1.65 (max 2.5). Sustained: 122,000 ops/s
(keyed churn 169,300 in the same run), p99 24.6 µs, 93.0 B/op, 896,219 view patches applied of 896,219.
Allocation: an observed view adds 0 allocations to a recorded write (`crates/undra-ffi/tests/derived_alloc.rs`,
release: 6 for the source alone, 7 with a view = 7 with any second claimed slot).

Model runs: `tests/derived.rs` 1,500 cases by default; **100,000 cases once in debug: passed in 377 s**.
`tests/derived_seeded.rs`: 60,001 ops, 24,009 transactions, 23,745 change-sets, ~3 s debug.

## Deviations, and why

1. **A panicking closure is isolated, not abandoned.** The brief predates the ADR-019 amendment: a derived
   slot is evaluated under the same guard as a computed (held back, `failed_signals`, the rest of the store
   delivered, rebuilt and sent whole when it next evaluates). Tests: filter on commit, map on observe, a
   Rust read that unwinds.
2. **The allocation test is in `undra-ffi`**, not `undra-signals`: a counting allocator is `unsafe`, which R2
   keeps in that crate. It asserts "the same as any second claimed slot", because a second dirty slot costs
   the commit one allocation of its own (`group_by_store`'s id list grows), measured by swapping the view
   for a plain counter.
3. **No bindgen golden changed.** The `stores` golden already describes a computed keyed list (`visible`),
   and `schema_hash.rs` pins every golden case's hash, so the test adds the comparison signal to its own
   copy of the schema instead of to the fixture.
4. **Optimisations beyond the brief** (after the first measurement missed `param_flip` 1.43 ms and
   `rebuild_after_replace` 1.37 ms): ranks only when an op is kept, the walk skips unchanged rows, full
   values encoded from the source rows without materialising (commits no longer fill the cache). The
   index test runs every op on a rank-free twin that must stay identical.
5. **Bench `Views.reset` swaps in a prebuilt list** (a raw write that allocates nothing), so
   `rebuild_after_replace` measures the rebuild and not 10,000 `format!`s (that alone was ~1 ms).
6. **Ratio maxima.** `derived_sort_scaling` is cache-bound, so it takes the bound of what it guards (4, the
   ADR's) as the fan-out ratio does; `derived_vs_keyed_update` takes 2.5 (largest host sample x the
   one-family runner spread), not 1.15x of a host sample (1.9): there are no runner samples yet.
7. **S11 raw sub-steps** of Kotlin and Swift decoded `visible` as a full value after `set_filter`; they now
   apply its patch (the ADR expected S11 unchanged; TypeScript's S11 reads values and was).
8. **60,000-op platform check through recorded vectors** (generated by run.sh, not committed: 4.7 MB).
9. **Smaller choices.** `count()` drops a sort stage (an unsorted node); E0001 for a bare `DerivedList`;
   `DerivedStats` is `#[non_exhaustive]` and `full_values` counts observes too; a Rust read runs inside a
   transaction (writes made by a parameter's computed commit after the drain lock is released); debug
   builds panic on duplicate keys in a full value (a commit isolates it); tap ops are cloned after the
   change (a panicking `Clone` leaves the item in the list and the taps stale; documented on `push`,
   `insert`, `update_at`).

## Verification

**After the last merge of `main` (`0aa98a4`, rn-adapters + dev-reload + the upgrade-test fix; bindings and
site regenerated):** `cargo fmt --check`, clippy `-D warnings`, rustdoc `-D warnings` clean; `cargo test
--workspace` 2,689 passed, 0 failed, 12 ignored (`UNDRA_REQUIRE_TOOLCHAINS=1`, tsc on PATH); `undra-signals`
release, Track A release regressions, `sync_alloc` / `commit_alloc` / `derived_alloc` release, the wasm32 build
of `undra-signals`, `bindgen --check --docs` (`0xc5f05c376fde398c`), site `build-all` + `check-links --words`
(342 words): pass. The coordinator asked for the report before the platform half of that run finished, so
the platform suites below are from the merge before it.

**After merging `main` at `e119d4c`:** `cargo fmt --check`, `cargo clippy --workspace --all-targets -D warnings`, `cargo doc --workspace --no-deps`
with `-D warnings`: clean. `cargo test --workspace`: 2,683 passed, 0 failed (`UNDRA_REQUIRE_TOOLCHAINS=1`).
`undra-signals` 382 in debug and in release; Track A's release regressions (`weak_ctx`, `computed_isolation`,
`write_context`) pass; `sync_alloc`, `commit_alloc`, `derived_alloc` pass in release. TypeScript runtime
1,128; Kotlin runtime 612 under kotlinc 2.4.20 and under CI's 2.0.21 (and the contract runner, S19
included, compiles and passes under 2.0.21); Swift runtime 480. `bash contract-tests/run-all.sh`: 57/57.
`undra bindgen -C examples/playground --check --docs`: up to date at `0x933d362fb48d39ac`.
`cargo test -p undra-cli --test schema_docs -- --ignored`: pass. Budgets and stress gates (release) pass,
both ratios hold. Playground web `npm test` 112, `npm run build` OK; the live demo (dev server, browser pane):
Todos add / toggle / filter on the derived `visible`, the 10k list's insert, the stress screen at ~10,500
updates a second with 0 dropped frames, no console errors. Android `undra build --platform android
--release` + `assembleDebug`: the APK carries the new core. `node site/scripts/build-all.mjs` and
`check-links.mjs --words`: 23 pages OK, 342 words. `cargo build -p undra-signals --target
wasm32-unknown-unknown`: OK.

## Size

The playground core grows with this piece (both built here, `undra build`, release): wasm 592.3 KB to 625.7
KB (+33.4 KB, +5.6%; gzip 226.4 to 241.0 KB, +14.6 KB), Android arm64 1,622,256 to 1,709,424 bytes (+87 KB,
+5.4%). Two node instantiations (`visible`, `remaining`), the two trees, the pipeline closures and the
teaching messages. Worth a look by the wasm-size bet (E5): the node's non-generic parts could be factored
out of the monomorphised code.

## Open items (for the integrator)

* Landing page: proposal only, `site/data/bench.json` untouched (card labels do not count toward the 350
  words; prose is at 342). Proposed row: `{"id": "derived-view", "operation": "Filtered view of a
  10,000-row list, one row changed - 158 bytes on the wire, was 353 KB", "value": 392, "unit": "ns",
  "budget": 1, "budgetUnit": "µs", "gate": "signals/derived_10k/update_visible ≤ 2 µs", "source":
  ".../bench/RESULTS.md#5-a-computed-list-over-a-keyed-list-cost-the-list-again-a-derived-list-costs-the-change-adr-039"}`;
  or a harsh card "Derived churn - a 10,000-row list and a sorted view of it, both mirrored; p99 24.6 µs",
  122 k/s, floor 22 k/s.
* Runner samples for the two ratios (and `bench/baselines/apple-m5-pro.toml` has no derived rows; CI records
  its baseline from the base commit, so the rows are absolute-gated until then).
* The React Native contract column (`runtimes/rn`, out of scope) lists its scenario files explicitly and
  does not run S19.
* ADR-039 section 10's follow-ups stand (a derived list as a source, `map_with`, more aggregates, a
  position index by key, paging with E3).
