# Derived keyed lists (E2, ADR-039) — adversarial review

**Date:** 2026-10-02 · **Reviewer:** senior-engineer (adversarial, `docs/AGENT_WORKFLOW.md` section 3) · **Piece:**
`wt/derived-lists` at `643a767` (`main` `0aa98a4` merged) · **Read:** `CLAUDE.md` (R1, R3, R5, R9, R11, R12), ADR-039 with
its implementation notes, `.10x/decisions/architect/derived-keyed-lists.md` (integrator decisions),
`.10x/decisions/sde/derived-lists.md`, ADR-019 (amendment), ADR-027, ADR-031, ADR-034, `docs/SPEC.md` 3.8, 5.5, 5.9,
16.1, and `git diff main...HEAD` in full: `crates/undra-signals/src/derived/*` (index, tap, node, pipeline, slot,
mod), the `signal`, `signal/list`, `store`, `computed` and `deps` integration, every test under
`crates/undra-signals/tests/derived*` and `support/seeded_views.rs`, `crates/undra-macros/src/impl_/store.rs` and its
two trybuild goldens, the meta and bindgen pins, the playground `Todos`, S19 on the three runners, the bench
fixtures, rows and stress scenario · **Fixes:** `518227a`, `f0db513`, and the counts with this record; then `main` merged.

## Verdict

**Sound; merge.** No High, no Medium. The index is right under every op shape I could aim at, including the ones the
first model test did not reach: a second model (four views and a computed over one source, one view parameterised by a
`count()` of that same source, one with both a filter and a sort parameter) driven through transactions of up to 4,396
operations, read part-way or not, so that a tap overflows and operations keep arriving after it did *inside the same
transaction*, and so that the derived ops overflow while the tap does not. It passes 3 x 3,000 cases on fresh seeds
in release and 1,500 in debug, and it fails at once when the tap's overflow is mutated to drop ops instead of going
stale (a check that the model has teeth). The original model passed 40,000 fresh-seed cases in release; the trees'
own proptests (rank, size, sum, parent links and AVL balance after every op) passed on three fresh seeds.

The exact boundaries hold: a parameter walk of 256 ops on top of replayed source ops in the same drain is one patch
(266 and 258 ops), 257 is the full value, for a filter (inserts/removes) and for a sort key (moves only); the tap's
4,096 and the slot's 4,096 are exact. A row that moves, changes and is removed in one transaction is `Move`, `Update`,
`Remove` and applies; the same three ops spread over two commits and concatenated (what ADR-031's mirrors do) apply
too. Duplicate keys never corrupt the view: maintenance does not look at keys, debug builds isolate a full value that
carries one (the slot is held back like a failed computed, then recovers), release sends it as it is.

The platforms agree with the core after every drain on all three: S19 now starts with a scripted prologue whose op
shapes the Rust side asserts (a row's `Move` + `Update` in both sorted views, then its `Remove`; a rebuild's full
values, then patches), every runner replays it change-set by change-set against the core's hashes, the TypeScript
mirror drains each pair as one and checks they merged, and the Kotlin and Swift mirrors get the same shapes in their
`CoalesceTests` (their contract runners cannot reach the mirror's internal enqueue). S19 also runs on the React
Native column now (18 pass + S17 app-tested).

Every claim of the record that was re-runnable was re-run after the final merge (the record's platform half was from
the merge before): numbers below. The landing card (Option A) is applied; prose stays at 342 words.

## Findings

No High. No Medium.

**L1 — S19 never ran on the React Native column (fixed, `f0db513`).** `vitest.contract.config.ts` listed its scenario
files and stopped at S18. S19 is pure JavaScript over the wasm stand-in: it passes there unchanged. Added to the glob;
CI's React Native job writes the recording first (`contract-tests/derived-vectors.sh`); `docs/REACT_NATIVE.md` and
`docs/ONBOARDING.md` say 18 pass (and the package's unit-test count, 39 there, is 60).

**L2 — ADR-031 merging of derived shapes was proved on TypeScript only (fixed, `518227a`).** Kotlin's and Swift's S19
step 9 apply change-set by change-set; only TypeScript went through a mirror. The generic merge model tests of both
runtimes would catch a concatenation bug, but nothing named the derived shapes. Added: the scripted prologue (records
2-6 of the recording, `support/seeded_views.rs:294`), TypeScript draining records 2-3 and 4-5 as one with the applied
counts asserted (three and six), and a `CoalesceTests` case on Kotlin and Swift (`a derived list's shapes merge per
drain`, `testADerivedListsShapesMergePerDrainLikeAppliedOneByOne`): one drain per change-set and one per pair end
equal; a full value and the patches around it apply as the full value once and one merged patch.

**L3 — The over-cap rebuild inside one drain had no test (fixed, `518227a`).** `7e0e147` made the fast path rebuild in
the same drain when a replayed op does not fit; it is defended rather than reachable (the tap invariant rules such an
op out, and every unwinding path sets `in_flight`, which forces the slow path). What *is* reachable, a tap that went
stale part-way through a transaction followed by more writes and a read before the commit, is now tested exactly
(`an_op_that_arrives_after_the_tap_overflowed_is_rebuilt_in_the_drain_that_meets_it`: one rebuild, mid-transaction
reads equal the reference, the commit sends the full value, patches resume) and by the second model's bursts.

**L4 — What a read inside a transaction sees was not stated (fixed, `518227a`).** It sees the writes the transaction
already made (taps record at write time, not at commit), as a `Signal` read does, and the commit still sends them all
as one patch; a read never deadlocks with a write of the same transaction (the drain nests a transaction, holds only
the list's own lock while closures run and the source's read lock only to take ops and a snapshot; inside the
source's own `update` closure it panics with ADR-021 L3). Stated on `DerivedList` and in SPEC 16.1; tested
(`a_read_inside_a_transaction_sees_the_writes_made_before_it_and_the_commit_sends_them_all`, and every `Read` edit of
both models).

**L5 — Eight landing cards would have laid out ragged (fixed, `f0db513`).** `build-numbers.mjs`'s `spans` gave the
first n - 4 cards span 4 for any n >= 7: at eight that is spans 4, 4, 4, 4, 3, 3, 3, 3, which wrap as 12 + 10 + 6
columns (three cards, three, then two with half the row empty). A multiple of four from eight on is now rows of four.

**L6 — README's test counts were stale (fixed, `docs(derived-lists): adversarial review`).** "4,000+ ... Rust 2,168 ·
TypeScript 931 · Kotlin 500 · Swift 425" is now "4,900+ ... Rust 2,700 · TypeScript 1,132 · Kotlin 617 · Swift 512" (483 here, 512 with `main`'s
`swift-fs` merged), in
README and in the landing page's tests card (which carried the same stale numbers).

**I1 — The generated docs say "Computed by the core; read-only.", not "derived" (not changed).** ADR-039 section 7
pins a derived list's declaration as byte-for-byte a computed list's (`crates/undra-bindgen/tests/generators.rs:1207`
asserts it), and the schema can tell them apart (`computed` + `key` exists only for a derived list), so a one-line
`signal_doc` change in `crates/undra-bindgen/src/model.rs:401` ("Derived by the core from a list; read-only, updated
by keyed patches.") would say it without a hash move. It changes the `stores` and `full` goldens, the playground
bindings and three reference pages, and contradicts the ADR's pinned property: the integrator's call. The site's
concepts page ("Derived lists") and SPEC say it plainly.

**I2 — The 256-op limit is per walk, not per patch.** Two parameter changes drained separately within one transaction
(a Rust read between them) can send 2 x 256 walk ops plus the replayed ops in one patch; it is still bounded by the
slot's 4,096 and always applies. ADR-039's wording ("the walk emits a patch of at most 256 ops") holds per walk.

**I3 — Duplicate keys have no typed outcome in release.** By design (ADR-039 section 9: keys are for the platforms'
identity, maintenance ignores them): debug builds isolate the slot with the duplicate message, release sends rows
that share a key and the view stays exact (`duplicate_keys_never_corrupt_the_view`). A UI keyed by that field
mis-animates; the data does not diverge.

**I4 — Debug builds pay O(view) per derived commit.** `node.rs:797` takes a snapshot and `:822-835` materialises the
view and replays every patch on a shadow copy, so a debug core holds a second copy of each observed view and each
commit costs its length. Right for tests; worth remembering when a debug dev loop shows a 100,000-row view.

## The attack, surface by surface

1. **The index under every op shape.** Insert, remove, update, move, replace-all (`replace`, `set`, raw `update`),
   clear; a sort key changed to tie with its neighbour (`Stay` + `Update`, no `Move`, ties by source position); equal
   keys over source moves (a `Move` only when the row crosses an equal-key neighbour; the scripted prologue's last
   step checks the no-`Move` case); filter flips on update (`Insert` / `Remove`); a key change (`Move` + `Update`);
   duplicate keys (I3); a tap over its cap followed by more ops in the same transaction (L3); the 256/257 walk with
   replayed ops ahead of it, for a filter and for a sort parameter; the slot's own overflow without a tap overflow (a
   sort-key burst of 2,051 ops: 4,102 view ops, the full value, no rebuild). The second model
   (`tests/derived_review.rs`) adds a view whose parameter is a `count()` of its own source and a computed over a
   derived list. Clean.
2. **ADR-031 coalescing.** L2. Every merged drain equals the core's hashes on TypeScript; Kotlin and Swift merged
   drains equal one-by-one application for the same shapes. Clean after the fix.
3. **Isolation and lifecycle.** A panicking map: the source, a sibling view and a `count()` are delivered on both
   failing commits, `computed_failed` fires once, a Rust read panics and leaves the list usable, the next successful
   input sends the full value and `computed_recovered` fires once, patches resume
   (`a_panicking_map_poisons_only_its_list_is_reported_once_and_recovers`). Restore: `restore_rebuilds_the_derived_field_from_its_source`
   (macros) rebuilds from the restored source under the same handle and observes in full; SPEC 5.9 keeps derived slots
   out of snapshots. Shutdown mid-walk: a walk blocked half-way on another thread while this thread pushes, updates
   and raw-writes the source, drops the store cell and every other handle and builds a new view of the source; no
   writer waits, the walk finishes on its snapshot (`writers_never_wait_for_a_drain_and_dropping_every_other_handle_mid_walk_is_safe`).
   `Runtime::shutdown` drops stores the same way and never calls into a derived list (read, not tested at the
   runtime level). Clean.
4. **R5 and R12.** L4. No clock, randomness, hashing or thread in `derived/` (searched); ties are source order, the
   trees are AVL, the walk order is source order. Clean.
5. **Hashes and generated code.** The pin `canonical.rs:575` checks `0xd5b8_c3a3_afbd_bc33`, which is `main`'s golden
   for the representative schema (`main:crates/undra-meta/src/canonical.rs:538`), not a value computed by the test.
   `undra bindgen -C examples/playground --check --docs`: up to date at `0xc5f05c376fde398c`; `schema_docs` and
   `schema_retention` (ignored, run): pass. The generated `visible` is `public private(set) var visible: [Todo]`,
   `val visible: StateFlow<List<Todo>>`, `readonly visible: Signal<Todo[]>`, each with the patch case in `apply`; I1
   on the doc line.
6. **Size (ADR-052).** `scripts/wasm-size.sh`: hello-world wasm 102,666 bytes gzipped against the record's 102,722
   (gate 107,858): the index is not linked into a core without derived lists (everything in `derived/` is generic and
   instantiated only by `derive()`; a recorded op on a list with no derived list pays one `OnceLock` load). Runtime
   JS 24,880 (gate 26,000). The playground's +14.6 KB gzip is the record's, not re-measured.
7. **The matrix**, once, after the fixes: below.

## Suite counts (after the fixes, on `f0db513`)

| Suite | Result |
|---|---|
| `cargo fmt --check`, clippy `-D warnings` (host, all targets) and on wasm32 (`undra-ffi`), `cargo doc -D warnings` | clean |
| `cargo test --workspace` (`UNDRA_REQUIRE_TOOLCHAINS=1`, tsc on PATH) | 2,700 passed, 0 failed, 12 ignored (the record's 2,689 plus the 11 tests of `derived_review.rs`) |
| `undra-signals` in release | 393 passed |
| Track A release (`write_context`, `weak_ctx`, `computed_isolation`, `write_checker`) | pass |
| `sync_alloc`, `commit_alloc`, `derived_alloc` (release) | pass |
| wasm32 build of the core crates and `playground-core` | pass |
| `bindgen --check --docs`; `schema_docs` + `schema_retention` (ignored) | up to date `0xc5f05c376fde398c`; pass |
| Budgets (`--test budgets --release`), the nine derived rows and both ratios | pass: p50 `update_visible` 388.7 ns (10k) / 394.2 ns (100k), `toggle_membership` 369.1 ns, `sort_key_change` 569.2 / 597.1 ns, `insert_sorted` 8.29 µs, `param_flip` 221 µs, `rebuild_after_replace` 196 µs, `count_toggle` 373.8 ns, `stress/derived_churn_10k/ops_x1000` 8.78 ms; ratios `derived_sort_scaling` 1.05 (max 4), `derived_vs_keyed_update` 1.42 (max 2.5) |
| Stress gates (`--test stress --release`), `derived_churn_10k/sustained` | pass (12 gates): `derived_churn_10k/sustained` 111.4 k ops/s, p99 26.1 µs, 93.0 bytes/op |
| Swift runtime `swift test` | 483 (482 + the new case), 0 failures; 512 after merging `main` (`swift-fs`) |
| Kotlin runtime, kotlinc 2.4.20 and CI's 2.0.21 | 617 cases, 0 failed, 2 skipped (no native library), both compilers |
| TypeScript runtime `npm test` + typecheck | 1,132 pass; clean |
| `bash contract-tests/run-all.sh` | 57/57 (Kotlin and Swift 19/19 in the full run, TypeScript 19/19 on its re-run after the S19 step 9b fix) |
| React Native: `npm test`, typecheck, `test:contract` | 60 pass; clean; 18 pass + S17 skipped (app-tested) |
| Playground web `npm test` + `npm run build` | 112 pass; built |
| Live demo (dev server, browser pane) | Todos: add three, toggle one, filter Active shows the two open, "2 left"; 10k list: insert middle and top (2.0 ms); stress: 10,002 updates/s, 87 drains/s, 1 apply per 59 received, 0 dropped frames, 12.4 MB heap; no console errors. The pane was hidden for the 10k and stress screens, so those are its text readout, not screenshots |
| Android: `undra build --platform android --release` + `:app:assembleDebug` | BUILD SUCCESSFUL |
| `scripts/wasm-size.sh` | within both gates (above) |
| Site `build-all` + `check-links --words` | pass; 342 words (budget 350) |
| Model runs on fresh seeds | `tests/derived.rs` 3 x 4,000 + 40,000 cases (release); `tests/derived_review.rs` 3 x 3,000 (release) + 1,500 (debug) |

## Open items

* I1, the generated doc line: the integrator's call against ADR-039 section 7.
* Runner samples for the two ratios and `bench/baselines/apple-m5-pro.toml` rows for the derived benches (the
  record's open item stands).
* ADR-039 section 10's follow-ups stand (a derived list as a source, `map_with`, more aggregates, a position index by
  key, paging with E3).
